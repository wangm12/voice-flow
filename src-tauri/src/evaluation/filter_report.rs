//! Explicit offline consumer for SHA-bound paired filter evidence.
//! This mode never connects to admission progress or admits a runner.
use super::{
    artifacts::AtomicArtifact,
    contracts::{AudioIdentity, CaseRecordV1, ObservedText, RunRecord},
    scoring::{
        self, FilterSelection, InternalDamageKind, InternalDamageReview, InternalFilterArmEvidence,
        InternalFilterEvidence, InternalFinalReview,
    },
    validation,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

const REPORT_TYPE: &str = "voiceflow_filter_quality_report";
const MAX_REPORT_BYTES: usize = super::artifacts::MAX_ARTIFACT_BYTES;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    artifact_type: String,
    schema_version: u32,
    #[serde(default)]
    evidence_id: Option<String>,
    #[serde(default)]
    evidence_source: Option<String>,
    #[serde(default)]
    quality_eligibility: Option<String>,
    input_runs: Vec<InputRun>,
    roster: Vec<RosterEntry>,
    pairs: Vec<PairEvidence>,
    #[serde(default)]
    comparability: Option<Comparability>,
    acceptance_policy: serde_json::Value,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputRun {
    arm: String,
    relative_path: String,
    sha256: String,
    run_id: String,
    selected_job_id: String,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
struct RosterEntry {
    source_case_id: String,
    repeat: u32,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RowBinding {
    case_id: String,
    repeat: u32,
    job_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PairEvidence {
    source_case_id: String,
    repeat: u32,
    #[serde(default)]
    cluster: Option<serde_json::Value>,
    original_audio: AudioIdentity,
    filtered_audio: FilteredAudio,
    #[serde(default)]
    human_gold: Option<HumanGold>,
    #[serde(default)]
    comparability: Option<serde_json::Value>,
    arms: BTreeMap<String, ArmEvidence>,
    #[serde(default)]
    damage_annotations: Vec<DamageAnnotation>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FilteredAudio {
    state: String,
    identity: Option<AudioIdentity>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArmEvidence {
    row_binding: RowBinding,
    selection: SelectionEvidence,
    quality_text: QualityText,
    #[serde(default)]
    final_attribution: Option<FinalAttribution>,
    owner_false_reject: Option<OwnerTrial>,
    #[serde(default)]
    timing: Option<serde_json::Value>,
    #[serde(default)]
    trial_proof: Option<ReceiptRef>,
}
#[derive(Debug)]
struct RowEvidence {
    original_audio: AudioIdentity,
    filtered_audio: Option<AudioIdentity>,
    selected_audio: AudioIdentity,
    selection: Selection,
    final_attribution: Option<FinalAttribution>,
    attribution_receipt_complete: bool,
    human_gold: Option<HumanGold>,
    owner_false_reject: Option<OwnerTrial>,
    damage_annotations: Vec<DamageAnnotation>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Selection {
    Original,
    Filtered,
    RawFallback,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectionEvidence {
    state: String,
    selected: String,
    selected_audio_sha256: String,
    reason: String,
    filter_fallback: serde_json::Value,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct QualityText {
    field: String,
    utf8_sha256: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptRef {
    relative_path: String,
    sha256: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FinalAttribution {
    #[serde(default)]
    receipt: Option<ReceiptRef>,
    #[serde(default)]
    final_text_utf8_sha256: String,
    #[serde(default)]
    all_output_examined: bool,
    #[serde(default)]
    utf8_ranges: Vec<[u32; 2]>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct HumanGold {
    #[serde(default)]
    state: String,
    #[serde(default)]
    reviewer_ids: Vec<String>,
    #[serde(default)]
    protocol_id: String,
    #[serde(default)]
    locked_revision: String,
    #[serde(default)]
    receipt: Option<ReceiptRef>,
    #[serde(default)]
    audio_sha256: String,
    #[serde(default)]
    reference_binding: serde_json::Value,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerTrial {
    event: bool,
    eligible: bool,
    trial_unit: String,
    protocol: String,
    roster: String,
    independence: String,
    receipt: ReceiptRef,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DamageAnnotation {
    kind: String,
    #[serde(default)]
    reference_target: Option<String>,
    reference_sha256: String,
    #[serde(default)]
    reference_range: Option<[u32; 2]>,
    assessment: DamageAssessment,
    #[serde(default)]
    receipt: Option<ReceiptRef>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DamageAssessment {
    filter_off: DamageArmAssessment,
    filter_on: DamageArmAssessment,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DamageArmAssessment {
    state: String,
    final_utf8_range: Option<[u32; 2]>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Comparability {
    #[serde(default)]
    same_original_audio_gold_asr_cleanup_controls_except_selected_audio: Option<bool>,
    #[serde(default)]
    receipt: Option<ReceiptRef>,
}
#[derive(Debug, Serialize)]
struct Criterion {
    state: &'static str,
    n: usize,
    unknown: usize,
    coverage: f64,
    method: &'static str,
    unit: &'static str,
}
#[derive(Debug, Serialize)]
struct Pair {
    case_id: String,
    repeat: u32,
    cer_off: Option<f64>,
    cer_on: Option<f64>,
    cer_on_minus_off: Option<f64>,
    cer_off_state: &'static str,
    cer_on_state: &'static str,
    cer_state: &'static str,
    cer_upper_bound: &'static str,
    wer_off: Option<f64>,
    wer_on: Option<f64>,
    wer_on_minus_off: Option<f64>,
    wer_off_state: &'static str,
    wer_on_state: &'static str,
    wer_state: &'static str,
    wer_upper_bound: &'static str,
    synthetic: bool,
    gold_state: &'static str,
    final_review_coverage: String,
}
#[derive(Debug, Serialize)]
struct DamageKindReport {
    reviewed_arm_units: usize,
    expected_arm_units: usize,
    observed_subset_spans: usize,
    observed_subset_errors: usize,
    state: &'static str,
}
#[derive(Debug, Serialize)]
struct OwnerMetric {
    eligible_n: usize,
    false_rejects: usize,
    unknown_trials: usize,
    state: &'static str,
}
#[derive(Debug, Serialize)]
struct BinaryConfidence {
    state: &'static str,
    requested_rows: usize,
    not_applicable_rows: usize,
    applicable_rows: usize,
    unknown_eligibility_rows: usize,
    unknown_qualification_rows: usize,
    qualified_subset_unique_trials: usize,
    independent_n: Option<usize>,
    observed_event_rows: usize,
    unknown_event_rows: usize,
    observed_error_rows: usize,
    conservative_errors: Option<usize>,
    upper_95: Option<f64>,
    method: &'static str,
    tail_alpha: f64,
    unit: &'static str,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct TrialIdentity {
    id: String,
    proof: String,
}
#[derive(Debug, Clone)]
struct BernoulliTrial {
    applicable: Option<bool>,
    qualified: bool,
    event: Option<bool>,
    identity: Option<TrialIdentity>,
}
fn trial_admission(
    eligibility: Option<bool>,
    proof_complete: bool,
    independent: bool,
    owner_declaration_complete: bool,
) -> (Option<bool>, bool) {
    let applicable = proof_complete.then_some(eligibility).flatten();
    let qualified = applicable == Some(true) && independent && owner_declaration_complete;
    (applicable, qualified)
}

fn known_eligibility_conflict(
    left: Option<&serde_json::Value>,
    right: Option<&serde_json::Value>,
) -> bool {
    ["owner_false_reject", "bystander_final"].iter().any(|key| {
        left.and_then(|value| value.get(*key))
            .and_then(serde_json::Value::as_bool)
            .zip(
                right
                    .and_then(|value| value.get(*key))
                    .and_then(serde_json::Value::as_bool),
            )
            .is_some_and(|(a, b)| a != b)
    })
}

fn validate_trial_eligibility_facts(
    evidence_path: &Path,
    proof: &serde_json::Value,
    row: &CaseRecordV1,
    owner: Option<&OwnerTrial>,
) -> Result<(), validation::InputError> {
    let proof_eligibility = proof.get("eligibility");
    if proof_eligibility
        .filter(|value| !value.is_null())
        .is_some_and(|value| !value.is_object())
        || proof_eligibility
            .and_then(serde_json::Value::as_object)
            .is_some_and(|eligibility| {
                ["owner_false_reject", "bystander_final"].iter().any(|key| {
                    eligibility
                        .get(*key)
                        .filter(|value| !value.is_null())
                        .is_some_and(|value| !value.is_boolean())
                })
            })
    {
        return Err(err(
            evidence_path,
            "evidence_contract",
            "trial eligibility has the wrong JSON type",
        ));
    }
    if owner.is_some_and(|owner| {
        proof_eligibility
            .and_then(|value| value.get("owner_false_reject"))
            .and_then(serde_json::Value::as_bool)
            .is_some_and(|eligible| eligible != owner.eligible)
    }) {
        return Err(err(
            evidence_path,
            "identity_mismatch",
            "trial eligibility contradicts OwnerTrial eligibility",
        ));
    }
    let Some((roster, _)) = proof_child(evidence_path, proof, "roster")? else {
        return Ok(());
    };
    let trials = roster.get("trials");
    if trials
        .filter(|value| !value.is_null())
        .is_some_and(|value| !value.is_array())
    {
        return Err(err(
            evidence_path,
            "evidence_contract",
            "trial roster entries have the wrong JSON type",
        ));
    }
    let Some(trial_id) = proof.get("trial_id").and_then(serde_json::Value::as_str) else {
        return Ok(());
    };
    for declaration in trials
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|declaration| {
            declaration
                .get("trial_id")
                .and_then(serde_json::Value::as_str)
                == Some(trial_id)
        })
    {
        let binding = declaration.get("row_binding");
        if binding
            .filter(|value| !value.is_null())
            .is_some_and(|value| !value.is_object())
            || ["job_id", "case_id"].iter().any(|key| {
                binding
                    .and_then(|value| value.get(*key))
                    .filter(|value| !value.is_null())
                    .is_some_and(|value| !value.is_string())
            })
            || binding
                .and_then(|value| value.get("repeat"))
                .filter(|value| !value.is_null())
                .is_some_and(|value| !value.is_u64())
        {
            return Err(err(
                evidence_path,
                "evidence_contract",
                "trial roster entry contains a value of the wrong JSON type",
            ));
        }
        let eligibility = declaration.get("eligibility");
        if eligibility
            .filter(|value| !value.is_null())
            .is_some_and(|value| !value.is_object())
            || eligibility
                .and_then(serde_json::Value::as_object)
                .is_some_and(|eligibility| {
                    ["owner_false_reject", "bystander_final"].iter().any(|key| {
                        eligibility
                            .get(*key)
                            .filter(|value| !value.is_null())
                            .is_some_and(|value| !value.is_boolean())
                    })
                })
        {
            return Err(err(
                evidence_path,
                "evidence_contract",
                "roster trial row contains a value of the wrong JSON type",
            ));
        }
        let compatible = [
            (
                binding
                    .and_then(|value| value.get("job_id"))
                    .and_then(serde_json::Value::as_str),
                row.job_id.as_str(),
            ),
            (
                binding
                    .and_then(|value| value.get("case_id"))
                    .and_then(serde_json::Value::as_str),
                row.case_id.as_str(),
            ),
        ]
        .iter()
        .all(|(declared, expected)| declared.map_or(true, |declared| declared == *expected))
            && binding
                .and_then(|value| value.get("repeat"))
                .and_then(serde_json::Value::as_u64)
                .map_or(true, |repeat| repeat == u64::from(row.repeat));
        if !compatible {
            continue;
        }
        if known_eligibility_conflict(eligibility, proof_eligibility)
            || owner.is_some_and(|owner| {
                eligibility
                    .and_then(|value| value.get("owner_false_reject"))
                    .and_then(serde_json::Value::as_bool)
                    .is_some_and(|eligible| eligible != owner.eligible)
            })
        {
            return Err(err(
                evidence_path,
                "identity_mismatch",
                "roster and trial eligibility declarations disagree",
            ));
        }
    }
    Ok(())
}
fn finalize_binary_confidence(trials: &[BernoulliTrial]) -> BinaryConfidence {
    let not_applicable_rows = trials
        .iter()
        .filter(|t| t.applicable == Some(false))
        .count();
    let applicable_rows = trials.iter().filter(|t| t.applicable == Some(true)).count();
    let unknown_eligibility_rows = trials.iter().filter(|t| t.applicable.is_none()).count();
    let eligible: Vec<_> = trials
        .iter()
        .filter(|t| t.applicable == Some(true))
        .collect();
    let mut unknown_qualification_rows = eligible.iter().filter(|t| !t.qualified).count();
    let qualified: Vec<_> = eligible.iter().filter(|t| t.qualified).collect();
    let observed_event_rows = eligible.iter().filter(|t| t.event.is_some()).count();
    let unknown_event_rows = eligible.len() - observed_event_rows;
    let observed_error_rows = eligible.iter().filter(|t| t.event == Some(true)).count();
    let mut independent_trials: BTreeMap<&str, (&str, Option<bool>)> = BTreeMap::new();
    let mut identity_less_errors = 0usize;
    let mut identity_less_n = 0usize;
    let mut conflicting_ids = BTreeSet::new();
    let mut event_conflicting_ids = BTreeSet::new();
    for trial in &qualified {
        let Some(identity) = trial.identity.as_ref() else {
            identity_less_n += 1;
            if trial.event != Some(false) {
                identity_less_errors += 1;
            }
            continue;
        };
        match independent_trials.get_mut(identity.id.as_str()) {
            Some((proof, event)) if *proof == identity.proof.as_str() => {
                if let (Some(previous), Some(current)) = (*event, trial.event) {
                    if previous != current {
                        event_conflicting_ids.insert(identity.id.as_str());
                    }
                }
                *event = match (*event, trial.event) {
                    (Some(previous), Some(current)) if previous == current => Some(previous),
                    _ => None,
                };
            }
            Some(_) => {
                conflicting_ids.insert(identity.id.as_str());
            }
            None => {
                independent_trials
                    .insert(identity.id.as_str(), (identity.proof.as_str(), trial.event));
            }
        }
    }
    unknown_qualification_rows += qualified
        .iter()
        .filter(|trial| {
            trial.identity.as_ref().is_some_and(|identity| {
                conflicting_ids.contains(identity.id.as_str())
                    || event_conflicting_ids.contains(identity.id.as_str())
            })
        })
        .count();
    let excluded_ids = conflicting_ids.union(&event_conflicting_ids).count();
    let identity_n = independent_trials.len().saturating_sub(excluded_ids);
    let identity_errors = independent_trials
        .iter()
        .filter(|(id, (_, event))| {
            !conflicting_ids.contains(*id)
                && !event_conflicting_ids.contains(*id)
                && *event != Some(false)
        })
        .count();
    let qualified_errors = identity_less_errors + identity_errors;
    let unique_qualified_n = identity_less_n + identity_n;
    let conservative_errors = if unknown_eligibility_rows == 0 && unknown_qualification_rows == 0 {
        Some(qualified_errors)
    } else {
        None
    };
    let independent_n = conservative_errors.map(|_| unique_qualified_n);
    let upper_95 = independent_n
        .zip(conservative_errors)
        .and_then(|(n, errors)| scoring::filter_clopper_pearson_upper(errors as u64, n as u64));
    let state =
        if trials.is_empty() || unknown_eligibility_rows > 0 || unknown_qualification_rows > 0 {
            "unknown"
        } else if applicable_rows == 0 {
            "not_applicable"
        } else {
            "observed"
        };
    BinaryConfidence {
        state,
        requested_rows: trials.len(),
        not_applicable_rows,
        applicable_rows,
        unknown_eligibility_rows,
        unknown_qualification_rows,
        qualified_subset_unique_trials: unique_qualified_n,
        independent_n,
        observed_event_rows,
        unknown_event_rows,
        observed_error_rows,
        conservative_errors,
        upper_95,
        method: "clopper_pearson_two_sided_95_upper",
        tail_alpha: 0.025,
        unit: "source_case_id_repeat",
    }
}
#[derive(Debug, Serialize)]
struct Report {
    artifact_type: &'static str,
    schema_version: u32,
    input_sha256: String,
    evidence_sha256: String,
    policy_sha256: String,
    provenance: String,
    status: &'static str,
    criteria: BTreeMap<&'static str, Criterion>,
    pairs: Vec<Pair>,
    descriptive_cer_delta: scoring::FilterDeltaInterval,
    descriptive_wer_delta: scoring::FilterDeltaInterval,
    false_reject_statistics: BinaryConfidence,
    bystander_final_statistics: BinaryConfidence,
    final_bystander_reviewed_arm_units: usize,
    final_bystander_expected_arm_units: usize,
    final_bystander_observed_subset_spans: usize,
    final_bystander_observed_subset_leaks: usize,
    damage_by_kind: BTreeMap<&'static str, DamageKindReport>,
    owner_false_reject: OwnerMetric,
}

fn filter_delta_observations(
    roster: &BTreeSet<RosterEntry>,
    pairs: &[PairEvidence],
    quality: &scoring::InternalFilterQuality,
    wer: bool,
) -> Vec<scoring::FilterDeltaObservation> {
    roster
        .iter()
        .map(|entry| {
            let pair = quality
                .per_pair
                .iter()
                .find(|pair| pair.case_id == entry.source_case_id && pair.repeat == entry.repeat);
            let evidence = pairs.iter().find(|pair| {
                pair.source_case_id == entry.source_case_id && pair.repeat == entry.repeat
            });
            let (off_state, on_state, delta) = match (pair, wer) {
                (Some(pair), false) => {
                    (pair.cer_off_state, pair.cer_on_state, pair.cer_on_minus_off)
                }
                (Some(pair), true) => {
                    (pair.wer_off_state, pair.wer_on_state, pair.wer_on_minus_off)
                }
                (None, _) => ("unknown", "unknown", None),
            };
            let state = if off_state == "not_applicable" && on_state == "not_applicable" {
                "not_applicable"
            } else if off_state == "observed" && on_state == "observed" && delta.is_some() {
                "observed"
            } else {
                "unknown"
            };
            let cluster = evidence.and_then(|pair| pair.cluster.as_ref());
            let provenance_ok = cluster
                .and_then(|cluster| cluster.get("provenance"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| !value.trim().is_empty());
            let speaker_id = cluster
                .filter(|_| provenance_ok)
                .and_then(|cluster| cluster.get("speaker_id"))
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned);
            let session_id = cluster
                .filter(|_| provenance_ok)
                .and_then(|cluster| cluster.get("source_session_id"))
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned);
            scoring::FilterDeltaObservation {
                case_id: entry.source_case_id.clone(),
                repeat: entry.repeat,
                speaker_id,
                session_id,
                delta: if state == "observed" { delta } else { None },
                state,
            }
        })
        .collect()
}

fn err(path: &Path, code: &'static str, detail: &'static str) -> validation::InputError {
    validation::error(path, code, detail)
}
fn resolve(owner: &Path, name: &str) -> Result<PathBuf, validation::InputError> {
    validation::relative_file(owner, name)
}
fn same_audio(a: &AudioIdentity, b: &AudioIdentity) -> bool {
    a.audio_sha256 == b.audio_sha256
        && a.pcm_sha256 == b.pcm_sha256
        && a.sample_count == b.sample_count
        && a.sample_rate == b.sample_rate
        && a.channels == b.channels
        && a.bits_per_sample == b.bits_per_sample
}
fn text(value: &ObservedText) -> Option<&str> {
    match value {
        ObservedText::KnownEmpty {} => Some(""),
        ObservedText::KnownValue { value } => Some(value),
        ObservedText::Unknown { .. } => None,
    }
}
fn validate_receipt(owner: &Path, receipt: &ReceiptRef) -> Result<String, validation::InputError> {
    if !validation::digest(&receipt.sha256) {
        return Err(err(
            owner,
            "evidence_contract",
            "receipt SHA-256 is malformed",
        ));
    }
    let path = resolve(owner, &receipt.relative_path)?;
    let bytes = validation::read_bounded(&path, super::artifacts::MAX_ARTIFACT_BYTES as u64)?;
    if validation::sha256(&bytes) != receipt.sha256 {
        return Err(err(
            &path,
            "sha256_mismatch",
            "selected receipt does not match its declared SHA-256",
        ));
    }
    let _: serde_json::Value = validation::decode_json(&path, &bytes)?;
    Ok(receipt.sha256.clone())
}
fn read_receipt(
    owner: &Path,
    receipt: &ReceiptRef,
) -> Result<serde_json::Value, validation::InputError> {
    let path = resolve(owner, &receipt.relative_path)?;
    let bytes = validation::read_bounded(&path, super::artifacts::MAX_ARTIFACT_BYTES as u64)?;
    if validation::sha256(&bytes) != receipt.sha256 {
        return Err(err(
            &path,
            "sha256_mismatch",
            "selected receipt does not match its declared SHA-256",
        ));
    }
    validation::decode_json(&path, &bytes)
}
fn criterion(n: usize, unknown: usize, method: &'static str, unit: &'static str) -> Criterion {
    Criterion {
        state: if unknown == 0 { "observed" } else { "unknown" },
        n,
        unknown,
        coverage: if n == 0 {
            0.0
        } else {
            (n - unknown) as f64 / n as f64
        },
        method,
        unit,
    }
}
fn bound_row<'a>(
    record: &'a RunRecord,
    job: &str,
    case: &str,
    repeat: u32,
) -> Result<&'a CaseRecordV1, validation::InputError> {
    let mut rows = record
        .cases
        .iter()
        .filter(|r| r.job_id == job && r.case_id == case && r.repeat == repeat);
    let row = rows.next().ok_or_else(|| {
        err(
            Path::new("RunRecord"),
            "evidence_contract",
            "row binding is absent from selected job",
        )
    })?;
    if rows.next().is_some() {
        return Err(err(
            Path::new("RunRecord"),
            "evidence_contract",
            "duplicate RunRecord row binding",
        ));
    }
    Ok(row)
}

fn validate_owner_trial_facts(
    evidence_path: &Path,
    owner: &OwnerTrial,
    row: &CaseRecordV1,
    proof: Option<&serde_json::Value>,
) -> Result<bool, validation::InputError> {
    let protocol = proof
        .map(|proof| proof_child(evidence_path, proof, "protocol"))
        .transpose()?
        .flatten();
    let roster = proof
        .map(|proof| proof_child(evidence_path, proof, "roster"))
        .transpose()?
        .flatten();
    let independence = proof
        .map(|proof| proof_child(evidence_path, proof, "independence"))
        .transpose()?
        .flatten();
    if proof
        .and_then(|proof| proof.get("trial_unit"))
        .filter(|v| !v.is_null())
        .is_some_and(|v| !v.is_string())
        || [
            protocol.as_ref().and_then(|(v, _)| v.get("protocol_id")),
            roster.as_ref().and_then(|(v, _)| v.get("roster_id")),
            independence
                .as_ref()
                .and_then(|(v, _)| v.get("independence_id")),
        ]
        .into_iter()
        .any(|value| {
            value
                .filter(|v| !v.is_null())
                .is_some_and(|v| !v.is_string())
        })
    {
        return Err(err(
            evidence_path,
            "evidence_contract",
            "trial proof DAG identifier has the wrong JSON type",
        ));
    }
    let receipt = read_receipt(evidence_path, &owner.receipt)?;
    let dag_fields = [
        (
            "trial_unit",
            proof
                .and_then(|proof| proof.get("trial_unit"))
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.trim().is_empty()),
        ),
        (
            "protocol",
            protocol
                .as_ref()
                .and_then(|(v, _)| v.get("protocol_id"))
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.trim().is_empty()),
        ),
        (
            "roster",
            roster
                .as_ref()
                .and_then(|(v, _)| v.get("roster_id"))
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.trim().is_empty()),
        ),
        (
            "independence",
            independence
                .as_ref()
                .and_then(|(v, _)| v.get("independence_id"))
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.trim().is_empty()),
        ),
    ];
    let owner_fields = [
        owner.trial_unit.as_str(),
        owner.protocol.as_str(),
        owner.roster.as_str(),
        owner.independence.as_str(),
    ];
    let mut complete = true;
    for ((key, dag_value), owner_value) in dag_fields.into_iter().zip(owner_fields) {
        let receipt_key = if key == "trial_unit" {
            "trial_unit"
        } else {
            key
        };
        let receipt_value = receipt.get(receipt_key);
        if receipt_value
            .filter(|v| !v.is_null())
            .is_some_and(|v| !v.is_string())
        {
            return Err(err(
                evidence_path,
                "evidence_contract",
                "owner false-reject receipt contains a value of the wrong JSON type",
            ));
        }
        let receipt_value = receipt_value
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty());
        if (!owner_value.is_empty() && dag_value.is_some_and(|actual| actual != owner_value))
            || receipt_value
                .zip(dag_value)
                .is_some_and(|(declared, actual)| declared != actual)
            || receipt_value
                .is_some_and(|declared| !owner_value.is_empty() && declared != owner_value)
        {
            return Err(err(
                evidence_path,
                "identity_mismatch",
                "OwnerTrial receipt or declaration contradicts trial proof DAG",
            ));
        }
        complete &= proof.is_some()
            && !owner_value.is_empty()
            && dag_value.is_some_and(|actual| actual == owner_value)
            && receipt_value == Some(owner_value);
    }
    for (key, actual) in [
        ("event", Some(owner.event)),
        ("eligible", Some(owner.eligible)),
    ] {
        let value = receipt.get(key);
        if value
            .filter(|v| !v.is_null())
            .is_some_and(|v| !v.is_boolean())
        {
            return Err(err(
                evidence_path,
                "evidence_contract",
                "owner false-reject receipt contains a value of the wrong JSON type",
            ));
        }
        let declared = value.and_then(serde_json::Value::as_bool);
        if declared
            .zip(actual)
            .is_some_and(|(declared, expected)| declared != expected)
        {
            return Err(err(
                evidence_path,
                "identity_mismatch",
                "OwnerTrial event or eligibility contradicts its receipt",
            ));
        }
        complete &= declared == actual;
    }
    for (key, expected) in [
        ("case_id", row.case_id.as_str()),
        ("job_id", row.job_id.as_str()),
    ] {
        let value = receipt.get(key);
        if value
            .filter(|v| !v.is_null())
            .is_some_and(|v| !v.is_string())
        {
            return Err(err(
                evidence_path,
                "evidence_contract",
                "owner false-reject receipt contains a value of the wrong JSON type",
            ));
        }
        let declared = value
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty());
        if declared.is_some_and(|declared| declared != expected) {
            return Err(err(
                evidence_path,
                "identity_mismatch",
                "owner false-reject receipt contradicts the selected row",
            ));
        }
        complete &= declared == Some(expected);
    }
    let repeat = receipt.get("repeat");
    if repeat.filter(|v| !v.is_null()).is_some_and(|v| !v.is_u64()) {
        return Err(err(
            evidence_path,
            "evidence_contract",
            "owner false-reject receipt contains a value of the wrong JSON type",
        ));
    }
    let declared_repeat = repeat.and_then(serde_json::Value::as_u64);
    if declared_repeat.is_some_and(|declared| declared != u64::from(row.repeat)) {
        return Err(err(
            evidence_path,
            "identity_mismatch",
            "owner false-reject receipt contradicts the selected row",
        ));
    }
    complete &= declared_repeat == Some(u64::from(row.repeat));
    Ok(complete)
}
fn valid_range(text: &str, range: [u32; 2]) -> bool {
    let (a, b) = (range[0] as usize, range[1] as usize);
    a <= b && b <= text.len() && text.is_char_boundary(a) && text.is_char_boundary(b)
}
fn proof_ref(
    owner: &Path,
    value: &serde_json::Value,
) -> Result<(serde_json::Value, String), validation::InputError> {
    let reference: ReceiptRef = serde_json::from_value(value.clone()).map_err(|_| {
        err(
            owner,
            "evidence_contract",
            "trial proof receipt reference is malformed",
        )
    })?;
    validate_receipt(owner, &reference)?;
    let body = read_receipt(owner, &reference)?;
    Ok((body, reference.sha256))
}
fn proof_child(
    owner: &Path,
    parent: &serde_json::Value,
    key: &str,
) -> Result<Option<(serde_json::Value, String)>, validation::InputError> {
    parent
        .get(key)
        .filter(|v| !v.is_null())
        .map(|v| proof_ref(owner, v))
        .transpose()
}
fn present_string_mismatch(value: Option<&serde_json::Value>, expected: &str) -> bool {
    value
        .filter(|v| !v.is_null())
        .is_some_and(|v| v.as_str() != Some(expected))
}
fn present_u64_mismatch(value: Option<&serde_json::Value>, expected: u64) -> bool {
    value
        .filter(|v| !v.is_null())
        .is_some_and(|v| v.as_u64() != Some(expected))
}
fn internal_arm(
    evidence: &RowEvidence,
    row: &CaseRecordV1,
    reference_sha256: &str,
    controls_sha256: &str,
    synthetic: bool,
    off_arm: bool,
    receipt_incomplete: &[InternalDamageKind],
) -> InternalFilterArmEvidence {
    let selected_text_sha256 = text(&row.filtered_text)
        .map(|t| validation::sha256(t.as_bytes()))
        .unwrap_or_default();
    let final_review = text(&row.final_text).and_then(|final_text| {
        evidence.final_attribution.as_ref().map(|attribution| {
            let final_text_sha256 = validation::sha256(final_text.as_bytes());
            let mut incomplete_damage_kinds = evidence
                .damage_annotations
                .iter()
                .filter_map(|item| {
                    let assessment = if off_arm {
                        &item.assessment.filter_off
                    } else {
                        &item.assessment.filter_on
                    };
                    if assessment.state == "unknown"
                        || item.receipt.is_none()
                        || item.reference_range.is_none()
                    {
                        Some(match item.kind.as_str() {
                            "head" => InternalDamageKind::Head,
                            "tail" => InternalDamageKind::Tail,
                            "number" => InternalDamageKind::Number,
                            "entity" => InternalDamageKind::Entity,
                            _ => return None,
                        })
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            incomplete_damage_kinds.extend_from_slice(receipt_incomplete);
            let damage = evidence
                .damage_annotations
                .iter()
                .filter_map(|item| {
                    item.receipt.as_ref()?;
                    let reference_range = item.reference_range?;
                    let assessment = if off_arm {
                        &item.assessment.filter_off
                    } else {
                        &item.assessment.filter_on
                    };
                    let kind = match item.kind.as_str() {
                        "head" => InternalDamageKind::Head,
                        "tail" => InternalDamageKind::Tail,
                        "number" => InternalDamageKind::Number,
                        "entity" => InternalDamageKind::Entity,
                        _ => return None,
                    };
                    if !["retained", "damaged"].contains(&assessment.state.as_str()) {
                        return None;
                    }
                    let final_range = assessment.final_utf8_range?;
                    Some(InternalDamageReview {
                        kind,
                        reference_sha256: item.reference_sha256.clone(),
                        final_text_sha256: final_text_sha256.clone(),
                        reference_range,
                        final_range,
                        damaged: assessment.state == "damaged",
                    })
                })
                .collect::<Vec<_>>();
            let attribution_reviewed = evidence.attribution_receipt_complete
                && attribution.receipt.is_some()
                && attribution.all_output_examined
                && attribution
                    .utf8_ranges
                    .iter()
                    .all(|r| r[0] < r[1] && valid_range(final_text, *r));
            InternalFinalReview {
                final_text_sha256,
                attribution_reviewed,
                damage_reviewed: !damage.is_empty(),
                bystander_ranges: attribution.utf8_ranges.clone(),
                damage,
                incomplete_damage_kinds,
            }
        })
    });
    let selection = match evidence.selection {
        Selection::Original => FilterSelection::Original,
        Selection::Filtered => FilterSelection::Filtered,
        Selection::RawFallback => FilterSelection::RawFallback,
    };
    InternalFilterArmEvidence {
        job_id: row.job_id.clone(),
        case_id: row.case_id.clone(),
        repeat: row.repeat,
        selected_audio: evidence.selected_audio.clone(),
        original_audio: evidence.original_audio.clone(),
        filtered_audio: evidence.filtered_audio.clone(),
        reference_sha256: reference_sha256.to_owned(),
        controls_sha256: controls_sha256.to_owned(),
        selected_text_sha256,
        selection,
        eligibility: None,
        outcome: None,
        owner_eligibility: evidence
            .owner_false_reject
            .as_ref()
            .map(|owner| owner.eligible),
        owner_false_reject: evidence
            .owner_false_reject
            .as_ref()
            .map(|owner| owner.event),
        final_review,
        synthetic,
    }
}
pub(super) fn run(evidence_arg: &Path, report_arg: &Path) -> Result<i32, validation::InputError> {
    let evidence_path = validation::external(evidence_arg)?;
    let evidence_bytes =
        validation::read_bounded(&evidence_path, super::artifacts::MAX_ARTIFACT_BYTES as u64)?;
    let evidence: Evidence = validation::decode_json(&evidence_path, &evidence_bytes)?;
    if evidence.artifact_type != "voiceflow_filter_evidence" || evidence.schema_version != 1 {
        return Err(err(
            &evidence_path,
            "schema",
            "unsupported filter evidence identity",
        ));
    }
    if evidence.input_runs.len() != 2 {
        return Err(err(
            &evidence_path,
            "evidence_contract",
            "exactly two runs are required",
        ));
    }
    let mut records: BTreeMap<String, (RunRecord, String)> = BTreeMap::new();
    for run in &evidence.input_runs {
        if !["filter_off", "filter_on"].contains(&run.arm.as_str())
            || !validation::digest(&run.sha256)
        {
            return Err(err(
                &evidence_path,
                "evidence_contract",
                "run arm or SHA-256 is invalid",
            ));
        }
        let path = resolve(&evidence_path, &run.relative_path)?;
        let bytes = validation::read_bounded(&path, super::artifacts::MAX_ARTIFACT_BYTES as u64)?;
        if validation::sha256(&bytes) != run.sha256 {
            return Err(err(
                &path,
                "sha256_mismatch",
                "selected RunRecord does not match its declared SHA-256",
            ));
        }
        let record: RunRecord = validation::decode_json(&path, &bytes)?;
        if record.run_id != run.run_id
            || !record.cases.iter().any(|r| r.job_id == run.selected_job_id)
        {
            return Err(err(
                &path,
                "identity_mismatch",
                "RunRecord identity or selected job binding is inconsistent",
            ));
        }
        if records
            .insert(run.arm.clone(), (record, run.sha256.clone()))
            .is_some()
        {
            return Err(err(
                &evidence_path,
                "evidence_contract",
                "duplicate run arm",
            ));
        }
    }
    if !records.contains_key("filter_off") || !records.contains_key("filter_on") {
        return Err(err(
            &evidence_path,
            "evidence_contract",
            "exactly one off and one on run are required",
        ));
    }
    let inputs: BTreeMap<_, _> = evidence
        .input_runs
        .iter()
        .map(|r| (r.arm.as_str(), r))
        .collect();
    let mut actual_union = BTreeSet::new();
    for run_arm in ["filter_off", "filter_on"] {
        let input = inputs[run_arm];
        let selected: Vec<_> = records[run_arm]
            .0
            .cases
            .iter()
            .filter(|r| r.job_id == input.selected_job_id)
            .collect();
        let selected_keys: BTreeSet<_> = selected
            .iter()
            .map(|r| RosterEntry {
                source_case_id: r.case_id.clone(),
                repeat: r.repeat,
            })
            .collect();
        if selected_keys.len() != selected.len() {
            return Err(err(
                &evidence_path,
                "identity_mismatch",
                "selected RunRecord job has duplicate case/repeat bindings",
            ));
        }
        actual_union.extend(selected_keys);
    }
    let pair_roster: BTreeSet<_> = evidence
        .pairs
        .iter()
        .map(|p| RosterEntry {
            source_case_id: p.source_case_id.clone(),
            repeat: p.repeat,
        })
        .collect();
    if pair_roster.len() != evidence.pairs.len() {
        return Err(err(
            &evidence_path,
            "evidence_contract",
            "duplicate pair key",
        ));
    }
    let declared: BTreeSet<_> = evidence.roster.iter().cloned().collect();
    if declared.len() != evidence.roster.len() {
        return Err(err(
            &evidence_path,
            "evidence_contract",
            "duplicate roster key",
        ));
    }
    if declared.is_empty()
        || (!evidence.pairs.is_empty() && declared != actual_union)
        || !pair_roster.is_subset(&declared)
    {
        return Err(err(
            &evidence_path,
            "evidence_contract",
            "declared roster must match the selected job union and pairs cannot add rows",
        ));
    }
    let mut comparability_receipt_confirms = false;
    if let Some(comparability) = &evidence.comparability {
        if comparability.same_original_audio_gold_asr_cleanup_controls_except_selected_audio
            == Some(false)
        {
            return Err(err(
                &evidence_path,
                "identity_mismatch",
                "comparability claim contradicts declared paired controls",
            ));
        }
        if let Some(receipt) = &comparability.receipt {
            validate_receipt(&evidence_path, receipt)?;
            let comparison_receipt = read_receipt(&evidence_path, receipt)?;
            if comparability.same_original_audio_gold_asr_cleanup_controls_except_selected_audio
                == Some(true)
                && comparison_receipt
                    .get("same_original_audio_gold_asr_cleanup_controls_except_selected_audio")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false)
            {
                return Err(err(
                    &evidence_path,
                    "identity_mismatch",
                    "comparability receipt does not confirm the declared control projection",
                ));
            }
            comparability_receipt_confirms = comparability
                .same_original_audio_gold_asr_cleanup_controls_except_selected_audio
                == Some(true)
                && comparison_receipt
                    .get("same_original_audio_gold_asr_cleanup_controls_except_selected_audio")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true);
        }
    }
    let kinds = ["head", "tail", "number", "entity"];
    let mut control_projections = Vec::new();
    let off_run_id = inputs["filter_off"].selected_job_id.as_str();
    let on_run_id = inputs["filter_on"].selected_job_id.as_str();
    let mut off_internal = InternalFilterEvidence {
        off_job_id: Some(off_run_id.to_owned()),
        on_job_id: Some(on_run_id.to_owned()),
        arms: Vec::new(),
    };
    let mut on_internal = InternalFilterEvidence {
        off_job_id: Some(off_run_id.to_owned()),
        on_job_id: Some(on_run_id.to_owned()),
        arms: Vec::new(),
    };
    let control_projection = |row: &CaseRecordV1| {
        serde_json::json!({
            "requested": &row.requested,
            "effective": &row.effective,
            "control_input_identities": &row.control_input_identities,
            "local_cleanup": &row.local_cleanup,
            "local_model": &row.local_model,
        })
    };
    let mut control_projection_complete = true;
    for entry in &declared {
        let off_row = records["filter_off"].0.cases.iter().find(|row| {
            row.job_id == off_run_id
                && row.case_id == entry.source_case_id
                && row.repeat == entry.repeat
        });
        let on_row = records["filter_on"].0.cases.iter().find(|row| {
            row.job_id == on_run_id
                && row.case_id == entry.source_case_id
                && row.repeat == entry.repeat
        });
        match (off_row, on_row) {
            (Some(off), Some(on)) => {
                let off_controls = control_projection(off);
                let on_controls = control_projection(on);
                if off_controls != on_controls {
                    return Err(err(
                        &evidence_path,
                        "identity_mismatch",
                        "paired RunRecord control projections differ",
                    ));
                }
                control_projections.push(serde_json::json!({"case_id":entry.source_case_id,"repeat":entry.repeat,"controls":off_controls}));
            }
            _ => control_projection_complete = false,
        }
    }
    let mut owner_cp_trials = Vec::new();
    let mut bystander_cp_trials = Vec::new();
    let mut owner_trial_facts_complete = BTreeMap::new();
    for entry in &declared {
        let mut row_by_arm = BTreeMap::new();
        let mut case_by_arm = BTreeMap::new();
        let mut pair_trial_id: Option<String> = None;
        let Some(pair) = evidence
            .pairs
            .iter()
            .find(|p| p.source_case_id == entry.source_case_id && p.repeat == entry.repeat)
        else {
            owner_cp_trials.push(BernoulliTrial {
                applicable: None,
                qualified: false,
                event: None,
                identity: None,
            });
            bystander_cp_trials.push(BernoulliTrial {
                applicable: None,
                qualified: false,
                event: None,
                identity: None,
            });
            continue;
        };
        let mut receipt_incomplete_damage: BTreeMap<&str, Vec<InternalDamageKind>> =
            BTreeMap::new();
        if pair.damage_annotations.iter().any(|d| {
            !["head", "tail", "number", "entity"].contains(&d.kind.as_str())
                || d.reference_target
                    .as_deref()
                    .is_some_and(|target| target != "asr_reference")
        }) {
            return Err(err(
                &evidence_path,
                "evidence_contract",
                "damage annotation kind or reference target is invalid",
            ));
        }
        if pair.damage_annotations.iter().any(|d| {
            [&d.assessment.filter_off, &d.assessment.filter_on]
                .iter()
                .any(|a| match a.state.as_str() {
                    "retained" | "damaged" => a.final_utf8_range.is_none(),
                    "unknown" => a.final_utf8_range.is_some(),
                    _ => true,
                })
        }) {
            return Err(err(
                &evidence_path,
                "evidence_contract",
                "damage assessment must declare a known span or an explicit unknown without a span",
            ));
        }
        if pair
            .comparability
            .as_ref()
            .is_some_and(|c| c.get("state").and_then(serde_json::Value::as_str) == Some("conflict"))
        {
            return Err(err(
                &evidence_path,
                "identity_mismatch",
                "pair comparability explicitly reports a contradiction",
            ));
        }
        if pair.human_gold.as_ref().is_some_and(|gold| {
            gold.state == "human_listened"
                && !gold.reviewer_ids.is_empty()
                && !gold.protocol_id.is_empty()
                && !gold.locked_revision.is_empty()
                && gold.receipt.is_some()
        }) && pair.cluster.is_none()
        {
            return Err(err(
                &evidence_path,
                "evidence_contract",
                "human Gold rows require a namespaced reviewed cluster binding",
            ));
        }
        for arm_name in ["filter_off", "filter_on"] {
            if let Some(proof) = pair
                .arms
                .get(arm_name)
                .and_then(|arm| arm.trial_proof.as_ref())
            {
                let proof_json =
                    serde_json::json!({"relative_path":proof.relative_path,"sha256":proof.sha256});
                let (body, _) = proof_ref(&evidence_path, &proof_json)?;
                let protocol = proof_child(&evidence_path, &body, "protocol")?;
                let roster = proof_child(&evidence_path, &body, "roster")?;
                let independence = proof_child(&evidence_path, &body, "independence")?;
                if let (Some((r, _)), Some((_, psha))) = (&roster, &protocol) {
                    if present_string_mismatch(r.get("protocol_sha256"), psha) {
                        return Err(err(
                            &evidence_path,
                            "identity_mismatch",
                            "trial roster declares a different protocol receipt SHA",
                        ));
                    }
                }
                if let (Some((i, _)), Some((_, psha))) = (&independence, &protocol) {
                    if present_string_mismatch(i.get("protocol_sha256"), psha) {
                        return Err(err(
                            &evidence_path,
                            "identity_mismatch",
                            "independence receipt declares a different protocol receipt SHA",
                        ));
                    }
                }
                if let (Some((i, _)), Some((_, rsha))) = (&independence, &roster) {
                    if present_string_mismatch(i.get("roster_sha256"), rsha) {
                        return Err(err(
                            &evidence_path,
                            "identity_mismatch",
                            "independence receipt declares a different roster receipt SHA",
                        ));
                    }
                }
            }
        }
        for (arm, run_arm) in [("off", "filter_off"), ("on", "filter_on")] {
            let input = inputs[run_arm];
            let rec = &records[run_arm].0;
            let Some(arm_ev) = pair.arms.get(run_arm) else {
                continue;
            };
            if arm_ev.row_binding.case_id != entry.source_case_id
                || arm_ev.row_binding.repeat != entry.repeat
                || arm_ev.row_binding.job_id != input.selected_job_id
            {
                return Err(err(
                    &evidence_path,
                    "evidence_contract",
                    "arm row binding does not match the selected run, job and roster",
                ));
            }
            let row = bound_row(
                rec,
                &input.selected_job_id,
                &entry.source_case_id,
                entry.repeat,
            )?;
            let owner_complete = if let Some(owner) = &arm_ev.owner_false_reject {
                if let Some(proof) = &arm_ev.trial_proof {
                    let proof_json = serde_json::json!({"relative_path":proof.relative_path,"sha256":proof.sha256});
                    let (proof_body, _) = proof_ref(&evidence_path, &proof_json)?;
                    validate_owner_trial_facts(&evidence_path, owner, row, Some(&proof_body))?
                } else {
                    validate_owner_trial_facts(&evidence_path, owner, row, None)?
                }
            } else {
                true
            };
            owner_trial_facts_complete.insert(arm.to_owned(), owner_complete);
            if let Some(proof) = &arm_ev.trial_proof {
                let proof_json =
                    serde_json::json!({"relative_path":proof.relative_path,"sha256":proof.sha256});
                let (proof_body, _) = proof_ref(&evidence_path, &proof_json)?;
                validate_trial_eligibility_facts(
                    &evidence_path,
                    &proof_body,
                    row,
                    arm_ev.owner_false_reject.as_ref(),
                )?;
            }
            if let Some(proof) = &arm_ev.trial_proof {
                let proof_json =
                    serde_json::json!({"relative_path":proof.relative_path,"sha256":proof.sha256});
                let (proof_body, _) = proof_ref(&evidence_path, &proof_json)?;
                let proof_binding = proof_body.get("row_binding");
                if proof_binding
                    .filter(|v| !v.is_null())
                    .is_some_and(|v| !v.is_object())
                    || present_string_mismatch(
                        proof_binding.and_then(|v| v.get("case_id")),
                        &row.case_id,
                    )
                    || present_u64_mismatch(
                        proof_binding.and_then(|v| v.get("repeat")),
                        u64::from(row.repeat),
                    )
                    || present_string_mismatch(
                        proof_binding.and_then(|v| v.get("job_id")),
                        &row.job_id,
                    )
                    || present_string_mismatch(
                        proof_body.get("run_sha256"),
                        records
                            .get(run_arm)
                            .map(|(_, sha)| sha.as_str())
                            .unwrap_or_default(),
                    )
                    || present_string_mismatch(
                        proof_body.get("original_audio_sha256"),
                        &pair.original_audio.audio_sha256,
                    )
                    || proof_body
                        .get("trial_unit")
                        .filter(|v| !v.is_null())
                        .is_some_and(|v| !v.is_string())
                {
                    return Err(err(
                        &evidence_path,
                        "identity_mismatch",
                        "trial proof does not bind its selected arm row, run and original audio",
                    ));
                }
                if proof_body
                    .get("trial_id")
                    .filter(|v| !v.is_null())
                    .is_some_and(|v| !v.is_string())
                {
                    return Err(err(
                        &evidence_path,
                        "identity_mismatch",
                        "trial proof trial_id has the wrong JSON type",
                    ));
                }
                if let Some(id) = proof_body
                    .get("trial_id")
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                {
                    if pair_trial_id
                        .as_deref()
                        .is_some_and(|previous| previous != id)
                    {
                        return Err(err(
                            &evidence_path,
                            "identity_mismatch",
                            "paired arm proofs declare different trial_id values",
                        ));
                    }
                    pair_trial_id = Some(id.to_owned());
                    // Repeated observations are retained at row level; independent-trial
                    // deduplication is applied only after the proof establishes a unit.
                }
                let roster = proof_child(&evidence_path, &proof_body, "roster")?;
                if let Some((roster_body, _)) = &roster {
                    if roster_body
                        .get("trials")
                        .filter(|v| !v.is_null())
                        .is_some_and(|v| !v.is_array())
                    {
                        return Err(err(
                            &evidence_path,
                            "evidence_contract",
                            "trial roster entries have the wrong JSON type",
                        ));
                    }
                    if let Some(id) = proof_body
                        .get("trial_id")
                        .and_then(serde_json::Value::as_str)
                        .filter(|s| !s.trim().is_empty())
                    {
                        let declarations: Vec<_> = roster_body
                            .get("trials")
                            .and_then(serde_json::Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter(|t| {
                                t.get("trial_id").and_then(serde_json::Value::as_str) == Some(id)
                            })
                            .collect();
                        let matching: Vec<_> = declarations
                            .iter()
                            .copied()
                            .filter(|t| {
                                t.get("row_binding")
                                    .and_then(|b| b.get("case_id"))
                                    .and_then(serde_json::Value::as_str)
                                    == Some(row.case_id.as_str())
                                    && t.get("row_binding")
                                        .and_then(|b| b.get("repeat"))
                                        .and_then(serde_json::Value::as_u64)
                                        == Some(u64::from(row.repeat))
                            })
                            .collect();
                        if roster_body
                            .get("trials")
                            .and_then(serde_json::Value::as_array)
                            .is_some_and(|items| {
                                items.iter().any(|t| {
                                    t.get("trial_id")
                                        .filter(|v| !v.is_null())
                                        .is_some_and(|v| !v.is_string())
                                        || t.get("row_binding")
                                            .filter(|v| !v.is_null())
                                            .is_some_and(|v| !v.is_object())
                                        || ["job_id", "case_id"].iter().any(|key| {
                                            t.get("row_binding")
                                                .and_then(|b| b.get(*key))
                                                .filter(|v| !v.is_null())
                                                .is_some_and(|v| !v.is_string())
                                        })
                                        || t.get("row_binding")
                                            .and_then(|b| b.get("repeat"))
                                            .filter(|v| !v.is_null())
                                            .is_some_and(|v| !v.is_u64())
                                        || ["run_sha256", "original_audio_sha256"].iter().any(
                                            |key| {
                                                t.get(*key)
                                                    .filter(|v| !v.is_null())
                                                    .is_some_and(|v| !v.is_string())
                                            },
                                        )
                                })
                            })
                        {
                            return Err(err(
                                &evidence_path,
                                "evidence_contract",
                                "trial roster entry contains a value of the wrong JSON type",
                            ));
                        }
                        if declarations.is_empty() || matching.is_empty() {
                            let known_conflict = declarations.iter().any(|t| {
                                t.get("run_sha256")
                                    .and_then(serde_json::Value::as_str)
                                    .is_some_and(|s| s != records[run_arm].1)
                                    || t.get("original_audio_sha256")
                                        .and_then(serde_json::Value::as_str)
                                        .is_some_and(|s| s != pair.original_audio.audio_sha256)
                                    || t.get("row_binding")
                                        .and_then(|b| b.get("job_id"))
                                        .and_then(serde_json::Value::as_str)
                                        .is_some_and(|s| s != row.job_id)
                                    || t.get("row_binding")
                                        .and_then(|b| b.get("case_id"))
                                        .and_then(serde_json::Value::as_str)
                                        .is_some_and(|s| s != row.case_id)
                                    || t.get("row_binding")
                                        .and_then(|b| b.get("repeat"))
                                        .and_then(serde_json::Value::as_u64)
                                        .is_some_and(|n| n != u64::from(row.repeat))
                            });
                            if known_conflict {
                                return Err(err(
                                    &evidence_path,
                                    "identity_mismatch",
                                    "roster entry contradicts selected trial row, run or audio",
                                ));
                            }
                        } else if matching.len() > 1 {
                            return Err(err(
                                &evidence_path,
                                "identity_mismatch",
                                "roster contains duplicate declarations for a trial_id",
                            ));
                        } else if let Some(rt) = matching.first() {
                            if rt
                                .get("row_binding")
                                .and_then(|b| b.get("job_id"))
                                .and_then(serde_json::Value::as_str)
                                .is_some_and(|job| job != row.job_id)
                                || rt
                                    .get("run_sha256")
                                    .and_then(serde_json::Value::as_str)
                                    .is_some_and(|sha| sha != records[run_arm].1)
                                || rt
                                    .get("original_audio_sha256")
                                    .and_then(serde_json::Value::as_str)
                                    .is_some_and(|sha| sha != pair.original_audio.audio_sha256)
                            {
                                return Err(err(
                                    &evidence_path,
                                    "identity_mismatch",
                                    "roster entry contradicts selected trial row, run or audio",
                                ));
                            }
                        }
                    }
                }
            }
            let selection = match (
                arm,
                arm_ev.selection.selected.as_str(),
                arm_ev.selection.reason.as_str(),
            ) {
                ("off", "original", _) => Selection::Original,
                ("on", "filtered", _) => Selection::Filtered,
                ("on", "original", "raw_audio_fallback") => Selection::RawFallback,
                _ => {
                    return Err(err(
                        &evidence_path,
                        "identity_mismatch",
                        "filter selection contradicts the declared arm",
                    ))
                }
            };
            let fallback_occurred = arm_ev
                .selection
                .filter_fallback
                .get("occurred")
                .and_then(serde_json::Value::as_bool);
            if (selection == Selection::RawFallback && fallback_occurred != Some(true))
                || (selection != Selection::RawFallback && fallback_occurred == Some(true))
            {
                return Err(err(
                    &evidence_path,
                    "identity_mismatch",
                    "filter fallback facts contradict selected audio",
                ));
            }
            let selected_audio = match selection {
                Selection::Original | Selection::RawFallback => pair.original_audio.clone(),
                Selection::Filtered if pair.filtered_audio.state == "observed" => {
                    pair.filtered_audio.identity.clone().ok_or_else(|| {
                        err(
                            &evidence_path,
                            "identity_missing",
                            "filtered selection has no audio identity",
                        )
                    })?
                }
                Selection::Filtered => {
                    return Err(err(
                        &evidence_path,
                        "identity_missing",
                        "filtered selection is not evidenced",
                    ))
                }
            };
            if arm_ev.selection.state != "observed"
                || arm_ev.selection.selected_audio_sha256 != selected_audio.audio_sha256
                || !same_audio(&selected_audio, &row.audio)
            {
                return Err(err(
                    &evidence_path,
                    "identity_mismatch",
                    "selected audio identity contradicts the RunRecord row or selection receipt",
                ));
            }
            match (&selection, arm) {
                (Selection::Original, "off")
                    if same_audio(&selected_audio, &pair.original_audio) => {}
                (Selection::Filtered, "on")
                    if pair
                        .filtered_audio
                        .identity
                        .as_ref()
                        .is_some_and(|a| same_audio(a, &selected_audio)) => {}
                (Selection::RawFallback, "on")
                    if same_audio(&selected_audio, &pair.original_audio) => {}
                _ => {
                    return Err(err(
                        &evidence_path,
                        "identity_mismatch",
                        "selected audio does not match the declared filter selection",
                    ))
                }
            }
            let observed_text = text(&row.filtered_text);
            if arm_ev.quality_text.field != "filtered_text" {
                return Err(err(
                    &evidence_path,
                    "evidence_contract",
                    "quality text must identify filtered_text",
                ));
            }
            if !validation::digest(&arm_ev.quality_text.utf8_sha256) {
                return Err(err(
                    &evidence_path,
                    "evidence_contract",
                    "filtered_text SHA-256 is malformed",
                ));
            }
            if observed_text.is_some_and(|t| {
                validation::sha256(t.as_bytes()) != arm_ev.quality_text.utf8_sha256
            }) {
                return Err(err(
                    &evidence_path,
                    "identity_mismatch",
                    "filtered_text SHA-256 contradicts row evidence",
                ));
            }
            if let Some(receipt) = pair
                .human_gold
                .as_ref()
                .and_then(|gold| gold.receipt.as_ref())
            {
                validate_receipt(&evidence_path, receipt)?;
            }
            if let Some(receipt) = arm_ev
                .final_attribution
                .as_ref()
                .and_then(|attribution| attribution.receipt.as_ref())
            {
                validate_receipt(&evidence_path, receipt)?;
            }
            if let Some(owner) = &arm_ev.owner_false_reject {
                validate_receipt(&evidence_path, &owner.receipt)?;
                if arm_ev.trial_proof.is_none() {
                    owner_trial_facts_complete.insert(arm.to_owned(), false);
                }
            }
            for damage in &pair.damage_annotations {
                if let Some(receipt) = &damage.receipt {
                    validate_receipt(&evidence_path, receipt)?;
                }
            }
            if pair.human_gold.as_ref().is_some_and(|gold| {
                !gold.audio_sha256.is_empty()
                    && gold.audio_sha256 != pair.original_audio.audio_sha256
            }) {
                return Err(err(
                    &evidence_path,
                    "identity_mismatch",
                    "Gold receipt audio identity contradicts original audio",
                ));
            }
            if let Some(gold) = pair.human_gold.as_ref() {
                if let Some(gold_receipt_ref) = gold.receipt.as_ref() {
                    let gold_receipt = read_receipt(&evidence_path, gold_receipt_ref)?;
                    let reference_sha = row
                        .score_input
                        .as_ref()
                        .and_then(|s| s.asr_reference.as_deref())
                        .map(|s| validation::sha256(s.as_bytes()));
                    if gold_receipt.get("original_audio_sha256").is_some_and(|v| {
                        v.as_str() != Some(pair.original_audio.audio_sha256.as_str())
                    }) || reference_sha
                        .as_deref()
                        .zip(
                            gold_receipt
                                .get("asr_reference_sha256")
                                .and_then(serde_json::Value::as_str),
                        )
                        .is_some_and(|(expected, actual)| expected != actual)
                        || gold_receipt.get("reviewer_ids").is_some_and(|v| {
                            !gold.reviewer_ids.is_empty()
                                && v != &serde_json::json!(&gold.reviewer_ids)
                        })
                        || gold_receipt.get("locked_revision").is_some_and(|v| {
                            !gold.locked_revision.is_empty()
                                && v.as_str() != Some(gold.locked_revision.as_str())
                        })
                        || gold_receipt.get("protocol_id").is_some_and(|v| {
                            !gold.protocol_id.is_empty()
                                && v.as_str() != Some(gold.protocol_id.as_str())
                        })
                    {
                        return Err(err(
                            &evidence_path,
                            "identity_mismatch",
                            "human Gold receipt contradicts declared review bindings",
                        ));
                    }
                }
            }
            let mut incomplete_attribution_receipt = false;
            if let Some(attribution) = &arm_ev.final_attribution {
                if !attribution.final_text_utf8_sha256.is_empty()
                    && !validation::digest(&attribution.final_text_utf8_sha256)
                {
                    return Err(err(
                        &evidence_path,
                        "evidence_contract",
                        "final attribution text SHA-256 is malformed",
                    ));
                }
                if let Some(final_text) = text(&row.final_text) {
                    if !attribution.final_text_utf8_sha256.is_empty()
                        && validation::sha256(final_text.as_bytes())
                            != attribution.final_text_utf8_sha256
                    {
                        return Err(err(
                            &evidence_path,
                            "identity_mismatch",
                            "final attribution SHA contradicts RunRecord row",
                        ));
                    }
                    if attribution
                        .utf8_ranges
                        .iter()
                        .any(|range| range[0] >= range[1] || !valid_range(final_text, *range))
                    {
                        return Err(err(
                            &evidence_path,
                            "evidence_contract",
                            "final attribution ranges or text SHA are invalid",
                        ));
                    }
                } else if attribution.all_output_examined || !attribution.utf8_ranges.is_empty() {
                    return Err(err(&evidence_path, "identity_mismatch", "final attribution claims a review that is not bound to observed RunRecord text"));
                }
                if let Some(receipt_ref) = &attribution.receipt {
                    let receipt = read_receipt(&evidence_path, receipt_ref)?;
                    if let Some(final_text) = text(&row.final_text) {
                        let final_sha = validation::sha256(final_text.as_bytes());
                        if receipt
                            .get("final_text_utf8_sha256")
                            .is_some_and(|v| v.as_str() != Some(final_sha.as_str()))
                            || receipt.get("all_output_examined").is_some_and(|v| {
                                v.as_bool() != Some(attribution.all_output_examined)
                            })
                            || receipt
                                .get("bystander_utf8_ranges")
                                .is_some_and(|v| v != &serde_json::json!(&attribution.utf8_ranges))
                        {
                            return Err(err(&evidence_path, "identity_mismatch", "final attribution receipt contradicts a present final text or review binding"));
                        }
                    }
                    incomplete_attribution_receipt =
                        receipt.get("final_text_utf8_sha256").is_none()
                            || receipt.get("all_output_examined").is_none()
                            || receipt.get("bystander_utf8_ranges").is_none();
                }
            }
            if let Some(reference) = row
                .score_input
                .as_ref()
                .and_then(|score| score.asr_reference.as_deref())
            {
                let reference_sha = validation::sha256(reference.as_bytes());
                let final_text = text(&row.final_text);
                for damage in &pair.damage_annotations {
                    let Some(reference_range) = damage.reference_range else {
                        continue;
                    };
                    if !valid_range(reference, reference_range)
                        || reference_range[0] == reference_range[1]
                    {
                        return Err(err(
                            &evidence_path,
                            "evidence_contract",
                            "damage annotation requires a valid nonempty reference range",
                        ));
                    }
                    if damage.reference_sha256 != reference_sha {
                        return Err(err(
                            &evidence_path,
                            "identity_mismatch",
                            "damage reference SHA contradicts selected RunRecord",
                        ));
                    }
                    let Some(final_text) = final_text else {
                        continue;
                    };
                    let status = if arm == "off" {
                        &damage.assessment.filter_off
                    } else {
                        &damage.assessment.filter_on
                    };
                    if status
                        .final_utf8_range
                        .is_some_and(|range| !valid_range(final_text, range))
                    {
                        return Err(err(
                            &evidence_path,
                            "evidence_contract",
                            "damage final range is not a UTF-8 boundary in its bound text",
                        ));
                    }
                    let Some(damage_receipt) = &damage.receipt else {
                        continue;
                    };
                    let receipt = read_receipt(&evidence_path, damage_receipt)?;
                    let arm_name = if arm == "off" {
                        "filter_off"
                    } else {
                        "filter_on"
                    };
                    let final_sha = validation::sha256(final_text.as_bytes());
                    let final_sha_binding = receipt
                        .get("final_text_utf8_sha256")
                        .and_then(serde_json::Value::as_object)
                        .and_then(|m| m.get(arm_name))
                        .and_then(serde_json::Value::as_str);
                    let reference_material =
                        &reference[reference_range[0] as usize..reference_range[1] as usize];
                    let material_sha = validation::sha256(reference_material.as_bytes());
                    if receipt
                        .get("reference_sha256")
                        .is_some_and(|v| v.as_str() != Some(reference_sha.as_str()))
                        || receipt
                            .get("kind")
                            .is_some_and(|v| v.as_str() != Some(damage.kind.as_str()))
                        || receipt
                            .get("reference_range")
                            .is_some_and(|v| v != &serde_json::json!(reference_range))
                        || receipt
                            .get("reference_material_sha256")
                            .is_some_and(|v| v.as_str() != Some(material_sha.as_str()))
                        || final_sha_binding.is_some_and(|sha| sha != final_sha)
                        || receipt
                            .pointer(&format!("/assessment/{arm_name}"))
                            .is_some_and(|v| v != &serde_json::json!(status))
                    {
                        return Err(err(&evidence_path, "identity_mismatch", "damage receipt contradicts a present reference, final text or arm assessment binding"));
                    }
                    if receipt.get("reference_sha256").is_none()
                        || receipt.get("kind").is_none()
                        || receipt.get("reference_range").is_none()
                        || receipt.get("reference_material_sha256").is_none()
                        || final_sha_binding.is_none()
                        || receipt
                            .pointer(&format!("/assessment/{arm_name}"))
                            .is_none()
                    {
                        let kind = match damage.kind.as_str() {
                            "head" => InternalDamageKind::Head,
                            "tail" => InternalDamageKind::Tail,
                            "number" => InternalDamageKind::Number,
                            "entity" => InternalDamageKind::Entity,
                            _ => continue,
                        };
                        let kinds = receipt_incomplete_damage.entry(run_arm).or_default();
                        if !kinds.contains(&kind) {
                            kinds.push(kind);
                        }
                        continue;
                    }
                }
            }
            let _timing_evidence = &arm_ev.timing;
            let ev = RowEvidence {
                original_audio: pair.original_audio.clone(),
                filtered_audio: pair.filtered_audio.identity.clone(),
                selected_audio,
                selection,
                final_attribution: arm_ev.final_attribution.clone(),
                attribution_receipt_complete: !incomplete_attribution_receipt,
                human_gold: pair.human_gold.clone(),
                owner_false_reject: arm_ev.owner_false_reject.clone(),
                damage_annotations: pair.damage_annotations.clone(),
            };
            row_by_arm.insert(arm, ev);
            case_by_arm.insert(arm, row);
        }
        if row_by_arm.len() != 2 {
            let mut owner_applicability = None;
            let mut bystander_applicability = None;
            let mut owner_qualified = false;
            let mut bystander_qualified = false;
            let on_event = row_by_arm.get("on").and_then(|ev| {
                let row = case_by_arm.get("on")?;
                let attribution = ev.final_attribution.as_ref()?;
                let final_text = text(&row.final_text)?;
                (ev.attribution_receipt_complete
                    && attribution.receipt.is_some()
                    && attribution.all_output_examined
                    && attribution.final_text_utf8_sha256
                        == validation::sha256(final_text.as_bytes()))
                .then_some(!attribution.utf8_ranges.is_empty())
            });
            let owner_event = row_by_arm
                .get("on")
                .and_then(|ev| ev.owner_false_reject.as_ref())
                .filter(|owner| owner.eligible)
                .map(|owner| owner.event);
            let mut trial_identity = None;
            if let (Some(ev), Some(row), Some(proof_ref_value)) = (
                row_by_arm.get("on"),
                case_by_arm.get("on"),
                pair.arms
                    .get("filter_on")
                    .and_then(|arm| arm.trial_proof.as_ref()),
            ) {
                let proof_json = serde_json::json!({"relative_path":proof_ref_value.relative_path,"sha256":proof_ref_value.sha256});
                let (body, _) = proof_ref(&evidence_path, &proof_json)?;
                let trial_id = body
                    .get("trial_id")
                    .and_then(serde_json::Value::as_str)
                    .filter(|id| !id.trim().is_empty());
                let protocol = proof_child(&evidence_path, &body, "protocol")?;
                let roster = proof_child(&evidence_path, &body, "roster")?;
                let independence = proof_child(&evidence_path, &body, "independence")?;
                if let (
                    Some(id),
                    Some(unit),
                    Some((_, protocol_sha)),
                    Some((_, roster_sha)),
                    Some((_, independence_sha)),
                ) = (
                    trial_id,
                    body.get("trial_unit")
                        .and_then(serde_json::Value::as_str)
                        .filter(|s| !s.is_empty()),
                    protocol.as_ref(),
                    roster.as_ref(),
                    independence.as_ref(),
                ) {
                    trial_identity = Some(TrialIdentity {
                        id: id.to_owned(),
                        proof: format!("{unit}|{protocol_sha}|{roster_sha}|{independence_sha}"),
                    });
                }
                let binding = body.get("row_binding");
                let matching: Vec<_> = roster
                    .as_ref()
                    .and_then(|(r, _)| r.get("trials").and_then(serde_json::Value::as_array))
                    .into_iter()
                    .flatten()
                    .filter(|t| {
                        trial_id.is_some_and(|id| {
                            t.get("trial_id").and_then(serde_json::Value::as_str) == Some(id)
                        }) && t
                            .get("row_binding")
                            .and_then(|b| b.get("case_id"))
                            .and_then(serde_json::Value::as_str)
                            == Some(row.case_id.as_str())
                            && t.get("row_binding")
                                .and_then(|b| b.get("repeat"))
                                .and_then(serde_json::Value::as_u64)
                                == Some(u64::from(row.repeat))
                    })
                    .collect();
                if matching.len() > 1 {
                    return Err(err(
                        &evidence_path,
                        "identity_mismatch",
                        "roster contains duplicate declarations for a trial_id",
                    ));
                }
                let roster_trial = matching.first().copied();
                if let Some(rt) = roster_trial {
                    if ["run_sha256", "original_audio_sha256"].iter().any(|key| {
                        rt.get(*key)
                            .filter(|v| !v.is_null())
                            .is_some_and(|v| !v.is_string())
                    }) || rt
                        .get("row_binding")
                        .filter(|v| !v.is_null())
                        .is_some_and(|v| !v.is_object())
                        || ["job_id", "case_id"].iter().any(|key| {
                            rt.get("row_binding")
                                .and_then(|b| b.get(*key))
                                .filter(|v| !v.is_null())
                                .is_some_and(|v| !v.is_string())
                        })
                        || rt
                            .get("row_binding")
                            .and_then(|b| b.get("repeat"))
                            .filter(|v| !v.is_null())
                            .is_some_and(|v| !v.is_u64())
                    {
                        return Err(err(
                            &evidence_path,
                            "evidence_contract",
                            "roster trial row contains a value of the wrong JSON type",
                        ));
                    }
                    if rt
                        .get("run_sha256")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|s| s != records["filter_on"].1)
                        || rt
                            .get("original_audio_sha256")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|s| s != pair.original_audio.audio_sha256)
                        || rt
                            .get("row_binding")
                            .and_then(|b| b.get("job_id"))
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|s| s != row.job_id)
                        || rt
                            .get("row_binding")
                            .and_then(|b| b.get("case_id"))
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|s| s != row.case_id)
                        || rt
                            .get("row_binding")
                            .and_then(|b| b.get("repeat"))
                            .and_then(serde_json::Value::as_u64)
                            .is_some_and(|n| n != u64::from(row.repeat))
                    {
                        return Err(err(
                            &evidence_path,
                            "identity_mismatch",
                            "roster entry contradicts selected trial row, run or audio",
                        ));
                    }
                }
                let identity_complete = trial_id.is_some()
                    && binding
                        .and_then(|v| v.get("job_id"))
                        .and_then(serde_json::Value::as_str)
                        == Some(row.job_id.as_str())
                    && binding
                        .and_then(|v| v.get("case_id"))
                        .and_then(serde_json::Value::as_str)
                        == Some(row.case_id.as_str())
                    && binding
                        .and_then(|v| v.get("repeat"))
                        .and_then(serde_json::Value::as_u64)
                        == Some(u64::from(row.repeat))
                    && body.get("run_sha256").and_then(serde_json::Value::as_str)
                        == Some(records["filter_on"].1.as_str())
                    && body
                        .get("original_audio_sha256")
                        .and_then(serde_json::Value::as_str)
                        == Some(pair.original_audio.audio_sha256.as_str())
                    && body
                        .get("trial_unit")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|s| !s.is_empty());
                let protocol_ok = protocol.as_ref().is_some_and(|(p, _)| {
                    p.get("trial_unit").and_then(serde_json::Value::as_str)
                        == body.get("trial_unit").and_then(serde_json::Value::as_str)
                        && p.get("false_reject_event")
                            .and_then(serde_json::Value::as_str)
                            == Some("human_owner_present_and_owner_output_rejected_by_filter")
                        && p.get("bystander_event").and_then(serde_json::Value::as_str)
                            == Some("human_attributed_bystander_content_in_actual_final_text")
                });
                let roster_ok = roster.as_ref().is_some_and(|(r, _)| {
                    r.get("protocol_sha256").and_then(serde_json::Value::as_str)
                        == protocol.as_ref().map(|(_, sha)| sha.as_str())
                        && roster_trial.is_some_and(|rt| {
                            rt.get("trial_id").and_then(serde_json::Value::as_str) == trial_id
                                && rt.get("run_sha256").and_then(serde_json::Value::as_str)
                                    == Some(records["filter_on"].1.as_str())
                                && rt
                                    .get("original_audio_sha256")
                                    .and_then(serde_json::Value::as_str)
                                    == Some(pair.original_audio.audio_sha256.as_str())
                                && rt
                                    .get("row_binding")
                                    .and_then(|b| b.get("job_id"))
                                    .and_then(serde_json::Value::as_str)
                                    == Some(row.job_id.as_str())
                                && rt
                                    .get("row_binding")
                                    .and_then(|b| b.get("case_id"))
                                    .and_then(serde_json::Value::as_str)
                                    == Some(row.case_id.as_str())
                                && rt
                                    .get("row_binding")
                                    .and_then(|b| b.get("repeat"))
                                    .and_then(serde_json::Value::as_u64)
                                    == Some(u64::from(row.repeat))
                                && rt.get("eligibility") == body.get("eligibility")
                        })
                });
                let eligibility_complete = body
                    .get("eligibility")
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|elig| {
                        ["owner_false_reject", "bystander_final"].iter().all(|key| {
                            elig.get(*key)
                                .and_then(serde_json::Value::as_bool)
                                .is_some()
                        })
                    });
                let base_ok = identity_complete && protocol_ok && roster_ok && eligibility_complete;
                if base_ok {
                    owner_applicability = body
                        .get("eligibility")
                        .and_then(|v| v.get("owner_false_reject"))
                        .and_then(serde_json::Value::as_bool);
                    bystander_applicability = body
                        .get("eligibility")
                        .and_then(|v| v.get("bystander_final"))
                        .and_then(serde_json::Value::as_bool);
                    let gold_complete = if let Some(gold) = ev.human_gold.as_ref() {
                        if gold.state == "human_listened"
                            && !gold.reviewer_ids.is_empty()
                            && !gold.protocol_id.is_empty()
                            && !gold.locked_revision.is_empty()
                        {
                            if let Some(receipt_ref) = gold.receipt.as_ref() {
                                let receipt = read_receipt(&evidence_path, receipt_ref)?;
                                let reference_sha = row
                                    .score_input
                                    .as_ref()
                                    .and_then(|s| s.asr_reference.as_deref())
                                    .map(|s| validation::sha256(s.as_bytes()));
                                receipt
                                    .get("original_audio_sha256")
                                    .and_then(serde_json::Value::as_str)
                                    == Some(pair.original_audio.audio_sha256.as_str())
                                    && reference_sha.as_deref().is_some_and(|expected| {
                                        receipt
                                            .get("asr_reference_sha256")
                                            .and_then(serde_json::Value::as_str)
                                            == Some(expected)
                                    })
                                    && receipt.get("reviewer_ids")
                                        == Some(&serde_json::json!(gold.reviewer_ids))
                                    && receipt
                                        .get("locked_revision")
                                        .and_then(serde_json::Value::as_str)
                                        == Some(gold.locked_revision.as_str())
                                    && receipt
                                        .get("protocol_id")
                                        .and_then(serde_json::Value::as_str)
                                        == Some(gold.protocol_id.as_str())
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    let off_row = records["filter_off"].0.cases.iter().find(|r| {
                        r.job_id == inputs["filter_off"].selected_job_id
                            && r.case_id == row.case_id
                            && r.repeat == row.repeat
                    });
                    let synthetic_trial = evidence.evidence_source.as_deref() == Some("synthetic")
                        || evidence.quality_eligibility.as_deref() == Some("synthetic_only")
                        || ev
                            .human_gold
                            .as_ref()
                            .is_some_and(|g| g.state == "synthetic_only")
                        || match off_row {
                            None => false,
                            Some(r) => {
                                r.score_input
                                    .as_ref()
                                    .is_some_and(|s| s.source_kind == "synthetic_fixture")
                                    || r.score_input
                                        .as_ref()
                                        .and_then(|s| s.corpus.as_ref())
                                        .is_some_and(|c| {
                                            matches!(
                                                &c.source_kind,
                                                super::contracts::SourceKind::Synthetic
                                            )
                                        })
                            }
                        }
                        || row
                            .score_input
                            .as_ref()
                            .is_some_and(|s| s.source_kind == "synthetic_fixture")
                        || row
                            .score_input
                            .as_ref()
                            .and_then(|s| s.corpus.as_ref())
                            .is_some_and(|c| {
                                matches!(&c.source_kind, super::contracts::SourceKind::Synthetic)
                            });
                    let independent = !synthetic_trial
                        && gold_complete
                        && base_ok
                        && trial_id.is_some()
                        && independence.as_ref().is_some_and(|(i, _)| {
                            i.get("protocol_sha256").and_then(serde_json::Value::as_str)
                                == protocol.as_ref().map(|(_, sha)| sha.as_str())
                                && i.get("roster_sha256").and_then(serde_json::Value::as_str)
                                    == roster.as_ref().map(|(_, sha)| sha.as_str())
                                && i.get("trial_unit").and_then(serde_json::Value::as_str)
                                    == body.get("trial_unit").and_then(serde_json::Value::as_str)
                                && i.get("independent").and_then(serde_json::Value::as_bool)
                                    == Some(true)
                                && protocol
                                    .as_ref()
                                    .and_then(|(p, _)| p.get("protocol_id"))
                                    .and_then(serde_json::Value::as_str)
                                    .is_some_and(|s| !s.is_empty())
                                && roster
                                    .as_ref()
                                    .and_then(|(r, _)| r.get("roster_id"))
                                    .and_then(serde_json::Value::as_str)
                                    .is_some_and(|s| !s.is_empty())
                                && i.get("independence_id")
                                    .and_then(serde_json::Value::as_str)
                                    .is_some_and(|s| !s.is_empty())
                                && i.get("trial_ids")
                                    .and_then(serde_json::Value::as_array)
                                    .is_some_and(|ids| ids.iter().any(|v| v.as_str() == trial_id))
                        });
                    let owner_trial_complete = owner_trial_facts_complete
                        .get("on")
                        .copied()
                        .unwrap_or(ev.owner_false_reject.is_none());
                    (owner_applicability, owner_qualified) = trial_admission(
                        owner_applicability,
                        true,
                        independent,
                        owner_trial_complete,
                    );
                    let bystander_gold_complete = if let Some(gold) = ev.human_gold.as_ref() {
                        if gold.state == "human_listened"
                            && !gold.reviewer_ids.is_empty()
                            && !gold.protocol_id.is_empty()
                            && !gold.locked_revision.is_empty()
                        {
                            if let Some(receipt_ref) = gold.receipt.as_ref() {
                                let receipt = read_receipt(&evidence_path, receipt_ref)?;
                                let reference_sha = row
                                    .score_input
                                    .as_ref()
                                    .and_then(|s| s.asr_reference.as_deref())
                                    .map(|s| validation::sha256(s.as_bytes()));
                                receipt
                                    .get("original_audio_sha256")
                                    .and_then(serde_json::Value::as_str)
                                    == Some(pair.original_audio.audio_sha256.as_str())
                                    && reference_sha.as_deref().is_some_and(|expected| {
                                        receipt
                                            .get("asr_reference_sha256")
                                            .and_then(serde_json::Value::as_str)
                                            == Some(expected)
                                    })
                                    && receipt.get("reviewer_ids")
                                        == Some(&serde_json::json!(gold.reviewer_ids))
                                    && receipt
                                        .get("locked_revision")
                                        .and_then(serde_json::Value::as_str)
                                        == Some(gold.locked_revision.as_str())
                                    && receipt
                                        .get("protocol_id")
                                        .and_then(serde_json::Value::as_str)
                                        == Some(gold.protocol_id.as_str())
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    let off_row = records["filter_off"].0.cases.iter().find(|r| {
                        r.job_id == inputs["filter_off"].selected_job_id
                            && r.case_id == row.case_id
                            && r.repeat == row.repeat
                    });
                    let synthetic_trial = evidence.evidence_source.as_deref() == Some("synthetic")
                        || evidence.quality_eligibility.as_deref() == Some("synthetic_only")
                        || ev
                            .human_gold
                            .as_ref()
                            .is_some_and(|g| g.state == "synthetic_only")
                        || off_row.is_some_and(|r| {
                            r.score_input
                                .as_ref()
                                .is_some_and(|s| s.source_kind == "synthetic_fixture")
                                || r.score_input
                                    .as_ref()
                                    .and_then(|s| s.corpus.as_ref())
                                    .is_some_and(|c| {
                                        matches!(
                                            &c.source_kind,
                                            super::contracts::SourceKind::Synthetic
                                        )
                                    })
                        })
                        || row
                            .score_input
                            .as_ref()
                            .is_some_and(|s| s.source_kind == "synthetic_fixture")
                        || row
                            .score_input
                            .as_ref()
                            .and_then(|s| s.corpus.as_ref())
                            .is_some_and(|c| {
                                matches!(&c.source_kind, super::contracts::SourceKind::Synthetic)
                            });
                    let bystander_independent = !synthetic_trial
                        && bystander_gold_complete
                        && base_ok
                        && trial_id.is_some()
                        && independence.as_ref().is_some_and(|(i, _)| {
                            i.get("protocol_sha256").and_then(serde_json::Value::as_str)
                                == protocol.as_ref().map(|(_, sha)| sha.as_str())
                                && i.get("roster_sha256").and_then(serde_json::Value::as_str)
                                    == roster.as_ref().map(|(_, sha)| sha.as_str())
                                && i.get("trial_unit").and_then(serde_json::Value::as_str)
                                    == body.get("trial_unit").and_then(serde_json::Value::as_str)
                                && i.get("independent").and_then(serde_json::Value::as_bool)
                                    == Some(true)
                                && protocol
                                    .as_ref()
                                    .and_then(|(p, _)| p.get("protocol_id"))
                                    .and_then(serde_json::Value::as_str)
                                    .is_some_and(|s| !s.is_empty())
                                && roster
                                    .as_ref()
                                    .and_then(|(r, _)| r.get("roster_id"))
                                    .and_then(serde_json::Value::as_str)
                                    .is_some_and(|s| !s.is_empty())
                                && i.get("independence_id")
                                    .and_then(serde_json::Value::as_str)
                                    .is_some_and(|s| !s.is_empty())
                                && i.get("trial_ids")
                                    .and_then(serde_json::Value::as_array)
                                    .is_some_and(|ids| ids.iter().any(|v| v.as_str() == trial_id))
                        });
                    if ev.owner_false_reject.is_none() {
                        (owner_applicability, owner_qualified) =
                            trial_admission(owner_applicability, true, bystander_independent, true);
                    }
                    (bystander_applicability, bystander_qualified) =
                        trial_admission(bystander_applicability, true, bystander_independent, true);
                }
            }
            owner_cp_trials.push(BernoulliTrial {
                applicable: owner_applicability,
                qualified: owner_qualified,
                event: owner_event,
                identity: trial_identity.clone(),
            });
            bystander_cp_trials.push(BernoulliTrial {
                applicable: bystander_applicability,
                qualified: bystander_qualified,
                event: on_event,
                identity: trial_identity,
            });
            continue;
        }
        let off_ev = &row_by_arm["off"];
        let on_ev = &row_by_arm["on"];
        if !same_audio(&off_ev.original_audio, &on_ev.original_audio) {
            return Err(err(
                &evidence_path,
                "identity_mismatch",
                "paired arms declare different original audio",
            ));
        }
        if let (Some(off_gold), Some(on_gold)) = (&off_ev.human_gold, &on_ev.human_gold) {
            if (!off_gold.reference_binding.is_null()
                && !on_gold.reference_binding.is_null()
                && off_gold.reference_binding != on_gold.reference_binding)
                || (!off_gold.audio_sha256.is_empty()
                    && !on_gold.audio_sha256.is_empty()
                    && off_gold.audio_sha256 != on_gold.audio_sha256)
            {
                return Err(err(
                    &evidence_path,
                    "identity_mismatch",
                    "paired Gold or original audio bindings contradict",
                ));
            }
        }
        let off = case_by_arm["off"];
        let on = case_by_arm["on"];
        let on_ev = &row_by_arm["on"];
        let synthetic_trial = evidence.evidence_source.as_deref() == Some("synthetic")
            || evidence.quality_eligibility.as_deref() == Some("synthetic_only")
            || on_ev
                .human_gold
                .as_ref()
                .is_some_and(|g| g.state == "synthetic_only")
            || [off, on].iter().any(|r| {
                r.score_input
                    .as_ref()
                    .is_some_and(|s| s.source_kind == "synthetic_fixture")
                    || r.score_input
                        .as_ref()
                        .and_then(|s| s.corpus.as_ref())
                        .is_some_and(|c| {
                            matches!(&c.source_kind, super::contracts::SourceKind::Synthetic)
                        })
            });
        let proof = pair
            .arms
            .get("filter_on")
            .and_then(|a| a.trial_proof.as_ref());
        let mut owner_eligibility = None;
        let mut bystander_eligibility = None;
        let mut owner_qualified = false;
        let mut bystander_qualified = false;
        let mut trial_identity = None;
        if let Some(proof_reference) = proof {
            let proof_json = serde_json::json!({"relative_path":proof_reference.relative_path,"sha256":proof_reference.sha256});
            let (body, _) = proof_ref(&evidence_path, &proof_json)?;
            let binding = body.get("row_binding");
            if binding
                .filter(|v| !v.is_null())
                .is_some_and(|v| !v.is_object())
                || present_string_mismatch(binding.and_then(|v| v.get("job_id")), &on.job_id)
                || present_string_mismatch(binding.and_then(|v| v.get("case_id")), &on.case_id)
                || present_u64_mismatch(binding.and_then(|v| v.get("repeat")), u64::from(on.repeat))
                || present_string_mismatch(
                    body.get("run_sha256"),
                    records
                        .get("filter_on")
                        .map(|(_, sha)| sha.as_str())
                        .unwrap_or_default(),
                )
                || present_string_mismatch(
                    body.get("original_audio_sha256"),
                    &pair.original_audio.audio_sha256,
                )
                || body
                    .get("trial_unit")
                    .filter(|v| !v.is_null())
                    .is_some_and(|v| !v.is_string())
            {
                return Err(err(
                    &evidence_path,
                    "identity_mismatch",
                    "trial proof does not bind selected on row, run and original audio",
                ));
            }
            if body
                .get("trial_id")
                .filter(|v| !v.is_null())
                .is_some_and(|v| !v.is_string())
            {
                return Err(err(
                    &evidence_path,
                    "identity_mismatch",
                    "trial proof trial_id has the wrong JSON type",
                ));
            }
            let trial_id = body
                .get("trial_id")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.trim().is_empty());
            let protocol = proof_child(&evidence_path, &body, "protocol")?;
            let roster_receipt = proof_child(&evidence_path, &body, "roster")?;
            let independence = proof_child(&evidence_path, &body, "independence")?;
            if let (
                Some(id),
                Some(unit),
                Some((_, protocol_sha)),
                Some((_, roster_sha)),
                Some((_, independence_sha)),
            ) = (
                trial_id,
                body.get("trial_unit")
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.is_empty()),
                protocol.as_ref(),
                roster_receipt.as_ref(),
                independence.as_ref(),
            ) {
                trial_identity = Some(TrialIdentity {
                    id: id.to_owned(),
                    proof: format!("{unit}|{protocol_sha}|{roster_sha}|{independence_sha}"),
                });
            }
            if let (Some((roster_body, _)), Some((_, protocol_sha))) = (&roster_receipt, &protocol)
            {
                if present_string_mismatch(roster_body.get("protocol_sha256"), protocol_sha) {
                    return Err(err(
                        &evidence_path,
                        "identity_mismatch",
                        "trial roster declares a different protocol receipt SHA",
                    ));
                }
            }
            if let (Some((ind_body, _)), Some((_, roster_sha))) = (&independence, &roster_receipt) {
                if present_string_mismatch(ind_body.get("roster_sha256"), roster_sha) {
                    return Err(err(
                        &evidence_path,
                        "identity_mismatch",
                        "independence receipt declares a different roster receipt SHA",
                    ));
                }
            }
            if let (Some((ind_body, _)), Some((_, protocol_sha)), Some((_, roster_sha))) =
                (&independence, &protocol, &roster_receipt)
            {
                if present_string_mismatch(ind_body.get("protocol_sha256"), protocol_sha)
                    || present_string_mismatch(ind_body.get("roster_sha256"), roster_sha)
                {
                    return Err(err(
                        &evidence_path,
                        "identity_mismatch",
                        "independence receipt declares different protocol or roster receipt SHA",
                    ));
                }
            }
            let roster_trials = roster_receipt
                .as_ref()
                .and_then(|(r, _)| r.get("trials").and_then(serde_json::Value::as_array))
                .map(|ts| {
                    ts.iter()
                        .filter(|t| {
                            trial_id.is_some_and(|id| {
                                t.get("trial_id").and_then(serde_json::Value::as_str) == Some(id)
                            }) && t
                                .get("row_binding")
                                .and_then(|b| b.get("case_id"))
                                .and_then(serde_json::Value::as_str)
                                == Some(on.case_id.as_str())
                                && t.get("row_binding")
                                    .and_then(|b| b.get("repeat"))
                                    .and_then(serde_json::Value::as_u64)
                                    == Some(u64::from(on.repeat))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if roster_trials.len() > 1 {
                return Err(err(
                    &evidence_path,
                    "identity_mismatch",
                    "roster contains duplicate declarations for a trial_id",
                ));
            }
            let roster_trial = roster_trials.first().copied();
            let roster_declarations = roster_receipt
                .as_ref()
                .and_then(|(r, _)| r.get("trials").and_then(serde_json::Value::as_array))
                .into_iter()
                .flatten()
                .filter(|t| {
                    trial_id.is_some_and(|id| {
                        t.get("trial_id").and_then(serde_json::Value::as_str) == Some(id)
                    })
                });
            for declaration in roster_declarations {
                if ["run_sha256", "original_audio_sha256"].iter().any(|key| {
                    declaration
                        .get(*key)
                        .filter(|v| !v.is_null())
                        .is_some_and(|v| !v.is_string())
                }) || declaration
                    .get("row_binding")
                    .filter(|v| !v.is_null())
                    .is_some_and(|v| !v.is_object())
                    || ["job_id", "case_id"].iter().any(|key| {
                        declaration
                            .get("row_binding")
                            .and_then(|b| b.get(*key))
                            .filter(|v| !v.is_null())
                            .is_some_and(|v| !v.is_string())
                    })
                    || declaration
                        .get("row_binding")
                        .and_then(|b| b.get("repeat"))
                        .filter(|v| !v.is_null())
                        .is_some_and(|v| !v.is_u64())
                {
                    return Err(err(
                        &evidence_path,
                        "evidence_contract",
                        "roster trial row contains a value of the wrong JSON type",
                    ));
                }
                let declaration_binding = declaration.get("row_binding");
                let row_candidate = [
                    (
                        declaration_binding
                            .and_then(|b| b.get("job_id"))
                            .and_then(serde_json::Value::as_str),
                        on.job_id.as_str(),
                    ),
                    (
                        declaration_binding
                            .and_then(|b| b.get("case_id"))
                            .and_then(serde_json::Value::as_str),
                        on.case_id.as_str(),
                    ),
                ]
                .iter()
                .all(|(declared, expected)| {
                    declared.map_or(true, |declared| declared == *expected)
                }) && declaration_binding
                    .and_then(|b| b.get("repeat"))
                    .and_then(serde_json::Value::as_u64)
                    .map_or(true, |repeat| repeat == u64::from(on.repeat));
                if row_candidate
                    && (declaration
                        .get("run_sha256")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|s| {
                            s != records
                                .get("filter_on")
                                .map(|(_, sha)| sha.as_str())
                                .unwrap_or_default()
                        })
                        || declaration
                            .get("original_audio_sha256")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|s| s != pair.original_audio.audio_sha256)
                        || declaration
                            .get("row_binding")
                            .and_then(|b| b.get("job_id"))
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|s| s != on.job_id)
                        || declaration
                            .get("row_binding")
                            .and_then(|b| b.get("case_id"))
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|s| s != on.case_id)
                        || declaration
                            .get("row_binding")
                            .and_then(|b| b.get("repeat"))
                            .and_then(serde_json::Value::as_u64)
                            .is_some_and(|n| n != u64::from(on.repeat)))
                {
                    return Err(err(
                        &evidence_path,
                        "identity_mismatch",
                        "roster entry contradicts selected trial row, run or audio",
                    ));
                }
            }
            if let Some(rt) = roster_trial {
                if ["run_sha256", "original_audio_sha256"].iter().any(|key| {
                    rt.get(*key)
                        .filter(|v| !v.is_null())
                        .is_some_and(|v| !v.is_string())
                }) || rt
                    .get("row_binding")
                    .filter(|v| !v.is_null())
                    .is_some_and(|v| !v.is_object())
                    || ["job_id", "case_id"].iter().any(|key| {
                        rt.get("row_binding")
                            .and_then(|b| b.get(*key))
                            .filter(|v| !v.is_null())
                            .is_some_and(|v| !v.is_string())
                    })
                    || rt
                        .get("row_binding")
                        .and_then(|b| b.get("repeat"))
                        .filter(|v| !v.is_null())
                        .is_some_and(|v| !v.is_u64())
                {
                    return Err(err(
                        &evidence_path,
                        "evidence_contract",
                        "roster trial row contains a value of the wrong JSON type",
                    ));
                }
                if rt
                    .get("run_sha256")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|s| {
                        s != records
                            .get("filter_on")
                            .map(|(_, sha)| sha.as_str())
                            .unwrap_or_default()
                    })
                    || rt
                        .get("original_audio_sha256")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|s| s != pair.original_audio.audio_sha256)
                    || rt
                        .get("row_binding")
                        .and_then(|b| b.get("job_id"))
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|s| s != on.job_id)
                    || rt
                        .get("row_binding")
                        .and_then(|b| b.get("case_id"))
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|s| s != on.case_id)
                    || rt
                        .get("row_binding")
                        .and_then(|b| b.get("repeat"))
                        .and_then(serde_json::Value::as_u64)
                        .is_some_and(|n| n != u64::from(on.repeat))
                {
                    return Err(err(
                        &evidence_path,
                        "identity_mismatch",
                        "roster entry contradicts selected trial row, run or audio",
                    ));
                }
            }
            let identity_complete = binding
                .and_then(|v| v.get("job_id"))
                .and_then(serde_json::Value::as_str)
                == Some(on.job_id.as_str())
                && binding
                    .and_then(|v| v.get("case_id"))
                    .and_then(serde_json::Value::as_str)
                    == Some(on.case_id.as_str())
                && binding
                    .and_then(|v| v.get("repeat"))
                    .and_then(serde_json::Value::as_u64)
                    == Some(u64::from(on.repeat))
                && body.get("run_sha256").and_then(serde_json::Value::as_str)
                    == records.get("filter_on").map(|(_, sha)| sha.as_str())
                && body
                    .get("original_audio_sha256")
                    .and_then(serde_json::Value::as_str)
                    == Some(pair.original_audio.audio_sha256.as_str())
                && body
                    .get("trial_unit")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|s| !s.is_empty());
            let eligibility_complete = body
                .get("eligibility")
                .and_then(serde_json::Value::as_object)
                .is_some_and(|elig| {
                    ["owner_false_reject", "bystander_final"].iter().all(|key| {
                        elig.get(*key)
                            .and_then(serde_json::Value::as_bool)
                            .is_some()
                    })
                });
            let base_ok = trial_id.is_some()
                && roster_trial.is_some()
                && identity_complete
                && eligibility_complete
                && protocol.as_ref().is_some_and(|(p, _)| {
                    p.get("trial_unit").and_then(serde_json::Value::as_str)
                        == body.get("trial_unit").and_then(serde_json::Value::as_str)
                        && p.get("false_reject_event")
                            .and_then(serde_json::Value::as_str)
                            == Some("human_owner_present_and_owner_output_rejected_by_filter")
                        && p.get("bystander_event").and_then(serde_json::Value::as_str)
                            == Some("human_attributed_bystander_content_in_actual_final_text")
                })
                && roster_receipt.as_ref().is_some_and(|(r, _)| {
                    r.get("protocol_sha256").and_then(serde_json::Value::as_str)
                        == protocol.as_ref().map(|(_, s)| s.as_str())
                        && roster_trial.is_some_and(|t| {
                            t.get("run_sha256").and_then(serde_json::Value::as_str)
                                == records.get("filter_on").map(|(_, s)| s.as_str())
                                && t.get("original_audio_sha256")
                                    .and_then(serde_json::Value::as_str)
                                    == Some(pair.original_audio.audio_sha256.as_str())
                                && t.get("row_binding")
                                    .and_then(|b| b.get("job_id"))
                                    .and_then(serde_json::Value::as_str)
                                    == Some(on.job_id.as_str())
                                && t.get("row_binding")
                                    .and_then(|b| b.get("case_id"))
                                    .and_then(serde_json::Value::as_str)
                                    == Some(on.case_id.as_str())
                                && t.get("row_binding")
                                    .and_then(|b| b.get("repeat"))
                                    .and_then(serde_json::Value::as_u64)
                                    == Some(u64::from(on.repeat))
                                && t.get("trial_id").and_then(serde_json::Value::as_str) == trial_id
                                && t.get("eligibility") == body.get("eligibility")
                                && t.get("eligibility")
                                    .and_then(serde_json::Value::as_object)
                                    .is_some_and(|elig| {
                                        ["owner_false_reject", "bystander_final"].iter().all(
                                            |key| {
                                                elig.get(*key)
                                                    .and_then(serde_json::Value::as_bool)
                                                    .is_some()
                                            },
                                        )
                                    })
                        })
                });
            let declared_elig = body.get("eligibility");
            let owner_false = declared_elig
                .and_then(|v| v.get("owner_false_reject"))
                .and_then(serde_json::Value::as_bool);
            let bystander = declared_elig
                .and_then(|v| v.get("bystander_final"))
                .and_then(serde_json::Value::as_bool);
            if base_ok {
                owner_eligibility = owner_false;
                bystander_eligibility = bystander;
            }
            let gold_complete = if let Some(gold) = on_ev.human_gold.as_ref() {
                if gold.state == "human_listened"
                    && !gold.reviewer_ids.is_empty()
                    && !gold.protocol_id.is_empty()
                    && !gold.locked_revision.is_empty()
                {
                    if let Some(receipt_ref) = gold.receipt.as_ref() {
                        let receipt = read_receipt(&evidence_path, receipt_ref)?;
                        let reference_sha = on
                            .score_input
                            .as_ref()
                            .and_then(|s| s.asr_reference.as_deref())
                            .map(|s| validation::sha256(s.as_bytes()));
                        receipt
                            .get("original_audio_sha256")
                            .and_then(serde_json::Value::as_str)
                            == Some(pair.original_audio.audio_sha256.as_str())
                            && reference_sha.as_deref().is_some_and(|expected| {
                                receipt
                                    .get("asr_reference_sha256")
                                    .and_then(serde_json::Value::as_str)
                                    == Some(expected)
                            })
                            && receipt.get("reviewer_ids")
                                == Some(&serde_json::json!(gold.reviewer_ids))
                            && receipt
                                .get("locked_revision")
                                .and_then(serde_json::Value::as_str)
                                == Some(gold.locked_revision.as_str())
                            && receipt
                                .get("protocol_id")
                                .and_then(serde_json::Value::as_str)
                                == Some(gold.protocol_id.as_str())
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };
            let independent = !synthetic_trial
                && gold_complete
                && base_ok
                && trial_id.is_some()
                && independence.as_ref().is_some_and(|(i, _)| {
                    i.get("protocol_sha256").and_then(serde_json::Value::as_str)
                        == protocol.as_ref().map(|(_, s)| s.as_str())
                        && i.get("roster_sha256").and_then(serde_json::Value::as_str)
                            == roster_receipt.as_ref().map(|(_, s)| s.as_str())
                        && i.get("trial_unit").and_then(serde_json::Value::as_str)
                            == body.get("trial_unit").and_then(serde_json::Value::as_str)
                        && i.get("independent").and_then(serde_json::Value::as_bool) == Some(true)
                        && protocol
                            .as_ref()
                            .and_then(|(p, _)| p.get("protocol_id"))
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|s| !s.is_empty())
                        && roster_receipt
                            .as_ref()
                            .and_then(|(r, _)| r.get("roster_id"))
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|s| !s.is_empty())
                        && i.get("independence_id")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|s| !s.is_empty())
                        && i.get("trial_ids")
                            .and_then(serde_json::Value::as_array)
                            .is_some_and(|ids| ids.iter().any(|v| v.as_str() == trial_id))
                });
            let owner_trial_complete = owner_trial_facts_complete
                .get("on")
                .copied()
                .unwrap_or(on_ev.owner_false_reject.is_none());
            (owner_eligibility, owner_qualified) = trial_admission(
                owner_eligibility,
                base_ok,
                independent,
                owner_trial_complete,
            );
            (bystander_eligibility, bystander_qualified) =
                trial_admission(bystander_eligibility, base_ok, independent, true);
        }
        let owner_event = on_ev
            .owner_false_reject
            .as_ref()
            .filter(|o| o.eligible)
            .map(|o| o.event);
        let final_event = on_ev.final_attribution.as_ref().and_then(|a| {
            let final_text = text(&on.final_text)?;
            (on_ev.attribution_receipt_complete
                && a.receipt.is_some()
                && a.all_output_examined
                && a.final_text_utf8_sha256 == validation::sha256(final_text.as_bytes()))
            .then_some(!a.utf8_ranges.is_empty())
        });
        owner_cp_trials.push(BernoulliTrial {
            applicable: owner_eligibility,
            qualified: owner_qualified,
            event: owner_event,
            identity: trial_identity.clone(),
        });
        bystander_cp_trials.push(BernoulliTrial {
            applicable: bystander_eligibility,
            qualified: bystander_qualified,
            event: final_event,
            identity: trial_identity,
        });
        let off_reference = off
            .score_input
            .as_ref()
            .and_then(|s| s.asr_reference.as_deref());
        let on_reference = on
            .score_input
            .as_ref()
            .and_then(|s| s.asr_reference.as_deref());
        let ref_sha_off = off_reference.map(|r| validation::sha256(r.as_bytes()));
        let ref_sha_on = on_reference.map(|r| validation::sha256(r.as_bytes()));
        let gold_ref = off_ev
            .human_gold
            .as_ref()
            .and_then(|gold| gold.reference_binding.get("asr_reference_sha256"))
            .and_then(serde_json::Value::as_str);
        if ref_sha_off.as_ref() != ref_sha_on.as_ref()
            || ref_sha_off
                .as_deref()
                .zip(gold_ref)
                .is_some_and(|(actual, declared)| actual != declared)
        {
            return Err(err(
                &evidence_path,
                "identity_mismatch",
                "RunRecord reference and Gold reference binding contradict",
            ));
        }
        if let Some(ref_sha) = ref_sha_off.as_deref() {
            if let Some(gold) = &off_ev.human_gold {
                if let Some(gold_ref) = &gold.receipt {
                    let gold_receipt = read_receipt(&evidence_path, gold_ref)?;
                    if gold_receipt
                        .get("original_audio_sha256")
                        .is_some_and(|v| v.as_str() != Some(gold.audio_sha256.as_str()))
                        || gold_receipt
                            .get("asr_reference_sha256")
                            .is_some_and(|v| v.as_str() != Some(ref_sha))
                        || (!gold.reviewer_ids.is_empty()
                            && gold_receipt
                                .get("reviewer_ids")
                                .is_some_and(|v| v != &serde_json::json!(&gold.reviewer_ids)))
                        || (!gold.locked_revision.is_empty()
                            && gold_receipt
                                .get("locked_revision")
                                .is_some_and(|v| v.as_str() != Some(gold.locked_revision.as_str())))
                        || (!gold.protocol_id.is_empty()
                            && gold_receipt
                                .get("protocol_id")
                                .is_some_and(|v| v.as_str() != Some(gold.protocol_id.as_str())))
                    {
                        return Err(err(&evidence_path, "identity_mismatch", "human Gold receipt does not bind the selected audio, reference and reviewers"));
                    }
                }
            }
        }
        for (_arm, ev, row) in [("filter_off", off_ev, off), ("filter_on", on_ev, on)] {
            if let (Some(final_text), Some(attribution)) =
                (text(&row.final_text), ev.final_attribution.as_ref())
            {
                let final_sha = validation::sha256(final_text.as_bytes());
                if !attribution.final_text_utf8_sha256.is_empty()
                    && final_sha != attribution.final_text_utf8_sha256
                {
                    return Err(err(
                        &evidence_path,
                        "identity_mismatch",
                        "final attribution SHA does not match actual final_text",
                    ));
                }
                if attribution
                    .utf8_ranges
                    .iter()
                    .any(|r| r[0] >= r[1] || !valid_range(final_text, *r))
                {
                    return Err(err(
                        &evidence_path,
                        "evidence_contract",
                        "final attribution ranges or text SHA are invalid",
                    ));
                }
                if let Some(attribution_receipt) = &attribution.receipt {
                    let receipt = read_receipt(&evidence_path, attribution_receipt)?;
                    if receipt
                        .get("final_text_utf8_sha256")
                        .is_some_and(|v| v.as_str() != Some(final_sha.as_str()))
                        || receipt
                            .get("all_output_examined")
                            .is_some_and(|v| v.as_bool() != Some(attribution.all_output_examined))
                        || receipt
                            .get("bystander_utf8_ranges")
                            .is_some_and(|v| v != &serde_json::json!(&attribution.utf8_ranges))
                    {
                        return Err(err(&evidence_path, "identity_mismatch", "final attribution receipt does not bind actual final text and reviewed ranges"));
                    }
                }
            }
            if let Some(owner) = ev.owner_false_reject.as_ref() {
                let receipt = read_receipt(&evidence_path, &owner.receipt)?;
                if receipt.get("event").and_then(serde_json::Value::as_bool) != Some(owner.event)
                    || receipt.get("eligible").and_then(serde_json::Value::as_bool)
                        != Some(owner.eligible)
                    || (!owner.trial_unit.is_empty()
                        && receipt
                            .get("trial_unit")
                            .and_then(serde_json::Value::as_str)
                            != Some(owner.trial_unit.as_str()))
                    || (!owner.protocol.is_empty()
                        && receipt.get("protocol").and_then(serde_json::Value::as_str)
                            != Some(owner.protocol.as_str()))
                    || (!owner.roster.is_empty()
                        && receipt.get("roster").and_then(serde_json::Value::as_str)
                            != Some(owner.roster.as_str()))
                    || (!owner.independence.is_empty()
                        && receipt
                            .get("independence")
                            .and_then(serde_json::Value::as_str)
                            != Some(owner.independence.as_str()))
                    || receipt.get("case_id").and_then(serde_json::Value::as_str)
                        != Some(row.case_id.as_str())
                    || receipt.get("repeat").and_then(serde_json::Value::as_u64)
                        != Some(u64::from(row.repeat))
                    || receipt.get("job_id").and_then(serde_json::Value::as_str)
                        != Some(row.job_id.as_str())
                {
                    return Err(err(
                        &evidence_path,
                        "identity_mismatch",
                        "owner false-reject receipt contradicts declared trial facts",
                    ));
                }
            }
        }
        let ref_sha = ref_sha_off.as_deref().unwrap_or("");
        let synthetic = evidence.evidence_source.as_deref() == Some("synthetic")
            || evidence.quality_eligibility.as_deref() == Some("synthetic_only")
            || pair
                .human_gold
                .as_ref()
                .is_some_and(|gold| gold.state == "synthetic_only");
        off_internal.arms.push(internal_arm(
            off_ev,
            off,
            ref_sha,
            "",
            synthetic,
            true,
            receipt_incomplete_damage
                .get("filter_off")
                .map_or(&[], Vec::as_slice),
        ));
        on_internal.arms.push(internal_arm(
            on_ev,
            on,
            ref_sha,
            "",
            synthetic,
            false,
            receipt_incomplete_damage
                .get("filter_on")
                .map_or(&[], Vec::as_slice),
        ));
    }
    control_projections.sort_by_key(|v| {
        (
            v["case_id"].as_str().unwrap_or_default().to_owned(),
            v["repeat"].as_u64().unwrap_or_default(),
        )
    });
    let control_projection_sha =
        validation::sha256(&serde_json::to_vec(&control_projections).map_err(|_| {
            err(
                &evidence_path,
                "evidence_contract",
                "control projection serialization failed",
            )
        })?);
    let mut comparability_fully_bound = false;
    if let Some(comparability) = &evidence.comparability {
        if let Some(comparability_receipt) = &comparability.receipt {
            let receipt = read_receipt(&evidence_path, comparability_receipt)?;
            let run_sha = receipt
                .get("run_sha256")
                .and_then(serde_json::Value::as_object);
            let bound_controls = receipt
                .get("control_projection_sha256")
                .and_then(serde_json::Value::as_str);
            let expected_off = records.get("filter_off").map(|(_, sha)| sha.as_str());
            let expected_on = records.get("filter_on").map(|(_, sha)| sha.as_str());
            let declared_off = run_sha
                .and_then(|m| m.get("filter_off"))
                .and_then(serde_json::Value::as_str);
            let declared_on = run_sha
                .and_then(|m| m.get("filter_on"))
                .and_then(serde_json::Value::as_str);
            if declared_off.zip(expected_off).is_some_and(|(a, b)| a != b)
                || declared_on.zip(expected_on).is_some_and(|(a, b)| a != b)
                || (control_projection_complete
                    && bound_controls.is_some_and(|sha| sha != control_projection_sha))
            {
                return Err(err(
                    &evidence_path,
                    "identity_mismatch",
                    "comparability receipt contradicts selected runs or their controls",
                ));
            }
            comparability_fully_bound = comparability_receipt_confirms
                && declared_off == expected_off
                && declared_on == expected_on
                && control_projection_complete
                && bound_controls == Some(control_projection_sha.as_str());
        }
    }
    if comparability_fully_bound {
        for arm in off_internal
            .arms
            .iter_mut()
            .chain(on_internal.arms.iter_mut())
        {
            arm.controls_sha256 = control_projection_sha.clone();
        }
    }
    let quality = scoring::derive_filter_quality_for_arms(
        &records["filter_off"].0,
        &records["filter_on"].0,
        off_run_id,
        on_run_id,
        Some((&off_internal, &on_internal)),
    );
    let descriptive_cer_delta = scoring::filter_delta_interval(&filter_delta_observations(
        &declared,
        &evidence.pairs,
        &quality,
        false,
    ));
    let descriptive_wer_delta = scoring::filter_delta_interval(&filter_delta_observations(
        &declared,
        &evidence.pairs,
        &quality,
        true,
    ));
    let false_reject_statistics = finalize_binary_confidence(&owner_cp_trials);
    let bystander_final_statistics = finalize_binary_confidence(&bystander_cp_trials);
    let mut report_rows = quality
        .per_pair
        .iter()
        .map(|p| {
            let pair_evidence = evidence
                .pairs
                .iter()
                .find(|e| e.source_case_id == p.case_id && e.repeat == p.repeat);
            let synthetic = evidence.evidence_source.as_deref() == Some("synthetic")
                || evidence.quality_eligibility.as_deref() == Some("synthetic_only")
                || pair_evidence.is_some_and(|e| {
                    e.human_gold
                        .as_ref()
                        .is_some_and(|gold| gold.state == "synthetic_only")
                });
            Pair {
                case_id: p.case_id.clone(),
                repeat: p.repeat,
                cer_off: p.cer_off,
                cer_on: p.cer_on,
                cer_on_minus_off: p.cer_on_minus_off,
                cer_off_state: p.cer_off_state,
                cer_on_state: p.cer_on_state,
                cer_state: if p.cer_off_state == "observed" && p.cer_on_state == "observed" {
                    "observed"
                } else if p.cer_off_state == "not_applicable" || p.cer_on_state == "not_applicable"
                {
                    "not_applicable"
                } else {
                    "unknown"
                },
                cer_upper_bound: if p.cer_unbounded {
                    "upper_unbounded"
                } else {
                    "finite"
                },
                wer_off: p.wer_off,
                wer_on: p.wer_on,
                wer_on_minus_off: p.wer_on_minus_off,
                wer_off_state: p.wer_off_state,
                wer_on_state: p.wer_on_state,
                wer_state: if p.wer_off_state == "observed" && p.wer_on_state == "observed" {
                    "observed"
                } else if p.wer_off_state == "not_applicable" || p.wer_on_state == "not_applicable"
                {
                    "not_applicable"
                } else {
                    "unknown"
                },
                wer_upper_bound: if p.wer_unbounded {
                    "upper_unbounded"
                } else {
                    "finite"
                },
                synthetic,
                gold_state: if synthetic {
                    "synthetic_only"
                } else {
                    "unknown"
                },
                final_review_coverage: "see per-kind and reviewed-arm coverage".into(),
            }
        })
        .collect::<Vec<_>>();
    report_rows.sort_by_key(|p| (p.case_id.clone(), p.repeat));
    let cer_observed = report_rows
        .iter()
        .filter(|p| p.cer_state == "observed" || p.cer_state == "not_applicable")
        .count();
    let wer_observed = report_rows
        .iter()
        .filter(|p| p.wer_state == "observed" || p.wer_state == "not_applicable")
        .count();
    let final_reviewed = quality.final_bystander_observed as usize;
    let final_expected = quality.union_rows.unwrap_or(0) as usize * 2;
    let final_spans = quality.final_bystander_spans as usize;
    let final_leaks = quality.final_bystander_leaks as usize;
    let mut criteria = BTreeMap::new();
    criteria.insert(
        "cer",
        criterion(
            declared.len(),
            declared.len() - cer_observed,
            "voiceflow-fixture-scoring-v1_normalized_cer",
            "per_pair_on_minus_off",
        ),
    );
    criteria.insert(
        "wer",
        criterion(
            declared.len(),
            declared.len() - wer_observed,
            "voiceflow-fixture-scoring-v1_english_wer",
            "per_pair_on_minus_off",
        ),
    );
    criteria.insert(
        "gold",
        criterion(
            declared.len(),
            declared.len(),
            "receipt_bound_human_lock",
            "case_repeat",
        ),
    );
    criteria.insert(
        "final_attribution",
        criterion(
            final_expected,
            final_expected - final_reviewed,
            "reviewed_utf8_ranges",
            "arm",
        ),
    );
    criteria.insert(
        "owner_false_reject",
        criterion(
            quality.owner_n as usize + quality.owner_eligibility_unknown as usize,
            quality.owner_eligibility_unknown as usize,
            "receipt_bound_trial",
            "case_repeat",
        ),
    );
    criteria.insert(
        "cer_delta_ci",
        criterion(declared.len(), declared.len(), "bootstrap_task73", "rate"),
    );
    criteria.insert(
        "wer_delta_ci",
        criterion(declared.len(), declared.len(), "bootstrap_task73", "rate"),
    );
    criteria.insert(
        "false_reject_cp",
        criterion(
            false_reject_statistics.requested_rows,
            false_reject_statistics.unknown_eligibility_rows
                + false_reject_statistics.unknown_qualification_rows,
            "clopper_pearson_task73",
            "rate",
        ),
    );
    criteria.insert(
        "bystander_final_cp",
        criterion(
            bystander_final_statistics.requested_rows,
            bystander_final_statistics.unknown_eligibility_rows
                + bystander_final_statistics.unknown_qualification_rows,
            "clopper_pearson_task73",
            "rate",
        ),
    );
    criteria.insert(
        "human_gold_speakers",
        criterion(
            declared.len(),
            declared.len(),
            "reviewed_namespaced_speakers",
            "speaker",
        ),
    );
    criteria.insert(
        "filter_compute_rtf",
        criterion(
            declared.len(),
            declared.len(),
            "max_synced_compute_rtf_task74",
            "ratio",
        ),
    );
    criteria.insert(
        "filter_drain_p95",
        criterion(
            declared.len(),
            declared.len(),
            "nearest_rank_p95_task74",
            "ms",
        ),
    );
    criteria.insert(
        "paired_e2e_delta_p95",
        criterion(
            declared.len(),
            declared.len(),
            "nearest_rank_p95_task74",
            "ms",
        ),
    );
    let mut damage_by_kind = BTreeMap::new();
    for (i, kind) in kinds.iter().enumerate() {
        damage_by_kind.insert(
            *kind,
            DamageKindReport {
                reviewed_arm_units: quality.damage_by_kind_observed[i] as usize,
                expected_arm_units: final_expected,
                observed_subset_spans: quality.damage_by_kind_spans[i] as usize,
                observed_subset_errors: quality.damage_by_kind_count[i] as usize,
                state: if quality.damage_by_kind_observed[i] as usize == final_expected
                    && final_expected > 0
                {
                    "observed"
                } else {
                    "unknown"
                },
            },
        );
    }
    let policy_bytes = serde_json::to_vec(&evidence.acceptance_policy).map_err(|_| {
        err(
            &evidence_path,
            "evidence_contract",
            "policy serialization failed",
        )
    })?;
    let policy_sha = validation::sha256(&policy_bytes);
    let input_sha = validation::sha256(
        records
            .values()
            .map(|(_, sha)| sha.as_str())
            .collect::<Vec<_>>()
            .join(":")
            .as_bytes(),
    );
    let provenance = format!(
        "offline_selected_records;evidence_source={};quality_eligibility={};evidence_id={}",
        evidence.evidence_source.as_deref().unwrap_or("unknown"),
        evidence.quality_eligibility.as_deref().unwrap_or("unknown"),
        evidence.evidence_id.as_deref().unwrap_or("unknown")
    );
    let report = Report {
        artifact_type: REPORT_TYPE,
        schema_version: 1,
        input_sha256: input_sha,
        evidence_sha256: validation::sha256(&evidence_bytes),
        policy_sha256: policy_sha,
        provenance,
        status: "unknown",
        criteria,
        pairs: report_rows,
        descriptive_cer_delta,
        descriptive_wer_delta,
        false_reject_statistics,
        bystander_final_statistics,
        final_bystander_reviewed_arm_units: final_reviewed,
        final_bystander_expected_arm_units: final_expected,
        final_bystander_observed_subset_spans: final_spans,
        final_bystander_observed_subset_leaks: final_leaks,
        damage_by_kind,
        owner_false_reject: OwnerMetric {
            eligible_n: quality.owner_n as usize,
            false_rejects: quality.owner_false_rejects as usize,
            unknown_trials: quality.owner_eligibility_unknown as usize,
            state: if quality.owner_eligibility_unknown == 0 {
                "observed"
            } else {
                "unknown"
            },
        },
    };
    let output = super::runner::output_path(report_arg)?;
    if output == evidence_path
        || evidence
            .input_runs
            .iter()
            .any(|r| resolve(&evidence_path, &r.relative_path).is_ok_and(|p| p == output))
    {
        return Err(err(
            report_arg,
            "conflicting_paths",
            "report must be a new path distinct from all inputs",
        ));
    }
    let encoded = serde_json::to_vec_pretty(&report)
        .map_err(|_| err(report_arg, "filter_report", "report serialization failed"))?;
    if encoded.len().saturating_add(1) > MAX_REPORT_BYTES {
        return Err(err(
            report_arg,
            "filter_report_budget",
            "report exceeds the artifact budget",
        ));
    }
    let mut artifact = AtomicArtifact::new(&output).map_err(|_| {
        err(
            report_arg,
            "filter_report_write",
            "report output could not be safely initialized",
        )
    })?;
    artifact.write_json(&report).map_err(|_| {
        err(
            report_arg,
            "filter_report_write",
            "report could not be exclusively installed",
        )
    })?;
    Ok(3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluation::{
        contracts::{CaseInputPaths, InputPaths, StageState},
        fixtures::FixtureDirectory,
        runner,
        tests::journal_fixture,
    };

    fn receipt(
        directory: &FixtureDirectory,
        name: &str,
        value: serde_json::Value,
    ) -> serde_json::Value {
        let path = directory.write(name, &value);
        let bytes = std::fs::read(path).unwrap();
        serde_json::json!({"relative_path": name, "sha256": validation::sha256(&bytes)})
    }
    fn rebind_trial_proof(
        directory: &FixtureDirectory,
        proof: &mut serde_json::Value,
        prefix: &str,
    ) {
        let child = |proof: &serde_json::Value, key: &str| {
            let path = directory
                .0
                .join(proof[key]["relative_path"].as_str().unwrap());
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(path).unwrap()).unwrap()
        };
        let protocol = child(proof, "protocol");
        let protocol_sha = proof["protocol"]["sha256"].clone();
        let mut roster = child(proof, "roster");
        roster["protocol_sha256"] = protocol_sha.clone();
        if let Some(trial) = roster["trials"]
            .as_array_mut()
            .and_then(|trials| trials.first_mut())
        {
            trial["run_sha256"] = proof["run_sha256"].clone();
            trial["original_audio_sha256"] = proof["original_audio_sha256"].clone();
            trial["row_binding"] = proof["row_binding"].clone();
        }
        let roster_ref = receipt(directory, &format!("{prefix}-roster.json"), roster);
        let mut independence = child(proof, "independence");
        independence["protocol_sha256"] = protocol_sha;
        independence["roster_sha256"] = roster_ref["sha256"].clone();
        let independence_ref = receipt(
            directory,
            &format!("{prefix}-independence.json"),
            independence,
        );
        proof["protocol"] = receipt(directory, &format!("{prefix}-protocol.json"), protocol);
        proof["roster"] = roster_ref;
        proof["independence"] = independence_ref;
    }
    fn add_trial_proof(directory: &FixtureDirectory, evidence_path: &Path) {
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(evidence_path).unwrap()).unwrap();
        let run_sha = evidence["input_runs"][1]["sha256"]
            .as_str()
            .unwrap()
            .to_owned();
        let pair = &evidence["pairs"][0];
        let case_id = pair["source_case_id"].as_str().unwrap().to_owned();
        let repeat = pair["repeat"].as_u64().unwrap();
        let job_id = pair["arms"]["filter_on"]["row_binding"]["job_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let original_sha = pair["original_audio"]["audio_sha256"]
            .as_str()
            .unwrap()
            .to_owned();
        let row_binding = serde_json::json!({"job_id":job_id,"case_id":case_id,"repeat":repeat});
        let eligibility = serde_json::json!({"owner_false_reject":true,"bystander_final":true});
        let protocol = receipt(
            directory,
            "cp-protocol.json",
            serde_json::json!({
                "protocol_id":"cp-test-v1","trial_unit":"source_case_id_repeat",
                "false_reject_event":"human_owner_present_and_owner_output_rejected_by_filter",
                "bystander_event":"human_attributed_bystander_content_in_actual_final_text"
            }),
        );
        let protocol_sha = protocol["sha256"].as_str().unwrap();
        let roster = receipt(
            directory,
            "cp-roster.json",
            serde_json::json!({
                "roster_id":"cp-roster-v1","protocol_sha256":protocol_sha,
                "trials":[{"trial_id":"trial-1","row_binding":row_binding,"run_sha256":run_sha,
                    "original_audio_sha256":original_sha,"eligibility":eligibility}]
            }),
        );
        let independence = receipt(
            directory,
            "cp-independence.json",
            serde_json::json!({
                "independence_id":"cp-ind-v1","protocol_sha256":protocol_sha,
                "roster_sha256":roster["sha256"],"trial_unit":"source_case_id_repeat",
                "trial_ids":["trial-1"],"independent":true
            }),
        );
        let trial = receipt(
            directory,
            "cp-trial.json",
            serde_json::json!({
                "row_binding":row_binding,"run_sha256":run_sha,"original_audio_sha256":original_sha,
                "trial_id":"trial-1","trial_unit":"source_case_id_repeat","eligibility":eligibility,
                "protocol":protocol,"roster":roster,"independence":independence
            }),
        );
        evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"] = trial;
        directory.write("evidence.json", &evidence);
    }

    fn bound_fixture(
        directory: &FixtureDirectory,
        missing_peer: bool,
    ) -> (PathBuf, RunRecord, RunRecord) {
        let (_inputs, journal) = journal_fixture(directory);
        let baseline = journal.snapshot().identity.baseline().clone();
        let recipe_path = directory.0.join("recipe.json");
        let mut recipe: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&recipe_path).unwrap()).unwrap();
        let jobs = recipe["jobs"].as_array_mut().unwrap();
        let right_job = jobs.first().cloned().unwrap();
        jobs.push(right_job);
        recipe["scoring"] = serde_json::json!({
            "method": scoring::METHOD,
            "comparison": {"left_job":0,"right_job":1,"stage":"filtered_asr","cluster_by":"speaker","seed":1,"bootstrap_replicates":100}
        });
        let paired_recipe = directory.write("paired-recipe.json", &recipe);
        let inputs = super::super::validation::validate(
            &directory.0.join("speech.json"),
            Some(&directory.0.join("corpus.json")),
            Some(&paired_recipe),
            None,
        )
        .unwrap();
        let base = runner::build_report(&inputs, baseline).unwrap();
        let ids = base
            .scoring
            .as_ref()
            .unwrap()
            .comparison_job_ids
            .clone()
            .unwrap();
        let mut off = base.clone();
        off.run_id = "synthetic-off-run".into();
        off.terminal = StageState::Completed {};
        off.cases.retain(|r| r.job_id == ids[0]);
        for row in &mut off.cases {
            row.job_id = "same-job-id".into();
            let input = row.score_input.as_mut().unwrap();
            input.asr_reference = Some("the cat".into());
            let corpus = input.corpus.as_mut().unwrap();
            corpus.language = super::super::contracts::Language::En;
            corpus.reference_sha256 = validation::sha256(b"the cat");
            row.filtered_text = ObservedText::known("the cat");
            row.final_text = ObservedText::known("the cat");
            row.input_paths = CaseInputPaths::Speech(InputPaths {
                manifest: "synthetic-never-opened.json".into(),
                declared_audio: "synthetic-never-opened.wav".into(),
                canonical_audio: "synthetic-never-opened.wav".into(),
            });
        }
        let mut on = base;
        on.run_id = "synthetic-on-run".into();
        on.terminal = StageState::Completed {};
        on.cases.retain(|r| r.job_id == ids[1]);
        for row in &mut on.cases {
            row.job_id = "same-job-id".into();
            let input = row.score_input.as_mut().unwrap();
            input.asr_reference = Some("the cat".into());
            let corpus = input.corpus.as_mut().unwrap();
            corpus.language = super::super::contracts::Language::En;
            corpus.reference_sha256 = validation::sha256(b"the cat");
            row.filtered_text = ObservedText::known("the dog");
            row.final_text = ObservedText::known("the dog");
            row.input_paths = CaseInputPaths::Speech(InputPaths {
                manifest: "synthetic-never-opened.json".into(),
                declared_audio: "synthetic-never-opened.wav".into(),
                canonical_audio: "synthetic-never-opened.wav".into(),
            });
        }
        if missing_peer {
            let mut extra = off.cases.first().unwrap().clone();
            extra.case_id = "synthetic-off-only".into();
            extra.repeat = 1;
            off.cases.push(extra);
        }
        (paired_recipe, off, on)
    }

    fn write_case(
        directory: &FixtureDirectory,
        filtered_selection: bool,
        missing_peer: bool,
    ) -> (PathBuf, PathBuf) {
        let (_recipe, off, on) = bound_fixture(directory, missing_peer);
        let off_value = serde_json::to_value(&off).unwrap();
        let on_value = serde_json::to_value(&on).unwrap();
        let off_bytes = serde_json::to_vec(&off_value).unwrap();
        let on_bytes = serde_json::to_vec(&on_value).unwrap();
        directory.write("run-off.json", &off_value);
        directory.write("run-on.json", &on_value);
        let off_row = off.cases.first().unwrap();
        let on_row = on.cases.first().unwrap();
        let off_text = text(&off_row.filtered_text).unwrap();
        let on_text = text(&on_row.filtered_text).unwrap();
        let off_final = text(&off_row.final_text).unwrap();
        let on_final = text(&on_row.final_text).unwrap();
        let ref_text = off_row
            .score_input
            .as_ref()
            .unwrap()
            .asr_reference
            .as_deref()
            .unwrap();
        let gold = receipt(
            directory,
            "gold-receipt.json",
            serde_json::json!({
                "original_audio_sha256": off_row.audio.audio_sha256,
                "asr_reference_sha256": validation::sha256(ref_text.as_bytes()),
                "reviewer_ids": ["synthetic-reviewer"], "locked_revision":"synthetic-r1", "protocol_id":"synthetic-protocol"
            }),
        );
        let off_attribution = receipt(
            directory,
            "attribution-off.json",
            serde_json::json!({"final_text_utf8_sha256":validation::sha256(off_final.as_bytes()),"all_output_examined":true,"bystander_utf8_ranges":[]}),
        );
        let on_attribution = receipt(
            directory,
            "attribution-on.json",
            serde_json::json!({"final_text_utf8_sha256":validation::sha256(on_final.as_bytes()),"all_output_examined":true,"bystander_utf8_ranges":[[4,7]]}),
        );
        let reference_sha = validation::sha256(ref_text.as_bytes());
        let off_final_sha = validation::sha256(off_final.as_bytes());
        let on_final_sha = validation::sha256(on_final.as_bytes());
        let damage_annotations = ["head", "tail", "number", "entity"].into_iter().map(|kind| {
            let assessment = serde_json::json!({
                "filter_off":{"state":"retained","final_utf8_range":[0,3]},
                "filter_on":{"state":if kind == "number" {"damaged"} else {"retained"},"final_utf8_range":[0,3]}
            });
            let damage_receipt = receipt(directory, &format!("damage-{kind}.json"), serde_json::json!({
                "kind":kind,"reference_range":[0,3],"reference_material_sha256":validation::sha256(b"the"),
                "reference_sha256":reference_sha,
                "final_text_utf8_sha256":{"filter_off":off_final_sha,"filter_on":on_final_sha},
                "assessment":assessment
            }));
            serde_json::json!({"kind":kind,"reference_target":"asr_reference","reference_sha256":reference_sha,"reference_range":[0,3],"assessment":assessment,"receipt":damage_receipt})
        }).collect::<Vec<_>>();
        let projection = serde_json::json!({"requested":off_row.requested,"effective":off_row.effective,"control_input_identities":off_row.control_input_identities,"local_cleanup":off_row.local_cleanup,"local_model":off_row.local_model});
        let controls = vec![
            serde_json::json!({"case_id":off_row.case_id,"repeat":off_row.repeat,"controls":projection}),
        ];
        let comparison = receipt(
            directory,
            "comparison-receipt.json",
            serde_json::json!({
                "same_original_audio_gold_asr_cleanup_controls_except_selected_audio":true,
                "run_sha256":{"filter_off":validation::sha256(&off_bytes),"filter_on":validation::sha256(&on_bytes)},
                "control_projection_sha256":validation::sha256(&serde_json::to_vec(&controls).unwrap())
            }),
        );
        let make_arm = |_arm: &str,
                        _run_id: &str,
                        row: &CaseRecordV1,
                        quality: &str,
                        final_text: &str,
                        attribution: serde_json::Value,
                        selected: &str,
                        reason: &str,
                        fallback: bool| {
            serde_json::json!({
                "row_binding":{"job_id":"same-job-id","case_id":row.case_id,"repeat":row.repeat},
                "selection":{"state":"observed","selected":selected,"selected_audio_sha256":row.audio.audio_sha256,"reason":reason,"filter_fallback":{"occurred":fallback}},
                "quality_text":{"field":"filtered_text","utf8_sha256":validation::sha256(quality.as_bytes())},
                "final_attribution":{"receipt":attribution,"final_text_utf8_sha256":validation::sha256(final_text.as_bytes()),"all_output_examined":true,"utf8_ranges":if final_text == "the dog" {vec![[4,7]]} else {vec![]}}
            })
        };
        let arm_off = make_arm(
            "filter_off",
            &off.run_id,
            off_row,
            off_text,
            off_final,
            off_attribution,
            "original",
            "filter_disabled",
            false,
        );
        let arm_on = if filtered_selection {
            make_arm(
                "filter_on",
                &on.run_id,
                on_row,
                on_text,
                on_final,
                on_attribution,
                "filtered",
                "filter_applied",
                false,
            )
        } else {
            make_arm(
                "filter_on",
                &on.run_id,
                on_row,
                on_text,
                on_final,
                on_attribution,
                "original",
                "raw_audio_fallback",
                true,
            )
        };
        let mut roster =
            vec![serde_json::json!({"source_case_id":off_row.case_id,"repeat":off_row.repeat})];
        if missing_peer {
            roster.push(serde_json::json!({"source_case_id":"synthetic-off-only","repeat":1}));
        }
        let evidence = serde_json::json!({
            "artifact_type":"voiceflow_filter_evidence","schema_version":1,"evidence_source":"synthetic","quality_eligibility":"synthetic_only",
            "input_runs":[
                {"arm":"filter_off","relative_path":"run-off.json","sha256":validation::sha256(&off_bytes),"run_id":off.run_id,"selected_job_id":"same-job-id"},
                {"arm":"filter_on","relative_path":"run-on.json","sha256":validation::sha256(&on_bytes),"run_id":on.run_id,"selected_job_id":"same-job-id"}
            ],
            "roster":roster,
            "pairs":[{"source_case_id":off_row.case_id,"repeat":off_row.repeat,"original_audio":off_row.audio,
                "filtered_audio":if filtered_selection {serde_json::json!({"state":"observed","identity":on_row.audio})} else {serde_json::json!({"state":"unknown","identity":null})},
                "human_gold":{"state":"synthetic_only","reviewer_ids":["synthetic-reviewer"],"protocol_id":"synthetic-protocol","locked_revision":"synthetic-r1","receipt":gold,"audio_sha256":off_row.audio.audio_sha256,"reference_binding":{"asr_reference_sha256":validation::sha256(ref_text.as_bytes())}},
                "arms":{"filter_off":arm_off,"filter_on":arm_on},"damage_annotations":damage_annotations}],
            "comparability":{"same_original_audio_gold_asr_cleanup_controls_except_selected_audio":true,"receipt":comparison},"acceptance_policy":{"synthetic_only":true}
        });
        let evidence_path = directory.write("evidence.json", &evidence);
        (evidence_path, directory.0.join("report.json"))
    }

    #[test]
    fn run_emits_descriptive_filter_delta_intervals_for_synthetic_fixture() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        evidence["pairs"][0]["cluster"] = serde_json::json!({
            "speaker_id": "synthetic-speaker",
            "source_session_id": "synthetic-session",
            "provenance": "synthetic_only"
        });
        directory.write("evidence.json", &evidence);

        assert_eq!(run(&evidence_path, &report_path).unwrap(), 3);
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
        for name in ["descriptive_cer_delta", "descriptive_wer_delta"] {
            let interval = &report[name];
            assert_eq!(interval["state"], "observed");
            assert_eq!(interval["point_estimate"], 0.5);
            assert_eq!(interval["percentile_95"], serde_json::json!([0.5, 0.5]));
            assert_eq!(interval["bootstrap_replicates"], 10000);
            assert_eq!(interval["seed"], 19);
            assert_eq!(interval["requested_count"], 1);
        }
        assert_eq!(report["criteria"]["cer_delta_ci"]["state"], "unknown");
        assert_eq!(report["criteria"]["wer_delta_ci"]["state"], "unknown");
        assert_eq!(report["status"], "unknown");
    }

    #[test]
    fn repeated_trial_id_across_unproven_rows_remains_unknown() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, true);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let on_path = directory
            .0
            .join(evidence["input_runs"][1]["relative_path"].as_str().unwrap());
        let mut on_run: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&on_path).unwrap()).unwrap();
        let mut second_on_row = on_run["cases"][0].clone();
        second_on_row["case_id"] = serde_json::json!("synthetic-off-only");
        on_run["cases"].as_array_mut().unwrap().push(second_on_row);
        let on_bytes = serde_json::to_vec(&on_run).unwrap();
        directory.write("run-on.json", &on_run);
        evidence["input_runs"][1]["sha256"] = serde_json::json!(validation::sha256(&on_bytes));
        evidence["comparability"] = serde_json::Value::Null;

        let first_ref = evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"].clone();
        let first_path = directory
            .0
            .join(first_ref["relative_path"].as_str().unwrap());
        let mut first_proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(first_path).unwrap()).unwrap();
        first_proof["run_sha256"] = evidence["input_runs"][1]["sha256"].clone();
        rebind_trial_proof(&directory, &mut first_proof, "cp-first-run-updated");
        evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"] = receipt(
            &directory,
            "cp-trial-first-run-updated.json",
            first_proof.clone(),
        );

        let mut second_pair = evidence["pairs"][0].clone();
        second_pair["source_case_id"] = serde_json::json!("synthetic-off-only");
        for arm in ["filter_off", "filter_on"] {
            second_pair["arms"][arm]["row_binding"]["case_id"] =
                serde_json::json!("synthetic-off-only");
        }
        let mut second_proof = first_proof;
        second_proof["row_binding"]["case_id"] = serde_json::json!("synthetic-off-only");
        rebind_trial_proof(&directory, &mut second_proof, "cp-second-case");
        second_pair["arms"]["filter_on"]["trial_proof"] = receipt(
            &directory,
            "cp-trial-second-case-same-id.json",
            second_proof,
        );
        evidence["pairs"].as_array_mut().unwrap().push(second_pair);
        directory.write("evidence.json", &evidence);
        assert_eq!(run(&evidence_path, &report_path).unwrap(), 3);
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(report_path).unwrap()).unwrap();
        assert_eq!(report["bystander_final_statistics"]["requested_rows"], 2);
        assert_eq!(
            report["bystander_final_statistics"]["independent_n"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn same_trial_unit_roster_declarations_for_other_repeat_do_not_conflict() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let off_run_path = directory
            .0
            .join(evidence["input_runs"][0]["relative_path"].as_str().unwrap());
        let on_run_path = directory
            .0
            .join(evidence["input_runs"][1]["relative_path"].as_str().unwrap());
        let mut off_run: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&off_run_path).unwrap()).unwrap();
        let mut on_run: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&on_run_path).unwrap()).unwrap();
        let mut second_off = off_run["cases"][0].clone();
        let mut second_on = on_run["cases"][0].clone();
        second_off["repeat"] = serde_json::json!(2);
        second_on["repeat"] = serde_json::json!(2);
        off_run["cases"].as_array_mut().unwrap().push(second_off);
        on_run["cases"].as_array_mut().unwrap().push(second_on);
        let off_written = directory.write("run-off.json", &off_run);
        let on_written = directory.write("run-on.json", &on_run);
        let off_sha = validation::sha256(&std::fs::read(off_written).unwrap());
        let on_sha = validation::sha256(&std::fs::read(on_written).unwrap());
        evidence["input_runs"][0]["sha256"] = serde_json::json!(off_sha);
        evidence["input_runs"][1]["sha256"] = serde_json::json!(on_sha);
        evidence["comparability"] = serde_json::Value::Null;
        let repeated_case_id = evidence["pairs"][0]["source_case_id"].clone();
        evidence["roster"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "source_case_id": repeated_case_id,
                "repeat": 2
            }));

        let old_proof_ref = evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"].clone();
        let old_proof_path = directory
            .0
            .join(old_proof_ref["relative_path"].as_str().unwrap());
        let original_proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(old_proof_path).unwrap()).unwrap();
        let protocol_ref = original_proof["protocol"].clone();
        let old_roster_path = directory
            .0
            .join(original_proof["roster"]["relative_path"].as_str().unwrap());
        let mut roster: serde_json::Value =
            serde_json::from_slice(&std::fs::read(old_roster_path).unwrap()).unwrap();
        let mut second_declaration = roster["trials"][0].clone();
        roster["trials"][0]["run_sha256"] = serde_json::json!(on_sha);
        roster["trials"][0]["row_binding"]["repeat"] = serde_json::json!(1);
        second_declaration["run_sha256"] = serde_json::json!(on_sha);
        second_declaration["row_binding"]["repeat"] = serde_json::json!(2);
        roster["trials"]
            .as_array_mut()
            .unwrap()
            .push(second_declaration);
        let roster_ref = receipt(&directory, "cp-roster-two-repeats.json", roster);
        let old_independence_path = directory.0.join(
            original_proof["independence"]["relative_path"]
                .as_str()
                .unwrap(),
        );
        let mut independence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(old_independence_path).unwrap()).unwrap();
        independence["roster_sha256"] = roster_ref["sha256"].clone();
        let independence_ref =
            receipt(&directory, "cp-independence-two-repeats.json", independence);

        let mut first_proof = original_proof.clone();
        first_proof["run_sha256"] = serde_json::json!(on_sha);
        first_proof["roster"] = roster_ref.clone();
        first_proof["independence"] = independence_ref.clone();
        evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"] =
            receipt(&directory, "cp-trial-repeat-one.json", first_proof);
        let mut second_pair = evidence["pairs"][0].clone();
        second_pair["repeat"] = serde_json::json!(2);
        for arm in ["filter_off", "filter_on"] {
            second_pair["arms"][arm]["row_binding"]["repeat"] = serde_json::json!(2);
        }
        let mut second_proof = original_proof;
        second_proof["run_sha256"] = serde_json::json!(on_sha);
        second_proof["row_binding"]["repeat"] = serde_json::json!(2);
        second_proof["roster"] = roster_ref;
        second_proof["independence"] = independence_ref;
        second_pair["arms"]["filter_on"]["trial_proof"] =
            receipt(&directory, "cp-trial-repeat-two.json", second_proof);
        evidence["pairs"].as_array_mut().unwrap().push(second_pair);
        directory.write("evidence.json", &evidence);

        assert_eq!(run(&evidence_path, &report_path).unwrap(), 3);
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
        assert_eq!(report["bystander_final_statistics"]["requested_rows"], 2);
        assert_eq!(
            report["bystander_final_statistics"]["unknown_qualification_rows"],
            2
        );
    }

    #[test]
    fn binary_confidence_keeps_missing_events_conservative_and_unknown_qualification_out_of_ci() {
        let missing = finalize_binary_confidence(&[BernoulliTrial {
            applicable: Some(true),
            qualified: true,
            event: None,
            identity: None,
        }]);
        assert_eq!(missing.independent_n, Some(1));
        assert_eq!(missing.conservative_errors, Some(1));
        assert_eq!(missing.upper_95, Some(1.0));
        let observed = finalize_binary_confidence(&[BernoulliTrial {
            applicable: Some(true),
            qualified: true,
            event: Some(false),
            identity: None,
        }]);
        assert_eq!(observed.conservative_errors, Some(0));
        assert!((observed.upper_95.unwrap() - 0.975).abs() < 1e-12);
        let unknown = finalize_binary_confidence(&[BernoulliTrial {
            applicable: Some(true),
            qualified: false,
            event: Some(false),
            identity: None,
        }]);
        assert_eq!(unknown.independent_n, None);
        assert_eq!(unknown.upper_95, None);
    }

    #[test]
    fn absent_owner_event_does_not_erase_proved_qualification() {
        let (applicable, qualified) = trial_admission(Some(true), true, true, true);
        assert_eq!(applicable, Some(true));
        assert!(qualified);
        let confidence = finalize_binary_confidence(&[BernoulliTrial {
            applicable,
            qualified,
            event: None,
            identity: None,
        }]);
        assert_eq!(confidence.unknown_qualification_rows, 0);
        assert_eq!(confidence.independent_n, Some(1));
        assert_eq!(confidence.conservative_errors, Some(1));
        assert_eq!(confidence.upper_95, Some(1.0));
    }

    #[test]
    fn missing_repeated_event_is_not_a_known_event_conflict() {
        let identity = TrialIdentity {
            id: "trial-1".into(),
            proof: "unit|protocol|roster|independence".into(),
        };
        let confidence = finalize_binary_confidence(&[
            BernoulliTrial {
                applicable: Some(true),
                qualified: true,
                event: Some(false),
                identity: Some(identity.clone()),
            },
            BernoulliTrial {
                applicable: Some(true),
                qualified: true,
                event: None,
                identity: Some(identity),
            },
        ]);
        assert_eq!(confidence.requested_rows, 2);
        assert_eq!(confidence.qualified_subset_unique_trials, 1);
        assert_eq!(confidence.unknown_qualification_rows, 0);
        assert_eq!(confidence.unknown_event_rows, 1);
        assert_eq!(confidence.independent_n, Some(1));
        assert_eq!(confidence.conservative_errors, Some(1));
    }

    #[test]
    fn incomplete_trial_admission_keeps_eligibility_unknown() {
        let (applicable, qualified) = trial_admission(Some(false), false, false, true);
        assert_eq!(applicable, None);
        assert!(!qualified);
        let confidence = finalize_binary_confidence(&[BernoulliTrial {
            applicable,
            qualified,
            event: Some(false),
            identity: None,
        }]);
        assert_eq!(confidence.unknown_eligibility_rows, 1);
        assert_eq!(confidence.independent_n, None);
    }

    #[test]
    fn repeated_consistent_trial_id_with_conflicting_events_is_unknown() {
        let identity = TrialIdentity {
            id: "trial-1".into(),
            proof: "unit|protocol|roster|independence".into(),
        };
        let confidence = finalize_binary_confidence(&[
            BernoulliTrial {
                applicable: Some(true),
                qualified: true,
                event: Some(false),
                identity: Some(identity.clone()),
            },
            BernoulliTrial {
                applicable: Some(true),
                qualified: true,
                event: Some(true),
                identity: Some(identity),
            },
        ]);
        assert_eq!(confidence.requested_rows, 2);
        assert_eq!(confidence.qualified_subset_unique_trials, 0);
        assert_eq!(confidence.unknown_qualification_rows, 2);
        assert_eq!(confidence.independent_n, None);
        assert_eq!(confidence.conservative_errors, None);
        assert_eq!(confidence.upper_95, None);
    }

    #[test]
    fn repeated_trial_id_with_conflicting_proof_signature_stays_unknown() {
        let confidence = finalize_binary_confidence(&[
            BernoulliTrial {
                applicable: Some(true),
                qualified: true,
                event: Some(false),
                identity: Some(TrialIdentity {
                    id: "trial-1".into(),
                    proof: "unit|protocol-a|roster|independence".into(),
                }),
            },
            BernoulliTrial {
                applicable: Some(true),
                qualified: true,
                event: Some(false),
                identity: Some(TrialIdentity {
                    id: "trial-1".into(),
                    proof: "unit|protocol-b|roster|independence".into(),
                }),
            },
        ]);
        assert_eq!(confidence.requested_rows, 2);
        assert_eq!(confidence.unknown_qualification_rows, 2);
        assert_eq!(confidence.independent_n, None);
        assert_eq!(confidence.state, "unknown");
    }

    #[test]
    fn synthetic_cp_proof_dag_is_consumed_but_never_qualifies() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        assert_eq!(run(&evidence_path, &report_path).unwrap(), 3);
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(report_path).unwrap()).unwrap();
        assert_eq!(report["false_reject_statistics"]["requested_rows"], 1);
        assert_eq!(
            report["false_reject_statistics"]["independent_n"],
            serde_json::Value::Null
        );
        assert_eq!(report["false_reject_statistics"]["state"], "unknown");
        assert_eq!(
            report["bystander_final_statistics"]["observed_event_rows"],
            1
        );
        assert_eq!(
            report["bystander_final_statistics"]["observed_error_rows"],
            1
        );
        assert_eq!(
            report["bystander_final_statistics"]["unknown_event_rows"],
            0
        );
        assert_eq!(report["false_reject_statistics"]["tail_alpha"], 0.025);
    }

    #[test]
    fn present_trial_proof_run_binding_conflict_is_rejected() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let old_ref = evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"].clone();
        let old_path = directory.0.join(old_ref["relative_path"].as_str().unwrap());
        let mut proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(old_path).unwrap()).unwrap();
        proof["run_sha256"] = serde_json::json!("0".repeat(64));
        let new_ref = receipt(&directory, "cp-trial-conflict.json", proof);
        evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"] = new_ref;
        directory.write("evidence.json", &evidence);
        let error = run(&evidence_path, &report_path).unwrap_err();
        assert!(error
            .to_string()
            .contains("trial proof does not bind its selected arm row, run and original audio"));
    }

    #[test]
    fn present_trial_proof_wrong_type_identity_is_rejected() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let old_ref = evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"].clone();
        let old_path = directory.0.join(old_ref["relative_path"].as_str().unwrap());
        let mut proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(old_path).unwrap()).unwrap();
        proof["row_binding"]["repeat"] = serde_json::json!("1");
        let new_ref = receipt(&directory, "cp-trial-wrong-type.json", proof);
        evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"] = new_ref;
        directory.write("evidence.json", &evidence);
        let error = run(&evidence_path, &report_path).unwrap_err();
        assert!(error
            .to_string()
            .contains("trial proof does not bind its selected arm row, run and original audio"));
    }

    #[test]
    fn present_wrong_type_forward_sha_is_not_treated_as_missing() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let old_ref = evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"].clone();
        let old_path = directory.0.join(old_ref["relative_path"].as_str().unwrap());
        let mut proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(old_path).unwrap()).unwrap();
        let roster_ref = proof["roster"].clone();
        let roster_path = directory
            .0
            .join(roster_ref["relative_path"].as_str().unwrap());
        let mut roster: serde_json::Value =
            serde_json::from_slice(&std::fs::read(roster_path).unwrap()).unwrap();
        roster["protocol_sha256"] = serde_json::json!(7);
        let roster_ref = receipt(&directory, "cp-roster-wrong-sha-type.json", roster);
        proof["roster"] = roster_ref;
        let new_ref = receipt(&directory, "cp-trial-roster-wrong-sha-type.json", proof);
        evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"] = new_ref;
        directory.write("evidence.json", &evidence);
        let error = run(&evidence_path, &report_path).unwrap_err();
        assert!(error
            .to_string()
            .contains("trial roster declares a different protocol receipt SHA"));
    }

    #[test]
    fn missing_trial_id_and_false_eligibility_remain_unknown_not_not_applicable() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let old_ref = evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"].clone();
        let old_path = directory.0.join(old_ref["relative_path"].as_str().unwrap());
        let mut proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(old_path).unwrap()).unwrap();
        proof["trial_id"] = serde_json::Value::Null;
        proof["eligibility"]["owner_false_reject"] = serde_json::json!(false);
        proof["eligibility"]["bystander_final"] = serde_json::json!(false);
        let roster_ref = proof["roster"].clone();
        let roster_path = directory
            .0
            .join(roster_ref["relative_path"].as_str().unwrap());
        let mut roster: serde_json::Value =
            serde_json::from_slice(&std::fs::read(roster_path).unwrap()).unwrap();
        roster["trials"][0]["trial_id"] = serde_json::Value::Null;
        roster["trials"][0]["eligibility"] = proof["eligibility"].clone();
        let new_roster = receipt(&directory, "cp-roster-no-id.json", roster);
        proof["roster"] = new_roster.clone();
        let old_independence = proof["independence"].clone();
        let independence_path = directory
            .0
            .join(old_independence["relative_path"].as_str().unwrap());
        let mut independence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(independence_path).unwrap()).unwrap();
        independence["roster_sha256"] = new_roster["sha256"].clone();
        proof["independence"] = receipt(&directory, "cp-independence-no-id.json", independence);
        let new_ref = receipt(&directory, "cp-trial-no-id.json", proof);
        evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"] = new_ref;
        directory.write("evidence.json", &evidence);
        assert_eq!(run(&evidence_path, &report_path).unwrap(), 3);
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(report_path).unwrap()).unwrap();
        assert_eq!(report["false_reject_statistics"]["not_applicable_rows"], 0);
        assert_eq!(
            report["false_reject_statistics"]["unknown_eligibility_rows"],
            1
        );
        assert_eq!(
            report["false_reject_statistics"]["independent_n"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn missing_explicit_trial_unit_keeps_false_eligibility_unknown() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let old_ref = evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"].clone();
        let old_path = directory.0.join(old_ref["relative_path"].as_str().unwrap());
        let mut proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(old_path).unwrap()).unwrap();
        proof["eligibility"]["owner_false_reject"] = serde_json::json!(false);
        proof["trial_unit"] = serde_json::Value::Null;
        let roster_ref = proof["roster"].clone();
        let roster_path = directory
            .0
            .join(roster_ref["relative_path"].as_str().unwrap());
        let mut roster: serde_json::Value =
            serde_json::from_slice(&std::fs::read(roster_path).unwrap()).unwrap();
        roster["trials"][0]["eligibility"] = proof["eligibility"].clone();
        let new_roster = receipt(&directory, "cp-roster-missing-unit.json", roster);
        proof["roster"] = new_roster.clone();
        let independence_ref = proof["independence"].clone();
        let independence_path = directory
            .0
            .join(independence_ref["relative_path"].as_str().unwrap());
        let mut independence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(independence_path).unwrap()).unwrap();
        independence["roster_sha256"] = new_roster["sha256"].clone();
        proof["independence"] = receipt(
            &directory,
            "cp-independence-missing-unit.json",
            independence,
        );
        let new_ref = receipt(&directory, "cp-trial-missing-unit.json", proof);
        evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"] = new_ref;
        directory.write("evidence.json", &evidence);
        assert_eq!(run(&evidence_path, &report_path).unwrap(), 3);
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(report_path).unwrap()).unwrap();
        assert_eq!(report["false_reject_statistics"]["not_applicable_rows"], 0);
        assert_eq!(
            report["false_reject_statistics"]["unknown_eligibility_rows"],
            1
        );
        assert_eq!(
            report["false_reject_statistics"]["independent_n"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn missing_peer_arm_keeps_declared_count_and_validates_present_on_arm() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        evidence["pairs"][0]["arms"]
            .as_object_mut()
            .unwrap()
            .remove("filter_off");
        directory.write("evidence.json", &evidence);
        assert_eq!(run(&evidence_path, &report_path).unwrap(), 3);
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(report_path).unwrap()).unwrap();
        assert_eq!(report["false_reject_statistics"]["requested_rows"], 1);
        assert_eq!(report["bystander_final_statistics"]["requested_rows"], 1);
        assert_eq!(
            report["bystander_final_statistics"]["observed_event_rows"],
            1
        );
        assert_eq!(
            report["bystander_final_statistics"]["observed_error_rows"],
            1
        );
        assert_eq!(
            report["bystander_final_statistics"]["independent_n"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn missing_on_arm_keeps_one_declared_unknown_row() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        evidence["pairs"][0]["arms"]
            .as_object_mut()
            .unwrap()
            .remove("filter_on");
        directory.write("evidence.json", &evidence);
        assert_eq!(run(&evidence_path, &report_path).unwrap(), 3);
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(report_path).unwrap()).unwrap();
        assert_eq!(report["false_reject_statistics"]["requested_rows"], 1);
        assert_eq!(
            report["false_reject_statistics"]["unknown_eligibility_rows"],
            1
        );
        assert_eq!(report["bystander_final_statistics"]["requested_rows"], 1);
        assert_eq!(
            report["bystander_final_statistics"]["unknown_eligibility_rows"],
            1
        );
    }

    #[test]
    fn absent_pair_keeps_selected_run_union_in_requested_denominators() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        evidence["pairs"] = serde_json::json!([]);
        directory.write("evidence.json", &evidence);
        assert_eq!(run(&evidence_path, &report_path).unwrap(), 3);
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(report_path).unwrap()).unwrap();
        assert_eq!(report["false_reject_statistics"]["requested_rows"], 1);
        assert_eq!(
            report["false_reject_statistics"]["unknown_eligibility_rows"],
            1
        );
        assert_eq!(report["bystander_final_statistics"]["requested_rows"], 1);
        assert_eq!(
            report["bystander_final_statistics"]["unknown_eligibility_rows"],
            1
        );
    }

    #[test]
    fn selected_job_with_no_rows_remains_an_identity_error() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let run_path = directory
            .0
            .join(evidence["input_runs"][1]["relative_path"].as_str().unwrap());
        let mut on_run: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&run_path).unwrap()).unwrap();
        on_run["cases"].as_array_mut().unwrap().clear();
        let written = directory.write("run-on-empty.json", &on_run);
        evidence["input_runs"][1]["relative_path"] =
            serde_json::json!(written.file_name().unwrap().to_string_lossy());
        evidence["input_runs"][1]["sha256"] =
            serde_json::json!(validation::sha256(&std::fs::read(written).unwrap()));
        directory.write("evidence.json", &evidence);
        let error = run(&evidence_path, &report_path).unwrap_err();
        assert!(error
            .to_string()
            .contains("RunRecord identity or selected job binding is inconsistent"));
    }

    #[test]
    fn missing_selected_run_peer_leaves_controls_and_quality_unknown() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let on_run_path = directory
            .0
            .join(evidence["input_runs"][1]["relative_path"].as_str().unwrap());
        let mut on_run: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&on_run_path).unwrap()).unwrap();
        let mut identity_witness = on_run["cases"][0].clone();
        identity_witness["case_id"] = serde_json::json!("identity-witness");
        on_run["cases"]
            .as_array_mut()
            .unwrap()
            .push(identity_witness);
        evidence["roster"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "source_case_id": "identity-witness",
                "repeat": on_run["cases"][1]["repeat"],
            }));
        let written = directory.write("run-on.json", &on_run);
        let on_sha = validation::sha256(&std::fs::read(written).unwrap());
        evidence["input_runs"][1]["sha256"] = serde_json::json!(on_sha);
        let comparability_ref = evidence["comparability"]["receipt"].clone();
        let comparability_path = directory
            .0
            .join(comparability_ref["relative_path"].as_str().unwrap());
        let mut comparability: serde_json::Value =
            serde_json::from_slice(&std::fs::read(comparability_path).unwrap()).unwrap();
        comparability["run_sha256"]["filter_on"] = serde_json::json!(on_sha);
        evidence["comparability"]["receipt"] = receipt(
            &directory,
            "comparability-run-on-missing.json",
            comparability,
        );
        evidence["pairs"][0]["arms"]
            .as_object_mut()
            .unwrap()
            .remove("filter_on");
        directory.write("evidence.json", &evidence);
        assert_eq!(run(&evidence_path, &report_path).unwrap(), 3);
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(report_path).unwrap()).unwrap();
        assert_eq!(report["false_reject_statistics"]["requested_rows"], 2);
        assert_eq!(
            report["false_reject_statistics"]["unknown_eligibility_rows"],
            2
        );
        assert_eq!(report["bystander_final_statistics"]["requested_rows"], 2);
        assert_eq!(
            report["bystander_final_statistics"]["unknown_eligibility_rows"],
            2
        );
    }

    #[test]
    fn missing_off_arm_does_not_hide_present_on_row_identity_conflict() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        evidence["pairs"][0]["arms"]
            .as_object_mut()
            .unwrap()
            .remove("filter_off");
        evidence["pairs"][0]["arms"]["filter_on"]["row_binding"]["job_id"] =
            serde_json::json!("wrong-job");
        directory.write("evidence.json", &evidence);
        assert!(run(&evidence_path, &report_path).is_err());
    }

    #[test]
    fn missing_off_arm_does_not_hide_present_on_final_receipt_sha_conflict() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        evidence["pairs"][0]["arms"]
            .as_object_mut()
            .unwrap()
            .remove("filter_off");
        evidence["pairs"][0]["arms"]["filter_on"]["final_attribution"]["receipt"]["sha256"] =
            serde_json::json!("0".repeat(64));
        directory.write("evidence.json", &evidence);
        assert!(run(&evidence_path, &report_path).is_err());
    }

    #[test]
    fn present_on_child_bad_sha_is_rejected_when_off_arm_is_missing() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        evidence["pairs"][0]["arms"]
            .as_object_mut()
            .unwrap()
            .remove("filter_off");
        let old_ref = evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"].clone();
        let old_path = directory.0.join(old_ref["relative_path"].as_str().unwrap());
        let mut proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(old_path).unwrap()).unwrap();
        proof["protocol"]["sha256"] = serde_json::json!("0".repeat(64));
        let new_ref = receipt(&directory, "cp-trial-bad-child-sha.json", proof);
        evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"] = new_ref;
        directory.write("evidence.json", &evidence);
        assert!(run(&evidence_path, &report_path).is_err());
    }

    #[test]
    fn missing_off_arm_does_not_hide_invalid_present_on_ranges() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        evidence["pairs"][0]["arms"]
            .as_object_mut()
            .unwrap()
            .remove("filter_off");
        evidence["pairs"][0]["arms"]["filter_on"]["final_attribution"]["utf8_ranges"] =
            serde_json::json!([[4, 999]]);
        let receipt_ref =
            evidence["pairs"][0]["arms"]["filter_on"]["final_attribution"]["receipt"].clone();
        let receipt_path = directory
            .0
            .join(receipt_ref["relative_path"].as_str().unwrap());
        let mut attribution_receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(receipt_path).unwrap()).unwrap();
        attribution_receipt["bystander_utf8_ranges"] = serde_json::json!([[4, 999]]);
        let new_receipt = receipt(&directory, "cp-on-invalid-ranges.json", attribution_receipt);
        evidence["pairs"][0]["arms"]["filter_on"]["final_attribution"]["receipt"] = new_receipt;
        directory.write("evidence.json", &evidence);
        assert!(run(&evidence_path, &report_path).is_err());
    }

    #[test]
    fn off_arm_child_bad_sha_is_rejected() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let old_ref = evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"].clone();
        let old_path = directory.0.join(old_ref["relative_path"].as_str().unwrap());
        let mut proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(old_path).unwrap()).unwrap();
        proof["run_sha256"] = evidence["input_runs"][0]["sha256"].clone();
        proof["protocol"]["sha256"] = serde_json::json!("0".repeat(64));
        evidence["pairs"][0]["arms"]["filter_off"]["trial_proof"] =
            receipt(&directory, "cp-off-trial-bad-child-sha.json", proof);
        directory.write("evidence.json", &evidence);
        assert!(run(&evidence_path, &report_path).is_err());
    }

    #[test]
    fn independence_roster_edge_is_checked_when_protocol_child_is_absent() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let old_ref = evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"].clone();
        let old_path = directory.0.join(old_ref["relative_path"].as_str().unwrap());
        let mut proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(old_path).unwrap()).unwrap();
        proof["protocol"] = serde_json::Value::Null;
        let independence_ref = proof["independence"].clone();
        let independence_path = directory
            .0
            .join(independence_ref["relative_path"].as_str().unwrap());
        let mut independence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(independence_path).unwrap()).unwrap();
        independence["roster_sha256"] = serde_json::json!("0".repeat(64));
        proof["independence"] = receipt(
            &directory,
            "cp-independence-roster-conflict.json",
            independence,
        );
        let new_ref = receipt(
            &directory,
            "cp-trial-protocol-absent-edge-conflict.json",
            proof,
        );
        evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"] = new_ref;
        directory.write("evidence.json", &evidence);
        assert!(run(&evidence_path, &report_path).is_err());
    }

    #[test]
    fn duplicate_roster_declarations_for_one_trial_are_rejected() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let old_ref = evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"].clone();
        let old_path = directory.0.join(old_ref["relative_path"].as_str().unwrap());
        let mut proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(old_path).unwrap()).unwrap();
        let old_roster = proof["roster"].clone();
        let roster_path = directory
            .0
            .join(old_roster["relative_path"].as_str().unwrap());
        let mut roster: serde_json::Value =
            serde_json::from_slice(&std::fs::read(roster_path).unwrap()).unwrap();
        let duplicate = roster["trials"][0].clone();
        roster["trials"].as_array_mut().unwrap().push(duplicate);
        let new_roster = receipt(&directory, "cp-roster-duplicate.json", roster);
        proof["roster"] = new_roster.clone();
        let old_independence = proof["independence"].clone();
        let independence_path = directory
            .0
            .join(old_independence["relative_path"].as_str().unwrap());
        let mut independence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(independence_path).unwrap()).unwrap();
        independence["roster_sha256"] = new_roster["sha256"].clone();
        proof["independence"] = receipt(
            &directory,
            "cp-independence-duplicate-roster.json",
            independence,
        );
        let new_ref = receipt(&directory, "cp-trial-duplicate-roster.json", proof);
        evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"] = new_ref;
        directory.write("evidence.json", &evidence);
        let error = run(&evidence_path, &report_path).unwrap_err();
        assert!(error
            .to_string()
            .contains("roster contains duplicate declarations for a trial_id"));
    }

    #[test]
    fn roster_job_binding_conflict_is_rejected() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let old_ref = evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"].clone();
        let old_path = directory.0.join(old_ref["relative_path"].as_str().unwrap());
        let mut proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(old_path).unwrap()).unwrap();
        let old_roster = proof["roster"].clone();
        let roster_path = directory
            .0
            .join(old_roster["relative_path"].as_str().unwrap());
        let mut roster: serde_json::Value =
            serde_json::from_slice(&std::fs::read(roster_path).unwrap()).unwrap();
        roster["trials"][0]["row_binding"]["job_id"] = serde_json::json!("wrong-job");
        let new_roster = receipt(&directory, "cp-roster-wrong-job.json", roster);
        proof["roster"] = new_roster.clone();
        let old_independence = proof["independence"].clone();
        let independence_path = directory
            .0
            .join(old_independence["relative_path"].as_str().unwrap());
        let mut independence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(independence_path).unwrap()).unwrap();
        independence["roster_sha256"] = new_roster["sha256"].clone();
        proof["independence"] = receipt(
            &directory,
            "cp-independence-wrong-roster-job.json",
            independence,
        );
        let new_ref = receipt(&directory, "cp-trial-wrong-roster-job.json", proof);
        evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"] = new_ref;
        directory.write("evidence.json", &evidence);
        let error = run(&evidence_path, &report_path).unwrap_err();
        assert!(error
            .to_string()
            .contains("roster entry contradicts selected trial row, run or audio"));
    }

    #[test]
    fn partial_owner_trial_missing_identifier_is_unknown_not_a_conflict() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let old_ref = evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"].clone();
        let old_path = directory.0.join(old_ref["relative_path"].as_str().unwrap());
        let mut proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(old_path).unwrap()).unwrap();
        proof["eligibility"]["owner_false_reject"] = serde_json::json!(true);
        let roster_ref = proof["roster"].clone();
        let roster_path = directory
            .0
            .join(roster_ref["relative_path"].as_str().unwrap());
        let mut roster: serde_json::Value =
            serde_json::from_slice(&std::fs::read(roster_path).unwrap()).unwrap();
        roster["trials"][0]["eligibility"] = proof["eligibility"].clone();
        proof["roster"] = receipt(&directory, "cp-roster-owner-unknown.json", roster);
        evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"] =
            receipt(&directory, "cp-trial-owner-unknown.json", proof);
        let binding = evidence["pairs"][0]["arms"]["filter_on"]["row_binding"].clone();
        let owner_receipt = receipt(
            &directory,
            "owner-partial-unknown.json",
            serde_json::json!({
                "event":true,"eligible":true,"trial_unit":"","protocol":"","roster":"","independence":"",
                "case_id":binding["case_id"],"repeat":binding["repeat"],"job_id":binding["job_id"]
            }),
        );
        evidence["pairs"][0]["arms"]["filter_on"]["owner_false_reject"] = serde_json::json!({
            "event":true,"eligible":true,"trial_unit":"","protocol":"","roster":"","independence":"","receipt":owner_receipt
        });
        directory.write("evidence.json", &evidence);
        assert_eq!(run(&evidence_path, &report_path).unwrap(), 3);
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(report_path).unwrap()).unwrap();
        assert_eq!(
            report["false_reject_statistics"]["unknown_qualification_rows"],
            1
        );
        assert_eq!(
            report["false_reject_statistics"]["independent_n"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn partial_owner_trial_known_eligibility_conflict_is_rejected() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let old_ref = evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"].clone();
        let old_path = directory.0.join(old_ref["relative_path"].as_str().unwrap());
        let mut proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(old_path).unwrap()).unwrap();
        proof["eligibility"]["owner_false_reject"] = serde_json::json!(true);
        let roster_ref = proof["roster"].clone();
        let roster_path = directory
            .0
            .join(roster_ref["relative_path"].as_str().unwrap());
        let mut roster: serde_json::Value =
            serde_json::from_slice(&std::fs::read(roster_path).unwrap()).unwrap();
        roster["trials"][0]["eligibility"] = proof["eligibility"].clone();
        proof["roster"] = receipt(&directory, "cp-roster-owner-eligibility.json", roster);
        rebind_trial_proof(&directory, &mut proof, "cp-owner-eligibility-rebound");
        evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"] =
            receipt(&directory, "cp-trial-owner-eligibility.json", proof);
        let binding = evidence["pairs"][0]["arms"]["filter_on"]["row_binding"].clone();
        let owner_receipt = receipt(
            &directory,
            "owner-known-eligibility-conflict.json",
            serde_json::json!({
                "event":false,"eligible":false,"trial_unit":"","protocol":"","roster":"","independence":"",
                "case_id":binding["case_id"],"repeat":binding["repeat"],"job_id":binding["job_id"]
            }),
        );
        evidence["pairs"][0]["arms"]["filter_on"]["owner_false_reject"] = serde_json::json!({
            "event":false,"eligible":false,"trial_unit":"","protocol":"","roster":"","independence":"","receipt":owner_receipt
        });
        directory.write("evidence.json", &evidence);
        let error = run(&evidence_path, &report_path).unwrap_err();
        assert!(error
            .to_string()
            .contains("trial eligibility contradicts OwnerTrial eligibility"));
    }

    #[test]
    fn owner_receipt_known_event_conflict_is_rejected_without_trial_proof() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let binding = evidence["pairs"][0]["arms"]["filter_on"]["row_binding"].clone();
        let owner_receipt = receipt(
            &directory,
            "owner-without-proof-conflict.json",
            serde_json::json!({
                "event": false,
                "eligible": true,
                "trial_unit": "",
                "protocol": "",
                "roster": "",
                "independence": "",
                "case_id": binding["case_id"],
                "repeat": binding["repeat"],
                "job_id": binding["job_id"]
            }),
        );
        evidence["pairs"][0]["arms"]["filter_on"]["owner_false_reject"] = serde_json::json!({
            "event": true,
            "eligible": true,
            "trial_unit": "",
            "protocol": "",
            "roster": "",
            "independence": "",
            "receipt": owner_receipt
        });
        directory.write("evidence.json", &evidence);
        let error = run(&evidence_path, &report_path).unwrap_err();
        assert!(error
            .to_string()
            .contains("OwnerTrial event or eligibility contradicts its receipt"));
    }

    #[test]
    fn partial_roster_row_known_eligibility_conflict_is_rejected() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let old_ref = evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"].clone();
        let old_path = directory.0.join(old_ref["relative_path"].as_str().unwrap());
        let mut proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(old_path).unwrap()).unwrap();
        proof["eligibility"]["owner_false_reject"] = serde_json::json!(true);
        let roster_ref = proof["roster"].clone();
        let roster_path = directory
            .0
            .join(roster_ref["relative_path"].as_str().unwrap());
        let mut roster: serde_json::Value =
            serde_json::from_slice(&std::fs::read(roster_path).unwrap()).unwrap();
        roster["trials"][0]["eligibility"]["owner_false_reject"] = serde_json::json!(false);
        roster["trials"][0]["row_binding"]
            .as_object_mut()
            .unwrap()
            .remove("case_id");
        let new_roster = receipt(
            &directory,
            "cp-roster-partial-owner-eligibility.json",
            roster,
        );
        proof["roster"] = new_roster.clone();
        let independence_ref = proof["independence"].clone();
        let independence_path = directory
            .0
            .join(independence_ref["relative_path"].as_str().unwrap());
        let mut independence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(independence_path).unwrap()).unwrap();
        independence["roster_sha256"] = new_roster["sha256"].clone();
        proof["independence"] = receipt(
            &directory,
            "cp-independence-partial-owner-eligibility.json",
            independence,
        );
        evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"] =
            receipt(&directory, "cp-trial-partial-owner-eligibility.json", proof);
        directory.write("evidence.json", &evidence);
        let error = run(&evidence_path, &report_path).unwrap_err();
        assert!(error
            .to_string()
            .contains("roster and trial eligibility declarations disagree"));
    }

    #[test]
    fn missing_peer_does_not_hide_present_roster_eligibility_type_error() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let old_ref = evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"].clone();
        let old_path = directory.0.join(old_ref["relative_path"].as_str().unwrap());
        let mut proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(old_path).unwrap()).unwrap();
        let roster_ref = proof["roster"].clone();
        let roster_path = directory
            .0
            .join(roster_ref["relative_path"].as_str().unwrap());
        let mut roster: serde_json::Value =
            serde_json::from_slice(&std::fs::read(roster_path).unwrap()).unwrap();
        roster["trials"][0]["eligibility"]["owner_false_reject"] = serde_json::json!("true");
        proof["roster"] = receipt(
            &directory,
            "cp-roster-wrong-owner-eligibility-type.json",
            roster,
        );
        rebind_trial_proof(&directory, &mut proof, "cp-wrong-owner-eligibility-type");
        evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"] = receipt(
            &directory,
            "cp-trial-wrong-owner-eligibility-type.json",
            proof,
        );
        evidence["pairs"][0]["arms"]
            .as_object_mut()
            .unwrap()
            .remove("filter_off");
        directory.write("evidence.json", &evidence);
        let error = run(&evidence_path, &report_path).unwrap_err();
        assert!(error
            .to_string()
            .contains("roster trial row contains a value of the wrong JSON type"));
    }

    #[test]
    fn paired_arm_trial_id_conflict_is_rejected() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let on_ref = evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"].clone();
        let on_path = directory.0.join(on_ref["relative_path"].as_str().unwrap());
        let mut off_proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(on_path).unwrap()).unwrap();
        off_proof["run_sha256"] = evidence["input_runs"][0]["sha256"].clone();
        off_proof["trial_id"] = serde_json::json!("different-paired-trial-id");
        evidence["pairs"][0]["arms"]["filter_off"]["trial_proof"] =
            receipt(&directory, "cp-off-trial-different-id.json", off_proof);
        directory.write("evidence.json", &evidence);
        assert!(run(&evidence_path, &report_path).is_err());
    }

    #[test]
    fn paired_off_on_same_trial_id_counts_as_one_requested_trial() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let on_ref = evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"].clone();
        let on_path = directory.0.join(on_ref["relative_path"].as_str().unwrap());
        let mut off_proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(on_path).unwrap()).unwrap();
        off_proof["run_sha256"] = evidence["input_runs"][0]["sha256"].clone();
        rebind_trial_proof(&directory, &mut off_proof, "cp-off-same-id");
        evidence["pairs"][0]["arms"]["filter_off"]["trial_proof"] =
            receipt(&directory, "cp-off-trial-same-id.json", off_proof);
        directory.write("evidence.json", &evidence);
        assert_eq!(run(&evidence_path, &report_path).unwrap(), 3);
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(report_path).unwrap()).unwrap();
        assert_eq!(report["bystander_final_statistics"]["requested_rows"], 1);
        assert_eq!(
            report["bystander_final_statistics"]["independent_n"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn partial_owner_trial_known_protocol_conflict_is_rejected() {
        let directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&directory, false, false);
        add_trial_proof(&directory, &evidence_path);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let old_ref = evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"].clone();
        let old_path = directory.0.join(old_ref["relative_path"].as_str().unwrap());
        let mut proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(old_path).unwrap()).unwrap();
        let protocol_ref = proof["protocol"].clone();
        let protocol_path = directory
            .0
            .join(protocol_ref["relative_path"].as_str().unwrap());
        let mut protocol: serde_json::Value =
            serde_json::from_slice(&std::fs::read(protocol_path).unwrap()).unwrap();
        protocol["protocol_id"] = serde_json::json!("actual-protocol-id");
        let new_protocol = receipt(&directory, "cp-protocol-owner-conflict.json", protocol);
        proof["protocol"] = new_protocol.clone();
        let roster_ref = proof["roster"].clone();
        let roster_path = directory
            .0
            .join(roster_ref["relative_path"].as_str().unwrap());
        let mut roster: serde_json::Value =
            serde_json::from_slice(&std::fs::read(roster_path).unwrap()).unwrap();
        roster["protocol_sha256"] = new_protocol["sha256"].clone();
        let new_roster = receipt(&directory, "cp-roster-owner-conflict.json", roster);
        proof["roster"] = new_roster.clone();
        let independence_ref = proof["independence"].clone();
        let independence_path = directory
            .0
            .join(independence_ref["relative_path"].as_str().unwrap());
        let mut independence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(independence_path).unwrap()).unwrap();
        independence["protocol_sha256"] = new_protocol["sha256"].clone();
        independence["roster_sha256"] = new_roster["sha256"].clone();
        proof["independence"] = receipt(
            &directory,
            "cp-independence-owner-conflict.json",
            independence,
        );
        let proof_ref = receipt(&directory, "cp-trial-owner-conflict.json", proof);
        evidence["pairs"][0]["arms"]["filter_on"]["trial_proof"] = proof_ref;
        let binding = evidence["pairs"][0]["arms"]["filter_on"]["row_binding"].clone();
        let owner_receipt = receipt(
            &directory,
            "owner-partial-conflict.json",
            serde_json::json!({
                "event":true,"eligible":true,"trial_unit":"","protocol":"declared-other","roster":"","independence":"",
                "case_id":binding["case_id"],"repeat":binding["repeat"],"job_id":binding["job_id"]
            }),
        );
        evidence["pairs"][0]["arms"]["filter_on"]["owner_false_reject"] = serde_json::json!({
            "event":true,"eligible":true,"trial_unit":"","protocol":"declared-other","roster":"","independence":"","receipt":owner_receipt
        });
        directory.write("evidence.json", &evidence);
        let error = run(&evidence_path, &report_path).unwrap_err();
        assert!(error
            .to_string()
            .contains("OwnerTrial receipt or declaration contradicts trial proof DAG"));
    }

    #[tokio::test]
    async fn main_entry_scores_bound_synthetic_runs_with_scoped_equal_job_ids_and_exclusive_report()
    {
        let directory = FixtureDirectory::new();
        let (evidence, report) = write_case(&directory, false, false);
        let args = vec![
            "--filter-evidence".into(),
            evidence.display().to_string(),
            "--filter-report".into(),
            report.display().to_string(),
        ];
        assert_eq!(crate::evaluation::main(args.clone()).await.unwrap(), 3);
        let result: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
        assert_eq!(result["status"], "unknown");
        assert_eq!(result["pairs"][0]["synthetic"], true);
        assert!(result["pairs"][0]["cer_off"].is_number());
        assert!(result["pairs"][0]["cer_on"].is_number());
        assert_eq!(result["pairs"][0]["cer_off"], 0.0);
        assert_eq!(result["pairs"][0]["cer_on"], 0.5);
        assert_eq!(result["pairs"][0]["cer_on_minus_off"], 0.5);
        assert_eq!(result["pairs"][0]["cer_upper_bound"], "finite");
        assert_eq!(result["pairs"][0]["wer_off"], 0.0);
        assert_eq!(result["pairs"][0]["wer_on"], 0.5);
        assert_eq!(result["pairs"][0]["wer_on_minus_off"], 0.5);
        assert_eq!(result["pairs"][0]["wer_upper_bound"], "finite");
        assert_eq!(result["final_bystander_expected_arm_units"], 2);
        assert_eq!(result["final_bystander_reviewed_arm_units"], 2);
        assert_eq!(result["final_bystander_observed_subset_spans"], 1);
        assert_eq!(result["final_bystander_observed_subset_leaks"], 1);
        assert_eq!(result["damage_by_kind"]["number"]["reviewed_arm_units"], 2);
        assert_eq!(
            result["damage_by_kind"]["number"]["observed_subset_spans"],
            2
        );
        assert_eq!(
            result["damage_by_kind"]["number"]["observed_subset_errors"],
            1
        );
        assert!(crate::evaluation::main(args).await.is_err());

        let filtered_directory = FixtureDirectory::new();
        let (filtered_evidence, filtered_report) = write_case(&filtered_directory, true, false);
        let filtered_args = vec![
            "--filter-evidence".into(),
            filtered_evidence.display().to_string(),
            "--filter-report".into(),
            filtered_report.display().to_string(),
        ];
        assert_eq!(crate::evaluation::main(filtered_args).await.unwrap(), 3);
        let filtered_result: serde_json::Value =
            serde_json::from_slice(&std::fs::read(filtered_report).unwrap()).unwrap();
        assert!(filtered_result["pairs"][0]["cer_on"].is_number());

        let missing_directory = FixtureDirectory::new();
        let (missing_evidence, missing_report) = write_case(&missing_directory, false, true);
        let missing_args = vec![
            "--filter-evidence".into(),
            missing_evidence.display().to_string(),
            "--filter-report".into(),
            missing_report.display().to_string(),
        ];
        assert_eq!(crate::evaluation::main(missing_args).await.unwrap(), 3);
        let missing_result: serde_json::Value =
            serde_json::from_slice(&std::fs::read(missing_report).unwrap()).unwrap();
        assert_eq!(missing_result["final_bystander_expected_arm_units"], 4);
        assert_eq!(missing_result["pairs"].as_array().unwrap().len(), 2);
        assert_eq!(
            missing_result["pairs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["case_id"] == "synthetic-off-only")
                .unwrap()["cer_state"],
            "unknown"
        );

        let absent_directory = FixtureDirectory::new();
        let (absent_evidence, absent_report) = write_case(&absent_directory, false, false);
        let mut absent: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&absent_evidence).unwrap()).unwrap();
        absent.as_object_mut().unwrap().remove("comparability");
        absent["pairs"][0]
            .as_object_mut()
            .unwrap()
            .remove("human_gold");
        absent["pairs"][0]["arms"]["filter_off"]
            .as_object_mut()
            .unwrap()
            .remove("final_attribution");
        absent["pairs"][0]["arms"]["filter_on"]
            .as_object_mut()
            .unwrap()
            .remove("final_attribution");
        let absent_path = absent_directory.write("evidence-absent-proof.json", &absent);
        let absent_args = vec![
            "--filter-evidence".into(),
            absent_path.display().to_string(),
            "--filter-report".into(),
            absent_report.display().to_string(),
        ];
        assert_eq!(crate::evaluation::main(absent_args).await.unwrap(), 3);
        let absent_result: serde_json::Value =
            serde_json::from_slice(&std::fs::read(absent_report).unwrap()).unwrap();
        assert_eq!(absent_result["status"], "unknown");
        assert!(absent_result["pairs"][0]["cer_off"].is_number());
        assert!(absent_result["pairs"][0]["cer_on"].is_number());
        assert!(absent_result["pairs"][0]["cer_on_minus_off"].is_null());
        assert!(absent_result["pairs"][0]["wer_on_minus_off"].is_null());
        assert_eq!(
            absent_result["pairs"][0]["cer_upper_bound"],
            "upper_unbounded"
        );
        assert_eq!(
            absent_result["pairs"][0]["wer_upper_bound"],
            "upper_unbounded"
        );

        let contradictory_directory = FixtureDirectory::new();
        let (contradictory_evidence, contradictory_report) =
            write_case(&contradictory_directory, false, false);
        let mut contradictory: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&contradictory_evidence).unwrap()).unwrap();
        contradictory["pairs"][0]["arms"]["filter_on"]["final_attribution"]
            ["final_text_utf8_sha256"] = serde_json::json!("0".repeat(64));
        let contradictory_path =
            contradictory_directory.write("evidence-contradictory-final-sha.json", &contradictory);
        let contradictory_args = vec![
            "--filter-evidence".into(),
            contradictory_path.display().to_string(),
            "--filter-report".into(),
            contradictory_report.display().to_string(),
        ];
        assert!(crate::evaluation::main(contradictory_args).await.is_err());

        let mut bad_hash: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence).unwrap()).unwrap();
        bad_hash["input_runs"][0]["sha256"] = serde_json::json!("f".repeat(64));
        let bad_hash_path = directory.write("evidence-bad-hash.json", &bad_hash);
        let bad_hash_args = vec![
            "--filter-evidence".into(),
            bad_hash_path.display().to_string(),
            "--filter-report".into(),
            directory
                .0
                .join("bad-hash-report.json")
                .display()
                .to_string(),
        ];
        assert!(crate::evaluation::main(bad_hash_args).await.is_err());

        let mut bad_range: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence).unwrap()).unwrap();
        bad_range["pairs"][0]["arms"]["filter_on"]["final_attribution"]["utf8_ranges"] =
            serde_json::json!([[0, 999]]);
        let bad_range_path = directory.write("evidence-bad-range.json", &bad_range);
        let bad_range_args = vec![
            "--filter-evidence".into(),
            bad_range_path.display().to_string(),
            "--filter-report".into(),
            directory
                .0
                .join("bad-range-report.json")
                .display()
                .to_string(),
        ];
        assert!(crate::evaluation::main(bad_range_args).await.is_err());
    }

    #[tokio::test]
    async fn partial_receipt_proofs_keep_unknown_coverage_and_reject_present_conflicts() {
        let attribution_directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&attribution_directory, false, false);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let mut receipt_value: serde_json::Value = serde_json::from_slice(
            &std::fs::read(attribution_directory.0.join("attribution-on.json")).unwrap(),
        )
        .unwrap();
        receipt_value
            .as_object_mut()
            .unwrap()
            .remove("bystander_utf8_ranges");
        let receipt_ref = receipt(
            &attribution_directory,
            "attribution-on-missing-ranges.json",
            receipt_value,
        );
        evidence["pairs"][0]["arms"]["filter_on"]["final_attribution"]["receipt"] = receipt_ref;
        let evidence_path =
            attribution_directory.write("evidence-attribution-missing-ranges.json", &evidence);
        let args = vec![
            "--filter-evidence".into(),
            evidence_path.display().to_string(),
            "--filter-report".into(),
            report_path.display().to_string(),
        ];
        assert_eq!(crate::evaluation::main(args).await.unwrap(), 3);
        let result: serde_json::Value =
            serde_json::from_slice(&std::fs::read(report_path).unwrap()).unwrap();
        assert_eq!(result["final_bystander_reviewed_arm_units"], 1);
        assert_eq!(result["final_bystander_expected_arm_units"], 2);

        let damage_directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&damage_directory, false, false);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let mut receipt_value: serde_json::Value = serde_json::from_slice(
            &std::fs::read(damage_directory.0.join("damage-head.json")).unwrap(),
        )
        .unwrap();
        receipt_value
            .as_object_mut()
            .unwrap()
            .remove("reference_material_sha256");
        let missing_ref = receipt(
            &damage_directory,
            "damage-head-missing-material.json",
            receipt_value.clone(),
        );
        let mut missing_only: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        missing_only["pairs"][0]["damage_annotations"][0]["receipt"] = missing_ref;
        let missing_path =
            damage_directory.write("evidence-damage-missing-material.json", &missing_only);
        let missing_args = vec![
            "--filter-evidence".into(),
            missing_path.display().to_string(),
            "--filter-report".into(),
            damage_directory
                .0
                .join("damage-missing-report.json")
                .display()
                .to_string(),
        ];
        assert_eq!(crate::evaluation::main(missing_args).await.unwrap(), 3);
        receipt_value["kind"] = serde_json::json!("tail");
        let receipt_ref = receipt(
            &damage_directory,
            "damage-head-missing-proof-conflict.json",
            receipt_value,
        );
        evidence["pairs"][0]["damage_annotations"][0]["receipt"] = receipt_ref;
        let evidence_path =
            damage_directory.write("evidence-damage-missing-proof-conflict.json", &evidence);
        let args = vec![
            "--filter-evidence".into(),
            evidence_path.display().to_string(),
            "--filter-report".into(),
            report_path.display().to_string(),
        ];
        assert!(crate::evaluation::main(args).await.is_err());

        let gold_directory = FixtureDirectory::new();
        let (evidence_path, report_path) = write_case(&gold_directory, false, false);
        let mut evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let mut receipt_value: serde_json::Value = serde_json::from_slice(
            &std::fs::read(gold_directory.0.join("gold-receipt.json")).unwrap(),
        )
        .unwrap();
        receipt_value
            .as_object_mut()
            .unwrap()
            .remove("reviewer_ids");
        let missing_ref = receipt(
            &gold_directory,
            "gold-missing-reviewers.json",
            receipt_value.clone(),
        );
        let mut missing_only: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        missing_only["pairs"][0]["human_gold"]["receipt"] = missing_ref;
        let missing_path =
            gold_directory.write("evidence-gold-missing-reviewers.json", &missing_only);
        let missing_args = vec![
            "--filter-evidence".into(),
            missing_path.display().to_string(),
            "--filter-report".into(),
            gold_directory
                .0
                .join("gold-missing-report.json")
                .display()
                .to_string(),
        ];
        assert_eq!(crate::evaluation::main(missing_args).await.unwrap(), 3);
        receipt_value["asr_reference_sha256"] = serde_json::json!("f".repeat(64));
        let receipt_ref = receipt(
            &gold_directory,
            "gold-missing-reviewer-conflict.json",
            receipt_value,
        );
        evidence["pairs"][0]["human_gold"]["receipt"] = receipt_ref;
        let evidence_path =
            gold_directory.write("evidence-gold-missing-reviewer-conflict.json", &evidence);
        let args = vec![
            "--filter-evidence".into(),
            evidence_path.display().to_string(),
            "--filter-report".into(),
            report_path.display().to_string(),
        ];
        assert!(crate::evaluation::main(args).await.is_err());
    }
}
