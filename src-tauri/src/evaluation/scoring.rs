//! Versioned descriptive scoring of explicit fixtures. No quality gate or human
//! label is inferred from text similarity, declared groups or successful calls.
use super::contracts::*;
use super::validation::{self, error, InputError, ValidatedCase, ValidatedInputs};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::Hash;
use std::path::Path;

pub const METHOD: &str = "voiceflow-fixture-scoring-v1";
// Match the admitted reference/variant bytes and the existing largest bounded
// ASR response. Unitization happens after this byte admission, per metric.
const MAX_REFERENCE_BYTES: usize = 16_384;
const MAX_HYPOTHESIS_BYTES: usize = 4 * 1024 * 1024;
const DISTANCE_METHOD: &str = "exact-myers-multiword64-common-affix-v1";
macro_rules! record {
    ($name:ident { $($(#[$attr:meta])* $field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Debug, Clone, Deserialize, Serialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name { $($(#[$attr])* pub $field: $ty),* }
    };
}
record!(ScoringConfig { method: String, annotations: Option<ControlFixtureLocator>, comparison: Option<ComparisonSpec> });
record!(ComparisonSpec {
    left_job: u32,
    right_job: u32,
    stage: ScoreStage,
    cluster_by: ClusterDimension,
    seed: u64,
    bootstrap_replicates: u32
});
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClusterDimension {
    Speaker,
    SourceSession,
    TemplateFamily,
}
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceTarget {
    Asr,
    Final,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiteralKind {
    Path,
    Code,
    Flag,
    Version,
    Amount,
    Negation,
    Name,
    Other,
}
record!(LiteralTransform {
    from: String,
    to: String
});
record!(LiteralAnnotation { id: String, kind: LiteralKind, target: ReferenceTarget, utf8_range: [u32; 2], source: String, acceptable_variants: Vec<String>, authorized_transforms: Vec<LiteralTransform> });
record!(CaseAnnotation { case_id: String, reference_sha256: String, expected_final_sha256: String, literals: Vec<LiteralAnnotation> });
record!(ScoringAnnotationsV1 { artifact_type: String, schema_version: u32, revision: String, reviewed: bool, cases: Vec<CaseAnnotation> });
#[derive(Debug, Default)]
pub struct ScoringInputs {
    pub annotation_identity: Option<ControlInputIdentity>,
    pub annotations: BTreeMap<String, CaseAnnotation>,
    pub annotation_raw: Vec<u8>,
    pub annotation_revision: Option<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScoreInput {
    pub method: String,
    pub input_sha256: String,
    pub negative: bool,
    pub source_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asr_reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_references: Option<Vec<String>>,
    pub annotation_revision: Option<String>,
    pub corpus: Option<CorpusCaseV1>,
    pub annotations: Vec<LiteralAnnotation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_only: Option<super::speech_input::ScoreScope>,
}
impl ScoreInput {
    fn cleanup_applicable(&self) -> bool {
        !self.negative && self.asr_only.is_none()
    }
    fn reference_eligible(&self) -> bool {
        !self.negative && self.asr_only.as_ref().is_none_or(|s| s.reference_eligible)
    }
    fn applies(&self, stage: ScoreStage) -> bool {
        matches!(stage, ScoreStage::ProviderAsr | ScoreStage::FilteredAsr)
            || self.cleanup_applicable()
    }
}
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ScoreStage {
    ProviderAsr,
    FilteredAsr,
    Prepared,
    ProviderCleanup,
    AcceptedCleanup,
    Final,
}
const STAGES: [ScoreStage; 6] = [
    ScoreStage::ProviderAsr,
    ScoreStage::FilteredAsr,
    ScoreStage::Prepared,
    ScoreStage::ProviderCleanup,
    ScoreStage::AcceptedCleanup,
    ScoreStage::Final,
];
record!(EditScore { state: StageState, edits: Option<u64>, reference_units: Option<u64>, rate: Option<f64>, reference_variant: Option<usize> });
record!(TextScores { state: StageState, raw_scalar_cer: EditScore, normalized_cer: EditScore, english_wer: EditScore, chinese_cer: EditScore, mixed_token_error_rate: EditScore, exact_accepted_reference: Option<bool> });
record!(LiteralScore { annotation_id: String, kind: LiteralKind, state: StageState, bounded_exact_match: Option<bool>, matched_variant: Option<usize> });
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReportDenominator {
    pub eligible: u64,
    pub requested: u64,
    pub called: u64,
    pub completed: u64,
    pub nonempty: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub unknown: u64,
    pub not_applicable: u64,
    pub not_run: u64,
    pub fresh: u64,
    pub replayed: u64,
    pub fallback: u64,
    pub mixed: u64,
    pub scored: u64,
}
record!(StageScore { stage: ScoreStage, source: String, state: StageState, text_sha256: Option<String>, text: TextScores, literals: Vec<LiteralScore>, denominator: ReportDenominator });
record!(CaseScores { input_sha256: String, stages: Vec<StageScore>, cleanup_conditional_eligible: bool, negative_outcome: StageState, negative_kind: Option<String>, distance_method: String, word_updates: u64 });
record!(Rate { numerator: u64, denominator: u64, value: Option<f64> });
record!(TimingDistribution { count: u64, p50_ms: Option<f64>, p95_ms: Option<f64>, maximum_ms: Option<f64> });
record!(MetricAggregate { samples: u64, edits: u64, reference_units: u64, micro_rate: Option<f64>, mean_case_rate: Option<f64> });
record!(QualityAggregate {
    raw_scalar_cer: MetricAggregate,
    normalized_cer: MetricAggregate,
    english_wer: MetricAggregate,
    chinese_cer: MetricAggregate,
    mixed_token_error_rate: MetricAggregate,
    exact_reference_proxy: Rate,
    declared_literal_proxy: Rate
});
record!(QualityPartitions {
    fresh: QualityAggregate,
    historical: QualityAggregate,
    mixed: QualityAggregate
});
// Retain the v1 JSON key while naming its nonempty-only statistic precisely.
record!(JobScores {
    job_id: String, requested_cases: u64, asr_success_cases: u64, workflow: ReportDenominator,
    stages: BTreeMap<String, ReportDenominator>, stage_quality: BTreeMap<String, QualityPartitions>, cleanup_asr_success_conditional: ReportDenominator,
    all_speech_e2e: ReportDenominator,
    #[serde(rename = "usable_final_rate_all_speech")]
    nonempty_final_proxy_rate_all_speech: Rate,
    accepted_cleanup_rate_asr_success: Rate, negative: BTreeMap<String, u64>,
    fresh_latency_ms: BTreeMap<String, TimingDistribution>, replay_local_latency_ms: BTreeMap<String, TimingDistribution>, mixed_local_latency_ms: BTreeMap<String, TimingDistribution>,
    local_adapter_starts: u64, retry_starts: u64, observed_cleanup_usage_units: BTreeMap<String, f64>,
    usage_coverage_cases: u64, unavailable: BTreeMap<String, StageState>
});
record!(PairedStatistics {
    state: StageState, method: String, spec: Option<ComparisonSpec>, requested_left_rows: u64,
    requested_right_rows: u64, matched_repeat_pairs: u64, eligible_cases: u64,
    excluded_cases: u64, eligible_clusters: u64, mean_right_minus_left: Option<f64>,
    percentile_95: Option<[f64; 2]>, coverage: String
});
record!(HumanAgreement { state: StageState, method: String, paired_reviewed_labels: u64, agreement: Option<f64>, kappa: Option<f64> });
record!(ScoringSummary {
    method: String, annotation_identity: Option<ControlInputIdentity>, annotation_revision: Option<String>, comparison_spec: Option<ComparisonSpec>, comparison_job_ids: Option<[String; 2]>,
    jobs: Vec<JobScores>, paired: PairedStatistics, human_agreement: HumanAgreement,
    human_pilot: StageState, locked_quality: StageState, ranking: StageState, training_labels: StageState,
    semantic_correctness: StageState, performance_scope: String
});

// Internal only: this evidence is deliberately absent from the persisted v1
// scoring schema. Callers must bind it to an actual selected row before use.
#[derive(Debug, Clone)]
pub(super) struct InternalFilterArmEvidence {
    pub job_id: String,
    pub case_id: String,
    pub repeat: u32,
    pub selected_audio: AudioIdentity,
    pub original_audio: AudioIdentity,
    pub filtered_audio: Option<AudioIdentity>,
    pub reference_sha256: String,
    pub controls_sha256: String,
    pub selected_text_sha256: String,
    pub selection: FilterSelection,
    pub eligibility: Option<bool>,
    pub outcome: Option<bool>,
    pub owner_eligibility: Option<bool>,
    pub owner_false_reject: Option<bool>,
    pub final_review: Option<InternalFinalReview>,
    pub synthetic: bool,
}

#[derive(Debug, Clone)]
pub(super) struct InternalFinalReview {
    pub final_text_sha256: String,
    pub attribution_reviewed: bool,
    pub damage_reviewed: bool,
    pub bystander_ranges: Vec<[u32; 2]>,
    pub damage: Vec<InternalDamageReview>,
    pub incomplete_damage_kinds: Vec<InternalDamageKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InternalDamageKind {
    Head,
    Tail,
    Number,
    Entity,
}

#[derive(Debug, Clone)]
pub(super) struct InternalDamageReview {
    pub kind: InternalDamageKind,
    pub reference_sha256: String,
    pub final_text_sha256: String,
    pub reference_range: [u32; 2],
    pub final_range: [u32; 2],
    pub damaged: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FilterSelection {
    Original,
    Filtered,
    RawFallback,
}

#[derive(Debug, Clone, Default)]
pub(super) struct InternalFilterEvidence {
    pub off_job_id: Option<String>,
    pub on_job_id: Option<String>,
    pub arms: Vec<InternalFilterArmEvidence>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct InternalFilterPairQuality {
    pub case_id: String,
    pub repeat: u32,
    pub cer_off: Option<f64>,
    pub cer_on: Option<f64>,
    pub cer_on_minus_off: Option<f64>,
    pub cer_off_state: &'static str,
    pub cer_on_state: &'static str,
    pub cer_unbounded: bool,
    pub wer_off: Option<f64>,
    pub wer_on: Option<f64>,
    pub wer_on_minus_off: Option<f64>,
    pub wer_off_state: &'static str,
    pub wer_on_state: &'static str,
    pub wer_unbounded: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct InternalFilterQuality {
    pub requested_off: Option<u64>,
    pub requested_on: Option<u64>,
    pub union_rows: Option<u64>,
    pub observed_rows: u64,
    pub unknown_rows: u64,
    pub invalid_rows: u64,
    pub invalid_reasons: Vec<String>,
    pub eligible_n: Option<u64>,
    pub known_eligible_rows: u64,
    pub eligibility_unknown: u64,
    pub binary_errors: u64,
    pub binary_rate: Option<f64>,
    pub owner_n: u64,
    pub owner_false_rejects: u64,
    pub owner_eligibility_unknown: u64,
    pub cer_off: Option<f64>,
    pub cer_on: Option<f64>,
    pub wer_off: Option<f64>,
    pub wer_on: Option<f64>,
    pub cer_on_minus_off: Option<f64>,
    pub wer_on_minus_off: Option<f64>,
    pub cer_unbounded: bool,
    pub wer_unbounded: bool,
    pub wer_applicable: bool,
    pub per_pair: Vec<InternalFilterPairQuality>,
    pub synthetic_rows: u64,
    pub final_bystander_observed: u64,
    pub final_bystander_leaks: u64,
    pub final_bystander_spans: u64,
    pub damage_observed: u64,
    pub damage_count: u64,
    pub damage_by_kind_observed: [u64; 4],
    pub damage_by_kind_spans: [u64; 4],
    pub damage_by_kind_count: [u64; 4],
    pub state: &'static str,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(super) struct FilterDeltaObservation {
    pub case_id: String,
    pub repeat: u32,
    pub speaker_id: Option<String>,
    pub session_id: Option<String>,
    pub delta: Option<f64>,
    pub state: &'static str,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(super) struct FilterDeltaInterval {
    pub state: &'static str,
    pub requested_count: usize,
    pub observed_count: usize,
    pub not_applicable_count: usize,
    pub unknown_count: usize,
    pub case_count: usize,
    pub speaker_count: usize,
    pub session_count: usize,
    pub point_estimate: Option<f64>,
    pub percentile_95: Option<[f64; 2]>,
    pub upper_bound: &'static str,
    pub bootstrap_replicates: usize,
    pub seed: u64,
    pub speaker_weight: &'static str,
    pub session_weight: &'static str,
    pub method: &'static str,
}

fn filter_delta_bootstrap_with<F>(
    groups: &BTreeMap<String, BTreeMap<String, Vec<f64>>>,
    mut draw: F,
) -> Vec<f64>
where
    F: FnMut(usize) -> usize,
{
    let speakers: Vec<_> = groups.iter().collect();
    let mut estimates = Vec::new();
    for _ in 0..speakers.len() {
        let speaker = &speakers[draw(speakers.len())].1;
        let sessions: Vec<_> = speaker.iter().collect();
        let mut sum = 0.0;
        let mut count = 0;
        for _ in 0..sessions.len() {
            let cases = sessions[draw(sessions.len())].1;
            for value in cases {
                sum += value;
                count += 1;
            }
        }
        estimates.push(sum / count as f64);
    }
    estimates
}

pub(super) fn filter_delta_interval(
    observations: &[FilterDeltaObservation],
) -> FilterDeltaInterval {
    const B: usize = 10_000;
    const SEED: u64 = 19;
    let mut result = FilterDeltaInterval {
        state: "unknown",
        requested_count: observations.len(),
        observed_count: 0,
        not_applicable_count: 0,
        unknown_count: 0,
        case_count: 0,
        speaker_count: 0,
        session_count: 0,
        point_estimate: None,
        percentile_95: None,
        upper_bound: "upper_unbounded",
        bootstrap_replicates: B,
        seed: SEED,
        speaker_weight: "equal_speaker",
        session_weight: "resample_sessions_within_speaker",
        method: "hierarchical_bootstrap_percentile_95",
    };
    let mut cases: BTreeMap<String, Vec<(u32, String, String, f64)>> = BTreeMap::new();
    let mut multiplicity: BTreeMap<(String, u32), usize> = BTreeMap::new();
    for observation in observations {
        *multiplicity
            .entry((observation.case_id.clone(), observation.repeat))
            .or_default() += 1;
    }
    for observation in observations {
        if multiplicity[&(observation.case_id.clone(), observation.repeat)] > 1 {
            result.unknown_count += 1;
            continue;
        }
        if observation.state == "not_applicable" {
            result.not_applicable_count += 1;
        } else if observation.state == "observed" {
            result.observed_count += 1;
            match (
                &observation.speaker_id,
                &observation.session_id,
                observation.delta,
            ) {
                (Some(speaker), Some(session), Some(delta))
                    if !speaker.is_empty() && !session.is_empty() && delta.is_finite() =>
                {
                    cases.entry(observation.case_id.clone()).or_default().push((
                        observation.repeat,
                        speaker.clone(),
                        session.clone(),
                        delta,
                    ));
                }
                _ => result.unknown_count += 1,
            }
        } else {
            result.unknown_count += 1;
        }
    }
    let mut grouped: BTreeMap<String, BTreeMap<String, Vec<f64>>> = BTreeMap::new();
    for (_, rows) in cases {
        let mut rows = rows;
        rows.sort_by_key(|row| row.0);
        let mut values = Vec::new();
        let mut cluster: Option<(String, String)> = None;
        for (repeat, speaker, session, delta) in rows {
            let _ = repeat;
            if cluster
                .as_ref()
                .is_some_and(|c| c != &(speaker.clone(), session.clone()))
            {
                result.unknown_count += 1;
                continue;
            }
            cluster = Some((speaker.clone(), session.clone()));
            values.push(delta);
        }
        if let Some((speaker, session)) = cluster {
            if !values.is_empty() {
                let mean = values.iter().sum::<f64>() / values.len() as f64;
                grouped
                    .entry(speaker)
                    .or_default()
                    .entry(session)
                    .or_default()
                    .push(mean);
            }
        }
    }
    result.case_count = grouped
        .values()
        .map(|sessions| sessions.values().map(Vec::len).sum::<usize>())
        .sum();
    result.speaker_count = grouped.len();
    result.session_count = grouped.values().map(BTreeMap::len).sum();
    if result.unknown_count > 0 {
        return result;
    }
    if result.observed_count == 0 && result.not_applicable_count > 0 {
        result.state = "not_applicable";
        result.upper_bound = "upper_unbounded";
        return result;
    }
    if result.observed_count == 0 || grouped.is_empty() {
        return result;
    }
    let speaker_means: Vec<f64> = grouped
        .values()
        .map(|sessions| {
            let values: Vec<f64> = sessions
                .values()
                .flat_map(|cases| cases.iter().copied())
                .collect();
            values.iter().sum::<f64>() / values.len() as f64
        })
        .collect();
    result.point_estimate = Some(speaker_means.iter().sum::<f64>() / speaker_means.len() as f64);
    let mut counter = 0u64;
    let mut rng = |n: usize| {
        let bytes = [SEED.to_le_bytes(), counter.to_le_bytes()].concat();
        counter += 1;
        let digest = validation::sha256(&bytes);
        usize::try_from(u64::from_str_radix(&digest[..16], 16).expect("SHA hex owner") % n as u64)
            .unwrap()
    };
    let mut samples = Vec::with_capacity(B);
    for _ in 0..B {
        let values = filter_delta_bootstrap_with(&grouped, &mut rng);
        samples.push(values.iter().sum::<f64>() / values.len() as f64);
    }
    samples.sort_by(f64::total_cmp);
    result.percentile_95 = Some([samples[249], samples[9750]]);
    result.state = "observed";
    result.upper_bound = "finite";
    result
}

pub(super) fn filter_clopper_pearson_upper(errors: u64, n: u64) -> Option<f64> {
    if n == 0 || errors > n {
        return None;
    }
    if errors == n {
        return Some(1.0);
    }
    let target = 0.025;
    let cdf = |p: f64| -> f64 {
        if p == 0.0 {
            return 1.0;
        }
        let log_term = |k: u64| {
            let mut log_choose = 0.0;
            for i in 0..k {
                log_choose += ((n - i) as f64 / (i + 1) as f64).ln();
            }
            log_choose + k as f64 * p.ln() + (n - k) as f64 * (1.0 - p).ln()
        };
        let max_log = (0..=errors).map(log_term).fold(f64::NEG_INFINITY, f64::max);
        (0..=errors)
            .map(|k| (log_term(k) - max_log).exp())
            .sum::<f64>()
            * max_log.exp()
    };
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..100 {
        let mid = (lo + hi) / 2.0;
        if cdf(mid) > target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some((lo + hi) / 2.0)
}

/// Derive a private, non-wire observation. A missing or contradictory binding
/// remains unknown; no row is removed from the requested case/repeat union.
pub(super) fn derive_filter_quality(
    report: &RunRecord,
    evidence: Option<&InternalFilterEvidence>,
) -> InternalFilterQuality {
    let Some([off_id, on_id]) = report
        .scoring
        .as_ref()
        .and_then(|s| s.comparison_job_ids.as_ref())
    else {
        return InternalFilterQuality {
            state: "unknown",
            cer_unbounded: true,
            wer_unbounded: true,
            ..Default::default()
        };
    };
    derive_filter_quality_for_arms(report, report, off_id, on_id, evidence.map(|e| (e, e)))
}

/// Shared paired-filter scoring core for legacy in-record comparisons and the
/// explicit offline consumer. The two source RunRecords stay separate and
/// selected job IDs may be equal because each ID is scoped to its own run.
pub(super) fn derive_filter_quality_for_arms(
    off_report: &RunRecord,
    on_report: &RunRecord,
    off_id: &str,
    on_id: &str,
    evidence: Option<(&InternalFilterEvidence, &InternalFilterEvidence)>,
) -> InternalFilterQuality {
    let off: BTreeMap<_, _> = off_report
        .cases
        .iter()
        .filter(|r| r.job_id == off_id)
        .map(|r| ((r.case_id.as_str(), r.repeat), r))
        .collect();
    let on: BTreeMap<_, _> = on_report
        .cases
        .iter()
        .filter(|r| r.job_id == on_id)
        .map(|r| ((r.case_id.as_str(), r.repeat), r))
        .collect();
    if off.len()
        != off_report
            .cases
            .iter()
            .filter(|r| r.job_id == off_id)
            .count()
        || on.len() != on_report.cases.iter().filter(|r| r.job_id == on_id).count()
    {
        let keys: BTreeSet<_> = off_report
            .cases
            .iter()
            .filter(|r| r.job_id == off_id)
            .chain(on_report.cases.iter().filter(|r| r.job_id == on_id))
            .map(|r| (r.case_id.as_str(), r.repeat))
            .collect();
        return InternalFilterQuality {
            requested_off: Some(off.len() as u64),
            requested_on: Some(on.len() as u64),
            union_rows: Some(keys.len() as u64),
            unknown_rows: keys.len() as u64,
            invalid_rows: keys.len() as u64,
            eligibility_unknown: keys.len() as u64,
            owner_eligibility_unknown: keys.len() as u64,
            per_pair: keys
                .iter()
                .map(|(case_id, repeat)| InternalFilterPairQuality {
                    case_id: (*case_id).to_owned(),
                    repeat: *repeat,
                    cer_unbounded: true,
                    wer_off_state: "unknown",
                    wer_on_state: "unknown",
                    cer_off_state: "unknown",
                    cer_on_state: "unknown",
                    wer_unbounded: true,
                    ..Default::default()
                })
                .collect(),
            invalid_reasons: vec!["duplicate case/repeat row binding".into()],
            state: "unknown",
            cer_unbounded: true,
            wer_unbounded: true,
            ..Default::default()
        };
    }
    let keys: BTreeSet<_> = off.keys().chain(on.keys()).copied().collect();
    let mut out = InternalFilterQuality {
        requested_off: Some(off.len() as u64),
        requested_on: Some(on.len() as u64),
        union_rows: Some(keys.len() as u64),
        state: "unknown",
        ..Default::default()
    };
    out.per_pair = keys
        .iter()
        .map(|(case_id, repeat)| InternalFilterPairQuality {
            case_id: (*case_id).to_owned(),
            repeat: *repeat,
            cer_unbounded: true,
            wer_off_state: "unknown",
            wer_on_state: "unknown",
            cer_off_state: "unknown",
            cer_on_state: "unknown",
            wer_unbounded: true,
            ..Default::default()
        })
        .collect();
    out.cer_unbounded = true;
    out.wer_unbounded = true;
    let Some((off_evidence, on_evidence)) = evidence else {
        out.unknown_rows = keys.len() as u64;
        out.eligibility_unknown = keys.len() as u64;
        out.owner_eligibility_unknown = keys.len() as u64;
        return out;
    };
    if off_evidence.off_job_id.as_deref() != Some(off_id)
        || on_evidence.on_job_id.as_deref() != Some(on_id)
    {
        out.unknown_rows = keys.len() as u64;
        out.eligibility_unknown = keys.len() as u64;
        out.owner_eligibility_unknown = keys.len() as u64;
        return out;
    }
    let terminal = |record: &RunRecord| {
        matches!(
            record.terminal,
            StageState::Completed {} | StageState::Failed { .. } | StageState::Cancelled { .. }
        )
    };
    let current = |record: &RunRecord| {
        record
            .replay
            .as_ref()
            .is_none_or(|replay| replay.current_quality_eligible)
    };
    if !terminal(off_report) || !terminal(on_report) || !current(off_report) || !current(on_report)
    {
        out.unknown_rows = keys.len() as u64;
        out.eligibility_unknown = keys.len() as u64;
        out.owner_eligibility_unknown = keys.len() as u64;
        return out;
    }
    if off_evidence.arms.iter().any(|a| {
        a.job_id == off_id
            && off_evidence
                .arms
                .iter()
                .filter(|b| b.job_id == a.job_id && b.case_id == a.case_id && b.repeat == a.repeat)
                .count()
                != 1
    }) || on_evidence.arms.iter().any(|a| {
        a.job_id == on_id
            && on_evidence
                .arms
                .iter()
                .filter(|b| b.job_id == a.job_id && b.case_id == a.case_id && b.repeat == a.repeat)
                .count()
                != 1
    }) {
        out.unknown_rows = keys.len() as u64;
        out.invalid_rows = keys.len() as u64;
        out.invalid_reasons
            .push("duplicate or conflicting arm evidence binding".into());
        out.eligibility_unknown = keys.len() as u64;
        out.owner_eligibility_unknown = keys.len() as u64;
        return out;
    }
    let mut cer = [0.0, 0.0];
    let mut wer = [0.0, 0.0];
    let mut cer_count = [0u64; 2];
    let mut wer_count = [0u64; 2];
    out.wer_applicable = true;
    let mut eligibility_known = true;
    let mut controls_bound_all = true;
    let mut invalid_keys = BTreeSet::new();
    for (pair_index, key) in keys.into_iter().enumerate() {
        let left_arm = off_evidence
            .arms
            .iter()
            .find(|a| a.job_id == off_id && a.case_id == key.0 && a.repeat == key.1);
        let right_arm = on_evidence
            .arms
            .iter()
            .find(|a| a.job_id == on_id && a.case_id == key.0 && a.repeat == key.1);
        let (le, re) = (left_arm, right_arm);
        match (le, re) {
            (Some(le), Some(re)) => match (le.eligibility, re.eligibility) {
                (Some(false), Some(false)) => {}
                (Some(true), Some(true)) => {
                    out.known_eligible_rows += 1;
                    if le.outcome != Some(false) || re.outcome != Some(false) {
                        out.binary_errors += 1;
                    }
                }
                (Some(a), Some(b)) if a != b => {
                    out.eligibility_unknown += 1;
                    eligibility_known = false;
                    if invalid_keys.insert(key) {
                        out.invalid_rows += 1;
                    }
                    out.invalid_reasons.push(format!(
                        "eligibility disagreement for {} repeat {}",
                        key.0, key.1
                    ));
                }
                _ => {
                    out.eligibility_unknown += 1;
                    eligibility_known = false;
                }
            },
            _ => {
                out.eligibility_unknown += 1;
                eligibility_known = false;
            }
        }
        if let Some(re) = re {
            match re.owner_eligibility {
                Some(true) => {
                    out.owner_n += 1;
                    if re.owner_false_reject != Some(false) {
                        out.owner_false_rejects += 1;
                    }
                }
                Some(false) => {}
                None => out.owner_eligibility_unknown += 1,
            }
        } else {
            out.owner_eligibility_unknown += 1;
        }
        let (Some(left), Some(right), Some(le), Some(re)) = (off.get(&key), on.get(&key), le, re)
        else {
            if invalid_keys.insert(key) {
                out.invalid_rows += 1;
            }
            out.invalid_reasons.push(format!(
                "missing arm or case binding for {} repeat {}",
                key.0, key.1
            ));
            out.unknown_rows += 1;
            continue;
        };
        if le.controls_sha256.is_empty() || re.controls_sha256.is_empty() {
            controls_bound_all = false;
        }
        if left.asr_replay.is_some()
            || left.cleanup_replay.is_some()
            || right.asr_replay.is_some()
            || right.cleanup_replay.is_some()
        {
            out.unknown_rows += 1;
            continue;
        }
        let (Some(lt), Some(rt)) = (
            known_observed_text(&left.filtered_text),
            known_observed_text(&right.filtered_text),
        ) else {
            out.unknown_rows += 1;
            continue;
        };
        if le.selected_text_sha256 != validation::sha256(lt.as_bytes())
            || re.selected_text_sha256 != validation::sha256(rt.as_bytes())
            || !same_audio(&le.selected_audio, &left.audio)
            || !same_audio(&re.selected_audio, &right.audio)
            || !same_audio(&le.original_audio, &re.original_audio)
            || le.reference_sha256 != re.reference_sha256
            || (!le.controls_sha256.is_empty()
                && !re.controls_sha256.is_empty()
                && le.controls_sha256 != re.controls_sha256)
            || le.selection != FilterSelection::Original
            || !same_audio(&le.selected_audio, &le.original_audio)
            || !matches!(
                re.selection,
                FilterSelection::Filtered | FilterSelection::RawFallback
            )
            || (re.selection == FilterSelection::RawFallback
                && !same_audio(&re.selected_audio, &re.original_audio))
            || (re.selection == FilterSelection::Filtered
                && re
                    .filtered_audio
                    .as_ref()
                    .is_none_or(|filtered| !same_audio(filtered, &re.selected_audio)))
        {
            out.unknown_rows += 1;
            if invalid_keys.insert(key) {
                out.invalid_rows += 1;
            }
            out.invalid_reasons.push(format!(
                "contradictory audio or content binding for {} repeat {}",
                key.0, key.1
            ));
            continue;
        }
        let (Some(li), Some(ri)) = (&left.score_input, &right.score_input) else {
            out.unknown_rows += 1;
            continue;
        };
        let (Some(lref), Some(rref)) = (li.asr_reference.as_deref(), ri.asr_reference.as_deref())
        else {
            out.unknown_rows += 1;
            continue;
        };
        if validation::sha256(lref.as_bytes()) != le.reference_sha256
            || validation::sha256(rref.as_bytes()) != re.reference_sha256
        {
            out.unknown_rows += 1;
            if invalid_keys.insert(key) {
                out.invalid_rows += 1;
            }
            out.invalid_reasons.push(format!(
                "reference digest contradiction for {} repeat {}",
                key.0, key.1
            ));
            continue;
        }
        out.observed_rows += 1;
        out.synthetic_rows += u64::from(le.synthetic || re.synthetic);
        for (row, arm_evidence) in [(*left, le), (*right, re)] {
            if let (ObservedText::KnownEmpty {} | ObservedText::KnownValue { .. }, Some(review)) =
                (&row.final_text, &arm_evidence.final_review)
            {
                let final_text = match &row.final_text {
                    ObservedText::KnownEmpty {} => "",
                    ObservedText::KnownValue { value } => value.as_str(),
                    ObservedText::Unknown { .. } => unreachable!(),
                };
                let digest = validation::sha256(final_text.as_bytes());
                if review.final_text_sha256 == digest
                    && review.attribution_reviewed
                    && review
                        .bystander_ranges
                        .iter()
                        .all(|r| valid_nonempty_utf8_range(final_text, *r))
                {
                    out.final_bystander_observed += 1;
                    out.final_bystander_leaks += u64::from(!review.bystander_ranges.is_empty());
                    out.final_bystander_spans += review.bystander_ranges.len() as u64;
                }
                let damage_review_bound =
                    review.final_text_sha256 == digest && review.damage_reviewed;
                if damage_review_bound {
                    let mut all_kinds_complete = true;
                    for kind in [
                        InternalDamageKind::Head,
                        InternalDamageKind::Tail,
                        InternalDamageKind::Number,
                        InternalDamageKind::Entity,
                    ] {
                        let index = damage_kind_index(kind);
                        let relevant: Vec<_> = review
                            .damage
                            .iter()
                            .filter(|damage| damage.kind == kind)
                            .collect();
                        let kind_complete = !review.incomplete_damage_kinds.contains(&kind)
                            && !relevant.is_empty()
                            && relevant.iter().all(|damage| {
                                valid_damage_annotation(
                                    damage,
                                    lref,
                                    final_text,
                                    &arm_evidence.reference_sha256,
                                    &digest,
                                )
                            });
                        if kind_complete {
                            out.damage_by_kind_observed[index] += 1;
                            let damaged =
                                relevant.iter().filter(|damage| damage.damaged).count() as u64;
                            out.damage_by_kind_spans[index] += relevant.len() as u64;
                            out.damage_by_kind_count[index] += damaged;
                            out.damage_count += damaged;
                        } else {
                            all_kinds_complete = false;
                        }
                    }
                    if all_kinds_complete {
                        out.damage_observed += 1;
                    }
                }
            }
        }
        for (index, (input, text)) in [(li, lt), (ri, rt)].into_iter().enumerate() {
            let mut work = 0;
            let scores = text_scores(input, ScoreStage::FilteredAsr, text, &mut work);
            if let Some(rate) = scores.normalized_cer.rate {
                cer[index] += rate;
                cer_count[index] += 1;
                if index == 0 {
                    out.per_pair[pair_index].cer_off = Some(rate);
                } else {
                    out.per_pair[pair_index].cer_on = Some(rate);
                }
            }
            match (&scores.normalized_cer.state, scores.normalized_cer.rate) {
                (StageState::Completed {}, Some(_)) => {
                    if index == 0 {
                        out.per_pair[pair_index].cer_off_state = "observed";
                    } else {
                        out.per_pair[pair_index].cer_on_state = "observed";
                    }
                }
                (StageState::NotApplicable {}, _) => {
                    if index == 0 {
                        out.per_pair[pair_index].cer_off_state = "not_applicable";
                    } else {
                        out.per_pair[pair_index].cer_on_state = "not_applicable";
                    }
                }
                _ => {
                    if index == 0 {
                        out.per_pair[pair_index].cer_off_state = "unknown";
                    } else {
                        out.per_pair[pair_index].cer_on_state = "unknown";
                    }
                }
            }
            match (&scores.english_wer.state, scores.english_wer.rate) {
                (StageState::Completed {}, Some(rate)) => {
                    wer[index] += rate;
                    wer_count[index] += 1;
                    if index == 0 {
                        out.per_pair[pair_index].wer_off = Some(rate);
                    } else {
                        out.per_pair[pair_index].wer_on = Some(rate);
                    }
                    if index == 0 {
                        out.per_pair[pair_index].wer_off_state = "observed";
                    } else {
                        out.per_pair[pair_index].wer_on_state = "observed";
                    }
                }
                (StageState::NotApplicable {}, _) => {
                    out.wer_applicable = false;
                    if index == 0 {
                        out.per_pair[pair_index].wer_off_state = "not_applicable";
                    } else {
                        out.per_pair[pair_index].wer_on_state = "not_applicable";
                    }
                }
                _ => {
                    out.wer_unbounded = true;
                }
            }
        }
        let pair = &mut out.per_pair[pair_index];
        if !le.controls_sha256.is_empty() && !re.controls_sha256.is_empty() {
            pair.cer_on_minus_off = pair.cer_off.zip(pair.cer_on).map(|(off, on)| on - off);
        }
        pair.cer_unbounded = pair.cer_off_state == "unknown"
            || pair.cer_on_state == "unknown"
            || le.controls_sha256.is_empty()
            || re.controls_sha256.is_empty();
        if !le.controls_sha256.is_empty() && !re.controls_sha256.is_empty() {
            pair.wer_on_minus_off = pair.wer_off.zip(pair.wer_on).map(|(off, on)| on - off);
        }
        pair.wer_unbounded = pair.wer_off_state == "unknown"
            || pair.wer_on_state == "unknown"
            || le.controls_sha256.is_empty()
            || re.controls_sha256.is_empty();
    }
    let single_pair = out.union_rows == Some(1);
    out.cer_off = (single_pair && cer_count[0] == 1).then(|| cer[0]);
    out.cer_on = (single_pair && cer_count[1] == 1).then(|| cer[1]);
    out.wer_off = (single_pair && wer_count[0] == 1).then(|| wer[0]);
    out.wer_on = (single_pair && wer_count[1] == 1).then(|| wer[1]);
    if controls_bound_all {
        out.cer_on_minus_off = out.cer_off.zip(out.cer_on).map(|(off, on)| on - off);
        out.wer_on_minus_off = out.wer_off.zip(out.wer_on).map(|(off, on)| on - off);
    }
    out.cer_unbounded = !single_pair || out.per_pair.iter().any(|pair| pair.cer_unbounded);
    out.wer_unbounded = if out.wer_applicable {
        !single_pair
            || wer_count.iter().any(|n| *n < out.union_rows.unwrap_or(0))
            || out.per_pair.iter().any(|pair| pair.wer_unbounded)
    } else {
        out.unknown_rows > 0 || out.per_pair.iter().any(|pair| pair.wer_unbounded)
    };
    out.eligible_n = eligibility_known.then_some(out.known_eligible_rows);
    out.binary_rate = if eligibility_known {
        out.eligible_n
            .filter(|n| *n > 0)
            .map(|n| out.binary_errors as f64 / n as f64)
    } else {
        None
    };
    let expected_review_units = out.union_rows.unwrap_or(0).saturating_mul(2);
    let review_coverage_unknown = out.final_bystander_observed < expected_review_units
        || out.owner_eligibility_unknown > 0
        || out
            .damage_by_kind_observed
            .iter()
            .any(|n| *n < expected_review_units);
    out.state = if out.unknown_rows == 0
        && out.eligibility_unknown == 0
        && !review_coverage_unknown
        && !out.cer_unbounded
        && (!out.wer_applicable || !out.wer_unbounded)
    {
        "observed"
    } else {
        "unknown"
    };
    out
}

fn valid_utf8_range(text: &str, range: [u32; 2]) -> bool {
    let (start, end) = (range[0] as usize, range[1] as usize);
    start <= end && end <= text.len() && text.is_char_boundary(start) && text.is_char_boundary(end)
}

fn valid_nonempty_utf8_range(text: &str, range: [u32; 2]) -> bool {
    range[0] < range[1] && valid_utf8_range(text, range)
}

fn damage_kind_index(kind: InternalDamageKind) -> usize {
    match kind {
        InternalDamageKind::Head => 0,
        InternalDamageKind::Tail => 1,
        InternalDamageKind::Number => 2,
        InternalDamageKind::Entity => 3,
    }
}

fn valid_damage_annotation(
    damage: &InternalDamageReview,
    reference: &str,
    final_text: &str,
    expected_reference_sha256: &str,
    expected_final_sha256: &str,
) -> bool {
    damage.reference_sha256 == expected_reference_sha256
        && damage.final_text_sha256 == expected_final_sha256
        && valid_nonempty_utf8_range(reference, damage.reference_range)
        && valid_utf8_range(final_text, damage.final_range)
        && (damage.damaged || damage.final_range[0] < damage.final_range[1])
        && match damage.kind {
            InternalDamageKind::Head => damage.reference_range[0] == 0,
            InternalDamageKind::Tail => reference.len() == damage.reference_range[1] as usize,
            InternalDamageKind::Number | InternalDamageKind::Entity => true,
        }
}

fn same_audio(a: &AudioIdentity, b: &AudioIdentity) -> bool {
    a.audio_sha256 == b.audio_sha256
        && a.pcm_sha256 == b.pcm_sha256
        && a.sample_count == b.sample_count
        && a.sample_rate == b.sample_rate
        && a.channels == b.channels
        && a.bits_per_sample == b.bits_per_sample
}

fn known_observed_text(text: &ObservedText) -> Option<&str> {
    match text {
        ObservedText::KnownEmpty {} => Some(""),
        ObservedText::KnownValue { value } => Some(value.as_str()),
        ObservedText::Unknown { .. } => None,
    }
}

fn unknown(reason: &str) -> StageState {
    StageState::Unknown {
        reason: reason.into(),
    }
}
fn na() -> StageState {
    StageState::NotApplicable {}
}
fn rate(numerator: u64, denominator: u64) -> Rate {
    Rate {
        numerator,
        denominator,
        value: (denominator > 0).then(|| numerator as f64 / denominator as f64),
    }
}
pub(super) fn validate(
    path: &Path,
    recipe: &ExperimentRecipe,
    cases: &[ValidatedCase],
) -> Result<ScoringInputs, InputError> {
    let mut inputs = ScoringInputs::default();
    let expansion: usize = recipe.jobs.iter().map(|j| j.repeats as usize).sum();
    let reference_bytes: usize = cases
        .iter()
        .map(|c| {
            if let Some(source) = c.case.asr_speech() {
                serde_json::to_vec(&(&source.case, &source.source))
            } else {
                serde_json::to_vec(&(
                    c.case
                        .speech()
                        .map(|s| (&s.reference, &s.expected_final, &s.reference_variants)),
                    &c.corpus,
                ))
            }
            .expect("typed scoring input")
            .len()
        })
        .sum();
    if reference_bytes.saturating_mul(expansion) > 8 * 1024 * 1024 {
        return Err(error(
            path,
            "input_budget",
            "expanded serialized scoring references/group facts exceed 8 MiB",
        ));
    }
    let Some(config) = &recipe.scoring else {
        return Ok(inputs);
    };
    if config.method != METHOD {
        return Err(error(
            path,
            "scoring_method",
            "only voiceflow-fixture-scoring-v1 is supported",
        ));
    }
    if let Some(pair) = &config.comparison {
        if pair.left_job == pair.right_job
            || pair.left_job as usize >= recipe.jobs.len()
            || pair.right_job as usize >= recipe.jobs.len()
            || !(100..=400).contains(&pair.bootstrap_replicates)
            || !matches!(
                pair.stage,
                ScoreStage::FilteredAsr | ScoreStage::AcceptedCleanup | ScoreStage::Final
            )
        {
            return Err(error(
                path,
                "scoring_comparison",
                "select distinct declared jobs, one cluster dimension, seed and 100..400 resamples",
            ));
        }
    }
    if let Some(locator) = &config.annotations {
        let canonical = validation::relative_file(path, &locator.relative_path)?;
        let raw = validation::read_bounded(&canonical, validation::MAX_JSON_BYTES)?;
        if !validation::digest(&locator.sha256) || validation::sha256(&raw) != locator.sha256 {
            return Err(error(
                &canonical,
                "sha_mismatch",
                "scoring annotation digest mismatch",
            ));
        }
        let parsed: ScoringAnnotationsV1 = validation::decode_json(&canonical, &raw)?;
        if parsed.artifact_type != "scoring_annotations"
            || parsed.schema_version != 1
            || !parsed.reviewed
            || !validation::bounded(&parsed.revision, 256)
            || parsed.cases.len() > cases.len()
        {
            return Err(error(
                &canonical,
                "scoring_annotations",
                "strict reviewed ScoringAnnotationsV1 required",
            ));
        }
        inputs.annotation_revision = Some(parsed.revision.clone());
        let mut spans = 0usize;
        for annotation in parsed.cases {
            let case = cases
                .iter()
                .find(|c| c.case.id() == annotation.case_id && !c.negative())
                .ok_or_else(|| {
                    error(
                        &canonical,
                        "identity_mismatch",
                        "annotation must reference a declared speech case",
                    )
                })?;
            if inputs.annotations.contains_key(&annotation.case_id)
                || annotation.reference_sha256
                    != validation::sha256(
                        case.case
                            .speech()
                            .expect("speech annotation")
                            .reference
                            .as_bytes(),
                    )
                || annotation.expected_final_sha256
                    != validation::sha256(
                        case.case
                            .speech()
                            .expect("speech annotation")
                            .expected_final
                            .as_bytes(),
                    )
                || annotation.literals.len() > 16
            {
                return Err(error(
                    &canonical,
                    "identity_mismatch",
                    "duplicate annotation or mismatched frozen reference identity",
                ));
            }
            let mut ids = BTreeSet::new();
            for literal in &annotation.literals {
                let reference = match literal.target {
                    ReferenceTarget::Asr => {
                        &case.case.speech().expect("speech annotation").reference
                    }
                    ReferenceTarget::Final => {
                        &case
                            .case
                            .speech()
                            .expect("speech annotation")
                            .expected_final
                    }
                };
                let [start, end] = literal.utf8_range.map(|n| n as usize);
                let mut variants = BTreeSet::from([literal.source.as_str()]);
                if !validation::bounded(&literal.id, 128)
                    || !ids.insert(&literal.id)
                    || !validation::bounded(&literal.source, 512)
                    || start >= end
                    || reference.get(start..end) != Some(literal.source.as_str())
                    || literal.acceptable_variants.len() > 4
                    || literal.authorized_transforms.len() > 4
                    || literal
                        .acceptable_variants
                        .iter()
                        .any(|v| !validation::bounded(v, 512) || !variants.insert(v.as_str()))
                    || literal.authorized_transforms.iter().any(|t| {
                        t.from != literal.source
                            || !validation::bounded(&t.to, 512)
                            || !variants.insert(t.to.as_str())
                    })
                {
                    return Err(error(&canonical, "scoring_literal", "literal needs unique bounded IDs/variants and an exact UTF-8 reference span; transforms must explicitly authorize source -> destination"));
                }
            }
            spans += annotation.literals.len();
            inputs
                .annotations
                .insert(annotation.case_id.clone(), annotation);
        }
        let expansion: usize = recipe.jobs.iter().map(|j| j.repeats as usize).sum();
        if spans.saturating_mul(expansion) > 20_000
            || raw.len().saturating_mul(expansion) > 4 * 1024 * 1024
        {
            return Err(error(&canonical, "input_budget", "expanded scoring annotations exceed 20,000 spans or 4 MiB raw-equivalent retained text"));
        }
        inputs.annotation_raw = raw.clone();
        inputs.annotation_identity = Some(ControlInputIdentity {
            relative_path: locator.relative_path.clone(),
            canonical_path: canonical.display().to_string(),
            sha256: locator.sha256.clone(),
            bytes: raw.len() as u64,
        });
    }
    Ok(inputs)
}
pub(super) fn input(case: &ValidatedCase, annotations: &ScoringInputs, legacy: bool) -> ScoreInput {
    let mut result = ScoreInput {
        method: METHOD.into(),
        input_sha256: String::new(),
        negative: case.negative(),
        source_kind: case.source_kind.clone(),
        asr_only: case.case.asr_speech().map(|s| s.score_scope()),
        asr_reference: if legacy {
            Some(case.case.legacy().expect("V1 scoring projection").reference)
        } else {
            case.case.reference().map(str::to_owned)
        },
        final_references: if legacy {
            let c = case.case.legacy().expect("V1 scoring projection");
            Some(
                std::iter::once(c.expected_final)
                    .chain(c.reference_variants)
                    .collect(),
            )
        } else {
            case.case.speech().map(|s| {
                std::iter::once(s.expected_final.clone())
                    .chain(s.reference_variants.clone())
                    .collect()
            })
        },
        annotation_revision: annotations.annotation_revision.clone(),
        corpus: case.corpus.clone(),
        annotations: annotations
            .annotations
            .get(case.case.id())
            .map(|a| a.literals.clone())
            .unwrap_or_default(),
    };
    result.input_sha256 = super::resume::hash(&result);
    result
}
pub(super) fn summary(inputs: &ValidatedInputs) -> ScoringSummary {
    let spec = inputs
        .recipe
        .as_ref()
        .and_then(|r| r.scoring.as_ref())
        .and_then(|s| s.comparison.clone());
    ScoringSummary {
        method: METHOD.into(), annotation_identity: inputs.scoring.annotation_identity.clone(), annotation_revision: inputs.scoring.annotation_revision.clone(), comparison_spec: spec.clone(), comparison_job_ids: spec.as_ref().and_then(|s| { let jobs=&inputs.recipe.as_ref()?.jobs; Some([jobs.get(s.left_job as usize)?.id(s.left_job as usize),jobs.get(s.right_job as usize)?.id(s.right_job as usize)]) }),
        jobs: vec![], paired: empty_pair(spec), human_agreement: HumanAgreement {
            state: na(), method: "cohen_kappa_nominal_v1_not_applicable_without_blind_review_labels".into(), paired_reviewed_labels: 0, agreement: None, kappa: None,
        }, human_pilot: StageState::NotRun {}, locked_quality: StageState::NotRun {}, ranking: StageState::NotRun {}, training_labels: StageState::NotRun {},
        semantic_correctness: unknown("text_similarity_and_declared_literal_presence_are_not_factual_correctness"),
        performance_scope: "component_adapter_timing_only; no stop-to-insert/pricing/receipt/cold-warm/process-family-peak inference".into(),
    }
}
fn empty_edit(reason: &str) -> EditScore {
    EditScore {
        state: unknown(reason),
        edits: None,
        reference_units: None,
        rate: None,
        reference_variant: None,
    }
}
fn empty_text(reason: &str) -> TextScores {
    TextScores {
        state: unknown(reason),
        raw_scalar_cer: empty_edit(reason),
        normalized_cer: empty_edit(reason),
        english_wer: empty_edit(reason),
        chinese_cer: empty_edit(reason),
        mixed_token_error_rate: empty_edit(reason),
        exact_accepted_reference: None,
    }
}
fn han(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x3134f)
}
fn ignored(c: char) -> bool {
    c.is_whitespace() || c.is_ascii_punctuation() || "，。！？；：、（）【】《》“”‘’…—".contains(c)
}
fn normalized(text: &str) -> Vec<char> {
    text.chars()
        .filter(|c| !ignored(*c))
        .map(|c| c.to_ascii_lowercase())
        .collect()
}
fn words(text: &str, mixed: bool) -> Vec<String> {
    let mut result = Vec::new();
    let mut word = String::new();
    for c in text.chars().map(|c| c.to_ascii_lowercase()) {
        if c.is_alphanumeric() && !han(c) {
            word.push(c);
        } else {
            if !word.is_empty() {
                result.push(std::mem::take(&mut word));
            }
            if mixed && !ignored(c) {
                result.push(c.to_string());
            }
        }
    }
    if !word.is_empty() {
        result.push(word);
    }
    result
}
// Exact Levenshtein; neither banding nor an edit cutoff selects which errors
// remain scoreable. The shorter sequence is the bit pattern. All callers bound
// reference bytes before tokenization, so it has <=16,384 units (256 words).
// At most one pattern-sized equality mask per distinct unit: <=32 MiB of mask
// payload, plus O(pattern units) keys/vectors. Words/tokens borrow their keys.
// Prefix/suffix reduction helps near-equal long text without changing distance.
fn distance<T: Eq + Hash>(reference: &[T], hypothesis: &[T], work: &mut u64) -> u64 {
    let prefix = reference
        .iter()
        .zip(hypothesis)
        .take_while(|(a, b)| a == b)
        .count();
    let reference = &reference[prefix..];
    let hypothesis = &hypothesis[prefix..];
    let suffix = reference
        .iter()
        .rev()
        .zip(hypothesis.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let reference = &reference[..reference.len() - suffix];
    let hypothesis = &hypothesis[..hypothesis.len() - suffix];
    let (pattern, text) = if reference.len() <= hypothesis.len() {
        (reference, hypothesis)
    } else {
        (hypothesis, reference)
    };
    if pattern.is_empty() {
        return text.len() as u64;
    }
    let words = pattern.len().div_ceil(64);
    let mut equality: HashMap<&T, Vec<u64>> = HashMap::new();
    for (index, unit) in pattern.iter().enumerate() {
        equality.entry(unit).or_insert_with(|| vec![0; words])[index / 64] |= 1 << (index % 64);
    }
    let mut positive = vec![u64::MAX; words];
    let mut negative = vec![0u64; words];
    let last_bit = 1u64 << ((pattern.len() - 1) % 64);
    let mut score = pattern.len() as u64;
    for unit in text {
        let mask = equality.get(unit);
        let mut add_carry = false;
        let mut positive_shift = 1u64;
        let mut negative_shift = 0u64;
        for block in 0..words {
            let equal = mask.map_or(0, |m| m[block]);
            let pv = positive[block];
            let mv = negative[block];
            let vertical = equal | mv;
            // Multiword addition and the two shifts each have independent
            // carries. Padding bits cannot influence lower/real pattern bits.
            let (sum, carry1) = (equal & pv).overflowing_add(pv);
            let (sum, carry2) = sum.overflowing_add(u64::from(add_carry));
            add_carry = carry1 || carry2;
            let horizontal = (sum ^ pv) | equal;
            let ph = mv | !(horizontal | pv);
            let mh = pv & horizontal;
            if block == words - 1 {
                score += u64::from(ph & last_bit != 0);
                score -= u64::from(mh & last_bit != 0);
            }
            let shifted_ph = (ph << 1) | positive_shift;
            let shifted_mh = (mh << 1) | negative_shift;
            positive_shift = ph >> 63;
            negative_shift = mh >> 63;
            positive[block] = shifted_mh | !(vertical | shifted_ph);
            negative[block] = shifted_ph & vertical;
        }
        *work = work.saturating_add(words as u64);
    }
    score
}
fn metric<T: Eq + Hash>(references: &[Vec<T>], hypothesis: &[T], work: &mut u64) -> EditScore {
    let mut best: Option<(u64, usize, usize)> = None;
    for (index, reference) in references.iter().enumerate() {
        let edits = distance(reference, hypothesis, work);
        if reference.is_empty() {
            continue;
        }
        if best.is_none_or(|(b, units, _)| {
            (edits as u128 * units as u128) < (b as u128 * reference.len() as u128)
        }) {
            best = Some((edits, reference.len(), index));
        }
    }
    match best {
        Some((edits, units, index)) => EditScore {
            state: StageState::Completed {},
            edits: Some(edits),
            reference_units: Some(units as u64),
            rate: Some(edits as f64 / units as f64),
            reference_variant: Some(index),
        },
        None => EditScore {
            state: na(),
            edits: None,
            reference_units: Some(0),
            rate: None,
            reference_variant: None,
        },
    }
}
pub(super) fn text_scores(
    input: &ScoreInput,
    stage: ScoreStage,
    text: &str,
    work: &mut u64,
) -> TextScores {
    if input.negative || !input.applies(stage) || !input.reference_eligible() {
        let mut result = empty_text("negative_has_no_speech_reference");
        result.state = na();
        for metric in [
            &mut result.raw_scalar_cer,
            &mut result.normalized_cer,
            &mut result.english_wer,
            &mut result.chinese_cer,
            &mut result.mixed_token_error_rate,
        ] {
            metric.state = na();
            metric.reference_units = Some(0);
        }
        return result;
    }
    let references = if matches!(stage, ScoreStage::ProviderAsr | ScoreStage::FilteredAsr) {
        let Some(reference) = input.asr_reference.as_deref() else {
            return empty_text("speech_reference_unavailable");
        };
        vec![reference]
    } else {
        let Some(references) = input.final_references.as_ref() else {
            return empty_text("speech_reference_unavailable");
        };
        references.iter().map(String::as_str).collect()
    };
    if text.len() > MAX_HYPOTHESIS_BYTES {
        return empty_text("scoring_hypothesis_byte_bound_exceeded_no_truncation");
    }
    if references.iter().any(|r| r.len() > MAX_REFERENCE_BYTES) {
        return empty_text("scoring_reference_outside_admitted_byte_bound");
    }
    let Some(primary) = references.first() else {
        return empty_text("reference_missing");
    };
    let raw = metric(
        &[primary.chars().collect::<Vec<_>>()],
        &text.chars().collect::<Vec<_>>(),
        work,
    );
    let norm = metric(
        &references.iter().map(|r| normalized(r)).collect::<Vec<_>>(),
        &normalized(text),
        work,
    );
    let source_language = input
        .asr_only
        .as_ref()
        .and_then(|s| match s.language.as_str() {
            "zh" => Some(Language::Zh),
            "en" => Some(Language::En),
            "mixed" => Some(Language::Mixed),
            _ => None,
        });
    let language = input
        .corpus
        .as_ref()
        .map(|c| &c.language)
        .or(source_language.as_ref());
    let english = if matches!(language, Some(Language::En | Language::Mixed)) {
        metric(
            &references
                .iter()
                .map(|r| words(r, false))
                .collect::<Vec<_>>(),
            &words(text, false),
            work,
        )
    } else {
        let mut v = empty_edit("language_metadata_unknown");
        if language.is_some() {
            v.state = na();
        }
        v
    };
    let chinese = if matches!(language, Some(Language::Zh | Language::Mixed)) {
        metric(
            &references
                .iter()
                .map(|r| r.chars().filter(|c| han(*c)).collect::<Vec<_>>())
                .collect::<Vec<_>>(),
            &text.chars().filter(|c| han(*c)).collect::<Vec<_>>(),
            work,
        )
    } else {
        let mut v = empty_edit("language_metadata_unknown");
        if language.is_some() {
            v.state = na();
        }
        v
    };
    let mixed = if matches!(language, Some(Language::Mixed)) {
        metric(
            &references
                .iter()
                .map(|r| words(r, true))
                .collect::<Vec<_>>(),
            &words(text, true),
            work,
        )
    } else {
        let mut v = empty_edit("language_metadata_unknown");
        if language.is_some() {
            v.state = na();
        }
        v
    };
    let state = if [&raw, &norm, &english, &chinese, &mixed]
        .iter()
        .any(|m| matches!(m.state, StageState::Unknown { .. }))
    {
        unknown("component_metric_unavailable_or_scoring_budget")
    } else {
        StageState::Completed {}
    };
    TextScores {
        state,
        raw_scalar_cer: raw,
        normalized_cer: norm,
        english_wer: english,
        chinese_cer: chinese,
        mixed_token_error_rate: mixed,
        exact_accepted_reference: Some(references.contains(&text)),
    }
}
fn continuation(c: char) -> bool {
    c.is_alphanumeric()
        || "_./\\:@+-".contains(c)
        || matches!(c as u32,0x300..=0x36f|0x1ab0..=0x1aff|0x1dc0..=0x1dff|0x200d|0xfe00..=0xfe0f|0x1f3fb..=0x1f3ff)
}
fn anchored_literal(text: &str, literal: &str, reference: &str, range: [u32; 2]) -> bool {
    let source_left = reference
        .get(..range[0] as usize)
        .and_then(|s| s.chars().next_back());
    let source_right = reference
        .get(range[1] as usize..)
        .and_then(|s| s.chars().next());
    text.match_indices(literal).any(|(index, _)| {
        let left = text[..index].chars().next_back();
        let right = text[index + literal.len()..].chars().next();
        (left == source_left && right == source_right)
            || (!left.is_some_and(continuation) && !right.is_some_and(continuation))
    })
}
fn literal_scores(input: &ScoreInput, stage: ScoreStage, text: &str) -> Vec<LiteralScore> {
    let target = if matches!(stage, ScoreStage::ProviderAsr | ScoreStage::FilteredAsr) {
        ReferenceTarget::Asr
    } else {
        ReferenceTarget::Final
    };
    input
        .annotations
        .iter()
        .filter(|a| a.target == target)
        .map(|a| {
            if text.len() > MAX_HYPOTHESIS_BYTES {
                return LiteralScore {
                    annotation_id: a.id.clone(),
                    kind: a.kind.clone(),
                    state: unknown("scoring_hypothesis_byte_bound_exceeded_no_truncation"),
                    bounded_exact_match: None,
                    matched_variant: None,
                };
            }
            let variants = std::iter::once(a.source.as_str())
                .chain(a.acceptable_variants.iter().map(String::as_str))
                .chain(a.authorized_transforms.iter().map(|t| t.to.as_str()));
            let matched = variants.enumerate().find_map(|(i, v)| {
                anchored_literal(
                    text,
                    v,
                    match target {
                        ReferenceTarget::Asr => input.asr_reference.as_deref().unwrap_or(""),
                        ReferenceTarget::Final => input
                            .final_references
                            .as_ref()
                            .and_then(|r| r.first())
                            .map(String::as_str)
                            .unwrap_or(""),
                    },
                    a.utf8_range,
                )
                .then_some(i)
            });
            LiteralScore {
                annotation_id: a.id.clone(),
                kind: a.kind.clone(),
                state: StageState::Completed {},
                bounded_exact_match: Some(matched.is_some()),
                matched_variant: matched,
            }
        })
        .collect()
}
fn stage_text(row: &CaseRecordV1, stage: ScoreStage) -> ObservedText {
    match stage {
        ScoreStage::ProviderAsr => row.provider_text.clone(),
        ScoreStage::FilteredAsr => row.filtered_text.clone(),
        ScoreStage::Prepared => row.prepared_text.clone(),
        ScoreStage::ProviderCleanup => row
            .cleanup_observation
            .as_ref()
            .map(|o| o.provider_reply.clone())
            .unwrap_or_else(|| ObservedText::unknown("not_observed")),
        ScoreStage::AcceptedCleanup => row.adapter_candidate.clone(),
        ScoreStage::Final => row.final_text.clone(),
    }
}
fn replayed(row: &CaseRecordV1, stage: ScoreStage) -> bool {
    match stage {
        ScoreStage::FilteredAsr => row.asr_replay.is_some(),
        ScoreStage::AcceptedCleanup => row.cleanup_replay.is_some(),
        ScoreStage::Prepared => row.asr_replay.is_some(),
        ScoreStage::Final => row.asr_replay.is_some() || row.cleanup_replay.is_some(),
        _ => false,
    }
}
fn stage_state(
    row: &CaseRecordV1,
    input: &ScoreInput,
    stage: ScoreStage,
    text: &ObservedText,
) -> StageState {
    let requested = !matches!(
        stage,
        ScoreStage::ProviderCleanup | ScoreStage::AcceptedCleanup
    ) || row.requested.cleanup.candidate_id.is_some();
    if !requested || !input.applies(stage) {
        return na();
    }
    if input.asr_only.is_some()
        && stage == ScoreStage::FilteredAsr
        && matches!(text, ObservedText::KnownEmpty {})
        && !row
            .selected_asr_observation()
            .is_some_and(crate::observations::AsrObservation::confirmed_empty)
    {
        return unknown("speech_empty_completion_unconfirmed");
    }
    if input.asr_only.is_some() && stage == ScoreStage::ProviderAsr {
        let complete = row.asr_observation.as_ref().is_some_and(|o| {
            matches!(
                o.origin.as_str(),
                "provider_response" | "local_model_output"
            ) && matches!(
                o.protocol_completion.as_str(),
                "json_complete" | "stop" | "complete" | "completed"
            )
        });
        if !complete {
            return unknown("raw_asr_origin_or_completion_unconfirmed");
        }
    }
    if stage == ScoreStage::ProviderCleanup && !row.requested.cleanup.capture_provider_candidate {
        return na();
    }
    if stage == ScoreStage::ProviderCleanup {
        if matches!(row.state, StageState::Cancelled { .. }) {
            return row.state.clone();
        }
        let Some(observation) = &row.cleanup_observation else {
            return if matches!(row.state, StageState::NotRun {}) {
                StageState::NotRun {}
            } else {
                unknown("provider_protocol_not_observed")
            };
        };
        // A finish token alone can precede a later disconnect or invalid frame.
        // Only the shared parser's terminal success fact establishes completion;
        // acceptance/final guards are downstream and do not select raw quality.
        return match observation.protocol_complete {
            Some(true) if !matches!(text, ObservedText::Unknown { .. }) => StageState::Completed {},
            Some(false) => StageState::Failed {
                reason: "provider_protocol_incomplete".into(),
            },
            _ => unknown("provider_protocol_or_raw_text_completion_unconfirmed"),
        };
    }
    match text {
        ObservedText::KnownEmpty {}
            if !input.negative && input.asr_only.is_none() && stage == ScoreStage::FilteredAsr =>
        {
            StageState::Failed {
                reason: "speech_empty".into(),
            }
        }
        ObservedText::KnownEmpty {} | ObservedText::KnownValue { .. } => StageState::Completed {},
        ObservedText::Unknown { reason } => {
            if matches!(row.state, StageState::Cancelled { .. }) {
                return row.state.clone();
            }
            if stage == ScoreStage::AcceptedCleanup
                && matches!(row.cleanup_adapter, StageState::Failed { .. })
            {
                return row.cleanup_adapter.clone();
            }
            if matches!(stage, ScoreStage::ProviderAsr | ScoreStage::FilteredAsr)
                && row.asr_terminal != "speech_text"
                && matches!(row.state, StageState::Failed { .. })
            {
                return row.state.clone();
            }
            if matches!(row.state, StageState::NotRun {})
                || (!matches!(stage, ScoreStage::ProviderAsr | ScoreStage::FilteredAsr)
                    && row.asr_terminal != "speech_text")
            {
                StageState::NotRun {}
            } else {
                unknown(reason)
            }
        }
    }
}
// The stage call count is adapter-local. Local preparation/finalization have
// no adapter call, but retain the origin of their contributing candidates.
fn stage_started(row: &CaseRecordV1, stage: ScoreStage) -> bool {
    match stage {
        ScoreStage::ProviderAsr | ScoreStage::FilteredAsr | ScoreStage::Prepared => row
            .attempt_facts
            .iter()
            .any(|f| f.stage == AttemptStage::Asr),
        ScoreStage::ProviderCleanup | ScoreStage::AcceptedCleanup => row
            .attempt_facts
            .iter()
            .any(|f| f.stage == AttemptStage::Cleanup),
        ScoreStage::Final => !row.attempt_facts.is_empty(),
    }
}
fn denominator(
    row: &CaseRecordV1,
    input: &ScoreInput,
    stage: ScoreStage,
    text: &ObservedText,
    state: &StageState,
    scored: bool,
) -> ReportDenominator {
    let requested = match stage {
        ScoreStage::ProviderCleanup | ScoreStage::AcceptedCleanup => {
            input.cleanup_applicable() && row.requested.cleanup.candidate_id.is_some()
        }
        ScoreStage::Prepared | ScoreStage::Final => input.cleanup_applicable(),
        _ => true,
    };
    let attempt = match stage {
        ScoreStage::ProviderAsr | ScoreStage::FilteredAsr => Some(AttemptStage::Asr),
        ScoreStage::ProviderCleanup | ScoreStage::AcceptedCleanup => Some(AttemptStage::Cleanup),
        _ => None,
    };
    let called = attempt.is_some_and(|s| row.attempt_facts.iter().any(|f| f.stage == s));
    let cached = replayed(row, stage);
    let started = stage_started(row, stage);
    ReportDenominator {
        eligible: u64::from(
            input.reference_eligible()
                && requested
                && !matches!(state, StageState::NotApplicable {}),
        ),
        requested: u64::from(requested),
        called: u64::from(called),
        completed: u64::from(matches!(state, StageState::Completed {})),
        nonempty: u64::from(
            matches!(text,ObservedText::KnownValue{value} if !value.trim().is_empty()),
        ),
        failed: u64::from(matches!(state, StageState::Failed { .. })),
        cancelled: u64::from(matches!(state, StageState::Cancelled { .. })),
        unknown: u64::from(matches!(state, StageState::Unknown { .. })),
        not_applicable: u64::from(matches!(state, StageState::NotApplicable {})),
        not_run: u64::from(matches!(state, StageState::NotRun {})),
        fresh: u64::from(started && !cached),
        replayed: u64::from(cached && !started),
        fallback: u64::from(stage == ScoreStage::Final && row.fallback.is_some()),
        mixed: u64::from(cached && started),
        scored: u64::from(scored),
    }
}
pub(super) fn refresh_row(row: &mut CaseRecordV1) {
    let Some(input) = &row.score_input else {
        return;
    };
    let previous = row.scores.take();
    let mut work = 0u64;
    let mut stages: Vec<StageScore> = Vec::with_capacity(STAGES.len());
    for stage in STAGES {
        let text = stage_text(row, stage);
        let state = stage_state(row, input, stage, &text);
        let known = match &text {
            ObservedText::KnownEmpty {} => Some(""),
            ObservedText::KnownValue { value } => Some(value.as_str()),
            ObservedText::Unknown { .. } => None,
        };
        let sha = known.map(|s| validation::sha256(s.as_bytes()));
        let reused = previous
            .as_ref()
            .filter(|p| p.input_sha256 == input.input_sha256)
            .and_then(|p| {
                p.stages
                    .iter()
                    .find(|s| s.stage == stage && s.text_sha256 == sha && sha.is_some())
            });
        // Same reference target + known text has the same metrics/literals,
        // independent of downstream adapter state. Reuse within this refresh as
        // well as across persistence; never substitute another stage's text.
        let reused = reused.or_else(|| {
            stages.iter().find(|s| {
                s.text_sha256 == sha
                    && sha.is_some()
                    && matches!(s.stage, ScoreStage::ProviderAsr | ScoreStage::FilteredAsr)
                        == matches!(stage, ScoreStage::ProviderAsr | ScoreStage::FilteredAsr)
            })
        });
        let (metrics, literals) = if !input.applies(stage) || !input.reference_eligible() {
            (text_scores(input, stage, "", &mut work), vec![])
        } else if input.asr_only.is_some()
            && matches!(stage, ScoreStage::ProviderAsr | ScoreStage::FilteredAsr)
            && matches!(state, StageState::Unknown { .. })
        {
            (
                empty_text("raw_asr_origin_or_completion_unconfirmed"),
                vec![],
            )
        } else if let Some(old) = reused {
            (old.text.clone(), old.literals.clone())
        } else if let Some(value) = known {
            (
                text_scores(input, stage, value, &mut work),
                literal_scores(input, stage, value),
            )
        } else {
            (empty_text("stage_text_unobserved"), vec![])
        };
        let counts = denominator(
            row,
            input,
            stage,
            &text,
            &state,
            matches!(metrics.normalized_cer.state, StageState::Completed {}),
        );
        let source = if known.is_none() {
            "unknown_unobserved"
        } else if stage == ScoreStage::Final && row.fallback.is_some() {
            "current_local_fallback"
        } else if replayed(row, stage) {
            "historical_candidate_current_local_rules"
        } else {
            match stage {
                ScoreStage::ProviderAsr => "current_parsed_provider",
                ScoreStage::FilteredAsr => "current_provider_postfilter",
                ScoreStage::Prepared => "current_local_preparation",
                ScoreStage::ProviderCleanup => "explicit_captured_parsed_reply",
                ScoreStage::AcceptedCleanup => "current_adapter_accepted_candidate",
                ScoreStage::Final => {
                    if row.fallback.is_some() {
                        "current_local_fallback"
                    } else {
                        "current_production_finalizer"
                    }
                }
            }
        };
        stages.push(StageScore {
            stage,
            source: source.into(),
            state,
            text_sha256: sha,
            text: metrics,
            literals,
            denominator: counts,
        });
    }
    let conditional = input.cleanup_applicable()
        && row.asr_terminal == "speech_text"
        && matches!(&row.filtered_text,ObservedText::KnownValue{value} if !value.trim().is_empty());
    let (negative_outcome, negative_kind) = if !input.negative {
        (na(), None)
    } else if matches!(row.state, StageState::Cancelled { .. }) {
        (row.state.clone(), Some("unresolved".into()))
    } else if row
        .selected_asr_observation()
        .is_some_and(|o| o.confirmed_empty())
    {
        (
            StageState::Completed {},
            Some(
                if row
                    .selected_asr_observation()
                    .is_some_and(|o| o.terminal == "locally_suppressed")
                {
                    "locally_suppressed"
                } else if row
                    .selected_asr_observation()
                    .is_some_and(|o| o.terminal == "local_complete_empty")
                {
                    "local_complete_empty"
                } else {
                    "provider_empty"
                }
                .into(),
            ),
        )
    } else if matches!(&row.filtered_text,ObservedText::KnownValue{value} if !value.is_empty()) {
        (
            StageState::Failed {
                reason: "negative_false_output".into(),
            },
            Some("false_output".into()),
        )
    } else {
        (
            if matches!(row.state, StageState::Completed {}) {
                unknown("negative_empty_origin_not_confirmed")
            } else {
                row.state.clone()
            },
            Some(
                if matches!(row.state, StageState::Completed {}) {
                    "unknown_empty_origin"
                } else {
                    "unresolved"
                }
                .into(),
            ),
        )
    };
    row.scores = Some(CaseScores {
        input_sha256: input.input_sha256.clone(),
        stages,
        cleanup_conditional_eligible: conditional,
        negative_outcome,
        negative_kind,
        distance_method: DISTANCE_METHOD.into(),
        word_updates: work,
    });
}

fn add(to: &mut ReportDenominator, from: &ReportDenominator) {
    macro_rules! sum { ($($f:ident),*) => { $(to.$f += from.$f;)* }; }
    sum!(
        eligible,
        requested,
        called,
        completed,
        nonempty,
        failed,
        cancelled,
        unknown,
        not_applicable,
        not_run,
        fresh,
        replayed,
        fallback,
        mixed,
        scored
    );
}
fn stage_name(stage: ScoreStage) -> &'static str {
    match stage {
        ScoreStage::ProviderAsr => "provider_asr",
        ScoreStage::FilteredAsr => "filtered_asr",
        ScoreStage::Prepared => "prepared",
        ScoreStage::ProviderCleanup => "provider_cleanup",
        ScoreStage::AcceptedCleanup => "accepted_cleanup",
        ScoreStage::Final => "final",
    }
}
fn distribution(mut values: Vec<f64>) -> TimingDistribution {
    values.retain(|v| v.is_finite() && *v >= 0.0);
    TimingDistribution {
        count: values.len() as u64,
        p50_ms: super::quantile::linear_quantile(&values, 0.5),
        p95_ms: super::quantile::linear_quantile(&values, 0.95),
        maximum_ms: values.iter().copied().max_by(f64::total_cmp),
    }
}
fn workflow(row: &CaseRecordV1) -> ReportDenominator {
    let input = row.score_input.as_ref();
    let cached = row.asr_replay.is_some() || row.cleanup_replay.is_some();
    ReportDenominator {
        eligible: u64::from(input.is_some_and(|i| !i.negative)),
        requested: 1,
        called: u64::from(!row.attempt_facts.is_empty()),
        completed: u64::from(matches!(row.state, StageState::Completed {})),
        nonempty: u64::from(
            matches!(if input.is_some_and(|i|i.asr_only.is_some()){&row.filtered_text}else{&row.final_text},ObservedText::KnownValue{value} if !value.trim().is_empty()),
        ),
        failed: u64::from(matches!(row.state, StageState::Failed { .. })),
        cancelled: u64::from(matches!(row.state, StageState::Cancelled { .. })),
        unknown: u64::from(matches!(row.state, StageState::Unknown { .. })),
        not_applicable: u64::from(matches!(row.state, StageState::NotApplicable {})),
        not_run: u64::from(matches!(row.state, StageState::NotRun {})),
        fresh: u64::from(!cached && !row.attempt_facts.is_empty()),
        replayed: u64::from(cached && row.attempt_facts.is_empty()),
        fallback: u64::from(row.fallback.is_some()),
        mixed: u64::from(cached && !row.attempt_facts.is_empty()),
        scored: u64::from(row.scores.as_ref().is_some_and(|s| {
            s.stages.iter().any(|s| {
                s.stage
                    == (if input.is_some_and(|i| i.asr_only.is_some()) {
                        ScoreStage::FilteredAsr
                    } else {
                        ScoreStage::Final
                    })
                    && s.denominator.scored > 0
            })
        })),
    }
}
fn job_summary(id: &str, rows: &[&CaseRecordV1]) -> JobScores {
    let mut result = JobScores {
        job_id: id.into(),
        requested_cases: rows.len() as u64,
        asr_success_cases: 0,
        workflow: ReportDenominator::default(),
        stages: BTreeMap::new(),
        stage_quality: BTreeMap::new(),
        cleanup_asr_success_conditional: ReportDenominator::default(),
        all_speech_e2e: ReportDenominator::default(),
        nonempty_final_proxy_rate_all_speech: rate(0, 0),
        accepted_cleanup_rate_asr_success: rate(0, 0),
        negative: BTreeMap::new(),
        fresh_latency_ms: BTreeMap::new(),
        replay_local_latency_ms: BTreeMap::new(),
        mixed_local_latency_ms: BTreeMap::new(),
        local_adapter_starts: 0,
        retry_starts: 0,
        observed_cleanup_usage_units: BTreeMap::new(),
        usage_coverage_cases: 0,
        unavailable: BTreeMap::new(),
    };
    let mut fresh: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut cached: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut mixed: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut unavailable_usage = BTreeSet::new();
    if rows
        .iter()
        .any(|r| r.score_input.as_ref().is_some_and(|i| i.asr_only.is_some()))
    {
        result.unavailable.insert("asr_only_scope".into(),unknown("ASR-only workflow/ASR-stage denominators; cleanup/final E2E inapplicable; source conversation/template independence unknown; no CI/holdout/ranking"));
    }
    for row in rows {
        let counts = workflow(row);
        add(&mut result.workflow, &counts);
        if row
            .score_input
            .as_ref()
            .is_some_and(ScoreInput::cleanup_applicable)
        {
            add(&mut result.all_speech_e2e, &counts);
        }
        if let Some(scores) = &row.scores {
            for stage in &scores.stages {
                add(
                    result
                        .stages
                        .entry(stage_name(stage.stage).into())
                        .or_default(),
                    &stage.denominator,
                );
            }
            if row
                .score_input
                .as_ref()
                .is_some_and(|i| i.asr_only.is_some())
                && counts.nonempty > 0
                && matches!(row.state, StageState::Completed {})
            {
                result.asr_success_cases += 1;
            }
            if scores.cleanup_conditional_eligible {
                result.asr_success_cases += 1;
                if let Some(stage) = scores
                    .stages
                    .iter()
                    .find(|s| s.stage == ScoreStage::AcceptedCleanup)
                {
                    add(
                        &mut result.cleanup_asr_success_conditional,
                        &stage.denominator,
                    );
                }
            }
            if row.score_input.as_ref().is_some_and(|i| i.negative) {
                *result.negative.entry("requested".into()).or_default() += 1;
                for (name, count) in [
                    ("eligible", 1),
                    ("called", counts.called),
                    ("completed", counts.completed),
                    (
                        "nonempty",
                        u64::from(
                            matches!(&row.filtered_text,ObservedText::KnownValue{value} if !value.is_empty()),
                        ),
                    ),
                    ("task_failed", counts.failed),
                    ("fresh", counts.fresh),
                    ("replayed", counts.replayed),
                    ("mixed", counts.mixed),
                ] {
                    *result.negative.entry(name.into()).or_default() += count;
                }
                let presence = |text: &ObservedText| match text {
                    ObservedText::KnownEmpty {} => "empty",
                    ObservedText::KnownValue { .. } => "nonempty",
                    ObservedText::Unknown { .. } => "unknown",
                };
                *result
                    .negative
                    .entry(format!(
                        "observed_provider_{}",
                        presence(&row.provider_text)
                    ))
                    .or_default() += 1;
                *result
                    .negative
                    .entry(format!(
                        "observed_postfilter_{}",
                        presence(&row.filtered_text)
                    ))
                    .or_default() += 1;

                let outcome = match scores.negative_outcome {
                    StageState::Completed {} => "confirmed_expected_empty",
                    StageState::Failed { .. } => "failed",
                    StageState::Cancelled { .. } => "cancelled",
                    StageState::NotRun {} => "not_run",
                    StageState::Unknown { .. } => "unknown",
                    StageState::NotApplicable {} => "not_applicable",
                };
                *result.negative.entry(outcome.into()).or_default() += 1;
                *result
                    .negative
                    .entry(scores.negative_kind.clone().unwrap_or("unresolved".into()))
                    .or_default() += 1;
            }
        }
        result.local_adapter_starts += row.attempt_facts.len() as u64;
        result.retry_starts += row.attempt_facts.iter().filter(|f| f.ordinal > 1).count() as u64;
        if let Some(observation) = &row.cleanup_observation {
            if row.cleanup_replay.is_none() && !observation.usage.is_empty() {
                result.usage_coverage_cases += 1;
                for (unit, value) in &observation.usage {
                    let sum = result
                        .observed_cleanup_usage_units
                        .get(unit)
                        .copied()
                        .unwrap_or(0.0)
                        + value;
                    if value.is_finite()
                        && *value >= 0.0
                        && sum.is_finite()
                        && !unavailable_usage.contains(unit)
                    {
                        result
                            .observed_cleanup_usage_units
                            .insert(unit.clone(), sum);
                    } else {
                        unavailable_usage.insert(unit.clone());
                        result.observed_cleanup_usage_units.remove(unit);
                        result.unavailable.insert(
                            format!("cleanup_usage_unit:{unit}"),
                            unknown("invalid_or_unrepresentable_usage_aggregate"),
                        );
                    }
                }
            }
        }
        for (name, value) in &row.timings_ms {
            // Presence is a current local producing timing, not an LLM call.
            // Provider fields separately require a fresh matching adapter start.
            let has_asr = row
                .attempt_facts
                .iter()
                .any(|f| f.stage == AttemptStage::Asr);
            let has_cleanup = row
                .attempt_facts
                .iter()
                .any(|f| f.stage == AttemptStage::Cleanup);
            let origin = match name.as_str() {
                "asr" if has_asr && row.asr_replay.is_none() => Some(0),
                "cleanup_provider" | "cleanup_provider_wait" | "cleanup_retry_wait"
                    if has_cleanup && row.cleanup_replay.is_none() =>
                {
                    Some(0)
                }
                "local_replay_attestation" | "preparation" => {
                    Some(if row.asr_replay.is_some() { 1 } else { 0 })
                }
                "finalize" | "cleanup_total" => Some(
                    if row.asr_replay.is_some() || row.cleanup_replay.is_some() {
                        if row.attempt_facts.is_empty() {
                            1
                        } else {
                            2
                        }
                    } else {
                        0
                    },
                ),
                _ => None,
            };
            let target = match origin {
                Some(0) => &mut fresh,
                Some(1) => &mut cached,
                Some(2) => &mut mixed,
                _ => continue,
            };
            target.entry(name.clone()).or_default().push(*value);
        }
    }
    for stage in STAGES {
        result
            .stage_quality
            .insert(stage_name(stage).into(), quality_partitions(rows, stage));
    }
    let languages: BTreeSet<_> = rows
        .iter()
        .filter_map(|r| {
            r.score_input
                .as_ref()?
                .asr_only
                .as_ref()
                .map(|s| s.language.as_str())
        })
        .collect();
    for language in languages {
        let selected: Vec<_> = rows
            .iter()
            .copied()
            .filter(|r| {
                r.score_input
                    .as_ref()
                    .and_then(|i| i.asr_only.as_ref())
                    .is_some_and(|s| s.language == language)
            })
            .collect();
        for stage in [ScoreStage::ProviderAsr, ScoreStage::FilteredAsr] {
            let name = format!("asr_speech:{language}:{}", stage_name(stage));
            let mut denominator = ReportDenominator::default();
            for row in &selected {
                if let Some(s) = row
                    .scores
                    .as_ref()
                    .and_then(|s| s.stages.iter().find(|s| s.stage == stage))
                {
                    add(&mut denominator, &s.denominator);
                }
            }
            result.stages.insert(name.clone(), denominator);
            result
                .stage_quality
                .insert(name, quality_partitions(&selected, stage));
        }
    }
    result.nonempty_final_proxy_rate_all_speech = rate(
        result.all_speech_e2e.nonempty,
        result.all_speech_e2e.requested,
    );
    result.unavailable.insert(
        "human_usable_final_rate_all_speech".into(),
        unknown("human_task_usability_not_reviewed; legacy_usable_final_rate_all_speech_is_nonempty_proxy_only"),
    );
    result.accepted_cleanup_rate_asr_success = rate(
        result.cleanup_asr_success_conditional.nonempty,
        result.cleanup_asr_success_conditional.requested,
    );
    result.fresh_latency_ms = fresh
        .into_iter()
        .map(|(k, v)| (k, distribution(v)))
        .collect();
    result.replay_local_latency_ms = cached
        .into_iter()
        .map(|(k, v)| (k, distribution(v)))
        .collect();
    result.mixed_local_latency_ms = mixed
        .into_iter()
        .map(|(k, v)| (k, distribution(v)))
        .collect();
    // Stable empty distributions make zero-sample quantiles explicit.
    for key in [
        "asr",
        "cleanup_provider",
        "cleanup_total",
        "preparation",
        "finalize",
        "cleanup_provider_wait",
        "cleanup_retry_wait",
    ] {
        result
            .fresh_latency_ms
            .entry(key.into())
            .or_insert_with(|| distribution(vec![]));
        result
            .replay_local_latency_ms
            .entry(key.into())
            .or_insert_with(|| distribution(vec![]));
        result
            .mixed_local_latency_ms
            .entry(key.into())
            .or_insert_with(|| distribution(vec![]));
    }
    for key in [
        "cold_warm_state",
        "asr_usage_units",
        "failed_retry_usage",
        "transport_submission",
        "server_receipt",
        "process_family_peak_rss",
        "whole_task_cpu",
        "monetary_cost",
    ] {
        result.unavailable.insert(
            key.into(),
            unknown("not_instrumented_by_current_file_report"),
        );
    }
    for key in ["submit", "readback", "stop_to_insert"] {
        result.unavailable.insert(key.into(), na());
    }
    result
}
fn empty_pair(spec: Option<ComparisonSpec>) -> PairedStatistics {
    PairedStatistics{state:if spec.is_some(){StageState::NotRun{}}else{na()},method:"paired-case-mean_then_single-declared-cluster-mean_percentile-bootstrap-sha256-counter-v1".into(),spec,requested_left_rows:0,requested_right_rows:0,matched_repeat_pairs:0,eligible_cases:0,excluded_cases:0,eligible_clusters:0,mean_right_minus_left:None,percentile_95:None,coverage:"descriptive declared groups only; no multiway/pilot/holdout certification".into()}
}

/// Run-local workflow outcomes for the declared pair. This is deliberately
/// derived from CaseRecordV1 rows and is not part of the persisted schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PairedWorkflowFailures {
    pub requested_left_rows: usize,
    pub requested_right_rows: usize,
    pub union_rows: usize,
    pub classified_rows: usize,
    pub unresolved_rows: usize,
    pub left_only_failures: usize,
    pub right_only_failures: usize,
    pub both_failures: usize,
    pub unavailable_reason: Option<&'static str>,
}

impl PairedWorkflowFailures {
    pub(super) fn coverage(&self) -> &'static str {
        if self.unavailable_reason.is_some() {
            "unknown"
        } else if self.unresolved_rows == 0 {
            "complete"
        } else {
            "partial_unknown"
        }
    }
}

pub(super) fn paired_workflow_failures(report: &RunRecord) -> Option<PairedWorkflowFailures> {
    let summary = report.scoring.as_ref()?;
    summary.comparison_spec.as_ref()?;
    Some(derive_paired_workflow_failures(
        &report.cases,
        summary.comparison_job_ids.as_ref(),
        matches!(
            report.terminal,
            StageState::Completed {} | StageState::Failed { .. } | StageState::Cancelled { .. }
        ),
        report
            .replay
            .as_ref()
            .is_none_or(|replay| replay.current_quality_eligible),
    ))
}

pub(super) fn paired_workflow_report_line(report: &RunRecord) -> Option<String> {
    let workflow = paired_workflow_failures(report)?;
    if let Some(reason) = workflow.unavailable_reason {
        return Some(format!(
            "Paired workflow failures: unavailable=unknown reason={reason} requested_rows={}/{} union_case_repeat_rows={} classified={} unresolved={} coverage={}",
            workflow.requested_left_rows,
            workflow.requested_right_rows,
            workflow.union_rows,
            workflow.classified_rows,
            workflow.unresolved_rows,
            workflow.coverage(),
        ));
    }
    Some(format!(
        "Paired workflow failures: unit=case_id+repeat left_only={} right_only={} both={} requested_rows={}/{} union_case_repeat_rows={} classified={} unresolved={} coverage={}",
        workflow.left_only_failures,
        workflow.right_only_failures,
        workflow.both_failures,
        workflow.requested_left_rows,
        workflow.requested_right_rows,
        workflow.union_rows,
        workflow.classified_rows,
        workflow.unresolved_rows,
        workflow.coverage(),
    ))
}

pub(super) fn paired_filter_quality_report_line(report: &RunRecord) -> Option<String> {
    format_filter_quality(report, None)
}

fn format_filter_quality(
    report: &RunRecord,
    evidence: Option<&InternalFilterEvidence>,
) -> Option<String> {
    let summary = report.scoring.as_ref()?;
    summary.comparison_spec.as_ref()?;
    let quality = derive_filter_quality(report, evidence);
    let metric = |value: Option<f64>| {
        value
            .map(|v| format!("{v:.6}"))
            .unwrap_or_else(|| "unknown".into())
    };
    let observed = |count: u64, n: u64| {
        if n == 0 {
            "unknown".into()
        } else {
            format!("{count}/{n}")
        }
    };
    let count = |value: Option<u64>| {
        value
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unknown".into())
    };
    let coverage = |reviewed: u64, expected: Option<u64>| {
        expected
            .map(|n| format!("{reviewed}/{n}"))
            .unwrap_or_else(|| "unknown".into())
    };
    let owner = if quality.owner_eligibility_unknown > 0 {
        "unknown".into()
    } else {
        observed(quality.owner_false_rejects, quality.owner_n)
    };
    let reviewed_expected = quality.union_rows.map(|n| n.saturating_mul(2));
    let reviewed_count = |value: u64, reviewed: u64| {
        if reviewed_expected.is_some_and(|expected| expected > 0 && reviewed == expected) {
            value.to_string()
        } else {
            "unknown".into()
        }
    };
    let terminal = matches!(
        report.terminal,
        StageState::Completed {} | StageState::Failed { .. } | StageState::Cancelled { .. }
    );
    let current_quality = report
        .replay
        .as_ref()
        .is_none_or(|r| r.current_quality_eligible);
    let reason = if summary.comparison_job_ids.is_none() {
        "comparison_job_identity_missing"
    } else if !terminal {
        "run_nonterminal"
    } else if !current_quality {
        "replay_quality_ineligible"
    } else if evidence.is_none() {
        "filter_evidence_missing"
    } else if evidence.is_some_and(|e| {
        summary.comparison_job_ids.as_ref().is_some_and(|ids| {
            e.off_job_id.as_deref() != Some(ids[0].as_str())
                || e.on_job_id.as_deref() != Some(ids[1].as_str())
        })
    }) {
        "filter_evidence_job_identity_mismatch"
    } else if quality.state == "unknown" {
        "filter_quality_incomplete_or_invalid"
    } else {
        "none"
    };
    let provenance = if evidence.is_none() {
        "unknown"
    } else if quality.observed_rows == 0 {
        "internal_no_observed_pairs"
    } else if quality.synthetic_rows == 0 {
        "internal_observed_non_synthetic_pairs"
    } else if quality.synthetic_rows == quality.observed_rows {
        "internal_observed_synthetic_pairs"
    } else {
        "internal_observed_mixed_pairs"
    };
    Some(format!(
        "Paired filter quality: state={} reason={} actual_selected_audio={} cer={}/{} cer_on_minus_off={} cer_missing_bound={} wer={}/{} wer_on_minus_off={} wer_missing_bound={} binary_errors={} known_eligible_n={} eligible_n={} binary_rate={} owner_false_reject={} owner_eligibility_unknown={} final_bystander_leaks={} final_bystander_reviewed_arm_units={} head_damage_spans={} head_reviewed_arm_units={} tail_damage_spans={} tail_reviewed_arm_units={} number_damage_spans={} number_reviewed_arm_units={} entity_damage_spans={} entity_reviewed_arm_units={} requested_rows={}/{} union_case_repeat_rows={} observed_pairs={} unknown_pairs={} eligibility_unknown={} synthetic_pairs={} synthetic_provenance={}",
        quality.state, reason,
        if quality.observed_rows > 0 { "bound" } else { "unknown" },
        metric(quality.cer_off), metric(quality.cer_on), metric(quality.cer_on_minus_off), if quality.cer_unbounded { "unbounded" } else { "bounded" },
        if quality.wer_applicable { metric(quality.wer_off) } else if quality.wer_unbounded || quality.observed_rows == 0 { "unknown".into() } else { "not_applicable".into() },
        if quality.wer_applicable { metric(quality.wer_on) } else if quality.wer_unbounded || quality.observed_rows == 0 { "unknown".into() } else { "not_applicable".into() },
        metric(quality.wer_on_minus_off), if quality.wer_unbounded { "unbounded" } else { "bounded" },
        quality.binary_errors, quality.known_eligible_rows, quality.eligible_n.map(|v| v.to_string()).unwrap_or_else(|| "unknown".into()), metric(quality.binary_rate),
        owner, quality.owner_eligibility_unknown,
        reviewed_count(quality.final_bystander_leaks, quality.final_bystander_observed), coverage(quality.final_bystander_observed, reviewed_expected),
        reviewed_count(quality.damage_by_kind_count[0], quality.damage_by_kind_observed[0]), coverage(quality.damage_by_kind_observed[0], reviewed_expected),
        reviewed_count(quality.damage_by_kind_count[1], quality.damage_by_kind_observed[1]), coverage(quality.damage_by_kind_observed[1], reviewed_expected),
        reviewed_count(quality.damage_by_kind_count[2], quality.damage_by_kind_observed[2]), coverage(quality.damage_by_kind_observed[2], reviewed_expected),
        reviewed_count(quality.damage_by_kind_count[3], quality.damage_by_kind_observed[3]), coverage(quality.damage_by_kind_observed[3], reviewed_expected),
        count(quality.requested_off), count(quality.requested_on), count(quality.union_rows),
        quality.observed_rows, quality.unknown_rows, quality.eligibility_unknown,
        quality.synthetic_rows, provenance,
    ))
}

fn derive_paired_workflow_failures(
    rows: &[CaseRecordV1],
    ids: Option<&[String; 2]>,
    terminal: bool,
    current_quality: bool,
) -> PairedWorkflowFailures {
    let empty = || PairedWorkflowFailures {
        requested_left_rows: 0,
        requested_right_rows: 0,
        union_rows: 0,
        classified_rows: 0,
        unresolved_rows: 0,
        left_only_failures: 0,
        right_only_failures: 0,
        both_failures: 0,
        unavailable_reason: None,
    };
    let Some([left_id, right_id]) = ids else {
        let mut result = empty();
        result.unavailable_reason = Some("comparison_job_identity_missing");
        return result;
    };
    let left: BTreeMap<_, _> = rows
        .iter()
        .filter(|row| row.job_id == *left_id)
        .map(|row| ((row.case_id.as_str(), row.repeat), row))
        .collect();
    let right: BTreeMap<_, _> = rows
        .iter()
        .filter(|row| row.job_id == *right_id)
        .map(|row| ((row.case_id.as_str(), row.repeat), row))
        .collect();
    let keys: BTreeSet<_> = left.keys().chain(right.keys()).copied().collect();
    let mut result = empty();
    result.requested_left_rows = left.len();
    result.requested_right_rows = right.len();
    result.union_rows = keys.len();
    if keys.is_empty() {
        result.unavailable_reason = Some("comparison_has_no_requested_rows");
        return result;
    }
    if !terminal {
        result.unavailable_reason = Some("run_not_terminal");
        return result;
    }
    if !current_quality {
        result.unavailable_reason = Some("replay_policy_excludes_current_workflow_classification");
        return result;
    }
    for key in keys {
        let (Some(left_row), Some(right_row)) = (left.get(&key), right.get(&key)) else {
            result.unresolved_rows += 1;
            continue;
        };
        if left_row.asr_replay.is_some()
            || left_row.cleanup_replay.is_some()
            || right_row.asr_replay.is_some()
            || right_row.cleanup_replay.is_some()
        {
            result.unresolved_rows += 1;
            continue;
        }
        match (&left_row.state, &right_row.state) {
            (StageState::Completed {}, StageState::Completed {}) => {
                result.classified_rows += 1;
            }
            (StageState::Failed { .. }, StageState::Completed {}) => {
                result.classified_rows += 1;
                result.left_only_failures += 1;
            }
            (StageState::Completed {}, StageState::Failed { .. }) => {
                result.classified_rows += 1;
                result.right_only_failures += 1;
            }
            (StageState::Failed { .. }, StageState::Failed { .. }) => {
                result.classified_rows += 1;
                result.both_failures += 1;
            }
            _ => result.unresolved_rows += 1,
        }
    }
    result
}

fn pair_summary(
    rows: &[CaseRecordV1],
    spec: Option<ComparisonSpec>,
    ids: Option<[String; 2]>,
    terminal: bool,
    current_quality: bool,
) -> PairedStatistics {
    let mut result = empty_pair(spec.clone());
    let Some(spec) = spec else {
        return result;
    };
    let Some([left_id, right_id]) = ids else {
        result.state = unknown("comparison_job_identity_missing");
        return result;
    };
    let left: BTreeMap<_, _> = rows
        .iter()
        .filter(|r| r.job_id == left_id)
        .map(|r| ((r.case_id.as_str(), r.repeat), r))
        .collect();
    let right: BTreeMap<_, _> = rows
        .iter()
        .filter(|r| r.job_id == right_id)
        .map(|r| ((r.case_id.as_str(), r.repeat), r))
        .collect();
    result.requested_left_rows = left.len() as u64;
    result.requested_right_rows = right.len() as u64;
    if !terminal {
        return result;
    }
    if !current_quality {
        result.state = unknown("quality_replay_run_excluded_from_current_paired_quality");
        return result;
    }
    let mut cases: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    let mut groups: BTreeMap<&str, String> = BTreeMap::new();
    let mut all = BTreeSet::new();
    for row in rows
        .iter()
        .filter(|r| r.job_id == left_id || r.job_id == right_id)
    {
        all.insert(row.case_id.as_str());
    }
    for ((case, repeat), a) in left {
        let Some(b) = right.get(&(case, repeat)) else {
            continue;
        };
        if a.asr_replay.is_some()
            || a.cleanup_replay.is_some()
            || b.asr_replay.is_some()
            || b.cleanup_replay.is_some()
        {
            continue;
        }
        let value = |r: &CaseRecordV1| {
            r.scores
                .as_ref()?
                .stages
                .iter()
                .find(|s| s.stage == spec.stage && matches!(s.state, StageState::Completed {}))?
                .text
                .normalized_cer
                .rate
        };
        let (Some(a_value), Some(b_value)) = (value(a), value(b)) else {
            continue;
        };
        let (Some(a_corpus), Some(b_corpus)) = (
            a.score_input.as_ref().and_then(|s| s.corpus.as_ref()),
            b.score_input.as_ref().and_then(|s| s.corpus.as_ref()),
        ) else {
            continue;
        };
        let group = |c: &CorpusCaseV1| match spec.cluster_by {
            ClusterDimension::Speaker => c.speaker_id.clone(),
            ClusterDimension::SourceSession => c.source_session_id.clone(),
            ClusterDimension::TemplateFamily => c.template_family_id.clone(),
        };
        let id = group(a_corpus);
        if id != group(b_corpus) {
            result.state = unknown("paired_group_identity_mismatch");
            return result;
        }
        if groups
            .insert(case, id.clone())
            .is_some_and(|previous| previous != id)
        {
            result.state = unknown("ambiguous_group_across_repeats");
            return result;
        }
        cases.entry(case).or_default().push(b_value - a_value);
        result.matched_repeat_pairs += 1;
    }
    result.eligible_cases = cases.len() as u64;
    result.excluded_cases = all.len() as u64 - result.eligible_cases;
    let mut clusters: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for (case, values) in cases {
        clusters
            .entry(groups[case].clone())
            .or_default()
            .push(values.iter().sum::<f64>() / values.len() as f64);
    }
    let means: Vec<f64> = clusters
        .values()
        .map(|v| v.iter().sum::<f64>() / v.len() as f64)
        .collect();
    result.eligible_clusters = means.len() as u64;
    if means.is_empty() {
        result.state = unknown("no_fresh_paired_scored_cases_with_declared_cluster_identity");
        return result;
    }
    result.mean_right_minus_left = Some(means.iter().sum::<f64>() / means.len() as f64);
    if means.len() < 2 {
        result.state = unknown("at_least_two_declared_clusters_required_for_interval");
        return result;
    }
    let mut resamples = Vec::with_capacity(spec.bootstrap_replicates as usize);
    let mut counter = 0u64;
    for _ in 0..spec.bootstrap_replicates {
        let mut sum = 0.0;
        for _ in 0..means.len() {
            let bytes = [spec.seed.to_le_bytes(), counter.to_le_bytes()].concat();
            counter += 1;
            let digest = validation::sha256(&bytes);
            let draw = u64::from_str_radix(&digest[..16], 16).expect("SHA hex owner");
            sum += means[(draw % means.len() as u64) as usize];
        }
        resamples.push(sum / means.len() as f64);
    }
    resamples.sort_by(f64::total_cmp);
    let last = resamples.len() - 1;
    result.percentile_95 = Some([
        resamples[(0.025 * last as f64).floor() as usize],
        resamples[(0.975 * last as f64).ceil() as usize],
    ]);
    result.state = StageState::Completed {};
    result
}
pub(super) fn refresh_summary(report: &mut RunRecord) {
    let Some(mut summary) = report.scoring.take() else {
        return;
    };
    let mut jobs: BTreeMap<&str, Vec<&CaseRecordV1>> = BTreeMap::new();
    for row in &report.cases {
        jobs.entry(&row.job_id).or_default().push(row);
    }
    summary.jobs = jobs
        .into_iter()
        .map(|(id, rows)| job_summary(id, &rows))
        .collect();
    summary.paired = pair_summary(
        &report.cases,
        summary.comparison_spec.clone(),
        summary.comparison_job_ids.clone(),
        matches!(
            report.terminal,
            StageState::Completed {} | StageState::Failed { .. } | StageState::Cancelled { .. }
        ),
        report
            .replay
            .as_ref()
            .is_none_or(|r| r.current_quality_eligible),
    );
    report.scoring = Some(summary);
}

fn quality_partitions(rows: &[&CaseRecordV1], stage: ScoreStage) -> QualityPartitions {
    fn aggregate(rows: &[&CaseRecordV1], stage: ScoreStage, partition: u8) -> QualityAggregate {
        let selected: Vec<&StageScore> = rows
            .iter()
            .filter(|r| {
                let cached = replayed(r, stage);
                let fresh = stage_started(r, stage);
                match partition {
                    0 => !cached && fresh,
                    1 => cached && !fresh,
                    _ => cached && fresh,
                }
            })
            .filter_map(|r| r.scores.as_ref()?.stages.iter().find(|s| s.stage == stage))
            .filter(|s| matches!(s.state, StageState::Completed {}))
            .collect();
        let metric = |pick: fn(&TextScores) -> &EditScore| {
            let mut result = MetricAggregate {
                samples: 0,
                edits: 0,
                reference_units: 0,
                micro_rate: None,
                mean_case_rate: None,
            };
            let mut sum = 0.0;
            for stage in &selected {
                let value = pick(&stage.text);
                if let (Some(edits), Some(units), Some(rate)) =
                    (value.edits, value.reference_units, value.rate)
                {
                    result.samples += 1;
                    result.edits += edits;
                    result.reference_units += units;
                    sum += rate;
                }
            }
            if result.reference_units > 0 {
                result.micro_rate = Some(result.edits as f64 / result.reference_units as f64);
            }
            if result.samples > 0 {
                result.mean_case_rate = Some(sum / result.samples as f64);
            }
            result
        };
        let mut exact = rate(0, 0);
        let mut literals = rate(0, 0);
        for stage in &selected {
            if let Some(value) = stage.text.exact_accepted_reference {
                exact.denominator += 1;
                exact.numerator += u64::from(value);
            }
            for literal in &stage.literals {
                if let Some(value) = literal.bounded_exact_match {
                    literals.denominator += 1;
                    literals.numerator += u64::from(value);
                }
            }
        }
        QualityAggregate {
            raw_scalar_cer: metric(|m| &m.raw_scalar_cer),
            normalized_cer: metric(|m| &m.normalized_cer),
            english_wer: metric(|m| &m.english_wer),
            chinese_cer: metric(|m| &m.chinese_cer),
            mixed_token_error_rate: metric(|m| &m.mixed_token_error_rate),
            exact_reference_proxy: rate(exact.numerator, exact.denominator),
            declared_literal_proxy: rate(literals.numerator, literals.denominator),
        }
    }
    QualityPartitions {
        fresh: aggregate(rows, stage, 0),
        historical: aggregate(rows, stage, 1),
        mixed: aggregate(rows, stage, 2),
    }
}

#[cfg(test)]
#[path = "scoring_tests.rs"]
mod tests;

#[cfg(test)]
mod timing_quantile_tests {
    use super::distribution;

    #[test]
    fn scoring_distribution_uses_shared_linear_quantiles_and_empty_none() {
        let summary = distribution(vec![100.0, 0.0, 20.0, 10.0, -1.0, f64::NAN]);
        assert_eq!(summary.count, 4);
        assert_eq!(summary.p50_ms, Some(15.0));
        assert!((summary.p95_ms.unwrap() - 88.0).abs() < 1e-9);
        let empty = distribution(Vec::new());
        assert_eq!(empty.count, 0);
        assert_eq!(empty.p50_ms, None);
        assert_eq!(empty.p95_ms, None);
    }
}
