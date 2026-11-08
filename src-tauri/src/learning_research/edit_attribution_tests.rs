use super::{LearningArmV2, LearningConsent, LearningEpisodeV2, LearningScopeV2};
use crate::dictionary_learn::{
    ControlledAttributionResult, ControlledAttributionSample, ObserveLimits,
};
use crate::edit_attribution::{
    AttributionInput, ByteRange, DeliveryEvidence, DeliverySource, EditSettlement, EditSourceTrust,
    Evidence, ObservedEdit, ReferenceSource, ReferenceText, StageEvidence, StageId, StageText,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// All source evidence and observer inputs for an origin are fixed before an
/// arm is opened. Expected verdicts and write permissions deliberately live in
/// the individual assertions, never in this pinned evidence.
#[derive(Clone, PartialEq, Eq)]
struct PinnedAttributionFixture {
    evidence: AttributionInput,
    scope: LearningScopeV2,
    ordered_samples: Vec<ControlledAttributionSample>,
    limits: ObserveLimits,
}

/// Opaque proof that the in-memory suite owner checked an exact fixture match.
/// It borrows the canonical fixture so callers cannot pair the token with a
/// replacement scope, evidence record, observer trace, or limits.
pub(crate) struct VerifiedAttributionOrigin<'a> {
    fixture: &'a PinnedAttributionFixture,
}

impl VerifiedAttributionOrigin<'_> {
    pub(crate) fn origin_id(&self) -> &str {
        &self.fixture.evidence.origin_id
    }

    pub(crate) fn evidence(&self) -> &AttributionInput {
        &self.fixture.evidence
    }

    pub(crate) fn scope(&self) -> &LearningScopeV2 {
        &self.fixture.scope
    }

    pub(crate) fn ordered_samples(&self) -> &[ControlledAttributionSample] {
        &self.fixture.ordered_samples
    }

    pub(crate) fn limits(&self) -> ObserveLimits {
        self.fixture.limits
    }
}

/// Owns the immutable in-memory origin manifest across all arm opens and
/// reopens in one Rust test process. No alternate manifest is accepted by the
/// execution or reopen helpers.
struct AttributionSuiteOwner {
    origins: BTreeMap<String, PinnedAttributionFixture>,
}

impl AttributionSuiteOwner {
    fn pin(fixtures: impl IntoIterator<Item = PinnedAttributionFixture>) -> Self {
        let mut origins = BTreeMap::new();
        for fixture in fixtures {
            assert!(!fixture.evidence.origin_id.is_empty());
            assert_eq!(
                fixture.evidence.origin_id,
                fixture.evidence.reference.origin_id
            );
            assert!(origins
                .insert(fixture.evidence.origin_id.clone(), fixture)
                .is_none());
        }
        Self { origins }
    }

    fn verify<'a>(
        &'a self,
        origin_id: &str,
        submitted: &PinnedAttributionFixture,
    ) -> Result<VerifiedAttributionOrigin<'a>, &'static str> {
        let Some(fixture) = self.origins.get(origin_id) else {
            return Err("origin_not_pinned");
        };
        if submitted.evidence.origin_id != origin_id || fixture != submitted {
            return Err("origin_evidence_conflict");
        }
        Ok(VerifiedAttributionOrigin { fixture })
    }

    fn open_arm(
        &self,
        origin_id: &str,
        submitted: &PinnedAttributionFixture,
        base_root: &Path,
        episode: &LearningEpisodeV2,
    ) -> anyhow::Result<LearningArmV2> {
        let verified = self
            .verify(origin_id, submitted)
            .map_err(anyhow::Error::msg)?;
        if verified.scope() != &episode.scope {
            anyhow::bail!("origin_scope_mismatch");
        }
        LearningArmV2::open(base_root, episode)
    }

    fn record(
        &self,
        origin_id: &str,
        submitted: &PinnedAttributionFixture,
        arm: &mut LearningArmV2,
        consent: &super::LearningConsent,
        operation_id: &str,
    ) -> anyhow::Result<ControlledAttributionResult> {
        let verified = self
            .verify(origin_id, submitted)
            .map_err(anyhow::Error::msg)?;
        crate::dictionary_learn::record_controlled_fixture_attribution_trajectory(
            &arm.root,
            &mut arm.settings,
            consent,
            &verified,
            operation_id,
        )
    }
}

/// Count-only output for the selected synthetic origin set. It deliberately
/// contains no transcript text or origin identifiers.
#[derive(Debug, PartialEq, Eq)]
struct AttributionCoverageSnapshot {
    declared_origins: usize,
    attempted_origins: usize,
    judged_origins: usize,
    judged_receipts: usize,
    terminal_only_origins: usize,
    unrun_origins: usize,
    unknown_by_reason: Vec<(crate::edit_attribution::UnknownReason, usize)>,
    unknown_without_reason: usize,
    terminal_pre_judge_errors: Vec<(&'static str, usize)>,
    terminal_adapter_errors: usize,
    same_origin_replays: usize,
    receipt_learning_write_attempts: usize,
    candidate_guard_counts: Vec<(crate::edit_attribution::CandidateAssessment, bool, usize)>,
}

struct AttributionCoverageSummary<'a> {
    suite: &'a AttributionSuiteOwner,
    attempted_origins: BTreeSet<String>,
    judged_origins: BTreeSet<String>,
    judged_receipts: usize,
    unknown_by_reason: Vec<(crate::edit_attribution::UnknownReason, usize)>,
    unknown_without_reason: usize,
    terminal_pre_judge_errors: Vec<(&'static str, usize)>,
    terminal_adapter_errors: usize,
    same_origin_replays: usize,
    receipt_learning_write_attempts: usize,
    candidate_guard_counts: Vec<(crate::edit_attribution::CandidateAssessment, bool, usize)>,
}

impl<'a> AttributionCoverageSummary<'a> {
    fn from_suite(suite: &'a AttributionSuiteOwner) -> Self {
        Self {
            suite,
            attempted_origins: BTreeSet::new(),
            judged_origins: BTreeSet::new(),
            judged_receipts: 0,
            unknown_by_reason: Vec::new(),
            unknown_without_reason: 0,
            terminal_pre_judge_errors: Vec::new(),
            terminal_adapter_errors: 0,
            same_origin_replays: 0,
            receipt_learning_write_attempts: 0,
            candidate_guard_counts: Vec::new(),
        }
    }

    fn run_fixture(
        &mut self,
        origin_id: &str,
        fixture: &PinnedAttributionFixture,
        database_root: &Path,
        settings: &mut crate::store::Settings,
        consent: &LearningConsent,
        operation_id: &str,
    ) -> anyhow::Result<ControlledAttributionResult> {
        if !self.suite.origins.contains_key(origin_id) {
            anyhow::bail!("origin_not_selected");
        }
        self.attempted_origins.insert(origin_id.to_owned());
        let verified = match self.suite.verify(origin_id, fixture) {
            Ok(verified) => verified,
            Err(error) => {
                self.record_pre_judge_error(origin_id, error);
                return Err(anyhow::Error::msg(error));
            }
        };
        let result = match crate::dictionary_learn::record_controlled_fixture_attribution_trajectory(
            database_root,
            settings,
            consent,
            &verified,
            operation_id,
        ) {
            Ok(result) => result,
            Err(error) => {
                self.record_adapter_error(origin_id);
                return Err(error);
            }
        };
        self.record_result(origin_id, &result);
        Ok(result)
    }

    fn record_result(&mut self, origin_id: &str, result: &ControlledAttributionResult) {
        assert!(self.suite.origins.contains_key(origin_id));
        self.attempted_origins.insert(origin_id.to_owned());
        self.judged_receipts = self.judged_receipts.saturating_add(1);
        if result.candidate_writer_called {
            self.receipt_learning_write_attempts =
                self.receipt_learning_write_attempts.saturating_add(1);
        }
        if !self.judged_origins.insert(origin_id.to_owned()) {
            self.same_origin_replays = self.same_origin_replays.saturating_add(1);
            return;
        }

        if result.judgment.verdict == crate::edit_attribution::Verdict::Unknown {
            if let Some(reason) = result.judgment.reason {
                let mut counted = false;
                for entry in &mut self.unknown_by_reason {
                    if entry.0 == reason {
                        entry.1 = entry.1.saturating_add(1);
                        counted = true;
                        break;
                    }
                }
                if !counted {
                    self.unknown_by_reason.push((reason, 1));
                }
            } else {
                self.unknown_without_reason = self.unknown_without_reason.saturating_add(1);
            }
        }

        let candidate = result.judgment.cleanup_candidate;
        let mut counted = false;
        for entry in &mut self.candidate_guard_counts {
            if entry.0 == candidate.assessment && entry.1 == candidate.delivered {
                entry.2 = entry.2.saturating_add(1);
                counted = true;
                break;
            }
        }
        if !counted {
            self.candidate_guard_counts
                .push((candidate.assessment, candidate.delivered, 1));
        }
    }

