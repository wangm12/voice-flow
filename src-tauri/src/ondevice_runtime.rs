//! Local MLX sidecar process and bounded JSON-lines IPC.

use crate::ondevice_asr::{model_dir, model_files_are_ready, platform_supported};
use crate::ondevice_models::{mlx_model, MlxLanguagePolicy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

const PROTOCOL_VERSION: u32 = 1;
const MAX_RESPONSE_LINE_BYTES: usize = 64 * 1024;
const MAX_AUDIO_BYTES: usize = 30_000_000;
const MAX_AUDIO_DURATION_SECS: u64 = 15 * 60;
const MODEL_IDLE_UNLOAD_SECS: u64 = 15 * 60;

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeSnapshot {
    pub platform_supported: bool,
    pub runtime_status: String,
    pub runtime_version: Option<String>,
    pub loaded_model_id: Option<String>,
    pub loaded_model_version: Option<String>,
    pub loading_model_id: Option<String>,
    pub error: Option<String>,
}

impl RuntimeSnapshot {
    fn initial() -> Self {
        Self {
            platform_supported: platform_supported(),
            runtime_status: if platform_supported() {
                if sidecar_executable().is_some() {
                    "not_checked".to_owned()
                } else {
                    "sidecar_missing".to_owned()
                }
            } else {
                "unsupported_platform".to_owned()
            },
            runtime_version: None,
            loaded_model_id: None,
            loaded_model_version: None,
            loading_model_id: None,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Request {
    protocol_version: u32,
    request_id: String,
    session_id: Option<String>,
    model_version: Option<String>,
    op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_max_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens_per_chunk: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens_total: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
struct Response {
    protocol_version: u32,
    request_id: String,
    session_id: Option<String>,
    model_version: Option<String>,
    op: String,
    status: String,
    #[serde(default)]
    platform_supported: Option<bool>,
    #[serde(default)]
    runtime_version: Option<String>,
    #[serde(default)]
    loaded_model_id: Option<String>,
    #[serde(default)]
    loaded_model_version: Option<String>,
    #[serde(default)]
    result: Option<TranscriptionResult>,
    #[serde(default)]
    error: Option<SidecarFailure>,
}

#[derive(Debug, Clone, Deserialize)]
struct SidecarFailure {
    code: String,
    message: String,
}

impl Response {
    fn is_successful(&self) -> bool {
        matches!(
            self.status.as_str(),
            "ready" | "loaded" | "unloaded" | "completed"
        )
    }

    fn error_message(&self, fallback: &str) -> String {
        self.error
            .as_ref()
            .map(|error| format!("{}: {}", error.code, error.message))
            .unwrap_or_else(|| fallback.to_owned())
    }
}

#[derive(Debug, Clone, Deserialize)]
struct TranscriptionResult {
    text: String,
    #[serde(default)]
    language: Option<String>,
    generation_tokens: usize,
    covered_samples: u64,
    chunks_completed: usize,
    chunks_total: usize,
    completion: String,
}

struct ChildSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

pub struct OnDeviceRuntime {
    models_root: PathBuf,
    active_pid: AtomicU32,
    sequence: AtomicU64,
    load_generation: AtomicU64,
    shutting_down: AtomicBool,
    child_creation: Mutex<()>,
    loading_model_id: Mutex<Option<String>>,
    session: tokio::sync::Mutex<Option<ChildSession>>,
    status: Mutex<RuntimeSnapshot>,
    verified_files: Mutex<HashMap<String, Vec<FileIdentity>>>,
    idle_generation: AtomicU64,
    idle_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    path: String,
    bytes: u64,
    modified_ns: u128,
    device: u64,
    inode: u64,
}

impl OnDeviceRuntime {
    fn new(models_root: PathBuf) -> Self {
        Self {
            models_root,
            active_pid: AtomicU32::new(0),
            sequence: AtomicU64::new(0),
            load_generation: AtomicU64::new(0),
            shutting_down: AtomicBool::new(false),
            child_creation: Mutex::new(()),
            loading_model_id: Mutex::new(None),
            session: tokio::sync::Mutex::new(None),
            status: Mutex::new(RuntimeSnapshot::initial()),
            verified_files: Mutex::new(HashMap::new()),
            idle_generation: AtomicU64::new(0),
            idle_task: Mutex::new(None),
        }
    }

    pub async fn status(&self) -> RuntimeSnapshot {
        let current = self.snapshot();
        if self.shutting_down.load(Ordering::Acquire) || current.runtime_status == "loading" {
            return current;
        }
        if !platform_supported() {
            return self.update_status("unsupported_platform", None, None, None, None);
        }
        if sidecar_executable().is_none() {
            return self.update_status("sidecar_missing", None, None, None, None);
        }
        let mut session = match self.session.try_lock() {
            Ok(session) => session,
            Err(_) => return self.snapshot(),
        };
        if let Err(error) = self.ensure_session(&mut session, None).await {
            return self.update_status("runtime_failed", None, None, None, Some(error));
        }
        let request = self.request("status", None, None);
        let response = match Self::exchange(
            session.as_mut().expect("sidecar session is available"),
            &request,
            Duration::from_secs(5),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => {
                self.kill_session(&mut session);
                return self.update_status("runtime_failed", None, None, None, Some(error));
            }
        };
        if !self.response_matches(&request, &response) || !response.is_successful() {
            let message = response.error_message("sidecar status failed");
            self.kill_session(&mut session);
            return self.update_status("runtime_failed", None, None, None, Some(message));
        }
        let loaded_id = response.loaded_model_id.clone();
        let loaded_version = response.loaded_model_version.clone();
        self.update_status(
            if loaded_id.is_some() {
                "loaded"
            } else {
                "runtime_ready"
            },
            response.runtime_version,
            loaded_id,
            loaded_version,
            None,
        )
    }

    pub async fn transcribe(
        &self,
        model_id: &str,
        language: Option<&str>,
        audio: Vec<u8>,
    ) -> Result<(String, Option<String>), String> {
        self.ensure_running()?;
        let manifest = mlx_model(model_id).ok_or_else(|| "unsupported local model".to_owned())?;
        if !platform_supported() {
            return Err(
                "Local MLX transcription requires Apple Silicon and macOS 14 or later.".into(),
            );
        }
        if !model_files_are_ready(&self.models_root, model_id) {
            return Err("The selected local model files are missing or corrupt.".into());
        }
        let audio_samples = validate_wav(&audio)?;
        match manifest.language_policy {
            MlxLanguagePolicy::AutoOrExplicit => {
                if language.is_some_and(|value| !matches!(value, "zh" | "en")) {
                    return Err("Qwen local ASR accepts auto, Chinese, or English.".into());
                }
            }
            MlxLanguagePolicy::ExplicitChineseOrEnglish => {
                if !language.is_some_and(|value| matches!(value, "zh" | "en")) {
                    return Err("Cohere local ASR requires an explicit Chinese or English language setting.".into());
                }
            }
        }

        let request_id = self.next_id();
        let session_id = request_id.clone();
        let version = manifest.revision.to_owned();
        let directory = checked_model_directory(&self.models_root, model_id)?;
        let mut session = self.session.lock().await;
        let previous_status = self.snapshot();
        let loading_generation = self.begin_model_load(&mut session, model_id, &version)?;
        let mut load_guard = loading_generation.map(|generation| {
            ModelLoadCancellationGuard::new(self, model_id, generation, previous_status)
        });
        if let Err(error) = self.verify_model_integrity(model_id).await {
            if let Some(guard) = load_guard.as_mut() {
                guard.finish_failure(&error);
            } else {
                self.finish_model_load_failure(model_id, loading_generation, &error);
            }
            return Err(error);
        }
        if let Err(error) = self.ensure_session(&mut session, load_guard.as_mut()).await {
            if let Some(guard) = load_guard.as_mut() {
                guard.finish_failure(&error);
            } else {
                self.finish_model_load_failure(model_id, loading_generation, &error);
            }
            return Err(error);
        }
        let pid = session
            .as_ref()
            .and_then(|child| child.child.id())
            .unwrap_or(0);
        if let Some(guard) = load_guard.as_mut() {
            guard.record_child_pid(pid);
        }
        let mut kill_guard = KillOnDrop::new(pid, &self.active_pid);
        let child = session.as_mut().expect("sidecar session is available");
        if let Err(error) = self
            .ensure_model_loaded(
                child,
                model_id,
                &version,
                &directory,
                &request_id,
                &session_id,
                loading_generation,
            )
            .await
        {
            self.kill_session(&mut session);
            if let Some(guard) = load_guard.as_mut() {
                guard.finish_failure(&error);
            } else {
                self.finish_model_load_failure(model_id, loading_generation, &error);
            }
            return Err(error);
        }
        if let Some(guard) = load_guard.as_mut() {
            guard.disarm();
        }
        self.ensure_running()?;
        // Startup scavenging runs during ensure_session. Create this request's
        // private WAV only after that boundary and while holding the one-model
        // runtime lock so no other request can mistake it for stale audio.
        let audio_file = PrivateAudioFile::create(&self.models_root, &request_id, &audio)?;
        let audio_secs = audio_samples.div_ceil(16_000) as usize;
        let total_tokens = audio_secs.saturating_mul(128).clamp(4_096, 131_072);
        let request = Request {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            session_id: Some(session_id),
            model_version: Some(version.clone()),
            op: "transcribe".into(),
            model_id: Some(model_id.to_owned()),
            model_directory: None,
            audio_path: Some(audio_file.path.to_string_lossy().into_owned()),
            language: language.map(str::to_owned),
            audio_max_bytes: Some(MAX_AUDIO_BYTES),
            max_tokens_per_chunk: Some(4_096),
            max_tokens_total: Some(total_tokens),
        };
        let deadline = Duration::from_secs((audio_secs as u64 * 20).clamp(120, 1_800));
        let response = match Self::exchange(child, &request, deadline).await {
            Ok(response) => response,
            Err(error) => {
                self.kill_session(&mut session);
                self.update_status("runtime_failed", None, None, None, Some(error.clone()));
                return Err(error);
            }
        };
        if !self.response_matches(&request, &response) || !response.is_successful() {
            let message = response.error_message("local inference failed");
            self.kill_session(&mut session);
            self.update_status("runtime_failed", None, None, None, Some(message.clone()));
            return Err(message);
        }
        let Some(result) = response.result else {
            self.kill_session(&mut session);
            let error = "local runtime omitted transcription result".to_owned();
            self.update_status("runtime_failed", None, None, None, Some(error.clone()));
            return Err(error);
        };
        if result.completion != "complete"
            || result.covered_samples != audio_samples
            || result.chunks_completed == 0
            || result.chunks_completed != result.chunks_total
            || result.generation_tokens == 0
            || result.generation_tokens > total_tokens
            || result.text.trim().is_empty()
        {
            self.kill_session(&mut session);
            let error = "Local transcription did not complete the full audio; the recording remains available for retry.";
            self.update_status("runtime_failed", None, None, None, Some(error.to_owned()));
            return Err(error.into());
        }
        kill_guard.disarm();
        self.update_status(
            "loaded",
            response.runtime_version,
            Some(model_id.to_owned()),
            Some(version),
            None,
        );
        schedule_idle_unload(&self.models_root, model_id);
        Ok((result.text, result.language))
    }

    pub async fn preload(&self, model_id: &str) -> Result<(), String> {
        self.ensure_running()?;
        let manifest = mlx_model(model_id).ok_or_else(|| "unsupported local model".to_owned())?;
        if !platform_supported() {
            return Err(
                "Local MLX transcription requires Apple Silicon and macOS 14 or later.".into(),
            );
        }
        if !model_files_are_ready(&self.models_root, model_id) {
            return Err("The selected local model files are missing or corrupt.".into());
        }
        let version = manifest.revision.to_owned();
        let directory = checked_model_directory(&self.models_root, model_id)?;
        let request_id = self.next_id();
        let session_id = request_id.clone();
        let mut session = self.session.lock().await;
        let previous_status = self.snapshot();
        let loading_generation = self.begin_model_load(&mut session, model_id, &version)?;
        let mut load_guard = loading_generation.map(|generation| {
            ModelLoadCancellationGuard::new(self, model_id, generation, previous_status)
        });
        if let Err(error) = self.verify_model_integrity(model_id).await {
            if let Some(guard) = load_guard.as_mut() {
                guard.finish_failure(&error);
            } else {
                self.finish_model_load_failure(model_id, loading_generation, &error);
            }
            return Err(error);
        }
        if let Err(error) = self.ensure_session(&mut session, load_guard.as_mut()).await {
            if let Some(guard) = load_guard.as_mut() {
                guard.finish_failure(&error);
            } else {
                self.finish_model_load_failure(model_id, loading_generation, &error);
            }
            return Err(error);
        }
        let pid = session
            .as_ref()
            .and_then(|child| child.child.id())
            .unwrap_or(0);
        if let Some(guard) = load_guard.as_mut() {
            guard.record_child_pid(pid);
        }
        let mut kill_guard = KillOnDrop::new(pid, &self.active_pid);
        let child = session.as_mut().expect("sidecar session is available");
        if let Err(error) = self
            .ensure_model_loaded(
                child,
                model_id,
                &version,
                &directory,
                &request_id,
                &session_id,
                loading_generation,
            )
            .await
        {
            self.kill_session(&mut session);
            if let Some(guard) = load_guard.as_mut() {
                guard.finish_failure(&error);
            } else {
                self.finish_model_load_failure(model_id, loading_generation, &error);
            }
            return Err(error);
        }
        if let Some(guard) = load_guard.as_mut() {
            guard.disarm();
        }
        self.ensure_running()?;
        kill_guard.disarm();
        schedule_idle_unload(&self.models_root, model_id);
        Ok(())
    }

    pub async fn unload(&self, model_id: Option<&str>) -> Result<(), String> {
        self.ensure_running()?;
        let mut session = self.session.lock().await;
        self.unload_locked(&mut session, model_id).await
    }

    async fn unload_if_idle(&self, expected_generation: u64, model_id: &str) -> Result<(), String> {
        let mut session = self.session.lock().await;
        if self.idle_generation.load(Ordering::Acquire) != expected_generation
            || self.snapshot().loaded_model_id.as_deref() != Some(model_id)
        {
            return Ok(());
        }
        self.unload_locked(&mut session, Some(model_id)).await
    }

    async fn unload_locked(
        &self,
        session: &mut Option<ChildSession>,
        model_id: Option<&str>,
    ) -> Result<(), String> {
        if session.is_none() {
            return Ok(());
        }
        let snapshot = self.snapshot();
        if let (Some(requested), Some(loaded)) = (model_id, snapshot.loaded_model_id.as_deref()) {
            if requested != loaded {
                return Ok(());
            }
        }
        if snapshot.loaded_model_id.is_none() {
            return Ok(());
        }
        let unload_id = model_id.or(snapshot.loaded_model_id.as_deref());
        let request = self.request(
            "unload",
            unload_id,
            snapshot.loaded_model_version.as_deref(),
        );
        let response = Self::exchange(
            session.as_mut().expect("checked sidecar session"),
            &request,
            Duration::from_secs(60),
        )
        .await?;
        if !self.response_matches(&request, &response) || !response.is_successful() {
            return Err(response.error_message("local model unload failed"));
        }
        self.update_status("runtime_ready", response.runtime_version, None, None, None);
        Ok(())
    }

    pub async fn shutdown(&self) {
        self.begin_shutdown();
        let Ok(mut session) =
            tokio::time::timeout(Duration::from_secs(8), self.session.lock()).await
        else {
            log::warn!("timed out waiting for local runtime shutdown lock");
            return;
        };
        let Some(child) = session.as_mut() else {
            self.update_status("not_checked", None, None, None, None);
            return;
        };
        let _ = child.child.start_kill();
        if tokio::time::timeout(Duration::from_secs(3), child.child.wait())
            .await
            .is_err()
        {
            let _ = child.child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.child.wait()).await;
        }
        *session = None;
        self.active_pid.store(0, Ordering::Release);
        self.update_status("not_checked", None, None, None, None);
    }

    /// Seal child creation before normal app exit and synchronously terminate
    /// an active sidecar so a request holding the async session lock can exit.
    pub fn begin_shutdown(&self) {
        let _creation = self
            .child_creation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }
        self.load_generation.fetch_add(1, Ordering::AcqRel);
        *self
            .loading_model_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.update_status("shutting_down", None, None, None, None);
        let pid = self.active_pid.swap(0, Ordering::AcqRel);
        kill_process(pid);
    }

    pub fn cancel_model_load(&self, model_id: &str) -> bool {
        let _creation = self
            .child_creation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.shutting_down.load(Ordering::Acquire) {
            return false;
        }
        let mut loading = self
            .loading_model_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if loading.as_deref() != Some(model_id) {
            return false;
        }
        self.load_generation.fetch_add(1, Ordering::AcqRel);
        *loading = None;
        drop(loading);
        let pid = self.active_pid.swap(0, Ordering::AcqRel);
        kill_process(pid);
        self.update_status("not_checked", None, None, None, None);
        true
    }

    fn ensure_running(&self) -> Result<(), String> {
        if self.shutting_down.load(Ordering::Acquire) {
            Err("The local MLX runtime is shutting down.".into())
        } else {
            Ok(())
        }
    }

    fn begin_model_load(
        &self,
        session: &mut Option<ChildSession>,
        model_id: &str,
        model_version: &str,
    ) -> Result<Option<u64>, String> {
        self.ensure_running()?;
        let snapshot = self.snapshot();
        let live_session = session.as_mut().is_some_and(|session| {
            session
                .child
                .id()
                .is_some_and(|pid| self.active_pid.load(Ordering::Acquire) == pid)
                && session
                    .child
                    .try_wait()
                    .is_ok_and(|status| status.is_none())
        });
        if snapshot.runtime_status == "loaded"
            && snapshot.loaded_model_id.as_deref() == Some(model_id)
            && snapshot.loaded_model_version.as_deref() == Some(model_version)
            && live_session
        {
            return Ok(None);
        }
        let mut loading = self
            .loading_model_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.ensure_running()?;
        let generation = self.load_generation.fetch_add(1, Ordering::AcqRel) + 1;
        *loading = Some(model_id.to_owned());
        let mut snapshot = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        snapshot.runtime_status = "loading".into();
        snapshot.loading_model_id = Some(model_id.to_owned());
        snapshot.error = None;
        drop(snapshot);
        drop(loading);
        Ok(Some(generation))
    }

    fn load_is_current(&self, generation: u64) -> bool {
        !self.shutting_down.load(Ordering::Acquire)
            && self.load_generation.load(Ordering::Acquire) == generation
    }

    fn is_model_loading(&self) -> bool {
        self.loading_model_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

    fn clear_loading_model(&self, generation: Option<u64>) {
        let Some(generation) = generation else {
            return;
        };
        if self.load_generation.load(Ordering::Acquire) != generation {
            return;
        }
        *self
            .loading_model_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    fn finish_model_load_failure(&self, model_id: &str, generation: Option<u64>, error: &str) {
        if self.shutting_down.load(Ordering::Acquire)
            || generation.is_some_and(|generation| {
                !self.load_is_current(generation)
                    || self
                        .loading_model_id
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .as_deref()
                        != Some(model_id)
            })
        {
            return;
        }
        self.clear_loading_model(generation);
        self.update_status("runtime_failed", None, None, None, Some(error.to_owned()));
    }

    fn cancel_dropped_model_load(
        &self,
        model_id: &str,
        generation: u64,
        child_pid: Option<u32>,
        previous_status: &RuntimeSnapshot,
    ) {
        let mut loading = self
            .loading_model_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.shutting_down.load(Ordering::Acquire)
            || self.load_generation.load(Ordering::Acquire) != generation
            || loading.as_deref() != Some(model_id)
        {
            return;
        }
        *loading = None;

        if let Some(pid) = child_pid {
            if self
                .active_pid
                .compare_exchange(pid, 0, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                kill_process(pid);
            }
        }

        let mut status = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if child_pid.is_some() {
            *status = RuntimeSnapshot::initial();
        } else {
            let mut restored = previous_status.clone();
            restored.loading_model_id = None;
            if restored.runtime_status == "loading" {
                restored.runtime_status = if restored.loaded_model_id.is_some() {
                    "loaded".to_owned()
                } else {
                    "not_checked".to_owned()
                };
            }
            *status = restored;
        }
    }

    fn request(&self, op: &str, model_id: Option<&str>, model_version: Option<&str>) -> Request {
        Request {
            protocol_version: PROTOCOL_VERSION,
            request_id: self.next_id(),
            session_id: None,
            model_version: model_version.map(str::to_owned),
            op: op.to_owned(),
            model_id: model_id.map(str::to_owned),
            model_directory: None,
            audio_path: None,
            language: None,
            audio_max_bytes: None,
            max_tokens_per_chunk: None,
            max_tokens_total: None,
        }
    }

    fn next_id(&self) -> String {
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("vf-{}-{seq}-{nanos}", std::process::id())
    }

    async fn ensure_session(
        &self,
        session: &mut Option<ChildSession>,
        load_guard: Option<&mut ModelLoadCancellationGuard<'_>>,
    ) -> Result<(), String> {
        let expected_load_generation = load_guard.as_ref().map(|guard| guard.generation);
        self.ensure_running()?;
        if expected_load_generation.is_some_and(|generation| !self.load_is_current(generation)) {
            return Err("Local model load was cancelled.".into());
        }
        if let Some(existing) = session.as_mut() {
            let pid = existing.child.id().unwrap_or(0);
            if pid == 0 || self.active_pid.load(Ordering::Acquire) != pid {
                let _ = existing.child.start_kill();
                *session = None;
            } else {
                match existing.child.try_wait() {
                    Ok(None) => return Ok(()),
                    Ok(Some(_)) => *session = None,
                    Err(error) => {
                        *session = None;
                        return Err(error.to_string());
                    }
                }
            }
        }
        if !platform_supported() {
            return Err(
                "Local MLX transcription requires Apple Silicon and macOS 14 or later.".into(),
            );
        }
        let executable = sidecar_executable()
            .ok_or_else(|| "The VoiceFlow MLX runtime is not included in this build.".to_owned())?;
        std::fs::create_dir_all(&self.models_root).map_err(|error| error.to_string())?;
        set_private_directory(&self.models_root)?;
        let audio_root = self.models_root.join("audio-tmp");
        std::fs::create_dir_all(&audio_root).map_err(|error| error.to_string())?;
        let audio_metadata =
            std::fs::symlink_metadata(&audio_root).map_err(|error| error.to_string())?;
        if audio_metadata.file_type().is_symlink() || !audio_metadata.is_dir() {
            return Err("private audio storage is unavailable".into());
        }
        set_private_directory(&audio_root)?;
        cleanup_private_audio_files(&audio_root);
        let creation = self
            .child_creation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.ensure_running()?;
        if expected_load_generation.is_some_and(|generation| !self.load_is_current(generation)) {
            return Err("Local model load was cancelled.".into());
        }
        let mut child = Command::new(executable)
            .arg("--models-root")
            .arg(&self.models_root)
            .arg("--audio-root")
            .arg(&audio_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("Could not start the local MLX runtime: {error}"))?;
        let pid = child.id().unwrap_or(0);
        if let Some(guard) = load_guard {
            guard.record_child_pid(pid);
        }
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "local runtime stdin is unavailable".to_owned())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "local runtime stdout is unavailable".to_owned())?;
        *session = Some(ChildSession {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        });
        self.active_pid.store(pid, Ordering::Release);
        drop(creation);
        let request = self.request("hello", None, None);
        let response = match Self::exchange(
            session.as_mut().expect("new sidecar session is available"),
            &request,
            Duration::from_secs(8),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => {
                self.kill_session(session);
                return Err(error);
            }
        };
        if !self.response_matches(&request, &response)
            || !response.is_successful()
            || response.platform_supported != Some(true)
        {
            let message = response.error_message("local MLX runtime is unsupported or unavailable");
            self.kill_session(session);
            return Err(message);
        }
        if self.is_model_loading() {
            self.update_status("loading", response.runtime_version, None, None, None);
        } else {
            self.update_status("runtime_ready", response.runtime_version, None, None, None);
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "Keep model identity, protocol session, and cancellable load generation explicit."
    )]
    async fn ensure_model_loaded(
        &self,
        child: &mut ChildSession,
        model_id: &str,
        model_version: &str,
        directory: &Path,
        request_id: &str,
        session_id: &str,
        loading_generation: Option<u64>,
    ) -> Result<(), String> {
        self.ensure_running()?;
        let snapshot = self.snapshot();
        if snapshot.loaded_model_id.as_deref() == Some(model_id)
            && snapshot.loaded_model_version.as_deref() == Some(model_version)
        {
            return Ok(());
        }
        if loading_generation.is_some_and(|generation| !self.load_is_current(generation)) {
            return Err("Local model load was cancelled.".into());
        }
        if snapshot.loaded_model_id.is_some() {
            let unload = Request {
                protocol_version: PROTOCOL_VERSION,
                request_id: self.next_id(),
                session_id: None,
                model_version: snapshot.loaded_model_version,
                op: "unload".into(),
                model_id: snapshot.loaded_model_id,
                model_directory: None,
                audio_path: None,
                language: None,
                audio_max_bytes: None,
                max_tokens_per_chunk: None,
                max_tokens_total: None,
            };
            let response = Self::exchange(child, &unload, Duration::from_secs(60)).await?;
            if !self.response_matches(&unload, &response) || !response.is_successful() {
                return Err(response.error_message("could not release the previous local model"));
            }
        }
        if loading_generation.is_some_and(|generation| !self.load_is_current(generation)) {
            return Err("Local model load was cancelled.".into());
        }
        let canonical = directory
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let request = Request {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.to_owned(),
            session_id: Some(session_id.to_owned()),
            model_version: Some(model_version.to_owned()),
            op: "load".into(),
            model_id: Some(model_id.to_owned()),
            model_directory: Some(canonical.to_string_lossy().into_owned()),
            audio_path: None,
            language: None,
            audio_max_bytes: None,
            max_tokens_per_chunk: None,
            max_tokens_total: None,
        };
        let response = Self::exchange(child, &request, Duration::from_secs(300)).await?;
        if !self.response_matches(&request, &response) || !response.is_successful() {
            return Err(response.error_message("local model could not be loaded"));
        }
        if loading_generation.is_some_and(|generation| !self.load_is_current(generation)) {
            return Err("Local model load was cancelled.".into());
        }
        self.clear_loading_model(loading_generation);
        self.update_status(
            "loaded",
            response.runtime_version,
            Some(model_id.to_owned()),
            Some(model_version.to_owned()),
            None,
        );
        Ok(())
    }

    async fn exchange(
        child: &mut ChildSession,
        request: &Request,
        deadline: Duration,
    ) -> Result<Response, String> {
        let mut encoded = serde_json::to_vec(request).map_err(|error| error.to_string())?;
        encoded.push(b'\n');
        tokio::time::timeout(deadline, async {
            child.stdin.write_all(&encoded).await?;
            child.stdin.flush().await?;
            let mut line = Vec::new();
            let mut bytes = 0usize;
            loop {
                let available = child.stdout.fill_buf().await?;
                if available.is_empty() {
                    break;
                }
                let line_end = available.iter().position(|byte| *byte == b'\n');
                let take = line_end.map_or(available.len(), |end| end + 1);
                if bytes.saturating_add(take) > MAX_RESPONSE_LINE_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "sidecar response is too large",
                    ));
                }
                line.extend_from_slice(&available[..take]);
                child.stdout.consume(take);
                bytes += take;
                if line_end.is_some() {
                    break;
                }
            }
            if bytes == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "sidecar exited",
                ));
            }
            if line.len() > MAX_RESPONSE_LINE_BYTES || !line.ends_with(b"\n") {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "sidecar response is too large",
                ));
            }
            serde_json::from_slice::<Response>(&line)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid sidecar response"))
        })
        .await
        .map_err(|_| "local runtime request timed out".to_owned())?
        .map_err(|error| error.to_string())
    }

    fn response_matches(&self, request: &Request, response: &Response) -> bool {
        response.protocol_version == PROTOCOL_VERSION
            && response.request_id == request.request_id
            && response.session_id == request.session_id
            && response.model_version == request.model_version
            && response.op == request.op
    }

    async fn verify_model_integrity(&self, model_id: &str) -> Result<(), String> {
        let manifest = mlx_model(model_id).ok_or_else(|| "unsupported local model".to_owned())?;
        let root = self.models_root.clone();
        let id = model_id.to_owned();
        let expected: Vec<(String, u64, String)> = manifest
            .files
            .iter()
            .map(|file| (file.path.to_owned(), file.bytes, file.sha256.to_owned()))
            .collect();
        let cached = self
            .verified_files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(model_id)
            .cloned();
        let identities = tokio::task::spawn_blocking(move || {
            verify_pinned_model_files(&root, &id, &expected, cached.as_deref())
        })
        .await
        .map_err(|_| "model integrity verification was interrupted".to_owned())??;
        let mut verified = self
            .verified_files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        verified.insert(model_id.to_owned(), identities);
        Ok(())
    }

    fn kill_session(&self, session: &mut Option<ChildSession>) {
        if let Some(child) = session.as_mut() {
            let _ = child.child.start_kill();
        }
        *session = None;
        self.active_pid.store(0, Ordering::Release);
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn update_status(
        &self,
        runtime_status: &str,
        runtime_version: Option<String>,
        loaded_model_id: Option<String>,
        loaded_model_version: Option<String>,
        error: Option<String>,
    ) -> RuntimeSnapshot {
        let loading_model_id = self
            .loading_model_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let snapshot = RuntimeSnapshot {
            platform_supported: platform_supported(),
            runtime_status: if self.shutting_down.load(Ordering::Acquire) {
                "shutting_down".to_owned()
            } else if loading_model_id.is_some() && runtime_status == "runtime_ready" {
                "loading".to_owned()
            } else {
                runtime_status.to_owned()
            },
            runtime_version,
            loaded_model_id,
            loaded_model_version,
            loading_model_id,
            error: error.map(|message| sanitize_runtime_error(&message)),
        };
        *self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshot.clone();
        snapshot
    }
}

