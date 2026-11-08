//! Bounded, process-local performance diagnostics.
//!
//! Samples contain only stage names, configured provider/model labels, fixed
//! outcome codes, and monotonic durations. Transcript text, audio, context,
//! endpoints, credentials, screenshots, and target identities are never kept.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const MAX_SAMPLES_PER_STAGE: usize = 128;
const MAX_GROUPS: usize = 32;
const MAX_LABEL_CHARS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    PrefetchAsr,
    FinalAsr,
    /// Compatibility alias for callers that have not been migrated yet.
    #[allow(dead_code, reason = "Retained for the existing internal metrics API.")]
    Asr,
    Cleanup,
    Validation,
    Paste,
    PasteSubmission,
    ReadbackConfirmation,
    HistoryJournal,
    CaptureSpoolFlush,
    RetainedHistory,
    StopToInsert,
}

/// Coarse, content-free stage boundaries used by the shared dictation trace.
#[allow(
    dead_code,
    reason = "Consumed by the follow-on ASR and cleanup trace instrumentation."
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    AsrInference,
    FileValidation,
    RuntimeStartup,
    ModelLoad,
    PromptPreparation,
    QueueWait,
    MlxGateWait,
    Prefill,
    Decode,
    FinalGuard,
    Paste,
    TargetConfirmation,
    LmStudioStatusProbe,
}

/// The single coarse class assigned from observed session facts by trace owners.
#[allow(
    dead_code,
    reason = "Consumed by the follow-on trace classification owners."
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DictationClass {
    Unclassified,
    Cold,
    FirstAfterLaunch,
    Warm,
    Long,
    Contention,
    FirstAfterIdleUnload,
}

