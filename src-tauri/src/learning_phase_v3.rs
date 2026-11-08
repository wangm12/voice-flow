#![cfg(any(test, feature = "research-harness"))]

//! Strict phase-plan admission facts for the research-only V3 learning path.
//! This module does not define rule identity or production mutation behavior.
//! The dispatcher must still verify actual UTF-8 operation bytes against the
//! declared SHA-256 and byte lengths before invoking any owner.

use super::{
    LearningActorV2, LearningEditStateV2, LearningOracleV2, LearningRightsV2, LearningScopeV2,
    LearningSourceKindV2, LearningSourceV2, LearningTextLayerV2, TermFixture,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub(crate) const LEARNING_PHASE_V3_SCHEMA_VERSION: u8 = 3;
const PHASES_MIN: usize = 2;
const PHASES_MAX: usize = 5;
const TERMS_MAX: usize = 8;
const OPERATIONS_PER_LEARN_PHASE_MAX: usize = 8;
const EDIT_STATES_MIN: usize = 2;
const EDIT_STATES_MAX: usize = 6;
const TEXT_MAX_BYTES: usize = 1_024;
const OUTPUT_MAX_BYTES: usize = 1_024;
const PHASE_TTL_MIN_MS: u64 = 1_000;
const PHASE_TTL_MAX_MS: u64 = 1_200_000;
const OBSERVE_PHASES_MIN: usize = 1;
const OBSERVE_PHASE_TTL_MIN_MS: u64 = 7_000;
const OBSERVE_SCOPE_MAPPING_ID: &str = "research-owned-native-oracle-host-v1";
const OBSERVE_SCOPE_BUNDLE_ID: &str = "local.voiceflow.learning-oracle-fixture.v1";
const OBSERVE_FIXTURE_ID: &str = "90000000-0000-4000-8000-000000000002";
const OBSERVE_WINDOW_TITLE: &str =
    "Voice harness owned fixture 90000000-0000-4000-8000-000000000002";
const OBSERVE_CONTROL_ID: &str = "vf-fixture-90000000-0000-4000-8000-000000000002-target";
const OBSERVE_EXPECTED_SHA256: &str =
    "e7b17776dd843cfaa300566ee48aaa3a260364bb8edaebe7401ae749c1e9bfe1";
const OBSERVE_EXPECTED_BYTE_LENGTH: usize = 29;
const OBSERVE_RECIPE_SHA256: &str =
    "1d2f3f722c0232bc78e5896fe80c4a328c800e6813e34ead983167ab0a27ce15";
const OBSERVE_INFO_PLIST_SHA256: &str =
    "8b7bee050e73472f890e0767f9e036c023d712fb9239ff7d41ca6b18613b74d5";
const OBSERVE_SOURCE_MANIFEST_SHA256: &str =
    "4e143f250d877feebe36098020df5545cf200d6273e95a84eb4af16656d6668f";
const PASTE_SESSION_SCOPE_MAPPING_ID: &str = "research-owned-native-paste-session-v1";
const PASTE_SESSION_SCOPE_BUNDLE_ID: &str = "local.voiceflow.learning-oracle-fixture.v1";
const PASTE_SESSION_FIXTURE_ID: &str = "90000000-0000-4000-8000-000000000013";
const PASTE_SESSION_WINDOW_TITLE: &str =
    "Voice harness owned fixture 90000000-0000-4000-8000-000000000013";
const PASTE_SESSION_CONTROL_ID: &str = "vf-fixture-90000000-0000-4000-8000-000000000013-target";
const PASTE_SESSION_RECIPE_SHA256: &str =
    "498a5af64c3c7626239484a3ae899da7c6f4f58a7198e7c72ac2716a8009f9cf";
const PASTE_SESSION_INFO_PLIST_SHA256: &str =
    "8b7bee050e73472f890e0767f9e036c023d712fb9239ff7d41ca6b18613b74d5";
const PASTE_SESSION_SOURCE_MANIFEST_SHA256: &str =
    "4e143f250d877feebe36098020df5545cf200d6273e95a84eb4af16656d6668f";
const PASTE_SESSION_INSERTED_SHA256: &str =
    "c72a301aa81e686da1dae98534e6b3333222cd305e3a1fef714491697afcce95";
const PASTE_SESSION_INSERTED_BYTE_LENGTH: usize = 8;
const PASTE_SESSION_POST_INSERT_SHA256: &str =
    "56ceef9d7bb12e9bc5a5bbe8b661a60432ad5ac741cd84c64e1bb021e1bbbcc9";
const PASTE_SESSION_POST_INSERT_BYTE_LENGTH: usize = 14;
const PASTE_SESSION_SETTLED_SHA256: &str =
    "901c32b29acb890cc90420da89d82ea6361f718dc45fc0b8afb1000169deb304";
const PASTE_SESSION_SETTLED_BYTE_LENGTH: usize = 15;
const PASTE_SESSION_TTL_MIN_MS: u64 = 10_000;
const PASTE_SESSION_TTL_MAX_MS: u64 = 30_000;

fn is_false(value: &bool) -> bool {
    !value
}

/// C1 rule identity, canonical casing, and pair-key conflict behavior remain
/// unresolved; V3 admission preserves that gap instead of projecting it.
pub(crate) const LEARNING_PHASE_V3_KNOWN_GAPS: &[&str] =
    &["c1_rule_identity_casing_and_pair_key_conflicts_unresolved"];

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LearningPhasePlanV3 {
    pub schema_version: u8,
    pub plan_id: String,
    pub arm_state_id: String,
    pub terms: Vec<TermFixture>,
    pub phases: Vec<LearningPhaseV3>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LearningPhaseV3 {
    pub phase_id: String,
    pub ttl_ms: u64,
    /// The current action scope; an ApplyFollowup's application scope lives here.
    pub scope: LearningScopeV2,
    pub local_only_grant: bool,
    pub actor: LearningActorV2,
    pub target_oracle: LearningOracleV2,
    pub source: Option<LearningSourceV2>,
    /// Allows this phase's declared source text to be admitted as learning input.
    pub source_text_learning_authorized: bool,
    /// Grants candidate learning only for a LearnText action.
    pub learning_authorized: bool,
    /// Grants local output application only for an ApplyFollowup action.
    pub local_replacement_authorized: bool,
    /// Independent undo grant; it cannot be inferred from another permission.
    pub undo_authorized: bool,
    pub edit_states: Vec<LearningEditStateV2>,
    pub action: LearningPhaseActionV3,
    /// Separate permission for one registered owned-host AX sample.
    #[serde(default, skip_serializing_if = "is_false")]
    pub target_observation_authorized: bool,
    /// Present only for ObserveTarget phases; absent fields preserve old V3 identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_binding: Option<LearningTargetBindingV1>,
    /// Grants one registered original-paste-owner session, independently of learning.
    #[serde(default, skip_serializing_if = "is_false")]
    pub owned_paste_session_authorized: bool,
    /// Present only for the single-phase owned-paste-session action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owned_paste_session_binding: Option<OwnedPasteSessionBindingV1>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct LearningTargetBindingV1 {
    pub fixture_id: String,
    pub source_manifest_sha256: String,
    pub helper_executable_sha256: String,
    pub host_executable_sha256: String,
    pub host_bundle_identifier: String,
    pub host_info_plist_sha256: String,
    pub recipe_sha256: String,
    pub window_title: String,
    pub control_identifier: String,
    pub expected_value_sha256: String,
    pub expected_value_byte_length: usize,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct OwnedPasteSessionBindingV1 {
    pub fixture_id: String,
    pub source_manifest_sha256: String,
    pub helper_executable_sha256: String,
    pub host_executable_sha256: String,
    pub host_bundle_identifier: String,
    pub host_info_plist_sha256: String,
    pub recipe_sha256: String,
    pub window_title: String,
    pub control_identifier: String,
    pub inserted_text_sha256: String,
    pub inserted_text_byte_length: usize,
    pub expected_post_insert_sha256: String,
    pub expected_post_insert_byte_length: usize,
    pub expected_settled_sha256: String,
    pub expected_settled_byte_length: usize,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum LearningPhaseActionV3 {
    LearnText {
        operation_ids: Vec<String>,
        term_fixture_id: String,
    },
    ApplyFollowup {
        task_id: String,
        operation_id: String,
        /// Earlier LearnText phase that owns the rule; it may have another scope.
        rule_owner_phase_id: String,
        term_fixture_id: String,
        expected_output_sha256: String,
        expected_output_byte_length: usize,
    },
    Undo {
        operation_id: String,
        rule_owner_phase_id: String,
        term_fixture_id: String,
        /// A prior planned application; runtime must also require its success.
        required_application_phase_id: String,
    },
    ObserveTarget {
        operation_id: String,
    },
    RunOwnedPasteSession {
        operation_id: String,
    },
}

impl LearningPhasePlanV3 {
    /// Validates the declared plan metadata and phase-to-phase references.
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != LEARNING_PHASE_V3_SCHEMA_VERSION {
            return Err("learning_phase_v3_version_unsupported");
        }
        let has_observe_target = self
            .phases
            .iter()
            .any(|phase| matches!(&phase.action, LearningPhaseActionV3::ObserveTarget { .. }));
        let has_owned_paste_session = self.phases.iter().any(|phase| {
            matches!(
                &phase.action,
                LearningPhaseActionV3::RunOwnedPasteSession { .. }
            )
        });
        let observe_target_only = !self.phases.is_empty()
            && self
                .phases
                .iter()
                .all(|phase| matches!(&phase.action, LearningPhaseActionV3::ObserveTarget { .. }));
        let owned_paste_session_only = self.phases.len() == 1
            && self.phases.iter().all(|phase| {
                matches!(
                    &phase.action,
                    LearningPhaseActionV3::RunOwnedPasteSession { .. }
                )
            });
        if has_observe_target && !observe_target_only {
            return Err("learning_phase_v3_mixed_target_observation_unsupported");
        }
        if has_owned_paste_session && !owned_paste_session_only {
            return Err("learning_phase_v3_mixed_owned_paste_session_unsupported");
        }
        let valid_phase_count = if owned_paste_session_only {
            true
        } else if observe_target_only {
            (OBSERVE_PHASES_MIN..=PHASES_MAX).contains(&self.phases.len())
        } else {
            (PHASES_MIN..=PHASES_MAX).contains(&self.phases.len())
        };
        if !valid_phase_count {
            return Err("learning_phase_v3_phase_count_invalid");
        }
        if self.terms.is_empty() || self.terms.len() > TERMS_MAX {
            return Err("learning_phase_v3_term_count_invalid");
        }

        let mut entity_ids = HashSet::new();
        register_uuid(&self.plan_id, &mut entity_ids)?;
        register_uuid(&self.arm_state_id, &mut entity_ids)?;

        let mut term_ids = HashSet::<String>::new();
        let mut exact_term_pairs = HashSet::new();
        for term in &self.terms {
            register_uuid(&term.fixture_id, &mut entity_ids)?;
            term_ids.insert(term.fixture_id.clone());
            if term.before.is_empty()
                || term.after.is_empty()
                || term.before == term.after
                || term.before.len() > 256
                || term.after.len() > 256
                || term.before.chars().any(char::is_control)
                || term.after.chars().any(char::is_control)
                || !exact_term_pairs.insert((term.before.as_str(), term.after.as_str()))
            {
                return Err("learning_phase_v3_term_invalid_or_duplicate");
            }
        }

        let mut has_two_distinct_native_scopes = false;
        let mut registered_target_binding: Option<&LearningTargetBindingV1> = None;
        for (index, phase) in self.phases.iter().enumerate() {
            register_uuid(&phase.phase_id, &mut entity_ids)?;
            if !(PHASE_TTL_MIN_MS..=PHASE_TTL_MAX_MS).contains(&phase.ttl_ms) {
                return Err("learning_phase_v3_ttl_invalid");
            }
            validate_scope(&phase.scope)?;
            if phase.actor != LearningActorV2::ProgrammaticFixture {
                return Err("learning_phase_v3_actor_unsupported");
            }

            if owned_paste_session_only {
                if !(PASTE_SESSION_TTL_MIN_MS..=PASTE_SESSION_TTL_MAX_MS).contains(&phase.ttl_ms) {
                    return Err("learning_phase_v3_owned_paste_session_ttl_invalid");
                }
                if phase.scope != registered_paste_session_scope() {
                    return Err("learning_phase_v3_owned_paste_session_scope_mismatch");
                }
                if phase.target_oracle != LearningOracleV2::IndependentTarget
                    || !phase.owned_paste_session_authorized
                    || phase.target_observation_authorized
                    || phase.local_only_grant
                    || phase.source_text_learning_authorized
                    || phase.learning_authorized
                    || phase.local_replacement_authorized
                    || phase.undo_authorized
                    || phase.source.is_some()
                    || !phase.edit_states.is_empty()
                    || phase.target_binding.is_some()
                {
                    return Err("learning_phase_v3_owned_paste_session_grant_mismatch");
                }
                let binding = phase
                    .owned_paste_session_binding
                    .as_ref()
                    .ok_or("learning_phase_v3_owned_paste_session_binding_required")?;
                validate_owned_paste_session_binding(binding)?;
                if let LearningPhaseActionV3::RunOwnedPasteSession { operation_id } = &phase.action
                {
                    register_uuid(operation_id, &mut entity_ids)?;
                }
                continue;
            }

            if observe_target_only {
                if phase.ttl_ms < OBSERVE_PHASE_TTL_MIN_MS {
                    return Err("learning_phase_v3_observe_target_ttl_too_short");
                }
                if phase.scope != registered_observe_scope() {
                    return Err("learning_phase_v3_observe_target_scope_mismatch");
                }
                if phase.target_oracle != LearningOracleV2::IndependentTarget
                    || !phase.target_observation_authorized
                    || phase.local_only_grant
                    || phase.source_text_learning_authorized
                    || phase.learning_authorized
                    || phase.local_replacement_authorized
                    || phase.undo_authorized
                    || phase.owned_paste_session_authorized
                    || phase.owned_paste_session_binding.is_some()
                    || phase.source.is_some()
                    || !phase.edit_states.is_empty()
                {
                    return Err("learning_phase_v3_observe_target_grant_mismatch");
                }
                let binding = phase
                    .target_binding
                    .as_ref()
                    .ok_or("learning_phase_v3_observe_target_binding_required")?;
                validate_target_binding(binding)?;
                if let Some(previous) = registered_target_binding {
                    if previous != binding {
                        return Err("learning_phase_v3_observe_target_binding_mismatch");
                    }
                } else {
                    registered_target_binding = Some(binding);
                }
                if let LearningPhaseActionV3::ObserveTarget { operation_id } = &phase.action {
                    register_uuid(operation_id, &mut entity_ids)?;
                }
                continue;
            }

            has_two_distinct_native_scopes |= self.phases[index + 1..].iter().any(|other| {
                phase.scope.family == other.scope.family
                    && phase.scope.mapping_id != other.scope.mapping_id
                    && phase.scope.native_bundle != other.scope.native_bundle
            });
            if phase.target_oracle != LearningOracleV2::None {
                return Err("learning_phase_v3_target_oracle_unsupported");
            }
            if phase.target_observation_authorized || phase.target_binding.is_some() {
                return Err("learning_phase_v3_observe_target_grant_mismatch");
            }
            if phase.owned_paste_session_authorized || phase.owned_paste_session_binding.is_some() {
                return Err("learning_phase_v3_owned_paste_session_grant_mismatch");
            }
            if !phase.local_only_grant {
                return Err("learning_phase_v3_local_only_grant_required");
            }

            match &phase.action {
                LearningPhaseActionV3::LearnText {
                    operation_ids,
                    term_fixture_id,
                } => {
                    if !term_ids.contains(term_fixture_id.as_str()) {
                        return Err("learning_phase_v3_term_not_declared");
                    }
                    if !(1..=OPERATIONS_PER_LEARN_PHASE_MAX).contains(&operation_ids.len()) {
                        return Err("learning_phase_v3_operation_count_invalid");
                    }
                    for operation_id in operation_ids {
                        register_uuid(operation_id, &mut entity_ids)?;
                    }
                    if !phase.source_text_learning_authorized
                        || !phase.learning_authorized
                        || phase.local_replacement_authorized
                        || phase.undo_authorized
                    {
                        return Err("learning_phase_v3_learn_grant_mismatch");
                    }
                    let source = phase
                        .source
                        .as_ref()
                        .ok_or("learning_phase_v3_learn_source_required")?;
                    validate_source(source, &mut entity_ids)?;
                    if phase.edit_states.len() < EDIT_STATES_MIN
                        || phase.edit_states.len() > EDIT_STATES_MAX
                    {
                        return Err("learning_phase_v3_edit_state_count_invalid");
                    }
                    validate_edit_states(&phase.edit_states, source, &mut entity_ids)?;
                }
                LearningPhaseActionV3::ApplyFollowup {
                    task_id,
                    operation_id,
                    term_fixture_id,
                    expected_output_sha256,
                    expected_output_byte_length,
                    ..
                } => {
                    register_uuid(task_id, &mut entity_ids)?;
                    register_uuid(operation_id, &mut entity_ids)?;
                    if !term_ids.contains(term_fixture_id.as_str()) {
                        return Err("learning_phase_v3_term_not_declared");
                    }
                    if phase.source_text_learning_authorized
                        || phase.learning_authorized
                        || !phase.local_replacement_authorized
                        || phase.undo_authorized
                        || !phase.edit_states.is_empty()
                    {
                        return Err("learning_phase_v3_followup_grant_mismatch");
                    }
                    let source = phase
                        .source
                        .as_ref()
                        .ok_or("learning_phase_v3_followup_source_required")?;
                    validate_source(source, &mut entity_ids)?;
                    if *expected_output_byte_length == 0
                        || *expected_output_byte_length > OUTPUT_MAX_BYTES
                        || !valid_sha256(expected_output_sha256)
                    {
                        return Err("learning_phase_v3_followup_output_invalid");
                    }
                }
                LearningPhaseActionV3::Undo {
                    operation_id,
                    term_fixture_id,
                    ..
                } => {
                    register_uuid(operation_id, &mut entity_ids)?;
                    if !term_ids.contains(term_fixture_id.as_str()) {
                        return Err("learning_phase_v3_term_not_declared");
                    }
                    if phase.source.is_some()
                        || phase.source_text_learning_authorized
                        || phase.learning_authorized
                        || phase.local_replacement_authorized
                        || !phase.undo_authorized
                        || !phase.edit_states.is_empty()
                    {
                        return Err("learning_phase_v3_undo_grant_mismatch");
                    }
                }
                LearningPhaseActionV3::ObserveTarget { .. } => {
                    return Err("learning_phase_v3_mixed_target_observation_unsupported");
                }
                LearningPhaseActionV3::RunOwnedPasteSession { .. } => {
                    return Err("learning_phase_v3_mixed_owned_paste_session_unsupported");
                }
            }
        }
        if !observe_target_only && !owned_paste_session_only && !has_two_distinct_native_scopes {
            return Err("learning_phase_v3_two_native_scopes_required");
        }

        if observe_target_only || owned_paste_session_only {
            return Ok(());
        }

        for (index, phase) in self.phases.iter().enumerate() {
            match &phase.action {
                LearningPhaseActionV3::LearnText { .. } => {}
                LearningPhaseActionV3::ObserveTarget { .. } => {
                    return Err("learning_phase_v3_mixed_target_observation_unsupported");
                }
                LearningPhaseActionV3::RunOwnedPasteSession { .. } => {
                    return Err("learning_phase_v3_mixed_owned_paste_session_unsupported");
                }
                LearningPhaseActionV3::ApplyFollowup {
                    rule_owner_phase_id,
                    term_fixture_id,
                    ..
                } => {
                    validate_uuid_reference(rule_owner_phase_id)?;
                    let owner = prior_phase(&self.phases, index, rule_owner_phase_id)
                        .ok_or("learning_phase_v3_rule_owner_not_prior")?;
                    if !learns_term(owner, term_fixture_id) {
                        return Err("learning_phase_v3_rule_owner_term_mismatch");
                    }
                }
                LearningPhaseActionV3::Undo {
                    rule_owner_phase_id,
                    term_fixture_id,
                    required_application_phase_id,
                    ..
                } => {
                    validate_uuid_reference(rule_owner_phase_id)?;
                    validate_uuid_reference(required_application_phase_id)?;
                    let owner = prior_phase(&self.phases, index, rule_owner_phase_id)
                        .ok_or("learning_phase_v3_rule_owner_not_prior")?;
                    if !learns_term(owner, term_fixture_id) {
                        return Err("learning_phase_v3_rule_owner_term_mismatch");
                    }
                    if phase.scope != owner.scope {
                        return Err("learning_phase_v3_undo_scope_mismatch");
                    }
                    let application =
                        prior_phase(&self.phases, index, required_application_phase_id)
                            .ok_or("learning_phase_v3_required_application_not_prior")?;
                    if application.scope != phase.scope
                        || !matches!(
                            &application.action,
                            LearningPhaseActionV3::ApplyFollowup {
                                rule_owner_phase_id: application_owner,
                                term_fixture_id: application_term,
                                ..
                            } if application_owner == rule_owner_phase_id
                                && application_term == term_fixture_id
                        )
                    {
                        return Err("learning_phase_v3_required_application_mismatch");
                    }
                    if self.phases[..index].iter().any(|prior| {
                        matches!(
                            &prior.action,
                            LearningPhaseActionV3::Undo {
                                rule_owner_phase_id: undone_owner,
                                term_fixture_id: undone_term,
                                ..
                            } if undone_owner == rule_owner_phase_id
                                && undone_term == term_fixture_id
                        )
                    }) {
                        return Err("learning_phase_v3_rule_already_undone");
                    }
                }
            }
        }

        Ok(())
    }

    pub(crate) fn identity_sha256(&self) -> anyhow::Result<String> {
        self.validate().map_err(anyhow::Error::msg)?;
        let identity = serde_json::to_vec(self)?;
        Ok(hex::encode(Sha256::digest(identity)))
    }

    pub(crate) fn phase(&self, phase_id: &str) -> Option<&LearningPhaseV3> {
        self.phases.iter().find(|phase| phase.phase_id == phase_id)
    }

    pub(crate) fn term(&self, fixture_id: &str) -> Option<&TermFixture> {
        self.terms.iter().find(|term| term.fixture_id == fixture_id)
    }
}

fn registered_observe_scope() -> LearningScopeV2 {
    LearningScopeV2 {
        mapping_id: OBSERVE_SCOPE_MAPPING_ID.to_owned(),
        family: crate::context::ContextFamily::General,
        native_bundle: OBSERVE_SCOPE_BUNDLE_ID.to_owned(),
    }
}

fn registered_paste_session_scope() -> LearningScopeV2 {
    LearningScopeV2 {
        mapping_id: PASTE_SESSION_SCOPE_MAPPING_ID.to_owned(),
        family: crate::context::ContextFamily::General,
        native_bundle: PASTE_SESSION_SCOPE_BUNDLE_ID.to_owned(),
    }
}

fn validate_target_binding(binding: &LearningTargetBindingV1) -> Result<(), &'static str> {
    if binding.fixture_id != OBSERVE_FIXTURE_ID
        || binding.source_manifest_sha256 != OBSERVE_SOURCE_MANIFEST_SHA256
        || !valid_sha256(&binding.helper_executable_sha256)
        || !valid_sha256(&binding.host_executable_sha256)
        || binding.host_bundle_identifier != OBSERVE_SCOPE_BUNDLE_ID
        || binding.host_info_plist_sha256 != OBSERVE_INFO_PLIST_SHA256
        || binding.recipe_sha256 != OBSERVE_RECIPE_SHA256
        || binding.window_title != OBSERVE_WINDOW_TITLE
        || binding.control_identifier != OBSERVE_CONTROL_ID
        || binding.expected_value_sha256 != OBSERVE_EXPECTED_SHA256
        || binding.expected_value_byte_length != OBSERVE_EXPECTED_BYTE_LENGTH
    {
        return Err("learning_phase_v3_observe_target_binding_invalid");
    }
    Ok(())
}

fn validate_owned_paste_session_binding(
    binding: &OwnedPasteSessionBindingV1,
) -> Result<(), &'static str> {
    if binding.fixture_id != PASTE_SESSION_FIXTURE_ID
        || binding.source_manifest_sha256 != PASTE_SESSION_SOURCE_MANIFEST_SHA256
        || !valid_sha256(&binding.helper_executable_sha256)
        || !valid_sha256(&binding.host_executable_sha256)
        || binding.host_bundle_identifier != PASTE_SESSION_SCOPE_BUNDLE_ID
        || binding.host_info_plist_sha256 != PASTE_SESSION_INFO_PLIST_SHA256
        || binding.recipe_sha256 != PASTE_SESSION_RECIPE_SHA256
        || binding.window_title != PASTE_SESSION_WINDOW_TITLE
        || binding.control_identifier != PASTE_SESSION_CONTROL_ID
        || binding.inserted_text_sha256 != PASTE_SESSION_INSERTED_SHA256
        || binding.inserted_text_byte_length != PASTE_SESSION_INSERTED_BYTE_LENGTH
        || binding.expected_post_insert_sha256 != PASTE_SESSION_POST_INSERT_SHA256
        || binding.expected_post_insert_byte_length != PASTE_SESSION_POST_INSERT_BYTE_LENGTH
        || binding.expected_settled_sha256 != PASTE_SESSION_SETTLED_SHA256
        || binding.expected_settled_byte_length != PASTE_SESSION_SETTLED_BYTE_LENGTH
    {
        return Err("learning_phase_v3_owned_paste_session_binding_invalid");
    }
    Ok(())
}

fn register_uuid(id: &str, entity_ids: &mut HashSet<String>) -> Result<(), &'static str> {
    validate_uuid_reference(id)?;
    if !entity_ids.insert(id.to_owned()) {
        return Err("learning_phase_v3_entity_id_reused");
    }
    Ok(())
}

