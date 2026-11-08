//! Session-owned atomic artifacts. A path alone never authorizes replacement or cleanup.
use super::contracts::StageState;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::unix::{
    ffi::OsStrExt,
    fs::MetadataExt,
    io::{AsRawFd, FromRawFd},
};
use std::path::{Path, PathBuf};

/// Bound content/identity facts. Directory sync confirms a particular installation,
/// while applicability answers whether that content still occupies the selected path.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactSnapshot {
    pub selected_path: String,
    pub applicable_path: Option<String>,
    pub file_identity: ArtifactFileIdentity,
    pub sha256: String,
    pub provider_attempts: Option<u64>,
    pub file_sync: StageState,
    pub directory_sync: StageState,
    pub path_applicability: StageState,
}
#[derive(Debug, Clone, Copy, serde::Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactFileIdentity {
    pub device: u64,
    pub inode: u64,
}
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactOutcome {
    pub current: Option<Box<ArtifactSnapshot>>,
    pub last_confirmed: Option<Box<ArtifactSnapshot>>,
}

pub const MAX_ARTIFACT_BYTES: usize = 256 * 1024 * 1024;
#[derive(Debug, Clone, Serialize)]
pub struct RetainedArtifact {
    pub path: String,
    pub ownership: String,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[error("{reason}; committed={committed}; retained={retained:?}")]
pub struct ArtifactFailure {
    pub reason: String,
    pub committed: bool,
    pub outcome: ArtifactOutcome,
    pub retained: Vec<RetainedArtifact>,
}
impl ArtifactFailure {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            committed: false,
            outcome: ArtifactOutcome::default(),
            retained: vec![],
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FileId {
    device: u64,
    inode: u64,
}
impl FileId {
    pub(super) fn evidence(self) -> ArtifactFileIdentity {
        ArtifactFileIdentity {
            device: self.device,
            inode: self.inode,
        }
    }
}
pub(super) fn id(meta: &Metadata) -> std::io::Result<FileId> {
    #[cfg(unix)]
    {
        Ok(FileId {
            device: meta.dev(),
            inode: meta.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic artifact identity unavailable on this platform",
        ))
    }
}
#[derive(Debug)]
struct OwnedFile {
    file: File,
    id: FileId,
    digest: Option<[u8; 32]>,
    provider_attempts: Option<u64>,
    directory_synced: bool,
}
impl OwnedFile {
    fn finish(&mut self) -> std::io::Result<()> {
        self.file.sync_all()?;
        self.digest = Some(file_digest(&mut self.file)?);
        Ok(())
    }
}
fn file_digest(file: &mut File) -> std::io::Result<[u8; 32]> {
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut total = 0usize;
    let mut bytes = [0; 64 * 1024];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count)
            .ok_or_else(|| std::io::Error::other("artifact size overflow"))?;
        if total > MAX_ARTIFACT_BYTES {
            return Err(std::io::Error::other("artifact exceeds size bound"));
        }
        hash.update(&bytes[..count]);
    }
    Ok(hash.finalize().into())
}
#[cfg(test)]
#[derive(Debug, Default)]
pub(super) struct Faults {
    pub prior_cleanup: bool,
    pub directory_sync: bool,
}
#[derive(Debug)]
pub(crate) struct AtomicArtifact {
    path: PathBuf,
    parent: File,
    parent_id: FileId,
    current: Option<OwnedFile>,
    last_confirmed: Option<Box<ArtifactSnapshot>>,
    #[cfg(test)]
    pub(super) faults: Faults,
    retained: Vec<OwnedFile>,
    unknown_retained: Vec<File>,
    byte_limit: usize,
}
impl AtomicArtifact {
    pub(crate) fn new(path: &Path) -> Result<Self, ArtifactFailure> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = File::open(parent)
            .map_err(|_| ArtifactFailure::new("artifact directory unavailable"))?;
        let parent_id = id(&parent
            .metadata()
            .map_err(|_| ArtifactFailure::new("artifact directory identity unavailable"))?)
        .map_err(|_| ArtifactFailure::new("artifact identity unsupported"))?;
        Ok(Self {
            path: path.to_owned(),
            parent,
            parent_id,
            current: None,
            last_confirmed: None,
            #[cfg(test)]
            faults: Faults::default(),
            retained: vec![],
            unknown_retained: vec![],
            byte_limit: MAX_ARTIFACT_BYTES,
        })
    }
    fn snapshot(&self, file: &OwnedFile) -> Option<Box<ArtifactSnapshot>> {
        let digest = file.digest?;
        let applicable = self.directory_current() && self.matches(file, &self.path, true);
        Some(Box::new(ArtifactSnapshot {
            selected_path: self.path.display().to_string(),
            applicable_path: applicable.then(|| self.path.display().to_string()),
            file_identity: ArtifactFileIdentity {
                device: file.id.device,
                inode: file.id.inode,
            },
            sha256: hex::encode(digest),
            provider_attempts: file.provider_attempts,
            file_sync: StageState::Completed {},
            directory_sync: if file.directory_synced {
                StageState::Completed {}
            } else {
                StageState::Unknown {
                    reason: "installation_directory_sync_not_confirmed".into(),
                }
            },
            path_applicability: if applicable {
                StageState::Completed {}
            } else {
                StageState::Unknown {
                    reason: "installed_content_selected_path_applicability_unresolved".into(),
                }
            },
        }))
    }
    pub(crate) fn outcome(&self) -> ArtifactOutcome {
        let current = self.current.as_ref().and_then(|file| self.snapshot(file));
        let last_confirmed = self.last_confirmed.clone().map(|mut snapshot| {
            let applicable = current.as_ref().is_some_and(|current| {
                current.file_identity == snapshot.file_identity
                    && current.sha256 == snapshot.sha256
                    && current.applicable_path.is_some()
            });
            snapshot.applicable_path = applicable.then(|| self.path.display().to_string());
            snapshot.path_applicability = if applicable {
                StageState::Completed {}
            } else {
                StageState::Unknown {
                    reason: "previously_confirmed_content_no_longer_at_selected_path".into(),
                }
            };
            snapshot
        });
        ArtifactOutcome {
            current,
            last_confirmed,
        }
    }
    fn directory_current(&self) -> bool {
        std::fs::metadata(self.path.parent().unwrap_or(Path::new(".")))
            .ok()
            .and_then(|m| id(&m).ok())
            == Some(self.parent_id)
    }
    fn temp_path(&self) -> PathBuf {
        self.path
            .with_extension(format!("tmp-{}", std::process::id()))
    }
    pub(crate) fn write_json<T: Serialize>(&mut self, value: &T) -> Result<(), ArtifactFailure> {
        self.write_json_with_attempts(value, None)
    }
    pub(crate) fn write_json_with_attempts<T: Serialize>(
        &mut self,
        value: &T,
        provider_attempts: Option<u64>,
    ) -> Result<(), ArtifactFailure> {
        self.write_with(provider_attempts, |file| {
            serde_json::to_writer_pretty(&mut *file, value)?;
            file.write_all(b"\n")
        })
    }
    fn write_with(
        &mut self,
        provider_attempts: Option<u64>,
        serialize: impl FnOnce(&mut dyn Write) -> std::io::Result<()>,
    ) -> Result<(), ArtifactFailure> {
        self.write_inner(provider_attempts, serialize)
            .map_err(|mut failure| {
                failure.outcome = self.outcome();
                failure
            })
    }
    fn write_inner(
        &mut self,
        provider_attempts: Option<u64>,
        serialize: impl FnOnce(&mut dyn Write) -> std::io::Result<()>,
    ) -> Result<(), ArtifactFailure> {
        if !self.directory_current() {
            return Err(ArtifactFailure::new(
                "artifact directory path changed; existing files preserved",
            ));
        }
        if let Some(current) = &self.current {
            if !self.matches(current, &self.path, true) {
                return Err(ArtifactFailure::new(
                    "artifact output identity or content changed; caller content preserved",
                ));
            }
        }
        let temp_path = self.temp_path();
        let file = self
            .create_temp(&temp_path)
            .map_err(|cause| ArtifactFailure {
                reason: "artifact temporary creation failed; no temporary ownership acquired"
                    .into(),
                committed: false,
                outcome: ArtifactOutcome::default(),
                retained: vec![RetainedArtifact {
                    path: temp_path.display().to_string(),
                    ownership: "not_acquired".into(),
                    reason: format!(
                        "creation rejected: {:?}; no cleanup attempted",
                        cause.kind()
                    ),
                }],
            })?;
        let identity = match file.metadata().and_then(|m| id(&m)) {
            Ok(id) => id,
            Err(_) => {
                self.unknown_retained.push(file);
                return Err(ArtifactFailure {
                    reason: "created temporary identity unavailable; temporary retained".into(),
                    committed: false,
                    outcome: ArtifactOutcome::default(),
                    retained: vec![RetainedArtifact {
                        path: temp_path.display().to_string(),
                        ownership: "created_handle_identity_unknown".into(),
                        reason: "cleanup authority cannot be established".into(),
                    }],
                });
            }
        };
        let mut temporary = OwnedFile {
            file,
            id: identity,
            digest: None,
            provider_attempts,
            directory_synced: false,
        };
        let write = (|| -> std::io::Result<()> {
            let mut writer = BudgetWriter {
                file: &mut temporary.file,
                written: 0,
                limit: self.byte_limit,
            };
            serialize(&mut writer)?;
            temporary.finish()
        })();
        if write.is_err() {
            return Err(self.fail_temporary(
                temporary,
                &temp_path,
                "artifact serialization or fsync failed",
                false,
            ));
        }
        if !self.directory_current() || !self.matches(&temporary, &temp_path, true) {
            return Err(self.fail_temporary(
                temporary,
                &temp_path,
                "artifact temporary/directory identity changed before commit",
                false,
            ));
        }
        if self.current.is_none() {
            if self.rename(&temp_path, &self.path, false).is_err() {
                return Err(self.fail_temporary(
                    temporary,
                    &temp_path,
                    "exclusive artifact installation failed; requested output preserved",
                    false,
                ));
            }
            self.current = Some(temporary);
        } else {
            // Exchange lets us examine the displaced object before removing it;
            // plain rename would silently destroy a racing caller replacement.
            if self.rename(&temp_path, &self.path, true).is_err() {
                return Err(self.fail_temporary(
                    temporary,
                    &temp_path,
                    "artifact exchange failed; prior complete record preserved",
                    false,
                ));
            }
            let previous = self.current.take().unwrap();
            if !self.matches(&previous, &temp_path, true) {
                if self.matches(&temporary, &self.path, true)
                    && self.rename(&temp_path, &self.path, true).is_ok()
                {
                    self.current = Some(previous);
                    return Err(self.fail_temporary(
                        temporary,
                        &temp_path,
                        "output changed during commit; displaced caller file restored",
                        false,
                    ));
                }
                let mut failure=ArtifactFailure::new("output changed during exchange; rollback authority unavailable; resources retained");
                failure.retained.push(RetainedArtifact {
                    path: temp_path.display().to_string(),
                    ownership: "unresolved_displaced_output".into(),
                    reason: "never delete unowned displaced file".into(),
                });
                failure.retained.push(RetainedArtifact {
                    path: self.path.display().to_string(),
                    ownership: "created_file_handle_retained_path_unknown".into(),
                    reason: "caller changed output during rollback".into(),
                });
                self.retained.push(previous);
                self.retained.push(temporary);
                return Err(failure);
            }
            self.current = Some(temporary);
            if !self.matches(self.current.as_ref().unwrap(), &self.path, true) {
                let mut failure = ArtifactFailure::new(
                    "installed path changed before prior record cleanup; prior record retained",
                );
                failure.committed = true;
                failure.retained.push(RetainedArtifact {
                    path: temp_path.display().to_string(),
                    ownership: "owned_prior_record_retained".into(),
                    reason: "installed path applicability unresolved".into(),
                });
                self.retained.push(previous);
                return Err(failure);
            }
            if let Err(reason) = self.cleanup_previous(&previous, &temp_path) {
                let mut failure = ArtifactFailure::new(reason);
                failure.committed = true;
                failure.retained.push(RetainedArtifact {
                    path: temp_path.display().to_string(),
                    ownership: "owned_file_handle_retained".into(),
                    reason: "prior record cleanup failed".into(),
                });
                self.retained.push(previous);
                return Err(failure);
            }
        }
        if !self.directory_current()
            || !self.matches(self.current.as_ref().unwrap(), &self.path, true)
        {
            let mut failure = ArtifactFailure::new(
                "artifact path changed after commit; committed path applicability unknown",
            );
            failure.committed = true;
            return Err(failure);
        }
        self.sync_directory().map_err(|_| ArtifactFailure {
            reason: "complete artifact installed; directory durability unconfirmed".into(),
            committed: true,
            outcome: ArtifactOutcome::default(),
            retained: vec![],
        })?;
        self.current.as_mut().unwrap().directory_synced = true;
        self.last_confirmed = self.current.as_ref().and_then(|file| self.snapshot(file));
        Ok(())
    }
    fn cleanup_previous(&mut self, previous: &OwnedFile, path: &Path) -> Result<(), String> {
        #[cfg(test)]
        if std::mem::take(&mut self.faults.prior_cleanup) {
            return Err("injected prior cleanup failure".into());
        }
        self.remove_owned(previous, path)
    }
    fn sync_directory(&mut self) -> std::io::Result<()> {
        #[cfg(test)]
        if std::mem::take(&mut self.faults.directory_sync) {
            return Err(std::io::Error::other("injected directory sync failure"));
        }
        self.parent.sync_all()
    }

    fn fail_temporary(
        &mut self,
        temporary: OwnedFile,
        path: &Path,
        reason: &str,
        committed: bool,
    ) -> ArtifactFailure {
        let mut failure = ArtifactFailure {
            reason: reason.into(),
            committed,
            outcome: ArtifactOutcome::default(),
            retained: vec![],
        };
        if let Err(cleanup) = self.remove_owned(&temporary, path) {
            failure.retained.push(RetainedArtifact {
                path: path.display().to_string(),
                ownership: "owned_file_handle_retained_path_may_have_changed".into(),
                reason: cleanup,
            });
            self.retained.push(temporary);
        }
        failure
    }
    fn matches(&self, owned: &OwnedFile, path: &Path, content: bool) -> bool {
        #[cfg(unix)]
        {
            let Some(name) = path.file_name() else {
                return false;
            };
            let Ok(name) = CString::new(name.as_bytes()) else {
                return false;
            };
            let fd = unsafe {
                libc::openat(
                    self.parent.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return false;
            }
            let mut file = unsafe { File::from_raw_fd(fd) };
            let Ok(meta) = file.metadata() else {
                return false;
            };
            if !meta.is_file() || id(&meta).ok() != Some(owned.id) {
                return false;
            }
            if !content {
                return true;
            }
            let Some(expected) = owned.digest else {
                return false;
            };
            file_digest(&mut file).ok() == Some(expected)
        }
        #[cfg(not(unix))]
        {
            let _ = (owned, path, content);
            false
        }
    }
    fn remove_owned(&self, file: &OwnedFile, path: &Path) -> Result<(), String> {
        if !self.matches(file, path, file.digest.is_some()) {
            return Err("cleanup identity unresolved; path preserved".into());
        }
        #[cfg(unix)]
        {
            let name = CString::new(
                path.file_name()
                    .ok_or("cleanup filename unavailable")?
                    .as_bytes(),
            )
            .map_err(|_| "cleanup filename invalid")?;
            // The anchored directory handle prevents a replaced parent path from
            // redirecting cleanup. File identity was established by create_new.
            if unsafe { libc::unlinkat(self.parent.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                return Err("owned temporary unlink failed".into());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err("identity-safe cleanup unsupported".into())
        }
    }
    fn create_temp(&self, path: &Path) -> std::io::Result<File> {
        #[cfg(unix)]
        {
            let name = CString::new(
                path.file_name()
                    .ok_or_else(|| std::io::Error::other("temporary filename missing"))?
                    .as_bytes(),
            )?;
            let fd = unsafe {
                libc::openat(
                    self.parent.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDWR
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_CLOEXEC
                        | libc::O_NOFOLLOW,
                    0o600,
                )
            };
            if fd < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(unsafe { File::from_raw_fd(fd) })
            }
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "atomic artifacts unsupported",
            ))
        }
    }
    fn rename(&self, source: &Path, target: &Path, exchange: bool) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            let from = CString::new(
                source
                    .file_name()
                    .ok_or_else(|| std::io::Error::other("source filename missing"))?
                    .as_bytes(),
            )?;
            let to = CString::new(
                target
                    .file_name()
                    .ok_or_else(|| std::io::Error::other("target filename missing"))?
                    .as_bytes(),
            )?;
            #[cfg(target_os = "macos")]
            let result = unsafe {
                libc::renameatx_np(
                    self.parent.as_raw_fd(),
                    from.as_ptr(),
                    self.parent.as_raw_fd(),
                    to.as_ptr(),
                    if exchange {
                        libc::RENAME_SWAP
                    } else {
                        libc::RENAME_EXCL
                    },
                )
            };
            #[cfg(target_os = "linux")]
            let result = unsafe {
                libc::renameat2(
                    self.parent.as_raw_fd(),
                    from.as_ptr(),
                    self.parent.as_raw_fd(),
                    to.as_ptr(),
                    if exchange {
                        libc::RENAME_EXCHANGE
                    } else {
                        libc::RENAME_NOREPLACE
                    },
                )
            };
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                let _ = (from, to, exchange);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "atomic artifact exchange unsupported",
                ));
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            if result != 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (source, target, exchange);
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "atomic artifacts unsupported",
            ))
        }
    }
}
struct BudgetWriter<'a> {
    file: &'a mut File,
    written: usize,
    limit: usize,
}
impl Write for BudgetWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self
            .written
            .checked_add(bytes.len())
            .is_none_or(|n| n > self.limit)
        {
            return Err(std::io::Error::other("external artifact exceeds 256 MiB"));
        }
        let count = self.file.write(bytes)?;
        self.written += count;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
