//! VoiceFlow-owned llama.cpp runtime for pinned text GGUF cleanup models.
//!
//! Each resident model has one managed llama-server child. The child accepts
//! HTTP only over a mode-0700 private AF_UNIX socket; no port is opened, no
//! secret is placed in argv/environment, and `--offline` prevents the helper
//! from resolving remote model sources.

use crate::gguf_cleanup_store::{self, Manager, ResolvedModel};
use futures_util::StreamExt;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

const LLAMA_COMMIT: &str = "cd959bae99ff4bed22bef2604f45241c09bbb8c8";
const LLAMA_RELEASE: &str = "v0.9.0";
const SERVER_RELATIVE_PATH: &str = "llama-runtime/bin/llama-server";
const MAX_REQUEST_BYTES: usize = 2 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_OUTPUT_TOKENS: usize = 2_048;
const CONTEXT_TOKENS: usize = 8_192;
const GPU_LAYER_BUDGET: &str = "99";
const INFERENCE_TIMEOUT: Duration = Duration::from_secs(300);
const MODEL_LOAD_TIMEOUT: Duration = Duration::from_secs(900);
const CHILD_EXIT_TIMEOUT: Duration = Duration::from_secs(12);
const SOCKET_RETRIES: u32 = 32;
const SOCKET_PATH_MAX_BYTES: usize = 103;
static SOCKET_SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
const VERIFICATION_TEST_ROOT_ENV: &str = "VOICEFLOW_LOCAL_CLEANUP_TEST_ROOT";
#[cfg(test)]
const VERIFICATION_TEST_HELPER_ENV: &str = "VOICEFLOW_LLAMA_SERVER";

type RuntimeMap = Mutex<HashMap<(PathBuf, PathBuf), Arc<Runtime>>>;
type ActiveCancellation = Arc<Mutex<Option<(u64, &'static str, CancellationToken)>>>;
static RUNTIMES: OnceLock<RuntimeMap> = OnceLock::new();

#[derive(Debug)]
pub enum RuntimeError {
    UnsupportedPlatform,
    HelperMissing,
    HelperIdentityMismatch,
    ModelUnavailable,
    ModelMismatch,
    CriticalMemoryPressure,
    MemoryUnavailable(crate::model_resources::MemoryUnavailableReason),
    InputTooLarge {
        input_bytes: usize,
        max_bytes: usize,
    },
    Timeout,
    Cancelled,
    RetainedPending,
    SidecarExited,
    InvalidResponse,
    EmptyResponse,
    IncompleteGeneration,
    GenerationFailed,
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::UnsupportedPlatform => {
                "Local GGUF cleanup requires Apple silicon and macOS 13.3 or later."
            }
            Self::HelperMissing => {
                "The VoiceFlow llama.cpp cleanup runtime is not included in this build."
            }
            Self::HelperIdentityMismatch => {
                "The bundled llama.cpp cleanup runtime failed identity verification."
            }
            Self::ModelUnavailable => {
                "The selected local cleanup model is missing or failed verification."
            }
            Self::ModelMismatch => {
                "The selected local cleanup model does not match the verified model."
            }
            Self::CriticalMemoryPressure => {
                "Local cleanup was skipped because system memory pressure is critical."
            }
            Self::MemoryUnavailable(reason) => {
                return write!(
                    f,
                    "Local cleanup is unavailable under the current memory budget ({}).",
                    reason.diagnostic_code()
                );
            }
            Self::InputTooLarge { .. } => {
                "The local cleanup prompt exceeds the runtime request limit."
            }
            Self::Timeout => "Local GGUF cleanup timed out.",
            Self::Cancelled => "Local GGUF cleanup was cancelled.",
            Self::RetainedPending => {
                "The local cleanup process is still stopping; retry after it closes."
            }
            Self::SidecarExited => "The local cleanup process exited unexpectedly.",
            Self::InvalidResponse => "The local cleanup runtime returned an invalid response.",
            Self::EmptyResponse => "The local cleanup runtime returned an empty response.",
            Self::IncompleteGeneration => "Local cleanup stopped before completing its response.",
            Self::GenerationFailed => "Local GGUF cleanup could not complete.",
        })
    }
}

impl std::error::Error for RuntimeError {}