    fn record_adapter_error(&mut self, origin_id: &str) {
        assert!(self.suite.origins.contains_key(origin_id));
        self.attempted_origins.insert(origin_id.to_owned());
        self.terminal_adapter_errors = self.terminal_adapter_errors.saturating_add(1);
    }

    fn record_pre_judge_error(&mut self, origin_id: &str, error: &'static str) {
        assert!(self.suite.origins.contains_key(origin_id));
        self.attempted_origins.insert(origin_id.to_owned());
        let mut counted = false;
        for entry in &mut self.terminal_pre_judge_errors {
            if entry.0 == error {
                entry.1 = entry.1.saturating_add(1);
                counted = true;
                break;
            }
        }
        if !counted {
            self.terminal_pre_judge_errors.push((error, 1));
        }
    }

    fn snapshot(&self) -> AttributionCoverageSnapshot {
        AttributionCoverageSnapshot {
            declared_origins: self.suite.origins.len(),
            attempted_origins: self.attempted_origins.len(),
            judged_origins: self.judged_origins.len(),
            judged_receipts: self.judged_receipts,
            terminal_only_origins: self
                .attempted_origins
                .len()
                .saturating_sub(self.judged_origins.len()),
            unrun_origins: self
                .suite
                .origins
                .len()
                .saturating_sub(self.attempted_origins.len()),
            unknown_by_reason: self.unknown_by_reason.clone(),
            unknown_without_reason: self.unknown_without_reason,
            terminal_pre_judge_errors: self.terminal_pre_judge_errors.clone(),
            terminal_adapter_errors: self.terminal_adapter_errors,
            same_origin_replays: self.same_origin_replays,
            receipt_learning_write_attempts: self.receipt_learning_write_attempts,
            candidate_guard_counts: self.candidate_guard_counts.clone(),
        }
    }
}

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock must be after the epoch")
            .as_nanos();
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "voiceflow-edit-attribution-test-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).expect("create isolated test root");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .expect("make isolated test root private");
        }
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, PartialEq)]
struct LearningSqliteSnapshot {
    selected_arm_state: Vec<u8>,
    pairs: Vec<crate::store::LearnPairRecord>,
    observations: Vec<(String, String)>,
    learned_term_usage: Vec<(String, u64, String)>,
    general_usage: Vec<(String, i64, i64, f64)>,
    style_drafts: i64,
    settings_bytes: Vec<u8>,
}