impl Drop for OnDeviceRuntime {
    fn drop(&mut self) {
        if let Some(task) = self
            .idle_task
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            task.abort();
        }
        let pid = self.active_pid.load(Ordering::Acquire);
        kill_process(pid);
    }
}

pub fn shared_runtime(models_root: &Path) -> Arc<OnDeviceRuntime> {
    static RUNTIMES: OnceLock<Mutex<HashMap<PathBuf, Arc<OnDeviceRuntime>>>> = OnceLock::new();
    let runtimes = RUNTIMES.get_or_init(|| Mutex::new(HashMap::new()));
    let key = models_root.to_path_buf();
    let mut map = runtimes
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    map.entry(key.clone())
        .or_insert_with(|| Arc::new(OnDeviceRuntime::new(key)))
        .clone()
}

fn schedule_idle_unload(models_root: &Path, model_id: &str) {
    let runtime = shared_runtime(models_root);
    let generation = runtime.idle_generation.fetch_add(1, Ordering::AcqRel) + 1;
    let model_id = model_id.to_owned();
    let task_runtime = runtime.clone();
    let task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(MODEL_IDLE_UNLOAD_SECS)).await;
        if let Err(error) = task_runtime.unload_if_idle(generation, &model_id).await {
            log::debug!("idle local model unload failed: {error}");
        }
    });
    let previous = {
        runtime
            .idle_task
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .replace(task)
    };
    if let Some(previous) = previous {
        previous.abort();
    }
}