#[derive(Debug, Clone)]
pub struct Completion {
    pub request_id: String,
    pub child_generation: u64,
    pub model_id: String,
    pub text: String,
    pub prompt_tokens: usize,
    pub generation_tokens: usize,
    pub prompt_time_seconds: Option<f64>,
    pub generation_time_seconds: Option<f64>,
    pub runtime_startup_seconds: f64,
    pub model_was_resident: bool,
    pub request_total_seconds: f64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RuntimeStatus {
    pub(crate) model_id: Option<&'static str>,
    pub(crate) generation: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    max_tokens: usize,
    temperature: f32,
    stream: bool,
}

#[derive(Debug, Deserialize)]
struct ModelList {
    data: Vec<ModelInfo>,
}

#[derive(Debug, Deserialize)]
struct ModelInfo {
    id: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<TokenUsage>,
    #[serde(default)]
    timings: Option<ModelTimings>,
}

#[derive(Debug, Deserialize)]
struct ModelTimings {
    prompt_ms: Option<f64>,
    predicted_ms: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenUsage {
    prompt_tokens: Option<usize>,
    completion_tokens: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct BuildIdentity {
    schema_version: u32,
    status: String,
    runtime: String,
    source: BuildSource,
    artifact: BuildArtifact,
    build: BuildConfiguration,
    signing: BuildSigning,
}

#[derive(Debug, Deserialize)]
struct BuildSource {
    release_tag: String,
    commit: String,
}

#[derive(Debug, Deserialize)]
struct BuildArtifact {
    architecture: String,
    minimum_macos: String,
    sha256_signed: String,
    size_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct BuildSigning {
    mode: String,
    hardened_runtime: bool,
}

#[derive(Debug, Deserialize)]
struct BuildConfiguration {
    metal: bool,
    metal_embedded: bool,
    #[serde(rename = "static_native_libraries")]
    static_linking: bool,
    openssl: bool,
    web_ui: bool,
}

struct ServerSession {
    model: ResolvedModel,
    client: Client,
    socket_directory: PathBuf,
    process: ProcessOwner,
    generation: u64,
}

struct ProcessOwner {
    pid: u32,
    server_path: PathBuf,
    cancel: CancellationToken,
    exited: watch::Receiver<bool>,
}

impl ProcessOwner {
    fn has_exited(&self) -> bool {
        *self.exited.borrow()
    }

    async fn wait_for_exit(&mut self, deadline: Duration) -> bool {
        tokio::time::timeout(deadline, async {
            loop {
                if *self.exited.borrow() {
                    return true;
                }
                if self.exited.changed().await.is_err() {
                    return *self.exited.borrow();
                }
            }
        })
        .await
        .unwrap_or(false)
    }
}

impl Drop for ProcessOwner {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Drop for ServerSession {
    fn drop(&mut self) {
        self.process.cancel.cancel();
    }
}

pub struct Runtime {
    models_root: PathBuf,
    manager: Manager,
    helper_path: PathBuf,
    shutting_down: Arc<AtomicBool>,
    generation: AtomicU64,
    session: tokio::sync::Mutex<Option<ServerSession>>,
    operation_gate: Arc<tokio::sync::Mutex<()>>,
    snapshot: Arc<Mutex<RuntimeSnapshot>>,
    active_cancel: ActiveCancellation,
}

#[derive(Default)]
struct RuntimeSnapshot {
    state: &'static str,
    model_id: Option<&'static str>,
    generation: Option<u64>,
}

impl Runtime {
    pub fn new(models_root: PathBuf, manager: Manager, helper_path: PathBuf) -> Self {
        Self {
            models_root,
            manager,
            helper_path,
            shutting_down: Arc::new(AtomicBool::new(false)),
            generation: AtomicU64::new(0),
            session: tokio::sync::Mutex::new(None),
            operation_gate: Arc::new(tokio::sync::Mutex::new(())),
            snapshot: Arc::new(Mutex::new(RuntimeSnapshot {
                state: "not_loaded",
                model_id: None,
                generation: None,
            })),
            active_cancel: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_verification(
        models_root: PathBuf,
        manager: Manager,
        helper_path: PathBuf,
    ) -> Self {
        Self::new(models_root, manager, helper_path)
    }

    fn platform_admitted(&self) -> bool {
        platform_supported()
    }

    pub fn status(&self) -> RuntimeStatus {
        let snapshot = self.snapshot.lock().unwrap_or_else(|p| p.into_inner());
        RuntimeStatus {
            model_id: snapshot.model_id,
            generation: snapshot.generation,
        }
    }

    pub fn helper_path(&self) -> &Path {
        &self.helper_path
    }

    pub fn begin_shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some((_, _, cancellation)) = self
            .active_cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
        {
            cancellation.cancel();
        }
    }

    pub async fn shutdown(&self) -> bool {
        self.begin_shutdown();
        let Ok(_gate) = tokio::time::timeout(CHILD_EXIT_TIMEOUT, self.operation_gate.lock()).await
        else {
            let status = self.status();
            self.set_snapshot("stopping", status.model_id, status.generation);
            return false;
        };
        let Ok(mut session) = tokio::time::timeout(CHILD_EXIT_TIMEOUT, self.session.lock()).await
        else {
            let status = self.status();
            self.set_snapshot("stopping", status.model_id, status.generation);
            return false;
        };
        if let Some(session) = session.as_mut() {
            session.process.cancel.cancel();
            if !session.process.wait_for_exit(CHILD_EXIT_TIMEOUT).await {
                self.set_snapshot(
                    "stopping",
                    Some(session.model.id()),
                    Some(session.generation),
                );
                return false;
            }
        }
        *session = None;
        self.set_snapshot("not_loaded", None, None);
        true
    }

    pub async fn unload(&self, model_id: Option<&str>) -> Result<(), String> {
        if let Some((_, active_model_id, cancellation)) = self
            .active_cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
        {
            if model_id.is_none_or(|id| id == *active_model_id) {
                cancellation.cancel();
            }
        }
        let _gate = tokio::time::timeout(CHILD_EXIT_TIMEOUT, self.operation_gate.lock())
            .await
            .map_err(|_| RuntimeError::RetainedPending.to_string())?;
        let mut session = tokio::time::timeout(CHILD_EXIT_TIMEOUT, self.session.lock())
            .await
            .map_err(|_| RuntimeError::RetainedPending.to_string())?;
        let Some(current) = session.as_mut() else {
            self.set_snapshot("not_loaded", None, None);
            return Ok(());
        };
        if model_id.is_some_and(|id| id != current.model.id()) {
            return Ok(());
        }
        current.process.cancel.cancel();
        if !current.process.wait_for_exit(CHILD_EXIT_TIMEOUT).await {
            self.set_snapshot(
                "stopping",
                Some(current.model.id()),
                Some(current.generation),
            );
            return Err(RuntimeError::RetainedPending.to_string());
        }
        *session = None;
        self.set_snapshot("not_loaded", None, None);
        Ok(())
    }

    pub async fn complete(
        &self,
        model_id: &str,
        messages: Vec<(String, String)>,
    ) -> Result<Completion, RuntimeError> {
        self.ensure_running()?;
        if !self.platform_admitted() {
            return Err(RuntimeError::UnsupportedPlatform);
        }
        if !gguf_cleanup_store::is_supported_model(model_id) {
            return Err(RuntimeError::ModelMismatch);
        }
        let request_id = self.next_request_id();
        let started = Instant::now();
        let request = chat_request(model_id, &messages)?;
        let request_bytes =
            serde_json::to_vec(&request).map_err(|_| RuntimeError::InvalidResponse)?;
        if request_bytes.len() > MAX_REQUEST_BYTES {
            return Err(RuntimeError::InputTooLarge {
                input_bytes: request_bytes.len(),
                max_bytes: MAX_REQUEST_BYTES,
            });
        }
        let _gate = self.operation_gate.lock().await;
        self.ensure_running()?;
        let mut session = self.session.lock().await;
        let had_resident_model = session.as_ref().is_some_and(|current| {
            current.model.id() == model_id
                && !current.process.cancel.is_cancelled()
                && !current.process.has_exited()
        });
        let startup_started = Instant::now();
        self.ensure_session(&mut session, model_id).await?;
        self.ensure_running()?;
        let session = session.as_mut().ok_or(RuntimeError::ModelUnavailable)?;
        let startup_seconds = if had_resident_model {
            0.0
        } else {
            startup_started.elapsed().as_secs_f64()
        };
        self.set_snapshot(
            "cleaning",
            Some(session.model.id()),
            Some(session.generation),
        );
        let mut cancellation_guard = CancelChildOnDrop::new(session.process.cancel.clone());
        let generation_started = Instant::now();
        let url = "http://localhost/v1/chat/completions";
        let response = tokio::time::timeout(
            INFERENCE_TIMEOUT,
            session.client.post(url).json(&request).send(),
        )
        .await
        .map_err(|_| RuntimeError::Timeout)?
        .map_err(|_| RuntimeError::SidecarExited)?;
        let response_status = response.status();
        let body = read_limited_body(response, MAX_RESPONSE_BYTES).await?;
        if !response_status.is_success() {
            return Err(
                if matches!(
                    response_status,
                    StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT
                ) {
                    RuntimeError::Timeout
                } else {
                    RuntimeError::GenerationFailed
                },
            );
        }
        let reply: ChatResponse =
            serde_json::from_slice(&body).map_err(|_| RuntimeError::InvalidResponse)?;
        let choice = reply.choices.first().ok_or(RuntimeError::InvalidResponse)?;
        let text = choice
            .message
            .content
            .as_deref()
            .ok_or(RuntimeError::InvalidResponse)?
            .trim()
            .to_owned();
        if text.is_empty() {
            return Err(RuntimeError::EmptyResponse);
        }
        if matches!(
            choice.finish_reason.as_deref(),
            Some("length" | "max_tokens")
        ) {
            return Err(RuntimeError::IncompleteGeneration);
        }
        if let Some(reason) = choice.finish_reason.as_deref() {
            if !matches!(reason, "stop" | "eos" | "end_turn" | "stop_sequence") {
                return Err(RuntimeError::GenerationFailed);
            }
        }
        let _generation_wall_seconds = generation_started.elapsed().as_secs_f64();
        let usage = reply.usage;
        let timings = reply.timings;
        let prompt_tokens = usage
            .as_ref()
            .and_then(|usage| usage.prompt_tokens)
            .unwrap_or(0);
        let generation_tokens = usage
            .as_ref()
            .and_then(|usage| usage.completion_tokens)
            .unwrap_or(0);
        cancellation_guard.disarm();
        self.set_snapshot("ready", Some(session.model.id()), Some(session.generation));
        Ok(Completion {
            request_id,
            child_generation: session.generation,
            model_id: model_id.to_owned(),
            text,
            prompt_tokens,
            generation_tokens,
            prompt_time_seconds: timings
                .as_ref()
                .and_then(|timings| timings.prompt_ms)
                .map(|milliseconds| milliseconds / 1_000.0),
            generation_time_seconds: timings
                .as_ref()
                .and_then(|timings| timings.predicted_ms)
                .map(|milliseconds| milliseconds / 1_000.0),
            runtime_startup_seconds: startup_seconds,
            model_was_resident: had_resident_model,
            request_total_seconds: started.elapsed().as_secs_f64(),
        })
    }

    fn ensure_running(&self) -> Result<(), RuntimeError> {
        if self.shutting_down.load(Ordering::Acquire) {
            Err(RuntimeError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn next_request_id(&self) -> String {
        let sequence = self.generation.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("vfllama-{}-{sequence}-{nanos}", std::process::id())
    }

    fn set_snapshot(
        &self,
        state: &'static str,
        model_id: Option<&'static str>,
        generation: Option<u64>,
    ) {
        let mut snapshot = self.snapshot.lock().unwrap_or_else(|p| p.into_inner());
        if state == "loading" {
            snapshot.state = state;
            snapshot.model_id = model_id;
            snapshot.generation = generation;
        } else if state == "not_loaded" {
            if generation.is_none() || snapshot.generation == generation {
                snapshot.state = state;
                snapshot.model_id = None;
                snapshot.generation = None;
            }
        } else if snapshot.generation == generation {
            snapshot.state = state;
            snapshot.model_id = model_id;
        }
    }

    async fn ensure_session(
        &self,
        session: &mut Option<ServerSession>,
        model_id: &str,
    ) -> Result<(), RuntimeError> {
        self.ensure_running()?;
        if session.as_ref().is_some_and(|current| {
            current.process.has_exited() || current.process.cancel.is_cancelled()
        }) {
            self.retire_session(session).await?;
        }
        if let Some(current) = session.as_ref() {
            if current.model.id() == model_id {
                verify_child_identity(current.process.pid, &current.process.server_path)?;
                return Ok(());
            }
        }
        if session.is_some() {
            self.retire_session(session).await?;
        }
        let model = self
            .manager
            .resolve_ready(&self.models_root, model_id)
            .map_err(|_| RuntimeError::ModelUnavailable)?;
        let server_path = self.verified_helper_path().await?;
        self.ensure_running()?;
        let model_path = model
            .model_path()
            .canonicalize()
            .map_err(|_| RuntimeError::ModelUnavailable)?;
        let root_reservation = crate::model_resources::RootLoadReservation::admit(
            &self.models_root,
            model.id(),
            model.revision(),
            crate::model_resources::LoadKind::Cleanup,
        )
        .map_err(|reason| {
            if reason == crate::model_resources::MemoryUnavailableReason::CriticalPressure {
                RuntimeError::CriticalMemoryPressure
            } else {
                RuntimeError::MemoryUnavailable(reason)
            }
        })?;
        let root_reservation = Arc::new(Mutex::new(Some(root_reservation)));
        self.ensure_running()?;
        let (socket_directory, socket_path) = private_socket_location()?;
        let generation = self
            .generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let mut child = match Command::new(&server_path)
            .arg("--model")
            .arg(&model_path)
            .arg("--host")
            .arg(&socket_path)
            .arg("--alias")
            .arg(model_id)
            .arg("--no-ui")
            .arg("--offline")
            .arg("--jinja")
            .arg("--reasoning")
            .arg("off")
            .arg("--ctx-size")
            .arg(CONTEXT_TOKENS.to_string())
            .arg("--predict")
            .arg(MAX_OUTPUT_TOKENS.to_string())
            .arg("--gpu-layers")
            .arg(GPU_LAYER_BUDGET)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                let _ = fs::remove_dir_all(&socket_directory);
                return Err(if error.kind() == std::io::ErrorKind::NotFound {
                    RuntimeError::HelperMissing
                } else {
                    RuntimeError::GenerationFailed
                });
            }
        };
        let pid = child.id().ok_or(RuntimeError::HelperIdentityMismatch)?;
        if let Err(error) = verify_child_identity(pid, &server_path) {
            let _ = child.start_kill();
            let _ = child.wait().await;
            let _ = fs::remove_dir_all(&socket_directory);
            return Err(error);
        }
        let Some(registration) =
            crate::model_resources::ChildRegistration::register(pid, generation, &self.models_root)
        else {
            let _ = child.start_kill();
            let _ = child.wait().await;
            let _ = fs::remove_dir_all(&socket_directory);
            return Err(RuntimeError::HelperIdentityMismatch);
        };
        let cancel = CancellationToken::new();
        let supervisor_cancel = cancel.clone();
        let (exit_tx, exit_rx) = watch::channel(false);
        let supervisor_registration = registration.clone();
        let supervisor_context = ChildSupervisorContext {
            model: model.clone(),
            socket_directory: socket_directory.clone(),
            exit_tx,
            snapshot: Arc::clone(&self.snapshot),
            active_cancel: Arc::clone(&self.active_cancel),
            generation,
            root_reservation: Arc::clone(&root_reservation),
        };
        {
            let mut active = self
                .active_cancel
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if self.shutting_down.load(Ordering::Acquire) {
                cancel.cancel();
            } else {
                *active = Some((generation, model.id(), cancel.clone()));
            }
        }
        self.set_snapshot("loading", Some(model.id()), Some(generation));
        tokio::spawn(async move {
            supervise_child(
                &mut child,
                supervisor_cancel,
                supervisor_registration,
                supervisor_context,
            )
            .await;
        });
        let client = match client_for_socket(&socket_path) {
            Ok(client) => client,
            Err(error) => {
                cancel.cancel();
                let mut owner = ProcessOwner {
                    pid,
                    server_path,
                    cancel,
                    exited: exit_rx,
                };
                owner.wait_for_exit(CHILD_EXIT_TIMEOUT).await;
                return Err(error);
            }
        };
        let mut new_session = ServerSession {
            model,
            client,
            socket_directory,
            process: ProcessOwner {
                pid,
                server_path,
                cancel,
                exited: exit_rx,
            },
            generation,
        };
        let ready = wait_until_ready(&mut new_session).await;
        if let Err(error) = ready {
            new_session.process.cancel.cancel();
            if new_session.process.wait_for_exit(CHILD_EXIT_TIMEOUT).await {
                let _ = fs::remove_dir_all(&new_session.socket_directory);
            }
            return Err(error);
        }
        if new_session.process.has_exited() || new_session.process.cancel.is_cancelled() {
            return Err(RuntimeError::SidecarExited);
        }
        settle_root_reservation(&root_reservation);
        let model_id = new_session.model.id();
        *session = Some(new_session);
        self.set_snapshot("ready", Some(model_id), Some(generation));
        Ok(())
    }

    async fn retire_session(
        &self,
        session: &mut Option<ServerSession>,
    ) -> Result<(), RuntimeError> {
        let Some(current) = session.as_mut() else {
            return Ok(());
        };
        current.process.cancel.cancel();
        if !current.process.wait_for_exit(CHILD_EXIT_TIMEOUT).await {
            self.set_snapshot(
                "stopping",
                Some(current.model.id()),
                Some(current.generation),
            );
            return Err(RuntimeError::RetainedPending);
        }
        let model_id = current.model.id();
        let socket_directory = current.socket_directory.clone();
        *session = None;
        let _ = fs::remove_dir_all(socket_directory);
        self.set_snapshot("not_loaded", None, None);
        log::debug!("stopped local cleanup helper for model {model_id}");
        Ok(())
    }

    async fn verified_helper_path(&self) -> Result<PathBuf, RuntimeError> {
        if !self.platform_admitted() {
            return Err(RuntimeError::UnsupportedPlatform);
        }
        verify_helper_bundle(&self.helper_path).await
    }
}

struct CancelChildOnDrop {
    cancel: CancellationToken,
    armed: bool,
}

impl CancelChildOnDrop {
    fn new(cancel: CancellationToken) -> Self {
        Self {
            cancel,
            armed: true,
        }
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CancelChildOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.cancel.cancel();
        }
    }
}

struct ChildSupervisorContext {
    model: ResolvedModel,
    socket_directory: PathBuf,
    exit_tx: watch::Sender<bool>,
    snapshot: Arc<Mutex<RuntimeSnapshot>>,
    active_cancel: ActiveCancellation,
    generation: u64,
    root_reservation: Arc<Mutex<Option<crate::model_resources::RootLoadReservation>>>,
}

async fn supervise_child(
    child: &mut Child,
    cancel: CancellationToken,
    registration: crate::model_resources::ChildRegistration,
    context: ChildSupervisorContext,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                break;
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {}
                    Err(error) => {
                        log::warn!("failed to poll local cleanup helper process: {error}");
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        break;
                    }
                }
            }
        }
    }
    let _ = fs::remove_dir_all(context.socket_directory);
    // This lease lives in the supervisor so the model store cannot remove
    // weights before the child has confirmed exit.
    drop(context.model);
    registration.retire_after_exit();
    context
        .root_reservation
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    {
        let mut active = context
            .active_cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active
            .as_ref()
            .is_some_and(|(active_generation, _, _)| *active_generation == context.generation)
        {
            active.take();
        }
    }
    {
        let mut snapshot = context
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if snapshot.generation == Some(context.generation) {
            snapshot.state = "not_loaded";
            snapshot.model_id = None;
            snapshot.generation = None;
        }
    }
    let _ = context.exit_tx.send(true);
}

fn settle_root_reservation(
    reservation: &Arc<Mutex<Option<crate::model_resources::RootLoadReservation>>>,
) {
    if let Some(reservation) = reservation
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
    {
        reservation.settle();
    }
}

async fn wait_until_ready(session: &mut ServerSession) -> Result<(), RuntimeError> {
    let started = Instant::now();
    loop {
        if session.process.has_exited() {
            return Err(RuntimeError::SidecarExited);
        }
        if session.process.cancel.is_cancelled() {
            return Err(RuntimeError::Cancelled);
        }
        verify_child_identity(session.process.pid, &session.process.server_path)?;
        let health = session.client.get("http://localhost/health").send().await;
        if matches!(health, Ok(ref response) if response.status().is_success()) {
            let models = session
                .client
                .get("http://localhost/v1/models")
                .send()
                .await;
            if let Ok(models) = models {
                if models.status().is_success() {
                    let bytes = read_limited_body(models, 64 * 1024).await?;
                    if let Ok(models) = serde_json::from_slice::<ModelList>(&bytes) {
                        if models
                            .data
                            .iter()
                            .any(|model| model.id == session.model.id())
                        {
                            return Ok(());
                        }
                    }
                }
            }
        }
        if started.elapsed() >= MODEL_LOAD_TIMEOUT {
            return Err(RuntimeError::Timeout);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn read_limited_body(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, RuntimeError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(RuntimeError::InvalidResponse);
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| RuntimeError::SidecarExited)?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(RuntimeError::InvalidResponse);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn chat_request<'a>(
    model: &'a str,
    messages: &'a [(String, String)],
) -> Result<ChatRequest<'a>, RuntimeError> {
    let messages = messages
        .iter()
        .map(|(role, content)| {
            if !matches!(role.as_str(), "system" | "user" | "assistant") {
                return Err(RuntimeError::InvalidResponse);
            }
            Ok(ChatMessage {
                role: role.as_str(),
                content,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ChatRequest {
        model,
        messages,
        max_tokens: MAX_OUTPUT_TOKENS,
        temperature: 0.2,
        stream: false,
    })
}

fn client_for_socket(path: &Path) -> Result<Client, RuntimeError> {
    #[cfg(unix)]
    {
        Client::builder()
            .unix_socket(path.to_path_buf())
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .timeout(INFERENCE_TIMEOUT)
            .build()
            .map_err(|_| RuntimeError::GenerationFailed)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(RuntimeError::UnsupportedPlatform)
    }
}

fn private_socket_location() -> Result<(PathBuf, PathBuf), RuntimeError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let temp_root = Path::new("/tmp");
        let parent = temp_root
            .canonicalize()
            .map_err(|_| RuntimeError::GenerationFailed)?;
        for _ in 0..SOCKET_RETRIES {
            let sequence = SOCKET_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let directory = parent.join(format!("vf-{}-{sequence:x}", std::process::id()));
            let result = fs::DirBuilder::new().mode(0o700).create(&directory);
            if result.is_err() {
                continue;
            }
            let socket = directory.join("s.sock");
            use std::os::unix::ffi::OsStrExt;
            if socket.as_os_str().as_bytes().len() > SOCKET_PATH_MAX_BYTES {
                let _ = fs::remove_dir(&directory);
                return Err(RuntimeError::GenerationFailed);
            }
            return Ok((directory, socket));
        }
        Err(RuntimeError::GenerationFailed)
    }
    #[cfg(not(unix))]
    {
        Err(RuntimeError::UnsupportedPlatform)
    }
}

async fn verify_helper_bundle(path: &Path) -> Result<PathBuf, RuntimeError> {
    let canonical = path
        .canonicalize()
        .map_err(|_| RuntimeError::HelperMissing)?;
    let metadata = fs::symlink_metadata(&canonical).map_err(|_| RuntimeError::HelperMissing)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || !is_executable(&metadata) {
        return Err(RuntimeError::HelperMissing);
    }
    let resource_root = canonical
        .parent()
        .and_then(Path::parent)
        .ok_or(RuntimeError::HelperIdentityMismatch)?;
    let identity_path = resource_root.join("BUILD-IDENTITY.json");
    let identity_metadata =
        fs::symlink_metadata(&identity_path).map_err(|_| RuntimeError::HelperIdentityMismatch)?;
    if identity_metadata.file_type().is_symlink()
        || !identity_metadata.is_file()
        || identity_metadata.len() > 64 * 1024
    {
        return Err(RuntimeError::HelperIdentityMismatch);
    }
    let identity = read_build_identity(&identity_path)?;
    validate_helper_identity(&identity, &canonical, &metadata)?;
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(&canonical)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| RuntimeError::HelperIdentityMismatch)?
    .map_err(|_| RuntimeError::HelperMissing)?;
    let version = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() || !version.contains(&LLAMA_COMMIT[..7]) {
        return Err(RuntimeError::HelperIdentityMismatch);
    }
    Ok(canonical)
}

fn read_build_identity(path: &Path) -> Result<BuildIdentity, RuntimeError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| RuntimeError::HelperIdentityMismatch)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 64 * 1024 {
        return Err(RuntimeError::HelperIdentityMismatch);
    }
    let bytes = fs::read(path).map_err(|_| RuntimeError::HelperIdentityMismatch)?;
    serde_json::from_slice(&bytes).map_err(|_| RuntimeError::HelperIdentityMismatch)
}

fn validate_helper_identity(
    identity: &BuildIdentity,
    helper_path: &Path,
    metadata: &fs::Metadata,
) -> Result<(), RuntimeError> {
    if identity.schema_version != 1
        || identity.status != "ready"
        || identity.runtime != "llama.cpp"
        || identity.source.release_tag != LLAMA_RELEASE
        || identity.source.commit != LLAMA_COMMIT
        || identity.artifact.architecture != "arm64"
        || identity.artifact.minimum_macos != "13.3"
        || !identity.build.metal
        || !identity.build.metal_embedded
        || !identity.build.static_linking
        || identity.build.openssl
        || identity.build.web_ui
        || !matches!(
            identity.signing.mode.as_str(),
            "developer_id" | "signed" | "ad_hoc"
        )
        || (identity.signing.mode == "ad_hoc" && identity.signing.hardened_runtime)
        || (identity.signing.mode != "ad_hoc" && !identity.signing.hardened_runtime)
        || identity.artifact.size_bytes != metadata.len()
        || identity.artifact.sha256_signed.len() != 64
        || !identity
            .artifact
            .sha256_signed
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(RuntimeError::HelperIdentityMismatch);
    }
    let bytes = fs::read(helper_path).map_err(|_| RuntimeError::HelperIdentityMismatch)?;
    let digest = hex::encode(Sha256::digest(&bytes));
    if digest != identity.artifact.sha256_signed {
        return Err(RuntimeError::HelperIdentityMismatch);
    }
    verify_helper_signature(helper_path)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn verify_helper_signature(path: &Path) -> Result<(), RuntimeError> {
    let status = std::process::Command::new("/usr/bin/codesign")
        .arg("--verify")
        .arg("--strict")
        .arg(path)
        .status()
        .map_err(|_| RuntimeError::HelperIdentityMismatch)?;
    status
        .success()
        .then_some(())
        .ok_or(RuntimeError::HelperIdentityMismatch)
}

#[cfg(not(target_os = "macos"))]
fn verify_helper_signature(_path: &Path) -> Result<(), RuntimeError> {
    Err(RuntimeError::UnsupportedPlatform)
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    false
}

fn verify_child_identity(pid: u32, expected: &Path) -> Result<(), RuntimeError> {
    let actual = process_executable_path(pid).ok_or(RuntimeError::HelperIdentityMismatch)?;
    let expected = expected
        .canonicalize()
        .map_err(|_| RuntimeError::HelperIdentityMismatch)?;
    if actual != expected {
        return Err(RuntimeError::HelperIdentityMismatch);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn process_executable_path(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let mut buffer = vec![0u8; 4096];
    let written =
        unsafe { libc::proc_pidpath(pid as i32, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if written <= 0 || written as usize >= buffer.len() {
        return None;
    }
    buffer.truncate(written as usize);
    PathBuf::from(std::ffi::OsString::from_vec(buffer))
        .canonicalize()
        .ok()
}

#[cfg(target_os = "linux")]
fn process_executable_path(pid: u32) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/exe"))
        .ok()?
        .canonicalize()
        .ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_executable_path(_pid: u32) -> Option<PathBuf> {
    None
}

pub fn resolve_server_executable(resource_dir: &Path) -> PathBuf {
    let bundled = resource_dir.join(SERVER_RELATIVE_PATH);
    if bundled.is_file() {
        return bundled;
    }
    #[cfg(debug_assertions)]
    {
        let staged = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/llama-runtime/stage")
            .join(SERVER_RELATIVE_PATH);
        if staged.is_file() {
            return staged;
        }
    }
    bundled
}

pub fn compatibility_error(helper_path: &Path) -> Option<&'static str> {
    if !platform_supported() {
        return Some("unsupported_platform");
    }
    if helper_bundle_status(helper_path).is_err() {
        return Some("runtime_missing");
    }
    None
}

fn helper_bundle_status(helper_path: &Path) -> Result<(), RuntimeError> {
    let canonical = helper_path
        .canonicalize()
        .map_err(|_| RuntimeError::HelperMissing)?;
    let metadata = fs::symlink_metadata(&canonical).map_err(|_| RuntimeError::HelperMissing)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || !is_executable(&metadata) {
        return Err(RuntimeError::HelperMissing);
    }
    let resource_root = canonical
        .parent()
        .and_then(Path::parent)
        .ok_or(RuntimeError::HelperIdentityMismatch)?
        .to_path_buf();
    let identity_path = resource_root.join("BUILD-IDENTITY.json");
    let identity = read_build_identity(&identity_path)?;
    validate_helper_identity(&identity, &canonical, &metadata)
}

pub fn shared_runtime(models_root: &Path, manager: Manager, helper_path: &Path) -> Arc<Runtime> {
    let runtimes = RUNTIMES.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (models_root.to_path_buf(), helper_path.to_path_buf());
    runtimes
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry(key.clone())
        .or_insert_with(|| Arc::new(Runtime::new(key.0.clone(), manager, key.1.clone())))
        .clone()
}

fn platform_supported() -> bool {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        macos_13_3_or_later()
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        false
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn macos_13_3_or_later() -> bool {
    let mut length = 0usize;
    let name = b"kern.osrelease\0";
    let first = unsafe {
        libc::sysctlbyname(
            name.as_ptr().cast(),
            std::ptr::null_mut(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if first != 0 || length == 0 || length > 128 {
        return false;
    }
    let mut bytes = vec![0u8; length];
    let second = unsafe {
        libc::sysctlbyname(
            name.as_ptr().cast(),
            bytes.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if second != 0 {
        return false;
    }
    let Some(release) = std::ffi::CStr::from_bytes_until_nul(&bytes)
        .ok()
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let mut parts = release.split('.');
    let Some(darwin_major) = parts.next().and_then(|part| part.parse::<u32>().ok()) else {
        return false;
    };
    let darwin_minor = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or(0);
    darwin_major > 22 || (darwin_major == 22 && darwin_minor >= 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StrictOfflineRestore(bool);

    impl Drop for StrictOfflineRestore {
        fn drop(&mut self) {
            crate::network_policy::set_strict_offline(self.0);
        }
    }

    #[test]
    fn chat_request_keeps_only_text_roles_and_uses_bounded_output() {
        let messages = vec![
            ("system".to_owned(), "system text".to_owned()),
            ("user".to_owned(), "input text".to_owned()),
        ];
        let request = chat_request(gguf_cleanup_store::QWEN_35_2B_ID, &messages).unwrap();
        assert_eq!(request.model, gguf_cleanup_store::QWEN_35_2B_ID);
        assert_eq!(request.max_tokens, MAX_OUTPUT_TOKENS);
        assert!(!request.stream);
        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.messages[0].role, "system");
    }

    #[test]
    fn request_rejects_roles_outside_the_chat_contract() {
        let messages = vec![("tool".to_owned(), "untrusted tool result".to_owned())];
        assert!(matches!(
            chat_request(gguf_cleanup_store::QWEN_35_2B_ID, &messages),
            Err(RuntimeError::InvalidResponse)
        ));
    }

    #[test]
    fn socket_path_fits_darwin_sockaddr_and_is_private() {
        let (directory, socket) = private_socket_location().unwrap();
        use std::os::unix::ffi::OsStrExt;
        assert!(socket.as_os_str().as_bytes().len() <= SOCKET_PATH_MAX_BYTES);
        let metadata = fs::metadata(&directory).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn staged_helper_resolver_uses_frozen_relative_layout() {
        let resource_dir = PathBuf::from("/tmp/voiceflow-no-resource");
        let expected = resource_dir.join(SERVER_RELATIVE_PATH);
        let resolved = resolve_server_executable(&resource_dir);
        #[cfg(debug_assertions)]
        assert!(resolved == expected || resolved.ends_with(SERVER_RELATIVE_PATH));
        #[cfg(not(debug_assertions))]
        assert_eq!(resolved, expected);
    }

    #[test]
    fn packaged_identity_fixture_uses_signed_artifact_fields() {
        let fixture = serde_json::json!({
            "schema_version": 1,
            "status": "ready",
            "runtime": "llama.cpp",
            "source": {
                "repository": "https://github.com/ggml-org/llama.cpp",
                "release_tag": "v0.9.0",
                "annotated_tag_object_sha1": "c4261a74a59c5a719e8f73038e28633b08f2bea4",
                "commit": LLAMA_COMMIT,
                "license": "MIT"
            },
            "artifact": {
                "path": "bin/llama-server",
                "architecture": "arm64",
                "minimum_macos": "13.3",
                "size_bytes_pre_signing": 1,
                "sha256_pre_signing": "0".repeat(64),
                "size_bytes": 2,
                "sha256_signed": "1".repeat(64)
            },
            "build": {
                "configuration": "Release",
                "cmake_options": [],
                "metal": true,
                "metal_embedded": true,
                "static_native_libraries": true,
                "openssl": false,
                "web_ui": false
            },
            "signing": {
                "mode": "ad_hoc",
                "hardened_runtime": false,
                "authority": null,
                "team_identifier": "not set"
            },
            "toolchain": {},
            "version_output": "version: 0.9.0-dev (build 1, commit cd959ba)"
        });
        let identity: BuildIdentity = serde_json::from_value(fixture).unwrap();
        assert_eq!(identity.source.commit, LLAMA_COMMIT);
        assert_eq!(identity.artifact.sha256_signed, "1".repeat(64));
        assert_eq!(identity.artifact.size_bytes, 2);
        assert_eq!(identity.signing.mode, "ad_hoc");
        assert!(!identity.signing.hardened_runtime);
    }

    #[tokio::test]
    #[ignore = "requires the pinned local GGUF files and packaged llama-server"]
    async fn verify_pinned_models_offline() {
        let verification_root = std::env::var_os(VERIFICATION_TEST_ROOT_ENV)
            .map(PathBuf::from)
            .expect("set VOICEFLOW_LOCAL_CLEANUP_TEST_ROOT to an isolated temporary root");
        let helper_path = std::env::var_os(VERIFICATION_TEST_HELPER_ENV)
            .map(PathBuf::from)
            .expect("set VOICEFLOW_LLAMA_SERVER to the packaged llama-server");
        assert!(verification_root.is_absolute());
        assert!(verification_root.starts_with(std::env::temp_dir()));
        assert!(helper_path.is_absolute());
        let models_root = verification_root.join("models");
        let manager = Manager::default();
        let model_ids = std::env::var("VOICEFLOW_LOCAL_CLEANUP_MODEL_IDS").unwrap_or_else(|_| {
            gguf_cleanup_store::MODELS
                .iter()
                .map(|manifest| manifest.id)
                .collect::<Vec<_>>()
                .join(",")
        });
        let model_ids = model_ids.split(',').map(str::trim).collect::<Vec<_>>();
        assert!(!model_ids.is_empty());
        for id in &model_ids {
            let manifest = gguf_cleanup_store::model(id).expect("supported model id");
            let status = manager.status(&models_root, id, true, None);
            assert!(status.verified, "{id} is not installed and verified");
            let resolved = manager
                .resolve_ready(&models_root, id)
                .expect("production model resolver accepts verified model");
            assert_eq!(resolved.id(), manifest.id);
            assert_eq!(resolved.revision(), manifest.revision);
            drop(resolved);
        }

        let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
        let _restore_strict_offline =
            StrictOfflineRestore(crate::network_policy::is_strict_offline());
        crate::network_policy::set_strict_offline(true);
        assert!(crate::network_policy::is_strict_offline());

        let llama_runtime =
            Runtime::new_for_verification(models_root.clone(), manager, helper_path);
        let mlx_runtime =
            crate::mlx_cleanup_runtime::Runtime::new_for_demand_test(models_root.clone());
        let input = "Mara will send 17 notes tomorrow, but not today.";
        let intent = crate::llm::CleanupIntent::implicit(input);
        let verification = async {
            for id in model_ids {
                let (reply, _) =
                    crate::llm::cleanup_with_provider_and_local_runtimes_and_intent_and_source(
                        crate::providers::EngineProvider::OnDeviceLlamaCpp,
                        &mlx_runtime,
                        Some(&llama_runtime),
                        "",
                        id,
                        input,
                        "",
                        None,
                        None,
                        None,
                        Some(&intent),
                        None,
                        crate::llm::CleanupEffort::Light,
                        None,
                        None,
                        None,
                        None,
                    )
                    .await
                    .map_err(|error| format!("production cleanup failed for {id}: {error}"))?;
                let accepted = reply.accepted_text.to_lowercase();
                if reply.accepted_text.trim().is_empty()
                    || reply.native_request_id.is_none()
                    || !accepted.contains("mara")
                    || !accepted.contains("17")
                    || !accepted.contains("tomorrow")
                    || !accepted.contains("today")
                    || !crate::protected_span::preserves(
                        input,
                        &reply.accepted_text,
                        Some(crate::llm::CleanupOperation::Cleanup),
                    )
                {
                    return Err(format!(
                        "production cleanup did not preserve the synthetic fact fixture for {id}"
                    ));
                }
                llama_runtime
                    .unload(Some(id))
                    .await
                    .map_err(|error| format!("failed to unload {id}: {error}"))?;
                println!(
                    "verified_local_cleanup_model={id} accepted_utf8_bytes={}",
                    reply.accepted_text.len()
                );
            }
            Ok::<(), String>(())
        }
        .await;
        let shutdown_succeeded = llama_runtime.shutdown().await;
        assert!(shutdown_succeeded, "runtime did not shut down cleanly");
        verification.unwrap_or_else(|error| panic!("{error}"));
    }
}