/// Process-local trace payload. Stage offsets and durations are milliseconds
/// from the owning host monotonic clock; child-process clocks are not mixed in.
#[allow(
    dead_code,
    reason = "Consumed by the follow-on ASR and cleanup trace instrumentation."
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DictationTrace {
    pub session_generation: u64,
    pub stages: Vec<(Stage, u64, u64)>,
    pub reported_stages: Vec<ReportedStageDuration>,
    pub cache_status: Option<CacheStatus>,
    pub class: DictationClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheStatus {
    MemoryHit,
    SnapshotHit,
    Bypassed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportedScope {
    FullPrompt,
    SuffixAfterPrefixBuild,
    ComputedSuffix,
    Generation,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub struct ReportedStageDuration {
    pub stage: Stage,
    pub duration_ms: u64,
    pub scope: ReportedScope,
}

const MAX_TRACE_SPANS: usize = 256;

#[derive(Debug, Clone)]
pub struct TraceHandle {
    state: Arc<Mutex<TraceState>>,
    terminal: Arc<AtomicBool>,
}

#[derive(Debug)]
struct TraceState {
    origin: Instant,
    payload: DictationTrace,
    sealed_at: Option<Instant>,
    next_span_id: u64,
    open_spans: BTreeMap<u64, (Stage, Instant)>,
}

#[derive(Debug)]
pub struct TraceSpan {
    handle: TraceHandle,
    span_id: Option<u64>,
}

impl TraceHandle {
    fn try_seal_snapshot_at(&self, now: Instant) -> Option<DictationTrace> {
        // The terminal fence is independent from the payload mutex so a busy
        // trace can never leave late child callbacks authorized to mutate it.
        self.terminal.store(true, Ordering::Release);
        let mut state = match self.state.try_lock() {
            Ok(state) => state,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        if state.sealed_at.is_none() {
            state.sealed_at = Some(now);
            let origin = state.origin;
            let open = std::mem::take(&mut state.open_spans);
            for (_, (stage, started)) in open {
                push_trace_span(&mut state, stage, started, now, origin);
            }
        }
        Some(state.payload.clone())
    }

    pub fn session_generation(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .payload
            .session_generation
    }

    pub fn begin(&self, stage: Stage) -> TraceSpan {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let span_id = if !self.terminal.load(Ordering::Acquire)
            && state.sealed_at.is_none()
            && state.payload.stages.len()
                + state.payload.reported_stages.len()
                + state.open_spans.len()
                < MAX_TRACE_SPANS
        {
            let id = state.next_span_id;
            state.next_span_id = state.next_span_id.saturating_add(1);
            state.open_spans.insert(id, (stage, Instant::now()));
            Some(id)
        } else {
            None
        };
        TraceSpan {
            handle: self.clone(),
            span_id,
        }
    }

    pub fn snapshot(&self) -> DictationTrace {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .payload
            .clone()
    }

    /// Store a validated child-reported duration without placing it on the host clock.
    pub fn record_reported_seconds(&self, stage: Stage, seconds: f64, scope: ReportedScope) {
        if !seconds.is_finite() || seconds < 0.0 {
            return;
        }
        let millis = (seconds * 1_000.0).round();
        if !millis.is_finite() || millis < 0.0 || millis > u64::MAX as f64 {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if self.terminal.load(Ordering::Acquire)
            || state.sealed_at.is_some()
            || state.payload.stages.len()
                + state.payload.reported_stages.len()
                + state.open_spans.len()
                >= MAX_TRACE_SPANS
        {
            return;
        }
        state.payload.reported_stages.push(ReportedStageDuration {
            stage,
            duration_ms: millis as u64,
            scope,
        });
    }

    pub fn record_cache_status(&self, status: Option<&str>) {
        let value = match status {
            Some("memory_hit") => CacheStatus::MemoryHit,
            Some("snapshot_hit") => CacheStatus::SnapshotHit,
            Some("bypassed") => CacheStatus::Bypassed,
            _ => CacheStatus::Unknown,
        };
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if !self.terminal.load(Ordering::Acquire) && state.sealed_at.is_none() {
            state.payload.cache_status = Some(value);
        }
    }
}

fn push_trace_span(
    state: &mut TraceState,
    stage: Stage,
    start: Instant,
    end: Instant,
    origin: Instant,
) {
    if state.payload.stages.len() >= MAX_TRACE_SPANS {
        return;
    }
    let start_ms = start
        .saturating_duration_since(origin)
        .as_millis()
        .min(u64::MAX as u128) as u64;
    let duration_ms = end
        .saturating_duration_since(start)
        .as_millis()
        .min(u64::MAX as u128) as u64;
    let item = (stage, start_ms, duration_ms);
    let index = state
        .payload
        .stages
        .partition_point(|existing| existing.1 <= start_ms);
    state.payload.stages.insert(index, item);
}

impl TraceSpan {
    pub fn finish(&mut self) {
        let Some(id) = self.span_id.take() else {
            return;
        };
        let mut state = self.handle.state.lock().unwrap_or_else(|p| p.into_inner());
        if self.handle.terminal.load(Ordering::Acquire) || state.sealed_at.is_some() {
            return;
        }
        if let Some((stage, started)) = state.open_spans.remove(&id) {
            let now = Instant::now();
            let origin = state.origin;
            push_trace_span(&mut state, stage, started, now, origin);
        }
    }
}
impl Drop for TraceSpan {
    fn drop(&mut self) {
        self.finish();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct MetricGroup {
    pub provider: String,
    pub model: String,
    pub path: String,
}

impl MetricGroup {
    pub fn new(provider: &str, model: &str, path: &str) -> Self {
        Self {
            provider: safe_label(provider),
            model: safe_label(model),
            path: safe_label(path),
        }
    }

    pub fn from_provenance(provenance: &str, path: &str) -> Self {
        let (provider, model) = provenance
            .split_once(':')
            .unwrap_or((provenance, "unknown"));
        Self::new(provider, model, path)
    }

    pub fn unknown(path: &str) -> Self {
        Self::new("unknown", "unknown", path)
    }
}

fn safe_label(value: &str) -> String {
    if value.contains("://") || value.contains(['?', '#']) {
        return "unknown".to_owned();
    }
    let label = value
        .trim()
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '/')
        })
        .take(MAX_LABEL_CHARS)
        .collect::<String>();
    if label.is_empty() {
        "unknown".to_owned()
    } else {
        label
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryOutcome {
    PasteConfirmed,
    PasteUnconfirmed,
    Copied,
    HistoryOnly,
    PreviewOnly,
    Failed,
    Cancelled,
}

impl DeliveryOutcome {
    const ALL: [Self; 7] = [
        Self::PasteConfirmed,
        Self::PasteUnconfirmed,
        Self::Copied,
        Self::HistoryOnly,
        Self::PreviewOnly,
        Self::Failed,
        Self::Cancelled,
    ];
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct LatencySummary {
    pub sample_count: usize,
    pub p50_ms: Option<u64>,
    pub p95_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct NamedCount {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DeliveryCounts {
    pub paste_sent: usize,
    pub readback_confirmed: usize,
    pub paste_unconfirmed: usize,
    pub paste_failed: usize,
    pub paste_cancelled: usize,
    pub copied: usize,
    pub history_only: usize,
    pub preview_only: usize,
    pub failed: usize,
    pub cancelled: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DiagnosticGroupSummary {
    pub provider: String,
    pub model: String,
    pub path: String,
    pub prefetch_asr: LatencySummary,
    pub final_asr: LatencySummary,
    pub cleanup: LatencySummary,
    pub validation: LatencySummary,
    pub paste_submission: LatencySummary,
    pub readback_confirmation: LatencySummary,
    pub history_journal: LatencySummary,
    pub capture_spool_flush: LatencySummary,
    pub retained_history: LatencySummary,
    pub stop_to_insert: LatencySummary,
    pub stop_to_insert_by_outcome: Vec<OutcomeLatencySummary>,
    pub delivery: DeliveryCounts,
    pub asr_usage: AsrUsageSummary,
    pub fallback_reasons: Vec<NamedCount>,
    pub error_reasons: Vec<NamedCount>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutcomeLatencySummary {
    pub outcome: DeliveryOutcome,
    pub latency: LatencySummary,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct AsrUsageSummary {
    /// Counts actual HTTP transcription attempts, including queue retries.
    pub request_count: usize,
    pub failed_request_count: usize,
    /// Sum of decoded WAV durations submitted to the provider. Requests whose
    /// WAV duration could not be read are reflected in request_count only.
    pub submitted_audio_seconds: f64,
    pub audio_duration_request_count: usize,
    /// Soniox attempts use a realtime websocket and are kept separate from
    /// HTTP transcription request counts and returned provider usage.
    pub stream_attempt_count: usize,
    pub failed_stream_attempt_count: usize,
    /// Wall time from streaming attempt start to completion (connect, audio,
    /// and finalization). This diagnostic duration is not billed usage.
    pub stream_session_seconds: f64,
    /// Captured audio duration actually sent over the streaming websocket;
    /// this is not the provider's billed duration or returned usage.
    pub stream_audio_seconds: f64,
    /// Audio duration sent again during a complete-audio recovery replay.
    pub reprocessed_audio_seconds: f64,
    /// Provider-reported units stay separate; they are not converted to cost.
    pub returned_usage: Vec<UsageUnitTotal>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageUnitTotal {
    pub unit: &'static str,
    pub amount: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AsrUsageUnit {
    PromptAudioSeconds,
    InputAudioSeconds,
    Seconds,
    PromptTokens,
    InputTokens,
    CompletionTokens,
    OutputTokens,
    TotalTokens,
    AudioDurationMilliseconds,
    RequestTimeMilliseconds,
}

impl AsrUsageUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PromptAudioSeconds => "prompt_audio_seconds",
            Self::InputAudioSeconds => "input_audio_seconds",
            Self::Seconds => "seconds",
            Self::PromptTokens => "prompt_tokens",
            Self::InputTokens => "input_tokens",
            Self::CompletionTokens => "completion_tokens",
            Self::OutputTokens => "output_tokens",
            Self::TotalTokens => "total_tokens",
            Self::AudioDurationMilliseconds => "audio_duration_milliseconds",
            Self::RequestTimeMilliseconds => "request_time_milliseconds",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LatencyMetrics {
    pub prefetch_asr: LatencySummary,
    pub final_asr: LatencySummary,
    /// Kept for compatibility; mirrors the final ASR bucket.
    pub asr: LatencySummary,
    pub cleanup: LatencySummary,
    pub paste: LatencySummary,
    pub history_journal: LatencySummary,
    pub capture_spool_flush: LatencySummary,
    pub retained_history: LatencySummary,
    pub stop_to_insert: LatencySummary,
    pub cleanup_guard_fallbacks: usize,
    pub groups: Vec<DiagnosticGroupSummary>,
}

#[derive(Clone, Default)]
pub struct Metrics {
    samples: Arc<Mutex<MetricSamples>>,
}

#[derive(Default)]
struct MetricSamples {
    prefetch_asr: VecDeque<u64>,
    final_asr: VecDeque<u64>,
    asr: VecDeque<u64>,
    cleanup: VecDeque<u64>,
    paste: VecDeque<u64>,
    history_journal: VecDeque<u64>,
    capture_spool_flush: VecDeque<u64>,
    retained_history: VecDeque<u64>,
    stop_to_insert: VecDeque<u64>,
    cleanup_guard_fallbacks: usize,
    groups: BTreeMap<MetricGroup, GroupSamples>,
}

#[derive(Default)]
struct GroupSamples {
    latencies: BTreeMap<MetricKind, VecDeque<u64>>,
    stop_to_insert_by_outcome: BTreeMap<DeliveryOutcome, VecDeque<u64>>,
    delivery: DeliveryCounts,
    asr_usage: AsrUsageSamples,
    fallback_reasons: BTreeMap<String, usize>,
    error_reasons: BTreeMap<String, usize>,
}

#[derive(Default)]
struct AsrUsageSamples {
    request_count: usize,
    failed_request_count: usize,
    submitted_audio_seconds: f64,
    audio_duration_request_count: usize,
    stream_attempt_count: usize,
    failed_stream_attempt_count: usize,
    stream_session_seconds: f64,
    stream_audio_seconds: f64,
    reprocessed_audio_seconds: f64,
    returned_usage: BTreeMap<AsrUsageUnit, f64>,
}

impl Metrics {
    pub fn timer(&self, kind: MetricKind) -> LatencyTimer {
        self.timer_for(kind, MetricGroup::unknown("dictation"))
    }

    pub fn timer_for(&self, kind: MetricKind, group: MetricGroup) -> LatencyTimer {
        LatencyTimer {
            metrics: self.clone(),
            kind,
            group,
            started: Instant::now(),
            finished: false,
        }
    }

    pub fn stop_to_insert(
        &self,
        group: MetricGroup,
        cancellation: CancellationToken,
    ) -> StopToInsertTimer {
        StopToInsertTimer {
            metrics: self.clone(),
            group,
            cancellation,
            started: Instant::now(),
            finished: false,
            trace: None,
            terminal_handoff: None,
        }
    }

    pub fn record_cleanup_guard_fallback(&self) {
        let mut samples = self
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        samples.cleanup_guard_fallbacks = samples.cleanup_guard_fallbacks.saturating_add(1);
    }

    pub fn record_fallback(&self, group: &MetricGroup, reason: &'static str) {
        let mut samples = self
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let group = group_samples_mut(&mut samples, group);
        increment_reason(&mut group.fallback_reasons, reason);
    }

    pub fn record_error(&self, group: &MetricGroup, reason: &'static str) {
        let mut samples = self
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let group = group_samples_mut(&mut samples, group);
        increment_reason(&mut group.error_reasons, reason);
    }

    pub fn record_asr_request(&self, group: &MetricGroup, duration_secs: Option<f64>) {
        let mut samples = self
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let group = group_samples_mut(&mut samples, group);
        increment_count(&mut group.asr_usage.request_count);
        if let Some(seconds) =
            duration_secs.filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        {
            group.asr_usage.submitted_audio_seconds =
                bounded_sum(group.asr_usage.submitted_audio_seconds, seconds);
            increment_count(&mut group.asr_usage.audio_duration_request_count);
        }
    }

    pub fn record_asr_request_failure(&self, group: &MetricGroup, reason: &'static str) {
        let mut samples = self
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let group = group_samples_mut(&mut samples, group);
        increment_count(&mut group.asr_usage.failed_request_count);
        increment_reason(&mut group.error_reasons, reason);
    }

    fn record_asr_stream_attempt(&self, group: &MetricGroup) {
        let mut samples = self
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let group = group_samples_mut(&mut samples, group);
        increment_count(&mut group.asr_usage.stream_attempt_count);
    }

    fn complete_asr_stream_attempt(
        &self,
        group: &MetricGroup,
        failure: Option<&'static str>,
        connection_duration: Duration,
        audio_seconds: f64,
        reprocessed_audio_seconds: f64,
    ) {
        let mut samples = self
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let group = group_samples_mut(&mut samples, group);
        if let Some(reason) = failure {
            increment_count(&mut group.asr_usage.failed_stream_attempt_count);
            increment_reason(&mut group.error_reasons, reason);
        }
        group.asr_usage.stream_session_seconds = bounded_sum(
            group.asr_usage.stream_session_seconds,
            connection_duration.as_secs_f64(),
        );
        group.asr_usage.stream_audio_seconds =
            bounded_sum(group.asr_usage.stream_audio_seconds, audio_seconds);
        group.asr_usage.reprocessed_audio_seconds = bounded_sum(
            group.asr_usage.reprocessed_audio_seconds,
            reprocessed_audio_seconds,
        );
    }

    pub fn record_asr_usage(&self, group: &MetricGroup, unit: AsrUsageUnit, amount: f64) {
        if !amount.is_finite() || amount < 0.0 {
            return;
        }
        let mut samples = self
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let group = group_samples_mut(&mut samples, group);
        let total = group.asr_usage.returned_usage.entry(unit).or_default();
        *total = bounded_sum(*total, amount);
    }

    pub fn record_duration(&self, kind: MetricKind, group: &MetricGroup, elapsed: Duration) {
        self.observe(kind, group, elapsed);
    }

    fn observe(&self, kind: MetricKind, group: &MetricGroup, elapsed: Duration) {
        let millis = elapsed.as_millis().min(u64::MAX as u128) as u64;
        let mut samples = self
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match kind {
            MetricKind::PrefetchAsr => push_sample(&mut samples.prefetch_asr, millis),
            MetricKind::FinalAsr => {
                push_sample(&mut samples.final_asr, millis);
                push_sample(&mut samples.asr, millis);
            }
            MetricKind::Asr => push_sample(&mut samples.asr, millis),
            MetricKind::Cleanup => push_sample(&mut samples.cleanup, millis),
            MetricKind::Paste => push_sample(&mut samples.paste, millis),
            MetricKind::HistoryJournal => push_sample(&mut samples.history_journal, millis),
            MetricKind::CaptureSpoolFlush => push_sample(&mut samples.capture_spool_flush, millis),
            MetricKind::RetainedHistory => push_sample(&mut samples.retained_history, millis),
            MetricKind::PasteSubmission
            | MetricKind::Validation
            | MetricKind::ReadbackConfirmation => {}
            MetricKind::StopToInsert => push_sample(&mut samples.stop_to_insert, millis),
        }
        let group_samples = group_samples_mut(&mut samples, group);
        let bucket = group_samples.latencies.entry(kind).or_default();
        push_sample(bucket, millis);
    }

    fn record_delivery(
        &self,
        group: &MetricGroup,
        outcome: DeliveryOutcome,
        reason: Option<&'static str>,
        elapsed: Duration,
    ) {
        let mut samples = self
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let millis = elapsed.as_millis().min(u64::MAX as u128) as u64;
        push_sample(
            group_samples_mut(&mut samples, group)
                .stop_to_insert_by_outcome
                .entry(outcome)
                .or_default(),
            millis,
        );
        if outcome == DeliveryOutcome::PasteConfirmed {
            push_sample(&mut samples.stop_to_insert, millis);
        }
        let group_samples = group_samples_mut(&mut samples, group);
        match outcome {
            DeliveryOutcome::PasteConfirmed => {
                increment_count(&mut group_samples.delivery.paste_sent);
                increment_count(&mut group_samples.delivery.readback_confirmed);
                let bucket = group_samples
                    .latencies
                    .entry(MetricKind::StopToInsert)
                    .or_default();
                push_sample(bucket, millis);
            }
            DeliveryOutcome::PasteUnconfirmed => {
                increment_count(&mut group_samples.delivery.paste_sent);
                increment_count(&mut group_samples.delivery.paste_unconfirmed);
                increment_count(&mut group_samples.delivery.copied);
            }
            DeliveryOutcome::Copied => increment_count(&mut group_samples.delivery.copied),
            DeliveryOutcome::HistoryOnly => {
                increment_count(&mut group_samples.delivery.history_only)
            }
            DeliveryOutcome::PreviewOnly => {
                increment_count(&mut group_samples.delivery.preview_only)
            }
            DeliveryOutcome::Failed => {
                increment_count(&mut group_samples.delivery.failed);
                increment_reason(
                    &mut group_samples.error_reasons,
                    reason.unwrap_or("delivery_failed"),
                );
            }
            DeliveryOutcome::Cancelled => {
                increment_count(&mut group_samples.delivery.cancelled);
                increment_reason(
                    &mut group_samples.error_reasons,
                    reason.unwrap_or("cancelled"),
                );
            }
        }
        if matches!(
            outcome,
            DeliveryOutcome::Copied | DeliveryOutcome::PasteUnconfirmed
        ) {
            if let Some(reason) = reason {
                increment_reason(&mut group_samples.fallback_reasons, reason);
            }
        }
    }

    pub fn record_paste_failure(&self, group: &MetricGroup, reason: &'static str, cancelled: bool) {
        let mut samples = self
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let group = group_samples_mut(&mut samples, group);
        if cancelled {
            increment_count(&mut group.delivery.paste_cancelled);
        } else {
            increment_count(&mut group.delivery.paste_failed);
        }
        increment_reason(&mut group.error_reasons, reason);
    }

    pub fn snapshot(&self) -> LatencyMetrics {
        let samples = self
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let groups = samples
            .groups
            .iter()
            .map(|(key, group)| summarize_group(key, group))
            .collect();
        LatencyMetrics {
            prefetch_asr: summarize(&samples.prefetch_asr),
            final_asr: summarize(&samples.final_asr),
            asr: summarize(&samples.asr),
            cleanup: summarize(&samples.cleanup),
            paste: summarize(&samples.paste),
            history_journal: summarize(&samples.history_journal),
            capture_spool_flush: summarize(&samples.capture_spool_flush),
            retained_history: summarize(&samples.retained_history),
            stop_to_insert: summarize(&samples.stop_to_insert),
            cleanup_guard_fallbacks: samples.cleanup_guard_fallbacks,
            groups,
        }
    }
}

fn group_samples_mut<'a>(
    samples: &'a mut MetricSamples,
    key: &MetricGroup,
) -> &'a mut GroupSamples {
    if samples.groups.contains_key(key) {
        return samples
            .groups
            .get_mut(key)
            .expect("group was checked above");
    }
    let overflow = MetricGroup::unknown("other");
    if key != &overflow && samples.groups.len() >= MAX_GROUPS - 1 {
        return samples.groups.entry(overflow).or_default();
    }
    samples.groups.entry(key.clone()).or_default()
}

fn push_sample(samples: &mut VecDeque<u64>, value: u64) {
    if samples.len() == MAX_SAMPLES_PER_STAGE {
        samples.pop_front();
    }
    samples.push_back(value);
}

fn increment_reason(reasons: &mut BTreeMap<String, usize>, reason: &str) {
    increment_count(reasons.entry(safe_label(reason)).or_default());
}

fn increment_count(count: &mut usize) {
    *count = count.saturating_add(1);
}

fn bounded_sum(total: f64, amount: f64) -> f64 {
    (total + amount).min(1_000_000_000_000.0)
}

#[derive(Clone)]
pub struct AsrRequestDiagnostics {
    trace: Option<crate::research_trace::Trace>,
    metrics: Metrics,
    group: MetricGroup,
}

impl std::fmt::Debug for AsrRequestDiagnostics {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AsrRequestDiagnostics")
            .field("group", &self.group)
            .finish_non_exhaustive()
    }
}

impl AsrRequestDiagnostics {
    pub fn new(metrics: Metrics, group: MetricGroup) -> Self {
        Self {
            metrics,
            group,
            trace: crate::research_trace::current(),
        }
    }

    pub fn record_request(&self, duration_secs: Option<f64>) {
        crate::research_trace::asr_attempt(
            self.trace.as_ref(),
            &self.group.provider,
            &self.group.model,
            &self.group.path,
        );
        self.metrics.record_asr_request(&self.group, duration_secs);
    }

    pub fn record_usage(&self, unit: AsrUsageUnit, amount: f64) {
        self.metrics.record_asr_usage(&self.group, unit, amount);
    }

    pub fn record_failure(&self, reason: &'static str) {
        self.metrics.record_asr_request_failure(&self.group, reason);
    }

    pub fn record_issue(&self, reason: &'static str) {
        self.metrics.record_error(&self.group, reason);
    }

    pub fn begin_request(&self, duration_secs: Option<f64>) -> AsrRequestAttemptGuard {
        self.record_request(duration_secs);
        AsrRequestAttemptGuard {
            diagnostics: self.clone(),
            completed: false,
        }
    }

    pub fn begin_stream(&self) -> AsrStreamAttemptGuard {
        crate::research_trace::asr_attempt(
            self.trace.as_ref(),
            &self.group.provider,
            &self.group.model,
            &self.group.path,
        );
        self.metrics.record_asr_stream_attempt(&self.group);
        AsrStreamAttemptGuard {
            diagnostics: self.clone(),
            started_at: Instant::now(),
            completed: false,
        }
    }
}

pub struct AsrStreamAttemptGuard {
    diagnostics: AsrRequestDiagnostics,
    started_at: Instant,
    completed: bool,
}

impl AsrStreamAttemptGuard {
    pub fn complete(
        mut self,
        failure: Option<&'static str>,
        connection_duration: Duration,
        audio_seconds: f64,
        reprocessed_audio_seconds: f64,
    ) {
        self.diagnostics.metrics.complete_asr_stream_attempt(
            &self.diagnostics.group,
            failure,
            connection_duration,
            audio_seconds,
            reprocessed_audio_seconds,
        );
        self.completed = true;
    }
}

impl Drop for AsrStreamAttemptGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.diagnostics.metrics.complete_asr_stream_attempt(
                &self.diagnostics.group,
                Some("interrupted"),
                self.started_at.elapsed(),
                0.0,
                0.0,
            );
        }
    }
}

pub struct AsrRequestAttemptGuard {
    diagnostics: AsrRequestDiagnostics,
    completed: bool,
}

impl AsrRequestAttemptGuard {
    pub fn complete(mut self, failure: Option<&'static str>) {
        if let Some(reason) = failure {
            self.diagnostics.record_failure(reason);
        }
        self.completed = true;
    }
}

impl Drop for AsrRequestAttemptGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.diagnostics.record_failure("asr_interrupted");
        }
    }
}

fn summarize_group(key: &MetricGroup, group: &GroupSamples) -> DiagnosticGroupSummary {
    let summary = |kind| {
        group
            .latencies
            .get(&kind)
            .map(summarize)
            .unwrap_or_default()
    };
    DiagnosticGroupSummary {
        provider: key.provider.clone(),
        model: key.model.clone(),
        path: key.path.clone(),
        prefetch_asr: summary(MetricKind::PrefetchAsr),
        final_asr: summary(MetricKind::FinalAsr),
        cleanup: summary(MetricKind::Cleanup),
        validation: summary(MetricKind::Validation),
        paste_submission: summary(MetricKind::PasteSubmission),
        readback_confirmation: summary(MetricKind::ReadbackConfirmation),
        history_journal: summary(MetricKind::HistoryJournal),
        capture_spool_flush: summary(MetricKind::CaptureSpoolFlush),
        retained_history: summary(MetricKind::RetainedHistory),
        stop_to_insert: summary(MetricKind::StopToInsert),
        stop_to_insert_by_outcome: DeliveryOutcome::ALL
            .into_iter()
            .map(|outcome| OutcomeLatencySummary {
                outcome,
                latency: group
                    .stop_to_insert_by_outcome
                    .get(&outcome)
                    .map(summarize)
                    .unwrap_or_default(),
            })
            .collect(),
        delivery: group.delivery.clone(),
        asr_usage: AsrUsageSummary {
            request_count: group.asr_usage.request_count,
            failed_request_count: group.asr_usage.failed_request_count,
            submitted_audio_seconds: group.asr_usage.submitted_audio_seconds,
            audio_duration_request_count: group.asr_usage.audio_duration_request_count,
            stream_attempt_count: group.asr_usage.stream_attempt_count,
            failed_stream_attempt_count: group.asr_usage.failed_stream_attempt_count,
            stream_session_seconds: group.asr_usage.stream_session_seconds,
            stream_audio_seconds: group.asr_usage.stream_audio_seconds,
            reprocessed_audio_seconds: group.asr_usage.reprocessed_audio_seconds,
            returned_usage: group
                .asr_usage
                .returned_usage
                .iter()
                .map(|(unit, amount)| UsageUnitTotal {
                    unit: unit.as_str(),
                    amount: *amount,
                })
                .collect(),
        },
        fallback_reasons: named_counts(&group.fallback_reasons),
        error_reasons: named_counts(&group.error_reasons),
    }
}

fn named_counts(values: &BTreeMap<String, usize>) -> Vec<NamedCount> {
    values
        .iter()
        .map(|(name, count)| NamedCount {
            name: name.clone(),
            count: *count,
        })
        .collect()
}

pub struct LatencyTimer {
    metrics: Metrics,
    kind: MetricKind,
    group: MetricGroup,
    started: Instant,
    finished: bool,
}

impl LatencyTimer {
    #[allow(
        dead_code,
        reason = "Retained for existing internal callers and compatibility."
    )]
    pub fn finish(mut self) {
        self.record();
    }

    fn record(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.metrics
            .observe(self.kind, &self.group, self.started.elapsed());
    }
}

impl Drop for LatencyTimer {
    fn drop(&mut self) {
        // A generic timer cannot determine whether insertion was confirmed.
        // Production uses StopToInsertTimer and records that sample only after
        // target readback confirms the inserted value.
        if self.kind != MetricKind::StopToInsert {
            self.record();
        }
    }
}

pub struct StopToInsertTimer {
    metrics: Metrics,
    group: MetricGroup,
    cancellation: CancellationToken,
    started: Instant,
    finished: bool,
    trace: Option<TraceHandle>,
    terminal_handoff: Option<TerminalHandoff>,
}

type TerminalHandoff = Box<dyn FnOnce(DictationTrace, DeliveryOutcome, u64) + Send>;

impl StopToInsertTimer {
    pub(crate) fn bind_terminal_handoff(
        &mut self,
        handoff: impl FnOnce(DictationTrace, DeliveryOutcome, u64) + Send + 'static,
    ) {
        self.terminal_handoff = Some(Box::new(handoff));
    }

    fn terminal_snapshot(
        &mut self,
        outcome: DeliveryOutcome,
        terminal_at: Instant,
        elapsed_ms: u64,
    ) {
        if let Some(trace) = self.trace.take() {
            let snapshot = trace.try_seal_snapshot_at(terminal_at);
            if let (Some(handoff), Some(snapshot)) = (self.terminal_handoff.take(), snapshot) {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    handoff(snapshot, outcome, elapsed_ms)
                }));
            }
        } else {
            self.terminal_handoff.take();
        }
    }

    #[cfg(test)]
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn bind_trace(&mut self, generation: u64) -> TraceHandle {
        let trace = TraceHandle {
            state: Arc::new(Mutex::new(TraceState {
                origin: self.started,
                payload: DictationTrace {
                    session_generation: generation,
                    stages: Vec::new(),
                    reported_stages: Vec::new(),
                    cache_status: None,
                    class: DictationClass::Unclassified,
                },
                sealed_at: None,
                next_span_id: 0,
                open_spans: BTreeMap::new(),
            })),
            terminal: Arc::new(AtomicBool::new(false)),
        };
        self.trace = Some(trace.clone());
        trace
    }
    pub fn trace_handle(&self) -> Option<TraceHandle> {
        self.trace.clone()
    }

    pub fn set_group(&mut self, group: MetricGroup) {
        self.group = group;
    }

    pub fn finish(mut self, outcome: DeliveryOutcome, reason: Option<&'static str>) {
        if self.finished {
            return;
        }
        self.finished = true;
        let terminal_at = Instant::now();
        let elapsed = terminal_at.saturating_duration_since(self.started);
        let elapsed_ms = elapsed.as_millis().min(u64::MAX as u128) as u64;
        self.terminal_snapshot(outcome, terminal_at, elapsed_ms);
        self.metrics
            .record_delivery(&self.group, outcome, reason, elapsed);
    }
}

