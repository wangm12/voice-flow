//! Managed storage for VoiceFlow's pinned native MLX cleanup model.
//!
//! The model is identified by a repository revision and exact file digests. A
//! path alone never creates a runtime handle: `resolve_ready` checks the
//! inventory, model metadata, and every file before returning one.

use crate::mlx_diagnostics::{self, EventName, Severity, Target};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub const MODEL_ID: &str = "qwen3_8_27b_mlx_6bit";
pub const MODEL_LABEL: &str = "Qwen 3.8 27B · MLX 6-bit";
pub const REPOSITORY: &str = "lmstudio-community/Qwen3.8-27B-MLX-6bit";
pub const REVISION: &str = "05f38c1f39e2f39619653616fe8e96a5d676358e";
pub const MANIFEST_SHA256: &str =
    "f67d38ec1261b4d693cee8c006d2c56007e6919a290508266705ba4f571c9ed2";
pub const MODEL_BYTES: u64 = 22_804_836_386;
const INSTALLED_MANIFEST: &str = ".voiceflow-cleanup-model.json";
const STAGING_DIR: &str = ".qwen3-8-27b-mlx-6bit.partial";
const DURABLE_CACHE_DIR: &str = ".voiceflow-mlx-cache";
const DURABLE_PROOF_FILE: &str = "model-verification.json";
const DURABLE_PROOF_SCHEMA: u32 = 1;
const MAX_DURABLE_PROOF_BYTES: u64 = 64 * 1024;
static DURABLE_PROOF_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelFile {
    pub path: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

pub const MODEL_FILES: &[ModelFile] = &[
    ModelFile {
        path: "chat_template.jinja",
        bytes: 8_952,
        sha256: "c3cf9e34abf4f9e36c2d72165aa9c132d3e2a725b6c2586aaa3a8af9d7a81041",
    },
    ModelFile {
        path: "config.json",
        bytes: 4_932,
        sha256: "f2490d237e42fc8418ee62b7416e360785b7de308f43d8f794ccf4f185bdeec9",
    },
    ModelFile {
        path: "generation_config.json",
        bytes: 202,
        sha256: "e70c136c1b78ddc1fb0905bac8e733a4dc448d4f852a5dd75143fffc70be550e",
    },
    ModelFile {
        path: "model-00001-of-00005.safetensors",
        bytes: 5_362_726_126,
        sha256: "39a48bfd79609b341f4ed420cdc92e3edce316cd3f5ef7db2dcd371f48c90588",
    },
    ModelFile {
        path: "model-00002-of-00005.safetensors",
        bytes: 5_344_281_968,
        sha256: "d9011deb397afa986754a8abe3872b87a21c2bd6d7f9ed50bd8cba3890b782f9",
    },
    ModelFile {
        path: "model-00003-of-00005.safetensors",
        bytes: 5_331_479_330,
        sha256: "760d392182ee77093df96328110596e230a9f4cab37febd70ae29173f7ff7e01",
    },
    ModelFile {
        path: "model-00004-of-00005.safetensors",
        bytes: 5_331_479_394,
        sha256: "513fbeb37662c2b6020ebe4d61e5e9192f7abb5970aef0cd291b7a55f445c23e",
    },
    ModelFile {
        path: "model-00005-of-00005.safetensors",
        bytes: 1_407_912_947,
        sha256: "6ed59540691cfb8f8adeaa186506d3fc44b0f0ce392c7caf59116bb6ae580c01",
    },
    ModelFile {
        path: "model.safetensors.index.json",
        bytes: 218_281,
        sha256: "dafc019bb8416ae2a289f24c598473b5531b14ca87a0cabfd9b0d6aec67bffc1",
    },
    ModelFile {
        path: "preprocessor_config.json",
        bytes: 390,
        sha256: "27225450ac9c6529872ee1924fcb0962ff5634834f817040f444118116f4e516",
    },
    ModelFile {
        path: "processor_config.json",
        bytes: 991,
        sha256: "45fc17c8dd2474af6b493b52483c26c0584b0082d368c480f9fa611e73070040",
    },
    ModelFile {
        path: "tokenizer.json",
        bytes: 19_989_325,
        sha256: "06b9509352d2af50381ab2247e083b80d32d5c0aba91c272ca9ff729b6a0e523",
    },
    ModelFile {
        path: "tokenizer_config.json",
        bytes: 10_404,
        sha256: "47a7a8371379b07650b22f3c8a78c32fed85c4e35ea88e3da0c4b7a7611e8082",
    },
    ModelFile {
        path: "video_preprocessor_config.json",
        bytes: 385,
        sha256: "7768af27c1fafa9cc9011c1dc20067e03f8915e03b63504550e11d5066986d13",
    },
    ModelFile {
        path: "vocab.json",
        bytes: 6_722_759,
        sha256: "ce99b4cb2983d118806ce0a8b777a35b093e2000a503ebde25853284c9dfa003",
    },
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupModelStatus {
    pub id: String,
    pub label: String,
    pub repository: String,
    pub revision: String,
    pub state: String,
    pub operation: Option<String>,
    pub source_kind: Option<String>,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub error: Option<String>,
    pub last_operation: Option<String>,
    pub last_operation_error: Option<String>,
    pub managed: bool,
    pub partial_managed: bool,
}

impl CleanupModelStatus {
    fn initial() -> Self {
        Self {
            id: MODEL_ID.to_owned(),
            label: MODEL_LABEL.to_owned(),
            repository: REPOSITORY.to_owned(),
            revision: REVISION.to_owned(),
            state: "missing".to_owned(),
            operation: None,
            source_kind: None,
            bytes_done: 0,
            bytes_total: MODEL_BYTES,
            error: None,
            last_operation: None,
            last_operation_error: None,
            managed: false,
            partial_managed: false,
        }
    }
}

#[derive(Clone, Default)]
pub struct Manager {
    inner: Arc<Mutex<ManagerInner>>,
    leases: Arc<AtomicUsize>,
    revision: tokio::sync::watch::Sender<u64>,
}

impl Manager {
    pub(crate) fn subscribe_revision(&self) -> tokio::sync::watch::Receiver<u64> {
        self.revision.subscribe()
    }

    fn notify_revision(&self) {
        self.revision
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    pub(crate) fn notify_application_exit(&self) {
        self.notify_revision();
    }
}

struct ManagerInner {
    status: CleanupModelStatus,
    job: Option<ActiveJob>,
    generation: u64,
    deleting: bool,
    trusted: Option<TrustedInventory>,
    status_root_for_commit: Option<PathBuf>,
    last_operation: Option<String>,
    last_operation_error: Option<String>,
}

struct ActiveJob {
    generation: u64,
    cancel: CancellationToken,
    bytes_done: Arc<AtomicU64>,
}

#[derive(Clone)]
struct TrustedInventory {
    root: PathBuf,
    files: Vec<VerifiedFile>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct InventoryVerification {
    pub(crate) cache: &'static str,
    pub(crate) cache_reason: &'static str,
    pub(crate) file_count: usize,
    pub(crate) hashed_files: usize,
    pub(crate) hashed_bytes: u64,
    pub(crate) elapsed_seconds: f64,
    pub(crate) sha256_backend: &'static str,
    pub(crate) sha256_hardware_available: Option<bool>,
}

struct VerificationTrace<'a> {
    request_id: &'a str,
    child_generation: u64,
    session_generation: Option<u64>,
}

fn emit_inventory_verification_failed(trace: &VerificationTrace<'_>) {
    mlx_diagnostics::event(
        EventName::CleanupModelInventory,
        Severity::Warn,
        Target::ModelStore,
    )
    .request_id(trace.request_id)
    .child_generation(trace.child_generation)
    .session_generation(trace.session_generation)
    .outcome("failed")
    .stage("inventory_verification")
    .reason("verification_rejected")
    .emit();
}

fn emit_inventory_verified(trace: &VerificationTrace<'_>, facts: InventoryVerification) {
    mlx_diagnostics::event(
        EventName::CleanupModelInventory,
        Severity::Info,
        Target::ModelStore,
    )
    .request_id(trace.request_id)
    .child_generation(trace.child_generation)
    .session_generation(trace.session_generation)
    .outcome("verified")
    .inventory(
        facts.cache,
        facts.cache_reason,
        facts.file_count,
        facts.hashed_files,
        facts.hashed_bytes,
        facts.sha256_backend,
        facts.sha256_hardware_available,
    )
    .validation_ms(facts.elapsed_seconds * 1_000.0)
    .emit();
}

#[derive(Default)]
struct VerificationProgress {
    hashed_files: usize,
    hashed_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifiedFile {
    path: String,
    bytes: u64,
    sha256: String,
    identity: FileIdentity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    bytes: u64,
    modified_seconds: u64,
    modified_nanos: u32,
    changed_seconds: u64,
    changed_nanos: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct InstalledManifest {
    schema_version: u32,
    id: String,
    repository: String,
    revision: String,
    manifest_sha256: String,
    files: Vec<VerifiedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DurableFileIdentity {
    file: FileIdentity,
    flags: Option<u32>,
    mode: u32,
    links: u64,
    owner: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct DurableProofFile {
    path: String,
    bytes: u64,
    sha256: String,
    identity: DurableFileIdentity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DurableVerificationProof {
    schema_version: u32,
    model_id: String,
    repository: String,
    revision: String,
    manifest_sha256: String,
    models_root: String,
    managed_directory: String,
    managed_directory_identity: DurableFileIdentity,
    installed_manifest_sha256: String,
    installed_manifest_identity: DurableFileIdentity,
    files: Vec<DurableProofFile>,
}

#[derive(Clone, Copy)]
struct DurableProofBinding<'a> {
    model_id: &'a str,
    repository: &'a str,
    revision: &'a str,
    manifest_sha256: &'a str,
    files: &'a [ModelFile],
}

fn production_durable_proof_binding() -> DurableProofBinding<'static> {
    DurableProofBinding {
        model_id: MODEL_ID,
        repository: REPOSITORY,
        revision: REVISION,
        manifest_sha256: MANIFEST_SHA256,
        files: MODEL_FILES,
    }
}

struct ModelLease {
    counter: Arc<AtomicUsize>,
}

impl Drop for ModelLease {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Only `Manager::resolve_ready` can create a verified model handle.
#[derive(Clone)]
pub struct ResolvedCleanupModel {
    id: &'static str,
    repository: &'static str,
    revision: &'static str,
    manifest_sha256: &'static str,
    managed_dir: PathBuf,
    files: Arc<[VerifiedFile]>,
    _lease: Arc<ModelLease>,
}

impl ResolvedCleanupModel {
    pub(crate) fn id(&self) -> &'static str {
        self.id
    }
    pub(crate) fn repository(&self) -> &'static str {
        self.repository
    }
    pub(crate) fn revision(&self) -> &'static str {
        self.revision
    }
    pub(crate) fn manifest_sha256(&self) -> &'static str {
        self.manifest_sha256
    }
    pub(crate) fn managed_dir(&self) -> &Path {
        &self.managed_dir
    }
    pub(crate) fn files(&self) -> &[VerifiedFile] {
        &self.files
    }
}

#[cfg(test)]
const TINY_RUNTIME_FILES: &[ModelFile] = &[
    ModelFile {
        path: "first.safetensors",
        bytes: 5,
        sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
    },
    ModelFile {
        path: "second.safetensors",
        bytes: 5,
        sha256: "486ea46224d1bb4fb680f34f7c9ad96a8f24ec88be73ea8e5a6c65260e9cb8a7",
    },
];

#[cfg(test)]
fn seed_tiny_fixture(root: &Path, fault: TinyFixtureFault) -> Result<PathBuf, String> {
    let files = TINY_RUNTIME_FILES;
    let directory = managed_dir(root);
    fs::create_dir_all(&directory).map_err(|_| "fixture directory creation failed".to_owned())?;
    fs::write(directory.join(files[0].path), b"hello")
        .map_err(|_| "fixture file write failed".to_owned())?;
    fs::write(directory.join(files[1].path), b"world")
        .map_err(|_| "fixture file write failed".to_owned())?;
    let mut seeded_files = Vec::with_capacity(files.len());
    for expected in files {
        seeded_files.push(VerifiedFile {
            path: expected.path.to_owned(),
            bytes: expected.bytes,
            sha256: expected.sha256.to_owned(),
            identity: file_identity(&directory.join(expected.path))?,
        });
    }
    let manifest = InstalledManifest {
        schema_version: 1,
        id: MODEL_ID.to_owned(),
        repository: REPOSITORY.to_owned(),
        revision: REVISION.to_owned(),
        manifest_sha256: MANIFEST_SHA256.to_owned(),
        files: seeded_files,
    };
    fs::write(
        directory.join(INSTALLED_MANIFEST),
        serde_json::to_vec(&manifest).map_err(|_| "fixture manifest encoding failed")?,
    )
    .map_err(|_| "fixture manifest write failed".to_owned())?;
    match fault {
        TinyFixtureFault::None => {}
        TinyFixtureFault::InventoryReject => {
            let mut altered = manifest.files.clone();
            altered.pop();
            let invalid = InstalledManifest {
                files: altered,
                ..manifest
            };
            fs::write(
                directory.join(INSTALLED_MANIFEST),
                serde_json::to_vec(&invalid).map_err(|_| "fixture manifest encoding failed")?,
            )
            .map_err(|_| "fixture manifest write failed".to_owned())?;
        }
        TinyFixtureFault::HashReject => {
            fs::write(directory.join(files[0].path), b"HELLO")
                .map_err(|_| "fixture file write failed".to_owned())?;
        }
    }
    Ok(directory)
}

#[cfg(test)]
pub(crate) fn tiny_verified_fixture(
    root: &Path,
    manager: &Manager,
    request_id: &str,
    child_generation: u64,
    session_generation: Option<u64>,
    fault: TinyFixtureFault,
) -> Result<(ResolvedCleanupModel, InventoryVerification), String> {
    let directory = seed_tiny_fixture(root, fault)?;
    let started = Instant::now();
    let mut progress = VerificationProgress::default();
    let trace = VerificationTrace {
        request_id,
        child_generation,
        session_generation,
    };
    let verified = match verify_directory_with_binding(
        &directory,
        Some(&trace),
        &mut progress,
        tiny_test_durable_proof_binding(),
    ) {
        Ok(verified) => verified,
        Err(error) => {
            emit_inventory_verification_failed(&trace);
            return Err(error);
        }
    };
    let facts = InventoryVerification {
        cache: "miss",
        cache_reason: "test_fixture",
        file_count: verified.len(),
        hashed_files: progress.hashed_files,
        hashed_bytes: progress.hashed_bytes,
        elapsed_seconds: started.elapsed().as_secs_f64(),
        sha256_backend: "rust",
        sha256_hardware_available: None,
    };
    emit_inventory_verified(&trace, facts);
    let verified: Arc<[VerifiedFile]> = verified.into();
    manager.leases.fetch_add(1, Ordering::AcqRel);
    let model = ResolvedCleanupModel {
        id: MODEL_ID,
        repository: REPOSITORY,
        revision: REVISION,
        manifest_sha256: MANIFEST_SHA256,
        managed_dir: directory,
        files: verified,
        _lease: Arc::new(ModelLease {
            counter: Arc::clone(&manager.leases),
        }),
    };
    Ok((model, facts))
}

#[cfg(test)]
#[derive(Clone, Copy, Default)]
pub(crate) enum TinyFixtureFault {
    #[default]
    None,
    InventoryReject,
    HashReject,
}

#[cfg(test)]
fn tiny_test_durable_proof_binding() -> DurableProofBinding<'static> {
    DurableProofBinding {
        model_id: MODEL_ID,
        repository: REPOSITORY,
        revision: REVISION,
        manifest_sha256: MANIFEST_SHA256,
        files: TINY_RUNTIME_FILES,
    }
}

impl Manager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ManagerInner {
                status: CleanupModelStatus::initial(),
                job: None,
                generation: 0,
                deleting: false,
                trusted: None,
                status_root_for_commit: None,
                last_operation: None,
                last_operation_error: None,
            })),
            leases: Arc::new(AtomicUsize::new(0)),
            revision: tokio::sync::watch::channel(0).0,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_lease_count(&self) -> usize {
        self.leases.load(Ordering::Acquire)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ManagerInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn status(&self, models_root: &Path) -> CleanupModelStatus {
        let (mut status, last_operation, last_operation_error) = {
            let inner = self.lock();
            if inner.status.operation.is_some() || inner.deleting {
                let mut status = inner.status.clone();
                status.bytes_done = inner.job.as_ref().map_or(status.bytes_done, |job| {
                    job.bytes_done.load(Ordering::Relaxed)
                });
                status.last_operation = inner.last_operation.clone();
                status.last_operation_error = inner.last_operation_error.clone();
                return status;
            }
            (
                inner.status.clone(),
                inner.last_operation.clone(),
                inner.last_operation_error.clone(),
            )
        };
        status.last_operation = last_operation;
        status.last_operation_error = last_operation_error;
        match read_installed_inventory(&managed_dir(models_root)) {
            Ok(Some(files)) => {
                status.managed = true;
                status.partial_managed = path_entry_exists(&staging_dir(models_root));
                status.bytes_done = MODEL_BYTES;
                let canonical_root = canonical_or_absolute(&managed_dir(models_root));
                let trusted = self.lock().trusted.clone().filter(|inventory| {
                    inventory.root == canonical_root && inventory.files == files
                });
                status.state = if trusted.is_some() {
                    "ready"
                } else {
                    "installed"
                }
                .to_owned();
                status.error = None;
            }
            Ok(None) => {
                status.managed = false;
                status.partial_managed = path_entry_exists(&staging_dir(models_root));
                status.bytes_done = if status.partial_managed {
                    partial_bytes(&staging_dir(models_root))
                } else {
                    0
                };
                status.state = if status.partial_managed {
                    "partial"
                } else {
                    "missing"
                }
                .to_owned();
                status.error = None;
            }
            Err(error) => {
                status.managed = path_entry_exists(&managed_dir(models_root));
                status.partial_managed = path_entry_exists(&staging_dir(models_root));
                status.state = "invalid".to_owned();
                status.error = Some(error);
            }
        }
        status
    }

    pub(crate) fn resolve_ready_for_runtime(
        &self,
        models_root: &Path,
        request_id: &str,
        child_generation: u64,
        session_generation: Option<u64>,
    ) -> Result<(ResolvedCleanupModel, InventoryVerification), String> {
        let result = self.resolve_ready_inner(
            models_root,
            Some(VerificationTrace {
                request_id,
                child_generation,
                session_generation,
            }),
        );
        if result.is_err() {
            emit_inventory_verification_failed(&VerificationTrace {
                request_id,
                child_generation,
                session_generation,
            });
        }
        result
    }

    fn resolve_ready_inner(
        &self,
        models_root: &Path,
        trace: Option<VerificationTrace<'_>>,
    ) -> Result<(ResolvedCleanupModel, InventoryVerification), String> {
        self.resolve_ready_inner_with_binding(
            models_root,
            trace,
            production_durable_proof_binding(),
        )
    }

    fn resolve_ready_inner_with_binding(
        &self,
        models_root: &Path,
        trace: Option<VerificationTrace<'_>>,
        binding: DurableProofBinding<'static>,
    ) -> Result<(ResolvedCleanupModel, InventoryVerification), String> {
        let verification_started = Instant::now();
        let directory = managed_dir(models_root);
        let root = canonical_or_absolute(&directory);
        let lease = {
            let inner = self.lock();
            if inner.deleting {
                return Err("cleanup model is being deleted".to_owned());
            }
            if inner.job.is_some() {
                return Err("cleanup model storage operation is still running".to_owned());
            }
            self.leases.fetch_add(1, Ordering::AcqRel);
            Arc::new(ModelLease {
                counter: Arc::clone(&self.leases),
            })
        };
        let cached = self.lock().trusted.clone();
        let mut progress = VerificationProgress::default();
        let durable = read_durable_proof(models_root);
        let (verified, cache, mut cache_reason, write_durable_proof) = if let Some(proof) =
            durable.proof.as_ref()
        {
            match verified_files_from_durable_proof(proof, models_root, &directory, binding) {
                Some(files) => (files, "hit", "durable_identity_match", false),
                None => (
                    verify_directory_with_binding(
                        &directory,
                        trace.as_ref(),
                        &mut progress,
                        binding,
                    )?,
                    "miss",
                    "durable_identity_changed",
                    true,
                ),
            }
        } else if let Some(inventory) = cached {
            if inventory.root != root {
                (
                    verify_directory_with_binding(
                        &directory,
                        trace.as_ref(),
                        &mut progress,
                        binding,
                    )?,
                    "miss",
                    "managed_root_changed",
                    true,
                )
            } else {
                match read_installed_inventory_with_binding(&directory, binding) {
                    Ok(Some(files)) if files == inventory.files => {
                        (files, "hit", "trusted_file_identity_match", true)
                    }
                    Ok(_) | Err(_) => (
                        verify_directory_with_binding(
                            &directory,
                            trace.as_ref(),
                            &mut progress,
                            binding,
                        )?,
                        "miss",
                        "installed_inventory_changed_or_unreadable",
                        true,
                    ),
                }
            }
        } else {
            (
                verify_directory_with_binding(&directory, trace.as_ref(), &mut progress, binding)?,
                "miss",
                durable.reason,
                true,
            )
        };
        if write_durable_proof
            && write_durable_proof_record(models_root, &directory, &verified, binding).is_err()
        {
            // A cache write failure is only a future cache miss. The model has
            // already passed the full verifier (or the same-process verifier).
            cache_reason = "durable_proof_write_failed";
        }
        {
            let mut inner = self.lock();
            if inner.deleting || inner.job.is_some() {
                return Err("cleanup model changed while it was being verified".to_owned());
            }
            inner.trusted = Some(TrustedInventory {
                root,
                files: verified.clone(),
            });
        }
        let (sha256_backend, sha256_hardware_available) = sha256_backend_facts();
        let facts = InventoryVerification {
            cache,
            cache_reason,
            file_count: verified.len(),
            hashed_files: progress.hashed_files,
            hashed_bytes: progress.hashed_bytes,
            elapsed_seconds: verification_started.elapsed().as_secs_f64(),
            sha256_backend,
            sha256_hardware_available,
        };
        if let Some(trace) = trace.as_ref() {
            emit_inventory_verified(trace, facts);
        }
        Ok((
            ResolvedCleanupModel {
                id: binding.model_id,
                repository: binding.repository,
                revision: binding.revision,
                manifest_sha256: binding.manifest_sha256,
                managed_dir: directory,
                files: Arc::from(verified),
                _lease: lease,
            },
            facts,
        ))
    }

    pub fn start_import(&self, models_root: PathBuf, selected_dir: PathBuf) -> Result<(), String> {
        let selected_meta = fs::symlink_metadata(&selected_dir)
            .map_err(|error| format!("cannot open selected model folder: {error}"))?;
        if selected_meta.file_type().is_symlink() || !selected_meta.is_dir() {
            return Err("selected model path must be a real folder".to_owned());
        }
        let selected = fs::canonicalize(&selected_dir)
            .map_err(|error| format!("cannot open selected model folder: {error}"))?;
        let managed = canonical_or_absolute(&managed_dir(&models_root));
        let staging = canonical_or_absolute(&staging_dir(&models_root));
        if paths_overlap(&selected, &managed) || paths_overlap(&selected, &staging) {
            return Err("the selected folder overlaps VoiceFlow model storage; choose the original model folder".to_owned());
        }
        let (generation, cancel, bytes_done) =
            self.begin_operation(&models_root, "importing", "import")?;
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let result = import_model(&models_root, &selected, &cancel, &bytes_done).await;
            manager.finish_operation(generation, result.as_ref().err().cloned(), result.ok());
        });
        Ok(())
    }

    pub fn start_download(&self, models_root: PathBuf) -> Result<(), String> {
        let (generation, cancel, bytes_done) =
            self.begin_operation(&models_root, "downloading", "download")?;
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let result = download_model(&models_root, &cancel, &bytes_done).await;
            manager.finish_operation(generation, result.as_ref().err().cloned(), result.ok());
        });
        Ok(())
    }

    pub fn cancel(&self) -> bool {
        let inner = self.lock();
        if let Some(job) = &inner.job {
            job.cancel.cancel();
            return true;
        }
        false
    }

    pub async fn delete_with_unload<F, Fut>(
        &self,
        models_root: PathBuf,
        unload: F,
    ) -> Result<(), String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<(), String>>,
    {
        {
            let mut inner = self.lock();
            if inner.job.is_some() || inner.deleting {
                return Err("cleanup model storage is busy".to_owned());
            }
            inner.deleting = true;
            inner.status.operation = Some("delete".to_owned());
            inner.status.state = "deleting".to_owned();
            inner.status.error = None;
        }
        self.notify_revision();
        let unload_result = unload().await;
        if let Err(error) = unload_result {
            self.finish_delete(Some(error.clone()));
            return Err(error);
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while self.leases.load(Ordering::Acquire) != 0 {
            if tokio::time::Instant::now() >= deadline {
                let error = "cleanup model is still in use after unload".to_owned();
                self.finish_delete(Some(error.clone()));
                return Err(error);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let directory = managed_dir(&models_root);
        let result = validate_model_parent(&models_root)
            .and_then(|()| remove_path(&directory))
            .and_then(|()| remove_path(&staging_dir(&models_root)))
            .and_then(|()| remove_path(&durable_cache_dir(&models_root)));
        match result {
            Ok(()) => {
                let mut inner = self.lock();
                inner.deleting = false;
                inner.trusted = None;
                inner.last_operation = Some("delete".to_owned());
                inner.last_operation_error = None;
                inner.status_root_for_commit = None;
                inner.status = CleanupModelStatus::initial();
                inner.status.last_operation = inner.last_operation.clone();
                drop(inner);
                self.notify_revision();
                Ok(())
            }
            Err(error) => {
                self.finish_delete(Some(error.clone()));
                Err(error)
            }
        }
    }

    fn begin_operation(
        &self,
        models_root: &Path,
        state: &str,
        operation: &str,
    ) -> Result<(u64, CancellationToken, Arc<AtomicU64>), String> {
        let mut inner = self.lock();
        if inner.deleting || inner.job.is_some() {
            return Err("cleanup model storage is busy".to_owned());
        }
        if self.leases.load(Ordering::Acquire) != 0 {
            return Err(
                "cleanup model is in use; unload it before importing or downloading".to_owned(),
            );
        }
        if path_entry_exists(&managed_dir(models_root)) {
            return Err(
                "a managed model folder already exists; delete it before replacing or repairing"
                    .to_owned(),
            );
        }
        inner.generation = inner.generation.wrapping_add(1);
        let generation = inner.generation;
        let cancel = CancellationToken::new();
        let bytes_done = Arc::new(AtomicU64::new(0));
        inner.job = Some(ActiveJob {
            generation,
            cancel: cancel.clone(),
            bytes_done: Arc::clone(&bytes_done),
        });
        inner.status = CleanupModelStatus::initial();
        inner.status.state = state.to_owned();
        inner.status.operation = Some(operation.to_owned());
        inner.status.bytes_done = 0;
        inner.status.error = None;
        inner.status_root_for_commit = Some(managed_dir(models_root));
        inner.last_operation = None;
        inner.last_operation_error = None;
        inner.status.last_operation = None;
        inner.status.last_operation_error = None;
        drop(inner);
        self.notify_revision();
        Ok((generation, cancel, bytes_done))
    }

    fn finish_operation(
        &self,
        generation: u64,
        error: Option<String>,
        files: Option<Vec<VerifiedFile>>,
    ) {
        let mut inner = self.lock();
        if inner
            .job
            .as_ref()
            .is_none_or(|job| job.generation != generation)
        {
            return;
        }
        let bytes_done = inner
            .job
            .as_ref()
            .map_or(0, |job| job.bytes_done.load(Ordering::Relaxed));
        let operation = inner
            .status
            .operation
            .clone()
            .unwrap_or_else(|| "model operation".to_owned());
        inner.job = None;
        inner.last_operation = Some(operation.clone());
        inner.last_operation_error = error.clone();
        if let Some(files) = files {
            let root = inner.status_root_for_commit.take();
            inner.trusted = root.map(|root| TrustedInventory {
                root: canonical_or_absolute(&root),
                files,
            });
            inner.status.state = "ready".to_owned();
            inner.status.managed = true;
            inner.status.bytes_done = MODEL_BYTES;
            inner.status.source_kind = Some(match inner.status.operation.as_deref() {
                Some("import") => "import".to_owned(),
                _ => "download".to_owned(),
            });
            inner.status.error = None;
            inner.status.last_operation = Some(operation);
            inner.status.last_operation_error = None;
        } else {
            inner.status.bytes_done = bytes_done;
            inner.status.state = if error.as_deref() == Some("download cancelled")
                || error.as_deref() == Some("import cancelled")
            {
                "cancelled"
            } else {
                "error"
            }
            .to_owned();
            inner.status.error = error;
            inner.status.last_operation = Some(operation);
            inner.status.last_operation_error = inner.last_operation_error.clone();
        }
        inner.status.operation = None;
        if inner.status.state != "ready" {
            inner.status_root_for_commit = None;
        }
        drop(inner);
        self.notify_revision();
    }

    fn finish_delete(&self, error: Option<String>) {
        let mut inner = self.lock();
        inner.deleting = false;
        inner.status.operation = None;
        inner.last_operation = Some("delete".to_owned());
        inner.last_operation_error = error.clone();
        inner.status.last_operation = inner.last_operation.clone();
        inner.status.last_operation_error = error;
        inner.status.error = None;
        drop(inner);
        self.notify_revision();
    }
}

impl Default for ManagerInner {
    fn default() -> Self {
        Self {
            status: CleanupModelStatus::initial(),
            job: None,
            generation: 0,
            deleting: false,
            trusted: None,
            status_root_for_commit: None,
            last_operation: None,
            last_operation_error: None,
        }
    }
}

fn managed_dir(models_root: &Path) -> PathBuf {
    models_root.join("cleanup").join(MODEL_ID)
}

fn staging_dir(models_root: &Path) -> PathBuf {
    models_root.join("cleanup").join(STAGING_DIR)
}

fn durable_cache_dir(models_root: &Path) -> PathBuf {
    models_root.join("cleanup").join(DURABLE_CACHE_DIR)
}

fn durable_proof_path(models_root: &Path) -> PathBuf {
    durable_cache_dir(models_root).join(DURABLE_PROOF_FILE)
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn path_entry_exists(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

fn partial_bytes(staging: &Path) -> u64 {
    fs::read_dir(staging)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let metadata = fs::symlink_metadata(entry.path()).ok()?;
            (metadata.is_file() && !metadata.file_type().is_symlink()).then_some(metadata.len())
        })
        .fold(0u64, u64::saturating_add)
}

fn canonical_or_absolute(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name() else {
            return absolute;
        };
        missing.push(name.to_os_string());
        let Some(parent) = ancestor.parent() else {
            return absolute;
        };
        ancestor = parent;
    }
    let mut canonical = fs::canonicalize(ancestor).unwrap_or_else(|_| ancestor.to_path_buf());
    for component in missing.into_iter().rev() {
        canonical.push(component);
    }
    canonical
}

fn validate_manifest_path(path: &str) -> Result<(), String> {
    let candidate = Path::new(path);
    if candidate.is_absolute()
        || candidate.components().count() != 1
        || !candidate
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err("pinned model manifest contains an invalid path".to_owned());
    }
    Ok(())
}