fn validate_uuid_reference(id: &str) -> Result<(), &'static str> {
    crate::onboarding::validate_id(id).map_err(|_| "learning_phase_v3_uuid_invalid")
}

fn validate_scope(scope: &LearningScopeV2) -> Result<(), &'static str> {
    if scope.mapping_id.is_empty()
        || scope.mapping_id.len() > 128
        || scope.mapping_id.chars().any(char::is_control)
        || scope.native_bundle.is_empty()
        || scope.native_bundle.len() > 256
        || scope.native_bundle.chars().any(char::is_control)
        || scope.family == crate::context::ContextFamily::BrowserSearch
        || crate::context::is_browser_application(Some(&scope.native_bundle))
    {
        return Err("learning_phase_v3_scope_invalid_or_browser");
    }
    Ok(())
}

fn validate_source(
    source: &LearningSourceV2,
    entity_ids: &mut HashSet<String>,
) -> Result<(), &'static str> {
    register_uuid(&source.fixture_id, entity_ids)?;
    if source.kind != LearningSourceKindV2::DeterministicText
        || source.rights == LearningRightsV2::PublicCcBySa4_0
        || source.byte_length == 0
        || source.byte_length > TEXT_MAX_BYTES
        || !valid_sha256(&source.sha256)
    {
        return Err("learning_phase_v3_source_invalid");
    }
    Ok(())
}