pub fn sidecar_executable() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("VOICEFLOW_MLX_SIDECAR_PATH") {
        let path = PathBuf::from(path);
        return executable_file(&path).then_some(path);
    }
    let current = std::env::current_exe().ok()?;
    let parent = current.parent()?;
    let candidates = [
        parent.join("voiceflow-mlx-sidecar"),
        parent.join("../Resources/voiceflow-mlx-sidecar"),
        parent.join("../Helpers/voiceflow-mlx-sidecar"),
        parent.join("Resources/voiceflow-mlx-sidecar"),
    ];
    candidates
        .into_iter()
        .find(|path| executable_file(path))
        .and_then(|path| path.canonicalize().ok())
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn checked_model_directory(models_root: &Path, model_id: &str) -> Result<PathBuf, String> {
    let root = models_root
        .canonicalize()
        .map_err(|error| format!("model cache is unavailable: {error}"))?;
    let model = model_dir(models_root, model_id)
        .canonicalize()
        .map_err(|error| format!("model directory is unavailable: {error}"))?;
    if !model.starts_with(&root) || model.parent() != Some(root.as_path()) {
        return Err("model directory escaped the VoiceFlow model cache".into());
    }
    Ok(model)
}

fn validate_wav(bytes: &[u8]) -> Result<u64, String> {
    if bytes.len() > MAX_AUDIO_BYTES {
        return Err("local ASR audio exceeds the supported size limit".into());
    }
    let mut reader = hound::WavReader::new(std::io::Cursor::new(bytes))
        .map_err(|_| "local ASR requires a complete PCM WAV recording".to_owned())?;
    let spec = reader.spec();
    if spec.sample_rate != 16_000
        || spec.channels != 1
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
    {
        return Err("local ASR requires mono 16 kHz 16-bit PCM WAV audio".into());
    }
    let duration = reader.duration() as u64;
    if duration == 0 || duration > MAX_AUDIO_DURATION_SECS * 16_000 {
        return Err("local ASR audio is empty or exceeds the 15-minute limit".into());
    }
    for sample in reader.samples::<i16>() {
        sample.map_err(|_| "local ASR audio WAV is incomplete or corrupt".to_owned())?;
    }
    Ok(duration)
}

