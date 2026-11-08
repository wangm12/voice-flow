//! VoiceFlow-owned Qwen MLX text cleanup process. It shares the packaged
//! executable with ASR, but has a separate child and resident model slot.

use crate::mlx_diagnostics::{self, EventName, Severity, Target};
use crate::modelstore::{self, Manager, ResolvedCleanupModel};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

const PROTOCOL_VERSION: u32 = 1;
const MODEL_ID: &str = modelstore::MODEL_ID;
const MODEL_REVISION: &str = modelstore::REVISION;
const MAX_REQUEST_LINE_BYTES: usize = 1_000_000;
const MAX_RESPONSE_LINE_BYTES: usize = 64 * 1024;
pub(crate) const MAX_OUTPUT_TOKENS: usize = 2_048;
const INFERENCE_TIMEOUT: Duration = Duration::from_secs(300);
const MODEL_LOAD_TIMEOUT: Duration = Duration::from_secs(900);
const OPERATION_TIMEOUT: Duration = Duration::from_secs(1_260);
const CHILD_EXIT_TIMEOUT: Duration = Duration::from_secs(12);
#[cfg(test)]
type TestChildExitObserver = (u64, watch::Receiver<bool>);
#[cfg(test)]
type TestChildExitStatus = (u64, bool, Option<i32>);

const PHASE_NOT_LOADED: u8 = 0;
const PHASE_LOADING: u8 = 1;
const PHASE_READY: u8 = 2;
const PHASE_CLEANING: u8 = 3;
const PHASE_STOPPING: u8 = 4;
const PHASE_PREWARMING: u8 = 5;

fn error_after_child_retirement(
    primary_error: RuntimeError,
    retirement_error: RuntimeError,
) -> RuntimeError {
    if matches!(
        &primary_error,
        RuntimeError::CriticalMemoryPressure | RuntimeError::MemoryUnavailable(_)
    ) {
        log::warn!("memory admission failure remains the cleanup cause while child retirement is pending: {retirement_error}");
        primary_error
    } else {
        retirement_error
    }
}

static CLEANUP_OPERATION_GATE: OnceLock<Arc<tokio::sync::Mutex<()>>> = OnceLock::new();

/// Serialize requests to this cleanup child, including unload and model loading.
fn cleanup_operation_gate() -> &'static Arc<tokio::sync::Mutex<()>> {
    CLEANUP_OPERATION_GATE.get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
}

fn startup_warmup_pressure_error(
    pressure: crate::model_resources::HostMemoryPressure,
) -> Option<RuntimeError> {
    match pressure {
        crate::model_resources::HostMemoryPressure::Normal => None,
        crate::model_resources::HostMemoryPressure::Critical => {
            Some(RuntimeError::CriticalMemoryPressure)
        }
        crate::model_resources::HostMemoryPressure::Warning => {
            Some(RuntimeError::MemoryUnavailable(
                crate::model_resources::MemoryUnavailableReason::PressureNotNormal,
            ))
        }
        crate::model_resources::HostMemoryPressure::Unknown => {
            Some(RuntimeError::MemoryUnavailable(
                crate::model_resources::MemoryUnavailableReason::UnknownPressure,
            ))
        }
    }
}

#[derive(Debug)]
pub enum RuntimeError {
    UnsupportedPlatform,
    SidecarMissing,
    ModelUnavailable,
    ModelMismatch,
    ModelLoadFailed,
    CriticalMemoryPressure,
    MemoryUnavailable(crate::model_resources::MemoryUnavailableReason),
    ModelNotLoaded,
    InputTooLarge {
        prompt_tokens: usize,
        requested_output_tokens: usize,
        requested_total_tokens: Option<usize>,
        max_total_tokens: usize,
    },
    Timeout,
    Cancelled,
    StartupWarmupYielded {
        child_generation: u64,
    },
    RetainedPending,
    InvalidResponse,
    IncompleteGeneration,
    GenerationFailed,
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Self::MemoryUnavailable(reason) = self {
            return write!(
                f,
                "Local model loading is unavailable under the current memory budget ({}).",
                reason.diagnostic_code()
            );
        }
        f.write_str(match self {
            Self::UnsupportedPlatform => {
                "Local MLX cleanup requires Apple silicon and macOS 14 or later."
            }
            Self::SidecarMissing => {
                "The VoiceFlow MLX cleanup runtime is not included in this build."
            }
            Self::ModelUnavailable => "The verified local cleanup model is unavailable.",
            Self::ModelMismatch => {
                "The selected model does not match the pinned local MLX cleanup model."
            }
            Self::ModelLoadFailed => "The local cleanup model could not be loaded.",
            Self::CriticalMemoryPressure => {
                "Local cleanup was skipped because system memory pressure is critical."
            }
            Self::MemoryUnavailable(_) => unreachable!("typed memory cause was formatted above"),
            Self::ModelNotLoaded => "The local cleanup model is not loaded.",
            Self::InputTooLarge { .. } => {
                "The local cleanup prompt and requested output exceed the token budget."
            }
            Self::Timeout => "Local MLX cleanup timed out.",
            Self::Cancelled => "Local MLX cleanup was cancelled.",
            Self::StartupWarmupYielded { .. } => {
                "Startup cleanup warmup yielded to a foreground request."
            }
            Self::RetainedPending => {
                "The local cleanup process is still stopping; retry after it closes."
            }
            Self::InvalidResponse => "The local cleanup runtime returned an invalid response.",
            Self::IncompleteGeneration => "Local MLX cleanup stopped before a complete response.",
            Self::GenerationFailed => "Local MLX cleanup could not complete.",
        })
    }
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub request_id: String,
    pub child_generation: u64,
    pub text: String,
    pub prompt_tokens: usize,
    pub generation_tokens: usize,
    pub prompt_time_seconds: f64,
    pub generation_time_seconds: f64,
    /// Time to create the long-lived cleanup child and complete its handshake.
    pub runtime_startup_seconds: f64,
    /// One-time verification before loading this model into the native runtime.
    pub model_inventory_validation_seconds: Option<f64>,
    /// One-time MLX container load exchange; absent on warm resident requests.
    pub model_load_seconds: Option<f64>,
    pub model_was_resident: bool,
    /// Time spent waiting behind another cleanup request in the cleanup-only gate.
    pub queue_wait_seconds: f64,
    /// Same-process-monotonic sidecar measurements; none are subtracted from Rust Instants.
    pub sidecar_prepare_seconds: Option<f64>,
    pub sidecar_first_visible_chunk_seconds: Option<f64>,
    pub sidecar_generation_drain_seconds: Option<f64>,
    pub sidecar_prompt_time_scope: Option<String>,
    pub prompt_cache_status: Option<String>,
    pub prompt_cache_reason: Option<String>,
    pub cached_prefix_tokens: usize,
    pub computed_suffix_tokens: usize,
    pub prompt_cache_snapshot_bytes: u64,
    pub prompt_cache_lookup_seconds: Option<f64>,
    pub prompt_cache_build_seconds: Option<f64>,
    pub prompt_cache_copy_seconds: Option<f64>,
    pub prompt_cache_restore_seconds: Option<f64>,
    /// Bounded sidecar resource snapshots; emitted only for requests over six seconds.
    pub cleanup_runtime_diagnostics: Option<crate::mlx_diagnostics::CleanupRuntimeDiagnostics>,
    /// Includes cleanup queue wait, session setup, model verification/load, and generation.
    pub request_total_seconds: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct WarmupCompletion {
    pub(crate) request_id: String,
    pub(crate) child_generation: u64,
    pub(crate) variant: &'static str,
    pub(crate) queue_wait_seconds: f64,
    pub(crate) runtime_startup_seconds: f64,
    pub(crate) model_inventory_validation_seconds: Option<f64>,
    pub(crate) model_load_seconds: Option<f64>,
    pub(crate) model_was_resident: bool,
    pub(crate) prompt_tokens: usize,
    pub(crate) generation_tokens: Option<usize>,
    pub(crate) completion: Option<String>,
    pub(crate) info_observed: Option<bool>,
    pub(crate) tool_call_observed: Option<bool>,
    pub(crate) completion_text_seen: Option<bool>,
    pub(crate) prompt_evaluation_observed: Option<bool>,
    pub(crate) observed_prompt_tokens: Option<usize>,
    pub(crate) stream_drained: Option<bool>,
    pub(crate) evaluation_synchronized: Option<bool>,
    pub(crate) evaluation_seconds: f64,
    pub(crate) prepare_seconds: Option<f64>,
    pub(crate) prompt_time_seconds: Option<f64>,
    pub(crate) generation_time_seconds: Option<f64>,
    pub(crate) prompt_cache_status: String,
    pub(crate) prompt_cache_reason: Option<String>,
    pub(crate) cached_prefix_tokens: usize,
    pub(crate) computed_suffix_tokens: usize,
    pub(crate) prompt_cache_snapshot_bytes: u64,
    pub(crate) prompt_cache_lookup_seconds: Option<f64>,
    pub(crate) prompt_cache_build_seconds: Option<f64>,
    pub(crate) prompt_cache_copy_seconds: Option<f64>,
    pub(crate) prompt_cache_restore_seconds: Option<f64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuntimeOperation {
    Cleanup,
    StartupWarmup,
}

enum OperationReply {
    Cleanup(Completion),
    StartupWarmup(WarmupCompletion),
}

#[derive(Debug, Clone, Copy)]
struct ModelLoadTiming {
    inventory_validation_seconds: f64,
    model_load_seconds: f64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RuntimeStatus {
    pub(crate) state: &'static str,
    pub(crate) model_loaded: bool,
}

#[derive(Debug, Clone, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Clone, Serialize)]
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
    messages: Option<Vec<ChatMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stable_prefix_user_bytes: Option<usize>,
}

impl Request {
    fn cancel(request_id: String, session_id: Option<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            session_id,
            model_version: Some(MODEL_REVISION.to_owned()),
            op: "cancel".to_owned(),
            model_id: Some(MODEL_ID.to_owned()),
            model_directory: None,
            messages: None,
            max_output_tokens: None,
            stable_prefix_user_bytes: None,
        }
    }

    fn cleanup(
        request_id: String,
        session_id: Option<String>,
        messages: Vec<(String, String)>,
        stable_prefix_user_bytes: Option<usize>,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            session_id,
            model_version: Some(MODEL_REVISION.to_owned()),
            op: "cleanup".to_owned(),
            model_id: Some(MODEL_ID.to_owned()),
            model_directory: None,
            messages: Some(
                messages
                    .into_iter()
                    .map(|(role, content)| ChatMessage { role, content })
                    .collect(),
            ),
            max_output_tokens: Some(MAX_OUTPUT_TOKENS),
            stable_prefix_user_bytes,
        }
    }

    fn startup_warmup(
        request_id: String,
        session_id: Option<String>,
        messages: Vec<(String, String)>,
        stable_prefix_user_bytes: Option<usize>,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            session_id,
            model_version: Some(MODEL_REVISION.to_owned()),
            op: "warmup".to_owned(),
            model_id: Some(MODEL_ID.to_owned()),
            model_directory: None,
            messages: Some(
                messages
                    .into_iter()
                    .map(|(role, content)| ChatMessage { role, content })
                    .collect(),
            ),
            // The cache identity and admission use the production output cap.
            // The sidecar separately limits the internal evaluation to 1 token.
            max_output_tokens: Some(MAX_OUTPUT_TOKENS),
            stable_prefix_user_bytes,
        }
    }
}

#[cfg(test)]
pub(crate) fn export_cleanup_request_pair(
    messages: Vec<(String, String)>,
    stable_prefix_user_bytes: Option<usize>,
) -> Result<(serde_json::Value, serde_json::Value), String> {
    let serialize = |boundary| {
        let request = Request::cleanup(
            "r18d-private-prompt-export".to_owned(),
            Some("r18d-private-prompt-export".to_owned()),
            messages.clone(),
            boundary,
        );
        serde_json::to_value(request).map_err(|_| "cleanup request serialization failed".to_owned())
    };
    Ok((serialize(None)?, serialize(stable_prefix_user_bytes)?))
}

#[cfg(test)]
pub(crate) fn export_startup_warmup_request_pair(
    messages: Vec<(String, String)>,
    stable_prefix_user_bytes: Option<usize>,
) -> Result<(serde_json::Value, serde_json::Value), String> {
    let serialize = |boundary| {
        let request = Request::startup_warmup(
            "r19-startup-warmup-private-export".to_owned(),
            Some("r19-startup-warmup-private-export".to_owned()),
            messages.clone(),
            boundary,
        );
        serde_json::to_value(request)
            .map_err(|_| "startup warmup request serialization failed".to_owned())
    };
    Ok((serialize(None)?, serialize(stable_prefix_user_bytes)?))
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
    loaded_model_id: Option<String>,
    #[serde(default)]
    loaded_model_version: Option<String>,
    #[serde(default)]
    cleanup_result: Option<CleanupResult>,
    #[serde(default, deserialize_with = "deserialize_cleanup_diagnostics")]
    cleanup_diagnostics: Option<crate::mlx_diagnostics::CleanupRuntimeDiagnostics>,
    #[serde(default)]
    warmup_result: Option<SidecarWarmupResult>,
    #[serde(default)]
    error: Option<SidecarFailure>,
}

fn deserialize_cleanup_diagnostics<'de, D>(
    deserializer: D,
) -> Result<Option<crate::mlx_diagnostics::CleanupRuntimeDiagnostics>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| serde_json::from_value(value).ok()))
}

#[derive(Debug, Clone, Deserialize)]
struct CleanupResult {
    text: String,
    prompt_tokens: usize,
    generation_tokens: usize,
    prompt_time_seconds: f64,
    generation_time_seconds: f64,
    #[serde(default)]
    prepare_time_seconds: Option<f64>,
    #[serde(default)]
    first_visible_chunk_seconds: Option<f64>,
    #[serde(default)]
    generation_drain_seconds: Option<f64>,
    #[serde(default)]
    prompt_cache_status: Option<String>,
    #[serde(default)]
    prompt_cache_reason: Option<String>,
    #[serde(default)]
    cached_prefix_tokens: Option<usize>,
    #[serde(default)]
    computed_suffix_tokens: Option<usize>,
    #[serde(default)]
    prompt_cache_snapshot_bytes: Option<u64>,
    #[serde(default)]
    prompt_cache_lookup_seconds: Option<f64>,
    #[serde(default)]
    prompt_cache_build_seconds: Option<f64>,
    #[serde(default)]
    prompt_cache_copy_seconds: Option<f64>,
    #[serde(default)]
    prompt_cache_restore_seconds: Option<f64>,
    #[serde(default)]
    prompt_time_scope: Option<String>,
    completion: String,
}

#[derive(Debug, Clone, Deserialize)]
struct SidecarWarmupResult {
    status: String,
    prompt_tokens: usize,
    #[serde(default)]
    generation_tokens: Option<usize>,
    #[serde(default)]
    completion: Option<String>,
    #[serde(default)]
    info_observed: Option<bool>,
    #[serde(default)]
    tool_call_observed: Option<bool>,
    #[serde(default)]
    completion_text_seen: Option<bool>,
    #[serde(default)]
    prompt_evaluation_observed: Option<bool>,
    #[serde(default)]
    observed_prompt_tokens: Option<usize>,
    #[serde(default)]
    stream_drained: Option<bool>,
    #[serde(default)]
    evaluation_synchronized: Option<bool>,
    #[serde(default)]
    prepare_time_seconds: Option<f64>,
    evaluation_time_seconds: f64,
    #[serde(default)]
    prompt_time_seconds: Option<f64>,
    #[serde(default)]
    generation_time_seconds: Option<f64>,
    #[serde(default)]
    prompt_cache_status: Option<String>,
    #[serde(default)]
    prompt_cache_reason: Option<String>,
    #[serde(default)]
    cached_prefix_tokens: Option<usize>,
    #[serde(default)]
    computed_suffix_tokens: Option<usize>,
    #[serde(default)]
    prompt_cache_snapshot_bytes: Option<u64>,
    #[serde(default)]
    prompt_cache_lookup_seconds: Option<f64>,
    #[serde(default)]
    prompt_cache_build_seconds: Option<f64>,
    #[serde(default)]
    prompt_cache_copy_seconds: Option<f64>,
    #[serde(default)]
    prompt_cache_restore_seconds: Option<f64>,
}

#[derive(Debug, Deserialize, Clone)]
struct SidecarFailure {
    code: String,
    #[serde(default)]
    prompt_tokens: Option<usize>,
    #[serde(default)]
    requested_output_tokens: Option<usize>,
    #[serde(default)]
    requested_total_tokens: Option<usize>,
    #[serde(default)]
    max_total_tokens: Option<usize>,
    #[serde(default)]
    prompt_cache_status: Option<String>,
    #[serde(default)]
    prompt_cache_reason: Option<String>,
    #[serde(default)]
    cached_prefix_tokens: Option<usize>,
    #[serde(default)]
    computed_suffix_tokens: Option<usize>,
    #[serde(default)]
    prompt_cache_snapshot_bytes: Option<u64>,
    #[serde(default)]
    prompt_cache_lookup_seconds: Option<f64>,
    #[serde(default)]
    prompt_cache_build_seconds: Option<f64>,
    #[serde(default)]
    prompt_cache_copy_seconds: Option<f64>,
    #[serde(default)]
    prompt_cache_restore_seconds: Option<f64>,
}

struct ChildSession {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    process: ProcessOwner,
    model: Option<ResolvedCleanupModel>,
}

type OwnedCleanupSession = tokio::sync::OwnedMutexGuard<Option<ChildSession>>;

/// The supervisor owns `Child` until `wait()` confirms exit. The model lease
/// lives in the supervisor too, so dropping a request/session cannot make a
/// still-running process look closed to the model-store deletion owner.
struct ProcessOwner {
    id: u64,
    cancel: CancellationToken,
    exited: watch::Receiver<bool>,
    lease_slot: Arc<Mutex<Option<ResolvedCleanupModel>>>,
    cleanup_gate_slot: Arc<Mutex<Option<tokio::sync::OwnedMutexGuard<()>>>>,
    registration: crate::model_resources::ChildRegistration,
}

impl ProcessOwner {
    fn retain_cleanup_gate(&self, gate: tokio::sync::OwnedMutexGuard<()>) {
        *self
            .cleanup_gate_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(gate);
    }

    fn release_cleanup_gate(&self) {
        self.cleanup_gate_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }
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

pub struct Runtime {
    models_root: PathBuf,
    manager: Manager,
    sequence: AtomicU64,
    child_sequence: AtomicU64,
    shutting_down: Arc<AtomicBool>,
    phase: Arc<AtomicU8>,
    model_resident: Arc<AtomicBool>,
    startup_warmup_generation: AtomicU64,
    startup_warmup_control_sequence: AtomicU64,
    startup_warmup_demands: AtomicUsize,
    startup_warmup_control: Mutex<Option<(u64, CancellationToken)>>,
    startup_warmup_in_flight_owners: Arc<AtomicUsize>,
    active_process: Arc<Mutex<Option<(u64, CancellationToken)>>>,
    generation_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    #[cfg(test)]
    child_exit_observers: Arc<Mutex<Vec<TestChildExitObserver>>>,
    #[cfg(test)]
    child_exit_statuses: Arc<Mutex<Vec<TestChildExitStatus>>>,
    #[cfg(test)]
    test_emit_capture: Option<crate::mlx_diagnostics::EmitRecordCaptureForTest>,
    #[cfg(test)]
    test_response_handoff_pause: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    #[cfg(test)]
    test_settled_owner_pause: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    #[cfg(test)]
    test_generation_join_reached: Option<Arc<tokio::sync::Notify>>,
    #[cfg(test)]
    test_fixture_fault: modelstore::TinyFixtureFault,
    session: Arc<tokio::sync::Mutex<Option<ChildSession>>>,
    #[cfg(test)]
    test_worker: Option<(PathBuf, Vec<String>)>,
    #[cfg(test)]
    test_tiny_model: bool,
    #[cfg(test)]
    test_platform_supported: Option<bool>,
    #[cfg(test)]
    test_command_resolution_error: bool,
}

struct StartupWarmupYieldGuard<'a> {
    runtime: &'a Runtime,
    id: u64,
}

impl Drop for StartupWarmupYieldGuard<'_> {
    fn drop(&mut self) {
        let mut control = self
            .runtime
            .startup_warmup_control
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if control.as_ref().is_some_and(|(id, _)| *id == self.id) {
            *control = None;
        }
    }
}

impl Runtime {
    #[cfg(test)]
    pub(crate) fn new_for_demand_test(models_root: PathBuf) -> Self {
        Self::new(models_root, Manager::new())
    }

    fn new(models_root: PathBuf, manager: Manager) -> Self {
        Self {
            models_root,
            manager,
            sequence: AtomicU64::new(0),
            child_sequence: AtomicU64::new(0),
            shutting_down: Arc::new(AtomicBool::new(false)),
            phase: Arc::new(AtomicU8::new(PHASE_NOT_LOADED)),
            model_resident: Arc::new(AtomicBool::new(false)),
            startup_warmup_generation: AtomicU64::new(0),
            startup_warmup_control_sequence: AtomicU64::new(0),
            startup_warmup_demands: AtomicUsize::new(0),
            startup_warmup_control: Mutex::new(None),
            startup_warmup_in_flight_owners: Arc::new(AtomicUsize::new(0)),
            active_process: Arc::new(Mutex::new(None)),
            generation_task: Mutex::new(None),
            #[cfg(test)]
            child_exit_observers: Arc::new(Mutex::new(Vec::new())),
            #[cfg(test)]
            child_exit_statuses: Arc::new(Mutex::new(Vec::new())),
            #[cfg(test)]
            test_emit_capture: None,
            #[cfg(test)]
            test_response_handoff_pause: None,
            #[cfg(test)]
            test_settled_owner_pause: None,
            #[cfg(test)]
            test_generation_join_reached: None,
            #[cfg(test)]
            test_fixture_fault: modelstore::TinyFixtureFault::None,
            session: Arc::new(tokio::sync::Mutex::new(None)),
            #[cfg(test)]
            test_worker: None,
            #[cfg(test)]
            test_tiny_model: false,
            #[cfg(test)]
            test_platform_supported: None,
            #[cfg(test)]
            test_command_resolution_error: false,
        }
    }

    #[cfg(test)]
    fn with_test_worker(
        models_root: PathBuf,
        manager: Manager,
        program: PathBuf,
        args: Vec<String>,
    ) -> Self {
        let mut runtime = Self::new(models_root, manager);
        runtime.test_worker = Some((program, args));
        runtime.test_tiny_model = true;
        runtime.test_platform_supported = Some(true);
        runtime
    }

    fn platform_admitted(&self) -> bool {
        #[cfg(test)]
        if let Some(supported) = self.test_platform_supported {
            return supported;
        }
        platform_supported()
    }

    #[cfg(test)]
    async fn wait_for_all_test_children(&self) -> Option<Vec<TestChildExitStatus>> {
        let observers = self
            .child_exit_observers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        for (_, mut receiver) in observers {
            let exited = tokio::time::timeout(CHILD_EXIT_TIMEOUT, async move {
                loop {
                    if *receiver.borrow() {
                        return true;
                    }
                    if receiver.changed().await.is_err() {
                        return *receiver.borrow();
                    }
                }
            })
            .await
            .unwrap_or(false);
            if !exited {
                return None;
            }
        }
        Some(
            self.child_exit_statuses
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone(),
        )
    }

    fn child_command(&self) -> Result<Command, RuntimeError> {
        #[cfg(test)]
        if self.test_command_resolution_error {
            return Err(RuntimeError::SidecarMissing);
        }
        #[cfg(test)]
        if let Some((program, args)) = &self.test_worker {
            let mut command = Command::new(program);
            command.args(args).env("PYTHONDONTWRITEBYTECODE", "1");
            return Ok(command);
        }
        let executable =
            crate::ondevice_runtime::sidecar_executable().ok_or(RuntimeError::SidecarMissing)?;
        let mut command = Command::new(executable);
        command
            .arg("--mode")
            .arg("cleanup")
            .arg("--models-root")
            .arg(&self.models_root);
        Ok(command)
    }

    pub(crate) fn status(&self) -> RuntimeStatus {
        let phase = self.phase.load(Ordering::Acquire);
        match phase {
            PHASE_LOADING => RuntimeStatus {
                state: "loading",
                model_loaded: false,
            },
            PHASE_READY => RuntimeStatus {
                state: "ready",
                model_loaded: true,
            },
            PHASE_CLEANING => RuntimeStatus {
                state: "cleaning",
                model_loaded: true,
            },
            PHASE_PREWARMING => RuntimeStatus {
                state: "warming",
                model_loaded: true,
            },
            PHASE_STOPPING => RuntimeStatus {
                state: "stopping",
                model_loaded: false,
            },
            _ => RuntimeStatus {
                state: "not_loaded",
                model_loaded: false,
            },
        }
    }

    pub async fn complete(
        &self,
        messages: Vec<(String, String)>,
    ) -> Result<Completion, RuntimeError> {
        self.complete_with_source_history(messages, None).await
    }

    pub(crate) async fn complete_with_source_history(
        &self,
        messages: Vec<(String, String)>,
        source_history_id: Option<i64>,
    ) -> Result<Completion, RuntimeError> {
        self.complete_with_source_history_and_prefix(messages, source_history_id, None)
            .await
    }

    pub(crate) async fn complete_with_source_history_and_prefix(
        &self,
        messages: Vec<(String, String)>,
        source_history_id: Option<i64>,
        stable_prefix_user_bytes: Option<usize>,
    ) -> Result<Completion, RuntimeError> {
        self.complete_with_source_history_and_prefix_traced(
            messages,
            source_history_id,
            stable_prefix_user_bytes,
            None,
        )
        .await
    }

