//! Pinned, VoiceFlow-managed text GGUF downloads for local cleanup.
//!
//! A model is usable only after its installed manifest and GGUF digest have
//! both been checked. A small durable receipt binds that digest to the file's
//! filesystem identity so settings refreshes do not re-hash multi-gigabyte
//! weights on every status poll.

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

pub const QWEN_35_2B_ID: &str = "qwen3.5-2b";
pub const QWEN_35_4B_ID: &str = "qwen3.5-4b";
pub const GEMMA_4_E4B_IT_ID: &str = "gemma-4-e4b-it";
pub const DOWNLOAD_EVENT: &str = "local-cleanup-model://status";

/// New local cleanup downloads are currently curated as MLX models only.
/// Keep installed GGUFs in the catalog for verification and file management.
pub const USER_AVAILABLE_MODEL_IDS: &[&str] = &[];

pub fn is_user_available_model(id: &str) -> bool {
    USER_AVAILABLE_MODEL_IDS.contains(&id)
}

const INSTALLED_MANIFEST: &str = ".voiceflow-gguf-model.json";
const VERIFICATION_RECEIPT: &str = ".voiceflow-gguf-verification.json";
const STATUS_INTERVAL: Duration = Duration::from_millis(250);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const DOWNLOAD_USER_AGENT: &str = "VoiceFlow/0.1";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelManifest {
    pub id: &'static str,
    pub label: &'static str,
    pub repository: &'static str,
    pub revision: &'static str,
    pub filename: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub license: &'static str,
    pub language: &'static str,
}

pub const MODELS: &[ModelManifest] = &[
    ModelManifest {
        id: QWEN_35_2B_ID,
        label: "Qwen3.5 2B · Q4_K_M",
        repository: "lmstudio-community/Qwen3.5-2B-GGUF",
        revision: "bb84e11355a036e28f080c7793fa6d22b7c4e344",
        filename: "Qwen3.5-2B-Q4_K_M.gguf",
        bytes: 1_270_808_032,
        sha256: "0bfe35afc9f05b7fac3fa04925e051ac7939a42a8a17ea11afc99701bea826cc",
        license: "Apache-2.0",
        language: "Multilingual; product coverage not yet validated",
    },
    ModelManifest {
        id: QWEN_35_4B_ID,
        label: "Qwen3.5 4B · Q4_K_M",
        repository: "lmstudio-community/Qwen3.5-4B-GGUF",
        revision: "f9f88ac3e234be915e23811a6d28ea287bdb927e",
        filename: "Qwen3.5-4B-Q4_K_M.gguf",
        bytes: 2_707_513_696,
        sha256: "25082a7dd3776cc3c741c6347d3bd04523f05796607b3fbc32fa3a25dfa1418c",
        license: "Apache-2.0",
        language: "Multilingual; product coverage not yet validated",
    },
    ModelManifest {
        id: GEMMA_4_E4B_IT_ID,
        label: "Gemma 4 E4B IT · Q4_0",
        repository: "ggml-org/gemma-4-E4B-it-GGUF",
        revision: "27cd6836f8e2a6e5e6b57ea0e77fa72490706985",
        filename: "gemma-4-E4B-it-Q4_0.gguf",
        bytes: 4_590_807_392,
        sha256: "a555b900214b477d8880e7832e0b8925e139b0159640036b09fe472b6f2097f2",
        license: "Apache-2.0",
        language: "Multilingual; product coverage not yet validated",
    },
];

pub fn model(id: &str) -> Option<&'static ModelManifest> {
    MODELS.iter().find(|candidate| candidate.id == id)
}

