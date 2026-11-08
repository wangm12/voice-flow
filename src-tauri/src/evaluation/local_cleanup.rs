//! Explicit fixed-model research server. No attach, credentials, downloads or defaults.
//! Admission is read-only; spawn/ready must follow the evaluator's seeded journal.
use super::admission_progress::{Progress, Scope};
use super::artifacts::{create_directory, id, FileId};
use super::validation::{error, external, InputError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

pub(super) const ID: &str = "vf14-qwen3-4b-instruct-2507-untrained";
const MODEL_REVISION: &str = "cdbee75f17c01a7cc42f958dc650907174af0554";
const MODEL_MANIFEST: &str = "8841dc6565a636a0b92c230437e8dcd14b430c349152001b23a4d1406ec1f001";
const SOURCE_MANIFEST: &str = "a6f5038f3cd655ce6910f7fe213b7d81efc0fef5a356f4132085d6d7425d1e64";
const LOCK: &str = "7e6ddea517d1c68f0ffc003d5f47025fe5e1f4d6389ca4dad2825442b2b707d6";
const LAUNCHER: &str = "42141bd898e4250e6bfaddb659af8d3b3b078c4030781940b0785e4364b1de62";
const PYTHON: &str = "7a314947596fc484d4ec36ffc28e843b78d6d749c68736f8b5c24e7333f418eb";
const RUNTIME: &str = "064addcefa0f28936cb2d7b18c67d4b145c62c4d0d9b4853658b8571b8072c0e";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedFile {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnknownFact {
    pub kind: String,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UntrainedProvenance {
    pub kind: String,
    pub dataset: String,
    pub adapter: String,
    pub merged_weights: String,
    pub conversion: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub message_utf8_bytes: usize,
    pub message_count: usize,
    pub request_body_bytes: usize,
    pub output_tokens: usize,
    pub response_bytes: usize,
    pub request_timeout_ms: u64,
    pub startup_timeout_ms: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupCandidateManifestV1 {
    pub artifact_type: String,
    pub schema_version: u32,
    pub candidate_id: String,
    pub base_model_repo: String,
    pub base_model_revision: String,
    pub asset_root: PathBuf,
    pub launcher: PinnedFile,
    pub python: PinnedFile,
    pub model_manifest: PinnedFile,
    pub source_manifest: PinnedFile,
    pub dependency_lock: PinnedFile,
    pub runtime_manifest: PinnedFile,
    pub tokenizer_sha256: String,
    pub tokenizer_config_sha256: String,
    pub chat_template_sha256: String,
    pub training: UntrainedProvenance,
    pub origin: String,
    pub path: String,
    pub limits: Limits,
    pub loaded_precision: UnknownFact,
    pub server_attestation: UnknownFact,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalResearchIdentity {
    pub candidate_id: String,
    pub manifest_sha256: String,
    pub fixture_sha256: String,
    pub expected_model_manifest_sha256: String,
    pub expected_tokenizer_sha256: String,
    pub expected_template_sha256: String,
    pub expected_runtime_manifest_sha256: String,
    pub actual_runtime_identity: UnknownFact,
    pub endpoint: String,
    pub workspace: PathBuf,
    pub workspace_identity: super::artifacts::ArtifactFileIdentity,
    pub workspace_path_applicable: Option<bool>,
    pub workspace_retained: bool,
    pub limits: Limits,
    pub actual_owned_loader_model_revision: Option<String>,
    pub expected_model_revision: String,
    pub expected_launcher_sha256: String,
    pub expected_source_manifest_sha256: String,
    pub owned_listener_observed: bool,
    pub owned_fixed_model_load_observed: bool,
    pub actual_loaded_precision: UnknownFact,
    pub independent_server_attestation: UnknownFact,
    pub lifecycle: String,
    pub cleanup_failure: Option<String>,
    pub replay: String,
}
struct HeldFile {
    file: File,
    selected: PathBuf,
    canonical: PathBuf,
    version: FileVersion,
}
#[derive(PartialEq, Eq)]
struct FileVersion {
    id: FileId,
    bytes: u64,
    modified: std::time::SystemTime,
    changed: (i64, i64),
}
/// Check the import/model namespace as well as known files: an added module,
/// bytecode file or weight shard must not silently become a new loader input.
fn exact_inventory(
    root: &Path,
    expected: &std::collections::BTreeSet<String>,
) -> Result<(), InputError> {
    let mut pending = vec![root.to_owned()];
    let mut count = 0usize;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)
            .map_err(|_| error(root, "input_unavailable", "pinned namespace unavailable"))?
        {
            let entry =
                entry.map_err(|_| error(root, "input_unavailable", "pinned entry unavailable"))?;
            count += 1;
            require(
                count <= expected.len().saturating_mul(2).saturating_add(32),
                root,
                "extra namespace entries",
            )?;
            let relative = entry
                .path()
                .strip_prefix(root)
                .expect("rooted entry")
                .to_path_buf();
            let name = relative
                .to_str()
                .ok_or_else(|| error(root, "path_escape", "non UTF-8 source name"))?;
            let kind = entry
                .file_type()
                .map_err(|_| error(root, "input_unavailable", "entry type unavailable"))?;
            if kind.is_dir() {
                require(
                    expected.iter().any(|n| n.starts_with(&format!("{name}/"))),
                    root,
                    "extra source directory",
                )?;
                pending.push(entry.path());
            } else {
                require(
                    kind.is_file() && expected.contains(name),
                    root,
                    "extra or nonregular loader input",
                )?;
            }
        }
    }
    Ok(())
}
impl HeldFile {
    fn open(
        pin: &PinnedFile,
        allow_symlink: bool,
        progress: &mut Progress,
    ) -> Result<(Self, Option<Vec<u8>>), InputError> {
        let path = &pin.path;
        if !digest(&pin.sha256) || !path.is_absolute() {
            return Err(error(
                path,
                "local_cleanup_identity",
                "absolute file and SHA256 required",
            ));
        }
        if !allow_symlink
            && std::fs::symlink_metadata(path)
                .map_err(|_| error(path, "input_unavailable", "selected pinned file missing"))?
                .file_type()
                .is_symlink()
        {
            return Err(error(
                path,
                "path_escape",
                "pinned source cannot be a symlink",
            ));
        }
        let canonical = external(path)?;
        require(
            allow_symlink || canonical == *path,
            path,
            "noncanonical/symlinked pinned source path",
        )?;
        let mut file = File::open(&canonical)
            .map_err(|_| error(path, "input_unavailable", "cannot open pinned file"))?;
        let version = file_version(&file, path)?;
        if version.bytes != pin.bytes {
            return Err(error(
                path,
                "identity_mismatch",
                "pinned byte length differs",
            ));
        }
        let mut hash = Sha256::new();
        let mut bytes = 0u64;
        let mut block = [0u8; 65536];
        // Only small manifests/configs are retained; weights are streamed once.
        let mut captured = (pin.bytes <= 1024 * 1024).then(Vec::new);
        loop {
            let n = file
                .read(&mut block)
                .map_err(|_| error(path, "input_unavailable", "pinned read failed"))?;
            if n == 0 {
                break;
            }
            progress.read(n as u64);
            bytes = bytes.saturating_add(n as u64);
            if bytes > pin.bytes {
                return Err(error(path, "identity_mismatch", "pinned file grew"));
            }
            hash.update(&block[..n]);
            progress.file_hash_feed(n as u64);
            if let Some(v) = &mut captured {
                v.extend_from_slice(&block[..n]);
            }
        }
        if bytes != pin.bytes || hex::encode(hash.finalize()) != pin.sha256 {
            return Err(error(
                path,
                "identity_mismatch",
                "pinned file digest differs",
            ));
        }
        let held = Self {
            file,
            selected: path.clone(),
            canonical,
            version,
        };
        held.current()?;
        progress.verified_file(bytes);
        Ok((held, captured))
    }
    fn current(&self) -> Result<(), InputError> {
        if external(&self.selected)? != self.canonical
            || file_version(&self.file, &self.selected)? != self.version
            || file_version(
                &File::open(&self.selected).map_err(|_| {
                    error(
                        &self.selected,
                        "identity_mismatch",
                        "selected file unavailable",
                    )
                })?,
                &self.selected,
            )? != self.version
        {
            return Err(error(
                &self.selected,
                "identity_mismatch",
                "held file no longer applies to selected source",
            ));
        }
        Ok(())
    }
}
#[cfg(unix)]
fn changed(meta: &std::fs::Metadata) -> (i64, i64) {
    use std::os::unix::fs::MetadataExt;
    (meta.ctime(), meta.ctime_nsec())
}
#[cfg(not(unix))]
fn changed(_meta: &std::fs::Metadata) -> (i64, i64) {
    (0, 0)
}
fn file_version(file: &File, path: &Path) -> Result<FileVersion, InputError> {
    let m = file
        .metadata()
        .map_err(|_| error(path, "identity_unavailable", "file identity unavailable"))?;
    if !m.is_file() {
        return Err(error(path, "schema", "regular file required"));
    }
    Ok(FileVersion {
        id: id(&m).map_err(|_| error(path, "identity_unavailable", "file identity unavailable"))?,
        bytes: m.len(),
        changed: changed(&m),
        modified: m
            .modified()
            .map_err(|_| error(path, "identity_unavailable", "file version unavailable"))?,
    })
}
fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn relative(s: &str) -> bool {
    !s.is_empty()
        && Path::new(s)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}
fn require(ok: bool, path: &Path, message: &str) -> Result<(), InputError> {
    if ok {
        Ok(())
    } else {
        Err(error(path, "local_cleanup_identity", message))
    }
}

pub(super) struct Plan {
    pub manifest: CleanupCandidateManifestV1,
    pub manifest_sha256: String,
    files: Vec<HeldFile>,
    inventories: Vec<(PathBuf, std::collections::BTreeSet<String>)>,
    workspace: PathBuf,
    parent: File,
    parent_id: FileId,
}
impl Plan {
    pub(super) fn workspace_path(&self) -> &Path {
        &self.workspace
    }
    /// Does no spawn, request, directory creation or credential lookup.
    pub fn admit(
        path: &Path,
        workspace: &Path,
        progress: &mut Progress,
    ) -> Result<Self, InputError> {
        progress.begin(Scope::Candidate);
        let bytes = super::validation::read_bounded(path, 1024 * 1024)?;
        progress.read(bytes.len() as u64);
        progress.declare_files([Some(bytes.len() as u64)]);
        progress.next_entry();
        let manifest: CleanupCandidateManifestV1 =
            serde_json::from_slice(&bytes).map_err(|e| error(path, "schema", e.to_string()))?;
        let m = &manifest;
        require(
            m.artifact_type == "voiceflow_cleanup_candidate"
                && m.schema_version == 1
                && m.candidate_id == ID
                && m.base_model_repo == "Qwen/Qwen3-4B-Instruct-2507"
                && m.base_model_revision == MODEL_REVISION,
            path,
            "only pinned untrained2507 research candidate supported",
        )?;
        require(
            m.training.kind == "untrained"
                && [
                    &m.training.dataset,
                    &m.training.adapter,
                    &m.training.merged_weights,
                    &m.training.conversion,
                ]
                .into_iter()
                .all(|s| s == "not_applicable"),
            path,
            "untrained dataset/adapter/fusion/conversion must be known not applicable",
        )?;
        require(
            [&m.loaded_precision, &m.server_attestation]
                .into_iter()
                .all(|f| f.kind == "unknown" && !f.reason.is_empty()),
            path,
            "expected pins cannot assert actual loaded precision/attestation",
        )?;
        endpoint(&m.origin, &m.path)?;
        require(
            m.launcher.sha256 == LAUNCHER
                && m.python.sha256 == PYTHON
                && m.model_manifest.sha256 == MODEL_MANIFEST
                && m.source_manifest.sha256 == SOURCE_MANIFEST
                && m.dependency_lock.sha256 == LOCK
                && m.runtime_manifest.sha256 == RUNTIME,
            path,
            "unsupported launcher/runtime/model identity; no arbitrary commands",
        )?;
        require(
            external(&m.asset_root)? == m.asset_root,
            path,
            "asset root must be selected canonical external directory",
        )?;
        for (pin, rel) in [
            (&m.python, "runtime-venv/bin/python"),
            (&m.model_manifest, "model-original/acquisition.json"),
            (&m.source_manifest, "runtime-source/source-manifest.json"),
            (&m.dependency_lock, "planning/runtime-requirements.lock"),
            (&m.runtime_manifest, "runtime-manifest.json"),
        ] {
            require(
                pin.path == m.asset_root.join(rel),
                path,
                "asset pin path differs from fixed local layout",
            )?;
        }
        let l = &m.limits;
        // Existing fixed-wrapper capacities and shared parser bounds, not model context claims.
        require(
            l.message_utf8_bytes > 0
                && l.message_utf8_bytes <= 4096
                && l.message_count > 0
                && l.message_count <= 8
                && l.request_body_bytes > 0
                && l.request_body_bytes <= 65536
                && l.output_tokens > 0
                && l.output_tokens <= 512
                && l.response_bytes > 0
                && l.response_bytes <= 1024 * 1024
                && l.request_timeout_ms > 0
                && l.startup_timeout_ms > 0,
            path,
            "limits exceed fixed wrapper/shared parser or omit deadline",
        )?;
        let manifest_pin = PinnedFile {
            path: path.to_path_buf(),
            bytes: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(&bytes)),
        };
        progress.file_hash_feed(bytes.len() as u64);
        progress.candidate(&manifest_pin.sha256);
        let mut files = vec![HeldFile::open(&manifest_pin, false, progress)?.0];
        progress.end(Scope::Candidate, true);
        let mut model = None;
        let mut source = None;
        // The declared role selects the symlink policy and sole inventory-byte
        // consumer; every role retains its independently validated descriptor.
        #[derive(Clone, Copy)]
        enum PinRole {
            ExpectedFile,
            Interpreter,
            ModelInventory,
            SourceInventory,
        }
        let pins = [
            (PinRole::ExpectedFile, &m.launcher),
            (PinRole::Interpreter, &m.python),
            (PinRole::ModelInventory, &m.model_manifest),
            (PinRole::SourceInventory, &m.source_manifest),
            (PinRole::ExpectedFile, &m.dependency_lock),
            (PinRole::ExpectedFile, &m.runtime_manifest),
        ];
        progress.begin(Scope::FixedPins);
        progress.declare_files(pins.iter().map(|(_, pin)| Some(pin.bytes)));
        for (role, pin) in pins {
            progress.next_entry();
            let (held, raw) = HeldFile::open(pin, matches!(role, PinRole::Interpreter), progress)?;
            match role {
                PinRole::ModelInventory => model = raw,
                PinRole::SourceInventory => source = raw,
                PinRole::ExpectedFile | PinRole::Interpreter => {}
            }
            files.push(held);
        }
        progress.end(Scope::FixedPins, true);
        progress.begin(Scope::ModelFiles);
        let model: serde_json::Value = serde_json::from_slice(
            model
                .as_deref()
                .ok_or_else(|| error(path, "input_budget", "model manifest exceeds capture"))?,
        )
        .map_err(|e| error(path, "schema", e.to_string()))?;
        require(
            model["revision"].as_str() == Some(MODEL_REVISION),
            path,
            "model revision mismatch",
        )?;
        let entries = model["files"]
            .as_array()
            .ok_or_else(|| error(path, "schema", "model file inventory missing"))?;
        progress.declare_files(entries.iter().map(|e| e["actual_bytes"].as_u64()));
        let mut names = std::collections::BTreeSet::new();
        for e in entries {
            progress.next_entry();
            let name = e["path"]
                .as_str()
                .ok_or_else(|| error(path, "schema", "model path missing"))?;
            require(
                relative(name) && names.insert(name),
                path,
                "duplicate/escaping model file",
            )?;
            let pin = entry_pin(&m.asset_root.join("model-original"), name, e, path)?;
            let (held, raw) = HeldFile::open(&pin, false, progress)?;
            if name == "tokenizer.json" {
                require(pin.sha256 == m.tokenizer_sha256, path, "tokenizer mismatch")?;
            }
            if name == "tokenizer_config.json" {
                require(
                    pin.sha256 == m.tokenizer_config_sha256,
                    path,
                    "tokenizer config mismatch",
                )?;
                let v: serde_json::Value =
                    serde_json::from_slice(raw.as_deref().ok_or_else(|| {
                        error(path, "input_budget", "tokenizer config too large")
                    })?)
                    .map_err(|e| error(path, "schema", e.to_string()))?;
                let template = v["chat_template"]
                    .as_str()
                    .ok_or_else(|| error(path, "schema", "template missing"))?;
                require(
                    hex::encode(Sha256::digest(template.as_bytes())) == m.chat_template_sha256,
                    path,
                    "template digest mismatch",
                )?;
            }
            files.push(held);
        }
        // The pinned loader may discover safetensors: extra files must not change its input.
        for e in std::fs::read_dir(m.asset_root.join("model-original"))
            .map_err(|_| error(path, "input_unavailable", "model directory unavailable"))?
        {
            let name = e
                .map_err(|_| error(path, "input_unavailable", "model entry unavailable"))?
                .file_name();
            require(
                name.to_str()
                    .is_some_and(|n| n == "acquisition.json" || names.contains(n)),
                path,
                "extra file in fixed model directory",
            )?;
        }
        progress.end(Scope::ModelFiles, true);
        progress.begin(Scope::RuntimeSources);
        let source: serde_json::Value = serde_json::from_slice(
            source
                .as_deref()
                .ok_or_else(|| error(path, "input_budget", "source manifest too large"))?,
        )
        .map_err(|e| error(path, "schema", e.to_string()))?;
        let source_files = source["source_files"]
            .as_object()
            .ok_or_else(|| error(path, "schema", "source file inventory missing"))?;
        progress.declare_files(source_files.values().map(|e| e["bytes"].as_u64()));
        for (name, e) in source_files {
            progress.next_entry();
            require(relative(name), path, "escaping runtime source")?;
            let pin = PinnedFile {
                path: m.asset_root.join("runtime-source").join(name),
                bytes: e["bytes"]
                    .as_u64()
                    .ok_or_else(|| error(path, "schema", "source bytes missing"))?,
                sha256: e["sha256"]
                    .as_str()
                    .ok_or_else(|| error(path, "schema", "source SHA missing"))?
                    .into(),
            };
            files.push(HeldFile::open(&pin, false, progress)?.0);
        }
        progress.end(Scope::RuntimeSources, true);
        progress.begin(Scope::VersionShim);
        let shim = &source["generated_version_shim"];
        let name = shim["path"]
            .as_str()
            .ok_or_else(|| error(path, "schema", "version shim missing"))?;
        require(relative(name), path, "escaping shim")?;
        let shim_path = m.asset_root.join("runtime-source").join(name);
        let pin = PinnedFile {
            bytes: std::fs::metadata(&shim_path)
                .map_err(|_| error(path, "input_unavailable", "shim unavailable"))?
                .len(),
            path: shim_path,
            sha256: shim["sha256"]
                .as_str()
                .ok_or_else(|| error(path, "schema", "shim SHA missing"))?
                .into(),
        };
        progress.declare_files([Some(pin.bytes)]);
        progress.next_entry();
        files.push(HeldFile::open(&pin, false, progress)?.0);
        progress.end(Scope::VersionShim, true);
        progress.begin(Scope::ExactInventories);
        let inventories = vec![
            (
                m.asset_root.join("model-original"),
                names
                    .into_iter()
                    .map(str::to_owned)
                    .chain(std::iter::once("acquisition.json".into()))
                    .collect(),
            ),
            (
                m.asset_root.join("runtime-source"),
                source_files
                    .keys()
                    .cloned()
                    .chain([name.to_owned(), "source-manifest.json".into()])
                    .collect(),
            ),
        ];
        progress.declare_files([None, None]);
        for (root, names) in &inventories {
            progress.next_entry();
            exact_inventory(root, names)?;
            progress.entry_done();
        }
        progress.end(Scope::ExactInventories, true);
        progress.begin(Scope::WorkspaceParent);
        progress.declare_files([None]);
        progress.next_entry();
        require(
            matches!(std::fs::symlink_metadata(workspace), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
                && workspace.is_absolute(),
            workspace,
            "workspace must be a NEW absolute external path",
        )?;
        let parent_path = external(
            workspace
                .parent()
                .ok_or_else(|| error(workspace, "path_escape", "workspace parent missing"))?,
        )?;
        let parent = File::open(&parent_path).map_err(|_| {
            error(
                workspace,
                "input_unavailable",
                "workspace parent unavailable",
            )
        })?;
        let parent_id = id(&parent.metadata().map_err(|_| {
            error(
                workspace,
                "identity_unavailable",
                "workspace parent identity unavailable",
            )
        })?)
        .map_err(|_| {
            error(
                workspace,
                "identity_unavailable",
                "workspace parent identity unavailable",
            )
        })?;
        let workspace = parent_path.join(
            workspace
                .file_name()
                .ok_or_else(|| error(workspace, "path_escape", "workspace filename required"))?,
        );
        require(
            !workspace.starts_with(&m.asset_root)
                && !m.asset_root.starts_with(&workspace)
                && !workspace
                    .starts_with(m.launcher.path.parent().expect("absolute pinned launcher")),
            &workspace,
            "workspace must be disjoint from immutable assets/launcher source",
        )?;
        progress.entry_done();
        progress.end(Scope::WorkspaceParent, true);
        let manifest_sha256 = hex::encode(Sha256::digest(&bytes));
        progress.file_hash_feed(bytes.len() as u64);
        Ok(Self {
            manifest,
            manifest_sha256,
            files,
            inventories,
            workspace,
            parent,
            parent_id,
        })
    }
    /// Caller must seed the real journal before this admitted side effect. Return
    /// the owner immediately: readiness failure cannot erase a spawned child.
    pub fn spawn(self, fixture_sha256: &str) -> Result<OwnedServerLease, InputError> {
        require(
            digest(fixture_sha256),
            &self.workspace,
            "fixture SHA256 required",
        )?;
        for f in &self.files {
            f.current()?;
        }
        require(
            id(&self
                .parent
                .metadata()
                .map_err(|_| error(&self.workspace, "identity_unavailable", "parent lost"))?)
            .ok()
                == Some(self.parent_id)
                && File::open(self.workspace.parent().expect("admitted parent"))
                    .ok()
                    .and_then(|f| f.metadata().ok())
                    .and_then(|m| id(&m).ok())
                    == Some(self.parent_id),
            &self.workspace,
            "held workspace parent/path changed",
        )?;
        let directory = create_directory(
            &self.parent,
            self.workspace.file_name().expect("admitted workspace name"),
        )
        .map_err(|_| {
            error(
                &self.workspace,
                "local_cleanup_setup",
                "exclusive owned directory creation failed; partial workspace retained",
            )
        })?;
        let directory_id = id(&directory.metadata().map_err(|_| {
            error(
                &self.workspace,
                "identity_unavailable",
                "owned workspace retained",
            )
        })?)
        .map_err(|_| {
            error(
                &self.workspace,
                "identity_unavailable",
                "owned workspace identity unsupported",
            )
        })?;
        let mut random = [0u8; 32];
        File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut random))
            .map_err(|_| {
                error(
                    &self.workspace,
                    "local_cleanup_setup",
                    "lease nonce unavailable; workspace retained",
                )
            })?;
        let nonce = hex::encode(random);
        // No asynchronous gap separates this re-admission from spawn. This is
        // current pathname applicability, not a claim against arbitrary external mutation.
        for f in &self.files {
            f.current()?;
        }
        for (root, names) in &self.inventories {
            exact_inventory(root, names)?;
        }
        require(
            File::open(&self.workspace)
                .ok()
                .and_then(|f| f.metadata().ok())
                .and_then(|m| id(&m).ok())
                == Some(directory_id),
            &self.workspace,
            "new workspace path changed before spawn; owned directory retained",
        )?;
        let m = &self.manifest;
        let mut command = Command::new(&m.python.path);
        command
            .arg("-s")
            .arg(&m.launcher.path)
            .arg("--asset-root")
            .arg(&m.asset_root)
            .arg("--workspace")
            .arg(&self.workspace)
            .arg("--lease-nonce")
            .arg(&nonce)
            .arg("--fixture-sha256")
            .arg(fixture_sha256)
            .arg("--port")
            .arg("18084");
        command
            .current_dir(&self.workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .env("PYTHONPATH", m.asset_root.join("runtime-source"))
            .env("PYTHONNOUSERSITE", "1")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("PYTHONUNBUFFERED", "1")
            .env("HF_HUB_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1")
            .env("HF_HUB_DISABLE_IMPLICIT_TOKEN", "1")
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .env("HF_TOKEN", "")
            .env("HUGGING_FACE_HUB_TOKEN", "")
            .env("TOKENIZERS_PARALLELISM", "false");
        // HOME/CODEX_HOME remain inherited; no ambient environment is recorded.
        let mut child = command.spawn().map_err(|_| {
            error(
                &self.workspace,
                "local_cleanup_spawn",
                "owned workspace retained; pinned child spawn failed",
            )
        })?;
        let stdout = child.stdout.take().expect("piped stdout");
        let (signals, rx) = watch::channel(Signals::default());
        let expected = ReceiptExpectation {
            nonce: nonce.clone(),
            fixture_sha256: fixture_sha256.into(),
        };
        let reader = tokio::spawn(read_signals(stdout, signals, expected));
        let identity = LocalResearchIdentity {
            candidate_id: ID.into(),
            manifest_sha256: self.manifest_sha256.clone(),
            fixture_sha256: fixture_sha256.into(),
            expected_model_manifest_sha256: MODEL_MANIFEST.into(),
            expected_tokenizer_sha256: self.manifest.tokenizer_sha256.clone(),
            expected_template_sha256: self.manifest.chat_template_sha256.clone(),
            expected_runtime_manifest_sha256: RUNTIME.into(),
            actual_runtime_identity: UnknownFact { kind: "unknown".into(), reason: "runtime manifest is an expected historical dependency record; current loaded native/dependency bytes are not independently attested".into() },
            endpoint: format!("{}{}", self.manifest.origin, self.manifest.path),
            workspace: self.workspace.clone(),
            workspace_identity: directory_id.evidence(),
            workspace_path_applicable: Some(true),
            workspace_retained: true,
            limits: self.manifest.limits.clone(),
            actual_owned_loader_model_revision: None,
            expected_model_revision: MODEL_REVISION.into(),
            expected_launcher_sha256: LAUNCHER.into(),
            expected_source_manifest_sha256: SOURCE_MANIFEST.into(),
            owned_listener_observed: false,
            owned_fixed_model_load_observed: false,
            actual_loaded_precision: UnknownFact {
                kind: "unknown".into(),
                reason: "loaded tensor precision not attested by this launcher".into(),
            },
            independent_server_attestation: UnknownFact {
                kind: "unknown".into(),
                reason:
                    "owned source/loader receipt is not independent cryptographic model attestation"
                        .into(),
            },
            lifecycle: "spawned_not_ready".into(),
            cleanup_failure: None,
            replay: "fresh_only_runtime_precision_identity_incomplete".into(),
        };
        Ok(OwnedServerLease {
            plan: self,
            child: Some(child),
            reader: Some(reader),
            signals: rx,
            nonce,
            directory,
            directory_id,
            identity,
            usable: false,
            inflight: false,
        })
    }
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({"candidate_id":ID,"manifest_sha256":self.manifest_sha256,"expected_model_revision":MODEL_REVISION,"workspace":self.workspace,"loaded":"not_run","replay":"fresh_only"})
    }
}
fn entry_pin(
    root: &Path,
    name: &str,
    e: &serde_json::Value,
    path: &Path,
) -> Result<PinnedFile, InputError> {
    Ok(PinnedFile {
        path: root.join(name),
        bytes: e["actual_bytes"]
            .as_u64()
            .ok_or_else(|| error(path, "schema", "model bytes missing"))?,
        sha256: e["actual_sha256"]
            .as_str()
            .ok_or_else(|| error(path, "schema", "model SHA missing"))?
            .into(),
    })
}
fn endpoint(origin: &str, path: &str) -> Result<String, InputError> {
    let p = Path::new("cleanup_candidate.origin");
    let u = reqwest::Url::parse(origin)
        .map_err(|_| error(p, "schema", "literal loopback origin required"))?;
    require(
        origin == "http://127.0.0.1:18084"
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none()
            && path == "/v1/chat/completions",
        p,
        "only declared literal loopback origin and fixed chat path allowed",
    )?;
    Ok(format!("{origin}{path}"))
}

#[derive(Clone, Default)]
struct Signals {
    listening: bool,
    loaded: bool,
    error: Option<String>,
}
struct ReceiptExpectation {
    nonce: String,
    fixture_sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    kind: String,
    nonce: String,
    fixture_sha256: String,
    launcher_sha256: String,
    model_manifest_sha256: String,
    model_revision: String,
    source_manifest_sha256: String,
}
async fn read_signals(
    mut stdout: tokio::process::ChildStdout,
    signals: watch::Sender<Signals>,
    expected: ReceiptExpectation,
) {
    let mut block = [0u8; 4096];
    let mut line = Vec::new();
    let mut total = 0usize;
    let mut failed = false;
    loop {
        let n = match stdout.read(&mut block).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => {
                signals.send_modify(|s| s.error = Some("owned child stdout read failed".into()));
                return;
            }
        };
        total = total.saturating_add(n);
        if total > 1024 * 1024 {
            failed = true;
            signals.send_modify(|s| {
                s.error = Some("owned diagnostics exceed1MiB; receipt scope unknown".into())
            });
        }
        for &byte in &block[..n] {
            if failed {
                continue;
            }
            if byte != b'\n' {
                line.push(byte);
                if line.len() > 4096 {
                    failed = true;
                    signals.send_modify(|s| {
                        s.error = Some("owned diagnostic line exceeds4096B".into())
                    });
                }
                continue;
            }
            if let Some(raw) = line.strip_prefix(b"VF14_IDENTITY ") {
                match serde_json::from_slice::<Receipt>(raw) {
                    Ok(v)
                        if v.nonce == expected.nonce
                            && v.fixture_sha256 == expected.fixture_sha256
                            && v.launcher_sha256 == LAUNCHER
                            && v.model_manifest_sha256 == MODEL_MANIFEST
                            && v.source_manifest_sha256 == SOURCE_MANIFEST
                            && v.model_revision == MODEL_REVISION =>
                    {
                        signals.send_modify(|s| match v.kind.as_str() {
                            "listening" if !s.listening => s.listening = true,
                            "loaded" => s.loaded = true,
                            _ => s.error = Some("unexpected owned identity event".into()),
                        });
                    }
                    _ => {
                        signals.send_modify(|s| {
                            s.error = Some("owned child identity mismatch".into())
                        });
                    }
                }
            }
            line.clear();
        }
    }
    signals.send_modify(|s| s.error = Some("owned child stdout closed".into()));
}
pub(crate) struct OwnedServerLease {
    plan: Plan,
    child: Option<Child>,
    reader: Option<tokio::task::JoinHandle<()>>,
    signals: watch::Receiver<Signals>,
    nonce: String,
    directory: File,
    directory_id: FileId,
    identity: LocalResearchIdentity,
    usable: bool,
    inflight: bool,
}
impl OwnedServerLease {
    pub async fn ready(&mut self, cancel: &CancellationToken) -> Result<(), InputError> {
        let timeout = Duration::from_millis(self.plan.manifest.limits.startup_timeout_ms);
        let deadline = tokio::time::Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| {
                error(
                    &self.plan.workspace,
                    "schema",
                    "startup deadline overflows clock",
                )
            })?;
        loop {
            self.alive()?;
            let state = self.signals.borrow().clone();
            if let Some(reason) = state.error {
                return Err(error(&self.plan.workspace, "local_cleanup_startup", reason));
            }
            if state.listening {
                self.usable = true;
                self.identity.owned_listener_observed = true;
                self.identity.lifecycle = "owned_listener_not_loaded".into();
                return Ok(());
            }
            tokio::select! {biased;_=cancel.cancelled()=>return Err(error(&self.plan.workspace,"cancelled","cancelled before owned server ready")),_=tokio::time::sleep_until(deadline)=>return Err(error(&self.plan.workspace,"local_cleanup_timeout","owned startup deadline exceeded")),changed=self.signals.changed()=>{changed.map_err(|_|error(&self.plan.workspace,"local_cleanup_startup","identity pipe closed"))?;}}
        }
    }
    fn alive(&mut self) -> Result<(), InputError> {
        require(
            id(&self.directory.metadata().map_err(|_| {
                error(
                    &self.plan.workspace,
                    "identity_unavailable",
                    "held workspace unavailable",
                )
            })?)
            .ok()
                == Some(self.directory_id),
            &self.plan.workspace,
            "held workspace identity unavailable",
        )?;
        require(
            File::open(&self.plan.workspace)
                .ok()
                .and_then(|f| f.metadata().ok())
                .and_then(|m| id(&m).ok())
                == Some(self.directory_id),
            &self.plan.workspace,
            "workspace path changed; owned resources retained",
        )?;
        let child = self.child.as_mut().ok_or_else(|| {
            error(
                &self.plan.workspace,
                "local_cleanup_closed",
                "owned server already closed",
            )
        })?;
        match child.try_wait() {
            Ok(None) => Ok(()),
            Ok(Some(_)) => {
                self.usable = false;
                self.identity.lifecycle = "child_exited".into();
                Err(error(
                    &self.plan.workspace,
                    "local_cleanup_exited",
                    "owned child exited; no attach/fallback",
                ))
            }
            Err(_) => {
                self.usable = false;
                Err(error(
                    &self.plan.workspace,
                    "identity_unavailable",
                    "owned child state unknown",
                ))
            }
        }
    }
    pub fn identity(&self) -> LocalResearchIdentity {
        let mut i = self.identity.clone();
        i.workspace_path_applicable = std::fs::metadata(&self.plan.workspace)
            .ok()
            .and_then(|m| id(&m).ok())
            .map(|observed| observed == self.directory_id);
        i.owned_fixed_model_load_observed = self.signals.borrow().loaded;
        i.actual_owned_loader_model_revision = i
            .owned_fixed_model_load_observed
            .then(|| MODEL_REVISION.into());
        i
    }
    pub fn capability(&mut self) -> Result<VerifiedLocalOpenAi<'_>, InputError> {
        self.alive()?;
        require(
            self.usable && !self.inflight,
            &self.plan.workspace,
            "owned lease not ready, tainted or already dispatching",
        )?;
        if let Some(reason) = self.signals.borrow().error.clone() {
            return Err(error(
                &self.plan.workspace,
                "local_cleanup_identity",
                reason,
            ));
        }
        for f in &self.plan.files {
            f.current()?;
        }
        Ok(VerifiedLocalOpenAi {
            lease: self,
            settled: false,
        })
    }
    pub async fn shutdown(&mut self) -> Result<(), InputError> {
        self.usable = false;
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        self.identity.lifecycle = "stopping".into();
        if matches!(child.try_wait(), Ok(Some(_))) {
            self.child = None;
            if let Some(reader) = self.reader.take() {
                reader.abort();
            }
            self.identity.lifecycle = "reaped".into();
            return Ok(());
        }
        if let Err(e) = child.start_kill() {
            self.identity.cleanup_failure = Some(e.to_string());
            return Err(error(
                &self.plan.workspace,
                "local_cleanup_cleanup",
                "owned kill failed; child retained",
            ));
        }
        match tokio::time::timeout(Duration::from_secs(3), child.wait()).await {
            Ok(Ok(_)) => {
                self.child = None;
                if let Some(reader) = self.reader.take() {
                    reader.abort();
                }
                self.identity.lifecycle = "reaped".into();
                Ok(())
            }
            _ => {
                self.identity.cleanup_failure =
                    Some("owned wait failed/timed out; child retained".into());
                Err(error(
                    &self.plan.workspace,
                    "local_cleanup_cleanup",
                    "owned wait failed; child retained",
                ))
            }
        }
    }
}
/// Borrows the *live owner*, never a cloned URL/PID/nonce. Drop taints an
/// unsettled dispatched request; separate observer protocol facts stay intact.
pub(crate) struct VerifiedLocalOpenAi<'a> {
    lease: &'a mut OwnedServerLease,
    settled: bool,
}
impl VerifiedLocalOpenAi<'_> {
    pub(crate) fn endpoint(&self) -> String {
        format!(
            "{}{}",
            self.lease.plan.manifest.origin, self.lease.plan.manifest.path
        )
    }
    pub(crate) fn model(&self) -> &str {
        ID
    }
    pub(crate) fn limits(&self) -> &Limits {
        &self.lease.plan.manifest.limits
    }
    pub(crate) fn nonce(&self) -> &str {
        &self.lease.nonce
    }
    pub(crate) fn begin_dispatch(&mut self) -> Result<(), InputError> {
        self.lease.alive()?;
        for f in &self.lease.plan.files {
            f.current()?;
        }
        for (root, names) in &self.lease.plan.inventories {
            exact_inventory(root, names)?;
        }
        require(
            self.lease.usable && !self.lease.inflight,
            &self.lease.plan.workspace,
            "lease is no longer usable",
        )?;
        self.lease.inflight = true;
        self.lease.identity.lifecycle = "request_started_terminal_unknown".into();
        Ok(())
    }
    pub(crate) async fn confirm_complete(&mut self) -> Result<(), InputError> {
        self.lease.alive()?;
        loop {
            let signals = self.lease.signals.borrow().clone();
            if let Some(reason) = signals.error {
                return Err(error(
                    &self.lease.plan.workspace,
                    "local_cleanup_identity",
                    reason,
                ));
            }
            if signals.loaded {
                break;
            }
            self.lease.signals.changed().await.map_err(|_| {
                error(
                    &self.lease.plan.workspace,
                    "local_cleanup_identity",
                    "no actual fixed-loader receipt",
                )
            })?;
        }
        self.lease.identity.owned_fixed_model_load_observed = true;
        self.lease.identity.lifecycle = "completed_protocol_internal_quiescence_unknown".into();
        self.lease.inflight = false;
        self.settled = true;
        Ok(())
    }
}
impl Drop for VerifiedLocalOpenAi<'_> {
    fn drop(&mut self) {
        if !self.settled && self.lease.inflight {
            self.lease.usable = false;
            self.lease.identity.lifecycle = "request_owner_unsettled_requires_shutdown".into();
        }
        self.lease.inflight = false;
    }
}
struct RetainedChild {
    _child: Child,
    _reader: Option<tokio::task::JoinHandle<()>>,
    _reason: String,
}
fn retained() -> &'static Mutex<Vec<RetainedChild>> {
    static RETAINED: OnceLock<Mutex<Vec<RetainedChild>>> = OnceLock::new();
    RETAINED.get_or_init(|| Mutex::new(Vec::new()))
}
impl Drop for OwnedServerLease {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
            let reader = self.reader.take();
            let reason = self.identity.cleanup_failure.clone().unwrap_or_else(|| {
                "owner dropped without confirmed reap; resources retained".into()
            });
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    match tokio::time::timeout(Duration::from_secs(3), child.wait()).await {
                        Ok(Ok(_)) => {
                            if let Some(reader) = reader {
                                reader.abort();
                            }
                        }
                        _ => {
                            retained().lock().unwrap_or_else(|e| e.into_inner()).push(
                                RetainedChild {
                                    _child: child,
                                    _reader: reader,
                                    _reason: reason,
                                },
                            );
                        }
                    }
                });
            } else {
                retained()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(RetainedChild {
                        _child: child,
                        _reader: reader,
                        _reason: reason,
                    });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vf14_manifest_endpoint_has_no_custom_localhost_exemption() {
        assert!(endpoint("http://127.0.0.1:18084", "/v1/chat/completions").is_ok());
        for origin in [
            "http://localhost:18084",
            "http://127.0.0.2:18084",
            "https://127.0.0.1:18084",
            "http://u:p@127.0.0.1:18084",
            "http://127.0.0.1:18084?q=1",
            "http://127.0.0.1:18084#fragment",
            "http://127.0.0.1:18084/",
            "http://127.0.0.1:18085",
            "http://[::1]:18084",
        ] {
            assert!(
                endpoint(origin, "/v1/chat/completions").is_err(),
                "{origin}"
            );
        }
        assert!(endpoint("http://127.0.0.1:18084", "/api/chat").is_err());
    }
    #[test]
    fn vf14_file_pin_and_manifest_values_are_strict() {
        assert!(serde_json::from_str::<PinnedFile>(
            r#"{"path":"/fixture/x","bytes":1,"sha256":"x","unexpected":true}"#
        )
        .is_err());
        assert!(serde_json::from_str::<PinnedFile>(
            r#"{"path":"/fixture/x","path":"/fixture/y","bytes":1,"sha256":"x"}"#
        )
        .is_err());
        assert!(!digest(&"A".repeat(64)));
        assert!(!relative("../model"));
        assert!(!relative("/model"));
        assert!(!relative("x/../../y"));
        assert!(relative("mlx_lm/server.py"));
    }
    #[cfg(unix)]
    #[test]
    fn vf14_held_source_detects_in_place_change_and_replaced_path() {
        let owner = super::super::fixtures::FixtureDirectory::new();
        // This explicitly owned positive fixture must supply a canonical
        // locator; /var temporary aliases are not admissible production pins.
        let root = std::fs::canonicalize(&owner.0).unwrap();
        let path = root.join("source");
        std::fs::write(&path, b"fixture-a").unwrap();
        let pin = PinnedFile {
            path: path.clone(),
            bytes: 9,
            sha256: hex::encode(Sha256::digest(b"fixture-a")),
        };
        let held = HeldFile::open(&pin, false, &mut Progress::disabled())
            .unwrap()
            .0;
        std::fs::rename(&path, root.join("owned-original")).unwrap();
        std::fs::write(&path, b"fixture-a").unwrap();
        assert!(held.current().is_err());
        // Restore the source pathname, then change same-length bytes: ctime and
        // held version must reject even if a caller restores modified time.
        std::fs::remove_file(&path).unwrap();
        std::fs::rename(root.join("owned-original"), &path).unwrap();
        let held = HeldFile::open(&pin, false, &mut Progress::disabled())
            .unwrap()
            .0;
        std::fs::write(&path, b"fixture-b").unwrap();
        assert!(held.current().is_err());
        drop(held);
    }
    #[cfg(unix)]
    #[test]
    fn vf14_actual_held_file_progress_keeps_reads_hashes_and_verified_history_separate() {
        use super::super::admission_progress::tests::Sink;
        use super::super::admission_progress::{AdmissionState, Event};
        let sink = Sink::new();
        let path = sink.owner.0.canonicalize().unwrap().join("source");
        std::fs::write(&path, b"fixture-a").unwrap();
        let pin = PinnedFile {
            path: path.clone(),
            bytes: 9,
            sha256: hex::encode(Sha256::digest(b"fixture-a")),
        };
        let (result, records) = sink.collect(|| {
            let mut progress = sink.progress();
            let result = progress.scope(Scope::LocalCleanupAdmission, |p| {
                p.begin(Scope::ModelFiles);
                p.declare_files([Some(pin.bytes)]);
                p.next_entry();
                let (held, raw) = HeldFile::open(&pin, false, p)?;
                assert_eq!(raw.as_deref(), Some(b"fixture-a".as_slice()));
                held.current()?;
                p.end(Scope::ModelFiles, true);
                Ok(())
            });
            progress.finish(result)
        });
        result.unwrap();
        let final_record = records.last().unwrap();
        assert_eq!(final_record.event, Event::AdmissionEnd);
        assert_eq!(final_record.admission_state, Some(AdmissionState::Complete));
        let c = final_record.cleanup.unwrap();
        assert_eq!(
            (
                c.read_bytes,
                c.file_hash_feed_bytes,
                c.verified_file_bytes,
                c.verified_files
            ),
            (9, 9, 9, 1)
        );
        let sample = records
            .iter()
            .find(|r| r.event == Event::ScopeSample)
            .unwrap()
            .cleanup
            .unwrap();
        assert_eq!(
            (
                sample.read_bytes,
                sample.file_hash_feed_bytes,
                sample.verified_files
            ),
            (9, 9, 0)
        );
        // A later current-path failure does not erase already observed file-boundary history.
        let (result, records) = sink.collect(|| {
            let mut progress = sink.progress();
            let result = progress.scope(Scope::LocalCleanupAdmission, |p| {
                p.begin(Scope::ModelFiles);
                p.declare_files([Some(pin.bytes)]);
                p.next_entry();
                let (held, _) = HeldFile::open(&pin, false, p)?;
                std::fs::write(&path, b"fixture-b").unwrap();
                held.current()?;
                Ok(())
            });
            progress.finish(result)
        });
        assert_eq!(result.unwrap_err().code, "identity_mismatch");
        assert_eq!(
            records.last().unwrap().admission_state,
            Some(AdmissionState::InputError)
        );
        assert_eq!(records.last().unwrap().cleanup.unwrap().verified_files, 1);
    }
    #[cfg(unix)]
    #[test]
    fn vf14_actual_held_digest_failure_preserves_zero_verified_and_primary_when_progress_breaks() {
        use super::super::admission_progress::tests::Sink;
        use super::super::admission_progress::{AdmissionState, Event};
        let sink = Sink::new();
        let path = sink.owner.0.canonicalize().unwrap().join("source");
        std::fs::write(&path, b"fixture-b").unwrap();
        let pin = PinnedFile {
            path,
            bytes: 9,
            sha256: hex::encode(Sha256::digest(b"fixture-a")),
        };
        let (result, records) = sink.collect(|| {
            let mut progress = sink.progress();
            let result = progress.scope(Scope::LocalCleanupAdmission, |p| {
                p.begin(Scope::ModelFiles);
                p.declare_files([Some(pin.bytes)]);
                p.next_entry();
                HeldFile::open(&pin, false, p).map(|_| ())
            });
            progress.finish(result)
        });
        let original = result.unwrap_err();
        assert_eq!(original.code, "identity_mismatch");
        assert_eq!(original.artifact_path, pin.path.display().to_string());
        assert_eq!(original.safe_detail, "pinned file digest differs");
        let last = records.last().unwrap();
        assert_eq!(last.event, Event::AdmissionEnd);
        assert_eq!(last.admission_state, Some(AdmissionState::InputError));
        let c = last.cleanup.unwrap();
        assert_eq!(
            (c.read_bytes, c.file_hash_feed_bytes, c.verified_files),
            (9, 9, 0)
        );
        let expected = serde_json::to_value(&original).unwrap();
        let mut closed = Sink::new();
        let mut progress = closed.progress();
        closed.close_receiver();
        let result = progress.scope(Scope::LocalCleanupAdmission, |p| {
            p.begin(Scope::ModelFiles);
            p.declare_files([Some(pin.bytes)]);
            p.next_entry();
            HeldFile::open(&pin, false, p).map(|_| ())
        });
        let failure = progress.finish(result).unwrap_err();
        assert_eq!(serde_json::to_value(failure).unwrap(), expected);
    }
    #[test]
    fn vf14_namespace_rejects_new_imports_and_symlinks() {
        let owner = super::super::fixtures::FixtureDirectory::new();
        let names = ["module.py".to_owned()].into_iter().collect();
        std::fs::write(owner.0.join("module.py"), b"# fixture").unwrap();
        assert!(exact_inventory(&owner.0, &names).is_ok());
        std::fs::write(owner.0.join("extra.py"), b"# extra import input").unwrap();
        assert!(exact_inventory(&owner.0, &names).is_err());
        #[cfg(unix)]
        {
            std::fs::remove_file(owner.0.join("extra.py")).unwrap();
            std::os::unix::fs::symlink("module.py", owner.0.join("link.py")).unwrap();
            let linked_names = ["module.py".to_owned(), "link.py".to_owned()]
                .into_iter()
                .collect();
            assert!(exact_inventory(&owner.0, &linked_names).is_err());
        }
    }
}
