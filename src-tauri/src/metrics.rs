//! Bounded, process-local performance diagnostics.
//!
//! Samples contain only stage names, configured provider/model labels, fixed
//! outcome codes, and monotonic durations. Transcript text, audio, context,
//! endpoints, credentials, screenshots, and target identities are never kept.

use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
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
    StopToInsert,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryOutcome {
    PasteConfirmed,
    PasteUnconfirmed,
    Copied,
    HistoryOnly,
    PreviewOnly,
    Failed,
    Cancelled,
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
    pub stop_to_insert: LatencySummary,
    pub delivery: DeliveryCounts,
    pub asr_usage: AsrUsageSummary,
    pub fallback_reasons: Vec<NamedCount>,
    pub error_reasons: Vec<NamedCount>,
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
    stop_to_insert: VecDeque<u64>,
    cleanup_guard_fallbacks: usize,
    groups: BTreeMap<MetricGroup, GroupSamples>,
}

#[derive(Default)]
struct GroupSamples {
    latencies: BTreeMap<MetricKind, VecDeque<u64>>,
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
        Self { metrics, group }
    }

    pub fn record_request(&self, duration_secs: Option<f64>) {
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
        stop_to_insert: summary(MetricKind::StopToInsert),
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
}

impl StopToInsertTimer {
    pub fn set_group(&mut self, group: MetricGroup) {
        self.group = group;
    }

    pub fn finish(mut self, outcome: DeliveryOutcome, reason: Option<&'static str>) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.metrics
            .record_delivery(&self.group, outcome, reason, self.started.elapsed());
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
        self.metrics.record_delivery(
            &self.group,
            outcome,
            Some(if outcome == DeliveryOutcome::Cancelled {
                "cancelled"
            } else {
                "processing_failed"
            }),
            self.started.elapsed(),
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
}