fn verify_pinned_model_files(
    models_root: &Path,
    model_id: &str,
    expected: &[(String, u64, String)],
    cached: Option<&[FileIdentity]>,
) -> Result<Vec<FileIdentity>, String> {
    let directory = checked_model_directory(models_root, model_id)?;
    let allowed_weights: std::collections::HashSet<&str> = expected
        .iter()
        .map(|(relative, _, _)| relative.as_str())
        .filter(|relative| relative.ends_with(".safetensors"))
        .collect();
    let entries = std::fs::read_dir(&directory)
        .map_err(|_| "the pinned local model directory could not be inspected".to_owned())?;
    for entry in entries {
        let entry = entry
            .map_err(|_| "the pinned local model directory could not be inspected".to_owned())?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err("the pinned local model directory contains an invalid filename".into());
        };
        if name.ends_with(".safetensors") && !allowed_weights.contains(name) {
            return Err("the local model directory contains an unpinned weights file".into());
        }
    }
    let mut identities = Vec::with_capacity(expected.len());
    for (relative, expected_bytes, _) in expected {
        let path = directory.join(relative);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|_| "a pinned local model file is missing".to_owned())?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() != *expected_bytes
        {
            return Err("a pinned local model file has an unexpected type or size".into());
        }
        identities.push(file_identity(relative, &metadata));
    }
    if cached.is_some_and(|cached| cached == identities.as_slice()) {
        return Ok(identities);
    }

    use std::io::Read;
    for ((relative, expected_bytes, expected_sha), identity) in expected.iter().zip(&identities) {
        let path = directory.join(relative);
        let before = std::fs::symlink_metadata(&path)
            .map_err(|_| "a pinned local model file changed during verification".to_owned())?;
        if before.file_type().is_symlink() || file_identity(relative, &before) != *identity {
            return Err("a pinned local model file changed during verification".into());
        }
        let mut file = std::fs::File::open(&path)
            .map_err(|_| "a pinned local model file could not be opened".to_owned())?;
        let opened = file
            .metadata()
            .map_err(|_| "a pinned local model file could not be inspected".to_owned())?;
        if !opened.is_file() || file_identity(relative, &opened) != *identity {
            return Err("a pinned local model file changed during verification".into());
        }
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 1024 * 1024];
        let mut total = 0u64;
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|_| "a pinned local model file could not be read".to_owned())?;
            if read == 0 {
                break;
            }
            total = total.saturating_add(read as u64);
            hasher.update(&buffer[..read]);
        }
        let after = std::fs::symlink_metadata(&path)
            .map_err(|_| "a pinned local model file changed during verification".to_owned())?;
        let digest = format!("{:x}", hasher.finalize());
        if total != *expected_bytes
            || digest != *expected_sha
            || file_identity(relative, &after) != *identity
        {
            return Err("a pinned local model file failed integrity verification".into());
        }
    }
    Ok(identities)
}

