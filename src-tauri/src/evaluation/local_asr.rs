//! Explicit immutable source -> fresh mutable MLX workspace. No downloads or ambient roots.
use super::artifacts::{create_directory, id};
use super::contracts::{RecognitionLanguage, RunRecord};
use super::validation::{error, external, InputError, ValidatedInputs};
use crate::ondevice_models::{mlx_model, QWEN_ASR_06_ID, QWEN_ASR_17_ID};
use crate::ondevice_runtime::evaluation_source::VerifiedModelSource;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

pub(super) const IDS: &[&str] = &["local_qwen3_asr_0_6b", "local_qwen3_asr_1_7b"];
pub(super) fn model_id(candidate: &str) -> Option<&'static str> {
    match candidate {
        "local_qwen3_asr_0_6b" => Some(QWEN_ASR_06_ID),
        "local_qwen3_asr_1_7b" => Some(QWEN_ASR_17_ID),
        _ => None,
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalModelIdentity {
    pub model_id: String,
    pub repository: String,
    pub revision: String,
    pub precision_label: String,
    pub loaded_precision: Option<String>,
    pub selected_source_root: String,
    pub canonical_source_root: String,
    pub working_root: String,
    pub working_root_identity: super::artifacts::ArtifactFileIdentity,
    pub source_integrity_ms: Option<f64>,
    pub materialization_ms: f64,
    pub files: Vec<LocalPinnedFile>,
    pub production_marker_sha256: String,
    pub derived_tokenizer_recipe: String,
    pub derived_tokenizer_sha256: Option<String>,
    pub derived_tokenizer_state: super::contracts::StageState,
    pub workspace_policy: String,
}
impl LocalModelIdentity {
    pub(super) fn observe_loaded(
        &mut self,
        runtime: Option<&crate::observations::LocalAsrObservation>,
    ) {
        self.loaded_precision = runtime
            .filter(|r| {
                r.observed_model_id.as_deref() == Some(self.model_id.as_str())
                    && r.observed_revision.as_deref() == Some(self.revision.as_str())
            })
            .and_then(|r| r.loaded_attestation.as_ref())
            .filter(|a| a.complete())
            .map(|a| serde_json::to_string(&a.parameter_dtypes).expect("dtype inventory"));
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalPinnedFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Debug)]
pub(super) struct Plan {
    selected_source: PathBuf,
    pub sources: BTreeMap<String, VerifiedModelSource>,
    working: Option<PathBuf>,
    parent: Option<File>,
    source_integrity_ms: BTreeMap<String, f64>,
}
impl Plan {
    pub(super) fn working_path(&self) -> Option<&Path> {
        self.working.as_deref()
    }
    pub fn admit(
        inputs: &ValidatedInputs,
        source: Option<&Path>,
        working: Option<&Path>,
        run: bool,
    ) -> Result<Option<Self>, InputError> {
        let models: std::collections::BTreeSet<_> = inputs
            .recipe
            .iter()
            .flat_map(|r| &r.jobs)
            .filter_map(|j| model_id(&j.candidate_id))
            .collect();
        if models.is_empty() {
            if source.is_some() || working.is_some() {
                return Err(error(Path::new("arguments"),"conflicting_arguments","model roots require a local ASR experiment job; legacy cloud controls do not consume local roots"));
            }
            return Ok(None);
        }
        let selected_source = source.ok_or_else(|| {
            error(
                Path::new("arguments"),
                "missing_models_root",
                "local ASR requires explicit read-only --models-root",
            )
        })?;
        // Required before even hashing inputs, resolving credentials or creating directories.
        if run && working.is_none() {
            return Err(error(
                Path::new("arguments"),
                "missing_models_work_root",
                "local --run requires a NEW exclusive --models-work-root",
            ));
        }
        let root = external(selected_source).map_err(|_| {
            error(
                selected_source,
                "local_model_missing",
                "explicit source model root is unavailable or not external",
            )
        })?;
        if !root.is_dir() {
            return Err(error(
                &root,
                "local_model_missing",
                "model root must be an existing directory",
            ));
        }
        let (working, parent) = if let Some(selected) = working {
            let p = external(
                selected
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )?;
            let name = selected.file_name().ok_or_else(|| {
                error(
                    selected,
                    "path_escape",
                    "working root needs a new directory name",
                )
            })?;
            let target = p.join(name);
            if target.starts_with(&root) || root.starts_with(&target) {
                return Err(error(
                    selected,
                    "path_escape",
                    "source and working roots must be disjoint",
                ));
            }
            if std::fs::symlink_metadata(&target).is_ok() {
                return Err(error(
                    selected,
                    "output_exists",
                    "working root already exists; fresh execution never reuses it",
                ));
            }
            let parent = File::open(&p).map_err(|_| {
                error(
                    selected,
                    "local_workspace_unavailable",
                    "cannot hold working parent",
                )
            })?;
            (Some(target), Some(parent))
        } else {
            (None, None)
        };
        if run && !crate::ondevice_asr::platform_supported() {
            return Err(error(&root,"local_platform_unsupported","local execution requires Apple Silicon macOS14+; validation alone does not load a runtime"));
        }
        if run && crate::ondevice_runtime::sidecar_executable().is_none() {
            return Err(error(&root,"local_runtime_unavailable","explicit or bundled sidecar executable is unavailable; no automatic install/download"));
        }
        let mut sources = BTreeMap::new();
        let mut source_integrity_ms = BTreeMap::new();
        for model in models {
            let manifest = mlx_model(model).expect("local candidate uses fixed manifest");
            for path in std::iter::once(root.join(model))
                .chain(manifest.files.iter().map(|f| root.join(model).join(f.path)))
            {
                let meta = std::fs::symlink_metadata(&path).map_err(|_| {
                    error(
                        &path,
                        "local_model_missing",
                        "declared pinned model path is missing",
                    )
                })?;
                if meta.file_type().is_symlink() {
                    return Err(error(
                        &path,
                        "path_escape",
                        "pinned model paths must not be symlinks",
                    ));
                }
            }
            let started = std::time::Instant::now();
            let snapshot = VerifiedModelSource::open(&root, model)
                .map_err(|reason| error(&root.join(model), "local_model_corrupt", reason))?;
            source_integrity_ms.insert(model.into(), started.elapsed().as_secs_f64() * 1000.0);
            sources.insert(model.into(), snapshot);
        }
        Ok(Some(Self {
            selected_source: selected_source.into(),
            sources,
            working,
            parent,
            source_integrity_ms,
        }))
    }
    pub fn source_summary(&self) -> serde_json::Value {
        serde_json::json!({"selected_root":self.selected_source,"canonical_roots":self.sources.values().map(|s|&s.root).collect::<Vec<_>>(),"models":self.sources.iter().map(|(id,source)|serde_json::json!({"model_id":id,"revision":mlx_model(id).map(|m|m.revision),"files_validated":true,"files":source.files(),"runtime_handshake":"not_run","model_loaded":"not_run","inference":"not_run"})).collect::<Vec<_>>()})
    }
    pub fn materialize(
        &self,
        cancel: &tokio_util::sync::CancellationToken,
        journal: &super::journal::RunJournal,
    ) -> Result<Workspace, super::ExecutionError> {
        use super::contracts::{SetupCopy, SetupWorkspace, StageState};
        let path = self.working.as_deref().unwrap_or(Path::new("arguments"));
        let failure = |reason: String| {
            journal.input_error(error(path, "local_materialization_failed", reason))
        };
        let check_cancel = || {
            if cancel.is_cancelled() {
                Err(setup_cancelled(journal, path))
            } else {
                Ok(())
            }
        };
        check_cancel()?;
        let parent = self
            .parent
            .as_ref()
            .ok_or_else(|| failure("working parent unavailable".into()))?;
        let parent_path = path
            .parent()
            .ok_or_else(|| failure("working root unavailable".into()))?;
        if id(&std::fs::metadata(parent_path).map_err(|e| failure(e.to_string()))?).ok()
            != id(&parent.metadata().map_err(|e| failure(e.to_string()))?).ok()
        {
            return Err(journal.input_error(error(
                path,
                "local_workspace_identity_changed",
                "working parent no longer names held directory",
            )));
        }
        check_cancel()?;
        journal.setup(|s| {
            s.workspace = Some(SetupWorkspace {
                selected_path: path.display().to_string(),
                held_identity: None,
                creation: StageState::Unknown {
                    reason: "exclusive_directory_creation_in_progress".into(),
                },
                path_applicable: None,
                retained: None,
            })
        });
        journal.persist()?;
        let root = create_directory(parent, path.file_name().unwrap()).map_err(|e| {
            journal.setup(|s| {
                if let Some(fact) = &mut s.workspace {
                    fact.creation = StageState::Failed {
                        reason: "exclusive_directory_create_or_open_failed".into(),
                    };
                }
            });
            journal.input_error(error(path, "local_workspace_create_failed", e.to_string()))
        })?;
        let mut workspace = Workspace {
            root: path.into(),
            handle: root,
            models: BTreeMap::new(),
            directories: vec![],
            runtime: None,
        };
        let held_identity = id(&workspace
            .handle
            .metadata()
            .map_err(|e| failure(e.to_string()))?)
        .map_err(|e| failure(e.to_string()))?
        .evidence();
        journal.setup(|s| {
            s.workspace = Some(SetupWorkspace {
                selected_path: path.display().to_string(),
                held_identity: Some(held_identity),
                creation: StageState::Completed {},
                path_applicable: workspace.path_applicable(),
                retained: Some(true),
            })
        });
        let result = (|| -> Result<(), super::ExecutionError> {
            journal.persist()?;
            workspace.check().map_err(|e| journal.input_error(e))?;
            check_cancel()?;
            let recipe = serde_json::json!({"artifact_type":"voiceflow_eval_local_workspace","schema_version":1,"state":"materializing","source_root":self.selected_source,"policy":"fresh_independent_copies_retained","derived_tokenizer_recipe":"existing Swift loader removes/regenerates tokenizer.json from pinned vocab/merges/tokenizer_config; derived bytes are not publisher files","models":self.sources.keys().collect::<Vec<_>>()});
            write_file(
                &workspace.handle,
                "workspace-recipe.json",
                &serde_json::to_vec_pretty(&recipe).map_err(|e| failure(e.to_string()))?,
            )
            .map_err(failure)?;
            for (model, source) in &self.sources {
                check_cancel()?;
                let copy_started = std::time::Instant::now();
                let manifest =
                    mlx_model(model).ok_or_else(|| failure("unsupported local model".into()))?;
                let dir = create_directory(&workspace.handle, std::ffi::OsStr::new(model))
                    .map_err(|e| failure(e.to_string()))?;
                for (index, (name, bytes, _)) in source.files().iter().enumerate() {
                    check_cancel()?;
                    let mut copy_index = 0;
                    journal.setup(|s| {
                        copy_index = s.copies.len();
                        s.copies.push(SetupCopy {
                            model_id: model.clone(),
                            relative_path: name.clone(),
                            expected_bytes: *bytes,
                            written_bytes: 0,
                            state: StageState::Unknown {
                                reason: "copy_in_progress".into(),
                            },
                        });
                    });
                    journal.persist()?;
                    let mut output = create_file(&dir, name).map_err(|e| failure(e.to_string()))?;
                    let copied = source.copy_file(index, &mut output, cancel, |written| {
                        journal.copy_progress(copy_index, written);
                    });
                    if let Err(error) = copied {
                        let error = match error {
                            crate::ondevice_runtime::evaluation_source::CopyFailure::Cancelled => {
                                setup_cancelled(journal, path)
                            }
                            crate::ondevice_runtime::evaluation_source::CopyFailure::Failed(
                                reason,
                            ) => failure(reason),
                        };
                        journal.setup(|s| s.copies[copy_index].state = error.terminal.clone());
                        return Err(error);
                    }
                    journal.setup(|s| s.copies[copy_index].state = StageState::Completed {});
                    journal.persist()?;
                }
                check_cancel()?;
                let marker = serde_json::json!({"id":model,"revision":manifest.revision,"files":source.files().iter().map(|(path,bytes,sha)|serde_json::json!({"path":path,"bytes":bytes,"sha256":sha})).collect::<Vec<_>>()});
                let marker_bytes =
                    serde_json::to_vec(&marker).map_err(|e| failure(e.to_string()))?;
                write_file(&dir, ".voiceflow-model-manifest.json", &marker_bytes)
                    .map_err(failure)?;
                dir.sync_all().map_err(|e| failure(e.to_string()))?;
                let working_root_identity = id(&workspace
                    .handle
                    .metadata()
                    .map_err(|e| failure(e.to_string()))?)
                .map_err(|e| failure(e.to_string()))?
                .evidence();
                workspace.models.insert(model.clone(), LocalModelIdentity {
                    model_id: model.clone(), repository: manifest.repository.into(), revision: manifest.revision.into(),
                    precision_label: "BF16 (production manifest label)".into(), loaded_precision: None,
                    selected_source_root: self.selected_source.display().to_string(), canonical_source_root: source.root.display().to_string(),
                    working_root: path.display().to_string(), working_root_identity,
                    source_integrity_ms: self.source_integrity_ms.get(model).copied(), materialization_ms: copy_started.elapsed().as_secs_f64()*1000.0,
                    files: source.files().iter().map(|(path,bytes,sha)|LocalPinnedFile {path:path.clone(),bytes:*bytes,sha256:sha.clone()}).collect(),
                    production_marker_sha256: super::validation::sha256(&marker_bytes),
                    derived_tokenizer_recipe: "Swift runtime-owned tokenizer.json regeneration; immutable source files copied independently".into(),
                    derived_tokenizer_sha256: None, derived_tokenizer_state: StageState::NotRun {}, workspace_policy: "fresh_exclusive_retained_no_resume".into(),
                });
                workspace.directories.push((model.clone(), dir));
            }
            check_cancel()?;
            write_file(
                &workspace.handle,
                "materialization-complete.json",
                &serde_json::to_vec(&workspace.models).map_err(|e| failure(e.to_string()))?,
            )
            .map_err(failure)?;
            workspace
                .handle
                .sync_all()
                .map_err(|e| failure(e.to_string()))?;
            workspace.check().map_err(|e| journal.input_error(e))?;
            check_cancel()?;
            Ok(())
        })();
        journal.setup(|s| {
            if let Some(fact) = &mut s.workspace {
                fact.path_applicable = workspace.path_applicable();
            }
        });
        result?;
        workspace.runtime = Some(crate::ondevice_runtime::shared_runtime(&workspace.root));
        Ok(workspace)
    }
}
pub(super) fn setup_cancelled(
    journal: &super::journal::RunJournal,
    path: &Path,
) -> super::ExecutionError {
    let mut error = journal.input_error(error(
        path,
        "setup_cancelled",
        "cooperative setup cancellation; owned partial workspace retained",
    ));
    error.terminal = super::contracts::StageState::Cancelled {
        reason: "setup_cancelled".into(),
    };
    error
}

pub(super) struct Workspace {
    pub root: PathBuf,
    handle: File,
    pub models: BTreeMap<String, LocalModelIdentity>,
    directories: Vec<(String, File)>,
    runtime: Option<std::sync::Arc<crate::ondevice_runtime::OnDeviceRuntime>>,
}
impl Workspace {
    fn path_applicable(&self) -> Option<bool> {
        let held = id(&self.handle.metadata().ok()?).ok()?;
        match std::fs::symlink_metadata(&self.root) {
            Ok(current) => Some(!current.file_type().is_symlink() && id(&current).ok()? == held),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(false),
            Err(_) => None,
        }
    }
    pub fn check(&self) -> Result<(), InputError> {
        let same = |path: &Path, handle: &File| -> bool {
            std::fs::symlink_metadata(path).is_ok_and(|m| {
                !m.file_type().is_symlink()
                    && m.is_dir()
                    && id(&m).ok() == handle.metadata().ok().and_then(|m| id(&m).ok())
            })
        };
        if !same(&self.root, &self.handle)
            || self
                .directories
                .iter()
                .any(|(model, dir)| !same(&self.root.join(model), dir))
        {
            return Err(error(&self.root,"local_workspace_identity_changed","working root/model path no longer names held owner; retained, never cleaned by path"));
        }
        Ok(())
    }
    pub fn attach(&self, report: &mut RunRecord) {
        for row in &mut report.cases {
            row.local_model = model_id(&row.requested.candidate_id)
                .and_then(|id| self.models.get(id))
                .cloned();
        }
    }
    pub fn observed_identity(&self, model: &str) -> Option<LocalModelIdentity> {
        let mut value = self.models.get(model)?.clone();
        let path = self.root.join(model).join("tokenizer.json");
        let read = (|| -> Result<String, &'static str> {
            self.check().map_err(|_| "workspace_identity_unknown")?;
            let before =
                std::fs::symlink_metadata(&path).map_err(|_| "derived_tokenizer_not_observed")?;
            if before.file_type().is_symlink()
                || !before.is_file()
                || before.len() > 32 * 1024 * 1024
            {
                return Err("derived_tokenizer_type_or_capacity_unavailable");
            }
            let mut input = File::open(&path).map_err(|_| "derived_tokenizer_unreadable")?;
            let opened = input
                .metadata()
                .map_err(|_| "derived_tokenizer_unreadable")?;
            if id(&before).ok() != id(&opened).ok() {
                return Err("derived_tokenizer_identity_changed");
            }
            use sha2::{Digest, Sha256};
            use std::io::Read;
            let mut raw = vec![];
            (&mut input)
                .take(32 * 1024 * 1024 + 1)
                .read_to_end(&mut raw)
                .map_err(|_| "derived_tokenizer_unreadable")?;
            let after = std::fs::symlink_metadata(&path)
                .map_err(|_| "derived_tokenizer_identity_changed")?;
            if raw.len() as u64 != before.len()
                || after.len() != before.len()
                || after.modified().ok() != before.modified().ok()
                || id(&after).ok() != id(&opened).ok()
            {
                return Err("derived_tokenizer_identity_changed");
            }
            Ok(hex::encode(Sha256::digest(raw)))
        })();
        match read {
            Ok(sha) => {
                value.derived_tokenizer_sha256 = Some(sha);
                value.derived_tokenizer_state = super::contracts::StageState::Completed {};
            }
            Err(reason) => {
                value.derived_tokenizer_state = super::contracts::StageState::Unknown {
                    reason: reason.into(),
                }
            }
        }
        Some(value)
    }
    pub async fn attest(&self, model: &str) -> Result<(), String> {
        self.check().map_err(|e| e.code.to_owned())?;
        let runtime = self.runtime.as_ref().ok_or("local_runtime_unavailable")?;
        runtime.preload(model).await?;
        self.check().map_err(|e| e.code.to_owned())?;
        Ok(())
    }
    pub async fn shutdown(&self) {
        let _ = self.shutdown_with_status().await;
    }
    pub async fn shutdown_with_status(&self) -> Result<(), &'static str> {
        if let Some(runtime) = &self.runtime {
            runtime.shutdown_with_status().await
        } else {
            Ok(())
        }
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        if let Some(runtime) = &self.runtime {
            runtime.begin_shutdown();
        }
    }
}
// Every mutation uses an owned directory descriptor, exclusive creation and
// O_NOFOLLOW. Failed or completed working trees are retained, never recursively
// removed; pathname replacement cannot authorize deleting somebody else's data.
#[cfg(unix)]
fn create_file(parent: &File, name: &str) -> std::io::Result<File> {
    use std::os::unix::io::{AsRawFd, FromRawFd};
    let name = std::ffi::CString::new(name)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
#[cfg(not(unix))]
fn create_file(_: &File, _: &str) -> std::io::Result<File> {
    Err(std::io::Error::other(
        "local workspaces require Unix directory ownership",
    ))
}
fn write_file(parent: &File, name: &str, bytes: &[u8]) -> Result<(), String> {
    let mut file = create_file(parent, name).map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}

pub(super) fn options(
    job: &super::contracts::JobV1,
) -> Result<crate::asr::AsrOptions, &'static str> {
    use super::contracts::{Context, Vocabulary};
    let id = model_id(&job.candidate_id).ok_or("unsupported local candidate")?;
    let provider = crate::ondevice_asr::OnDeviceAsrProvider::new(PathBuf::new(), id);
    use crate::asr::AsrProvider;
    let caps = provider.capabilities();
    if !matches!(job.vocabulary, Vocabulary::None {})
        || !matches!(job.context, Context::Disabled {})
    {
        return Err("local decoder has no vocabulary/context parameter; use explicit local post-replacement fixtures separately");
    }
    if caps.language_support != crate::asr::AsrLanguageSupport::AutoDetectOrFixedLanguage {
        return Err("local model does not support requested language factors");
    }
    Ok(crate::asr::AsrOptions {
        model: id.into(),
        language: match job.recognition_language {
            RecognitionLanguage::Auto => None,
            RecognitionLanguage::Zh => Some("zh".into()),
            RecognitionLanguage::En => Some("en".into()),
        },
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::FixtureDirectory;
    use super::*;
    fn materialize(plan: &Plan) -> Result<Workspace, super::super::ExecutionError> {
        let d = FixtureDirectory::new();
        let (_, journal) = super::super::tests::journal_fixture(&d);
        plan.materialize(&tokio_util::sync::CancellationToken::new(), &journal)
    }
    fn fixture_plan(source: &FixtureDirectory, output: &FixtureDirectory) -> Plan {
        let source_root = source.0.canonicalize().unwrap();
        let snapshot = VerifiedModelSource::fixture(&source_root, QWEN_ASR_06_ID, b"PINN");
        Plan {
            selected_source: source.0.clone(),
            sources: BTreeMap::from([(QWEN_ASR_06_ID.into(), snapshot)]),
            working: Some(output.0.canonicalize().unwrap().join("work")),
            parent: Some(File::open(&output.0).unwrap()),
            source_integrity_ms: BTreeMap::new(),
        }
    }
    #[test]
    fn workspace_uses_independent_copies_and_retains_replaced_or_partial_paths() {
        let source = FixtureDirectory::new();
        let output = FixtureDirectory::new();
        let plan = fixture_plan(&source, &output);
        let workspace = materialize(&plan).unwrap();
        let original = source.0.join(QWEN_ASR_06_ID).join("model.safetensors");
        let working = workspace
            .root
            .join(QWEN_ASR_06_ID)
            .join("model.safetensors");
        assert_eq!(std::fs::read(&working).unwrap(), b"PINN");
        assert_ne!(
            id(&std::fs::metadata(&working).unwrap()).unwrap(),
            id(&std::fs::metadata(&original).unwrap()).unwrap()
        );
        assert_eq!(
            workspace.models[QWEN_ASR_06_ID].files[0].sha256,
            super::super::validation::sha256(b"PINN")
        );
        std::fs::write(
            workspace.root.join(QWEN_ASR_06_ID).join("tokenizer.json"),
            b"derived tokenizer",
        )
        .unwrap();
        let identity = workspace.observed_identity(QWEN_ASR_06_ID).unwrap();
        assert_eq!(
            identity.derived_tokenizer_sha256.as_deref(),
            Some(super::super::validation::sha256(b"derived tokenizer").as_str())
        );
        assert!(!source
            .0
            .join(QWEN_ASR_06_ID)
            .join("tokenizer.json")
            .exists());
        assert_eq!(std::fs::read(&original).unwrap(), b"PINN");
        let selected = workspace.root.clone();
        let moved = selected.with_extension("moved");
        std::fs::rename(&selected, &moved).unwrap();
        std::fs::create_dir(&selected).unwrap();
        std::fs::write(selected.join("sentinel"), b"foreign").unwrap();
        assert_eq!(
            workspace.check().unwrap_err().code,
            "local_workspace_identity_changed"
        );
        drop(workspace);
        assert_eq!(
            std::fs::read(selected.join("sentinel")).unwrap(),
            b"foreign"
        );
        assert!(moved.exists());
        assert_eq!(
            materialize(&plan).err().unwrap().code,
            "local_workspace_create_failed"
        );
        assert_eq!(
            std::fs::read(selected.join("sentinel")).unwrap(),
            b"foreign"
        );
    }
    #[test]
    fn source_edit_between_admission_and_copy_fails_without_deleting_partial_workspace() {
        let source = FixtureDirectory::new();
        let output = FixtureDirectory::new();
        let plan = fixture_plan(&source, &output);
        std::fs::write(
            source.0.join(QWEN_ASR_06_ID).join("model.safetensors"),
            b"FAIL",
        )
        .unwrap();
        let failure = materialize(&plan).err().unwrap();
        assert_eq!(failure.code, "local_materialization_failed");
        let retained = output.0.join("work");
        assert!(retained.join("workspace-recipe.json").is_file());
        assert!(!retained.join("materialization-complete.json").exists());
        assert_eq!(
            std::fs::read(source.0.join(QWEN_ASR_06_ID).join("model.safetensors")).unwrap(),
            b"FAIL"
        );
    }
    #[test]
    fn local_controls_and_roots_fail_before_credentials_or_implicit_directories() {
        let d = FixtureDirectory::new();
        let (mut inputs, _) = super::super::tests::journal_fixture(&d);
        let job = &mut inputs.recipe.as_mut().unwrap().jobs[0];
        job.candidate_id = "local_qwen3_asr_0_6b".into();
        for (language, expected) in [
            (RecognitionLanguage::Auto, None),
            (RecognitionLanguage::Zh, Some("zh")),
            (RecognitionLanguage::En, Some("en")),
        ] {
            job.recognition_language = language;
            let options = options(job).unwrap();
            assert_eq!(options.language.as_deref(), expected);
            assert!(options.api_key.is_empty());
            assert!(options.prompt.is_none() && options.keywords.is_empty());
        }
        job.context = super::super::contracts::Context::Fixture {
            source_sha256: "0".repeat(64),
            relative_path: "unused.json".into(),
            permission_snapshot: super::super::contracts::PermissionSnapshot {
                accessibility: false,
                screen_recording: false,
                provider_text_granted: true,
            },
        };
        assert!(options(job).is_err());
        job.context = super::super::contracts::Context::Disabled {};
        assert_eq!(
            Plan::admit(&inputs, None, None, false).unwrap_err().code,
            "missing_models_root"
        );
        assert_eq!(
            Plan::admit(&inputs, Some(&d.0), None, true)
                .unwrap_err()
                .code,
            "missing_models_work_root"
        );
        assert_eq!(
            Plan::admit(&inputs, Some(&d.0), None, false)
                .unwrap_err()
                .code,
            "local_model_missing"
        );
        let existing = d.0.join("existing");
        std::fs::create_dir(&existing).unwrap();
        assert_eq!(
            Plan::admit(&inputs, Some(&d.0), Some(&existing), false)
                .unwrap_err()
                .code,
            "path_escape"
        );
    }
    #[tokio::test]
    async fn setup_cancel_after_first_write_joins_worker_keeps_partial_and_never_reads_keys() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let source = FixtureDirectory::new();
        let output = FixtureDirectory::new();
        let source_root = source.0.canonicalize().unwrap();
        let bytes = vec![17; 3 * 1024 * 1024];
        let snapshot = VerifiedModelSource::fixture(&source_root, QWEN_ASR_06_ID, &bytes);
        let plan = Plan {
            selected_source: source.0.clone(),
            sources: BTreeMap::from([(QWEN_ASR_06_ID.into(), snapshot)]),
            working: Some(output.0.canonicalize().unwrap().join("work")),
            parent: Some(File::open(&output.0).unwrap()),
            source_integrity_ms: BTreeMap::new(),
        };
        let (_, journal) = super::super::tests::journal_fixture(&output);
        let cancel = tokio_util::sync::CancellationToken::new();
        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_cancel = cancel.clone();
        let count = callback_count.clone();
        journal.on_copy_progress(Arc::new(move |written| {
            assert!(written > 0);
            count.fetch_add(1, Ordering::SeqCst);
            callback_cancel.cancel();
        }));
        let key_reads = AtomicUsize::new(0);
        let failure = super::super::runner::setup_execution(Some(plan), &journal, &cancel, || {
            key_reads.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
        .err()
        .unwrap();
        assert_eq!(failure.code, "setup_cancelled");
        let failure = journal.fail(failure, None);
        assert_eq!(failure.provider_attempts, 0);
        assert_eq!(key_reads.load(Ordering::SeqCst), 0);
        assert_eq!(callback_count.load(Ordering::SeqCst), 1);
        let fact = failure.setup.as_ref().unwrap();
        assert!(matches!(
            fact.worker_join,
            super::super::contracts::StageState::Completed {}
        ));
        assert!(matches!(
            fact.materialization,
            super::super::contracts::StageState::Cancelled { .. }
        ));
        assert!(matches!(
            fact.credentials,
            super::super::contracts::StageState::NotRun {}
        ));
        assert!(fact.cancellation_requested);
        assert_eq!(fact.workspace.as_ref().unwrap().retained, Some(true));
        let partial = output
            .0
            .join("work")
            .join(QWEN_ASR_06_ID)
            .join("model.safetensors");
        let length = std::fs::metadata(&partial).unwrap().len();
        assert!(length > 0 && length < bytes.len() as u64);
        assert_eq!(fact.copies[0].written_bytes, length);
        assert!(matches!(
            fact.copies[0].state,
            super::super::contracts::StageState::Cancelled { .. }
        ));
        let before = std::fs::read(&partial).unwrap();
        tokio::task::yield_now().await;
        assert_eq!(std::fs::read(&partial).unwrap(), before);
        assert!(!output.0.join("work/materialization-complete.json").exists());
        let saved: RunRecord =
            serde_json::from_slice(&std::fs::read(output.0.join("run.json")).unwrap()).unwrap();
        assert!(matches!(
            saved.terminal,
            super::super::contracts::StageState::Cancelled { .. }
        ));
        assert_eq!(saved.provider_attempts, 0);
        assert_eq!(saved.setup.unwrap().copies[0].written_bytes, length);
    }
    #[tokio::test]
    async fn setup_copy_failure_is_journaled_after_join_without_credentials_or_provider() {
        let source = FixtureDirectory::new();
        let output = FixtureDirectory::new();
        let plan = fixture_plan(&source, &output);
        std::fs::write(
            source.0.join(QWEN_ASR_06_ID).join("model.safetensors"),
            b"FAIL",
        )
        .unwrap();
        let (_, journal) = super::super::tests::journal_fixture(&output);
        let cancel = tokio_util::sync::CancellationToken::new();
        let failure = super::super::runner::setup_execution(
            Some(plan),
            &journal,
            &cancel,
            || -> Result<(), InputError> { panic!("copy failure must precede credential access") },
        )
        .await
        .err()
        .unwrap();
        assert_eq!(failure.code, "local_materialization_failed");
        let failure = journal.fail(failure, None);
        let setup = failure.setup.as_ref().unwrap();
        assert!(matches!(
            setup.worker_join,
            super::super::contracts::StageState::Completed {}
        ));
        assert!(matches!(
            setup.materialization,
            super::super::contracts::StageState::Failed { .. }
        ));
        assert_eq!(setup.copies[0].written_bytes, 0);
        assert!(matches!(
            setup.copies[0].state,
            super::super::contracts::StageState::Failed { .. }
        ));
        assert_eq!(setup.workspace.as_ref().unwrap().retained, Some(true));
        assert!(matches!(
            setup.credentials,
            super::super::contracts::StageState::NotRun {}
        ));
        assert!(std::fs::metadata(output.0.join("work")).unwrap().is_dir());
        assert_eq!(failure.provider_attempts, 0);
        let saved: RunRecord =
            serde_json::from_slice(&std::fs::read(output.0.join("run.json")).unwrap()).unwrap();
        assert!(matches!(
            saved.terminal,
            super::super::contracts::StageState::Failed { .. }
        ));
        assert_eq!(saved.provider_attempts, 0);
    }
    #[tokio::test]
    async fn setup_report_replacement_preserves_foreign_file_and_returns_partial_facts() {
        use std::sync::Arc;
        let source = FixtureDirectory::new();
        let output = FixtureDirectory::new();
        let plan = fixture_plan(&source, &output);
        let (_, journal) = super::super::tests::journal_fixture(&output);
        let report = output.0.join("run.json");
        let saved = output.0.join("saved-owned-report.json");
        journal.on_copy_progress(Arc::new(move |_| {
            std::fs::rename(&report, &saved).unwrap();
            std::fs::write(&report, b"foreign report sentinel").unwrap();
        }));
        let failure = super::super::runner::setup_execution(
            Some(plan),
            &journal,
            &tokio_util::sync::CancellationToken::new(),
            || -> Result<(), InputError> {
                panic!("report write failure must precede credentials")
            },
        )
        .await
        .err()
        .unwrap();
        let failure = journal.fail(failure, None);
        assert_eq!(failure.code, "report_error");
        assert_eq!(failure.provider_attempts, 0);
        let setup = failure.setup.as_ref().unwrap();
        assert!(matches!(
            setup.worker_join,
            super::super::contracts::StageState::Completed {}
        ));
        assert!(matches!(
            setup.materialization,
            super::super::contracts::StageState::Failed { .. }
        ));
        assert_eq!(setup.copies[0].written_bytes, 4);
        assert!(matches!(
            setup.copies[0].state,
            super::super::contracts::StageState::Completed {}
        ));
        assert!(matches!(
            setup.credentials,
            super::super::contracts::StageState::NotRun {}
        ));
        assert_eq!(setup.workspace.as_ref().unwrap().retained, Some(true));
        assert_eq!(
            std::fs::read(output.0.join("run.json")).unwrap(),
            b"foreign report sentinel"
        );
        assert_eq!(
            std::fs::read(
                output
                    .0
                    .join("work")
                    .join(QWEN_ASR_06_ID)
                    .join("model.safetensors")
            )
            .unwrap(),
            b"PINN"
        );
        assert!(!output.0.join("work/materialization-complete.json").exists());
    }
    #[tokio::test]
    async fn setup_credentials_failure_retains_completed_copy_and_never_invokes_adapter() {
        let source = FixtureDirectory::new();
        let output = FixtureDirectory::new();
        let plan = fixture_plan(&source, &output);
        let (_, journal) = super::super::tests::journal_fixture(&output);
        let failure = super::super::runner::setup_execution(
            Some(plan),
            &journal,
            &tokio_util::sync::CancellationToken::new(),
            || -> Result<(), InputError> {
                Err(error(
                    Path::new("explicit-key-file"),
                    "credential_unavailable",
                    "controlled key read failure",
                ))
            },
        )
        .await
        .err()
        .unwrap();
        assert_eq!(failure.code, "credential_unavailable");
        let failure = journal.fail(failure, None);
        let setup = failure.setup.as_ref().unwrap();
        assert!(matches!(
            setup.materialization,
            super::super::contracts::StageState::Completed {}
        ));
        assert!(matches!(
            setup.worker_join,
            super::super::contracts::StageState::Completed {}
        ));
        assert!(matches!(
            setup.credentials,
            super::super::contracts::StageState::Failed { .. }
        ));
        assert_eq!(setup.copies[0].written_bytes, 4);
        assert!(output.0.join("work/materialization-complete.json").exists());
        assert_eq!(failure.provider_attempts, 0);
    }
    #[tokio::test]
    async fn local_adapter_missing_model_needs_no_cloud_key_and_retains_typed_admission_failure() {
        let source = FixtureDirectory::new();
        let d = FixtureDirectory::new();
        let (inputs, _) = super::super::tests::journal_fixture(&d);
        let capture = crate::observations::FixtureCapture::new(false);
        let candidate = super::super::legacy::AsrCandidate::local(
            "local_qwen3_asr_0_6b",
            QWEN_ASR_06_ID,
            source.0.clone(),
            crate::asr::AsrOptions {
                model: QWEN_ASR_06_ID.into(),
                ..Default::default()
            },
        );
        let fixtures = super::super::legacy::AudioManifest {
            cases: vec![super::super::legacy::AudioFixture {
                id: "one".into(),
                audio: "one.wav".into(),
                reference: "fixture".into(),
                validated_audio: Some(inputs.cases[0].bytes.clone()),
            }],
        };
        let result = capture
            .scope(super::super::legacy::run_asr_candidate(
                &candidate,
                inputs.cases[0].document_path(),
                &fixtures,
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(result.provider_invoked_cases, 1);
        assert_eq!(result.key_source, "not_applicable_local");
        assert_eq!(result.provider, "ondevice");
        assert!(result.sanitized_host.is_empty());
        assert_eq!(result.cases[0].transcription_state, "provider_error");
        assert!(result.cases[0].raw_asr_text.is_none());
        let record = capture.snapshot();
        assert!(record.asr_request.is_none());
        let local = record.local_asr.unwrap();
        assert!(local
            .events
            .iter()
            .any(|e| e.reason.as_deref() == Some("local_model_missing")));
        assert!(local.observed_model_id.is_none() && local.coverage.is_none());
    }
}