    pub(crate) async fn complete_with_source_history_and_prefix_traced(
        &self,
        messages: Vec<(String, String)>,
        source_history_id: Option<i64>,
        stable_prefix_user_bytes: Option<usize>,
        trace: Option<crate::metrics::TraceHandle>,
    ) -> Result<Completion, RuntimeError> {
        let request_id = self.next_id();
        let session_generation = trace
            .as_ref()
            .map(crate::metrics::TraceHandle::session_generation);
        let started = Instant::now();
        let message_count = messages.len();
        let input_utf8_bytes = messages.iter().fold(0usize, |total, (_, content)| {
            total.saturating_add(content.len())
        });
        let source_history_id = source_history_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "none".to_owned());
        mlx_diagnostics::event(EventName::CleanupRequest, Severity::Info, Target::Runtime)
            .request_id(&request_id)
            .session_generation(session_generation)
            .source_history_id(source_history_id.parse().ok())
            .phase("begin")
            .message_facts(message_count, input_utf8_bytes)
            .emit();
        let mut terminal = RequestTerminalLog::new(
            request_id.clone(),
            source_history_id.clone(),
            started,
            session_generation,
        );
        let result = match tokio::time::timeout(
            OPERATION_TIMEOUT,
            self.complete_bounded(
                messages,
                stable_prefix_user_bytes,
                started,
                request_id,
                source_history_id,
                RuntimeOperation::Cleanup,
                None,
                None,
                trace.clone(),
                None,
            ),
        )
        .await
        {
            Ok(Ok(OperationReply::Cleanup(completion))) => Ok(completion),
            Ok(Ok(OperationReply::StartupWarmup(_))) => Err(RuntimeError::InvalidResponse),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(RuntimeError::Timeout),
        };
        match &result {
            Ok(completion) => {
                let runtime_diagnostics = completion.cleanup_runtime_diagnostics.as_ref();
                let slow_runtime_diagnostics = mlx_diagnostics::slow_cleanup_runtime_diagnostics(
                    completion.request_total_seconds,
                    runtime_diagnostics,
                );
                let generation_event = mlx_diagnostics::event(
                    EventName::CleanupGeneration,
                    Severity::Info,
                    Target::Runtime,
                )
                .request_id(&completion.request_id)
                .session_generation(session_generation)
                .child_generation(completion.child_generation)
                .prompt_tokens(completion.prompt_tokens)
                .generation_tokens(completion.generation_tokens)
                .wait_ms(completion.queue_wait_seconds * 1_000.0)
                .sidecar_generation_metrics(
                    completion.prompt_time_seconds * 1_000.0,
                    completion.generation_time_seconds * 1_000.0,
                    completion
                        .sidecar_prepare_seconds
                        .map(|seconds| seconds * 1_000.0),
                    completion
                        .sidecar_first_visible_chunk_seconds
                        .map(|seconds| seconds * 1_000.0),
                    completion
                        .sidecar_generation_drain_seconds
                        .map(|seconds| seconds * 1_000.0),
                    completion.sidecar_prompt_time_scope.as_deref(),
                )
                .prompt_cache_metrics(
                    completion
                        .prompt_cache_status
                        .as_deref()
                        .unwrap_or("bypassed"),
                    completion.prompt_cache_reason.as_deref(),
                    completion.prompt_cache_snapshot_bytes,
                    completion.cached_prefix_tokens,
                    completion.computed_suffix_tokens,
                    completion
                        .prompt_cache_lookup_seconds
                        .map(|seconds| seconds * 1_000.0),
                    completion
                        .prompt_cache_build_seconds
                        .map(|seconds| seconds * 1_000.0),
                    completion
                        .prompt_cache_copy_seconds
                        .map(|seconds| seconds * 1_000.0),
                    completion
                        .prompt_cache_restore_seconds
                        .map(|seconds| seconds * 1_000.0),
                );
                let generation_event = generation_event
                    .cleanup_phase_timings(runtime_diagnostics)
                    .cleanup_runtime_diagnostics(slow_runtime_diagnostics);
                generation_event.emit();
                terminal.finish("completed", "generation", completion.request_total_seconds);
            }
            Err(error) => {
                if let RuntimeError::InputTooLarge {
                    prompt_tokens,
                    requested_output_tokens,
                    requested_total_tokens,
                    max_total_tokens,
                } = error
                {
                    mlx_diagnostics::event(
                        EventName::CleanupAdmission,
                        Severity::Warn,
                        Target::Runtime,
                    )
                    .request_id(&terminal.request_id)
                    .session_generation(terminal.session_generation)
                    .source_history_id(terminal.source_history_id.parse().ok())
                    .outcome("rejected")
                    .reason("input_too_large")
                    .token_budget(
                        *prompt_tokens,
                        *requested_output_tokens,
                        *requested_total_tokens,
                        *max_total_tokens,
                    )
                    .emit();
                }
                terminal.finish(
                    runtime_error_outcome(error),
                    runtime_error_stage(error),
                    started.elapsed().as_secs_f64(),
                );
            }
        }
        result
    }

    pub(crate) async fn prewarm_startup(
        &self,
        messages: Vec<(String, String)>,
        stable_prefix_user_bytes: Option<usize>,
        variant: &'static str,
        still_current: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<WarmupCompletion, RuntimeError> {
        let request_id = self.next_id();
        let started = Instant::now();
        mlx_diagnostics::event(EventName::StartupWarmup, Severity::Info, Target::Runtime)
            .request_id(&request_id)
            .startup_warmup_variant(variant)
            .phase("begin")
            .outcome("accepted")
            .emit();
        if !still_current() {
            self.log_startup_warmup_skipped(
                &request_id,
                started,
                "configuration_changed",
                Some(variant),
            );
            return Err(RuntimeError::Cancelled);
        }
        let warmup_id = self
            .startup_warmup_control_sequence
            .fetch_add(1, Ordering::AcqRel)
            + 1;
        let yield_control = self.install_startup_warmup_control(warmup_id);
        let _yield_guard = StartupWarmupYieldGuard {
            runtime: self,
            id: warmup_id,
        };
        let _in_flight =
            StartupWarmupInFlightGuard::new(Arc::clone(&self.startup_warmup_in_flight_owners));
        let result = match tokio::time::timeout(
            OPERATION_TIMEOUT,
            self.complete_bounded(
                messages,
                stable_prefix_user_bytes,
                started,
                request_id.clone(),
                "none".to_owned(),
                RuntimeOperation::StartupWarmup,
                Some(variant),
                Some(Arc::clone(&still_current)),
                None,
                Some(yield_control),
            ),
        )
        .await
        {
            Ok(Ok(OperationReply::StartupWarmup(completion))) => Ok(completion),
            Ok(Ok(OperationReply::Cleanup(_))) => Err(RuntimeError::InvalidResponse),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(RuntimeError::Timeout),
        };
        match &result {
            Ok(completion) => {
                let current = still_current();
                let mut event = mlx_diagnostics::event(
                    EventName::StartupWarmup,
                    Severity::Info,
                    Target::Runtime,
                )
                .request_id(&completion.request_id)
                .startup_warmup_variant(completion.variant)
                .child_generation(completion.child_generation)
                .phase("terminal")
                .outcome(if current { "ready" } else { "skipped" })
                .stage(if current {
                    "generation"
                } else {
                    "model_readiness"
                });
                if !current {
                    event = event.reason("configuration_changed");
                }
                event = event
                    .startup_ms(completion.runtime_startup_seconds * 1_000.0)
                    .wait_ms(completion.queue_wait_seconds * 1_000.0)
                    .reused_model(
                        completion.model_was_resident,
                        false,
                        !completion.model_was_resident,
                    );
                if let Some(validation_ms) = completion
                    .model_inventory_validation_seconds
                    .map(|seconds| seconds * 1_000.0)
                {
                    event = event.validation_ms(validation_ms);
                }
                if let Some(load_ms) = completion
                    .model_load_seconds
                    .map(|seconds| seconds * 1_000.0)
                {
                    event = event.load_ms(load_ms);
                }
                event = event
                    .prompt_tokens(completion.prompt_tokens)
                    .warmup_terminal_facts(
                        completion.info_observed,
                        completion.tool_call_observed,
                        completion.completion_text_seen,
                        completion.prompt_evaluation_observed,
                        completion.observed_prompt_tokens,
                        completion.generation_tokens,
                        completion.completion.as_deref(),
                        completion.stream_drained,
                        completion.evaluation_synchronized,
                    );
                if let (Some(prompt_seconds), Some(generation_seconds)) = (
                    valid_sidecar_seconds(completion.prompt_time_seconds),
                    valid_sidecar_seconds(completion.generation_time_seconds),
                ) {
                    event = event.sidecar_generation_metrics(
                        prompt_seconds * 1_000.0,
                        generation_seconds * 1_000.0,
                        completion.prepare_seconds.map(|s| s * 1_000.0),
                        None,
                        Some(completion.evaluation_seconds * 1_000.0),
                        Some("internal_warmup"),
                    );
                }
                event
                    .prompt_cache_metrics(
                        &completion.prompt_cache_status,
                        completion.prompt_cache_reason.as_deref(),
                        completion.prompt_cache_snapshot_bytes,
                        completion.cached_prefix_tokens,
                        completion.computed_suffix_tokens,
                        completion.prompt_cache_lookup_seconds.map(|s| s * 1_000.0),
                        completion.prompt_cache_build_seconds.map(|s| s * 1_000.0),
                        completion.prompt_cache_copy_seconds.map(|s| s * 1_000.0),
                        completion.prompt_cache_restore_seconds.map(|s| s * 1_000.0),
                    )
                    .elapsed_ms(started.elapsed().as_secs_f64() * 1_000.0)
                    .emit();
                if !current {
                    self.startup_warmup_generation
                        .compare_exchange(
                            completion.child_generation,
                            0,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .ok();
                }
            }
            Err(error) => {
                let mut event = mlx_diagnostics::event(
                    EventName::StartupWarmup,
                    Severity::Warn,
                    Target::Runtime,
                )
                .request_id(&request_id)
                .startup_warmup_variant(variant)
                .phase("terminal")
                .outcome(runtime_error_outcome(error))
                .stage(runtime_error_stage(error))
                .error_code(runtime_error_code(error));
                if let RuntimeError::InputTooLarge {
                    prompt_tokens,
                    requested_output_tokens,
                    requested_total_tokens,
                    max_total_tokens,
                } = error
                {
                    event = event.token_budget(
                        *prompt_tokens,
                        *requested_output_tokens,
                        *requested_total_tokens,
                        *max_total_tokens,
                    );
                }
                event
                    .elapsed_ms(started.elapsed().as_secs_f64() * 1_000.0)
                    .emit();
            }
        }
        result
    }

    pub(crate) fn begin_foreground_model_demand(&self) {
        let control = self
            .startup_warmup_control
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        self.startup_warmup_demands.fetch_add(1, Ordering::AcqRel);
        if let Some((_, control)) = control.as_ref() {
            control.cancel();
        }
    }

    fn install_startup_warmup_control(&self, id: u64) -> CancellationToken {
        let token = CancellationToken::new();
        let mut control = self
            .startup_warmup_control
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if self.startup_warmup_demands.load(Ordering::Acquire) != 0 {
            token.cancel();
        }
        *control = Some((id, token.clone()));
        token
    }

    pub(crate) fn end_foreground_model_demand(&self) {
        let _control = self
            .startup_warmup_control
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        self.startup_warmup_demands.fetch_sub(1, Ordering::AcqRel);
    }

    #[cfg(test)]
    pub(crate) fn foreground_model_demand_count(&self) -> usize {
        self.startup_warmup_demands.load(Ordering::Acquire)
    }

    fn log_startup_warmup_skipped(
        &self,
        request_id: &str,
        started: Instant,
        reason: &str,
        variant: Option<&str>,
    ) {
        mlx_diagnostics::event(EventName::StartupWarmup, Severity::Info, Target::Runtime)
            .request_id(request_id)
            .startup_warmup_variant(variant.unwrap_or_default())
            .phase("terminal")
            .outcome("skipped")
            .stage("model_admission")
            .reason(reason)
            .elapsed_ms(started.elapsed().as_secs_f64() * 1_000.0)
            .emit();
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "these are distinct bounded IPC request, operation, and ownership facts"
    )]
    async fn complete_bounded(
        &self,
        messages: Vec<(String, String)>,
        stable_prefix_user_bytes: Option<usize>,
        request_started: Instant,
        request_id: String,
        source_history_id: String,
        operation: RuntimeOperation,
        warmup_variant: Option<&'static str>,
        still_current: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
        trace: Option<crate::metrics::TraceHandle>,
        warmup_yield: Option<CancellationToken>,
    ) -> Result<OperationReply, RuntimeError> {
        let _phase_guard = RestorePhaseOnDrop {
            phase: Arc::clone(&self.phase),
            model_resident: Arc::clone(&self.model_resident),
            shutting_down: &self.shutting_down,
        };
        self.ensure_running()?;
        if !self.platform_admitted() {
            return Err(RuntimeError::UnsupportedPlatform);
        }
        if messages.len() != 2
            || messages[0].0 != "system"
            || messages[1].0 != "user"
            || messages.iter().any(|(_, content)| content.is_empty())
        {
            return Err(RuntimeError::InvalidResponse);
        }
        if stable_prefix_user_bytes.is_some_and(|bytes| messages[1].1.get(..bytes).is_none()) {
            return Err(RuntimeError::InvalidResponse);
        }

        let queue_started = Instant::now();
        let session_generation = trace
            .as_ref()
            .map(crate::metrics::TraceHandle::session_generation);
        let mut gate_span = trace
            .as_ref()
            .map(|trace| trace.begin(crate::metrics::Stage::MlxGateWait));
        let startup_warmup_active_when_queued =
            self.startup_warmup_in_flight_owners.load(Ordering::Acquire) > 0;
        let cleanup_gate = Arc::clone(cleanup_operation_gate());
        let cleanup_guard = tokio::time::timeout(OPERATION_TIMEOUT, cleanup_gate.lock_owned())
            .await
            .map_err(|_| RuntimeError::Timeout)?;
        let queue_wait_seconds = queue_started.elapsed().as_secs_f64();
        if let Some(span) = &mut gate_span {
            span.finish();
        }
        mlx_diagnostics::event(EventName::CleanupQueue, Severity::Info, Target::Runtime)
            .request_id(&request_id)
            .session_generation(session_generation)
            .source_history_id(source_history_id.parse().ok())
            .startup_warmup_variant(warmup_variant.unwrap_or_default())
            .outcome("acquired")
            .wait_ms(queue_wait_seconds * 1_000.0)
            .startup_warmup_active_when_queued(startup_warmup_active_when_queued)
            .emit();
        self.ensure_running()?;
        if still_current.as_ref().is_some_and(|check| !check()) {
            return Err(RuntimeError::Cancelled);
        }
        let mut session =
            tokio::time::timeout(OPERATION_TIMEOUT, Arc::clone(&self.session).lock_owned())
                .await
                .map_err(|_| RuntimeError::Timeout)?;

        let prior_child_generation = session.as_ref().map(|child| child.process.id);
        let session_started = Instant::now();
        self.ensure_session(&mut session, &request_id, session_generation, trace.clone())
            .await?;
        if still_current.as_ref().is_some_and(|check| !check()) {
            return Err(RuntimeError::Cancelled);
        }
        let runtime_startup_seconds = session_started.elapsed().as_secs_f64();
        let child_generation = session
            .as_ref()
            .ok_or(RuntimeError::SidecarMissing)?
            .process
            .id;
        mlx_diagnostics::event(EventName::CleanupChild, Severity::Info, Target::Runtime)
            .request_id(&request_id)
            .session_generation(session_generation)
            .child_generation(child_generation)
            .outcome("ready")
            .reused_model(
                prior_child_generation == Some(child_generation),
                prior_child_generation.is_none(),
                false,
            )
            .startup_ms(runtime_startup_seconds * 1_000.0)
            .emit();
        let process_cancel = session
            .as_ref()
            .ok_or(RuntimeError::SidecarMissing)?
            .process
            .cancel
            .clone();
        session
            .as_ref()
            .ok_or(RuntimeError::SidecarMissing)?
            .process
            .retain_cleanup_gate(cleanup_guard);
        let mut operation_guard = CancelChildOnDrop::new(process_cancel, Arc::clone(&self.phase));

        let model_timing = match self
            .ensure_model_loaded(
                &mut session,
                &request_id,
                child_generation,
                session_generation,
                trace.clone(),
            )
            .await
        {
            Ok(timing) => timing,
            Err(error) => {
                let startup_warmup_pressure_yield = operation == RuntimeOperation::StartupWarmup
                    && matches!(
                        &error,
                        RuntimeError::MemoryUnavailable(
                            crate::model_resources::MemoryUnavailableReason::PressureNotNormal
                                | crate::model_resources::MemoryUnavailableReason::UnknownPressure
                        )
                    );
                if startup_warmup_pressure_yield {
                    if let Some(child) = session.as_ref() {
                        child.process.release_cleanup_gate();
                    }
                    operation_guard.disarm();
                    let model_is_resident = session
                        .as_ref()
                        .and_then(|child| child.model.as_ref())
                        .is_some_and(|model| {
                            model.id() == MODEL_ID
                                && model.revision() == MODEL_REVISION
                                && model.manifest_sha256() == modelstore::MANIFEST_SHA256
                        });
                    self.phase.store(
                        if model_is_resident {
                            PHASE_READY
                        } else {
                            PHASE_NOT_LOADED
                        },
                        Ordering::Release,
                    );
                    return Err(error);
                }
                // A failed load or model admission can leave work in the child.
                // Retire it under the supervisor before another cleanup
                // request can enter this process.
                let close = self.retire_session(&mut session, CHILD_EXIT_TIMEOUT).await;
                operation_guard.disarm();
                if let Err(close_error) = close {
                    self.phase.store(PHASE_STOPPING, Ordering::Release);
                    return Err(error_after_child_retirement(error, close_error));
                }
                self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
                return Err(error);
            }
        };
        if operation == RuntimeOperation::StartupWarmup {
            let pressure = session
                .as_ref()
                .ok_or(RuntimeError::SidecarMissing)?
                .process
                .registration
                .pressure();
            if let Some(error) = startup_warmup_pressure_error(pressure) {
                session
                    .as_ref()
                    .ok_or(RuntimeError::SidecarMissing)?
                    .process
                    .release_cleanup_gate();
                operation_guard.disarm();
                self.phase.store(PHASE_READY, Ordering::Release);
                return Err(error);
            }
        }
        if still_current.as_ref().is_some_and(|check| !check()) {
            session
                .as_ref()
                .ok_or(RuntimeError::SidecarMissing)?
                .process
                .release_cleanup_gate();
            operation_guard.disarm();
            self.phase.store(PHASE_READY, Ordering::Release);
            return Err(RuntimeError::Cancelled);
        }
        if warmup_yield
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
            || (operation == RuntimeOperation::StartupWarmup
                && self.startup_warmup_demands.load(Ordering::Acquire) != 0)
        {
            if let Some(yield_control) = warmup_yield.as_ref() {
                yield_control.cancel();
            }
            session
                .as_ref()
                .ok_or(RuntimeError::SidecarMissing)?
                .process
                .release_cleanup_gate();
            operation_guard.disarm();
            self.phase.store(PHASE_READY, Ordering::Release);
            return Err(RuntimeError::StartupWarmupYielded { child_generation });
        }
        self.phase.store(
            if operation == RuntimeOperation::StartupWarmup {
                PHASE_PREWARMING
            } else {
                PHASE_CLEANING
            },
            Ordering::Release,
        );
        let session_id = Some(self.next_id());
        let request = match operation {
            RuntimeOperation::Cleanup => Request::cleanup(
                request_id.clone(),
                session_id,
                messages,
                stable_prefix_user_bytes,
            ),
            RuntimeOperation::StartupWarmup => Request::startup_warmup(
                request_id.clone(),
                session_id,
                messages,
                stable_prefix_user_bytes,
            ),
        };
        let process_cancel = session
            .as_ref()
            .ok_or(RuntimeError::SidecarMissing)?
            .process
            .cancel
            .clone();
        let cancel_request = Request::cancel(self.next_id(), request.session_id.clone());
        let caller_cancel = CancellationToken::new();
        let mut request_guard =
            CancelGenerationOnDrop::new(caller_cancel.clone(), Arc::clone(&self.phase));
        let generation_exchange_started = Instant::now();
        let phase = Arc::clone(&self.phase);
        let owner_phase = Arc::clone(&phase);
        let model_resident = Arc::clone(&self.model_resident);
        let owner_cancel = caller_cancel.clone();
        let owner_warmup_yield = warmup_yield.clone();
        let owner_process_cancel = process_cancel.clone();
        let owner_cancel_armed = request_guard.armed.clone();
        let shutting_down = Arc::clone(&self.shutting_down);
        let caller_request = request.clone();
        let owner_warmup_guard = (operation == RuntimeOperation::StartupWarmup).then(|| {
            StartupWarmupInFlightGuard::retain(Arc::clone(&self.startup_warmup_in_flight_owners))
        });
        let session_handoff = Arc::new(tokio::sync::Mutex::new(None::<OwnedCleanupSession>));
        let owner_session_handoff = Arc::clone(&session_handoff);
        #[cfg(test)]
        let response_handoff_pause = self.test_response_handoff_pause.clone();
        #[cfg(test)]
        let settled_owner_pause = self.test_settled_owner_pause.clone();
        let (owner_sender, owner_receiver) = tokio::sync::oneshot::channel();
        let (handoff_ack_sender, handoff_ack_receiver) = tokio::sync::oneshot::channel();
        let previous_owner = {
            self.generation_task
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
        };
        if let Some(previous_owner) = previous_owner {
            let mut previous_owner =
                GenerationTaskHandoff::new(&self.generation_task, previous_owner);
            #[cfg(test)]
            if let Some(reached) = &self.test_generation_join_reached {
                reached.notify_one();
            }
            let joined = tokio::select! {
                biased;
                result = previous_owner.join() => { let _ = result; true }
                _ = caller_cancel.cancelled() => false,
            };
            if !joined {
                return Err(RuntimeError::Cancelled);
            }
        }
        let owner = tokio::spawn(async move {
            let _owner_warmup_guard = owner_warmup_guard;
            let Some(child) = session.as_mut() else {
                let _ = owner_sender.send(GenerationOwnerOutcome::Retired(
                    RuntimeError::SidecarMissing,
                ));
                return;
            };
            let mut owner_retirement_guard =
                CancelChildOnDrop::new(owner_process_cancel.clone(), Arc::clone(&owner_phase));
            let outcome = exchange_generation_with_cancel(
                child,
                &request,
                &cancel_request,
                &owner_cancel,
                owner_warmup_yield.as_ref(),
            )
            .await;
            match outcome {
                GenerationExchangeOutcome::Response(response) => {
                    let fallback_response = (*response).clone();
                    *owner_session_handoff.lock().await = Some(session);
                    match owner_sender.send(GenerationOwnerOutcome::Response(response)) {
                        Ok(()) => {
                            #[cfg(test)]
                            if let Some((reached, release)) = response_handoff_pause {
                                reached.notify_one();
                                release.notified().await;
                            }
                            tokio::select! {
                                biased;
                                acknowledgement = handoff_ack_receiver => {
                                    if acknowledgement.is_ok() {
                                        owner_cancel_armed.store(false, Ordering::Release);
                                        owner_retirement_guard.disarm();
                                    } else {
                                        owner_cancel_armed.store(false, Ordering::Release);
                                        if let Some(session) = owner_session_handoff.lock().await.take() {
                                            settle_unclaimed_response(
                                                session,
                                                &request,
                                                &fallback_response,
                                                owner_process_cancel.is_cancelled(),
                                                Arc::clone(&owner_phase),
                                                Arc::clone(&model_resident),
                                                Arc::clone(&shutting_down),
                                            ).await;
                                        }
                                        owner_retirement_guard.disarm();
                                        #[cfg(test)]
                                        if let Some((reached, release)) = settled_owner_pause {
                                            reached.notify_one();
                                            release.notified().await;
                                        }
                                    }
                                }
                                _ = owner_cancel.cancelled() => {
                                    owner_cancel_armed.store(false, Ordering::Release);
                                    if let Some(session) = owner_session_handoff.lock().await.take() {
                                        settle_unclaimed_response(
                                            session,
                                            &request,
                                            &fallback_response,
                                            owner_process_cancel.is_cancelled(),
                                            Arc::clone(&owner_phase),
                                            Arc::clone(&model_resident),
                                            Arc::clone(&shutting_down),
                                        ).await;
                                    }
                                    owner_retirement_guard.disarm();
                                }
                                _ = owner_process_cancel.cancelled() => {
                                    owner_cancel_armed.store(false, Ordering::Release);
                                    if let Some(session) = owner_session_handoff.lock().await.take() {
                                        settle_unclaimed_response(
                                            session,
                                            &request,
                                            &fallback_response,
                                            true,
                                            Arc::clone(&owner_phase),
                                            Arc::clone(&model_resident),
                                            Arc::clone(&shutting_down),
                                        ).await;
                                    }
                                    owner_retirement_guard.disarm();
                                }
                            }
                        }
                        Err(GenerationOwnerOutcome::Response(response)) => {
                            owner_cancel_armed.store(false, Ordering::Release);
                            if let Some(session) = owner_session_handoff.lock().await.take() {
                                settle_unclaimed_response(
                                    session,
                                    &request,
                                    &response,
                                    owner_process_cancel.is_cancelled(),
                                    Arc::clone(&owner_phase),
                                    Arc::clone(&model_resident),
                                    Arc::clone(&shutting_down),
                                )
                                .await;
                            }
                            owner_retirement_guard.disarm();
                        }
                        Err(_) => unreachable!("only the response outcome is sent from this arm"),
                    }
                }
                GenerationExchangeOutcome::SettledCancellation(primary_error) => {
                    owner_cancel_armed.store(false, Ordering::Release);
                    if let Some(child) = session.as_ref() {
                        child.process.release_cleanup_gate();
                    }
                    if !shutting_down.load(Ordering::Acquire) {
                        owner_phase.store(PHASE_READY, Ordering::Release);
                    } else {
                        owner_phase.store(PHASE_STOPPING, Ordering::Release);
                    }
                    model_resident.store(true, Ordering::Release);
                    owner_retirement_guard.disarm();
                    let _ = owner_sender
                        .send(GenerationOwnerOutcome::SettledCancellation(primary_error));
                }
                GenerationExchangeOutcome::Retire(error) => {
                    owner_cancel_armed.store(false, Ordering::Release);
                    if let Some(child) = session.as_mut() {
                        child.process.cancel.cancel();
                        if child.process.wait_for_exit(CHILD_EXIT_TIMEOUT).await {
                            *session = None;
                            owner_phase.store(PHASE_NOT_LOADED, Ordering::Release);
                        } else {
                            owner_phase.store(PHASE_STOPPING, Ordering::Release);
                        }
                    }
                    owner_retirement_guard.disarm();
                    let _ = owner_sender.send(GenerationOwnerOutcome::Retired(error));
                }
            }
        });
        *self
            .generation_task
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(owner);
        operation_guard.disarm();
        let owner_outcome = match owner_receiver.await {
            Ok(outcome) => outcome,
            Err(_) => {
                request_guard.disarm();
                self.phase.store(PHASE_STOPPING, Ordering::Release);
                return Err(RuntimeError::RetainedPending);
            }
        };
        let response = match owner_outcome {
            GenerationOwnerOutcome::Response(response) => {
                let Some(owned_session) = session_handoff.lock().await.take() else {
                    request_guard.disarm();
                    self.phase.store(PHASE_STOPPING, Ordering::Release);
                    return Err(RuntimeError::RetainedPending);
                };
                session = owned_session;
                let _ = handoff_ack_sender.send(());
                request_guard.disarm();
                operation_guard = CancelChildOnDrop::new(process_cancel, Arc::clone(&phase));
                *response
            }
            GenerationOwnerOutcome::SettledCancellation(Some(error)) => {
                request_guard.disarm();
                mlx_diagnostics::event(
                    EventName::CleanupSidecarExchange,
                    Severity::Warn,
                    Target::Runtime,
                )
                .request_id(&request_id)
                .session_generation(session_generation)
                .child_generation(child_generation)
                .outcome("cancelled_after_timeout")
                .stage("exchange")
                .elapsed_ms(generation_exchange_started.elapsed().as_secs_f64() * 1_000.0)
                .error_code(runtime_error_code(&error))
                .emit();
                return Err(error);
            }
            GenerationOwnerOutcome::SettledCancellation(None) => {
                request_guard.disarm();
                if warmup_yield
                    .as_ref()
                    .is_some_and(CancellationToken::is_cancelled)
                    && still_current.as_ref().is_some_and(|check| check())
                {
                    return Err(RuntimeError::StartupWarmupYielded { child_generation });
                }
                return Err(RuntimeError::Cancelled);
            }
            GenerationOwnerOutcome::Retired(error) => {
                request_guard.disarm();
                mlx_diagnostics::event(
                    EventName::CleanupSidecarExchange,
                    Severity::Warn,
                    Target::Runtime,
                )
                .request_id(&request_id)
                .session_generation(session_generation)
                .child_generation(child_generation)
                .outcome("failed")
                .stage("exchange")
                .elapsed_ms(generation_exchange_started.elapsed().as_secs_f64() * 1_000.0)
                .error_code(runtime_error_code(&error))
                .emit();
                return Err(error);
            }
        };
        mlx_diagnostics::event(
            EventName::CleanupSidecarExchange,
            Severity::Info,
            Target::Runtime,
        )
        .request_id(&request_id)
        .session_generation(session_generation)
        .child_generation(child_generation)
        .outcome("response_received")
        .elapsed_ms(generation_exchange_started.elapsed().as_secs_f64() * 1_000.0)
        .response_status(&response.status)
        .emit();
        let cleanup_model_identity_mismatch = operation == RuntimeOperation::Cleanup
            && response.status == "completed"
            && (response.loaded_model_id.as_deref() != Some(MODEL_ID)
                || response.loaded_model_version.as_deref() != Some(MODEL_REVISION));
        if !response_matches(&caller_request, &response)
            || response.platform_supported != Some(true)
            || cleanup_model_identity_mismatch
        {
            let close = self.retire_session(&mut session, CHILD_EXIT_TIMEOUT).await;
            operation_guard.disarm();
            close?;
            self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
            return Err(RuntimeError::InvalidResponse);
        }
        if operation == RuntimeOperation::Cleanup {
            self.log_startup_warmup_first_request(
                &request_id,
                child_generation,
                session_generation,
                &response,
            );
        }
        if response.status != "completed" {
            let error = map_sidecar_error(response.error.as_ref());
            let mut terminal_event = mlx_diagnostics::event(
                EventName::CleanupSidecarTerminal,
                Severity::Warn,
                Target::Runtime,
            )
            .request_id(&request_id)
            .session_generation(session_generation)
            .child_generation(child_generation)
            .outcome("error")
            .stage(runtime_error_stage(&error))
            .error_code(runtime_error_code(&error))
            .sidecar_error_code(
                response
                    .error
                    .as_ref()
                    .map_or("unknown", |failure| failure.code.as_str()),
            );
            if let Some(failure) = response
                .error
                .as_ref()
                .filter(|failure| failure.prompt_cache_status.is_some())
            {
                terminal_event = terminal_event.prompt_cache_metrics(
                    failure.prompt_cache_status.as_deref().unwrap_or("bypassed"),
                    failure.prompt_cache_reason.as_deref(),
                    failure.prompt_cache_snapshot_bytes.unwrap_or(0),
                    failure.cached_prefix_tokens.unwrap_or(0),
                    failure.computed_suffix_tokens.unwrap_or(0),
                    valid_sidecar_seconds(failure.prompt_cache_lookup_seconds)
                        .map(|seconds| seconds * 1_000.0),
                    valid_sidecar_seconds(failure.prompt_cache_build_seconds)
                        .map(|seconds| seconds * 1_000.0),
                    valid_sidecar_seconds(failure.prompt_cache_copy_seconds)
                        .map(|seconds| seconds * 1_000.0),
                    valid_sidecar_seconds(failure.prompt_cache_restore_seconds)
                        .map(|seconds| seconds * 1_000.0),
                );
            }
            if let Some(warmup) = response.warmup_result.as_ref() {
                terminal_event = terminal_event
                    .prompt_tokens(warmup.prompt_tokens)
                    .warmup_terminal_facts(
                        warmup.info_observed,
                        warmup.tool_call_observed,
                        warmup.completion_text_seen,
                        warmup.prompt_evaluation_observed,
                        warmup.observed_prompt_tokens,
                        warmup.generation_tokens,
                        warmup.completion.as_deref(),
                        warmup.stream_drained,
                        warmup.evaluation_synchronized,
                    );
                if let (Some(prompt_seconds), Some(generation_seconds)) = (
                    valid_sidecar_seconds(warmup.prompt_time_seconds),
                    valid_sidecar_seconds(warmup.generation_time_seconds),
                ) {
                    terminal_event = terminal_event.sidecar_generation_metrics(
                        prompt_seconds * 1_000.0,
                        generation_seconds * 1_000.0,
                        valid_sidecar_seconds(warmup.prepare_time_seconds)
                            .map(|seconds| seconds * 1_000.0),
                        None,
                        valid_sidecar_seconds(Some(warmup.evaluation_time_seconds))
                            .map(|seconds| seconds * 1_000.0),
                        Some("internal_warmup"),
                    );
                }
            }
            terminal_event.emit();
            let pre_generation_budget_rejection =
                matches!(&error, RuntimeError::InputTooLarge { .. })
                    && response.loaded_model_id.as_deref() == Some(MODEL_ID)
                    && response.loaded_model_version.as_deref() == Some(MODEL_REVISION);
            let safe_to_reuse_session = pre_generation_budget_rejection
                || response.error.as_ref().is_some_and(|failure| {
                    matches!(
                        failure.code.as_str(),
                        "generation_incomplete" | "response_too_large" | "invalid_request"
                    ) || (operation == RuntimeOperation::StartupWarmup
                        && failure.code == "warmup_incomplete"
                        && response.loaded_model_id.as_deref() == Some(MODEL_ID)
                        && response.loaded_model_version.as_deref() == Some(MODEL_REVISION))
                });
            if !safe_to_reuse_session {
                let close = self.retire_session(&mut session, CHILD_EXIT_TIMEOUT).await;
                if let Err(close_error) = close {
                    operation_guard.disarm();
                    return Err(error_after_child_retirement(error, close_error));
                }
            }
            if safe_to_reuse_session {
                session
                    .as_ref()
                    .ok_or(RuntimeError::SidecarMissing)?
                    .process
                    .release_cleanup_gate();
            }
            operation_guard.disarm();
            self.phase.store(
                if safe_to_reuse_session {
                    PHASE_READY
                } else {
                    PHASE_NOT_LOADED
                },
                Ordering::Release,
            );
            return Err(error);
        }
        if response.loaded_model_id.as_deref() != Some(MODEL_ID)
            || response.loaded_model_version.as_deref() != Some(MODEL_REVISION)
        {
            let close = self.retire_session(&mut session, CHILD_EXIT_TIMEOUT).await;
            operation_guard.disarm();
            close?;
            self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
            return Err(RuntimeError::InvalidResponse);
        }
        if operation == RuntimeOperation::StartupWarmup {
            let Some(result) = response.warmup_result else {
                let close = self.retire_session(&mut session, CHILD_EXIT_TIMEOUT).await;
                operation_guard.disarm();
                close?;
                self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
                return Err(RuntimeError::InvalidResponse);
            };
            let cache_status = result
                .prompt_cache_status
                .as_deref()
                .ok_or(RuntimeError::InvalidResponse)?;
            let cached_prefix_tokens = result.cached_prefix_tokens.unwrap_or(0);
            let computed_suffix_tokens = result
                .computed_suffix_tokens
                .unwrap_or(result.prompt_tokens);
            let cache_metrics_valid = valid_warmup_cache_metrics(
                cache_status,
                result.prompt_tokens,
                cached_prefix_tokens,
                computed_suffix_tokens,
            );
            if response.cleanup_result.is_some()
                || result.status != "evaluated"
                || result.prompt_tokens == 0
                || result.prompt_tokens > MAX_TOTAL_TOKENS
                || !valid_warmup_terminal_facts(&result)
                || !result.evaluation_time_seconds.is_finite()
                || result.evaluation_time_seconds < 0.0
                || !valid_sidecar_seconds(result.prompt_time_seconds)
                    .is_some_and(|seconds| seconds > 0.0)
                || !valid_sidecar_seconds(result.generation_time_seconds).is_some()
                || result
                    .prepare_time_seconds
                    .is_some_and(|seconds| !seconds.is_finite() || seconds < 0.0)
                || cached_prefix_tokens > result.prompt_tokens
                || computed_suffix_tokens > result.prompt_tokens
                || !cache_metrics_valid
                || result.prompt_cache_snapshot_bytes.unwrap_or(0) > 512 * 1024 * 1024
            {
                session
                    .as_ref()
                    .ok_or(RuntimeError::SidecarMissing)?
                    .process
                    .release_cleanup_gate();
                operation_guard.disarm();
                self.phase.store(PHASE_READY, Ordering::Release);
                return Err(RuntimeError::InvalidResponse);
            }
            let still_current = still_current.as_ref().is_some_and(|check| check());
            if still_current {
                let _ = self.startup_warmup_generation.compare_exchange(
                    0,
                    child_generation,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            }
            session
                .as_ref()
                .ok_or(RuntimeError::SidecarMissing)?
                .process
                .release_cleanup_gate();
            operation_guard.disarm();
            self.phase.store(PHASE_READY, Ordering::Release);
            return Ok(OperationReply::StartupWarmup(WarmupCompletion {
                request_id,
                child_generation,
                variant: warmup_variant.ok_or(RuntimeError::InvalidResponse)?,
                queue_wait_seconds,
                runtime_startup_seconds,
                model_inventory_validation_seconds: model_timing
                    .map(|timing| timing.inventory_validation_seconds),
                model_load_seconds: model_timing.map(|timing| timing.model_load_seconds),
                model_was_resident: model_timing.is_none(),
                prompt_tokens: result.prompt_tokens,
                generation_tokens: result.generation_tokens,
                completion: result.completion,
                info_observed: result.info_observed,
                tool_call_observed: result.tool_call_observed,
                completion_text_seen: result.completion_text_seen,
                prompt_evaluation_observed: result.prompt_evaluation_observed,
                observed_prompt_tokens: result.observed_prompt_tokens,
                stream_drained: result.stream_drained,
                evaluation_synchronized: result.evaluation_synchronized,
                evaluation_seconds: result.evaluation_time_seconds,
                prepare_seconds: valid_sidecar_seconds(result.prepare_time_seconds),
                prompt_time_seconds: result.prompt_time_seconds,
                generation_time_seconds: result.generation_time_seconds,
                prompt_cache_status: cache_status.to_owned(),
                prompt_cache_reason: result.prompt_cache_reason,
                cached_prefix_tokens,
                computed_suffix_tokens,
                prompt_cache_snapshot_bytes: result.prompt_cache_snapshot_bytes.unwrap_or(0),
                prompt_cache_lookup_seconds: valid_sidecar_seconds(
                    result.prompt_cache_lookup_seconds,
                ),
                prompt_cache_build_seconds: valid_sidecar_seconds(
                    result.prompt_cache_build_seconds,
                ),
                prompt_cache_copy_seconds: valid_sidecar_seconds(result.prompt_cache_copy_seconds),
                prompt_cache_restore_seconds: valid_sidecar_seconds(
                    result.prompt_cache_restore_seconds,
                ),
            }));
        }
        if response.warmup_result.is_some() {
            let close = self.retire_session(&mut session, CHILD_EXIT_TIMEOUT).await;
            operation_guard.disarm();
            close?;
            self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
            return Err(RuntimeError::InvalidResponse);
        }
        let Some(result) = response.cleanup_result else {
            let close = self.retire_session(&mut session, CHILD_EXIT_TIMEOUT).await;
            operation_guard.disarm();
            close?;
            self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
            return Err(RuntimeError::InvalidResponse);
        };
        if result.completion != "stop"
            || result.text.trim().is_empty()
            || result.text.len() > 48 * 1_024
            || result.generation_tokens == 0
            || result.generation_tokens > MAX_OUTPUT_TOKENS
            || !result.prompt_time_seconds.is_finite()
            || !result.generation_time_seconds.is_finite()
        {
            session
                .as_ref()
                .ok_or(RuntimeError::SidecarMissing)?
                .process
                .release_cleanup_gate();
            operation_guard.disarm();
            self.phase.store(PHASE_READY, Ordering::Release);
            return Err(RuntimeError::IncompleteGeneration);
        }
        session
            .as_ref()
            .ok_or(RuntimeError::SidecarMissing)?
            .process
            .release_cleanup_gate();
        operation_guard.disarm();
        self.phase.store(PHASE_READY, Ordering::Release);
        Ok(OperationReply::Cleanup(Completion {
            request_id,
            child_generation,
            text: result.text,
            prompt_tokens: result.prompt_tokens,
            generation_tokens: result.generation_tokens,
            prompt_time_seconds: result.prompt_time_seconds,
            generation_time_seconds: result.generation_time_seconds,
            runtime_startup_seconds,
            model_inventory_validation_seconds: model_timing
                .map(|timing| timing.inventory_validation_seconds),
            model_load_seconds: model_timing.map(|timing| timing.model_load_seconds),
            model_was_resident: model_timing.is_none(),
            queue_wait_seconds,
            sidecar_prepare_seconds: valid_sidecar_seconds(result.prepare_time_seconds),
            sidecar_first_visible_chunk_seconds: valid_sidecar_seconds(
                result.first_visible_chunk_seconds,
            ),
            sidecar_generation_drain_seconds: valid_sidecar_seconds(
                result.generation_drain_seconds,
            ),
            sidecar_prompt_time_scope: result.prompt_time_scope.clone(),
            prompt_cache_status: result.prompt_cache_status.clone(),
            prompt_cache_reason: result.prompt_cache_reason.clone(),
            cached_prefix_tokens: result.cached_prefix_tokens.unwrap_or(0),
            computed_suffix_tokens: result
                .computed_suffix_tokens
                .unwrap_or(result.prompt_tokens),
            prompt_cache_snapshot_bytes: result.prompt_cache_snapshot_bytes.unwrap_or(0),
            prompt_cache_lookup_seconds: valid_sidecar_seconds(result.prompt_cache_lookup_seconds),
            prompt_cache_build_seconds: valid_sidecar_seconds(result.prompt_cache_build_seconds),
            prompt_cache_copy_seconds: valid_sidecar_seconds(result.prompt_cache_copy_seconds),
            prompt_cache_restore_seconds: valid_sidecar_seconds(
                result.prompt_cache_restore_seconds,
            ),
            cleanup_runtime_diagnostics: response.cleanup_diagnostics,
            request_total_seconds: request_started.elapsed().as_secs_f64(),
        }))
    }

    fn log_startup_warmup_first_request(
        &self,
        request_id: &str,
        child_generation: u64,
        session_generation: Option<u64>,
        response: &Response,
    ) {
        if self
            .startup_warmup_generation
            .compare_exchange(child_generation, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let result = response.cleanup_result.as_ref();
        let failure = response.error.as_ref();
        let cache_status = result
            .and_then(|value| value.prompt_cache_status.as_deref())
            .or_else(|| failure.and_then(|value| value.prompt_cache_status.as_deref()))
            .unwrap_or("bypassed");
        let reused = matches!(cache_status, "memory_hit" | "snapshot_hit");
        mlx_diagnostics::event(EventName::StartupWarmup, Severity::Info, Target::Runtime)
            .request_id(request_id)
            .session_generation(session_generation)
            .child_generation(child_generation)
            .phase("terminal")
            .outcome(if reused { "reused" } else { "missed" })
            .stage("generation")
            .prompt_tokens(
                result
                    .map(|value| value.prompt_tokens)
                    .or_else(|| failure.and_then(|value| value.prompt_tokens))
                    .unwrap_or(0),
            )
            .generation_tokens(result.map(|value| value.generation_tokens).unwrap_or(0))
            .prompt_cache_metrics(
                cache_status,
                result
                    .and_then(|value| value.prompt_cache_reason.as_deref())
                    .or_else(|| failure.and_then(|value| value.prompt_cache_reason.as_deref())),
                result
                    .and_then(|value| value.prompt_cache_snapshot_bytes)
                    .or_else(|| failure.and_then(|value| value.prompt_cache_snapshot_bytes))
                    .unwrap_or(0),
                result
                    .and_then(|value| value.cached_prefix_tokens)
                    .or_else(|| failure.and_then(|value| value.cached_prefix_tokens))
                    .unwrap_or(0),
                result
                    .and_then(|value| value.computed_suffix_tokens)
                    .or_else(|| failure.and_then(|value| value.computed_suffix_tokens))
                    .unwrap_or(0),
                result
                    .and_then(|value| value.prompt_cache_lookup_seconds)
                    .or_else(|| failure.and_then(|value| value.prompt_cache_lookup_seconds))
                    .and_then(|seconds| valid_sidecar_seconds(Some(seconds)))
                    .map(|seconds| seconds * 1_000.0),
                result
                    .and_then(|value| value.prompt_cache_build_seconds)
                    .or_else(|| failure.and_then(|value| value.prompt_cache_build_seconds))
                    .and_then(|seconds| valid_sidecar_seconds(Some(seconds)))
                    .map(|seconds| seconds * 1_000.0),
                result
                    .and_then(|value| value.prompt_cache_copy_seconds)
                    .or_else(|| failure.and_then(|value| value.prompt_cache_copy_seconds))
                    .and_then(|seconds| valid_sidecar_seconds(Some(seconds)))
                    .map(|seconds| seconds * 1_000.0),
                result
                    .and_then(|value| value.prompt_cache_restore_seconds)
                    .or_else(|| failure.and_then(|value| value.prompt_cache_restore_seconds))
                    .and_then(|seconds| valid_sidecar_seconds(Some(seconds)))
                    .map(|seconds| seconds * 1_000.0),
            )
            .emit();
    }

    pub async fn unload(&self) -> Result<(), String> {
        let result = tokio::time::timeout(OPERATION_TIMEOUT, self.unload_bounded()).await;
        match result {
            Ok(result) => result,
            Err(_) => Err(RuntimeError::Timeout.to_string()),
        }
    }

    async fn unload_bounded(&self) -> Result<(), String> {
        let cleanup_gate = Arc::clone(cleanup_operation_gate());
        let cleanup_guard = tokio::time::timeout(CHILD_EXIT_TIMEOUT, cleanup_gate.lock_owned())
            .await
            .map_err(|_| RuntimeError::RetainedPending.to_string())?;
        let mut session = tokio::time::timeout(CHILD_EXIT_TIMEOUT, self.session.lock())
            .await
            .map_err(|_| RuntimeError::RetainedPending.to_string())?;
        let Some(existing) = session.as_mut() else {
            self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
            return Ok(());
        };
        if existing.process.has_exited() {
            *session = None;
            self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
            return Ok(());
        }
        if existing.process.cancel.is_cancelled() {
            let exited = existing.process.wait_for_exit(CHILD_EXIT_TIMEOUT).await;
            if exited {
                *session = None;
                self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
                return Ok(());
            }
            self.phase.store(PHASE_STOPPING, Ordering::Release);
            return Err(RuntimeError::RetainedPending.to_string());
        }
        if existing.model.is_none() {
            self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
            return Ok(());
        }
        let request = Request {
            protocol_version: PROTOCOL_VERSION,
            request_id: self.next_id(),
            session_id: Some(self.next_id()),
            model_version: Some(MODEL_REVISION.to_owned()),
            op: "unload".to_owned(),
            model_id: Some(MODEL_ID.to_owned()),
            model_directory: None,
            messages: None,
            max_output_tokens: None,
            stable_prefix_user_bytes: None,
        };
        let token = session
            .as_ref()
            .expect("session exists")
            .process
            .cancel
            .clone();
        session
            .as_ref()
            .expect("session exists")
            .process
            .retain_cleanup_gate(cleanup_guard);
        let mut operation_guard = CancelChildOnDrop::new(token.clone(), Arc::clone(&self.phase));
        let mut request_guard = CancelChildOnDrop::new(token, Arc::clone(&self.phase));
        let response = match exchange(
            session.as_mut().expect("session exists"),
            &request,
            Duration::from_secs(60),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => {
                request_guard.disarm();
                let close = self.retire_session(&mut session, CHILD_EXIT_TIMEOUT).await;
                operation_guard.disarm();
                if let Err(close_error) = close {
                    return Err(close_error.to_string());
                }
                self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
                return Err(error.to_string());
            }
        };
        request_guard.disarm();
        if !response_matches(&request, &response)
            || response.status != "unloaded"
            || response.loaded_model_id.is_some()
            || response.loaded_model_version.is_some()
        {
            let close = self.retire_session(&mut session, CHILD_EXIT_TIMEOUT).await;
            operation_guard.disarm();
            if let Err(close_error) = close {
                return Err(close_error.to_string());
            }
            self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
            return Err("local cleanup model unload failed".to_owned());
        }
        let child = session.as_mut().expect("session remains after unload");
        child.model = None;
        child
            .process
            .lease_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        child.process.release_cleanup_gate();
        operation_guard.disarm();
        self.model_resident.store(false, Ordering::Release);
        self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
        Ok(())
    }

    pub fn begin_shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }
        self.phase.store(PHASE_STOPPING, Ordering::Release);
        if let Some((_, cancel)) = self
            .active_process
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
        {
            cancel.cancel();
        }
    }

    pub async fn shutdown(&self) -> bool {
        self.begin_shutdown();
        if !self.join_generation_task(CHILD_EXIT_TIMEOUT).await {
            return false;
        }
        let Ok(session) = tokio::time::timeout(CHILD_EXIT_TIMEOUT, self.session.lock()).await
        else {
            return false;
        };
        drop(session);
        if !self.join_generation_task(CHILD_EXIT_TIMEOUT).await {
            return false;
        }
        let Ok(mut session) = tokio::time::timeout(CHILD_EXIT_TIMEOUT, self.session.lock()).await
        else {
            return false;
        };
        let Some(child) = session.as_mut() else {
            return true;
        };
        child.process.cancel.cancel();
        if child.process.wait_for_exit(CHILD_EXIT_TIMEOUT).await {
            *session = None;
            self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
            true
        } else {
            false
        }
        // If the wait timed out, retain the session and its model lease. The
        // supervisor continues to own the child and releases the lease on exit.
    }

    async fn join_generation_task(&self, deadline: Duration) -> bool {
        let Some(handle) = self
            .generation_task
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        else {
            return true;
        };
        let mut handoff = GenerationTaskHandoff::new(&self.generation_task, handle);
        match tokio::time::timeout(deadline, handoff.join()).await {
            Ok(Ok(())) => true,
            Ok(Err(_)) => false,
            Err(_) => {
                // The RAII handoff restores the still-owned task if shutdown is
                // cancelled or this bounded wait expires.
                drop(handoff);
                false
            }
        }
    }

    fn ensure_running(&self) -> Result<(), RuntimeError> {
        if self.shutting_down.load(Ordering::Acquire) {
            Err(RuntimeError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn next_id(&self) -> String {
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("vfmlx-{}-{seq}-{nanos}", std::process::id())
    }

    async fn ensure_session(
        &self,
        session: &mut Option<ChildSession>,
        request_id: &str,
        session_generation: Option<u64>,
        trace: Option<crate::metrics::TraceHandle>,
    ) -> Result<(), RuntimeError> {
        self.ensure_running()?;
        if session
            .as_ref()
            .is_some_and(|existing| existing.process.has_exited())
        {
            *session = None;
            self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
        }
        if let Some(existing) = session.as_mut() {
            if existing.process.cancel.is_cancelled() {
                if !existing.process.wait_for_exit(CHILD_EXIT_TIMEOUT).await {
                    self.phase.store(PHASE_STOPPING, Ordering::Release);
                    return Err(RuntimeError::RetainedPending);
                }
                *session = None;
                self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
            } else {
                return Ok(());
            }
        }
        // Resolve the executable before claiming startup or allocating a
        // child generation. Healthy reuse has already returned above.
        let mut command = self.child_command()?;
        // This interval therefore begins only for an actual child spawn/hello
        // attempt, including restarts.
        let _startup_span = trace
            .as_ref()
            .map(|trace| trace.begin(crate::metrics::Stage::RuntimeStartup));
        self.ensure_running()?;
        let id = self
            .child_sequence
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let spawn_started = Instant::now();
        mlx_diagnostics::event(
            EventName::CleanupChildSpawn,
            Severity::Info,
            Target::Runtime,
        )
        .request_id(request_id)
        .session_generation(session_generation)
        .child_generation(id)
        .phase("begin")
        .emit();
        let mut child = match command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,
            Err(_) => {
                mlx_diagnostics::event(
                    EventName::CleanupChildSpawn,
                    Severity::Warn,
                    Target::Runtime,
                )
                .request_id(request_id)
                .session_generation(session_generation)
                .child_generation(id)
                .outcome("failed")
                .reason("spawn_failed")
                .emit();
                return Err(RuntimeError::SidecarMissing);
            }
        };
        let stdin = child.stdin.take().ok_or(RuntimeError::SidecarMissing)?;
        let stdout = child.stdout.take().ok_or(RuntimeError::SidecarMissing)?;
        let pid = child.id().ok_or(RuntimeError::SidecarMissing)?;
        let child_registration =
            crate::model_resources::ChildRegistration::register(pid, id, &self.models_root)
                .ok_or(RuntimeError::SidecarMissing)?;
        let cancel = CancellationToken::new();
        let supervisor_cancel = cancel.clone();
        let lease_slot = Arc::new(Mutex::new(None::<ResolvedCleanupModel>));
        let supervisor_lease = Arc::clone(&lease_slot);
        let cleanup_gate_slot = Arc::new(Mutex::new(None::<tokio::sync::OwnedMutexGuard<()>>));
        let supervisor_cleanup_gate = Arc::clone(&cleanup_gate_slot);
        let active = Arc::clone(&self.active_process);
        let (exit_tx, exit_rx) = watch::channel(false);
        #[cfg(test)]
        {
            self.child_exit_observers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push((id, exit_rx.clone()));
        }
        #[cfg(test)]
        let exit_statuses = Arc::clone(&self.child_exit_statuses);
        let child_cancel = supervisor_cancel.clone();
        let phase = Arc::clone(&self.phase);
        let model_resident = Arc::clone(&self.model_resident);
        let supervisor_registration = child_registration.clone();
        *self
            .active_process
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((id, cancel.clone()));
        tokio::spawn(async move {
            loop {
                if child_cancel.is_cancelled() {
                    let _ = child.start_kill();
                }
                match child.try_wait() {
                    Ok(Some(_status)) => {
                        #[cfg(test)]
                        exit_statuses
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .push((id, _status.success(), _status.code()));
                        break;
                    }
                    Ok(None) => {}
                    Err(_) => {
                        let _ = child.start_kill();
                    }
                }
                tokio::select! {
                    _ = child_cancel.cancelled(), if !child_cancel.is_cancelled() => {},
                    _ = tokio::time::sleep(Duration::from_millis(50)) => {},
                }
            }
            supervisor_lease
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            supervisor_cleanup_gate
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            model_resident.store(false, Ordering::Release);
            phase.store(PHASE_NOT_LOADED, Ordering::Release);
            supervisor_registration.retire_after_exit();
            let mut active = active
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if active
                .as_ref()
                .is_some_and(|(active_id, _)| *active_id == id)
            {
                *active = None;
            }
            let _ = exit_tx.send(true);
        });
        *session = Some(ChildSession {
            stdin,
            stdout: BufReader::new(stdout),
            process: ProcessOwner {
                id,
                cancel,
                exited: exit_rx,
                lease_slot,
                cleanup_gate_slot,
                registration: child_registration,
            },
            model: None,
        });
        let request = Request {
            protocol_version: PROTOCOL_VERSION,
            request_id: self.next_id(),
            session_id: None,
            model_version: None,
            op: "hello".to_owned(),
            model_id: None,
            model_directory: None,
            messages: None,
            max_output_tokens: None,
            stable_prefix_user_bytes: None,
        };
        let token = session
            .as_ref()
            .expect("spawned cleanup session")
            .process
            .cancel
            .clone();
        let mut request_guard = CancelChildOnDrop::new(token, Arc::clone(&self.phase));
        let response = match exchange(
            session.as_mut().expect("spawned cleanup session"),
            &request,
            Duration::from_secs(8),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => {
                request_guard.disarm();
                let close = self.retire_session(session, CHILD_EXIT_TIMEOUT).await;
                return Err(close.err().unwrap_or(error));
            }
        };
        request_guard.disarm();
        if !response_matches(&request, &response)
            || response.status != "ready"
            || response.platform_supported != Some(true)
        {
            let close = self.retire_session(session, CHILD_EXIT_TIMEOUT).await;
            mlx_diagnostics::event(
                EventName::CleanupChildReady,
                Severity::Warn,
                Target::Runtime,
            )
            .request_id(request_id)
            .session_generation(session_generation)
            .child_generation(id)
            .outcome("failed")
            .elapsed_ms(spawn_started.elapsed().as_secs_f64() * 1_000.0)
            .reason("handshake_rejected")
            .emit();
            return Err(close.err().unwrap_or(RuntimeError::UnsupportedPlatform));
        }
        mlx_diagnostics::event(
            EventName::CleanupChildReady,
            Severity::Info,
            Target::Runtime,
        )
        .request_id(request_id)
        .session_generation(session_generation)
        .child_generation(id)
        .outcome("ready")
        .elapsed_ms(spawn_started.elapsed().as_secs_f64() * 1_000.0)
        .emit();
        Ok(())
    }

    async fn ensure_model_loaded(
        &self,
        session: &mut Option<ChildSession>,
        request_id: &str,
        child_generation: u64,
        session_generation: Option<u64>,
        trace: Option<crate::metrics::TraceHandle>,
    ) -> Result<Option<ModelLoadTiming>, RuntimeError> {
        if session
            .as_ref()
            .and_then(|child| child.model.as_ref())
            .is_some_and(|model| {
                model.id() == MODEL_ID
                    && model.revision() == MODEL_REVISION
                    && model.manifest_sha256() == modelstore::MANIFEST_SHA256
            })
        {
            self.phase.store(PHASE_READY, Ordering::Release);
            mlx_diagnostics::event(EventName::CleanupModelLoad, Severity::Info, Target::Runtime)
                .request_id(request_id)
                .session_generation(session_generation)
                .child_generation(child_generation)
                .outcome("reused")
                .reused_model(true, false, false)
                .hashed_counts(0, 0)
                .emit();
            return Ok(None);
        }
        self.phase.store(PHASE_LOADING, Ordering::Release);
        let validation_started = Instant::now();
        let manager = self.manager.clone();
        let root = self.models_root.clone();
        let trace_request_id = request_id.to_owned();
        #[cfg(test)]
        let emit_capture = self.test_emit_capture.clone();
        #[cfg(test)]
        let fixture_fault = self.test_fixture_fault;
        let mut validation_span = trace
            .as_ref()
            .map(|trace| trace.begin(crate::metrics::Stage::FileValidation));
        #[cfg(test)]
        let resolved = if self.test_tiny_model {
            tokio::task::spawn_blocking(move || {
                if let Some(capture) = emit_capture {
                    capture.with_scope(|| {
                        modelstore::tiny_verified_fixture(
                            &root,
                            &manager,
                            &trace_request_id,
                            child_generation,
                            session_generation,
                            fixture_fault,
                        )
                    })
                } else {
                    modelstore::tiny_verified_fixture(
                        &root,
                        &manager,
                        &trace_request_id,
                        child_generation,
                        session_generation,
                        fixture_fault,
                    )
                }
            })
            .await
            .map_err(|_| RuntimeError::ModelUnavailable)?
            .map_err(|_| RuntimeError::ModelUnavailable)
        } else {
            tokio::task::spawn_blocking(move || {
                manager.resolve_ready_for_runtime(
                    &root,
                    &trace_request_id,
                    child_generation,
                    session_generation,
                )
            })
            .await
            .map_err(|_| RuntimeError::ModelUnavailable)?
            .map_err(|_| RuntimeError::ModelUnavailable)
        };
        #[cfg(not(test))]
        let resolved = tokio::task::spawn_blocking(move || {
            manager.resolve_ready_for_runtime(
                &root,
                &trace_request_id,
                child_generation,
                session_generation,
            )
        })
        .await
        .map_err(|_| RuntimeError::ModelUnavailable)?
        .map_err(|_| RuntimeError::ModelUnavailable);
        if let Some(span) = &mut validation_span {
            span.finish();
        }
        let (model, inventory_facts) = resolved?;
        let inventory_validation_seconds = validation_started.elapsed().as_secs_f64();
        mlx_diagnostics::event(
            EventName::CleanupModelInventoryReady,
            Severity::Info,
            Target::Runtime,
        )
        .request_id(request_id)
        .session_generation(session_generation)
        .child_generation(child_generation)
        .inventory(
            inventory_facts.cache,
            inventory_facts.cache_reason,
            inventory_facts.file_count,
            inventory_facts.hashed_files,
            inventory_facts.hashed_bytes,
            inventory_facts.sha256_backend,
            inventory_facts.sha256_hardware_available,
        )
        .validation_ms(inventory_validation_seconds * 1_000.0)
        .emit();
        #[cfg(test)]
        let inventory_file_count_valid = if self.test_tiny_model {
            model.files().len() == 2
        } else {
            model.files().len() == modelstore::MODEL_FILES.len()
        };
        #[cfg(not(test))]
        let inventory_file_count_valid = model.files().len() == modelstore::MODEL_FILES.len();
        if model.id() != MODEL_ID
            || model.repository() != modelstore::REPOSITORY
            || model.revision() != MODEL_REVISION
            || model.manifest_sha256() != modelstore::MANIFEST_SHA256
            || !inventory_file_count_valid
        {
            return Err(RuntimeError::ModelUnavailable);
        }
        let directory = model
            .managed_dir()
            .canonicalize()
            .map_err(|_| RuntimeError::ModelUnavailable)?;
        let request = Request {
            protocol_version: PROTOCOL_VERSION,
            request_id: self.next_id(),
            session_id: Some(self.next_id()),
            model_version: Some(model.revision().to_owned()),
            op: "load".to_owned(),
            model_id: Some(model.id().to_owned()),
            model_directory: Some(directory.to_string_lossy().into_owned()),
            messages: None,
            max_output_tokens: None,
            stable_prefix_user_bytes: None,
        };
        let child = session.as_mut().ok_or(RuntimeError::SidecarMissing)?;
        let load_reservation = child
            .process
            .registration
            .admit_cold_load(
                MODEL_ID,
                MODEL_REVISION,
                crate::model_resources::LoadKind::Cleanup,
            )
            .map_err(|reason| {
                if reason == crate::model_resources::MemoryUnavailableReason::CriticalPressure {
                    RuntimeError::CriticalMemoryPressure
                } else {
                    RuntimeError::MemoryUnavailable(reason)
                }
            })?;
        // The supervisor owns the verified lease before the load request is
        // sent, and keeps it until child.wait() confirms exit or unload succeeds.
        child.model = Some(model.clone());
        child
            .process
            .lease_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .replace(model.clone());
        let token = child.process.cancel.clone();
        let mut request_guard = CancelChildOnDrop::new(token, Arc::clone(&self.phase));
        let load_started = Instant::now();
        let mut model_load_span = trace
            .as_ref()
            .map(|trace| trace.begin(crate::metrics::Stage::ModelLoad));
        mlx_diagnostics::event(EventName::CleanupModelLoad, Severity::Info, Target::Runtime)
            .request_id(request_id)
            .session_generation(session_generation)
            .child_generation(child_generation)
            .phase("dispatched")
            .model_identity(MODEL_ID, MODEL_REVISION)
            .reused_model(false, false, true)
            .emit();
        let exchange_result = exchange(
            session.as_mut().expect("session exists"),
            &request,
            MODEL_LOAD_TIMEOUT,
        )
        .await;
        let response = match exchange_result {
            Ok(response) => response,
            Err(error) => {
                request_guard.disarm();
                let close = self.retire_session(session, CHILD_EXIT_TIMEOUT).await;
                if let Err(close_error) = close {
                    self.phase.store(PHASE_STOPPING, Ordering::Release);
                    return Err(close_error);
                }
                self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
                return Err(error);
            }
        };
        settle_cleanup_load_reservation_if_terminal(
            load_reservation,
            &request,
            &response,
            MODEL_ID,
            MODEL_REVISION,
        );
        if let Some(span) = &mut model_load_span {
            span.finish();
        }
        let model_load_seconds = load_started.elapsed().as_secs_f64();
        request_guard.disarm();
        if !response_matches(&request, &response)
            || response.status != "loaded"
            || response.loaded_model_id.as_deref() != Some(MODEL_ID)
            || response.loaded_model_version.as_deref() != Some(MODEL_REVISION)
        {
            let error = map_sidecar_error(response.error.as_ref());
            mlx_diagnostics::event(EventName::CleanupModelLoad, Severity::Warn, Target::Runtime)
                .request_id(request_id)
                .session_generation(session_generation)
                .child_generation(child_generation)
                .outcome("failed")
                .load_ms(load_started.elapsed().as_secs_f64() * 1_000.0)
                .error_code(runtime_error_code(&error))
                .emit();
            let close = self.retire_session(session, CHILD_EXIT_TIMEOUT).await;
            if let Err(close_error) = close {
                self.phase.store(PHASE_STOPPING, Ordering::Release);
                return Err(error_after_child_retirement(error, close_error));
            }
            self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
            return Err(error);
        }
        self.model_resident.store(true, Ordering::Release);
        self.phase.store(PHASE_READY, Ordering::Release);
        mlx_diagnostics::event(EventName::CleanupModelLoad, Severity::Info, Target::Runtime)
            .request_id(request_id)
            .session_generation(session_generation)
            .child_generation(child_generation)
            .outcome("loaded")
            .load_ms(model_load_seconds * 1_000.0)
            .reused_model(false, false, true)
            .emit();
        Ok(Some(ModelLoadTiming {
            inventory_validation_seconds,
            model_load_seconds,
        }))
    }

    async fn retire_session(
        &self,
        session: &mut Option<ChildSession>,
        deadline: Duration,
    ) -> Result<(), RuntimeError> {
        let Some(child) = session.as_mut() else {
            return Ok(());
        };
        child.process.cancel.cancel();
        if !child.process.wait_for_exit(deadline).await {
            self.phase.store(PHASE_STOPPING, Ordering::Release);
            return Err(RuntimeError::RetainedPending);
        }
        let id = child.process.id;
        *session = None;
        self.clear_active_child(id);
        self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
        Ok(())
    }

    fn clear_active_child(&self, id: u64) {
        let mut active = self
            .active_process
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active
            .as_ref()
            .is_some_and(|(active_id, _)| *active_id == id)
        {
            *active = None;
        }
    }
}
struct RequestTerminalLog {
    request_id: String,
    source_history_id: String,
    started: Instant,
    session_generation: Option<u64>,
    finished: bool,
}

impl RequestTerminalLog {
    fn new(
        request_id: String,
        source_history_id: String,
        started: Instant,
        session_generation: Option<u64>,
    ) -> Self {
        Self {
            request_id,
            source_history_id,
            started,
            session_generation,
            finished: false,
        }
    }

    fn finish(&mut self, outcome: &str, stage: &str, elapsed_seconds: f64) {
        mlx_diagnostics::event(EventName::CleanupRequest, Severity::Info, Target::Runtime)
            .request_id(&self.request_id)
            .session_generation(self.session_generation)
            .source_history_id(self.source_history_id.parse().ok())
            .phase("terminal")
            .outcome(outcome)
            .stage(stage)
            .request_total_ms(elapsed_seconds * 1_000.0)
            .emit();
        self.finished = true;
    }
}

impl Drop for RequestTerminalLog {
    fn drop(&mut self) {
        if !self.finished {
            mlx_diagnostics::event(EventName::CleanupRequest, Severity::Warn, Target::Runtime)
                .request_id(&self.request_id)
                .session_generation(self.session_generation)
                .source_history_id(self.source_history_id.parse().ok())
                .phase("terminal")
                .outcome("cancelled_or_abandoned")
                .stage("unknown")
                .request_total_ms(self.started.elapsed().as_secs_f64() * 1_000.0)
                .emit();
        }
    }
}

struct CancelChildOnDrop {
    cancel: CancellationToken,
    phase: Arc<AtomicU8>,
    armed: bool,
}

struct CancelGenerationOnDrop {
    cancel: CancellationToken,
    phase: Arc<AtomicU8>,
    armed: Arc<AtomicBool>,
}

struct GenerationTaskHandoff<'a> {
    slot: &'a Mutex<Option<tokio::task::JoinHandle<()>>>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl<'a> GenerationTaskHandoff<'a> {
    fn new(
        slot: &'a Mutex<Option<tokio::task::JoinHandle<()>>>,
        handle: tokio::task::JoinHandle<()>,
    ) -> Self {
        Self {
            slot,
            handle: Some(handle),
        }
    }

    async fn join(&mut self) -> Result<(), tokio::task::JoinError> {
        let result = self
            .handle
            .as_mut()
            .expect("unjoined generation task")
            .await;
        self.handle = None;
        result
    }
}

impl Drop for GenerationTaskHandoff<'_> {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let mut slot = self.slot.lock().unwrap_or_else(|p| p.into_inner());
            if slot.is_none() {
                *slot = Some(handle);
            } else {
                // A newer task has already taken ownership of the slot. Keep
                // the old task tracked by aborting it rather than detaching it.
                handle.abort();
            }
        }
    }
}

