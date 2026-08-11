//! Small, in-memory latency samples for local diagnostics.
//!
//! Metrics are intentionally bounded and never leave the process. They help
//! compare the dictation path against the product's latency goals without
//! adding telemetry or retaining user text.

use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_SAMPLES: usize = 128;

#[derive(Clone, Copy)]
pub enum MetricKind {
    PrefetchAsr,
    FinalAsr,
    /// Compatibility alias for callers that have not been migrated yet.
    #[allow(dead_code)]
    Asr,
    Cleanup,
    Paste,
    StopToInsert,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct LatencySummary {
    pub sample_count: usize,
    pub p50_ms: Option<u64>,
    pub p95_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LatencyMetrics {
    pub prefetch_asr: LatencySummary,
    pub final_asr: LatencySummary,
    /// Kept for the existing internal diagnostics consumer; it mirrors the
    /// final ASR bucket and is not a user-facing dashboard.
    pub asr: LatencySummary,
    pub cleanup: LatencySummary,
    pub paste: LatencySummary,
    pub stop_to_insert: LatencySummary,
}

#[derive(Clone, Default)]
pub struct Metrics {
    samples: Arc<Mutex<MetricSamples>>,
}

#[derive(Default)]
struct MetricSamples {
    prefetch_asr: Vec<u64>,
    final_asr: Vec<u64>,
    asr: Vec<u64>,
    cleanup: Vec<u64>,
    paste: Vec<u64>,
    stop_to_insert: Vec<u64>,
}

impl Metrics {
    pub fn timer(&self, kind: MetricKind) -> LatencyTimer {
        LatencyTimer {
            metrics: self.clone(),
            kind,
            started: Instant::now(),
            finished: false,
        }
    }

    fn observe(&self, kind: MetricKind, elapsed: Duration) {
        let millis = elapsed.as_millis().min(u64::MAX as u128) as u64;
        let mut samples = self
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let bucket = match kind {
            MetricKind::PrefetchAsr => &mut samples.prefetch_asr,
            MetricKind::FinalAsr => &mut samples.final_asr,
            MetricKind::Asr => &mut samples.asr,
            MetricKind::Cleanup => &mut samples.cleanup,
            MetricKind::Paste => &mut samples.paste,
            MetricKind::StopToInsert => &mut samples.stop_to_insert,
        };
        if bucket.len() == MAX_SAMPLES {
            bucket.remove(0);
        }
        bucket.push(millis);
        if matches!(kind, MetricKind::FinalAsr) {
            if samples.asr.len() == MAX_SAMPLES {
                samples.asr.remove(0);
            }
            samples.asr.push(millis);
        }
    }

    pub fn snapshot(&self) -> LatencyMetrics {
        let samples = self
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        LatencyMetrics {
            prefetch_asr: summarize(&samples.prefetch_asr),
            final_asr: summarize(&samples.final_asr),
            asr: summarize(&samples.asr),
            cleanup: summarize(&samples.cleanup),
            paste: summarize(&samples.paste),
            stop_to_insert: summarize(&samples.stop_to_insert),
        }
    }
}

pub struct LatencyTimer {
    metrics: Metrics,
    kind: MetricKind,
    started: Instant,
    finished: bool,
}

impl LatencyTimer {
    pub fn finish(mut self) {
        self.record();
    }

    fn record(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.metrics.observe(self.kind, self.started.elapsed());
    }
}

impl Drop for LatencyTimer {
    fn drop(&mut self) {
        self.record();
    }
}

fn summarize(values: &[u64]) -> LatencySummary {
    if values.is_empty() {
        return LatencySummary::default();
    }
    let mut sorted = values.to_vec();
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
        for _ in 0..(MAX_SAMPLES + 10) {
            metrics.observe(MetricKind::Asr, Duration::from_millis(10));
        }
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.asr.sample_count, MAX_SAMPLES);
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
        metrics.observe(MetricKind::PrefetchAsr, Duration::from_millis(12));
        metrics.observe(MetricKind::FinalAsr, Duration::from_millis(34));
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.prefetch_asr.p50_ms, Some(12));
        assert_eq!(snapshot.final_asr.p50_ms, Some(34));
        assert_eq!(snapshot.asr.p50_ms, Some(34));
    }
}