fn file_identity(path: &str, metadata: &std::fs::Metadata) -> FileIdentity {
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    #[cfg(unix)]
    let (device, inode) = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let (device, inode) = (0, 0);
    FileIdentity {
        path: path.to_owned(),
        bytes: metadata.len(),
        modified_ns,
        device,
        inode,
    }
}

struct PrivateAudioFile {
    path: PathBuf,
}

impl PrivateAudioFile {
    fn create(models_root: &Path, id: &str, bytes: &[u8]) -> Result<Self, String> {
        let root = models_root.join("audio-tmp");
        let root_metadata =
            std::fs::symlink_metadata(models_root).map_err(|error| error.to_string())?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err("private audio storage is unavailable".into());
        }
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        let audio_metadata = std::fs::symlink_metadata(&root).map_err(|error| error.to_string())?;
        if audio_metadata.file_type().is_symlink() || !audio_metadata.is_dir() {
            return Err("private audio storage is unavailable".into());
        }
        set_private_directory(&root)?;
        let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
        let canonical_models = models_root
            .canonicalize()
            .map_err(|error| error.to_string())?;
        if !canonical_root.starts_with(&canonical_models)
            || canonical_root.parent() != Some(canonical_models.as_path())
        {
            return Err("private audio storage escaped the VoiceFlow model cache".into());
        }
        let path = canonical_root.join(format!("{id}.wav"));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path).map_err(|error| error.to_string())?;
        if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
            let _ = std::fs::remove_file(&path);
            return Err(error.to_string());
        }
        Ok(Self { path })
    }
}