fn hash_file(path: &Path, expected: &ModelFile) -> Result<FileIdentity, String> {
    hash_file_with_progress(path, expected, &mut VerificationProgress::default())
}

fn hash_file_with_progress(
    path: &Path,
    expected: &ModelFile,
    progress: &mut VerificationProgress,
) -> Result<FileIdentity, String> {
    let before = file_identity(path)?;
    if before.bytes != expected.bytes {
        return Err(format!("model file size mismatch: {}", expected.path));
    }
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 256 * 1024];
    let mut file_counted = false;
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        if !file_counted {
            progress.hashed_files = progress.hashed_files.saturating_add(1);
            file_counted = true;
        }
        progress.hashed_bytes = progress.hashed_bytes.saturating_add(count as u64);
        hasher.update(&buffer[..count]);
    }
    if hex::encode(hasher.finalize()) != expected.sha256 {
        return Err(format!("model file checksum mismatch: {}", expected.path));
    }
    let after = file_identity(path)?;
    if before != after {
        return Err(format!(
            "model file changed while being verified: {}",
            expected.path
        ));
    }
    Ok(after)
}

fn file_identity(path: &Path) -> Result<FileIdentity, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "model asset is not a regular file: {}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("unknown")
        ));
    }
    Ok(file_identity_from_metadata(&metadata))
}

