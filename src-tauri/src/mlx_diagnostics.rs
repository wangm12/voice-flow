use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
thread_local! {
    static TEST_WRITER_OVERRIDE: std::cell::RefCell<Option<SyncSender<(Severity, Target, String)>>> = const { std::cell::RefCell::new(None) };
    static PANIC_NEXT_EMIT_FOR_TEST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static PANIC_EVENT_FOR_TEST: std::cell::Cell<Option<EventName>> = const { std::cell::Cell::new(None) };
    static PANIC_EVENTS_FOR_TEST: std::cell::RefCell<Vec<EventName>> = const { std::cell::RefCell::new(Vec::new()) };
    static PANIC_EVENT_PHASE_FOR_TEST: std::cell::Cell<Option<(EventName, Phase)>> = const { std::cell::Cell::new(None) };
    static EMIT_RECORDS_FOR_TEST: std::cell::RefCell<Option<std::sync::Arc<std::sync::Mutex<Vec<String>>>>> = const { std::cell::RefCell::new(None) };
    static PANICKED_EMIT_RECORDS_FOR_TEST: std::cell::RefCell<Option<std::sync::Arc<std::sync::Mutex<Vec<String>>>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn panic_on_next_emit_for_test() {
    PANIC_NEXT_EMIT_FOR_TEST.with(|panic_next| panic_next.set(true));
}

#[cfg(test)]
pub(crate) fn panic_on_event_for_test(event: EventName) {
    PANIC_EVENT_FOR_TEST.with(|selector| selector.set(Some(event)));
}

#[cfg(test)]
pub(crate) fn panic_on_event_phase_for_test(event: EventName, phase: &'static str) {
    let phase = match phase {
        "begin" => Phase::Begin,
        "terminal" => Phase::Terminal,
        "dispatched" => Phase::Dispatched,
        _ => panic!("unsupported diagnostic phase selector"),
    };
    PANIC_EVENT_PHASE_FOR_TEST.with(|selector| selector.set(Some((event, phase))));
}

#[cfg(test)]
pub(crate) fn panic_on_events_for_test(events: &[EventName]) {
    PANIC_EVENTS_FOR_TEST.with(|selector| selector.replace(events.to_vec()));
}

#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct EmitRecordCaptureForTest {
    delivered: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    panicked: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

#[cfg(test)]
impl EmitRecordCaptureForTest {
    pub(crate) fn snapshot(&self) -> Vec<String> {
        self.delivered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(crate) fn panicked_attempts(&self) -> Vec<String> {
        self.panicked
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(crate) fn with_scope<T>(&self, operation: impl FnOnce() -> T) -> T {
        struct Restore(
            Option<std::sync::Arc<std::sync::Mutex<Vec<String>>>>,
            Option<std::sync::Arc<std::sync::Mutex<Vec<String>>>>,
        );
        impl Drop for Restore {
            fn drop(&mut self) {
                let previous = self.0.take();
                let previous_panicked = self.1.take();
                EMIT_RECORDS_FOR_TEST.with(|slot| {
                    slot.replace(previous);
                });
                PANICKED_EMIT_RECORDS_FOR_TEST.with(|slot| {
                    slot.replace(previous_panicked);
                });
            }
        }
        let previous = EMIT_RECORDS_FOR_TEST
            .with(|slot| slot.replace(Some(std::sync::Arc::clone(&self.delivered))));
        let previous_panicked = PANICKED_EMIT_RECORDS_FOR_TEST
            .with(|slot| slot.replace(Some(std::sync::Arc::clone(&self.panicked))));
        let _restore = Restore(previous, previous_panicked);
        operation()
    }

    pub(crate) async fn with_scope_future<T>(
        &self,
        operation: impl std::future::Future<Output = T>,
    ) -> T {
        struct Restore(
            Option<std::sync::Arc<std::sync::Mutex<Vec<String>>>>,
            Option<std::sync::Arc<std::sync::Mutex<Vec<String>>>>,
        );
        impl Drop for Restore {
            fn drop(&mut self) {
                let previous = self.0.take();
                let previous_panicked = self.1.take();
                EMIT_RECORDS_FOR_TEST.with(|slot| {
                    slot.replace(previous);
                });
                PANICKED_EMIT_RECORDS_FOR_TEST.with(|slot| {
                    slot.replace(previous_panicked);
                });
            }
        }
        let previous = EMIT_RECORDS_FOR_TEST
            .with(|slot| slot.replace(Some(std::sync::Arc::clone(&self.delivered))));
        let previous_panicked = PANICKED_EMIT_RECORDS_FOR_TEST
            .with(|slot| slot.replace(Some(std::sync::Arc::clone(&self.panicked))));
        let _restore = Restore(previous, previous_panicked);
        operation.await
    }
}

const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_EVENT_BYTES: usize = 4 * 1024;
const ACTIVE_FILE: &str = "mlx-cleanup-performance.jsonl";

static SINK: OnceLock<Mutex<SinkState>> = OnceLock::new();
static WARNED_SINK_UNAVAILABLE: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static WARNING_BACKEND_FOR_TEST: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);
type DiagnosticWriterMessage = (Severity, Target, String);

static WRITER: OnceLock<Mutex<Option<SyncSender<DiagnosticWriterMessage>>>> = OnceLock::new();
static WRITER_TASK: OnceLock<Mutex<Option<tauri::async_runtime::JoinHandle<()>>>> = OnceLock::new();
static WARNING_TASK: OnceLock<Mutex<Option<tauri::async_runtime::JoinHandle<()>>>> =
    OnceLock::new();
const WRITER_QUEUE_CAPACITY: usize = 256;
#[cfg(test)]
static WARNING_ATTEMPTS_FOR_TEST: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static DIAGNOSTIC_WARNING_TEST_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EventName {
    CleanupRequest,
    CleanupGeneration,
    CleanupAdmission,
    CleanupQueue,
    CleanupChild,
    CleanupChildSpawn,
    CleanupChildReady,
    CleanupSidecarExchange,
    CleanupSidecarTerminal,
    CleanupModelLoad,
    CleanupModelInventory,
    CleanupModelInventoryReady,
    CleanupModelFileHash,
    CleanupGuard,
    LocalAsrStage,
    StartupWarmup,
    HistoryReclean,
    HistoryRecleanFinalizer,
    HistoryRecleanRevision,
    HistoryRecleanClipboard,
    HistoryRecleanRevisionState,
    HistoryRecleanTerminal,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Severity {
    Debug,
    Info,
    Warn,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Target {
    Runtime,
    ModelStore,
    History,
}

impl Target {
    fn logger_target(self) -> &'static str {
        match self {
            Self::Runtime => "voiceflow::mlx::runtime",
            Self::ModelStore => "voiceflow::mlx::modelstore",
            Self::History => "voiceflow::mlx::history",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Begin,
    Terminal,
    Dispatched,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Accepted,
    Acquired,
    Cancelled,
    CancelledOrAbandoned,
    Completed,
    Error,
    Failed,
    Loaded,
    PersistenceFailed,
    Ready,
    Rejected,
    ResponseReceived,
    Reused,
    Missed,
    Skipped,
    Saved,
    Timeout,
    Updated,
    Verified,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Stage {
    Cancellation,
    ChildShutdown,
    ChildStartup,
    Clipboard,
    Exchange,
    Finalizer,
    Generation,
    Handshake,
    InventoryVerification,
    ModelAdmission,
    ModelLoad,
    ModelReadiness,
    PlatformAdmission,
    PromptAdmission,
    Protocol,
    Queue,
    Revision,
    RevisionState,
    FilesValidated,
    SidecarSession,
    InferenceExchange,
    InferenceTerminal,
    RequestTotal,
    Timeout,
    Unknown,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Reason {
    CleanupDisabled,
    ConfigurationChanged,
    HandshakeRejected,
    InputTooLarge,
    ModelNotInstalled,
    NotOnboarded,
    PreservationGuard,
    ProviderNotNative,
    SpawnFailed,
    LocalRouteOnly,
    VerificationRejected,
    Unknown,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ErrorCode {
    Cancelled,
    GenerationFailed,
    IncompleteGeneration,
    InputTooLarge,
    InvalidRequest,
    InvalidResponse,
    ModelLoadFailed,
    ModelMismatch,
    ModelNotLoaded,
    ModelUnavailable,
    RetainedPending,
    ResponseTooLarge,
    SidecarMissing,
    Timeout,
    UnsupportedPlatform,
    Unknown,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ResponseStatus {
    Completed,
    Loaded,
    Ready,
    Other,
}

/// Fixed, content-free terminal classifications for local ASR failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LocalAsrTerminalCode {
    InferenceExchangeFailed,
    ResponseIdentityRejected,
    ResponseStatusFailed,
    LoadedModelIdentityRejected,
    TranscriptionResultMissing,
    LocalCompletionIncomplete,
    LocalCoverageIncomplete,
    LocalGenerationBudgetInvalid,
    LocalEmptyOutputNotAccepted,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum InventoryCache {
    Hit,
    Miss,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum InventoryCacheReason {
    DurableCacheMissing,
    DurableCacheDirectoryInvalid,
    DurableCachePermissionsInvalid,
    DurableCacheTargetUnsupported,
    DurableIdentityChanged,
    DurableIdentityMatch,
    DurableProofMalformed,
    DurableProofMissing,
    DurableProofPermissionsOrSizeInvalid,
    DurableProofSchemaChanged,
    DurableProofUnreadable,
    DurableProofWriteFailed,
    InstalledInventoryChangedOrUnreadable,
    ManagedRootChanged,
    ProcessCacheEmpty,
    TrustedFileIdentityMatch,
    Unknown,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum PromptCacheStatus {
    Bypassed,
    Built,
    MemoryHit,
    SnapshotHit,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum PromptCacheReason {
    BoundaryNotProvided,
    CacheKeyChanged,
    SnapshotCorrupt,
    SnapshotMissing,
    SnapshotTooLarge,
    SnapshotWriteFailed,
    PrefixBelowPrefillStep,
    UnsupportedCacheState,
    Unknown,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum PromptMetricScope {
    ComputedSuffix,
    FullPrompt,
    SuffixAfterPrefixBuild,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum StartupWarmupVariant {
    CodeImplicitLive,
    CodeHistoryReclean,
    GeneralImplicitLive,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum WarmupCompletionReason {
    Stop,
    Length,
    Cancelled,
    Unknown,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Sha256Backend {
    Sha2Aarch64Intrinsics,
    Sha2SoftwareFallback,
    Sha2TargetDefault,
    Unknown,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum HardwareSupport {
    Available,
    Unavailable,
    Unknown,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum CleanupStatus {
    AiFailedLocal,
    AiFailedRaw,
    AiSuccess,
    LocalOnly,
    PreservationGuard,
    SnippetBypass,
    Unknown,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum DeliveryOutcome {
    ClipboardChanged,
    ClipboardOwnershipUnverified,
    ClipboardWriteFailed,
    Copied,
    NotAttemptedDegraded,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum MetricSource {
    MlxGenerationInfo,
    SidecarMonotonic,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum DatabaseTimestampSource {
    SqliteDatetimeNow,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum DatabaseTimestampPrecision {
    Seconds,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum DurationClock {
    RustMonotonic,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LocalAsrOperation {
    Transcribe,
    Preload,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct CleanupRuntimeDiagnostics {
    #[serde(default)]
    model_prepare_seconds: Option<f64>,
    #[serde(default)]
    cache_plan_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_plan_timings: Option<CleanupCachePlanTimings>,
    #[serde(default)]
    phases: Vec<CleanupRuntimeSample>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct CleanupCachePlanTimings {
    candidate_prepare_count: u8,
    candidate_prepare_milliseconds: f64,
    stable_user_prefix_prepare_count: u8,
    stable_user_prefix_prepare_milliseconds: f64,
    system_only_prepare_count: u8,
    system_only_prepare_milliseconds: f64,
    #[serde(default)]
    perform_to_closure_entry_milliseconds: Option<f64>,
    #[serde(default)]
    mlx_generate_call_milliseconds: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
struct CleanupRuntimeSample {
    phase: CleanupRuntimePhase,
    elapsed_seconds: f64,
    mlx_active_memory_bytes: u64,
    mlx_cache_memory_bytes: u64,
    mlx_peak_memory_bytes: u64,
    mlx_memory_limit_bytes: u64,
    device: CleanupRuntimeDevice,
    task_priority: CleanupRuntimeTaskPriority,
    thread_qos: CleanupRuntimeThreadQos,
    thermal_state: CleanupRuntimeThermalState,
    #[serde(default)]
    process_user_cpu_seconds: Option<f64>,
    #[serde(default)]
    process_system_cpu_seconds: Option<f64>,
    #[serde(default)]
    process_minor_page_faults: Option<u64>,
    #[serde(default)]
    process_major_page_faults: Option<u64>,
    #[serde(default)]
    process_page_ins: Option<u64>,
    #[serde(default)]
    process_physical_footprint_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CleanupRuntimePhase {
    BeforePrepare,
    AfterPrepare,
    AfterPlan,
    Terminal,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CleanupRuntimeDevice {
    Cpu,
    Gpu,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CleanupRuntimeTaskPriority {
    UserInitiated,
    High,
    Medium,
    Low,
    Utility,
    Background,
    Other,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CleanupRuntimeThreadQos {
    UserInteractive,
    UserInitiated,
    Utility,
    Background,
    Maintenance,
    Unspecified,
    Other,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CleanupRuntimeThermalState {
    Nominal,
    Fair,
    Serious,
    Critical,
    Unknown,
}

impl CleanupRuntimeDiagnostics {
    fn is_bounded_and_valid(&self) -> bool {
        self.phases.len() <= 4
            && self
                .model_prepare_seconds
                .is_none_or(|value| valid_duration(value).is_some())
            && self
                .cache_plan_seconds
                .is_none_or(|value| valid_duration(value).is_some())
            && self.phases.iter().all(|sample| {
                valid_duration(sample.elapsed_seconds).is_some()
                    && sample
                        .process_user_cpu_seconds
                        .is_none_or(|value| valid_duration(value).is_some())
                    && sample
                        .process_system_cpu_seconds
                        .is_none_or(|value| valid_duration(value).is_some())
            })
    }
}

impl CleanupCachePlanTimings {
    fn is_bounded_and_valid(&self) -> bool {
        let expected_count = self
            .stable_user_prefix_prepare_count
            .saturating_add(self.system_only_prepare_count);
        let expected_milliseconds =
            self.stable_user_prefix_prepare_milliseconds + self.system_only_prepare_milliseconds;
        self.candidate_prepare_count <= 4
            && self.stable_user_prefix_prepare_count <= 2
            && self.system_only_prepare_count <= 2
            && self.candidate_prepare_count == expected_count
            && valid_duration(self.candidate_prepare_milliseconds).is_some()
            && valid_duration(self.stable_user_prefix_prepare_milliseconds).is_some()
            && valid_duration(self.system_only_prepare_milliseconds).is_some()
            && (self.candidate_prepare_milliseconds - expected_milliseconds).abs() <= 1e-9
            && (self.stable_user_prefix_prepare_count != 0
                || self.stable_user_prefix_prepare_milliseconds == 0.0)
            && (self.system_only_prepare_count != 0 || self.system_only_prepare_milliseconds == 0.0)
            && self
                .perform_to_closure_entry_milliseconds
                .is_none_or(|value| valid_duration(value).is_some())
            && self
                .mlx_generate_call_milliseconds
                .is_none_or(|value| valid_duration(value).is_some())
    }
}

pub(crate) fn slow_cleanup_runtime_diagnostics(
    request_total_seconds: f64,
    diagnostics: Option<&CleanupRuntimeDiagnostics>,
) -> Option<&CleanupRuntimeDiagnostics> {
    if request_total_seconds > 6.0 {
        diagnostics
    } else {
        None
    }
}

#[derive(Serialize)]
struct DiagnosticRecord {
    schema_version: u8,
    timestamp_unix_ms: Option<u64>,
    event: EventName,
    severity: Severity,
    target: Target,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    local_asr_operation: Option<LocalAsrOperation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_history_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    revision_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    child_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_index: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<Phase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<Outcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stage: Option<Stage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_code: Option<LocalAsrTerminalCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<Reason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<ErrorCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sidecar_error_code: Option<ErrorCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_status: Option<ResponseStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_utf8_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requested_output_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requested_total_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requested_total_overflow: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_total_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transformations: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hashed_files: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hashed_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache: Option<InventoryCache>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_reason: Option<InventoryCacheReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_status: Option<PromptCacheStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_reason: Option<PromptCacheReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached_prefix_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    computed_suffix_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256_backend: Option<Sha256Backend>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256_hardware_support: Option<HardwareSupport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reused: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    spawn: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    startup_warmup_active_when_queued: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    startup_warmup_variant: Option<StartupWarmupVariant>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warmup_info_observed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warmup_tool_call_observed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warmup_completion_text_seen: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warmup_prompt_evaluation_observed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warmup_observed_prompt_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warmup_stream_drained: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warmup_evaluation_synchronized: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warmup_completion_reason: Option<WarmupCompletionReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    load: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    degraded: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cleanup_status: Option<CleanupStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delivery_outcome: Option<DeliveryOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_id: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_revision: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_metric_source: Option<MetricSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_metric_scope: Option<PromptMetricScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prepare_metric_source: Option<MetricSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_visible_chunk_metric_source: Option<MetricSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_drain_metric_source: Option<MetricSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    database_timestamp_source: Option<DatabaseTimestampSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    database_timestamp_precision: Option<DatabaseTimestampPrecision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_clock: Option<DurationClock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wait_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    startup_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    validation_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    load_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_metric_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_metric_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_lookup_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_build_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_copy_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_restore_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_prepare_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_plan_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_plan_timings: Option<CleanupCachePlanTimings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cleanup_runtime_diagnostics: Option<CleanupRuntimeDiagnostics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prepare_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_visible_chunk_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_drain_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    elapsed_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    db_write_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    clipboard_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state_write_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_total_ms: Option<f64>,
}

pub(crate) struct EventBuilder {
    record: DiagnosticRecord,
}

pub(crate) fn event(event: EventName, severity: Severity, target: Target) -> EventBuilder {
    EventBuilder {
        record: DiagnosticRecord {
            schema_version: 1,
            timestamp_unix_ms: None,
            event,
            severity,
            target,
            request_id: None,
            local_asr_operation: None,
            source_history_id: None,
            revision_id: None,
            child_generation: None,
            session_generation: None,
            file_index: None,
            phase: None,
            outcome: None,
            stage: None,
            terminal_code: None,
            reason: None,
            error_code: None,
            sidecar_error_code: None,
            response_status: None,
            message_count: None,
            input_utf8_bytes: None,
            prompt_tokens: None,
            requested_output_tokens: None,
            requested_total_tokens: None,
            requested_total_overflow: None,
            max_total_tokens: None,
            generation_tokens: None,
            transformations: None,
            file_count: None,
            hashed_files: None,
            hashed_bytes: None,
            cache: None,
            cache_reason: None,
            prompt_cache_status: None,
            prompt_cache_reason: None,
            prompt_cache_bytes: None,
            cached_prefix_tokens: None,
            computed_suffix_tokens: None,
            sha256_backend: None,
            sha256_hardware_support: None,
            reused: None,
            spawn: None,
            startup_warmup_active_when_queued: None,
            startup_warmup_variant: None,
            warmup_info_observed: None,
            warmup_tool_call_observed: None,
            warmup_completion_text_seen: None,
            warmup_prompt_evaluation_observed: None,
            warmup_observed_prompt_tokens: None,
            warmup_stream_drained: None,
            warmup_evaluation_synchronized: None,
            warmup_completion_reason: None,
            load: None,
            degraded: None,
            cleanup_status: None,
            delivery_outcome: None,
            model_id: None,
            model_revision: None,
            prompt_metric_source: None,
            prompt_metric_scope: None,
            prepare_metric_source: None,
            first_visible_chunk_metric_source: None,
            generation_drain_metric_source: None,
            database_timestamp_source: None,
            database_timestamp_precision: None,
            duration_clock: Some(DurationClock::RustMonotonic),
            wait_ms: None,
            startup_ms: None,
            validation_ms: None,
            load_ms: None,
            prompt_metric_ms: None,
            generation_metric_ms: None,
            prompt_cache_lookup_ms: None,
            prompt_cache_build_ms: None,
            prompt_cache_copy_ms: None,
            prompt_cache_restore_ms: None,
            model_prepare_ms: None,
            cache_plan_ms: None,
            cache_plan_timings: None,
            cleanup_runtime_diagnostics: None,
            prepare_ms: None,
            first_visible_chunk_ms: None,
            generation_drain_ms: None,
            elapsed_ms: None,
            db_write_ms: None,
            clipboard_ms: None,
            state_write_ms: None,
            request_total_ms: None,
        },
    }
}

impl EventBuilder {
    pub(crate) fn cleanup_phase_timings(
        mut self,
        value: Option<&CleanupRuntimeDiagnostics>,
    ) -> Self {
        if let Some(facts) = value {
            if facts.is_bounded_and_valid() {
                self.record.model_prepare_ms = facts
                    .model_prepare_seconds
                    .and_then(valid_duration)
                    .and_then(|seconds| valid_duration(seconds * 1_000.0));
                self.record.cache_plan_ms = facts
                    .cache_plan_seconds
                    .and_then(valid_duration)
                    .and_then(|seconds| valid_duration(seconds * 1_000.0));
            }
            self.record.cache_plan_timings = facts
                .cache_plan_timings
                .as_ref()
                .filter(|timings| timings.is_bounded_and_valid())
                .cloned();
        }
        self
    }

    pub(crate) fn cleanup_runtime_diagnostics(
        mut self,
        value: Option<&CleanupRuntimeDiagnostics>,
    ) -> Self {
        self.record.cleanup_runtime_diagnostics = value
            .filter(|facts| facts.is_bounded_and_valid())
            .cloned()
            .map(|mut facts| {
                // Lightweight cache-plan timings are emitted at the event top level by
                // cleanup_phase_timings, so the slow-only resource block stays non-duplicative.
                facts.cache_plan_timings = None;
                facts
            });
        self
    }

    pub(crate) fn request_id(mut self, value: &str) -> Self {
        self.record.request_id = safe_identifier(value);
        self
    }

    pub(crate) fn local_asr_operation(mut self, value: LocalAsrOperation) -> Self {
        self.record.local_asr_operation = Some(value);
        self
    }

    pub(crate) fn source_history_id(mut self, value: Option<i64>) -> Self {
        self.record.source_history_id = value;
        self
    }

    pub(crate) fn revision_id(mut self, value: i64) -> Self {
        self.record.revision_id = Some(value);
        self
    }

    pub(crate) fn optional_revision_id(mut self, value: Option<i64>) -> Self {
        self.record.revision_id = value;
        self
    }

    pub(crate) fn child_generation(mut self, value: u64) -> Self {
        self.record.child_generation = Some(value);
        self
    }

    pub(crate) fn session_generation(mut self, value: Option<u64>) -> Self {
        self.record.session_generation = value;
        self
    }

    pub(crate) fn phase(mut self, value: &str) -> Self {
        self.record.phase = match value {
            "begin" => Some(Phase::Begin),
            "terminal" => Some(Phase::Terminal),
            "dispatched" => Some(Phase::Dispatched),
            _ => None,
        };
        self
    }

    pub(crate) fn outcome(mut self, value: &str) -> Self {
        self.record.outcome = Some(match value {
            "accepted" => Outcome::Accepted,
            "acquired" => Outcome::Acquired,
            "cancelled" => Outcome::Cancelled,
            "cancelled_or_abandoned" => Outcome::CancelledOrAbandoned,
            "completed" => Outcome::Completed,
            "error" => Outcome::Error,
            "failed" => Outcome::Failed,
            "loaded" => Outcome::Loaded,
            "persistence_failed" => Outcome::PersistenceFailed,
            "ready" => Outcome::Ready,
            "rejected" => Outcome::Rejected,
            "response_received" => Outcome::ResponseReceived,
            "reused" => Outcome::Reused,
            "missed" => Outcome::Missed,
            "skipped" => Outcome::Skipped,
            "saved" => Outcome::Saved,
            "timeout" => Outcome::Timeout,
            "updated" => Outcome::Updated,
            "verified" => Outcome::Verified,
            _ => Outcome::Failed,
        });
        self
    }

    pub(crate) fn stage(mut self, value: &str) -> Self {
        self.record.stage = Some(match value {
            "cancellation" => Stage::Cancellation,
            "child_shutdown" => Stage::ChildShutdown,
            "child_startup" => Stage::ChildStartup,
            "clipboard" => Stage::Clipboard,
            "exchange" => Stage::Exchange,
            "files_validated" => Stage::FilesValidated,
            "finalizer" => Stage::Finalizer,
            "generation" => Stage::Generation,
            "handshake" => Stage::Handshake,
            "inference_exchange" => Stage::InferenceExchange,
            "inference_terminal" => Stage::InferenceTerminal,
            "inventory_verification" => Stage::InventoryVerification,
            "model_admission" => Stage::ModelAdmission,
            "model_load" => Stage::ModelLoad,
            "model_readiness" => Stage::ModelReadiness,
            "platform_admission" => Stage::PlatformAdmission,
            "prompt_admission" => Stage::PromptAdmission,
            "protocol" => Stage::Protocol,
            "queue" => Stage::Queue,
            "request_total" => Stage::RequestTotal,
            "revision" => Stage::Revision,
            "revision_state" => Stage::RevisionState,
            "sidecar_session" => Stage::SidecarSession,
            "timeout" => Stage::Timeout,
            _ => Stage::Unknown,
        });
        self
    }

    pub(crate) fn local_asr_terminal_code(mut self, value: LocalAsrTerminalCode) -> Self {
        self.record.terminal_code = Some(value);
        self
    }

    pub(crate) fn reason(mut self, value: &str) -> Self {
        self.record.reason = Some(match value {
            "cleanup_disabled" => Reason::CleanupDisabled,
            "configuration_changed" => Reason::ConfigurationChanged,
            "handshake_rejected" => Reason::HandshakeRejected,
            "input_too_large" => Reason::InputTooLarge,
            "model_not_installed" => Reason::ModelNotInstalled,
            "not_onboarded" => Reason::NotOnboarded,
            "preservation_guard" => Reason::PreservationGuard,
            "provider_not_native" => Reason::ProviderNotNative,
            "spawn_failed" => Reason::SpawnFailed,
            "local_route_only" => Reason::LocalRouteOnly,
            "verification_rejected" => Reason::VerificationRejected,
            _ => Reason::Unknown,
        });
        self
    }

    pub(crate) fn error_code(mut self, value: &str) -> Self {
        self.record.error_code = Some(ErrorCode::parse(value));
        self
    }

    pub(crate) fn sidecar_error_code(mut self, value: &str) -> Self {
        self.record.sidecar_error_code = Some(ErrorCode::parse(value));
        self
    }

    pub(crate) fn response_status(mut self, value: &str) -> Self {
        self.record.response_status = Some(match value {
            "completed" => ResponseStatus::Completed,
            "loaded" => ResponseStatus::Loaded,
            "ready" => ResponseStatus::Ready,
            _ => ResponseStatus::Other,
        });
        self
    }

    pub(crate) fn message_facts(mut self, count: usize, input_utf8_bytes: usize) -> Self {
        self.record.message_count = Some(count);
        self.record.input_utf8_bytes = Some(input_utf8_bytes);
        self
    }

    pub(crate) fn token_budget(
        mut self,
        prompt_tokens: usize,
        requested_output_tokens: usize,
        requested_total_tokens: Option<usize>,
        max_total_tokens: usize,
    ) -> Self {
        self.record.prompt_tokens = Some(prompt_tokens);
        self.record.requested_output_tokens = Some(requested_output_tokens);
        self.record.requested_total_tokens = requested_total_tokens;
        self.record.requested_total_overflow = Some(requested_total_tokens.is_none());
        self.record.max_total_tokens = Some(max_total_tokens);
        self
    }

    pub(crate) fn generation_tokens(mut self, value: usize) -> Self {
        self.record.generation_tokens = Some(value);
        self
    }

    pub(crate) fn transformations(mut self, value: usize) -> Self {
        self.record.transformations = Some(value);
        self
    }

    pub(crate) fn prompt_tokens(mut self, value: usize) -> Self {
        self.record.prompt_tokens = Some(value);
        self
    }

    pub(crate) fn hashed_counts(mut self, files: usize, bytes: u64) -> Self {
        self.record.hashed_files = Some(files);
        self.record.hashed_bytes = Some(bytes);
        self
    }

    pub(crate) fn file_hash_facts(
        mut self,
        index: usize,
        verified: bool,
        hashed_bytes: u64,
        elapsed_ms: f64,
    ) -> Self {
        self.record.file_index = u8::try_from(index).ok().filter(|index| *index < 15);
        self.record.outcome = Some(if verified {
            Outcome::Verified
        } else {
            Outcome::Failed
        });
        self.record.hashed_bytes = Some(hashed_bytes);
        self.record.elapsed_ms = valid_duration(elapsed_ms);
        self
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "inventory event fields have distinct units and privacy-safe meanings"
    )]
    pub(crate) fn inventory(
        mut self,
        cache: &str,
        cache_reason: &str,
        file_count: usize,
        hashed_files: usize,
        hashed_bytes: u64,
        backend: &str,
        hardware: Option<bool>,
    ) -> Self {
        self.record.cache = Some(match cache {
            "hit" => InventoryCache::Hit,
            _ => InventoryCache::Miss,
        });
        self.record.cache_reason = Some(match cache_reason {
            "durable_cache_missing" => InventoryCacheReason::DurableCacheMissing,
            "durable_cache_directory_invalid" => InventoryCacheReason::DurableCacheDirectoryInvalid,
            "durable_cache_permissions_invalid" => {
                InventoryCacheReason::DurableCachePermissionsInvalid
            }
            "durable_cache_target_unsupported" => {
                InventoryCacheReason::DurableCacheTargetUnsupported
            }
            "durable_identity_changed" => InventoryCacheReason::DurableIdentityChanged,
            "durable_identity_match" => InventoryCacheReason::DurableIdentityMatch,
            "durable_proof_malformed" => InventoryCacheReason::DurableProofMalformed,
            "durable_proof_missing" => InventoryCacheReason::DurableProofMissing,
            "durable_proof_permissions_or_size_invalid" => {
                InventoryCacheReason::DurableProofPermissionsOrSizeInvalid
            }
            "durable_proof_schema_changed" => InventoryCacheReason::DurableProofSchemaChanged,
            "durable_proof_unreadable" => InventoryCacheReason::DurableProofUnreadable,
            "durable_proof_write_failed" => InventoryCacheReason::DurableProofWriteFailed,
            "installed_inventory_changed_or_unreadable" => {
                InventoryCacheReason::InstalledInventoryChangedOrUnreadable
            }
            "managed_root_changed" => InventoryCacheReason::ManagedRootChanged,
            "process_cache_empty" => InventoryCacheReason::ProcessCacheEmpty,
            "trusted_file_identity_match" => InventoryCacheReason::TrustedFileIdentityMatch,
            _ => InventoryCacheReason::Unknown,
        });
        self.record.file_count = Some(file_count);
        self.record.hashed_files = Some(hashed_files);
        self.record.hashed_bytes = Some(hashed_bytes);
        self.record.sha256_backend = Some(match backend {
            "sha2_aarch64_intrinsics" => Sha256Backend::Sha2Aarch64Intrinsics,
            "sha2_software_fallback" => Sha256Backend::Sha2SoftwareFallback,
            "sha2_target_default" => Sha256Backend::Sha2TargetDefault,
            _ => Sha256Backend::Unknown,
        });
        self.record.sha256_hardware_support = Some(match hardware {
            Some(true) => HardwareSupport::Available,
            Some(false) => HardwareSupport::Unavailable,
            None => HardwareSupport::Unknown,
        });
        self
    }

    pub(crate) fn model_identity(mut self, id: &'static str, revision: &'static str) -> Self {
        self.record.model_id = Some(id);
        self.record.model_revision = Some(revision);
        self
    }

    pub(crate) fn reused_model(mut self, reused: bool, spawn: bool, load: bool) -> Self {
        self.record.reused = Some(reused);
        self.record.spawn = Some(spawn);
        self.record.load = Some(load);
        self
    }

    pub(crate) fn spawned_child(mut self, spawned: bool) -> Self {
        self.record.spawn = Some(spawned);
        self
    }

    pub(crate) fn startup_warmup_active_when_queued(mut self, active: bool) -> Self {
        self.record.startup_warmup_active_when_queued = Some(active);
        self
    }

    pub(crate) fn startup_warmup_variant(mut self, value: &str) -> Self {
        self.record.startup_warmup_variant = match value {
            "code_implicit_live" => Some(StartupWarmupVariant::CodeImplicitLive),
            "code_history_reclean" => Some(StartupWarmupVariant::CodeHistoryReclean),
            "general_implicit_live" => Some(StartupWarmupVariant::GeneralImplicitLive),
            _ => None,
        };
        self
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "warmup acceptance records each optional sidecar observation independently"
    )]
    pub(crate) fn warmup_terminal_facts(
        mut self,
        info_observed: Option<bool>,
        tool_call_observed: Option<bool>,
        completion_text_seen: Option<bool>,
        prompt_evaluation_observed: Option<bool>,
        observed_prompt_tokens: Option<usize>,
        generation_tokens: Option<usize>,
        completion: Option<&str>,
        stream_drained: Option<bool>,
        evaluation_synchronized: Option<bool>,
    ) -> Self {
        self.record.warmup_info_observed = info_observed;
        self.record.warmup_tool_call_observed = tool_call_observed;
        self.record.warmup_completion_text_seen = completion_text_seen;
        self.record.warmup_prompt_evaluation_observed = prompt_evaluation_observed;
        self.record.warmup_observed_prompt_tokens = observed_prompt_tokens;
        self.record.generation_tokens = generation_tokens;
        self.record.warmup_completion_reason = completion.map(|reason| match reason {
            "stop" => WarmupCompletionReason::Stop,
            "length" => WarmupCompletionReason::Length,
            "cancelled" => WarmupCompletionReason::Cancelled,
            _ => WarmupCompletionReason::Unknown,
        });
        self.record.warmup_stream_drained = stream_drained;
        self.record.warmup_evaluation_synchronized = evaluation_synchronized;
        self
    }

    pub(crate) fn sidecar_generation_metrics(
        mut self,
        prompt_metric_ms: f64,
        generation_metric_ms: f64,
        prepare_ms: Option<f64>,
        first_visible_chunk_ms: Option<f64>,
        generation_drain_ms: Option<f64>,
        prompt_time_scope: Option<&str>,
    ) -> Self {
        self.record.prompt_metric_source = Some(MetricSource::MlxGenerationInfo);
        self.record.prompt_metric_ms = valid_duration(prompt_metric_ms);
        self.record.generation_metric_ms = valid_duration(generation_metric_ms);
        self.record.prompt_metric_scope = prompt_time_scope.map(|scope| match scope {
            "full_prompt" => PromptMetricScope::FullPrompt,
            "suffix_after_prefix_build" => PromptMetricScope::SuffixAfterPrefixBuild,
            _ => PromptMetricScope::ComputedSuffix,
        });
        self.record.prepare_metric_source = prepare_ms.map(|_| MetricSource::SidecarMonotonic);
        self.record.prepare_ms = prepare_ms.and_then(valid_duration);
        self.record.first_visible_chunk_metric_source =
            first_visible_chunk_ms.map(|_| MetricSource::SidecarMonotonic);
        self.record.first_visible_chunk_ms = first_visible_chunk_ms.and_then(valid_duration);
        self.record.generation_drain_metric_source =
            generation_drain_ms.map(|_| MetricSource::SidecarMonotonic);
        self.record.generation_drain_ms = generation_drain_ms.and_then(valid_duration);
        self
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "cache metrics preserve separate token, byte, and phase timing facts"
    )]
    pub(crate) fn prompt_cache_metrics(
        mut self,
        status: &str,
        reason: Option<&str>,
        snapshot_bytes: u64,
        cached_prefix_tokens: usize,
        computed_suffix_tokens: usize,
        lookup_ms: Option<f64>,
        build_ms: Option<f64>,
        copy_ms: Option<f64>,
        restore_ms: Option<f64>,
    ) -> Self {
        self.record.prompt_cache_status = Some(match status {
            "built" => PromptCacheStatus::Built,
            "memory_hit" => PromptCacheStatus::MemoryHit,
            "snapshot_hit" => PromptCacheStatus::SnapshotHit,
            _ => PromptCacheStatus::Bypassed,
        });
        self.record.prompt_cache_reason = reason.map(|value| match value {
            "boundary_not_provided" => PromptCacheReason::BoundaryNotProvided,
            "cache_key_changed" => PromptCacheReason::CacheKeyChanged,
            "snapshot_corrupt" => PromptCacheReason::SnapshotCorrupt,
            "snapshot_missing" => PromptCacheReason::SnapshotMissing,
            "snapshot_too_large" => PromptCacheReason::SnapshotTooLarge,
            "snapshot_write_failed" => PromptCacheReason::SnapshotWriteFailed,
            "prefix_below_prefill_step" => PromptCacheReason::PrefixBelowPrefillStep,
            "unsupported_cache_state" => PromptCacheReason::UnsupportedCacheState,
            _ => PromptCacheReason::Unknown,
        });
        self.record.prompt_cache_bytes = Some(snapshot_bytes);
        self.record.cached_prefix_tokens = Some(cached_prefix_tokens);
        self.record.computed_suffix_tokens = Some(computed_suffix_tokens);
        self.record.prompt_cache_lookup_ms = lookup_ms.and_then(valid_duration);
        self.record.prompt_cache_build_ms = build_ms.and_then(valid_duration);
        self.record.prompt_cache_copy_ms = copy_ms.and_then(valid_duration);
        self.record.prompt_cache_restore_ms = restore_ms.and_then(valid_duration);
        self
    }

    pub(crate) fn degraded(mut self, value: bool) -> Self {
        self.record.degraded = Some(value);
        self
    }

    pub(crate) fn cleanup_status(mut self, value: &str) -> Self {
        self.record.cleanup_status = Some(match value {
            "ai_failed_local" => CleanupStatus::AiFailedLocal,
            "ai_failed_raw" => CleanupStatus::AiFailedRaw,
            "ai_success" => CleanupStatus::AiSuccess,
            "local_only" => CleanupStatus::LocalOnly,
            "preservation_guard" => CleanupStatus::PreservationGuard,
            "snippet_bypass" => CleanupStatus::SnippetBypass,
            _ => CleanupStatus::Unknown,
        });
        self
    }

    pub(crate) fn delivery_outcome(mut self, value: &str) -> Self {
        self.record.delivery_outcome = Some(match value {
            "clipboard_changed" => DeliveryOutcome::ClipboardChanged,
            "clipboard_ownership_unverified" => DeliveryOutcome::ClipboardOwnershipUnverified,
            "clipboard_write_failed" => DeliveryOutcome::ClipboardWriteFailed,
            "copied" => DeliveryOutcome::Copied,
            _ => DeliveryOutcome::NotAttemptedDegraded,
        });
        self
    }

    pub(crate) fn elapsed_ms(mut self, value: f64) -> Self {
        self.record.elapsed_ms = valid_duration(value);
        self
    }

    pub(crate) fn wait_ms(mut self, value: f64) -> Self {
        self.record.wait_ms = valid_duration(value);
        self
    }

    pub(crate) fn startup_ms(mut self, value: f64) -> Self {
        self.record.startup_ms = valid_duration(value);
        self
    }

    pub(crate) fn validation_ms(mut self, value: f64) -> Self {
        self.record.validation_ms = valid_duration(value);
        self
    }

    pub(crate) fn load_ms(mut self, value: f64) -> Self {
        self.record.load_ms = valid_duration(value);
        self
    }

    pub(crate) fn db_write_ms(mut self, value: f64) -> Self {
        self.record.db_write_ms = valid_duration(value);
        self
    }

    pub(crate) fn clipboard_ms(mut self, value: Option<f64>) -> Self {
        self.record.clipboard_ms = value.and_then(valid_duration);
        self
    }

    pub(crate) fn state_write_ms(mut self, value: Option<f64>) -> Self {
        self.record.state_write_ms = value.and_then(valid_duration);
        self
    }

    pub(crate) fn request_total_ms(mut self, value: f64) -> Self {
        self.record.request_total_ms = valid_duration(value);
        self
    }

    pub(crate) fn history_timestamp_facts(mut self) -> Self {
        self.record.database_timestamp_source = Some(DatabaseTimestampSource::SqliteDatetimeNow);
        self.record.database_timestamp_precision = Some(DatabaseTimestampPrecision::Seconds);
        self.record.duration_clock = Some(DurationClock::RustMonotonic);
        self
    }

    pub(crate) fn emit(self) {
        // Diagnostics are best effort. In particular, a logging backend or
        // sink callback must not unwind through cleanup, delivery, or Drop.
        let _ = best_effort_observer(|| self.emit_inner());
    }

    fn emit_inner(mut self) {
        #[cfg(test)]
        PANIC_NEXT_EMIT_FOR_TEST.with(|panic_next| {
            if panic_next.replace(false) {
                panic!("injected MLX diagnostic emitter panic");
            }
        });
        self.record.timestamp_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok());
        let Ok(line) = serde_json::to_string(&self.record) else {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                warn_sink_unavailable_once();
            }));
            return;
        };
        if line.len().saturating_add(1) > MAX_EVENT_BYTES {
            warn_sink_unavailable_once();
            return;
        }
        #[cfg(test)]
        {
            let selected_sequence = PANIC_EVENTS_FOR_TEST.with(|selector| {
                let mut selector = selector.borrow_mut();
                selector
                    .iter()
                    .position(|event| {
                        std::mem::discriminant(event) == std::mem::discriminant(&self.record.event)
                    })
                    .map(|index| {
                        selector.remove(index);
                    })
                    .is_some()
            });
            let selected_phase = PANIC_EVENT_PHASE_FOR_TEST.with(|selector| {
                if selector.get().is_some_and(|(event, phase)| {
                    std::mem::discriminant(&event) == std::mem::discriminant(&self.record.event)
                        && self.record.phase == Some(phase)
                }) {
                    selector.set(None);
                    true
                } else {
                    false
                }
            });
            let selected_event = PANIC_EVENT_FOR_TEST.with(|selector| {
                if selector.get().is_some_and(|event| {
                    std::mem::discriminant(&event) == std::mem::discriminant(&self.record.event)
                }) {
                    selector.set(None);
                    true
                } else {
                    false
                }
            });
            let selected = selected_sequence || selected_phase || selected_event;
            if selected {
                PANICKED_EMIT_RECORDS_FOR_TEST.with(|slot| {
                    if let Some(records) = slot.borrow().as_ref() {
                        records
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(line.clone());
                    }
                });
                panic!("injected selected MLX diagnostic emitter panic");
            }
        }
        #[cfg(test)]
        EMIT_RECORDS_FOR_TEST.with(|slot| {
            if let Some(records) = slot.borrow().as_ref() {
                records
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(line.clone());
            }
        });
        let writer = test_writer_override().or_else(|| {
            let writer = WRITER.get()?;
            let writer = writer.try_lock().ok()?;
            writer.as_ref().cloned()
        });
        let Some(writer) = writer else {
            warn_sink_unavailable_once();
            return;
        };
        if writer
            .try_send((self.record.severity, self.record.target, line))
            .is_err()
        {
            warn_sink_unavailable_once();
        }
    }
}

fn test_writer_override() -> Option<SyncSender<(Severity, Target, String)>> {
    #[cfg(test)]
    {
        TEST_WRITER_OVERRIDE.with(|writer| writer.borrow().clone())
    }
    #[cfg(not(test))]
    {
        None
    }
}

fn drain_writer(
    receiver: std::sync::mpsc::Receiver<(Severity, Target, String)>,
    mut write: impl FnMut(Severity, Target, String),
) {
    while let Ok((severity, target, line)) = receiver.recv() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            write(severity, target, line);
        }));
        if result.is_err() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                warn_sink_unavailable_once();
            }));
        }
    }
}

fn spawn_diagnostic_writer(
    receiver: std::sync::mpsc::Receiver<(Severity, Target, String)>,
    write: impl FnMut(Severity, Target, String) + Send + 'static,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn_blocking(move || drain_writer(receiver, write))
}

fn best_effort_observer<T>(observer: impl FnOnce() -> T) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(observer)).ok()
}

fn valid_duration(value: f64) -> Option<f64> {
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn safe_identifier(value: &str) -> Option<String> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    valid.then(|| value.to_owned())
}

impl ErrorCode {
    fn parse(value: &str) -> Self {
        match value {
            "cancelled" => Self::Cancelled,
            "generation_failed" => Self::GenerationFailed,
            "incomplete_generation" | "generation_incomplete" => Self::IncompleteGeneration,
            "input_too_large" => Self::InputTooLarge,
            "invalid_request" => Self::InvalidRequest,
            "invalid_response" => Self::InvalidResponse,
            "model_load_failed" => Self::ModelLoadFailed,
            "model_mismatch" | "model_version_mismatch" => Self::ModelMismatch,
            "model_not_loaded" => Self::ModelNotLoaded,
            "model_unavailable" | "invalid_model" => Self::ModelUnavailable,
            "retained_pending" => Self::RetainedPending,
            "response_too_large" => Self::ResponseTooLarge,
            "sidecar_missing" => Self::SidecarMissing,
            "timeout" => Self::Timeout,
            "unsupported_platform" => Self::UnsupportedPlatform,
            _ => Self::Unknown,
        }
    }
}

enum SinkState {
    Uninitialized,
    Ready(FileSink),
    Disabled,
}

struct FileSink {
    active: PathBuf,
    backup: PathBuf,
    max_file_bytes: u64,
    max_event_bytes: usize,
}

impl FileSink {
    fn for_app_data_dir(app_data_dir: &Path) -> io::Result<Self> {
        let directory = app_data_dir.join("diagnostics");
        fs::create_dir_all(&directory)?;
        let metadata = fs::symlink_metadata(&directory)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "diagnostic destination is not a regular directory",
            ));
        }
        set_private_directory_permissions(&directory)?;
        Ok(Self::with_limits(
            directory.join(ACTIVE_FILE),
            MAX_FILE_BYTES,
            MAX_EVENT_BYTES,
        ))
    }

    fn with_limits(active: PathBuf, max_file_bytes: u64, max_event_bytes: usize) -> Self {
        let mut backup = active.as_os_str().to_os_string();
        backup.push(".1");
        Self {
            active,
            backup: PathBuf::from(backup),
            max_file_bytes,
            max_event_bytes,
        }
    }

    fn append(&mut self, line: &[u8]) -> io::Result<()> {
        let line_bytes = line.len().saturating_add(1);
        if line_bytes > self.max_event_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "structured diagnostic event exceeds its byte limit",
            ));
        }
        let current_bytes = match fs::symlink_metadata(&self.active) {
            Ok(metadata) if metadata.file_type().is_file() => metadata.len(),
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "diagnostic destination is not a regular file",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error),
        };
        if current_bytes.saturating_add(line_bytes as u64) > self.max_file_bytes {
            self.rotate(current_bytes)?;
        }
        if let Some(parent) = self.active.parent() {
            if !parent.is_dir() {
                fs::create_dir_all(parent)?;
                set_private_directory_permissions(parent)?;
            }
        }
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&self.active)?;
        set_private_permissions(&file)?;
        let mut record = Vec::with_capacity(line_bytes);
        record.extend_from_slice(line);
        record.push(b'\n');
        file.write_all(&record)
    }

    fn rotate(&self, current_bytes: u64) -> io::Result<()> {
        remove_if_present(&self.backup)?;
        match fs::symlink_metadata(&self.active) {
            Ok(metadata) if !metadata.file_type().is_file() => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "diagnostic destination is not a regular file",
            )),
            Ok(_) if current_bytes <= self.max_file_bytes => {
                fs::rename(&self.active, &self.backup)?;
                set_private_path_permissions(&self.backup)
            }
            Ok(_) => fs::remove_file(&self.active),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn set_private_permissions(file: &fs::File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Ok(())
    }
}

fn set_private_path_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn set_private_directory_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

pub(crate) fn initialize(app_data_dir: &Path) {
    let state = SINK.get_or_init(|| Mutex::new(SinkState::Uninitialized));
    let mut state = lock_recover(state);
    if !matches!(&*state, SinkState::Uninitialized) {
        return;
    }
    match FileSink::for_app_data_dir(app_data_dir) {
        Ok(sink) => *state = SinkState::Ready(sink),
        Err(_) => {
            *state = SinkState::Disabled;
            warn_sink_unavailable_once();
        }
    }
    if WRITER.get().is_none() {
        let (sender, receiver) = sync_channel::<(Severity, Target, String)>(WRITER_QUEUE_CAPACITY);
        let task = spawn_diagnostic_writer(receiver, |severity, target, line| {
            match severity {
                Severity::Debug => log::debug!(target: target.logger_target(), "{line}"),
                Severity::Info => log::info!(target: target.logger_target(), "{line}"),
                Severity::Warn => log::warn!(target: target.logger_target(), "{line}"),
            }
            append_to_global_sink(line.as_bytes());
        });
        let _ = WRITER.set(Mutex::new(Some(sender)));
        let _ = WRITER_TASK.set(Mutex::new(Some(task)));
    }
}

pub(crate) async fn shutdown_writer() {
    if let Some(writer) = WRITER.get() {
        if let Ok(mut writer) = writer.lock() {
            writer.take();
        }
    }
    let task = WRITER_TASK.get().and_then(|task| task.lock().ok()?.take());
    if let Some(task) = task {
        if task.await.is_err() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                warn_sink_unavailable_once();
            }));
        }
    }
    let warning = WARNING_TASK.get().and_then(|task| task.lock().ok()?.take());
    if let Some(warning) = warning {
        let _ = warning.await;
    }
}

fn append_to_global_sink(line: &[u8]) {
    let Some(state) = SINK.get() else {
        warn_sink_unavailable_once();
        return;
    };
    let mut state = lock_recover(state);
    match &mut *state {
        SinkState::Ready(sink) => append_best_effort(sink, line),
        SinkState::Disabled => {}
        SinkState::Uninitialized => warn_sink_unavailable_once(),
    }
}

fn append_best_effort(sink: &mut FileSink, line: &[u8]) {
    if sink.append(line).is_err() {
        warn_sink_unavailable_once();
    }
}

fn warn_sink_unavailable_once() {
    if !WARNED_SINK_UNAVAILABLE.swap(true, Ordering::AcqRel) {
        let task = tauri::async_runtime::spawn_blocking(|| {
            #[cfg(test)]
            WARNING_ATTEMPTS_FOR_TEST.fetch_add(1, Ordering::Relaxed);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                #[cfg(test)]
                if let Some(test_backend) = lock_recover(&WARNING_BACKEND_FOR_TEST).take() {
                    test_backend();
                    return;
                }
                log::warn!(
                    target: "voiceflow::mlx::diagnostics",
                    "local MLX diagnostic event could not be recorded; retaining cleanup behavior"
                );
            }));
        });
        let tasks = WARNING_TASK.get_or_init(|| Mutex::new(None));
        *lock_recover(tasks) = Some(task);
    }
}

fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_directory() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after the Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("voiceflow-mlx-diagnostics-{unique}"))
    }

    #[test]
    fn diagnostics_record_does_not_block_async_executor() {
        use std::sync::mpsc::channel;
        use std::thread;
        use std::time::Duration;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build current-thread executor");
        let (writer, receiver) = sync_channel(WRITER_QUEUE_CAPACITY);
        let (entered_tx, entered_rx) = channel();
        let (release_tx, release_rx) = channel();
        let (written_tx, written_rx) = channel();
        let worker = spawn_diagnostic_writer(receiver, move |_, _, _| {
            let _ = entered_tx.send(());
            release_rx.recv().expect("test releases diagnostic sink");
            written_tx.send(()).expect("test remains to observe write");
        });

        let (emit_done_tx, emit_done_rx) = channel();
        let emit_thread = thread::spawn(move || {
            TEST_WRITER_OVERRIDE.with(|slot| slot.replace(Some(writer)));
            event(EventName::CleanupQueue, Severity::Info, Target::Runtime).emit();
            TEST_WRITER_OVERRIDE.with(|slot| slot.replace(None));
            let _ = emit_done_tx.send(());
        });

        let outcome = runtime.block_on(async {
            let (progress_tx, progress_rx) = tokio::sync::oneshot::channel();
            let progressed = tokio::spawn(progress_rx);
            let worker_entered = tokio::time::timeout(
                Duration::from_secs(5),
                tokio::task::spawn_blocking(move || entered_rx.recv()),
            )
            .await
            .is_ok_and(|result| result.is_ok_and(|received| received.is_ok()));
            let emitted = tokio::time::timeout(
                Duration::from_secs(5),
                tokio::task::spawn_blocking(move || emit_done_rx.recv()),
            )
            .await
            .is_ok_and(|result| result.is_ok_and(|received| received.is_ok()));
            let progress_released = progress_tx.send(()).is_ok();
            let future_progressed = progress_released
                && tokio::time::timeout(Duration::from_secs(5), progressed)
                    .await
                    .is_ok_and(|result| result.is_ok_and(|value| value.is_ok()));
            (worker_entered, emitted, future_progressed)
        });

        // Always release the blocked sink and close the queue before joining owners,
        // including when one of the watchdog observations failed.
        let _ = release_tx.send(());
        drop(emit_thread.join());
        let drained = written_rx.recv_timeout(Duration::from_secs(5)).is_ok();
        let worker_joined = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), worker)
                .await
                .is_ok_and(|result| result.is_ok())
        });
        assert!(outcome.0, "writer reached the controlled blocking sink");
        assert!(
            outcome.1,
            "production emit returned while the sink was held"
        );
        assert!(
            outcome.2,
            "another async future progressed while the sink was held"
        );
        assert!(drained, "closing the writer drains the queued event");
        assert!(worker_joined, "closed writer worker joins cleanly");
    }

    #[test]
    fn diagnostic_writer_full_closed_and_panicking_sink_are_best_effort() {
        use std::panic::{catch_unwind, AssertUnwindSafe};
        let _guard = DIAGNOSTIC_WARNING_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        WARNED_SINK_UNAVAILABLE.store(false, Ordering::Release);

        let (full_sender, _full_receiver) = sync_channel(0);
        TEST_WRITER_OVERRIDE.with(|slot| slot.replace(Some(full_sender)));
        let full = catch_unwind(AssertUnwindSafe(|| {
            event(EventName::CleanupQueue, Severity::Info, Target::Runtime).emit();
        }));
        TEST_WRITER_OVERRIDE.with(|slot| slot.replace(None));
        assert!(
            full.is_ok(),
            "full writer queue does not unwind through emit"
        );

        let (closed_sender, closed_receiver) = sync_channel(1);
        drop(closed_receiver);
        TEST_WRITER_OVERRIDE.with(|slot| slot.replace(Some(closed_sender)));
        let closed = catch_unwind(AssertUnwindSafe(|| {
            event(EventName::CleanupQueue, Severity::Info, Target::Runtime).emit();
        }));
        TEST_WRITER_OVERRIDE.with(|slot| slot.replace(None));
        assert!(closed.is_ok(), "closed writer does not unwind through emit");

        let (sender, receiver) = sync_channel(1);
        TEST_WRITER_OVERRIDE.with(|slot| slot.replace(Some(sender)));
        let emitted = catch_unwind(AssertUnwindSafe(|| {
            event(EventName::CleanupQueue, Severity::Info, Target::Runtime).emit();
        }));
        let sender = TEST_WRITER_OVERRIDE.with(|slot| slot.replace(None));
        drop(sender);
        assert!(emitted.is_ok(), "sink panic is isolated from the caller");

        let worker = spawn_diagnostic_writer(receiver, |_, _, _| {
            panic!("injected local diagnostic sink panic");
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build current-thread executor");
        let joined = runtime.block_on(worker);
        assert!(
            joined.is_ok(),
            "per-record sink panic does not panic the worker"
        );
        if let Some(warning) = WARNING_TASK.get().and_then(|task| task.lock().ok()?.take()) {
            assert!(runtime.block_on(warning).is_ok());
        }
        WARNED_SINK_UNAVAILABLE.store(false, Ordering::Release);
    }

    #[test]
    fn diagnostic_writer_survives_event_and_warning_sink_panics() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::Arc;
        let _guard = DIAGNOSTIC_WARNING_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        WARNED_SINK_UNAVAILABLE.store(false, Ordering::Release);
        *lock_recover(&WARNING_BACKEND_FOR_TEST) = Some(Box::new(|| {
            panic!("injected warning backend panic inside blocking worker");
        }));
        let (sender, receiver) = sync_channel(4);
        sender
            .send((Severity::Info, Target::Runtime, "first".to_owned()))
            .unwrap();
        sender
            .send((Severity::Info, Target::Runtime, "second".to_owned()))
            .unwrap();
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&attempts);
        let worker = spawn_diagnostic_writer(receiver, move |_, _, _| {
            observed.fetch_add(1, Ordering::Relaxed);
            panic!("injected event and warning sink panic");
        });
        drop(sender);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build current-thread executor");
        let joined = runtime.block_on(worker);
        assert!(joined.is_ok(), "warning panic is contained by the worker");
        assert!(
            attempts.load(Ordering::Relaxed) >= 2,
            "worker handles a later event after the first event and warning panic"
        );
        if let Some(warning) = WARNING_TASK.get().and_then(|task| task.lock().ok()?.take()) {
            assert!(runtime.block_on(warning).is_ok());
        }
        assert!(lock_recover(&WARNING_BACKEND_FOR_TEST).is_none());
        WARNED_SINK_UNAVAILABLE.store(false, Ordering::Release);
    }

    #[test]
    fn full_diagnostic_queue_warns_off_caller_and_joins_warning_task() {
        use std::future::Future as _;
        use std::sync::mpsc::channel;
        use std::time::Duration;
        let _guard = DIAGNOSTIC_WARNING_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        WARNED_SINK_UNAVAILABLE.store(false, Ordering::Release);
        WARNING_ATTEMPTS_FOR_TEST.store(0, Ordering::Release);
        let (entered_tx, entered_rx) = channel();
        let (release_tx, release_rx) = channel();
        *lock_recover(&WARNING_BACKEND_FOR_TEST) = Some(Box::new(move || {
            let _ = entered_tx.send(());
            release_rx.recv().expect("test releases warning backend");
        }));
        let (sender, _receiver) = sync_channel(0);
        TEST_WRITER_OVERRIDE.with(|slot| slot.replace(Some(sender)));
        let emitted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            event(EventName::CleanupQueue, Severity::Info, Target::Runtime).emit();
        }));
        TEST_WRITER_OVERRIDE.with(|slot| slot.replace(None));
        assert!(emitted.is_ok(), "full queue failure returns through emit");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build current-thread executor");
        assert!(entered_rx.recv_timeout(Duration::from_secs(5)).is_ok());
        let mut close = Box::pin(shutdown_writer());
        let close_pending = runtime.block_on(async {
            std::future::poll_fn(|context| match close.as_mut().poll(context) {
                std::task::Poll::Pending => std::task::Poll::Ready(true),
                std::task::Poll::Ready(()) => std::task::Poll::Ready(false),
            })
            .await
        });
        let future_progressed = runtime.block_on(async {
            let (progress_tx, progress_rx) = tokio::sync::oneshot::channel();
            let progress_task = tokio::spawn(async move {
                let _ = progress_tx.send(());
            });
            let progressed = tokio::time::timeout(Duration::from_secs(5), progress_rx)
                .await
                .is_ok_and(|result| result.is_ok());
            let _ = progress_task.await;
            progressed
        });
        let _ = release_tx.send(());
        let close_joined = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), close.as_mut())
                .await
                .is_ok()
        });
        assert!(
            close_pending,
            "shutdown waits while the warning backend is held"
        );
        assert!(
            future_progressed,
            "another current-thread future progresses while warning is held"
        );
        assert!(close_joined, "shutdown joins the owned warning worker");
        assert_eq!(WARNING_ATTEMPTS_FOR_TEST.load(Ordering::Acquire), 1);
        assert!(lock_recover(&WARNING_BACKEND_FOR_TEST).is_none());
        WARNED_SINK_UNAVAILABLE.store(false, Ordering::Release);
    }

    #[test]
    fn panicking_diagnostic_observer_is_best_effort() {
        let result = best_effort_observer(|| -> &'static str { panic!("injected observer panic") });
        assert!(result.is_none());
        assert_eq!(
            best_effort_observer(|| "cleanup continues"),
            Some("cleanup continues")
        );
    }

    #[test]
    fn local_asr_stage_events_keep_failed_and_abandoned_outcomes_content_free() {
        let failed = event(EventName::LocalAsrStage, Severity::Warn, Target::Runtime)
            .request_id("vf-4321-7-1782412345678901234")
            .local_asr_operation(LocalAsrOperation::Transcribe)
            .phase("terminal")
            .stage("model_load")
            .outcome("failed")
            .model_identity("qwen3-asr-0.6b", "revision-1")
            .elapsed_ms(1250.5)
            .spawned_child(true);
        let failed_json = serde_json::to_string(&failed.record).expect("typed failure event");
        for fact in [
            "\"event\":\"local_asr_stage\"",
            "\"local_asr_operation\":\"transcribe\"",
            "\"stage\":\"model_load\"",
            "\"outcome\":\"failed\"",
            "\"model_id\":\"qwen3-asr-0.6b\"",
            "\"model_revision\":\"revision-1\"",
            "\"spawn\":true",
        ] {
            assert!(failed_json.contains(fact));
        }

        let abandoned = event(EventName::LocalAsrStage, Severity::Info, Target::Runtime)
            .request_id("vf-4321-7-1782412345678901234")
            .local_asr_operation(LocalAsrOperation::Preload)
            .phase("terminal")
            .stage("inference_exchange")
            .outcome("cancelled_or_abandoned")
            .model_identity("qwen3-asr-0.6b", "revision-1")
            .elapsed_ms(9.25);
        let abandoned_json = serde_json::to_string(&abandoned.record).expect("typed abandoned");
        assert!(abandoned_json.contains("\"outcome\":\"cancelled_or_abandoned\""));
        for serialized in [&failed_json, &abandoned_json] {
            for forbidden in [
                "transcript",
                "prompt_text",
                "audio_bytes",
                "audio_path",
                "model_directory",
                "raw_error",
                "api_key",
            ] {
                assert!(!serialized.contains(forbidden));
            }
            assert!(serialized.len() < MAX_EVENT_BYTES);
        }
    }

    #[test]
    fn local_asr_stage_event_correlates_only_when_session_generation_is_owned() {
        let correlated = event(EventName::LocalAsrStage, Severity::Info, Target::Runtime)
            .request_id("vf-test")
            .session_generation(Some(41))
            .local_asr_operation(LocalAsrOperation::Transcribe)
            .phase("terminal")
            .stage("queue")
            .outcome("completed");
        let correlated = serde_json::to_string(&correlated.record).expect("trace event");
        assert!(correlated.contains("\"session_generation\":41"));

        let standalone = event(EventName::LocalAsrStage, Severity::Info, Target::Runtime)
            .request_id("vf-test")
            .session_generation(None)
            .local_asr_operation(LocalAsrOperation::Preload)
            .phase("terminal")
            .stage("queue")
            .outcome("completed");
        let standalone = serde_json::to_string(&standalone.record).expect("standalone event");
        assert!(!standalone.contains("session_generation"));
    }

    #[test]
    fn local_asr_terminal_code_is_omitted_when_unset() {
        let record = event(EventName::LocalAsrStage, Severity::Warn, Target::Runtime)
            .request_id("vf-4321-7-1782412345678901234")
            .local_asr_operation(LocalAsrOperation::Transcribe)
            .phase("terminal")
            .stage("inference_terminal")
            .outcome("failed");
        let serialized = serde_json::to_string(&record.record).expect("unset terminal code");
        assert!(!serialized.contains("\"terminal_code\""));
    }

    #[test]
    fn local_asr_terminal_code_serializes_only_a_fixed_enum_value() {
        let record = event(EventName::LocalAsrStage, Severity::Warn, Target::Runtime)
            .request_id("vf-4321-7-1782412345678901234")
            .local_asr_operation(LocalAsrOperation::Transcribe)
            .phase("terminal")
            .stage("inference_terminal")
            .outcome("failed")
            .local_asr_terminal_code(LocalAsrTerminalCode::LocalEmptyOutputNotAccepted);
        let serialized = serde_json::to_string(&record.record).expect("typed terminal code");
        assert!(serialized.contains("\"terminal_code\":\"local_empty_output_not_accepted\""));
        for forbidden in ["raw_error", "api_key", "audio_path", "transcript"] {
            assert!(!serialized.contains(forbidden));
        }
    }

    #[test]
    fn typed_event_serializes_only_allowlisted_metadata() {
        let builder = event(
            EventName::CleanupGeneration,
            Severity::Info,
            Target::Runtime,
        )
        .request_id("12345678-1234-1234-1234-123456789abc")
        .source_history_id(Some(12))
        .child_generation(2)
        .generation_tokens(4)
        .sidecar_generation_metrics(
            17.0,
            40.0,
            Some(2.0),
            Some(5.0),
            Some(39.0),
            Some("full_prompt"),
        );
        let builder = builder.prompt_cache_metrics(
            "snapshot_hit",
            None,
            123_456,
            512,
            200,
            Some(2.0),
            None,
            Some(0.5),
            Some(1.0),
        );
        let serialized = serde_json::to_string(&builder.record)
            .expect("typed event contains only serializable metadata");
        assert!(serialized.contains("\"event\":\"cleanup_generation\""));
        assert!(serialized.contains("\"first_visible_chunk_ms\":5.0"));
        assert!(serialized.contains("\"prompt_cache_status\":\"snapshot_hit\""));
        assert!(serialized.contains("\"cached_prefix_tokens\":512"));
        assert!(serialized.contains("\"computed_suffix_tokens\":200"));
        for forbidden in ["transcript", "prompt_text", "body", "secret", "path"] {
            assert!(!serialized.contains(forbidden));
        }
    }

    #[test]
    fn cleanup_resource_facts_are_bounded_and_contain_no_text() {
        let facts: CleanupRuntimeDiagnostics = serde_json::from_value(serde_json::json!({
            "model_prepare_seconds": 1.5,
            "cache_plan_seconds": 0.25,
            "cache_plan_timings": {
                "candidate_prepare_count": 4,
                "candidate_prepare_milliseconds": 40.0,
                "stable_user_prefix_prepare_count": 2,
                "stable_user_prefix_prepare_milliseconds": 25.0,
                "system_only_prepare_count": 2,
                "system_only_prepare_milliseconds": 15.0,
                "perform_to_closure_entry_milliseconds": 5.0,
                "mlx_generate_call_milliseconds": 50.0
            },
            "phases": [{
                "phase": "before_prepare",
                "elapsed_seconds": 0.0,
                "mlx_active_memory_bytes": 1024,
                "mlx_cache_memory_bytes": 2048,
                "mlx_peak_memory_bytes": 4096,
                "mlx_memory_limit_bytes": 8192,
                "device": "gpu",
                "task_priority": "user_initiated",
                "thread_qos": "user_initiated",
                "thermal_state": "nominal",
                "process_user_cpu_seconds": 1.25,
                "process_system_cpu_seconds": 0.25,
                "process_minor_page_faults": 12,
                "process_major_page_faults": 1,
                "process_page_ins": 3,
                "process_physical_footprint_bytes": 16384
            }]
        }))
        .expect("runtime facts are typed metadata");
        let builder = event(
            EventName::CleanupGeneration,
            Severity::Info,
            Target::Runtime,
        )
        .cleanup_phase_timings(Some(&facts))
        .cleanup_runtime_diagnostics(Some(&facts));
        let serialized = serde_json::to_string(&builder.record)
            .expect("resource facts serialize within the diagnostic allowlist");
        assert!(serialized.contains("\"phase\":\"before_prepare\""));
        assert!(serialized.contains("\"process_physical_footprint_bytes\":16384"));
        assert!(serialized.contains("\"model_prepare_ms\":1500.0"));
        assert!(serialized.contains("\"cache_plan_ms\":250.0"));
        assert!(serialized.contains("\"candidate_prepare_count\":4"));
        assert!(serialized.contains("\"mlx_generate_call_milliseconds\":50.0"));
        assert!(serialized.len() < MAX_EVENT_BYTES);
        for forbidden in ["transcript", "prompt_text", "secret", "api_key", "path"] {
            assert!(!serialized.contains(forbidden));
        }

        let oversized = CleanupRuntimeDiagnostics {
            model_prepare_seconds: facts.model_prepare_seconds,
            cache_plan_seconds: facts.cache_plan_seconds,
            cache_plan_timings: facts.cache_plan_timings.clone(),
            phases: vec![facts.phases[0].clone(); 5],
        };
        let suppressed = event(
            EventName::CleanupGeneration,
            Severity::Info,
            Target::Runtime,
        )
        .cleanup_phase_timings(Some(&oversized))
        .cleanup_runtime_diagnostics(Some(&oversized));
        assert!(suppressed.record.cleanup_runtime_diagnostics.is_none());
        assert!(suppressed.record.model_prepare_ms.is_none());
        assert!(suppressed.record.cache_plan_ms.is_none());

        let fast_runtime_diagnostics = slow_cleanup_runtime_diagnostics(4.972_192, Some(&facts));
        assert!(fast_runtime_diagnostics.is_none());
        let fast = event(
            EventName::CleanupGeneration,
            Severity::Info,
            Target::Runtime,
        )
        .cleanup_phase_timings(Some(&facts))
        .cleanup_runtime_diagnostics(fast_runtime_diagnostics);
        let fast_json = serde_json::to_string(&fast.record).expect("numeric timing facts");
        assert!(fast_json.contains("\"model_prepare_ms\":1500.0"));
        assert!(fast_json.contains("\"cache_plan_ms\":250.0"));
        assert!(fast_json.contains("\"cache_plan_timings\":{"));
        assert!(fast_json.contains("\"candidate_prepare_count\":4"));
        assert!(!fast_json.contains("\"phases\""));
        assert!(!fast_json.contains("\"cleanup_runtime_diagnostics\""));
    }

    #[test]
    fn memory_hit_cache_plan_timings_fit_event_budget_and_invalid_values_suppress() {
        let facts: CleanupRuntimeDiagnostics = serde_json::from_value(serde_json::json!({
            "cache_plan_timings": {
                "candidate_prepare_count": 4,
                "candidate_prepare_milliseconds": 40.0,
                "stable_user_prefix_prepare_count": 2,
                "stable_user_prefix_prepare_milliseconds": 25.0,
                "system_only_prepare_count": 2,
                "system_only_prepare_milliseconds": 15.0,
                "perform_to_closure_entry_milliseconds": 5.0,
                "mlx_generate_call_milliseconds": 50.0
            },
            "phases": [{
                "phase": "before_prepare",
                "elapsed_seconds": 0.0,
                "mlx_active_memory_bytes": 1024,
                "mlx_cache_memory_bytes": 2048,
                "mlx_peak_memory_bytes": 4096,
                "mlx_memory_limit_bytes": 8192,
                "device": "gpu",
                "task_priority": "user_initiated",
                "thread_qos": "user_initiated",
                "thermal_state": "nominal",
                "process_user_cpu_seconds": 1.25,
                "process_system_cpu_seconds": 0.25,
                "process_minor_page_faults": 12,
                "process_major_page_faults": 1,
                "process_page_ins": 3,
                "process_physical_footprint_bytes": 16384
            }]
        }))
        .expect("bounded staged timings deserialize");
        assert!(facts.is_bounded_and_valid());
        let full_facts = CleanupRuntimeDiagnostics {
            model_prepare_seconds: Some(1.5),
            cache_plan_seconds: Some(0.25),
            cache_plan_timings: facts.cache_plan_timings.clone(),
            phases: vec![facts.phases[0].clone(); 4],
        };
        let full_event = event(
            EventName::CleanupGeneration,
            Severity::Info,
            Target::Runtime,
        )
        .prompt_cache_metrics(
            "memory_hit",
            None,
            0,
            467,
            170,
            Some(0.000_292),
            None,
            Some(0.14),
            None,
        )
        .cleanup_phase_timings(Some(&full_facts))
        .cleanup_runtime_diagnostics(Some(&full_facts));
        let full_json = serde_json::to_string(&full_event.record)
            .expect("memory-hit event serializes within the existing allowlist");
        assert!(full_json.len() < MAX_EVENT_BYTES);
        assert!(full_json.contains("\"prompt_cache_status\":\"memory_hit\""));
        assert!(full_json.contains("\"cached_prefix_tokens\":467"));
        assert!(full_json.contains("\"computed_suffix_tokens\":170"));
        assert!(full_json.contains("\"candidate_prepare_count\":4"));
        for forbidden in ["transcript", "prompt_text", "api_key", "model_directory"] {
            assert!(!full_json.contains(forbidden));
        }

        let invalid_facts = CleanupRuntimeDiagnostics {
            model_prepare_seconds: Some(1.0),
            cache_plan_seconds: Some(2.0),
            cache_plan_timings: Some(CleanupCachePlanTimings {
                candidate_prepare_count: 5,
                candidate_prepare_milliseconds: f64::INFINITY,
                stable_user_prefix_prepare_count: 3,
                stable_user_prefix_prepare_milliseconds: 1_000.0,
                system_only_prepare_count: 2,
                system_only_prepare_milliseconds: 1_000.0,
                perform_to_closure_entry_milliseconds: Some(f64::NAN),
                mlx_generate_call_milliseconds: Some(1_000.0),
            }),
            phases: Vec::new(),
        };
        assert!(invalid_facts.is_bounded_and_valid());
        assert!(!invalid_facts
            .cache_plan_timings
            .as_ref()
            .unwrap()
            .is_bounded_and_valid());
        let invalid_event = event(
            EventName::CleanupGeneration,
            Severity::Info,
            Target::Runtime,
        )
        .cleanup_phase_timings(Some(&invalid_facts))
        .cleanup_runtime_diagnostics(Some(&invalid_facts));
        assert!(invalid_event.record.cleanup_runtime_diagnostics.is_some());
        assert!(invalid_event.record.cache_plan_timings.is_none());
        assert_eq!(invalid_event.record.model_prepare_ms, Some(1_000.0));
        assert_eq!(invalid_event.record.cache_plan_ms, Some(2_000.0));
        let invalid_json = serde_json::to_string(&invalid_event.record)
            .expect("invalid new timings are omitted before event serialization");
        assert!(!invalid_json.contains("cache_plan_timings"));
        assert!(invalid_json.contains("\"model_prepare_ms\":1000.0"));
    }

    #[test]
    fn warmup_terminal_diagnostics_preserve_empty_output_facts_without_content() {
        let builder = event(EventName::StartupWarmup, Severity::Warn, Target::Runtime)
            .request_id("12345678-1234-1234-1234-123456789abc")
            .startup_warmup_variant("code_history_reclean")
            .warmup_terminal_facts(
                Some(true),
                Some(false),
                Some(false),
                Some(true),
                Some(899),
                Some(0),
                Some("stop"),
                Some(true),
                Some(true),
            );
        let serialized = serde_json::to_string(&builder.record)
            .expect("warmup event contains only bounded terminal metadata");
        assert!(serialized.contains("\"startup_warmup_variant\":\"code_history_reclean\""));
        assert!(serialized.contains("\"warmup_info_observed\":true"));
        assert!(serialized.contains("\"warmup_completion_text_seen\":false"));
        assert!(serialized.contains("\"warmup_observed_prompt_tokens\":899"));
        assert!(serialized.contains("\"generation_tokens\":0"));
        assert!(serialized.contains("\"warmup_completion_reason\":\"stop\""));
        assert!(serialized.contains("\"warmup_stream_drained\":true"));
        assert!(serialized.contains("\"warmup_evaluation_synchronized\":true"));
        assert!(!serialized.contains("transcript"));

        let general = event(EventName::StartupWarmup, Severity::Info, Target::Runtime)
            .startup_warmup_variant("general_implicit_live");
        let general_serialized = serde_json::to_string(&general.record)
            .expect("General warmup uses the fixed startup variant allowlist");
        assert!(general_serialized.contains("\"startup_warmup_variant\":\"general_implicit_live\""));
    }

    #[test]
    fn sink_rotation_and_event_limit_bound_both_files() {
        let directory = test_directory();
        fs::create_dir_all(&directory).expect("create isolated diagnostics test directory");
        let active = directory.join(ACTIVE_FILE);
        let mut sink = FileSink::with_limits(active.clone(), 32, 16);
        for _ in 0..4 {
            sink.append(b"123456789012345")
                .expect("bounded record fits");
        }
        let backup = directory.join(format!("{ACTIVE_FILE}.1"));
        let active_bytes = fs::metadata(&active).expect("active log exists").len();
        let backup_bytes = fs::metadata(&backup).expect("rotated log exists").len();
        assert!(active_bytes <= 32);
        assert!(backup_bytes <= 32);
        assert!(active_bytes + backup_bytes <= 64);
        assert!(sink.append(b"1234567890123456").is_err());
        assert_eq!(
            fs::metadata(&active).expect("active remains").len(),
            active_bytes
        );
        fs::remove_dir_all(directory).expect("remove isolated diagnostics test directory");
    }

    #[test]
    fn failed_file_write_is_best_effort_and_does_not_escape_emitter() {
        let directory = test_directory();
        fs::create_dir_all(&directory).expect("create isolated diagnostics test directory");
        let blocker = directory.join("regular-file");
        fs::write(&blocker, b"blocker").expect("create file that cannot be a parent");
        let mut sink = FileSink::with_limits(blocker.join(ACTIVE_FILE), 32, 16);
        let emitted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            append_best_effort(&mut sink, b"{}");
        }));
        assert!(emitted.is_ok());
        assert_eq!(
            fs::read(&blocker).expect("diagnostic failure leaves other data alone"),
            b"blocker"
        );
        fs::remove_dir_all(directory).expect("remove isolated diagnostics test directory");
    }
}