pub fn is_supported_model(id: &str) -> bool {
    model(id).is_some()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelStatus {
    pub id: String,
    pub label: String,
    pub repository: String,
    pub revision: String,
    pub filename: String,
    pub bytes: u64,
    pub sha256: Option<String>,
    pub license: String,
    pub language: String,
    pub state: String,
    pub operation: Option<String>,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub error: Option<String>,
    pub last_operation: Option<String>,
    pub last_operation_error: Option<String>,
    pub managed: bool,
    pub partial_managed: bool,
    pub verified: bool,
    pub download_available: bool,
    pub downloadable: bool,
    pub selectable: bool,
    pub compatible: bool,
    pub compatibility_error: Option<String>,
}

impl ModelStatus {
    fn from_manifest(
        manifest: &ModelManifest,
        compatible: bool,
        compatibility_error: Option<&str>,
    ) -> Self {
        Self {
            id: manifest.id.to_owned(),
            label: manifest.label.to_owned(),
            repository: manifest.repository.to_owned(),
            revision: manifest.revision.to_owned(),
            filename: manifest.filename.to_owned(),
            bytes: manifest.bytes,
            sha256: Some(manifest.sha256.to_owned()),
            license: manifest.license.to_owned(),
            language: manifest.language.to_owned(),
            state: "missing".to_owned(),
            operation: None,
            bytes_done: 0,
            bytes_total: manifest.bytes,
            error: None,
            last_operation: None,
            last_operation_error: None,
            managed: false,
            partial_managed: false,
            verified: false,
            download_available: is_user_available_model(manifest.id),
            downloadable: false,
            selectable: false,
            compatible,
            compatibility_error: compatibility_error.map(str::to_owned),
        }
    }
}

#[derive(Clone, Default)]
pub struct Manager {
    inner: Arc<Mutex<ManagerInner>>,
}

#[derive(Default)]
struct ManagerInner {
    next_generation: u64,
    jobs: HashMap<String, ActiveJob>,
    last_operations: HashMap<String, LastOperation>,
    leases: HashMap<String, usize>,
    deleting: HashSet<String>,
}

struct ActiveJob {
    generation: u64,
    cancel: CancellationToken,
    bytes_done: Arc<AtomicU64>,
}

#[derive(Clone)]
struct LastOperation {
    operation: String,
    error: Option<String>,
}

impl Manager {
    fn lock(&self) -> std::sync::MutexGuard<'_, ManagerInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn status(
        &self,
        models_root: &Path,
        id: &str,
        compatible: bool,
        compatibility_error: Option<&str>,
    ) -> ModelStatus {
        let Some(manifest) = model(id) else {
            return invalid_status(id);
        };
        let (job, last_operation, deleting) = {
            let inner = self.lock();
            (
                inner.jobs.get(id).map(|job| {
                    (
                        job.bytes_done.load(Ordering::Acquire),
                        job.cancel.is_cancelled(),
                    )
                }),
                inner.last_operations.get(id).cloned(),
                inner.deleting.contains(id),
            )
        };
        let mut status = ModelStatus::from_manifest(manifest, compatible, compatibility_error);
        if let Some(last) = last_operation {
            status.last_operation = Some(last.operation);
            status.last_operation_error = last.error;
        }
        if deleting {
            status.operation = Some("delete".to_owned());
            status.state = "deleting".to_owned();
            return status;
        }
        if let Some((bytes_done, cancelled)) = job {
            status.operation = Some("download".to_owned());
            status.state = if cancelled {
                "canceling"
            } else {
                "downloading"
            }
            .to_owned();
            status.bytes_done = bytes_done.min(manifest.bytes);
            return status;
        }

        let installed = model_directory(models_root, id);
        let partial = partial_directory(models_root, id);
        status.managed = path_entry_exists(&installed);
        status.partial_managed = path_entry_exists(&partial);
        if status.partial_managed {
            match fs::symlink_metadata(&partial) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                    status.state = "corrupt".to_owned();
                    status.error = Some("download staging path is not a regular directory".into());
                    return status;
                }
                Ok(_) => {
                    status.bytes_done = file_len_if_regular(&partial.join(manifest.filename))
                        .or_else(|| {
                            file_len_if_regular(
                                &partial.join(format!("{}.download", manifest.filename)),
                            )
                        })
                        .unwrap_or(0)
                        .min(manifest.bytes);
                }
                Err(error) => {
                    status.state = "corrupt".to_owned();
                    status.error = Some(error.to_string());
                    return status;
                }
            }
        }
        match verify_installed_model(models_root, manifest) {
            Ok(Some(_)) => {
                status.state = "installed".to_owned();
                status.bytes_done = manifest.bytes;
                status.managed = true;
                status.verified = true;
            }
            Ok(None) if status.managed => {
                status.state = "corrupt".to_owned();
                status.error =
                    Some("The stored model did not match its pinned manifest or SHA-256.".into());
            }
            Ok(None) => {
                status.state = "missing".to_owned();
            }
            Err(error) => {
                status.state = "corrupt".to_owned();
                status.error = Some(error);
            }
        }
        status
    }

    pub fn cancel(&self, id: &str) -> Result<bool, String> {
        if model(id).is_none() {
            return Err("unsupported local cleanup model".into());
        }
        let mut inner = self.lock();
        let Some(job) = inner.jobs.get_mut(id) else {
            return Ok(false);
        };
        job.cancel.cancel();
        Ok(true)
    }

    pub fn cancel_all_downloads(&self) {
        let inner = self.lock();
        for job in inner.jobs.values() {
            job.cancel.cancel();
        }
    }

    pub async fn shutdown_downloads(&self) -> bool {
        self.cancel_all_downloads();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
        loop {
            if self.lock().jobs.is_empty() {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub fn start_background_download<E>(
        &self,
        models_root: PathBuf,
        id: String,
        compatible: bool,
        compatibility_error: Option<String>,
        emit: E,
    ) -> Result<(), String>
    where
        E: Fn(ModelStatus) + Send + Sync + 'static,
    {
        let manifest = model(&id).ok_or_else(|| "unsupported local cleanup model".to_owned())?;
        if !is_user_available_model(manifest.id) {
            return Err("this local cleanup model is not available for new downloads".into());
        }
        if !compatible {
            return Err("the bundled local cleanup runtime is unavailable on this Mac".into());
        }
        match verify_installed_model(&models_root, manifest) {
            Ok(Some(_)) => return Err("model is already downloaded and verified".into()),
            Ok(None) => {}
            Err(_) => {}
        }
        let (generation, cancel, bytes_done) = {
            let mut inner = self.lock();
            if inner.deleting.contains(&id) {
                return Err("model deletion is in progress".into());
            }
            if inner.jobs.contains_key(&id) {
                return Err("model download is already in progress".into());
            }
            inner.next_generation = inner.next_generation.wrapping_add(1).max(1);
            let generation = inner.next_generation;
            let cancel = CancellationToken::new();
            let bytes_done = Arc::new(AtomicU64::new(0));
            inner.jobs.insert(
                id.clone(),
                ActiveJob {
                    generation,
                    cancel: cancel.clone(),
                    bytes_done: Arc::clone(&bytes_done),
                },
            );
            (generation, cancel, bytes_done)
        };
        let manager = self.clone();
        let emit = Arc::new(emit);
        let status_emit = Arc::clone(&emit);
        let completion_emit = Arc::clone(&emit);
        let status_manager = self.clone();
        let status_root = models_root.clone();
        let status_id = id.clone();
        let poll_compatibility_error = compatibility_error.clone();
        let completion_compatibility_error = compatibility_error.clone();
        let stop_status = CancellationToken::new();
        let stop_status_task = stop_status.clone();
        let cancel_status_task = cancel.clone();
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(STATUS_INTERVAL);
            loop {
                tokio::select! {
                    _ = cancel_status_task.cancelled() => break,
                    _ = stop_status_task.cancelled() => break,
                    _ = interval.tick() => {
                        let status = status_manager.status(
                            &status_root,
                            &status_id,
                            compatible,
                            poll_compatibility_error.as_deref(),
                        );
                        status_emit(status);
                    }
                }
            }
        });
        let task_cancel = cancel.clone();
        let completion_root = models_root.clone();
        let completion_id = id.clone();
        tauri::async_runtime::spawn(async move {
            let result = download_model(
                &completion_root,
                manifest,
                Arc::clone(&bytes_done),
                task_cancel,
            )
            .await;
            manager.finish_download(&completion_id, generation, result.err());
            stop_status.cancel();
            let status = manager.status(
                &completion_root,
                &completion_id,
                compatible,
                completion_compatibility_error.as_deref(),
            );
            completion_emit(status);
        });
        // Send a status immediately so the UI can display the transfer state.
        let status = self.status(
            &models_root,
            &id,
            compatible,
            compatibility_error.as_deref(),
        );
        (emit)(status);
        Ok(())
    }

    fn finish_download(&self, id: &str, generation: u64, error: Option<String>) {
        let mut inner = self.lock();
        if !inner
            .jobs
            .get(id)
            .is_some_and(|job| job.generation == generation)
        {
            return;
        }
        inner.jobs.remove(id);
        inner.last_operations.insert(
            id.to_owned(),
            LastOperation {
                operation: "download".to_owned(),
                error: error.map(|message| sanitize_error(&message)),
            },
        );
        drop(inner);
    }

    pub fn resolve_ready(&self, models_root: &Path, id: &str) -> Result<ResolvedModel, String> {
        let manifest = model(id).ok_or_else(|| "unsupported local cleanup model".to_owned())?;
        self.resolve_ready_with_manifest(models_root, manifest)
    }

    fn resolve_ready_with_manifest(
        &self,
        models_root: &Path,
        manifest: &ModelManifest,
    ) -> Result<ResolvedModel, String> {
        let id = manifest.id;
        {
            let inner = self.lock();
            if inner.deleting.contains(id) {
                return Err("selected local cleanup model is being deleted".into());
            }
        }
        let directory = verify_installed_model(models_root, manifest)?
            .ok_or_else(|| "selected local cleanup model is missing or not verified".to_owned())?;
        let mut inner = self.lock();
        if inner.deleting.contains(id) {
            return Err("selected local cleanup model is being deleted".into());
        }
        *inner.leases.entry(id.to_owned()).or_default() += 1;
        Ok(ResolvedModel {
            id: manifest.id,
            revision: manifest.revision,
            filename: manifest.filename,
            directory,
            _lease: Arc::new(ModelLease {
                manager: self.clone(),
                id: id.to_owned(),
            }),
        })
    }

    pub async fn delete_with_unload<F, Fut>(
        &self,
        models_root: PathBuf,
        id: &str,
        unload: F,
    ) -> Result<(), String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<(), String>>,
    {
        if model(id).is_none() {
            return Err("unsupported local cleanup model".into());
        }
        {
            let mut inner = self.lock();
            if inner.jobs.contains_key(id) {
                return Err("cancel the model download before deleting it".into());
            }
            if !inner.deleting.insert(id.to_owned()) {
                return Err("model deletion is already in progress".into());
            }
        }
        let _deleting = DeletingGuard {
            manager: self.clone(),
            id: id.to_owned(),
        };
        unload().await?;
        {
            let inner = self.lock();
            if inner.leases.get(id).copied().unwrap_or_default() != 0 {
                return Err("the local cleanup model is still in use".into());
            }
        }
        let root = models_root.clone();
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            if !checked_store_root(&root)? {
                return Ok::<_, String>(());
            }
            let installed = model_directory(&root, &id);
            let partial = partial_directory(&root, &id);
            remove_managed_entry(&installed)?;
            remove_managed_entry(&partial)?;
            Ok::<_, String>(())
        })
        .await
        .map_err(|error| error.to_string())??;
        let id = _deleting.id.clone();
        self.lock().last_operations.insert(
            id,
            LastOperation {
                operation: "delete".to_owned(),
                error: None,
            },
        );
        Ok(())
    }
}