fn file_identity_from_metadata(metadata: &fs::Metadata) -> FileIdentity {
    #[cfg(unix)]
    let (device, inode, modified_seconds, modified_nanos, changed_seconds, changed_nanos) = {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.dev(),
            metadata.ino(),
            metadata.mtime() as u64,
            metadata.mtime_nsec() as u32,
            metadata.ctime() as u64,
            metadata.ctime_nsec() as u32,
        )
    };
    #[cfg(not(unix))]
    let (device, inode, modified_seconds, modified_nanos, changed_seconds, changed_nanos) = {
        let modified = metadata
            .modified()
            .unwrap_or(std::time::UNIX_EPOCH)
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        (0, 0, modified.as_secs(), modified.subsec_nanos(), 0, 0)
    };
    FileIdentity {
        device,
        inode,
        bytes: metadata.len(),
        modified_seconds,
        modified_nanos,
        changed_seconds,
        changed_nanos,
    }
}

pub(crate) fn durable_file_identity(path: &Path) -> Result<DurableFileIdentity, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !(metadata.is_file() || metadata.is_dir()) {
        return Err("cache proof target must be a real file or directory".to_owned());
    }
    #[cfg(target_os = "macos")]
    let flags = Some(mac_file_flags(path)?);
    #[cfg(not(target_os = "macos"))]
    let flags = None;
    #[cfg(unix)]
    let (mode, links, owner) = {
        use std::os::unix::fs::MetadataExt;
        (metadata.mode() & 0o7777, metadata.nlink(), metadata.uid())
    };
    #[cfg(not(unix))]
    let (mode, links, owner) = (0, 0, 0);
    Ok(DurableFileIdentity {
        file: file_identity_from_metadata(&metadata),
        flags,
        mode,
        links,
        owner,
    })
}

/// Returns a bounded, app-private proof-record location adjacent to (not under)
/// the model root so publishing the record cannot change the root identity.
pub(crate) fn local_model_proof_path(models_root: &Path, model_id: &str) -> PathBuf {
    let parent = models_root.parent().unwrap_or(models_root);
    parent.join(".voiceflow-model-integrity").join(format!(
        "{}.json",
        hex::encode(Sha256::digest(model_id.as_bytes()))
    ))
}