impl Drop for PrivateAudioFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

struct ModelLoadCancellationGuard<'a> {
    runtime: &'a OnDeviceRuntime,
    model_id: String,
    generation: u64,
    previous_status: RuntimeSnapshot,
    child_pid: Option<u32>,
    armed: bool,
}

impl<'a> ModelLoadCancellationGuard<'a> {
    fn new(
        runtime: &'a OnDeviceRuntime,
        model_id: &str,
        generation: u64,
        previous_status: RuntimeSnapshot,
    ) -> Self {
        Self {
            runtime,
            model_id: model_id.to_owned(),
            generation,
            previous_status,
            child_pid: None,
            armed: true,
        }
    }

    fn record_child_pid(&mut self, pid: u32) {
        if pid != 0 {
            self.child_pid = Some(pid);
        }
    }

    fn finish_failure(&mut self, error: &str) {
        self.runtime
            .finish_model_load_failure(&self.model_id, Some(self.generation), error);
        self.armed = false;
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ModelLoadCancellationGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.runtime.cancel_dropped_model_load(
                &self.model_id,
                self.generation,
                self.child_pid,
                &self.previous_status,
            );
        }
    }
}

struct KillOnDrop<'a> {
    pid: u32,
    active_pid: &'a AtomicU32,
    armed: bool,
}