fn learning_snapshot(
    arm: &LearningArmV2,
    episode: &LearningEpisodeV2,
) -> anyhow::Result<LearningSqliteSnapshot> {
    use rusqlite::{Connection, OpenFlags};

    let mut pairs = crate::store::list_learn_pairs(&arm.root)?;
    pairs.sort_by(|left, right| left.pair_key.cmp(&right.pair_key));
    let database = Connection::open_with_flags(
        arm.root.join("history.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let observations = {
        let mut statement = database.prepare(
            "SELECT observation_key, source_kind FROM learn_observations ORDER BY observation_key",
        )?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<(String, String)>, _>>()?;
        rows
    };
    let learned_term_usage = {
        let mut statement = database.prepare(
            "SELECT word, replacement_runs, last_replaced_at FROM learned_term_usage ORDER BY word",
        )?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<Vec<(String, u64, String)>, _>>()?;
        rows
    };
    let general_usage = {
        let mut statement = database.prepare(
            "SELECT day, asr_requests, llm_requests, audio_seconds FROM usage ORDER BY day",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<Result<Vec<(String, i64, i64, f64)>, _>>()?;
        rows
    };
    let style_drafts = database.query_row("SELECT COUNT(*) FROM style_drafts", [], |row| {
        row.get::<_, i64>(0)
    })?;
    let selected_arm_state = serde_json::to_vec(&arm.selected_state(episode)?)?;
    let settings_bytes = std::fs::read(arm.root.join("settings.json"))?;
    Ok(LearningSqliteSnapshot {
        selected_arm_state,
        pairs,
        observations,
        learned_term_usage,
        general_usage,
        style_drafts,
        settings_bytes,
    })
}

#[derive(Clone, Copy)]
enum Scenario {
    AsrSubstitution,
    GuardFallbackAsrCorrection,
    GuardFallbackUnchanged,
    LocalPreparationRegression,
    CleanupRegression,
    MissingReference,
    AsrOmission,
    UserAddition,
    ContentEdit,
    IntentConflict,
}

#[derive(Clone, Copy)]
enum DeployScenario {
    UserAddition,
    AsrOmission,
    CleanupRegression,
    MissingReference,
}

fn fixture_episode() -> LearningEpisodeV2 {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/learning-v2-text.json"))
            .expect("load the existing synthetic V2 fixture");
    serde_json::from_value(fixture["episode"].clone()).expect("parse learning episode")
}

fn consent_for(episode: &LearningEpisodeV2, fixture: &PinnedAttributionFixture) -> LearningConsent {
    LearningConsent {
        selected_state_id: episode.selected_state_id.clone(),
        baseline_run: episode.baseline_run.clone(),
        edit_fixture_id: episode.edit_fixture_id.clone(),
        purpose: episode.purpose,
        mapping_id: fixture.scope.mapping_id.clone(),
        bundle_id: fixture.scope.native_bundle.clone(),
        observer_enabled: true,
        post_insert_field: fixture.evidence.delivery.value.inserted_text.clone(),
        settled_field: fixture.evidence.edit.value.stable_text.clone(),
        terms: episode.terms.clone(),
    }
}

fn controlled_samples(final_text: &str) -> Vec<ControlledAttributionSample> {
    vec![
        ControlledAttributionSample::FieldText(final_text.to_owned()),
        ControlledAttributionSample::FieldText(final_text.to_owned()),
    ]
}

fn fast_observe_limits() -> ObserveLimits {
    ObserveLimits {
        initial: std::time::Duration::from_millis(1),
        interval: std::time::Duration::from_millis(1),
        idle_grace: std::time::Duration::from_millis(1),
        max_total: std::time::Duration::from_millis(3),
    }
}

fn stage_evidence(text: &str) -> StageEvidence {
    StageEvidence {
        text: StageText::Present(text.to_owned()),
        rule_changes: Vec::new(),
    }
}

fn append_beta_suffix(fixture: &mut PinnedAttributionFixture) {
    fn replace(text: &mut String) {
        *text = match text.as_str() {
            "we saw VoiceFlo today" => "we saw VoiceFlo-beta today".to_owned(),
            "we saw VoiceFlow today" => "we saw VoiceFlow-beta today".to_owned(),
            "we saw VoiceFlo today after lunch" => {
                "we saw VoiceFlo-beta today after lunch".to_owned()
            }
            "we saw VoiceFlow today after lunch" => {
                "we saw VoiceFlow-beta today after lunch".to_owned()
            }
            _ => text.clone(),
        };
    }

    if let ReferenceText::Present { text, .. } = &mut fixture.evidence.reference.value {
        replace(text);
    }
    for stage in [
        &mut fixture.evidence.asr.value,
        &mut fixture.evidence.local_preparation.value,
        &mut fixture.evidence.cleanup_candidate.value,
    ] {
        if let StageText::Present(text) = &mut stage.text {
            replace(text);
        }
    }
    replace(&mut fixture.evidence.delivery.value.inserted_text);
    replace(&mut fixture.evidence.delivery.value.first_snapshot);
    fixture.evidence.delivery.value.inserted_range.end =
        fixture.evidence.delivery.value.first_snapshot.len();
    replace(&mut fixture.evidence.edit.value.stable_text);
    for sample in &mut fixture.ordered_samples {
        if let ControlledAttributionSample::FieldText(text) = sample {
            replace(text);
        }
    }
}

fn evidence<T>(origin_id: &str, value: T) -> Evidence<T> {
    Evidence {
        origin_id: origin_id.to_owned(),
        value,
    }
}

fn voiceflo_fixture(
    origin_id: &str,
    scope: LearningScopeV2,
    scenario: Scenario,
) -> PinnedAttributionFixture {
    let reference_text = "we saw VoiceFlow today";
    let bad_text = "we saw VoiceFlo today";
    let (reference, asr_text, preparation_text, cleanup_text, delivery_source, stable_text, intent) =
        match scenario {
            Scenario::AsrSubstitution => (
                ReferenceText::Present {
                    text: reference_text.to_owned(),
                    source: ReferenceSource::Synthetic,
                },
                bad_text,
                bad_text,
                bad_text,
                DeliverySource::PassThrough(StageId::AsrOriginal),
                reference_text,
                None,
            ),
            Scenario::GuardFallbackAsrCorrection => (
                ReferenceText::Present {
                    text: "we saw VoiceFlow today after lunch".to_owned(),
                    source: ReferenceSource::Synthetic,
                },
                "we saw VoiceFlo today after lunch",
                "we saw VoiceFlo today after lunch",
                "we saw VoiceFlo today",
                DeliverySource::FallbackTo(StageId::AsrOriginal),
                "we saw VoiceFlow today after lunch",
                None,
            ),
            Scenario::GuardFallbackUnchanged => (
                ReferenceText::Present {
                    text: reference_text.to_owned(),
                    source: ReferenceSource::Synthetic,
                },
                reference_text,
                reference_text,
                bad_text,
                DeliverySource::FallbackTo(StageId::LocalPreparation),
                reference_text,
                None,
            ),
            Scenario::LocalPreparationRegression => (
                ReferenceText::Present {
                    text: reference_text.to_owned(),
                    source: ReferenceSource::Synthetic,
                },
                reference_text,
                bad_text,
                bad_text,
                DeliverySource::PassThrough(StageId::LocalPreparation),
                reference_text,
                None,
            ),
            Scenario::CleanupRegression => (
                ReferenceText::Present {
                    text: reference_text.to_owned(),
                    source: ReferenceSource::Synthetic,
                },
                reference_text,
                reference_text,
                bad_text,
                DeliverySource::CleanupCandidate,
                reference_text,
                None,
            ),
            Scenario::MissingReference => (
                ReferenceText::Missing,
                bad_text,
                bad_text,
                bad_text,
                DeliverySource::PassThrough(StageId::AsrOriginal),
                reference_text,
                None,
            ),
            Scenario::AsrOmission => (
                ReferenceText::Present {
                    text: reference_text.to_owned(),
                    source: ReferenceSource::Synthetic,
                },
                "we saw today",
                "we saw today",
                "we saw today",
                DeliverySource::PassThrough(StageId::AsrOriginal),
                reference_text,
                None,
            ),
            Scenario::UserAddition => (
                ReferenceText::Present {
                    text: "we saw today".to_owned(),
                    source: ReferenceSource::Synthetic,
                },
                "we saw today",
                "we saw today",
                "we saw today",
                DeliverySource::PassThrough(StageId::AsrOriginal),
                reference_text,
                Some(crate::edit_attribution::EditIntent::LaterAddition),
            ),
            Scenario::ContentEdit => (
                ReferenceText::Present {
                    text: reference_text.to_owned(),
                    source: ReferenceSource::Synthetic,
                },
                reference_text,
                reference_text,
                reference_text,
                DeliverySource::PassThrough(StageId::AsrOriginal),
                bad_text,
                None,
            ),
            Scenario::IntentConflict => (
                ReferenceText::Present {
                    text: reference_text.to_owned(),
                    source: ReferenceSource::Synthetic,
                },
                bad_text,
                bad_text,
                bad_text,
                DeliverySource::PassThrough(StageId::AsrOriginal),
                reference_text,
                Some(crate::edit_attribution::EditIntent::LaterAddition),
            ),
        };
    let delivered_text = match delivery_source {
        DeliverySource::PassThrough(StageId::AsrOriginal)
        | DeliverySource::FallbackTo(StageId::AsrOriginal) => asr_text,
        DeliverySource::PassThrough(StageId::LocalPreparation)
        | DeliverySource::FallbackTo(StageId::LocalPreparation) => preparation_text,
        DeliverySource::CleanupCandidate => cleanup_text,
        _ => unreachable!("fixture only uses declared delivery stages"),
    };
    let scope_evidence = crate::edit_attribution::LearningScope(scope.mapping_id.clone());
    let delivered_range = ByteRange {
        start: 0,
        end: delivered_text.len(),
    };
    let evidence = AttributionInput {
        origin_id: origin_id.to_owned(),
        reference: evidence(origin_id, reference),
        asr: evidence(origin_id, stage_evidence(asr_text)),
        local_preparation: evidence(origin_id, stage_evidence(preparation_text)),
        cleanup_candidate: evidence(origin_id, stage_evidence(cleanup_text)),
        delivery: evidence(
            origin_id,
            DeliveryEvidence {
                source: delivery_source,
                inserted_text: delivered_text.to_owned(),
                field_id: "synthetic-editor-field".to_owned(),
                first_snapshot: delivered_text.to_owned(),
                inserted_range: delivered_range,
                guard_output: None,
            },
        ),
        edit: evidence(
            origin_id,
            ObservedEdit {
                field_id: "synthetic-editor-field".to_owned(),
                stable_text: stable_text.to_owned(),
                settlement: EditSettlement::Stable,
                source_trust: EditSourceTrust::SameFieldObserver,
            },
        ),
        intent: intent.map(|value| evidence(origin_id, value)),
        learning_scope: Some(evidence(origin_id, scope_evidence)),
    };
    let ordered_samples = controlled_samples(stable_text);
    let mut limits = fast_observe_limits();
    if matches!(scenario, Scenario::GuardFallbackUnchanged) {
        limits.initial = std::time::Duration::from_millis(2);
    }
    PinnedAttributionFixture {
        evidence,
        scope,
        ordered_samples,
        limits,
    }
}

fn deploy_after_lunch_fixture(
    origin_id: &str,
    scope: LearningScopeV2,
    scenario: DeployScenario,
) -> PinnedAttributionFixture {
    let delivered_text = "Deploy today";
    let final_text = "Deploy today after lunch";
    let (reference, asr, preparation, cleanup, source, intent) = match scenario {
        DeployScenario::UserAddition => (
            ReferenceText::Present {
                text: delivered_text.to_owned(),
                source: ReferenceSource::Synthetic,
            },
            delivered_text,
            delivered_text,
            delivered_text,
            DeliverySource::PassThrough(StageId::AsrOriginal),
            Some(crate::edit_attribution::EditIntent::LaterAddition),
        ),
        DeployScenario::AsrOmission => (
            ReferenceText::Present {
                text: final_text.to_owned(),
                source: ReferenceSource::Synthetic,
            },
            delivered_text,
            delivered_text,
            delivered_text,
            DeliverySource::PassThrough(StageId::AsrOriginal),
            None,
        ),
        DeployScenario::CleanupRegression => (
            ReferenceText::Present {
                text: final_text.to_owned(),
                source: ReferenceSource::Synthetic,
            },
            final_text,
            final_text,
            delivered_text,
            DeliverySource::CleanupCandidate,
            None,
        ),
        DeployScenario::MissingReference => (
            ReferenceText::Missing,
            delivered_text,
            delivered_text,
            delivered_text,
            DeliverySource::PassThrough(StageId::AsrOriginal),
            None,
        ),
    };
    let evidence = AttributionInput {
        origin_id: origin_id.to_owned(),
        reference: evidence(origin_id, reference),
        asr: evidence(origin_id, stage_evidence(asr)),
        local_preparation: evidence(origin_id, stage_evidence(preparation)),
        cleanup_candidate: evidence(origin_id, stage_evidence(cleanup)),
        delivery: evidence(
            origin_id,
            DeliveryEvidence {
                source,
                inserted_text: delivered_text.to_owned(),
                field_id: "synthetic-editor-field".to_owned(),
                first_snapshot: delivered_text.to_owned(),
                inserted_range: ByteRange {
                    start: 0,
                    end: delivered_text.len(),
                },
                guard_output: None,
            },
        ),
        edit: evidence(
            origin_id,
            ObservedEdit {
                field_id: "synthetic-editor-field".to_owned(),
                stable_text: final_text.to_owned(),
                settlement: EditSettlement::Stable,
                source_trust: EditSourceTrust::SameFieldObserver,
            },
        ),
        intent: intent.map(|value| evidence(origin_id, value)),
        learning_scope: Some(evidence(
            origin_id,
            crate::edit_attribution::LearningScope(scope.mapping_id.clone()),
        )),
    };
    PinnedAttributionFixture {
        evidence,
        scope,
        ordered_samples: controlled_samples(final_text),
        limits: fast_observe_limits(),
    }
}

fn assert_judgment(
    result: &ControlledAttributionResult,
    fixture: &PinnedAttributionFixture,
    verdict: crate::edit_attribution::Verdict,
    reason: Option<crate::edit_attribution::UnknownReason>,
    first_difference: Option<crate::edit_attribution::DifferenceStage>,
    candidate_assessment: crate::edit_attribution::CandidateAssessment,
    delivered: bool,
) {
    assert_eq!(
        result.judgment,
        crate::edit_attribution::judge(&fixture.evidence)
    );
    assert_eq!(result.judgment.verdict, verdict);
    assert_eq!(result.judgment.reason, reason);
    assert_eq!(result.judgment.first_difference_stage, first_difference);
    assert_eq!(
        result.judgment.cleanup_candidate.assessment,
        candidate_assessment
    );
    assert_eq!(result.judgment.cleanup_candidate.delivered, delivered);
}

#[test]
fn attribution_gate_allows_only_a_proven_asr_substitution_to_write() {
    use crate::edit_attribution::{CandidateAssessment, DifferenceStage, UnknownReason, Verdict};

    let root = TestRoot::new();
    let episode = fixture_episode();
    let scope = episode.scope.clone();
    let cases = [
        (
            voiceflo_fixture("asr-substitution", scope.clone(), Scenario::AsrSubstitution),
            Verdict::AsrSubstitution,
            None,
            Some(DifferenceStage::AsrOriginal),
            CandidateAssessment::NoRegression,
            false,
            true,
        ),
        (
            voiceflo_fixture(
                "local-preparation-regression",
                scope.clone(),
                Scenario::LocalPreparationRegression,
            ),
            Verdict::LocalPrepRegression,
            None,
            Some(DifferenceStage::LocalPreparation),
            CandidateAssessment::NoRegression,
            false,
            false,
        ),
        (
            voiceflo_fixture(
                "cleanup-regression",
                scope.clone(),
                Scenario::CleanupRegression,
            ),
            Verdict::CleanupRegression,
            None,
            Some(DifferenceStage::CleanupCandidate),
            CandidateAssessment::Regression,
            true,
            false,
        ),
        (
            voiceflo_fixture(
                "missing-reference",
                scope.clone(),
                Scenario::MissingReference,
            ),
            Verdict::Unknown,
            Some(UnknownReason::ReferenceMissing),
            None,
            CandidateAssessment::NotAssessable,
            false,
            false,
        ),
        (
            voiceflo_fixture(
                "guard-fallback-asr",
                scope.clone(),
                Scenario::GuardFallbackAsrCorrection,
            ),
            Verdict::AsrSubstitution,
            None,
            Some(DifferenceStage::AsrOriginal),
            CandidateAssessment::Regression,
            false,
            true,
        ),
        (
            voiceflo_fixture(
                "guard-fallback-unchanged",
                scope.clone(),
                Scenario::GuardFallbackUnchanged,
            ),
            Verdict::Unchanged,
            None,
            None,
            CandidateAssessment::Regression,
            false,
            false,
        ),
        (
            voiceflo_fixture("asr-omission", scope.clone(), Scenario::AsrOmission),
            Verdict::AsrOmission,
            None,
            Some(DifferenceStage::AsrOriginal),
            CandidateAssessment::NoRegression,
            false,
            false,
        ),
        (
            voiceflo_fixture("user-addition", scope.clone(), Scenario::UserAddition),
            Verdict::UserAddition,
            None,
            None,
            CandidateAssessment::NoRegression,
            false,
            false,
        ),
        (
            voiceflo_fixture("content-edit", scope.clone(), Scenario::ContentEdit),
            Verdict::ContentEdit,
            None,
            None,
            CandidateAssessment::NoRegression,
            false,
            false,
        ),
        (
            voiceflo_fixture("intent-conflict", scope.clone(), Scenario::IntentConflict),
            Verdict::Unknown,
            Some(UnknownReason::IntentConflict),
            Some(DifferenceStage::AsrOriginal),
            CandidateAssessment::NoRegression,
            false,
            false,
        ),
    ];
    let suite = AttributionSuiteOwner::pin(cases.iter().map(|(fixture, ..)| fixture.clone()));
    let mut arm = suite
        .open_arm("asr-substitution", &cases[0].0, root.path(), &episode)
        .expect("open the isolated learning arm through its pinned suite owner");
    let before = learning_snapshot(&arm, &episode).expect("read initial SQLite state");

    for (fixture, verdict, reason, difference, candidate, delivered, _write_allowed) in
        cases.iter().filter(|case| !case.6)
    {
        let consent = consent_for(&episode, fixture);
        let result = suite
            .record(
                &fixture.evidence.origin_id,
                fixture,
                &mut arm,
                &consent,
                &episode.operations[0].operation_id,
            )
            .expect("run the controlled trajectory through the real observer");
        assert_judgment(
            &result,
            fixture,
            *verdict,
            *reason,
            *difference,
            *candidate,
            *delivered,
        );
        assert!(result.settled);
        assert_eq!(result.states_consumed, fixture.ordered_samples.len());
        assert!(result.samples_observed >= result.states_consumed);
        if fixture.evidence.origin_id == "guard-fallback-unchanged" {
            assert_eq!(
                fixture.ordered_samples,
                vec![
                    ControlledAttributionSample::FieldText("we saw VoiceFlow today".to_owned()),
                    ControlledAttributionSample::FieldText("we saw VoiceFlow today".to_owned()),
                ]
            );
            assert_eq!(
                fixture.limits.max_total,
                std::time::Duration::from_millis(3)
            );
            assert_eq!(result.samples_observed, 2);
        }
        assert!(!result.candidate_writer_called);
        assert_eq!(
            learning_snapshot(&arm, &episode).expect("read isolated SQLite state"),
            before
        );
    }

    let fixture = &cases[0].0;
    let consent = consent_for(&episode, fixture);
    let result = suite
        .record(
            &fixture.evidence.origin_id,
            fixture,
            &mut arm,
            &consent,
            &episode.operations[0].operation_id,
        )
        .expect("record the eligible ASR correction");
    assert_judgment(
        &result,
        fixture,
        Verdict::AsrSubstitution,
        None,
        Some(DifferenceStage::AsrOriginal),
        CandidateAssessment::NoRegression,
        false,
    );
    assert!(result.settled);
    assert_eq!(result.states_consumed, fixture.ordered_samples.len());
    assert!(result.candidate_writer_called);
    let key = crate::dictionary_learn::pair_key("VoiceFlo", "VoiceFlow");
    let row = crate::store::get_learn_pair(&arm.root, &key)
        .expect("read recorded pair")
        .expect("eligible candidate is stored");
    assert_eq!(row.hits, 1);
    assert!(!row.is_live_promoted());
    assert_eq!(
        row.family.as_deref(),
        Some(crate::context::family_id(scope.family))
    );
    assert_eq!(row.mapping_id.as_deref(), Some(scope.mapping_id.as_str()));
    assert_eq!(
        row.native_bundle.as_deref(),
        Some(scope.native_bundle.as_str())
    );
    let after = learning_snapshot(&arm, &episode).expect("read first eligible write state");
    assert_eq!(after.settings_bytes, before.settings_bytes);
    assert_eq!(after.pairs.len(), 1);
    assert_eq!(
        after
            .observations
            .iter()
            .filter(|(_, kind)| kind == "research_fixture_edit")
            .count(),
        1
    );

    let fallback_fixture = &cases[4].0;
    let fallback_consent = consent_for(&episode, fallback_fixture);
    let fallback = suite
        .record(
            &fallback_fixture.evidence.origin_id,
            fallback_fixture,
            &mut arm,
            &fallback_consent,
            &episode.operations[1].operation_id,
        )
        .expect("a bad candidate rejected by the guard is attributed on the ASR fallback path");
    assert_judgment(
        &fallback,
        fallback_fixture,
        Verdict::AsrSubstitution,
        None,
        Some(DifferenceStage::AsrOriginal),
        CandidateAssessment::Regression,
        false,
    );
    assert!(fallback.candidate_writer_called);
    assert_eq!(fallback.candidate_write, "candidate_written");
    assert_eq!(
        crate::store::get_learn_pair(&arm.root, &key)
            .expect("read fallback candidate")
            .expect("fallback correction adds an independent observation")
            .hits,
        2
    );
}

#[test]
fn grants_scope_and_observer_mismatches_leave_the_isolated_arm_unchanged() {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Gate {
        PairPermission,
        Scope,
        Observer,
        LocalLearningSetting,
        SettledField,
    }

    let root = TestRoot::new();
    let episode = fixture_episode();
    let fixture = voiceflo_fixture(
        "authorization-mismatch",
        episode.scope.clone(),
        Scenario::AsrSubstitution,
    );
    let suite = AttributionSuiteOwner::pin([fixture.clone()]);
    let gates = [
        Gate::PairPermission,
        Gate::Scope,
        Gate::Observer,
        Gate::LocalLearningSetting,
        Gate::SettledField,
    ];

    for (index, configure_gate) in gates.into_iter().enumerate() {
        let mut case_episode = episode.clone();
        case_episode.arm_state_id = format!("70000000-0000-4000-8000-{index:012x}");
        let mut arm = suite
            .open_arm(
                &fixture.evidence.origin_id,
                &fixture,
                root.path(),
                &case_episode,
            )
            .expect("open an independent gated test arm");
        let mut consent = consent_for(&case_episode, &fixture);
        match configure_gate {
            Gate::PairPermission => {
                consent.terms[0].before = "VoiceFlow".to_owned();
                consent.terms[0].after = "VoiceFlo".to_owned();
            }
            Gate::Scope => consent.mapping_id = "another-controlled-mapping".to_owned(),
            Gate::Observer => consent.observer_enabled = false,
            Gate::LocalLearningSetting => {
                arm.settings.local_edit_learning_enabled = false;
                crate::store::save_configuration(&arm.root, &arm.settings)
                    .expect("persist the disabled local learning grant in the isolated arm");
            }
            Gate::SettledField => {
                consent.settled_field = "we saw VoiceFlow next week".to_owned();
            }
        }
        let before = learning_snapshot(&arm, &case_episode).expect("read gate baseline");
        let result = suite
            .record(
                &fixture.evidence.origin_id,
                &fixture,
                &mut arm,
                &consent,
                &episode.operations[index % episode.operations.len()].operation_id,
            )
            .expect("authorization mismatch is a recorded denial, not a store error");
        assert!(!result.candidate_writer_called);
        if configure_gate == Gate::SettledField {
            assert!(result.settled);
            assert_eq!(
                result.judgment.verdict,
                crate::edit_attribution::Verdict::AsrSubstitution
            );
            assert_eq!(
                result.judgment.first_difference_stage,
                Some(crate::edit_attribution::DifferenceStage::AsrOriginal)
            );
        }
        assert_eq!(
            learning_snapshot(&arm, &case_episode).expect("read gate readback"),
            before
        );
        assert!(crate::store::list_learn_pairs(&arm.root)
            .expect("read all selected learning pairs")
            .is_empty());
    }
}

#[test]
fn canonical_untrusted_edit_source_is_not_upgraded_by_stable_observation() {
    use crate::edit_attribution::{EditSourceTrust, UnknownReason, Verdict};

    let root = TestRoot::new();
    let episode = fixture_episode();
    let mut fixture = voiceflo_fixture(
        "canonical-untrusted-edit-source",
        episode.scope.clone(),
        Scenario::AsrSubstitution,
    );
    fixture.evidence.edit.value.source_trust = EditSourceTrust::Untrusted;
    let suite = AttributionSuiteOwner::pin([fixture.clone()]);
    let mut arm = suite
        .open_arm(&fixture.evidence.origin_id, &fixture, root.path(), &episode)
        .expect("open isolated untrusted-source arm");
    let before = learning_snapshot(&arm, &episode).expect("read untrusted-source baseline");
    let consent = consent_for(&episode, &fixture);
    let result = suite
        .record(
            &fixture.evidence.origin_id,
            &fixture,
            &mut arm,
            &consent,
            &episode.operations[0].operation_id,
        )
        .expect("untrusted canonical edit still produces a diagnostic receipt");
    assert_eq!(result.observer_outcome, "single_token");
    assert!(result.settled);
    assert_eq!(result.states_consumed, fixture.ordered_samples.len());
    assert_eq!(result.judgment.verdict, Verdict::Unknown);
    assert_eq!(
        result.judgment.reason,
        Some(UnknownReason::EditSourceUntrusted)
    );
    assert!(!result.candidate_writer_called);
    assert_eq!(
        learning_snapshot(&arm, &episode).expect("read untrusted-source state"),
        before
    );
}

#[test]
fn attribution_no_write_gate_uses_stable_observation_without_opening_sqlite() {
    use crate::edit_attribution::{DifferenceStage, Verdict};

    let root = TestRoot::new();
    let episode = fixture_episode();
    let fixture = voiceflo_fixture(
        "stable-cleanup-regression-no-write",
        episode.scope.clone(),
        Scenario::CleanupRegression,
    );
    let suite = AttributionSuiteOwner::pin([fixture.clone()]);
    let verified = suite
        .verify(&fixture.evidence.origin_id, &fixture)
        .expect("pin the complete cleanup evidence before observation");
    let database_root = root.path().join("arm-that-must-not-open");
    let mut settings = crate::store::Settings::default();
    let consent = consent_for(&episode, &fixture);
    let result = crate::dictionary_learn::record_controlled_fixture_attribution_trajectory(
        &database_root,
        &mut settings,
        &consent,
        &verified,
        &episode.operations[0].operation_id,
    )
    .expect("stable but ineligible attribution returns its gate receipt");

    assert!(result.settled);
    assert_eq!(result.states_consumed, fixture.ordered_samples.len());
    assert_eq!(result.judgment.verdict, Verdict::CleanupRegression);
    assert_eq!(
        result.judgment.first_difference_stage,
        Some(DifferenceStage::CleanupCandidate)
    );
    assert!(!result.candidate_writer_called);
    assert_eq!(result.candidate_write, "attribution_not_asr_substitution");
    assert!(!database_root.exists());

    let mut mismatch_fixture = voiceflo_fixture(
        "observer-pair-differs-from-attribution-pair",
        episode.scope.clone(),
        Scenario::AsrSubstitution,
    );
    append_beta_suffix(&mut mismatch_fixture);
    let mismatch_suite = AttributionSuiteOwner::pin([mismatch_fixture.clone()]);
    let verified = mismatch_suite
        .verify(&mismatch_fixture.evidence.origin_id, &mismatch_fixture)
        .expect("pin observer and judge evidence for the same origin");
    let mismatch_database_root = root.path().join("pair-mismatch-must-not-open");
    let mut mismatch_consent = consent_for(&episode, &mismatch_fixture);
    mismatch_consent.terms[0].before = "VoiceFlo-beta".to_owned();
    mismatch_consent.terms[0].after = "VoiceFlow-beta".to_owned();
    let mut mismatch_settings = crate::store::Settings::default();
    let mismatch = crate::dictionary_learn::record_controlled_fixture_attribution_trajectory(
        &mismatch_database_root,
        &mut mismatch_settings,
        &mismatch_consent,
        &verified,
        &episode.operations[1].operation_id,
    )
    .expect("the pair mismatch is a recorded attribution denial");
    let delivered_text = mismatch_fixture
        .evidence
        .delivery
        .value
        .inserted_text
        .as_str();
    let settled_text = mismatch_fixture
        .ordered_samples
        .iter()
        .rev()
        .find_map(|sample| match sample {
            ControlledAttributionSample::FieldText(text) => Some(text.as_str()),
            ControlledAttributionSample::MissingRead | ControlledAttributionSample::FocusLost => {
                None
            }
        })
        .expect("the pinned observer trace ends with a field-text sample");
    let observer_pair =
        crate::dictionary_learn::dictionary_suggestions(delivered_text, settled_text);
    assert_eq!(observer_pair.len(), 1);
    assert_eq!(observer_pair[0].before_span, "VoiceFlo-beta");
    assert_eq!(observer_pair[0].after, "VoiceFlow-beta");
    assert_eq!(mismatch.observer_outcome, "single_token");
    assert!(mismatch.settled);
    assert_eq!(mismatch.judgment.verdict, Verdict::AsrSubstitution);
    let proposed_pair = mismatch
        .judgment
        .proposed_learning_pair
        .as_ref()
        .expect("ASR diagnosis still reports the judge's pair");
    assert_eq!(proposed_pair.observed, "VoiceFlo");
    assert_eq!(proposed_pair.correction, "VoiceFlow");
    assert_ne!(
        (
            observer_pair[0].before_span.as_str(),
            observer_pair[0].after.as_str()
        ),
        (
            proposed_pair.observed.as_str(),
            proposed_pair.correction.as_str()
        )
    );
    assert!(!mismatch.candidate_writer_called);
    assert_eq!(
        mismatch.candidate_write,
        "learning_grant_or_scope_not_admitted"
    );
    assert!(!mismatch_database_root.exists());
}

#[test]
fn attribution_coverage_summary_accounts_for_judgments_unknown_replay_and_unrun() {
    use crate::edit_attribution::{CandidateAssessment, UnknownReason, Verdict};

    let root = TestRoot::new();
    let episode = fixture_episode();
    let scope = episode.scope.clone();
    let fixtures = [
        voiceflo_fixture(
            "coverage-decided",
            scope.clone(),
            Scenario::CleanupRegression,
        ),
        voiceflo_fixture(
            "coverage-unknown",
            scope.clone(),
            Scenario::MissingReference,
        ),
        voiceflo_fixture(
            "coverage-terminal-only",
            scope.clone(),
            Scenario::AsrSubstitution,
        ),
        voiceflo_fixture(
            "coverage-adapter-error",
            scope.clone(),
            Scenario::AsrSubstitution,
        ),
        voiceflo_fixture("coverage-unrun", scope, Scenario::UserAddition),
    ];
    let suite = AttributionSuiteOwner::pin(fixtures.iter().cloned());
    let mut summary = AttributionCoverageSummary::from_suite(&suite);
    let database_root = root.path().join("coverage-arm-that-must-not-open");
    let mut settings = crate::store::Settings::default();
    let planned = [
        (
            0,
            0,
            Verdict::CleanupRegression,
            None,
            CandidateAssessment::Regression,
            true,
        ),
        (
            1,
            1,
            Verdict::Unknown,
            Some(UnknownReason::ReferenceMissing),
            CandidateAssessment::NotAssessable,
            false,
        ),
    ];
    let mut first_decided_judgment = None;
    for (fixture_index, operation_index, verdict, reason, candidate, delivered) in planned {
        let fixture = &fixtures[fixture_index];
        let consent = consent_for(&episode, fixture);
        let result = summary
            .run_fixture(
                &fixture.evidence.origin_id,
                fixture,
                &database_root,
                &mut settings,
                &consent,
                &episode.operations[operation_index].operation_id,
            )
            .expect("selected fixture reaches the real controlled adapter");
        assert_eq!(result.judgment.verdict, verdict);
        assert_eq!(result.judgment.reason, reason);
        assert_eq!(result.judgment.cleanup_candidate.assessment, candidate);
        assert_eq!(result.judgment.cleanup_candidate.delivered, delivered);
        assert!(result.settled);
        assert!(!result.candidate_writer_called);
        if fixture_index == 0 {
            first_decided_judgment = Some(result.judgment.clone());
        }
    }

    let decided = &fixtures[0];
    let decided_consent = consent_for(&episode, decided);
    let replay = summary
        .run_fixture(
            &decided.evidence.origin_id,
            decided,
            &database_root,
            &mut settings,
            &decided_consent,
            &episode.operations[2].operation_id,
        )
        .expect("same pinned origin can be replayed through the adapter");
    assert_eq!(
        replay.judgment,
        first_decided_judgment.expect("the planned decided fixture ran")
    );
    assert!(!replay.candidate_writer_called);

    let mut conflicting_decided = decided.clone();
    conflicting_decided.evidence.local_preparation.value.text =
        StageText::Present("we saw another name today".to_owned());
    let mut terminal_only_fixture = fixtures[2].clone();
    terminal_only_fixture.evidence.local_preparation.value.text =
        StageText::Present("we saw another name today".to_owned());
    let terminal_only_consent = consent_for(&episode, &fixtures[2]);
    for (origin_id, conflicting_fixture, consent, operation_id) in [
        (
            decided.evidence.origin_id.as_str(),
            &conflicting_decided,
            &decided_consent,
            episode.operations[0].operation_id.as_str(),
        ),
        (
            fixtures[2].evidence.origin_id.as_str(),
            &terminal_only_fixture,
            &terminal_only_consent,
            episode.operations[0].operation_id.as_str(),
        ),
    ] {
        let conflict = match summary.run_fixture(
            origin_id,
            conflicting_fixture,
            &database_root,
            &mut settings,
            consent,
            operation_id,
        ) {
            Err(error) => error,
            Ok(_) => panic!("same-origin changed evidence must be rejected before judgment"),
        };
        assert_eq!(conflict.to_string(), "origin_evidence_conflict");
    }

    let adapter_fixture = &fixtures[3];
    let adapter_consent = consent_for(&episode, adapter_fixture);
    let mut admitted_settings = super::initial_arm_settings(&episode);
    let invalid_database_root = root.path().join("database-parent-is-a-file");
    let blocking_file = b"synthetic path blocker";
    std::fs::write(&invalid_database_root, blocking_file)
        .expect("create isolated file that cannot contain SQLite children");
    let before_adapter_error = summary.snapshot();
    let adapter_error = match summary.run_fixture(
        &adapter_fixture.evidence.origin_id,
        adapter_fixture,
        &invalid_database_root,
        &mut admitted_settings,
        &adapter_consent,
        &episode.operations[2].operation_id,
    ) {
        Err(error) => error,
        Ok(_) => panic!("the isolated invalid database path must return an adapter error"),
    };
    assert!(!adapter_error.to_string().is_empty());
    let after_adapter_error = summary.snapshot();
    assert_eq!(
        after_adapter_error.judged_receipts,
        before_adapter_error.judged_receipts
    );
    assert_eq!(
        after_adapter_error.judged_origins,
        before_adapter_error.judged_origins
    );
    assert_eq!(
        after_adapter_error.receipt_learning_write_attempts,
        before_adapter_error.receipt_learning_write_attempts
    );
    assert_eq!(
        after_adapter_error.terminal_pre_judge_errors,
        before_adapter_error.terminal_pre_judge_errors
    );
    assert!(invalid_database_root.is_file());
    assert_eq!(
        std::fs::read(&invalid_database_root).expect("read isolated path blocker"),
        blocking_file
    );

    let snapshot = summary.snapshot();
    assert_eq!(snapshot.declared_origins, 5);
    assert_eq!(snapshot.attempted_origins, 4);
    assert_eq!(snapshot.judged_origins, 2);
    assert_eq!(snapshot.judged_receipts, 3);
    assert_eq!(snapshot.terminal_only_origins, 2);
    assert_eq!(snapshot.unrun_origins, 1);
    assert_eq!(
        snapshot.unknown_by_reason,
        vec![(UnknownReason::ReferenceMissing, 1)]
    );
    assert_eq!(snapshot.unknown_without_reason, 0);
    assert_eq!(
        snapshot.terminal_pre_judge_errors,
        vec![("origin_evidence_conflict", 2)]
    );
    assert_eq!(snapshot.terminal_adapter_errors, 1);
    assert_eq!(snapshot.same_origin_replays, 1);
    assert_eq!(snapshot.receipt_learning_write_attempts, 0);
    assert_eq!(
        snapshot.candidate_guard_counts,
        vec![
            (CandidateAssessment::Regression, true, 1),
            (CandidateAssessment::NotAssessable, false, 1),
        ]
    );
    assert!(!database_root.exists());
}

#[test]
fn same_insertion_evidence_matrix_is_observer_gated_without_sqlite_writes() {
    use crate::edit_attribution::{CandidateAssessment, DifferenceStage, UnknownReason, Verdict};

    let root = TestRoot::new();
    let episode = fixture_episode();
    let scope = episode.scope.clone();
    let cases = [
        (
            deploy_after_lunch_fixture(
                "deploy-user-addition",
                scope.clone(),
                DeployScenario::UserAddition,
            ),
            Verdict::UserAddition,
            None,
            None,
            CandidateAssessment::NoRegression,
            false,
        ),
        (
            deploy_after_lunch_fixture(
                "deploy-asr-omission",
                scope.clone(),
                DeployScenario::AsrOmission,
            ),
            Verdict::AsrOmission,
            None,
            Some(DifferenceStage::AsrOriginal),
            CandidateAssessment::NoRegression,
            false,
        ),
        (
            deploy_after_lunch_fixture(
                "deploy-cleanup-regression",
                scope.clone(),
                DeployScenario::CleanupRegression,
            ),
            Verdict::CleanupRegression,
            None,
            Some(DifferenceStage::CleanupCandidate),
            CandidateAssessment::Regression,
            true,
        ),
        (
            deploy_after_lunch_fixture(
                "deploy-missing-reference",
                scope,
                DeployScenario::MissingReference,
            ),
            Verdict::Unknown,
            Some(UnknownReason::ReferenceMissing),
            None,
            CandidateAssessment::NotAssessable,
            false,
        ),
    ];
    let suite = AttributionSuiteOwner::pin(cases.iter().map(|(fixture, ..)| fixture.clone()));
    let mut arm = suite
        .open_arm(
            &cases[0].0.evidence.origin_id,
            &cases[0].0,
            root.path(),
            &episode,
        )
        .expect("open the shared insertion matrix arm through its pinned owner");
    let before = learning_snapshot(&arm, &episode).expect("read insertion matrix baseline");

    for (fixture, verdict, reason, difference, candidate, delivered) in &cases {
        assert!(fixture.scope == cases[0].0.scope);
        assert_eq!(
            fixture.evidence.delivery.value.inserted_text,
            "Deploy today"
        );
        assert_eq!(
            fixture.evidence.delivery.value.first_snapshot,
            "Deploy today"
        );
        assert_eq!(
            fixture.evidence.edit.value.stable_text,
            "Deploy today after lunch"
        );
        assert_eq!(fixture.ordered_samples, cases[0].0.ordered_samples);
        let consent = consent_for(&episode, fixture);
        let result = suite
            .record(
                &fixture.evidence.origin_id,
                fixture,
                &mut arm,
                &consent,
                &episode.operations[0].operation_id,
            )
            .expect("the insertion matrix returns a controlled attribution receipt");
        assert_judgment(
            &result,
            fixture,
            *verdict,
            *reason,
            *difference,
            *candidate,
            *delivered,
        );
        assert!(result.settled);
        assert_eq!(result.states_consumed, fixture.ordered_samples.len());
        assert!(result.samples_observed >= result.states_consumed);
        assert!(!result.candidate_writer_called);
        assert_eq!(
            learning_snapshot(&arm, &episode).expect("read insertion matrix state"),
            before
        );
    }
}

#[test]
fn observer_requires_stable_same_field_reads_and_consumes_the_pinned_trace() {
    use crate::edit_attribution::{UnknownReason, Verdict};

    let root = TestRoot::new();
    let episode = fixture_episode();
    let scope = episode.scope.clone();
    let mut short_trace = voiceflo_fixture(
        "observer-short-trace",
        scope.clone(),
        Scenario::AsrSubstitution,
    );
    short_trace.ordered_samples.truncate(1);
    let mut missing_read = voiceflo_fixture(
        "observer-missing-read",
        scope.clone(),
        Scenario::AsrSubstitution,
    );
    missing_read.ordered_samples = vec![
        ControlledAttributionSample::FieldText("we saw VoiceFlow today".to_owned()),
        ControlledAttributionSample::MissingRead,
    ];
    let focus_lost = {
        let mut fixture = voiceflo_fixture(
            "observer-focus-lost",
            scope.clone(),
            Scenario::AsrSubstitution,
        );
        fixture.ordered_samples = vec![ControlledAttributionSample::FocusLost];
        fixture
    };
    let mut unconsumed_trace = voiceflo_fixture(
        "observer-unconsumed-trace",
        scope,
        Scenario::AsrSubstitution,
    );
    unconsumed_trace
        .ordered_samples
        .push(ControlledAttributionSample::FieldText(
            "we saw VoiceFlow today".to_owned(),
        ));
    let fixtures = [short_trace, missing_read, focus_lost, unconsumed_trace];
    let owner = AttributionSuiteOwner::pin(fixtures.iter().cloned());
    let mut arm = owner
        .open_arm(
            &fixtures[0].evidence.origin_id,
            &fixtures[0],
            root.path(),
            &episode,
        )
        .expect("open observer test arm");
    let before = learning_snapshot(&arm, &episode).expect("read initial SQLite state");

    let expected_consumption = [(1, 1), (2, 2), (1, 0), (2, 2)];
    let expected_reasons = [
        UnknownReason::EditUnsettled,
        UnknownReason::EditUnsettled,
        UnknownReason::EditSourceUntrusted,
        UnknownReason::EditUnsettled,
    ];
    for (index, fixture) in fixtures.iter().enumerate() {
        let consent = consent_for(&episode, fixture);
        let result = owner
            .record(
                &fixture.evidence.origin_id,
                fixture,
                &mut arm,
                &consent,
                &episode.operations[0].operation_id,
            )
            .expect("observer negative case produces an isolated receipt");
        assert!(!result.settled);
        assert_eq!(result.judgment.verdict, Verdict::Unknown);
        assert_eq!(result.judgment.reason, Some(expected_reasons[index]));
        assert_eq!(result.states_consumed, expected_consumption[index].0);
        assert_eq!(result.samples_observed, expected_consumption[index].1);
        assert!(!result.candidate_writer_called);
        assert_eq!(
            learning_snapshot(&arm, &episode).expect("read observer state"),
            before
        );
    }

    assert!(fixtures[2]
        .ordered_samples
        .iter()
        .any(|sample| matches!(sample, ControlledAttributionSample::FocusLost)));
    assert!(fixtures[1]
        .ordered_samples
        .iter()
        .any(|sample| matches!(sample, ControlledAttributionSample::MissingRead)));
    assert_eq!(fixtures[3].ordered_samples.len(), 3);
}

#[test]
fn stable_origins_dedupe_promote_apply_by_scope_and_survive_undo_reopen() {
    use crate::edit_attribution::{DifferenceStage, Verdict};

    let root = TestRoot::new();
    let episode = fixture_episode();
    let scope = episode.scope.clone();
    let fixtures = [
        voiceflo_fixture(
            "stable-origin-one",
            scope.clone(),
            Scenario::AsrSubstitution,
        ),
        voiceflo_fixture(
            "stable-origin-two",
            scope.clone(),
            Scenario::AsrSubstitution,
        ),
        voiceflo_fixture(
            "stable-origin-three",
            scope.clone(),
            Scenario::AsrSubstitution,
        ),
        voiceflo_fixture(
            "post-undo-independent-origin",
            scope.clone(),
            Scenario::AsrSubstitution,
        ),
    ];
    let suite = AttributionSuiteOwner::pin(fixtures.iter().cloned());
    let first = &fixtures[0];
    let mut arm = suite
        .open_arm(&first.evidence.origin_id, first, root.path(), &episode)
        .expect("open one isolated arm for all independent origins");
    let initial = learning_snapshot(&arm, &episode).expect("read initial SQLite state");

    let first_consent = consent_for(&episode, first);
    let first_receipt = suite
        .record(
            &first.evidence.origin_id,
            first,
            &mut arm,
            &first_consent,
            &episode.operations[0].operation_id,
        )
        .expect("record first origin");
    assert_eq!(first_receipt.judgment.verdict, Verdict::AsrSubstitution);
    assert_eq!(
        first_receipt.judgment.first_difference_stage,
        Some(DifferenceStage::AsrOriginal)
    );
    assert!(first_receipt.candidate_writer_called);
    assert_eq!(first_receipt.candidate_write, "candidate_written");
    assert_eq!(first_receipt.states_consumed, first.ordered_samples.len());
    assert!(first_receipt.settled);
    let key = crate::dictionary_learn::pair_key("VoiceFlo", "VoiceFlow");
    let first_state = learning_snapshot(&arm, &episode).expect("read first origin state");
    assert_eq!(
        crate::store::get_learn_pair(&arm.root, &key)
            .expect("read pending row")
            .expect("first origin stores a pending pair")
            .hits,
        1
    );
    assert_eq!(
        first_state
            .observations
            .iter()
            .filter(|(_, kind)| kind == "research_fixture_edit")
            .count(),
        1
    );
    assert_eq!(first_state.settings_bytes, initial.settings_bytes);

    let duplicate = suite
        .record(
            &first.evidence.origin_id,
            first,
            &mut arm,
            &first_consent,
            &episode.operations[1].operation_id,
        )
        .expect("operation ID does not change the fixed source origin");
    assert!(duplicate.candidate_writer_called);
    assert_eq!(duplicate.candidate_write, "duplicate_observation");
    assert_eq!(
        learning_snapshot(&arm, &episode).expect("read duplicate state"),
        first_state
    );

    let mut changed_stage = first.clone();
    changed_stage.evidence.local_preparation.value.text =
        StageText::Present("we saw another name today".to_owned());
    let mut changed_trajectory = first.clone();
    changed_trajectory
        .ordered_samples
        .push(ControlledAttributionSample::FieldText(
            "we saw VoiceFlo today".to_owned(),
        ));
    let mut changed_scope = first.clone();
    changed_scope.scope.mapping_id = "another-controlled-mapping".to_owned();
    let mut changed_origin = first.clone();
    changed_origin.evidence.origin_id = "different-origin".to_owned();
    for changed in [
        &changed_stage,
        &changed_trajectory,
        &changed_scope,
        &changed_origin,
    ] {
        let error = match suite.record(
            &first.evidence.origin_id,
            changed,
            &mut arm,
            &first_consent,
            &episode.operations[2].operation_id,
        ) {
            Err(error) => error,
            Ok(_) => panic!("same origin with altered evidence must be rejected"),
        };
        assert!(error.to_string().contains("origin_evidence_conflict"));
        assert_eq!(
            learning_snapshot(&arm, &episode).expect("read state after origin conflict"),
            first_state
        );
    }
    assert!(suite
        .open_arm(
            &first.evidence.origin_id,
            &changed_scope,
            root.path(),
            &episode
        )
        .is_err());

    drop(arm);
    let mut arm = suite
        .open_arm(&first.evidence.origin_id, first, root.path(), &episode)
        .expect("reopen the same arm through the same in-memory pin owner");
    let reopen_replay = suite
        .record(
            &first.evidence.origin_id,
            first,
            &mut arm,
            &first_consent,
            &episode.operations[2].operation_id,
        )
        .expect("reopen replay remains tied to the original source origin");
    assert_eq!(reopen_replay.candidate_write, "duplicate_observation");
    assert_eq!(
        learning_snapshot(&arm, &episode).expect("read reopened replay"),
        first_state
    );

    for (index, fixture) in fixtures[1..3].iter().enumerate() {
        let consent = consent_for(&episode, fixture);
        let result = suite
            .record(
                &fixture.evidence.origin_id,
                fixture,
                &mut arm,
                &consent,
                &episode.operations[index].operation_id,
            )
            .expect("independent origin records distinct evidence");
        assert_eq!(result.judgment.verdict, Verdict::AsrSubstitution);
        assert!(result.candidate_writer_called);
        assert_eq!(result.candidate_write, "candidate_written");
        let row = crate::store::get_learn_pair(&arm.root, &key)
            .expect("read candidate row")
            .expect("candidate is available");
        assert_eq!(row.hits, (index + 2) as u32);
        assert_eq!(row.is_live_promoted(), index == 1);
    }

    let promoted = crate::store::get_learn_pair(&arm.root, &key)
        .expect("read promoted candidate")
        .expect("candidate is present");
    assert!(promoted.is_live_promoted());
    assert!(promoted.auto_scoped);
    assert_eq!(
        promoted.mapping_id.as_deref(),
        Some(scope.mapping_id.as_str())
    );
    assert_eq!(
        promoted.native_bundle.as_deref(),
        Some(scope.native_bundle.as_str())
    );
    let before_application = learning_snapshot(&arm, &episode).expect("read promotion state");
    assert_eq!(before_application.settings_bytes, initial.settings_bytes);

    drop(arm);
    let mut arm = suite
        .open_arm(&first.evidence.origin_id, first, root.path(), &episode)
        .expect("reopen the promoted arm through the same in-memory pin owner");
    assert_eq!(
        learning_snapshot(&arm, &episode).expect("read promoted state after reopen"),
        before_application
    );
    let reopened_promoted = crate::store::get_learn_pair(&arm.root, &key)
        .expect("read promoted row after reopen")
        .expect("promoted row survives reopen");
    assert_eq!(reopened_promoted.hits, 3);
    assert!(reopened_promoted.is_live_promoted());

    let same_scope = crate::store::LearnPairScope {
        family: Some(crate::context::family_id(scope.family).to_owned()),
        mapping_id: Some(scope.mapping_id.clone()),
        browser_host: None,
        native_bundle: Some(scope.native_bundle.clone()),
    };
    let all_rows = crate::store::list_learn_pairs(&arm.root).expect("read candidate rows");
    let same_scope_rows = same_scope.applicable_promoted_records(&all_rows);
    assert_eq!(same_scope_rows.len(), 1);
    let same_scope_pairs =
        crate::lexicon::replaceable_pairs(&same_scope_rows, &arm.settings.dictionary);
    let (same_scope_text, _, same_scope_applied) =
        crate::lexicon::apply_explicit_replacements_with_facts(
            "we will see VoiceFlo next week",
            &same_scope_pairs,
            &arm.settings.dictionary,
        );
    assert_eq!(same_scope_text, "we will see VoiceFlow next week");
    assert_eq!(same_scope_applied.len(), 1);

    let other_scope = crate::store::LearnPairScope {
        family: same_scope.family.clone(),
        mapping_id: Some("another-controlled-mapping".to_owned()),
        browser_host: None,
        native_bundle: Some("com.example.another-fixture".to_owned()),
    };
    let other_scope_rows = other_scope.applicable_promoted_records(&all_rows);
    assert!(other_scope_rows.is_empty());
    let other_scope_pairs =
        crate::lexicon::replaceable_pairs(&other_scope_rows, &arm.settings.dictionary);
    let (other_scope_text, _, other_scope_applied) =
        crate::lexicon::apply_explicit_replacements_with_facts(
            "we will see VoiceFlo next week",
            &other_scope_pairs,
            &arm.settings.dictionary,
        );
    assert_eq!(other_scope_text, "we will see VoiceFlo next week");
    assert!(other_scope_applied.is_empty());

    let apply = &episode.followups[0];
    let apply_receipt = arm
        .execute_followup(
            &episode,
            &apply.task_id,
            &apply.operation_id,
            "we will see VoiceFlo tomorrow",
        )
        .expect("same-scope isolated rule applies to a new sentence");
    assert!(apply_receipt.expected_output_match);
    assert_eq!(apply_receipt.output_text, "we will see VoiceFlow tomorrow");
    assert_eq!(
        apply_receipt.selected_pair_attribution,
        "selected_learning_pair"
    );
    assert_eq!(apply_receipt.usage_receipt, "selected_pair_usage_recorded");
    let after_application = learning_snapshot(&arm, &episode).expect("read application usage");
    assert_eq!(after_application.settings_bytes, initial.settings_bytes);
    assert_eq!(after_application.learned_term_usage.len(), 1);
    assert_eq!(after_application.learned_term_usage[0].0, "VoiceFlow");
    assert_eq!(after_application.learned_term_usage[0].1, 1);

    let undo = &episode.undo_operations[0];
    let undo_receipt = arm
        .execute_undo(&episode, &undo.operation_id)
        .expect("undo the isolated auto-scoped rule after actual usage");
    assert_eq!(undo_receipt.state_after, "tombstoned");
    let after_undo = learning_snapshot(&arm, &episode).expect("read tombstone state");
    assert_eq!(after_undo.settings_bytes, initial.settings_bytes);
    let tombstone = crate::store::get_learn_pair(&arm.root, &key)
        .expect("read tombstoned row")
        .expect("undo retains the candidate history");
    assert!(tombstone.tombstoned_at.is_some());
    assert!(!tombstone.is_live_promoted());
    assert!(crate::store::list_learned_term_usage(&arm.root)
        .expect("read applicable usage")
        .is_empty());

    drop(arm);
    let mut arm = suite
        .open_arm(&first.evidence.origin_id, first, root.path(), &episode)
        .expect("reopen the tombstoned isolated arm through the same owner");
    let reopened_state = arm
        .selected_state(&episode)
        .expect("read reopened tombstone");
    assert_eq!(reopened_state.pairs[0].state, "tombstoned");
    let no_relearn_before = learning_snapshot(&arm, &episode).expect("read before passive relearn");

    let duplicate_after_undo = suite
        .record(
            &first.evidence.origin_id,
            first,
            &mut arm,
            &first_consent,
            &episode.operations[0].operation_id,
        )
        .expect("same source replay cannot revive an undone rule");
    assert!(duplicate_after_undo.candidate_writer_called);
    assert_eq!(
        learning_snapshot(&arm, &episode).expect("read after same-source replay"),
        no_relearn_before
    );

    let new_origin = &fixtures[3];
    let new_consent = consent_for(&episode, new_origin);
    let independent_after_undo = suite
        .record(
            &new_origin.evidence.origin_id,
            new_origin,
            &mut arm,
            &new_consent,
            &episode.operations[1].operation_id,
        )
        .expect("independent source cannot passively relearn a tombstoned pair");
    assert!(independent_after_undo.candidate_writer_called);
    assert_eq!(
        learning_snapshot(&arm, &episode).expect("read after independent post-undo source"),
        no_relearn_before
    );
    let after_attempts = crate::store::get_learn_pair(&arm.root, &key)
        .expect("read final row")
        .expect("tombstoned rule stays stored");
    assert_eq!(after_attempts.hits, 3);
    assert!(after_attempts.tombstoned_at.is_some());

    let tombstoned_followup = &episode.followups[1];
    let unchanged = arm
        .execute_followup(
            &episode,
            &tombstoned_followup.task_id,
            &tombstoned_followup.operation_id,
            "we will see VoiceFlo next week",
        )
        .expect("reopened undo prevents the prior rule applying again");
    assert_eq!(unchanged.output_text, "we will see VoiceFlo next week");
    assert_eq!(unchanged.selected_pair_attribution, "no_pair_applied");
    assert_eq!(
        crate::store::get_learn_pair(&arm.root, &key)
            .expect("read final tombstone")
            .expect("tombstone remains")
            .hits,
        3
    );
    assert_eq!(
        std::fs::read(arm.root.join("settings.json")).expect("read final settings bytes"),
        initial.settings_bytes
    );
}