/// A private proof read is advisory: any unavailable, unsafe, or oversized
/// record is a cache miss and the caller must perform full verification.
pub(crate) fn read_private_proof_record(path: &Path, max_bytes: u64) -> Option<Vec<u8>> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (path, max_bytes);
        None
    }
    #[cfg(target_os = "macos")]
    {
        let parent = path.parent()?;
        let directory = fs::symlink_metadata(parent).ok()?;
        if directory.file_type().is_symlink()
            || !directory.is_dir()
            || !private_directory_metadata(&directory)
        {
            return None;
        }
        let metadata = fs::symlink_metadata(path).ok()?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || !private_file_metadata(&metadata)
            || metadata.len() > max_bytes
        {
            return None;
        }
        fs::read(path)
            .ok()
            .filter(|bytes| bytes.len() as u64 <= max_bytes)
    }
}

/// Publishes one bounded proof record with the existing private atomic writer.
pub(crate) fn write_private_proof_record(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() as u64 > MAX_DURABLE_PROOF_BYTES {
        return Err("durable model proof exceeds its size bound".to_owned());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Err("durable identity cache is unavailable on this target".to_owned())
    }
    #[cfg(target_os = "macos")]
    {
        atomic_write_private(path, bytes)
    }
}

#[cfg(target_os = "macos")]
fn mac_file_flags(path: &Path) -> Result<u32, String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "cache proof path contains a NUL byte".to_owned())?;
    let mut status = std::mem::MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: `status` is writable storage and `path` is a valid C string.
    let result = unsafe { libc::lstat(path.as_ptr(), status.as_mut_ptr()) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: lstat returned success and initialized the stat structure.
    Ok(unsafe { status.assume_init() }.st_flags)
}

#[derive(Clone)]
struct DurableProofRead {
    proof: Option<DurableVerificationProof>,
    reason: &'static str,
}

fn read_durable_proof(models_root: &Path) -> DurableProofRead {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = models_root;
        return DurableProofRead {
            proof: None,
            reason: "durable_cache_target_unsupported",
        };
    }
    #[cfg(target_os = "macos")]
    {
        let cache_dir = durable_cache_dir(models_root);
        let path = durable_proof_path(models_root);
        let directory = match fs::symlink_metadata(&cache_dir) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => metadata,
            Ok(_) => {
                return DurableProofRead {
                    proof: None,
                    reason: "durable_cache_directory_invalid",
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return DurableProofRead {
                    proof: None,
                    reason: "durable_cache_missing",
                };
            }
            Err(_) => {
                return DurableProofRead {
                    proof: None,
                    reason: "durable_cache_unreadable",
                };
            }
        };
        if !private_directory_metadata(&directory) {
            return DurableProofRead {
                proof: None,
                reason: "durable_cache_permissions_invalid",
            };
        }
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
            Ok(_) => {
                return DurableProofRead {
                    proof: None,
                    reason: "durable_proof_invalid",
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return DurableProofRead {
                    proof: None,
                    reason: "durable_proof_missing",
                };
            }
            Err(_) => {
                return DurableProofRead {
                    proof: None,
                    reason: "durable_proof_unreadable",
                };
            }
        };
        if !private_file_metadata(&metadata) || metadata.len() > MAX_DURABLE_PROOF_BYTES {
            return DurableProofRead {
                proof: None,
                reason: "durable_proof_permissions_or_size_invalid",
            };
        }
        match fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<DurableVerificationProof>(&bytes).ok())
        {
            Some(proof) if proof.schema_version == DURABLE_PROOF_SCHEMA => DurableProofRead {
                proof: Some(proof),
                reason: "durable_identity_changed",
            },
            Some(_) => DurableProofRead {
                proof: None,
                reason: "durable_proof_schema_changed",
            },
            None => DurableProofRead {
                proof: None,
                reason: "durable_proof_malformed",
            },
        }
    }
}

#[cfg(target_os = "macos")]
fn private_directory_metadata(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.mode() & 0o777 == 0o700 && metadata.uid() == unsafe { libc::geteuid() }
}

#[cfg(target_os = "macos")]
fn private_file_metadata(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.mode() & 0o777 == 0o600 && metadata.uid() == unsafe { libc::geteuid() }
}

fn verified_files_from_durable_proof(
    proof: &DurableVerificationProof,
    models_root: &Path,
    directory: &Path,
    binding: DurableProofBinding<'_>,
) -> Option<Vec<VerifiedFile>> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (proof, models_root, directory, binding);
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        if proof.schema_version != DURABLE_PROOF_SCHEMA
            || proof.model_id != binding.model_id
            || proof.repository != binding.repository
            || proof.revision != binding.revision
            || proof.manifest_sha256 != binding.manifest_sha256
            || proof.models_root
                != canonical_or_absolute(models_root)
                    .to_string_lossy()
                    .into_owned()
            || proof.managed_directory
                != canonical_or_absolute(directory)
                    .to_string_lossy()
                    .into_owned()
            || proof.files.len() != binding.files.len()
        {
            return None;
        }
        let directory_before = durable_file_identity(directory).ok()?;
        if directory_before != proof.managed_directory_identity {
            return None;
        }
        let manifest_path = directory.join(INSTALLED_MANIFEST);
        let manifest_identity = durable_file_identity(&manifest_path).ok()?;
        if manifest_identity != proof.installed_manifest_identity {
            return None;
        }
        let manifest_bytes = fs::read(&manifest_path).ok()?;
        let manifest_digest = hex::encode(Sha256::digest(&manifest_bytes));
        if manifest_digest != proof.installed_manifest_sha256 {
            return None;
        }
        let installed: InstalledManifest = serde_json::from_slice(&manifest_bytes).ok()?;
        if installed.schema_version != 1
            || installed.id != binding.model_id
            || installed.repository != binding.repository
            || installed.revision != binding.revision
            || installed.manifest_sha256 != binding.manifest_sha256
            || installed.files.len() != binding.files.len()
        {
            return None;
        }
        if validate_safetensors_inventory(directory, binding.files).is_err() {
            return None;
        }
        let mut verified = Vec::with_capacity(binding.files.len());
        for ((actual, expected), cached) in
            installed.files.iter().zip(binding.files).zip(&proof.files)
        {
            if actual.path != expected.path
                || actual.bytes != expected.bytes
                || actual.sha256 != expected.sha256
                || cached.path != expected.path
                || cached.bytes != expected.bytes
                || cached.sha256 != expected.sha256
                || cached.identity.file != actual.identity
            {
                return None;
            }
            let identity = durable_file_identity(&directory.join(expected.path)).ok()?;
            if identity != cached.identity {
                return None;
            }
            verified.push(VerifiedFile {
                path: expected.path.to_owned(),
                bytes: expected.bytes,
                sha256: expected.sha256.to_owned(),
                identity: identity.file,
            });
        }
        if durable_file_identity(directory).ok()? != directory_before {
            return None;
        }
        Some(verified)
    }
}

fn write_durable_proof_record(
    models_root: &Path,
    directory: &Path,
    files: &[VerifiedFile],
    binding: DurableProofBinding<'_>,
) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (models_root, directory, files, binding);
        return Err("durable identity cache is unavailable on this target".to_owned());
    }
    #[cfg(target_os = "macos")]
    {
        if files.len() != binding.files.len() {
            return Err("verified model inventory is incomplete".to_owned());
        }
        let cache_dir = durable_cache_dir(models_root);
        ensure_private_dir(&cache_dir)?;
        let directory_identity = durable_file_identity(directory)?;
        let manifest_path = directory.join(INSTALLED_MANIFEST);
        let manifest_identity = durable_file_identity(&manifest_path)?;
        let manifest_bytes = fs::read(&manifest_path).map_err(|error| error.to_string())?;
        if manifest_bytes.len() > MAX_DURABLE_PROOF_BYTES as usize {
            return Err("installed model manifest exceeds the proof bound".to_owned());
        }
        let installed: InstalledManifest =
            serde_json::from_slice(&manifest_bytes).map_err(|error| error.to_string())?;
        if installed.schema_version != 1
            || installed.id != binding.model_id
            || installed.repository != binding.repository
            || installed.revision != binding.revision
            || installed.manifest_sha256 != binding.manifest_sha256
            || installed.files.len() != binding.files.len()
        {
            return Err("installed model manifest changed before proof write".to_owned());
        }
        let mut proof_files = Vec::with_capacity(binding.files.len());
        for ((verified, actual), expected) in files.iter().zip(&installed.files).zip(binding.files)
        {
            if verified.path != expected.path
                || verified.bytes != expected.bytes
                || verified.sha256 != expected.sha256
                || actual.path != verified.path
                || actual.bytes != verified.bytes
                || actual.sha256 != verified.sha256
                || actual.identity != verified.identity
            {
                return Err("verified model inventory changed before proof write".to_owned());
            }
            let identity = durable_file_identity(&directory.join(expected.path))?;
            if identity.file != verified.identity {
                return Err("model asset identity changed before proof write".to_owned());
            }
            proof_files.push(DurableProofFile {
                path: verified.path.clone(),
                bytes: verified.bytes,
                sha256: verified.sha256.clone(),
                identity,
            });
        }
        if durable_file_identity(directory)? != directory_identity
            || durable_file_identity(&manifest_path)? != manifest_identity
        {
            return Err("model directory changed before proof write".to_owned());
        }
        let proof = DurableVerificationProof {
            schema_version: DURABLE_PROOF_SCHEMA,
            model_id: binding.model_id.to_owned(),
            repository: binding.repository.to_owned(),
            revision: binding.revision.to_owned(),
            manifest_sha256: binding.manifest_sha256.to_owned(),
            models_root: canonical_or_absolute(models_root)
                .to_string_lossy()
                .into_owned(),
            managed_directory: canonical_or_absolute(directory)
                .to_string_lossy()
                .into_owned(),
            managed_directory_identity: directory_identity.clone(),
            installed_manifest_sha256: hex::encode(Sha256::digest(&manifest_bytes)),
            installed_manifest_identity: manifest_identity.clone(),
            files: proof_files,
        };
        let bytes = serde_json::to_vec(&proof).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_DURABLE_PROOF_BYTES as usize {
            return Err("durable model proof exceeds its size bound".to_owned());
        }
        let proof_path = durable_proof_path(models_root);
        atomic_write_private(&proof_path, &bytes)?;

        // The proof lives outside the managed model directory. Read the bound
        // identities again only after the atomic replacement so a cache write
        // cannot accidentally make its own directory identity stale.
        if durable_file_identity(directory)? != directory_identity
            || durable_file_identity(&manifest_path)? != manifest_identity
            || durable_file_identity(&proof_path).is_err()
        {
            return Err("model identity changed while publishing durable proof".to_owned());
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn atomic_write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;

    let parent = path
        .parent()
        .ok_or_else(|| "durable proof path has no parent".to_owned())?;
    ensure_private_dir(parent)?;
    let sequence = DURABLE_PROOF_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".proof-{}-{sequence}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    let result = (|| {
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        restrict_file_mode(&temporary)?;
        fs::rename(&temporary, path).map_err(|error| error.to_string())?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn verify_directory_with_binding(
    directory: &Path,
    trace: Option<&VerificationTrace<'_>>,
    progress: &mut VerificationProgress,
    binding: DurableProofBinding<'_>,
) -> Result<Vec<VerifiedFile>, String> {
    verify_directory_against_binding(
        directory,
        trace,
        progress,
        binding,
        binding.files == MODEL_FILES,
    )
}

fn verify_directory_against_binding(
    directory: &Path,
    trace: Option<&VerificationTrace<'_>>,
    progress: &mut VerificationProgress,
    binding: DurableProofBinding<'_>,
    validate_pinned_metadata: bool,
) -> Result<Vec<VerifiedFile>, String> {
    let expected_files = binding.files;
    validate_model_parent_from_managed(directory)?;
    let metadata = fs::symlink_metadata(directory)
        .map_err(|error| format!("managed cleanup model is unavailable: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("managed cleanup model folder must be a real directory".to_owned());
    }
    let installed = read_manifest(directory)?;
    if installed.schema_version != 1
        || installed.id != binding.model_id
        || installed.repository != binding.repository
        || installed.revision != binding.revision
        || installed.manifest_sha256 != binding.manifest_sha256
    {
        return Err("installed cleanup model manifest does not match the pinned model".to_owned());
    }
    if installed.files.len() != expected_files.len() {
        return Err("installed cleanup model inventory is incomplete".to_owned());
    }
    validate_safetensors_inventory(directory, expected_files)?;
    let mut verified = Vec::with_capacity(expected_files.len());
    for (index, (actual, expected)) in installed.files.iter().zip(expected_files).enumerate() {
        validate_manifest_path(expected.path)?;
        if actual.path != expected.path
            || actual.bytes != expected.bytes
            || actual.sha256 != expected.sha256
        {
            return Err("installed cleanup model inventory changed".to_owned());
        }
        let file_started = Instant::now();
        let bytes_before = progress.hashed_bytes;
        let hash_result =
            hash_file_with_progress(&directory.join(expected.path), expected, progress);
        if let Some(trace) = trace {
            mlx_diagnostics::event(
                EventName::CleanupModelFileHash,
                Severity::Debug,
                Target::ModelStore,
            )
            .request_id(trace.request_id)
            .child_generation(trace.child_generation)
            .session_generation(trace.session_generation)
            .file_hash_facts(
                index,
                hash_result.is_ok(),
                progress.hashed_bytes.saturating_sub(bytes_before),
                file_started.elapsed().as_secs_f64() * 1_000.0,
            )
            .emit();
        }
        let identity = hash_result?;
        if identity != actual.identity {
            return Err(format!(
                "model file identity changed since verification: {}",
                expected.path
            ));
        }
        verified.push(VerifiedFile {
            path: expected.path.to_owned(),
            bytes: expected.bytes,
            sha256: expected.sha256.to_owned(),
            identity,
        });
    }
    if validate_pinned_metadata {
        validate_model_metadata(directory)?;
    }
    Ok(verified)
}

fn validate_safetensors_inventory(
    directory: &Path,
    expected_files: &[ModelFile],
) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        if name.to_string_lossy().ends_with(".safetensors")
            && !expected_files
                .iter()
                .any(|expected| Path::new(expected.path).file_name() == Some(name.as_os_str()))
        {
            return Err("installed cleanup model contains an unlisted safetensors file".to_owned());
        }
    }
    Ok(())
}

fn sha256_backend_facts() -> (&'static str, Option<bool>) {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let hardware = std::arch::is_aarch64_feature_detected!("sha2");
        (
            if hardware {
                "sha2_aarch64_intrinsics"
            } else {
                "sha2_software_fallback"
            },
            Some(hardware),
        )
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        ("sha2_target_default", None)
    }
}