struct StartupWarmupInFlightGuard {
    owners: Arc<AtomicUsize>,
}

impl StartupWarmupInFlightGuard {
    fn new(owners: Arc<AtomicUsize>) -> Self {
        owners.fetch_add(1, Ordering::AcqRel);
        Self { owners }
    }

    fn retain(owners: Arc<AtomicUsize>) -> Self {
        owners.fetch_add(1, Ordering::AcqRel);
        Self { owners }
    }
}

impl Drop for StartupWarmupInFlightGuard {
    fn drop(&mut self) {
        self.owners.fetch_sub(1, Ordering::AcqRel);
    }
}

struct RestorePhaseOnDrop<'a> {
    phase: Arc<AtomicU8>,
    model_resident: Arc<AtomicBool>,
    shutting_down: &'a AtomicBool,
}

impl Drop for RestorePhaseOnDrop<'_> {
    fn drop(&mut self) {
        if self.shutting_down.load(Ordering::Acquire) {
            self.phase.store(PHASE_STOPPING, Ordering::Release);
        } else if self.phase.load(Ordering::Acquire) == PHASE_STOPPING {
            // A cancelled request keeps its cleanup process and cleanup gate owned by
            // the supervisor until the child has actually exited.
        } else if self.model_resident.load(Ordering::Acquire) {
            self.phase.store(PHASE_READY, Ordering::Release);
        } else {
            self.phase.store(PHASE_NOT_LOADED, Ordering::Release);
        }
    }
}

impl CancelChildOnDrop {
    fn new(cancel: CancellationToken, phase: Arc<AtomicU8>) -> Self {
        Self {
            cancel,
            phase,
            armed: true,
        }
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl CancelGenerationOnDrop {
    fn new(cancel: CancellationToken, phase: Arc<AtomicU8>) -> Self {
        Self {
            cancel,
            phase,
            armed: Arc::new(AtomicBool::new(true)),
        }
    }

    fn disarm(&mut self) {
        self.armed.store(false, Ordering::Release);
    }
}

impl Drop for CancelGenerationOnDrop {
    fn drop(&mut self) {
        if self.armed.swap(false, Ordering::AcqRel) {
            self.phase.store(PHASE_STOPPING, Ordering::Release);
            self.cancel.cancel();
        }
    }
}

impl Drop for CancelChildOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.cancel.cancel();
            self.phase.store(PHASE_STOPPING, Ordering::Release);
        }
    }
}

async fn exchange(
    child: &mut ChildSession,
    request: &Request,
    deadline: Duration,
) -> Result<Response, RuntimeError> {
    let mut encoded = serde_json::to_vec(request).map_err(|_| RuntimeError::InvalidResponse)?;
    if encoded.len() > MAX_REQUEST_LINE_BYTES {
        return Err(RuntimeError::InvalidResponse);
    }
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
                    "sidecar response too large",
                ));
            }
            line.extend_from_slice(&available[..take]);
            child.stdout.consume(take);
            bytes += take;
            if line_end.is_some() {
                break;
            }
        }
        if bytes == 0 || !line.ends_with(b"\n") {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "sidecar exited",
            ));
        }
        serde_json::from_slice::<Response>(&line)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid sidecar response"))
    })
    .await
    .map_err(|_| RuntimeError::Timeout)?
    .map_err(|_| RuntimeError::InvalidResponse)
}

fn valid_sidecar_seconds(value: Option<f64>) -> Option<f64> {
    value.filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
}

fn valid_warmup_cache_metrics(
    status: &str,
    prompt_tokens: usize,
    cached_prefix_tokens: usize,
    computed_suffix_tokens: usize,
) -> bool {
    match status {
        "memory_hit" | "snapshot_hit" => {
            cached_prefix_tokens > 0
                && cached_prefix_tokens.saturating_add(computed_suffix_tokens) == prompt_tokens
        }
        "built" => {
            cached_prefix_tokens == 0
                && computed_suffix_tokens > 0
                && computed_suffix_tokens < prompt_tokens
        }
        "bypassed" => cached_prefix_tokens == 0 && computed_suffix_tokens == prompt_tokens,
        _ => false,
    }
}

fn runtime_error_code(error: &RuntimeError) -> &'static str {
    match error {
        RuntimeError::UnsupportedPlatform => "unsupported_platform",
        RuntimeError::SidecarMissing => "sidecar_missing",
        RuntimeError::ModelUnavailable => "model_unavailable",
        RuntimeError::ModelMismatch => "model_mismatch",
        RuntimeError::ModelLoadFailed => "model_load_failed",
        RuntimeError::CriticalMemoryPressure => "memory_pressure_critical",
        RuntimeError::MemoryUnavailable(_) => "local_memory_unavailable",
        RuntimeError::ModelNotLoaded => "model_not_loaded",
        RuntimeError::InputTooLarge { .. } => "input_too_large",
        RuntimeError::Timeout => "timeout",
        RuntimeError::Cancelled => "cancelled",
        RuntimeError::StartupWarmupYielded { .. } => "startup_warmup_yielded",
        RuntimeError::RetainedPending => "retained_pending",
        RuntimeError::InvalidResponse => "invalid_response",
        RuntimeError::IncompleteGeneration => "incomplete_generation",
        RuntimeError::GenerationFailed => "generation_failed",
    }
}

fn runtime_error_stage(error: &RuntimeError) -> &'static str {
    match error {
        RuntimeError::UnsupportedPlatform => "platform_admission",
        RuntimeError::SidecarMissing | RuntimeError::ModelUnavailable => "model_readiness",
        RuntimeError::ModelMismatch | RuntimeError::ModelNotLoaded => "model_admission",
        RuntimeError::ModelLoadFailed => "model_load",
        RuntimeError::CriticalMemoryPressure => "model_admission",
        RuntimeError::MemoryUnavailable(_) => "model_admission",
        RuntimeError::InputTooLarge { .. } => "prompt_admission",
        RuntimeError::Timeout => "timeout",
        RuntimeError::Cancelled => "cancellation",
        RuntimeError::StartupWarmupYielded { .. } => "foreground_demand",
        RuntimeError::RetainedPending => "child_shutdown",
        RuntimeError::InvalidResponse => "protocol",
        RuntimeError::IncompleteGeneration | RuntimeError::GenerationFailed => "generation",
    }
}

fn runtime_error_outcome(error: &RuntimeError) -> &'static str {
    match error {
        RuntimeError::Cancelled => "cancelled",
        RuntimeError::StartupWarmupYielded { .. } => "yielded",
        RuntimeError::Timeout => "timeout",
        RuntimeError::InputTooLarge { .. } => "rejected",
        _ => "failed",
    }
}

fn response_matches(request: &Request, response: &Response) -> bool {
    response.protocol_version == PROTOCOL_VERSION
        && response.request_id == request.request_id
        && response.session_id == request.session_id
        && response.model_version == request.model_version
        && response.op == request.op
}

fn settle_cleanup_load_reservation_if_terminal(
    reservation: crate::model_resources::PendingLoadReservation,
    request: &Request,
    response: &Response,
    model_id: &str,
    model_version: &str,
) -> bool {
    let terminal = response_matches(request, response)
        && (response.status == "error"
            || (response.status == "loaded"
                && response.loaded_model_id.as_deref() == Some(model_id)
                && response.loaded_model_version.as_deref() == Some(model_version)));
    if terminal {
        reservation.settle();
    }
    terminal
}

fn completed_response_is_safe_to_reuse(request: &Request, response: &Response) -> bool {
    response_matches(request, response)
        && response.status == "completed"
        && response.platform_supported == Some(true)
        && response.loaded_model_id.as_deref() == Some(MODEL_ID)
        && response.loaded_model_version.as_deref() == Some(MODEL_REVISION)
}

async fn settle_unclaimed_response(
    mut session: OwnedCleanupSession,
    request: &Request,
    response: &Response,
    force_retire: bool,
    phase: Arc<AtomicU8>,
    model_resident: Arc<AtomicBool>,
    shutting_down: Arc<AtomicBool>,
) {
    if !force_retire && completed_response_is_safe_to_reuse(request, response) {
        if let Some(child) = session.as_ref() {
            child.process.release_cleanup_gate();
            model_resident.store(true, Ordering::Release);
            if shutting_down.load(Ordering::Acquire) {
                phase.store(PHASE_STOPPING, Ordering::Release);
            } else {
                phase.store(PHASE_READY, Ordering::Release);
            }
        }
        return;
    }
    if let Some(child) = session.as_mut() {
        child.process.cancel.cancel();
        if child.process.wait_for_exit(CHILD_EXIT_TIMEOUT).await {
            *session = None;
            phase.store(PHASE_NOT_LOADED, Ordering::Release);
        } else {
            phase.store(PHASE_STOPPING, Ordering::Release);
        }
    }
}

#[derive(Debug)]
enum GenerationExchangeOutcome {
    Response(Box<Response>),
    SettledCancellation(Option<RuntimeError>),
    Retire(RuntimeError),
}

enum GenerationOwnerOutcome {
    Response(Box<Response>),
    SettledCancellation(Option<RuntimeError>),
    Retired(RuntimeError),
}

#[derive(Debug)]
enum GenerationRequestWrite {
    Written(Option<tokio::time::Instant>),
    Suppressed,
}

fn latch_cancel_deadline(deadline: &mut Option<tokio::time::Instant>) {
    deadline.get_or_insert_with(|| tokio::time::Instant::now() + Duration::from_secs(2));
}

async fn write_generation_request<W: AsyncWrite + Unpin>(
    child: &mut W,
    encoded: &[u8],
    deadline: tokio::time::Instant,
    cancel_requested: &CancellationToken,
    pre_dispatch_yield: Option<&CancellationToken>,
) -> Result<GenerationRequestWrite, RuntimeError> {
    let mut offset = 0;
    let mut cancellation_deadline = None;
    while offset < encoded.len() {
        if offset == 0
            && (cancel_requested.is_cancelled()
                || pre_dispatch_yield.is_some_and(CancellationToken::is_cancelled))
        {
            return Ok(GenerationRequestWrite::Suppressed);
        }
        tokio::select! {
            biased;
            _ = async {
                match pre_dispatch_yield {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending::<()>().await,
                }
            }, if offset == 0 => return Ok(GenerationRequestWrite::Suppressed),
            result = child.write(&encoded[offset..]) => {
                let written = result.map_err(|_| RuntimeError::InvalidResponse)?;
                if written == 0 {
                    return Err(RuntimeError::InvalidResponse);
                }
                offset += written;
            }
            _ = cancel_requested.cancelled(), if offset == 0 => {
                return Ok(GenerationRequestWrite::Suppressed);
            }
            _ = cancel_requested.cancelled(), if offset > 0 && cancellation_deadline.is_none() => {
                latch_cancel_deadline(&mut cancellation_deadline);
            }
            _ = async {
                match pre_dispatch_yield {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending::<()>().await,
                }
            }, if offset > 0 && cancellation_deadline.is_none() => {
                latch_cancel_deadline(&mut cancellation_deadline);
            }
            _ = tokio::time::sleep_until(cancellation_deadline.unwrap_or(deadline)), if cancellation_deadline.is_some() => {
                return Err(RuntimeError::Timeout);
            }
            _ = tokio::time::sleep_until(deadline), if cancellation_deadline.is_none() => {
                return Err(RuntimeError::Timeout);
            }
        }
    }
    let flush_deadline = cancellation_deadline.unwrap_or(deadline);
    tokio::select! {
        biased;
        result = tokio::time::timeout_at(flush_deadline, child.flush()) => {
            result.map_err(|_| RuntimeError::Timeout)?
                .map_err(|_| RuntimeError::InvalidResponse)?;
        }
        _ = cancel_requested.cancelled(), if cancellation_deadline.is_none() => {
            latch_cancel_deadline(&mut cancellation_deadline);
            tokio::time::timeout_at(cancellation_deadline.expect("cancel deadline set"), child.flush())
                .await
                .map_err(|_| RuntimeError::Timeout)?
                .map_err(|_| RuntimeError::InvalidResponse)?;
        }
        _ = async {
            match pre_dispatch_yield {
                Some(token) => token.cancelled().await,
                None => std::future::pending::<()>().await,
            }
        }, if cancellation_deadline.is_none() => {
            latch_cancel_deadline(&mut cancellation_deadline);
            tokio::time::timeout_at(cancellation_deadline.expect("cancel deadline set"), child.flush())
                .await
                .map_err(|_| RuntimeError::Timeout)?
                .map_err(|_| RuntimeError::InvalidResponse)?;
        }
    }
    Ok(GenerationRequestWrite::Written(cancellation_deadline))
}

