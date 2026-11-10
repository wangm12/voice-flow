//! Fixture-selected learning observations. No ordinary launch retains a record.
//! Text authority is declared in the lease; receipts identify fixtures, not text.
#[cfg(any(test, feature = "research-harness"))]
use crate::{research_trace, store};
use serde::{Deserialize, Serialize};
#[cfg(test)]
pub(crate) mod edit_attribution_tests;
#[cfg(all(feature = "research-harness", target_os = "macos"))]
#[path = "learning_native_oracle.rs"]
mod learning_native_oracle;
#[cfg(all(feature = "research-harness", target_os = "macos"))]
#[path = "learning_native_paste_session.rs"]
mod learning_native_paste_session;
#[cfg(any(test, feature = "research-harness"))]
#[path = "learning_phase_v3.rs"]
mod learning_phase_v3;
#[cfg(all(feature = "research-harness", target_os = "macos"))]
pub(crate) use learning_phase_v3::OwnedPasteSessionBindingV1;
#[cfg(any(test, feature = "research-harness"))]
use std::cell::RefCell;
#[cfg(any(test, feature = "research-harness"))]
use std::path::{Path, PathBuf};

#[cfg(any(test, feature = "research-harness"))]
const LEARNING_EPISODE_SCHEMA_V2: u8 = 2;
#[cfg(any(test, feature = "research-harness"))]
const LEARNING_ARM_SCHEMA_V2: u8 = 2;
#[cfg(any(test, feature = "research-harness"))]
const LEARNING_TEXT_MAX_BYTES: usize = 1024;
#[cfg(any(test, feature = "research-harness"))]
const LEARNING_OUTPUT_MAX_BYTES: usize = 1024;
#[cfg(any(test, feature = "research-harness"))]
const LEARNING_TERMS_MAX: usize = 8;
#[cfg(any(test, feature = "research-harness"))]
const LEARNING_OPERATIONS_MAX: usize = 8;
#[cfg(any(test, feature = "research-harness"))]
const LEARNING_EDIT_STATES_MAX: usize = 6;
#[cfg(any(test, feature = "research-harness"))]
const LEARNING_FOLLOWUPS_MAX: usize = 2;
#[cfg(any(test, feature = "research-harness"))]
const LEARNING_WAV_OPERATIONS_MAX: usize = 1;
#[cfg(any(test, feature = "research-harness"))]
const LEARNING_WAV_MAX_BYTES: usize = 30_000_000;
#[cfg(any(test, feature = "research-harness"))]
const LEARNING_LEASE_MIN_MS: u64 = 1_000;
#[cfg(any(test, feature = "research-harness"))]
const LEARNING_LEASE_MAX_MS: u64 = 1_200_000;
#[cfg(feature = "research-harness")]
const LEARNING_STDIN_MAX_BYTES: usize = 8_192;
#[cfg(feature = "research-harness")]
const LEARNING_REQUEST_MAX: usize = 256;