fn read_installed_inventory(directory: &Path) -> Result<Option<Vec<VerifiedFile>>, String> {
    read_installed_inventory_with_binding(directory, production_durable_proof_binding())
}

fn read_installed_inventory_with_binding(
    directory: &Path,
    binding: DurableProofBinding<'_>,
) -> Result<Option<Vec<VerifiedFile>>, String> {
    validate_model_parent_from_managed(directory)?;
    let dir_meta = match fs::symlink_metadata(directory) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if dir_meta.file_type().is_symlink() || !dir_meta.is_dir() {
        return Err("managed cleanup model folder is not a real directory".to_owned());
    }
    let manifest = read_manifest(directory)?;
    if manifest.schema_version != 1
        || manifest.id != binding.model_id
        || manifest.repository != binding.repository
        || manifest.revision != binding.revision
        || manifest.manifest_sha256 != binding.manifest_sha256
        || manifest.files.len() != binding.files.len()
    {
        return Err("installed cleanup model manifest does not match the pinned model".to_owned());
    }
    validate_safetensors_inventory(directory, binding.files)?;
    for (actual, expected) in manifest.files.iter().zip(binding.files) {
        if actual.path != expected.path
            || actual.bytes != expected.bytes
            || actual.sha256 != expected.sha256
        {
            return Err("installed cleanup model inventory changed".to_owned());
        }
        if file_identity(&directory.join(expected.path))? != actual.identity {
            return Err(format!(
                "installed cleanup model changed: {}",
                expected.path
            ));
        }
    }
    Ok(Some(manifest.files))
}

fn read_manifest(directory: &Path) -> Result<InstalledManifest, String> {
    let path = directory.join(INSTALLED_MANIFEST);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("installed model manifest is missing: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("installed model manifest must be a regular file".to_owned());
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("installed model manifest is invalid: {error}"))
}

fn validate_model_metadata(directory: &Path) -> Result<(), String> {
    let config: serde_json::Value = serde_json::from_slice(
        &fs::read(directory.join("config.json")).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if config.get("model_type").and_then(serde_json::Value::as_str) != Some("qwen3_5")
        || config
            .get("architectures")
            .and_then(serde_json::Value::as_array)
            .and_then(|values| values.first())
            .and_then(serde_json::Value::as_str)
            != Some("Qwen3_5ForConditionalGeneration")
        || config
            .pointer("/quantization_config/bits")
            .and_then(serde_json::Value::as_u64)
            != Some(6)
        || config
            .pointer("/quantization_config/group_size")
            .and_then(serde_json::Value::as_u64)
            != Some(64)
        || config
            .pointer("/quantization_config/mode")
            .and_then(serde_json::Value::as_str)
            != Some("affine")
        || config
            .pointer("/text_config/num_hidden_layers")
            .and_then(serde_json::Value::as_u64)
            != Some(64)
        || config
            .pointer("/text_config/vocab_size")
            .and_then(serde_json::Value::as_u64)
            != Some(248_320)
    {
        return Err("model config is not the pinned Qwen 3.8 MLX 6-bit layout".to_owned());
    }
    let index: serde_json::Value = serde_json::from_slice(
        &fs::read(directory.join("model.safetensors.index.json"))
            .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if index
        .get("metadata")
        .and_then(|v| v.get("total_size"))
        .and_then(serde_json::Value::as_u64)
        != Some(22_777_600_480)
        || index
            .get("weight_map")
            .and_then(serde_json::Value::as_object)
            .is_none_or(|weights| {
                weights.len() != 2_180
                    || weights.values().any(|name| {
                        name.as_str().is_none_or(|name| {
                            !MODEL_FILES
                                .iter()
                                .any(|file| file.path == name && name.starts_with("model-"))
                        })
                    })
            })
    {
        return Err(
            "model shard index does not match the pinned 2,180 tensor inventory".to_owned(),
        );
    }
    Ok(())
}

fn write_installed_manifest(
    directory: &Path,
    files: Vec<VerifiedFile>,
) -> Result<Vec<VerifiedFile>, String> {
    if files.len() != MODEL_FILES.len() {
        return Err("model inventory is incomplete".to_owned());
    }
    for (actual, expected) in files.iter().zip(MODEL_FILES) {
        if actual.path != expected.path
            || actual.bytes != expected.bytes
            || actual.sha256 != expected.sha256
        {
            return Err("model inventory does not match the pinned files".to_owned());
        }
        if file_identity(&directory.join(expected.path))? != actual.identity {
            return Err(format!(
                "model file changed before manifest commit: {}",
                expected.path
            ));
        }
    }
    validate_model_metadata(directory)?;
    let manifest = InstalledManifest {
        schema_version: 1,
        id: MODEL_ID.to_owned(),
        repository: REPOSITORY.to_owned(),
        revision: REVISION.to_owned(),
        manifest_sha256: MANIFEST_SHA256.to_owned(),
        files: files.clone(),
    };
    let bytes = serde_json::to_vec(&manifest).map_err(|error| error.to_string())?;
    write_atomic(&directory.join(INSTALLED_MANIFEST), &bytes)?;
    Ok(files)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "invalid model manifest path".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let tmp = parent.join(format!(".model-manifest-{}.tmp", std::process::id()));
    let result = (|| {
        let mut output = File::create(&tmp).map_err(|error| error.to_string())?;
        output.write_all(bytes).map_err(|error| error.to_string())?;
        output.sync_all().map_err(|error| error.to_string())?;
        fs::rename(&tmp, path).map_err(|error| error.to_string())
    })();
    match result {
        Ok(()) => Ok(()),
        Err(error) => match remove_path(&tmp) {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(format!(
                "{error}; temporary manifest could not be removed: {cleanup_error}"
            )),
        },
    }
}

async fn import_model(
    models_root: &Path,
    selected: &Path,
    cancel: &CancellationToken,
    bytes_done: &AtomicU64,
) -> Result<Vec<VerifiedFile>, String> {
    ensure_operation_not_cancelled(cancel, "import")?;
    ensure_model_parent(models_root)?;
    let source_meta = fs::symlink_metadata(selected).map_err(|error| error.to_string())?;
    if source_meta.file_type().is_symlink() || !source_meta.is_dir() {
        return Err("selected model folder must be a real directory".to_owned());
    }
    let staging = staging_dir(models_root);
    remove_path(&staging)?;
    ensure_private_dir(&staging)?;
    let result = (|| -> Result<Vec<VerifiedFile>, String> {
        let mut total = 0u64;
        let mut inventory = Vec::with_capacity(MODEL_FILES.len());
        for expected in MODEL_FILES {
            ensure_operation_not_cancelled(cancel, "import")?;
            validate_manifest_path(expected.path)?;
            let source = selected.join(expected.path);
            let dest = staging.join(expected.path);
            let source_metadata = fs::symlink_metadata(&source).map_err(|error| {
                format!("cannot read source model file {}: {error}", expected.path)
            })?;
            if source_metadata.file_type().is_symlink() || !source_metadata.is_file() {
                return Err(format!(
                    "source model asset is not a regular file: {}",
                    expected.path
                ));
            }
            if source_metadata.len() != expected.bytes {
                return Err(format!(
                    "source model file size mismatch: {}",
                    expected.path
                ));
            }
            clone_file(&source, &dest)?;
            restrict_file_mode(&dest)?;
            let identity = hash_file(&dest, expected)?;
            inventory.push(VerifiedFile {
                path: expected.path.to_owned(),
                bytes: expected.bytes,
                sha256: expected.sha256.to_owned(),
                identity,
            });
            total = total.saturating_add(expected.bytes);
            bytes_done.store(total, Ordering::Relaxed);
        }
        let inventory = write_installed_manifest(&staging, inventory)?;
        ensure_operation_not_cancelled(cancel, "import")?;
        commit_directory(&staging, &managed_dir(models_root))?;
        Ok(inventory)
    })();
    match result {
        Ok(files) => Ok(files),
        Err(error) => match remove_path(&staging) {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(format!(
                "{error}; VoiceFlow could not remove the managed import staging files: {cleanup_error}"
            )),
        },
    }
}

fn clone_file(source: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let source_c = CString::new(source.as_os_str().as_bytes())
            .map_err(|_| "invalid source path".to_owned())?;
        let destination_c = CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| "invalid destination path".to_owned())?;
        // APFS clonefile shares blocks until either managed file is modified.
        let result = unsafe { clonefile(source_c.as_ptr(), destination_c.as_ptr(), 0) };
        if result == 0 {
            Ok(())
        } else {
            Err(format!(
                "could not APFS-clone model asset; choose Download or use an APFS volume ({})",
                std::io::Error::last_os_error()
            ))
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (source, destination);
        Err("model import requires APFS clone support on macOS".to_owned())
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn clonefile(
        source: *const std::ffi::c_char,
        destination: *const std::ffi::c_char,
        flags: u32,
    ) -> i32;
}

async fn download_model(
    models_root: &Path,
    cancel: &CancellationToken,
    bytes_done: &AtomicU64,
) -> Result<Vec<VerifiedFile>, String> {
    ensure_model_parent(models_root)?;
    let staging = staging_dir(models_root);
    ensure_private_dir(&staging)?;
    let client = reqwest::Client::builder()
        .https_only(true)
        .timeout(Duration::from_secs(60 * 60))
        .user_agent("VoiceFlow/0.1")
        .build()
        .map_err(|error| error.to_string())?;

    let files = download_files(
        &client,
        MODEL_FILES,
        &staging,
        |path| format!("https://huggingface.co/{REPOSITORY}/resolve/{REVISION}/{path}"),
        cancel,
        bytes_done,
    )
    .await?;
    let files = write_installed_manifest(&staging, files)?;
    ensure_operation_not_cancelled(cancel, "download")?;
    commit_directory(&staging, &managed_dir(models_root))?;
    Ok(files)
}

/// Transfers and verifies files into the store's temporary directory. The
/// production caller supplies only URLs for the pinned Hugging Face revision;
/// this private seam lets tests exercise the same byte/hash/rename path with a
/// tiny loopback fixture.
async fn download_files<F>(
    client: &reqwest::Client,
    files: &[ModelFile],
    staging: &Path,
    endpoint_for_path: F,
    cancel: &CancellationToken,
    bytes_done: &AtomicU64,
) -> Result<Vec<VerifiedFile>, String>
where
    F: Fn(&str) -> String,
{
    let mut completed = 0u64;
    let mut inventory = Vec::with_capacity(files.len());
    for model_file in files {
        ensure_operation_not_cancelled(cancel, "download")?;
        validate_manifest_path(model_file.path)?;
        let destination = staging.join(model_file.path);
        if let Ok(identity) = hash_file(&destination, model_file) {
            inventory.push(VerifiedFile {
                path: model_file.path.to_owned(),
                bytes: model_file.bytes,
                sha256: model_file.sha256.to_owned(),
                identity,
            });
            completed = completed.saturating_add(model_file.bytes);
            bytes_done.store(completed, Ordering::Relaxed);
            continue;
        }
        remove_path(&destination)?;
        let temporary = staging.join(format!(".{}.download", model_file.path));
        remove_path(&temporary)?;
        let url = endpoint_for_path(model_file.path);
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err("download cancelled".to_owned()),
            response = client.get(url).send() => response.map_err(|error| sanitize_error(&error.to_string()))?,
        };
        if !response.status().is_success() {
            return Err(format!(
                "model download failed ({})",
                response.status().as_u16()
            ));
        }
        let mut stream = response.bytes_stream();
        let mut output = File::create(&temporary).map_err(|error| error.to_string())?;
        restrict_file_mode(&temporary)?;
        let mut hasher = Sha256::new();
        let mut written = 0u64;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    return Err(remove_temporary_with_context(&temporary, "download cancelled"));
                },
                chunk = stream.next() => match chunk {
                    None => break,
                    Some(Err(error)) => {
                        return Err(remove_temporary_with_context(&temporary, &sanitize_error(&error.to_string())));
                    },
                    Some(Ok(chunk)) => {
                        written = written.checked_add(chunk.len() as u64)
                            .ok_or_else(|| "model file exceeded pinned size".to_owned())?;
                        if written > model_file.bytes {
                            return Err(remove_temporary_with_context(&temporary, "model file exceeded pinned size"));
                        }
                        output.write_all(&chunk).map_err(|error| error.to_string())?;
                        hasher.update(&chunk);
                        bytes_done.store(completed.saturating_add(written), Ordering::Relaxed);
                    }
                }
            }
        }
        if written != model_file.bytes || hex::encode(hasher.finalize()) != model_file.sha256 {
            return Err(remove_temporary_with_context(
                &temporary,
                &format!("model file size or checksum mismatch: {}", model_file.path),
            ));
        }
        output.sync_all().map_err(|error| error.to_string())?;
        drop(output);
        fs::rename(&temporary, &destination).map_err(|error| error.to_string())?;
        restrict_file_mode(&destination)?;
        let identity = file_identity(&destination)?;
        inventory.push(VerifiedFile {
            path: model_file.path.to_owned(),
            bytes: model_file.bytes,
            sha256: model_file.sha256.to_owned(),
            identity,
        });
        completed = completed.saturating_add(model_file.bytes);
        bytes_done.store(completed, Ordering::Relaxed);
    }
    // Keep previously verified files in the VoiceFlow-owned staging directory.
    // A later explicit Download can verify and resume them; the in-flight temp
    // file is removed at the cancellation boundary above.
    Ok(inventory)
}

