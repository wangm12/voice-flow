//! Deterministic, research-only attribution for synthetic edit trajectories.
//!
//! This module deliberately has no store, observer, settings, or expected-answer
//! dependency. Tests (and a later research adapter) provide bound evidence; the
//! judge only classifies what that evidence proves.

use std::ops::Range;

const MAX_ALIGNMENT_CHARS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Evidence<T> {
    pub(crate) origin_id: String,
    pub(crate) value: T,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReferenceText {
    Present {
        text: String,
        source: ReferenceSource,
    },
    Missing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReferenceSource {
    Synthetic,
    HumanReviewed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum StageId {
    AsrOriginal,
    LocalPreparation,
    CleanupCandidate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StageText {
    Present(String),
    Skipped,
    Missing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ByteRange {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RuleChange {
    pub(crate) rule_id: String,
    pub(crate) before: ByteRange,
    pub(crate) after: ByteRange,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StageEvidence {
    pub(crate) text: StageText,
    /// Present only for local preparation. IDs are attributed only when one
    /// range exactly matches the ASR-to-preparation diff.
    pub(crate) rule_changes: Vec<RuleChange>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeliverySource {
    PassThrough(StageId),
    CleanupCandidate,
    FallbackTo(StageId),
    GuardOutput { source: StageId },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeliveryEvidence {
    pub(crate) source: DeliverySource,
    /// Text actually inserted into the field.
    pub(crate) inserted_text: String,
    pub(crate) field_id: String,
    /// First observed field snapshot after insertion.
    pub(crate) first_snapshot: String,
    /// UTF-8 byte range occupied by this insertion in the field.
    pub(crate) inserted_range: ByteRange,
    /// Required only when a guard independently emitted a different string.
    pub(crate) guard_output: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditSettlement {
    Stable,
    Unsettled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditSourceTrust {
    SameFieldObserver,
    Untrusted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ObservedEdit {
    pub(crate) field_id: String,
    pub(crate) stable_text: String,
    pub(crate) settlement: EditSettlement,
    pub(crate) source_trust: EditSourceTrust,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditIntent {
    LaterAddition,
    ActiveRewrite,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LearningScope(pub(crate) String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AttributionInput {
    pub(crate) origin_id: String,
    pub(crate) reference: Evidence<ReferenceText>,
    pub(crate) asr: Evidence<StageEvidence>,
    pub(crate) local_preparation: Evidence<StageEvidence>,
    pub(crate) cleanup_candidate: Evidence<StageEvidence>,
    pub(crate) delivery: Evidence<DeliveryEvidence>,
    pub(crate) edit: Evidence<ObservedEdit>,
    pub(crate) intent: Option<Evidence<EditIntent>>,
    /// Scope is kept separate from origin. It gates only the proposed learning
    /// pair and never changes an otherwise supported attribution.
    pub(crate) learning_scope: Option<Evidence<LearningScope>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    UserAddition,
    ContentEdit,
    AsrOmission,
    AsrSubstitution,
    LocalPrepRegression,
    CleanupRegression,
    Unchanged,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnknownReason {
    OriginMismatch,
    DeliveryStageMismatch,
    DeliveryBaselineMismatch,
    RangeUnsupported,
    FieldMismatch,
    EditUnsettled,
    EditSourceUntrusted,
    ReferenceMissing,
    StageMissing,
    AlignmentAmbiguous,
    AlignmentLimitExceeded,
    InvalidDeliveryPath,
    IntentConflict,
    UnsupportedStageTransition,
    DeliveryStageUnattributed,
    UnsupportedChange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DifferenceStage {
    AsrOriginal,
    LocalPreparation,
    CleanupCandidate,
    GuardOutput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CandidateAssessment {
    NotAssessable,
    NoRegression,
    Regression,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CandidateObservation {
    pub(crate) assessment: CandidateAssessment,
    pub(crate) delivered: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScopedWordPair {
    pub(crate) scope: String,
    pub(crate) observed: String,
    pub(crate) correction: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Judgment {
    pub(crate) verdict: Verdict,
    pub(crate) reason: Option<UnknownReason>,
    pub(crate) first_difference_stage: Option<DifferenceStage>,
    /// Independent of the user's edit attribution, including for unchanged
    /// trajectories and rejected cleanup candidates.
    pub(crate) cleanup_candidate: CandidateObservation,
    /// A proposal only; existing lexical, authorization, and store gates remain
    /// the responsibility of the later integration layer.
    pub(crate) proposed_learning_pair: Option<ScopedWordPair>,
    /// Set only when one recorded local rule range exactly matches the unique
    /// ASR-to-preparation change.
    pub(crate) local_rule_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Unit {
    text: String,
    bytes: Range<usize>,
    kind: UnitKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UnitKind {
    Word,
    Whitespace,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Hunk {
    left_units: Range<usize>,
    right_units: Range<usize>,
    left_bytes: ByteRange,
    right_bytes: ByteRange,
    left_text: String,
    right_text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiffError {
    Ambiguous,
    TooLong,
}

#[derive(Clone, Debug)]
struct ResolvedDelivery {
    text: String,
    selected_stage: StageId,
    guard_generated_distinct_text: bool,
}

/// Classifies a single, uniquely aligned edit against origin-bound evidence.
/// It never reads a fixture's expected verdict, expected pair, or write permit.
pub(crate) fn judge(input: &AttributionInput) -> Judgment {
    let mut result = empty_judgment();
    if !evidence_has_one_origin(input) {
        return unknown(result, UnknownReason::OriginMismatch, None);
    }

    let delivery = match resolve_delivery(input) {
        Ok(delivery) => delivery,
        Err(reason) => return unknown(result, reason, None),
    };
    let selected_text_matches = delivery.text == input.delivery.value.inserted_text;
    let insertion_slice_matches = delivery_insertion_slice_matches(&input.delivery.value);
    result.cleanup_candidate = observe_candidate(
        input,
        &delivery,
        selected_text_matches && insertion_slice_matches,
    );
    result.first_difference_stage = known_first_difference(input, &delivery);

    if !selected_text_matches {
        return unknown(result, UnknownReason::DeliveryStageMismatch, None);
    }
    if !insertion_slice_matches {
        return unknown(result, UnknownReason::DeliveryBaselineMismatch, None);
    }
    if input.delivery.value.inserted_range.start != 0
        || input.delivery.value.inserted_range.end != input.delivery.value.first_snapshot.len()
        || input.delivery.value.first_snapshot != input.delivery.value.inserted_text
    {
        return unknown(result, UnknownReason::RangeUnsupported, None);
    }
    if input.delivery.value.field_id != input.edit.value.field_id {
        return unknown(result, UnknownReason::FieldMismatch, None);
    }
    if input.edit.value.source_trust != EditSourceTrust::SameFieldObserver {
        return unknown(result, UnknownReason::EditSourceUntrusted, None);
    }
    if input.edit.value.settlement != EditSettlement::Stable {
        return unknown(result, UnknownReason::EditUnsettled, None);
    }

    if input.edit.value.stable_text == input.delivery.value.inserted_text {
        result.verdict = Verdict::Unchanged;
        return result;
    }

    let reference = match &input.reference.value {
        ReferenceText::Present { text, .. } => text,
        ReferenceText::Missing => return unknown(result, UnknownReason::ReferenceMissing, None),
    };
    let edit_hunk = match single_hunk(
        &input.delivery.value.inserted_text,
        &input.edit.value.stable_text,
    ) {
        Ok(Some(hunk)) => hunk,
        Ok(None) => {
            result.verdict = Verdict::Unchanged;
            return result;
        }
        Err(DiffError::Ambiguous) => {
            return unknown(result, UnknownReason::AlignmentAmbiguous, None)
        }
        Err(DiffError::TooLong) => {
            return unknown(result, UnknownReason::AlignmentLimitExceeded, None)
        }
    };

    let reference_delivery_hunk = match single_hunk(reference, &delivery.text) {
        Ok(hunk) => hunk,
        Err(DiffError::Ambiguous) => {
            return unknown(result, UnknownReason::AlignmentAmbiguous, None)
        }
        Err(DiffError::TooLong) => {
            return unknown(result, UnknownReason::AlignmentLimitExceeded, None)
        }
    };

    let route_stages = match stages_through(delivery.selected_stage, input) {
        Ok(stages) => stages,
        Err(reason) => return unknown(result, reason, None),
    };

    if reference_delivery_hunk.is_none() {
        return classify_change_after_correct_delivery(
            input,
            reference,
            &edit_hunk,
            route_stages,
            result,
        );
    }

    let reference_delivery_hunk = reference_delivery_hunk.expect("checked above");
    if !same_target_restoration(&reference_delivery_hunk, &edit_hunk) {
        return unknown(result, UnknownReason::UnsupportedChange, None);
    }
    if delivery.guard_generated_distinct_text {
        return unknown(
            result,
            UnknownReason::DeliveryStageUnattributed,
            Some(DifferenceStage::GuardOutput),
        );
    }

    let first_difference = match first_persistent_difference(
        reference,
        &reference_delivery_hunk,
        &delivery,
        &route_stages,
    ) {
        Ok(stage) => stage,
        Err(reason) => return unknown(result, reason, None),
    };
    let Some(first_difference) = first_difference else {
        return unknown(
            result,
            UnknownReason::DeliveryStageUnattributed,
            Some(DifferenceStage::GuardOutput),
        );
    };
    result.first_difference_stage = Some(first_difference);

    if input.intent.as_ref().is_some_and(|intent| {
        intent.value == EditIntent::LaterAddition || intent.value == EditIntent::ActiveRewrite
    }) {
        return unknown(
            result,
            UnknownReason::IntentConflict,
            Some(first_difference),
        );
    }

    match first_difference {
        DifferenceStage::AsrOriginal if is_word_insertion(&reference_delivery_hunk, &edit_hunk) => {
            result.verdict = Verdict::AsrOmission;
        }
        DifferenceStage::AsrOriginal if is_single_word_replacement(&reference_delivery_hunk) => {
            result.verdict = Verdict::AsrSubstitution;
            result.proposed_learning_pair = proposed_pair(input, &edit_hunk);
        }
        DifferenceStage::LocalPreparation => {
            result.verdict = Verdict::LocalPrepRegression;
            result.local_rule_id = unique_local_rule_id(input);
        }
        DifferenceStage::CleanupCandidate => {
            result.verdict = Verdict::CleanupRegression;
        }
        DifferenceStage::GuardOutput => {
            return unknown(
                result,
                UnknownReason::DeliveryStageUnattributed,
                Some(DifferenceStage::GuardOutput),
            );
        }
        _ => {
            return unknown(
                result,
                UnknownReason::UnsupportedChange,
                Some(first_difference),
            );
        }
    }
    result
}

fn empty_judgment() -> Judgment {
    Judgment {
        verdict: Verdict::Unknown,
        reason: None,
        first_difference_stage: None,
        cleanup_candidate: CandidateObservation {
            assessment: CandidateAssessment::NotAssessable,
            delivered: false,
        },
        proposed_learning_pair: None,
        local_rule_id: None,
    }
}

fn unknown(
    mut judgment: Judgment,
    reason: UnknownReason,
    first_difference_stage: Option<DifferenceStage>,
) -> Judgment {
    judgment.verdict = Verdict::Unknown;
    judgment.reason = Some(reason);
    judgment.first_difference_stage = first_difference_stage.or(judgment.first_difference_stage);
    judgment.proposed_learning_pair = None;
    judgment.local_rule_id = None;
    judgment
}

fn evidence_has_one_origin(input: &AttributionInput) -> bool {
    if input.origin_id.is_empty()
        || input.reference.origin_id != input.origin_id
        || input.asr.origin_id != input.origin_id
        || input.local_preparation.origin_id != input.origin_id
        || input.cleanup_candidate.origin_id != input.origin_id
        || input.delivery.origin_id != input.origin_id
        || input.edit.origin_id != input.origin_id
    {
        return false;
    }
    input
        .intent
        .as_ref()
        .is_none_or(|intent| intent.origin_id == input.origin_id)
}

fn stage(input: &AttributionInput, id: StageId) -> &StageEvidence {
    match id {
        StageId::AsrOriginal => &input.asr.value,
        StageId::LocalPreparation => &input.local_preparation.value,
        StageId::CleanupCandidate => &input.cleanup_candidate.value,
    }
}

fn resolve_delivery(input: &AttributionInput) -> Result<ResolvedDelivery, UnknownReason> {
    let (selected_stage, path_text, guard_output) = match input.delivery.value.source {
        DeliverySource::PassThrough(selected) => (selected, None, false),
        DeliverySource::CleanupCandidate => (StageId::CleanupCandidate, None, false),
        DeliverySource::FallbackTo(StageId::CleanupCandidate) => {
            return Err(UnknownReason::InvalidDeliveryPath);
        }
        DeliverySource::FallbackTo(selected) => (selected, None, false),
        DeliverySource::GuardOutput { source } => {
            let Some(output) = input.delivery.value.guard_output.as_ref() else {
                return Err(UnknownReason::DeliveryStageMismatch);
            };
            (source, Some(output.clone()), true)
        }
    };
    let source_text = match &stage(input, selected_stage).text {
        StageText::Present(text) => text,
        StageText::Skipped | StageText::Missing => return Err(UnknownReason::StageMissing),
    };
    let text = path_text.unwrap_or_else(|| source_text.to_string());
    Ok(ResolvedDelivery {
        guard_generated_distinct_text: guard_output && text != *source_text,
        text,
        selected_stage,
    })
}

fn delivery_insertion_slice_matches(delivery: &DeliveryEvidence) -> bool {
    delivery
        .first_snapshot
        .get(delivery.inserted_range.start..delivery.inserted_range.end)
        == Some(delivery.inserted_text.as_str())
}

fn known_first_difference(
    input: &AttributionInput,
    delivery: &ResolvedDelivery,
) -> Option<DifferenceStage> {
    let ReferenceText::Present {
        text: reference, ..
    } = &input.reference.value
    else {
        return None;
    };
    let stages = stages_through(delivery.selected_stage, input).ok()?;
    for (id, evidence) in stages {
        let StageText::Present(text) = &evidence.text else {
            continue;
        };
        if text != reference {
            return Some(difference_stage(id));
        }
    }
    (delivery.guard_generated_distinct_text && delivery.text != *reference)
        .then_some(DifferenceStage::GuardOutput)
}

fn observe_candidate(
    input: &AttributionInput,
    delivery: &ResolvedDelivery,
    insertion_is_bound: bool,
) -> CandidateObservation {
    let delivered = insertion_is_bound
        && delivery.selected_stage == StageId::CleanupCandidate
        && delivery.text == input.delivery.value.inserted_text
        && matches!(
            &input.cleanup_candidate.value.text,
            StageText::Present(text) if *text == input.delivery.value.inserted_text
        );
    let candidate = match &input.cleanup_candidate.value.text {
        StageText::Present(text) => text,
        StageText::Skipped | StageText::Missing => {
            return CandidateObservation {
                assessment: CandidateAssessment::NotAssessable,
                delivered,
            }
        }
    };
    let prepared = match &input.local_preparation.value.text {
        StageText::Present(text) => text,
        StageText::Skipped | StageText::Missing => {
            return CandidateObservation {
                assessment: CandidateAssessment::NotAssessable,
                delivered,
            }
        }
    };
    let reference = match &input.reference.value {
        ReferenceText::Present { text, .. } => text,
        ReferenceText::Missing => {
            return CandidateObservation {
                assessment: CandidateAssessment::NotAssessable,
                delivered,
            }
        }
    };
    let assessment = candidate_regression_assessment(reference, prepared, candidate);
    CandidateObservation {
        assessment,
        delivered,
    }
}

fn candidate_regression_assessment(
    reference: &str,
    prepared: &str,
    candidate: &str,
) -> CandidateAssessment {
    if prepared == candidate || candidate == reference {
        return CandidateAssessment::NoRegression;
    }
    let candidate_change = match single_hunk(prepared, candidate) {
        Ok(Some(hunk)) => hunk,
        Ok(None) | Err(_) => return CandidateAssessment::NotAssessable,
    };
    let preparation_errors = match alignment_hunks(reference, prepared) {
        Ok(hunks) => hunks,
        Err(_) => return CandidateAssessment::NotAssessable,
    };
    let candidate_errors = match alignment_hunks(reference, candidate) {
        Ok(hunks) => hunks,
        Err(_) => return CandidateAssessment::NotAssessable,
    };
    let mut introduced_error = false;
    for candidate_error in candidate_errors {
        if range_contains(&candidate_change.right_units, &candidate_error.right_units) {
            if preparation_errors.iter().any(|preparation_error| {
                ranges_overlap(&candidate_change.left_units, &preparation_error.right_units)
            }) {
                return CandidateAssessment::NotAssessable;
            }
            introduced_error = true;
            continue;
        }
        let Some(prepared_range) =
            candidate_range_to_preparation(&candidate_error.right_units, &candidate_change)
        else {
            return CandidateAssessment::NotAssessable;
        };
        if !preparation_errors
            .iter()
            .any(|preparation_error| preparation_error.right_units == prepared_range)
        {
            return CandidateAssessment::NotAssessable;
        }
    }
    if introduced_error {
        CandidateAssessment::Regression
    } else {
        CandidateAssessment::NoRegression
    }
}

fn range_contains(outer: &Range<usize>, inner: &Range<usize>) -> bool {
    if inner.is_empty() {
        if outer.is_empty() {
            outer.start == inner.start
        } else {
            outer.start <= inner.start && inner.start <= outer.end
        }
    } else if outer.is_empty() {
        false
    } else {
        outer.start <= inner.start && inner.end <= outer.end
    }
}

fn ranges_overlap(left: &Range<usize>, right: &Range<usize>) -> bool {
    if left.is_empty() && right.is_empty() {
        left.start == right.start
    } else if left.is_empty() {
        right.start < left.start && left.start < right.end
    } else if right.is_empty() {
        left.start < right.start && right.start < left.end
    } else {
        left.start < right.end && right.start < left.end
    }
}

fn candidate_range_to_preparation(range: &Range<usize>, change: &Hunk) -> Option<Range<usize>> {
    if range.end <= change.right_units.start {
        return Some(range.clone());
    }
    if range.start >= change.right_units.end {
        let removed = change.left_units.len() as isize;
        let added = change.right_units.len() as isize;
        let shift = removed - added;
        let start = range.start.checked_add_signed(shift)?;
        let end = range.end.checked_add_signed(shift)?;
        return Some(start..end);
    }
    None
}

fn stages_through(
    selected: StageId,
    input: &AttributionInput,
) -> Result<Vec<(StageId, &StageEvidence)>, UnknownReason> {
    let ids: &[StageId] = match selected {
        StageId::AsrOriginal => &[StageId::AsrOriginal],
        StageId::LocalPreparation => &[StageId::AsrOriginal, StageId::LocalPreparation],
        StageId::CleanupCandidate => &[
            StageId::AsrOriginal,
            StageId::LocalPreparation,
            StageId::CleanupCandidate,
        ],
    };
    let mut stages = Vec::with_capacity(ids.len());
    for id in ids {
        let evidence = stage(input, *id);
        match &evidence.text {
            StageText::Present(_) => stages.push((*id, evidence)),
            StageText::Skipped | StageText::Missing => return Err(UnknownReason::StageMissing),
        }
    }
    Ok(stages)
}

fn classify_change_after_correct_delivery(
    input: &AttributionInput,
    reference: &str,
    edit_hunk: &Hunk,
    route_stages: Vec<(StageId, &StageEvidence)>,
    mut judgment: Judgment,
) -> Judgment {
    if input.intent.as_ref().is_some_and(|intent| {
        intent.value == EditIntent::LaterAddition
            && (!edit_hunk.left_units.is_empty()
                || edit_hunk.right_units.is_empty()
                || !is_word_content(&edit_hunk.right_text))
    }) {
        return unknown(judgment, UnknownReason::IntentConflict, None);
    }

    if input
        .intent
        .as_ref()
        .is_some_and(|intent| intent.value == EditIntent::LaterAddition)
    {
        for (id, evidence) in route_stages {
            let StageText::Present(text) = &evidence.text else {
                continue;
            };
            if text != reference {
                return unknown(
                    judgment,
                    UnknownReason::IntentConflict,
                    Some(difference_stage(id)),
                );
            }
        }
        judgment.verdict = Verdict::UserAddition;
    } else {
        judgment.verdict = Verdict::ContentEdit;
    }
    judgment
}

fn same_target_restoration(reference_delivery: &Hunk, edit: &Hunk) -> bool {
    reference_delivery.right_units == edit.left_units
        && reference_delivery.left_text == edit.right_text
        && reference_delivery.right_text == edit.left_text
}

fn first_persistent_difference(
    reference: &str,
    reference_delivery: &Hunk,
    delivery: &ResolvedDelivery,
    stages: &[(StageId, &StageEvidence)],
) -> Result<Option<DifferenceStage>, UnknownReason> {
    if delivery.guard_generated_distinct_text {
        return Ok(Some(DifferenceStage::GuardOutput));
    }
    let mut first = None;
    let mut recovered = false;
    for (id, evidence) in stages {
        let StageText::Present(text) = &evidence.text else {
            continue;
        };
        if text == reference {
            if first.is_some() {
                recovered = true;
            }
            continue;
        }
        let stage_hunk = match single_hunk(reference, text) {
            Ok(Some(hunk)) => hunk,
            Ok(None) => return Err(UnknownReason::UnsupportedStageTransition),
            Err(DiffError::Ambiguous) => return Err(UnknownReason::AlignmentAmbiguous),
            Err(DiffError::TooLong) => return Err(UnknownReason::AlignmentLimitExceeded),
        };
        if stage_hunk.left_units != reference_delivery.left_units
            || stage_hunk.right_units != reference_delivery.right_units
            || stage_hunk.right_text != reference_delivery.right_text
        {
            return Err(UnknownReason::UnsupportedStageTransition);
        }
        if recovered {
            return Err(UnknownReason::UnsupportedStageTransition);
        }
        if first.is_none() {
            first = Some(difference_stage(*id));
        }
    }
    Ok(first)
}

fn difference_stage(stage: StageId) -> DifferenceStage {
    match stage {
        StageId::AsrOriginal => DifferenceStage::AsrOriginal,
        StageId::LocalPreparation => DifferenceStage::LocalPreparation,
        StageId::CleanupCandidate => DifferenceStage::CleanupCandidate,
    }
}

fn is_word_insertion(reference_delivery: &Hunk, edit: &Hunk) -> bool {
    reference_delivery.right_units.is_empty()
        && !reference_delivery.left_units.is_empty()
        && edit.left_units.is_empty()
        && !edit.right_units.is_empty()
        && is_word_content(&reference_delivery.left_text)
        && reference_delivery.left_text == edit.right_text
}

fn is_word_content(text: &str) -> bool {
    let units = tokenize(text);
    units.iter().any(|unit| unit.kind == UnitKind::Word)
}

fn is_single_word_replacement(hunk: &Hunk) -> bool {
    hunk.left_units.len() == 1
        && hunk.right_units.len() == 1
        && match (
            single_word_surface(&hunk.left_text),
            single_word_surface(&hunk.right_text),
        ) {
            (Some(left), Some(right)) => left != right,
            _ => false,
        }
}

fn single_word_surface(text: &str) -> Option<String> {
    let surface = text.trim_matches(char::is_whitespace);
    (!surface.is_empty() && surface.chars().all(|ch| char_kind(ch) == UnitKind::Word))
        .then(|| surface.to_string())
}

fn proposed_pair(input: &AttributionInput, edit_hunk: &Hunk) -> Option<ScopedWordPair> {
    let scope = input.learning_scope.as_ref()?;
    if scope.origin_id != input.origin_id || scope.value.0.trim().is_empty() {
        return None;
    }
    if !is_single_word_replacement(edit_hunk) {
        return None;
    }
    let observed = single_word_surface(&edit_hunk.left_text)?;
    let correction = single_word_surface(&edit_hunk.right_text)?;
    Some(ScopedWordPair {
        scope: scope.value.0.clone(),
        observed,
        correction,
    })
}

fn unique_local_rule_id(input: &AttributionInput) -> Option<String> {
    let asr = match &input.asr.value.text {
        StageText::Present(text) => text,
        StageText::Skipped | StageText::Missing => return None,
    };
    let prepared = match &input.local_preparation.value.text {
        StageText::Present(text) => text,
        StageText::Skipped | StageText::Missing => return None,
    };
    let change = match single_hunk(asr, prepared) {
        Ok(Some(change)) => change,
        Ok(None) | Err(_) => return None,
    };
    let [rule] = input.local_preparation.value.rule_changes.as_slice() else {
        return None;
    };
    let (before_leading, before_trailing, after_leading, after_trailing) =
        shared_edge_whitespace(&change.left_text, &change.right_text);
    let changed_before = ByteRange {
        start: change.left_bytes.start + before_leading,
        end: change.left_bytes.end - before_trailing,
    };
    let changed_after = ByteRange {
        start: change.right_bytes.start + after_leading,
        end: change.right_bytes.end - after_trailing,
    };
    (rule.before == changed_before
        && rule.after == changed_after
        && !rule.rule_id.trim().is_empty())
    .then(|| rule.rule_id.clone())
}

fn shared_edge_whitespace(left: &str, right: &str) -> (usize, usize, usize, usize) {
    let left_chars: Vec<(usize, char)> = left.char_indices().collect();
    let right_chars: Vec<(usize, char)> = right.char_indices().collect();
    let mut left_leading = 0;
    let mut right_leading = 0;
    for ((left_index, left_char), (right_index, right_char)) in left_chars.iter().zip(&right_chars)
    {
        if left_char != right_char || !left_char.is_whitespace() {
            break;
        }
        left_leading = *left_index + left_char.len_utf8();
        right_leading = *right_index + right_char.len_utf8();
    }

    let mut left_trailing = 0;
    let mut right_trailing = 0;
    for ((left_index, left_char), (right_index, right_char)) in
        left_chars.iter().rev().zip(right_chars.iter().rev())
    {
        if left_char != right_char || !left_char.is_whitespace() {
            break;
        }
        if *left_index < left_leading || *right_index < right_leading {
            break;
        }
        left_trailing += left_char.len_utf8();
        right_trailing += right_char.len_utf8();
    }
    (left_leading, left_trailing, right_leading, right_trailing)
}

fn single_hunk(left_text: &str, right_text: &str) -> Result<Option<Hunk>, DiffError> {
    let mut hunks = alignment_hunks(left_text, right_text)?;
    match hunks.len() {
        0 => Ok(None),
        1 => Ok(hunks.pop()),
        _ => Err(DiffError::Ambiguous),
    }
}

fn alignment_hunks(left_text: &str, right_text: &str) -> Result<Vec<Hunk>, DiffError> {
    if left_text == right_text {
        return Ok(Vec::new());
    }
    if left_text.chars().count() > MAX_ALIGNMENT_CHARS
        || right_text.chars().count() > MAX_ALIGNMENT_CHARS
    {
        return Err(DiffError::TooLong);
    }
    let left = tokenize(left_text);
    let right = tokenize(right_text);
    let left_len = left.len();
    let right_len = right.len();
    let normalized_left = normalized_alignment_keys(&left);
    let normalized_right = normalized_alignment_keys(&right);
    let _ = unique_lcs_mappings(&normalized_left, &normalized_right)?;
    let left_keys: Vec<String> = left.iter().map(|unit| unit.text.clone()).collect();
    let right_keys: Vec<String> = right.iter().map(|unit| unit.text.clone()).collect();
    let mut mappings = unique_lcs_mappings(&left_keys, &right_keys)?;

    let mut hunks = Vec::new();
    let mut left_cursor = 0;
    let mut right_cursor = 0;
    for (left_match, right_match) in mappings.pop().expect("one alignment") {
        if left_cursor != left_match || right_cursor != right_match {
            hunks.push(make_hunk(
                left_text,
                right_text,
                &left,
                &right,
                left_cursor..left_match,
                right_cursor..right_match,
            ));
        }
        left_cursor = left_match + 1;
        right_cursor = right_match + 1;
    }
    if left_cursor != left_len || right_cursor != right_len {
        hunks.push(make_hunk(
            left_text,
            right_text,
            &left,
            &right,
            left_cursor..left_len,
            right_cursor..right_len,
        ));
    }
    Ok(hunks)
}

fn normalized_alignment_keys(units: &[Unit]) -> Vec<String> {
    units
        .iter()
        .filter_map(|unit| {
            let key = unit.text.trim_matches(char::is_whitespace);
            (!key.is_empty()).then(|| key.to_string())
        })
        .collect()
}

fn unique_lcs_mappings(
    left: &[String],
    right: &[String],
) -> Result<Vec<Vec<(usize, usize)>>, DiffError> {
    let mut lcs = vec![vec![0_u16; right.len() + 1]; left.len() + 1];
    for i in (0..left.len()).rev() {
        for j in (0..right.len()).rev() {
            lcs[i][j] = if left[i] == right[j] {
                1 + lcs[i + 1][j + 1]
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut mappings = Vec::with_capacity(2);
    let mut current = Vec::with_capacity(lcs[0][0] as usize);
    collect_unique_lcs_mappings(0, 0, left, right, &lcs, &mut current, &mut mappings);
    if mappings.len() == 1 {
        Ok(mappings)
    } else {
        Err(DiffError::Ambiguous)
    }
}

fn collect_unique_lcs_mappings(
    left_start: usize,
    right_start: usize,
    left: &[String],
    right: &[String],
    lcs: &[Vec<u16>],
    current: &mut Vec<(usize, usize)>,
    results: &mut Vec<Vec<(usize, usize)>>,
) {
    if results.len() >= 2 {
        return;
    }
    let remaining = lcs[left_start][right_start];
    if remaining == 0 {
        results.push(current.clone());
        return;
    }
    for i in left_start..left.len() {
        for j in right_start..right.len() {
            if left[i] == right[j] && lcs[i + 1][j + 1] + 1 == remaining {
                current.push((i, j));
                collect_unique_lcs_mappings(i + 1, j + 1, left, right, lcs, current, results);
                current.pop();
                if results.len() >= 2 {
                    return;
                }
            }
        }
    }
}

fn make_hunk(
    left_text: &str,
    right_text: &str,
    left: &[Unit],
    right: &[Unit],
    left_units: Range<usize>,
    right_units: Range<usize>,
) -> Hunk {
    let left_bytes = unit_range_to_bytes(left_text, left, left_units.clone());
    let right_bytes = unit_range_to_bytes(right_text, right, right_units.clone());
    Hunk {
        left_text: left_text[left_bytes.start..left_bytes.end].to_string(),
        right_text: right_text[right_bytes.start..right_bytes.end].to_string(),
        left_units,
        right_units,
        left_bytes,
        right_bytes,
    }
}

fn unit_range_to_bytes(text: &str, units: &[Unit], range: Range<usize>) -> ByteRange {
    if range.is_empty() {
        let position = units
            .get(range.start)
            .map_or(text.len(), |unit| unit.bytes.start);
        return ByteRange {
            start: position,
            end: position,
        };
    }
    ByteRange {
        start: units[range.start].bytes.start,
        end: units[range.end - 1].bytes.end,
    }
}

fn tokenize(text: &str) -> Vec<Unit> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut result: Vec<Unit> = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let leading_whitespace = if chars[index].1.is_whitespace() {
            let start = chars[index].0;
            while index < chars.len() && chars[index].1.is_whitespace() {
                index += 1;
            }
            if index == chars.len() {
                if let Some(previous) = result.last_mut() {
                    previous.text.push_str(&text[start..]);
                    previous.bytes.end = text.len();
                } else {
                    result.push(Unit {
                        text: text[start..].to_string(),
                        bytes: start..text.len(),
                        kind: UnitKind::Whitespace,
                    });
                }
                break;
            }
            Some(start)
        } else {
            None
        };

        let token_start = chars[index].0;
        let kind = char_kind(chars[index].1);
        index += 1;
        while index < chars.len()
            && !chars[index].1.is_whitespace()
            && char_kind(chars[index].1) == kind
        {
            index += 1;
        }
        let token_end = chars[index - 1].0 + chars[index - 1].1.len_utf8();
        let start = leading_whitespace.unwrap_or(token_start);
        result.push(Unit {
            text: text[start..token_end].to_string(),
            bytes: start..token_end,
            kind,
        });
    }
    result
}

fn char_kind(ch: char) -> UnitKind {
    if ch.is_alphanumeric() || ch == '_' || is_combining_mark(ch) {
        UnitKind::Word
    } else if ch.is_whitespace() {
        UnitKind::Whitespace
    } else {
        UnitKind::Other
    }
}

fn is_combining_mark(ch: char) -> bool {
    matches!(
        ch as u32,
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: &str = "fixture-voice-1";
    const SCOPE: &str = "test-scope";

    struct Expected {
        verdict: Verdict,
        reason: Option<UnknownReason>,
        difference: Option<DifferenceStage>,
        candidate: CandidateAssessment,
        candidate_delivered: bool,
        pair: Option<(&'static str, &'static str)>,
    }

    struct Fixture {
        input: AttributionInput,
        expected: Expected,
    }

    fn evidence<T>(origin: &str, value: T) -> Evidence<T> {
        Evidence {
            origin_id: origin.to_string(),
            value,
        }
    }

    fn present(text: &str) -> StageEvidence {
        StageEvidence {
            text: StageText::Present(text.to_string()),
            rule_changes: Vec::new(),
        }
    }

    fn skipped() -> StageEvidence {
        StageEvidence {
            text: StageText::Skipped,
            rule_changes: Vec::new(),
        }
    }

    fn input(
        reference: ReferenceText,
        asr: StageEvidence,
        prep: StageEvidence,
        candidate: StageEvidence,
        source: DeliverySource,
        selected_text: &str,
        stable_text: &str,
    ) -> AttributionInput {
        let selected_text_owned = match source {
            DeliverySource::PassThrough(StageId::AsrOriginal) => match &asr.text {
                StageText::Present(text) => text.clone(),
                _ => String::new(),
            },
            DeliverySource::PassThrough(StageId::LocalPreparation)
            | DeliverySource::FallbackTo(StageId::LocalPreparation) => match &prep.text {
                StageText::Present(text) => text.clone(),
                _ => String::new(),
            },
            DeliverySource::CleanupCandidate
            | DeliverySource::PassThrough(StageId::CleanupCandidate)
            | DeliverySource::FallbackTo(StageId::CleanupCandidate) => match &candidate.text {
                StageText::Present(text) => text.clone(),
                _ => String::new(),
            },
            DeliverySource::FallbackTo(StageId::AsrOriginal) => match &asr.text {
                StageText::Present(text) => text.clone(),
                _ => String::new(),
            },
            DeliverySource::GuardOutput { .. } => selected_text.to_string(),
        };
        let inserted_text = if matches!(source, DeliverySource::GuardOutput { .. }) {
            selected_text.to_string()
        } else {
            selected_text_owned
        };
        let length = inserted_text.len();
        AttributionInput {
            origin_id: ORIGIN.to_string(),
            reference: evidence(ORIGIN, reference),
            asr: evidence(ORIGIN, asr),
            local_preparation: evidence(ORIGIN, prep),
            cleanup_candidate: evidence(ORIGIN, candidate),
            delivery: evidence(
                ORIGIN,
                DeliveryEvidence {
                    source,
                    inserted_text: inserted_text.clone(),
                    field_id: "field-1".to_string(),
                    first_snapshot: inserted_text,
                    inserted_range: ByteRange {
                        start: 0,
                        end: length,
                    },
                    guard_output: matches!(source, DeliverySource::GuardOutput { .. })
                        .then(|| selected_text.to_string()),
                },
            ),
            edit: evidence(
                ORIGIN,
                ObservedEdit {
                    field_id: "field-1".to_string(),
                    stable_text: stable_text.to_string(),
                    settlement: EditSettlement::Stable,
                    source_trust: EditSourceTrust::SameFieldObserver,
                },
            ),
            intent: None,
            learning_scope: Some(evidence(ORIGIN, LearningScope(SCOPE.to_string()))),
        }
    }

    fn expected(
        verdict: Verdict,
        reason: Option<UnknownReason>,
        difference: Option<DifferenceStage>,
        candidate: CandidateAssessment,
        candidate_delivered: bool,
        pair: Option<(&'static str, &'static str)>,
    ) -> Expected {
        Expected {
            verdict,
            reason,
            difference,
            candidate,
            candidate_delivered,
            pair,
        }
    }

    fn check_case(case: Fixture) {
        let actual = judge(&case.input);
        assert_eq!(actual.verdict, case.expected.verdict);
        assert_eq!(actual.reason, case.expected.reason);
        assert_eq!(actual.first_difference_stage, case.expected.difference);
        assert_eq!(actual.cleanup_candidate.assessment, case.expected.candidate);
        assert_eq!(
            actual.cleanup_candidate.delivered,
            case.expected.candidate_delivered
        );
        assert_eq!(
            actual
                .proposed_learning_pair
                .as_ref()
                .map(|pair| (pair.observed.as_str(), pair.correction.as_str())),
            case.expected.pair
        );
        if let Some(pair) = &actual.proposed_learning_pair {
            assert_eq!(pair.scope, SCOPE);
        }
    }

    #[test]
    fn pure_fixture_matrix_covers_supported_attribution_and_separate_candidate_observation() {
        let mut cases = Vec::new();

        let mut addition = input(
            ReferenceText::Present {
                text: "Deploy today".into(),
                source: ReferenceSource::Synthetic,
            },
            present("Deploy today"),
            present("Deploy today"),
            present("Deploy today"),
            DeliverySource::CleanupCandidate,
            "Deploy today",
            "Deploy today after lunch",
        );
        addition.intent = Some(evidence(ORIGIN, EditIntent::LaterAddition));
        cases.push(Fixture {
            input: addition,
            expected: expected(
                Verdict::UserAddition,
                None,
                None,
                CandidateAssessment::NoRegression,
                true,
                None,
            ),
        });

        let unlabelled_addition = input(
            ReferenceText::Present {
                text: "Deploy today".into(),
                source: ReferenceSource::Synthetic,
            },
            present("Deploy today"),
            present("Deploy today"),
            present("Deploy today"),
            DeliverySource::CleanupCandidate,
            "Deploy today",
            "Deploy today after lunch",
        );
        cases.push(Fixture {
            input: unlabelled_addition,
            expected: expected(
                Verdict::ContentEdit,
                None,
                None,
                CandidateAssessment::NoRegression,
                true,
                None,
            ),
        });

        cases.push(Fixture {
            input: input(
                ReferenceText::Present {
                    text: "Deploy today after lunch".into(),
                    source: ReferenceSource::HumanReviewed,
                },
                present("Deploy today"),
                skipped(),
                skipped(),
                DeliverySource::PassThrough(StageId::AsrOriginal),
                "Deploy today",
                "Deploy today after lunch",
            ),
            expected: expected(
                Verdict::AsrOmission,
                None,
                Some(DifferenceStage::AsrOriginal),
                CandidateAssessment::NotAssessable,
                false,
                None,
            ),
        });

        let cleanup = input(
            ReferenceText::Present {
                text: "Deploy today after lunch".into(),
                source: ReferenceSource::Synthetic,
            },
            present("Deploy today after lunch"),
            present("Deploy today after lunch"),
            present("Deploy today"),
            DeliverySource::CleanupCandidate,
            "Deploy today",
            "Deploy today after lunch",
        );
        let cleanup_expected = expected(
            Verdict::CleanupRegression,
            None,
            Some(DifferenceStage::CleanupCandidate),
            CandidateAssessment::Regression,
            true,
            None,
        );
        cases.push(Fixture {
            input: cleanup,
            expected: cleanup_expected,
        });

        let rejected_candidate = input(
            ReferenceText::Present {
                text: "Deploy today after lunch".into(),
                source: ReferenceSource::Synthetic,
            },
            present("Deploy today after lunch"),
            present("Deploy today after lunch"),
            present("Deploy today"),
            DeliverySource::FallbackTo(StageId::LocalPreparation),
            "Deploy today after lunch",
            "Deploy today after lunch",
        );
        let rejected_expected = expected(
            Verdict::Unchanged,
            None,
            None,
            CandidateAssessment::Regression,
            false,
            None,
        );
        cases.push(Fixture {
            input: rejected_candidate,
            expected: rejected_expected,
        });

        let rejected_asr = input(
            ReferenceText::Present {
                text: "Deploy today tomorrow after lunch".into(),
                source: ReferenceSource::HumanReviewed,
            },
            present("Deploy late tomorrow after lunch"),
            present("Deploy late tomorrow after lunch"),
            present("Deploy late tomorrow"),
            DeliverySource::FallbackTo(StageId::LocalPreparation),
            "Deploy late tomorrow after lunch",
            "Deploy today tomorrow after lunch",
        );
        let rejected_asr_expected = expected(
            Verdict::AsrSubstitution,
            None,
            Some(DifferenceStage::AsrOriginal),
            CandidateAssessment::Regression,
            false,
            Some(("late", "today")),
        );
        cases.push(Fixture {
            input: rejected_asr,
            expected: rejected_asr_expected,
        });

        cases.push(Fixture {
            input: input(
                ReferenceText::Missing,
                present("Deploy today"),
                present("Deploy today"),
                present("Deploy today"),
                DeliverySource::CleanupCandidate,
                "Deploy today",
                "Deploy today after lunch",
            ),
            expected: expected(
                Verdict::Unknown,
                Some(UnknownReason::ReferenceMissing),
                None,
                CandidateAssessment::NotAssessable,
                true,
                None,
            ),
        });

        let mut prep_regression = input(
            ReferenceText::Present {
                text: "VoiceFlow".into(),
                source: ReferenceSource::Synthetic,
            },
            present("VoiceFlow"),
            present("VoiceFlo"),
            present("VoiceFlo"),
            DeliverySource::FallbackTo(StageId::LocalPreparation),
            "VoiceFlo",
            "VoiceFlow",
        );
        if let StageText::Present(asr) = &prep_regression.asr.value.text {
            let hunk = single_hunk(asr, "VoiceFlo").unwrap().unwrap();
            prep_regression.local_preparation.value.rule_changes = vec![RuleChange {
                rule_id: "local-rule-7".into(),
                before: hunk.left_bytes,
                after: hunk.right_bytes,
            }];
        }
        cases.push(Fixture {
            input: prep_regression,
            expected: expected(
                Verdict::LocalPrepRegression,
                None,
                Some(DifferenceStage::LocalPreparation),
                CandidateAssessment::NoRegression,
                false,
                None,
            ),
        });

        let mut conflicting_intent = input(
            ReferenceText::Present {
                text: "Deploy today after lunch".into(),
                source: ReferenceSource::Synthetic,
            },
            present("Deploy today"),
            skipped(),
            skipped(),
            DeliverySource::PassThrough(StageId::AsrOriginal),
            "Deploy today",
            "Deploy today after lunch",
        );
        conflicting_intent.intent = Some(evidence(ORIGIN, EditIntent::LaterAddition));
        cases.push(Fixture {
            input: conflicting_intent,
            expected: expected(
                Verdict::Unknown,
                Some(UnknownReason::IntentConflict),
                Some(DifferenceStage::AsrOriginal),
                CandidateAssessment::NotAssessable,
                false,
                None,
            ),
        });

        for case in cases {
            check_case(case);
        }
    }

    #[test]
    fn cleanup_candidate_regression_requires_a_new_reference_error_at_a_disjoint_span() {
        let fixed_error = input(
            ReferenceText::Present {
                text: "Deploy today after lunch".into(),
                source: ReferenceSource::Synthetic,
            },
            present("Deploi today after launch"),
            present("Deploi today after launch"),
            present("Deploy today after launch"),
            DeliverySource::CleanupCandidate,
            "Deploy today after launch",
            "Deploy today after launch",
        );
        let fixed = judge(&fixed_error);
        assert_eq!(fixed.verdict, Verdict::Unchanged);
        assert_eq!(
            fixed.cleanup_candidate.assessment,
            CandidateAssessment::NoRegression
        );
        assert!(fixed.cleanup_candidate.delivered);

        let overlapping_error = input(
            ReferenceText::Present {
                text: "VoiceFlow".into(),
                source: ReferenceSource::HumanReviewed,
            },
            present("VoiceFlo"),
            present("VoiceFlo"),
            present("VoiceFlow Pro"),
            DeliverySource::FallbackTo(StageId::LocalPreparation),
            "VoiceFlo",
            "VoiceFlow",
        );
        let overlapping = judge(&overlapping_error);
        assert_eq!(overlapping.verdict, Verdict::AsrSubstitution);
        assert_eq!(
            overlapping.cleanup_candidate.assessment,
            CandidateAssessment::NotAssessable
        );
        assert!(!overlapping.cleanup_candidate.delivered);

        let same_point_omission = input(
            ReferenceText::Present {
                text: "Deploy today".into(),
                source: ReferenceSource::Synthetic,
            },
            present("Deploy"),
            present("Deploy"),
            present("Deploy tomorrow"),
            DeliverySource::CleanupCandidate,
            "Deploy tomorrow",
            "Deploy tomorrow",
        );
        let same_point = judge(&same_point_omission);
        assert_eq!(same_point.verdict, Verdict::Unchanged);
        assert_eq!(
            same_point.cleanup_candidate.assessment,
            CandidateAssessment::NotAssessable
        );
        assert!(same_point.cleanup_candidate.delivered);
    }

    #[test]
    fn candidate_and_stage_observations_survive_unsettled_unsupported_and_partial_field_edits() {
        let mut value = input(
            ReferenceText::Present {
                text: "Deploy today tomorrow after lunch".into(),
                source: ReferenceSource::Synthetic,
            },
            present("Deploy late tomorrow after lunch"),
            present("Deploy late tomorrow after lunch"),
            present("Deploy late tomorrow"),
            DeliverySource::FallbackTo(StageId::LocalPreparation),
            "Deploy late tomorrow after lunch",
            "Deploy late tomorrow after lunch please",
        );
        let unrelated_edit = judge(&value);
        assert_eq!(unrelated_edit.verdict, Verdict::Unknown);
        assert_eq!(
            unrelated_edit.reason,
            Some(UnknownReason::UnsupportedChange)
        );
        assert_eq!(
            unrelated_edit.first_difference_stage,
            Some(DifferenceStage::AsrOriginal)
        );
        assert_eq!(
            unrelated_edit.cleanup_candidate.assessment,
            CandidateAssessment::Regression
        );
        assert!(!unrelated_edit.cleanup_candidate.delivered);

        value.edit.value.settlement = EditSettlement::Unsettled;
        let unsettled = judge(&value);
        assert_eq!(unsettled.verdict, Verdict::Unknown);
        assert_eq!(unsettled.reason, Some(UnknownReason::EditUnsettled));
        assert_eq!(
            unsettled.first_difference_stage,
            Some(DifferenceStage::AsrOriginal)
        );
        assert_eq!(
            unsettled.cleanup_candidate.assessment,
            CandidateAssessment::Regression
        );
        assert!(!unsettled.cleanup_candidate.delivered);

        value.edit.value.settlement = EditSettlement::Stable;
        value.delivery.value.first_snapshot = "Prefix Deploy late tomorrow after lunch".into();
        value.delivery.value.inserted_range = ByteRange { start: 7, end: 39 };
        let partial = judge(&value);
        assert_eq!(partial.verdict, Verdict::Unknown);
        assert_eq!(partial.reason, Some(UnknownReason::RangeUnsupported));
        assert_eq!(
            partial.first_difference_stage,
            Some(DifferenceStage::AsrOriginal)
        );
        assert_eq!(
            partial.cleanup_candidate.assessment,
            CandidateAssessment::Regression
        );
        assert!(!partial.cleanup_candidate.delivered);

        value.delivery.value.first_snapshot = "Deploy late tomorrow after lunch".into();
        value.delivery.value.inserted_range = ByteRange { start: 0, end: 32 };
        value.edit.value.field_id = "other-field".into();
        let wrong_field = judge(&value);
        assert_eq!(wrong_field.verdict, Verdict::Unknown);
        assert_eq!(wrong_field.reason, Some(UnknownReason::FieldMismatch));
        assert_eq!(
            wrong_field.first_difference_stage,
            Some(DifferenceStage::AsrOriginal)
        );
        assert_eq!(
            wrong_field.cleanup_candidate.assessment,
            CandidateAssessment::Regression
        );
    }

    #[test]
    fn skipped_stages_require_an_explicit_source_proven_path() {
        let mut skipped_preparation = input(
            ReferenceText::Present {
                text: "Deploy today after lunch".into(),
                source: ReferenceSource::Synthetic,
            },
            present("Deploy today after lunch"),
            skipped(),
            present("Deploy today"),
            DeliverySource::CleanupCandidate,
            "Deploy today",
            "Deploy today after lunch",
        );
        let skipped_judgment = judge(&skipped_preparation);
        assert_eq!(skipped_judgment.verdict, Verdict::Unknown);
        assert_eq!(skipped_judgment.reason, Some(UnknownReason::StageMissing));
        assert_eq!(
            skipped_judgment.cleanup_candidate.assessment,
            CandidateAssessment::NotAssessable
        );
        assert!(skipped_judgment.cleanup_candidate.delivered);

        skipped_preparation.delivery.value.source =
            DeliverySource::FallbackTo(StageId::CleanupCandidate);
        let contradictory_fallback = judge(&skipped_preparation);
        assert_eq!(contradictory_fallback.verdict, Verdict::Unknown);
        assert_eq!(
            contradictory_fallback.reason,
            Some(UnknownReason::InvalidDeliveryPath)
        );

        let explicit_fallback = input(
            ReferenceText::Present {
                text: "Deploy today".into(),
                source: ReferenceSource::Synthetic,
            },
            present("Deploy today"),
            skipped(),
            skipped(),
            DeliverySource::FallbackTo(StageId::AsrOriginal),
            "Deploy today",
            "Deploy today",
        );
        let fallback = judge(&explicit_fallback);
        assert_eq!(fallback.verdict, Verdict::Unchanged);
        assert_eq!(fallback.reason, None);
    }

    #[test]
    fn whitespace_and_contextual_punctuation_do_not_create_false_ambiguity_or_word_pairs() {
        let mut spaced_addition = input(
            ReferenceText::Present {
                text: "Deploy today".into(),
                source: ReferenceSource::Synthetic,
            },
            present("Deploy today"),
            present("Deploy today"),
            present("Deploy today"),
            DeliverySource::CleanupCandidate,
            "Deploy today",
            "Deploy safely today",
        );
        spaced_addition.intent = Some(evidence(ORIGIN, EditIntent::LaterAddition));
        let spaced = judge(&spaced_addition);
        assert_eq!(spaced.verdict, Verdict::UserAddition);
        assert_eq!(spaced.reason, None);

        let mut punctuated_addition = input(
            ReferenceText::Present {
                text: "Deploy today".into(),
                source: ReferenceSource::Synthetic,
            },
            present("Deploy today"),
            present("Deploy today"),
            present("Deploy today"),
            DeliverySource::CleanupCandidate,
            "Deploy today",
            "Deploy today, after lunch.",
        );
        punctuated_addition.intent = Some(evidence(ORIGIN, EditIntent::LaterAddition));
        let punctuated = judge(&punctuated_addition);
        assert_eq!(punctuated.verdict, Verdict::UserAddition);

        let punctuated_omission = input(
            ReferenceText::Present {
                text: "Deploy today, after lunch.".into(),
                source: ReferenceSource::HumanReviewed,
            },
            present("Deploy today"),
            skipped(),
            skipped(),
            DeliverySource::PassThrough(StageId::AsrOriginal),
            "Deploy today",
            "Deploy today, after lunch.",
        );
        let omission = judge(&punctuated_omission);
        assert_eq!(omission.verdict, Verdict::AsrOmission);
        assert_eq!(omission.proposed_learning_pair, None);

        let punctuated_substitution = input(
            ReferenceText::Present {
                text: "VoiceFlow,".into(),
                source: ReferenceSource::HumanReviewed,
            },
            present("VoiceFlo."),
            skipped(),
            skipped(),
            DeliverySource::PassThrough(StageId::AsrOriginal),
            "VoiceFlo.",
            "VoiceFlow,",
        );
        let substitution = judge(&punctuated_substitution);
        assert_eq!(substitution.verdict, Verdict::Unknown);
        assert_eq!(substitution.proposed_learning_pair, None);
    }

    #[test]
    fn utf8_ranges_are_sliced_only_at_valid_boundaries() {
        let hunk = single_hunk("旧文", "新文").unwrap().unwrap();
        assert!("旧文".is_char_boundary(hunk.left_bytes.start));
        assert!("旧文".is_char_boundary(hunk.left_bytes.end));
        assert!("新文".is_char_boundary(hunk.right_bytes.start));
        assert!("新文".is_char_boundary(hunk.right_bytes.end));

        let mut partial = input(
            ReferenceText::Present {
                text: "VoiceFlow".into(),
                source: ReferenceSource::Synthetic,
            },
            present("VoiceFlo"),
            present("VoiceFlo"),
            skipped(),
            DeliverySource::PassThrough(StageId::AsrOriginal),
            "VoiceFlo",
            "VoiceFlow",
        );
        partial.delivery.value.first_snapshot = "前缀 VoiceFlo".into();
        let start = "前缀 ".len();
        partial.delivery.value.inserted_range = ByteRange {
            start,
            end: start + "VoiceFlo".len(),
        };
        let judgment = judge(&partial);
        assert_eq!(judgment.verdict, Verdict::Unknown);
        assert_eq!(judgment.reason, Some(UnknownReason::RangeUnsupported));

        partial.delivery.value.inserted_range.start = 1;
        let invalid_boundary = judge(&partial);
        assert_eq!(invalid_boundary.verdict, Verdict::Unknown);
        assert_eq!(
            invalid_boundary.reason,
            Some(UnknownReason::DeliveryBaselineMismatch)
        );
    }

    #[test]
    fn origin_path_baseline_and_field_bindings_abstain_with_specific_reasons() {
        let base = || {
            input(
                ReferenceText::Present {
                    text: "VoiceFlow".into(),
                    source: ReferenceSource::Synthetic,
                },
                present("VoiceFlo"),
                present("VoiceFlo"),
                skipped(),
                DeliverySource::PassThrough(StageId::AsrOriginal),
                "VoiceFlo",
                "VoiceFlow",
            )
        };

        let mut mismatch = base();
        mismatch.asr.origin_id = "other-origin".into();
        assert_judgment(
            &mismatch,
            Verdict::Unknown,
            Some(UnknownReason::OriginMismatch),
        );

        let mut path_mismatch = base();
        path_mismatch.delivery.value.inserted_text = "VoiceFlow".into();
        path_mismatch.delivery.value.first_snapshot = "VoiceFlow".into();
        path_mismatch.delivery.value.inserted_range.end = "VoiceFlow".len();
        assert_judgment(
            &path_mismatch,
            Verdict::Unknown,
            Some(UnknownReason::DeliveryStageMismatch),
        );

        let mut baseline_mismatch = base();
        baseline_mismatch.delivery.value.first_snapshot = "Other".into();
        assert_judgment(
            &baseline_mismatch,
            Verdict::Unknown,
            Some(UnknownReason::DeliveryBaselineMismatch),
        );

        let mut partial_field = base();
        partial_field.delivery.value.first_snapshot = "Prefix VoiceFlo".into();
        partial_field.delivery.value.inserted_range = ByteRange { start: 7, end: 15 };
        assert_judgment(
            &partial_field,
            Verdict::Unknown,
            Some(UnknownReason::RangeUnsupported),
        );

        let mut malformed_slice = base();
        malformed_slice.delivery.value.first_snapshot = "Prefix VoiceFxl".into();
        malformed_slice.delivery.value.inserted_range = ByteRange { start: 7, end: 15 };
        assert_judgment(
            &malformed_slice,
            Verdict::Unknown,
            Some(UnknownReason::DeliveryBaselineMismatch),
        );

        let mut wrong_field = base();
        wrong_field.edit.value.field_id = "field-2".into();
        assert_judgment(
            &wrong_field,
            Verdict::Unknown,
            Some(UnknownReason::FieldMismatch),
        );
    }

    #[test]
    fn unsettled_untrusted_missing_stage_and_distinct_guard_output_abstain() {
        let base = || {
            input(
                ReferenceText::Present {
                    text: "VoiceFlow".into(),
                    source: ReferenceSource::Synthetic,
                },
                present("VoiceFlo"),
                present("VoiceFlo"),
                skipped(),
                DeliverySource::FallbackTo(StageId::LocalPreparation),
                "VoiceFlo",
                "VoiceFlow",
            )
        };

        let mut unsettled = base();
        unsettled.edit.value.settlement = EditSettlement::Unsettled;
        assert_judgment(
            &unsettled,
            Verdict::Unknown,
            Some(UnknownReason::EditUnsettled),
        );

        let mut untrusted = base();
        untrusted.edit.value.source_trust = EditSourceTrust::Untrusted;
        assert_judgment(
            &untrusted,
            Verdict::Unknown,
            Some(UnknownReason::EditSourceUntrusted),
        );

        let mut missing_stage = base();
        missing_stage.asr.value.text = StageText::Missing;
        assert_judgment(
            &missing_stage,
            Verdict::Unknown,
            Some(UnknownReason::StageMissing),
        );

        let mut guard_output = base();
        guard_output.delivery.value.source = DeliverySource::GuardOutput {
            source: StageId::LocalPreparation,
        };
        guard_output.delivery.value.guard_output = Some("VoiceFlo".into());
        assert_judgment(&guard_output, Verdict::AsrSubstitution, None);

        guard_output.delivery.value.guard_output = Some("VoiceFlow Pro".into());
        guard_output.delivery.value.inserted_text = "VoiceFlow Pro".into();
        guard_output.delivery.value.first_snapshot = "VoiceFlow Pro".into();
        guard_output.delivery.value.inserted_range.end = "VoiceFlow Pro".len();
        guard_output.edit.value.stable_text = "VoiceFlow".into();
        assert_judgment(
            &guard_output,
            Verdict::Unknown,
            Some(UnknownReason::DeliveryStageUnattributed),
        );
    }

    #[test]
    fn repeated_tokens_multiple_hunks_and_scope_mismatch_do_not_propose_learning() {
        let repeated_omission = input(
            ReferenceText::Present {
                text: "go go".into(),
                source: ReferenceSource::HumanReviewed,
            },
            present("go"),
            skipped(),
            skipped(),
            DeliverySource::PassThrough(StageId::AsrOriginal),
            "go",
            "go go",
        );
        let ambiguous_omission = judge(&repeated_omission);
        assert_eq!(ambiguous_omission.verdict, Verdict::Unknown);
        assert_eq!(
            ambiguous_omission.reason,
            Some(UnknownReason::AlignmentAmbiguous)
        );
        assert_eq!(
            ambiguous_omission.first_difference_stage,
            Some(DifferenceStage::AsrOriginal)
        );
        assert_eq!(ambiguous_omission.proposed_learning_pair, None);
        assert_eq!(single_hunk("go", "go go"), Err(DiffError::Ambiguous));
        assert_eq!(single_hunk("go go", "go"), Err(DiffError::Ambiguous));

        let repeated = input(
            ReferenceText::Present {
                text: "go go".into(),
                source: ReferenceSource::Synthetic,
            },
            present("go go"),
            present("go go"),
            present("go go"),
            DeliverySource::CleanupCandidate,
            "go go",
            "go go go",
        );
        assert_judgment(
            &repeated,
            Verdict::Unknown,
            Some(UnknownReason::AlignmentAmbiguous),
        );

        let multiple_hunks = input(
            ReferenceText::Present {
                text: "set red and set red".into(),
                source: ReferenceSource::Synthetic,
            },
            present("set red and set red"),
            present("set red and set red"),
            present("set red and set red"),
            DeliverySource::CleanupCandidate,
            "set red and set red",
            "set blue and set blue",
        );
        assert_judgment(
            &multiple_hunks,
            Verdict::Unknown,
            Some(UnknownReason::AlignmentAmbiguous),
        );

        let mut no_scope = input(
            ReferenceText::Present {
                text: "VoiceFlow".into(),
                source: ReferenceSource::Synthetic,
            },
            present("VoiceFlo"),
            present("VoiceFlo"),
            skipped(),
            DeliverySource::PassThrough(StageId::AsrOriginal),
            "VoiceFlo",
            "VoiceFlow",
        );
        no_scope.learning_scope = None;
        let judgment = judge(&no_scope);
        assert_eq!(judgment.verdict, Verdict::AsrSubstitution);
        assert_eq!(judgment.proposed_learning_pair, None);

        no_scope.learning_scope = Some(evidence("other-origin", LearningScope("x".into())));
        let judgment = judge(&no_scope);
        assert_eq!(judgment.verdict, Verdict::AsrSubstitution);
        assert_eq!(judgment.proposed_learning_pair, None);
    }

    #[test]
    fn local_rule_id_requires_one_exactly_aligned_change() {
        let mut value = input(
            ReferenceText::Present {
                text: "VoiceFlow".into(),
                source: ReferenceSource::Synthetic,
            },
            present("VoiceFlow"),
            present("VoiceFlo"),
            skipped(),
            DeliverySource::FallbackTo(StageId::LocalPreparation),
            "VoiceFlo",
            "VoiceFlow",
        );
        let hunk = single_hunk("VoiceFlow", "VoiceFlo").unwrap().unwrap();
        let exact = RuleChange {
            rule_id: "rule-1".into(),
            before: hunk.left_bytes.clone(),
            after: hunk.right_bytes.clone(),
        };
        value.local_preparation.value.rule_changes = vec![exact.clone()];
        let exact_judgment = judge(&value);
        assert_eq!(exact_judgment.verdict, Verdict::LocalPrepRegression);
        assert_eq!(exact_judgment.local_rule_id.as_deref(), Some("rule-1"));

        value.local_preparation.value.rule_changes = vec![
            exact.clone(),
            RuleChange {
                rule_id: "rule-2".into(),
                before: hunk.left_bytes,
                after: hunk.right_bytes,
            },
        ];
        let multiple = judge(&value);
        assert_eq!(multiple.verdict, Verdict::LocalPrepRegression);
        assert_eq!(multiple.local_rule_id, None);

        let mut contextual = input(
            ReferenceText::Present {
                text: "Use VoiceFlow".into(),
                source: ReferenceSource::Synthetic,
            },
            present("Use VoiceFlow"),
            present("Use VoiceFlo"),
            skipped(),
            DeliverySource::FallbackTo(StageId::LocalPreparation),
            "Use VoiceFlo",
            "Use VoiceFlow",
        );
        contextual.local_preparation.value.rule_changes = vec![RuleChange {
            rule_id: "rule-4".into(),
            before: ByteRange { start: 4, end: 13 },
            after: ByteRange { start: 4, end: 12 },
        }];
        let contextual_judgment = judge(&contextual);
        assert_eq!(contextual_judgment.verdict, Verdict::LocalPrepRegression);
        assert_eq!(contextual_judgment.local_rule_id.as_deref(), Some("rule-4"));
    }

    fn assert_judgment(input: &AttributionInput, verdict: Verdict, reason: Option<UnknownReason>) {
        let judgment = judge(input);
        assert_eq!(judgment.verdict, verdict);
        assert_eq!(judgment.reason, reason);
    }
}