fn response_line(line: &[u8]) -> Result<Response, RuntimeError> {
    if line.is_empty() || line.len() > MAX_RESPONSE_LINE_BYTES || !line.ends_with(b"\n") {
        return Err(RuntimeError::InvalidResponse);
    }
    serde_json::from_slice::<Response>(line).map_err(|_| RuntimeError::InvalidResponse)
}

/// Read one bounded JSONL frame. `line` belongs to the operation owner and is
/// retained if the future is cancelled while `fill_buf` is pending.
async fn read_response_frame<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    line: &mut Vec<u8>,
    deadline: tokio::time::Instant,
) -> Result<Response, RuntimeError> {
    loop {
        let available = tokio::time::timeout_at(deadline, reader.fill_buf())
            .await
            .map_err(|_| RuntimeError::Timeout)?
            .map_err(|_| RuntimeError::InvalidResponse)?;
        if available.is_empty() {
            return Err(RuntimeError::InvalidResponse);
        }
        let line_end = available.iter().position(|byte| *byte == b'\n');
        let take = line_end.map_or(available.len(), |end| end + 1);
        if line.len().saturating_add(take) > MAX_RESPONSE_LINE_BYTES {
            return Err(RuntimeError::InvalidResponse);
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if line_end.is_some() {
            return response_line(line);
        }
    }
}

fn valid_cancel_pair(
    original_request: &Request,
    cancel_request: &Request,
    terminal: &Response,
    acknowledgement: &Response,
) -> bool {
    response_matches(original_request, terminal)
        && terminal.status == "cancelled"
        && terminal.platform_supported == Some(true)
        && terminal.loaded_model_id.as_deref() == Some(MODEL_ID)
        && terminal.loaded_model_version.as_deref() == Some(MODEL_REVISION)
        && terminal.cleanup_result.is_none()
        && terminal.warmup_result.is_none()
        && terminal.error.is_none()
        && response_matches(cancel_request, acknowledgement)
        && acknowledgement.status == "cancelled"
        && acknowledgement.platform_supported == Some(true)
        && acknowledgement.loaded_model_id.as_deref() == Some(MODEL_ID)
        && acknowledgement.loaded_model_version.as_deref() == Some(MODEL_REVISION)
        && acknowledgement.cleanup_result.is_none()
        && acknowledgement.warmup_result.is_none()
        && acknowledgement.error.is_none()
}

async fn exchange_generation_with_cancel_io<W, R>(
    stdin: &mut W,
    stdout: &mut R,
    request: &Request,
    cancel_request: &Request,
    cancel_requested: &CancellationToken,
    pre_dispatch_yield: Option<&CancellationToken>,
) -> GenerationExchangeOutcome
where
    W: AsyncWrite + Unpin,
    R: tokio::io::AsyncBufRead + Unpin,
{
    let (exchange_cancel, _cancellation_bridge_guard) =
        if let Some(yield_cancel) = pre_dispatch_yield {
            let exchange_cancel = CancellationToken::new();
            let bridge = exchange_cancel.clone();
            let caller_cancel = cancel_requested.clone();
            let yield_cancel = yield_cancel.clone();
            let cancellation_bridge = tokio::spawn(async move {
                tokio::select! {
                    _ = caller_cancel.cancelled() => bridge.cancel(),
                    _ = yield_cancel.cancelled() => bridge.cancel(),
                }
            });
            (
                exchange_cancel,
                Some(CancellationBridgeGuard(cancellation_bridge)),
            )
        } else {
            (cancel_requested.clone(), None)
        };
    let generation_deadline = tokio::time::Instant::now() + INFERENCE_TIMEOUT;
    let mut encoded = match serde_json::to_vec(request) {
        Ok(encoded) if encoded.len() <= MAX_REQUEST_LINE_BYTES => encoded,
        _ => return GenerationExchangeOutcome::Retire(RuntimeError::InvalidResponse),
    };
    encoded.push(b'\n');
    let mut cancellation_deadline = match write_generation_request(
        stdin,
        &encoded,
        generation_deadline,
        // Preserve the caller's token synchronously through the first write.
        // The yield bridge below is useful while reading a dispatched request,
        // but must never delay a caller-drop suppression at offset zero.
        cancel_requested,
        pre_dispatch_yield,
    )
    .await
    {
        Ok(GenerationRequestWrite::Written(deadline)) => deadline,
        Ok(GenerationRequestWrite::Suppressed) => {
            return GenerationExchangeOutcome::SettledCancellation(None);
        }
        Err(error) => return GenerationExchangeOutcome::Retire(error),
    };

    let mut original_line = Vec::new();
    let read_deadline = cancellation_deadline.unwrap_or(generation_deadline);
    let primary_error = tokio::select! {
        biased;
        result = read_response_frame(stdout, &mut original_line, read_deadline) => {
            match result {
                Ok(response) => return GenerationExchangeOutcome::Response(Box::new(response)),
                Err(RuntimeError::Timeout) if cancellation_deadline.is_some() => {
                    return GenerationExchangeOutcome::Retire(RuntimeError::Timeout);
                }
                Err(RuntimeError::Timeout) => Some(RuntimeError::Timeout),
                Err(error) => return GenerationExchangeOutcome::Retire(error),
            }
        }
        _ = exchange_cancel.cancelled() => {
            // The original terminal may have completed at the same instant
            // as Drop. Prefer that already-buffered complete frame.
            if original_line.ends_with(b"\n") {
                return match response_line(&original_line) {
                    Ok(response) => GenerationExchangeOutcome::Response(Box::new(response)),
                    Err(error) => GenerationExchangeOutcome::Retire(error),
                };
            }
            latch_cancel_deadline(&mut cancellation_deadline);
            None
        }
        _ = tokio::time::sleep_until(read_deadline), if cancellation_deadline.is_some() => {
            return GenerationExchangeOutcome::Retire(RuntimeError::Timeout);
        }
        _ = tokio::time::sleep_until(generation_deadline), if cancellation_deadline.is_none() => {
            Some(RuntimeError::Timeout)
        }
    };

    // A generation write is never interrupted halfway through. At this point
    // the full original frame is flushed, so the control frame cannot corrupt it.
    let cancellation_deadline = cancellation_deadline
        .unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(2));
    let mut cancel_encoded = match serde_json::to_vec(cancel_request) {
        Ok(encoded) if encoded.len() <= MAX_REQUEST_LINE_BYTES => encoded,
        _ => return GenerationExchangeOutcome::Retire(RuntimeError::InvalidResponse),
    };
    cancel_encoded.push(b'\n');
    match tokio::time::timeout_at(cancellation_deadline, async {
        stdin.write_all(&cancel_encoded).await?;
        stdin.flush().await
    })
    .await
    {
        Err(_) => {
            return GenerationExchangeOutcome::Retire(
                primary_error.unwrap_or(RuntimeError::Timeout),
            );
        }
        Ok(Err(_)) => {
            return GenerationExchangeOutcome::Retire(
                primary_error.unwrap_or(RuntimeError::InvalidResponse),
            );
        }
        Ok(Ok(())) => {}
    }

    let terminal = if original_line.ends_with(b"\n") {
        response_line(&original_line)
    } else {
        read_response_frame(stdout, &mut original_line, cancellation_deadline).await
    };
    let mut acknowledgement_line = Vec::new();
    let acknowledgement = match terminal {
        Ok(terminal) => {
            read_response_frame(stdout, &mut acknowledgement_line, cancellation_deadline)
                .await
                .map(|acknowledgement| (terminal, acknowledgement))
        }
        Err(error) => Err(error),
    };
    match acknowledgement {
        Ok((terminal, acknowledgement))
            if valid_cancel_pair(request, cancel_request, &terminal, &acknowledgement) =>
        {
            GenerationExchangeOutcome::SettledCancellation(primary_error)
        }
        Ok(_) => GenerationExchangeOutcome::Retire(
            primary_error.unwrap_or(RuntimeError::InvalidResponse),
        ),
        Err(error) => GenerationExchangeOutcome::Retire(primary_error.unwrap_or(error)),
    }
}

async fn exchange_generation_with_cancel(
    child: &mut ChildSession,
    request: &Request,
    cancel_request: &Request,
    cancel_requested: &CancellationToken,
    pre_dispatch_yield: Option<&CancellationToken>,
) -> GenerationExchangeOutcome {
    exchange_generation_with_cancel_io(
        &mut child.stdin,
        &mut child.stdout,
        request,
        cancel_request,
        cancel_requested,
        pre_dispatch_yield,
    )
    .await
}

struct CancellationBridgeGuard(tokio::task::JoinHandle<()>);

impl Drop for CancellationBridgeGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

const MAX_TOTAL_TOKENS: usize = 8_192;

fn map_sidecar_error(failure: Option<&SidecarFailure>) -> RuntimeError {
    if let Some(failure) = failure.filter(|failure| failure.code == "input_too_large") {
        let (Some(prompt_tokens), Some(requested_output_tokens), Some(max_total_tokens)) = (
            failure.prompt_tokens,
            failure.requested_output_tokens,
            failure.max_total_tokens,
        ) else {
            return RuntimeError::InvalidResponse;
        };
        if requested_output_tokens == 0 || requested_output_tokens > MAX_OUTPUT_TOKENS {
            return RuntimeError::InvalidResponse;
        }
        let (requested_total_tokens, overflow) =
            prompt_tokens.overflowing_add(requested_output_tokens);
        if max_total_tokens != MAX_TOTAL_TOKENS
            || (!overflow && requested_total_tokens <= max_total_tokens)
            || (!overflow && failure.requested_total_tokens != Some(requested_total_tokens))
            || (overflow && failure.requested_total_tokens.is_some())
        {
            return RuntimeError::InvalidResponse;
        }
        return RuntimeError::InputTooLarge {
            prompt_tokens,
            requested_output_tokens,
            requested_total_tokens: (!overflow).then_some(requested_total_tokens),
            max_total_tokens,
        };
    }
    match failure.map(|failure| failure.code.as_str()) {
        Some("unsupported_platform") => RuntimeError::UnsupportedPlatform,
        Some("model_unavailable" | "invalid_model" | "model_version_mismatch") => {
            RuntimeError::ModelUnavailable
        }
        Some("model_not_loaded") => RuntimeError::ModelNotLoaded,
        Some("model_load_failed") => RuntimeError::ModelLoadFailed,
        Some("memory_pressure_critical") => RuntimeError::CriticalMemoryPressure,
        Some("generation_incomplete") => RuntimeError::IncompleteGeneration,
        Some("warmup_incomplete") => RuntimeError::GenerationFailed,
        Some("generation_failed" | "response_too_large") => RuntimeError::GenerationFailed,
        _ => RuntimeError::InvalidResponse,
    }
}

fn platform_supported() -> bool {
    crate::ondevice_asr::platform_supported()
}

fn valid_warmup_terminal_facts(result: &SidecarWarmupResult) -> bool {
    valid_warmup_terminal_values(
        result.prompt_tokens,
        result.observed_prompt_tokens,
        result.generation_tokens,
        result.completion.as_deref(),
        result.info_observed,
        result.tool_call_observed,
        result.completion_text_seen,
        result.prompt_evaluation_observed,
        result.stream_drained,
        result.evaluation_synchronized,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "each optional terminal observation is independently required for warmup admission"
)]
fn valid_warmup_terminal_values(
    prompt_tokens: usize,
    observed_prompt_tokens: Option<usize>,
    generation_tokens: Option<usize>,
    completion: Option<&str>,
    info_observed: Option<bool>,
    tool_call_observed: Option<bool>,
    completion_text_seen: Option<bool>,
    prompt_evaluation_observed: Option<bool>,
    stream_drained: Option<bool>,
    evaluation_synchronized: Option<bool>,
) -> bool {
    let Some(observed_prompt_tokens) = observed_prompt_tokens else {
        return false;
    };
    let Some(generation_tokens) = generation_tokens else {
        return false;
    };
    let Some(completion) = completion else {
        return false;
    };

    info_observed == Some(true)
        && tool_call_observed == Some(false)
        && completion_text_seen.is_some()
        && prompt_evaluation_observed == Some(true)
        && observed_prompt_tokens > 0
        && observed_prompt_tokens <= prompt_tokens
        && generation_tokens <= 1
        && match completion {
            "stop" => true,
            "length" => generation_tokens == 1,
            _ => false,
        }
        && stream_drained == Some(true)
        && evaluation_synchronized == Some(true)
}