fn ensure_operation_not_cancelled(
    cancel: &CancellationToken,
    operation: &str,
) -> Result<(), String> {
    if cancel.is_cancelled() {
        Err(format!("{operation} cancelled"))
    } else {
        Ok(())
    }
}

fn remove_temporary_with_context(path: &Path, reason: &str) -> String {
    match remove_path(path) {
        Ok(()) => reason.to_owned(),
        Err(error) => format!("{reason}; temporary model file could not be removed: {error}"),
    }
}

fn commit_directory(source: &Path, destination: &Path) -> Result<(), String> {
    let parent = destination
        .parent()
        .ok_or_else(|| "invalid managed model directory".to_owned())?;
    ensure_private_dir(parent)?;
    match fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(
                "a managed model folder already exists; delete it before replacement".to_owned(),
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    fs::rename(source, destination).map_err(|error| error.to_string())
}

fn ensure_private_dir(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err("model storage path must be a real directory".to_owned());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|error| error.to_string())?
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

fn ensure_model_parent(models_root: &Path) -> Result<(), String> {
    ensure_private_dir(models_root)?;
    ensure_private_dir(&models_root.join("cleanup"))
}

fn validate_model_parent(models_root: &Path) -> Result<(), String> {
    validate_model_parent_from_managed(&managed_dir(models_root))
}

fn validate_model_parent_from_managed(directory: &Path) -> Result<(), String> {
    let parent = directory
        .parent()
        .ok_or_else(|| "invalid managed model path".to_owned())?;
    if let Ok(metadata) = fs::symlink_metadata(parent) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("model storage parent must be a real directory".to_owned());
        }
    }
    if let Some(root) = parent.parent() {
        if let Ok(metadata) = fs::symlink_metadata(root) {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err("model storage root must be a real directory".to_owned());
            }
        }
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

fn remove_path(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(path).map_err(|error| error.to_string())
        }
        Ok(_) => fs::remove_file(path).map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn sanitize_error(message: &str) -> String {
    let mut result = String::new();
    let mut rest = message;
    while let Some(scheme_at) = rest.find("://") {
        result.push_str(&rest[..scheme_at]);
        let after = &rest[scheme_at..];
        let url_end = after.find(char::is_whitespace).unwrap_or(after.len());
        let url = &after[..url_end];
        result.push_str(url.split('?').next().unwrap_or(url));
        rest = &after[url_end..];
    }
    result.push_str(rest);
    result
}