#[cfg(any(test, feature = "research-harness"))]
thread_local! {
    static SCOPED_CONSENT: RefCell<Option<LearningConsent>> = const { RefCell::new(None) };
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TermFixture {
    pub fixture_id: String,
    pub before: String,
    pub after: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LearningConsent {
    pub selected_state_id: String,
    pub baseline_run: Option<String>,
    pub edit_fixture_id: String,
    /// Provenance is caller-declared; it is never a measured quality verdict.
    pub purpose: LearningPurpose,
    pub mapping_id: String,
    pub bundle_id: String,
    pub observer_enabled: bool,
    pub post_insert_field: String,
    pub settled_field: String,
    pub terms: Vec<TermFixture>,
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LearningPurpose {
    Correction,
    Baseline,
    Unseen,
    Negative,
    Undo,
    Delete,
    Restart,
}

/// Explicit source and edit authority for the standalone controlled-learning
/// protocol. Text bodies arrive only in `LearningTextOperation`; receipts and
/// arm manifests retain hashes, IDs, and fixed outcome codes.
#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LearningEpisodeV2 {
    pub schema_version: u8,
    pub ttl_ms: u64,
    pub episode_id: String,
    pub arm_state_id: String,
    pub selected_state_id: String,
    pub baseline_run: Option<String>,
    pub edit_fixture_id: String,
    pub purpose: LearningPurpose,
    pub source: LearningSourceV2,
    pub source_text_learning_authorized: bool,
    pub learning_authorized: bool,
    pub actor: LearningActorV2,
    pub authorized_text_layers: Vec<LearningTextLayerV2>,
    pub edit_states: Vec<LearningEditStateV2>,
    pub scope: LearningScopeV2,
    pub local_only_grant: bool,
    pub target_oracle: LearningOracleV2,
    pub terms: Vec<TermFixture>,
    pub operations: Vec<LearningOperationV2>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub followups: Vec<LearningFollowupV2>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub undo_operations: Vec<LearningUndoOperationV2>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wav_operations: Vec<LearningWavOperationV2>,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LearningFollowupV2 {
    pub task_id: String,
    pub operation_id: String,
    pub source: LearningSourceV2,
    pub scope: LearningScopeV2,
    pub term_fixture_id: String,
    pub expected_rule_state: LearningRuleStateV2,
    pub expected_output_sha256: String,
    pub expected_output_byte_length: usize,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LearningUndoOperationV2 {
    pub operation_id: String,
    pub term_fixture_id: String,
    pub required_followup_task_id: String,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LearningWavOperationV2 {
    pub task_id: String,
    pub operation_id: String,
    pub source: LearningWavSourceV2,
    pub model_id: String,
    pub language: Option<String>,
    pub learning_authorized: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_raw_asr: Option<LearningRawAsrIdentityV2>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<LearningWavEditV2>,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LearningWavSourceV2 {
    pub fixture_id: String,
    pub relative_path: String,
    pub sha256: String,
    pub byte_length: usize,
    pub rights: LearningRightsV2,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LearningRawAsrIdentityV2 {
    pub sha256: String,
    pub byte_length: usize,
    pub provenance: LearningRawAsrProvenanceV2,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LearningRawAsrProvenanceV2 {
    DerivedFromObservedAsr,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LearningWavEditV2 {
    pub term_fixture_id: String,
    pub settled_text_sha256: String,
    pub settled_text_byte_length: usize,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LearningRuleStateV2 {
    Promoted,
    Tombstoned,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LearningSourceV2 {
    pub kind: LearningSourceKindV2,
    pub fixture_id: String,
    pub sha256: String,
    pub byte_length: usize,
    /// Caller-declared provenance. This does not establish transcription or
    /// model quality and is not a rights review.
    pub rights: LearningRightsV2,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LearningSourceKindV2 {
    DeterministicText,
    OwnedPublicWav,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LearningRightsV2 {
    Synthetic,
    UserOwned,
    UserLicensed,
    PublicCcBySa4_0,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LearningActorV2 {
    ProgrammaticFixture,
    Human,
    Unknown,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LearningTextLayerV2 {
    Source,
    PostInsert,
    Settled,
    RawAsr,
    Prepared,
    ProviderCandidate,
    FinalOutput,
    TargetReadback,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LearningOracleV2 {
    None,
    ProductReadback,
    IndependentTarget,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LearningEditStateV2 {
    pub sequence: u8,
    pub state_id: String,
    pub layer: LearningTextLayerV2,
    pub sha256: String,
    pub byte_length: usize,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct LearningScopeV2 {
    pub mapping_id: String,
    pub family: crate::context::ContextFamily,
    pub native_bundle: String,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LearningOperationV2 {
    pub operation_id: String,
    pub source_sha256: String,
    pub settled_state_sha256: String,
    pub term_fixture_id: String,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LearningTextStateInputV2 {
    pub sequence: u8,
    pub state_id: String,
    pub layer: LearningTextLayerV2,
    pub text: String,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LearningTextOperationV2 {
    pub episode_id: String,
    pub operation_id: String,
    pub source_fixture_id: String,
    pub states: Vec<LearningTextStateInputV2>,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Clone, Serialize)]
pub(crate) struct LearningOperationReceiptV2 {
    pub schema_version: u8,
    pub episode_id: String,
    pub arm_state_id: String,
    pub operation_id: String,
    pub source_fixture_id: String,
    pub source_kind: LearningSourceKindV2,
    pub source_sha256: String,
    pub source_byte_length: usize,
    pub rights: LearningRightsV2,
    pub actor: LearningActorV2,
    pub admission: &'static str,
    pub observer_mode: &'static str,
    pub observer_outcome: &'static str,
    pub trajectory_samples_observed: usize,
    pub trajectory_states_consumed: usize,
    pub trajectory_settled: bool,
    pub candidate_class: Option<&'static str>,
    pub term_fixture_id: Option<String>,
    pub hits_before: Option<u32>,
    pub hits_after: Option<u32>,
    pub promote_threshold: Option<u32>,
    pub promoted: Option<bool>,
    pub sql_readback: &'static str,
    pub candidate_write: &'static str,
    pub arm_settings_readback: &'static str,
    pub insertion: &'static str,
    pub ax_observation: &'static str,
    pub independent_target_oracle: &'static str,
    pub asr_quality: &'static str,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Serialize)]
pub(crate) struct LearningFollowupReceiptV2 {
    pub schema_version: u8,
    pub episode_id: String,
    pub arm_state_id: String,
    pub task_id: String,
    pub operation_id: String,
    pub term_fixture_id: String,
    pub source_fixture_id: String,
    pub source_sha256: String,
    pub source_byte_length: usize,
    pub rights: LearningRightsV2,
    pub expected_rule_state: LearningRuleStateV2,
    pub observed_rule_state: &'static str,
    pub application_owner: &'static str,
    pub output_text: String,
    pub output_sha256: String,
    pub output_byte_length: usize,
    pub expected_output_sha256: String,
    pub expected_output_byte_length: usize,
    pub expected_output_match: bool,
    pub applied_pairs: Vec<LearningAppliedPairV2>,
    pub selected_pair_attribution: &'static str,
    pub usage_receipt: &'static str,
    pub pipeline_final: &'static str,
    pub asr_quality: &'static str,
    pub insertion: &'static str,
    pub ax_observation: &'static str,
    pub independent_target_oracle: &'static str,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Serialize)]
pub(crate) struct LearningAppliedPairV2 {
    pub before: String,
    pub after: String,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Serialize)]
pub(crate) struct LearningUndoReceiptV2 {
    pub schema_version: u8,
    pub episode_id: String,
    pub arm_state_id: String,
    pub operation_id: String,
    pub required_followup_task_id: String,
    pub term_fixture_id: String,
    pub state_before: &'static str,
    pub state_after: &'static str,
    pub sql_outcome: &'static str,
    pub settings_readback: &'static str,
    pub dictionary_entry_policy: &'static str,
    pub pipeline_final: &'static str,
    pub insertion: &'static str,
    pub ax_observation: &'static str,
    pub independent_target_oracle: &'static str,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Serialize)]
pub(crate) struct LearningWavReceiptV2 {
    pub schema_version: u8,
    pub episode_id: String,
    pub arm_state_id: String,
    pub task_id: String,
    pub operation_id: String,
    pub replay_policy: &'static str,
    pub source_fixture_id: String,
    pub source_sha256: String,
    pub source_byte_length: usize,
    pub source_rights: LearningRightsV2,
    pub source_origin_policy: &'static str,
    pub source_path_disclosed: bool,
    pub model_id: String,
    pub model_revision: String,
    pub model_manifest_sha256: String,
    pub model_file_count: usize,
    pub transcript_owner: &'static str,
    pub loaded_model_id: Option<String>,
    pub loaded_model_revision: Option<String>,
    pub runtime_error: Option<String>,
    pub raw_asr_text: Option<String>,
    pub raw_asr_sha256: Option<String>,
    pub raw_asr_byte_length: Option<usize>,
    pub asr_error_category: Option<&'static str>,
    pub asr_error_detail: Option<String>,
    pub expected_raw_asr_sha256: Option<String>,
    pub expected_raw_asr_byte_length: Option<usize>,
    pub fixture_prediction_provenance: Option<LearningRawAsrProvenanceV2>,
    pub fixture_prediction_match: Option<bool>,
    pub runtime_status: String,
    pub runtime_attestation: &'static str,
    pub asr_outcome: &'static str,
    pub learning_outcome: &'static str,
    pub term_fixture_id: Option<String>,
    pub observer_outcome: Option<&'static str>,
    pub candidate_class: Option<&'static str>,
    pub hits_before: Option<u32>,
    pub hits_after: Option<u32>,
    pub promote_threshold: Option<u32>,
    pub promoted: Option<bool>,
    pub sql_readback: &'static str,
    pub candidate_write: &'static str,
    pub settings_readback: &'static str,
    pub insertion: &'static str,
    pub ax_observation: &'static str,
    pub independent_target_oracle: &'static str,
    pub asr_quality: &'static str,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Serialize)]
pub(crate) struct LearningArmSnapshotV2 {
    pub schema_version: u8,
    pub arm_state_id: String,
    pub identity_sha256: String,
    pub settings_sha256: String,
    pub settings_consistency: &'static str,
    pub pairs: Vec<LearningPairSnapshotV2>,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Serialize)]
pub(crate) struct LearningPairSnapshotV2 {
    pub term_fixture_id: String,
    pub hits: u32,
    pub promoted: bool,
    pub tombstoned: bool,
    pub ignored: bool,
    pub state: &'static str,
    pub row_sha256: String,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LearningArmManifestV2 {
    artifact_type: String,
    schema_version: u8,
    arm_state_id: String,
    identity_sha256: String,
}

#[cfg(any(test, feature = "research-harness"))]
pub(crate) struct LearningWavResources<'a> {
    wav_root: &'a Path,
    model_source_root: &'a Path,
    expires: std::time::Instant,
}

/// A separately rooted research profile. It deliberately owns only settings
/// created from `Settings::default()` plus the caller-declared fixture scope;
/// it never loads ordinary app settings, History, or credentials.
#[cfg(any(test, feature = "research-harness"))]
pub(crate) struct LearningArmV2 {
    root: PathBuf,
    identity_sha256: String,
    settings: store::Settings,
}

#[cfg(any(test, feature = "research-harness"))]
impl LearningEpisodeV2 {
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != LEARNING_EPISODE_SCHEMA_V2 {
            return Err("learning_episode_version_unsupported");
        }
        if !(LEARNING_LEASE_MIN_MS..=LEARNING_LEASE_MAX_MS).contains(&self.ttl_ms) {
            return Err("learning_episode_expiry_invalid");
        }
        for id in [
            &self.episode_id,
            &self.arm_state_id,
            &self.selected_state_id,
            &self.edit_fixture_id,
            &self.source.fixture_id,
        ] {
            crate::onboarding::validate_id(id).map_err(|_| "learning_episode_invalid_uuid")?;
        }
        if let Some(id) = &self.baseline_run {
            crate::onboarding::validate_id(id).map_err(|_| "learning_episode_invalid_uuid")?;
        }
        if !self.source_text_learning_authorized || !self.learning_authorized {
            return Err("learning_source_or_learning_grant_required");
        }
        if self.source.kind != LearningSourceKindV2::DeterministicText {
            return Err("unsupported_learning_source");
        }
        if self.actor != LearningActorV2::ProgrammaticFixture {
            return Err("unsupported_learning_actor");
        }
        if self.target_oracle != LearningOracleV2::None {
            return Err("unsupported_target_oracle");
        }
        if !self.local_only_grant {
            return Err("local_only_learning_grant_required");
        }
        if self.source.rights == LearningRightsV2::PublicCcBySa4_0 {
            return Err("text_source_rights_mismatch");
        }
        if self.source.byte_length == 0
            || self.source.byte_length > LEARNING_TEXT_MAX_BYTES
            || !valid_sha256(&self.source.sha256)
        {
            return Err("learning_source_bounds");
        }
        if self.scope.mapping_id.is_empty()
            || self.scope.mapping_id.len() > 128
            || self.scope.native_bundle.is_empty()
            || self.scope.native_bundle.len() > 256
            || crate::context::is_browser_application(Some(&self.scope.native_bundle))
        {
            return Err("learning_scope_bounds_or_unsupported");
        }
        let expected_layer_count = 3
            + usize::from(!self.followups.is_empty())
            + usize::from(!self.wav_operations.is_empty());
        let layers_valid = self.authorized_text_layers.len() == expected_layer_count
            && self
                .authorized_text_layers
                .contains(&LearningTextLayerV2::FinalOutput)
                == !self.followups.is_empty()
            && self
                .authorized_text_layers
                .contains(&LearningTextLayerV2::RawAsr)
                == !self.wav_operations.is_empty();
        if !layers_valid
            || !self
                .authorized_text_layers
                .contains(&LearningTextLayerV2::Source)
            || !self
                .authorized_text_layers
                .contains(&LearningTextLayerV2::PostInsert)
            || !self
                .authorized_text_layers
                .contains(&LearningTextLayerV2::Settled)
        {
            return Err("unsupported_text_layer_authority");
        }
        if !(2..=LEARNING_EDIT_STATES_MAX).contains(&self.edit_states.len()) {
            return Err("unsupported_edit_state_count");
        }
        let post_insert = self
            .edit_states
            .first()
            .expect("minimum edit states checked");
        let settled = self
            .edit_states
            .last()
            .expect("minimum edit states checked");
        let mut state_ids = std::collections::HashSet::new();
        for (sequence, state) in self.edit_states.iter().enumerate() {
            crate::onboarding::validate_id(&state.state_id)
                .map_err(|_| "learning_episode_invalid_uuid")?;
            if !state_ids.insert(state.state_id.as_str()) {
                return Err("learning_edit_state_id_reused");
            }
            let expected_layer = if sequence == 0 {
                LearningTextLayerV2::PostInsert
            } else {
                LearningTextLayerV2::Settled
            };
            if state.sequence as usize != sequence
                || state.layer != expected_layer
                || state.byte_length == 0
                || state.byte_length > LEARNING_TEXT_MAX_BYTES
                || !valid_sha256(&state.sha256)
            {
                return Err("learning_edit_state_contract_invalid");
            }
        }
        if post_insert.sha256 != self.source.sha256
            || post_insert.byte_length != self.source.byte_length
        {
            return Err("learning_edit_state_contract_invalid");
        }
        if self.terms.is_empty() || self.terms.len() > LEARNING_TERMS_MAX {
            return Err("learning_terms_bounds");
        }
        let mut term_ids = std::collections::HashSet::new();
        let mut pairs = std::collections::HashSet::new();
        for term in &self.terms {
            crate::onboarding::validate_id(&term.fixture_id)
                .map_err(|_| "learning_episode_invalid_uuid")?;
            if !term_ids.insert(&term.fixture_id)
                || !pairs.insert(crate::dictionary_learn::pair_key(&term.before, &term.after))
                || term.before.is_empty()
                || term.after.is_empty()
                || term.before.len() > 256
                || term.after.len() > 256
                || term.before.chars().any(char::is_control)
                || term.after.chars().any(char::is_control)
            {
                return Err("learning_terms_invalid_or_duplicate");
            }
        }
        if self.operations.is_empty() || self.operations.len() > LEARNING_OPERATIONS_MAX {
            return Err("learning_operations_bounds");
        }
        let mut operation_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut source_ids = std::collections::HashSet::from([self.source.fixture_id.as_str()]);
        for operation in &self.operations {
            crate::onboarding::validate_id(&operation.operation_id)
                .map_err(|_| "learning_episode_invalid_uuid")?;
            crate::onboarding::validate_id(&operation.term_fixture_id)
                .map_err(|_| "learning_episode_invalid_uuid")?;
            if !operation_ids.insert(operation.operation_id.as_str())
                || operation.source_sha256 != self.source.sha256
                || operation.settled_state_sha256 != settled.sha256
                || !self
                    .terms
                    .iter()
                    .any(|term| term.fixture_id == operation.term_fixture_id)
            {
                return Err("learning_operation_contract_invalid");
            }
        }
        if self.followups.len() > LEARNING_FOLLOWUPS_MAX
            || self.wav_operations.len() > LEARNING_WAV_OPERATIONS_MAX
            || self.undo_operations.len() > 1
            || self.operations.len()
                + self.followups.len()
                + self.undo_operations.len()
                + self.wav_operations.len()
                > LEARNING_OPERATIONS_MAX
        {
            return Err("learning_followup_bounds");
        }
        let mut task_ids = std::collections::HashSet::new();
        for followup in &self.followups {
            crate::onboarding::validate_id(&followup.task_id)
                .map_err(|_| "learning_episode_invalid_uuid")?;
            crate::onboarding::validate_id(&followup.operation_id)
                .map_err(|_| "learning_episode_invalid_uuid")?;
            crate::onboarding::validate_id(&followup.source.fixture_id)
                .map_err(|_| "learning_episode_invalid_uuid")?;
            crate::onboarding::validate_id(&followup.term_fixture_id)
                .map_err(|_| "learning_episode_invalid_uuid")?;
            if !task_ids.insert(followup.task_id.as_str())
                || !operation_ids.insert(followup.operation_id.as_str())
                || !source_ids.insert(followup.source.fixture_id.as_str())
                || followup.source.kind != LearningSourceKindV2::DeterministicText
                || followup.source.rights == LearningRightsV2::PublicCcBySa4_0
                || followup.source.byte_length == 0
                || followup.source.byte_length > LEARNING_TEXT_MAX_BYTES
                || !valid_sha256(&followup.source.sha256)
                || followup.scope != self.scope
                || !self
                    .terms
                    .iter()
                    .any(|term| term.fixture_id == followup.term_fixture_id)
                || followup.expected_output_byte_length == 0
                || followup.expected_output_byte_length > LEARNING_OUTPUT_MAX_BYTES
                || !valid_sha256(&followup.expected_output_sha256)
            {
                return Err("learning_followup_contract_invalid");
            }
        }
        for undo in &self.undo_operations {
            crate::onboarding::validate_id(&undo.operation_id)
                .map_err(|_| "learning_episode_invalid_uuid")?;
            crate::onboarding::validate_id(&undo.term_fixture_id)
                .map_err(|_| "learning_episode_invalid_uuid")?;
            crate::onboarding::validate_id(&undo.required_followup_task_id)
                .map_err(|_| "learning_episode_invalid_uuid")?;
            if !operation_ids.insert(undo.operation_id.as_str())
                || !self
                    .terms
                    .iter()
                    .any(|term| term.fixture_id == undo.term_fixture_id)
                || !self.followups.iter().any(|followup| {
                    followup.task_id == undo.required_followup_task_id
                        && followup.term_fixture_id == undo.term_fixture_id
                })
            {
                return Err("learning_undo_contract_invalid");
            }
        }
        for wav in &self.wav_operations {
            crate::onboarding::validate_id(&wav.task_id)
                .map_err(|_| "learning_episode_invalid_uuid")?;
            crate::onboarding::validate_id(&wav.operation_id)
                .map_err(|_| "learning_episode_invalid_uuid")?;
            crate::onboarding::validate_id(&wav.source.fixture_id)
                .map_err(|_| "learning_episode_invalid_uuid")?;
            if !task_ids.insert(wav.task_id.as_str())
                || !operation_ids.insert(wav.operation_id.as_str())
                || !source_ids.insert(wav.source.fixture_id.as_str())
                || !valid_learning_wav_relative_path(&wav.source.relative_path)
                || !valid_sha256(&wav.source.sha256)
                || wav.source.byte_length == 0
                || wav.source.byte_length > LEARNING_WAV_MAX_BYTES
                || wav.source.rights != LearningRightsV2::PublicCcBySa4_0
                || wav.model_id != crate::ondevice_models::QWEN_ASR_06_ID
                || wav
                    .language
                    .as_deref()
                    .is_some_and(|language| !matches!(language, "zh" | "en"))
            {
                return Err("learning_wav_source_contract_invalid");
            }
            if let Some(expected) = &wav.expected_raw_asr {
                if expected.byte_length == 0
                    || expected.byte_length > LEARNING_TEXT_MAX_BYTES
                    || !valid_sha256(&expected.sha256)
                {
                    return Err("learning_raw_asr_identity_invalid");
                }
            }
            if let Some(edit) = &wav.edit {
                crate::onboarding::validate_id(&edit.term_fixture_id)
                    .map_err(|_| "learning_episode_invalid_uuid")?;
                if !wav.learning_authorized
                    || wav.expected_raw_asr.is_none()
                    || !self
                        .terms
                        .iter()
                        .any(|term| term.fixture_id == edit.term_fixture_id)
                    || edit.settled_text_byte_length == 0
                    || edit.settled_text_byte_length > LEARNING_TEXT_MAX_BYTES
                    || !valid_sha256(&edit.settled_text_sha256)
                {
                    return Err("learning_wav_edit_contract_invalid");
                }
            } else if wav.learning_authorized {
                return Err("learning_wav_grant_without_edit");
            }
        }
        match (self.followups.as_slice(), self.undo_operations.as_slice()) {
            ([], []) => {
                if self
                    .authorized_text_layers
                    .contains(&LearningTextLayerV2::FinalOutput)
                {
                    return Err("unsupported_text_layer_authority");
                }
            }
            ([one], []) => {
                if one.expected_rule_state != LearningRuleStateV2::Promoted {
                    return Err("learning_followup_order_invalid");
                }
            }
            ([first, second], [undo]) => {
                if first.expected_rule_state != LearningRuleStateV2::Promoted
                    || second.expected_rule_state != LearningRuleStateV2::Tombstoned
                    || first.term_fixture_id != undo.term_fixture_id
                    || second.term_fixture_id != undo.term_fixture_id
                    || undo.required_followup_task_id != first.task_id
                    || !self
                        .authorized_text_layers
                        .contains(&LearningTextLayerV2::FinalOutput)
                {
                    return Err("learning_followup_order_invalid");
                }
            }
            _ => return Err("learning_followup_order_invalid"),
        }
        Ok(())
    }

    fn identity_sha256(&self) -> anyhow::Result<String> {
        use sha2::{Digest, Sha256};
        let identity = serde_json::to_vec(self)?;
        Ok(hex::encode(Sha256::digest(identity)))
    }

    fn term(&self, fixture_id: &str) -> Option<&TermFixture> {
        self.terms.iter().find(|term| term.fixture_id == fixture_id)
    }

    fn operation(&self, operation_id: &str) -> Option<&LearningOperationV2> {
        self.operations
            .iter()
            .find(|operation| operation.operation_id == operation_id)
    }

    fn followup(&self, task_id: &str) -> Option<&LearningFollowupV2> {
        self.followups
            .iter()
            .find(|followup| followup.task_id == task_id)
    }

    fn undo_operation(&self, operation_id: &str) -> Option<&LearningUndoOperationV2> {
        self.undo_operations
            .iter()
            .find(|operation| operation.operation_id == operation_id)
    }

    fn wav_operation(&self, task_id: &str) -> Option<&LearningWavOperationV2> {
        self.wav_operations
            .iter()
            .find(|operation| operation.task_id == task_id)
    }
}

#[cfg(any(test, feature = "research-harness"))]
impl LearningArmV2 {
    pub(crate) fn open(base_root: &Path, episode: &LearningEpisodeV2) -> anyhow::Result<Self> {
        episode.validate().map_err(anyhow::Error::msg)?;
        let base_root = validate_arm_base_root(base_root)?;
        let identity_sha256 = episode.identity_sha256()?;
        let arms_root = base_root.join("learning-arms-v2");
        create_private_directory(&arms_root, false)?;
        validate_private_directory(&arms_root)?;
        let root = arms_root.join(&episode.arm_state_id);
        let existed = root.exists();
        if existed {
            validate_private_directory(&root)?;
        } else {
            create_private_directory(&root, true)?;
        }
        validate_owned_arm_storage(&root)?;
        let manifest_path = root.join("arm.json");
        let settings_path = root.join("settings.json");
        let settings = if manifest_path.exists() {
            let manifest: LearningArmManifestV2 = read_private_json(&manifest_path)?;
            if manifest.artifact_type != "voiceflow_learning_arm"
                || manifest.schema_version != LEARNING_ARM_SCHEMA_V2
                || manifest.arm_state_id != episode.arm_state_id
                || manifest.identity_sha256 != identity_sha256
            {
                anyhow::bail!("learning arm identity mismatch");
            }
            let settings: store::Settings = read_private_json(&settings_path)?;
            validate_arm_settings(&settings, episode)?;
            settings
        } else {
            if std::fs::read_dir(&root)?.next().is_some() {
                anyhow::bail!("unowned files exist in learning arm root");
            }
            let settings = initial_arm_settings(episode);
            store::save_configuration(&root, &settings)?;
            let readback: store::Settings = read_private_json(&settings_path)?;
            validate_arm_settings(&readback, episode)?;
            let manifest = LearningArmManifestV2 {
                artifact_type: "voiceflow_learning_arm".into(),
                schema_version: LEARNING_ARM_SCHEMA_V2,
                arm_state_id: episode.arm_state_id.clone(),
                identity_sha256: identity_sha256.clone(),
            };
            write_private_json(&manifest_path, &manifest)?;
            readback
        };
        validate_owned_arm_storage(&root)?;
        Ok(Self {
            root,
            identity_sha256,
            settings,
        })
    }

    pub(crate) fn identity_sha256(&self) -> &str {
        &self.identity_sha256
    }

    pub(crate) fn selected_state(
        &self,
        episode: &LearningEpisodeV2,
    ) -> anyhow::Result<LearningArmSnapshotV2> {
        use sha2::{Digest, Sha256};

        validate_owned_arm_storage(&self.root)?;
        let mut pairs = Vec::with_capacity(episode.terms.len());
        let mut rows = Vec::with_capacity(episode.terms.len());
        let mut has_unsupported_promoted_ownership = false;
        for term in &episode.terms {
            validate_owned_arm_storage(&self.root)?;
            let row = store::get_learn_pair(
                &self.root,
                &crate::dictionary_learn::pair_key(&term.before, &term.after),
            )?;
            if row.as_ref().is_some_and(|row| {
                row.before_surface != term.before
                    || row.after_surface != term.after
                    || !row_within_bounds(row)
                    || !row_within_episode_scope(row, &episode.scope)
            }) {
                anyhow::bail!("selected learning row identity mismatch");
            }
            if row
                .as_ref()
                .is_some_and(|row| row.is_live_promoted() && !row.auto_scoped)
            {
                has_unsupported_promoted_ownership = true;
            }
            let row_json = serde_json::to_vec(&row)?;
            let state = row.as_ref().map_or("absent", |row| {
                if row.tombstoned_at.is_some() {
                    "tombstoned"
                } else if row.ignored {
                    "ignored"
                } else if row.is_live_promoted() {
                    "promoted"
                } else {
                    "pending"
                }
            });
            pairs.push(LearningPairSnapshotV2 {
                term_fixture_id: term.fixture_id.clone(),
                hits: row.as_ref().map_or(0, |row| row.hits),
                promoted: row
                    .as_ref()
                    .is_some_and(store::LearnPairRecord::is_live_promoted),
                tombstoned: row.as_ref().is_some_and(|row| row.tombstoned_at.is_some()),
                ignored: row.as_ref().is_some_and(|row| row.ignored),
                state,
                row_sha256: hex::encode(Sha256::digest(row_json)),
            });
            rows.push(row);
        }
        if matches!(episode.purpose, LearningPurpose::Baseline)
            && (!self.settings.dictionary.is_empty() || rows.iter().any(Option::is_some))
        {
            anyhow::bail!("baseline requires fresh arm state; no state is cleared");
        }
        let settings = read_private_bytes(&self.root.join("settings.json"))?;
        Ok(LearningArmSnapshotV2 {
            schema_version: LEARNING_ARM_SCHEMA_V2,
            arm_state_id: episode.arm_state_id.clone(),
            identity_sha256: self.identity_sha256.clone(),
            settings_sha256: hex::encode(Sha256::digest(settings)),
            settings_consistency: if has_unsupported_promoted_ownership {
                "promoted_rule_ownership_unsupported"
            } else {
                "selected_local_only_settings_unchanged"
            },
            pairs,
        })
    }

    pub(crate) fn execute_followup(
        &mut self,
        episode: &LearningEpisodeV2,
        task_id: &str,
        operation_id: &str,
        text: &str,
    ) -> anyhow::Result<LearningFollowupReceiptV2> {
        use sha2::{Digest, Sha256};

        episode.validate().map_err(anyhow::Error::msg)?;
        validate_owned_arm_storage(&self.root)?;
        let followup = episode
            .followup(task_id)
            .ok_or_else(|| anyhow::anyhow!("learning_followup_not_authorized"))?;
        if followup.operation_id != operation_id {
            anyhow::bail!("learning_operation_not_authorized");
        }
        if text.is_empty()
            || text.len() != followup.source.byte_length
            || hex::encode(Sha256::digest(text.as_bytes())) != followup.source.sha256
        {
            anyhow::bail!("learning_followup_source_mismatch");
        }
        let term = episode
            .term(&followup.term_fixture_id)
            .ok_or_else(|| anyhow::anyhow!("learning_term_fixture_mismatch"))?;
        let key = crate::dictionary_learn::pair_key(&term.before, &term.after);
        let row = store::get_learn_pair(&self.root, &key)?
            .ok_or_else(|| anyhow::anyhow!("selected_learning_pair_unavailable"))?;
        if row.before_surface != term.before
            || row.after_surface != term.after
            || !row_within_bounds(&row)
            || !row_within_episode_scope(&row, &episode.scope)
        {
            anyhow::bail!("selected_learning_pair_identity_mismatch");
        }
        let observed_state = learning_pair_state(&row);
        let expected_state = match followup.expected_rule_state {
            LearningRuleStateV2::Promoted => "promoted",
            LearningRuleStateV2::Tombstoned => "tombstoned",
        };
        if observed_state != expected_state {
            anyhow::bail!("learning_followup_rule_state_mismatch");
        }
        let state = self.selected_state(episode)?;
        if state.settings_consistency == "promoted_rule_ownership_unsupported" {
            anyhow::bail!("learning_global_rule_ownership_unsupported");
        }

        let applicable = if followup.expected_rule_state == LearningRuleStateV2::Promoted {
            let applicable = store::LearnPairScope {
                family: Some(crate::context::family_id(episode.scope.family).to_owned()),
                mapping_id: Some(episode.scope.mapping_id.clone()),
                browser_host: None,
                native_bundle: Some(episode.scope.native_bundle.clone()),
            }
            .applicable_promoted_records(std::slice::from_ref(&row));
            if applicable.len() != 1 {
                anyhow::bail!("learning_followup_pair_not_applicable");
            }
            applicable
        } else {
            Vec::new()
        };
        let replacement_pairs =
            crate::lexicon::replaceable_pairs(&applicable, &self.settings.dictionary);
        let (output_text, _, applied_pairs) =
            crate::lexicon::apply_explicit_replacements_with_facts(
                text,
                &replacement_pairs,
                &self.settings.dictionary,
            );
        if output_text.len() > LEARNING_OUTPUT_MAX_BYTES {
            anyhow::bail!("learning_followup_output_bounds");
        }
        let output_sha256 = hex::encode(Sha256::digest(output_text.as_bytes()));
        let expected_output_match = output_text.len() == followup.expected_output_byte_length
            && output_sha256 == followup.expected_output_sha256;
        let selected_pair_applied = applied_pairs
            .iter()
            .any(|pair| pair.before == term.before && pair.after == term.after);
        let output_byte_length = output_text.len();
        let selected_pair_attribution = if selected_pair_applied {
            "selected_learning_pair"
        } else if expected_output_match && output_text == text {
            "no_pair_applied"
        } else {
            "unknown"
        };
        let observation = store::LearnPairObservation::research_fixture_apply(
            &format!(
                "{}:{}:{}",
                episode.episode_id, followup.operation_id, followup.source.sha256
            ),
            &key,
        );
        validate_owned_arm_storage(&self.root)?;
        let inserted = store::record_research_fixture_application(
            &self.root,
            &key,
            &term.after,
            &observation,
            selected_pair_applied,
        )?;
        let usage_receipt = if selected_pair_applied {
            if inserted {
                "selected_pair_usage_recorded"
            } else {
                "duplicate_operation_no_usage_increment"
            }
        } else if inserted {
            "operation_recorded_without_selected_pair_usage"
        } else {
            "duplicate_operation_without_usage_increment"
        };
        Ok(LearningFollowupReceiptV2 {
            schema_version: LEARNING_EPISODE_SCHEMA_V2,
            episode_id: episode.episode_id.clone(),
            arm_state_id: episode.arm_state_id.clone(),
            task_id: followup.task_id.clone(),
            operation_id: followup.operation_id.clone(),
            term_fixture_id: followup.term_fixture_id.clone(),
            source_fixture_id: followup.source.fixture_id.clone(),
            source_sha256: followup.source.sha256.clone(),
            source_byte_length: followup.source.byte_length,
            rights: followup.source.rights,
            expected_rule_state: followup.expected_rule_state,
            observed_rule_state: observed_state,
            application_owner: "local_replacement_owner_with_production_facts",
            output_text,
            output_sha256,
            output_byte_length,
            expected_output_sha256: followup.expected_output_sha256.clone(),
            expected_output_byte_length: followup.expected_output_byte_length,
            expected_output_match,
            applied_pairs: applied_pairs
                .into_iter()
                .map(|pair| LearningAppliedPairV2 {
                    before: pair.before,
                    after: pair.after,
                })
                .collect(),
            selected_pair_attribution,
            usage_receipt,
            pipeline_final: "not_run",
            asr_quality: "not_measured",
            insertion: "not_run",
            ax_observation: "not_run",
            independent_target_oracle: "not_run",
        })
    }

    pub(crate) fn execute_undo(
        &mut self,
        episode: &LearningEpisodeV2,
        operation_id: &str,
    ) -> anyhow::Result<LearningUndoReceiptV2> {
        episode.validate().map_err(anyhow::Error::msg)?;
        validate_owned_arm_storage(&self.root)?;
        let undo = episode
            .undo_operation(operation_id)
            .ok_or_else(|| anyhow::anyhow!("learning_undo_not_authorized"))?;
        let selected_state = self.selected_state(episode)?;
        if selected_state.settings_consistency == "promoted_rule_ownership_unsupported" {
            anyhow::bail!("learning_global_rule_ownership_unsupported");
        }
        let required_followup = episode
            .followup(&undo.required_followup_task_id)
            .ok_or_else(|| anyhow::anyhow!("learning_undo_prerequisite_missing"))?;
        let term = episode
            .term(&undo.term_fixture_id)
            .ok_or_else(|| anyhow::anyhow!("learning_term_fixture_mismatch"))?;
        let key = crate::dictionary_learn::pair_key(&term.before, &term.after);
        let apply_observation = store::LearnPairObservation::research_fixture_apply(
            &format!(
                "{}:{}:{}",
                episode.episode_id, required_followup.operation_id, required_followup.source.sha256
            ),
            &key,
        );
        let undo_observation = store::LearnPairObservation::research_fixture_undo(
            &format!("{}:{}", episode.episode_id, undo.operation_id),
            &key,
        );
        validate_owned_arm_storage(&self.root)?;
        let undo_already_recorded =
            store::has_learn_pair_observation(&self.root, &undo_observation)?;
        validate_owned_arm_storage(&self.root)?;
        if !undo_already_recorded
            && !store::has_learn_pair_observation(&self.root, &apply_observation)?
        {
            anyhow::bail!("learning_undo_requires_prior_application");
        }
        let row = store::get_learn_pair(&self.root, &key)?
            .ok_or_else(|| anyhow::anyhow!("selected_learning_pair_unavailable"))?;
        if row.before_surface != term.before
            || row.after_surface != term.after
            || !row_within_bounds(&row)
            || !row_within_episode_scope(&row, &episode.scope)
        {
            anyhow::bail!("selected_learning_pair_identity_mismatch");
        }
        if row.is_live_promoted() && !row.auto_scoped {
            anyhow::bail!("learning_global_rule_ownership_unsupported");
        }
        let state_before = learning_pair_state(&row);
        if undo_already_recorded {
            if state_before != "tombstoned" {
                anyhow::bail!("learning_undo_receipt_state_mismatch");
            }
        } else {
            if state_before != "promoted" {
                anyhow::bail!("learning_undo_rule_state_mismatch");
            }
            let usages = store::list_learned_term_usage(&self.root)?;
            if !usages
                .iter()
                .any(|usage| usage.word == term.after && usage.replacement_runs > 0)
            {
                anyhow::bail!("learning_undo_requires_applied_pair_usage");
            }
        }
        let rows_before_undo = store::list_learn_pairs(&self.root)?;
        if crate::dictionary_learn::should_remove_dictionary_entry_after_undo(
            &row,
            &rows_before_undo,
        ) {
            anyhow::bail!("learning_global_dictionary_undo_unsupported");
        }
        let (tombstoned, duplicate) = if undo_already_recorded {
            (false, true)
        } else {
            validate_owned_arm_storage(&self.root)?;
            store::tombstone_research_learn_pair(&self.root, &key, &undo_observation)?
        };
        if !duplicate && !tombstoned {
            anyhow::bail!("learning_undo_pair_not_tombstoned");
        }
        validate_owned_arm_storage(&self.root)?;
        let current_row = store::get_learn_pair(&self.root, &key)?
            .ok_or_else(|| anyhow::anyhow!("selected_learning_pair_unavailable"))?;
        let rows_after_undo = store::list_learn_pairs(&self.root)?;
        if crate::dictionary_learn::should_remove_dictionary_entry_after_undo(
            &current_row,
            &rows_after_undo,
        ) {
            anyhow::bail!("learning_global_dictionary_undo_unsupported");
        }
        Ok(LearningUndoReceiptV2 {
            schema_version: LEARNING_EPISODE_SCHEMA_V2,
            episode_id: episode.episode_id.clone(),
            arm_state_id: episode.arm_state_id.clone(),
            operation_id: undo.operation_id.clone(),
            required_followup_task_id: undo.required_followup_task_id.clone(),
            term_fixture_id: undo.term_fixture_id.clone(),
            state_before,
            state_after: "tombstoned",
            sql_outcome: if duplicate {
                "duplicate_operation_no_tombstone_refresh"
            } else {
                "tombstone_and_operation_receipt_committed"
            },
            settings_readback: "not_required_auto_scoped_undo",
            dictionary_entry_policy: "retained_by_production_auto_scoped_policy",
            pipeline_final: "not_run",
            insertion: "not_run",
            ax_observation: "not_run",
            independent_target_oracle: "not_run",
        })
    }

    pub(crate) async fn execute_wav(
        &mut self,
        episode: &LearningEpisodeV2,
        task_id: &str,
        operation_id: &str,
        settled_text: Option<&str>,
        resources: LearningWavResources<'_>,
    ) -> anyhow::Result<LearningWavReceiptV2> {
        use sha2::{Digest, Sha256};

        let LearningWavResources {
            wav_root,
            model_source_root,
            expires,
        } = resources;
        episode.validate().map_err(anyhow::Error::msg)?;
        validate_owned_arm_storage(&self.root)?;
        let task = episode
            .wav_operation(task_id)
            .ok_or_else(|| anyhow::anyhow!("learning_wav_task_not_authorized"))?;
        if std::time::Instant::now() >= expires {
            anyhow::bail!("learning_episode_expired");
        }
        if task.operation_id != operation_id {
            anyhow::bail!("learning_operation_not_authorized");
        }
        let edit = task.edit.as_ref();
        if edit.is_some() != settled_text.is_some() {
            anyhow::bail!("learning_wav_edit_payload_mismatch");
        }
        let wav_root = validate_external_directory(wav_root)?;
        let model_source_root = validate_external_directory(model_source_root)?;
        let audio = read_declared_learning_wav(&wav_root, &task.source)?;
        if std::time::Instant::now() >= expires {
            anyhow::bail!("learning_episode_expired");
        }
        validate_research_wav(&audio)?;
        if std::time::Instant::now() >= expires {
            anyhow::bail!("learning_episode_expired");
        }
        if let (Some(edit), Some(settled_text)) = (edit, settled_text) {
            if settled_text.is_empty()
                || settled_text.len() != edit.settled_text_byte_length
                || hex::encode(Sha256::digest(settled_text.as_bytes())) != edit.settled_text_sha256
            {
                anyhow::bail!("learning_wav_edit_payload_mismatch");
            }
        }
        let model_work_root = self.root.join("model-work-v2");
        create_private_directory(&model_work_root, false)?;
        validate_private_directory(&model_work_root)?;
        if model_work_root.canonicalize()? != model_work_root {
            anyhow::bail!("learning_model_work_root_changed");
        }
        let episode_work_root = model_work_root.join(&episode.episode_id);
        create_private_directory(&episode_work_root, false)?;
        validate_private_directory(&episode_work_root)?;
        if episode_work_root.canonicalize()? != episode_work_root {
            anyhow::bail!("learning_model_work_root_changed");
        }
        let work_root = episode_work_root.join(&task.task_id);
        if work_root.starts_with(&model_source_root)
            || model_source_root.starts_with(&work_root)
            || work_root.starts_with(&wav_root)
            || wav_root.starts_with(&work_root)
        {
            anyhow::bail!("learning_model_work_root_overlaps_source");
        }
        create_private_directory(&work_root, false)?;
        validate_private_directory(&work_root)?;
        let work_root = work_root.canonicalize()?;
        if work_root.starts_with(&model_source_root)
            || model_source_root.starts_with(&work_root)
            || work_root.starts_with(&wav_root)
            || wav_root.starts_with(&work_root)
        {
            anyhow::bail!("learning_model_work_root_overlaps_source");
        }
        let models_root = work_root.join("models");
        create_private_directory(&models_root, false)?;
        let deadline = tokio::time::Instant::from_std(expires);
        let model_facts =
            materialize_verified_model(&model_source_root, &models_root, &task.model_id, deadline)
                .await
                .map_err(anyhow::Error::msg)?;
        validate_private_directory(&work_root)?;
        if std::fs::symlink_metadata(&models_root)?
            .file_type()
            .is_symlink()
            || models_root.canonicalize()? != models_root
        {
            anyhow::bail!("learning_model_work_root_changed");
        }
        if std::time::Instant::now() >= expires {
            anyhow::bail!("learning_episode_expired");
        }

        use crate::asr::AsrProvider;
        let provider = crate::ondevice_asr::OnDeviceAsrProvider::new(
            models_root.clone(),
            task.model_id.clone(),
        );
        let options = crate::asr::AsrOptions {
            api_key: String::new(),
            language: task.language.clone(),
            prompt: None,
            keywords: Vec::new(),
            model: task.model_id.clone(),
            session_generation: None,
            trace: None,
        };
        let mut transcription = provider.transcribe_batch(audio, options);
        let transcript_result = tokio::select! {
            result = &mut transcription => Some(result),
            _ = tokio::time::sleep_until(deadline) => None,
        };
        if transcript_result.is_none() {
            drop(transcription);
            let shutdown =
                crate::ondevice_runtime::shutdown_for_root_with_status(&models_root).await;
            if shutdown.is_err() {
                anyhow::bail!("learning_wav_cancel_owner_unsettled");
            }
            anyhow::bail!("learning_wav_deadline_expired_owner_settled");
        }
        let runtime = crate::ondevice_runtime::shared_runtime(&models_root);
        let runtime_snapshot = runtime.snapshot();
        let attestation = runtime.attestation();
        let diagnostic_paths = [
            (&self.root, "<arm-root>"),
            (&wav_root, "<wav-root>"),
            (&model_source_root, "<model-source-root>"),
            (&work_root, "<model-work-root>"),
            (&models_root, "<models-root>"),
        ];
        let runtime_error = runtime_snapshot
            .error
            .as_deref()
            .map(|detail| sanitize_research_diagnostic(detail, &diagnostic_paths));
        let shutdown = crate::ondevice_runtime::shutdown_for_root_with_status(&models_root).await;
        if shutdown.is_err() {
            anyhow::bail!("learning_wav_runtime_owner_unsettled");
        }
        let transcript_result =
            transcript_result.ok_or_else(|| anyhow::anyhow!("learning_wav_deadline_expired"))?;
        let (
            raw_asr_text,
            raw_asr_sha256,
            raw_asr_byte_length,
            asr_error_category,
            asr_error_detail,
        ) = match transcript_result {
            Ok(transcript) => {
                let raw_text = transcript.text;
                let byte_length = raw_text.len();
                let sha256 = hex::encode(Sha256::digest(raw_text.as_bytes()));
                let receipt_text = (byte_length <= LEARNING_TEXT_MAX_BYTES).then_some(raw_text);
                (receipt_text, Some(sha256), Some(byte_length), None, None)
            }
            Err(error) => {
                let (category, detail) = learning_asr_error_details(&error, &diagnostic_paths);
                (None, None, None, Some(category), Some(detail))
            }
        };
        let fixture_prediction_match =
            match (&task.expected_raw_asr, &raw_asr_sha256, raw_asr_byte_length) {
                (Some(expected), Some(sha256), Some(byte_length)) => {
                    Some(expected.byte_length == byte_length && expected.sha256 == *sha256)
                }
                _ => None,
            };
        let expected_raw_asr_sha256 = task
            .expected_raw_asr
            .as_ref()
            .map(|expected| expected.sha256.clone());
        let expected_raw_asr_byte_length = task
            .expected_raw_asr
            .as_ref()
            .map(|expected| expected.byte_length);
        let fixture_prediction_provenance = task
            .expected_raw_asr
            .as_ref()
            .map(|expected| expected.provenance);
        let attestation_status = if attestation.is_none() {
            "missing"
        } else {
            "validation_pending_shared_gate"
        };
        let mut asr_outcome = "transcription_completed";
        let learning_outcome;
        let mut observer_outcome = None;
        let mut candidate_class = None;
        let mut term_fixture_id = None;
        let mut hits_before = None;
        let mut hits_after = None;
        let mut promote_threshold = None;
        let mut promoted = None;
        let mut sql_readback = "not_attempted";
        let mut candidate_write = "not_attempted";
        let mut settings_readback = "not_attempted";
        if asr_error_category.is_some() {
            asr_outcome = "transcription_failed";
            learning_outcome = "not_run_asr_error";
        } else if raw_asr_byte_length == Some(0) {
            asr_outcome = "empty_transcription";
            learning_outcome = "not_run_empty_transcription";
        } else if raw_asr_byte_length
            .is_some_and(|byte_length| byte_length > LEARNING_TEXT_MAX_BYTES)
        {
            asr_outcome = "raw_asr_text_exceeds_receipt_bound";
            learning_outcome = "not_run_raw_asr_text_bounds";
        } else if std::time::Instant::now() >= expires {
            asr_outcome = "lease_expired_before_learning_commit";
            learning_outcome = "not_run_lease_expired";
        } else if attestation_status == "missing" {
            asr_outcome = "runtime_attestation_missing";
            learning_outcome = "not_run_without_runtime_attestation";
        } else if attestation_status != "complete" {
            asr_outcome = "loaded_attestation_validation_pending";
            learning_outcome = "not_run_without_complete_runtime_attestation";
        } else if fixture_prediction_match == Some(false) {
            asr_outcome = "fixture_prediction_mismatch";
            learning_outcome = "not_run_prediction_mismatch";
        } else if let Some(edit) = edit {
            let Some(raw_asr_text) = raw_asr_text.as_deref() else {
                anyhow::bail!("learning_raw_asr_text_missing");
            };
            let Some(settled_text) = settled_text else {
                anyhow::bail!("learning_wav_edit_payload_mismatch");
            };
            let term = episode
                .term(&edit.term_fixture_id)
                .ok_or_else(|| anyhow::anyhow!("learning_term_fixture_mismatch"))?;
            let key = crate::dictionary_learn::pair_key(&term.before, &term.after);
            let before_row = store::get_learn_pair(&self.root, &key)?;
            if before_row.as_ref().is_some_and(|row| {
                row.before_surface != term.before
                    || row.after_surface != term.after
                    || !row_within_bounds(row)
                    || !row_within_episode_scope(row, &episode.scope)
            }) {
                anyhow::bail!("selected_learning_pair_identity_mismatch");
            }
            let state_before = self.selected_state(episode)?;
            if state_before.settings_consistency == "promoted_rule_ownership_unsupported" {
                anyhow::bail!("learning_global_rule_ownership_unsupported");
            }
            let consent = LearningConsent {
                selected_state_id: episode.selected_state_id.clone(),
                baseline_run: episode.baseline_run.clone(),
                edit_fixture_id: task.source.fixture_id.clone(),
                purpose: LearningPurpose::Correction,
                mapping_id: episode.scope.mapping_id.clone(),
                bundle_id: episode.scope.native_bundle.clone(),
                observer_enabled: true,
                post_insert_field: raw_asr_text.to_owned(),
                settled_field: settled_text.to_owned(),
                terms: episode.terms.clone(),
            };
            consent.validate().map_err(anyhow::Error::msg)?;
            let stable_source_origin = format!("public_wav:{}", task.source.sha256);
            if std::time::Instant::now() >= expires {
                asr_outcome = "lease_expired_before_learning_commit";
                learning_outcome = "not_run_lease_expired";
            } else {
                let dictionary_before = self.settings.dictionary.clone();
                let outcome = with_scoped_consent(consent.clone(), || {
                    crate::dictionary_learn::record_controlled_fixture_edit(
                        &self.root,
                        &mut self.settings,
                        &episode.scope,
                        &consent,
                        crate::dictionary_learn::ControlledFixtureEditInput {
                            expected_before: &term.before,
                            expected_after: &term.after,
                            episode_id: &episode.episode_id,
                            source_sha256: &task.source.sha256,
                            operation_id: &task.operation_id,
                            stable_source_origin_id: Some(&stable_source_origin),
                        },
                    )
                })?;
                if self.settings.dictionary != dictionary_before {
                    anyhow::bail!("learning_local_only_mutated_global_dictionary");
                }
                validate_owned_arm_storage(&self.root)?;
                let after_row = store::get_learn_pair(&self.root, &key)?;
                candidate_write = outcome.candidate_write;
                settings_readback = "not_required_local_only_scoped_pair";
                observer_outcome = Some(outcome.observer_outcome);
                candidate_class = outcome.candidate_class;
                term_fixture_id = outcome.candidate_class.map(|_| term.fixture_id.clone());
                hits_before = Some(before_row.as_ref().map(|row| row.hits).unwrap_or_default());
                hits_after = after_row.as_ref().map(|row| row.hits);
                promote_threshold = outcome.promote_threshold;
                promoted = after_row.as_ref().map(|row| row.is_live_promoted());
                sql_readback = "selected_candidate_sql_row_read_back";
                learning_outcome = "programmatic_fixture_edit_owner_called";
            }
        } else {
            learning_outcome = "not_run_no_declared_programmatic_edit";
        }
        Ok(LearningWavReceiptV2 {
            schema_version: LEARNING_EPISODE_SCHEMA_V2,
            episode_id: episode.episode_id.clone(),
            arm_state_id: episode.arm_state_id.clone(),
            task_id: task.task_id.clone(),
            operation_id: task.operation_id.clone(),
            replay_policy: "same_operation_may_retry_after_process_restart",
            source_fixture_id: task.source.fixture_id.clone(),
            source_sha256: task.source.sha256.clone(),
            source_byte_length: task.source.byte_length,
            source_rights: task.source.rights,
            source_origin_policy: "same_content_sha256_same_research_origin",
            source_path_disclosed: false,
            model_id: task.model_id.clone(),
            model_revision: model_facts.revision,
            model_manifest_sha256: model_facts.manifest_sha256,
            model_file_count: model_facts.file_count,
            transcript_owner: "on_device_asr_provider",
            loaded_model_id: runtime_snapshot.loaded_model_id,
            loaded_model_revision: runtime_snapshot.loaded_model_version,
            runtime_error,
            raw_asr_text,
            raw_asr_sha256,
            raw_asr_byte_length,
            asr_error_category,
            asr_error_detail,
            expected_raw_asr_sha256,
            expected_raw_asr_byte_length,
            fixture_prediction_provenance,
            fixture_prediction_match,
            runtime_status: runtime_snapshot.runtime_status,
            runtime_attestation: attestation_status,
            asr_outcome,
            learning_outcome,
            term_fixture_id,
            observer_outcome,
            candidate_class,
            hits_before,
            hits_after,
            promote_threshold,
            promoted,
            sql_readback,
            candidate_write,
            settings_readback,
            insertion: "not_run",
            ax_observation: "not_run",
            independent_target_oracle: "not_run",
            asr_quality: "not_measured",
        })
    }

    pub(crate) fn execute_text_operation(
        &mut self,
        episode: &LearningEpisodeV2,
        operation: &LearningTextOperationV2,
    ) -> anyhow::Result<LearningOperationReceiptV2> {
        use sha2::{Digest, Sha256};

        episode.validate().map_err(anyhow::Error::msg)?;
        validate_owned_arm_storage(&self.root)?;
        if operation.episode_id != episode.episode_id {
            anyhow::bail!("learning_episode_mismatch");
        }
        let declared = episode
            .operation(&operation.operation_id)
            .ok_or_else(|| anyhow::anyhow!("learning_operation_not_authorized"))?;
        if operation.source_fixture_id != episode.source.fixture_id {
            anyhow::bail!("learning_source_fixture_mismatch");
        }
        if operation.states.len() != episode.edit_states.len() {
            anyhow::bail!("unsupported_edit_state_count");
        }
        let Some(post_insert) = operation.states.first() else {
            anyhow::bail!("unsupported_edit_state_count");
        };
        let Some(settled) = operation.states.last() else {
            anyhow::bail!("unsupported_edit_state_count");
        };
        for (input, expected) in operation.states.iter().zip(&episode.edit_states) {
            if input.sequence != expected.sequence
                || input.state_id != expected.state_id
                || input.layer != expected.layer
                || input.text.len() != expected.byte_length
                || hex::encode(Sha256::digest(input.text.as_bytes())) != expected.sha256
            {
                anyhow::bail!("learning_edit_state_mismatch");
            }
        }
        let term = episode
            .term(&declared.term_fixture_id)
            .ok_or_else(|| anyhow::anyhow!("learning_term_fixture_mismatch"))?;
        let consent = LearningConsent {
            selected_state_id: episode.selected_state_id.clone(),
            baseline_run: episode.baseline_run.clone(),
            edit_fixture_id: episode.edit_fixture_id.clone(),
            purpose: episode.purpose,
            mapping_id: episode.scope.mapping_id.clone(),
            bundle_id: episode.scope.native_bundle.clone(),
            observer_enabled: true,
            post_insert_field: post_insert.text.clone(),
            settled_field: settled.text.clone(),
            terms: episode.terms.clone(),
        };
        consent.validate().map_err(anyhow::Error::msg)?;
        let expected_source = hex::encode(Sha256::digest(post_insert.text.as_bytes()));
        if expected_source != episode.source.sha256
            || post_insert.text != consent.post_insert_field
            || settled.text != consent.settled_field
        {
            anyhow::bail!("learning_source_mismatch");
        }

        let key = crate::dictionary_learn::pair_key(&term.before, &term.after);
        let state_before = self.selected_state(episode)?;
        if state_before.settings_consistency == "promoted_rule_ownership_unsupported" {
            anyhow::bail!("learning_global_rule_ownership_unsupported");
        }
        let before_row = store::get_learn_pair(&self.root, &key)?;
        validate_owned_arm_storage(&self.root)?;
        let ordered_states = operation
            .states
            .iter()
            .map(|state| state.text.clone())
            .collect::<Vec<_>>();
        let dictionary_before = self.settings.dictionary.clone();
        let result = with_scoped_consent(consent.clone(), || {
            crate::dictionary_learn::record_controlled_fixture_edit_trajectory(
                &self.root,
                &mut self.settings,
                &episode.scope,
                &consent,
                crate::dictionary_learn::ControlledFixtureTrajectoryInput {
                    edit: crate::dictionary_learn::ControlledFixtureEditInput {
                        expected_before: &term.before,
                        expected_after: &term.after,
                        episode_id: &episode.episode_id,
                        source_sha256: &episode.source.sha256,
                        operation_id: &operation.operation_id,
                        stable_source_origin_id: None,
                    },
                    ordered_states: &ordered_states,
                },
            )
        });
        let outcome = result?;
        if self.settings.dictionary != dictionary_before {
            anyhow::bail!("learning_local_only_mutated_global_dictionary");
        }
        validate_owned_arm_storage(&self.root)?;
        let after_row = store::get_learn_pair(&self.root, &key)?;
        let candidate_write = outcome.candidate_write;
        let arm_settings_readback = "not_required_local_only_scoped_pair";
        let receipt = LearningOperationReceiptV2 {
            schema_version: LEARNING_EPISODE_SCHEMA_V2,
            episode_id: episode.episode_id.clone(),
            arm_state_id: episode.arm_state_id.clone(),
            operation_id: operation.operation_id.clone(),
            source_fixture_id: episode.source.fixture_id.clone(),
            source_kind: episode.source.kind,
            source_sha256: episode.source.sha256.clone(),
            source_byte_length: episode.source.byte_length,
            rights: episode.source.rights,
            actor: episode.actor,
            admission: "source_and_learning_grants_matched",
            observer_mode: "production_bounded_text_trajectory_observer_and_candidate_owner_without_target_read",
            observer_outcome: outcome.observer_outcome,
            trajectory_samples_observed: outcome.trajectory_samples_observed,
            trajectory_states_consumed: outcome.trajectory_states_consumed,
            trajectory_settled: outcome.trajectory_settled,
            candidate_class: outcome.candidate_class,
            term_fixture_id: outcome
                .candidate_class
                .map(|_| term.fixture_id.clone()),
            hits_before: Some(before_row.as_ref().map(|row| row.hits).unwrap_or_default()),
            hits_after: after_row.as_ref().map(|row| row.hits),
            promote_threshold: outcome.promote_threshold,
            promoted: after_row.as_ref().map(|row| row.is_live_promoted()),
            sql_readback: "selected_candidate_sql_row_read_back",
            candidate_write,
            arm_settings_readback,
            insertion: "not_run",
            ax_observation: "not_run",
            independent_target_oracle: "not_authorized_not_run",
            asr_quality: "not_measured",
        };
        Ok(receipt)
    }
}

#[cfg(any(test, feature = "research-harness"))]
pub(crate) fn with_scoped_consent<T>(consent: LearningConsent, operation: impl FnOnce() -> T) -> T {
    SCOPED_CONSENT.with(|slot| {
        let previous = slot.replace(Some(consent));
        struct Restore<'a> {
            slot: &'a RefCell<Option<LearningConsent>>,
            previous: Option<LearningConsent>,
        }
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                self.slot.replace(self.previous.take());
            }
        }
        let _restore = Restore { slot, previous };
        operation()
    })
}

#[cfg(any(test, feature = "research-harness"))]
fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(any(test, feature = "research-harness"))]
fn valid_learning_wav_relative_path(value: &str) -> bool {
    use std::path::Component;

    value.len() <= 512
        && !value.is_empty()
        && !value.chars().any(char::is_control)
        && Path::new(value)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        && Path::new(value)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"))
}

#[cfg(any(test, feature = "research-harness"))]
fn read_declared_learning_wav(
    root: &Path,
    source: &LearningWavSourceV2,
) -> anyhow::Result<Vec<u8>> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    use std::path::Component;

    if !valid_learning_wav_relative_path(&source.relative_path) {
        anyhow::bail!("learning_wav_relative_path_invalid");
    }
    let root = root.canonicalize()?;
    let relative = Path::new(&source.relative_path);
    let mut path = root.clone();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            anyhow::bail!("learning_wav_relative_path_invalid");
        };
        path.push(name);
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink()
            || (components.peek().is_some() && !metadata.is_dir())
            || (components.peek().is_none() && !metadata.is_file())
        {
            anyhow::bail!("learning_wav_source_file_invalid");
        }
    }
    let canonical_path = path.canonicalize()?;
    if !canonical_path.starts_with(&root) {
        anyhow::bail!("learning_wav_source_path_escaped_root");
    }
    let mut file = std::fs::File::open(&canonical_path)?;
    let before = file.metadata()?;
    if !before.is_file()
        || before.len() != source.byte_length as u64
        || before.len() > LEARNING_WAV_MAX_BYTES as u64
    {
        anyhow::bail!("learning_wav_source_length_mismatch");
    }
    let mut bytes = Vec::with_capacity(source.byte_length);
    file.by_ref()
        .take((LEARNING_WAV_MAX_BYTES as u64) + 1)
        .read_to_end(&mut bytes)?;
    let after_file = file.metadata()?;
    let after_path = std::fs::symlink_metadata(&canonical_path)?;
    if bytes.len() != source.byte_length
        || after_path.file_type().is_symlink()
        || !after_path.is_file()
        || before.len() != after_file.len()
        || before.modified().ok() != after_file.modified().ok()
        || before.len() != after_path.len()
    {
        anyhow::bail!("learning_wav_source_changed_during_read");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after_file.dev()
            || before.ino() != after_file.ino()
            || after_file.dev() != after_path.dev()
            || after_file.ino() != after_path.ino()
        {
            anyhow::bail!("learning_wav_source_identity_changed");
        }
    }
    if hex::encode(Sha256::digest(&bytes)) != source.sha256 {
        anyhow::bail!("learning_wav_source_sha_mismatch");
    }
    Ok(bytes)
}

#[cfg(any(test, feature = "research-harness"))]
fn validate_research_wav(bytes: &[u8]) -> anyhow::Result<()> {
    if bytes.len() > LEARNING_WAV_MAX_BYTES {
        anyhow::bail!("learning_wav_source_length_mismatch");
    }
    let mut reader = hound::WavReader::new(std::io::Cursor::new(bytes))
        .map_err(|_| anyhow::anyhow!("learning_wav_pcm_invalid"))?;
    let spec = reader.spec();
    if spec.sample_rate != 16_000
        || spec.channels != 1
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
    {
        anyhow::bail!("learning_wav_pcm_format_unsupported");
    }
    let duration = reader.duration() as u64;
    if duration == 0 || duration > 15 * 60 * 16_000 {
        anyhow::bail!("learning_wav_duration_unsupported");
    }
    for sample in reader.samples::<i16>() {
        sample.map_err(|_| anyhow::anyhow!("learning_wav_pcm_invalid"))?;
    }
    Ok(())
}

#[cfg(any(test, feature = "research-harness"))]
struct VerifiedResearchModelFacts {
    revision: String,
    manifest_sha256: String,
    file_count: usize,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Serialize)]
struct ResearchInstalledModelManifestV1<'a> {
    schema_version: u32,
    id: &'a str,
    revision: &'a str,
    files: Vec<ResearchInstalledModelFileV1<'a>>,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Serialize)]
struct ResearchInstalledModelFileV1<'a> {
    path: &'a str,
    bytes: u64,
    sha256: &'a str,
}

#[cfg(any(test, feature = "research-harness"))]
async fn materialize_verified_model(
    source_root: &Path,
    models_root: &Path,
    model_id: &str,
    deadline: tokio::time::Instant,
) -> Result<VerifiedResearchModelFacts, String> {
    if tokio::time::Instant::now() >= deadline {
        return Err("learning_episode_expired".into());
    }
    let source_root = source_root.to_owned();
    let models_root = models_root.to_owned();
    let model_id = model_id.to_owned();
    let cancel = tokio_util::sync::CancellationToken::new();
    let copy_cancel = cancel.clone();
    let mut owner = tokio::task::spawn_blocking(move || {
        materialize_verified_model_blocking(&source_root, &models_root, &model_id, &copy_cancel)
    });
    tokio::select! {
        result = &mut owner => result
            .map_err(|_| "learning_model_copy_owner_failed".to_owned())?,
        _ = tokio::time::sleep_until(deadline) => {
            cancel.cancel();
            let _settled = owner.await
                .map_err(|_| "learning_model_copy_owner_failed".to_owned())?;
            Err("learning_model_copy_deadline_expired_owner_settled".into())
        }
    }
}

#[cfg(any(test, feature = "research-harness"))]
fn materialize_verified_model_blocking(
    source_root: &Path,
    models_root: &Path,
    model_id: &str,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<VerifiedResearchModelFacts, String> {
    use sha2::{Digest, Sha256};
    use std::io::Write;

    if model_id != crate::ondevice_models::QWEN_ASR_06_ID {
        return Err("learning_model_id_unsupported".into());
    }
    if cancel.is_cancelled() {
        return Err("learning_model_copy_cancelled".into());
    }
    let source = crate::ondevice_runtime::evaluation_source::VerifiedModelSource::open(
        source_root,
        model_id,
    )
    .map_err(|_| "learning_model_source_unverified".to_owned())?;
    let source_files = source.files().to_vec();
    if cancel.is_cancelled() {
        return Err("learning_model_copy_cancelled".into());
    }
    create_private_directory(models_root, false).map_err(|_| "learning_model_work_root_invalid")?;
    validate_private_directory(models_root).map_err(|_| "learning_model_work_root_invalid")?;
    let model_directory = crate::ondevice_asr::model_dir(models_root, model_id);
    if model_directory.exists() {
        let model_metadata = std::fs::symlink_metadata(&model_directory)
            .map_err(|_| "learning_model_work_copy_unavailable")?;
        if model_metadata.file_type().is_symlink() || !model_metadata.is_dir() {
            return Err("learning_model_work_copy_invalid".into());
        }
        if crate::ondevice_asr::model_state(models_root, model_id)
            != crate::ondevice_asr::OnDeviceModelState::Ready
        {
            let models_root_canonical = models_root
                .canonicalize()
                .map_err(|_| "learning_model_work_root_invalid")?;
            let model_directory_canonical = model_directory
                .canonicalize()
                .map_err(|_| "learning_model_work_copy_invalid")?;
            if model_directory_canonical == models_root_canonical
                || !model_directory_canonical.starts_with(&models_root_canonical)
            {
                return Err("learning_model_work_copy_invalid".into());
            }
            std::fs::remove_dir_all(&model_directory)
                .map_err(|_| "learning_model_work_copy_cleanup_failed")?;
        }
    }
    if model_directory.exists() {
        let copied = crate::ondevice_runtime::evaluation_source::VerifiedModelSource::open(
            models_root,
            model_id,
        )
        .map_err(|_| "learning_model_work_copy_unverified".to_owned())?;
        if copied.files() != source_files.as_slice() || cancel.is_cancelled() {
            return Err("learning_model_work_copy_identity_mismatch".into());
        }
    } else {
        create_private_directory(&model_directory, true)
            .map_err(|_| "learning_model_work_root_invalid")?;
        for (index, (relative, _, _)) in source_files.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err("learning_model_copy_cancelled".into());
            }
            let relative_path = Path::new(relative);
            if relative_path.is_absolute()
                || relative_path
                    .components()
                    .any(|component| !matches!(component, std::path::Component::Normal(_)))
            {
                return Err("learning_model_manifest_path_invalid".into());
            }
            let destination = model_directory.join(relative_path);
            let parent = destination
                .parent()
                .ok_or_else(|| "learning_model_manifest_path_invalid".to_owned())?;
            if !parent.starts_with(&model_directory) {
                return Err("learning_model_manifest_path_invalid".into());
            }
            create_private_directory(parent, false)
                .map_err(|_| "learning_model_work_directory_invalid")?;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut output = options
                .open(&destination)
                .map_err(|_| "learning_model_work_file_create_failed")?;
            source
                .copy_file(index, &mut output, cancel, |_| {})
                .map_err(|failure| match failure {
                    crate::ondevice_runtime::evaluation_source::CopyFailure::Cancelled => {
                        "learning_model_copy_cancelled"
                    }
                    crate::ondevice_runtime::evaluation_source::CopyFailure::Failed(reason) => {
                        let _reason_length = reason.len();
                        "learning_model_copy_failed"
                    }
                })?;
            output
                .flush()
                .map_err(|_| "learning_model_copy_flush_failed")?;
        }
        if cancel.is_cancelled() {
            return Err("learning_model_copy_cancelled".into());
        }
        let copied = crate::ondevice_runtime::evaluation_source::VerifiedModelSource::open(
            models_root,
            model_id,
        )
        .map_err(|_| "learning_model_work_copy_unverified".to_owned())?;
        if copied.files() != source_files.as_slice() || cancel.is_cancelled() {
            return Err("learning_model_work_copy_identity_mismatch".into());
        }
        let manifest = crate::ondevice_models::mlx_model(model_id)
            .ok_or_else(|| "learning_model_id_unsupported".to_owned())?;
        let installed = ResearchInstalledModelManifestV1 {
            schema_version: 1,
            id: model_id,
            revision: manifest.revision,
            files: source_files
                .iter()
                .map(|(path, bytes, sha256)| ResearchInstalledModelFileV1 {
                    path,
                    bytes: *bytes,
                    sha256,
                })
                .collect(),
        };
        let marker_bytes = serde_json::to_vec(&installed)
            .map_err(|_| "learning_model_marker_encode_failed".to_owned())?;
        if cancel.is_cancelled() {
            return Err("learning_model_copy_cancelled".into());
        }
        let marker_path = model_directory.join(".voiceflow-model-manifest.json");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut marker = options
            .open(&marker_path)
            .map_err(|_| "learning_model_marker_create_failed")?;
        marker
            .write_all(&marker_bytes)
            .map_err(|_| "learning_model_marker_write_failed")?;
        marker
            .sync_all()
            .map_err(|_| "learning_model_marker_sync_failed")?;
        if cancel.is_cancelled()
            || crate::ondevice_asr::model_state(models_root, model_id)
                != crate::ondevice_asr::OnDeviceModelState::Ready
        {
            return Err("learning_model_work_copy_not_ready".into());
        }
    }
    let manifest = crate::ondevice_models::mlx_model(model_id)
        .ok_or_else(|| "learning_model_id_unsupported".to_owned())?;
    let marker_bytes = std::fs::read(model_directory.join(".voiceflow-model-manifest.json"))
        .map_err(|_| "learning_model_marker_read_failed")?;
    Ok(VerifiedResearchModelFacts {
        revision: manifest.revision.to_owned(),
        manifest_sha256: hex::encode(Sha256::digest(marker_bytes)),
        file_count: source_files.len(),
    })
}

#[cfg(any(test, feature = "research-harness"))]
fn initial_arm_settings(episode: &LearningEpisodeV2) -> store::Settings {
    let mut settings = store::Settings::default();
    settings.dictionary.clear();
    settings.dictionary_learn_enabled = true;
    settings.local_edit_learning_enabled = true;
    settings.context_enabled = true;
    settings.browser_access_enabled = false;
    settings.window_ocr_enabled = false;
    settings.always_on_microphone = false;
    settings.context_mappings = vec![crate::context::AppMapping {
        id: episode.scope.mapping_id.clone(),
        label: "Controlled learning fixture".into(),
        family: episode.scope.family,
        mode_id: None,
        bundle_id: Some(episode.scope.native_bundle.clone()),
        executable: None,
        browser_host: None,
        browser_path_prefix: None,
        focused_field: None,
        source_permissions: crate::context::ContextSourcePermissions::default(),
        style_example_input: None,
        style_example_output: None,
        style_example_pairs: Vec::new(),
        style_examples_approved: false,
        enabled: true,
        cleanup_effort: None,
        cleanup_intensity: None,
        cleanup_enabled: false,
        dictionary_learn_enabled: true,
    }];
    settings
}

#[cfg(any(test, feature = "research-harness"))]
fn validate_arm_settings(
    settings: &store::Settings,
    episode: &LearningEpisodeV2,
) -> anyhow::Result<()> {
    let [mapping] = settings.context_mappings.as_slice() else {
        anyhow::bail!("learning arm mapping state mismatch");
    };
    if !settings.api_key.is_empty()
        || !settings.asr_api_key.is_empty()
        || !settings.cleanup_api_key.is_empty()
        || !settings.provider_api_keys.is_empty()
        || !settings.snippets.is_empty()
        || settings.browser_access_enabled
        || settings.window_ocr_enabled
        || settings.always_on_microphone
        || !settings.context_enabled
        || !settings.dictionary_learn_enabled
        || !settings.local_edit_learning_enabled
        || !settings.unverified_credential_sources.is_empty()
        || !settings.credential_baselines.is_empty()
        || settings.session_only_custom_credential
        || settings.session_custom_credential_scope.is_some()
        || settings.session_custom_credential_rejected
        || mapping.id != episode.scope.mapping_id
        || mapping.label != "Controlled learning fixture"
        || mapping.bundle_id.as_deref() != Some(episode.scope.native_bundle.as_str())
        || mapping.family != episode.scope.family
        || mapping.mode_id.is_some()
        || mapping.executable.is_some()
        || mapping.browser_host.is_some()
        || mapping.browser_path_prefix.is_some()
        || mapping.focused_field.is_some()
        || !mapping.enabled
        || !mapping.dictionary_learn_enabled
        || mapping.source_permissions != crate::context::ContextSourcePermissions::default()
        || mapping.style_examples_approved
        || !mapping.style_example_pairs.is_empty()
        || mapping.style_example_input.is_some()
        || mapping.style_example_output.is_some()
        || mapping.cleanup_effort.is_some()
        || mapping.cleanup_intensity.is_some()
        || mapping.cleanup_enabled
        || settings.dictionary.iter().any(|word| {
            !episode
                .terms
                .iter()
                .any(|term| term.after.as_str() == word.as_str())
        })
        || settings
            .dictionary
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != settings.dictionary.len()
    {
        anyhow::bail!("learning arm settings mismatch");
    }
    Ok(())
}

#[cfg(any(test, feature = "research-harness"))]
fn validate_arm_base_root(path: &Path) -> anyhow::Result<PathBuf> {
    if !path.is_absolute() {
        anyhow::bail!("learning arm root must be absolute");
    }
    reject_symlink_directory(path)?;
    let canonical = path.canonicalize()?;
    let temp_root = std::env::temp_dir().canonicalize()?;
    if canonical == temp_root || !canonical.starts_with(&temp_root) {
        anyhow::bail!("learning arm root must be a dedicated directory under the system temp root");
    }
    validate_private_directory(&canonical)?;
    Ok(canonical)
}

#[cfg(any(test, feature = "research-harness"))]
fn validate_private_directory(path: &Path) -> anyhow::Result<()> {
    reject_symlink_directory(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)?.permissions().mode();
        if mode & 0o077 != 0 {
            anyhow::bail!("learning arm directory must be private");
        }
    }
    Ok(())
}

#[cfg(any(test, feature = "research-harness"))]
fn validate_owned_arm_storage(root: &Path) -> anyhow::Result<()> {
    validate_private_directory(root)?;
    let canonical_root = root.canonicalize()?;
    if canonical_root.as_path() != root {
        anyhow::bail!("learning arm path is no longer canonical");
    }
    for name in [
        "history.sqlite",
        "history.sqlite-wal",
        "history.sqlite-shm",
        "history.sqlite-journal",
    ] {
        let path = root.join(name);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            anyhow::bail!("learning arm database path must be a regular non-symlink file");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if metadata.permissions().mode() & 0o077 != 0 || metadata.nlink() != 1 {
                anyhow::bail!("learning arm database file must be private and singly linked");
            }
        }
    }
    Ok(())
}

#[cfg(any(test, feature = "research-harness"))]
fn reject_symlink_directory(path: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!("learning arm path must be a non-symlink directory");
    }
    Ok(())
}

#[cfg(any(test, feature = "research-harness"))]
fn create_private_directory(path: &Path, exclusive: bool) -> anyhow::Result<()> {
    if path.exists() {
        reject_symlink_directory(path)?;
        return Ok(());
    }
    if exclusive {
        std::fs::create_dir(path)?;
    } else {
        std::fs::create_dir_all(path)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    reject_symlink_directory(path)
}

#[cfg(any(test, feature = "research-harness"))]
fn read_private_json<T: serde::de::DeserializeOwned>(path: &Path) -> anyhow::Result<T> {
    Ok(serde_json::from_slice(&read_private_bytes(path)?)?)
}

#[cfg(any(test, feature = "research-harness"))]
fn read_private_bytes(path: &Path) -> anyhow::Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("learning arm file must be a regular non-symlink file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 || metadata.nlink() != 1 {
            anyhow::bail!("learning arm file must be private and singly linked");
        }
    }
    Ok(std::fs::read(path)?)
}

#[cfg(any(test, feature = "research-harness"))]
fn write_private_json(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid arm manifest path"))?;
    let bytes = serde_json::to_vec(value)?;
    let temp = parent.join(format!(".arm-json-{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let write_result = (|| -> anyhow::Result<()> {
        let mut file = options.open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    write_result
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LearningPhaseArmManifestV3 {
    artifact_type: String,
    schema_version: u8,
    plan_id: String,
    arm_state_id: String,
    identity_sha256: String,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Serialize)]
struct LearningPhasePairSnapshotV3 {
    term_fixture_id: String,
    pair_key: String,
    hits: u32,
    promoted: bool,
    tombstoned: bool,
    ignored: bool,
    state: &'static str,
    stored_scope: Option<serde_json::Value>,
    row_sha256: Option<String>,
}

#[cfg(any(test, feature = "research-harness"))]
#[derive(Serialize)]
struct LearningPhaseArmSnapshotV3 {
    schema_version: u8,
    plan_id: String,
    arm_state_id: String,
    identity_sha256: String,
    settings_sha256: String,
    settings_consistency: &'static str,
    pairs: Vec<LearningPhasePairSnapshotV3>,
}

/// Versioned, single-database research arm for an immutable set of explicitly
/// granted context phases. Every mapping declared by the plan stays in this
/// arm's settings for its full lifetime.
#[cfg(any(test, feature = "research-harness"))]
struct LearningArmV3 {
    root: PathBuf,
    identity_sha256: String,
    settings: store::Settings,
}

#[cfg(any(test, feature = "research-harness"))]
impl LearningArmV3 {
    fn open(
        base_root: &Path,
        plan: &learning_phase_v3::LearningPhasePlanV3,
    ) -> anyhow::Result<Self> {
        plan.validate().map_err(anyhow::Error::msg)?;
        let base_root = validate_arm_base_root(base_root)?;
        let identity_sha256 = plan.identity_sha256()?;
        let arms_root = base_root.join("learning-arms-v3");
        create_private_directory(&arms_root, false)?;
        validate_private_directory(&arms_root)?;
        let root = arms_root.join(&plan.arm_state_id);
        let existed = root.exists();
        if existed {
            validate_private_directory(&root)?;
        } else {
            create_private_directory(&root, true)?;
        }
        validate_owned_arm_storage(&root)?;
        let manifest_path = root.join("arm.json");
        let settings_path = root.join("settings.json");
        let settings = if manifest_path.exists() {
            let manifest: LearningPhaseArmManifestV3 = read_private_json(&manifest_path)?;
            if manifest.artifact_type != "voiceflow_learning_phase_arm"
                || manifest.schema_version != 3
                || manifest.plan_id != plan.plan_id
                || manifest.arm_state_id != plan.arm_state_id
                || manifest.identity_sha256 != identity_sha256
            {
                anyhow::bail!("learning phase arm identity mismatch");
            }
            let settings: store::Settings = read_private_json(&settings_path)?;
            validate_phase_arm_settings(&settings, plan)?;
            settings
        } else {
            if std::fs::read_dir(&root)?.next().is_some() {
                anyhow::bail!("unowned files exist in learning phase arm root");
            }
            let settings = initial_phase_arm_settings(plan)?;
            store::save_configuration(&root, &settings)?;
            let readback: store::Settings = read_private_json(&settings_path)?;
            validate_phase_arm_settings(&readback, plan)?;
            let manifest = LearningPhaseArmManifestV3 {
                artifact_type: "voiceflow_learning_phase_arm".into(),
                schema_version: 3,
                plan_id: plan.plan_id.clone(),
                arm_state_id: plan.arm_state_id.clone(),
                identity_sha256: identity_sha256.clone(),
            };
            write_private_json(&manifest_path, &manifest)?;
            readback
        };
        validate_owned_arm_storage(&root)?;
        Ok(Self {
            root,
            identity_sha256,
            settings,
        })
    }

    fn selected_state(
        &self,
        plan: &learning_phase_v3::LearningPhasePlanV3,
    ) -> anyhow::Result<LearningPhaseArmSnapshotV3> {
        use sha2::{Digest, Sha256};

        validate_owned_arm_storage(&self.root)?;
        validate_phase_arm_settings(&self.settings, plan)?;
        let mut pairs = Vec::with_capacity(plan.terms.len());
        for term in &plan.terms {
            validate_owned_arm_storage(&self.root)?;
            let key = crate::dictionary_learn::pair_key(&term.before, &term.after);
            let row = store::get_learn_pair(&self.root, &key)?;
            if let Some(row) = row.as_ref() {
                if row.before_surface != term.before
                    || row.after_surface != term.after
                    || !row_within_bounds(row)
                    || !row_within_phase_plan_scope(row, plan)
                {
                    anyhow::bail!("selected learning phase row identity mismatch");
                }
            }
            let row_json = row.as_ref().map(serde_json::to_vec).transpose()?;
            pairs.push(LearningPhasePairSnapshotV3 {
                term_fixture_id: term.fixture_id.clone(),
                pair_key: key,
                hits: row.as_ref().map_or(0, |row| row.hits),
                promoted: row
                    .as_ref()
                    .is_some_and(store::LearnPairRecord::is_live_promoted),
                tombstoned: row.as_ref().is_some_and(|row| row.tombstoned_at.is_some()),
                ignored: row.as_ref().is_some_and(|row| row.ignored),
                state: row.as_ref().map_or("absent", learning_pair_state),
                stored_scope: row.as_ref().map(phase_row_scope_value),
                row_sha256: row_json.map(|bytes| hex::encode(Sha256::digest(bytes))),
            });
        }
        let settings = read_private_bytes(&self.root.join("settings.json"))?;
        Ok(LearningPhaseArmSnapshotV3 {
            schema_version: 3,
            plan_id: plan.plan_id.clone(),
            arm_state_id: plan.arm_state_id.clone(),
            identity_sha256: self.identity_sha256.clone(),
            settings_sha256: hex::encode(Sha256::digest(settings)),
            settings_consistency:
                "all_declared_scope_mappings_preserved_global_dictionary_unchanged",
            pairs,
        })
    }

    fn execute_phase_text(
        &mut self,
        plan: &learning_phase_v3::LearningPhasePlanV3,
        phase: &learning_phase_v3::LearningPhaseV3,
        operation_id: &str,
        states: &[LearningTextStateInputV2],
    ) -> anyhow::Result<serde_json::Value> {
        use learning_phase_v3::LearningPhaseActionV3;
        use sha2::{Digest, Sha256};

        validate_owned_arm_storage(&self.root)?;
        validate_phase_arm_settings(&self.settings, plan)?;
        let LearningPhaseActionV3::LearnText {
            operation_ids,
            term_fixture_id,
        } = &phase.action
        else {
            anyhow::bail!("learning_phase_action_mismatch");
        };
        if !phase.local_only_grant
            || !phase.source_text_learning_authorized
            || !phase.learning_authorized
            || phase.local_replacement_authorized
            || phase.undo_authorized
            || phase.actor != LearningActorV2::ProgrammaticFixture
            || phase.target_oracle != LearningOracleV2::None
            || !operation_ids
                .iter()
                .any(|authorized| authorized == operation_id)
        {
            anyhow::bail!("learning_phase_grant_or_operation_mismatch");
        }
        let source = phase
            .source
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("learning_phase_source_required"))?;
        if source.kind != LearningSourceKindV2::DeterministicText
            || source.rights == LearningRightsV2::PublicCcBySa4_0
        {
            anyhow::bail!("learning_phase_source_unsupported");
        }
        if states.len() != phase.edit_states.len()
            || !(2..=LEARNING_EDIT_STATES_MAX).contains(&states.len())
        {
            anyhow::bail!("learning_phase_edit_state_count_mismatch");
        }
        for (input, declared) in states.iter().zip(&phase.edit_states) {
            if input.sequence != declared.sequence
                || input.state_id != declared.state_id
                || input.layer != declared.layer
                || input.text.is_empty()
                || input.text.len() != declared.byte_length
                || input.text.len() > LEARNING_TEXT_MAX_BYTES
                || hex::encode(Sha256::digest(input.text.as_bytes())) != declared.sha256
            {
                anyhow::bail!("learning_phase_edit_state_mismatch");
            }
        }
        let post_insert = states
            .first()
            .ok_or_else(|| anyhow::anyhow!("learning_phase_edit_state_missing"))?;
        let settled = states
            .last()
            .ok_or_else(|| anyhow::anyhow!("learning_phase_edit_state_missing"))?;
        if post_insert.layer != LearningTextLayerV2::PostInsert
            || settled.layer != LearningTextLayerV2::Settled
            || post_insert.text.len() != source.byte_length
            || hex::encode(Sha256::digest(post_insert.text.as_bytes())) != source.sha256
        {
            anyhow::bail!("learning_phase_source_state_mismatch");
        }
        let term = plan
            .term(term_fixture_id)
            .ok_or_else(|| anyhow::anyhow!("learning_phase_term_not_declared"))?;
        let consent = LearningConsent {
            selected_state_id: phase.phase_id.clone(),
            baseline_run: None,
            edit_fixture_id: source.fixture_id.clone(),
            purpose: LearningPurpose::Correction,
            mapping_id: phase.scope.mapping_id.clone(),
            bundle_id: phase.scope.native_bundle.clone(),
            observer_enabled: true,
            post_insert_field: post_insert.text.clone(),
            settled_field: settled.text.clone(),
            terms: vec![term.clone()],
        };
        consent.validate().map_err(anyhow::Error::msg)?;
        let mapping = self
            .settings
            .context_mappings
            .iter()
            .find(|mapping| mapping.id == phase.scope.mapping_id)
            .ok_or_else(|| anyhow::anyhow!("learning_phase_mapping_not_bound"))?;
        if mapping.bundle_id.as_deref() != Some(phase.scope.native_bundle.as_str())
            || mapping.family != phase.scope.family
        {
            anyhow::bail!("learning_phase_mapping_identity_mismatch");
        }

        let key = crate::dictionary_learn::pair_key(&term.before, &term.after);
        let state_before = self.selected_state(plan)?;
        let before_row = store::get_learn_pair(&self.root, &key)?;
        let ordered_states = states
            .iter()
            .map(|state| state.text.clone())
            .collect::<Vec<_>>();
        let dictionary_before = self.settings.dictionary.clone();
        let outcome = with_scoped_consent(consent.clone(), || {
            crate::dictionary_learn::record_controlled_fixture_edit_trajectory(
                &self.root,
                &mut self.settings,
                &phase.scope,
                &consent,
                crate::dictionary_learn::ControlledFixtureTrajectoryInput {
                    edit: crate::dictionary_learn::ControlledFixtureEditInput {
                        expected_before: &term.before,
                        expected_after: &term.after,
                        episode_id: &plan.plan_id,
                        source_sha256: &source.sha256,
                        operation_id,
                        stable_source_origin_id: None,
                    },
                    ordered_states: &ordered_states,
                },
            )
        })?;
        if self.settings.dictionary != dictionary_before {
            anyhow::bail!("learning_phase_local_only_mutated_global_dictionary");
        }
        validate_owned_arm_storage(&self.root)?;
        let after_row = store::get_learn_pair(&self.root, &key)?;
        if after_row.as_ref().is_some_and(|row| {
            row.before_surface != term.before
                || row.after_surface != term.after
                || !row_within_bounds(row)
                || !row_within_phase_plan_scope(row, plan)
        }) {
            anyhow::bail!("learning_phase_row_outside_declared_plan_scopes");
        }
        let owner_conflict = outcome.candidate_write == "candidate_row_scope_mismatch";
        Ok(serde_json::json!({
            "schema_version": 3,
            "plan_id": plan.plan_id,
            "arm_state_id": plan.arm_state_id,
            "phase_id": phase.phase_id,
            "operation_id": operation_id,
            "action": "learn_text",
            "source": {
                "kind": source.kind,
                "fixture_id": source.fixture_id,
                "sha256": source.sha256,
                "byte_length": source.byte_length,
                "rights": source.rights,
                "rights_provenance": "plan_declared",
            },
            "actor": phase.actor,
            "requested_scope": phase_scope_value(&phase.scope),
            "observer_outcome": outcome.observer_outcome,
            "trajectory_samples_observed": outcome.trajectory_samples_observed,
            "trajectory_states_consumed": outcome.trajectory_states_consumed,
            "trajectory_settled": outcome.trajectory_settled,
            "candidate_class": outcome.candidate_class,
            "candidate_write": outcome.candidate_write,
            "hits_before": before_row.as_ref().map(|row| row.hits).unwrap_or_default(),
            "hits_after": after_row.as_ref().map(|row| row.hits),
            "promoted": after_row.as_ref().map(store::LearnPairRecord::is_live_promoted),
            "actual_rule_id": after_row.as_ref().map(|row| row.pair_key.as_str()),
            "rule_id_kind": after_row.as_ref().map(|_| "legacy_normalized_pair_key"),
            "stored_scope": after_row.as_ref().map(phase_row_scope_value),
            "known_gap": if owner_conflict { Some("c1_legacy_pair_key_scope_conflict") } else { None },
            "sql_receipt": if owner_conflict { "not_committed_scope_conflict_no_hit" } else { outcome.candidate_write },
            "settings_readback": "unchanged_local_only_global_dictionary",
            "arm_settings_sha256_before": state_before.settings_sha256,
            "arm_settings_sha256_after": self.selected_state(plan)?.settings_sha256,
            "source_text_learning_authorized": phase.source_text_learning_authorized,
            "local_only_grant": phase.local_only_grant,
            "insertion": "not_run",
            "ax_observation": "not_run",
            "independent_target_oracle": "not_authorized_not_run",
            "asr_quality": "not_measured",
        }))
    }

    fn execute_phase_followup(
        &mut self,
        plan: &learning_phase_v3::LearningPhasePlanV3,
        phase: &learning_phase_v3::LearningPhaseV3,
        operation_id: &str,
        text: &str,
    ) -> anyhow::Result<serde_json::Value> {
        use learning_phase_v3::LearningPhaseActionV3;
        use sha2::{Digest, Sha256};

        validate_owned_arm_storage(&self.root)?;
        validate_phase_arm_settings(&self.settings, plan)?;
        let LearningPhaseActionV3::ApplyFollowup {
            task_id,
            operation_id: declared_operation_id,
            rule_owner_phase_id,
            term_fixture_id,
            expected_output_sha256,
            expected_output_byte_length,
        } = &phase.action
        else {
            anyhow::bail!("learning_phase_action_mismatch");
        };
        if !phase.local_only_grant
            || phase.source_text_learning_authorized
            || phase.learning_authorized
            || !phase.local_replacement_authorized
            || phase.undo_authorized
            || phase.actor != LearningActorV2::ProgrammaticFixture
            || phase.target_oracle != LearningOracleV2::None
            || declared_operation_id != operation_id
        {
            anyhow::bail!("learning_phase_grant_or_operation_mismatch");
        }
        let source = phase
            .source
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("learning_phase_source_required"))?;
        if source.kind != LearningSourceKindV2::DeterministicText
            || source.rights == LearningRightsV2::PublicCcBySa4_0
            || text.is_empty()
            || text.len() != source.byte_length
            || text.len() > LEARNING_TEXT_MAX_BYTES
            || hex::encode(Sha256::digest(text.as_bytes())) != source.sha256
        {
            anyhow::bail!("learning_phase_source_mismatch");
        }
        let owner_phase = plan
            .phase(rule_owner_phase_id)
            .ok_or_else(|| anyhow::anyhow!("learning_phase_rule_owner_missing"))?;
        if !matches!(
            &owner_phase.action,
            LearningPhaseActionV3::LearnText { term_fixture_id: owner_term, .. }
                if owner_term == term_fixture_id
        ) {
            anyhow::bail!("learning_phase_rule_owner_term_mismatch");
        }
        let term = plan
            .term(term_fixture_id)
            .ok_or_else(|| anyhow::anyhow!("learning_phase_term_not_declared"))?;
        let key = crate::dictionary_learn::pair_key(&term.before, &term.after);
        let state_before = self.selected_state(plan)?;
        let row = store::get_learn_pair(&self.root, &key)?;
        if let Some(row) = row.as_ref() {
            if row.before_surface != term.before
                || row.after_surface != term.after
                || !row_within_bounds(row)
                || !row_within_episode_scope(row, &owner_phase.scope)
            {
                anyhow::bail!("learning_phase_rule_owner_state_mismatch");
            }
            if row.is_live_promoted()
                && !phase_owner_learning_observations_present(&self.root, plan, owner_phase, &key)?
            {
                anyhow::bail!("learning_phase_owner_observations_missing");
            }
        }
        if !self.settings.context_mappings.iter().any(|mapping| {
            mapping.id == phase.scope.mapping_id
                && mapping.bundle_id.as_deref() == Some(phase.scope.native_bundle.as_str())
                && mapping.family == phase.scope.family
                && mapping.enabled
                && mapping.dictionary_learn_enabled
        }) {
            anyhow::bail!("learning_phase_application_scope_not_bound");
        }
        if crate::lexicon::is_default_learn_off_target(
            Some(phase.scope.native_bundle.as_str()),
            None,
        ) {
            anyhow::bail!("learning_phase_target_protected");
        }
        let application_scope = store::LearnPairScope {
            family: Some(crate::context::family_id(phase.scope.family).to_owned()),
            mapping_id: Some(phase.scope.mapping_id.clone()),
            browser_host: None,
            native_bundle: Some(phase.scope.native_bundle.clone()),
        };
        let scope_match = row.as_ref().map(|row| application_scope.assess_record(row));
        let applicable = row.as_ref().map_or_else(Vec::new, |row| {
            application_scope.applicable_promoted_records(std::slice::from_ref(row))
        });
        let replacement_pairs =
            crate::lexicon::replaceable_pairs(&applicable, &self.settings.dictionary);
        let (output_text, _, applied_pairs) =
            crate::lexicon::apply_explicit_replacements_with_facts(
                text,
                &replacement_pairs,
                &self.settings.dictionary,
            );
        if output_text.len() > LEARNING_OUTPUT_MAX_BYTES {
            anyhow::bail!("learning_phase_output_bounds");
        }
        let output_sha256 = hex::encode(Sha256::digest(output_text.as_bytes()));
        let expected_output_match = output_text.len() == *expected_output_byte_length
            && output_sha256 == *expected_output_sha256;
        let selected_pair_applied = applied_pairs
            .iter()
            .any(|pair| pair.before == term.before && pair.after == term.after);
        let applied_pair_facts = applied_pairs
            .into_iter()
            .map(|pair| LearningAppliedPairV2 {
                before: pair.before,
                after: pair.after,
            })
            .collect::<Vec<_>>();
        let selected_pair_attribution = if selected_pair_applied {
            "selected_learning_pair"
        } else if expected_output_match && output_text == text {
            "no_pair_applied"
        } else {
            "unknown"
        };
        let observation_origin =
            phase_application_origin(plan, phase, operation_id, &source.sha256);
        let positive_observation =
            store::LearnPairObservation::research_fixture_apply(&observation_origin, &key);
        let no_op_observation =
            store::LearnPairObservation::research_fixture_noop(&observation_origin, &key);
        let positive_observation_recorded =
            store::has_learn_pair_observation(&self.root, &positive_observation)?;
        let no_op_observation_recorded =
            store::has_learn_pair_observation(&self.root, &no_op_observation)?;
        if positive_observation_recorded && no_op_observation_recorded {
            anyhow::bail!("learning_phase_application_receipt_conflict");
        }
        let (observation, observation_kind, record_usage) = if positive_observation_recorded {
            (&positive_observation, "selected_pair_application", false)
        } else if no_op_observation_recorded {
            (&no_op_observation, "no_pair_application", false)
        } else if selected_pair_applied {
            (&positive_observation, "selected_pair_application", true)
        } else {
            (&no_op_observation, "no_pair_application", false)
        };
        validate_owned_arm_storage(&self.root)?;
        let inserted = store::record_research_fixture_application(
            &self.root,
            &key,
            &term.after,
            observation,
            record_usage,
        )?;
        let usage_receipt = if record_usage && selected_pair_applied {
            if inserted {
                "selected_pair_usage_recorded"
            } else {
                "duplicate_operation_no_usage_increment"
            }
        } else if positive_observation_recorded || no_op_observation_recorded {
            "duplicate_operation_no_usage_increment"
        } else if inserted {
            "operation_receipt_recorded_without_pair_usage"
        } else {
            "duplicate_operation_without_usage_increment"
        };
        validate_owned_arm_storage(&self.root)?;
        let row_after = store::get_learn_pair(&self.root, &key)?;
        if row_after.as_ref().is_some_and(|row| {
            row.before_surface != term.before
                || row.after_surface != term.after
                || !row_within_bounds(row)
                || !row_within_phase_plan_scope(row, plan)
        }) {
            anyhow::bail!("learning_phase_rule_changed_during_application");
        }
        let scope_assessment = match scope_match {
            Some(store::LearnPairScopeMatch::Match) => "match",
            Some(store::LearnPairScopeMatch::Mismatch) => "mismatch",
            Some(store::LearnPairScopeMatch::Unknown) | None => "unknown",
        };
        let state_after = self.selected_state(plan)?;
        let output_byte_length = output_text.len();
        Ok(serde_json::json!({
            "schema_version": 3,
            "plan_id": plan.plan_id,
            "arm_state_id": plan.arm_state_id,
            "phase_id": phase.phase_id,
            "task_id": task_id,
            "operation_id": operation_id,
            "term_fixture_id": term_fixture_id,
            "rule_owner_phase_id": rule_owner_phase_id,
            "application_scope": phase_scope_value(&phase.scope),
            "declared_rule_owner_scope": phase_scope_value(&owner_phase.scope),
            "rule_owner_scope_provenance": "plan_declared",
            "requested_scope_assessment": scope_assessment,
            "stored_scope": row_after.as_ref().map(phase_row_scope_value),
            "actual_rule_id": row_after.as_ref().map(|row| row.pair_key.as_str()),
            "rule_id_kind": if row_after.is_some() { "legacy_normalized_pair_key" } else { "unknown" },
            "observed_rule_state": row_after.as_ref().map_or("absent", learning_pair_state),
            "scope_application": if applicable.is_empty() { "not_applicable" } else { "applicable" },
            "scope_filter_semantics": "normal_production_applicability_filter",
            "output_text": output_text,
            "output_sha256": output_sha256,
            "output_byte_length": output_byte_length,
            "expected_output_sha256": expected_output_sha256,
            "expected_output_byte_length": expected_output_byte_length,
            "expected_output_match": expected_output_match,
            "applied_pairs": applied_pair_facts,
            "selected_pair_attribution": selected_pair_attribution,
            "application_receipt_kind": observation_kind,
            "usage_receipt": usage_receipt,
            "sql_settings_effect": "sql_only_global_settings_unchanged",
            "settings_sha256_before": state_before.settings_sha256,
            "settings_sha256_after": state_after.settings_sha256,
            "pipeline_final": "not_run",
            "insertion": "not_run",
            "ax_observation": "not_run",
            "independent_target_oracle": "not_run",
            "asr_quality": "not_measured",
        }))
    }

    fn execute_phase_undo(
        &mut self,
        plan: &learning_phase_v3::LearningPhasePlanV3,
        phase: &learning_phase_v3::LearningPhaseV3,
        operation_id: &str,
    ) -> anyhow::Result<serde_json::Value> {
        use learning_phase_v3::LearningPhaseActionV3;

        validate_owned_arm_storage(&self.root)?;
        validate_phase_arm_settings(&self.settings, plan)?;
        let LearningPhaseActionV3::Undo {
            operation_id: declared_operation_id,
            rule_owner_phase_id,
            term_fixture_id,
            required_application_phase_id,
        } = &phase.action
        else {
            anyhow::bail!("learning_phase_action_mismatch");
        };
        if !phase.local_only_grant
            || phase.source.is_some()
            || phase.source_text_learning_authorized
            || phase.learning_authorized
            || phase.local_replacement_authorized
            || !phase.undo_authorized
            || phase.actor != LearningActorV2::ProgrammaticFixture
            || phase.target_oracle != LearningOracleV2::None
            || declared_operation_id != operation_id
        {
            anyhow::bail!("learning_phase_undo_grant_or_operation_mismatch");
        }
        let owner_phase = plan
            .phase(rule_owner_phase_id)
            .ok_or_else(|| anyhow::anyhow!("learning_phase_rule_owner_missing"))?;
        if phase.scope != owner_phase.scope {
            anyhow::bail!("learning_phase_undo_owner_scope_grant_mismatch");
        }
        let LearningPhaseActionV3::LearnText {
            term_fixture_id: owner_term,
            ..
        } = &owner_phase.action
        else {
            anyhow::bail!("learning_phase_rule_owner_action_mismatch");
        };
        if owner_term != term_fixture_id {
            anyhow::bail!("learning_phase_rule_owner_term_mismatch");
        }
        let application_phase = plan
            .phase(required_application_phase_id)
            .ok_or_else(|| anyhow::anyhow!("learning_phase_required_application_missing"))?;
        let LearningPhaseActionV3::ApplyFollowup {
            operation_id: apply_operation_id,
            rule_owner_phase_id: apply_owner_id,
            term_fixture_id: apply_term_id,
            ..
        } = &application_phase.action
        else {
            anyhow::bail!("learning_phase_required_application_mismatch");
        };
        if application_phase.scope != phase.scope
            || apply_owner_id != rule_owner_phase_id
            || apply_term_id != term_fixture_id
        {
            anyhow::bail!("learning_phase_required_application_scope_mismatch");
        }
        let source = application_phase
            .source
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("learning_phase_application_source_missing"))?;
        let term = plan
            .term(term_fixture_id)
            .ok_or_else(|| anyhow::anyhow!("learning_phase_term_not_declared"))?;
        let key = crate::dictionary_learn::pair_key(&term.before, &term.after);
        let state_before = self.selected_state(plan)?;
        let dictionary_before = self.settings.dictionary.clone();
        let row = store::get_learn_pair(&self.root, &key)?
            .ok_or_else(|| anyhow::anyhow!("learning_phase_rule_not_found"))?;
        if row.before_surface != term.before
            || row.after_surface != term.after
            || !row_within_bounds(&row)
            || !row_within_episode_scope(&row, &owner_phase.scope)
            || !row.auto_scoped
        {
            anyhow::bail!("learning_phase_undo_rule_owner_mismatch");
        }
        if !phase_owner_learning_observations_present(&self.root, plan, owner_phase, &key)? {
            anyhow::bail!("learning_phase_owner_observations_missing");
        }
        let apply_observation = store::LearnPairObservation::research_fixture_apply(
            &phase_application_origin(plan, application_phase, apply_operation_id, &source.sha256),
            &key,
        );
        let undo_observation = store::LearnPairObservation::research_fixture_undo(
            &format!("{}:{}", plan.plan_id, operation_id),
            &key,
        );
        let undo_already_recorded =
            store::has_learn_pair_observation(&self.root, &undo_observation)?;
        if !undo_already_recorded
            && !store::has_learn_pair_observation(&self.root, &apply_observation)?
        {
            anyhow::bail!("learning_phase_undo_requires_prior_application");
        }
        let state_before_undo = learning_pair_state(&row);
        if undo_already_recorded {
            if state_before_undo != "tombstoned" {
                anyhow::bail!("learning_phase_undo_receipt_state_mismatch");
            }
        } else {
            if state_before_undo != "promoted" {
                anyhow::bail!("learning_phase_undo_rule_not_live");
            }
            let usages = store::list_learned_term_usage(&self.root)?;
            if !usages
                .iter()
                .any(|usage| usage.word == term.after && usage.replacement_runs > 0)
            {
                anyhow::bail!("learning_phase_undo_requires_selected_usage");
            }
        }
        let rows_before_undo = store::list_learn_pairs(&self.root)?;
        if crate::dictionary_learn::should_remove_dictionary_entry_after_undo(
            &row,
            &rows_before_undo,
        ) {
            anyhow::bail!("learning_phase_global_dictionary_undo_unsupported");
        }
        let (tombstoned, duplicate) = if undo_already_recorded {
            (false, true)
        } else {
            validate_owned_arm_storage(&self.root)?;
            store::tombstone_research_learn_pair(&self.root, &key, &undo_observation)?
        };
        if !duplicate && !tombstoned {
            anyhow::bail!("learning_phase_undo_not_tombstoned");
        }
        validate_owned_arm_storage(&self.root)?;
        let row_after = store::get_learn_pair(&self.root, &key)?
            .ok_or_else(|| anyhow::anyhow!("learning_phase_rule_disappeared"))?;
        if row_after.before_surface != term.before
            || row_after.after_surface != term.after
            || !row_within_episode_scope(&row_after, &owner_phase.scope)
            || learning_pair_state(&row_after) != "tombstoned"
        {
            anyhow::bail!("learning_phase_undo_readback_mismatch");
        }
        let state_after = self.selected_state(plan)?;
        let settings_sha256_before = state_before.settings_sha256.clone();
        let settings_sha256_after = state_after.settings_sha256.clone();
        if settings_sha256_before != settings_sha256_after
            || self.settings.dictionary != dictionary_before
        {
            anyhow::bail!("learning_phase_undo_changed_global_settings");
        }
        Ok(serde_json::json!({
            "schema_version": 3,
            "plan_id": plan.plan_id,
            "arm_state_id": plan.arm_state_id,
            "phase_id": phase.phase_id,
            "operation_id": operation_id,
            "term_fixture_id": term_fixture_id,
            "rule_owner_phase_id": rule_owner_phase_id,
            "required_application_phase_id": required_application_phase_id,
            "requested_scope": phase_scope_value(&phase.scope),
            "stored_scope": phase_row_scope_value(&row_after),
            "actual_rule_id": row_after.pair_key,
            "rule_id_kind": "legacy_normalized_pair_key",
            "state_before": state_before_undo,
            "state_after": "tombstoned",
            "sql_outcome": if duplicate { "duplicate_no_tombstone_refresh" } else { "tombstone_and_receipt_committed" },
            "undo_authorized": phase.undo_authorized,
            "undo_owner_scope_match": true,
            "settings_readback": "unchanged_auto_scoped_dictionary_retained",
            "settings_sha256_before": settings_sha256_before,
            "settings_sha256_after": state_after.settings_sha256,
            "pipeline_final": "not_run",
            "insertion": "not_run",
            "ax_observation": "not_run",
            "independent_target_oracle": "not_run",
        }))
    }
}

#[cfg(any(test, feature = "research-harness"))]
fn unique_phase_scopes(
    plan: &learning_phase_v3::LearningPhasePlanV3,
) -> anyhow::Result<Vec<LearningScopeV2>> {
    let mut scopes = Vec::<LearningScopeV2>::new();
    for phase in &plan.phases {
        if let Some(existing) = scopes
            .iter()
            .find(|scope| scope.mapping_id == phase.scope.mapping_id)
        {
            if existing != &phase.scope {
                anyhow::bail!("learning phase mapping identity changed within plan");
            }
        } else {
            scopes.push(phase.scope.clone());
        }
    }
    Ok(scopes)
}

#[cfg(any(test, feature = "research-harness"))]
fn phase_fixture_mapping(scope: &LearningScopeV2) -> crate::context::AppMapping {
    crate::context::AppMapping {
        id: scope.mapping_id.clone(),
        label: "Controlled learning phase fixture".into(),
        family: scope.family,
        mode_id: None,
        bundle_id: Some(scope.native_bundle.clone()),
        executable: None,
        browser_host: None,
        browser_path_prefix: None,
        focused_field: None,
        source_permissions: crate::context::ContextSourcePermissions::default(),
        style_example_input: None,
        style_example_output: None,
        style_example_pairs: Vec::new(),
        style_examples_approved: false,
        enabled: true,
        cleanup_effort: None,
        cleanup_intensity: None,
        cleanup_enabled: false,
        dictionary_learn_enabled: true,
    }
}

#[cfg(any(test, feature = "research-harness"))]
fn initial_phase_arm_settings(
    plan: &learning_phase_v3::LearningPhasePlanV3,
) -> anyhow::Result<store::Settings> {
    let settings = store::Settings {
        dictionary_learn_enabled: true,
        local_edit_learning_enabled: true,
        context_enabled: true,
        browser_access_enabled: false,
        window_ocr_enabled: false,
        always_on_microphone: false,
        context_mappings: unique_phase_scopes(plan)?
            .iter()
            .map(phase_fixture_mapping)
            .collect(),
        ..store::Settings::default()
    };
    Ok(settings)
}

#[cfg(any(test, feature = "research-harness"))]
fn validate_phase_arm_settings(
    settings: &store::Settings,
    plan: &learning_phase_v3::LearningPhasePlanV3,
) -> anyhow::Result<()> {
    let expected_mappings = unique_phase_scopes(plan)?
        .iter()
        .map(phase_fixture_mapping)
        .collect::<Vec<_>>();
    if !settings.api_key.is_empty()
        || !settings.asr_api_key.is_empty()
        || !settings.asr_base_url.is_empty()
        || !settings.cleanup_api_key.is_empty()
        || !settings.cleanup_base_url.is_empty()
        || !settings.provider_api_keys.is_empty()
        || !settings.snippets.is_empty()
        || settings
            .dictionary
            .iter()
            .any(|word| !plan.terms.iter().any(|term| term.after == *word))
        || settings
            .dictionary
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != settings.dictionary.len()
        || settings.browser_access_enabled
        || settings.window_ocr_enabled
        || settings.always_on_microphone
        || !settings.context_enabled
        || !settings.dictionary_learn_enabled
        || !settings.local_edit_learning_enabled
        || !settings.unverified_credential_sources.is_empty()
        || !settings.credential_baselines.is_empty()
        || settings.session_only_custom_credential
        || settings.session_custom_credential_scope.is_some()
        || settings.session_custom_credential_rejected
        || settings.context_mappings != expected_mappings
    {
        anyhow::bail!("learning phase arm settings mismatch");
    }
    Ok(())
}

#[cfg(any(test, feature = "research-harness"))]
fn row_within_phase_plan_scope(
    row: &store::LearnPairRecord,
    plan: &learning_phase_v3::LearningPhasePlanV3,
) -> bool {
    plan.phases
        .iter()
        .any(|phase| row_within_episode_scope(row, &phase.scope))
}

#[cfg(any(test, feature = "research-harness"))]
fn phase_row_scope_value(row: &store::LearnPairRecord) -> serde_json::Value {
    serde_json::json!({
        "family": row.family.as_deref(),
        "mapping_id": row.mapping_id.as_deref(),
        "browser_host": row.browser_host.as_deref(),
        "native_bundle": row.native_bundle.as_deref(),
    })
}

#[cfg(any(test, feature = "research-harness"))]
fn phase_scope_value(scope: &LearningScopeV2) -> serde_json::Value {
    serde_json::json!({
        "mapping_id": scope.mapping_id.as_str(),
        "family": crate::context::family_id(scope.family),
        "native_bundle": scope.native_bundle.as_str(),
        "identity_kind": "plan_declared_not_observed_target",
    })
}

#[cfg(any(test, feature = "research-harness"))]
fn phase_application_origin(
    plan: &learning_phase_v3::LearningPhasePlanV3,
    phase: &learning_phase_v3::LearningPhaseV3,
    operation_id: &str,
    source_sha256: &str,
) -> String {
    format!(
        "{}:{}:{}:{}",
        plan.plan_id, phase.phase_id, operation_id, source_sha256
    )
}

#[cfg(feature = "research-harness")]
fn phase_action_operation_ids(action: &learning_phase_v3::LearningPhaseActionV3) -> Vec<&str> {
    use learning_phase_v3::LearningPhaseActionV3;
    match action {
        LearningPhaseActionV3::LearnText { operation_ids, .. } => {
            operation_ids.iter().map(String::as_str).collect()
        }
        LearningPhaseActionV3::ApplyFollowup { operation_id, .. }
        | LearningPhaseActionV3::Undo { operation_id, .. }
        | LearningPhaseActionV3::ObserveTarget { operation_id }
        | LearningPhaseActionV3::RunOwnedPasteSession { operation_id } => {
            vec![operation_id.as_str()]
        }
    }
}

#[cfg(any(test, feature = "research-harness"))]
fn phase_owner_learning_observations_present(
    root: &Path,
    plan: &learning_phase_v3::LearningPhasePlanV3,
    owner_phase: &learning_phase_v3::LearningPhaseV3,
    pair_key: &str,
) -> anyhow::Result<bool> {
    use learning_phase_v3::LearningPhaseActionV3;
    let LearningPhaseActionV3::LearnText { operation_ids, .. } = &owner_phase.action else {
        return Ok(false);
    };
    let Some(source) = owner_phase.source.as_ref() else {
        return Ok(false);
    };
    for operation_id in operation_ids {
        let observation_id = format!("{}:{}:{}", plan.plan_id, operation_id, source.sha256);
        let observation =
            store::LearnPairObservation::research_fixture_edit(&observation_id, pair_key);
        if !store::has_learn_pair_observation(root, &observation)? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(feature = "research-harness")]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StandaloneLearningRequest {
    request_id: String,
    command: StandaloneLearningCommand,
}

#[cfg(feature = "research-harness")]
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum StandaloneLearningCommand {
    Prepare {
        episode: Box<LearningEpisodeV2>,
    },
    PreparePlan {
        plan: Box<learning_phase_v3::LearningPhasePlanV3>,
    },
    EnterPhase {
        plan_id: String,
        phase_id: String,
    },
    ApplyPhaseText {
        plan_id: String,
        phase_id: String,
        operation_id: String,
        states: Vec<LearningTextStateInputV2>,
    },
    ApplyPhaseFollowup {
        plan_id: String,
        phase_id: String,
        operation_id: String,
        text: String,
    },
    UndoPhase {
        plan_id: String,
        phase_id: String,
        operation_id: String,
    },
    EndPhase {
        plan_id: String,
        phase_id: String,
    },
    ObserveTarget {
        plan_id: String,
        phase_id: String,
        operation_id: String,
    },
    RunOwnedPasteSession {
        plan_id: String,
        phase_id: String,
        operation_id: String,
    },
    EndPlan {
        plan_id: String,
    },
    ApplyText {
        operation: LearningTextOperationV2,
    },
    ApplyWav {
        task_id: String,
        operation_id: String,
        #[serde(default)]
        settled_text: Option<String>,
    },
    ApplyFollowup {
        task_id: String,
        operation_id: String,
        text: String,
    },
    Undo {
        operation_id: String,
    },
    End {
        episode_id: String,
    },
    Quit,
}

#[cfg(feature = "research-harness")]
struct PreparedLearningEpisode {
    episode: LearningEpisodeV2,
    arm: LearningArmV2,
    expires: std::time::Instant,
}

#[cfg(feature = "research-harness")]
struct ActiveLearningPhaseLeaseV3 {
    phase_index: usize,
    phase_id: String,
    expires: std::time::Instant,
    completed_operation_ids: std::collections::HashSet<String>,
}

#[cfg(feature = "research-harness")]
struct PreparedLearningPhasePlanV3 {
    plan: learning_phase_v3::LearningPhasePlanV3,
    arm: LearningArmV3,
    next_phase_index: usize,
    active_phase: Option<ActiveLearningPhaseLeaseV3>,
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
struct ActiveOwnedTargetPhaseV1 {
    phase_index: usize,
    phase_id: String,
    expires: std::time::Instant,
    completed_operation_ids: std::collections::HashSet<String>,
    attempted_operation_ids: std::collections::HashSet<String>,
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
struct PreparedOwnedTargetPlanV1 {
    plan: learning_phase_v3::LearningPhasePlanV3,
    native_oracle: learning_native_oracle::PreparedOwnedTargetOracleV1,
    next_phase_index: usize,
    active_phase: Option<ActiveOwnedTargetPhaseV1>,
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
struct ActiveOwnedPasteSessionPhaseV1 {
    phase_index: usize,
    phase_id: String,
    expires: std::time::Instant,
    operation_id: String,
    attempted: bool,
    completed: bool,
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
struct PreparedOwnedPasteSessionPlanV1 {
    plan: learning_phase_v3::LearningPhasePlanV3,
    prepared: learning_native_paste_session::PreparedOwnedPasteSessionV1,
    active_phase: Option<ActiveOwnedPasteSessionPhaseV1>,
    next_phase_index: usize,
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
struct PendingOwnedPasteGeneralOwnerV1 {
    owner: crate::paste::ResearchClipboardGuardOwnerV1,
    restore_allowed_after_host_terminal: bool,
}

#[cfg(feature = "research-harness")]
struct StandaloneLearningDispatcher {
    base_root: PathBuf,
    wav_root: Option<PathBuf>,
    model_source_root: Option<PathBuf>,
    native_oracle_manifest: Option<PathBuf>,
    owned_paste_session_manifest: Option<PathBuf>,
    owned_paste_session_supervisor: bool,
    prepared: Option<PreparedLearningEpisode>,
    prepared_plan: Option<PreparedLearningPhasePlanV3>,
    #[cfg(target_os = "macos")]
    prepared_target_plan: Option<PreparedOwnedTargetPlanV1>,
    #[cfg(target_os = "macos")]
    prepared_owned_paste_session_plan: Option<PreparedOwnedPasteSessionPlanV1>,
    #[cfg(target_os = "macos")]
    pending_owned_paste_general_owner: Option<PendingOwnedPasteGeneralOwnerV1>,
    attempted_wav_operations: std::collections::HashSet<(String, String)>,
}

#[cfg(feature = "research-harness")]
impl StandaloneLearningDispatcher {
    fn new(config: StandaloneLearningConfig) -> Self {
        Self {
            base_root: config.base_root,
            wav_root: config.wav_root,
            model_source_root: config.model_source_root,
            native_oracle_manifest: config.native_oracle_manifest,
            owned_paste_session_manifest: config.owned_paste_session_manifest,
            owned_paste_session_supervisor: config.owned_paste_session_supervisor,
            prepared: None,
            prepared_plan: None,
            #[cfg(target_os = "macos")]
            prepared_target_plan: None,
            #[cfg(target_os = "macos")]
            prepared_owned_paste_session_plan: None,
            #[cfg(target_os = "macos")]
            pending_owned_paste_general_owner: None,
            attempted_wav_operations: std::collections::HashSet::new(),
        }
    }

    fn has_prepared_target_plan(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.prepared_target_plan.is_some() || self.prepared_owned_paste_session_plan.is_some()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    #[cfg(target_os = "macos")]
    fn resolve_pending_owned_paste_owner_command(
        &mut self,
        command: &str,
    ) -> Option<serde_json::Value> {
        let pending = self.pending_owned_paste_general_owner.as_mut()?;
        let host_terminal_confirmed = pending.restore_allowed_after_host_terminal;
        let requested_restore = command == "restore_if_owned";
        if command != "check_current" && !requested_restore {
            return Some(serde_json::json!({
                "ok": false,
                "reason": "owned_paste_general_owner_command_rejected",
                "owner_retained": true,
            }));
        }
        if requested_restore && !host_terminal_confirmed {
            return Some(serde_json::json!({
                "ok": false,
                "reason": "owned_paste_host_terminal_not_confirmed",
                "owner_retained": true,
                "restore": "not_attempted",
            }));
        }
        let result = if requested_restore {
            pending.owner.restore_if_owned()
        } else {
            pending.owner.current_ownership()
        };
        let resolved = matches!(
            result,
            crate::learning_clipboard_guard::ClipboardGuardReplyV1::Restored
                | crate::learning_clipboard_guard::ClipboardGuardReplyV1::NotWritten
                | crate::learning_clipboard_guard::ClipboardGuardReplyV1::OwnershipChanged
        );
        let result = if resolved && host_terminal_confirmed {
            pending.owner.close_after_restore()
        } else {
            result
        };
        let retained = !matches!(
            result,
            crate::learning_clipboard_guard::ClipboardGuardReplyV1::Restored
                | crate::learning_clipboard_guard::ClipboardGuardReplyV1::NotWritten
                | crate::learning_clipboard_guard::ClipboardGuardReplyV1::OwnershipChanged
        ) || !host_terminal_confirmed;
        if !retained {
            self.pending_owned_paste_general_owner = None;
        }
        Some(serde_json::json!({
            "ok": !retained,
            "owner_result": result,
            "owner_retained": retained,
            "host_terminal_confirmed": host_terminal_confirmed,
        }))
    }

    #[cfg(target_os = "macos")]
    fn hold_process_while_general_owner_is_pending(&self) {
        if self.pending_owned_paste_general_owner.is_some() {
            // EOF, output failure, or the request budget cannot authorize us
            // to drop the only in-memory General clipboard snapshot.
            loop {
                std::thread::park_timeout(std::time::Duration::from_secs(60));
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn resolve_pending_owned_paste_owner_command(
        &mut self,
        _command: &str,
    ) -> Option<serde_json::Value> {
        None
    }

    #[cfg(not(target_os = "macos"))]
    fn hold_process_while_general_owner_is_pending(&self) {}

    fn dispatch(
        &mut self,
        command: StandaloneLearningCommand,
    ) -> Result<serde_json::Value, String> {
        match command {
            StandaloneLearningCommand::PreparePlan { plan } => self.prepare_phase_plan(*plan),
            StandaloneLearningCommand::EnterPhase { plan_id, phase_id } => {
                self.enter_learning_phase(&plan_id, &phase_id)
            }
            StandaloneLearningCommand::ApplyPhaseText {
                plan_id,
                phase_id,
                operation_id,
                states,
            } => self.apply_phase_text(&plan_id, &phase_id, &operation_id, &states),
            StandaloneLearningCommand::ApplyPhaseFollowup {
                plan_id,
                phase_id,
                operation_id,
                text,
            } => self.apply_phase_followup(&plan_id, &phase_id, &operation_id, &text),
            StandaloneLearningCommand::UndoPhase {
                plan_id,
                phase_id,
                operation_id,
            } => self.undo_learning_phase(&plan_id, &phase_id, &operation_id),
            StandaloneLearningCommand::EndPhase { plan_id, phase_id } => {
                self.end_learning_phase(&plan_id, &phase_id)
            }
            StandaloneLearningCommand::ObserveTarget {
                plan_id,
                phase_id,
                operation_id,
            } => self.observe_target_phase(&plan_id, &phase_id, &operation_id),
            StandaloneLearningCommand::RunOwnedPasteSession {
                plan_id,
                phase_id,
                operation_id,
            } => self.run_owned_paste_session_phase(&plan_id, &phase_id, &operation_id),
            StandaloneLearningCommand::EndPlan { plan_id } => self.end_phase_plan(&plan_id),
            StandaloneLearningCommand::Prepare { episode } => {
                if self.native_oracle_manifest.is_some() {
                    return Err("native_oracle_manifest_requires_phase_plan".into());
                }
                if self.owned_paste_session_manifest.is_some() {
                    return Err("owned_paste_session_manifest_requires_phase_plan".into());
                }
                if self.prepared.is_some()
                    || self.prepared_plan.is_some()
                    || self.has_prepared_target_plan()
                {
                    return Err("end_previous_episode_first".into());
                }
                if let Err(reason) = episode.validate() {
                    if is_unsupported_learning_reason(reason) {
                        return Ok(serde_json::json!({
                            "admission": "unsupported",
                            "reason": reason,
                            "episode_id": episode.episode_id,
                            "arm_state_id": episode.arm_state_id,
                            "source_kind": episode.source.kind,
                            "insertion": "not_run",
                            "ax_observation": "not_run",
                            "independent_target_oracle": "not_authorized_not_run",
                            "asr_quality": "not_measured",
                            "owned_wav_learning_suppressed": true,
                        }));
                    }
                    return Err(reason.into());
                }
                if !episode.wav_operations.is_empty()
                    && (self.wav_root.is_none() || self.model_source_root.is_none())
                {
                    return Err("learning_wav_roots_required".into());
                }
                let arm = LearningArmV2::open(&self.base_root, &episode)
                    .map_err(|_| "learning_arm_open_failed".to_owned())?;
                let snapshot = arm
                    .selected_state(&episode)
                    .map_err(|_| "selected_learning_state_unavailable".to_owned())?;
                let result = serde_json::json!({
                    "admission": "prepared",
                    "execution": "not_started",
                    "episode_id": episode.episode_id,
                    "arm_state_id": episode.arm_state_id,
                    "arm_identity_sha256": arm.identity_sha256(),
                    "source": {
                        "kind": episode.source.kind,
                        "fixture_id": episode.source.fixture_id,
                        "sha256": episode.source.sha256,
                        "byte_length": episode.source.byte_length,
                        "rights": episode.source.rights,
                        "rights_provenance": "caller_declared",
                    },
                    "scope": {
                        "mapping_id": episode.scope.mapping_id,
                        "family": episode.scope.family,
                        "native_bundle": episode.scope.native_bundle,
                        "identity_kind": "fixture_declared_not_observed_target",
                    },
                    "selected_state": snapshot,
                    "insertion": "not_run",
                    "ax_observation": "not_run",
                    "independent_target_oracle": "not_authorized_not_run",
                    "asr_quality": "not_measured",
                    "expires_after_ms": episode.ttl_ms,
                });
                self.prepared = Some(PreparedLearningEpisode {
                    expires: std::time::Instant::now()
                        + std::time::Duration::from_millis(episode.ttl_ms),
                    episode: *episode,
                    arm,
                });
                Ok(result)
            }
            StandaloneLearningCommand::ApplyText { operation } => {
                if self
                    .prepared
                    .as_ref()
                    .is_some_and(|prepared| std::time::Instant::now() >= prepared.expires)
                {
                    self.prepared = None;
                    return Err("learning_episode_expired".into());
                }
                let prepared = self
                    .prepared
                    .as_mut()
                    .ok_or_else(|| "learning_episode_not_prepared".to_owned())?;
                let receipt = prepared
                    .arm
                    .execute_text_operation(&prepared.episode, &operation)
                    .map_err(|error| {
                        let reason = error.to_string();
                        if reason.len() <= 64
                            && reason.bytes().all(|byte| {
                                byte.is_ascii_lowercase() || byte == b'_' || byte.is_ascii_digit()
                            })
                        {
                            reason
                        } else {
                            "learning_operation_rejected".into()
                        }
                    })?;
                serde_json::to_value(receipt)
                    .map_err(|_| "learning_receipt_serialization_failed".into())
            }
            StandaloneLearningCommand::ApplyFollowup {
                task_id,
                operation_id,
                text,
            } => {
                if self
                    .prepared
                    .as_ref()
                    .is_some_and(|prepared| std::time::Instant::now() >= prepared.expires)
                {
                    self.prepared = None;
                    return Err("learning_episode_expired".into());
                }
                let prepared = self
                    .prepared
                    .as_mut()
                    .ok_or_else(|| "learning_episode_not_prepared".to_owned())?;
                let receipt = prepared
                    .arm
                    .execute_followup(&prepared.episode, &task_id, &operation_id, &text)
                    .map_err(sanitize_learning_operation_error)?;
                serde_json::to_value(receipt)
                    .map_err(|_| "learning_receipt_serialization_failed".into())
            }
            StandaloneLearningCommand::ApplyWav {
                task_id,
                operation_id,
                settled_text,
            } => {
                if self
                    .prepared
                    .as_ref()
                    .is_some_and(|prepared| std::time::Instant::now() >= prepared.expires)
                {
                    self.prepared = None;
                    return Err("learning_episode_expired".into());
                }
                let prepared = self
                    .prepared
                    .as_mut()
                    .ok_or_else(|| "learning_episode_not_prepared".to_owned())?;
                let authorized_task = prepared
                    .episode
                    .wav_operation(&task_id)
                    .ok_or_else(|| "learning_wav_task_not_authorized".to_owned())?;
                if authorized_task.operation_id != operation_id {
                    return Err("learning_operation_not_authorized".into());
                }
                let attempted_key = (
                    prepared.episode.episode_id.clone(),
                    authorized_task.task_id.clone(),
                );
                let arm_state = prepared
                    .arm
                    .selected_state(&prepared.episode)
                    .map_err(sanitize_learning_operation_error)?;
                if arm_state.settings_consistency == "promoted_rule_ownership_unsupported" {
                    return Err("learning_global_rule_ownership_unsupported".into());
                }
                if arm_state.settings_consistency != "selected_local_only_settings_unchanged" {
                    return Err("learning_arm_settings_effect_unsupported".into());
                }
                if !self.attempted_wav_operations.insert(attempted_key) {
                    return Err("learning_wav_operation_already_attempted_restart_required".into());
                }
                let wav_root = self
                    .wav_root
                    .as_deref()
                    .ok_or_else(|| "learning_wav_roots_required".to_owned())?;
                let model_source_root = self
                    .model_source_root
                    .as_deref()
                    .ok_or_else(|| "learning_wav_roots_required".to_owned())?;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| "learning_wav_runtime_unavailable".to_owned())?;
                let receipt = runtime
                    .block_on(prepared.arm.execute_wav(
                        &prepared.episode,
                        &task_id,
                        &operation_id,
                        settled_text.as_deref(),
                        LearningWavResources {
                            wav_root,
                            model_source_root,
                            expires: prepared.expires,
                        },
                    ))
                    .map_err(sanitize_learning_operation_error)?;
                serde_json::to_value(receipt)
                    .map_err(|_| "learning_receipt_serialization_failed".into())
            }
            StandaloneLearningCommand::Undo { operation_id } => {
                if self
                    .prepared
                    .as_ref()
                    .is_some_and(|prepared| std::time::Instant::now() >= prepared.expires)
                {
                    self.prepared = None;
                    return Err("learning_episode_expired".into());
                }
                let prepared = self
                    .prepared
                    .as_mut()
                    .ok_or_else(|| "learning_episode_not_prepared".to_owned())?;
                let receipt = prepared
                    .arm
                    .execute_undo(&prepared.episode, &operation_id)
                    .map_err(sanitize_learning_operation_error)?;
                serde_json::to_value(receipt)
                    .map_err(|_| "learning_receipt_serialization_failed".into())
            }
            StandaloneLearningCommand::End { episode_id } => {
                let Some(prepared) = self.prepared.take() else {
                    return Err("learning_episode_not_prepared".into());
                };
                if prepared.episode.episode_id != episode_id {
                    self.prepared = Some(prepared);
                    return Err("learning_episode_mismatch".into());
                }
                Ok(serde_json::json!({"ended": true, "episode_id": episode_id}))
            }
            StandaloneLearningCommand::Quit => {
                if self.prepared.is_some()
                    || self.prepared_plan.is_some()
                    || self.has_prepared_target_plan()
                {
                    return Err("end_learning_episode_before_quit".into());
                }
                Ok(serde_json::json!({"quit_requested": true}))
            }
        }
    }

    fn discard_expired_phase_plan(&mut self) -> bool {
        let plan_expired = self
            .prepared_plan
            .as_ref()
            .and_then(|prepared| prepared.active_phase.as_ref())
            .is_some_and(|active| std::time::Instant::now() >= active.expires);
        if plan_expired {
            self.prepared_plan = None;
        }
        #[cfg(target_os = "macos")]
        let target_expired = self
            .prepared_target_plan
            .as_ref()
            .and_then(|prepared| prepared.active_phase.as_ref())
            .is_some_and(|active| std::time::Instant::now() >= active.expires);
        #[cfg(target_os = "macos")]
        let owned_paste_session_expired = self
            .prepared_owned_paste_session_plan
            .as_ref()
            .and_then(|prepared| prepared.active_phase.as_ref())
            .is_some_and(|active| std::time::Instant::now() >= active.expires);
        #[cfg(target_os = "macos")]
        if target_expired {
            self.prepared_target_plan = None;
        }
        #[cfg(target_os = "macos")]
        if owned_paste_session_expired {
            self.prepared_owned_paste_session_plan = None;
        }
        #[cfg(target_os = "macos")]
        {
            plan_expired || target_expired || owned_paste_session_expired
        }
        #[cfg(not(target_os = "macos"))]
        {
            plan_expired
        }
    }

    fn prepare_phase_plan(
        &mut self,
        plan: learning_phase_v3::LearningPhasePlanV3,
    ) -> Result<serde_json::Value, String> {
        if self.prepared.is_some()
            || self.prepared_plan.is_some()
            || self.has_prepared_target_plan()
        {
            return Err("end_previous_learning_state_first".into());
        }
        plan.validate().map_err(str::to_owned)?;
        let identity_sha256 = plan
            .identity_sha256()
            .map_err(|_| "learning_phase_plan_identity_failed".to_owned())?;
        use learning_phase_v3::LearningPhaseActionV3;
        let owned_paste_session_only = plan.phases.len() == 1
            && matches!(
                &plan.phases[0].action,
                LearningPhaseActionV3::RunOwnedPasteSession { .. }
            );
        if owned_paste_session_only {
            #[cfg(target_os = "macos")]
            {
                return self.prepare_owned_paste_session_plan(plan, identity_sha256);
            }
            #[cfg(not(target_os = "macos"))]
            {
                return Err("owned_paste_session_requires_macos".into());
            }
        }
        if self.owned_paste_session_manifest.is_some() {
            return Err("owned_paste_session_manifest_requires_registered_plan".into());
        }
        let target_only = plan
            .phases
            .iter()
            .all(|phase| matches!(phase.action, LearningPhaseActionV3::ObserveTarget { .. }));
        if target_only {
            #[cfg(target_os = "macos")]
            {
                return self.prepare_owned_target_plan(plan, identity_sha256);
            }
            #[cfg(not(target_os = "macos"))]
            {
                return Err("native_oracle_requires_macos".into());
            }
        }
        if self.native_oracle_manifest.is_some() {
            return Err("native_oracle_manifest_requires_observe_only_plan".into());
        }
        let arm = LearningArmV3::open(&self.base_root, &plan)
            .map_err(|_| "learning_phase_arm_open_failed".to_owned())?;
        let selected_state = arm
            .selected_state(&plan)
            .map_err(|_| "selected_learning_phase_state_unavailable".to_owned())?;
        let result = serde_json::json!({
            "admission": "prepared",
            "execution": "not_started",
            "schema_version": 3,
            "plan_id": plan.plan_id,
            "arm_state_id": plan.arm_state_id,
            "plan_identity_sha256": identity_sha256,
            "phase_count": plan.phases.len(),
            "phase_order": plan.phases.iter().map(|phase| phase.phase_id.as_str()).collect::<Vec<_>>(),
            "selected_state": selected_state,
            "learning_state": "isolated_arm_opened",
            "source_types_supported": ["deterministic_text"],
            "actor": "programmatic_fixture",
            "target_oracle": "none",
            "insertion": "not_run",
            "ax_observation": "not_run",
            "independent_target_oracle": "not_authorized_not_run",
            "asr_quality": "not_measured",
            "known_gaps": learning_phase_v3::LEARNING_PHASE_V3_KNOWN_GAPS,
        });
        self.prepared_plan = Some(PreparedLearningPhasePlanV3 {
            plan,
            arm,
            next_phase_index: 0,
            active_phase: None,
        });
        Ok(result)
    }

    #[cfg(target_os = "macos")]
    fn prepare_owned_target_plan(
        &mut self,
        plan: learning_phase_v3::LearningPhasePlanV3,
        identity_sha256: String,
    ) -> Result<serde_json::Value, String> {
        let manifest_path = self
            .native_oracle_manifest
            .as_deref()
            .ok_or_else(|| "native_oracle_prepared_manifest_required".to_owned())?;
        let native_oracle =
            learning_native_oracle::PreparedOwnedTargetOracleV1::load(manifest_path)?;
        let binding = plan
            .phases
            .first()
            .and_then(|phase| phase.target_binding.as_ref())
            .ok_or_else(|| "learning_phase_v3_observe_target_binding_required".to_owned())?;
        native_oracle.validate_binding(binding)?;
        let result = serde_json::json!({
            "admission": "prepared",
            "execution": "not_started",
            "schema_version": 3,
            "plan_id": plan.plan_id,
            "arm_state_id": plan.arm_state_id,
            "plan_identity_sha256": identity_sha256,
            "phase_count": plan.phases.len(),
            "phase_order": plan.phases.iter().map(|phase| phase.phase_id.as_str()).collect::<Vec<_>>(),
            "learning_state": "not_opened_no_learning_arm",
            "source_types_supported": [],
            "actor": "programmatic_fixture",
            "target_oracle": "registered_owned_host_component",
            "insertion": "not_run",
            "ax_observation": "not_run",
            "independent_target_oracle": "authorized_prepared_not_started",
            "asr_quality": "not_measured",
            "known_gaps": learning_phase_v3::LEARNING_PHASE_V3_KNOWN_GAPS,
        });
        self.prepared_target_plan = Some(PreparedOwnedTargetPlanV1 {
            plan,
            native_oracle,
            next_phase_index: 0,
            active_phase: None,
        });
        Ok(result)
    }

    #[cfg(target_os = "macos")]
    fn prepare_owned_paste_session_plan(
        &mut self,
        plan: learning_phase_v3::LearningPhasePlanV3,
        identity_sha256: String,
    ) -> Result<serde_json::Value, String> {
        if self.native_oracle_manifest.is_some() {
            return Err("owned_paste_session_manifest_conflicts_with_target_oracle".into());
        }
        let manifest_path = self
            .owned_paste_session_manifest
            .as_deref()
            .ok_or_else(|| "owned_paste_session_prepared_manifest_required".to_owned())?;
        let prepared =
            learning_native_paste_session::PreparedOwnedPasteSessionV1::load(manifest_path)?;
        let binding = plan
            .phases
            .first()
            .and_then(|phase| phase.owned_paste_session_binding.as_ref())
            .ok_or_else(|| "learning_phase_v3_owned_paste_session_binding_required".to_owned())?;
        prepared.validate_binding(binding)?;
        let result = serde_json::json!({
            "admission": "prepared",
            "execution": "not_started",
            "schema_version": 3,
            "plan_id": plan.plan_id,
            "arm_state_id": plan.arm_state_id,
            "plan_identity_sha256": identity_sha256,
            "phase_count": 1,
            "phase_order": plan.phases.iter().map(|phase| phase.phase_id.as_str()).collect::<Vec<_>>(),
            "artifact_identity": prepared.artifact_identity_summary(),
            "learning_state": "not_opened_no_learning_arm",
            "source_types_supported": [],
            "actor": "programmatic_fixture",
            "target_oracle": "registered_owned_paste_session",
            "host_setup": "not_started",
            "paste_owner": "not_started",
            "programmatic_edit": "not_started",
            "independent_final_ax": "not_started",
            "insertion": "not_run",
            "learning": "not_run",
            "sql": "not_run",
            "settings": "not_run",
            "product_target_terminal": "not_probed",
            "asr_quality": "not_measured",
        });
        self.prepared_owned_paste_session_plan = Some(PreparedOwnedPasteSessionPlanV1 {
            plan,
            prepared,
            active_phase: None,
            next_phase_index: 0,
        });
        Ok(result)
    }

    fn enter_learning_phase(
        &mut self,
        plan_id: &str,
        phase_id: &str,
    ) -> Result<serde_json::Value, String> {
        #[cfg(target_os = "macos")]
        if self.prepared_target_plan.is_some() {
            return self.enter_owned_target_phase(plan_id, phase_id);
        }
        #[cfg(target_os = "macos")]
        if self.prepared_owned_paste_session_plan.is_some() {
            return self.enter_owned_paste_session_phase(plan_id, phase_id);
        }
        if self.discard_expired_phase_plan() {
            return Err("learning_phase_expired_plan_reprepare_required".into());
        }
        let prepared = self
            .prepared_plan
            .as_mut()
            .ok_or_else(|| "learning_phase_plan_not_prepared".to_owned())?;
        if prepared.plan.plan_id != plan_id {
            return Err("learning_phase_plan_mismatch".into());
        }
        if prepared.active_phase.is_some() {
            return Err("end_active_learning_phase_first".into());
        }
        let phase_index = prepared.next_phase_index;
        let phase = prepared
            .plan
            .phases
            .get(phase_index)
            .ok_or_else(|| "learning_phase_plan_already_complete".to_owned())?;
        if phase.phase_id != phase_id {
            return Err("learning_phase_order_invalid".into());
        }
        let selected_state = prepared
            .arm
            .selected_state(&prepared.plan)
            .map_err(|_| "selected_learning_phase_state_unavailable".to_owned())?;
        let expires = std::time::Instant::now() + std::time::Duration::from_millis(phase.ttl_ms);
        let result = serde_json::json!({
            "admission": "phase_grant_bound",
            "plan_id": plan_id,
            "phase_id": phase_id,
            "phase_index": phase_index,
            "scope": phase_scope_value(&phase.scope),
            "source": phase.source.as_ref().map(|source| serde_json::json!({
                "kind": source.kind,
                "fixture_id": source.fixture_id,
                "sha256": source.sha256,
                "byte_length": source.byte_length,
                "rights": source.rights,
            })),
            "grants": {
                "local_only": phase.local_only_grant,
                "source_text_learning": phase.source_text_learning_authorized,
                "learning": phase.learning_authorized,
                "local_replacement": phase.local_replacement_authorized,
                "undo": phase.undo_authorized,
            },
            "actor": phase.actor,
            "target_oracle": phase.target_oracle,
            "expires_after_ms": phase.ttl_ms,
            "selected_state": selected_state,
            "insertion": "not_run",
            "ax_observation": "not_run",
            "independent_target_oracle": "not_run",
        });
        prepared.active_phase = Some(ActiveLearningPhaseLeaseV3 {
            phase_index,
            phase_id: phase_id.to_owned(),
            expires,
            completed_operation_ids: std::collections::HashSet::new(),
        });
        Ok(result)
    }

    #[cfg(target_os = "macos")]
    fn enter_owned_target_phase(
        &mut self,
        plan_id: &str,
        phase_id: &str,
    ) -> Result<serde_json::Value, String> {
        if self.discard_expired_phase_plan() {
            return Err("learning_phase_expired_plan_reprepare_required".into());
        }
        let prepared = self
            .prepared_target_plan
            .as_mut()
            .ok_or_else(|| "learning_phase_plan_not_prepared".to_owned())?;
        if prepared.plan.plan_id != plan_id {
            return Err("learning_phase_plan_mismatch".into());
        }
        if prepared.active_phase.is_some() {
            return Err("end_active_learning_phase_first".into());
        }
        let phase_index = prepared.next_phase_index;
        let phase = prepared
            .plan
            .phases
            .get(phase_index)
            .ok_or_else(|| "learning_phase_plan_already_complete".to_owned())?;
        if phase.phase_id != phase_id
            || !matches!(
                phase.action,
                learning_phase_v3::LearningPhaseActionV3::ObserveTarget { .. }
            )
        {
            return Err("learning_phase_order_invalid".into());
        }
        let expires = std::time::Instant::now() + std::time::Duration::from_millis(phase.ttl_ms);
        let result = serde_json::json!({
            "admission": "phase_grant_bound",
            "plan_id": plan_id,
            "phase_id": phase_id,
            "phase_index": phase_index,
            "scope": phase_scope_value(&phase.scope),
            "target_observation_grant": phase.target_observation_authorized,
            "target_binding": phase.target_binding.as_ref().map(|binding| serde_json::json!({
                "fixture_id": binding.fixture_id,
                "source_manifest_sha256": binding.source_manifest_sha256,
                "helper_executable_sha256": binding.helper_executable_sha256,
                "host_executable_sha256": binding.host_executable_sha256,
                "host_bundle_identifier": binding.host_bundle_identifier,
                "host_info_plist_sha256": binding.host_info_plist_sha256,
                "recipe_sha256": binding.recipe_sha256,
                "window_title": binding.window_title,
                "control_identifier": binding.control_identifier,
                "expected_value_sha256": binding.expected_value_sha256,
                "expected_value_byte_length": binding.expected_value_byte_length,
            })),
            "actor": phase.actor,
            "target_oracle": phase.target_oracle,
            "expires_after_ms": phase.ttl_ms,
            "learning_state": "not_opened_no_learning_arm",
            "insertion": "not_run",
            "product_target_terminal": "not_probed",
            "independent_target_oracle": "prepared_component_sample_only",
        });
        prepared.active_phase = Some(ActiveOwnedTargetPhaseV1 {
            phase_index,
            phase_id: phase_id.to_owned(),
            expires,
            completed_operation_ids: std::collections::HashSet::new(),
            attempted_operation_ids: std::collections::HashSet::new(),
        });
        Ok(result)
    }

    #[cfg(target_os = "macos")]
    fn enter_owned_paste_session_phase(
        &mut self,
        plan_id: &str,
        phase_id: &str,
    ) -> Result<serde_json::Value, String> {
        if self.discard_expired_phase_plan() {
            return Err("learning_phase_expired_plan_reprepare_required".into());
        }
        let prepared = self
            .prepared_owned_paste_session_plan
            .as_mut()
            .ok_or_else(|| "learning_phase_plan_not_prepared".to_owned())?;
        if prepared.plan.plan_id != plan_id {
            return Err("learning_phase_plan_mismatch".into());
        }
        if prepared.active_phase.is_some() {
            return Err("end_active_learning_phase_first".into());
        }
        let phase_index = prepared.next_phase_index;
        let phase = prepared
            .plan
            .phases
            .get(phase_index)
            .ok_or_else(|| "learning_phase_plan_already_complete".to_owned())?;
        let learning_phase_v3::LearningPhaseActionV3::RunOwnedPasteSession { operation_id } =
            &phase.action
        else {
            return Err("learning_phase_order_invalid".into());
        };
        if phase.phase_id != phase_id {
            return Err("learning_phase_order_invalid".into());
        }
        let expires = std::time::Instant::now() + std::time::Duration::from_millis(phase.ttl_ms);
        let result = serde_json::json!({
            "admission": "phase_grant_bound",
            "plan_id": plan_id,
            "phase_id": phase_id,
            "phase_index": phase_index,
            "scope": phase_scope_value(&phase.scope),
            "operation_id": operation_id,
            "owned_paste_session_grant": phase.owned_paste_session_authorized,
            "target_binding": phase.owned_paste_session_binding.as_ref().map(|binding| serde_json::json!({
                "fixture_id": binding.fixture_id,
                "source_manifest_sha256": binding.source_manifest_sha256,
                "helper_executable_sha256": binding.helper_executable_sha256,
                "host_executable_sha256": binding.host_executable_sha256,
                "host_bundle_identifier": binding.host_bundle_identifier,
                "host_info_plist_sha256": binding.host_info_plist_sha256,
                "recipe_sha256": binding.recipe_sha256,
                "window_title": binding.window_title,
                "control_identifier": binding.control_identifier,
                "inserted_text_sha256": binding.inserted_text_sha256,
                "inserted_text_byte_length": binding.inserted_text_byte_length,
                "expected_post_insert_sha256": binding.expected_post_insert_sha256,
                "expected_post_insert_byte_length": binding.expected_post_insert_byte_length,
                "expected_settled_sha256": binding.expected_settled_sha256,
                "expected_settled_byte_length": binding.expected_settled_byte_length,
            })),
            "learning_state": "not_opened_no_learning_arm",
            "learning": "not_run",
            "sql": "not_run",
            "settings": "not_run",
            "expires_after_ms": phase.ttl_ms,
            "product_target_terminal": "not_probed",
        });
        prepared.active_phase = Some(ActiveOwnedPasteSessionPhaseV1 {
            phase_index,
            phase_id: phase_id.to_owned(),
            expires,
            operation_id: operation_id.clone(),
            attempted: false,
            completed: false,
        });
        Ok(result)
    }

    fn apply_phase_text(
        &mut self,
        plan_id: &str,
        phase_id: &str,
        operation_id: &str,
        states: &[LearningTextStateInputV2],
    ) -> Result<serde_json::Value, String> {
        if self.discard_expired_phase_plan() {
            return Err("learning_phase_expired_plan_reprepare_required".into());
        }
        let prepared = self
            .prepared_plan
            .as_mut()
            .ok_or_else(|| "learning_phase_plan_not_prepared".to_owned())?;
        if prepared.plan.plan_id != plan_id {
            return Err("learning_phase_plan_mismatch".into());
        }
        let active = prepared
            .active_phase
            .as_ref()
            .ok_or_else(|| "learning_phase_not_active".to_owned())?;
        if active.phase_id != phase_id {
            return Err("learning_phase_mismatch".into());
        }
        let phase = &prepared.plan.phases[active.phase_index];
        let receipt = prepared
            .arm
            .execute_phase_text(&prepared.plan, phase, operation_id, states)
            .map_err(sanitize_learning_operation_error)?;
        prepared
            .active_phase
            .as_mut()
            .ok_or_else(|| "learning_phase_not_active".to_owned())?
            .completed_operation_ids
            .insert(operation_id.to_owned());
        Ok(receipt)
    }

    fn apply_phase_followup(
        &mut self,
        plan_id: &str,
        phase_id: &str,
        operation_id: &str,
        text: &str,
    ) -> Result<serde_json::Value, String> {
        if self.discard_expired_phase_plan() {
            return Err("learning_phase_expired_plan_reprepare_required".into());
        }
        let prepared = self
            .prepared_plan
            .as_mut()
            .ok_or_else(|| "learning_phase_plan_not_prepared".to_owned())?;
        if prepared.plan.plan_id != plan_id {
            return Err("learning_phase_plan_mismatch".into());
        }
        let active = prepared
            .active_phase
            .as_ref()
            .ok_or_else(|| "learning_phase_not_active".to_owned())?;
        if active.phase_id != phase_id {
            return Err("learning_phase_mismatch".into());
        }
        let phase = &prepared.plan.phases[active.phase_index];
        let receipt = prepared
            .arm
            .execute_phase_followup(&prepared.plan, phase, operation_id, text)
            .map_err(sanitize_learning_operation_error)?;
        prepared
            .active_phase
            .as_mut()
            .ok_or_else(|| "learning_phase_not_active".to_owned())?
            .completed_operation_ids
            .insert(operation_id.to_owned());
        Ok(receipt)
    }

    fn undo_learning_phase(
        &mut self,
        plan_id: &str,
        phase_id: &str,
        operation_id: &str,
    ) -> Result<serde_json::Value, String> {
        if self.discard_expired_phase_plan() {
            return Err("learning_phase_expired_plan_reprepare_required".into());
        }
        let prepared = self
            .prepared_plan
            .as_mut()
            .ok_or_else(|| "learning_phase_plan_not_prepared".to_owned())?;
        if prepared.plan.plan_id != plan_id {
            return Err("learning_phase_plan_mismatch".into());
        }
        let active = prepared
            .active_phase
            .as_ref()
            .ok_or_else(|| "learning_phase_not_active".to_owned())?;
        if active.phase_id != phase_id {
            return Err("learning_phase_mismatch".into());
        }
        let phase = &prepared.plan.phases[active.phase_index];
        let receipt = prepared
            .arm
            .execute_phase_undo(&prepared.plan, phase, operation_id)
            .map_err(sanitize_learning_operation_error)?;
        prepared
            .active_phase
            .as_mut()
            .ok_or_else(|| "learning_phase_not_active".to_owned())?
            .completed_operation_ids
            .insert(operation_id.to_owned());
        Ok(receipt)
    }

    #[cfg(target_os = "macos")]
    fn observe_target_phase(
        &mut self,
        plan_id: &str,
        phase_id: &str,
        operation_id: &str,
    ) -> Result<serde_json::Value, String> {
        use learning_phase_v3::LearningPhaseActionV3;
        use std::time::Instant;

        if self.discard_expired_phase_plan() {
            return Err("learning_phase_expired_plan_reprepare_required".into());
        }
        let (binding, prepared_oracle, expires) = {
            let prepared = self
                .prepared_target_plan
                .as_mut()
                .ok_or_else(|| "learning_phase_plan_not_prepared".to_owned())?;
            if prepared.plan.plan_id != plan_id {
                return Err("learning_phase_plan_mismatch".into());
            }
            let active = prepared
                .active_phase
                .as_mut()
                .ok_or_else(|| "learning_phase_not_active".to_owned())?;
            if active.phase_id != phase_id {
                return Err("learning_phase_mismatch".into());
            }
            if Instant::now() >= active.expires {
                return Err("learning_phase_expired_plan_reprepare_required".into());
            }
            if active.attempted_operation_ids.contains(operation_id)
                || active.completed_operation_ids.contains(operation_id)
            {
                return Err("native_oracle_operation_already_attempted".into());
            }
            let phase = &prepared.plan.phases[active.phase_index];
            let LearningPhaseActionV3::ObserveTarget {
                operation_id: declared_operation_id,
            } = &phase.action
            else {
                return Err("learning_phase_action_mismatch".into());
            };
            if declared_operation_id != operation_id
                || phase.target_oracle != LearningOracleV2::IndependentTarget
                || !phase.target_observation_authorized
                || phase.local_only_grant
                || phase.source_text_learning_authorized
                || phase.learning_authorized
                || phase.local_replacement_authorized
                || phase.undo_authorized
                || phase.source.is_some()
                || !phase.edit_states.is_empty()
            {
                return Err("native_oracle_phase_grant_or_operation_mismatch".into());
            }
            let binding = phase
                .target_binding
                .clone()
                .ok_or_else(|| "learning_phase_v3_observe_target_binding_required".to_owned())?;
            let prepared_oracle = prepared.native_oracle.clone();
            prepared_oracle.validate_binding(&binding)?;
            (binding, prepared_oracle, active.expires)
        };

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "native_oracle_runtime_unavailable".to_owned())?;
        {
            let active = self
                .prepared_target_plan
                .as_mut()
                .and_then(|prepared| prepared.active_phase.as_mut())
                .ok_or_else(|| "learning_phase_not_active".to_owned())?;
            if active.phase_id != phase_id || Instant::now() >= active.expires {
                return Err("learning_phase_expired_plan_reprepare_required".into());
            }
            if active.attempted_operation_ids.contains(operation_id)
                || active.completed_operation_ids.contains(operation_id)
            {
                return Err("native_oracle_operation_already_attempted".into());
            }
            active
                .attempted_operation_ids
                .insert(operation_id.to_owned());
        }
        let cancellation = tokio_util::sync::CancellationToken::new();
        let outcome = runtime.block_on(learning_native_oracle::run_native_oracle_phase(
            &prepared_oracle,
            plan_id,
            phase_id,
            operation_id,
            &binding,
            expires,
            &cancellation,
        ))?;
        let comparison =
            learning_native_oracle::compare_registered_expected_value(&binding, &outcome);
        let comparison_name = match comparison {
            learning_native_oracle::NativeTargetExpectedComparisonV1::Matched => "matched",
            learning_native_oracle::NativeTargetExpectedComparisonV1::Mismatched => "mismatched",
            learning_native_oracle::NativeTargetExpectedComparisonV1::Unknown => "unknown",
        };
        let phase_lease_state = if Instant::now() < expires {
            if let Some(active) = self
                .prepared_target_plan
                .as_mut()
                .and_then(|prepared| prepared.active_phase.as_mut())
            {
                if active.phase_id == phase_id {
                    active
                        .completed_operation_ids
                        .insert(operation_id.to_owned());
                }
            }
            "active"
        } else {
            "expired_after_observation"
        };
        Ok(serde_json::json!({
            "admission": "operation_receipt",
            "operation_status": if phase_lease_state == "active" { "owner_completed" } else { "owner_returned_after_lease_expiry" },
            "plan_id": plan_id,
            "phase_id": phase_id,
            "operation_id": operation_id,
            "phase_lease": phase_lease_state,
            "target_oracle": "registered_owned_host_component_sample_only",
            "learning_state": "not_opened_no_learning_arm",
            "learning": "not_run",
            "insertion": "not_run",
            "pipeline_final": "not_run",
            "product_target_terminal": "not_probed",
            "target_terminal": outcome.target_terminal,
            "expected_comparison": comparison_name,
            "expected": {
                "sha256": binding.expected_value_sha256,
                "byte_length": binding.expected_value_byte_length,
            },
            "actual": {
                "state": outcome.actual_sample.state,
                "sha256": outcome.actual_sample.value_sha256,
                "byte_length": outcome.actual_sample.value_byte_length,
            },
            "outcome": native_target_oracle_summary(&outcome),
            "known_gaps": learning_phase_v3::LEARNING_PHASE_V3_KNOWN_GAPS,
        }))
    }

    #[cfg(target_os = "macos")]
    fn run_owned_paste_session_phase(
        &mut self,
        plan_id: &str,
        phase_id: &str,
        operation_id: &str,
    ) -> Result<serde_json::Value, String> {
        use learning_phase_v3::LearningPhaseActionV3;
        use std::time::Instant;

        if !self.owned_paste_session_supervisor {
            return Err("owned_paste_session_supervisor_required".into());
        }
        if self.pending_owned_paste_general_owner.is_some() {
            return Err("owned_paste_session_general_owner_pending".into());
        }

        if self.discard_expired_phase_plan() {
            return Err("learning_phase_expired_plan_reprepare_required".into());
        }
        let (plan, prepared, binding, expires) = {
            let plan_state = self
                .prepared_owned_paste_session_plan
                .as_mut()
                .ok_or_else(|| "learning_phase_plan_not_prepared".to_owned())?;
            if plan_state.plan.plan_id != plan_id {
                return Err("learning_phase_plan_mismatch".into());
            }
            let active = plan_state
                .active_phase
                .as_mut()
                .ok_or_else(|| "learning_phase_not_active".to_owned())?;
            if active.phase_id != phase_id {
                return Err("learning_phase_mismatch".into());
            }
            if Instant::now() >= active.expires {
                return Err("learning_phase_expired_plan_reprepare_required".into());
            }
            if active.operation_id != operation_id || active.attempted || active.completed {
                return Err("owned_paste_session_operation_already_attempted_or_mismatched".into());
            }
            let phase = &plan_state.plan.phases[active.phase_index];
            let LearningPhaseActionV3::RunOwnedPasteSession {
                operation_id: declared_operation_id,
            } = &phase.action
            else {
                return Err("learning_phase_action_mismatch".into());
            };
            if declared_operation_id != operation_id
                || !phase.owned_paste_session_authorized
                || phase.target_oracle != LearningOracleV2::IndependentTarget
                || phase.source_text_learning_authorized
                || phase.learning_authorized
                || phase.local_replacement_authorized
                || phase.undo_authorized
                || phase.local_only_grant
                || phase.source.is_some()
                || !phase.edit_states.is_empty()
            {
                return Err("owned_paste_session_phase_grant_or_operation_mismatch".into());
            }
            let binding = phase.owned_paste_session_binding.clone().ok_or_else(|| {
                "learning_phase_v3_owned_paste_session_binding_required".to_owned()
            })?;
            plan_state.prepared.validate_binding(&binding)?;
            active.attempted = true;
            (
                plan_state.plan.clone(),
                plan_state.prepared.clone(),
                binding,
                active.expires,
            )
        };

        let operation_receipt = execute_owned_paste_session_operation(
            &prepared,
            &plan.plan_id,
            phase_id,
            operation_id,
            &binding,
            expires,
            &mut self.pending_owned_paste_general_owner,
        );
        if operation_receipt.is_ok() {
            if let Some(active) = self
                .prepared_owned_paste_session_plan
                .as_mut()
                .and_then(|prepared| prepared.active_phase.as_mut())
            {
                if active.phase_id == phase_id && active.operation_id == operation_id {
                    active.completed = true;
                }
            }
        }
        operation_receipt
    }

    #[cfg(not(target_os = "macos"))]
    fn run_owned_paste_session_phase(
        &mut self,
        _plan_id: &str,
        _phase_id: &str,
        _operation_id: &str,
    ) -> Result<serde_json::Value, String> {
        Err("owned_paste_session_requires_macos".into())
    }

    #[cfg(not(target_os = "macos"))]
    fn observe_target_phase(
        &mut self,
        _plan_id: &str,
        _phase_id: &str,
        _operation_id: &str,
    ) -> Result<serde_json::Value, String> {
        Err("native_oracle_requires_macos".into())
    }

    #[cfg(target_os = "macos")]
    fn end_owned_target_phase(
        &mut self,
        plan_id: &str,
        phase_id: &str,
    ) -> Result<serde_json::Value, String> {
        if self.discard_expired_phase_plan() {
            return Err("learning_phase_expired_plan_reprepare_required".into());
        }
        let prepared = self
            .prepared_target_plan
            .as_mut()
            .ok_or_else(|| "learning_phase_plan_not_prepared".to_owned())?;
        if prepared.plan.plan_id != plan_id {
            return Err("learning_phase_plan_mismatch".into());
        }
        let active = prepared
            .active_phase
            .as_ref()
            .ok_or_else(|| "learning_phase_not_active".to_owned())?;
        if active.phase_id != phase_id {
            return Err("learning_phase_mismatch".into());
        }
        let phase = &prepared.plan.phases[active.phase_index];
        let required = phase_action_operation_ids(&phase.action);
        if required
            .iter()
            .any(|operation_id| !active.completed_operation_ids.contains(*operation_id))
        {
            return Err("learning_phase_operations_incomplete".into());
        }
        let completed = active.completed_operation_ids.len();
        prepared.active_phase = None;
        prepared.next_phase_index += 1;
        Ok(serde_json::json!({
            "ended": true,
            "plan_id": plan_id,
            "phase_id": phase_id,
            "operations_completed": completed,
            "next_phase_index": prepared.next_phase_index,
            "learning_state": "not_opened_no_learning_arm",
        }))
    }

    #[cfg(target_os = "macos")]
    fn end_owned_target_plan(&mut self, plan_id: &str) -> Result<serde_json::Value, String> {
        let prepared = self
            .prepared_target_plan
            .as_ref()
            .ok_or_else(|| "learning_phase_plan_not_prepared".to_owned())?;
        if prepared.plan.plan_id != plan_id {
            return Err("learning_phase_plan_mismatch".into());
        }
        if prepared.active_phase.is_some() {
            return Err("end_active_learning_phase_first".into());
        }
        let phase_count = prepared.plan.phases.len();
        let phases_completed = prepared.next_phase_index;
        self.prepared_target_plan = None;
        Ok(serde_json::json!({
            "ended": true,
            "plan_id": plan_id,
            "phases_completed": phases_completed,
            "phase_count": phase_count,
            "complete": phases_completed == phase_count,
            "selected_state": {"state": "not_applicable_no_learning_arm"},
            "learning_state": "not_opened_no_learning_arm",
            "target_oracle": "registered_owned_host_component_sample_only",
            "product_target_terminal": "not_probed",
        }))
    }

    #[cfg(target_os = "macos")]
    fn end_owned_paste_session_phase(
        &mut self,
        plan_id: &str,
        phase_id: &str,
    ) -> Result<serde_json::Value, String> {
        if self.discard_expired_phase_plan() {
            return Err("learning_phase_expired_plan_reprepare_required".into());
        }
        let prepared = self
            .prepared_owned_paste_session_plan
            .as_mut()
            .ok_or_else(|| "learning_phase_plan_not_prepared".to_owned())?;
        if prepared.plan.plan_id != plan_id {
            return Err("learning_phase_plan_mismatch".into());
        }
        let active = prepared
            .active_phase
            .as_ref()
            .ok_or_else(|| "learning_phase_not_active".to_owned())?;
        if active.phase_id != phase_id {
            return Err("learning_phase_mismatch".into());
        }
        if !active.completed {
            return Err("learning_phase_operations_incomplete".into());
        }
        let completed = if active.completed { 1 } else { 0 };
        prepared.active_phase = None;
        prepared.next_phase_index += 1;
        Ok(serde_json::json!({
            "ended": true,
            "plan_id": plan_id,
            "phase_id": phase_id,
            "operations_completed": completed,
            "next_phase_index": prepared.next_phase_index,
            "learning_state": "not_opened_no_learning_arm",
            "sql": "not_run",
            "settings": "not_run",
        }))
    }

    #[cfg(target_os = "macos")]
    fn end_owned_paste_session_plan(&mut self, plan_id: &str) -> Result<serde_json::Value, String> {
        let prepared = self
            .prepared_owned_paste_session_plan
            .as_ref()
            .ok_or_else(|| "learning_phase_plan_not_prepared".to_owned())?;
        if prepared.plan.plan_id != plan_id {
            return Err("learning_phase_plan_mismatch".into());
        }
        if prepared.active_phase.is_some() {
            return Err("end_active_learning_phase_first".into());
        }
        let phase_count = prepared.plan.phases.len();
        let phases_completed = prepared.next_phase_index;
        self.prepared_owned_paste_session_plan = None;
        Ok(serde_json::json!({
            "ended": true,
            "plan_id": plan_id,
            "phases_completed": phases_completed,
            "phase_count": phase_count,
            "complete": phases_completed == phase_count,
            "learning_state": "not_opened_no_learning_arm",
            "sql": "not_run",
            "settings": "not_run",
            "product_target_terminal": "not_probed",
        }))
    }

    fn end_learning_phase(
        &mut self,
        plan_id: &str,
        phase_id: &str,
    ) -> Result<serde_json::Value, String> {
        #[cfg(target_os = "macos")]
        if self.prepared_target_plan.is_some() {
            return self.end_owned_target_phase(plan_id, phase_id);
        }
        #[cfg(target_os = "macos")]
        if self.prepared_owned_paste_session_plan.is_some() {
            return self.end_owned_paste_session_phase(plan_id, phase_id);
        }
        if self.discard_expired_phase_plan() {
            return Err("learning_phase_expired_plan_reprepare_required".into());
        }
        let prepared = self
            .prepared_plan
            .as_mut()
            .ok_or_else(|| "learning_phase_plan_not_prepared".to_owned())?;
        if prepared.plan.plan_id != plan_id {
            return Err("learning_phase_plan_mismatch".into());
        }
        let active = prepared
            .active_phase
            .as_ref()
            .ok_or_else(|| "learning_phase_not_active".to_owned())?;
        if active.phase_id != phase_id {
            return Err("learning_phase_mismatch".into());
        }
        let phase = &prepared.plan.phases[active.phase_index];
        let required = phase_action_operation_ids(&phase.action);
        if required
            .iter()
            .any(|operation_id| !active.completed_operation_ids.contains(*operation_id))
        {
            return Err("learning_phase_operations_incomplete".into());
        }
        let completed = active.completed_operation_ids.len();
        prepared.active_phase = None;
        prepared.next_phase_index += 1;
        Ok(serde_json::json!({
            "ended": true,
            "plan_id": plan_id,
            "phase_id": phase_id,
            "operations_completed": completed,
            "next_phase_index": prepared.next_phase_index,
        }))
    }

    fn end_phase_plan(&mut self, plan_id: &str) -> Result<serde_json::Value, String> {
        #[cfg(target_os = "macos")]
        if self.prepared_target_plan.is_some() {
            return self.end_owned_target_plan(plan_id);
        }
        #[cfg(target_os = "macos")]
        if self.prepared_owned_paste_session_plan.is_some() {
            return self.end_owned_paste_session_plan(plan_id);
        }
        let prepared = self
            .prepared_plan
            .as_ref()
            .ok_or_else(|| "learning_phase_plan_not_prepared".to_owned())?;
        if prepared.plan.plan_id != plan_id {
            return Err("learning_phase_plan_mismatch".into());
        }
        if prepared.active_phase.is_some() {
            return Err("end_active_learning_phase_first".into());
        }
        let final_state = serde_json::json!(prepared
            .arm
            .selected_state(&prepared.plan)
            .map_err(|_| "selected_learning_phase_state_unavailable".to_owned())?);
        let phase_count = prepared.plan.phases.len();
        let phases_completed = prepared.next_phase_index;
        self.prepared_plan = None;
        Ok(serde_json::json!({
            "ended": true,
            "plan_id": plan_id,
            "phases_completed": phases_completed,
            "phase_count": phase_count,
            "complete": phases_completed == phase_count,
            "selected_state": final_state,
            "learning_state": "isolated_arm_opened",
        }))
    }
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
fn execute_owned_paste_session_operation(
    prepared: &learning_native_paste_session::PreparedOwnedPasteSessionV1,
    plan_id: &str,
    phase_id: &str,
    operation_id: &str,
    binding: &learning_phase_v3::OwnedPasteSessionBindingV1,
    expires_at: std::time::Instant,
    pending_owner: &mut Option<PendingOwnedPasteGeneralOwnerV1>,
) -> Result<serde_json::Value, String> {
    use std::time::Instant;
    use tokio_util::sync::CancellationToken;

    let cancellation = CancellationToken::new();
    let mut session = learning_native_paste_session::OwnedPasteSessionProcessV1::start(
        prepared,
        plan_id,
        phase_id,
        operation_id,
        binding,
        expires_at,
        &cancellation,
    )?;
    let ready = session.ready.clone();
    let host_setup_valid = ready.host_identity.source == "darwin_proc_pidinfo_bsdinfo_proc_pidpath"
        && ready.host_identity.pid > 0
        && ready.host_identity.parent_pid == ready.helper_pid
        && ready.host_identity.user_id > 0
        && std::path::Path::new(&ready.host_identity.executable_path).is_absolute();
    let host_live = host_setup_valid
        && Instant::now() < expires_at
        && !cancellation.is_cancelled()
        && session
            .registered_host_and_helper_are_live()
            .unwrap_or(false);
    if !host_live {
        let cleanup = session.close(operation_id);
        return Ok(serde_json::json!({
            "protocol": "voiceflow_registered_owned_paste_session_v1",
            "operation_status": "registered_host_preflight_rejected",
            "plan_id": plan_id,
            "phase_id": phase_id,
            "operation_id": operation_id,
            "host_setup": "rejected_before_clipboard_capture",
            "host_setup_receipt_sha256": ready.host_receipt_sha256,
            "host_cleanup": owned_paste_session_cleanup_receipt(&cleanup),
            "general_clipboard": "not_captured",
            "shortcut": "not_run",
            "learning": "not_run",
            "sql": "not_run",
            "settings": "not_opened",
            "general_guard_active_for": false,
        }));
    }

    let expires_at_monotonic_ns =
        crate::learning_clipboard_guard::monotonic_deadline_ns(expires_at)
            .ok_or_else(|| "owned_paste_session_deadline_invalid".to_owned())?;
    let authority = crate::learning_clipboard_guard::RegisteredOwnedPasteSessionAuthorityV1 {
        plan_id: plan_id.to_owned(),
        phase_id: phase_id.to_owned(),
        operation_id: operation_id.to_owned(),
        supervisor_pid: std::process::id() as i32,
        helper_pid: ready.helper_pid,
        host_identity: crate::learning_clipboard_guard::RegisteredOwnedPasteHostIdentityV1 {
            source: ready.host_identity.source.clone(),
            pid: ready.host_identity.pid,
            parent_pid: ready.host_identity.parent_pid,
            user_id: ready.host_identity.user_id,
            start_seconds: ready.host_identity.start_seconds.clone(),
            start_microseconds: ready.host_identity.start_microseconds.clone(),
            executable_path: ready.host_identity.executable_path.clone(),
        },
        binding: binding.clone(),
        expires_at_monotonic_ns,
    };

    let mut clipboard_owner = match crate::paste::ResearchClipboardGuardOwnerV1::capture_general() {
        Ok(owner) => owner,
        Err(reason) => {
            let cleanup = session.close(operation_id);
            return Ok(serde_json::json!({
                "protocol": "voiceflow_registered_owned_paste_session_v1",
                "operation_status": "general_snapshot_capture_failed",
                "plan_id": plan_id,
                "phase_id": phase_id,
                "operation_id": operation_id,
                "artifact_identity": prepared.artifact_identity_summary(),
                "host_setup": "ready_identity_and_helper_live",
                "host_setup_receipt_sha256": ready.host_receipt_sha256,
                "host_identity": registered_owned_paste_host_identity_summary(&ready.host_identity),
                "host_cleanup": owned_paste_session_cleanup_receipt(&cleanup),
                "general_clipboard_capture": reason,
                "child_started": false,
                "shortcut": "not_run",
                "learning": "not_run",
                "sql": "not_run",
                "settings": "not_opened",
                "general_guard_active_for": false,
            }));
        }
    };

    let parent_report = crate::learning_clipboard_guard::run_registered_owned_paste_session_parent(
        authority,
        &mut clipboard_owner,
        expires_at,
        &cancellation,
        |action| {
            match action {
                crate::learning_clipboard_guard::RegisteredOwnedPasteParentCallbackV1::RecheckPostAuthority => {
                    if Instant::now() < expires_at
                        && !cancellation.is_cancelled()
                        && session.registered_host_and_helper_are_live().unwrap_or(false)
                    {
                        crate::learning_clipboard_guard::RegisteredOwnedPasteParentCallbackResultV1::HostAndHelperLive
                    } else {
                        crate::learning_clipboard_guard::RegisteredOwnedPasteParentCallbackResultV1::HostOrHelperUnavailable
                    }
                }
                crate::learning_clipboard_guard::RegisteredOwnedPasteParentCallbackV1::ApplyDeclaredEdit => {
                    if Instant::now() >= expires_at
                        || cancellation.is_cancelled()
                        || !session.registered_host_and_helper_are_live().unwrap_or(false)
                    {
                        crate::learning_clipboard_guard::RegisteredOwnedPasteParentCallbackResultV1::HostOrHelperUnavailable
                    } else {
                        let result = session
                            .apply_declared_edit(operation_id)
                            .map(|sample| registered_owned_paste_edit_result(&sample, binding))
                            .unwrap_or_else(|_| unknown_owned_paste_edit_result());
                        crate::learning_clipboard_guard::RegisteredOwnedPasteParentCallbackResultV1::Edit(result)
                    }
                }
            }
        },
    );

    // The registered child has already been truly waited before helper close.
    // Only a successful close receipt carries both helper and Host exit facts.
    let cleanup = session.close(operation_id);
    let host_terminal_confirmed = cleanup.as_ref().is_ok_and(|cleanup| {
        cleanup.helper_exit_confirmed
            && cleanup.helper_exit_status == Some(0)
            && cleanup.host_exit_confirmed
            && cleanup.host_exit_status == Some(0)
    });
    let cleanup_receipt = owned_paste_session_cleanup_receipt(&cleanup);
    let mut parent_owner_result = None;
    let mut parent_restore_attempted = false;
    let owner_retained = if host_terminal_confirmed {
        let current = clipboard_owner.current_ownership();
        let result =
            if current == crate::learning_clipboard_guard::ClipboardGuardReplyV1::WriteOwned {
                parent_restore_attempted = true;
                clipboard_owner.restore_if_owned()
            } else {
                current
            };
        let resolved = matches!(
            result,
            crate::learning_clipboard_guard::ClipboardGuardReplyV1::Restored
                | crate::learning_clipboard_guard::ClipboardGuardReplyV1::NotWritten
                | crate::learning_clipboard_guard::ClipboardGuardReplyV1::OwnershipChanged
        );
        let result = if resolved {
            clipboard_owner.close_after_restore()
        } else {
            result
        };
        let owner_retained = !matches!(
            result,
            crate::learning_clipboard_guard::ClipboardGuardReplyV1::Restored
                | crate::learning_clipboard_guard::ClipboardGuardReplyV1::NotWritten
                | crate::learning_clipboard_guard::ClipboardGuardReplyV1::OwnershipChanged
        );
        parent_owner_result = Some(result);
        if owner_retained {
            *pending_owner = Some(PendingOwnedPasteGeneralOwnerV1 {
                owner: clipboard_owner,
                restore_allowed_after_host_terminal: true,
            });
        }
        owner_retained
    } else {
        *pending_owner = Some(PendingOwnedPasteGeneralOwnerV1 {
            owner: clipboard_owner,
            restore_allowed_after_host_terminal: false,
        });
        true
    };

    let child_result = parent_report.child_result;
    let complete = parent_report.state
        == crate::learning_clipboard_guard::RegisteredOwnedPasteParentStateV1::Complete
        && parent_report.child_exit_confirmed
        && host_terminal_confirmed
        && parent_report.child_result
            == crate::learning_clipboard_guard::RegisteredOwnedPasteChildResultKindV1::Completed
        && parent_owner_result
            == Some(crate::learning_clipboard_guard::ClipboardGuardReplyV1::Restored)
        && !owner_retained;
    Ok(serde_json::json!({
        "protocol": "voiceflow_registered_owned_paste_session_v1",
        "operation_status": if complete { "registered_same_host_paste_edit_readback_completed" } else { "registered_same_host_component_incomplete_or_unknown" },
        "plan_id": plan_id,
        "phase_id": phase_id,
        "operation_id": operation_id,
        "artifact_identity": prepared.artifact_identity_summary(),
        "registered_target": {
            "fixture_id": binding.fixture_id,
            "bundle_identifier": binding.host_bundle_identifier,
            "window_title": binding.window_title,
            "control_identifier": binding.control_identifier,
        },
        "host_setup": {
            "state": "helper_ready_and_host_identity_verified",
            "host_receipt_sha256": ready.host_receipt_sha256,
            "host_identity": registered_owned_paste_host_identity_summary(&ready.host_identity),
            "declared_settled_value": "recipe_declaration_not_observation",
        },
        "paste_and_remote_owner": parent_report,
        "child_result": child_result,
        "child_exit_confirmed": parent_report.child_exit_confirmed,
        "host_cleanup": cleanup_receipt,
        "general_clipboard": {
            "owner": "parent_only_original_snapshot_and_ownership_proof",
            "parent_restore": parent_owner_result,
            "host_terminal_confirmed_before_clipboard_resolution": host_terminal_confirmed,
            "parent_restore_attempted_after_terminal": parent_restore_attempted,
            "original_contents_readback": "not_probed",
            "owner_retained": owner_retained,
        },
        "general_guard_active_for": false,
        "cmd_v": if !parent_report.child_close_received {
            "unknown_after_child_disconnect"
        } else if parent_report.child_shortcut_sent {
            "posted_to_registered_host"
        } else {
            "not_posted"
        },
        "shortcut_sent": if parent_report.child_close_received {
            serde_json::Value::Bool(parent_report.child_shortcut_sent)
        } else {
            serde_json::Value::Null
        },
        "learning_state": "not_opened_no_learning_arm",
        "learning": "not_run",
        "sql": "not_run",
        "settings": "not_opened",
        "product_pipeline": "not_run",
        "asr_quality": "not_measured",
    }))
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
fn registered_owned_paste_host_identity_summary(
    identity: &learning_native_oracle::KernelProcessIdentityV1,
) -> serde_json::Value {
    serde_json::json!({
        "source": identity.source,
        "pid": identity.pid,
        "parent_pid": identity.parent_pid,
        "user_id": identity.user_id,
        "start_seconds": identity.start_seconds,
        "start_microseconds": identity.start_microseconds,
        "executable_path": identity.executable_path,
    })
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
pub(crate) fn validate_registered_owned_paste_binding_fields(
    binding: &learning_phase_v3::OwnedPasteSessionBindingV1,
) -> Result<(), String> {
    learning_native_paste_session::validate_registered_binding_fields(binding)
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
pub(crate) fn execute_registered_owned_paste_session_child(
    input: crate::learning_clipboard_guard::RegisteredOwnedPasteSessionChildInputV1,
) -> crate::learning_clipboard_guard::RegisteredOwnedPasteChildResultKindV1 {
    use sha2::{Digest, Sha256};
    use std::sync::atomic::AtomicU64;
    use std::time::{Duration, Instant};
    use tokio_util::sync::CancellationToken;

    use crate::learning_clipboard_guard::{
        ClipboardGuardOperationV1, ClipboardGuardRpcOutcomeV1, RegisteredOwnedPasteBaselineStateV1,
        RegisteredOwnedPasteBaselineV1, RegisteredOwnedPasteChildResultKindV1,
        RegisteredOwnedPasteComparisonV1, RegisteredOwnedPasteConfirmationV1,
        RegisteredOwnedPasteEditResultV1, RegisteredOwnedPasteEditSourceV1,
        RegisteredOwnedPasteEditStateV1, RegisteredOwnedPasteFinalSampleStateV1,
        RegisteredOwnedPasteFinalSampleV1, RegisteredOwnedPasteVerificationV1,
        RemoteProbeChildRestoreV1,
    };

    const INSERTED_TEXT: &str = "VoiceFlo";
    const OBSERVER_EPOCH: u64 = 1;
    const MAX_FIELD_UTF16: usize = 65_536;

    let authority = &input.authority;
    let remote = input.remote.clone();
    let cancellation: CancellationToken = input.cancellation.clone();
    let mut owner_completion;
    let mut observation_ticket;
    let paste_attempt;
    let mut paste_shortcut_sent = false;
    let mut paste_verified = false;
    let mut confirmation_dto = None;
    let mut baseline = RegisteredOwnedPasteBaselineV1 {
        state: RegisteredOwnedPasteBaselineStateV1::Unknown,
        has_value: false,
        sha256: [0; 32],
        byte_length: 0,
        utf16_length: 0,
        selection_start: 0,
        selection_length: 0,
    };
    let mut final_sample = RegisteredOwnedPasteFinalSampleV1 {
        state: RegisteredOwnedPasteFinalSampleStateV1::NotRun,
        has_value: false,
        sha256: [0; 32],
        byte_length: 0,
        utf16_length: 0,
        focus_matches: false,
    };
    let mut edit_result = RegisteredOwnedPasteEditResultV1 {
        state: RegisteredOwnedPasteEditStateV1::Unknown,
        comparison: RegisteredOwnedPasteComparisonV1::Unknown,
        source: RegisteredOwnedPasteEditSourceV1::Unknown,
        has_value: false,
        sha256: [0; 32],
        byte_length: 0,
        has_edit_receipt: false,
        programmatic_edit_receipt_sha256: [0; 32],
    };
    let mut edit_response_received = false;
    let mut observation_ticket_released = false;

    let invalid_context = input.remote.plan_id != authority.plan_id
        || input.remote.phase_id != authority.phase_id
        || input.remote.operation_id != authority.operation_id
        || Instant::now() >= input.expires_at
        || cancellation.is_cancelled()
        || validate_registered_owned_paste_binding_fields(&authority.binding).is_err()
        || input.host_identity.pid != authority.host_identity.pid
        || input.host_identity.parent_pid != authority.host_identity.parent_pid
        || input.host_identity.user_id != authority.host_identity.user_id
        || input.host_identity.executable_path.to_string_lossy()
            != authority.host_identity.executable_path;
    if invalid_context {
        return RegisteredOwnedPasteChildResultKindV1::ProtocolRejected;
    }

    let handoff = crate::paste::insert_registered_research_owned_paste_remote(
        INSERTED_TEXT,
        input.host_identity.clone(),
        remote.clone(),
        input.expires_at,
        cancellation.clone(),
    );
    paste_attempt = handoff.attempt;
    owner_completion = handoff.owner_completion;
    observation_ticket = handoff.observation_ticket;

    let confirmed = paste_attempt
        .as_ref()
        .ok()
        .and_then(|timed| timed.outcome.confirmed_insertion.as_ref());
    if let Ok(timed) = &paste_attempt {
        paste_shortcut_sent = timed.outcome.shortcut_sent;
        let handoff_tickets_are_unique = timed.outcome.learning_observation_ticket.is_none();
        let confirmed_is_bound = confirmed
            .zip(observation_ticket.as_ref())
            .is_some_and(|(confirmed, ticket)| confirmed.is_bound_to(ticket));
        let guard_matches = timed
            .outcome
            .post_insert_target_guard
            .as_ref()
            .is_some_and(|guard| {
                guard.pid == authority.host_identity.pid
                    && guard.bundle_id.as_deref()
                        == Some(authority.binding.host_bundle_identifier.as_str())
            });
        let exact_confirmation = confirmed.is_some_and(|confirmed| {
            confirmed.readback == crate::delivery_diagnostics::PasteVerificationFact::Verified
                && confirmed.source_field_utf16_length == 6
                && confirmed.inserted_field_utf16_length == 14
                && confirmed.replaced_range_before.start == 6
                && confirmed.replaced_range_before.length == 0
                && confirmed.inserted_range_after.start == 6
                && confirmed.inserted_range_after.length == 8
                && confirmed.inserted_text_utf16_length == 8
                && confirmed.target_identity.pid == authority.host_identity.pid
                && confirmed.target_identity.bundle_id.as_deref()
                    == Some(authority.binding.host_bundle_identifier.as_str())
        });
        paste_verified = timed.outcome.verified
            && timed.outcome.verification
                == Some(crate::delivery_diagnostics::PasteVerificationFact::Verified)
            && handoff_tickets_are_unique
            && confirmed_is_bound
            && guard_matches
            && exact_confirmation;
        confirmation_dto = confirmed.map(|confirmed| RegisteredOwnedPasteConfirmationV1 {
            verification: if confirmed.readback
                == crate::delivery_diagnostics::PasteVerificationFact::Verified
            {
                RegisteredOwnedPasteVerificationV1::Verified
            } else {
                RegisteredOwnedPasteVerificationV1::Unverified
            },
            target_pid: confirmed.target_identity.pid,
            target_bundle_matches: confirmed.target_identity.bundle_id.as_deref()
                == Some(authority.binding.host_bundle_identifier.as_str()),
            source_field_utf16_length: confirmed.source_field_utf16_length,
            inserted_field_utf16_length: confirmed.inserted_field_utf16_length,
            replaced_range_start: confirmed.replaced_range_before.start,
            replaced_range_length: confirmed.replaced_range_before.length,
            inserted_range_start: confirmed.inserted_range_after.start,
            inserted_range_length: confirmed.inserted_range_after.length,
            inserted_text_utf16_length: confirmed.inserted_text_utf16_length,
        });
    }

    let observer_epoch = std::sync::Arc::new(AtomicU64::new(OBSERVER_EPOCH));
    let mut baseline_matches = false;
    if paste_verified {
        if let Some(ticket) = observation_ticket.as_ref() {
            if Instant::now() < input.expires_at && !cancellation.is_cancelled() {
                if let Ok(sample) = ticket.read_sample(
                    &cancellation,
                    MAX_FIELD_UTF16,
                    std::sync::Arc::clone(&observer_epoch),
                    OBSERVER_EPOCH,
                ) {
                    let selection = sample.selection_range_utf16;
                    if let Some(value) = sample.value.as_deref() {
                        if sample.focus_matches == Some(true) {
                            let digest: [u8; 32] = Sha256::digest(value.as_bytes()).into();
                            let selection_start = selection
                                .and_then(|(start, _)| usize::try_from(start).ok())
                                .unwrap_or(0);
                            let selection_length = selection
                                .and_then(|(_, length)| usize::try_from(length).ok())
                                .unwrap_or(0);
                            let actual_utf16_length = value.encode_utf16().count();
                            baseline_matches = hex::encode(digest)
                                == authority.binding.expected_post_insert_sha256
                                && value.len()
                                    == authority.binding.expected_post_insert_byte_length
                                && actual_utf16_length == 14
                                && selection == Some((14, 0));
                            baseline = RegisteredOwnedPasteBaselineV1 {
                                state: if baseline_matches {
                                    RegisteredOwnedPasteBaselineStateV1::ObservedExpectedPostInsert
                                } else {
                                    RegisteredOwnedPasteBaselineStateV1::Mismatch
                                },
                                has_value: true,
                                sha256: digest,
                                byte_length: value.len(),
                                utf16_length: actual_utf16_length,
                                selection_start,
                                selection_length,
                            };
                        }
                    }
                }
            }
        }
    }

    let ticket_release_deadline =
        std::cmp::min(input.expires_at, Instant::now() + Duration::from_secs(5));

    let write_id = remote.client.registered_write_id();
    if paste_verified
        && baseline_matches
        && confirmation_dto.is_some()
        && write_id.is_some()
        && Instant::now() < input.expires_at
        && !cancellation.is_cancelled()
    {
        if let (Some(write_id), Some(confirmed_insertion)) = (write_id, confirmation_dto) {
            let response = remote.client.request(
                remote.lease_id,
                ClipboardGuardOperationV1::ApplyDeclaredEdit {
                    write_id,
                    confirmed_insertion,
                    baseline,
                },
            );
            if let Ok(ClipboardGuardRpcOutcomeV1::RegisteredOwnedEditObserved { result }) = response
            {
                edit_response_received = true;
                edit_result = result;
            }
        }
    }

    if edit_response_received {
        if let Some(ticket) = observation_ticket.as_ref() {
            if Instant::now() < input.expires_at && !cancellation.is_cancelled() {
                if let Ok(sample) = ticket.read_sample(
                    &cancellation,
                    MAX_FIELD_UTF16,
                    std::sync::Arc::clone(&observer_epoch),
                    OBSERVER_EPOCH,
                ) {
                    if let Some(value) = sample.value.as_deref() {
                        if sample.focus_matches == Some(true) {
                            let digest: [u8; 32] = Sha256::digest(value.as_bytes()).into();
                            let utf16_length = value.encode_utf16().count();
                            let helper_matches = edit_result.has_value
                                && edit_result.source
                                    == RegisteredOwnedPasteEditSourceV1::AxTargetReaderOwnedDeclaredTargetV1
                                && edit_result.sha256 == digest
                                && edit_result.byte_length == value.len();
                            let expected_matches = hex::encode(digest)
                                == authority.binding.expected_settled_sha256
                                && value.len() == authority.binding.expected_settled_byte_length
                                && utf16_length == 15;
                            final_sample = RegisteredOwnedPasteFinalSampleV1 {
                                state: if helper_matches && expected_matches {
                                    RegisteredOwnedPasteFinalSampleStateV1::MatchesIndependentAxAndExpected
                                } else if helper_matches {
                                    RegisteredOwnedPasteFinalSampleStateV1::MatchesIndependentAxExpectedMismatch
                                } else {
                                    RegisteredOwnedPasteFinalSampleStateV1::ObservedDifferentOrUnbound
                                },
                                has_value: true,
                                sha256: digest,
                                byte_length: value.len(),
                                utf16_length,
                                focus_matches: true,
                            };
                        }
                    }
                }
            }
        }
    }

    if let Some(ticket) = observation_ticket.take() {
        observation_ticket_released = ticket
            .release_and_wait_until(ticket_release_deadline)
            .is_ok();
    }
    let owner_terminal_deadline = input.expires_at + Duration::from_secs(5);
    let (_, terminal_state) =
        wait_anchor_owner_terminal(&mut owner_completion, owner_terminal_deadline);
    let owner_joined = terminal_state.joined();

    let mut result_kind = if !paste_verified {
        RegisteredOwnedPasteChildResultKindV1::PasteRejected
    } else if !baseline_matches {
        RegisteredOwnedPasteChildResultKindV1::BaselineMismatch
    } else if !edit_response_received
        || edit_result.state != RegisteredOwnedPasteEditStateV1::Observed
        || edit_result.comparison != RegisteredOwnedPasteComparisonV1::Match
    {
        RegisteredOwnedPasteChildResultKindV1::EditUnknown
    } else if final_sample.state
        != RegisteredOwnedPasteFinalSampleStateV1::MatchesIndependentAxAndExpected
    {
        RegisteredOwnedPasteChildResultKindV1::FinalReadUnknown
    } else if !observation_ticket_released || !owner_joined || !terminal_state.closed_cleanly() {
        RegisteredOwnedPasteChildResultKindV1::OwnerUnknown
    } else {
        RegisteredOwnedPasteChildResultKindV1::Completed
    };

    let child_restore = remote.client.child_restore_state();
    if let Some(write_id) =
        write_id.filter(|_| child_restore == RemoteProbeChildRestoreV1::Deferred)
    {
        let close = remote.client.request(
            remote.lease_id,
            ClipboardGuardOperationV1::CloseRegisteredOwnedPasteSession {
                write_id: Some(write_id),
                shortcut_sent: paste_shortcut_sent,
                result: result_kind,
                baseline,
                final_sample,
                edit_response_received,
                observation_ticket_released,
                owner_joined,
                child_restore,
            },
        );
        if close != Ok(ClipboardGuardRpcOutcomeV1::RegisteredOwnedPasteSessionCloseAcknowledged) {
            result_kind = match close {
                Err(crate::learning_clipboard_guard::ClipboardGuardClientErrorV1::AcknowledgementUnavailable) => {
                    RegisteredOwnedPasteChildResultKindV1::OwnerUnknown
                }
                _ => RegisteredOwnedPasteChildResultKindV1::ProtocolRejected,
            };
        }
    } else if result_kind == RegisteredOwnedPasteChildResultKindV1::Completed {
        result_kind = RegisteredOwnedPasteChildResultKindV1::OwnerUnknown;
    }

    result_kind
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
fn unknown_owned_paste_edit_result(
) -> crate::learning_clipboard_guard::RegisteredOwnedPasteEditResultV1 {
    crate::learning_clipboard_guard::RegisteredOwnedPasteEditResultV1 {
        state: crate::learning_clipboard_guard::RegisteredOwnedPasteEditStateV1::Unknown,
        comparison: crate::learning_clipboard_guard::RegisteredOwnedPasteComparisonV1::Unknown,
        source: crate::learning_clipboard_guard::RegisteredOwnedPasteEditSourceV1::Unknown,
        has_value: false,
        sha256: [0; 32],
        byte_length: 0,
        has_edit_receipt: false,
        programmatic_edit_receipt_sha256: [0; 32],
    }
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
fn registered_owned_paste_edit_result(
    sample: &learning_native_paste_session::OwnedPasteSessionFinalV1,
    binding: &learning_phase_v3::OwnedPasteSessionBindingV1,
) -> crate::learning_clipboard_guard::RegisteredOwnedPasteEditResultV1 {
    use crate::learning_clipboard_guard::{
        RegisteredOwnedPasteComparisonV1, RegisteredOwnedPasteEditResultV1,
        RegisteredOwnedPasteEditSourceV1, RegisteredOwnedPasteEditStateV1,
    };

    let digest = sample
        .actual_sha256
        .as_deref()
        .and_then(|value| hex::decode(value).ok())
        .and_then(|value| <[u8; 32]>::try_from(value.as_slice()).ok());
    let receipt = hex::decode(sample.programmatic_edit_receipt_sha256.as_str())
        .ok()
        .and_then(|value| <[u8; 32]>::try_from(value.as_slice()).ok());
    RegisteredOwnedPasteEditResultV1 {
        state: if sample.actual_state == "observed" && digest.is_some() {
            RegisteredOwnedPasteEditStateV1::Observed
        } else if sample.actual_state == "changed" {
            RegisteredOwnedPasteEditStateV1::Changed
        } else {
            RegisteredOwnedPasteEditStateV1::Unknown
        },
        comparison: match sample.expected_comparison.as_str() {
            "match" => RegisteredOwnedPasteComparisonV1::Match,
            "mismatch" => RegisteredOwnedPasteComparisonV1::Mismatch,
            _ => RegisteredOwnedPasteComparisonV1::Unknown,
        },
        source: if sample.source == "AXTargetReader_owned_declared_target_v1" {
            RegisteredOwnedPasteEditSourceV1::AxTargetReaderOwnedDeclaredTargetV1
        } else {
            RegisteredOwnedPasteEditSourceV1::Unknown
        },
        has_value: digest.is_some() && sample.actual_byte_length.is_some(),
        sha256: digest.unwrap_or([0; 32]),
        byte_length: sample.actual_byte_length.unwrap_or(0),
        has_edit_receipt: receipt.is_some()
            && sample.expected_comparison == "match"
            && sample.actual_sha256.as_deref() == Some(binding.expected_settled_sha256.as_str())
            && sample.actual_byte_length == Some(binding.expected_settled_byte_length),
        programmatic_edit_receipt_sha256: receipt.unwrap_or([0; 32]),
    }
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
fn owned_paste_session_cleanup_receipt(
    cleanup: &Result<learning_native_paste_session::OwnedPasteSessionCleanupV1, String>,
) -> serde_json::Value {
    match cleanup {
        Ok(cleanup) => serde_json::json!({
            "state": "confirmed",
            "helper_exit_confirmed": cleanup.helper_exit_confirmed,
            "helper_exit_status": cleanup.helper_exit_status,
            "host_exit_confirmed": cleanup.host_exit_confirmed,
            "host_exit_status": cleanup.host_exit_status,
            "host_pid": cleanup.host_pid,
            "reason": cleanup.reason,
        }),
        Err(error) => serde_json::json!({
            "state": "unknown",
            "reason": error,
        }),
    }
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
fn anchor_owner_close_error_code(error: crate::paste::AnchorOwnerCloseError) -> &'static str {
    use crate::paste::AnchorOwnerCloseError;

    match error {
        AnchorOwnerCloseError::Busy => "busy",
        AnchorOwnerCloseError::Expired => "expired",
        AnchorOwnerCloseError::TimedOut => "timed_out",
        AnchorOwnerCloseError::UnknownPending => "unknown_pending",
        AnchorOwnerCloseError::NotStarted => "not_started",
        AnchorOwnerCloseError::OwnerClosed => "owner_closed",
        AnchorOwnerCloseError::OwnerFailed => "owner_failed",
    }
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
fn wait_anchor_owner_terminal(
    owner_completion: &mut crate::paste::AnchorOwnerCompletion,
    deadline: std::time::Instant,
) -> (String, RegisteredOwnedPasteOwnerTerminalV1) {
    use crate::paste::{AnchorOwnerCloseError, AnchorOwnerTerminal};

    let bounded_wait = match owner_completion.wait_closed_until(deadline) {
        Ok(()) => "terminal_signal_before_deadline".to_owned(),
        Err(AnchorOwnerCloseError::NotStarted) => {
            return (
                "no_owner_started".into(),
                RegisteredOwnedPasteOwnerTerminalV1::NotStarted,
            );
        }
        Err(error) => format!("unknown_{}", anchor_owner_close_error_code(error)),
    };

    // The bounded ticket/terminal observation cannot close the Host. Join the
    // actual paste owner after the action lease, keeping its Host alive while
    // any synchronous AX call or owner teardown remains in flight.
    loop {
        match owner_completion.settle_after_phase() {
            Ok(AnchorOwnerTerminal::Closed) => {
                return (bounded_wait, RegisteredOwnedPasteOwnerTerminalV1::Closed);
            }
            Ok(AnchorOwnerTerminal::Failed) => {
                return (bounded_wait, RegisteredOwnedPasteOwnerTerminalV1::Failed);
            }
            Err(AnchorOwnerCloseError::NotStarted) => {
                return (
                    bounded_wait,
                    RegisteredOwnedPasteOwnerTerminalV1::NotStarted,
                );
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    }
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RegisteredOwnedPasteOwnerTerminalV1 {
    NotStarted,
    Closed,
    Failed,
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
impl RegisteredOwnedPasteOwnerTerminalV1 {
    fn joined(self) -> bool {
        matches!(self, Self::Closed | Self::Failed)
    }

    fn closed_cleanly(self) -> bool {
        self == Self::Closed
    }
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
fn native_target_oracle_summary(
    outcome: &learning_native_oracle::NativeTargetOracleOutcomeV1,
) -> serde_json::Value {
    let setup_receipt = outcome.setup.receipt.as_ref().map(|receipt| {
        serde_json::json!({
            "artifact_type": receipt.artifact_type,
            "schema_version": receipt.schema_version,
            "run_id": receipt.run_id,
            "case_id": receipt.case_id,
            "fixture_id": receipt.fixture_id,
            "process_identity": native_target_process_identity_summary(Some(&receipt.process_identity)),
            "recipe_sha256": receipt.recipe_sha256,
            "title": receipt.title,
            "control": receipt.control,
            "other_control": receipt.other_control,
            "ready_uptime_ns": receipt.ready_uptime_ns,
            "setup_observation": receipt.setup_observation.as_ref().map(|observation| serde_json::json!({
                "source": observation.source,
                "local_state": observation.local_state,
                "independent_ax_readiness": observation.independent_ax_readiness,
                "window_visible": observation.window_visible,
                "target_attached": observation.target_attached,
                "other_attached": observation.other_attached,
                "declared_text_and_range": observation.declared_text_and_range,
                "target_first_responder": observation.target_first_responder,
            })),
            "native_application": receipt.native_application.as_ref().map(|application| serde_json::json!({
                "source": application.source,
                "state": application.state,
                "bundle_identifier": application.bundle_identifier,
                "info_plist_sha256": application.info_plist_sha256,
                "reason": application.reason,
            })),
        })
    });
    let ax_readback = outcome.actual_sample.ax_readback.as_ref().map(|readback| {
        serde_json::json!({
            "source": readback.source,
            "applicability": readback.applicability,
            "reason": readback.reason,
            "startup_retryable": readback.startup_retryable,
            "process_identity_before": readback.process_identity_before,
            "process_identity_after": readback.process_identity_after,
            "process_identity_error": readback.process_identity_error,
            "binding": {
                "coverage": readback.binding.coverage,
                "window_count": readback.binding.window_count,
                "window_titles_examined": readback.binding.window_titles_examined,
                "matching_windows_observed": readback.binding.matching_windows_observed,
                "elements_examined": readback.binding.elements_examined,
                "pending_elements": readback.binding.pending_elements,
                "matching_controls_observed": readback.binding.matching_controls_observed,
                "matching_other_controls_observed": readback.binding.matching_other_controls_observed,
            },
            "attributes": readback.attributes.iter().map(|attribute| serde_json::json!({
                "element_path": attribute.element_path,
                "attribute": attribute.attribute,
                "operation": attribute.operation,
                "error_code": attribute.error_code,
                "error_name": attribute.error_name,
                "state": attribute.state,
                "returned_count": attribute.returned_count,
                "detail": attribute.detail,
            })).collect::<Vec<_>>(),
        })
    });
    serde_json::json!({
        "artifact_type": outcome.artifact_type,
        "schema_version": outcome.schema_version,
        "plan_id": outcome.plan_id,
        "phase_id": outcome.phase_id,
        "operation_id": outcome.operation_id,
        "session_id": outcome.session_id,
        "helper_pid": outcome.helper_pid,
        "overall_state": outcome.overall_state,
        "source_manifest_sha256": outcome.source_manifest_sha256,
        "helper_executable_sha256": outcome.helper_executable_sha256,
        "host_executable_sha256": outcome.host_executable_sha256,
        "host_bundle_identifier": outcome.host_bundle_identifier,
        "recipe_sha256": outcome.recipe_sha256,
        "setup": {
            "state": outcome.setup.state,
            "receipt_sha256": outcome.setup.receipt_sha256,
            "receipt": setup_receipt,
            "reason": outcome.setup.reason,
        },
        "actual_sample": {
            "state": outcome.actual_sample.state,
            "source": outcome.actual_sample.source,
            "sampled_uptime_ns": outcome.actual_sample.sampled_uptime_ns,
            "value_sha256": outcome.actual_sample.value_sha256,
            "value_byte_length": outcome.actual_sample.value_byte_length,
            "binding_mode": outcome.actual_sample.binding_mode,
            "ax_readback": ax_readback,
            "process_identity_before": native_target_process_identity_summary(
                outcome.actual_sample.process_identity_before.as_ref(),
            ),
            "process_identity_after": native_target_process_identity_summary(
                outcome.actual_sample.process_identity_after.as_ref(),
            ),
        },
        "cleanup": {
            "state": outcome.cleanup.state,
            "child_pid": outcome.cleanup.child_pid,
            "termination_status": outcome.cleanup.termination_status,
            "exit_confirmed": outcome.cleanup.exit_confirmed,
            "reason": outcome.cleanup.reason,
        },
        "helper_expected_comparison": outcome.expected_comparison,
        "target_terminal": outcome.target_terminal,
        "unknown_reason": outcome.unknown_reason,
    })
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
fn native_target_process_identity_summary(
    observation: Option<&learning_native_oracle::ProcessIdentityObservationV1>,
) -> serde_json::Value {
    let Some(observation) = observation else {
        return serde_json::Value::Null;
    };
    serde_json::json!({
        "requested_pid": observation.requested_pid,
        "kernel_state": observation.kernel_state,
        "kernel": observation.kernel.as_ref().map(|identity| serde_json::json!({
            "source": identity.source,
            "pid": identity.pid,
            "parent_pid": identity.parent_pid,
            "user_id": identity.user_id,
            "start_seconds": identity.start_seconds,
            "start_microseconds": identity.start_microseconds,
            "executable_path": identity.executable_path,
        })),
        "kernel_error": observation.kernel_error,
        "cocoa_launch_date_source": observation.cocoa_launch_date_source,
        "cocoa_launch_date_state": observation.cocoa_launch_date_state,
        "cocoa_launch_date": observation.cocoa_launch_date,
    })
}

#[cfg(any(test, feature = "research-harness"))]
fn learning_asr_error_details(
    error: &crate::asr::AsrError,
    paths: &[(&PathBuf, &str)],
) -> (&'static str, String) {
    use crate::asr::AsrError;

    let (category, detail) = match error {
        AsrError::Network(detail) => ("network", detail.as_str()),
        AsrError::Timeout => ("timeout", "request timed out"),
        AsrError::ContextAuthorizationChanged => (
            "context_authorization_changed",
            "context authorization changed during transcription",
        ),
        AsrError::Unauthorized(detail) => ("unauthorized", detail.as_str()),
        AsrError::RateLimited(detail) => ("rate_limited", detail.as_str()),
        AsrError::RetryableServer { message, .. } => ("retryable_server", message.as_str()),
        AsrError::EmptyResult => ("empty_result", "empty speech result"),
        AsrError::OnDeviceModelMissing(detail) => ("model_missing", detail.as_str()),
        AsrError::OnDeviceInferenceUnavailable(detail) => {
            ("inference_unavailable", detail.as_str())
        }
        AsrError::Other(detail) => ("other", detail.as_str()),
    };
    (category, sanitize_research_diagnostic(detail, paths))
}

#[cfg(any(test, feature = "research-harness"))]
fn sanitize_research_diagnostic(detail: &str, paths: &[(&PathBuf, &str)]) -> String {
    let mut redactions = paths
        .iter()
        .map(|(path, replacement)| (path.to_string_lossy().into_owned(), *replacement))
        .filter(|(path, _)| !path.is_empty())
        .collect::<Vec<_>>();
    redactions.sort_by_key(|(path, _)| std::cmp::Reverse(path.len()));
    let mut sanitized = detail.to_owned();
    for (path, replacement) in redactions {
        sanitized = sanitized.replace(&path, replacement);
    }
    sanitized
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(512)
        .collect()
}

#[cfg(feature = "research-harness")]
fn sanitize_learning_operation_error(error: anyhow::Error) -> String {
    let reason = error.to_string();
    if reason.len() <= 64
        && reason
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_' || byte.is_ascii_digit())
    {
        reason
    } else {
        "learning_operation_rejected".into()
    }
}

#[cfg(feature = "research-harness")]
fn is_unsupported_learning_reason(reason: &str) -> bool {
    matches!(
        reason,
        "unsupported_learning_source"
            | "unsupported_learning_actor"
            | "unsupported_target_oracle"
            | "unsupported_text_layer_authority"
            | "unsupported_edit_state_count"
    )
}

#[cfg(feature = "research-harness")]
struct StandaloneLearningConfig {
    base_root: PathBuf,
    wav_root: Option<PathBuf>,
    model_source_root: Option<PathBuf>,
    native_oracle_manifest: Option<PathBuf>,
    owned_paste_session_manifest: Option<PathBuf>,
    owned_paste_session_supervisor: bool,
}

#[cfg(feature = "research-harness")]
fn parse_standalone_arguments(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Result<StandaloneLearningConfig, &'static str> {
    let args = args.into_iter().collect::<Vec<_>>();
    let mut arm_root = None;
    let mut wav_root = None;
    let mut model_source_root = None;
    let mut native_oracle_manifest = None;
    let mut owned_paste_session_manifest = None;
    let mut owned_paste_session_supervisor = false;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index]
            .to_str()
            .ok_or("learning_harness_argument_invalid")?;
        if flag == "--owned-paste-session-supervisor" {
            if owned_paste_session_supervisor {
                return Err("learning_harness_argument_duplicate");
            }
            owned_paste_session_supervisor = true;
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .ok_or("learning_harness_argument_missing_value")?;
        let target = match flag {
            "--arm-root" => &mut arm_root,
            "--wav-root" => &mut wav_root,
            "--model-source-root" => &mut model_source_root,
            "--native-oracle-manifest" => &mut native_oracle_manifest,
            "--owned-paste-session-manifest" => &mut owned_paste_session_manifest,
            _ => return Err("learning_harness_argument_unsupported"),
        };
        if target.replace(PathBuf::from(value)).is_some() {
            return Err("learning_harness_argument_duplicate");
        }
        index += 2;
    }
    let root = arm_root.ok_or(
        "usage: voiceflow-learning-harness --arm-root ABSOLUTE_TEMP_DIRECTORY [--wav-root PUBLIC_WAV_ROOT --model-source-root VERIFIED_MODEL_ROOT] [--native-oracle-manifest PREPARED_JSON | --owned-paste-session-manifest PREPARED_JSON [--owned-paste-session-supervisor]]",
    )?;
    if native_oracle_manifest
        .as_ref()
        .is_some_and(|path| !path.is_absolute())
    {
        return Err("native_oracle_manifest_must_be_absolute");
    }
    if owned_paste_session_manifest
        .as_ref()
        .is_some_and(|path| !path.is_absolute())
    {
        return Err("owned_paste_session_manifest_must_be_absolute");
    }
    if native_oracle_manifest.is_some() && owned_paste_session_manifest.is_some() {
        return Err("learning_harness_native_manifests_conflict");
    }
    if owned_paste_session_supervisor
        && (owned_paste_session_manifest.is_none()
            || native_oracle_manifest.is_some()
            || wav_root.is_some()
            || model_source_root.is_some())
    {
        return Err("owned_paste_session_supervisor_requires_its_manifest");
    }
    if wav_root.is_some() != model_source_root.is_some() {
        return Err("learning_wav_roots_must_be_paired");
    }
    let base_root = validate_arm_base_root(&root).map_err(|_| "learning_arm_root_rejected")?;
    let wav_root = wav_root
        .map(|path| validate_external_directory(&path))
        .transpose()
        .map_err(|_| "learning_wav_root_rejected")?;
    let model_source_root = model_source_root
        .map(|path| validate_external_directory(&path))
        .transpose()
        .map_err(|_| "learning_model_source_root_rejected")?;
    for external_root in [wav_root.as_deref(), model_source_root.as_deref()]
        .into_iter()
        .flatten()
    {
        if base_root.starts_with(external_root) || external_root.starts_with(&base_root) {
            return Err("learning_wav_root_overlaps_arm_root");
        }
    }
    Ok(StandaloneLearningConfig {
        base_root,
        wav_root,
        model_source_root,
        native_oracle_manifest,
        owned_paste_session_manifest,
        owned_paste_session_supervisor,
    })
}

#[cfg(any(test, feature = "research-harness"))]
fn validate_external_directory(path: &Path) -> anyhow::Result<PathBuf> {
    if !path.is_absolute() {
        anyhow::bail!("research input directory must be absolute");
    }
    reject_symlink_directory(path)?;
    let canonical = path.canonicalize()?;
    reject_symlink_directory(&canonical)?;
    Ok(canonical)
}

#[cfg(feature = "research-harness")]
fn read_bounded_line(reader: &mut impl std::io::Read, line: &mut Vec<u8>) -> std::io::Result<bool> {
    line.clear();
    loop {
        let mut byte = [0_u8; 1];
        match reader.read(&mut byte)? {
            0 if line.is_empty() => return Ok(false),
            0 => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "unterminated request",
                ))
            }
            _ if byte[0] == b'\n' => return Ok(true),
            _ if line.len() >= LEARNING_STDIN_MAX_BYTES => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "request too large",
                ));
            }
            _ => line.push(byte[0]),
        }
    }
}

/// Separate feature-gated stdio binary. It does not initialize Tauri, load
/// normal settings, access Keychain, or start providers.
#[cfg(feature = "research-harness")]
pub(crate) fn run_standalone_stdio(args: impl IntoIterator<Item = std::ffi::OsString>) -> i32 {
    use std::collections::HashSet;
    use std::io::{BufReader, Write};

    let args = args.into_iter().collect::<Vec<_>>();
    if args.len() == 1 && args[0].to_str() == Some("--mainqueue-probe") {
        return run_standalone_mainqueue_probe();
    }
    if args.first().and_then(|arg| arg.to_str()) == Some("--owned-paste-session-child") {
        #[cfg(target_os = "macos")]
        {
            return crate::learning_clipboard_guard::run_registered_owned_paste_session_child_cli(
                args.into_iter().skip(1),
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            eprintln!("registered owned paste session is supported only on macOS");
            return 2;
        }
    }
    if args.first().and_then(|arg| arg.to_str()) == Some("--clipboard-guard-general-component") {
        return run_standalone_general_clipboard_guard(args.into_iter().skip(1), false);
    }
    if args.first().and_then(|arg| arg.to_str()) == Some("--clipboard-guard-general-remote-probe") {
        return run_standalone_general_remote_clipboard_probe(args.into_iter().skip(1));
    }
    if args.first().and_then(|arg| arg.to_str()) == Some("--clipboard-guard-general-child") {
        return run_standalone_general_clipboard_guard(args.into_iter().skip(1), true);
    }
    if args.first().and_then(|arg| arg.to_str()) == Some("--clipboard-guard-remote-probe") {
        return run_standalone_remote_clipboard_guard(args.into_iter().skip(1), false);
    }
    if args.first().and_then(|arg| arg.to_str()) == Some("--clipboard-guard-remote-probe-child") {
        return run_standalone_remote_clipboard_guard(args.into_iter().skip(1), true);
    }
    if args.first().and_then(|arg| arg.to_str()) == Some("--clipboard-guard-component") {
        return run_standalone_clipboard_guard(args.into_iter().skip(1), false);
    }
    if args.first().and_then(|arg| arg.to_str()) == Some("--clipboard-guard-child") {
        return run_standalone_clipboard_guard(args.into_iter().skip(1), true);
    }

    let config = match parse_standalone_arguments(args) {
        Ok(config) => config,
        Err(reason) => {
            eprintln!("{reason}");
            return 2;
        }
    };
    let mut output = std::io::stdout().lock();
    let hello = serde_json::json!({
        "protocol": "voiceflow_learning_stdio_v2",
        "schema_version": LEARNING_EPISODE_SCHEMA_V2,
        "pid": std::process::id(),
        "max_command_bytes": LEARNING_STDIN_MAX_BYTES,
        "max_requests": LEARNING_REQUEST_MAX,
        "target_insertion": "registered_owned_research_phase_only",
        "product_target_observation": "not_connected",
        "independent_target_oracle": "requires_explicit_registered_v3_phase",
        "keychain": "not_accessed",
        "provider": "not_started",
    });
    if writeln!(output, "{hello}")
        .and_then(|_| output.flush())
        .is_err()
    {
        return 2;
    }
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut dispatcher = StandaloneLearningDispatcher::new(config);
    let mut line = Vec::new();
    let mut request_ids = HashSet::new();
    for _ in 0..LEARNING_REQUEST_MAX {
        match read_bounded_line(&mut reader, &mut line) {
            Ok(false) => {
                dispatcher.hold_process_while_general_owner_is_pending();
                break;
            }
            Ok(true) => {}
            Err(_) => {
                let response =
                    serde_json::json!({"ok": false, "reason": "request_framing_rejected"});
                if writeln!(output, "{response}")
                    .and_then(|_| output.flush())
                    .is_err()
                {
                    dispatcher.hold_process_while_general_owner_is_pending();
                    return 2;
                }
                dispatcher.hold_process_while_general_owner_is_pending();
                return 2;
            }
        }
        let pending_command = std::str::from_utf8(&line).ok();
        let response = pending_command
            .and_then(|command| dispatcher.resolve_pending_owned_paste_owner_command(command))
            .unwrap_or_else(|| {
                match serde_json::from_slice::<StandaloneLearningRequest>(&line) {
                    Ok(request) => {
                        if crate::onboarding::validate_id(&request.request_id).is_err()
                            || !request_ids.insert(request.request_id.clone())
                        {
                            serde_json::json!({"ok": false, "reason": "invalid_or_reused_request_uuid"})
                        } else {
                            match dispatcher.dispatch(request.command) {
                                Ok(result) => {
                                    serde_json::json!({"request_id": request.request_id, "ok": true, "result": result})
                                }
                                Err(reason) => {
                                    serde_json::json!({"request_id": request.request_id, "ok": false, "reason": reason})
                                }
                            }
                        }
                    }
                    Err(_) => serde_json::json!({"ok": false, "reason": "invalid_request"}),
                }
            });
        if writeln!(output, "{response}")
            .and_then(|_| output.flush())
            .is_err()
        {
            dispatcher.hold_process_while_general_owner_is_pending();
            return 2;
        }
    }
    dispatcher.hold_process_while_general_owner_is_pending();
    0
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
fn run_standalone_mainqueue_probe() -> i32 {
    use std::io::Write;

    if unsafe { libc::pthread_main_np() } == 0 {
        let response = serde_json::json!({
            "protocol": "voiceflow_research_mainqueue_probe_v1",
            "ok": false,
            "reason": "caller_not_os_main_thread",
            "clipboard": "not_touched",
            "accessibility": "not_touched",
            "input_source_selection": "not_changed",
        });
        let mut output = std::io::stdout().lock();
        let _ = writeln!(output, "{response}").and_then(|_| output.flush());
        return 2;
    }

    let (ok, response) = match crate::paste::run_research_main_queue_dispatch_probe() {
        Ok(callback_on_main) => (
            callback_on_main,
            serde_json::json!({
                "protocol": "voiceflow_research_mainqueue_probe_v1",
                "ok": callback_on_main,
                "caller_is_os_main_thread": true,
                "worker_callback_on_os_main_thread": callback_on_main,
                "owner_join": "closed",
                "clipboard": "not_touched",
                "accessibility": "not_touched",
                "input_source_selection": "not_changed",
            }),
        ),
        Err(reason) => (
            false,
            serde_json::json!({
                "protocol": "voiceflow_research_mainqueue_probe_v1",
                "ok": false,
                "reason": reason,
                "caller_is_os_main_thread": true,
                "owner_join": "not_confirmed",
                "clipboard": "not_touched",
                "accessibility": "not_touched",
                "input_source_selection": "not_changed",
            }),
        ),
    };
    let mut output = std::io::stdout().lock();
    if writeln!(output, "{response}")
        .and_then(|_| output.flush())
        .is_err()
    {
        return 2;
    }
    if ok {
        0
    } else {
        1
    }
}

#[cfg(all(feature = "research-harness", not(target_os = "macos")))]
fn run_standalone_mainqueue_probe() -> i32 {
    use std::io::Write;

    let response = serde_json::json!({
        "protocol": "voiceflow_research_mainqueue_probe_v1",
        "ok": false,
        "reason": "unsupported_platform",
        "clipboard": "not_touched",
        "accessibility": "not_touched",
        "input_source_selection": "not_changed",
    });
    let mut output = std::io::stdout().lock();
    let _ = writeln!(output, "{response}").and_then(|_| output.flush());
    2
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
fn run_standalone_clipboard_guard(
    args: impl IntoIterator<Item = std::ffi::OsString>,
    child: bool,
) -> i32 {
    if child {
        crate::learning_clipboard_guard::run_child_cli(args)
    } else {
        crate::learning_clipboard_guard::run_component_cli(args)
    }
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
fn run_standalone_general_clipboard_guard(
    args: impl IntoIterator<Item = std::ffi::OsString>,
    child: bool,
) -> i32 {
    if child {
        crate::learning_clipboard_guard::run_general_child_cli(args)
    } else {
        crate::learning_clipboard_guard::run_general_component_cli(args)
    }
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
fn run_standalone_remote_clipboard_guard(
    args: impl IntoIterator<Item = std::ffi::OsString>,
    child: bool,
) -> i32 {
    if child {
        crate::learning_clipboard_guard::run_remote_probe_child_cli(args)
    } else {
        crate::learning_clipboard_guard::run_remote_probe_component_cli(args)
    }
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
fn run_standalone_general_remote_clipboard_probe(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> i32 {
    crate::learning_clipboard_guard::run_general_remote_probe_component_cli(args)
}

#[cfg(all(feature = "research-harness", not(target_os = "macos")))]
fn run_standalone_remote_clipboard_guard(
    _args: impl IntoIterator<Item = std::ffi::OsString>,
    _child: bool,
) -> i32 {
    eprintln!("named remote clipboard probe is supported only on macOS");
    2
}

#[cfg(all(feature = "research-harness", not(target_os = "macos")))]
fn run_standalone_general_remote_clipboard_probe(
    _args: impl IntoIterator<Item = std::ffi::OsString>,
) -> i32 {
    eprintln!("general remote clipboard probe is supported only on macOS");
    2
}

#[cfg(all(feature = "research-harness", not(target_os = "macos")))]
fn run_standalone_general_clipboard_guard(
    _args: impl IntoIterator<Item = std::ffi::OsString>,
    _child: bool,
) -> i32 {
    eprintln!("general clipboard guard component is supported only on macOS");
    2
}

#[cfg(all(feature = "research-harness", not(target_os = "macos")))]
fn run_standalone_clipboard_guard(
    _args: impl IntoIterator<Item = std::ffi::OsString>,
    _child: bool,
) -> i32 {
    eprintln!("clipboard guard component is supported only on macOS");
    2
}

impl LearningConsent {
    #[cfg(any(test, all(feature = "research-harness", target_os = "macos")))]
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        for id in [&self.selected_state_id, &self.edit_fixture_id] {
            crate::onboarding::validate_id(id).map_err(|_| "invalid_learning_fixture_uuid")?;
        }
        if let Some(id) = &self.baseline_run {
            crate::onboarding::validate_id(id).map_err(|_| "invalid_baseline_uuid")?;
        }
        if self.mapping_id.is_empty()
            || self.mapping_id.len() > 128
            || self.bundle_id.is_empty()
            || self.bundle_id.len() > 256
            || self.terms.is_empty()
            || self.terms.len() > 8
            || self.post_insert_field.len() > 1024
            || self.settled_field.len() > 1024
        {
            return Err("learning_fixture_bounds");
        }
        let mut ids = std::collections::HashSet::new();
        let mut pairs = std::collections::HashSet::new();
        for term in &self.terms {
            crate::onboarding::validate_id(&term.fixture_id)
                .map_err(|_| "invalid_term_fixture_uuid")?;
            if !ids.insert(&term.fixture_id)
                || !pairs.insert(crate::dictionary_learn::pair_key(&term.before, &term.after))
                || term.before.is_empty()
                || term.after.is_empty()
                || term.before.len() > 256
                || term.after.len() > 256
                || term.before.chars().any(char::is_control)
                || term.after.chars().any(char::is_control)
            {
                return Err("learning_term_bounds_or_duplicate");
            }
        }
        Ok(())
    }
    pub(crate) fn term(&self, before: &str, after: &str) -> Option<&TermFixture> {
        self.terms
            .iter()
            .find(|term| term.before == before && term.after == after)
    }
    #[cfg(any(test, all(feature = "research-harness", target_os = "macos")))]
    pub(crate) fn settings_match(&self, settings: &store::Settings) -> bool {
        settings.snippets.is_empty()
            && !settings.browser_access_enabled
            && !settings.window_ocr_enabled
            && !settings.always_on_microphone
            && settings.context_enabled
            && settings.dictionary_learn_enabled == self.observer_enabled
            && settings.dictionary.len() <= self.terms.len()
            && settings
                .dictionary
                .iter()
                .all(|word| self.terms.iter().any(|term| term.after == *word))
            && settings.context_mappings.len() == 1
            && settings.context_mappings.iter().all(|mapping| {
                mapping.label.len() <= 256
                    && mapping.mode_id.as_ref().is_none_or(|id| id.len() <= 128)
                    && mapping.id == self.mapping_id
                    && mapping.bundle_id.as_deref() == Some(self.bundle_id.as_str())
                    && mapping.browser_host.is_none()
                    && mapping.browser_path_prefix.is_none()
                    && mapping.executable.is_none()
                    && mapping.style_example_pairs.is_empty()
                    && mapping.style_example_input.is_none()
                    && mapping.style_example_output.is_none()
                    && !mapping.style_examples_approved
                    && mapping.enabled
                    && mapping.dictionary_learn_enabled == self.observer_enabled
                    && mapping.source_permissions.ax_text
                    && !mapping.source_permissions.local_ocr
                    && !mapping.source_permissions.cloud_vision
                    && !mapping.source_permissions.context_text_to_providers
            })
    }
}

/// Closed producing stage contract; stale persistence is a real observation,
/// distinct from completed SQL or settings persistence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LearningStage {
    #[cfg(any(test, all(feature = "research-harness", target_os = "macos")))]
    SelectedState,
    #[cfg(any(test, feature = "research-harness"))]
    SelectedPairRead,
    #[cfg(any(test, feature = "research-harness"))]
    ObserverRegistration,
    Observation,
    Classification,
    CandidateStore,
    #[cfg(any(test, feature = "research-harness"))]
    StyleCandidateStore,
    StoreGuard,
    Persistence,
    SettingsPersistence,
    StyleStore,
    #[cfg(any(test, feature = "research-harness"))]
    LocalPrepareUsage,
    #[cfg(any(test, all(feature = "research-harness", target_os = "macos")))]
    UndoDelete,
}

#[derive(Serialize)]
pub(crate) struct LearningRecordV1 {
    pub schema_version: u8,
    pub selected_state_id: String,
    pub baseline_run: Option<String>,
    pub edit_fixture_id: String,
    pub purpose: LearningPurpose,
    pub term_fixture_id: Option<String>,
    pub stage: LearningStage,
    pub reason: &'static str,
    pub eligible: Option<bool>,
    pub same_field_window: &'static str,
    pub attribution: &'static str,
    pub candidate_class: Option<&'static str>,
    pub hits_before: Option<u32>,
    pub hits_after: Option<u32>,
    pub promote_threshold: Option<u32>,
    pub promoted: Option<bool>,
    pub persistence: &'static str,
    pub persistence_version: Option<String>,
    pub hint_used: Option<bool>,
    pub replacement_used: Option<bool>,
    pub guard_effect: &'static str,
    pub undo_delete: Option<&'static str>,
    pub next_unseen_utterance: &'static str,
}
impl LearningRecordV1 {
    pub(crate) fn new(
        consent: &LearningConsent,
        stage: LearningStage,
        reason: &'static str,
    ) -> Self {
        Self {
            schema_version: 1,
            selected_state_id: consent.selected_state_id.clone(),
            baseline_run: consent.baseline_run.clone(),
            edit_fixture_id: consent.edit_fixture_id.clone(),
            purpose: consent.purpose,
            term_fixture_id: None,
            stage,
            reason,
            eligible: None,
            same_field_window: "unknown",
            attribution: "whole_field_edit_not_asr_correction_proof",
            candidate_class: None,
            hits_before: None,
            hits_after: None,
            promote_threshold: None,
            promoted: None,
            persistence: "not_observed",
            persistence_version: None,
            hint_used: None,
            replacement_used: None,
            guard_effect: "not_observed",
            undo_delete: None,
            next_unseen_utterance: "not_measured",
        }
    }
}
/// Default builds have no consent source (no scoped fixture, no trace lease),
/// so the production hooks are inline no-ops there.
#[cfg(not(any(test, feature = "research-harness")))]
mod inert_hooks {
    use super::{LearningConsent, LearningRecordV1, LearningStage};
    #[inline(always)]
    pub(crate) fn consent() -> Option<LearningConsent> {
        None
    }
    #[inline(always)]
    pub(crate) fn emit(_record: LearningRecordV1) {}
    #[inline(always)]
    pub(crate) fn registration(_eligible: bool, _reason: &'static str) {}
    #[inline(always)]
    pub(crate) fn stage(_stage: LearningStage, _reason: &'static str) {}
    #[inline(always)]
    pub(crate) fn pair_allowed(_before: &str, _after: &str) -> bool {
        true
    }
    #[inline(always)]
    pub(crate) fn style_candidate(_hits: u32, _threshold: u32, _kind: &'static str) {}
    #[inline(always)]
    pub(crate) fn usage(_terms: &[String], _hint: bool) {}
    #[inline(always)]
    pub(crate) fn selected_pairs(_dir: &std::path::Path) -> Vec<crate::store::LearnPairRecord> {
        Vec::new()
    }
}
#[cfg(not(any(test, feature = "research-harness")))]
pub(crate) use inert_hooks::*;

#[cfg(any(test, feature = "research-harness"))]
pub(crate) fn consent() -> Option<LearningConsent> {
    if let Some(consent) = SCOPED_CONSENT.with(|slot| slot.borrow().clone()) {
        return Some(consent);
    }
    research_trace::current()?.learning_consent().cloned()
}
#[cfg(any(test, feature = "research-harness"))]
pub(crate) fn emit(record: LearningRecordV1) {
    research_trace::emit(research_trace::Fact::LearningRecord {
        record: Box::new(record),
    });
}
#[cfg(any(test, feature = "research-harness"))]
pub(crate) fn registration(eligible: bool, reason: &'static str) {
    if let Some(consent) = consent() {
        let mut record =
            LearningRecordV1::new(&consent, LearningStage::ObserverRegistration, reason);
        record.eligible = Some(eligible);
        emit(record);
    }
}
#[cfg(any(test, feature = "research-harness"))]
pub(crate) fn stage(stage: LearningStage, reason: &'static str) {
    if let Some(consent) = consent() {
        emit(LearningRecordV1::new(&consent, stage, reason));
    }
}
#[cfg(any(test, feature = "research-harness"))]
pub(crate) fn pair_allowed(before: &str, after: &str) -> bool {
    consent().is_none_or(|consent| consent.term(before, after).is_some())
}
#[cfg(any(test, feature = "research-harness"))]
pub(crate) fn style_candidate(hits: u32, threshold: u32, kind: &'static str) {
    if let Some(consent) = consent() {
        let mut record = LearningRecordV1::new(&consent, LearningStage::StyleCandidateStore, kind);
        record.hits_after = Some(hits);
        record.promote_threshold = Some(threshold);
        record.persistence = "candidate_sql_returned_settings_not_proven";
        record.guard_effect = "style_or_intensity_candidate_not_word";
        emit(record);
    }
}
#[cfg(any(test, feature = "research-harness"))]
pub(crate) fn usage(terms: &[String], hint: bool) {
    let Some(consent) = consent() else {
        return;
    };
    for term in &consent.terms {
        let mut record = LearningRecordV1::new(
            &consent,
            LearningStage::LocalPrepareUsage,
            "actual_owner_output_not_accuracy",
        );
        record.term_fixture_id = Some(term.fixture_id.clone());
        let used = terms.contains(&term.after);
        if hint {
            record.hint_used = Some(used);
        } else {
            record.replacement_used = Some(used);
        }
        record.next_unseen_utterance = "local_prepare_only_quality_unknown";
        emit(record);
    }
}
#[cfg(any(test, feature = "research-harness"))]
fn row_within_bounds(row: &store::LearnPairRecord) -> bool {
    row.pair_key.len() <= 1024
        && row.before_surface.len() <= 256
        && row.after_surface.len() <= 256
        && row.last_at.len() <= 128
        && [
            &row.family,
            &row.mapping_id,
            &row.browser_host,
            &row.native_bundle,
            &row.last_used_at,
            &row.tombstoned_at,
        ]
        .iter()
        .all(|value| value.as_ref().is_none_or(|value| value.len() <= 512))
}

#[cfg(any(test, feature = "research-harness"))]
fn row_within_episode_scope(row: &store::LearnPairRecord, scope: &LearningScopeV2) -> bool {
    row.family.as_deref() == Some(crate::context::family_id(scope.family))
        && row.mapping_id.as_deref() == Some(scope.mapping_id.as_str())
        && row.browser_host.is_none()
        && row.native_bundle.as_deref() == Some(scope.native_bundle.as_str())
}

#[cfg(any(test, feature = "research-harness"))]
fn learning_pair_state(row: &store::LearnPairRecord) -> &'static str {
    if row.tombstoned_at.is_some() {
        "tombstoned"
    } else if row.ignored {
        "ignored"
    } else if row.is_live_promoted() {
        "promoted"
    } else {
        "pending"
    }
}

#[cfg(any(test, feature = "research-harness"))]
pub(crate) fn selected_pairs(dir: &std::path::Path) -> Vec<store::LearnPairRecord> {
    let Some(consent) = consent() else {
        return Vec::new();
    };
    // Exact caller-selected keys only: never enumerate an ambient learning DB.
    consent
        .terms
        .iter()
        .filter_map(|term| {
            match store::get_learn_pair(
                dir,
                &crate::dictionary_learn::pair_key(&term.before, &term.after),
            ) {
                Ok(Some(row))
                    if row.before_surface == term.before
                        && row.after_surface == term.after
                        && row_within_bounds(&row) =>
                {
                    Some(row)
                }
                Ok(None) => None,
                Ok(Some(_)) => {
                    stage(
                        LearningStage::SelectedPairRead,
                        "source_surface_or_bounds_mismatch",
                    );
                    None
                }
                Err(_) => {
                    stage(LearningStage::SelectedPairRead, "read_failed");
                    None
                }
            }
        })
        .collect()
}
#[derive(Serialize)]
#[cfg(any(test, feature = "research-harness"))]
pub(crate) struct SelectedLearningState {
    pub identity: String,
    pub records: Vec<LearningRecordV1>,
}
/// Content identity of only the caller-selected state, never an ambient DB dump.
#[cfg(any(test, feature = "research-harness"))]
pub(crate) fn selected_state_snapshot(
    consent: &LearningConsent,
    dir: &std::path::Path,
    settings: &store::Settings,
) -> anyhow::Result<SelectedLearningState> {
    use sha2::{Digest, Sha256};
    let rows = consent
        .terms
        .iter()
        .map(|term| {
            store::get_learn_pair(
                dir,
                &crate::dictionary_learn::pair_key(&term.before, &term.after),
            )
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    for (term, row) in consent.terms.iter().zip(&rows) {
        if row.as_ref().is_some_and(|row| {
            row.before_surface != term.before
                || row.after_surface != term.after
                || !row_within_bounds(row)
        }) {
            anyhow::bail!("selected normalized key has different or unbounded source facts");
        }
    }
    if matches!(consent.purpose, LearningPurpose::Baseline)
        && (!settings.dictionary.is_empty() || rows.iter().any(Option::is_some))
    {
        anyhow::bail!(
            "baseline requires caller-selected fresh state; never clear it automatically"
        );
    }
    let mut records = Vec::new();
    for (term, row) in consent.terms.iter().zip(&rows) {
        let mut record = LearningRecordV1::new(
            consent,
            LearningStage::SelectedState,
            "caller_selected_pair_readback",
        );
        record.term_fixture_id = Some(term.fixture_id.clone());
        record.hits_before = Some(row.as_ref().map_or(0, |row| row.hits));
        record.promoted = Some(
            row.as_ref()
                .is_some_and(store::LearnPairRecord::is_live_promoted),
        );
        record.persistence = if row.is_some() {
            "selected_pair_readback"
        } else {
            "selected_pair_absent"
        };
        record.persistence_version = Some(hex::encode(Sha256::digest(serde_json::to_vec(row)?)));
        records.push(record);
    }
    let bytes = serde_json::to_vec(&(
        consent,
        &settings.dictionary,
        &settings.context_mappings,
        &rows,
    ))?;
    let mut hash = Sha256::new();
    hash.update(b"voiceflow-selected-learning-state-v1\0");
    hash.update(bytes);
    Ok(SelectedLearningState {
        identity: hex::encode(hash.finalize()),
        records,
    })
}

#[cfg(test)]
mod fixtures {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "voiceflow-learning-v2-test-{}-{nonce}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
            Self(root)
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

    fn fixture_episode() -> (LearningEpisodeV2, LearningTextOperationV2) {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/learning-v2-text.json")).unwrap();
        let episode = serde_json::from_value(fixture["episode"].clone()).unwrap();
        let operation =
            serde_json::from_value(fixture["apply_text_template"]["operation"].clone()).unwrap();
        (episode, operation)
    }

    fn operation_for(
        episode: &LearningEpisodeV2,
        template: &LearningTextOperationV2,
        index: usize,
    ) -> LearningTextOperationV2 {
        let mut operation = template.clone();
        operation.operation_id = episode.operations[index].operation_id.clone();
        operation
    }

    #[test]
    fn consent_rejects_unknown_fields_and_normalized_key_aliases() {
        let mut value = serde_json::json!({"selected_state_id":"40000000-0000-4000-8000-000000000001","edit_fixture_id":"40000000-0000-4000-8000-000000000002","purpose":"negative","mapping_id":"fixture","bundle_id":"com.example.fixture","observer_enabled":true,"post_insert_field":"alpha","settled_field":"Alpha","terms":[{"fixture_id":"40000000-0000-4000-8000-000000000003","before":"alpha","after":"Alpha"}]});
        let consent: LearningConsent = serde_json::from_value(value.clone()).unwrap();
        assert!(consent.validate().is_ok());
        value["terms"].as_array_mut().unwrap().push(serde_json::json!({"fixture_id":"40000000-0000-4000-8000-000000000004","before":"Alpha","after":"ALPHA"}));
        assert!(serde_json::from_value::<LearningConsent>(value.clone())
            .unwrap()
            .validate()
            .is_err());
        value["ambient_dictionary"] = serde_json::json!(true);
        assert!(serde_json::from_value::<LearningConsent>(value).is_err());
    }

    #[test]
    fn v2_text_arm_records_deduplicates_promotes_and_reopens_isolated_state() {
        let root = TestRoot::new();
        let (mut episode, template) = fixture_episode();
        episode.terms.push(TermFixture {
            fixture_id: "50000000-0000-4000-8000-000000000020".into(),
            before: "foobaz".into(),
            after: "foobar".into(),
        });
        episode.validate().unwrap();

        let mut arm = LearningArmV2::open(root.path(), &episode).unwrap();
        let initial = arm.selected_state(&episode).unwrap();
        assert_eq!(initial.pairs[0].state, "absent");
        assert_eq!(initial.pairs[0].hits, 0);
        assert_eq!(
            initial.settings_consistency,
            "selected_local_only_settings_unchanged"
        );
        let initial_settings = read_private_bytes(&arm.root.join("settings.json")).unwrap();

        let first = operation_for(&episode, &template, 0);
        let first_receipt = arm.execute_text_operation(&episode, &first).unwrap();
        assert_eq!(first_receipt.observer_outcome, "single_token");
        assert_eq!(first_receipt.hits_after, Some(1));
        assert_eq!(first_receipt.candidate_write, "candidate_written");

        let duplicate = arm.execute_text_operation(&episode, &first).unwrap();
        assert_eq!(duplicate.hits_after, Some(1));
        assert_eq!(duplicate.candidate_write, "duplicate_observation");

        let second = operation_for(&episode, &template, 1);
        let second_receipt = arm.execute_text_operation(&episode, &second).unwrap();
        assert_eq!(second_receipt.hits_after, Some(2));
        assert_eq!(second_receipt.promoted, Some(false));

        let third = operation_for(&episode, &template, 2);
        let third_receipt = arm.execute_text_operation(&episode, &third).unwrap();
        assert_eq!(third_receipt.hits_after, Some(3));
        assert_eq!(third_receipt.promoted, Some(true));
        assert_eq!(
            third_receipt.arm_settings_readback,
            "not_required_local_only_scoped_pair"
        );
        assert_eq!(
            third_receipt.sql_readback,
            "selected_candidate_sql_row_read_back"
        );
        assert!(arm.settings.dictionary.is_empty());
        assert_eq!(
            read_private_bytes(&arm.root.join("settings.json")).unwrap(),
            initial_settings
        );

        drop(arm);
        let mut reopened = LearningArmV2::open(root.path(), &episode).unwrap();
        let reopened_state = reopened.selected_state(&episode).unwrap();
        assert_eq!(reopened_state.settings_sha256, initial.settings_sha256);
        assert_eq!(
            reopened_state.settings_consistency,
            "selected_local_only_settings_unchanged"
        );
        let replay = reopened.execute_text_operation(&episode, &third).unwrap();
        assert_eq!(replay.hits_after, Some(3));
        assert_eq!(replay.promoted, Some(true));
        assert_eq!(replay.candidate_write, "duplicate_observation");
        assert_eq!(
            replay.arm_settings_readback,
            "not_required_local_only_scoped_pair"
        );
        assert!(reopened.settings.dictionary.is_empty());

        let state = reopened.selected_state(&episode).unwrap();
        assert_eq!(
            state.settings_consistency,
            "selected_local_only_settings_unchanged"
        );
        assert_eq!(state.pairs[0].hits, 3);
        assert_eq!(state.pairs[0].state, "promoted");
        assert!(state.pairs[0].promoted);

        let replay_first = reopened.execute_text_operation(&episode, &first).unwrap();
        assert_eq!(replay_first.hits_after, Some(3));
        assert_eq!(replay_first.promoted, Some(true));
        assert_eq!(replay_first.candidate_write, "duplicate_observation");

        assert_eq!(
            read_private_bytes(&reopened.root.join("settings.json")).unwrap(),
            initial_settings
        );
        assert_eq!(first_receipt.insertion, "not_run");
        assert_eq!(first_receipt.ax_observation, "not_run");
        assert_eq!(
            first_receipt.independent_target_oracle,
            "not_authorized_not_run"
        );

        let mut other_episode = episode.clone();
        other_episode.arm_state_id = "50000000-0000-4000-8000-000000000099".into();
        let other_arm = LearningArmV2::open(root.path(), &other_episode).unwrap();
        assert_eq!(
            other_arm.selected_state(&other_episode).unwrap().pairs[0].state,
            "absent"
        );
    }

    #[test]
    fn v2_local_only_promotion_preserves_preexisting_global_dictionary_word() {
        let root = TestRoot::new();
        let (episode, template) = fixture_episode();
        let mut arm = LearningArmV2::open(root.path(), &episode).unwrap();
        arm.settings.dictionary.push("VoiceFlow".into());
        store::save_configuration(&arm.root, &arm.settings).unwrap();
        let settings_before = read_private_bytes(&arm.root.join("settings.json")).unwrap();
        drop(arm);

        let mut arm = LearningArmV2::open(root.path(), &episode).unwrap();
        assert_eq!(arm.settings.dictionary, vec![String::from("VoiceFlow")]);
        for index in 0..episode.operations.len() {
            let operation = operation_for(&episode, &template, index);
            let receipt = arm.execute_text_operation(&episode, &operation).unwrap();
            assert_eq!(
                receipt.arm_settings_readback,
                "not_required_local_only_scoped_pair"
            );
        }
        let state = arm.selected_state(&episode).unwrap();
        assert!(state.pairs[0].promoted);
        assert_eq!(
            state.settings_consistency,
            "selected_local_only_settings_unchanged"
        );
        assert_eq!(arm.settings.dictionary, vec![String::from("VoiceFlow")]);
        assert_eq!(
            read_private_bytes(&arm.root.join("settings.json")).unwrap(),
            settings_before
        );
    }

    #[test]
    fn historical_cobolt_fixture_is_rejected_by_the_local_only_owner() {
        let root = TestRoot::new();
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "fixtures/learning-v2-text-cobalt-invalid-local-only.json"
        ))
        .unwrap();
        assert_eq!(
            fixture["parity_status"],
            "invalid_for_production_local_only_classifier_parity"
        );
        let episode: LearningEpisodeV2 =
            serde_json::from_value(fixture["episode"].clone()).unwrap();
        let operation: LearningTextOperationV2 =
            serde_json::from_value(fixture["apply_text_template"]["operation"].clone()).unwrap();
        let mut arm = LearningArmV2::open(root.path(), &episode).unwrap();
        let receipt = arm.execute_text_operation(&episode, &operation).unwrap();
        let term = &episode.terms[0];
        let key = crate::dictionary_learn::pair_key(&term.before, &term.after);
        let row = store::get_learn_pair(&arm.root, &key).unwrap();
        let observation = store::LearnPairObservation::research_fixture_edit(
            &format!(
                "{}:{}:{}",
                episode.episode_id, operation.operation_id, episode.source.sha256
            ),
            &key,
        );

        assert_eq!(receipt.observer_outcome, "local_edit_guard_rejected");
        assert_eq!(receipt.candidate_class, None);
        assert_eq!(receipt.hits_after, None);
        assert_eq!(receipt.candidate_write, "not_attempted");
        assert!(row.is_none());
        assert!(!store::has_learn_pair_observation(&arm.root, &observation).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn v2_arm_rejects_symlinked_sqlite_files_and_sidecars() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = TestRoot::new();
        let arm_root = root.path().join("arm");
        create_private_directory(&arm_root, true).unwrap();
        let sentinel_root = root.path().join("sentinel");
        create_private_directory(&sentinel_root, true).unwrap();
        let sentinel = sentinel_root.join("external.sqlite");
        std::fs::write(&sentinel, b"sentinel").unwrap();
        std::fs::set_permissions(&sentinel, std::fs::Permissions::from_mode(0o600)).unwrap();

        for filename in [
            "history.sqlite",
            "history.sqlite-wal",
            "history.sqlite-shm",
            "history.sqlite-journal",
        ] {
            let path = arm_root.join(filename);
            symlink(&sentinel, &path).unwrap();
            assert!(validate_owned_arm_storage(&arm_root).is_err(), "{filename}");
            std::fs::remove_file(path).unwrap();
        }

        std::fs::hard_link(&sentinel, arm_root.join("history.sqlite")).unwrap();
        assert!(validate_owned_arm_storage(&arm_root).is_err());
    }

    #[test]
    fn v2_rejections_happen_before_learning_rows_or_receipts_are_written() {
        let root = TestRoot::new();
        let (episode, template) = fixture_episode();

        let mut no_grant = episode.clone();
        no_grant.learning_authorized = false;
        assert_eq!(
            no_grant.validate(),
            Err("learning_source_or_learning_grant_required")
        );
        assert!(!root.path().join("learning-arms-v2").exists());

        let mut wav = episode.clone();
        wav.source.kind = LearningSourceKindV2::OwnedPublicWav;
        wav.source.rights = LearningRightsV2::PublicCcBySa4_0;
        assert_eq!(wav.validate(), Err("unsupported_learning_source"));
        assert!(!root.path().join("learning-arms-v2").exists());

        let mut arm = LearningArmV2::open(root.path(), &episode).unwrap();
        let before = arm.selected_state(&episode).unwrap();
        let mut mismatched = template;
        mismatched.states[1].text = "we saw cobalx today".into();
        assert!(arm.execute_text_operation(&episode, &mismatched).is_err());
        let after = arm.selected_state(&episode).unwrap();
        assert_eq!(before.settings_sha256, after.settings_sha256);
        assert_eq!(after.pairs[0].state, "absent");

        let operation = operation_for(&episode, &mismatched, 0);
        let term = &episode.terms[0];
        let observation = store::LearnPairObservation::research_fixture_edit(
            &format!(
                "{}:{}:{}",
                episode.episode_id, operation.operation_id, episode.source.sha256
            ),
            &crate::dictionary_learn::pair_key(&term.before, &term.after),
        );
        assert!(!store::has_learn_pair_observation(&arm.root, &observation).unwrap());
    }
}

#[cfg(all(test, feature = "research-harness"))]
mod owned_paste_session_cli_contract_tests {
    use super::*;
    use std::ffi::OsString;

    fn private_temp_root() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "voiceflow-owned-paste-cli-contract-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        path
    }

    #[test]
    fn owned_paste_session_requires_explicit_supervisor_mode() {
        let root = private_temp_root();
        let manifest = std::env::temp_dir().join("prepared-owned-paste-session.json");

        let ordinary = parse_standalone_arguments([
            OsString::from("--arm-root"),
            root.clone().into_os_string(),
            OsString::from("--owned-paste-session-manifest"),
            manifest.clone().into_os_string(),
        ])
        .unwrap();
        assert!(!ordinary.owned_paste_session_supervisor);

        let missing_manifest = parse_standalone_arguments([
            OsString::from("--arm-root"),
            root.clone().into_os_string(),
            OsString::from("--owned-paste-session-supervisor"),
        ]);
        assert_eq!(
            missing_manifest.err(),
            Some("owned_paste_session_supervisor_requires_its_manifest")
        );

        let supervisor = parse_standalone_arguments([
            OsString::from("--arm-root"),
            root.clone().into_os_string(),
            OsString::from("--owned-paste-session-manifest"),
            manifest.into_os_string(),
            OsString::from("--owned-paste-session-supervisor"),
        ])
        .unwrap();
        assert!(supervisor.owned_paste_session_supervisor);

        let with_model_inputs = parse_standalone_arguments([
            OsString::from("--arm-root"),
            root.clone().into_os_string(),
            OsString::from("--owned-paste-session-manifest"),
            std::env::temp_dir()
                .join("prepared-owned-paste-session.json")
                .into_os_string(),
            OsString::from("--owned-paste-session-supervisor"),
            OsString::from("--wav-root"),
            root.clone().into_os_string(),
            OsString::from("--model-source-root"),
            root.clone().into_os_string(),
        ]);
        assert_eq!(
            with_model_inputs.err(),
            Some("owned_paste_session_supervisor_requires_its_manifest")
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn child_rejects_unregistered_binding_before_remote_paste_requests() {
        use crate::learning_clipboard_guard::{
            ClipboardGuardClientErrorV1, ClipboardGuardOperationV1, ClipboardGuardRpcClientV1,
            ClipboardGuardRpcOutcomeV1, RegisteredOwnedPasteHostIdentityV1,
            RegisteredOwnedPasteSessionAuthorityV1, RegisteredOwnedPasteSessionChildInputV1,
            RegisteredOwnedPasteSessionRemoteContextV1,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio_util::sync::CancellationToken;

        struct NoRequestClient(AtomicUsize);

        impl ClipboardGuardRpcClientV1 for NoRequestClient {
            fn request(
                &self,
                _lease_id: u64,
                _operation: ClipboardGuardOperationV1,
            ) -> Result<ClipboardGuardRpcOutcomeV1, ClipboardGuardClientErrorV1> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Err(ClipboardGuardClientErrorV1::Closed)
            }
        }

        let client = std::sync::Arc::new(NoRequestClient(AtomicUsize::new(0)));
        let authority = RegisteredOwnedPasteSessionAuthorityV1 {
            plan_id: "not-a-registered-plan".into(),
            phase_id: "90000000-0000-4000-8000-000000000032".into(),
            operation_id: "90000000-0000-4000-8000-000000000033".into(),
            supervisor_pid: std::process::id() as i32,
            helper_pid: 44,
            host_identity: RegisteredOwnedPasteHostIdentityV1 {
                source: "unregistered".into(),
                pid: 45,
                parent_pid: 44,
                user_id: 501,
                start_seconds: "1".into(),
                start_microseconds: "0".into(),
                executable_path: "/tmp/unregistered-host".into(),
            },
            binding: learning_phase_v3::OwnedPasteSessionBindingV1 {
                fixture_id: String::new(),
                source_manifest_sha256: String::new(),
                helper_executable_sha256: String::new(),
                host_executable_sha256: String::new(),
                host_bundle_identifier: String::new(),
                host_info_plist_sha256: String::new(),
                recipe_sha256: String::new(),
                window_title: String::new(),
                control_identifier: String::new(),
                inserted_text_sha256: String::new(),
                inserted_text_byte_length: 0,
                expected_post_insert_sha256: String::new(),
                expected_post_insert_byte_length: 0,
                expected_settled_sha256: String::new(),
                expected_settled_byte_length: 0,
            },
            expires_at_monotonic_ns: crate::learning_clipboard_guard::monotonic_deadline_ns(
                std::time::Instant::now() + std::time::Duration::from_secs(1),
            )
            .unwrap(),
        };
        let input = RegisteredOwnedPasteSessionChildInputV1 {
            remote: RegisteredOwnedPasteSessionRemoteContextV1 {
                client: std::sync::Arc::clone(&client)
                    as std::sync::Arc<dyn ClipboardGuardRpcClientV1>,
                lease_id: 1,
                plan_id: authority.plan_id.clone(),
                phase_id: authority.phase_id.clone(),
                operation_id: authority.operation_id.clone(),
            },
            authority,
            host_identity: crate::paste::RegisteredResearchHostIdentityV1 {
                pid: 45,
                parent_pid: 44,
                user_id: 501,
                start_seconds: 1,
                start_microseconds: 0,
                executable_path: PathBuf::from("/tmp/unregistered-host"),
            },
            expires_at: std::time::Instant::now() + std::time::Duration::from_secs(1),
            cancellation: CancellationToken::new(),
        };

        assert_eq!(
            execute_registered_owned_paste_session_child(input),
            crate::learning_clipboard_guard::RegisteredOwnedPasteChildResultKindV1::ProtocolRejected
        );
        assert_eq!(client.0.load(Ordering::Relaxed), 0);
    }
}

#[cfg(all(test, feature = "research-harness", target_os = "macos"))]
mod owned_paste_session_owner_terminal_tests {
    use super::RegisteredOwnedPasteOwnerTerminalV1;

    #[test]
    fn failed_owner_terminal_is_joined_but_does_not_complete_the_child() {
        assert!(RegisteredOwnedPasteOwnerTerminalV1::Closed.joined());
        assert!(RegisteredOwnedPasteOwnerTerminalV1::Closed.closed_cleanly());
        assert!(RegisteredOwnedPasteOwnerTerminalV1::Failed.joined());
        assert!(!RegisteredOwnedPasteOwnerTerminalV1::Failed.closed_cleanly());
        assert!(!RegisteredOwnedPasteOwnerTerminalV1::NotStarted.joined());
        assert!(!RegisteredOwnedPasteOwnerTerminalV1::NotStarted.closed_cleanly());
    }
}
