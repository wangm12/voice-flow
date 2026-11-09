//! Source-facts owner shared by the build script and evaluator. No generated receipt is an input.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const FORMAT: &str = "voiceflow-source-v3";
const DIRECTORIES: &[&str] = &["src", "src-tauri/src", "src-tauri/mlx-sidecar", "scripts"];
const FILES: &[&str] = &[
    "package.json",
    "package-lock.json",
    "src-tauri/Cargo.toml",
    "src-tauri/Cargo.lock",
    "src-tauri/build.rs",
    "src-tauri/source_identity.rs",
    "src-tauri/tauri.conf.json",
];
const MAX_FILE: u64 = 16 * 1024 * 1024;
const MAX_TOTAL: u64 = 128 * 1024 * 1024;
const MAX_ENTRIES: usize = 8192;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceFacts {
    pub format: String,
    pub source_commit: Option<String>,
    pub dirty: Option<bool>,
    pub source_sha256: Option<String>,
    pub lockfile_sha256: BTreeMap<String, String>,
    pub unavailable_reason: Option<String>,
    pub unavailable_path: Option<PathBuf>,
    pub git_unavailable_reason: Option<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceProvenance {
    BuildScriptSnapshotV3 {},
    // Old records contain runtime checkout facts, never an authenticated build snapshot.
    #[default]
    RuntimeCheckoutLegacy,
    Unknown {
        reason: String,
    },
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CheckoutObservation {
    pub path: PathBuf,
    pub canonical_path: Option<PathBuf>,
    pub facts: SourceFacts,
    pub content_comparison: CheckoutComparison,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum CheckoutComparison {
    Matches {},
    Mismatch {},
    Unknown { reason: String },
}
impl SourceFacts {
    pub fn unknown(reason: &str) -> Self {
        Self {
            format: FORMAT.into(),
            source_commit: None,
            dirty: None,
            source_sha256: None,
            lockfile_sha256: BTreeMap::new(),
            unavailable_reason: Some(reason.into()),
            unavailable_path: None,
            git_unavailable_reason: Some("git_identity_not_observed".into()),
        }
    }
    pub fn complete(&self) -> bool {
        fn digest(value: &str) -> bool {
            value.len() == 64
                && value
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        }
        self.format == FORMAT
            && self.unavailable_reason.is_none()
            && self.unavailable_path.is_none()
            && self.source_sha256.as_deref().is_some_and(digest)
            && self.lockfile_sha256.len() == 2
            && ["package-lock.json", "src-tauri/Cargo.lock"]
                .iter()
                .all(|name| {
                    self.lockfile_sha256
                        .get(*name)
                        .is_some_and(|value| digest(value))
                })
    }
    pub fn comparison(&self, live: &Self) -> CheckoutComparison {
        if !self.complete() || !live.complete() {
            return CheckoutComparison::Unknown {
                reason: "source_snapshot_unavailable".into(),
            };
        }
        match (&self.source_sha256, &live.source_sha256) {
            (Some(compiled), Some(checkout)) if self.format == live.format => {
                if compiled == checkout {
                    CheckoutComparison::Matches {}
                } else {
                    CheckoutComparison::Mismatch {}
                }
            }
            _ => CheckoutComparison::Unknown {
                reason: "source_snapshot_unavailable".into(),
            },
        }
    }
}
fn git(root: &Path, args: &[&str]) -> Option<String> {
    let result = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    if !result.status.success() || result.stdout.len() > 1024 * 1024 {
        return None;
    }
    String::from_utf8(result.stdout).ok()
}
fn git_root_matches(root: &Path) -> bool {
    git(root, &["rev-parse", "--show-toplevel"])
        .and_then(|path| std::fs::canonicalize(path.trim_end_matches('\n')).ok())
        .zip(std::fs::canonicalize(root).ok())
        .is_some_and(|(git_root, selected)| git_root == selected)
}
struct SourceFailure {
    reason: String,
    path: PathBuf,
}
impl SourceFailure {
    fn io(operation: &str, path: &Path, error: io::Error) -> Self {
        Self {
            reason: format!("{operation}:{:?}", error.kind()),
            path: path.into(),
        }
    }
    fn constraint(reason: &str, path: &Path) -> Self {
        Self {
            reason: reason.into(),
            path: path.into(),
        }
    }
}
fn read(path: &Path, total: &mut u64) -> Result<Vec<u8>, SourceFailure> {
    let file = std::fs::File::open(path).map_err(|e| SourceFailure::io("source_open", path, e))?;
    let metadata = file
        .metadata()
        .map_err(|e| SourceFailure::io("source_metadata", path, e))?;
    if !metadata.is_file() {
        return Err(SourceFailure::constraint("source_not_regular_file", path));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| SourceFailure::io("source_read", path, e))?;
    *total = total
        .checked_add(bytes.len() as u64)
        .ok_or_else(|| SourceFailure::constraint("source_total_limit", path))?;
    if bytes.len() as u64 > MAX_FILE || *total > MAX_TOTAL {
        return Err(SourceFailure::constraint("source_byte_limit", path));
    }
    Ok(bytes)
}
fn collect(
    root: &Path,
    relative: &Path,
    names: &mut Vec<PathBuf>,
    entries: &mut usize,
) -> Result<(), SourceFailure> {
    let path = root.join(relative);
    *entries += 1;
    if *entries > MAX_ENTRIES {
        return Err(SourceFailure::constraint("source_entry_limit", &path));
    }
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(SourceFailure::io("source_metadata", &path, e)),
    };
    // A symlink's bytes do not establish the bytes its consumer will compile.
    if metadata.file_type().is_symlink() {
        return Err(SourceFailure::constraint(
            "source_symlink_unresolved",
            &path,
        ));
    }
    if metadata.is_file() {
        names.push(relative.to_owned());
    } else if metadata.is_dir() {
        for entry in
            std::fs::read_dir(&path).map_err(|e| SourceFailure::io("source_directory", &path, e))?
        {
            let entry = entry.map_err(|e| SourceFailure::io("source_directory_entry", &path, e))?;
            // Bound directory enumeration even when every entry is excluded.
            *entries += 1;
            if *entries > MAX_ENTRIES {
                return Err(SourceFailure::constraint("source_entry_limit", &path));
            }
            let name = entry.file_name();
            if matches!(
                name.to_str(),
                Some(
                    ".git"
                        | ".build"
                        | ".swiftpm"
                        | "target"
                        | "node_modules"
                        | "__pycache__"
                        | ".DS_Store"
                )
            ) {
                continue;
            }
            if entry
                .path()
                .extension()
                .is_some_and(|ext| ext == "pyc" || ext == "pyo")
            {
                continue;
            }
            collect(root, &relative.join(name), names, entries)?;
        }
    } else {
        return Err(SourceFailure::constraint("source_entry_unsupported", &path));
    }
    Ok(())
}
fn source(root: &Path, locks: &mut BTreeMap<String, String>) -> Result<String, SourceFailure> {
    // A missing packaged checkout cannot become a digest of an empty source tree.
    if !root.join("src-tauri/src").is_dir() {
        return Err(SourceFailure::constraint(
            "source_tree_unavailable",
            &root.join("src-tauri/src"),
        ));
    }
    let mut names = Vec::new();
    let mut entries = 0;
    for name in DIRECTORIES.iter().chain(FILES) {
        collect(root, Path::new(name), &mut names, &mut entries)?;
    }
    names.sort();
    names.dedup();
    let mut hash = Sha256::new();
    hash.update(FORMAT.as_bytes());
    hash.update(b"\0");
    let mut total = 0;
    for name in names {
        let text = name.to_str().ok_or_else(|| {
            SourceFailure::constraint("source_filename_not_utf8", &root.join(&name))
        })?;
        // Convert path separators by component; a literal backslash in a Unix
        // filename must retain its identity rather than collide with a path.
        let text = if cfg!(windows) {
            text.replace('\\', "/")
        } else {
            text.to_owned()
        };
        let bytes = read(&root.join(&name), &mut total)?;
        let digest = hex::encode(Sha256::digest(&bytes));
        let value = format!("file:{digest}");
        hash.update((text.len() as u64).to_le_bytes());
        hash.update(text.as_bytes());
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value.as_bytes());
        if matches!(text.as_str(), "package-lock.json" | "src-tauri/Cargo.lock") {
            locks.insert(text, digest);
        }
    }
    if locks.len() != 2 {
        return Err(SourceFailure::constraint("required_lockfile_missing", root));
    }
    Ok(hex::encode(hash.finalize()))
}
pub(crate) fn capture(root: &Path) -> SourceFacts {
    let mut facts = SourceFacts::unknown("source_snapshot_unavailable");
    // Git facts and content facts are independent. A source archive can have a
    // content identity while commit/dirty remain unknown.
    if git_root_matches(root) {
        facts.source_commit = git(root, &["rev-parse", "--verify", "HEAD"])
            .map(|value| value.trim().to_owned())
            .filter(|commit| {
                matches!(commit.len(), 40 | 64) && commit.bytes().all(|b| b.is_ascii_hexdigit())
            });
        facts.dirty = git(root, &["status", "--porcelain", "--untracked-files=normal"])
            .map(|status| !status.is_empty());
        facts.git_unavailable_reason = match (&facts.source_commit, facts.dirty) {
            (Some(_), Some(_)) => None,
            (Some(_), None) => Some("git_dirty_state_unavailable".into()),
            (None, Some(_)) => Some("git_commit_unavailable".into()),
            (None, None) => Some("git_identity_unavailable".into()),
        };
    }
    match source(root, &mut facts.lockfile_sha256) {
        Ok(digest) => {
            facts.source_sha256 = Some(digest);
            facts.unavailable_reason = None;
        }
        Err(failure) => {
            facts.unavailable_reason = Some(failure.reason);
            facts.unavailable_path = Some(failure.path);
        }
    }
    facts
}
/// Cargo watches directories too, so new/deleted source and generated-cache
/// exclusions do not depend on git inventory. OUT_DIR stays outside this scope.
#[allow(dead_code)] // Build-script consumer only; capture is shared with runtime.
pub(crate) fn rerun_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<_> = DIRECTORIES
        .iter()
        .filter(|p| **p != "src-tauri/mlx-sidecar")
        .chain(FILES)
        .map(|p| root.join(p))
        .collect();
    // Watch the Swift sources, lockfiles, package recipe and hash-bound source
    // overlay that together determine the packaged native sidecar.
    paths.extend(
        [
            "Sources",
            "Tests",
            "Package.swift",
            "Package.resolved",
            "scripts/package.sh",
            "scripts/apply-sdk-cancellation-overlay.py",
            "scripts/normalize-staged-third-party-licenses.py",
            "sdk-overlay/patch-manifest.json",
            "sdk-overlay/voiceflow-mlx-audio-cancellation-v2.patch",
            "xcode/VoiceFlowMLXSidecar.xcodeproj/project.pbxproj",
            "xcode/VoiceFlowMLXSidecar.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved",
        ]
            .iter()
            .map(|p| root.join("src-tauri/mlx-sidecar").join(p)),
    );
    if !git_root_matches(root) {
        return paths;
    }
    if let Some(dir) = git(root, &["rev-parse", "--absolute-git-dir"]) {
        let dir = PathBuf::from(dir.trim_end_matches('\n'));
        paths.extend([dir.join("HEAD"), dir.join("index")]);
    }
    if let Some(dir) = git(root, &["rev-parse", "--git-common-dir"]) {
        let dir = root.join(dir.trim_end_matches('\n'));
        paths.extend([dir.join("refs"), dir.join("packed-refs")]);
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluation::fixtures::FixtureDirectory;
    fn tree(root: &Path) {
        std::fs::create_dir_all(root.join("src-tauri/src")).unwrap();
        std::fs::write(root.join("src-tauri/src/lib.rs"), b"pub fn fixture() {}\n").unwrap();
        std::fs::write(root.join("src-tauri/Cargo.lock"), b"cargo lock fixture\n").unwrap();
        std::fs::write(root.join("package-lock.json"), b"node lock fixture\n").unwrap();
    }
    fn git_command(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args([
                "-c",
                "user.name=VoiceFlow Fixture",
                "-c",
                "user.email=fixture@invalid.example",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "fixture git operation failed: {:?}",
            output.status
        );
    }
    #[test]
    fn source_archive_does_not_inherit_ancestor_git_identity_and_git_dirty_is_independent() {
        let d = FixtureDirectory::new();
        tree(&d.0);
        git_command(&d.0, &["-c", "init.templateDir=", "init", "--quiet"]);
        git_command(&d.0, &["add", "."]);
        git_command(
            &d.0,
            &[
                "commit",
                "--quiet",
                "--no-verify",
                "-m",
                "owned test source",
            ],
        );
        let clean = capture(&d.0);
        assert!(clean.complete());
        assert!(clean.source_commit.is_some());
        assert_eq!(clean.dirty, Some(false));
        std::fs::write(d.0.join("src-tauri/src/lib.rs"), b"pub fn changed() {}\n").unwrap();
        let dirty = capture(&d.0);
        assert_eq!(dirty.source_commit, clean.source_commit);
        assert_eq!(dirty.dirty, Some(true));
        assert_ne!(dirty.source_sha256, clean.source_sha256);
        let archive = FixtureDirectory::new_under(&d.0);
        tree(&archive.0);
        let archived = capture(&archive.0);
        assert!(archived.complete());
        assert!(archived.source_commit.is_none());
        assert!(archived.dirty.is_none());
        assert!(archived.git_unavailable_reason.is_some());
    }
    #[test]
    fn build_inputs_change_the_snapshot_but_generated_caches_and_out_dir_do_not() {
        let d = FixtureDirectory::new();
        tree(&d.0);
        std::fs::create_dir_all(d.0.join("scripts/research")).unwrap();
        std::fs::write(d.0.join("scripts/research/tool.py"), b"print('source')\n").unwrap();
        let before = capture(&d.0);
        for directory in [
            "scripts/research/__pycache__",
            "src-tauri/mlx-sidecar/.build",
            "src-tauri/target/out",
        ] {
            std::fs::create_dir_all(d.0.join(directory)).unwrap();
            std::fs::write(
                d.0.join(directory).join("derived.cache"),
                b"derived fixture",
            )
            .unwrap();
        }
        assert_eq!(capture(&d.0).source_sha256, before.source_sha256);
        std::fs::write(d.0.join("src-tauri/build.rs"), b"fn main() {}\n").unwrap();
        let build_changed = capture(&d.0);
        assert_ne!(build_changed.source_sha256, before.source_sha256);
        std::fs::write(d.0.join("src-tauri/Cargo.lock"), b"changed dependencies\n").unwrap();
        let lock_changed = capture(&d.0);
        assert_ne!(lock_changed.source_sha256, build_changed.source_sha256);
        assert_ne!(lock_changed.lockfile_sha256, build_changed.lockfile_sha256);
    }
    #[test]
    fn source_read_failure_and_missing_lock_preserve_reason_and_partial_facts() {
        let d = FixtureDirectory::new();
        tree(&d.0);
        std::fs::remove_file(d.0.join("src-tauri/Cargo.lock")).unwrap();
        let missing = capture(&d.0);
        assert!(!missing.complete());
        assert!(missing.source_sha256.is_none());
        assert_eq!(
            missing.unavailable_reason.as_deref(),
            Some("required_lockfile_missing")
        );
        assert_eq!(missing.lockfile_sha256.len(), 1);
        let path = d.0.join("missing.rs");
        let failure = read(&path, &mut 0).err().unwrap();
        assert_eq!(failure.path, path);
        assert_eq!(failure.reason, "source_open:NotFound");
        let sparse = d.0.join("oversized.rs");
        std::fs::File::create(&sparse)
            .unwrap()
            .set_len(MAX_FILE + 1)
            .unwrap();
        let failure = read(&sparse, &mut 0).err().unwrap();
        assert_eq!(failure.reason, "source_byte_limit");
        assert_eq!(failure.path, sparse);
    }
    #[cfg(unix)]
    #[test]
    fn unresolved_symlink_source_is_unknown_and_never_a_partial_success_digest() {
        let d = FixtureDirectory::new();
        tree(&d.0);
        std::os::unix::fs::symlink("lib.rs", d.0.join("src-tauri/src/link.rs")).unwrap();
        let facts = capture(&d.0);
        assert!(!facts.complete());
        assert!(facts.source_sha256.is_none());
        assert_eq!(
            facts.unavailable_reason.as_deref(),
            Some("source_symlink_unresolved")
        );
        assert_eq!(
            facts.unavailable_path,
            Some(d.0.join("src-tauri/src/link.rs"))
        );
    }
}