/// Shared production manager used by command handlers and cleanup runtime paths.
pub fn shared_manager() -> Manager {
    static MANAGER: std::sync::OnceLock<Manager> = std::sync::OnceLock::new();
    MANAGER.get_or_init(Manager::new).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    static TINY_FILES: [ModelFile; 2] = [
        ModelFile {
            path: "first.safetensors",
            bytes: 5,
            sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
        },
        ModelFile {
            path: "second.safetensors",
            bytes: 5,
            sha256: "486ea46224d1bb4fb680f34f7c9ad96a8f24ec88be73ea8e5a6c65260e9cb8a7",
        },
    ];

    fn unused_root() -> PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "voiceflow-modelstore-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn trusted_cache_rejects_unlisted_safetensors_without_rehashing_before_rejection() {
        let root = unused_root();
        let manager = Manager::new();
        let binding = tiny_test_durable_proof_binding();
        let directory = seed_tiny_fixture(&root, TinyFixtureFault::None)
            .expect("seed a real tiny installed inventory");

        let (_, first) = manager
            .resolve_ready_inner_with_binding(&root, None, binding)
            .expect("cold verification should establish this manager's trusted cache");
        assert_eq!(first.cache, "miss");
        assert_eq!(first.hashed_files, TINY_RUNTIME_FILES.len());
        assert_eq!(first.hashed_bytes, 10);

        remove_path(&durable_proof_path(&root)).expect("remove only this fixture's durable proof");
        let (_, second) = manager
            .resolve_ready_inner_with_binding(&root, None, binding)
            .expect("same-manager trusted hit should work without durable proof");
        assert_eq!(second.cache, "hit");
        assert_eq!(second.cache_reason, "trusted_file_identity_match");
        assert_eq!(
            second.hashed_files, 0,
            "trusted hit must not hash payload files"
        );
        assert_eq!(
            second.hashed_bytes, 0,
            "trusted hit must not read payload bytes"
        );

        fs::write(directory.join("unlisted.safetensors"), b"extra")
            .expect("add an unlisted safetensors file to the owned fixture");
        remove_path(&durable_proof_path(&root)).expect("remove proof recreated by the trusted hit");
        assert!(
            read_durable_proof(&root).proof.is_none(),
            "third resolve must exercise the trusted cache with no durable proof"
        );
        let third = manager.resolve_ready_inner_with_binding(&root, None, binding);
        assert!(
            third
                .as_ref()
                .err()
                .is_some_and(|error| error.contains("unlisted safetensors")),
            "trusted cache must reject an unlisted safetensors file"
        );
        remove_path(&root).expect("remove isolated trusted-cache regression fixture");
    }

    #[test]
    fn tiny_fixture_runs_real_inventory_and_file_hash_verifier_for_reject_cases() {
        for fault in [
            TinyFixtureFault::InventoryReject,
            TinyFixtureFault::HashReject,
        ] {
            let root = unused_root();
            let capture = mlx_diagnostics::EmitRecordCaptureForTest::default();
            let result = capture.with_scope(|| {
                tiny_verified_fixture(&root, &Manager::new(), "matrix-request", 7, Some(41), fault)
            });
            assert!(result.is_err());
            match fault {
                TinyFixtureFault::InventoryReject => assert!(
                    result
                        .as_ref()
                        .err()
                        .unwrap()
                        .contains("inventory is incomplete"),
                    "inventory corruption must be rejected before file hashing"
                ),
                TinyFixtureFault::HashReject => assert!(
                    result.as_ref().err().unwrap().contains("checksum mismatch"),
                    "same-length corruption must be rejected by the actual file hash"
                ),
                TinyFixtureFault::None => unreachable!(),
            }
            let records: Vec<serde_json::Value> = capture
                .snapshot()
                .iter()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            assert!(
                records.iter().any(|record| {
                    record.get("request_id").and_then(serde_json::Value::as_str)
                        == Some("matrix-request")
                        && record
                            .get("child_generation")
                            .and_then(serde_json::Value::as_u64)
                            == Some(7)
                        && record
                            .get("session_generation")
                            .and_then(serde_json::Value::as_u64)
                            == Some(41)
                }),
                "actual verifier events retain request, child, and captured session generation"
            );
            if matches!(fault, TinyFixtureFault::HashReject) {
                assert!(records.iter().any(|record| {
                    record.get("event").and_then(serde_json::Value::as_str)
                        == Some("cleanup_model_file_hash")
                        && record.get("outcome").and_then(serde_json::Value::as_str)
                            == Some("failed")
                        && record.get("file_index").and_then(serde_json::Value::as_u64)
                            == Some(0)
                        && record.get("hashed_bytes").and_then(serde_json::Value::as_u64)
                            == Some(5)
                        && record.get("request_id").and_then(serde_json::Value::as_str)
                            == Some("matrix-request")
                        && record.get("child_generation").and_then(serde_json::Value::as_u64)
                            == Some(7)
                        && record.get("session_generation").and_then(serde_json::Value::as_u64)
                            == Some(41)
                }), "same-length corruption reaches and fails the actual hash emitter for file zero");
            }
            assert!(records.iter().any(|record| {
                record.get("event").and_then(serde_json::Value::as_str)
                    == Some("cleanup_model_inventory")
                    && record.get("reason").and_then(serde_json::Value::as_str)
                        == Some("verification_rejected")
            }));
            remove_path(&root).expect("remove isolated tiny verifier fixture");
        }
    }

    #[test]
    fn cleanup_model_dir_rejects_extra_safetensors() {
        let root = unused_root();
        let directory = seed_tiny_fixture(&root, TinyFixtureFault::None)
            .expect("seed exact temporary cleanup model directory");
        let mut progress = VerificationProgress::default();
        let exact = verify_directory_with_binding(
            &directory,
            None,
            &mut progress,
            tiny_test_durable_proof_binding(),
        );
        assert_eq!(exact.expect("exact manifest shard set passes").len(), 2);

        fs::write(directory.join("unlisted.safetensors"), b"extra")
            .expect("add unlisted temporary model shard");
        let mut progress = VerificationProgress::default();
        let error = verify_directory_with_binding(
            &directory,
            None,
            &mut progress,
            tiny_test_durable_proof_binding(),
        )
        .expect_err("unlisted safetensors shard is rejected");
        assert!(error.contains("unlisted safetensors"));
        remove_path(&root).expect("remove isolated tiny model fixture");
    }

    #[test]
    fn selected_inventory_observer_panic_is_captured_without_changing_verification() {
        let root = unused_root();
        let manager = Manager::new();
        let capture = mlx_diagnostics::EmitRecordCaptureForTest::default();
        mlx_diagnostics::panic_on_event_for_test(EventName::CleanupModelInventory);
        let result = capture.with_scope(|| {
            tiny_verified_fixture(
                &root,
                &manager,
                "panic-observer-request",
                9,
                Some(52),
                TinyFixtureFault::None,
            )
        });
        let (model, facts) =
            result.expect("best-effort observer panic leaves verified model available");
        assert_eq!(facts.file_count, 2);
        assert_eq!(manager.test_lease_count(), 1);
        let delivered: Vec<serde_json::Value> = capture
            .snapshot()
            .iter()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let panicked: Vec<serde_json::Value> = capture
            .panicked_attempts()
            .iter()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        assert!(delivered.iter().any(|event| event
            .get("event")
            .and_then(serde_json::Value::as_str)
            == Some("cleanup_model_file_hash")));
        assert!(!delivered.iter().any(|event| event
            .get("event")
            .and_then(serde_json::Value::as_str)
            == Some("cleanup_model_inventory")));
        assert_eq!(
            panicked.len(),
            1,
            "one selected event was attempted and panicked"
        );
        assert_eq!(
            panicked[0].get("event").and_then(serde_json::Value::as_str),
            Some("cleanup_model_inventory")
        );
        assert_eq!(
            panicked[0]
                .get("request_id")
                .and_then(serde_json::Value::as_str),
            Some("panic-observer-request")
        );
        assert_eq!(
            panicked[0]
                .get("child_generation")
                .and_then(serde_json::Value::as_u64),
            Some(9)
        );
        assert_eq!(
            panicked[0]
                .get("session_generation")
                .and_then(serde_json::Value::as_u64),
            Some(52)
        );
        drop(model);
        assert_eq!(manager.test_lease_count(), 0);
        remove_path(&root).expect("remove verified tiny fixture after model lease release");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn malformed_durable_proof_is_a_cache_miss_not_model_corruption() {
        use std::os::unix::fs::PermissionsExt;

        let root = unused_root();
        let cache = durable_cache_dir(&root);
        fs::create_dir_all(&cache).expect("create isolated private cache directory");
        fs::set_permissions(&cache, fs::Permissions::from_mode(0o700))
            .expect("restrict isolated cache directory");
        let proof_path = durable_proof_path(&root);
        fs::write(&proof_path, b"{malformed").expect("write malformed proof fixture");
        fs::set_permissions(&proof_path, fs::Permissions::from_mode(0o600))
            .expect("restrict malformed proof fixture");

        let proof = read_durable_proof(&root);
        assert!(proof.proof.is_none());
        assert_eq!(proof.reason, "durable_proof_malformed");

        fs::remove_dir_all(root).expect("remove isolated durable proof fixture");
    }

    #[cfg(target_os = "macos")]
    fn tiny_proof_binding() -> DurableProofBinding<'static> {
        DurableProofBinding {
            model_id: "test/tiny-model",
            repository: "test/local-fixture",
            revision: "0123456789abcdef",
            manifest_sha256: "tiny-manifest-pin",
            files: &TINY_FILES,
        }
    }

    #[cfg(target_os = "macos")]
    fn seed_tiny_durable_proof() -> (PathBuf, PathBuf, Vec<VerifiedFile>) {
        let root = unused_root();
        let directory = root.join("cleanup").join("tiny-model");
        fs::create_dir_all(&directory).expect("create tiny model directory");
        fs::write(directory.join(TINY_FILES[0].path), b"hello").expect("write first tiny file");
        fs::write(directory.join(TINY_FILES[1].path), b"world").expect("write second tiny file");
        let verified = TINY_FILES
            .iter()
            .map(|expected| VerifiedFile {
                path: expected.path.to_owned(),
                bytes: expected.bytes,
                sha256: expected.sha256.to_owned(),
                identity: file_identity(&directory.join(expected.path)).expect("inspect tiny file"),
            })
            .collect::<Vec<_>>();
        let binding = tiny_proof_binding();
        let installed = InstalledManifest {
            schema_version: 1,
            id: binding.model_id.to_owned(),
            repository: binding.repository.to_owned(),
            revision: binding.revision.to_owned(),
            manifest_sha256: binding.manifest_sha256.to_owned(),
            files: verified.clone(),
        };
        let manifest_path = directory.join(INSTALLED_MANIFEST);
        fs::write(
            &manifest_path,
            serde_json::to_vec(&installed).expect("serialize tiny installed manifest"),
        )
        .expect("write tiny installed manifest");
        write_durable_proof_record(&root, &directory, &verified, binding)
            .expect("write app-owned tiny verification proof");
        (root, directory, verified)
    }

    #[cfg(target_os = "macos")]
    fn tiny_durable_proof_hit(root: &Path, directory: &Path) -> Option<Vec<VerifiedFile>> {
        // A fresh manager intentionally has no in-memory trusted inventory.
        let manager = Manager::new();
        assert!(manager.lock().trusted.is_none());
        let proof = read_durable_proof(root);
        verified_files_from_durable_proof(
            proof.proof.as_ref()?,
            root,
            directory,
            tiny_proof_binding(),
        )
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn durable_proof_and_prompt_snapshot_sibling_survive_a_new_manager() {
        use std::os::unix::fs::PermissionsExt;

        let (root, directory, expected) = seed_tiny_durable_proof();
        assert_eq!(
            tiny_durable_proof_hit(&root, &directory),
            Some(expected.clone())
        );

        // The prompt-cache snapshot is a sibling derived-cache owner. Creating
        // it must not change any identity bound to the installed model proof.
        let prefix = root
            .join("cleanup")
            .join(".voiceflow-mlx-cache")
            .join("prompt-prefix");
        let active = prefix.join("active");
        let cache_parent = prefix.parent().expect("prompt cache has a parent");
        fs::create_dir_all(&active).expect("create prompt cache sibling fixture");
        fs::set_permissions(cache_parent, fs::Permissions::from_mode(0o700))
            .expect("restrict prompt cache parent");
        fs::set_permissions(&prefix, fs::Permissions::from_mode(0o700))
            .expect("restrict prompt cache parent");
        fs::set_permissions(&active, fs::Permissions::from_mode(0o700))
            .expect("restrict prompt cache snapshot owner");
        fs::write(active.join("prefix.safetensors"), b"derived cache only")
            .expect("write prompt cache sibling fixture");
        assert_eq!(tiny_durable_proof_hit(&root, &directory), Some(expected));

        fs::remove_dir_all(root).expect("remove tiny durable proof fixture");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn durable_proof_rejects_extra_safetensors_in_model_directory() {
        let (root, directory, expected) = seed_tiny_durable_proof();
        assert_eq!(
            tiny_durable_proof_hit(&root, &directory),
            Some(expected),
            "exact manifest inventory keeps the durable proof fast path"
        );

        fs::write(directory.join("unlisted.safetensors"), b"extra")
            .expect("add unlisted model shard");
        assert!(
            tiny_durable_proof_hit(&root, &directory).is_none(),
            "durable proof must reject shards outside the installed manifest"
        );
        fs::remove_dir_all(root).expect("remove tiny durable proof fixture");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn durable_proof_misses_on_time_mode_inode_root_manifest_and_schema_changes() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let (root, directory, _) = seed_tiny_durable_proof();
        let first = directory.join(TINY_FILES[0].path);
        let original_mtime = fs::metadata(&first).expect("read initial metadata").mtime();
        let file = File::open(&first).expect("open tiny asset for timestamp change");
        file.set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
            .expect("change only the file timestamp");
        assert_ne!(fs::metadata(&first).unwrap().mtime(), original_mtime);
        assert!(tiny_durable_proof_hit(&root, &directory).is_none());
        fs::remove_dir_all(&root).unwrap();

        let (root, directory, _) = seed_tiny_durable_proof();
        fs::write(directory.join(TINY_FILES[0].path), b"jello").unwrap();
        assert_eq!(
            fs::metadata(directory.join(TINY_FILES[0].path))
                .unwrap()
                .len(),
            5
        );
        assert!(tiny_durable_proof_hit(&root, &directory).is_none());
        fs::remove_dir_all(&root).unwrap();

        let (root, directory, _) = seed_tiny_durable_proof();
        let mode_changed_asset = directory.join(TINY_FILES[0].path);
        fs::set_permissions(&mode_changed_asset, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(tiny_durable_proof_hit(&root, &directory).is_none());
        fs::remove_dir_all(&root).unwrap();

        let (root, directory, _) = seed_tiny_durable_proof();
        let asset = directory.join(TINY_FILES[0].path);
        let replacement = directory.join("replacement.tmp");
        fs::write(&replacement, b"hello").unwrap();
        fs::rename(&replacement, &asset).unwrap();
        assert!(tiny_durable_proof_hit(&root, &directory).is_none());
        fs::remove_dir_all(&root).unwrap();

        let (root, directory, _) = seed_tiny_durable_proof();
        let other_root = root.join("different-root");
        fs::create_dir_all(&other_root).unwrap();
        assert!(verified_files_from_durable_proof(
            read_durable_proof(&root).proof.as_ref().unwrap(),
            &other_root,
            &directory,
            tiny_proof_binding(),
        )
        .is_none());
        let manifest = directory.join(INSTALLED_MANIFEST);
        let mut manifest_bytes = fs::read(&manifest).unwrap();
        manifest_bytes.push(b' ');
        fs::write(&manifest, manifest_bytes).unwrap();
        assert!(tiny_durable_proof_hit(&root, &directory).is_none());
        fs::remove_dir_all(&root).unwrap();

        let (root, directory, _) = seed_tiny_durable_proof();
        let proof_path = durable_proof_path(&root);
        let mut proof: DurableVerificationProof =
            serde_json::from_slice(&fs::read(&proof_path).unwrap()).unwrap();
        proof.schema_version += 1;
        fs::write(&proof_path, serde_json::to_vec(&proof).unwrap()).unwrap();
        fs::set_permissions(&proof_path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            read_durable_proof(&root).reason,
            "durable_proof_schema_changed"
        );
        assert!(tiny_durable_proof_hit(&root, &directory).is_none());
        fs::remove_dir_all(&root).unwrap();
    }

    fn hold_lease(manager: &Manager) -> Arc<ModelLease> {
        manager.leases.fetch_add(1, Ordering::AcqRel);
        Arc::new(ModelLease {
            counter: Arc::clone(&manager.leases),
        })
    }

    fn test_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
    }

    async fn spawn_chunked_test_response(
        first: &'static [u8],
        rest: &'static [u8],
    ) -> (
        String,
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (first_chunk_sender, first_chunk_received) = tokio::sync::oneshot::channel();
        let (continue_sender, continue_received) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut request = Vec::new();
            let mut buffer = [0u8; 1024];
            loop {
                let Ok(count) = stream.read(&mut buffer).await else {
                    return;
                };
                if count == 0 {
                    return;
                }
                request.extend_from_slice(&buffer[..count]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let response_head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                first.len() + rest.len()
            );
            if stream.write_all(response_head.as_bytes()).await.is_err()
                || stream.write_all(first).await.is_err()
                || stream.flush().await.is_err()
            {
                return;
            }
            let _ = first_chunk_sender.send(());
            let _ = continue_received.await;
            let _ = stream.write_all(rest).await;
        });
        (endpoint, first_chunk_received, continue_sender, task)
    }

    #[test]
    fn failed_operation_reason_survives_inventory_refresh() {
        let manager = Manager::new();
        let root = unused_root();
        let (generation, _, _) = manager
            .begin_operation(&root, "importing", "import")
            .unwrap();
        manager.finish_operation(
            generation,
            Some("model file checksum mismatch".to_owned()),
            None,
        );

        let status = manager.status(&root);
        assert_eq!(status.state, "missing");
        assert_eq!(status.last_operation.as_deref(), Some("import"));
        assert_eq!(
            status.last_operation_error.as_deref(),
            Some("model file checksum mismatch")
        );
        assert_eq!(status.error, None);
    }

    #[tokio::test]
    async fn revision_watch_tracks_operation_boundaries_without_progress_notifications() {
        let manager = Manager::new();
        let root = unused_root();
        let mut revisions = manager.subscribe_revision();
        let initial = *revisions.borrow_and_update();

        let (generation, _, bytes_done) = manager
            .begin_operation(&root, "importing", "import")
            .unwrap();
        assert!(revisions.changed().await.is_ok());
        assert_eq!(*revisions.borrow_and_update(), initial.wrapping_add(1));

        bytes_done.store(17, Ordering::Relaxed);
        tokio::task::yield_now().await;
        assert_eq!(*revisions.borrow(), initial.wrapping_add(1));

        manager.finish_operation(generation, Some("import cancelled".to_owned()), None);
        assert!(revisions.changed().await.is_ok());
        let status = manager.status(&root);
        assert_eq!(status.state, "missing");
        assert_eq!(status.operation, None);
        assert_eq!(status.last_operation.as_deref(), Some("import"));
        assert_eq!(
            status.last_operation_error.as_deref(),
            Some("import cancelled")
        );
        assert_eq!(status.error, None);
        assert!(!status.managed);
        assert!(!status.partial_managed);
        assert_eq!(status.bytes_done, 0);
        assert_eq!(*revisions.borrow(), initial.wrapping_add(2));
    }

    #[tokio::test]
    async fn revision_watch_observes_delete_start_and_completion() {
        let manager = Manager::new();
        let root = unused_root();
        let mut revisions = manager.subscribe_revision();
        let (unload_started, unload_waiter) = tokio::sync::oneshot::channel();
        let (finish_unload, unload_finished) = tokio::sync::oneshot::channel();
        let delete_manager = manager.clone();
        let delete_root = root.clone();
        let delete = tokio::spawn(async move {
            delete_manager
                .delete_with_unload(delete_root, || async move {
                    let _ = unload_started.send(());
                    unload_finished.await.map_err(|error| error.to_string())
                })
                .await
        });

        assert!(revisions.changed().await.is_ok());
        assert_eq!(manager.status(&root).operation.as_deref(), Some("delete"));
        unload_waiter.await.unwrap();
        finish_unload.send(()).unwrap();
        assert!(revisions.changed().await.is_ok());
        assert_eq!(
            manager.status(&root).last_operation.as_deref(),
            Some("delete")
        );
        assert_eq!(delete.await.unwrap(), Ok(()));
    }

    #[test]
    fn storage_mutation_is_blocked_while_a_runtime_lease_exists() {
        let manager = Manager::new();
        let root = unused_root();
        let lease = hold_lease(&manager);

        let error = manager
            .begin_operation(&root, "importing", "import")
            .unwrap_err();
        assert!(error.contains("unload it before importing"));

        drop(lease);
        let (generation, _, _) = manager
            .begin_operation(&root, "importing", "import")
            .unwrap();
        manager.cancel();
        manager.finish_operation(generation, Some("import cancelled".to_owned()), None);
        assert_eq!(manager.status(&root).state, "missing");
        assert_eq!(
            manager.status(&root).last_operation_error.as_deref(),
            Some("import cancelled")
        );
    }

    #[test]
    fn import_rejects_staging_sources_and_overlapping_parent_paths() {
        let manager = Manager::new();
        let root = unused_root();
        let cleanup = root.join("cleanup");
        let staging = staging_dir(&root);
        fs::create_dir_all(&staging).unwrap();
        let marker = staging.join("caller-data");
        fs::write(&marker, b"keep this source folder").unwrap();

        assert!(manager.start_import(root.clone(), staging.clone()).is_err());
        let staging_alias = staging.join("..").join(STAGING_DIR);
        assert!(manager.start_import(root.clone(), staging_alias).is_err());
        assert!(manager.start_import(root.clone(), cleanup).is_err());
        assert_eq!(fs::read(&marker).unwrap(), b"keep this source folder");
        remove_path(&root).unwrap();
    }

    #[tokio::test]
    async fn delete_closes_new_leases_then_waits_for_the_cleanup_owner() {
        let manager = Manager::new();
        let root = unused_root();
        let lease = hold_lease(&manager);
        let callback_manager = manager.clone();
        let callback_root = root.clone();

        manager
            .delete_with_unload(root.clone(), move || async move {
                assert_eq!(callback_manager.status(&callback_root).state, "deleting");
                assert!(callback_manager
                    .resolve_ready_inner(&callback_root, None)
                    .is_err());
                assert!(callback_manager
                    .begin_operation(&callback_root, "importing", "import")
                    .is_err());
                drop(lease);
                Ok(())
            })
            .await
            .unwrap();

        let status = manager.status(&root);
        assert_eq!(status.state, "missing");
        assert_eq!(status.last_operation.as_deref(), Some("delete"));
        assert_eq!(status.last_operation_error, None);
    }

    #[tokio::test]
    async fn tiny_http_download_commits_only_exact_size_and_hash() {
        let endpoint =
            crate::test_http::spawn_response(200, "application/octet-stream", b"hello", &[]).await;
        let staging = unused_root();
        ensure_private_dir(&staging).unwrap();
        let bytes_done = AtomicU64::new(0);
        let inventory = download_files(
            &test_client(),
            &TINY_FILES[..1],
            &staging,
            |_| endpoint.clone(),
            &CancellationToken::new(),
            &bytes_done,
        )
        .await
        .unwrap();

        assert_eq!(inventory.len(), 1);
        assert_eq!(inventory[0].sha256, TINY_FILES[0].sha256);
        assert_eq!(
            fs::read(staging.join(TINY_FILES[0].path)).unwrap(),
            b"hello"
        );
        assert_eq!(bytes_done.load(Ordering::Relaxed), 5);
        remove_path(&staging).unwrap();
    }

    #[tokio::test]
    async fn tiny_http_size_and_hash_failures_remove_the_unverified_temporary_file() {
        for (body, expected_reason) in [
            (&b"hell"[..], "size or checksum mismatch"),
            (&b"jello"[..], "size or checksum mismatch"),
        ] {
            let endpoint =
                crate::test_http::spawn_response(200, "application/octet-stream", body, &[]).await;
            let manager = Manager::new();
            let root = unused_root();
            ensure_model_parent(&root).unwrap();
            let staging = staging_dir(&root);
            ensure_private_dir(&staging).unwrap();
            let (generation, cancel, bytes_done) = manager
                .begin_operation(&root, "downloading", "download")
                .unwrap();
            let error = download_files(
                &test_client(),
                &TINY_FILES[..1],
                &staging,
                |_| endpoint.clone(),
                &cancel,
                &bytes_done,
            )
            .await
            .unwrap_err();

            assert!(error.contains(expected_reason));
            assert!(!staging.join(TINY_FILES[0].path).exists());
            assert!(!staging.join(".first.safetensors.download").exists());
            manager.finish_operation(generation, Some(error.clone()), None);
            let status = manager.status(&root);
            assert_eq!(status.state, "partial");
            assert_eq!(status.last_operation_error.as_deref(), Some(error.as_str()));
            remove_path(&root).unwrap();
        }
    }

    #[tokio::test]
    async fn stream_cancel_removes_inflight_file_keeps_verified_partial_and_resumes_by_hash() {
        let first_endpoint =
            crate::test_http::spawn_response(200, "application/octet-stream", b"hello", &[]).await;
        let staging = unused_root();
        ensure_private_dir(&staging).unwrap();
        let cancel = CancellationToken::new();
        let bytes_done = AtomicU64::new(0);
        download_files(
            &test_client(),
            &TINY_FILES[..1],
            &staging,
            |_| first_endpoint.clone(),
            &cancel,
            &bytes_done,
        )
        .await
        .unwrap();

        let (second_endpoint, first_chunk, continue_server, server_task) =
            spawn_chunked_test_response(b"wo", b"rld").await;
        let cancel_download = cancel.clone();
        let progress = Arc::new(AtomicU64::new(5));
        let download_progress = Arc::clone(&progress);
        let download_stage = staging.clone();
        let client = test_client();
        let download = tokio::spawn(async move {
            download_files(
                &client,
                &TINY_FILES,
                &download_stage,
                |path| {
                    if path == TINY_FILES[0].path {
                        "http://127.0.0.1:1/should-not-be-requested".to_owned()
                    } else {
                        second_endpoint.clone()
                    }
                },
                &cancel_download,
                &download_progress,
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(2), first_chunk)
            .await
            .expect("download server should send a partial chunk")
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while progress.load(Ordering::Relaxed) <= 5 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("partial chunk should be counted before cancellation");
        cancel.cancel();
        let error = download.await.unwrap().unwrap_err();
        let _ = continue_server.send(());
        let _ = server_task.await;

        assert_eq!(error, "download cancelled");
        assert_eq!(
            fs::read(staging.join(TINY_FILES[0].path)).unwrap(),
            b"hello"
        );
        assert!(!staging.join(TINY_FILES[1].path).exists());
        assert!(!staging.join(".second.safetensors.download").exists());
        assert!(progress.load(Ordering::Relaxed) > 5);

        fs::write(staging.join(TINY_FILES[0].path), b"jello").unwrap();
        let first_retry =
            crate::test_http::spawn_response(200, "application/octet-stream", b"hello", &[]).await;
        let second_retry =
            crate::test_http::spawn_response(200, "application/octet-stream", b"world", &[]).await;
        let inventory = download_files(
            &test_client(),
            &TINY_FILES,
            &staging,
            |path| {
                if path == TINY_FILES[0].path {
                    first_retry.clone()
                } else {
                    second_retry.clone()
                }
            },
            &CancellationToken::new(),
            &AtomicU64::new(0),
        )
        .await
        .unwrap();
        assert_eq!(inventory.len(), 2);
        assert_eq!(
            fs::read(staging.join(TINY_FILES[0].path)).unwrap(),
            b"hello"
        );
        assert_eq!(
            fs::read(staging.join(TINY_FILES[1].path)).unwrap(),
            b"world"
        );
        remove_path(&staging).unwrap();
    }
}