fn validate_edit_states(
    states: &[LearningEditStateV2],
    source: &LearningSourceV2,
    entity_ids: &mut HashSet<String>,
) -> Result<(), &'static str> {
    let Some(post_insert) = states.first() else {
        return Err("learning_phase_v3_edit_state_count_invalid");
    };
    for (sequence, state) in states.iter().enumerate() {
        register_uuid(&state.state_id, entity_ids)?;
        let expected_layer = if sequence == 0 {
            LearningTextLayerV2::PostInsert
        } else {
            LearningTextLayerV2::Settled
        };
        if state.sequence as usize != sequence
            || state.layer != expected_layer
            || state.byte_length == 0
            || state.byte_length > TEXT_MAX_BYTES
            || !valid_sha256(&state.sha256)
        {
            return Err("learning_phase_v3_edit_state_invalid");
        }
    }
    if post_insert.sha256 != source.sha256 || post_insert.byte_length != source.byte_length {
        return Err("learning_phase_v3_source_state_mismatch");
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn prior_phase<'a>(
    phases: &'a [LearningPhaseV3],
    current_index: usize,
    phase_id: &str,
) -> Option<&'a LearningPhaseV3> {
    phases[..current_index]
        .iter()
        .find(|phase| phase.phase_id == phase_id)
}