#[derive(Debug, Default)]
pub(crate) struct ArtifactStore {
    files: BTreeMap<PathBuf, AtomicArtifact>,
}
impl ArtifactStore {
    pub(crate) fn write_json<T: Serialize>(
        &mut self,
        path: &Path,
        value: &T,
    ) -> Result<(), ArtifactFailure> {
        if !self.files.contains_key(path) {
            self.files
                .insert(path.to_owned(), AtomicArtifact::new(path)?);
        }
        self.files.get_mut(path).unwrap().write_json(value)
    }
}

// Shared exclusive directory resource: no model/marker semantics.
#[cfg(unix)]
pub(super) fn create_directory(parent: &File, name: &std::ffi::OsStr) -> std::io::Result<File> {
    use std::os::unix::{
        ffi::OsStrExt,
        io::{AsRawFd, FromRawFd},
    };
    let name = std::ffi::CString::new(name.as_bytes())?;
    if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
#[cfg(not(unix))]
pub(super) fn create_directory(_: &File, _: &std::ffi::OsStr) -> std::io::Result<File> {
    Err(std::io::Error::other(
        "local workspaces require Unix directory ownership",
    ))
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::FixtureDirectory as Directory;
    use super::*;
    #[test]
    fn colliding_temporary_is_never_owned_or_deleted_and_leaves_no_empty_output() {
        let d = Directory::new();
        let out = d.0.join("run.json");
        let mut owner = AtomicArtifact::new(&out).unwrap();
        let temp = owner.temp_path();
        std::fs::write(&temp, b"foreign producer sentinel").unwrap();
        let failure = owner
            .write_json(&serde_json::json!({"state":"initial"}))
            .unwrap_err();
        assert_eq!(std::fs::read(&temp).unwrap(), b"foreign producer sentinel");
        assert!(!out.exists());
        assert!(!failure.committed);
        assert_eq!(failure.retained[0].ownership, "not_acquired");
    }
    #[test]
    fn exclusive_initial_install_preserves_preexisting_requested_output() {
        let d = Directory::new();
        let out = d.0.join("run.json");
        std::fs::write(&out, b"caller report").unwrap();
        let mut owner = AtomicArtifact::new(&out).unwrap();
        assert!(owner
            .write_json(&serde_json::json!({"state":"new"}))
            .is_err());
        assert_eq!(std::fs::read(&out).unwrap(), b"caller report");
        assert!(!owner.temp_path().exists());
    }
    #[test]
    fn output_replacement_and_in_place_edit_are_preserved() {
        let d = Directory::new();
        let out = d.0.join("run.json");
        let mut owner = AtomicArtifact::new(&out).unwrap();
        owner
            .write_json(&serde_json::json!({"state":"one"}))
            .unwrap();
        std::fs::write(&out, b"caller changed owned file").unwrap();
        assert!(owner
            .write_json(&serde_json::json!({"state":"two"}))
            .is_err());
        assert_eq!(std::fs::read(&out).unwrap(), b"caller changed owned file");
        std::fs::rename(&out, d.0.join("saved-original.json")).unwrap();
        std::fs::write(&out, b"new caller identity").unwrap();
        assert!(owner
            .write_json(&serde_json::json!({"state":"three"}))
            .is_err());
        assert_eq!(std::fs::read(&out).unwrap(), b"new caller identity");
    }
    #[test]
    fn serialization_budget_preserves_previous_complete_record_and_removes_only_created_temp() {
        let d = Directory::new();
        let out = d.0.join("run.json");
        let mut owner = AtomicArtifact::new(&out).unwrap();
        owner
            .write_json(&serde_json::json!({"state":"complete"}))
            .unwrap();
        let previous = std::fs::read(&out).unwrap();
        owner.byte_limit = 8;
        let failure = owner
            .write_json(&serde_json::json!({"state":"long new record"}))
            .unwrap_err();
        assert!(!failure.committed);
        assert_eq!(std::fs::read(&out).unwrap(), previous);
        assert!(!owner.temp_path().exists());
        assert!(failure.retained.is_empty());
    }
    #[test]
    fn failed_serialization_after_temp_identity_replacement_retains_ownership_without_deleting_foreign_path(
    ) {
        struct ReplaceDuringSerialize {
            temp: PathBuf,
            saved: PathBuf,
        }
        impl serde::Serialize for ReplaceDuringSerialize {
            fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
                std::fs::rename(&self.temp, &self.saved).unwrap();
                std::fs::write(&self.temp, b"foreign replacement").unwrap();
                Err(serde::ser::Error::custom("controlled serializer failure"))
            }
        }
        let d = Directory::new();
        let out = d.0.join("run.json");
        let mut owner = AtomicArtifact::new(&out).unwrap();
        owner
            .write_json(&serde_json::json!({"state":"complete"}))
            .unwrap();
        let previous = std::fs::read(&out).unwrap();
        let temp = owner.temp_path();
        let failure = owner
            .write_json(&ReplaceDuringSerialize {
                temp: temp.clone(),
                saved: d.0.join("created-temp-moved.json"),
            })
            .unwrap_err();
        assert_eq!(std::fs::read(&temp).unwrap(), b"foreign replacement");
        assert_eq!(std::fs::read(&out).unwrap(), previous);
        assert_eq!(owner.retained.len(), 1);
        assert!(!failure.retained.is_empty());
    }
    #[test]
    fn directory_path_replacement_cannot_redirect_owned_output() {
        let mut d = Directory::new();
        let original = d.0.clone();
        let out = original.join("run.json");
        let mut owner = AtomicArtifact::new(&out).unwrap();
        owner
            .write_json(&serde_json::json!({"state":"complete"}))
            .unwrap();
        let saved = original.with_extension("moved");
        std::fs::rename(&original, &saved).unwrap();
        d.relocate(&saved);
        let mut replacement = Directory::new();
        std::fs::rename(&replacement.0, &original).unwrap();
        replacement.relocate(&original);
        std::fs::write(&out, b"foreign new directory").unwrap();
        assert!(owner
            .write_json(&serde_json::json!({"state":"new"}))
            .is_err());
        assert_eq!(std::fs::read(&out).unwrap(), b"foreign new directory");
        assert!(saved.join("run.json").exists());
    }
}