impl Drop for StopToInsertTimer {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let outcome = if self.cancellation.is_cancelled() {
            DeliveryOutcome::Cancelled
        } else {
            DeliveryOutcome::Failed
        };
        let terminal_at = Instant::now();
        let elapsed = terminal_at.saturating_duration_since(self.started);
        let elapsed_ms = elapsed.as_millis().min(u64::MAX as u128) as u64;
        self.terminal_snapshot(outcome, terminal_at, elapsed_ms);
        self.metrics.record_delivery(
            &self.group,
            outcome,
            Some(if outcome == DeliveryOutcome::Cancelled {
                "cancelled"
            } else {
                "processing_failed"
            }),
            elapsed,
        );
    }
}

fn summarize(values: &VecDeque<u64>) -> LatencySummary {
    if values.is_empty() {
        return LatencySummary::default();
    }
    let mut sorted = values.iter().copied().collect::<Vec<_>>();
    sorted.sort_unstable();
    LatencySummary {
        sample_count: sorted.len(),
        p50_ms: Some(percentile(&sorted, 50)),
        p95_ms: Some(percentile(&sorted, 95)),
    }
}

fn percentile(sorted: &[u64], percent: usize) -> u64 {
    let rank = (sorted.len() * percent).div_ceil(100);
    sorted[rank.max(1) - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lmstudio_status_probe_stage_has_strict_snake_case_wire_name() {
        let encoded = serde_json::to_string(&Stage::LmStudioStatusProbe).unwrap();
        assert_eq!(encoded, "\"lm_studio_status_probe\"");
        assert_eq!(
            serde_json::from_str::<Stage>(&encoded).unwrap(),
            Stage::LmStudioStatusProbe
        );
        assert!(serde_json::from_str::<Stage>("\"lmstudio_status_probe\"").is_err());
    }

    #[test]
    fn reports_percentiles_and_keeps_samples_bounded() {
        let metrics = Metrics::default();
        for _ in 0..(MAX_SAMPLES_PER_STAGE + 10) {
            metrics.observe(
                MetricKind::Asr,
                &MetricGroup::unknown("dictation"),
                Duration::from_millis(10),
            );
        }
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.asr.sample_count, MAX_SAMPLES_PER_STAGE);
        assert_eq!(snapshot.asr.p50_ms, Some(10));
        assert_eq!(snapshot.asr.p95_ms, Some(10));
    }

    #[test]
    fn timer_records_elapsed_work() {
        let metrics = Metrics::default();
        {
            let _timer = metrics.timer(MetricKind::Paste);
        }
        assert_eq!(metrics.snapshot().paste.sample_count, 1);
    }

    #[test]
    fn stop_to_insert_timer_is_reported_once_when_finished_explicitly() {
        let metrics = Metrics::default();
        metrics.timer(MetricKind::StopToInsert).finish();
        assert_eq!(metrics.snapshot().stop_to_insert.sample_count, 1);
    }

    #[test]
    fn slow_unconfirmed_delivery_counts_in_per_outcome_stop_to_insert() {
        let metrics = Metrics::default();
        let group = MetricGroup::unknown("dictation");
        metrics.record_delivery(
            &group,
            DeliveryOutcome::PasteUnconfirmed,
            Some("readback_mismatch"),
            Duration::from_millis(840),
        );

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.stop_to_insert.sample_count, 0);
        let serialized = serde_json::to_value(snapshot).expect("metrics serialize");
        let unconfirmed = serialized["groups"][0]["stop_to_insert_by_outcome"]
            .as_array()
            .and_then(|outcomes| {
                outcomes
                    .iter()
                    .find(|entry| entry["outcome"] == "paste_unconfirmed")
            })
            .expect("unconfirmed outcome has its own latency series");
        assert_eq!(unconfirmed["latency"]["p50_ms"], 840);
    }

    #[test]
    fn confirmed_only_stop_to_insert_unchanged() {
        let metrics = Metrics::default();
        let group = MetricGroup::unknown("dictation");
        for (outcome, millis) in [
            (DeliveryOutcome::PasteConfirmed, 10),
            (DeliveryOutcome::PasteUnconfirmed, 20),
            (DeliveryOutcome::Failed, 30),
            (DeliveryOutcome::Cancelled, 40),
            (DeliveryOutcome::PasteConfirmed, 50),
        ] {
            metrics.record_delivery(&group, outcome, None, Duration::from_millis(millis));
        }

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.stop_to_insert.sample_count, 2);
        assert_eq!(snapshot.stop_to_insert.p50_ms, Some(10));
        assert_eq!(snapshot.stop_to_insert.p95_ms, Some(50));
        assert_eq!(snapshot.groups[0].stop_to_insert.sample_count, 2);
        assert_eq!(snapshot.groups[0].stop_to_insert_by_outcome.len(), 7);
        assert_eq!(
            snapshot.groups[0]
                .stop_to_insert_by_outcome
                .iter()
                .map(|summary| summary.latency.sample_count)
                .sum::<usize>(),
            5
        );
    }

    #[test]
    fn stop_to_insert_outcome_timer_records_explicit_and_drop_once() {
        let metrics = Metrics::default();
        let group = MetricGroup::unknown("dictation");
        metrics
            .stop_to_insert(group.clone(), CancellationToken::new())
            .finish(DeliveryOutcome::Copied, None);
        drop(metrics.stop_to_insert(group, CancellationToken::new()));

        let summaries = &metrics.snapshot().groups[0].stop_to_insert_by_outcome;
        assert_eq!(
            summaries
                .iter()
                .find(|summary| summary.outcome == DeliveryOutcome::Copied)
                .unwrap()
                .latency
                .sample_count,
            1
        );
        assert_eq!(
            summaries
                .iter()
                .find(|summary| summary.outcome == DeliveryOutcome::Failed)
                .unwrap()
                .latency
                .sample_count,
            1
        );
    }

    #[test]
    fn cancelled_stop_to_insert_drop_is_counted_once_and_bounded() {
        let metrics = Metrics::default();
        let group = MetricGroup::unknown("dictation");
        let cancellation = CancellationToken::new();
        let timer = metrics.stop_to_insert(group.clone(), cancellation.clone());
        cancellation.cancel();
        drop(timer);
        for sample in 0..(MAX_SAMPLES_PER_STAGE + 2) {
            metrics.record_delivery(
                &group,
                DeliveryOutcome::PasteUnconfirmed,
                None,
                Duration::from_millis(sample as u64),
            );
        }

        let summaries = &metrics.snapshot().groups[0].stop_to_insert_by_outcome;
        assert_eq!(
            summaries
                .iter()
                .find(|summary| summary.outcome == DeliveryOutcome::Cancelled)
                .unwrap()
                .latency
                .sample_count,
            1
        );
        assert_eq!(
            summaries
                .iter()
                .find(|summary| summary.outcome == DeliveryOutcome::PasteUnconfirmed)
                .unwrap()
                .latency
                .sample_count,
            MAX_SAMPLES_PER_STAGE
        );
    }

    #[test]
    fn dictation_trace_uses_typed_content_free_stages_and_class() {
        let trace = DictationTrace {
            session_generation: 17,
            stages: vec![(Stage::QueueWait, 20, 4), (Stage::Paste, 30, 5)],
            reported_stages: Vec::new(),
            cache_status: Some(CacheStatus::SnapshotHit),
            class: DictationClass::FirstAfterLaunch,
        };
        let serialized = serde_json::to_value(trace).expect("trace serializes");
        assert_eq!(serialized["session_generation"], 17);
        assert_eq!(serialized["class"], "first_after_launch");
        assert_eq!(serialized["cache_status"], "snapshot_hit");
        assert_eq!(
            serialized["stages"][0],
            serde_json::json!(["queue_wait", 20, 4])
        );
    }

    #[test]
    fn trace_span_third_value_is_duration_not_end_offset() {
        let origin = Instant::now();
        let start = origin + Duration::from_millis(5);
        let end = origin + Duration::from_millis(15);
        let mut state = TraceState {
            origin,
            payload: DictationTrace {
                session_generation: 1,
                stages: Vec::new(),
                reported_stages: Vec::new(),
                cache_status: None,
                class: DictationClass::Unclassified,
            },
            sealed_at: None,
            next_span_id: 0,
            open_spans: BTreeMap::new(),
        };

        push_trace_span(&mut state, Stage::QueueWait, start, end, origin);

        assert_eq!(state.payload.stages, vec![(Stage::QueueWait, 5, 10)]);
    }

    #[test]
    fn reported_timing_is_bounded_validated_and_sealed_without_host_offsets() {
        let mut timer = Metrics::default()
            .stop_to_insert(MetricGroup::unknown("trace_test"), CancellationToken::new());
        let trace = timer.bind_trace(23);
        trace.record_reported_seconds(Stage::Prefill, 0.125, ReportedScope::Unknown);
        trace.record_reported_seconds(Stage::Decode, f64::NAN, ReportedScope::Generation);
        trace.record_reported_seconds(Stage::Decode, -1.0, ReportedScope::Generation);
        assert!(trace.snapshot().stages.is_empty());
        let snapshot = trace.snapshot();
        assert_eq!(snapshot.session_generation, 23);
        assert_eq!(snapshot.reported_stages.len(), 1);
        assert_eq!(snapshot.reported_stages[0].stage, Stage::Prefill);
        assert_eq!(snapshot.reported_stages[0].duration_ms, 125);
        assert_eq!(snapshot.reported_stages[0].scope, ReportedScope::Unknown);
        trace.record_cache_status(Some("unrecognized-status"));
        assert_eq!(trace.snapshot().cache_status, Some(CacheStatus::Unknown));
        for _ in 0..MAX_TRACE_SPANS {
            trace.record_reported_seconds(Stage::Decode, 0.001, ReportedScope::Generation);
        }
        let mut late = trace.begin(Stage::Paste);
        late.finish();
        let bounded = trace.snapshot();
        assert_eq!(bounded.reported_stages.len(), MAX_TRACE_SPANS);
        assert!(bounded.stages.is_empty());
        timer.finish(DeliveryOutcome::PasteUnconfirmed, None);
        trace.record_reported_seconds(Stage::Decode, 1.0, ReportedScope::Generation);
        assert_eq!(trace.snapshot().reported_stages.len(), MAX_TRACE_SPANS);

        let mut combined_timer = Metrics::default().stop_to_insert(
            MetricGroup::unknown("combined_trace_test"),
            CancellationToken::new(),
        );
        let combined = combined_timer.bind_trace(24);
        let mut host = combined.begin(Stage::QueueWait);
        host.finish();
        for _ in 0..MAX_TRACE_SPANS {
            combined.record_reported_seconds(Stage::Decode, 0.001, ReportedScope::Generation);
        }
        assert_eq!(combined.snapshot().stages.len(), 1);
        assert_eq!(
            combined.snapshot().reported_stages.len(),
            MAX_TRACE_SPANS - 1
        );
        combined_timer.finish(DeliveryOutcome::PasteUnconfirmed, None);
    }

    #[test]
    fn prefetch_and_final_asr_are_reported_separately() {
        let metrics = Metrics::default();
        let group = MetricGroup::unknown("dictation");
        metrics.observe(MetricKind::PrefetchAsr, &group, Duration::from_millis(12));
        metrics.observe(MetricKind::FinalAsr, &group, Duration::from_millis(34));
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.prefetch_asr.p50_ms, Some(12));
        assert_eq!(snapshot.final_asr.p50_ms, Some(34));
        assert_eq!(snapshot.asr.p50_ms, Some(34));
    }

    #[test]
    fn records_preservation_guard_fallbacks() {
        let metrics = Metrics::default();
        metrics.record_cleanup_guard_fallback();
        metrics.record_cleanup_guard_fallback();
        assert_eq!(metrics.snapshot().cleanup_guard_fallbacks, 2);
    }

    #[test]
    fn sealed_trace_rejects_late_generation_work() {
        let metrics = Metrics::default();
        let mut timer_n =
            metrics.stop_to_insert(MetricGroup::unknown("test"), CancellationToken::new());
        let trace_n = timer_n.bind_trace(10);
        let mut old = trace_n.begin(Stage::QueueWait);
        trace_n.try_seal_snapshot_at(Instant::now()).unwrap();
        let frozen = trace_n.snapshot();
        let mut timer_next =
            metrics.stop_to_insert(MetricGroup::unknown("test"), CancellationToken::new());
        let trace_next = timer_next.bind_trace(11);
        let mut newer = trace_next.begin(Stage::FileValidation);
        old.finish();
        drop(old);
        trace_n.record_reported_seconds(Stage::Decode, 0.25, ReportedScope::Generation);
        trace_n.record_cache_status(Some("memory_hit"));
        assert_eq!(trace_n.snapshot(), frozen);
        newer.finish();
        assert_eq!(trace_next.snapshot().session_generation, 11);
        assert_ne!(trace_next.snapshot(), frozen);
    }

    #[test]
    fn terminal_fence_seals_without_sink_and_when_payload_lock_is_busy() {
        let metrics = Metrics::default();
        let mut no_sink_timer =
            metrics.stop_to_insert(MetricGroup::unknown("no_sink"), CancellationToken::new());
        let no_sink_trace = no_sink_timer.bind_trace(31);
        no_sink_timer.finish(DeliveryOutcome::PasteUnconfirmed, None);
        no_sink_trace.record_reported_seconds(Stage::Decode, 0.25, ReportedScope::Generation);
        no_sink_trace.record_cache_status(Some("memory_hit"));
        assert!(no_sink_trace.begin(Stage::Paste).span_id.is_none());
        assert!(no_sink_trace.snapshot().reported_stages.is_empty());
        assert_eq!(no_sink_trace.snapshot().cache_status, None);

        let mut busy_timer =
            metrics.stop_to_insert(MetricGroup::unknown("busy_trace"), CancellationToken::new());
        let busy_trace = busy_timer.bind_trace(32);
        let payload_guard = busy_trace.state.lock().unwrap();
        busy_timer.finish(DeliveryOutcome::PasteUnconfirmed, None);
        drop(payload_guard);
        busy_trace.record_reported_seconds(Stage::Decode, 0.25, ReportedScope::Generation);
        busy_trace.record_cache_status(Some("snapshot_hit"));
        assert!(busy_trace.begin(Stage::Paste).span_id.is_none());
        assert!(busy_trace.snapshot().reported_stages.is_empty());
        assert_eq!(busy_trace.snapshot().cache_status, None);
    }

    #[test]
    fn terminal_handoff_uses_one_instant_for_open_span_and_elapsed() {
        let metrics = Metrics::default();
        let mut timer = metrics.stop_to_insert(
            MetricGroup::unknown("terminal_instant"),
            CancellationToken::new(),
        );
        let trace = timer.bind_trace(33);
        let _open = trace.begin(Stage::AsrInference);
        let (sender, receiver) = std::sync::mpsc::channel();
        timer.bind_terminal_handoff(move |snapshot, _, total_ms| {
            sender.send((snapshot, total_ms)).unwrap();
        });
        timer.finish(DeliveryOutcome::PasteConfirmed, None);
        let (snapshot, total_ms) = receiver.recv().unwrap();
        assert_eq!(snapshot.stages.len(), 1);
        let (_, offset_ms, duration_ms) = snapshot.stages[0];
        assert!(offset_ms + duration_ms <= total_ms);
        assert_eq!(metrics.snapshot().stop_to_insert.sample_count, 1);
    }
}