fn learns_term(phase: &LearningPhaseV3, term_fixture_id: &str) -> bool {
    matches!(
        &phase.action,
        LearningPhaseActionV3::LearnText {
            term_fixture_id: learned_term,
            ..
        } if learned_term == term_fixture_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: u32) -> String {
        format!("70000000-0000-4000-8000-{value:012x}")
    }

    fn sha256(text: &str) -> String {
        hex::encode(Sha256::digest(text.as_bytes()))
    }

    fn text_source(fixture_id: u32, text: &str) -> LearningSourceV2 {
        LearningSourceV2 {
            kind: LearningSourceKindV2::DeterministicText,
            fixture_id: id(fixture_id),
            sha256: sha256(text),
            byte_length: text.len(),
            rights: LearningRightsV2::Synthetic,
        }
    }

    fn edit_state(
        sequence: u8,
        state_id: u32,
        layer: LearningTextLayerV2,
        text: &str,
    ) -> LearningEditStateV2 {
        LearningEditStateV2 {
            sequence,
            state_id: id(state_id),
            layer,
            sha256: sha256(text),
            byte_length: text.len(),
        }
    }

    fn scope(mapping_id: &str, native_bundle: &str) -> LearningScopeV2 {
        LearningScopeV2 {
            mapping_id: mapping_id.into(),
            family: crate::context::ContextFamily::General,
            native_bundle: native_bundle.into(),
        }
    }

    fn phase(
        phase_number: u32,
        scope: LearningScopeV2,
        source: Option<LearningSourceV2>,
        edit_states: Vec<LearningEditStateV2>,
        action: LearningPhaseActionV3,
        grants: (bool, bool, bool, bool),
    ) -> LearningPhaseV3 {
        LearningPhaseV3 {
            phase_id: id(phase_number),
            ttl_ms: PHASE_TTL_MIN_MS,
            scope,
            local_only_grant: true,
            actor: LearningActorV2::ProgrammaticFixture,
            target_oracle: LearningOracleV2::None,
            source,
            source_text_learning_authorized: grants.0,
            learning_authorized: grants.1,
            local_replacement_authorized: grants.2,
            undo_authorized: grants.3,
            edit_states,
            action,
            target_observation_authorized: false,
            target_binding: None,
            owned_paste_session_authorized: false,
            owned_paste_session_binding: None,
        }
    }

    fn target_binding() -> LearningTargetBindingV1 {
        LearningTargetBindingV1 {
            fixture_id: OBSERVE_FIXTURE_ID.into(),
            source_manifest_sha256: OBSERVE_SOURCE_MANIFEST_SHA256.into(),
            helper_executable_sha256: sha256("prepared native oracle helper"),
            host_executable_sha256: sha256("prepared FixtureHost executable"),
            host_bundle_identifier: OBSERVE_SCOPE_BUNDLE_ID.into(),
            host_info_plist_sha256: OBSERVE_INFO_PLIST_SHA256.into(),
            recipe_sha256: OBSERVE_RECIPE_SHA256.into(),
            window_title: OBSERVE_WINDOW_TITLE.into(),
            control_identifier: OBSERVE_CONTROL_ID.into(),
            expected_value_sha256: OBSERVE_EXPECTED_SHA256.into(),
            expected_value_byte_length: OBSERVE_EXPECTED_BYTE_LENGTH,
        }
    }

    fn observe_phase(phase_number: u32, operation_number: u32) -> LearningPhaseV3 {
        LearningPhaseV3 {
            phase_id: id(phase_number),
            ttl_ms: OBSERVE_PHASE_TTL_MIN_MS,
            scope: registered_observe_scope(),
            local_only_grant: false,
            actor: LearningActorV2::ProgrammaticFixture,
            target_oracle: LearningOracleV2::IndependentTarget,
            source: None,
            source_text_learning_authorized: false,
            learning_authorized: false,
            local_replacement_authorized: false,
            undo_authorized: false,
            edit_states: Vec::new(),
            action: LearningPhaseActionV3::ObserveTarget {
                operation_id: id(operation_number),
            },
            target_observation_authorized: true,
            target_binding: Some(target_binding()),
            owned_paste_session_authorized: false,
            owned_paste_session_binding: None,
        }
    }

    fn owned_paste_session_binding() -> OwnedPasteSessionBindingV1 {
        OwnedPasteSessionBindingV1 {
            fixture_id: PASTE_SESSION_FIXTURE_ID.into(),
            source_manifest_sha256: PASTE_SESSION_SOURCE_MANIFEST_SHA256.into(),
            helper_executable_sha256: sha256("prepared owned paste helper"),
            host_executable_sha256: sha256("prepared owned paste Host"),
            host_bundle_identifier: PASTE_SESSION_SCOPE_BUNDLE_ID.into(),
            host_info_plist_sha256: PASTE_SESSION_INFO_PLIST_SHA256.into(),
            recipe_sha256: PASTE_SESSION_RECIPE_SHA256.into(),
            window_title: PASTE_SESSION_WINDOW_TITLE.into(),
            control_identifier: PASTE_SESSION_CONTROL_ID.into(),
            inserted_text_sha256: PASTE_SESSION_INSERTED_SHA256.into(),
            inserted_text_byte_length: PASTE_SESSION_INSERTED_BYTE_LENGTH,
            expected_post_insert_sha256: PASTE_SESSION_POST_INSERT_SHA256.into(),
            expected_post_insert_byte_length: PASTE_SESSION_POST_INSERT_BYTE_LENGTH,
            expected_settled_sha256: PASTE_SESSION_SETTLED_SHA256.into(),
            expected_settled_byte_length: PASTE_SESSION_SETTLED_BYTE_LENGTH,
        }
    }

    fn owned_paste_session_plan() -> LearningPhasePlanV3 {
        LearningPhasePlanV3 {
            schema_version: LEARNING_PHASE_V3_SCHEMA_VERSION,
            plan_id: id(61),
            arm_state_id: id(62),
            terms: vec![TermFixture {
                fixture_id: id(64),
                before: "unused term metadata".into(),
                after: "also unused".into(),
            }],
            phases: vec![LearningPhaseV3 {
                phase_id: id(60),
                ttl_ms: PASTE_SESSION_TTL_MIN_MS,
                scope: registered_paste_session_scope(),
                local_only_grant: false,
                actor: LearningActorV2::ProgrammaticFixture,
                target_oracle: LearningOracleV2::IndependentTarget,
                source: None,
                source_text_learning_authorized: false,
                learning_authorized: false,
                local_replacement_authorized: false,
                undo_authorized: false,
                edit_states: Vec::new(),
                action: LearningPhaseActionV3::RunOwnedPasteSession {
                    operation_id: id(63),
                },
                target_observation_authorized: false,
                target_binding: None,
                owned_paste_session_authorized: true,
                owned_paste_session_binding: Some(owned_paste_session_binding()),
            }],
        }
    }

    fn observe_only_plan(phase_count: u32) -> LearningPhasePlanV3 {
        LearningPhasePlanV3 {
            schema_version: LEARNING_PHASE_V3_SCHEMA_VERSION,
            plan_id: id(2),
            arm_state_id: id(3),
            // Required V3 metadata, intentionally unused by ObserveTarget.
            terms: vec![TermFixture {
                fixture_id: id(1),
                before: "unused term metadata".into(),
                after: "also unused".into(),
            }],
            phases: (0..phase_count)
                .map(|index| observe_phase(100 + index, 200 + index))
                .collect(),
        }
    }

    struct LearnPhaseFixtureV3<'a> {
        phase_number: u32,
        source_fixture_number: u32,
        first_state_number: u32,
        operation_numbers: &'a [u32],
        term_fixture_id: &'a str,
        scope: LearningScopeV2,
        source_text: &'a str,
        settled_text: &'a str,
    }

    fn learn_phase(fixture: LearnPhaseFixtureV3<'_>) -> LearningPhaseV3 {
        let LearnPhaseFixtureV3 {
            phase_number,
            source_fixture_number,
            first_state_number,
            operation_numbers,
            term_fixture_id,
            scope,
            source_text,
            settled_text,
        } = fixture;
        let source = text_source(source_fixture_number, source_text);
        let states = vec![
            edit_state(
                0,
                first_state_number,
                LearningTextLayerV2::PostInsert,
                source_text,
            ),
            edit_state(
                1,
                first_state_number + 1,
                LearningTextLayerV2::Settled,
                settled_text,
            ),
        ];
        phase(
            phase_number,
            scope,
            Some(source),
            states,
            LearningPhaseActionV3::LearnText {
                operation_ids: operation_numbers.iter().map(|value| id(*value)).collect(),
                term_fixture_id: term_fixture_id.into(),
            },
            (true, true, false, false),
        )
    }

    fn followup_phase(
        phase_number: u32,
        source_fixture_number: u32,
        task_number: u32,
        operation_number: u32,
        owner_phase_id: &str,
        term_fixture_id: &str,
        scope: LearningScopeV2,
    ) -> LearningPhaseV3 {
        let source_text = "The release note says VoiceFlo today.";
        let expected_output = "The release note says VoiceFlow today.";
        phase(
            phase_number,
            scope,
            Some(text_source(source_fixture_number, source_text)),
            Vec::new(),
            LearningPhaseActionV3::ApplyFollowup {
                task_id: id(task_number),
                operation_id: id(operation_number),
                rule_owner_phase_id: owner_phase_id.into(),
                term_fixture_id: term_fixture_id.into(),
                expected_output_sha256: sha256(expected_output),
                expected_output_byte_length: expected_output.len(),
            },
            (false, false, true, false),
        )
    }

    fn fixture_plan() -> LearningPhasePlanV3 {
        let term_fixture_id = id(1);
        let scope_a = scope("mapping-a", "com.example.editor.a");
        let scope_b = scope("mapping-b", "com.example.editor.b");
        let phase_a_learn = id(10);
        let phase_a_apply = id(13);
        LearningPhasePlanV3 {
            schema_version: LEARNING_PHASE_V3_SCHEMA_VERSION,
            plan_id: id(2),
            arm_state_id: id(3),
            terms: vec![TermFixture {
                fixture_id: term_fixture_id.clone(),
                before: "VoiceFlo".into(),
                after: "VoiceFlow".into(),
            }],
            phases: vec![
                learn_phase(LearnPhaseFixtureV3 {
                    phase_number: 10,
                    source_fixture_number: 20,
                    first_state_number: 30,
                    operation_numbers: &[40, 41, 42],
                    term_fixture_id: &term_fixture_id,
                    scope: scope_a.clone(),
                    source_text: "The release note says VoiceFlo.",
                    settled_text: "The release note says VoiceFlow.",
                }),
                followup_phase(
                    11,
                    21,
                    43,
                    44,
                    &phase_a_learn,
                    &term_fixture_id,
                    scope_b.clone(),
                ),
                learn_phase(LearnPhaseFixtureV3 {
                    phase_number: 12,
                    source_fixture_number: 22,
                    first_state_number: 32,
                    operation_numbers: &[45, 46, 47],
                    term_fixture_id: &term_fixture_id,
                    scope: scope_b,
                    source_text: "The release note says VoiceFlo.",
                    settled_text: "The release note says VoiceFlow.",
                }),
                followup_phase(
                    13,
                    23,
                    48,
                    49,
                    &phase_a_learn,
                    &term_fixture_id,
                    scope_a.clone(),
                ),
                phase(
                    14,
                    scope_a,
                    None,
                    Vec::new(),
                    LearningPhaseActionV3::Undo {
                        operation_id: id(50),
                        rule_owner_phase_id: phase_a_learn,
                        term_fixture_id,
                        required_application_phase_id: phase_a_apply,
                    },
                    (false, false, false, true),
                ),
            ],
        }
    }

    #[test]
    fn accepts_cross_scope_learning_followup_conflict_and_scoped_undo_plan() {
        let plan = fixture_plan();
        assert_eq!(plan.validate(), Ok(()));
        assert!(plan.phase(&id(10)).is_some());
        assert!(plan.phase("unknown-phase").is_none());
        assert_eq!(plan.term(&id(1)).unwrap().after, "VoiceFlow");
        let identity = plan.identity_sha256().unwrap();
        assert_eq!(identity.len(), 64);
        assert_eq!(identity, plan.identity_sha256().unwrap());
        let mut changed = plan.clone();
        changed.phases[0].ttl_ms += 1;
        assert_ne!(identity, changed.identity_sha256().unwrap());
        assert!(LEARNING_PHASE_V3_KNOWN_GAPS
            .contains(&"c1_rule_identity_casing_and_pair_key_conflicts_unresolved"));
    }

    #[test]
    fn preserves_legacy_none_serialization_and_identity_fields() {
        let plan = fixture_plan();
        let value = serde_json::to_value(&plan).unwrap();
        let phases = value["phases"].as_array().unwrap();
        assert!(phases.iter().all(|phase| {
            phase.get("target_observation_authorized").is_none()
                && phase.get("target_binding").is_none()
                && phase.get("owned_paste_session_authorized").is_none()
                && phase.get("owned_paste_session_binding").is_none()
        }));
        let mut legacy_shape = value;
        for phase in legacy_shape["phases"].as_array_mut().unwrap() {
            let object = phase.as_object_mut().unwrap();
            object.remove("target_observation_authorized");
            object.remove("target_binding");
            object.remove("owned_paste_session_authorized");
            object.remove("owned_paste_session_binding");
        }
        assert_eq!(
            serde_json::to_value(&plan).unwrap(),
            legacy_shape,
            "new None/false fields must be absent from legacy V3 identity"
        );
        assert_eq!(plan.identity_sha256().unwrap().len(), 64);
    }

    #[test]
    fn accepts_registered_owned_target_only_plan_without_product_scope_claims() {
        for count in [1, 2, PHASES_MAX as u32] {
            let plan = observe_only_plan(count);
            assert_eq!(plan.validate(), Ok(()));
            assert_eq!(plan.phases.len(), count as usize);
            assert_eq!(plan.identity_sha256().unwrap().len(), 64);
            assert!(plan.phases.iter().all(|phase| {
                phase.scope == registered_observe_scope()
                    && phase.target_oracle == LearningOracleV2::IndependentTarget
                    && phase.target_observation_authorized
                    && !phase.local_only_grant
                    && phase.source.is_none()
                    && phase.edit_states.is_empty()
            }));
            assert_eq!(plan.term(&id(1)).unwrap().before, "unused term metadata");
        }
    }

    #[test]
    fn accepts_single_registered_owned_paste_session_without_learning_grants() {
        let plan = owned_paste_session_plan();
        assert_eq!(plan.validate(), Ok(()));
        let identity = plan.identity_sha256().unwrap();
        assert_eq!(identity.len(), 64);
        let mut changed = plan.clone();
        changed.phases[0]
            .owned_paste_session_binding
            .as_mut()
            .unwrap()
            .helper_executable_sha256 = sha256("another prepared helper");
        assert_ne!(identity, changed.identity_sha256().unwrap());
    }

    #[test]
    fn owned_paste_session_binding_scope_ttl_and_grants_are_strict() {
        let mut plan = owned_paste_session_plan();
        plan.phases[0]
            .owned_paste_session_binding
            .as_mut()
            .unwrap()
            .expected_settled_sha256 = sha256("caller substituted output");
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_owned_paste_session_binding_invalid")
        );

        let mut plan = owned_paste_session_plan();
        plan.phases[0].scope.mapping_id = "caller-selected-mapping".into();
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_owned_paste_session_scope_mismatch")
        );

        let mut plan = owned_paste_session_plan();
        plan.phases[0].learning_authorized = true;
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_owned_paste_session_grant_mismatch")
        );

        let mut plan = owned_paste_session_plan();
        plan.phases[0].ttl_ms = PASTE_SESSION_TTL_MAX_MS + 1;
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_owned_paste_session_ttl_invalid")
        );
    }

    #[test]
    fn owned_paste_session_cannot_be_mixed_with_other_phase_actions() {
        let mut plan = owned_paste_session_plan();
        let mut second = plan.phases[0].clone();
        second.phase_id = id(65);
        second.action = LearningPhaseActionV3::RunOwnedPasteSession {
            operation_id: id(66),
        };
        plan.phases.push(second);
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_mixed_owned_paste_session_unsupported")
        );
    }

    #[test]
    fn observe_target_binding_is_in_plan_identity_and_phase_scope_is_registered() {
        let mut plan = observe_only_plan(1);
        let identity = plan.identity_sha256().unwrap();
        plan.phases[0]
            .target_binding
            .as_mut()
            .unwrap()
            .helper_executable_sha256 = sha256("different registered helper");
        assert_ne!(identity, plan.identity_sha256().unwrap());

        let mut plan = observe_only_plan(1);
        plan.phases[0].scope.native_bundle = "com.example.unregistered".into();
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_observe_target_scope_mismatch")
        );

        let mut plan = observe_only_plan(1);
        plan.phases[0]
            .target_binding
            .as_mut()
            .unwrap()
            .control_identifier = "other-control".into();
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_observe_target_binding_invalid")
        );
    }

    #[test]
    fn observe_target_requires_its_own_grant_and_rejects_mixed_or_unprepared_plans() {
        let mut plan = observe_only_plan(1);
        plan.phases[0].target_observation_authorized = false;
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_observe_target_grant_mismatch")
        );

        let mut plan = observe_only_plan(1);
        plan.phases[0].learning_authorized = true;
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_observe_target_grant_mismatch")
        );

        let mut plan = observe_only_plan(1);
        plan.phases[0].target_binding = None;
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_observe_target_binding_required")
        );

        let mut plan = observe_only_plan(1);
        plan.phases[0].ttl_ms = OBSERVE_PHASE_TTL_MIN_MS - 1;
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_observe_target_ttl_too_short")
        );

        let mut plan = observe_only_plan(2);
        plan.phases[1] = learn_phase(LearnPhaseFixtureV3 {
            phase_number: 102,
            source_fixture_number: 202,
            first_state_number: 302,
            operation_numbers: &[402],
            term_fixture_id: &id(1),
            scope: scope("mapping-b", "com.example.editor.b"),
            source_text: "The release note says VoiceFlo.",
            settled_text: "The release note says VoiceFlow.",
        });
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_mixed_target_observation_unsupported")
        );

        let plan = observe_only_plan((PHASES_MAX + 1) as u32);
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_phase_count_invalid")
        );
    }

    #[test]
    fn admits_owner_scope_and_cross_scope_followups_after_undo() {
        let mut plan = fixture_plan();
        let owner = plan.phases[0].clone();
        let first_application = plan.phases[3].clone();
        let undo = plan.phases[4].clone();
        let owner_scope = owner.scope.clone();
        let other_scope = plan.phases[1].scope.clone();
        let owner_phase_id = id(10);
        let term_fixture_id = id(1);

        let post_undo_owner_scope_application = followup_phase(
            15,
            24,
            51,
            52,
            &owner_phase_id,
            &term_fixture_id,
            owner_scope,
        );
        let post_undo_other_scope_application = followup_phase(
            16,
            25,
            53,
            54,
            &owner_phase_id,
            &term_fixture_id,
            other_scope,
        );
        plan.phases = vec![
            owner,
            first_application,
            undo,
            post_undo_owner_scope_application,
            post_undo_other_scope_application,
        ];

        assert_eq!(plan.validate(), Ok(()));
        assert_eq!(plan.phases.len(), 5);
    }

    #[test]
    fn rejects_invalid_grants_sources_scope_and_phase_references() {
        let mut plan = fixture_plan();
        plan.phases[1].learning_authorized = true;
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_followup_grant_mismatch")
        );

        let mut plan = fixture_plan();
        if let Some(source) = plan.phases[0].source.as_mut() {
            source.rights = LearningRightsV2::PublicCcBySa4_0;
        }
        assert_eq!(plan.validate(), Err("learning_phase_v3_source_invalid"));

        let mut plan = fixture_plan();
        plan.phases[0].scope.native_bundle = "com.google.Chrome".into();
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_scope_invalid_or_browser")
        );

        let mut plan = fixture_plan();
        let only_scope = plan.phases[0].scope.clone();
        for phase in &mut plan.phases {
            phase.scope = only_scope.clone();
        }
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_two_native_scopes_required")
        );

        let mut plan = fixture_plan();
        if let LearningPhaseActionV3::ApplyFollowup {
            rule_owner_phase_id,
            ..
        } = &mut plan.phases[1].action
        {
            *rule_owner_phase_id = id(12);
        }
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_rule_owner_not_prior")
        );

        let mut plan = fixture_plan();
        plan.phases[4].scope = scope("mapping-b", "com.example.editor.b");
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_undo_scope_mismatch")
        );

        let mut plan = fixture_plan();
        plan.phases[4].undo_authorized = false;
        plan.phases[4].learning_authorized = true;
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_undo_grant_mismatch")
        );

        let mut plan = fixture_plan();
        plan.phases[0].undo_authorized = true;
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_learn_grant_mismatch")
        );

        let mut plan = fixture_plan();
        plan.phases[0].ttl_ms = PHASE_TTL_MIN_MS - 1;
        assert_eq!(plan.validate(), Err("learning_phase_v3_ttl_invalid"));

        let mut plan = fixture_plan();
        plan.phases[0].edit_states[0].byte_length += 1;
        assert_eq!(
            plan.validate(),
            Err("learning_phase_v3_source_state_mismatch")
        );

        let mut plan = fixture_plan();
        if let LearningPhaseActionV3::LearnText { operation_ids, .. } = &mut plan.phases[2].action {
            operation_ids[1] = operation_ids[0].clone();
        }
        assert_eq!(plan.validate(), Err("learning_phase_v3_entity_id_reused"));
    }

    #[test]
    fn strict_plan_and_tagged_actions_reject_unknown_fields() {
        let mut value = serde_json::to_value(fixture_plan()).unwrap();
        value["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<LearningPhasePlanV3>(value).is_err());

        let mut value = serde_json::to_value(fixture_plan()).unwrap();
        value["phases"][0]["action"]["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<LearningPhasePlanV3>(value).is_err());

        let mut value = serde_json::to_value(fixture_plan()).unwrap();
        assert!(value["phases"][4]
            .as_object_mut()
            .unwrap()
            .remove("undo_authorized")
            .is_some());
        assert!(serde_json::from_value::<LearningPhasePlanV3>(value).is_err());
    }
}