pub fn shared_runtime(models_root: &Path, manager: Manager) -> Arc<Runtime> {
    static RUNTIMES: OnceLock<Mutex<HashMap<PathBuf, Arc<Runtime>>>> = OnceLock::new();
    let runtimes = RUNTIMES.get_or_init(|| Mutex::new(HashMap::new()));
    let key = models_root.to_path_buf();
    let mut map = runtimes
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    map.entry(key.clone())
        .or_insert_with(|| Arc::new(Runtime::new(key, manager)))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warmup_control_registration_race_cannot_lose_foreground_demand() {
        let runtime = Arc::new(Runtime::new(
            std::env::temp_dir().join("voiceflow-task24-control-race"),
            Manager::new(),
        ));
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let demand_runtime = Arc::clone(&runtime);
        let demand_barrier = Arc::clone(&barrier);
        let demand = std::thread::spawn(move || {
            demand_barrier.wait();
            demand_runtime.begin_foreground_model_demand();
        });
        let install_runtime = Arc::clone(&runtime);
        let install_barrier = Arc::clone(&barrier);
        let token = Arc::new(Mutex::new(None));
        let install_token = Arc::clone(&token);
        let install = std::thread::spawn(move || {
            install_barrier.wait();
            *install_token.lock().unwrap() =
                Some(install_runtime.install_startup_warmup_control(1));
        });
        barrier.wait();
        demand.join().expect("demand thread joins");
        install.join().expect("registration thread joins");
        let token = token.lock().unwrap().take().expect("control installed");
        assert!(
            token.is_cancelled(),
            "either lock order preserves the demand"
        );
        assert_eq!(runtime.startup_warmup_demands.load(Ordering::Acquire), 1);
        runtime.end_foreground_model_demand();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn command_resolution_failure_precedes_startup_and_child_generation() {
        let root = std::env::temp_dir().join(format!(
            "voiceflow-task18-command-resolution-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create owned command-resolution fixture");
        let mut runtime = Runtime::new(root.join("models"), Manager::new());
        runtime.test_platform_supported = Some(true);
        runtime.test_command_resolution_error = true;
        let capture = crate::mlx_diagnostics::EmitRecordCaptureForTest::default();
        runtime.test_emit_capture = Some(capture.clone());
        let mut timer = crate::metrics::Metrics::default().stop_to_insert(
            crate::metrics::MetricGroup::unknown("task18_command_resolution_failure"),
            CancellationToken::new(),
        );
        let trace = timer.bind_trace(181);
        let result = capture
            .with_scope_future(runtime.complete_with_source_history_and_prefix_traced(
                vec![
                    ("system".to_owned(), "synthetic system".to_owned()),
                    ("user".to_owned(), "synthetic input".to_owned()),
                ],
                None,
                None,
                Some(trace.clone()),
            ))
            .await;
        assert!(matches!(result, Err(RuntimeError::SidecarMissing)));
        let snapshot = trace.snapshot();
        timer.finish(crate::metrics::DeliveryOutcome::Failed, None);
        stop_reap_and_remove(&runtime, &root).await;

        assert!(!snapshot
            .stages
            .iter()
            .any(|(stage, _, _)| *stage == crate::metrics::Stage::RuntimeStartup));
        assert_eq!(runtime.child_sequence.load(Ordering::Relaxed), 0);
        let events: Vec<serde_json::Value> = capture
            .snapshot()
            .iter()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        assert!(!events.iter().any(|event| {
            event.get("event").and_then(serde_json::Value::as_str) == Some("cleanup_child_spawn")
                && event.get("phase").and_then(serde_json::Value::as_str) == Some("begin")
        }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn traced_runtime_inventory_and_hash_rejections_stop_before_load() {
        for (fault, expected_hash_event) in [
            (modelstore::TinyFixtureFault::InventoryReject, None),
            (
                modelstore::TinyFixtureFault::HashReject,
                Some((0_u64, 5_u64)),
            ),
        ] {
            static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
            let suffix = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "voiceflow-task18-runtime-model-reject-{}-{suffix}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).expect("create isolated runtime fixture");
            let worker_path = root.join("fake-worker.py");
            std::fs::write(
                &worker_path,
                "import json,sys\nfor line in sys.stdin:\n r=json.loads(line); b={'protocol_version':1,'request_id':r['request_id'],'session_id':r.get('session_id'),'model_version':r.get('model_version'),'op':r['op'],'platform_supported':True}\n if r['op']=='hello': b.update(status='ready')\n elif r['op']=='load': b.update(status='loaded',loaded_model_id=r['model_id'],loaded_model_version=r['model_version'])\n print(json.dumps(b,separators=(',',':')),flush=True)\n",
            )
            .expect("write owned fake worker");
            let mut runtime = Runtime::with_test_worker(
                root.join("models"),
                Manager::new(),
                PathBuf::from("python3"),
                vec!["-B".into(), worker_path.to_string_lossy().into_owned()],
            );
            runtime.test_fixture_fault = fault;
            let capture = crate::mlx_diagnostics::EmitRecordCaptureForTest::default();
            runtime.test_emit_capture = Some(capture.clone());
            let mut timer = crate::metrics::Metrics::default().stop_to_insert(
                crate::metrics::MetricGroup::unknown("task18_runtime_model_rejection"),
                CancellationToken::new(),
            );
            let trace = timer.bind_trace(182);
            let result = capture
                .with_scope_future(runtime.complete_with_source_history_and_prefix_traced(
                    vec![
                        ("system".to_owned(), "synthetic system".to_owned()),
                        ("user".to_owned(), "synthetic request".to_owned()),
                    ],
                    None,
                    None,
                    Some(trace.clone()),
                ))
                .await;
            assert!(matches!(result, Err(RuntimeError::ModelUnavailable)));
            let snapshot = trace.snapshot();
            timer.finish(crate::metrics::DeliveryOutcome::Failed, None);
            stop_reap_and_remove(&runtime, &root).await;

            let stages = snapshot
                .stages
                .iter()
                .map(|(stage, _, _)| *stage)
                .collect::<Vec<_>>();
            for reached in [
                crate::metrics::Stage::MlxGateWait,
                crate::metrics::Stage::RuntimeStartup,
                crate::metrics::Stage::FileValidation,
            ] {
                assert!(stages.contains(&reached), "rejection reaches {reached:?}");
            }
            for omitted in [
                crate::metrics::Stage::ModelLoad,
                crate::metrics::Stage::FinalGuard,
            ] {
                assert!(!stages.contains(&omitted), "rejection omits {omitted:?}");
            }
            assert!(snapshot.reported_stages.is_empty());
            let events: Vec<serde_json::Value> = capture
                .snapshot()
                .iter()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            let failed = events.iter().find(|event| {
                event.get("event").and_then(serde_json::Value::as_str)
                    == Some("cleanup_model_inventory")
                    && event.get("outcome").and_then(serde_json::Value::as_str) == Some("failed")
            });
            assert!(
                failed.is_some(),
                "actual verifier emits inventory rejection"
            );
            let failed = failed.expect("checked above");
            assert!(failed
                .get("request_id")
                .and_then(serde_json::Value::as_str)
                .is_some());
            assert_eq!(
                failed
                    .get("child_generation")
                    .and_then(serde_json::Value::as_u64),
                Some(1)
            );
            assert_eq!(
                failed
                    .get("session_generation")
                    .and_then(serde_json::Value::as_u64),
                Some(182)
            );
            let hash_events = events
                .iter()
                .filter(|event| {
                    event.get("event").and_then(serde_json::Value::as_str)
                        == Some("cleanup_model_file_hash")
                })
                .collect::<Vec<_>>();
            if let Some((index, bytes)) = expected_hash_event {
                assert_eq!(hash_events.len(), 1);
                assert_eq!(
                    hash_events[0]
                        .get("file_index")
                        .and_then(serde_json::Value::as_u64),
                    Some(index)
                );
                assert_eq!(
                    hash_events[0]
                        .get("hashed_bytes")
                        .and_then(serde_json::Value::as_u64),
                    Some(bytes)
                );
                assert_eq!(
                    hash_events[0]
                        .get("outcome")
                        .and_then(serde_json::Value::as_str),
                    Some("failed")
                );
            } else {
                assert!(
                    hash_events.is_empty(),
                    "inventory mismatch rejects before file hashing"
                );
            }
            let terminal = events
                .iter()
                .find(|event| {
                    event.get("event").and_then(serde_json::Value::as_str)
                        == Some("cleanup_request")
                        && event.get("phase").and_then(serde_json::Value::as_str)
                            == Some("terminal")
                })
                .expect("request terminal records model readiness failure");
            assert_eq!(
                terminal.get("outcome").and_then(serde_json::Value::as_str),
                Some("failed")
            );
            assert_eq!(
                terminal.get("stage").and_then(serde_json::Value::as_str),
                Some("model_readiness")
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn accepted_child_natural_exit_restarts_stale_session() {
        static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
        let suffix = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "voiceflow-task18-natural-exit-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create owned natural-exit fixture");
        let worker_path = root.join("fake-worker.py");
        let marker = root.join("first-child-exited");
        let worker_source = format!(
            "import json,os,sys\nmarker={:?}\nfor line in sys.stdin:\n r=json.loads(line); b={{'protocol_version':1,'request_id':r['request_id'],'session_id':r.get('session_id'),'model_version':r.get('model_version'),'op':r['op'],'platform_supported':True}}\n if r['op']=='hello': b.update(status='ready')\n elif r['op']=='load': b.update(status='loaded',loaded_model_id=r['model_id'],loaded_model_version=r['model_version'])\n elif r['op']=='cleanup': b.update(status='completed',loaded_model_id=r['model_id'],loaded_model_version=r['model_version'],cleanup_result={{'text':'synthetic accepted','prompt_tokens':3,'generation_tokens':1,'prompt_time_seconds':0.01,'generation_time_seconds':0.01,'completion':'stop'}})\n print(json.dumps(b,separators=(',',':')),flush=True)\n if r['op']=='cleanup' and not os.path.exists(marker): open(marker,'w').close(); sys.exit(0)\n",
            marker.to_string_lossy()
        );
        std::fs::write(&worker_path, worker_source).expect("write immutable owned exit worker");
        let mut runtime = Runtime::with_test_worker(
            root.join("models"),
            Manager::new(),
            PathBuf::from("python3"),
            vec!["-B".into(), worker_path.to_string_lossy().into_owned()],
        );
        let capture = crate::mlx_diagnostics::EmitRecordCaptureForTest::default();
        runtime.test_emit_capture = Some(capture.clone());
        let mut traces = Vec::new();
        let mut child_generations = Vec::new();
        for _ in 0..3 {
            let mut timer = crate::metrics::Metrics::default().stop_to_insert(
                crate::metrics::MetricGroup::unknown("task18_natural_exit_retry"),
                CancellationToken::new(),
            );
            let trace = timer.bind_trace(183);
            let completion = capture
                .with_scope_future(runtime.complete_with_source_history_and_prefix_traced(
                    vec![
                        ("system".to_owned(), "synthetic system".to_owned()),
                        ("user".to_owned(), "synthetic accepted request".to_owned()),
                    ],
                    None,
                    None,
                    Some(trace.clone()),
                ))
                .await
                .expect("valid accepted completion returns across child exit/restart");
            assert_eq!(completion.text, "synthetic accepted");
            child_generations.push(completion.child_generation);
            timer.finish(crate::metrics::DeliveryOutcome::PasteConfirmed, None);
            traces.push(trace.snapshot());
            if traces.len() == 1 {
                assert!(runtime.wait_for_all_test_children().await.is_some());
                let guard = runtime
                    .session
                    .try_lock()
                    .expect("session remains stored after natural exit");
                assert!(
                    guard.is_some(),
                    "natural exit leaves a stale Some session for next ensure"
                );
                drop(guard);
            }
        }
        let first_stages = traces[0]
            .stages
            .iter()
            .map(|(stage, _, _)| *stage)
            .collect::<Vec<_>>();
        let restart_stages = traces[1]
            .stages
            .iter()
            .map(|(stage, _, _)| *stage)
            .collect::<Vec<_>>();
        let healthy_stages = traces[2]
            .stages
            .iter()
            .map(|(stage, _, _)| *stage)
            .collect::<Vec<_>>();
        for stages in [&first_stages, &restart_stages] {
            for stage in [
                crate::metrics::Stage::RuntimeStartup,
                crate::metrics::Stage::FileValidation,
                crate::metrics::Stage::ModelLoad,
            ] {
                assert!(
                    stages.contains(&stage),
                    "cold/restart request reaches {stage:?}"
                );
            }
        }
        for omitted in [
            crate::metrics::Stage::RuntimeStartup,
            crate::metrics::Stage::FileValidation,
            crate::metrics::Stage::ModelLoad,
        ] {
            assert!(
                !healthy_stages.contains(&omitted),
                "healthy reuse omits {omitted:?}"
            );
        }
        assert_eq!(child_generations, vec![1, 2, 2]);
        assert!(traces.iter().all(|trace| trace.session_generation == 183));
        runtime.shutdown().await;
        let exits = runtime
            .wait_for_all_test_children()
            .await
            .expect("both child statuses observed");
        assert_eq!(exits.len(), 2);
        assert_eq!(
            exits
                .iter()
                .map(|(id, _, _)| *id)
                .collect::<std::collections::BTreeSet<_>>(),
            [1, 2].into_iter().collect()
        );
        assert_eq!(runtime.manager.test_lease_count(), 0);
        std::fs::remove_dir_all(&root)
            .expect("remove fixture after all child exits and lease release");
        let events: Vec<serde_json::Value> = capture
            .snapshot()
            .iter()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        assert!(events.iter().any(
            |event| event.get("event").and_then(serde_json::Value::as_str)
                == Some("cleanup_request")
                && event.get("phase").and_then(serde_json::Value::as_str) == Some("terminal")
                && event.get("outcome").and_then(serde_json::Value::as_str) == Some("completed")
        ));
    }

    async fn stop_reap_and_remove(runtime: &Runtime, root: &std::path::Path) {
        runtime.shutdown().await;
        let exits = runtime
            .wait_for_all_test_children()
            .await
            .expect("every owned fake child must be observed exited before fixture removal");
        let expected_ids: std::collections::HashSet<_> = runtime
            .child_exit_observers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(
            exits.len(),
            expected_ids.len(),
            "each spawned child has one successful try_wait observation"
        );
        let ids: std::collections::HashSet<_> = exits.iter().map(|(id, _, _)| *id).collect();
        assert_eq!(
            ids.len(),
            expected_ids.len(),
            "child reap observations retain distinct child ids"
        );
        assert_eq!(
            ids, expected_ids,
            "each registered child id has its own observed exit status"
        );
        assert_eq!(
            runtime.manager.test_lease_count(),
            0,
            "all model leases are released before fixture deletion"
        );
        std::fs::remove_dir_all(root)
            .expect("remove owned runtime fixture after confirmed child exit");
    }

    #[test]
    fn response_identity_guard_rejects_each_mismatched_control_field() {
        let request = Request::cleanup(
            "synthetic-request-1".to_owned(),
            Some("synthetic-session-1".to_owned()),
            vec![
                ("system".to_owned(), "synthetic prompt".to_owned()),
                ("user".to_owned(), "synthetic input".to_owned()),
            ],
            None,
        );
        let mut frame = serde_json::json!({
            "protocol_version": PROTOCOL_VERSION,
            "request_id": request.request_id,
            "session_id": request.session_id,
            "model_version": request.model_version,
            "op": request.op,
            "status": "completed",
            "platform_supported": true,
            "loaded_model_id": MODEL_ID,
            "loaded_model_version": MODEL_REVISION,
        });
        let valid: Response = serde_json::from_value(frame.clone()).expect("typed valid frame");
        assert!(response_matches(&request, &valid));
        for (field, mismatch) in [
            ("protocol_version", serde_json::json!(PROTOCOL_VERSION + 1)),
            ("request_id", serde_json::json!("synthetic-request-wrong")),
            ("session_id", serde_json::Value::Null),
            ("model_version", serde_json::Value::Null),
            ("op", serde_json::json!("warmup")),
        ] {
            frame[field] = mismatch;
            let parsed: Response = serde_json::from_value(frame.clone())
                .expect("identity mismatch retains the typed response shape");
            assert!(
                !response_matches(&request, &parsed),
                "identity field {field} must remain guarded"
            );
            frame[field] = match field {
                "protocol_version" => serde_json::json!(PROTOCOL_VERSION),
                "request_id" => serde_json::json!("synthetic-request-1"),
                "session_id" => serde_json::json!("synthetic-session-1"),
                "model_version" => serde_json::json!(MODEL_REVISION),
                "op" => serde_json::json!("cleanup"),
                _ => unreachable!(),
            };
        }
    }

    #[test]
    fn cancel_pair_rejects_mismatched_identity_status_and_payload() {
        let original = Request::cleanup(
            "synthetic-original".to_owned(),
            Some("synthetic-session".to_owned()),
            vec![
                ("system".to_owned(), "synthetic prompt".to_owned()),
                ("user".to_owned(), "synthetic input".to_owned()),
            ],
            None,
        );
        let cancel = Request::cancel("synthetic-cancel".to_owned(), original.session_id.clone());
        assert_ne!(original.request_id, cancel.request_id);
        let terminal_base = serde_json::json!({
            "protocol_version": PROTOCOL_VERSION,
            "request_id": original.request_id,
            "session_id": original.session_id,
            "model_version": MODEL_REVISION,
            "op": "cleanup",
            "status": "cancelled",
            "platform_supported": true,
            "loaded_model_id": MODEL_ID,
            "loaded_model_version": MODEL_REVISION,
            "cleanup_result": null,
            "warmup_result": null,
            "error": null
        });
        let acknowledgement_base = serde_json::json!({
            "protocol_version": PROTOCOL_VERSION,
            "request_id": cancel.request_id,
            "session_id": cancel.session_id,
            "model_version": MODEL_REVISION,
            "op": "cancel",
            "status": "cancelled",
            "platform_supported": true,
            "loaded_model_id": MODEL_ID,
            "loaded_model_version": MODEL_REVISION,
            "cleanup_result": null,
            "warmup_result": null,
            "error": null
        });
        let terminal: Response =
            serde_json::from_value(terminal_base.clone()).expect("typed original terminal");
        let acknowledgement: Response = serde_json::from_value(acknowledgement_base.clone())
            .expect("typed cancel acknowledgement");
        assert!(valid_cancel_pair(
            &original,
            &cancel,
            &terminal,
            &acknowledgement
        ));

        for (field, mismatch) in [
            ("protocol_version", serde_json::json!(PROTOCOL_VERSION + 1)),
            ("request_id", serde_json::json!("synthetic-stale")),
            ("session_id", serde_json::json!("synthetic-other-session")),
            (
                "model_version",
                serde_json::json!("synthetic-other-revision"),
            ),
            ("op", serde_json::json!("warmup")),
            ("status", serde_json::json!("completed")),
            ("platform_supported", serde_json::json!(false)),
            (
                "loaded_model_id",
                serde_json::json!("synthetic-other-model"),
            ),
            (
                "loaded_model_version",
                serde_json::json!("synthetic-other-revision"),
            ),
            (
                "cleanup_result",
                serde_json::json!({
                    "text": "must-not-escape", "prompt_tokens": 5, "generation_tokens": 1,
                    "prompt_time_seconds": 0.01, "generation_time_seconds": 0.01,
                    "completion": "stop"
                }),
            ),
            ("error", serde_json::json!({"code": "invalid_request"})),
            (
                "warmup_result",
                serde_json::json!({
                    "status": "evaluated",
                    "prompt_tokens": 1,
                    "evaluation_time_seconds": 0.01
                }),
            ),
        ] {
            let mut bad_terminal = terminal_base.clone();
            bad_terminal[field] = mismatch.clone();
            let bad_terminal = serde_json::from_value::<Response>(bad_terminal)
                .unwrap_or_else(|error| panic!("valid typed terminal mutation {field}: {error}"));
            assert!(
                !valid_cancel_pair(&original, &cancel, &bad_terminal, &acknowledgement),
                "original terminal field {field} must be rejected"
            );
            let mut bad_acknowledgement = acknowledgement_base.clone();
            bad_acknowledgement[field] = mismatch;
            let bad_acknowledgement = serde_json::from_value::<Response>(bad_acknowledgement)
                .unwrap_or_else(|error| {
                    panic!("valid typed acknowledgement mutation {field}: {error}")
                });
            assert!(
                !valid_cancel_pair(&original, &cancel, &terminal, &bad_acknowledgement),
                "cancel acknowledgement field {field} must be rejected"
            );
        }
        let mut malformed_terminal = terminal_base;
        malformed_terminal["cleanup_result"] =
            serde_json::json!({"text": "missing required fields"});
        assert!(serde_json::from_value::<Response>(malformed_terminal).is_err());
        let mut malformed_acknowledgement = acknowledgement_base;
        malformed_acknowledgement["warmup_result"] = serde_json::json!({"status": "evaluated"});
        assert!(serde_json::from_value::<Response>(malformed_acknowledgement).is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dropping_previous_owner_handoff_restores_join_handle_to_runtime_slot() {
        let slot = Mutex::new(None);
        let reached = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let task_reached = Arc::clone(&reached);
        let task_release = Arc::clone(&release);
        *slot.lock().unwrap() = Some(tokio::spawn(async move {
            task_reached.notify_one();
            task_release.notified().await;
        }));
        let previous = slot.lock().unwrap().take().expect("registered owner task");
        let handoff = GenerationTaskHandoff::new(&slot, previous);
        reached.notified().await;
        drop(handoff);
        assert!(
            slot.lock().unwrap().is_some(),
            "interrupted wait restores tracked task"
        );
        release.notify_one();
        let restored = slot.lock().unwrap().take().expect("restored owner task");
        let joined = restored.await;
        assert!(joined.is_ok());
    }

    #[test]
    fn warmup_owner_overlap_uses_one_atomic_source_of_truth() {
        use std::sync::Barrier;
        use std::thread;

        let owners = Arc::new(AtomicUsize::new(0));
        let old_owner = StartupWarmupInFlightGuard::retain(Arc::clone(&owners));
        let barrier = Arc::new(Barrier::new(2));
        let new_owners = Arc::clone(&owners);
        let new_barrier = Arc::clone(&barrier);
        let new_owner = thread::spawn(move || {
            new_barrier.wait();
            let owner = StartupWarmupInFlightGuard::retain(new_owners);
            assert!(owner.owners.load(Ordering::Acquire) > 0);
            owner
        });
        barrier.wait();
        drop(old_owner);
        let new_owner = new_owner.join().expect("new warmup owner created");
        assert_eq!(owners.load(Ordering::Acquire), 1);
        drop(new_owner);
        assert_eq!(owners.load(Ordering::Acquire), 0);
    }

    #[test]
    fn cancel_deadline_is_shared_across_write_flush_and_read_phases() {
        let original = tokio::time::Instant::now() + Duration::from_millis(200);
        let mut deadline = Some(original);
        latch_cancel_deadline(&mut deadline);
        assert_eq!(deadline, Some(original));
        std::thread::sleep(Duration::from_millis(220));
        latch_cancel_deadline(&mut deadline);
        assert_eq!(
            deadline,
            Some(original),
            "later phases keep the exhausted original budget"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_during_partial_generation_write_uses_one_two_second_deadline() {
        let (mut writer, mut reader) = tokio::io::duplex(1);
        let mut encoded = vec![b'x'; 32 * 1024];
        encoded.push(b'\n');
        let cancellation = CancellationToken::new();
        let deadline = tokio::time::Instant::now() + INFERENCE_TIMEOUT;
        let mut write = Box::pin(write_generation_request(
            &mut writer,
            &encoded,
            deadline,
            &cancellation,
            None,
        ));
        let mut first_byte = [0u8; 1];
        let partial_write_started = tokio::select! {
            biased;
            result = tokio::time::timeout(
                Duration::from_secs(1),
                tokio::io::AsyncReadExt::read_exact(&mut reader, &mut first_byte),
            ) => result.is_ok_and(|result| result.is_ok()),
            result = &mut write => panic!("bounded duplex unexpectedly accepted the whole request: {result:?}"),
        };
        let cancelled_at = tokio::time::Instant::now();
        cancellation.cancel();
        let write_result = tokio::time::timeout(Duration::from_millis(2_500), &mut write).await;
        let elapsed = cancelled_at.elapsed();
        drop(write);

        assert!(
            partial_write_started,
            "controlled writer accepted a partial request"
        );
        assert!(
            matches!(write_result, Ok(Err(RuntimeError::Timeout))),
            "partial JSON write is retired at its cancel deadline: {write_result:?}"
        );
        assert!(
            elapsed >= Duration::from_secs(2) && elapsed < Duration::from_millis(2_500),
            "one absolute grace interval starts at cancellation (elapsed={elapsed:?})"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn warmup_yield_during_backpressured_partial_write_retires_within_two_seconds() {
        let (mut stdin, mut worker_stdin) = tokio::io::duplex(1);
        let (worker_stdout, stdout) = tokio::io::duplex(64);
        let request = Request::startup_warmup(
            "synthetic-yield-partial-write".to_owned(),
            Some("synthetic-yield-session".to_owned()),
            vec![("user".to_owned(), "synthetic warmup".to_owned())],
            None,
        );
        let cancel = Request::cancel(
            "synthetic-yield-partial-cancel".to_owned(),
            request.session_id.clone(),
        );
        let caller_cancel = CancellationToken::new();
        let yield_cancel = CancellationToken::new();
        let exchange_request = request.clone();
        let exchange_cancel = cancel;
        let exchange_caller_cancel = caller_cancel.clone();
        let exchange_yield_cancel = yield_cancel.clone();
        let mut exchange = tokio::spawn(async move {
            exchange_generation_with_cancel_io(
                &mut stdin,
                &mut tokio::io::BufReader::new(stdout),
                &exchange_request,
                &exchange_cancel,
                &exchange_caller_cancel,
                Some(&exchange_yield_cancel),
            )
            .await
        });

        let mut first_byte = [0u8; 1];
        tokio::select! {
            biased;
            result = tokio::io::AsyncReadExt::read_exact(&mut worker_stdin, &mut first_byte) => {
                result.expect("worker observes first warmup request byte");
            }
            result = &mut exchange => panic!("one-byte duplex unexpectedly accepted full warmup frame: {result:?}"),
        }
        let cancelled_at = tokio::time::Instant::now();
        yield_cancel.cancel();
        assert!(
            !caller_cancel.is_cancelled(),
            "foreground caller token remains live"
        );
        let outcome = tokio::time::timeout(Duration::from_millis(2_500), &mut exchange)
            .await
            .expect("yield cancellation bounds the blocked partial write")
            .expect("exchange task remains joinable");
        let elapsed = cancelled_at.elapsed();
        assert!(matches!(
            outcome,
            GenerationExchangeOutcome::Retire(RuntimeError::Timeout)
        ));
        assert!(elapsed >= Duration::from_secs(2) && elapsed < Duration::from_millis(2_500));
        assert!(exchange.is_finished());
        drop(worker_stdin);
        drop(worker_stdout);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn caller_cancel_at_offset_zero_suppresses_exchange_with_yield_uncancelled() {
        let (mut stdin, mut worker_stdin) = tokio::io::duplex(64 * 1024);
        let (worker_stdout, mut stdout) = tokio::io::duplex(64);
        let request = Request::cleanup(
            "synthetic-caller-drop-before-dispatch".to_owned(),
            Some("synthetic-caller-drop-session".to_owned()),
            vec![("user".to_owned(), "synthetic input".to_owned())],
            None,
        );
        let cancel = Request::cancel(
            "synthetic-caller-drop-cancel".to_owned(),
            request.session_id.clone(),
        );
        let caller_cancel = CancellationToken::new();
        caller_cancel.cancel();
        let yield_cancel = CancellationToken::new();

        let outcome = exchange_generation_with_cancel_io(
            &mut stdin,
            &mut tokio::io::BufReader::new(&mut stdout),
            &request,
            &cancel,
            &caller_cancel,
            Some(&yield_cancel),
        )
        .await;
        drop(stdin);
        let mut unexpected = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut worker_stdin, &mut unexpected)
            .await
            .expect("read fake worker stdin after suppressed request");
        drop(worker_stdout);

        assert!(
            matches!(
                outcome,
                GenerationExchangeOutcome::SettledCancellation(None)
            ),
            "already-cancelled caller is synchronously suppressed: {outcome:?}"
        );
        assert!(unexpected.is_empty(), "no request frame was written");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancel_deadline_carries_from_partial_write_through_stalled_terminal_and_ack_reads() {
        let (mut client_stdin, mut server_stdin) = tokio::io::duplex(1);
        let (server_stdout, client_stdout) = tokio::io::duplex(1);
        let client_stdout = tokio::io::BufReader::new(client_stdout);
        let request = Request::cleanup(
            "synthetic-deadline-original".to_owned(),
            Some("synthetic-deadline-session".to_owned()),
            vec![("user".to_owned(), "synthetic input".to_owned())],
            None,
        );
        let cancel = Request::cancel(
            "synthetic-deadline-cancel".to_owned(),
            request.session_id.clone(),
        );
        let cancellation = CancellationToken::new();
        let exchange_request = request.clone();
        let exchange_cancel = cancel.clone();
        let exchange_cancellation = cancellation.clone();
        let started = tokio::time::Instant::now();
        let mut exchange = tokio::spawn(async move {
            exchange_generation_with_cancel_io(
                &mut client_stdin,
                &mut tokio::io::BufReader::new(client_stdout),
                &exchange_request,
                &exchange_cancel,
                &exchange_cancellation,
                None,
            )
            .await
        });
        let mut first_byte = [0u8; 1];
        tokio::select! {
            biased;
            result = tokio::io::AsyncReadExt::read_exact(&mut server_stdin, &mut first_byte) => {
                result.expect("worker receives a partial original request");
            }
            _result = &mut exchange => panic!("one-byte duplex unexpectedly accepted the full request"),
        }
        cancellation.cancel();

        // Keep the production write blocked until only a small part of the
        // latched two-second grace remains, then let the exact frame finish.
        tokio::time::sleep(Duration::from_millis(1_800)).await;
        let mut original_frame = vec![first_byte[0]];
        let mut byte = [0u8; 1];
        loop {
            tokio::time::timeout(
                Duration::from_secs(3),
                tokio::io::AsyncReadExt::read_exact(&mut server_stdin, &mut byte),
            )
            .await
            .expect("worker's original-frame drain remains live")
            .expect("worker drains the rest of the original frame");
            original_frame.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        let parsed_original: serde_json::Value = serde_json::from_slice(&original_frame)
            .expect("production request frame remains complete");
        assert_eq!(parsed_original["request_id"], request.request_id);

        let mut cancel_frame = Vec::new();
        loop {
            tokio::time::timeout(
                Duration::from_secs(3),
                tokio::io::AsyncReadExt::read_exact(&mut server_stdin, &mut byte),
            )
            .await
            .expect("worker's cancel-frame read remains live")
            .expect("worker receives the cancel frame");
            cancel_frame.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        let parsed_cancel: serde_json::Value =
            serde_json::from_slice(&cancel_frame).expect("production cancel frame is complete");
        assert_eq!(parsed_cancel["request_id"], cancel.request_id);

        // The fake worker deliberately withholds both terminal and ACK. The
        // exchange must retire at the original deadline, with no read reset.
        let outcome = tokio::time::timeout(Duration::from_millis(500), &mut exchange)
            .await
            .expect("original cancellation deadline bounds the stalled reads")
            .expect("production exchange task remains joinable");
        assert!(matches!(
            outcome,
            GenerationExchangeOutcome::Retire(RuntimeError::Timeout)
        ));
        assert!(started.elapsed() < Duration::from_millis(2_500));
        assert!(exchange.is_finished());
        drop(server_stdout);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn warmup_yield_deadline_carries_from_partial_write_through_stalled_terminal_and_ack_reads(
    ) {
        let (mut client_stdin, mut server_stdin) = tokio::io::duplex(1);
        let (server_stdout, client_stdout) = tokio::io::duplex(1);
        let request = Request::startup_warmup(
            "synthetic-yield-deadline-original".to_owned(),
            Some("synthetic-yield-deadline-session".to_owned()),
            vec![("user".to_owned(), "synthetic warmup".to_owned())],
            None,
        );
        let cancel = Request::cancel(
            "synthetic-yield-deadline-cancel".to_owned(),
            request.session_id.clone(),
        );
        let caller_cancel = CancellationToken::new();
        let yield_cancel = CancellationToken::new();
        let exchange_request = request.clone();
        let exchange_cancel = cancel.clone();
        let exchange_caller_cancel = caller_cancel.clone();
        let exchange_yield_cancel = yield_cancel.clone();
        let started = tokio::time::Instant::now();
        let mut exchange = tokio::spawn(async move {
            exchange_generation_with_cancel_io(
                &mut client_stdin,
                &mut tokio::io::BufReader::new(client_stdout),
                &exchange_request,
                &exchange_cancel,
                &exchange_caller_cancel,
                Some(&exchange_yield_cancel),
            )
            .await
        });
        let mut first_byte = [0u8; 1];
        tokio::select! {
            biased;
            result = tokio::io::AsyncReadExt::read_exact(&mut server_stdin, &mut first_byte) => {
                result.expect("worker observes first warmup request byte");
            }
            result = &mut exchange => panic!("one-byte duplex unexpectedly accepted full warmup frame: {result:?}"),
        }
        let yielded_at = tokio::time::Instant::now();
        yield_cancel.cancel();
        assert!(
            !caller_cancel.is_cancelled(),
            "foreground caller token remains live"
        );

        // Keep the original write blocked until a small part of its one grace
        // interval remains, then release the complete original and cancel frames.
        tokio::time::sleep(Duration::from_millis(1_800)).await;
        let mut original_frame = vec![first_byte[0]];
        let mut byte = [0u8; 1];
        loop {
            tokio::time::timeout(
                Duration::from_secs(3),
                tokio::io::AsyncReadExt::read_exact(&mut server_stdin, &mut byte),
            )
            .await
            .expect("worker's original-frame drain remains live")
            .expect("worker drains the rest of the original frame");
            original_frame.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        let parsed_original: serde_json::Value = serde_json::from_slice(&original_frame)
            .expect("production warmup frame remains complete");
        assert_eq!(parsed_original["request_id"], request.request_id);
        assert_eq!(parsed_original["op"], "warmup");

        let mut cancel_frame = Vec::new();
        loop {
            tokio::time::timeout(
                Duration::from_millis(250),
                tokio::io::AsyncReadExt::read_exact(&mut server_stdin, &mut byte),
            )
            .await
            .expect("worker receives cancel frame inside the original grace")
            .expect("worker receives the cancel frame");
            cancel_frame.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        let parsed_cancel: serde_json::Value = serde_json::from_slice(&cancel_frame)
            .expect("production cancel frame remains complete");
        assert_eq!(parsed_cancel["request_id"], cancel.request_id);
        assert!(yielded_at.elapsed() < Duration::from_millis(2_000));

        let outcome = tokio::time::timeout(Duration::from_millis(500), &mut exchange)
            .await
            .expect("same yield deadline bounds terminal and ACK reads")
            .expect("production exchange remains joinable");
        assert!(matches!(
            outcome,
            GenerationExchangeOutcome::Retire(RuntimeError::Timeout)
        ));
        assert!(started.elapsed() < Duration::from_millis(2_500));
        assert!(exchange.is_finished());
        drop(server_stdin);
        drop(server_stdout);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn traced_cleanup_negative_matrix_load_and_identity_rejections() {
        for (mode, expect_cleanup_exchange) in [
            ("load_reject", false),
            ("identity_request_id", true),
            ("identity_protocol", true),
            ("identity_session", true),
            ("identity_model_version", true),
            ("identity_operation", true),
            ("identity_platform", true),
            ("identity_loaded_model_id", true),
            ("identity_loaded_model_version", true),
            ("incomplete_completion", true),
            ("input_too_large", true),
        ] {
            static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
            let suffix = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "voiceflow-task18-negative-{mode}-{}-{suffix}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).expect("create isolated fake-worker fixture");
            let worker_path = root.join("fake-worker.py");
            let worker_source = format!(
                r#"import json, os, sys
mode = {mode:?}
cleanup_count = 0
for line in sys.stdin:
 req=json.loads(line)
 base={{'protocol_version':1,'request_id':req['request_id'],'session_id':req.get('session_id'),'model_version':req.get('model_version'),'op':req['op'],'platform_supported':True}}
 if req['op']=='hello': base.update(status='ready')
 elif req['op']=='load':
  if mode=='load_reject': base.update(status='error',error={{'code':'model_load_failed'}})
  else: base.update(status='loaded',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'])
 elif req['op']=='cleanup':
  cleanup_count += 1
  if mode=='input_too_large' and cleanup_count==1: base.update(status='error',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'],error={{'code':'input_too_large','prompt_tokens':8192,'requested_output_tokens':1,'requested_total_tokens':8193,'max_total_tokens':8192}})
  else:
   base.update(status='completed',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'],cleanup_result={{'text':'synthetic completion','prompt_tokens':5,'generation_tokens':1,'prompt_time_seconds':0.01,'generation_time_seconds':0.02,'completion':'length' if mode=='incomplete_completion' else 'stop'}})
  fault_marker=os.path.join(os.path.dirname(__file__),'identity-fault-used')
  inject_identity_fault=mode.startswith('identity_') and not os.path.exists(fault_marker)
  if inject_identity_fault:
   with open(fault_marker,'w',encoding='utf-8') as marker: marker.write('used')
   if mode=='identity_request_id': base['request_id']='wrong-request-id'
   elif mode=='identity_protocol': base['protocol_version']=2
   elif mode=='identity_session': base['session_id']='wrong-session-id'
   elif mode=='identity_model_version': base['model_version']='wrong-model-version'
   elif mode=='identity_operation': base['op']='warmup'
   elif mode=='identity_platform': base['platform_supported']=False
   elif mode=='identity_loaded_model_id': base['loaded_model_id']='wrong-loaded-model'
   elif mode=='identity_loaded_model_version': base['loaded_model_version']='wrong-loaded-version'
 with open(os.path.join(os.path.dirname(__file__),'requests.jsonl'),'a',encoding='utf-8') as f: f.write(json.dumps(req,separators=(',',':'))+'\n')
 with open(os.path.join(os.path.dirname(__file__),'responses.jsonl'),'a',encoding='utf-8') as f: f.write(json.dumps(base,separators=(',',':'))+'\n')
 sys.stdout.write(json.dumps(base,separators=(',',':'))+'\n'); sys.stdout.flush()
"#
            );
            std::fs::write(&worker_path, worker_source).expect("write owned fake protocol worker");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&worker_path, std::fs::Permissions::from_mode(0o600))
                    .expect("restrict fake worker source");
            }
            let mut runtime = Runtime::with_test_worker(
                root.join("models"),
                Manager::new(),
                PathBuf::from("python3"),
                vec!["-B".into(), worker_path.to_string_lossy().into_owned()],
            );
            let emitted = crate::mlx_diagnostics::EmitRecordCaptureForTest::default();
            runtime.test_emit_capture = Some(emitted.clone());
            let mut timer = crate::metrics::Metrics::default().stop_to_insert(
                crate::metrics::MetricGroup::unknown("task18_negative_runtime_case"),
                CancellationToken::new(),
            );
            let trace = timer.bind_trace(82);
            let result = emitted
                .with_scope_future(runtime.complete_with_source_history_and_prefix_traced(
                    vec![
                        ("system".to_owned(), "synthetic system".to_owned()),
                        ("user".to_owned(), "synthetic input".to_owned()),
                    ],
                    None,
                    None,
                    Some(trace.clone()),
                ))
                .await;
            let snapshot = trace.snapshot();
            timer.finish(crate::metrics::DeliveryOutcome::Failed, None);
            let reused_child_generation =
                if matches!(mode, "input_too_large" | "incomplete_completion") {
                    let mut second_timer = crate::metrics::Metrics::default().stop_to_insert(
                        crate::metrics::MetricGroup::unknown("task18_input_admission_reuse"),
                        CancellationToken::new(),
                    );
                    let second_trace = second_timer.bind_trace(83);
                    let second_result = emitted
                        .with_scope_future(runtime.complete_with_source_history_and_prefix_traced(
                            vec![
                                ("system".to_owned(), "synthetic system".to_owned()),
                                (
                                    "user".to_owned(),
                                    "synthetic input after admission".to_owned(),
                                ),
                            ],
                            None,
                            None,
                            Some(second_trace),
                        ))
                        .await;
                    second_timer.finish(crate::metrics::DeliveryOutcome::PasteConfirmed, None);
                    if mode == "input_too_large" {
                        Some(
                        second_result
                            .expect(
                                "loaded session remains reusable after input admission rejection",
                            )
                            .child_generation,
                    )
                    } else {
                        assert!(matches!(
                            second_result,
                            Err(RuntimeError::IncompleteGeneration)
                        ));
                        assert_eq!(runtime.child_sequence.load(Ordering::Relaxed), 1);
                        Some(runtime.child_sequence.load(Ordering::Relaxed))
                    }
                } else {
                    None
                };
            let restarted_child_generation = if mode.starts_with("identity_") {
                let mut second_timer = crate::metrics::Metrics::default().stop_to_insert(
                    crate::metrics::MetricGroup::unknown("task18_identity_child_restart"),
                    CancellationToken::new(),
                );
                let second_trace = second_timer.bind_trace(103);
                let second_result = emitted
                    .with_scope_future(runtime.complete_with_source_history_and_prefix_traced(
                        vec![
                            ("system".to_owned(), "synthetic system".to_owned()),
                            (
                                "user".to_owned(),
                                "synthetic input after identity retirement".to_owned(),
                            ),
                        ],
                        None,
                        None,
                        Some(second_trace),
                    ))
                    .await
                    .expect("valid response after identity rejection starts a fresh child");
                second_timer.finish(crate::metrics::DeliveryOutcome::PasteConfirmed, None);
                Some(second_result.child_generation)
            } else {
                None
            };
            let request_log =
                std::fs::read_to_string(root.join("requests.jsonl")).unwrap_or_default();
            stop_reap_and_remove(&runtime, &root).await;

            match (mode, &result) {
                ("load_reject", Err(RuntimeError::ModelLoadFailed))
                | ("identity_request_id", Err(RuntimeError::InvalidResponse))
                | ("identity_protocol", Err(RuntimeError::InvalidResponse))
                | ("identity_session", Err(RuntimeError::InvalidResponse))
                | ("identity_model_version", Err(RuntimeError::InvalidResponse))
                | ("identity_operation", Err(RuntimeError::InvalidResponse))
                | ("identity_platform", Err(RuntimeError::InvalidResponse))
                | ("identity_loaded_model_id", Err(RuntimeError::InvalidResponse))
                | ("identity_loaded_model_version", Err(RuntimeError::InvalidResponse))
                | ("incomplete_completion", Err(RuntimeError::IncompleteGeneration)) => {}
                (
                    "input_too_large",
                    Err(RuntimeError::InputTooLarge {
                        prompt_tokens: 8192,
                        requested_output_tokens: 1,
                        requested_total_tokens: Some(8193),
                        max_total_tokens: 8192,
                    }),
                ) => {}
                _ => panic!("{mode} returned an unexpected runtime result: {result:?}"),
            }
            let reached: Vec<_> = snapshot.stages.iter().map(|(stage, _, _)| *stage).collect();
            for stage in [
                crate::metrics::Stage::MlxGateWait,
                crate::metrics::Stage::RuntimeStartup,
                crate::metrics::Stage::FileValidation,
                crate::metrics::Stage::ModelLoad,
            ] {
                assert!(
                    reached.contains(&stage),
                    "{mode} must reach {stage:?}: {reached:?}"
                );
            }
            assert!(!reached.contains(&crate::metrics::Stage::FinalGuard));
            assert!(
                snapshot.reported_stages.is_empty(),
                "rejected work has no host Prefill/Decode/cache facts"
            );
            let requests: Vec<serde_json::Value> = request_log
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            assert!(requests
                .iter()
                .any(
                    |request| request.get("op").and_then(serde_json::Value::as_str) == Some("load")
                ));
            assert_eq!(
                requests.iter().any(|request| request
                    .get("op")
                    .and_then(serde_json::Value::as_str)
                    == Some("cleanup")),
                expect_cleanup_exchange
            );
            let captured: Vec<serde_json::Value> = emitted
                .snapshot()
                .iter()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            assert!(
                captured.iter().any(|event| {
                    event.get("event").and_then(serde_json::Value::as_str)
                        == Some("cleanup_model_file_hash")
                        && event
                            .get("request_id")
                            .and_then(serde_json::Value::as_str)
                            .is_some()
                        && event
                            .get("child_generation")
                            .and_then(serde_json::Value::as_u64)
                            .is_some()
                        && event
                            .get("session_generation")
                            .and_then(serde_json::Value::as_u64)
                            == Some(82)
                }),
                "real verifier hash events preserve request, child, and captured generation"
            );
            let rejected_request_id = captured
                .iter()
                .find(|event| {
                    event.get("event").and_then(serde_json::Value::as_str)
                        == Some("cleanup_request")
                        && event.get("phase").and_then(serde_json::Value::as_str) == Some("begin")
                        && event
                            .get("session_generation")
                            .and_then(serde_json::Value::as_u64)
                            == Some(82)
                })
                .and_then(|event| event.get("request_id"))
                .and_then(serde_json::Value::as_str);
            assert!(
                rejected_request_id.is_some(),
                "first request has an owned request ID"
            );
            let (expected_outcome, expected_stage) = match mode {
                "load_reject" => ("failed", "model_load"),
                "incomplete_completion" => ("failed", "generation"),
                "input_too_large" => ("rejected", "prompt_admission"),
                _ => ("failed", "protocol"),
            };
            assert!(
                captured.iter().any(|event| {
                    event.get("event").and_then(serde_json::Value::as_str)
                        == Some("cleanup_request")
                        && event.get("phase").and_then(serde_json::Value::as_str)
                            == Some("terminal")
                        && event.get("request_id").and_then(serde_json::Value::as_str)
                            == rejected_request_id
                        && event.get("outcome").and_then(serde_json::Value::as_str)
                            == Some(expected_outcome)
                        && event.get("stage").and_then(serde_json::Value::as_str)
                            == Some(expected_stage)
                }),
                "{mode} terminal event retains the expected result and stage"
            );
            assert!(
                !captured.iter().any(|event| {
                    event.get("event").and_then(serde_json::Value::as_str)
                        == Some("cleanup_generation")
                        && event.get("request_id").and_then(serde_json::Value::as_str)
                            == rejected_request_id
                }),
                "rejected request has no reported generation event"
            );
            if mode == "input_too_large" {
                assert!(
                    captured.iter().any(|event| {
                        event.get("event").and_then(serde_json::Value::as_str)
                            == Some("cleanup_admission")
                            && event
                                .get("session_generation")
                                .and_then(serde_json::Value::as_u64)
                                == Some(82)
                            && event.get("outcome").and_then(serde_json::Value::as_str)
                                == Some("rejected")
                            && event.get("reason").and_then(serde_json::Value::as_str)
                                == Some("input_too_large")
                    }),
                    "input admission event comes from the real runtime and retains generation 82; captured={captured:#?}"
                );
                let input_child_generation = captured
                    .iter()
                    .find(|event| {
                        event.get("event").and_then(serde_json::Value::as_str)
                            == Some("cleanup_model_inventory_ready")
                            && event
                                .get("session_generation")
                                .and_then(serde_json::Value::as_u64)
                                == Some(82)
                    })
                    .and_then(|event| event.get("child_generation"))
                    .and_then(serde_json::Value::as_u64);
                assert_eq!(
                    reused_child_generation, input_child_generation,
                    "a later valid request reuses the same child generation"
                );
            }
            if mode == "incomplete_completion" {
                let first_child_generation = captured
                    .iter()
                    .find(|event| {
                        event.get("event").and_then(serde_json::Value::as_str)
                            == Some("cleanup_model_inventory_ready")
                            && event
                                .get("session_generation")
                                .and_then(serde_json::Value::as_u64)
                                == Some(82)
                    })
                    .and_then(|event| event.get("child_generation"))
                    .and_then(serde_json::Value::as_u64);
                assert_eq!(reused_child_generation, first_child_generation);
                assert_eq!(
                    requests
                        .iter()
                        .filter(
                            |request| request.get("op").and_then(serde_json::Value::as_str)
                                == Some("cleanup")
                        )
                        .count(),
                    2,
                    "incomplete request and retry both traverse the actual child exchange"
                );
                assert_eq!(
                    requests
                        .iter()
                        .filter(
                            |request| request.get("op").and_then(serde_json::Value::as_str)
                                == Some("load")
                        )
                        .count(),
                    1,
                    "retry reuses the loaded model session"
                );
            }
            if mode.starts_with("identity_") {
                let events: Vec<serde_json::Value> = emitted
                    .snapshot()
                    .iter()
                    .filter_map(|line| serde_json::from_str(line).ok())
                    .collect();
                let first_child_generation = events
                    .iter()
                    .find(|event| {
                        event.get("event").and_then(serde_json::Value::as_str)
                            == Some("cleanup_model_inventory_ready")
                            && event
                                .get("session_generation")
                                .and_then(serde_json::Value::as_u64)
                                == Some(82)
                    })
                    .and_then(|event| event.get("child_generation"))
                    .and_then(serde_json::Value::as_u64);
                assert_ne!(
                    first_child_generation, restarted_child_generation,
                    "identity rejection retires child; next valid request uses a fresh generation"
                );
                let restarted_terminal = events.iter().find(|event| {
                    event.get("event").and_then(serde_json::Value::as_str)
                        == Some("cleanup_request")
                        && event.get("phase").and_then(serde_json::Value::as_str)
                            == Some("terminal")
                        && event
                            .get("session_generation")
                            .and_then(serde_json::Value::as_u64)
                            == Some(103)
                });
                assert!(
                    restarted_terminal.is_some(),
                    "restarted child completes a separately captured generation 103 request"
                );
                let statuses = runtime
                    .child_exit_statuses
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                assert_eq!(
                    statuses.len(),
                    2,
                    "both distinct child processes are reaped"
                );
                assert_eq!(
                    statuses
                        .iter()
                        .map(|(id, _, _)| *id)
                        .collect::<std::collections::BTreeSet<_>>()
                        .len(),
                    2,
                    "restart teardown records two unique child ids"
                );
                assert_eq!(statuses.len(), 2);
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn traced_cleanup_startup_warmup_records_bind_first_request_generation() {
        static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
        let suffix = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "voiceflow-task18-warmup-correlation-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create isolated fake-worker fixture");
        let worker_path = root.join("fake-worker.py");
        let worker_source = r#"import json, os, sys
for line in sys.stdin:
 req=json.loads(line)
 base={'protocol_version':1,'request_id':req['request_id'],'session_id':req.get('session_id'),'model_version':req.get('model_version'),'op':req['op'],'platform_supported':True}
 if req['op']=='hello': base.update(status='ready')
 elif req['op']=='load': base.update(status='loaded',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'])
 elif req['op']=='warmup':
  base.update(status='completed',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'],warmup_result={'status':'evaluated','prompt_tokens':5,'generation_tokens':1,'completion':'stop','info_observed':True,'tool_call_observed':False,'completion_text_seen':True,'prompt_evaluation_observed':True,'observed_prompt_tokens':5,'stream_drained':True,'evaluation_synchronized':True,'evaluation_time_seconds':0.01,'prompt_time_seconds':0.01,'generation_time_seconds':0.01,'prompt_cache_status':'bypassed','cached_prefix_tokens':0,'computed_suffix_tokens':5})
 elif req['op']=='cleanup':
  base.update(status='completed',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'],cleanup_result={'text':'synthetic completion','prompt_tokens':5,'generation_tokens':1,'prompt_time_seconds':0.01,'generation_time_seconds':0.01,'prompt_cache_status':'memory_hit','cached_prefix_tokens':5,'computed_suffix_tokens':0,'completion':'stop'})
 with open(os.path.join(os.path.dirname(__file__),'requests.jsonl'),'a',encoding='utf-8') as f: f.write(json.dumps(req,separators=(',',':'))+'\n')
 sys.stdout.write(json.dumps(base,separators=(',',':'))+'\n'); sys.stdout.flush()
"#;
        std::fs::write(&worker_path, worker_source).expect("write owned fake protocol worker");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&worker_path, std::fs::Permissions::from_mode(0o600))
                .expect("restrict fake worker source");
        }
        let mut runtime = Runtime::with_test_worker(
            root.join("models"),
            Manager::new(),
            PathBuf::from("python3"),
            vec!["-B".into(), worker_path.to_string_lossy().into_owned()],
        );
        let capture = crate::mlx_diagnostics::EmitRecordCaptureForTest::default();
        runtime.test_emit_capture = Some(capture.clone());
        let warmup = capture
            .with_scope_future(runtime.prewarm_startup(
                vec![
                    ("system".to_owned(), "synthetic warmup system".to_owned()),
                    ("user".to_owned(), "synthetic warmup input".to_owned()),
                ],
                None,
                "task18_synthetic",
                Arc::new(|| true),
            ))
            .await
            .expect("valid owned fake worker warmup completes");
        let registration = runtime
            .session
            .lock()
            .await
            .as_ref()
            .expect("warmup retains its child session")
            .process
            .registration
            .clone();
        let request_counts = || {
            let requests = std::fs::read_to_string(root.join("requests.jsonl"))
                .expect("read owned fake-worker requests");
            let operations: Vec<_> = requests
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .filter_map(|request| {
                    request
                        .get("op")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .collect();
            (
                operations.iter().filter(|op| *op == "load").count(),
                operations.iter().filter(|op| *op == "warmup").count(),
                operations.iter().filter(|op| *op == "unload").count(),
            )
        };
        for (pressure, expected_reason) in [
            (
                crate::model_resources::HostMemoryPressure::Warning,
                crate::model_resources::MemoryUnavailableReason::PressureNotNormal,
            ),
            (
                crate::model_resources::HostMemoryPressure::Unknown,
                crate::model_resources::MemoryUnavailableReason::UnknownPressure,
            ),
        ] {
            registration.set_test_sampler(crate::model_resources::FixedMemorySampler {
                pressure,
                physical_memory: Some(64 * 1024 * 1024 * 1024),
                footprint: crate::model_resources::FootprintSample::Observed(0),
            });
            let skipped = runtime
                .prewarm_startup(
                    vec![
                        ("system".to_owned(), "synthetic warmup system".to_owned()),
                        ("user".to_owned(), "synthetic warmup input".to_owned()),
                    ],
                    None,
                    "task18_synthetic",
                    Arc::new(|| true),
                )
                .await
                .expect_err("non-normal pressure skips startup EVAL");
            assert!(matches!(
                skipped,
                RuntimeError::MemoryUnavailable(reason) if reason == expected_reason
            ));
            {
                let session = runtime.session.lock().await;
                let child = session
                    .as_ref()
                    .expect("non-normal pressure must retain the loaded child");
                assert!(
                    child.model.is_some(),
                    "non-normal pressure must retain resident model"
                );
            }
            assert_eq!(request_counts(), (1, 1, 0));
        }
        registration.set_test_sampler(crate::model_resources::FixedMemorySampler {
            pressure: crate::model_resources::HostMemoryPressure::Normal,
            physical_memory: Some(64 * 1024 * 1024 * 1024),
            footprint: crate::model_resources::FootprintSample::Observed(0),
        });
        let mut timer = crate::metrics::Metrics::default().stop_to_insert(
            crate::metrics::MetricGroup::unknown("task18_warmup_first_cleanup"),
            CancellationToken::new(),
        );
        let trace = timer.bind_trace(107);
        let completion = capture
            .with_scope_future(runtime.complete_with_source_history_and_prefix_traced(
                vec![
                    ("system".to_owned(), "synthetic system".to_owned()),
                    ("user".to_owned(), "synthetic input".to_owned()),
                ],
                None,
                None,
                Some(trace),
            ))
            .await
            .expect("first traced cleanup after startup warmup succeeds");
        assert!(
            completion.model_was_resident,
            "cleanup reuses the retained resident model"
        );
        assert_eq!(request_counts(), (1, 1, 0));
        timer.finish(crate::metrics::DeliveryOutcome::PasteConfirmed, None);
        stop_reap_and_remove(&runtime, &root).await;

        let events: Vec<serde_json::Value> = capture
            .snapshot()
            .iter()
            .filter_map(|line| serde_json::from_str(line).ok())
            .filter(|event: &serde_json::Value| {
                event.get("event").and_then(serde_json::Value::as_str) == Some("startup_warmup")
            })
            .collect();
        let standalone: Vec<_> = events
            .iter()
            .filter(|event| {
                event.get("request_id").and_then(serde_json::Value::as_str)
                    == Some(warmup.request_id.as_str())
            })
            .collect();
        assert!(
            standalone.len() >= 2,
            "prewarm emits actual begin and terminal records"
        );
        assert!(
            standalone.iter().all(|event| {
                event
                    .get("session_generation")
                    .and_then(serde_json::Value::as_u64)
                    .is_none()
            }),
            "standalone startup warmup events omit captured dictation generation"
        );
        let first_request: Vec<_> = events
            .iter()
            .filter(|event| {
                event.get("request_id").and_then(serde_json::Value::as_str)
                    == Some(completion.request_id.as_str())
                    && event.get("phase").and_then(serde_json::Value::as_str) == Some("terminal")
            })
            .collect();
        assert_eq!(
            first_request.len(),
            1,
            "one first-request warmup terminal is correlated"
        );
        let event = first_request[0];
        assert_eq!(
            event
                .get("session_generation")
                .and_then(serde_json::Value::as_u64),
            Some(107)
        );
        assert_eq!(
            event
                .get("child_generation")
                .and_then(serde_json::Value::as_u64),
            Some(completion.child_generation)
        );
        assert_eq!(
            event.get("outcome").and_then(serde_json::Value::as_str),
            Some("reused")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dictation_trace_has_all_ten_stages_monotonic() {
        const SYNTHETIC_ASR_TEXT: &str = "synthetic transcript";
        static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
        let suffix = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "voiceflow-task18-runtime-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create owned runtime fixture directory");
        let worker_path = root.join("fake-worker.py");
        let worker_source = "import json, os, sys\ncleanup_count=0\nfor line in sys.stdin:\n req=json.loads(line)\n base={'protocol_version':1,'request_id':req['request_id'],'session_id':req.get('session_id'),'model_version':req.get('model_version'),'op':req['op'],'platform_supported':True}\n if req['op']=='hello': base.update(status='ready')\n elif req['op']=='load':\n  if not os.path.isdir(req.get('model_directory','')): base.update(status='error')\n  else: base.update(status='loaded',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'])\n elif req['op']=='cleanup':\n  cleanup_count += 1\n  cache_status='memory_hit' if cleanup_count < 3 else 'future_cache'\n  base.update(status='completed',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'],cleanup_result={'text':'synthetic cleaned text','prompt_tokens':5,'generation_tokens':2,'prompt_time_seconds':0.015,'generation_time_seconds':0.025,'prompt_time_scope':'future_scope','prompt_cache_status':cache_status,'prepare_time_seconds':-0.01,'prompt_cache_lookup_seconds':-0.02,'completion':'stop'})\n else: base.update(status='error')\n if req['op']=='cleanup' and cleanup_count==3: base['request_id']='synthetic-mismatched-request'\n fields=('protocol_version','request_id','session_id','model_version','op')\n identity_echo=all(base.get(key)==req.get(key) for key in fields)\n cleanup=base.get('cleanup_result',{})\n result_facts=all((key in cleanup) for key in ('text','prompt_tokens','generation_tokens','prompt_time_seconds','generation_time_seconds','completion'))\n encoded=json.dumps(base,separators=(',',':'))\n with open(os.path.join(os.path.dirname(__file__),'events.txt'),'a',encoding='utf-8') as events: events.write(req['op']+'|'+str(base.get('status'))+'|identity_echo='+str(identity_echo)+'|platform='+str(base.get('platform_supported'))+'|result='+str(result_facts)+'|completion='+str(cleanup.get('completion'))+'\\n')\n with open(os.path.join(os.path.dirname(__file__),'frames.jsonl'),'a',encoding='utf-8') as frames: frames.write(encoded+'\\n')\n sys.stdout.write(encoded+'\\n'); sys.stdout.flush()".to_owned();
        std::fs::write(&worker_path, worker_source).expect("write owned fake worker");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&worker_path, std::fs::Permissions::from_mode(0o600))
                .expect("restrict fake worker source");
        }
        let mut runtime = Runtime::with_test_worker(
            root.join("models"),
            Manager::new(),
            PathBuf::from("python3"),
            vec!["-B".into(), worker_path.to_string_lossy().into_owned()],
        );
        let emitted_records = crate::mlx_diagnostics::EmitRecordCaptureForTest::default();
        runtime.test_emit_capture = Some(emitted_records.clone());
        let mut timer = crate::metrics::Metrics::default().stop_to_insert(
            crate::metrics::MetricGroup::unknown("runtime_trace_test"),
            CancellationToken::new(),
        );
        let trace = timer.bind_trace(73);
        let asr_options = crate::asr::AsrOptions::default();
        let asr_scope = crate::queue::RequestScope::new("fake_asr", None, "fake", "");
        let asr_gate = crate::queue::RequestGate::new(None);
        let asr_result = crate::execute_reprojected_asr(
            &asr_gate,
            asr_scope.clone(),
            vec![0, 1, 2],
            CancellationToken::new(),
            crate::AsrTraceContext {
                trace: Some(trace.clone()),
                provider_inference: true,
            },
            move || {
                Some(crate::ContextualAsrAttempt {
                    request_identity: crate::prefetch_asr::PrefetchRequestIdentity::new(
                        "fake_asr",
                        None,
                        &asr_options,
                    ),
                    quota_scope: asr_scope.clone(),
                    options: asr_options.clone(),
                    context_source: None,
                })
            },
            |_, _| {
                Box::pin(async {
                    Ok(crate::asr::Transcript {
                        text: SYNTHETIC_ASR_TEXT.to_owned(),
                        asr_text: Some(SYNTHETIC_ASR_TEXT.to_owned()),
                        provider_cleaned_candidate: None,
                        language: Some("en".to_owned()),
                        confidence: Some(0.9),
                        segments: Vec::new(),
                        words: Vec::new(),
                        tokens: Vec::new(),
                        limits: crate::asr::RateLimits::default(),
                    })
                })
            },
        )
        .await;
        let asr_transcript = asr_result
            .expect("the production ASR owner returns the fake transcript")
            .0
            .text;
        let messages = vec![
            ("system".to_owned(), "synthetic system prompt".to_owned()),
            ("user".to_owned(), asr_transcript.clone()),
        ];

        let llm_gate = crate::queue::RequestGate::new(None);
        let llm_scope = crate::queue::RequestScope::new(
            "task18_fake_llm",
            None,
            crate::providers::EngineProvider::OnDeviceMlx.default_llm_model(),
            "synthetic",
        );
        crate::mlx_diagnostics::panic_on_events_for_test(&[
            crate::mlx_diagnostics::EventName::CleanupQueue,
            crate::mlx_diagnostics::EventName::CleanupGuard,
        ]);
        crate::mlx_diagnostics::panic_on_event_phase_for_test(
            crate::mlx_diagnostics::EventName::CleanupRequest,
            "terminal",
        );
        let llm_result = emitted_records
            .with_scope_future(crate::queue::execute_with_retry_scoped_cancelled_traced(
                &llm_gate,
                crate::queue::RequestKind::Llm,
                &llm_scope,
                || async {
                    crate::llm::cleanup_with_on_device_mlx_runtime_and_intent_and_source(
                        &runtime,
                        crate::providers::EngineProvider::OnDeviceMlx.default_llm_model(),
                        &asr_transcript,
                        &[],
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        crate::llm::CleanupEffort::Standard,
                        None,
                        None,
                        None,
                        Some(&trace),
                    )
                    .await
                },
                CancellationToken::new(),
                Some(trace.clone()),
            ))
            .await;
        let (accepted, _) = match llm_result {
            Ok(accepted) => accepted,
            Err(error) => {
                let events = std::fs::read_to_string(root.join("events.txt"))
                    .unwrap_or_else(|_| "events_missing".to_owned());
                stop_reap_and_remove(&runtime, &root).await;
                panic!("production prompt, queue, runtime, and guard failed: {error:?}; events={events}");
            }
        };
        let cold_snapshot = trace.snapshot();
        let mut warm_timer = crate::metrics::Metrics::default().stop_to_insert(
            crate::metrics::MetricGroup::unknown("runtime_warm_trace_test"),
            CancellationToken::new(),
        );
        let warm_trace = warm_timer.bind_trace(74);
        crate::mlx_diagnostics::panic_on_next_emit_for_test();
        let warm = emitted_records
            .with_scope_future(runtime.complete_with_source_history_and_prefix_traced(
                messages.clone(),
                None,
                None,
                Some(warm_trace.clone()),
            ))
            .await
            .expect("separate warm request reuses the loaded child");
        warm_timer.finish(crate::metrics::DeliveryOutcome::PasteConfirmed, None);
        let warm_snapshot = warm_trace.snapshot();
        let accepted_terminal_panicked_records = emitted_records
            .panicked_attempts()
            .iter()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|event| {
                event.get("event").and_then(serde_json::Value::as_str) == Some("cleanup_request")
                    && event.get("phase").and_then(serde_json::Value::as_str) == Some("terminal")
                    && event.get("outcome").and_then(serde_json::Value::as_str) == Some("completed")
            })
            .collect::<Vec<_>>();
        assert_eq!(accepted_terminal_panicked_records.len(), 1);
        let accepted_terminal_id = accepted_terminal_panicked_records[0]
            .get("request_id")
            .and_then(serde_json::Value::as_str);
        let accepted_terminal_attempts = emitted_records
            .snapshot()
            .into_iter()
            .chain(emitted_records.panicked_attempts())
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
            .filter(|event| {
                event.get("event").and_then(serde_json::Value::as_str) == Some("cleanup_request")
                    && event.get("phase").and_then(serde_json::Value::as_str) == Some("terminal")
                    && event.get("request_id").and_then(serde_json::Value::as_str)
                        == accepted_terminal_id
            })
            .count();
        assert_eq!(accepted_terminal_attempts, 1);
        crate::mlx_diagnostics::panic_on_event_for_test(
            crate::mlx_diagnostics::EventName::CleanupRequest,
        );
        crate::mlx_diagnostics::panic_on_event_phase_for_test(
            crate::mlx_diagnostics::EventName::CleanupRequest,
            "terminal",
        );
        let identity_rejection = emitted_records
            .with_scope_future(runtime.complete_with_source_history_and_prefix_traced(
                vec![
                    ("system".to_owned(), "synthetic system prompt".to_owned()),
                    ("user".to_owned(), "synthetic transcript".to_owned()),
                ],
                None,
                None,
                Some(trace.clone()),
            ))
            .await;
        let scene = crate::ResolvedCleanupScene::observed(
            crate::context::ContextFamily::General,
            crate::context::FocusKind::Unknown,
        );
        let final_context = crate::FinalizationContext {
            scene,
            operation: crate::llm::CleanupOperation::Cleanup,
            revision_source: None,
            prepared_transcript: None,
            revision_authorizations: &[],
            promoted_pair_protections: &[],
            list_revision_proof: None,
            structure_safe_source: None,
            output_projection_list_revision_proof: None,
            layout_source: None,
            exact_fallback: false,
        };
        let short_finalized = crate::with_traced_final_output(Some(&trace), || {
            crate::finalize_text_for_scene(
                &asr_transcript,
                crate::CleanupDecision::Provider(accepted.accepted_text.clone()),
                final_context,
            )
        })
        .expect("short scene finalizer accepts safe cleanup output");
        let long_finalized = crate::with_traced_final_output(Some(&trace), || {
            crate::guard_final_output_with_reason(
                &asr_transcript,
                &accepted.accepted_text,
                final_context,
            )
        });
        assert_eq!(short_finalized.text, accepted.accepted_text);
        assert!(!short_finalized.degraded);
        assert!(short_finalized.cleanup_rejection.is_none());
        assert_eq!(long_finalized.0, accepted.accepted_text);
        assert!(long_finalized.1.is_none());
        let fallback_short = crate::finalize_text_for_scene(
            "I do not approve",
            crate::CleanupDecision::Provider("I approve".to_owned()),
            final_context,
        )
        .expect("short finalizer preserves the ASR source after guard rejection");
        assert_eq!(fallback_short.text, "I do not approve");
        assert!(fallback_short.degraded);
        let rejection = fallback_short
            .cleanup_rejection
            .expect("short finalizer records the actual final-output rejection");
        assert_eq!(rejection.reason, crate::llm::GuardReason::ProtectedToken);
        assert_eq!(
            rejection.stage,
            crate::llm::CleanupRejectionStage::FinalOutput
        );
        let fallback_long =
            crate::guard_final_output_with_reason("I do not approve", "I approve", final_context);
        assert_eq!(fallback_long.0, "I do not approve");
        assert_eq!(
            fallback_long.1,
            Some(crate::llm::GuardReason::ProtectedToken)
        );

        let cancellation = CancellationToken::new();
        let expected_after = format!("before {}", long_finalized.0);
        let short_expected_after = format!("before {}", short_finalized.text);
        let (short_attempt, _) =
            crate::paste::run_traced_boundary(Some(&trace), crate::metrics::Stage::Paste, || {
                crate::paste::run_paste_attempt(
                    &cancellation,
                    || Ok(()),
                    || Ok(()),
                    |mark| {
                        mark();
                        Ok(())
                    },
                )
            });
        let (short_readback, _) =
            crate::paste::run_traced_confirmation(Some(&trace), short_attempt.result, || {
                Ok::<_, crate::paste::PasteError>(crate::paste::confirm_keyboard_paste_readback(
                    crate::paste::PasteReadbackExpectation {
                        expected_after: Some(&short_expected_after),
                        value_before: Some("before"),
                        insertion_end_caret_utf16: None,
                    },
                    &cancellation,
                    || (true, true),
                    || Some(short_expected_after.clone()),
                    |_, _| crate::paste::FullRangeReadback::Unavailable,
                    |_| {},
                ))
            });
        let short_readback = short_readback.expect("short route reaches actual proof readback");
        let short_verification = crate::paste::classify_confirmed_paste_readback(
            !cancellation.is_cancelled(),
            short_readback.proof,
            Some(&short_expected_after),
            Some("before"),
            short_readback.value_after.as_deref(),
            true,
            true,
        );
        let short_outcome = crate::paste::build_insert_outcome(
            true,
            short_readback.value_after,
            short_verification,
        );
        let short_delivery = crate::delivery_result_for_paste(short_outcome.verified, None);
        assert!(short_delivery.verified);
        assert_eq!(
            short_delivery.method,
            crate::delivery::DeliveryMethod::Paste
        );
        let prove_nonempty_fallback = |text: &str| {
            let expected = format!("before {text}");
            let cancellation = CancellationToken::new();
            let (attempt, _) = crate::paste::run_traced_boundary(
                Some(&trace),
                crate::metrics::Stage::Paste,
                || {
                    crate::paste::run_paste_attempt(
                        &cancellation,
                        || Ok(()),
                        || Ok(()),
                        |mark| {
                            mark();
                            Ok(())
                        },
                    )
                },
            );
            let (readback, _) =
                crate::paste::run_traced_confirmation(Some(&trace), attempt.result, || {
                    Ok::<_, crate::paste::PasteError>(
                        crate::paste::confirm_keyboard_paste_readback(
                            crate::paste::PasteReadbackExpectation {
                                expected_after: Some(&expected),
                                value_before: Some("before"),
                                insertion_end_caret_utf16: None,
                            },
                            &cancellation,
                            || (true, true),
                            || Some(expected.clone()),
                            |_, _| crate::paste::FullRangeReadback::Unavailable,
                            |_| {},
                        ),
                    )
                });
            let readback = readback.expect("nonempty source fallback reaches readback");
            let verification = crate::paste::classify_confirmed_paste_readback(
                !cancellation.is_cancelled(),
                readback.proof,
                Some(&expected),
                Some("before"),
                readback.value_after.as_deref(),
                true,
                true,
            );
            let outcome =
                crate::paste::build_insert_outcome(true, readback.value_after, verification);
            crate::delivery_result_for_paste(outcome.verified, None)
        };
        let fallback_short_delivery = prove_nonempty_fallback(&fallback_short.text);
        let fallback_long_delivery = prove_nonempty_fallback(&fallback_long.0);
        assert!(fallback_short_delivery.verified && fallback_long_delivery.verified);
        assert_eq!(
            fallback_short_delivery.method,
            crate::delivery::DeliveryMethod::Paste
        );
        assert_eq!(
            fallback_long_delivery.method,
            crate::delivery::DeliveryMethod::Paste
        );
        let mut authorization_callback_reached = false;
        let mut authorization_submit_reached = false;
        let (authorization_attempt, _) =
            crate::paste::run_traced_boundary(Some(&trace), crate::metrics::Stage::Paste, || {
                crate::paste::run_paste_attempt(
                    &cancellation,
                    || Err(crate::paste::PasteError::SecureInput),
                    || Ok(()),
                    |mark| {
                        authorization_submit_reached = true;
                        mark();
                        Ok(())
                    },
                )
            });
        let (authorization_readback, _) = crate::paste::run_traced_confirmation(
            Some(&trace),
            authorization_attempt.result,
            || {
                authorization_callback_reached = true;
                Ok::<_, crate::paste::PasteError>(())
            },
        );
        assert!(authorization_readback.is_err());
        assert!(!authorization_callback_reached && !authorization_submit_reached);
        let (attempt, _) =
            crate::paste::run_traced_boundary(Some(&trace), crate::metrics::Stage::Paste, || {
                crate::paste::run_paste_attempt(
                    &cancellation,
                    || Ok(()),
                    || Ok(()),
                    |mark| {
                        mark();
                        Ok(())
                    },
                )
            });
        let (readback, _) =
            crate::paste::run_traced_confirmation(Some(&trace), attempt.result, || {
                Ok::<_, crate::paste::PasteError>(crate::paste::confirm_keyboard_paste_readback(
                    crate::paste::PasteReadbackExpectation {
                        expected_after: Some(&expected_after),
                        value_before: Some("before"),
                        insertion_end_caret_utf16: None,
                    },
                    &cancellation,
                    || (true, true),
                    || Some(expected_after.clone()),
                    |_, _| crate::paste::FullRangeReadback::Unavailable,
                    |_| {},
                ))
            });
        let readback = readback.expect("actual readback operation was reached");
        let verification = crate::paste::classify_confirmed_paste_readback(
            !cancellation.is_cancelled(),
            readback.proof,
            Some(&expected_after),
            Some("before"),
            readback.value_after.as_deref(),
            true,
            true,
        );
        let outcome = crate::paste::build_insert_outcome(true, readback.value_after, verification);
        let delivered = crate::delivery_result_for_paste(outcome.verified, None);
        assert!(delivered.verified);
        assert_eq!(delivered.method, crate::delivery::DeliveryMethod::Paste);

        let (unconfirmed_attempt, _) =
            crate::paste::run_traced_boundary(Some(&trace), crate::metrics::Stage::Paste, || {
                crate::paste::run_paste_attempt(
                    &cancellation,
                    || Ok(()),
                    || Ok(()),
                    |mark| {
                        mark();
                        Ok(())
                    },
                )
            });
        let (unconfirmed_readback, _) =
            crate::paste::run_traced_confirmation(Some(&trace), unconfirmed_attempt.result, || {
                Ok::<_, crate::paste::PasteError>(crate::paste::confirm_keyboard_paste_readback(
                    crate::paste::PasteReadbackExpectation {
                        expected_after: Some(&expected_after),
                        value_before: Some("before"),
                        insertion_end_caret_utf16: None,
                    },
                    &cancellation,
                    || (false, true),
                    || Some(expected_after.clone()),
                    |_, _| crate::paste::FullRangeReadback::Unavailable,
                    |_| {},
                ))
            });
        let unconfirmed_readback = unconfirmed_readback.expect("readback callback ran");
        let unconfirmed_verification = crate::paste::classify_confirmed_paste_readback(
            !cancellation.is_cancelled(),
            unconfirmed_readback.proof,
            Some(&expected_after),
            Some("before"),
            unconfirmed_readback.value_after.as_deref(),
            false,
            true,
        );
        let unconfirmed_outcome = crate::paste::build_insert_outcome(
            true,
            unconfirmed_readback.value_after,
            unconfirmed_verification,
        );
        let unconfirmed_delivery =
            crate::delivery_result_for_paste(unconfirmed_outcome.verified, None);
        assert!(!unconfirmed_delivery.verified);
        assert_eq!(
            unconfirmed_delivery.method,
            crate::delivery::DeliveryMethod::PasteUnverified
        );
        let elapsed_ms = u64::try_from(timer.elapsed().as_millis()).unwrap_or(u64::MAX);
        let terminal_outcome = if unconfirmed_delivery.verified {
            crate::metrics::DeliveryOutcome::PasteConfirmed
        } else {
            crate::metrics::DeliveryOutcome::PasteUnconfirmed
        };
        assert_eq!(
            terminal_outcome,
            crate::metrics::DeliveryOutcome::PasteUnconfirmed
        );
        timer.finish(terminal_outcome, None);
        let snapshot = trace.snapshot();
        let protocol_events = std::fs::read_to_string(root.join("events.txt"))
            .unwrap_or_else(|_| "events_missing".to_owned());
        stop_reap_and_remove(&runtime, &root).await;

        let captured_events: Vec<serde_json::Value> = emitted_records
            .snapshot()
            .iter()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let panicked_events: Vec<serde_json::Value> = emitted_records
            .panicked_attempts()
            .iter()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        assert!(
            panicked_events.iter().any(|event| {
                event.get("event").and_then(serde_json::Value::as_str) == Some("cleanup_guard")
                    && event.get("outcome").and_then(serde_json::Value::as_str) == Some("accepted")
                    && event
                        .get("session_generation")
                        .and_then(serde_json::Value::as_u64)
                        == Some(73)
            }),
            "selected accepted-cleanup guard diagnostic panic is captured as an attempted record"
        );
        assert!(
            panicked_events.iter().any(|event| {
                event.get("event").and_then(serde_json::Value::as_str) == Some("cleanup_queue")
                    && event.get("outcome").and_then(serde_json::Value::as_str) == Some("acquired")
                    && event
                        .get("session_generation")
                        .and_then(serde_json::Value::as_u64)
                        == Some(73)
            }),
            "selected accepted-cleanup queue diagnostic panic is captured; panicked={panicked_events:#?}"
        );
        let rejected_begin = panicked_events
            .iter()
            .find(|event| {
                event.get("event").and_then(serde_json::Value::as_str) == Some("cleanup_request")
                    && event.get("phase").and_then(serde_json::Value::as_str) == Some("begin")
                    && event
                        .get("session_generation")
                        .and_then(serde_json::Value::as_u64)
                        == Some(73)
            })
            .and_then(|event| event.get("request_id"))
            .and_then(serde_json::Value::as_str);
        assert!(
            rejected_begin.is_some(),
            "selected rejected-request begin event is recorded as panicked"
        );
        let terminal_attempts = captured_events
            .iter()
            .chain(panicked_events.iter())
            .filter(|event| {
                event.get("event").and_then(serde_json::Value::as_str) == Some("cleanup_request")
                    && event.get("phase").and_then(serde_json::Value::as_str) == Some("terminal")
                    && event.get("request_id").and_then(serde_json::Value::as_str) == rejected_begin
            })
            .count();
        assert_eq!(terminal_attempts, 1, "identity-rejected request has exactly one terminal attempt across delivered and panicked captures");
        assert!(
            captured_events.iter().any(|event| {
                event.get("event").and_then(serde_json::Value::as_str)
                    == Some("cleanup_model_file_hash")
                    && event
                        .get("session_generation")
                        .and_then(serde_json::Value::as_u64)
                        == Some(73)
            }),
            "production file-hash emitters retain the captured dictation generation"
        );

        let cold_stages = cold_snapshot
            .stages
            .iter()
            .map(|(stage, _, _)| *stage)
            .collect::<Vec<_>>();
        assert!(cold_stages.contains(&crate::metrics::Stage::RuntimeStartup));
        assert!(cold_stages.contains(&crate::metrics::Stage::FileValidation));
        assert!(cold_stages.contains(&crate::metrics::Stage::ModelLoad));
        assert!(cold_stages.contains(&crate::metrics::Stage::QueueWait));
        assert!(cold_stages.contains(&crate::metrics::Stage::PromptPreparation));
        let cold_position = |stage| {
            cold_stages
                .iter()
                .position(|reached| *reached == stage)
                .expect("ordered cold pipeline stage is present")
        };
        let cold_cleanup_queue_position = cold_stages
            .iter()
            .rposition(|stage| *stage == crate::metrics::Stage::QueueWait)
            .expect("actual LLM queue wait is present after ASR");
        assert!(cold_position(crate::metrics::Stage::AsrInference) < cold_cleanup_queue_position);
        assert!(
            cold_cleanup_queue_position < cold_position(crate::metrics::Stage::PromptPreparation)
        );
        assert!(
            cold_position(crate::metrics::Stage::PromptPreparation)
                < cold_position(crate::metrics::Stage::MlxGateWait)
        );
        assert!(
            cold_position(crate::metrics::Stage::MlxGateWait)
                < cold_position(crate::metrics::Stage::RuntimeStartup)
        );
        assert!(
            cold_position(crate::metrics::Stage::RuntimeStartup)
                < cold_position(crate::metrics::Stage::FileValidation)
        );
        assert!(
            cold_position(crate::metrics::Stage::FileValidation)
                < cold_position(crate::metrics::Stage::ModelLoad)
        );
        assert_eq!(
            cold_stages
                .iter()
                .filter(|stage| **stage == crate::metrics::Stage::FinalGuard)
                .count(),
            1,
            "cold trace contains only the actual LLM acceptance guard"
        );
        assert_eq!(cold_snapshot.session_generation, 73);
        let warm_stages = warm_snapshot
            .stages
            .iter()
            .map(|(stage, _, _)| *stage)
            .collect::<Vec<_>>();
        for omitted in [
            crate::metrics::Stage::RuntimeStartup,
            crate::metrics::Stage::FileValidation,
            crate::metrics::Stage::ModelLoad,
        ] {
            assert!(
                !warm_stages.contains(&omitted),
                "warm trace omits {omitted:?}"
            );
        }
        assert_eq!(warm_snapshot.session_generation, 74);
        assert!(warm.model_was_resident);
        assert!(warm.model_load_seconds.is_none());
        assert_eq!(accepted.accepted_text, "synthetic cleaned text");
        assert!(matches!(
            identity_rejection,
            Err(RuntimeError::InvalidResponse)
        ));
        assert!(protocol_events
            .lines()
            .any(|line| line.contains("identity_echo=False")));
        assert_eq!(snapshot.session_generation, 73);
        let required_stages = [
            crate::metrics::Stage::AsrInference,
            crate::metrics::Stage::QueueWait,
            crate::metrics::Stage::MlxGateWait,
            crate::metrics::Stage::RuntimeStartup,
            crate::metrics::Stage::FileValidation,
            crate::metrics::Stage::ModelLoad,
            crate::metrics::Stage::PromptPreparation,
            crate::metrics::Stage::FinalGuard,
            crate::metrics::Stage::Paste,
            crate::metrics::Stage::TargetConfirmation,
        ];
        for required in required_stages {
            assert!(
                snapshot
                    .stages
                    .iter()
                    .any(|(stage, _, _)| *stage == required),
                "required production stage {required:?} was not reached"
            );
        }
        let stage_order = snapshot
            .stages
            .iter()
            .map(|(stage, _, _)| *stage)
            .collect::<Vec<_>>();
        let first_position = |stage| {
            stage_order
                .iter()
                .position(|reached| *reached == stage)
                .expect("required stage is present")
        };
        let guard_positions = stage_order
            .iter()
            .enumerate()
            .filter_map(|(index, stage)| {
                (*stage == crate::metrics::Stage::FinalGuard).then_some(index)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            guard_positions.len(),
            3,
            "one reached guard belongs to LLM acceptance, short finalization, and long finalization"
        );
        let cleanup_queue_position = stage_order
            .iter()
            .rposition(|stage| *stage == crate::metrics::Stage::QueueWait)
            .expect("cleanup reaches the real queue owner");
        assert!(first_position(crate::metrics::Stage::AsrInference) < cleanup_queue_position);
        assert!(cleanup_queue_position < first_position(crate::metrics::Stage::PromptPreparation));
        assert!(first_position(crate::metrics::Stage::PromptPreparation) < guard_positions[0]);
        assert!(guard_positions[0] < guard_positions[1]);
        assert!(guard_positions[1] < guard_positions[2]);
        assert!(guard_positions[2] < first_position(crate::metrics::Stage::Paste));
        assert_eq!(
            stage_order
                .iter()
                .filter(|stage| **stage == crate::metrics::Stage::Paste)
                .count(),
            6,
            "accepted routes, semantic fallbacks, authorization rejection, and unconfirmed route reach paste boundary"
        );
        assert_eq!(
            stage_order
                .iter()
                .filter(|stage| **stage == crate::metrics::Stage::TargetConfirmation)
                .count(),
            5,
            "each submitted accepted/fallback output reaches actual confirmation; authorization rejection does not"
        );
        for (_, offset_ms, duration_ms) in &snapshot.stages {
            assert!(offset_ms.saturating_add(*duration_ms) <= elapsed_ms);
        }
        assert!(
            snapshot
                .stages
                .windows(2)
                .all(|pair| { pair[0].1.saturating_add(pair[0].2) <= pair[1].1 }),
            "host intervals must be disjoint and ordered"
        );
        let disjoint_host_duration_ms = snapshot
            .stages
            .iter()
            .map(|(_, _, duration_ms)| *duration_ms)
            .fold(0_u64, u64::saturating_add);
        assert!(disjoint_host_duration_ms <= elapsed_ms);
        assert!(snapshot
            .stages
            .iter()
            .any(|(stage, _, _)| *stage == crate::metrics::Stage::MlxGateWait));
        assert_eq!(
            snapshot
                .stages
                .iter()
                .filter(|(stage, _, _)| *stage == crate::metrics::Stage::RuntimeStartup)
                .count(),
            1,
            "only the cold request spawns and handshakes a child"
        );
        assert!(snapshot
            .stages
            .iter()
            .any(|(stage, _, _)| *stage == crate::metrics::Stage::FileValidation));
        assert!(snapshot
            .stages
            .iter()
            .any(|(stage, _, _)| *stage == crate::metrics::Stage::ModelLoad));
        assert!(snapshot
            .stages
            .iter()
            .any(|(stage, _, _)| { *stage == crate::metrics::Stage::PromptPreparation }));
        assert!(snapshot
            .stages
            .iter()
            .any(|(stage, _, _)| { *stage == crate::metrics::Stage::QueueWait }));
        assert!(snapshot
            .stages
            .iter()
            .any(|(stage, _, _)| { *stage == crate::metrics::Stage::FinalGuard }));
        assert!(!snapshot.stages.iter().any(|(stage, _, _)| {
            matches!(
                *stage,
                crate::metrics::Stage::Prefill | crate::metrics::Stage::Decode
            )
        }));
        assert_eq!(snapshot.reported_stages.len(), 2);
        assert!(snapshot.reported_stages.iter().any(|fact| {
            fact.stage == crate::metrics::Stage::Prefill
                && fact.scope == crate::metrics::ReportedScope::Unknown
                && fact.duration_ms == 15
        }));
        assert!(snapshot.reported_stages.iter().any(|fact| {
            fact.stage == crate::metrics::Stage::Decode
                && fact.scope == crate::metrics::ReportedScope::Generation
                && fact.duration_ms == 25
        }));
        assert_eq!(warm.prompt_cache_status.as_deref(), Some("memory_hit"));
        assert_eq!(
            snapshot.cache_status,
            Some(crate::metrics::CacheStatus::MemoryHit)
        );
    }

    #[tokio::test]
    async fn traced_mlx_gate_drop_records_only_reached_boundary() {
        use std::future::Future;
        use std::task::{Context, Poll};
        let cleanup_gate = Arc::clone(cleanup_operation_gate());
        let held = cleanup_gate.lock_owned().await;
        let mut runtime = Runtime::with_test_worker(
            std::env::temp_dir().join("voiceflow-no-model-fixture"),
            Manager::new(),
            PathBuf::from("python3"),
            Vec::new(),
        );
        let capture = crate::mlx_diagnostics::EmitRecordCaptureForTest::default();
        runtime.test_emit_capture = Some(capture.clone());
        let mut timer = crate::metrics::Metrics::default().stop_to_insert(
            crate::metrics::MetricGroup::unknown("mlx_gate_drop_trace_test"),
            CancellationToken::new(),
        );
        let trace = timer.bind_trace(91);
        let mut operation = Box::pin(runtime.complete_with_source_history_and_prefix_traced(
            vec![
                ("system".to_owned(), "synthetic system".to_owned()),
                ("user".to_owned(), "synthetic input".to_owned()),
            ],
            None,
            None,
            Some(trace.clone()),
        ));
        let mut cx = Context::from_waker(std::task::Waker::noop());
        capture.with_scope(|| {
            assert!(matches!(operation.as_mut().poll(&mut cx), Poll::Pending));
            drop(operation);
        });
        drop(held);
        let snapshot = trace.snapshot();
        let reached = snapshot.stages.clone();
        assert!(reached
            .iter()
            .any(|(stage, _, _)| *stage == crate::metrics::Stage::MlxGateWait));
        assert!(!reached.iter().any(|(stage, _, _)| {
            matches!(
                *stage,
                crate::metrics::Stage::RuntimeStartup
                    | crate::metrics::Stage::FileValidation
                    | crate::metrics::Stage::ModelLoad
                    | crate::metrics::Stage::PromptPreparation
            )
        }));
        timer.finish(crate::metrics::DeliveryOutcome::Failed, None);
        let terminals: Vec<serde_json::Value> = capture
            .snapshot()
            .iter()
            .filter_map(|line| serde_json::from_str(line).ok())
            .filter(|event: &serde_json::Value| {
                event.get("event").and_then(serde_json::Value::as_str) == Some("cleanup_request")
                    && event.get("phase").and_then(serde_json::Value::as_str) == Some("terminal")
            })
            .collect();
        assert_eq!(terminals.len(), 1);
        assert_eq!(
            terminals[0]
                .get("outcome")
                .and_then(serde_json::Value::as_str),
            Some("cancelled_or_abandoned")
        );
        assert_eq!(snapshot.session_generation, 91);
        assert_eq!(
            runtime
                .child_exit_statuses
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .len(),
            0
        );
        assert_eq!(
            runtime
                .child_exit_observers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .len(),
            0
        );
        assert_eq!(runtime.manager.test_lease_count(), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_cleanup_reuses_child_generation() {
        use tokio::io::AsyncReadExt;

        static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
        let suffix = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "voiceflow-task22-cancel-reuse-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create owned Task 22 fixture");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind owned fake-worker observer");
        let port = listener.local_addr().expect("read observer port").port();
        let worker_path = root.join("cancel-worker.py");
        let worker_source = format!(
            r#"import json, socket, sys
port={port}
held=None
cleanups=0
def emit(req,status,**extra):
 base={{'protocol_version':1,'request_id':req['request_id'],'session_id':req.get('session_id'),'model_version':req.get('model_version'),'op':req['op'],'platform_supported':True,'status':status}}
 base.update(extra); sys.stdout.write(json.dumps(base,separators=(',',':'))+'\n'); sys.stdout.flush()
for line in sys.stdin:
 req=json.loads(line)
 if req['op']=='hello': emit(req,'ready')
 elif req['op']=='load': emit(req,'loaded',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'])
 elif req['op']=='cleanup':
  cleanups+=1
  if cleanups==1:
   control=socket.socket(); control.connect(('127.0.0.1',port))
   held=req; control.sendall(b'c'); control.recv(1)
  else:
   emit(req,'completed',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'],cleanup_result={{'text':'same-child-n-plus-one','prompt_tokens':5,'generation_tokens':1,'prompt_time_seconds':0.01,'generation_time_seconds':0.02,'completion':'stop'}})
 elif req['op']=='cancel' and held is not None:
  control.sendall(b'x')
  if control.recv(1)==b'q':
   emit(held,'cancelled',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'])
   control.sendall(b't')
   if control.recv(1)==b'a':
    emit(req,'cancelled',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'])
    held=None
 elif req['op']=='unload': emit(req,'unloaded')
 elif req['op']=='quit': emit(req,'completed'); break
"#
        );
        std::fs::write(&worker_path, worker_source).expect("write owned fake worker");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&worker_path, std::fs::Permissions::from_mode(0o600))
                .expect("restrict owned fake worker source");
        }
        let mut runtime = Runtime::with_test_worker(
            root.join("models"),
            Manager::new(),
            PathBuf::from("python3"),
            vec!["-B".into(), worker_path.to_string_lossy().into_owned()],
        );
        let capture = crate::mlx_diagnostics::EmitRecordCaptureForTest::default();
        runtime.test_emit_capture = Some(capture.clone());
        let first =
            capture.with_scope_future(runtime.complete_with_source_history_and_prefix_traced(
                vec![
                    ("system".to_owned(), "synthetic system".to_owned()),
                    ("user".to_owned(), "synthetic cancellation input".to_owned()),
                ],
                None,
                None,
                None,
            ));
        let mut first = Box::pin(first);
        let (mut control, _) = tokio::select! {
            biased;
            result = &mut first => panic!("fake cleanup did not block for cancellation: {result:?}"),
            accepted = listener.accept() => accepted.expect("worker notified cleanup dispatch"),
        };
        let mut first_notice = [0u8; 1];
        let cleanup_notice_ok = tokio::time::timeout(
            Duration::from_secs(3),
            control.read_exact(&mut first_notice),
        )
        .await
        .is_ok_and(|result| result.is_ok())
            && first_notice == *b"c";
        drop(first);
        let _ = control.write_all(b"d").await;
        let mut cancel_notice = [0u8; 1];
        let cancel_observed = tokio::time::timeout(
            Duration::from_secs(3),
            control.read_exact(&mut cancel_notice),
        )
        .await
        .is_ok_and(|result| result.is_ok())
            && cancel_notice == *b"x";
        let mut second = Box::pin(capture.with_scope_future(
            runtime.complete_with_source_history_and_prefix_traced(
                vec![
                    ("system".to_owned(), "synthetic system".to_owned()),
                    ("user".to_owned(), "synthetic N plus one".to_owned()),
                ],
                None,
                None,
                None,
            ),
        ));
        let preterminal_result = tokio::time::timeout(Duration::from_millis(100), &mut second)
            .await
            .ok();
        let pending_before_terminal = preterminal_result.is_none();
        let _ = control.write_all(b"q").await;
        let mut terminal_notice = [0u8; 1];
        let terminal_observed = tokio::time::timeout(
            Duration::from_secs(3),
            control.read_exact(&mut terminal_notice),
        )
        .await
        .is_ok_and(|result| result.is_ok())
            && terminal_notice == *b"t";
        let lease_retained_between_frames = runtime.manager.test_lease_count() > 0;
        let gate_retained_between_frames = cleanup_operation_gate().try_lock().is_err();
        let session_retained_between_frames = runtime.session.try_lock().is_err();
        let before_ack = if preterminal_result.is_none() {
            Some(tokio::time::timeout(Duration::from_millis(100), &mut second).await)
        } else {
            None
        };
        let pending_before_ack = matches!(before_ack.as_ref(), Some(Err(_)));
        let _ = control.write_all(b"a").await;
        let second = if let Some(result) = preterminal_result {
            Ok(result)
        } else if let Some(Ok(result)) = before_ack {
            Ok(result)
        } else {
            tokio::time::timeout(Duration::from_secs(15), &mut second).await
        };
        drop(control);
        let second_ok =
            matches!(second, Ok(Ok(ref reply)) if reply.text == "same-child-n-plus-one");
        let events: Vec<serde_json::Value> = capture
            .snapshot()
            .iter()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let child_generations: Vec<u64> = events
            .iter()
            .filter(|event| {
                event.get("event").and_then(serde_json::Value::as_str) == Some("cleanup_generation")
            })
            .filter_map(|event| {
                event
                    .get("child_generation")
                    .and_then(serde_json::Value::as_u64)
            })
            .collect();
        let only_one_child_generation = !child_generations.is_empty()
            && child_generations
                .iter()
                .all(|generation| *generation == child_generations[0]);
        runtime.begin_shutdown();
        let first_exit_observations = runtime
            .wait_for_all_test_children()
            .await
            .expect("all owned fake child handles resolve through actual try_wait");
        runtime.shutdown().await;
        let exits = runtime
            .wait_for_all_test_children()
            .await
            .expect("shutdown leaves no unobserved fake child handle");
        let exit_statuses = runtime
            .child_exit_statuses
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let leases_zero = runtime.manager.test_lease_count() == 0;
        let observer_count = runtime
            .child_exit_observers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len();
        let every_child_reaped = observer_count > 0
            && !first_exit_observations.is_empty()
            && !exits.is_empty()
            && exits.len() == exit_statuses.len()
            && exits.iter().all(|(id, _, code)| {
                exit_statuses.iter().any(|(observed_id, _, observed_code)| {
                    observed_id == id && observed_code == code
                })
            });
        if leases_zero && every_child_reaped {
            std::fs::remove_dir_all(&root)
                .expect("remove fixture after all child exits and leases");
        }

        assert!(
            cleanup_notice_ok,
            "the fake child received the original cleanup"
        );
        assert!(
            cancel_observed,
            "caller Drop sends a bound cancel to the same child"
        );
        assert!(
            pending_before_terminal,
            "N+1 remains blocked before the original terminal"
        );
        assert!(
            terminal_observed,
            "fake worker reaches an independent terminal barrier"
        );
        assert!(
            lease_retained_between_frames,
            "lease stays held between terminal and ACK"
        );
        assert!(
            gate_retained_between_frames,
            "operation gate stays held between terminal and ACK"
        );
        assert!(
            session_retained_between_frames,
            "session I/O stays exclusively owned between frames"
        );
        assert!(
            pending_before_ack,
            "N+1 remains blocked between terminal and ACK"
        );
        assert!(
            second_ok,
            "the next cleanup succeeds after terminal plus cancel ACK"
        );
        assert!(
            only_one_child_generation,
            "the settled cancellation reuses the same child"
        );
        assert!(
            every_child_reaped,
            "fixture child exit status was observed (observers={observer_count}, first={first_exit_observations:?}, exits={exits:?}, statuses={exit_statuses:?})"
        );
        assert!(leases_zero, "all model leases are released after shutdown");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn completed_response_caller_drop_settles_owner_and_releases_gate() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
        let suffix = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "voiceflow-task22-response-drop-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create owned response-drop fixture");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind owned response-drop observer");
        let port = listener.local_addr().expect("read observer port").port();
        let worker_path = root.join("response-drop-worker.py");
        let worker_source = format!(
            r#"import json, socket, sys
port={port}
cleanups=0
def emit(req):
 base={{'protocol_version':1,'request_id':req['request_id'],'session_id':req.get('session_id'),'model_version':req.get('model_version'),'op':req['op'],'platform_supported':True,'status':'completed','loaded_model_id':req.get('model_id'),'loaded_model_version':req.get('model_version'),'cleanup_result':{{'text':'completed-before-drop','prompt_tokens':5,'generation_tokens':1,'prompt_time_seconds':0.01,'generation_time_seconds':0.01,'completion':'stop'}}}}
 sys.stdout.write(json.dumps(base,separators=(',',':'))+'\n'); sys.stdout.flush()
for line in sys.stdin:
 req=json.loads(line)
 if req['op']=='hello':
  base={{'protocol_version':1,'request_id':req['request_id'],'session_id':req.get('session_id'),'model_version':req.get('model_version'),'op':'hello','platform_supported':True,'status':'ready'}}; sys.stdout.write(json.dumps(base,separators=(',',':'))+'\n'); sys.stdout.flush()
 elif req['op']=='load':
  base={{'protocol_version':1,'request_id':req['request_id'],'session_id':req.get('session_id'),'model_version':req.get('model_version'),'op':'load','platform_supported':True,'status':'loaded','loaded_model_id':req['model_id'],'loaded_model_version':req['model_version']}}; sys.stdout.write(json.dumps(base,separators=(',',':'))+'\n'); sys.stdout.flush()
 elif req['op']=='cleanup':
  cleanups+=1
  if cleanups==1:
   control=socket.socket(); control.connect(('127.0.0.1',port)); control.sendall(b'c')
   if control.recv(1)==b'r': emit(req)
  else: emit(req)
 elif req['op']=='unload': pass
 elif req['op']=='quit': break
"#
        );
        std::fs::write(&worker_path, worker_source).expect("write owned response-drop worker");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&worker_path, std::fs::Permissions::from_mode(0o600))
                .expect("restrict response-drop worker source");
        }
        let (reached, release) = (
            Arc::new(tokio::sync::Notify::new()),
            Arc::new(tokio::sync::Notify::new()),
        );
        let mut runtime = Runtime::with_test_worker(
            root.join("models"),
            Manager::new(),
            PathBuf::from("python3"),
            vec!["-B".into(), worker_path.to_string_lossy().into_owned()],
        );
        runtime.test_response_handoff_pause = Some((Arc::clone(&reached), Arc::clone(&release)));
        let (settled_reached, settled_release) = (
            Arc::new(tokio::sync::Notify::new()),
            Arc::new(tokio::sync::Notify::new()),
        );
        let join_reached = Arc::new(tokio::sync::Notify::new());
        runtime.test_settled_owner_pause =
            Some((Arc::clone(&settled_reached), Arc::clone(&settled_release)));
        let first = runtime.complete_with_source_history_and_prefix_traced(
            vec![
                ("system".to_owned(), "synthetic system".to_owned()),
                (
                    "user".to_owned(),
                    "synthetic response-drop input".to_owned(),
                ),
            ],
            None,
            None,
            None,
        );
        let mut first = Box::pin(first);
        let (mut control, _) = tokio::select! {
            biased;
            result = &mut first => panic!("fake cleanup did not reach response barrier: {result:?}"),
            accepted = listener.accept() => accepted.expect("worker notified response barrier"),
        };
        let mut dispatch_notice = [0u8; 1];
        let dispatched =
            control.read_exact(&mut dispatch_notice).await.is_ok() && dispatch_notice == *b"c";
        control
            .write_all(b"r")
            .await
            .expect("release completed response");
        reached.notified().await;
        drop(first);
        release.notify_one();
        runtime.test_response_handoff_pause = None;
        tokio::time::timeout(Duration::from_secs(5), settled_reached.notified())
            .await
            .expect("prior response owner reaches post-settlement barrier");
        runtime.test_generation_join_reached = Some(Arc::clone(&join_reached));
        let mut n_plus_one = Box::pin(runtime.complete_with_source_history_and_prefix_traced(
            vec![
                ("system".to_owned(), "synthetic system".to_owned()),
                ("user".to_owned(), "synthetic dropped N plus one".to_owned()),
            ],
            None,
            None,
            None,
        ));
        tokio::select! {
            biased;
            result = &mut n_plus_one => panic!("N+1 passed the held previous-owner completion: {result:?}"),
            result = tokio::time::timeout(Duration::from_secs(5), join_reached.notified()) => {
                result.expect("N+1 reaches previous-owner await")
            }
        }
        drop(n_plus_one);
        runtime.test_generation_join_reached = None;
        settled_release.notify_one();
        let previous_owner_joined = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if runtime.generation_task.lock().unwrap().is_some() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        let dropped_owner_child_reaped = tokio::time::timeout(
            CHILD_EXIT_TIMEOUT + Duration::from_secs(3),
            runtime.wait_for_all_test_children(),
        )
        .await
        .is_ok_and(|statuses| statuses.is_some());
        let mut n_plus_two = Box::pin(runtime.complete_with_source_history_and_prefix_traced(
            vec![
                ("system".to_owned(), "synthetic system".to_owned()),
                ("user".to_owned(), "synthetic N plus two".to_owned()),
            ],
            None,
            None,
            None,
        ));
        let (mut second_control, _) = tokio::select! {
            biased;
            result = &mut n_plus_two => panic!("N+2 failed before the replacement child: {result:?}"),
            accepted = tokio::time::timeout(Duration::from_secs(5), listener.accept()) => {
                accepted.expect("N+2 reaches the fake child after prior-owner join")
                    .expect("accept N+2 child notice")
            }
        };
        let mut second_notice = [0u8; 1];
        let second_dispatched =
            second_control.read_exact(&mut second_notice).await.is_ok() && second_notice == *b"c";
        second_control
            .write_all(b"r")
            .await
            .expect("release N+2 cleanup response");
        let next = tokio::time::timeout(Duration::from_secs(5), &mut n_plus_two).await;
        let next_debug = format!("{next:?}");
        let next_succeeded =
            matches!(next, Ok(Ok(ref reply)) if reply.text == "completed-before-drop");
        drop(n_plus_two);
        let child_count = runtime
            .child_exit_observers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len();
        runtime.test_settled_owner_pause = None;
        runtime.begin_shutdown();
        let observed = runtime
            .wait_for_all_test_children()
            .await
            .expect("response-drop children exit through actual supervisor");
        runtime.shutdown().await;
        let statuses = runtime
            .child_exit_statuses
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let leases_zero = runtime.manager.test_lease_count() == 0;
        let exit_observed = observed.len() == statuses.len()
            && observed.len() == child_count
            && observed.iter().all(|(id, _, code)| {
                statuses
                    .iter()
                    .any(|(status_id, _, status_code)| status_id == id && status_code == code)
            });
        if exit_observed && leases_zero {
            std::fs::remove_dir_all(&root)
                .expect("remove fixture after child exits and lease release");
        }

        assert!(dispatched, "first fake child reached cleanup dispatch");
        assert!(
            previous_owner_joined,
            "prior owner handle remained joinable"
        );
        assert!(
            dropped_owner_child_reaped,
            "dropped N+1 child exit was observed before N+2"
        );
        assert!(second_dispatched, "N+2 reached the replacement fake child");
        assert!(
            next_succeeded,
            "N+2 completes after dropped N+1: {next_debug}"
        );
        assert!(child_count >= 2, "dropped N+1 retires its child before N+2");
        assert!(exit_observed, "every child exit status was observed");
        assert!(leases_zero, "all model leases are released after shutdown");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dropping_shutdown_join_restores_actual_runtime_generation_owner() {
        static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
        let suffix = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "voiceflow-task22-shutdown-owner-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create owned shutdown-owner fixture");
        let worker_path = root.join("shutdown-owner-worker.py");
        let worker_source = r#"import json, sys
for line in sys.stdin:
 req=json.loads(line)
 base={'protocol_version':1,'request_id':req['request_id'],'session_id':req.get('session_id'),'model_version':req.get('model_version'),'op':req['op'],'platform_supported':True}
 if req['op']=='hello': base.update(status='ready')
 elif req['op']=='load': base.update(status='loaded',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'])
 elif req['op']=='cleanup': base.update(status='completed',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'],cleanup_result={'text':'shutdown-owner-response','prompt_tokens':2,'generation_tokens':1,'prompt_time_seconds':0.01,'generation_time_seconds':0.01,'completion':'stop'})
 elif req['op']=='quit': break
 sys.stdout.write(json.dumps(base,separators=(',',':'))+'\n'); sys.stdout.flush()
"#;
        std::fs::write(&worker_path, worker_source).expect("write owned shutdown-owner worker");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&worker_path, std::fs::Permissions::from_mode(0o600))
                .expect("restrict shutdown-owner worker source");
        }
        let (reached, release) = (
            Arc::new(tokio::sync::Notify::new()),
            Arc::new(tokio::sync::Notify::new()),
        );
        let mut runtime = Runtime::with_test_worker(
            root.join("models"),
            Manager::new(),
            PathBuf::from("python3"),
            vec!["-B".into(), worker_path.to_string_lossy().into_owned()],
        );
        runtime.test_response_handoff_pause = Some((Arc::clone(&reached), Arc::clone(&release)));
        let mut completion = Box::pin(runtime.complete_with_source_history_and_prefix_traced(
            vec![
                ("system".to_owned(), "synthetic system".to_owned()),
                ("user".to_owned(), "synthetic shutdown owner".to_owned()),
            ],
            None,
            None,
            None,
        ));
        let response = tokio::time::timeout(Duration::from_secs(5), &mut completion)
            .await
            .expect("actual fake child returns a response")
            .expect("actual runtime completes the request");
        assert_eq!(response.text, "shutdown-owner-response");
        drop(completion);
        tokio::time::timeout(Duration::from_secs(5), reached.notified())
            .await
            .expect("actual generation owner pauses before settling");
        assert!(runtime.generation_task.lock().unwrap().is_some());

        let mut first_shutdown = Box::pin(runtime.shutdown());
        tokio::select! {
            biased;
            result = &mut first_shutdown => panic!("shutdown unexpectedly passed the held generation join: {result:?}"),
            _ = tokio::task::yield_now() => {}
        }
        drop(first_shutdown);
        assert!(
            runtime.generation_task.lock().unwrap().is_some(),
            "dropping public shutdown restores the actual pending generation task"
        );

        release.notify_one();
        runtime.test_response_handoff_pause = None;
        tokio::time::timeout(
            CHILD_EXIT_TIMEOUT + Duration::from_secs(3),
            runtime.shutdown(),
        )
        .await
        .expect("second shutdown joins the restored owner and reaps its child");
        let exits = runtime
            .wait_for_all_test_children()
            .await
            .expect("actual fake child reaches observed exit");
        assert!(!exits.is_empty());
        assert_eq!(runtime.manager.test_lease_count(), 0);
        assert!(runtime.generation_task.lock().unwrap().is_none());
        std::fs::remove_dir_all(&root)
            .expect("remove fixture after owner join, child exit and lease release");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancel_timeout_falls_back_to_kill() {
        use tokio::io::AsyncReadExt;

        static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
        let suffix = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "voiceflow-task22-cancel-timeout-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create owned cancellation-timeout fixture");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind owned fake-worker observer");
        let port = listener.local_addr().expect("read observer port").port();
        let counter_path = root.join("worker-count");
        let worker_path = root.join("timeout-worker.py");
        let worker_source = format!(
            r#"import json, os, socket, sys
port={port}
counter={counter_path:?}
try:
 instance=int(open(counter).read())+1
except Exception:
 instance=1
open(counter,'w').write(str(instance))
held=None
def notice(value):
 s=socket.socket(); s.connect(('127.0.0.1',port)); s.sendall(value.encode()); s.close()
def emit(req,status,**extra):
 base={{'protocol_version':1,'request_id':req['request_id'],'session_id':req.get('session_id'),'model_version':req.get('model_version'),'op':req['op'],'platform_supported':True,'status':status}}
 base.update(extra); sys.stdout.write(json.dumps(base,separators=(',',':'))+'\n'); sys.stdout.flush()
for line in sys.stdin:
 req=json.loads(line)
 if req['op']=='hello': emit(req,'ready')
 elif req['op']=='load': emit(req,'loaded',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'])
 elif req['op']=='cleanup':
  if instance==1:
   held=req; notice('cleanup1')
  else:
   notice('cleanup2')
   emit(req,'completed',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'],cleanup_result={{'text':'replacement-after-reap','prompt_tokens':5,'generation_tokens':1,'prompt_time_seconds':0.01,'generation_time_seconds':0.02,'completion':'stop'}})
 elif req['op']=='cancel' and held is not None:
  notice('cancel')
  # Deliberately retain ownership without a terminal or ACK. Rust must kill
  # this exact process after the one bounded cooperative grace interval.
 elif req['op']=='unload': emit(req,'unloaded')
 elif req['op']=='quit': emit(req,'completed'); break
"#
        );
        std::fs::write(&worker_path, worker_source).expect("write owned fake worker");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&worker_path, std::fs::Permissions::from_mode(0o600))
                .expect("restrict owned fake worker source");
        }
        let mut runtime = Runtime::with_test_worker(
            root.join("models"),
            Manager::new(),
            PathBuf::from("python3"),
            vec!["-B".into(), worker_path.to_string_lossy().into_owned()],
        );
        let capture = crate::mlx_diagnostics::EmitRecordCaptureForTest::default();
        runtime.test_emit_capture = Some(capture.clone());
        let first =
            capture.with_scope_future(runtime.complete_with_source_history_and_prefix_traced(
                vec![
                    ("system".to_owned(), "synthetic system".to_owned()),
                    ("user".to_owned(), "synthetic cancellation input".to_owned()),
                ],
                None,
                None,
                None,
            ));
        let mut first = Box::pin(first);
        let (mut control, _) = tokio::select! {
            biased;
            result = &mut first => panic!("fake cleanup did not block before cancellation: {result:?}"),
            accepted = listener.accept() => accepted.expect("first child notified cleanup dispatch"),
        };
        let mut cleanup_notice = [0u8; 8];
        let cleanup1_ok =
            control.read_exact(&mut cleanup_notice).await.is_ok() && &cleanup_notice == b"cleanup1";
        drop(first);
        drop(control);
        let cancel_started = tokio::time::Instant::now();
        let cancel_notice = tokio::time::timeout(Duration::from_secs(3), listener.accept()).await;
        let cancel_observed = if let Ok(Ok((mut stream, _))) = cancel_notice {
            let mut notice = [0u8; 6];
            stream.read_exact(&mut notice).await.is_ok() && &notice == b"cancel"
        } else {
            false
        };

        let mut second = Box::pin(capture.with_scope_future(
            runtime.complete_with_source_history_and_prefix_traced(
                vec![
                    ("system".to_owned(), "synthetic system".to_owned()),
                    ("user".to_owned(), "synthetic N plus one".to_owned()),
                ],
                None,
                None,
                None,
            ),
        ));
        let mut unexpected_worker_activity = false;
        let early_result = tokio::select! {
            biased;
            result = &mut second => Some(result),
            accepted = listener.accept() => {
                let (mut stream, _) = accepted.expect("read any premature worker activity");
                let mut notice = [0u8; 8];
                let count = stream.read(&mut notice).await.unwrap_or(0);
                unexpected_worker_activity = count != 0;
                None
            }
            _ = tokio::time::sleep(Duration::from_millis(250)) => None,
        };
        let second_started_before_exit = early_result.is_some() || unexpected_worker_activity;
        let lease_retained_during_grace = runtime.manager.test_lease_count() > 0;
        let first_observer = runtime
            .child_exit_observers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .first()
            .cloned();
        let first_exit_observed = if let Some((_, mut receiver)) = first_observer {
            tokio::time::timeout(Duration::from_secs(5), async move {
                loop {
                    if *receiver.borrow() {
                        break true;
                    }
                    if receiver.changed().await.is_err() {
                        break *receiver.borrow();
                    }
                }
            })
            .await
            .unwrap_or(false)
        } else {
            false
        };
        let fallback_elapsed = cancel_started.elapsed();

        let first_child_id = runtime
            .child_exit_observers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .first()
            .map(|(id, _)| *id);
        let old_exit_status_before_replacement = first_child_id.is_some_and(|old_id| {
            runtime
                .child_exit_statuses
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .iter()
                .any(|(status_id, _, _)| *status_id == old_id)
        });
        let leases_released_after_old_exit =
            old_exit_status_before_replacement && runtime.manager.test_lease_count() == 0;

        let second_result = match early_result {
            Some(result) => Some(Ok(result)),
            None => Some(tokio::time::timeout(Duration::from_secs(15), &mut second).await),
        };
        let second_ok = matches!(second_result, Some(Ok(Ok(ref reply))) if reply.text == "replacement-after-reap");
        let child_ids: Vec<u64> = runtime
            .child_exit_observers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(|(id, _)| *id)
            .collect();
        let generations_distinct = child_ids.len() >= 2 && child_ids[0] != child_ids[1];
        runtime.begin_shutdown();
        let observed = runtime
            .wait_for_all_test_children()
            .await
            .expect("every Task 22 fake child exit is observed");
        runtime.shutdown().await;
        let statuses = runtime
            .child_exit_statuses
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let replacement_has_its_own_status = child_ids.get(1).is_some_and(|replacement_id| {
            statuses
                .iter()
                .any(|(status_id, _, _)| status_id == replacement_id)
        });
        let leases_zero = runtime.manager.test_lease_count() == 0;
        let all_owned_exits_observed = !observed.is_empty()
            && observed.len() == statuses.len()
            && observed.iter().all(|(id, _, code)| {
                statuses
                    .iter()
                    .any(|(status_id, _, status_code)| id == status_id && code == status_code)
            });
        if all_owned_exits_observed && leases_zero {
            std::fs::remove_dir_all(&root)
                .expect("remove owned fixture after every child exit and lease release");
        }

        assert!(
            cleanup1_ok,
            "first fake child reached actual cleanup dispatch"
        );
        assert!(
            cancel_observed,
            "first child received the bound cancel frame"
        );
        assert!(
            !second_started_before_exit,
            "N+1 stays blocked while N owns its lease/gate"
        );
        assert!(
            lease_retained_during_grace,
            "model lease remains held during cancel grace"
        );
        assert!(
            first_exit_observed,
            "fallback observes the first child's actual exit"
        );
        assert!(fallback_elapsed >= Duration::from_secs(2));
        assert!(fallback_elapsed < Duration::from_secs(4));
        assert!(second_ok, "N+1 runs only after actual fallback reap");
        assert!(
            generations_distinct,
            "fallback starts a distinct owned child only after retiring N"
        );
        assert!(
            old_exit_status_before_replacement && leases_released_after_old_exit,
            "N's actual ExitStatus and lease release precede N+1 (old={first_child_id:?}, statuses={:?}, leases={})",
            runtime.child_exit_statuses.lock().unwrap_or_else(|p| p.into_inner()),
            runtime.manager.test_lease_count()
        );
        assert!(
            replacement_has_its_own_status,
            "N+1 is also a separately observed child"
        );
        assert!(
            all_owned_exits_observed,
            "all child ExitStatus values were collected"
        );
        assert!(leases_zero, "all model leases are released after shutdown");
    }

    async fn startup_warmup_cancel_settlement_fixture(caller_drop: bool) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
        let suffix = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "voiceflow-task22-warmup-cancel-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create owned warmup fixture");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind owned warmup fake-worker observer");
        let port = listener.local_addr().expect("read observer port").port();
        let worker_path = root.join("warmup-cancel-worker.py");
        let worker_source = format!(
            r#"import json, socket, sys, threading, queue
port={port}
held=None
active_control=None
incoming=queue.Queue()
state_lock=threading.Lock()
def read_requests():
 for line in sys.stdin:
  req=json.loads(line)
  with state_lock:
   control=active_control if held is not None and req['op']=='cleanup' else None
  if control is not None:
   try: control.sendall(b'n')
   except OSError: pass
  incoming.put(req)
threading.Thread(target=read_requests,daemon=True).start()
def emit(req,status,**extra):
 base={{'protocol_version':1,'request_id':req['request_id'],'session_id':req.get('session_id'),'model_version':req.get('model_version'),'op':req['op'],'platform_supported':True,'status':status}}
 base.update(extra); sys.stdout.write(json.dumps(base,separators=(',',':'))+'\n'); sys.stdout.flush()
while True:
 req=incoming.get()
 if req['op']=='hello': emit(req,'ready')
 elif req['op']=='load': emit(req,'loaded',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'])
 elif req['op']=='warmup':
  control=socket.socket(); control.connect(('127.0.0.1',port))
  with state_lock: held=req; active_control=control
  control.sendall(b'w'); control.recv(1)
 elif req['op']=='cancel' and held is not None:
  control.sendall(b'x')
  if control.recv(1)==b'q':
   emit(held,'cancelled',loaded_model_id=req['model_id'],loaded_model_version=req['model_version']); control.sendall(b't')
   if control.recv(1)==b'a':
    emit(req,'cancelled',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'])
    with state_lock: held=None; active_control=None
 elif req['op']=='cleanup':
  emit(req,'completed',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'],cleanup_result={{'text':'cleanup-after-warmup-cancel','prompt_tokens':5,'generation_tokens':1,'prompt_time_seconds':0.01,'generation_time_seconds':0.01,'completion':'stop'}})
 elif req['op']=='unload': emit(req,'unloaded')
 elif req['op']=='quit': emit(req,'completed'); break
"#
        );
        std::fs::write(&worker_path, worker_source).expect("write owned warmup worker");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&worker_path, std::fs::Permissions::from_mode(0o600))
                .expect("restrict warmup worker source");
        }
        let runtime = Arc::new(Runtime::with_test_worker(
            root.join("models"),
            Manager::new(),
            PathBuf::from("python3"),
            vec!["-B".into(), worker_path.to_string_lossy().into_owned()],
        ));
        let warmup = runtime.prewarm_startup(
            vec![
                ("system".to_owned(), "synthetic warmup system".to_owned()),
                ("user".to_owned(), "synthetic warmup input".to_owned()),
            ],
            None,
            "task22_synthetic",
            Arc::new(|| true),
        );
        let mut warmup = Some(Box::pin(warmup));
        let (mut control, _) = tokio::select! {
            biased;
            result = warmup.as_mut().expect("warmup future retained").as_mut() => panic!("warmup did not reach fake worker barrier: {result:?}"),
            accepted = listener.accept() => accepted.expect("worker notified warmup dispatch"),
        };
        let mut warmup_notice = [0u8; 1];
        let warmup_dispatched =
            control.read_exact(&mut warmup_notice).await.is_ok() && warmup_notice == *b"w";
        let coordinator = crate::cleanup_warmup::Coordinator::default();
        let mut demand = None;
        let mut demand_registered = false;
        if caller_drop {
            warmup.take();
        } else {
            demand = Some(crate::register_cleanup_model_demand(
                &coordinator,
                Arc::clone(&runtime),
            ));
            demand_registered =
                coordinator.has_model_demand() && runtime.foreground_model_demand_count() == 1;
        }
        let in_flight_retained_after_caller_drop = runtime
            .startup_warmup_in_flight_owners
            .load(Ordering::Acquire)
            > 0;
        let _ = control.write_all(b"d").await;
        let mut cancel_notice = [0u8; 1];
        let cancel_observed = tokio::time::timeout(
            Duration::from_secs(3),
            control.read_exact(&mut cancel_notice),
        )
        .await
        .is_ok_and(|result| result.is_ok())
            && cancel_notice == *b"x";
        let trace_metrics = crate::metrics::Metrics::default();
        let mut trace_timer = trace_metrics.stop_to_insert(
            crate::metrics::MetricGroup::unknown("task24-warmup-eval-fixture"),
            CancellationToken::new(),
        );
        let foreground_trace = trace_timer.bind_trace(1);
        let mut cleanup = Box::pin(runtime.complete_with_source_history_and_prefix_traced(
            vec![
                ("system".to_owned(), "synthetic cleanup system".to_owned()),
                ("user".to_owned(), "synthetic N plus one".to_owned()),
            ],
            None,
            None,
            Some(foreground_trace.clone()),
        ));
        let pending_before_terminal =
            tokio::time::timeout(Duration::from_millis(100), &mut cleanup)
                .await
                .is_err();
        let _ = control.write_all(b"q").await;
        let mut terminal_notice = [0u8; 1];
        let terminal_observed = tokio::time::timeout(
            Duration::from_secs(3),
            control.read_exact(&mut terminal_notice),
        )
        .await
        .is_ok_and(|result| result.is_ok())
            && terminal_notice == *b"t";
        let in_flight_retained_between_frames = runtime
            .startup_warmup_in_flight_owners
            .load(Ordering::Acquire)
            > 0;
        let mut premature_frame_notice = [0u8; 1];
        let n_plus_one_observed_before_ack = tokio::time::timeout(
            Duration::from_millis(100),
            control.read_exact(&mut premature_frame_notice),
        )
        .await
        .is_ok_and(|result| result.is_ok() && premature_frame_notice == *b"n");
        let _ = control.write_all(b"a").await;
        let cleanup_result = tokio::time::timeout(Duration::from_secs(15), &mut cleanup).await;
        let cleanup_succeeded = matches!(cleanup_result, Ok(Ok(ref reply)) if reply.text == "cleanup-after-warmup-cancel");
        let owner_settled = tokio::time::timeout(Duration::from_secs(3), async {
            while runtime
                .generation_task
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_ref()
                .is_some_and(|task| !task.is_finished())
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        let warmup_yield_result = if caller_drop {
            None
        } else {
            Some(
                tokio::time::timeout(
                    Duration::from_secs(3),
                    warmup
                        .as_mut()
                        .expect("demand keeps warmup future alive")
                        .as_mut(),
                )
                .await,
            )
        };
        drop(demand.take());
        let demand_released =
            !coordinator.has_model_demand() && runtime.foreground_model_demand_count() == 0;
        let in_flight_cleared_after_settlement = runtime
            .startup_warmup_in_flight_owners
            .load(Ordering::Acquire)
            == 0;
        drop(control);
        stop_reap_and_remove(&runtime, &root).await;

        assert!(
            caller_drop || demand_registered,
            "fixture used production demand registration core"
        );
        assert!(
            demand_released,
            "fixture demand ownership releases after settlement"
        );
        assert!(warmup_dispatched, "warmup request reached the owned child");
        assert!(
            in_flight_retained_after_caller_drop,
            "driver retains warmup in-flight ownership after caller Drop"
        );
        assert!(
            cancel_observed,
            "worker received cancellation for the warmup request"
        );
        assert!(
            pending_before_terminal,
            "cleanup N+1 stays blocked before warmup terminal"
        );
        assert!(
            terminal_observed,
            "worker reached the independent warmup terminal barrier"
        );
        assert!(
            in_flight_retained_between_frames,
            "warmup remains in-flight between original terminal and cancel ACK"
        );
        assert!(
            !n_plus_one_observed_before_ack,
            "independent stdin reader sees no N+1 frame while the ACK is held"
        );
        assert!(
            cleanup_succeeded,
            "N+1 cleanup succeeds after warmup pair settles"
        );
        assert!(
            !foreground_trace
                .snapshot()
                .stages
                .iter()
                .any(|(stage, _, _)| *stage == crate::metrics::Stage::ModelLoad),
            "successful foreground trace contains no model-load stage"
        );
        assert!(
            foreground_trace
                .snapshot()
                .stages
                .iter()
                .any(|(stage, _, _)| *stage == crate::metrics::Stage::MlxGateWait),
            "trace records the foreground waiting at the actual MLX gate"
        );
        if let Some(Ok(Err(RuntimeError::StartupWarmupYielded { child_generation }))) =
            warmup_yield_result.as_ref()
        {
            assert_eq!(
                Some(*child_generation),
                cleanup_result
                    .as_ref()
                    .ok()
                    .and_then(|result| result.as_ref().ok())
                    .map(|completion| completion.child_generation),
                "foreground cleanup reuses the child from the settled warmup receipt"
            );
        } else if !caller_drop {
            panic!("foreground demand returns an explicit same-child yield receipt: {warmup_yield_result:?}");
        }
        assert!(
            owner_settled && in_flight_cleared_after_settlement,
            "owner settles and releases in-flight state"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_startup_warmup_retains_in_flight_until_pair_settles() {
        startup_warmup_cancel_settlement_fixture(true).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn foreground_demand_cancels_warmup_eval_and_reuses_settled_child() {
        startup_warmup_cancel_settlement_fixture(false).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn foreground_demand_during_load_skips_eval_and_reuses_same_child() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
        let suffix = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "voiceflow-task24-load-yield-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create owned load-yield fixture");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind owned load barrier");
        let port = listener
            .local_addr()
            .expect("read load barrier port")
            .port();
        let worker_path = root.join("load-yield-worker.py");
        let worker_source = r#"import json, socket, sys
port=PORT
def emit(req,status,**extra):
 base={'protocol_version':1,'request_id':req['request_id'],'session_id':req.get('session_id'),'model_version':req.get('model_version'),'op':req['op'],'platform_supported':True,'status':status}
 base.update(extra); sys.stdout.write(json.dumps(base,separators=(',',':'))+'\n'); sys.stdout.flush()
for line in sys.stdin:
 req=json.loads(line)
 if req['op']=='hello': emit(req,'ready')
 elif req['op']=='load':
  control=socket.socket(); control.connect(('127.0.0.1',port)); control.sendall(b'l'); control.recv(1)
  emit(req,'loaded',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'])
 elif req['op']=='warmup':
  control=socket.socket(); control.connect(('127.0.0.1',port)); control.sendall(b'w'); control.recv(1)
  emit(req,'completed',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'],warmup_result={'prompt_tokens':5,'generation_tokens':1,'prompt_time_seconds':0.01,'generation_time_seconds':0.01,'completion':'stop','info_observed':True,'tool_call_observed':False,'completion_text_seen':True,'prompt_evaluation_observed':True,'observed_prompt_tokens':5,'stream_drained':True,'evaluation_synchronized':True,'evaluation_time_seconds':0.02,'prompt_cache_status':'bypassed','cached_prefix_tokens':0,'computed_suffix_tokens':5})
 elif req['op']=='cleanup': emit(req,'completed',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'],cleanup_result={'text':'foreground-reused-load','prompt_tokens':5,'generation_tokens':1,'prompt_time_seconds':0.01,'generation_time_seconds':0.01,'completion':'stop'})
 elif req['op']=='unload': emit(req,'unloaded')
 elif req['op']=='quit': emit(req,'completed'); break
"#
        .replace("PORT", &port.to_string());
        std::fs::write(&worker_path, worker_source).expect("write owned load-yield worker");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&worker_path, std::fs::Permissions::from_mode(0o600))
                .expect("restrict load-yield worker source");
        }
        let runtime = Arc::new(Runtime::with_test_worker(
            root.join("models"),
            Manager::new(),
            PathBuf::from("python3"),
            vec!["-B".into(), worker_path.to_string_lossy().into_owned()],
        ));
        let mut warmup = Box::pin(runtime.prewarm_startup(
            vec![
                ("system".to_owned(), "synthetic warmup system".to_owned()),
                ("user".to_owned(), "synthetic warmup input".to_owned()),
            ],
            None,
            "task24_load_yield",
            Arc::new(|| true),
        ));
        let (mut load_control, _) = tokio::select! {
            biased;
            result = &mut warmup => panic!("warmup completed before held load: {result:?}"),
            accepted = listener.accept() => accepted.expect("worker reached held model load"),
        };
        let mut load_notice = [0u8; 1];
        let load_is_held =
            load_control.read_exact(&mut load_notice).await.is_ok() && load_notice == *b"l";
        let coordinator = crate::cleanup_warmup::Coordinator::default();
        let demand = crate::register_cleanup_model_demand(&coordinator, Arc::clone(&runtime));
        let demand_registered =
            coordinator.has_model_demand() && runtime.foreground_model_demand_count() == 1;
        let trace_metrics = crate::metrics::Metrics::default();
        let mut trace_timer = trace_metrics.stop_to_insert(
            crate::metrics::MetricGroup::unknown("task24-warmup-load-fixture"),
            CancellationToken::new(),
        );
        let foreground_trace = trace_timer.bind_trace(1);
        let mut foreground = Box::pin(runtime.complete_with_source_history_and_prefix_traced(
            vec![
                (
                    "system".to_owned(),
                    "synthetic foreground system".to_owned(),
                ),
                ("user".to_owned(), "synthetic foreground input".to_owned()),
            ],
            None,
            None,
            Some(foreground_trace.clone()),
        ));
        let foreground_waited_for_load =
            tokio::time::timeout(Duration::from_millis(100), &mut foreground)
                .await
                .is_err();
        let _ = load_control.write_all(b"r").await;
        let (warmup_result, warmup_eval_was_dispatched) = tokio::select! {
            biased;
            result = &mut warmup => (Some(result), false),
            accepted = listener.accept() => {
                let (mut eval_control, _) = accepted.expect("accept a wrongly dispatched warmup eval for cleanup");
                let mut eval_notice = [0u8; 1];
                let is_eval = eval_control.read_exact(&mut eval_notice).await.is_ok() && eval_notice == *b"w";
                let _ = eval_control.write_all(b"r").await;
                let result = tokio::time::timeout(Duration::from_secs(10), &mut warmup).await.ok();
                (result, is_eval)
            }
        };
        let foreground_result =
            tokio::time::timeout(Duration::from_secs(15), &mut foreground).await;
        drop(demand);
        drop(load_control);
        stop_reap_and_remove(&runtime, &root).await;

        assert!(
            demand_registered,
            "held-LOAD fixture used production demand registration core"
        );
        assert!(!coordinator.has_model_demand());
        assert_eq!(runtime.foreground_model_demand_count(), 0);

        let receipt_generation = match warmup_result {
            Some(Err(RuntimeError::StartupWarmupYielded { child_generation })) => {
                Some(child_generation)
            }
            other => panic!("held LOAD yields a concrete surviving-child receipt: {other:?}"),
        };
        let foreground = foreground_result
            .expect("foreground completes after load release")
            .expect("foreground cleanup succeeds");
        assert!(load_is_held, "fake worker reached actual model LOAD");
        assert!(
            foreground_waited_for_load,
            "foreground waits for held warmup LOAD"
        );
        assert!(
            !warmup_eval_was_dispatched,
            "demand suppresses undispatched warmup EVAL"
        );
        assert_eq!(receipt_generation, Some(foreground.child_generation));
        assert!(
            !foreground_trace
                .snapshot()
                .stages
                .iter()
                .any(|(stage, _, _)| *stage == crate::metrics::Stage::ModelLoad),
            "successful foreground trace contains no model-load stage"
        );
        assert!(
            foreground_trace
                .snapshot()
                .stages
                .iter()
                .any(|(stage, _, _)| *stage == crate::metrics::Stage::MlxGateWait),
            "trace records the foreground waiting at the actual MLX gate"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn traced_cleanup_pending_after_dispatch_drop_reaps_owned_child() {
        use tokio::io::AsyncReadExt;

        static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
        let suffix = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "voiceflow-task18-hold-cleanup-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create owned hold-cleanup fixture");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind owned loopback dispatch observer");
        let port = listener.local_addr().expect("read listener address").port();
        let worker_path = root.join("hold-worker.py");
        let worker_source = format!(
            r#"import json, socket, sys
port={port}
for line in sys.stdin:
 req=json.loads(line)
 base={{'protocol_version':1,'request_id':req['request_id'],'session_id':req.get('session_id'),'model_version':req.get('model_version'),'op':req['op'],'platform_supported':True}}
 if req['op']=='hello': base.update(status='ready')
 elif req['op']=='load': base.update(status='loaded',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'])
 elif req['op']=='cleanup':
  control=socket.socket(); control.connect(('127.0.0.1',port)); control.sendall(b'dispatched'); control.recv(1)
  base.update(status='completed',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'],cleanup_result={{'text':'synthetic completion','prompt_tokens':5,'generation_tokens':1,'prompt_time_seconds':0.01,'generation_time_seconds':0.02,'completion':'stop'}})
 sys.stdout.write(json.dumps(base,separators=(',',':'))+'\n'); sys.stdout.flush()
"#
        );
        std::fs::write(&worker_path, worker_source).expect("write owned hold-cleanup worker");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&worker_path, std::fs::Permissions::from_mode(0o600))
                .expect("restrict owned worker source");
        }
        let mut runtime = Runtime::with_test_worker(
            root.join("models"),
            Manager::new(),
            PathBuf::from("python3"),
            vec!["-B".into(), worker_path.to_string_lossy().into_owned()],
        );
        let capture = crate::mlx_diagnostics::EmitRecordCaptureForTest::default();
        runtime.test_emit_capture = Some(capture.clone());
        let mut timer = crate::metrics::Metrics::default().stop_to_insert(
            crate::metrics::MetricGroup::unknown("task18_hold_cleanup_drop"),
            CancellationToken::new(),
        );
        let trace = timer.bind_trace(94);
        crate::mlx_diagnostics::panic_on_event_phase_for_test(
            crate::mlx_diagnostics::EventName::CleanupRequest,
            "terminal",
        );
        let future = runtime.complete_with_source_history_and_prefix_traced(
            vec![
                ("system".to_owned(), "synthetic system".to_owned()),
                ("user".to_owned(), "synthetic input".to_owned()),
            ],
            None,
            None,
            Some(trace.clone()),
        );
        let mut operation = Box::pin(capture.with_scope_future(future));
        let (mut control, _) = tokio::select! {
            biased;
            result = &mut operation => panic!("held cleanup unexpectedly completed: {result:?}"),
            accepted = listener.accept() => accepted.expect("worker reached held cleanup exchange"),
        };
        let mut dispatch = [0u8; 10];
        control
            .read_exact(&mut dispatch)
            .await
            .expect("read explicit dispatch acknowledgment");
        assert_eq!(&dispatch, b"dispatched");
        let snapshot = trace.snapshot();
        drop(operation);
        timer.finish(crate::metrics::DeliveryOutcome::Cancelled, None);
        stop_reap_and_remove(&runtime, &root).await;

        let reached: Vec<_> = snapshot.stages.iter().map(|(stage, _, _)| *stage).collect();
        for stage in [
            crate::metrics::Stage::MlxGateWait,
            crate::metrics::Stage::RuntimeStartup,
            crate::metrics::Stage::FileValidation,
            crate::metrics::Stage::ModelLoad,
        ] {
            assert!(
                reached.contains(&stage),
                "held cleanup must reach {stage:?}: {reached:?}"
            );
        }
        assert!(!reached.contains(&crate::metrics::Stage::FinalGuard));
        assert!(
            snapshot.reported_stages.is_empty(),
            "cancelled held cleanup has no generation timing facts"
        );
        let terminals: Vec<serde_json::Value> = capture
            .snapshot()
            .into_iter()
            .chain(capture.panicked_attempts())
            .filter_map(|line| serde_json::from_str(&line).ok())
            .filter(|event: &serde_json::Value| {
                event.get("event").and_then(serde_json::Value::as_str) == Some("cleanup_request")
                    && event.get("phase").and_then(serde_json::Value::as_str) == Some("terminal")
                    && event
                        .get("session_generation")
                        .and_then(serde_json::Value::as_u64)
                        == Some(94)
            })
            .collect();
        assert_eq!(
            terminals.len(),
            1,
            "dropped dispatched request emits one terminal observation"
        );
        assert_eq!(
            terminals[0]
                .get("outcome")
                .and_then(serde_json::Value::as_str),
            Some("cancelled_or_abandoned")
        );
        assert_eq!(
            terminals[0]
                .get("stage")
                .and_then(serde_json::Value::as_str),
            Some("unknown")
        );
        let delivered_terminal_count = capture
            .snapshot()
            .iter()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|event| {
                event.get("event").and_then(serde_json::Value::as_str) == Some("cleanup_request")
                    && event.get("phase").and_then(serde_json::Value::as_str) == Some("terminal")
            })
            .count();
        let panicked_terminal_count = capture
            .panicked_attempts()
            .iter()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|event| {
                event.get("event").and_then(serde_json::Value::as_str) == Some("cleanup_request")
                    && event.get("phase").and_then(serde_json::Value::as_str) == Some("terminal")
            })
            .count();
        assert_eq!(delivered_terminal_count + panicked_terminal_count, 1);
        assert_eq!(
            panicked_terminal_count, 1,
            "dropped request terminal panic is captured once"
        );
        drop(control);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn traced_cleanup_late_generation_cannot_mutate_sealed_trace() {
        use std::future::Future;
        use std::task::{Context, Poll};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
        let suffix = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "voiceflow-task18-late-generation-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create owned late-generation fixture");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind owned loopback dispatch observer");
        let port = listener.local_addr().expect("read listener address").port();
        let worker_path = root.join("late-worker.py");
        let worker_source = format!(
            r#"import json, socket, sys
port={port}
cleanup_count=0
for line in sys.stdin:
 req=json.loads(line)
 base={{'protocol_version':1,'request_id':req['request_id'],'session_id':req.get('session_id'),'model_version':req.get('model_version'),'op':req['op'],'platform_supported':True}}
 if req['op']=='hello': base.update(status='ready')
 elif req['op']=='load': base.update(status='loaded',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'])
 elif req['op']=='cleanup':
  cleanup_count += 1
  if cleanup_count==1:
   control=socket.socket(); control.connect(('127.0.0.1',port)); control.sendall(b'held'); control.recv(1)
  base.update(status='completed',loaded_model_id=req['model_id'],loaded_model_version=req['model_version'],cleanup_result={{'text':'synthetic completion '+str(cleanup_count),'prompt_tokens':5,'generation_tokens':1,'prompt_time_seconds':0.01,'generation_time_seconds':0.02,'completion':'stop'}})
 sys.stdout.write(json.dumps(base,separators=(',',':'))+'\n'); sys.stdout.flush()
"#
        );
        std::fs::write(&worker_path, worker_source).expect("write owned late-generation worker");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&worker_path, std::fs::Permissions::from_mode(0o600))
                .expect("restrict owned worker source");
        }
        let mut runtime = Runtime::with_test_worker(
            root.join("models"),
            Manager::new(),
            PathBuf::from("python3"),
            vec!["-B".into(), worker_path.to_string_lossy().into_owned()],
        );
        let capture = crate::mlx_diagnostics::EmitRecordCaptureForTest::default();
        runtime.test_emit_capture = Some(capture.clone());
        let mut timer_n = crate::metrics::Metrics::default().stop_to_insert(
            crate::metrics::MetricGroup::unknown("task18_generation_n"),
            CancellationToken::new(),
        );
        let trace_n = timer_n.bind_trace(101);
        let mut timer_n1 = crate::metrics::Metrics::default().stop_to_insert(
            crate::metrics::MetricGroup::unknown("task18_generation_n_plus_one"),
            CancellationToken::new(),
        );
        let trace_n1 = timer_n1.bind_trace(102);
        let llm_gate = crate::queue::RequestGate::new(None);
        let llm_scope = crate::queue::RequestScope::new(
            "task18_late_generation_llm",
            None,
            crate::providers::EngineProvider::OnDeviceMlx.default_llm_model(),
            "synthetic",
        );

        let (first_result, second_result, sealed_n_snapshot, n1_wait_snapshot) = capture
            .with_scope_future(async {
                let first = crate::queue::execute_with_retry_scoped_cancelled_traced(
                    &llm_gate,
                    crate::queue::RequestKind::Llm,
                    &llm_scope,
                    || async {
                        crate::llm::cleanup_with_on_device_mlx_runtime_and_intent_and_source(
                            &runtime,
                            crate::providers::EngineProvider::OnDeviceMlx.default_llm_model(),
                            "synthetic dictation N",
                            &[], None, None, None, None, None, None,
                            crate::llm::CleanupEffort::Standard,
                            None, None, None,
                            Some(&trace_n),
                        ).await
                    },
                    CancellationToken::new(),
                    Some(trace_n.clone()),
                );
                let mut first = Box::pin(first);
                let (mut control, _) = tokio::select! {
                    biased;
                    result = &mut first => panic!("held generation N completed early: {result:?}"),
                    accepted = listener.accept() => accepted.expect("worker acknowledged held generation N"),
                };
                let mut acknowledgment = [0u8; 4];
                control.read_exact(&mut acknowledgment).await.expect("read held generation acknowledgment");
                assert_eq!(&acknowledgment, b"held");

                timer_n.finish(crate::metrics::DeliveryOutcome::Failed, None);
                let sealed_n_snapshot = trace_n.snapshot();
                let second = crate::queue::execute_with_retry_scoped_cancelled_traced(
                    &llm_gate,
                    crate::queue::RequestKind::Llm,
                    &llm_scope,
                    || async {
                        crate::llm::cleanup_with_on_device_mlx_runtime_and_intent_and_source(
                            &runtime,
                            crate::providers::EngineProvider::OnDeviceMlx.default_llm_model(),
                            "synthetic dictation N plus one",
                            &[], None, None, None, None, None, None,
                            crate::llm::CleanupEffort::Standard,
                            None, None, None,
                            Some(&trace_n1),
                        ).await
                    },
                    CancellationToken::new(),
                    Some(trace_n1.clone()),
                );
                let mut second = Box::pin(second);
                let mut cx = Context::from_waker(std::task::Waker::noop());
                assert!(matches!(second.as_mut().poll(&mut cx), Poll::Pending));
                let n1_wait_snapshot = trace_n1.snapshot();
                assert!(n1_wait_snapshot.stages.len() < 256);
                assert!(!n1_wait_snapshot.stages.iter().any(|(stage, _, _)| {
                    matches!(*stage, crate::metrics::Stage::RuntimeStartup | crate::metrics::Stage::FileValidation | crate::metrics::Stage::ModelLoad)
                }), "N+1 has not started child initialization while generation N owns the operation gate");
                assert!(n1_wait_snapshot.reported_stages.is_empty(), "N+1 has not received response facts before release");

                control.write_all(b"x").await.expect("release held generation N");
                drop(control);
                let (first_result, second_result) = tokio::join!(first, second);
                timer_n1.finish(crate::metrics::DeliveryOutcome::Failed, None);
                (first_result, second_result, sealed_n_snapshot, n1_wait_snapshot)
            })
            .await;
        assert!(
            first_result.is_ok(),
            "generation N response completes after its trace was sealed"
        );
        let accepted_n1 = second_result.expect("generation N+1 completes after gate release");
        assert_eq!(accepted_n1.0.accepted_text, "synthetic completion 2");
        assert!(
            sealed_n_snapshot.stages.len() < 256 && sealed_n_snapshot.reported_stages.len() < 256
        );
        assert_eq!(
            trace_n.snapshot(),
            sealed_n_snapshot,
            "late N result leaves its sealed host/reported/cache snapshot unchanged"
        );
        let snapshot_n1 = trace_n1.snapshot();
        assert!(snapshot_n1.stages.len() < 256 && snapshot_n1.reported_stages.len() < 256);
        assert!(snapshot_n1
            .stages
            .iter()
            .any(|(stage, _, _)| *stage == crate::metrics::Stage::MlxGateWait));
        assert!(
            !snapshot_n1.stages.iter().any(|(stage, _, _)| {
                matches!(
                    *stage,
                    crate::metrics::Stage::RuntimeStartup
                        | crate::metrics::Stage::FileValidation
                        | crate::metrics::Stage::ModelLoad
                )
            }),
            "healthy N+1 reuse reaches no startup, inventory, or load stages"
        );
        assert!(snapshot_n1
            .reported_stages
            .iter()
            .any(|fact| fact.stage == crate::metrics::Stage::Prefill));
        assert!(snapshot_n1
            .reported_stages
            .iter()
            .any(|fact| fact.stage == crate::metrics::Stage::Decode));
        assert!(n1_wait_snapshot.stages.len() < 256);
        let records: Vec<serde_json::Value> = capture
            .snapshot()
            .iter()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let generations: Vec<_> = records
            .iter()
            .filter(|event| {
                event.get("event").and_then(serde_json::Value::as_str) == Some("cleanup_generation")
            })
            .collect();
        assert_eq!(
            generations.len(),
            2,
            "late N and fresh N+1 events remain separately attributed"
        );
        assert!(generations.iter().any(|event| event
            .get("session_generation")
            .and_then(serde_json::Value::as_u64)
            == Some(101)));
        assert!(generations.iter().any(|event| event
            .get("session_generation")
            .and_then(serde_json::Value::as_u64)
            == Some(102)));
        let reused_child_generation = generations[0]
            .get("child_generation")
            .and_then(serde_json::Value::as_u64)
            .expect("generation event retains child ID");
        assert!(generations.iter().all(|event| event
            .get("child_generation")
            .and_then(serde_json::Value::as_u64)
            == Some(reused_child_generation)));
        let terminals: Vec<_> = records
            .iter()
            .filter(|event| {
                event.get("event").and_then(serde_json::Value::as_str) == Some("cleanup_request")
                    && event.get("phase").and_then(serde_json::Value::as_str) == Some("terminal")
            })
            .collect();
        assert_eq!(terminals.len(), 2);
        assert!(terminals.iter().any(|event| event
            .get("session_generation")
            .and_then(serde_json::Value::as_u64)
            == Some(101)));
        assert!(terminals.iter().any(|event| event
            .get("session_generation")
            .and_then(serde_json::Value::as_u64)
            == Some(102)));
        assert_eq!(
            runtime
                .child_exit_observers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .len(),
            1,
            "healthy queued N+1 reuses the same child generation"
        );
        runtime.shutdown().await;
        let exits = runtime
            .wait_for_all_test_children()
            .await
            .expect("all fake child exits observed before deletion");
        assert_eq!(exits.len(), 1, "the one reused child is reaped once");
        assert_eq!(exits[0].0, reused_child_generation);
        assert_eq!(runtime.manager.test_lease_count(), 0);
        std::fs::remove_dir_all(&root).expect("remove owned fixture after confirmed child exit");
    }

    #[test]
    fn warmup_cache_metrics_keep_build_and_bypass_work_distinct() {
        assert!(valid_warmup_cache_metrics("built", 712, 0, 200));
        assert!(valid_warmup_cache_metrics("memory_hit", 712, 512, 200));
        assert!(valid_warmup_cache_metrics("snapshot_hit", 712, 512, 200));
        assert!(valid_warmup_cache_metrics("bypassed", 712, 0, 712));
        assert!(!valid_warmup_cache_metrics("built", 712, 512, 200));
        assert!(!valid_warmup_cache_metrics("bypassed", 712, 0, 200));
        assert!(!valid_warmup_cache_metrics("memory_hit", 712, 0, 712));
    }

    #[test]
    fn warmup_requires_drained_evaluation_but_not_decoded_primer_text() {
        let facts = |generation_tokens, completion, drained, synchronized, tool_call| {
            valid_warmup_terminal_values(
                899,
                Some(899),
                Some(generation_tokens),
                Some(completion),
                Some(true),
                Some(tool_call),
                Some(false),
                Some(true),
                Some(drained),
                Some(synchronized),
            )
        };

        assert!(facts(0, "stop", true, true, false));
        assert!(facts(1, "length", true, true, false));
        assert!(!facts(0, "cancelled", true, true, false));
        assert!(!facts(1, "length", false, false, false));
        assert!(!facts(1, "length", true, true, true));
        assert!(!valid_warmup_terminal_values(
            899,
            Some(899),
            Some(0),
            Some("stop"),
            None,
            Some(false),
            Some(false),
            Some(true),
            Some(true),
            Some(true),
        ));
    }

    #[test]
    fn startup_warmup_uses_distinct_op_but_production_output_budget() {
        let request = Request::startup_warmup(
            "startup-warmup-test".to_owned(),
            Some("startup-warmup-session".to_owned()),
            vec![
                ("system".to_owned(), "fixed system prompt".to_owned()),
                ("user".to_owned(), "synthetic warmup transcript".to_owned()),
            ],
            Some(0),
        );
        let value = serde_json::to_value(request).expect("warmup request serializes");
        assert_eq!(value["op"], "warmup");
        assert_eq!(value["max_output_tokens"], MAX_OUTPUT_TOKENS);
        assert_eq!(value["stable_prefix_user_bytes"], 0);
        assert_eq!(
            value["messages"][1]["content"],
            "synthetic warmup transcript"
        );
    }

    #[test]
    fn response_runtime_facts_are_optional_and_typed() {
        let legacy: Response = serde_json::from_str(
            r#"{"protocol_version":1,"request_id":"request-1","session_id":null,"model_version":null,"op":"cleanup","status":"completed"}"#,
        )
        .expect("older sidecar responses omit optional runtime facts");
        assert!(legacy.cleanup_diagnostics.is_none());

        let malformed: Response = serde_json::from_str(
            r#"{"protocol_version":1,"request_id":"request-3","session_id":null,"model_version":null,"op":"cleanup","status":"completed","cleanup_diagnostics":{"phases":"unexpected"}}"#,
        )
        .expect("malformed optional diagnostics do not invalidate a cleanup response");
        assert!(malformed.cleanup_diagnostics.is_none());

        let old_shape: Response = serde_json::from_str(
            r#"{"protocol_version":1,"request_id":"request-old","session_id":null,"model_version":null,"op":"cleanup","status":"completed","cleanup_diagnostics":{"model_prepare_seconds":1.5,"cache_plan_seconds":0.25,"phases":[]}}"#,
        )
        .expect("older cleanup diagnostics may omit cache-plan substage timings");
        let old_shape_facts = serde_json::to_value(
            old_shape
                .cleanup_diagnostics
                .expect("old shape remains typed"),
        )
        .expect("old shape remains serializable");
        assert!(old_shape_facts["cache_plan_timings"].is_null());

        let response: Response = serde_json::from_str(
            r#"{"protocol_version":1,"request_id":"request-2","session_id":null,"model_version":null,"op":"cleanup","status":"completed","cleanup_diagnostics":{"model_prepare_seconds":1.5,"cache_plan_seconds":0.25,"cache_plan_timings":{"candidate_prepare_count":4,"candidate_prepare_milliseconds":40.0,"stable_user_prefix_prepare_count":2,"stable_user_prefix_prepare_milliseconds":25.0,"system_only_prepare_count":2,"system_only_prepare_milliseconds":15.0,"perform_to_closure_entry_milliseconds":5.0,"mlx_generate_call_milliseconds":50.0},"phases":[{"phase":"before_prepare","elapsed_seconds":0.0,"mlx_active_memory_bytes":1024,"mlx_cache_memory_bytes":2048,"mlx_peak_memory_bytes":4096,"mlx_memory_limit_bytes":8192,"device":"gpu","task_priority":"user_initiated","thread_qos":"user_initiated","thermal_state":"nominal","process_user_cpu_seconds":1.25,"process_system_cpu_seconds":0.25,"process_minor_page_faults":12,"process_major_page_faults":1,"process_page_ins":3,"process_physical_footprint_bytes":16384}]}}"#,
        )
        .expect("cleanup resource facts use a typed, bounded protocol shape");
        let facts = response
            .cleanup_diagnostics
            .expect("new sidecars can report cleanup resource facts");
        let encoded = serde_json::to_value(facts).expect("typed facts serialize without content");
        assert_eq!(encoded["phases"].as_array().map(Vec::len), Some(1));
        assert_eq!(encoded["cache_plan_timings"]["candidate_prepare_count"], 4);
        assert_eq!(
            encoded["cache_plan_timings"]["system_only_prepare_count"],
            2
        );
    }

    #[test]
    fn input_token_budget_failure_keeps_prompt_output_and_limit_facts() {
        let failure: SidecarFailure = serde_json::from_str(
            r#"{"code":"input_too_large","prompt_tokens":6145,"requested_output_tokens":2048,"requested_total_tokens":8193,"max_total_tokens":8192}"#,
        )
        .expect("the sidecar token-budget failure uses the bounded protocol shape");

        match map_sidecar_error(Some(&failure)) {
            RuntimeError::InputTooLarge {
                prompt_tokens,
                requested_output_tokens,
                requested_total_tokens,
                max_total_tokens,
            } => {
                assert_eq!(prompt_tokens, 6_145);
                assert_eq!(requested_output_tokens, 2_048);
                assert_eq!(requested_total_tokens, Some(8_193));
                assert_eq!(max_total_tokens, 8_192);
            }
            _ => panic!("valid over-budget token facts map to the typed local error"),
        }
    }

    #[test]
    fn malformed_or_non_exceeding_token_budget_facts_are_not_trusted() {
        for json in [
            r#"{"code":"input_too_large","prompt_tokens":6145,"requested_output_tokens":2048,"requested_total_tokens":8193,"max_total_tokens":4096}"#,
            r#"{"code":"input_too_large","prompt_tokens":6144,"requested_output_tokens":2048,"requested_total_tokens":8192,"max_total_tokens":8192}"#,
        ] {
            let failure: SidecarFailure = serde_json::from_str(json).expect("decode failure facts");
            assert!(matches!(
                map_sidecar_error(Some(&failure)),
                RuntimeError::InvalidResponse
            ));
        }
    }

    #[test]
    fn critical_memory_pressure_failure_remains_typed_for_the_g10_fallback() {
        let failure: SidecarFailure =
            serde_json::from_str(r#"{"code":"memory_pressure_critical"}"#)
                .expect("critical pressure uses a bounded sidecar failure code");
        assert!(matches!(
            map_sidecar_error(Some(&failure)),
            RuntimeError::CriticalMemoryPressure
        ));
    }

    #[test]
    fn startup_warmup_skips_warning_and_unknown_but_preserves_critical_cause() {
        use crate::model_resources::HostMemoryPressure as Pressure;
        assert!(startup_warmup_pressure_error(Pressure::Normal).is_none());
        assert!(matches!(
            startup_warmup_pressure_error(Pressure::Warning),
            Some(RuntimeError::MemoryUnavailable(
                crate::model_resources::MemoryUnavailableReason::PressureNotNormal
            ))
        ));
        assert!(matches!(
            startup_warmup_pressure_error(Pressure::Unknown),
            Some(RuntimeError::MemoryUnavailable(
                crate::model_resources::MemoryUnavailableReason::UnknownPressure
            ))
        ));
        assert!(matches!(
            startup_warmup_pressure_error(Pressure::Critical),
            Some(RuntimeError::CriticalMemoryPressure)
        ));
    }

    #[test]
    fn memory_unavailable_cause_survives_pending_child_retirement() {
        let cause = RuntimeError::MemoryUnavailable(
            crate::model_resources::MemoryUnavailableReason::BudgetExceeded,
        );
        assert!(cause.to_string().contains("budget_exceeded"));
        let preserved = error_after_child_retirement(cause, RuntimeError::RetainedPending);
        assert!(matches!(
            preserved,
            RuntimeError::MemoryUnavailable(
                crate::model_resources::MemoryUnavailableReason::BudgetExceeded
            )
        ));
        assert_eq!(runtime_error_code(&preserved), "local_memory_unavailable");
    }

    #[test]
    fn critical_pressure_keeps_its_fallback_cause_while_owned_child_retirement_is_pending() {
        static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
        let suffix = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "voiceflow-task27-critical-retirement-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create unique model-root fixture");
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn only this test's owned placeholder child");
        let registration =
            crate::model_resources::ChildRegistration::register(child.id(), suffix + 1, &root)
                .expect("register the actual placeholder child");
        let (preserved, remains_registered_with_pending_load) = {
            let _reservation = registration.reserve_pending_load();

            let preserved = error_after_child_retirement(
                RuntimeError::CriticalMemoryPressure,
                RuntimeError::RetainedPending,
            );
            let still_owned = crate::model_resources::owned_children_for_root(&root);
            let remains_registered_with_pending_load = still_owned.len() == 1 && still_owned[0].2;

            let _ = child.kill();
            let _ = child.wait().expect("observe placeholder child exit");
            registration.retire_after_exit();
            (preserved, remains_registered_with_pending_load)
        };
        let retired_after_observed_exit =
            crate::model_resources::owned_children_for_root(&root).is_empty();
        std::fs::remove_dir_all(&root).expect("remove fixture after observed exit");

        assert!(matches!(preserved, RuntimeError::CriticalMemoryPressure));
        assert!(remains_registered_with_pending_load);
        assert!(retired_after_observed_exit);
    }

    #[test]
    fn only_matching_load_terminals_settle_the_owned_reservation() {
        static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
        let suffix = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "voiceflow-cleanup-load-reservation-{}-{suffix}",
            std::process::id()
        ));
        let request = Request {
            protocol_version: PROTOCOL_VERSION,
            request_id: "load-request".into(),
            session_id: Some("load-session".into()),
            model_version: Some(MODEL_REVISION.into()),
            op: "load".into(),
            model_id: Some(MODEL_ID.into()),
            model_directory: Some("/fixture/model".into()),
            messages: None,
            max_output_tokens: None,
            stable_prefix_user_bytes: None,
        };
        let cases = [
            (
                "matching loaded",
                serde_json::json!({"status":"loaded","loaded_model_id":MODEL_ID,"loaded_model_version":MODEL_REVISION}),
                true,
            ),
            (
                "matching error without loaded identity",
                serde_json::json!({"status":"error","loaded_model_id":null,"loaded_model_version":null}),
                true,
            ),
            (
                "stale request",
                serde_json::json!({"request_id":"stale","status":"loaded","loaded_model_id":MODEL_ID,"loaded_model_version":MODEL_REVISION}),
                false,
            ),
            (
                "wrong session",
                serde_json::json!({"session_id":"other","status":"loaded","loaded_model_id":MODEL_ID,"loaded_model_version":MODEL_REVISION}),
                false,
            ),
            (
                "wrong operation",
                serde_json::json!({"op":"status","status":"loaded","loaded_model_id":MODEL_ID,"loaded_model_version":MODEL_REVISION}),
                false,
            ),
            (
                "wrong protocol",
                serde_json::json!({"protocol_version":PROTOCOL_VERSION + 1,"status":"loaded","loaded_model_id":MODEL_ID,"loaded_model_version":MODEL_REVISION}),
                false,
            ),
            (
                "wrong echoed revision",
                serde_json::json!({"model_version":"other","status":"loaded","loaded_model_id":MODEL_ID,"loaded_model_version":MODEL_REVISION}),
                false,
            ),
            (
                "wrong model",
                serde_json::json!({"status":"loaded","loaded_model_id":"other","loaded_model_version":MODEL_REVISION}),
                false,
            ),
            (
                "wrong revision",
                serde_json::json!({"status":"loaded","loaded_model_id":MODEL_ID,"loaded_model_version":"other"}),
                false,
            ),
            (
                "unknown status",
                serde_json::json!({"status":"ready","loaded_model_id":MODEL_ID,"loaded_model_version":MODEL_REVISION}),
                false,
            ),
            (
                "cancelled load status",
                serde_json::json!({"status":"cancelled"}),
                false,
            ),
            (
                "cancel acknowledgement",
                serde_json::json!({"op":"cancel","status":"cancelled"}),
                false,
            ),
        ];
        std::fs::create_dir_all(&root).unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let mut registrations = Vec::new();
        let mut observations = Vec::new();
        for (index, (name, overrides, settles)) in cases.into_iter().enumerate() {
            let generation = index as u64 + 1;
            let registration = match crate::model_resources::ChildRegistration::register(
                child.id(),
                generation,
                &root,
            ) {
                Some(registration) => registration,
                None => {
                    observations.push((
                        name,
                        false,
                        None,
                        settles,
                        Some("child registration was rejected".to_owned()),
                    ));
                    break;
                }
            };
            let mut response_json = serde_json::json!({
                "protocol_version": PROTOCOL_VERSION,
                "request_id": "load-request",
                "session_id": "load-session",
                "model_version": MODEL_REVISION,
                "op": "load",
                "status": "loaded",
                "loaded_model_id": MODEL_ID,
                "loaded_model_version": MODEL_REVISION
            });
            for (key, value) in overrides.as_object().unwrap() {
                response_json[key] = value.clone();
            }
            let response: Response = match serde_json::from_value(response_json) {
                Ok(response) => response,
                Err(error) => {
                    registrations.push(registration);
                    observations.push((name, false, None, settles, Some(error.to_string())));
                    break;
                }
            };
            let did_settle = settle_cleanup_load_reservation_if_terminal(
                registration.reserve_pending_load(),
                &request,
                &response,
                MODEL_ID,
                MODEL_REVISION,
            );
            let pending = crate::model_resources::owned_children_for_root(&root)
                .into_iter()
                .find(|(identity, _, _)| identity.generation == generation)
                .map(|record| record.2);
            observations.push((name, did_settle, pending, settles, None));
            registrations.push(registration);
        }
        let _ = child.kill();
        let exit_observed = child.wait().is_ok();
        let registrations_retired = if exit_observed {
            for registration in registrations {
                registration.retire_after_exit();
            }
            let remaining = crate::model_resources::owned_children_for_root(&root);
            let retired = remaining.is_empty();
            std::fs::remove_dir_all(&root).unwrap();
            retired
        } else {
            false
        };
        assert!(exit_observed, "owned placeholder child exit observed");
        assert!(
            registrations_retired,
            "all registrations retired after exit"
        );
        for (name, did_settle, pending, expected_settles, error) in observations {
            assert!(error.is_none(), "{name}: {error:?}");
            assert_eq!(did_settle, expected_settles, "{name} settle result");
            assert_eq!(
                pending,
                Some(!expected_settles),
                "{name} pending reservation"
            );
        }
    }
}