struct DeletingGuard {
    manager: Manager,
    id: String,
}

impl Drop for DeletingGuard {
    fn drop(&mut self) {
        let mut inner = self.manager.lock();
        inner.deleting.remove(&self.id);
        drop(inner);
    }
}

#[derive(Clone)]
pub struct ResolvedModel {
    id: &'static str,
    revision: &'static str,
    filename: &'static str,
    directory: PathBuf,
    _lease: Arc<ModelLease>,
}

impl ResolvedModel {
    pub fn id(&self) -> &'static str {
        self.id
    }
    pub fn revision(&self) -> &'static str {
        self.revision
    }
    pub fn model_path(&self) -> PathBuf {
        self.directory.join(self.filename)
    }
}

struct ModelLease {
    manager: Manager,
    id: String,
}

impl Drop for ModelLease {
    fn drop(&mut self) {
        let mut inner = self.manager.lock();
        if let Some(count) = inner.leases.get_mut(&self.id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                inner.leases.remove(&self.id);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct InstalledManifest {
    schema_version: u32,
    id: String,
    repository: String,
    revision: String,
    filename: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FileStamp {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    bytes: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VerificationReceipt {
    schema_version: u32,
    id: String,
    revision: String,
    filename: String,
    bytes: u64,
    sha256: String,
    stamp: FileStamp,
}

fn invalid_status(id: &str) -> ModelStatus {
    ModelStatus {
        id: id.to_owned(),
        label: id.to_owned(),
        repository: String::new(),
        revision: String::new(),
        filename: String::new(),
        bytes: 0,
        sha256: None,
        license: String::new(),
        language: String::new(),
        state: "unsupported".to_owned(),
        operation: None,
        bytes_done: 0,
        bytes_total: 0,
        error: Some("unsupported local cleanup model".into()),
        last_operation: None,
        last_operation_error: None,
        managed: false,
        partial_managed: false,
        verified: false,
        download_available: false,
        downloadable: false,
        selectable: false,
        compatible: false,
        compatibility_error: Some("unsupported_model".into()),
    }
}

fn model_directory(models_root: &Path, id: &str) -> PathBuf {
    models_root.join(id)
}

fn partial_directory(models_root: &Path, id: &str) -> PathBuf {
    models_root.join(format!(".{id}.partial"))
}

fn path_entry_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn file_len_if_regular(path: &Path) -> Option<u64> {
    let metadata = fs::symlink_metadata(path).ok()?;
    (!metadata.file_type().is_symlink() && metadata.is_file()).then_some(metadata.len())
}

fn ensure_private_dir(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err("local model directory is not a regular directory".into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or_else(|| "invalid local model directory".to_owned())?;
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            match fs::create_dir(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.to_string()),
            }
            let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err("local model directory is not a regular directory".into());
            }
        }
        Err(error) => return Err(error.to_string()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn restrict_file_mode(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
    }
    let _ = path;
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "invalid local model metadata path".to_owned())?;
    ensure_private_dir(parent)?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(".metadata-{}-{sequence}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| error.to_string())?;
        restrict_file_mode(&temp)?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        fs::rename(&temp, path).map_err(|error| error.to_string())?;
        restrict_file_mode(path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn expected_installed_manifest(manifest: &ModelManifest) -> InstalledManifest {
    InstalledManifest {
        schema_version: 1,
        id: manifest.id.to_owned(),
        repository: manifest.repository.to_owned(),
        revision: manifest.revision.to_owned(),
        filename: manifest.filename.to_owned(),
        bytes: manifest.bytes,
        sha256: manifest.sha256.to_owned(),
    }
}

fn verification_receipt_path(directory: &Path) -> PathBuf {
    directory.join(VERIFICATION_RECEIPT)
}

fn file_stamp(metadata: &fs::Metadata) -> Result<FileStamp, String> {
    let modified = metadata.modified().map_err(|error| error.to_string())?;
    let modified = modified
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?;
    #[cfg(unix)]
    let (device, inode, changed_seconds, changed_nanoseconds) = {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
    };
    #[cfg(not(unix))]
    let (changed_seconds, changed_nanoseconds) = (0, 0);
    Ok(FileStamp {
        #[cfg(unix)]
        device,
        #[cfg(unix)]
        inode,
        bytes: metadata.len(),
        modified_seconds: modified.as_secs() as i64,
        modified_nanoseconds: modified.subsec_nanos() as i64,
        changed_seconds,
        changed_nanoseconds,
    })
}

fn read_file_stamp(path: &Path) -> Result<Option<FileStamp>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok(None);
    }
    file_stamp(&metadata).map(Some)
}

fn write_verification_receipt(
    directory: &Path,
    manifest: &ModelManifest,
    stamp: FileStamp,
) -> Result<(), String> {
    let receipt = VerificationReceipt {
        schema_version: 1,
        id: manifest.id.to_owned(),
        revision: manifest.revision.to_owned(),
        filename: manifest.filename.to_owned(),
        bytes: manifest.bytes,
        sha256: manifest.sha256.to_owned(),
        stamp,
    };
    let bytes = serde_json::to_vec(&receipt).map_err(|error| error.to_string())?;
    atomic_write(&verification_receipt_path(directory), &bytes)
}

fn receipt_matches(directory: &Path, manifest: &ModelManifest, stamp: &FileStamp) -> bool {
    let path = verification_receipt_path(directory);
    let Ok(metadata) = fs::symlink_metadata(&path) else {
        return false;
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 16 * 1024 {
        return false;
    }
    let Ok(bytes) = fs::read(&path) else {
        return false;
    };
    let Ok(receipt) = serde_json::from_slice::<VerificationReceipt>(&bytes) else {
        return false;
    };
    receipt.schema_version == 1
        && receipt.id == manifest.id
        && receipt.revision == manifest.revision
        && receipt.filename == manifest.filename
        && receipt.bytes == manifest.bytes
        && receipt.sha256 == manifest.sha256
        && receipt.stamp == *stamp
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn verify_installed_model(
    models_root: &Path,
    manifest: &ModelManifest,
) -> Result<Option<PathBuf>, String> {
    if !checked_store_root(models_root)? {
        return Ok(None);
    }
    let directory = model_directory(models_root, manifest.id);
    let directory_metadata = match fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if directory_metadata.file_type().is_symlink() || !directory_metadata.is_dir() {
        return Err("managed model directory is not a regular directory".into());
    }
    let manifest_path = directory.join(INSTALLED_MANIFEST);
    let manifest_metadata = match fs::symlink_metadata(&manifest_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if manifest_metadata.file_type().is_symlink()
        || !manifest_metadata.is_file()
        || manifest_metadata.len() > 16 * 1024
    {
        return Err("managed model manifest is invalid".into());
    }
    let installed_bytes = fs::read(&manifest_path).map_err(|error| error.to_string())?;
    let installed = serde_json::from_slice::<InstalledManifest>(&installed_bytes)
        .map_err(|_| "managed model manifest is invalid".to_owned())?;
    if installed.schema_version != 1 || installed != expected_installed_manifest(manifest) {
        return Err("managed model manifest does not match its pinned artifact".into());
    }
    let model_path = directory.join(manifest.filename);
    let Some(stamp) = read_file_stamp(&model_path)? else {
        return Ok(None);
    };
    if stamp.bytes != manifest.bytes {
        return Err("model file size does not match its pinned artifact".into());
    }
    if receipt_matches(&directory, manifest, &stamp) {
        return Ok(Some(directory));
    }
    let actual_sha = hash_file(&model_path)?;
    if actual_sha != manifest.sha256 {
        return Err("model file SHA-256 does not match its pinned artifact".into());
    }
    // Re-read file identity after hashing. If the file changed while it was
    // being read, do not certify it from a stale digest.
    let Some(after_hash_stamp) = read_file_stamp(&model_path)? else {
        return Err("model file changed during verification".into());
    };
    if after_hash_stamp != stamp {
        return Err("model file changed during verification".into());
    }
    write_verification_receipt(&directory, manifest, stamp)?;
    Ok(Some(directory))
}

async fn download_model(
    models_root: &Path,
    manifest: &'static ModelManifest,
    bytes_done: Arc<AtomicU64>,
    cancel: CancellationToken,
) -> Result<(), String> {
    let partial = partial_directory(models_root, manifest.id);
    let result = download_model_inner(models_root, manifest, bytes_done, cancel).await;
    if let Err(error) = result {
        if let Err(cleanup_error) = safe_remove_staging(models_root, &partial) {
            return Err(format!("{error}; staging cleanup failed: {cleanup_error}"));
        }
        return Err(error);
    }
    Ok(())
}

async fn download_model_inner(
    models_root: &Path,
    manifest: &'static ModelManifest,
    bytes_done: Arc<AtomicU64>,
    cancel: CancellationToken,
) -> Result<(), String> {
    ensure_private_dir(models_root)?;
    let partial = partial_directory(models_root, manifest.id);
    ensure_private_dir(&partial)?;
    let temporary = partial.join(format!("{}.download", manifest.filename));
    let _ = fs::remove_file(&temporary);

    let client = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .user_agent(DOWNLOAD_USER_AGENT)
        .https_only(true)
        .build()
        .map_err(|error| sanitize_error(&error.to_string()))?;
    let url = format!(
        "https://huggingface.co/{}/resolve/{}/{}",
        manifest.repository, manifest.revision, manifest.filename
    );
    let response = tokio::select! {
        _ = cancel.cancelled() => return Err("download cancelled".into()),
        response = client.get(url).send() => {
            response.map_err(|error| sanitize_error(&error.to_string()))?
        }
    };
    if !response.status().is_success() {
        return Err(format!(
            "model file download failed ({})",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length != manifest.bytes)
    {
        return Err("model file size does not match its pinned artifact".into());
    }

    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .await
        .map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .await
            .map_err(|error| error.to_string())?;
    }
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => {
                drop(file);
                let _ = fs::remove_file(&temporary);
                let _ = fs::remove_dir_all(&partial);
                return Err("download cancelled".into());
            }
            next = stream.next() => next,
        };
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|error| sanitize_error(&error.to_string()))?;
        received = received
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| "model file size overflow".to_owned())?;
        if received > manifest.bytes {
            drop(file);
            let _ = fs::remove_file(&temporary);
            return Err("model file exceeds its pinned size".into());
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|error| error.to_string())?;
        bytes_done.store(received, Ordering::Release);
    }
    file.sync_all().await.map_err(|error| error.to_string())?;
    drop(file);
    if received != manifest.bytes || hex::encode(hasher.finalize()) != manifest.sha256 {
        let _ = fs::remove_file(&temporary);
        return Err("model file size or SHA-256 does not match its pinned artifact".into());
    }
    if cancel.is_cancelled() {
        let _ = fs::remove_file(&temporary);
        let _ = fs::remove_dir_all(&partial);
        return Err("download cancelled".into());
    }

    let staged_model = partial.join(manifest.filename);
    fs::rename(&temporary, &staged_model).map_err(|error| error.to_string())?;
    restrict_file_mode(&staged_model)?;
    let installed = expected_installed_manifest(manifest);
    atomic_write(
        &partial.join(INSTALLED_MANIFEST),
        &serde_json::to_vec(&installed).map_err(|error| error.to_string())?,
    )?;
    if cancel.is_cancelled() {
        let _ = fs::remove_dir_all(&partial);
        return Err("download cancelled".into());
    }
    replace_model_directory(&partial, &model_directory(models_root, manifest.id))?;
    // Persist the post-rename filesystem identity. A failure here leaves a
    // complete install that will be re-hashed on the next status/runtime use.
    let installed_path = model_directory(models_root, manifest.id);
    let stamp = read_file_stamp(&installed_path.join(manifest.filename))?
        .ok_or_else(|| "installed model file is missing".to_owned())?;
    write_verification_receipt(&installed_path, manifest, stamp)?;
    Ok(())
}

fn replace_model_directory(staging: &Path, destination: &Path) -> Result<(), String> {
    let staging_metadata = fs::symlink_metadata(staging).map_err(|error| error.to_string())?;
    if staging_metadata.file_type().is_symlink() || !staging_metadata.is_dir() {
        return Err("download staging directory is missing".into());
    }
    if let Ok(destination_metadata) = fs::symlink_metadata(destination) {
        if destination_metadata.file_type().is_symlink() {
            return Err("installed model directory is a symlink".into());
        }
    }
    if !path_entry_exists(destination) {
        fs::rename(staging, destination).map_err(|error| error.to_string())?;
        return Ok(());
    }
    let backup = destination.with_file_name(format!(
        ".{}.old-{}",
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("model"),
        std::process::id()
    ));
    remove_managed_entry(&backup)?;
    fs::rename(destination, &backup).map_err(|error| error.to_string())?;
    if let Err(error) = fs::rename(staging, destination) {
        let _ = fs::rename(&backup, destination);
        return Err(error.to_string());
    }
    remove_managed_entry(&backup)?;
    Ok(())
}

fn remove_managed_entry(path: &Path) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink() {
        return Err("refusing to remove a symlink from the local model store".into());
    }
    if metadata.is_dir() {
        fs::remove_dir_all(path).map_err(|error| error.to_string())
    } else {
        fs::remove_file(path).map_err(|error| error.to_string())
    }
}

fn checked_store_root(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err("local model store root is not a regular directory".into())
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn safe_remove_staging(models_root: &Path, staging: &Path) -> Result<(), String> {
    if !checked_store_root(models_root)? {
        return Ok(());
    }
    remove_managed_entry(staging)
}

fn sanitize_error(message: &str) -> String {
    let mut output = String::new();
    let mut rest = message;
    while let Some(scheme_at) = rest.find("://") {
        output.push_str(&rest[..scheme_at]);
        let after = &rest[scheme_at..];
        let end = after.find(char::is_whitespace).unwrap_or(after.len());
        let url = &after[..end];
        let url = url.split('?').next().unwrap_or(url);
        output.push_str(url);
        rest = &after[end..];
    }
    output.push_str(rest);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> std::io::Result<Self> {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "voiceflow-gguf-store-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path)?;
            Ok(Self(path))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const FIXTURE_MANIFEST: ModelManifest = ModelManifest {
        id: "qwen3.5-2b",
        label: "Fixture",
        repository: "fixture/model",
        revision: "fixture-revision",
        filename: "model.gguf",
        bytes: 5,
        sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
        license: "test",
        language: "test",
    };

    fn seed_fixture(root: &Path, manifest: &ModelManifest, contents: &[u8]) {
        let directory = model_directory(root, manifest.id);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join(manifest.filename), contents).unwrap();
        fs::write(
            directory.join(INSTALLED_MANIFEST),
            serde_json::to_vec(&expected_installed_manifest(manifest)).unwrap(),
        )
        .unwrap();
        let stamp = read_file_stamp(&directory.join(manifest.filename))
            .unwrap()
            .unwrap();
        write_verification_receipt(&directory, manifest, stamp).unwrap();
    }

    #[test]
    fn catalog_pins_the_three_requested_text_models() {
        assert_eq!(MODELS.len(), 3);
        assert_eq!(model(QWEN_35_2B_ID).unwrap().bytes, 1_270_808_032);
        assert_eq!(model(QWEN_35_4B_ID).unwrap().bytes, 2_707_513_696);
        assert_eq!(model(GEMMA_4_E4B_IT_ID).unwrap().bytes, 4_590_807_392);
        assert!(MODELS.iter().all(|model| {
            model.filename.ends_with(".gguf")
                && model.revision.len() == 40
                && model.sha256.len() == 64
                && model.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        }));
    }

    #[test]
    fn status_and_runtime_resolution_require_exact_digest_and_marker() {
        let root = TempDir::new().unwrap();
        let manager = Manager::default();
        seed_fixture(root.path(), &FIXTURE_MANIFEST, b"hello");
        assert!(verify_installed_model(root.path(), &FIXTURE_MANIFEST)
            .unwrap()
            .is_some());
        let resolved = manager
            .resolve_ready_with_manifest(root.path(), &FIXTURE_MANIFEST)
            .unwrap();
        assert_eq!(resolved.model_path().file_name().unwrap(), "model.gguf");
        drop(resolved);

        fs::write(
            model_directory(root.path(), FIXTURE_MANIFEST.id).join("model.gguf"),
            b"HELLO",
        )
        .unwrap();
        assert!(verify_installed_model(root.path(), &FIXTURE_MANIFEST).is_err());
        assert!(manager
            .resolve_ready_with_manifest(root.path(), &FIXTURE_MANIFEST)
            .is_err());
    }

    #[test]
    fn receipt_is_invalidated_when_file_identity_changes() {
        let root = TempDir::new().unwrap();
        seed_fixture(root.path(), &FIXTURE_MANIFEST, b"hello");
        let directory = model_directory(root.path(), FIXTURE_MANIFEST.id);
        fs::write(directory.join("replacement.gguf"), b"hello").unwrap();
        fs::rename(
            directory.join("replacement.gguf"),
            directory.join(FIXTURE_MANIFEST.filename),
        )
        .unwrap();
        assert!(verify_installed_model(root.path(), &FIXTURE_MANIFEST)
            .unwrap()
            .is_some());
    }

    #[test]
    fn managed_symlink_is_rejected_and_never_followed_for_delete() {
        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let protected = outside.path().join("keep");
        fs::write(&protected, b"keep").unwrap();
        let link = model_directory(root.path(), FIXTURE_MANIFEST.id);
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        let status = Manager::default().status(root.path(), FIXTURE_MANIFEST.id, true, None);
        assert_eq!(status.state, "corrupt");
        assert!(status.error.is_some());
        assert!(remove_managed_entry(&link).is_err());
        assert_eq!(fs::read(&protected).unwrap(), b"keep");
    }

    #[tokio::test]
    async fn deletion_waits_for_unload_and_rejects_live_model_leases() {
        let root = TempDir::new().unwrap();
        let manager = Manager::default();
        seed_fixture(root.path(), &FIXTURE_MANIFEST, b"hello");
        let lease = manager
            .resolve_ready_with_manifest(root.path(), &FIXTURE_MANIFEST)
            .unwrap();
        let called = Arc::new(AtomicU64::new(0));
        let called_unload = Arc::clone(&called);
        let error = manager
            .delete_with_unload(
                root.path().to_path_buf(),
                FIXTURE_MANIFEST.id,
                move || async move {
                    called_unload.fetch_add(1, Ordering::AcqRel);
                    Ok(())
                },
            )
            .await
            .unwrap_err();
        assert!(error.contains("still in use"));
        assert_eq!(called.load(Ordering::Acquire), 1);
        assert!(model_directory(root.path(), FIXTURE_MANIFEST.id).exists());
        drop(lease);
        manager
            .delete_with_unload(root.path().to_path_buf(), FIXTURE_MANIFEST.id, || async {
                Ok(())
            })
            .await
            .unwrap();
        assert!(!model_directory(root.path(), FIXTURE_MANIFEST.id).exists());
    }
}