impl<'a> KillOnDrop<'a> {
    fn new(pid: u32, active_pid: &'a AtomicU32) -> Self {
        Self {
            pid,
            active_pid,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for KillOnDrop<'_> {
    fn drop(&mut self) {
        if self.armed
            && self
                .active_pid
                .compare_exchange(self.pid, 0, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            kill_process(self.pid);
        }
    }
}

fn kill_process(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(unix)]
    unsafe {
        let _ = libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
}

fn set_private_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    let _ = path;
    Ok(())
}

fn sanitize_runtime_error(error: &str) -> String {
    let filtered = error
        .chars()
        .filter(|character| !character.is_control())
        .take(400)
        .collect::<String>();
    filtered
        .split("audio-tmp")
        .next()
        .unwrap_or("local runtime failed")
        .to_owned()
}

pub async fn shutdown_for_root(models_root: &Path) {
    let runtime = shared_runtime(models_root);
    runtime.shutdown().await;
    cleanup_private_audio_for_root(models_root);
}

pub fn cleanup_private_audio_for_root(models_root: &Path) {
    cleanup_private_audio_files(&models_root.join("audio-tmp"));
}

fn cleanup_private_audio_files(root: &Path) {
    let Ok(metadata) = std::fs::symlink_metadata(root) else {
        return;
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "wav")
            && std::fs::symlink_metadata(&path)
                .is_ok_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_file())
        {
            let _ = std::fs::remove_file(path);
        }
    }
}
