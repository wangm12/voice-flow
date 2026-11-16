//! Local, metadata-only candidate drafts from explicitly saved History edits.
//! Approved preferences remain owned by AppMapping Settings; this module never
//! activates a candidate or grants provider permission by itself.

use crate::context::{
    self, AppMapping, WritingPreference, WritingPreferenceDimension, WritingPreferenceStatus,
    WritingPreferenceValue, WritingTaskId,
};
use crate::preference_shadow::{
    classify_weak_history_candidate, HistoryCandidateDecision, WeakHistoryPreferenceEvidence,
};
use crate::store::{
    self, Settings, WritingPreferenceDraftDecision, WritingPreferenceDraftDecisionRecord,
    WritingPreferenceEvidenceInput, WritingPreferenceEvidenceRecord,
};
use crate::{lock_recover, run_blocking_command, AppState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use tauri::{Emitter, Manager, State};

const MAX_REVIEW_BATCH: usize = 64;

#[derive(Debug, Clone)]
pub(crate) struct ManualHistoryPreferenceEvidence {
    pub(crate) source_history_id: i64,
    pub(crate) source_revision_id: i64,
    /// The original dictation transaction, shared by later History revisions.
    pub(crate) origin_id: String,
    pub(crate) mapping_id: String,
    /// Captured selector fingerprint; it is distinct from the AppMapping ID.
    pub(crate) mapping_identity_fingerprint: String,
    pub(crate) mapping_revision: u64,
    pub(crate) task_id: WritingTaskId,
    pub(crate) dimension: WritingPreferenceDimension,
    pub(crate) value: WritingPreferenceValue,
    pub(crate) occurred_at_ms: u64,
    /// Device-local calendar date of the original dictation transaction.
    pub(crate) natural_day: String,
    /// One UUID per actual History editor-open session, reused across saves.
    pub(crate) editor_session_id: String,
    pub(crate) content_fingerprint: String,
    pub(crate) template_family_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct WritingPreferenceDraftView {
    pub(crate) candidate_id: String,
    pub(crate) candidate_generation: u64,
    /// Internal scope binding; never sent through IPC.
    #[serde(skip)]
    pub(crate) mapping_identity_fingerprint: String,
    pub(crate) mapping_id: String,
    pub(crate) mapping_revision: u64,
    pub(crate) app_label: String,
    pub(crate) task_id: WritingTaskId,
    pub(crate) dimension: WritingPreferenceDimension,
    pub(crate) value: WritingPreferenceValue,
    pub(crate) distinct_origins: usize,
    pub(crate) distinct_natural_days: usize,
    pub(crate) distinct_sessions: usize,
    pub(crate) qualified_at_ms: u64,
    /// Candidates contain no source excerpts or approved style samples.
    pub(crate) has_text_sample: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WritingPreferenceDraftAction {
    Approve,
    Dismiss,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct WritingPreferenceDraftDecisionInput {
    pub(crate) candidate_id: String,
    pub(crate) candidate_generation: u64,
    pub(crate) expected_mapping_revision: u64,
    pub(crate) decision: WritingPreferenceDraftAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WritingPreferenceDraftReviewStatus {
    Approved,
    Dismissed,
    StaleCandidate,
    StaleMapping,
    SourceRecheckFailed,
    MappingUnavailable,
    PermissionRevoked,
    PreferenceConflict,
    AlreadyConfigured,
    AlreadyDecided,
    NotFound,
    DuplicateRequest,
    SettingsSaveFailed,
    DecisionSaveFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct WritingPreferenceDraftReviewResult {
    pub(crate) candidate_id: String,
    pub(crate) mapping_id: Option<String>,
    pub(crate) decision: WritingPreferenceDraftAction,
    pub(crate) status: WritingPreferenceDraftReviewStatus,
    pub(crate) new_mapping_revision: Option<u64>,
}

#[derive(Debug, Clone)]
struct DerivedDraft {
    view: WritingPreferenceDraftView,
}

fn now_epoch_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn mapping_is_current_for_evidence(
    settings: &Settings,
    evidence: &ManualHistoryPreferenceEvidence,
) -> bool {
    if !settings.local_edit_learning_enabled
        || !settings.dictionary_learn_enabled
        || !settings.context_enabled
    {
        return false;
    }
    let Some(mapping) = settings
        .context_mappings
        .iter()
        .find(|mapping| mapping.id == evidence.mapping_id && !mapping.deleted)
    else {
        return false;
    };
    if !mapping.enabled
        || !mapping.dictionary_learn_enabled
        || crate::lexicon::is_default_learn_off_target(
            mapping.bundle_id.as_deref(),
            mapping.browser_host.as_deref(),
        )
        || mapping.preference_revision != evidence.mapping_revision
        || mapping.writing_task_id != Some(evidence.task_id)
        || !mapping.has_task_anchor()
        || mapping.declared_identity_fingerprint() != evidence.mapping_identity_fingerprint
        || (mapping.browser_host.is_some() && !settings.browser_access_enabled)
    {
        return false;
    }
    true
}

/// Ingest one current History editor commit, or invalidate the prior evidence
/// for that transaction when the latest edit is ambiguous/unsupported. The
/// settings gate serializes this with permission and mapping changes.
pub(crate) async fn replace_history_preference_evidence(
    app: &tauri::AppHandle,
    state: &AppState,
    source_history_id: i64,
    source_revision_id: i64,
    origin_id: String,
    evidence: Option<ManualHistoryPreferenceEvidence>,
) -> Result<(), String> {
    let _settings_gate = state.settings_gate.lock().await;
    replace_history_preference_evidence_under_settings_gate(
        app,
        state,
        &_settings_gate,
        source_history_id,
        source_revision_id,
        origin_id,
        evidence,
    )
    .await
}

pub(crate) async fn replace_history_preference_evidence_under_settings_gate(
    app: &tauri::AppHandle,
    state: &AppState,
    _settings_gate: &tokio::sync::MutexGuard<'_, ()>,
    source_history_id: i64,
    source_revision_id: i64,
    origin_id: String,
    evidence: Option<ManualHistoryPreferenceEvidence>,
) -> Result<(), String> {
    let settings = lock_recover(&state.settings).clone();
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let input = if let Some(evidence) = evidence {
        if !mapping_is_current_for_evidence(&settings, &evidence) {
            // Recheck History ownership before invalidating, so an old async
            // callback cannot erase a later edit for the same transaction.
            let root = dir.clone();
            let origin = origin_id.clone();
            let outcome = run_blocking_command(move || {
                store::replace_writing_preference_evidence(
                    &root,
                    source_history_id,
                    source_revision_id,
                    &origin,
                    None,
                )
                .map_err(|error| error.to_string())
            })
            .await?;
            let _ = outcome;
            return Ok(());
        }
        if evidence.source_history_id != source_history_id
            || evidence.source_revision_id != source_revision_id
            || evidence.origin_id != origin_id
        {
            return Err("history_preference_source_mismatch".into());
        }
        Some(WritingPreferenceEvidenceInput {
            source_history_id: evidence.source_history_id,
            source_revision_id: evidence.source_revision_id,
            mapping_id: evidence.mapping_id,
            mapping_identity_fingerprint: evidence.mapping_identity_fingerprint,
            mapping_revision: evidence.mapping_revision,
            task_id: evidence.task_id,
            dimension: evidence.dimension,
            value: evidence.value,
            origin_id: evidence.origin_id,
            content_fingerprint: evidence.content_fingerprint,
            template_family_fingerprint: evidence.template_family_fingerprint,
            occurred_at_ms: evidence.occurred_at_ms,
            natural_day: evidence.natural_day,
            editor_session_id: evidence.editor_session_id,
        })
    } else {
        None
    };

    let root = dir.clone();
    let origin = origin_id.clone();
    let outcome = run_blocking_command(move || {
        store::replace_writing_preference_evidence(
            &root,
            source_history_id,
            source_revision_id,
            &origin,
            input.as_ref(),
        )
        .map_err(|error| error.to_string())
    })
    .await?;
    match outcome {
        store::WritingPreferenceEvidenceWrite::AtCapacity => {
            log::warn!("writing preference evidence reached its bounded History cap");
        }
        store::WritingPreferenceEvidenceWrite::RejectedSource
        | store::WritingPreferenceEvidenceWrite::StaleOrigin => {
            log::debug!("ignored stale or untrusted History preference evidence");
        }
        store::WritingPreferenceEvidenceWrite::Inserted
        | store::WritingPreferenceEvidenceWrite::Replaced
        | store::WritingPreferenceEvidenceWrite::Duplicate => {}
    }
    Ok(())
}

fn candidate_id(
    mapping_id: &str,
    mapping_identity_fingerprint: &str,
    task_id: WritingTaskId,
    dimension: WritingPreferenceDimension,
    value: WritingPreferenceValue,
    generation: u64,
    rows: &[&WritingPreferenceEvidenceRecord],
) -> String {
    let mut sources = rows
        .iter()
        .filter(|row| row.value == value)
        .map(|row| {
            format!(
                "{}|{}|{}|{}|{}",
                row.source_history_id,
                row.source_revision_id,
                row.origin_id,
                row.content_fingerprint,
                row.template_family_fingerprint
                    .as_deref()
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    sources.sort_unstable();
    let identity = format!(
        "v2|{mapping_id}|{mapping_identity_fingerprint}|{task_id:?}|{dimension:?}|{value:?}|{generation}|{}",
        sources.join("\n")
    );
    hex::encode(Sha256::digest(identity.as_bytes()))
}

fn active_or_tombstoned_preference_exists(
    mapping: &AppMapping,
    task_id: WritingTaskId,
    dimension: WritingPreferenceDimension,
) -> bool {
    mapping.writing_preferences.iter().any(|preference| {
        preference.task_id.or(mapping.writing_task_id) == Some(task_id)
            && preference.dimension == dimension
    })
}

fn derive_candidates(
    settings: &Settings,
    evidence: &[WritingPreferenceEvidenceRecord],
    decisions: &[WritingPreferenceDraftDecisionRecord],
    now_ms: u64,
) -> Vec<DerivedDraft> {
    let cutoff = now_ms.saturating_sub(14 * 24 * 60 * 60 * 1_000);
    let mut groups =
        BTreeMap::<(String, String, WritingTaskId, WritingPreferenceDimension), Vec<_>>::new();
    for row in evidence.iter().filter(|row| row.eligible) {
        groups
            .entry((
                row.mapping_id.clone(),
                row.mapping_identity_fingerprint.clone(),
                row.task_id,
                row.dimension,
            ))
            .or_default()
            .push(row);
    }
    let mut drafts = Vec::new();
    for ((mapping_id, mapping_identity_fingerprint, task_id, dimension), rows) in groups {
        if mapping_identity_fingerprint.is_empty() {
            continue;
        }
        // Ignore only evidence from a generation with an explicit decision.
        // This follows §7.3: dismissing a generation closes it without
        // letting its old positives block a later opposing candidate forever.
        let current_rows = rows
            .iter()
            .copied()
            .filter(|row| {
                !decisions.iter().any(|decision| {
                    decision.mapping_id == row.mapping_id
                        && decision.task_id == row.task_id
                        && decision.dimension == row.dimension
                        && decision.value == row.value
                        && decision.generation == row.generation
                        && decision.decided_at_ms >= cutoff
                        && decision.decided_at_ms <= now_ms
                })
            })
            .collect::<Vec<_>>();
        let captured_mapping_revision = current_rows
            .iter()
            .map(|row| row.mapping_revision)
            .max()
            .unwrap_or_default();
        let current_mapping = settings
            .context_mappings
            .iter()
            .find(|mapping| mapping.id == mapping_id);
        let mapping_revision = current_mapping
            .map(|mapping| mapping.preference_revision)
            .unwrap_or(captured_mapping_revision);
        let app_label = current_mapping
            .map(|mapping| mapping.label.clone())
            .unwrap_or_default();
        let generations = current_rows
            .iter()
            .map(|row| row.generation)
            .collect::<BTreeSet<_>>();
        for generation in generations {
            let candidate_rows = current_rows
                .iter()
                .copied()
                .filter(|row| row.generation == generation)
                .collect::<Vec<_>>();
            let values = candidate_rows
                .iter()
                .map(|row| row.value)
                .collect::<BTreeSet<_>>();
            for value in values {
                // Candidate support stays within this value's generation,
                // while any undismissed opposing value in the full current
                // scope and 14-day window blocks readiness across generations.
                let classifier_evidence = current_rows
                    .iter()
                    .filter(|row| row.value != value || row.generation == generation)
                    .map(|row| WeakHistoryPreferenceEvidence {
                        task_id: row.task_id,
                        dimension: row.dimension,
                        value: row.value,
                        origin_id: row.origin_id.clone(),
                        content_fingerprint: row.content_fingerprint.clone(),
                        occurred_at_ms: row.occurred_at_ms,
                        natural_day: row.natural_day.clone(),
                        editor_session_id: row.editor_session_id.clone(),
                        template_family_fingerprint: row.template_family_fingerprint.clone(),
                        eligible: row.eligible,
                    })
                    .collect::<Vec<_>>();
                let HistoryCandidateDecision::CandidateReady {
                    supporting_origins,
                    distinct_days,
                    distinct_sessions,
                    ..
                } = classify_weak_history_candidate(
                    now_ms,
                    task_id,
                    dimension,
                    value,
                    &classifier_evidence,
                )
                else {
                    continue;
                };
                drafts.push(DerivedDraft {
                    view: WritingPreferenceDraftView {
                        candidate_id: candidate_id(
                            &mapping_id,
                            &mapping_identity_fingerprint,
                            task_id,
                            dimension,
                            value,
                            generation,
                            &candidate_rows,
                        ),
                        candidate_generation: generation,
                        mapping_identity_fingerprint: mapping_identity_fingerprint.clone(),
                        mapping_id: mapping_id.clone(),
                        mapping_revision,
                        app_label: app_label.clone(),
                        task_id,
                        dimension,
                        value,
                        distinct_origins: supporting_origins,
                        distinct_natural_days: distinct_days,
                        distinct_sessions,
                        qualified_at_ms: candidate_rows
                            .iter()
                            .filter(|row| row.value == value)
                            .map(|row| row.occurred_at_ms)
                            .max()
                            .unwrap_or_default(),
                        has_text_sample: false,
                    },
                });
            }
        }
    }
    drafts.sort_by(|left, right| {
        (
            &left.view.app_label,
            left.view.task_id,
            left.view.dimension,
            left.view.value,
        )
            .cmp(&(
                &right.view.app_label,
                right.view.task_id,
                right.view.dimension,
                right.view.value,
            ))
    });
    drafts
}

fn candidate_is_visible(settings: &Settings, draft: &WritingPreferenceDraftView) -> bool {
    if !settings.local_edit_learning_enabled
        || !settings.dictionary_learn_enabled
        || !settings.context_enabled
    {
        return false;
    }
    settings.context_mappings.iter().any(|mapping| {
        mapping.id == draft.mapping_id
            && !mapping.deleted
            && mapping.declared_identity_fingerprint() == draft.mapping_identity_fingerprint
            && mapping.enabled
            && mapping.dictionary_learn_enabled
            && !crate::lexicon::is_default_learn_off_target(
                mapping.bundle_id.as_deref(),
                mapping.browser_host.as_deref(),
            )
            && mapping.preference_revision == draft.mapping_revision
            && mapping.writing_task_id == Some(draft.task_id)
            && mapping.has_task_anchor()
            && !active_or_tombstoned_preference_exists(mapping, draft.task_id, draft.dimension)
            && (mapping.browser_host.is_none() || settings.browser_access_enabled)
    })
}

fn visible_candidates(
    settings: &Settings,
    evidence: &[WritingPreferenceEvidenceRecord],
    decisions: &[WritingPreferenceDraftDecisionRecord],
    now_ms: u64,
) -> Vec<WritingPreferenceDraftView> {
    let decided = decisions
        .iter()
        .map(|decision| decision.candidate_id.as_str())
        .collect::<HashSet<_>>();
    derive_candidates(settings, evidence, decisions, now_ms)
        .into_iter()
        .filter_map(|draft| {
            if decided.contains(draft.view.candidate_id.as_str())
                || !candidate_is_visible(settings, &draft.view)
            {
                return None;
            }
            Some(draft.view)
        })
        .collect()
}

fn merge_reviewed_preferences(
    existing: &AppMapping,
    drafts: &[&WritingPreferenceDraftView],
) -> Result<AppMapping, String> {
    let mut values_by_slot = BTreeMap::<
        (WritingTaskId, WritingPreferenceDimension),
        BTreeSet<WritingPreferenceValue>,
    >::new();
    for draft in drafts {
        values_by_slot
            .entry((draft.task_id, draft.dimension))
            .or_default()
            .insert(draft.value);
        if active_or_tombstoned_preference_exists(existing, draft.task_id, draft.dimension) {
            return Err("writing preference slot is already configured".into());
        }
    }
    if values_by_slot.values().any(|values| values.len() > 1) {
        return Err("writing preference batch contains conflicting values".into());
    }

    let mut next = existing.clone();
    for draft in drafts {
        next.writing_preferences.push(WritingPreference {
            task_id: Some(draft.task_id),
            dimension: draft.dimension,
            value: draft.value,
            status: WritingPreferenceStatus::Active,
            version: 1,
            source: context::WritingPreferenceSource::ReviewedHistory,
        });
        if matches!(
            draft.dimension,
            WritingPreferenceDimension::GreetingTone
                | WritingPreferenceDimension::TerminalPunctuation
        ) {
            // Provider consent is mapping-wide. A new reviewed value cannot
            // inherit an older grant; style-sample consent stays separate.
            next.preference_guidance_provider_approved = false;
        }
    }
    context::merge_reviewed_history_preferences(existing, &mut next)?;
    next.preference_guidance_provider_approved = existing.preference_guidance_provider_approved
        && next.preference_guidance_provider_approved;
    Ok(next)
}

fn replace_mapping_in_owner(
    mappings: &mut [AppMapping],
    replacement: AppMapping,
) -> Result<(), String> {
    let matches = mappings
        .iter()
        .enumerate()
        .filter_map(|(index, mapping)| (mapping.id == replacement.id).then_some(index))
        .collect::<Vec<_>>();
    let [index] = matches.as_slice() else {
        return Err("mapping owner is missing or ambiguous".into());
    };
    if mappings[*index].deleted || replacement.deleted {
        return Err("deleted mapping cannot own a reviewed preference".into());
    }
    mappings[*index] = replacement;
    Ok(())
}

#[tauri::command]
pub(crate) async fn list_writing_preference_drafts(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<WritingPreferenceDraftView>, String> {
    let settings = lock_recover(&state.settings).clone();
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let now_ms = now_epoch_millis();
    run_blocking_command(move || {
        let evidence = store::list_writing_preference_evidence(&dir, now_ms)
            .map_err(|error| error.to_string())?;
        let decisions = store::writing_preference_draft_decisions(&dir, now_ms)
            .map_err(|error| error.to_string())?;
        Ok(visible_candidates(&settings, &evidence, &decisions, now_ms))
    })
    .await
}

#[tauri::command]
pub(crate) async fn review_writing_preference_drafts(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    decisions: Vec<WritingPreferenceDraftDecisionInput>,
) -> Result<Vec<WritingPreferenceDraftReviewResult>, String> {
    if decisions.len() > MAX_REVIEW_BATCH {
        return Err("writing preference review batch is too large".into());
    }
    if decisions.iter().any(|item| {
        item.candidate_id.trim().is_empty()
            || item.candidate_id.len() > 128
            || item.expected_mapping_revision > i64::MAX as u64
            || item.candidate_generation > i64::MAX as u64
    }) {
        return Err("writing preference review item is invalid".into());
    }

    // History edits/deletes and review share this exclusive owner. Settings
    // mutations then serialize under the gate, so a source revision cannot
    // change between the final History recheck and the Settings write.
    let _history_owner = crate::history_commands::claim_history_mutation(&app, &state)?;
    let _settings_gate = state.settings_gate.lock().await;
    let settings = lock_recover(&state.settings).clone();
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let now_ms = now_epoch_millis();

    let evidence_dir = dir.clone();
    let evidence = run_blocking_command(move || {
        store::list_writing_preference_evidence(&evidence_dir, now_ms)
            .map_err(|error| error.to_string())
    })
    .await?;
    let decision_dir = dir.clone();
    let stored_decisions = run_blocking_command(move || {
        store::writing_preference_draft_decisions(&decision_dir, now_ms)
            .map_err(|error| error.to_string())
    })
    .await?;

    // Derive the candidate snapshot independently from current Settings. This
    // lets the response distinguish an unavailable permission from an unknown
    // ID and makes stale mapping revisions visible to compare-and-set.
    let candidates = derive_candidates(&settings, &evidence, &stored_decisions, now_ms)
        .into_iter()
        .map(|draft| (draft.view.candidate_id.clone(), draft.view))
        .collect::<HashMap<_, _>>();
    let prior_decisions = stored_decisions
        .iter()
        .map(|decision| (decision.candidate_id.clone(), decision.mapping_id.clone()))
        .collect::<HashMap<_, _>>();
    let request_counts =
        decisions
            .iter()
            .fold(HashMap::<String, usize>::new(), |mut counts, item| {
                *counts.entry(item.candidate_id.clone()).or_default() += 1;
                counts
            });

    let mut results = decisions
        .iter()
        .map(|item| WritingPreferenceDraftReviewResult {
            candidate_id: item.candidate_id.clone(),
            mapping_id: None,
            decision: item.decision,
            status: WritingPreferenceDraftReviewStatus::NotFound,
            new_mapping_revision: None,
        })
        .collect::<Vec<_>>();
    let mut actionable_by_mapping = BTreeMap::<String, Vec<usize>>::new();

    for (index, item) in decisions.iter().enumerate() {
        if request_counts
            .get(&item.candidate_id)
            .copied()
            .unwrap_or_default()
            > 1
        {
            results[index].status = WritingPreferenceDraftReviewStatus::DuplicateRequest;
            results[index].mapping_id = candidates
                .get(&item.candidate_id)
                .map(|draft| draft.mapping_id.clone())
                .or_else(|| prior_decisions.get(&item.candidate_id).cloned());
            continue;
        }
        if let Some(mapping_id) = prior_decisions.get(&item.candidate_id) {
            results[index].status = WritingPreferenceDraftReviewStatus::AlreadyDecided;
            results[index].mapping_id = Some(mapping_id.clone());
            continue;
        }
        let Some(draft) = candidates.get(&item.candidate_id) else {
            continue;
        };
        results[index].mapping_id = Some(draft.mapping_id.clone());
        if draft.candidate_generation != item.candidate_generation {
            results[index].status = WritingPreferenceDraftReviewStatus::StaleCandidate;
            continue;
        }
        if draft.mapping_revision != item.expected_mapping_revision {
            results[index].status = WritingPreferenceDraftReviewStatus::StaleMapping;
            continue;
        }

        let Some(mapping) = settings
            .context_mappings
            .iter()
            .find(|mapping| mapping.id == draft.mapping_id && !mapping.deleted)
        else {
            results[index].status = WritingPreferenceDraftReviewStatus::MappingUnavailable;
            continue;
        };
        if mapping.preference_revision != draft.mapping_revision {
            results[index].status = WritingPreferenceDraftReviewStatus::StaleMapping;
            continue;
        }
        if mapping.declared_identity_fingerprint() != draft.mapping_identity_fingerprint
            || mapping.writing_task_id != Some(draft.task_id)
        {
            results[index].status = WritingPreferenceDraftReviewStatus::MappingUnavailable;
            continue;
        }
        if !mapping.enabled
            || mapping.writing_task_id != Some(draft.task_id)
            || !mapping.has_task_anchor()
        {
            results[index].status = WritingPreferenceDraftReviewStatus::MappingUnavailable;
            continue;
        }
        if !settings.local_edit_learning_enabled
            || !settings.dictionary_learn_enabled
            || !settings.context_enabled
            || !mapping.dictionary_learn_enabled
            || crate::lexicon::is_default_learn_off_target(
                mapping.bundle_id.as_deref(),
                mapping.browser_host.as_deref(),
            )
            || (mapping.browser_host.is_some() && !settings.browser_access_enabled)
        {
            results[index].status = WritingPreferenceDraftReviewStatus::PermissionRevoked;
            continue;
        }
        if active_or_tombstoned_preference_exists(mapping, draft.task_id, draft.dimension) {
            results[index].status = WritingPreferenceDraftReviewStatus::AlreadyConfigured;
            continue;
        }
        actionable_by_mapping
            .entry(draft.mapping_id.clone())
            .or_default()
            .push(index);
    }

    let mut working_mappings = settings.context_mappings.clone();
    let settings_writer = TauriReviewedSettingsPersistence {
        app: &app,
        state: &state,
        settings: &settings,
    };
    commit_review_decisions(
        &dir,
        &settings,
        &candidates,
        &decisions,
        &mut results,
        &mut working_mappings,
        actionable_by_mapping,
        now_ms,
        &settings_writer,
    )
    .await?;
    let _ = app.emit("writing_preference_drafts://changed", ());
    Ok(results)
}

fn decision_record(
    draft: &WritingPreferenceDraftView,
    decision: WritingPreferenceDraftDecision,
    decided_at_ms: u64,
) -> WritingPreferenceDraftDecisionRecord {
    WritingPreferenceDraftDecisionRecord {
        candidate_id: draft.candidate_id.clone(),
        mapping_id: draft.mapping_id.clone(),
        mapping_revision: draft.mapping_revision,
        task_id: draft.task_id,
        dimension: draft.dimension,
        value: draft.value,
        generation: draft.candidate_generation,
        decision,
        decided_at_ms,
    }
}

trait ReviewedSettingsPersistence {
    fn persist<'a>(
        &'a self,
        mappings: Vec<AppMapping>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;
}

struct TauriReviewedSettingsPersistence<'a> {
    app: &'a tauri::AppHandle,
    state: &'a AppState,
    settings: &'a Settings,
}

impl ReviewedSettingsPersistence for TauriReviewedSettingsPersistence<'_> {
    fn persist<'a>(
        &'a self,
        mappings: Vec<AppMapping>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(crate::persist_context_state(
            self.app,
            self.state,
            self.settings.context_enabled,
            self.settings.browser_access_enabled,
            mappings,
        ))
    }
}

/// Apply every validated approval against one cumulative mapping owner and
/// persist its durable per-candidate decision. The Tauri command and the
/// isolated owner-chain regression both call this exact production helper.
#[allow(clippy::too_many_arguments)]
async fn commit_review_decisions(
    dir: &std::path::Path,
    settings: &Settings,
    candidates: &HashMap<String, WritingPreferenceDraftView>,
    requests: &[WritingPreferenceDraftDecisionInput],
    results: &mut [WritingPreferenceDraftReviewResult],
    working_mappings: &mut Vec<AppMapping>,
    decisions_by_mapping: BTreeMap<String, Vec<usize>>,
    now_ms: u64,
    settings_writer: &impl ReviewedSettingsPersistence,
) -> Result<(), String> {
    for (mapping_id, indices) in decisions_by_mapping {
        // Retention may prune evidence outside the Settings owner. Recheck it
        // just before the explicit approval is committed.
        let fresh_evidence_dir = dir.to_path_buf();
        let fresh_evidence = run_blocking_command(move || {
            store::list_writing_preference_evidence(&fresh_evidence_dir, now_ms)
                .map_err(|error| error.to_string())
        })
        .await;
        let fresh_evidence = match fresh_evidence {
            Ok(evidence) => evidence,
            Err(error) => {
                log::warn!("candidate source recheck failed before Settings commit: {error}");
                for index in indices {
                    results[index].status = WritingPreferenceDraftReviewStatus::SourceRecheckFailed;
                }
                continue;
            }
        };
        let fresh_decisions_dir = dir.to_path_buf();
        let fresh_decisions = run_blocking_command(move || {
            store::writing_preference_draft_decisions(&fresh_decisions_dir, now_ms)
                .map_err(|error| error.to_string())
        })
        .await;
        let fresh_decisions = match fresh_decisions {
            Ok(decisions) => decisions,
            Err(error) => {
                log::warn!("candidate decision recheck failed before Settings commit: {error}");
                for index in indices {
                    results[index].status = WritingPreferenceDraftReviewStatus::SourceRecheckFailed;
                }
                continue;
            }
        };
        let fresh_candidate_ids =
            derive_candidates(settings, &fresh_evidence, &fresh_decisions, now_ms)
                .into_iter()
                .map(|draft| draft.view.candidate_id)
                .collect::<HashSet<_>>();
        let indices = indices
            .into_iter()
            .filter(|index| {
                let still_current = fresh_candidate_ids.contains(&requests[*index].candidate_id);
                if !still_current {
                    results[*index].status = WritingPreferenceDraftReviewStatus::StaleCandidate;
                }
                still_current
            })
            .collect::<Vec<_>>();
        if indices.is_empty() {
            continue;
        }

        let Some(mapping_index) = working_mappings
            .iter()
            .position(|mapping| mapping.id == mapping_id && !mapping.deleted)
        else {
            for index in indices {
                results[index].status = WritingPreferenceDraftReviewStatus::MappingUnavailable;
            }
            continue;
        };
        let existing_mapping = working_mappings[mapping_index].clone();
        let mut approve_indices = Vec::new();
        let mut dismiss_indices = Vec::new();
        for index in indices {
            let request = &requests[index];
            let Some(draft) = candidates.get(&request.candidate_id) else {
                results[index].status = WritingPreferenceDraftReviewStatus::StaleCandidate;
                continue;
            };
            if existing_mapping.preference_revision != request.expected_mapping_revision
                || existing_mapping.preference_revision != draft.mapping_revision
            {
                results[index].status = WritingPreferenceDraftReviewStatus::StaleMapping;
                continue;
            }
            if request.decision == WritingPreferenceDraftAction::Approve {
                approve_indices.push(index);
            } else {
                dismiss_indices.push(index);
            }
        }

        let mut saved_revision = existing_mapping.preference_revision;
        let mut settings_saved = true;
        if !approve_indices.is_empty() {
            let reviewed_drafts = approve_indices
                .iter()
                .map(|index| &candidates[&requests[*index].candidate_id])
                .collect::<Vec<_>>();
            match merge_reviewed_preferences(&existing_mapping, &reviewed_drafts) {
                Err(error) => {
                    settings_saved = false;
                    for index in approve_indices.iter().copied() {
                        results[index].status =
                            WritingPreferenceDraftReviewStatus::PreferenceConflict;
                    }
                    log::warn!("candidate preferences could not merge into owner: {error}");
                }
                Ok(next_mapping) => {
                    let mut next_mappings = working_mappings.clone();
                    if let Err(error) =
                        replace_mapping_in_owner(&mut next_mappings, next_mapping.clone())
                    {
                        settings_saved = false;
                        for index in approve_indices.iter().copied() {
                            results[index].status =
                                WritingPreferenceDraftReviewStatus::MappingUnavailable;
                        }
                        log::warn!("reviewed preference mapping owner was ambiguous: {error}");
                    } else {
                        match settings_writer.persist(next_mappings.clone()).await {
                            Ok(()) => {
                                saved_revision = next_mapping.preference_revision;
                                *working_mappings = next_mappings;
                            }
                            Err(error) => {
                                settings_saved = false;
                                for index in approve_indices.iter().copied() {
                                    results[index].status =
                                        WritingPreferenceDraftReviewStatus::SettingsSaveFailed;
                                }
                                log::warn!("reviewed preference Settings commit failed: {error}");
                            }
                        }
                    }
                }
            }
        }

        for index in dismiss_indices {
            let draft = &candidates[&requests[index].candidate_id];
            let record = decision_record(draft, WritingPreferenceDraftDecision::Dismissed, now_ms);
            let decision_dir = dir.to_path_buf();
            let saved = run_blocking_command(move || {
                store::record_writing_preference_draft_decision(&decision_dir, &record)
                    .map_err(|error| error.to_string())
            })
            .await;
            results[index].status = match saved {
                Ok(true) => WritingPreferenceDraftReviewStatus::Dismissed,
                Ok(false) => WritingPreferenceDraftReviewStatus::AlreadyDecided,
                Err(error) => {
                    log::warn!("candidate dismissal decision could not be persisted: {error}");
                    WritingPreferenceDraftReviewStatus::DecisionSaveFailed
                }
            };
            results[index].mapping_id = Some(mapping_id.clone());
        }

        if settings_saved {
            for index in approve_indices {
                let draft = &candidates[&requests[index].candidate_id];
                let record =
                    decision_record(draft, WritingPreferenceDraftDecision::Approved, now_ms);
                let decision_dir = dir.to_path_buf();
                match run_blocking_command(move || {
                    store::record_writing_preference_draft_decision(&decision_dir, &record)
                        .map_err(|error| error.to_string())
                })
                .await
                {
                    Ok(_) => results[index].status = WritingPreferenceDraftReviewStatus::Approved,
                    Err(error) => {
                        // Settings is authoritative; report the committed
                        // approval so the UI does not invite a false retry.
                        log::warn!("Settings committed candidate approval without its decision marker: {error}");
                        results[index].status = WritingPreferenceDraftReviewStatus::Approved;
                    }
                }
                results[index].mapping_id = Some(mapping_id.clone());
                results[index].new_mapping_revision = Some(saved_revision);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{
        AppMapping, ContextFamily, ContextPolicy, ContextProfile, ContextSource, FocusKind,
        HistoryPreferenceOutputKind, HistoryPreferenceProvenanceV1, StyleExamplePair,
        WritingPreferenceSource,
    };
    use crate::preference_shadow::{HistoryEditorCommitV1, HistoryEditorInputKind};
    use std::path::{Path, PathBuf};

    const SESSION_ONE: &str = "11111111-1111-4111-8111-111111111111";
    const SESSION_TWO: &str = "22222222-2222-4222-8222-222222222222";
    const DAY_MS: u64 = 24 * 60 * 60 * 1_000;

    struct IsolatedDirectory(PathBuf);

    impl IsolatedDirectory {
        fn create() -> Self {
            let path = std::env::temp_dir().join(format!(
                "voiceflow-writing-preference-review-{}-{}",
                std::process::id(),
                now_epoch_millis()
            ));
            std::fs::create_dir_all(&path).expect("isolated data directory is created");
            Self(path)
        }
    }

    impl Drop for IsolatedDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct IsolatedSettingsPersistence {
        dir: PathBuf,
    }

    impl ReviewedSettingsPersistence for IsolatedSettingsPersistence {
        fn persist<'a>(
            &'a self,
            mappings: Vec<AppMapping>,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
            Box::pin(async move {
                let path = self.dir.join("settings.json");
                let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
                let mut settings: Settings =
                    serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
                settings.context_mappings = mappings;
                store::save_configuration(&self.dir, &settings).map_err(|error| error.to_string())
            })
        }
    }

    fn read_isolated_settings(dir: &Path) -> Settings {
        let bytes = std::fs::read(dir.join("settings.json")).expect("isolated Settings exist");
        serde_json::from_slice(&bytes).expect("isolated Settings reload")
    }

    fn mapping(
        id: &str,
        label: &str,
        family: ContextFamily,
        task_id: WritingTaskId,
        focus_kind: FocusKind,
        bundle_id: &str,
        preference_revision: u64,
    ) -> AppMapping {
        AppMapping {
            id: id.into(),
            label: label.into(),
            family,
            writing_task_id: Some(task_id),
            writing_preferences: Vec::new(),
            preference_guidance_provider_approved: false,
            preference_revision,
            deleted: false,
            mode_id: None,
            bundle_id: Some(bundle_id.into()),
            executable: None,
            browser_host: None,
            browser_path_prefix: None,
            focused_field: Some(focus_kind),
            source_permissions: Default::default(),
            style_example_input: Some("style example input".into()),
            style_example_output: Some("style example output".into()),
            style_example_pairs: vec![StyleExamplePair {
                input: "style example input".into(),
                output: "style example output".into(),
            }],
            style_examples_approved: id == "mapping-a",
            enabled: true,
            cleanup_effort: None,
            cleanup_intensity: None,
            cleanup_enabled: true,
            dictionary_learn_enabled: true,
        }
    }

    fn history_snapshot(
        mapping: &AppMapping,
        input_kind: FocusKind,
    ) -> crate::context::ContextSnapshot {
        let task_id = mapping.writing_task_id.expect("test mapping has a task");
        let mut snapshot = crate::context::ContextSnapshot::general();
        snapshot.profile = ContextProfile {
            id: format!("user.{}", mapping.id),
            family: mapping.family,
            writing_mode_id: mapping.mode_id.clone(),
            writing_task_id: Some(task_id),
            app_label: mapping.label.clone(),
            icon_key: "app".into(),
            source: ContextSource::UserMapping,
            confidence: 1.0,
        };
        snapshot.policy = ContextPolicy::for_family(mapping.family);
        snapshot.policy.input_kind = input_kind;
        snapshot.policy.output_mode = Some("auto".into());
        snapshot.policy.history_preference_provenance = Some(HistoryPreferenceProvenanceV1 {
            version: 1,
            task_id: Some(task_id),
            mapping_preference_revision: Some(mapping.preference_revision),
            intent_source: crate::llm::IntentSource::Implicit,
            output_kind: HistoryPreferenceOutputKind::PreparedTranscript,
            output_mode: Some("auto".into()),
            snippet_expanded: false,
            session_skip_llm_cleanup: Some(false),
        });
        snapshot.mapping_identity = Some(mapping.declared_identity_fingerprint());
        snapshot.mapping_preference_revision = Some(mapping.preference_revision);
        snapshot
    }

    fn store_input(evidence: &ManualHistoryPreferenceEvidence) -> WritingPreferenceEvidenceInput {
        WritingPreferenceEvidenceInput {
            source_history_id: evidence.source_history_id,
            source_revision_id: evidence.source_revision_id,
            mapping_id: evidence.mapping_id.clone(),
            mapping_identity_fingerprint: evidence.mapping_identity_fingerprint.clone(),
            mapping_revision: evidence.mapping_revision,
            task_id: evidence.task_id,
            dimension: evidence.dimension,
            value: evidence.value,
            origin_id: evidence.origin_id.clone(),
            content_fingerprint: evidence.content_fingerprint.clone(),
            template_family_fingerprint: evidence.template_family_fingerprint.clone(),
            occurred_at_ms: evidence.occurred_at_ms,
            natural_day: evidence.natural_day.clone(),
            editor_session_id: evidence.editor_session_id.clone(),
        }
    }

    fn insert_synthetic_history_owner(
        dir: &Path,
        mapping: &AppMapping,
        text: &str,
        origin_id: &str,
        recorded_at_ms: u64,
    ) -> i64 {
        let input_kind = mapping.focused_field.unwrap_or(FocusKind::Unknown);
        let snapshot = history_snapshot(mapping, input_kind);
        store::insert_history_with_asr_candidate_and_delivery_and_spool_and_cleanup(
            dir,
            text,
            Some(text),
            None,
            text,
            0.1,
            false,
            None,
            "ok",
            "paste",
            None,
            &snapshot,
            None,
            "isolated-owner-chain-regression",
            "success",
        )
        .expect("synthetic History transaction insert");
        let history_id = store::get_history_page(dir, 100, None, None)
            .expect("read isolated History")
            .items
            .first()
            .expect("inserted History row")
            .id;
        let connection = rusqlite::Connection::open(dir.join("history.sqlite"))
            .expect("open only isolated History database");
        connection
            .execute(
                "UPDATE dictations SET transaction_id = ?1,
                        created_at = datetime(?2 / 1000, 'unixepoch')
                 WHERE id = ?3",
                rusqlite::params![origin_id, recorded_at_ms as i64, history_id],
            )
            .expect("bind source transaction and recording time");
        history_id
    }

    fn review_input(
        draft: &WritingPreferenceDraftView,
        decision: WritingPreferenceDraftAction,
    ) -> WritingPreferenceDraftDecisionInput {
        WritingPreferenceDraftDecisionInput {
            candidate_id: draft.candidate_id.clone(),
            candidate_generation: draft.candidate_generation,
            expected_mapping_revision: draft.mapping_revision,
            decision,
        }
    }

    fn result_for(
        input: &WritingPreferenceDraftDecisionInput,
    ) -> WritingPreferenceDraftReviewResult {
        WritingPreferenceDraftReviewResult {
            candidate_id: input.candidate_id.clone(),
            mapping_id: None,
            decision: input.decision,
            status: WritingPreferenceDraftReviewStatus::NotFound,
            new_mapping_revision: None,
        }
    }

    fn evidence_row(
        mapping: &AppMapping,
        value: WritingPreferenceValue,
        generation: u64,
        ordinal: usize,
        occurred_at_ms: u64,
        natural_day: &str,
        identity_fingerprint: Option<&str>,
    ) -> WritingPreferenceEvidenceRecord {
        WritingPreferenceEvidenceRecord {
            source_history_id: ordinal as i64 + 1,
            source_revision_id: ordinal as i64 + 1,
            mapping_id: mapping.id.clone(),
            mapping_identity_fingerprint: identity_fingerprint
                .map(str::to_owned)
                .unwrap_or_else(|| mapping.declared_identity_fingerprint()),
            mapping_revision: mapping.preference_revision,
            task_id: mapping.writing_task_id.expect("mapping task"),
            dimension: WritingPreferenceDimension::GreetingTone,
            value,
            origin_id: format!("test-origin-{ordinal}"),
            content_fingerprint: format!("{ordinal:064x}"),
            template_family_fingerprint: Some(format!("{:064x}", ordinal + 1000)),
            occurred_at_ms,
            editor_session_id: if ordinal.is_multiple_of(2) {
                SESSION_ONE.into()
            } else {
                SESSION_TWO.into()
            },
            generation,
            eligible: true,
            natural_day: natural_day.into(),
        }
    }

    #[test]
    fn opposing_evidence_crosses_generations_and_dismissed_generation_does_not_block() {
        let now_ms = now_epoch_millis();
        let map = mapping(
            "mapping-a",
            "Mail app",
            ContextFamily::Email,
            WritingTaskId::Email,
            FocusKind::Email,
            "com.example.mail",
            5,
        );
        let settings = Settings {
            context_enabled: true,
            dictionary_learn_enabled: true,
            local_edit_learning_enabled: true,
            context_mappings: vec![map.clone()],
            ..Settings::default()
        };
        let mut evidence = Vec::new();
        for ordinal in 0..3 {
            evidence.push(evidence_row(
                &map,
                WritingPreferenceValue::RespectfulGreeting,
                1,
                ordinal,
                now_ms.saturating_sub(1_000),
                if ordinal == 0 {
                    "2026-11-10"
                } else {
                    "2026-11-11"
                },
                None,
            ));
        }
        for ordinal in 3..6 {
            evidence.push(evidence_row(
                &map,
                WritingPreferenceValue::ConversationalGreeting,
                1,
                ordinal,
                now_ms.saturating_sub(900),
                if ordinal == 3 {
                    "2026-11-10"
                } else {
                    "2026-11-11"
                },
                None,
            ));
        }
        for ordinal in 6..9 {
            evidence.push(evidence_row(
                &map,
                WritingPreferenceValue::RespectfulGreeting,
                2,
                ordinal,
                now_ms.saturating_sub(800),
                if ordinal == 4 {
                    "2026-11-10"
                } else {
                    "2026-11-11"
                },
                None,
            ));
        }
        let dismissed_generation = WritingPreferenceDraftDecisionRecord {
            candidate_id: "dismissed-respectful-generation-one".into(),
            mapping_id: map.id.clone(),
            mapping_revision: map.preference_revision,
            task_id: WritingTaskId::Email,
            dimension: WritingPreferenceDimension::GreetingTone,
            value: WritingPreferenceValue::RespectfulGreeting,
            generation: 1,
            decision: WritingPreferenceDraftDecision::Dismissed,
            decided_at_ms: now_ms.saturating_sub(500),
        };

        let blocked = visible_candidates(
            &settings,
            &evidence,
            std::slice::from_ref(&dismissed_generation),
            now_ms,
        );
        assert!(
            blocked.is_empty(),
            "undismissed opposing evidence in generation 1 blocks generation 2"
        );

        let old_positive_only = evidence
            .iter()
            .filter(|row| row.value == WritingPreferenceValue::RespectfulGreeting)
            .take(3)
            .cloned()
            .chain(
                evidence
                    .iter()
                    .filter(|row| row.value == WritingPreferenceValue::ConversationalGreeting)
                    .cloned(),
            )
            .collect::<Vec<_>>();
        let opposite_candidate = visible_candidates(
            &settings,
            &old_positive_only,
            std::slice::from_ref(&dismissed_generation),
            now_ms,
        );
        assert_eq!(opposite_candidate.len(), 1);
        assert_eq!(
            opposite_candidate[0].value,
            WritingPreferenceValue::ConversationalGreeting,
            "a dismissed generation closes its old positives under §7.3"
        );

        let mut expired_opposition = (0..3)
            .map(|ordinal| {
                evidence_row(
                    &map,
                    WritingPreferenceValue::RespectfulGreeting,
                    2,
                    ordinal + 10,
                    now_ms.saturating_sub(1_000),
                    if ordinal == 0 {
                        "2026-11-10"
                    } else {
                        "2026-11-11"
                    },
                    None,
                )
            })
            .collect::<Vec<_>>();
        expired_opposition.push(evidence_row(
            &map,
            WritingPreferenceValue::ConversationalGreeting,
            1,
            20,
            now_ms.saturating_sub(14 * DAY_MS + 1),
            "2026-10-20",
            None,
        ));
        let after_expiry = visible_candidates(&settings, &expired_opposition, &[], now_ms);
        assert_eq!(after_expiry.len(), 1);
        assert_eq!(
            after_expiry[0].value,
            WritingPreferenceValue::RespectfulGreeting
        );

        let stale_identity = "mapping:v1:stale-selector";
        let with_stale_opposition = expired_opposition
            .iter()
            .cloned()
            .chain(std::iter::once(evidence_row(
                &map,
                WritingPreferenceValue::ConversationalGreeting,
                1,
                21,
                now_ms.saturating_sub(900),
                "2026-11-11",
                Some(stale_identity),
            )))
            .collect::<Vec<_>>();
        let after_scope_change = visible_candidates(&settings, &with_stale_opposition, &[], now_ms);
        assert_eq!(after_scope_change.len(), 1);
        assert_eq!(
            after_scope_change[0].value,
            WritingPreferenceValue::RespectfulGreeting
        );

        let mut deleted_settings = settings;
        deleted_settings.context_mappings[0].deleted = true;
        assert!(visible_candidates(&deleted_settings, &expired_opposition, &[], now_ms).is_empty());
    }

    #[tokio::test]
    async fn history_edit_candidate_review_owner_chain_persists_and_applies() {
        let isolated = IsolatedDirectory::create();
        let dir = isolated.0.as_path();
        let mapping_a = mapping(
            "mapping-a",
            "Mail app",
            ContextFamily::Email,
            WritingTaskId::Email,
            FocusKind::Email,
            "com.example.mail",
            5,
        );
        let mapping_b = mapping(
            "mapping-b",
            "Project app",
            ContextFamily::ProjectManagement,
            WritingTaskId::ProjectUpdate,
            FocusKind::Document,
            "com.example.project",
            2,
        );
        let initial_settings = Settings {
            context_enabled: true,
            dictionary_learn_enabled: true,
            local_edit_learning_enabled: true,
            context_mappings: vec![mapping_a.clone(), mapping_b.clone()],
            ..Settings::default()
        };
        store::save_configuration(dir, &initial_settings).expect("isolated Settings save");
        let settings_writer = IsolatedSettingsPersistence {
            dir: dir.to_path_buf(),
        };

        let cases = [
            (
                "mapping-a",
                WritingPreferenceDimension::ListStructure,
                WritingPreferenceValue::Bullets,
                "更新接口\n修复测试",
                "- 更新接口\n- 修复测试",
            ),
            (
                "mapping-a",
                WritingPreferenceDimension::ListStructure,
                WritingPreferenceValue::Bullets,
                "整理项目计划\n联系核心产品设计团队\n确认会议时间",
                "- 整理项目计划\n- 联系核心产品设计团队\n- 确认会议时间",
            ),
            (
                "mapping-a",
                WritingPreferenceDimension::ListStructure,
                WritingPreferenceValue::Bullets,
                "补充完整发布文档\n确认团队安排\n更新项目状态\n整理会议记录",
                "- 补充完整发布文档\n- 确认团队安排\n- 更新项目状态\n- 整理会议记录",
            ),
            (
                "mapping-a",
                WritingPreferenceDimension::GreetingTone,
                WritingPreferenceValue::RespectfulGreeting,
                "你好，请稍后确认安排",
                "您好，请稍后确认安排",
            ),
            (
                "mapping-a",
                WritingPreferenceDimension::GreetingTone,
                WritingPreferenceValue::RespectfulGreeting,
                "你好，请确认1号和2号会议时间",
                "您好，请确认1号和2号会议时间",
            ),
            (
                "mapping-a",
                WritingPreferenceDimension::GreetingTone,
                WritingPreferenceValue::RespectfulGreeting,
                "你好，先确认项目安排，再更新团队状态",
                "您好，先确认项目安排，再更新团队状态",
            ),
            (
                "mapping-a",
                WritingPreferenceDimension::TerminalPunctuation,
                WritingPreferenceValue::TerminalPeriod,
                "请确认1号会议安排",
                "请确认1号会议安排。",
            ),
            (
                "mapping-a",
                WritingPreferenceDimension::TerminalPunctuation,
                WritingPreferenceValue::TerminalPeriod,
                "今天先确认时间，再更新项目状态",
                "今天先确认时间，再更新项目状态。",
            ),
            (
                "mapping-a",
                WritingPreferenceDimension::TerminalPunctuation,
                WritingPreferenceValue::TerminalPeriod,
                "请确认第2季度目标、负责人和截止日期",
                "请确认第2季度目标、负责人和截止日期。",
            ),
            (
                "mapping-b",
                WritingPreferenceDimension::ListStructure,
                WritingPreferenceValue::Bullets,
                "Review API\nFix tests",
                "- Review API\n- Fix tests",
            ),
            (
                "mapping-b",
                WritingPreferenceDimension::ListStructure,
                WritingPreferenceValue::Bullets,
                "Update the release plan\nContact the design team\nConfirm timing",
                "- Update the release plan\n- Contact the design team\n- Confirm timing",
            ),
            (
                "mapping-b",
                WritingPreferenceDimension::ListStructure,
                WritingPreferenceValue::Bullets,
                "Add complete status notes to project board\nConfirm owner\nCheck the new build\nNotify the team",
                "- Add complete status notes to project board\n- Confirm owner\n- Check the new build\n- Notify the team",
            ),
        ];
        let now_ms = now_epoch_millis();
        let event_times = [
            now_ms.saturating_sub(7 * DAY_MS),
            now_ms.saturating_sub(4 * DAY_MS),
            now_ms.saturating_sub(60 * 60 * 1_000),
        ];
        let mut ordinal_by_scope = HashMap::<(String, WritingPreferenceDimension), usize>::new();
        let mut evidence_by_scope = HashMap::<
            (String, WritingPreferenceDimension),
            Vec<ManualHistoryPreferenceEvidence>,
        >::new();

        for (case_index, (mapping_id, dimension, _value, before, after)) in cases.iter().enumerate()
        {
            let mapping = if *mapping_id == "mapping-a" {
                &mapping_a
            } else {
                &mapping_b
            };
            let task_id = mapping.writing_task_id.expect("test mapping task");
            let scope_key = (mapping_id.to_string(), *dimension);
            let within_scope = ordinal_by_scope.entry(scope_key.clone()).or_default();
            let occurred_at_ms = event_times[*within_scope];
            let editor_session_id = if *within_scope == 2 {
                SESSION_TWO
            } else {
                SESSION_ONE
            };
            *within_scope += 1;
            let editor_commit = HistoryEditorCommitV1 {
                version: 1,
                editor_session_id: editor_session_id.into(),
                input_kinds: vec![
                    HistoryEditorInputKind::TrustedText,
                    HistoryEditorInputKind::TrustedLineBreak,
                ],
                composing_at_save: false,
                save_action_trusted: true,
            };
            let input_kind = if *mapping_id == "mapping-a" {
                FocusKind::Email
            } else {
                FocusKind::Document
            };
            let snapshot = history_snapshot(mapping, input_kind);
            store::insert_history_with_asr_candidate_and_delivery_and_spool_and_cleanup(
                dir,
                before,
                Some(before),
                None,
                before,
                0.1,
                false,
                None,
                "ok",
                "paste",
                None,
                &snapshot,
                None,
                "isolated-owner-chain",
                "success",
            )
            .expect("synthetic History transaction insert");
            let history_id = store::get_history_page(dir, 100, None, None)
                .expect("read isolated History")
                .items
                .first()
                .expect("inserted History row")
                .id;
            let origin_id = format!("owner-chain-transaction-{case_index:02}");
            let connection = rusqlite::Connection::open(dir.join("history.sqlite"))
                .expect("open only isolated History database");
            connection
                .execute(
                    "UPDATE dictations SET transaction_id = ?1,
                            created_at = datetime(?2 / 1000, 'unixepoch')
                     WHERE id = ?3",
                    rusqlite::params![origin_id, occurred_at_ms as i64, history_id],
                )
                .expect("bind stable transaction origin and original recording time");
            drop(connection);

            let mut saved = store::save_manual_history_revision(dir, history_id, after)
                .expect("explicit History editor save");
            assert!(saved.changed);
            assert!(saved.first_edit_of_original_delivery);
            assert!(saved.source_created_at_ms.is_some());
            assert!(saved.natural_day.is_some());
            assert_eq!(saved.origin_id.as_deref(), Some(origin_id.as_str()));
            let revision_id = saved.revision_id.expect("manual revision id");
            let commit = editor_commit;
            let mut evidence = crate::history_commands::manual_history_preference_evidence(
                dir,
                history_id,
                revision_id,
                &origin_id,
                &saved,
                after,
                Some(&commit),
            )
            .expect("trusted History producer emits weak metadata");
            assert_eq!(evidence.task_id, task_id);
            evidence.occurred_at_ms = now_ms;
            assert_eq!(evidence.natural_day, saved.natural_day.as_deref().unwrap());

            if case_index == 0 {
                let provenance = saved
                    .scene
                    .policy
                    .as_mut()
                    .and_then(|policy| policy.history_preference_provenance.as_mut())
                    .expect("captured provenance");
                let trusted_task = provenance.task_id;
                provenance.task_id = None;
                assert!(crate::history_commands::manual_history_preference_evidence(
                    dir,
                    history_id,
                    revision_id,
                    &origin_id,
                    &saved,
                    after,
                    Some(&commit),
                )
                .is_none());
                saved
                    .scene
                    .policy
                    .as_mut()
                    .and_then(|policy| policy.history_preference_provenance.as_mut())
                    .expect("restore fixture provenance")
                    .task_id = trusted_task;

                let mut mismatched_task = store_input(&evidence);
                mismatched_task.task_id = WritingTaskId::ProjectUpdate;
                assert_eq!(
                    store::replace_writing_preference_evidence(
                        dir,
                        history_id,
                        revision_id,
                        &origin_id,
                        Some(&mismatched_task),
                    )
                    .expect("mismatched task is rejected"),
                    store::WritingPreferenceEvidenceWrite::RejectedSource
                );
            }
            let input = store_input(&evidence);
            assert_eq!(
                store::replace_writing_preference_evidence(
                    dir,
                    history_id,
                    revision_id,
                    &origin_id,
                    Some(&input),
                )
                .expect("metadata evidence write"),
                store::WritingPreferenceEvidenceWrite::Inserted
            );
            evidence_by_scope
                .entry(scope_key)
                .or_default()
                .push(evidence);
        }

        let unsupported_commit = HistoryEditorCommitV1 {
            version: 1,
            editor_session_id: SESSION_ONE.into(),
            input_kinds: vec![HistoryEditorInputKind::TrustedText],
            composing_at_save: false,
            save_action_trusted: true,
        };
        let reclean_origin = "owner-chain-reclean-source";
        let reclean_history = insert_synthetic_history_owner(
            dir,
            &mapping_a,
            "你好，请确认会议时间",
            reclean_origin,
            now_ms.saturating_sub(60 * 60 * 1_000),
        );
        let reclean_snapshot = history_snapshot(&mapping_a, FocusKind::Email);
        let reclean_intent = crate::llm::CleanupIntent::selected_text(
            crate::llm::CleanupOperation::Cleanup,
            "你好，请确认会议时间",
        );
        store::save_reclean_revision_with_facts(
            dir,
            store::RecleanHistorySave {
                dictation_id: reclean_history,
                final_text: "你好，请确认会议时间，稍后更新安排",
                cleanup_status: "ai_success",
                intent: &reclean_intent,
                model: crate::llm::MODEL,
                context_policy: &reclean_snapshot.policy,
                cleanup_rejected_candidate: None,
                cleanup_guard_reason: None,
                cleanup_attempt_kind: Some("success"),
                cleanup_transformations: Some(&[]),
                cleanup_final_guard_reason: None,
                cleanup_failure_reason: None,
            },
        )
        .expect("real History reclean revision is stored before the manual edit");
        let recleaned_manual = store::save_manual_history_revision(
            dir,
            reclean_history,
            "您好，请确认会议时间，稍后更新安排",
        )
        .expect("manual edit after reclean persists");
        assert!(recleaned_manual.changed);
        assert!(!recleaned_manual.first_edit_of_original_delivery);
        let recleaned_revision = recleaned_manual.revision_id.unwrap();
        assert!(crate::history_commands::manual_history_preference_evidence(
            dir,
            reclean_history,
            recleaned_revision,
            reclean_origin,
            &recleaned_manual,
            "您好，请确认会议时间，稍后更新安排",
            Some(&unsupported_commit),
        )
        .is_none());

        let existing_receipt_count = store::list_writing_preference_evidence(dir, now_ms)
            .expect("original-delivery first edits remain eligible")
            .len();
        assert_eq!(existing_receipt_count, cases.len());
        let stale_direct_input = ManualHistoryPreferenceEvidence {
            source_history_id: reclean_history,
            source_revision_id: recleaned_revision,
            origin_id: reclean_origin.into(),
            mapping_id: mapping_a.id.clone(),
            mapping_identity_fingerprint: mapping_a.declared_identity_fingerprint(),
            mapping_revision: mapping_a.preference_revision,
            task_id: WritingTaskId::Email,
            dimension: WritingPreferenceDimension::GreetingTone,
            value: WritingPreferenceValue::RespectfulGreeting,
            occurred_at_ms: now_ms,
            natural_day: recleaned_manual.natural_day.clone().unwrap(),
            editor_session_id: SESSION_TWO.into(),
            content_fingerprint: crate::preference_shadow::content_fingerprint(
                "你好，请确认会议时间",
            )
            .unwrap(),
            template_family_fingerprint: crate::preference_shadow::template_family_fingerprint(
                "你好，请确认会议时间",
            ),
        };
        assert_eq!(
            store::replace_writing_preference_evidence(
                dir,
                reclean_history,
                recleaned_revision,
                reclean_origin,
                Some(&store_input(&stale_direct_input)),
            )
            .expect("SQLite owner independently rejects post-reclean evidence"),
            store::WritingPreferenceEvidenceWrite::RejectedSource
        );
        let evidence_after_stale_direct_write =
            store::list_writing_preference_evidence(dir, now_ms)
                .expect("stale direct write leaves original receipts intact");
        assert_eq!(
            evidence_after_stale_direct_write.len(),
            existing_receipt_count
        );
        let retained_origin = evidence_by_scope
            .values()
            .flatten()
            .next()
            .expect("eligible original-delivery counterpart")
            .origin_id
            .clone();
        assert!(evidence_after_stale_direct_write
            .iter()
            .any(|row| row.origin_id == retained_origin));

        let unsupported_origin = "owner-chain-mixed-edit";
        let unsupported_history = insert_synthetic_history_owner(
            dir,
            &mapping_a,
            "你好，请确认 api 时间",
            unsupported_origin,
            now_ms.saturating_sub(60 * 60 * 1_000),
        );
        let mixed_first =
            store::save_manual_history_revision(dir, unsupported_history, "你好，请确认 API 时间")
                .expect("first unsupported mixed edit persists as a manual History revision");
        assert!(mixed_first.first_edit_of_original_delivery);
        let mixed_first_revision = mixed_first.revision_id.unwrap();
        assert!(crate::history_commands::manual_history_preference_evidence(
            dir,
            unsupported_history,
            mixed_first_revision,
            unsupported_origin,
            &mixed_first,
            "你好，请确认 API 时间",
            Some(&unsupported_commit),
        )
        .is_none());
        store::replace_writing_preference_evidence(
            dir,
            unsupported_history,
            mixed_first_revision,
            unsupported_origin,
            None,
        )
        .expect("unsupported first revision does not create a weak receipt");
        let mixed_second =
            store::save_manual_history_revision(dir, unsupported_history, "您好，请确认 API 时间")
                .expect("later narrow edit persists as another manual History revision");
        assert!(mixed_second.changed);
        assert!(!mixed_second.first_edit_of_original_delivery);
        let mixed_second_revision = mixed_second.revision_id.unwrap();
        assert!(crate::history_commands::manual_history_preference_evidence(
            dir,
            unsupported_history,
            mixed_second_revision,
            unsupported_origin,
            &mixed_second,
            "您好，请确认 API 时间",
            Some(&unsupported_commit),
        )
        .is_none());
        store::replace_writing_preference_evidence(
            dir,
            unsupported_history,
            mixed_second_revision,
            unsupported_origin,
            None,
        )
        .expect("later edit invalidates but does not add weak evidence");

        let old_origin = "owner-chain-old-source";
        let old_history = insert_synthetic_history_owner(
            dir,
            &mapping_a,
            "你好，请稍后确认安排",
            old_origin,
            now_ms.saturating_sub(15 * DAY_MS),
        );
        let old_save =
            store::save_manual_history_revision(dir, old_history, "您好，请稍后确认安排")
                .expect("old History row can still be edited");
        assert!(old_save.first_edit_of_original_delivery);
        assert!(crate::history_commands::manual_history_preference_evidence(
            dir,
            old_history,
            old_save.revision_id.unwrap(),
            old_origin,
            &old_save,
            "您好，请稍后确认安排",
            Some(&unsupported_commit),
        )
        .is_none());

        let near_expiry_mapping = mapping(
            "mapping-near-expiry",
            "Near expiry app",
            ContextFamily::Email,
            WritingTaskId::Email,
            FocusKind::Email,
            "com.example.near-expiry",
            1,
        );
        let near_expiry_origin = "owner-chain-source-expiry";
        let near_expiry_history = insert_synthetic_history_owner(
            dir,
            &near_expiry_mapping,
            "你好，请确认会议时间",
            near_expiry_origin,
            now_ms.saturating_sub(13 * DAY_MS),
        );
        let near_expiry_save =
            store::save_manual_history_revision(dir, near_expiry_history, "您好，请确认会议时间")
                .expect("13-day-old source remains within the capture window");
        let near_expiry_revision = near_expiry_save.revision_id.unwrap();
        let near_expiry_evidence = crate::history_commands::manual_history_preference_evidence(
            dir,
            near_expiry_history,
            near_expiry_revision,
            near_expiry_origin,
            &near_expiry_save,
            "您好，请确认会议时间",
            Some(&unsupported_commit),
        )
        .expect("source can be edited within its original 14-day window");
        assert_eq!(
            store::replace_writing_preference_evidence(
                dir,
                near_expiry_history,
                near_expiry_revision,
                near_expiry_origin,
                Some(&store_input(&near_expiry_evidence)),
            )
            .expect("persist near-expiry metadata"),
            store::WritingPreferenceEvidenceWrite::Inserted
        );
        assert!(store::list_writing_preference_evidence(dir, now_ms)
            .expect("near-expiry source is initially visible")
            .iter()
            .any(|row| row.origin_id == near_expiry_origin));
        assert!(
            store::list_writing_preference_evidence(dir, now_ms.saturating_add(2 * DAY_MS),)
                .expect("source-window expiry is pruned")
                .iter()
                .all(|row| row.origin_id != near_expiry_origin)
        );

        for ((mapping_id, dimension), evidence) in &evidence_by_scope {
            let families = evidence
                .iter()
                .filter_map(|item| item.template_family_fingerprint.as_ref())
                .collect::<BTreeSet<_>>();
            assert_eq!(families.len(), 3, "{mapping_id} {dimension:?} templates");
        }

        let evidence_rows = store::list_writing_preference_evidence(dir, now_ms)
            .expect("read bounded evidence metadata");
        assert_eq!(evidence_rows.len(), cases.len());
        let initial_candidates = visible_candidates(&initial_settings, &evidence_rows, &[], now_ms);
        assert_eq!(initial_candidates.len(), 4);
        assert!(initial_candidates
            .iter()
            .all(|draft| !draft.has_text_sample));
        assert_eq!(
            crate::active_preference_value(
                &initial_settings.context_mappings[0],
                WritingPreferenceDimension::ListStructure,
            ),
            None,
            "weak evidence must not auto-activate before review"
        );

        let mut changed_scope = initial_settings.clone();
        changed_scope.context_mappings[0].bundle_id = Some("com.example.changed".into());
        assert!(
            visible_candidates(&changed_scope, &evidence_rows, &[], now_ms)
                .iter()
                .all(|draft| draft.mapping_id != "mapping-a")
        );

        let first_approval = initial_candidates
            .iter()
            .find(|draft| {
                draft.mapping_id == "mapping-a"
                    && draft.dimension == WritingPreferenceDimension::ListStructure
            })
            .expect("first list candidate");
        let first_request = review_input(first_approval, WritingPreferenceDraftAction::Approve);
        let candidate_views = initial_candidates
            .iter()
            .map(|draft| (draft.candidate_id.clone(), draft.clone()))
            .collect::<HashMap<_, _>>();
        let mut first_results = vec![result_for(&first_request)];
        let mut first_batch = BTreeMap::new();
        first_batch
            .entry(first_approval.mapping_id.clone())
            .or_insert_with(Vec::new)
            .push(0);
        let mut working_mappings = initial_settings.context_mappings.clone();
        commit_review_decisions(
            dir,
            &initial_settings,
            &candidate_views,
            std::slice::from_ref(&first_request),
            &mut first_results,
            &mut working_mappings,
            first_batch,
            now_ms,
            &settings_writer,
        )
        .await
        .expect("first explicit approval commit");
        assert_eq!(
            first_results[0].status,
            WritingPreferenceDraftReviewStatus::Approved
        );

        let after_first_settings = read_isolated_settings(dir);
        let after_first_evidence = store::list_writing_preference_evidence(dir, now_ms)
            .expect("re-read evidence after unrelated preference revision");
        let after_first_decisions = store::writing_preference_draft_decisions(dir, now_ms)
            .expect("re-read durable review marker");
        let remaining_candidates = visible_candidates(
            &after_first_settings,
            &after_first_evidence,
            &after_first_decisions,
            now_ms,
        );
        assert_eq!(remaining_candidates.len(), 3);
        assert!(remaining_candidates.iter().any(|draft| {
            draft.mapping_id == "mapping-a"
                && draft.dimension == WritingPreferenceDimension::GreetingTone
        }));
        assert!(remaining_candidates.iter().any(|draft| {
            draft.mapping_id == "mapping-a"
                && draft.dimension == WritingPreferenceDimension::TerminalPunctuation
        }));
        assert!(remaining_candidates
            .iter()
            .any(|draft| draft.mapping_id == "mapping-b"));
        for candidate in &remaining_candidates {
            let original = initial_candidates
                .iter()
                .find(|item| item.candidate_id == candidate.candidate_id)
                .expect("unaffected candidate keeps its source-set token");
            assert_eq!(candidate.candidate_id, original.candidate_id);
            assert_eq!(
                candidate.mapping_revision,
                after_first_settings
                    .context_mappings
                    .iter()
                    .find(|mapping| mapping.id == candidate.mapping_id)
                    .unwrap()
                    .preference_revision
            );
        }

        // The same production helper now approves two dimensions for mapping A
        // and one candidate for mapping B in a single cumulative Settings batch.
        let second_requests = remaining_candidates
            .iter()
            .map(|draft| review_input(draft, WritingPreferenceDraftAction::Approve))
            .collect::<Vec<_>>();
        let second_views = remaining_candidates
            .iter()
            .map(|draft| (draft.candidate_id.clone(), draft.clone()))
            .collect::<HashMap<_, _>>();
        let mut second_results = second_requests.iter().map(result_for).collect::<Vec<_>>();
        let mut second_batch = BTreeMap::<String, Vec<usize>>::new();
        for (index, request) in second_requests.iter().enumerate() {
            let mapping_id = second_views[&request.candidate_id].mapping_id.clone();
            second_batch.entry(mapping_id).or_default().push(index);
        }
        let mut second_working_mappings = after_first_settings.context_mappings.clone();
        commit_review_decisions(
            dir,
            &after_first_settings,
            &second_views,
            &second_requests,
            &mut second_results,
            &mut second_working_mappings,
            second_batch,
            now_ms,
            &settings_writer,
        )
        .await
        .expect("multi-mapping approval batch commit");
        assert!(second_results
            .iter()
            .all(|result| result.status == WritingPreferenceDraftReviewStatus::Approved));

        let reloaded = read_isolated_settings(dir);
        let map_a = reloaded
            .context_mappings
            .iter()
            .find(|mapping| mapping.id == "mapping-a")
            .expect("mapping A persisted");
        let map_b = reloaded
            .context_mappings
            .iter()
            .find(|mapping| mapping.id == "mapping-b")
            .expect("mapping B persisted cumulatively");
        assert_eq!(map_a.preference_revision, mapping_a.preference_revision + 2);
        assert_eq!(map_b.preference_revision, mapping_b.preference_revision + 1);
        assert_eq!(
            crate::active_preference_value(map_a, WritingPreferenceDimension::ListStructure),
            Some(WritingPreferenceValue::Bullets)
        );
        assert_eq!(
            crate::active_preference_value(map_a, WritingPreferenceDimension::GreetingTone),
            Some(WritingPreferenceValue::RespectfulGreeting)
        );
        assert_eq!(
            crate::active_preference_value(map_a, WritingPreferenceDimension::TerminalPunctuation,),
            Some(WritingPreferenceValue::TerminalPeriod)
        );
        assert_eq!(
            crate::active_preference_value(map_b, WritingPreferenceDimension::ListStructure),
            Some(WritingPreferenceValue::Bullets)
        );
        assert!(map_a
            .writing_preferences
            .iter()
            .filter(|preference| preference.status == WritingPreferenceStatus::Active)
            .all(|preference| preference.source == WritingPreferenceSource::ReviewedHistory));
        assert!(!map_a.preference_guidance_provider_approved);
        assert!(map_a.style_examples_approved);
        assert!(!map_b.preference_guidance_provider_approved);

        let final_evidence = store::list_writing_preference_evidence(dir, now_ms)
            .expect("candidate evidence remains bounded after approval");
        let final_decisions = store::writing_preference_draft_decisions(dir, now_ms)
            .expect("per-item decisions persist");
        assert_eq!(final_decisions.len(), 4);
        assert!(
            visible_candidates(&reloaded, &final_evidence, &final_decisions, now_ms).is_empty()
        );

        let output_source = "The three tasks are: update API; fix tests; review docs.";
        let projection = crate::cleanup_output_resolver::resolve_cleanup_projection(
            crate::cleanup_output_resolver::CleanupOutputFacts {
                intensity: crate::llm::CleanupIntensity::Standard,
                output_mode: crate::cleanup_prompt::OutputMode::Auto,
                auto_scene: crate::cleanup_prompt::AutoScene::IssueBody,
                operation: crate::cleanup_output_resolver::OutputOperation::OrdinaryCleanup,
                model_request_eligible: true,
                output_mode_compatible: true,
                multiline_capable: true,
                focus_kind: FocusKind::Document,
                preferred_bullets: crate::active_preference_value(
                    map_a,
                    WritingPreferenceDimension::ListStructure,
                ) == Some(WritingPreferenceValue::Bullets),
                allow_model_selected_paragraphs: false,
            },
            output_source,
        )
        .expect("compatible local output resolves")
        .expect("reviewed preference affects a compatible next output");
        assert_eq!(
            projection.plan.mode_source,
            crate::cleanup_prompt::ModeSource::Preference
        );
        assert!(projection.projection.text.contains("- update API"));

        let exported = store::export_history_json(dir).expect("History export remains available");
        assert!(!exported.contains("candidate_id"));
        assert!(!exported.contains("writing_preference_evidence"));

        store::clear_all_data(dir).expect("clear isolated History and weak metadata");
        assert!(store::list_writing_preference_evidence(dir, now_ms)
            .expect("cleared evidence query")
            .is_empty());
        assert!(store::writing_preference_draft_decisions(dir, now_ms)
            .expect("cleared decisions query")
            .is_empty());
        let explicit_settings_after_clear = read_isolated_settings(dir);
        assert_eq!(
            crate::active_preference_value(
                explicit_settings_after_clear
                    .context_mappings
                    .iter()
                    .find(|mapping| mapping.id == "mapping-a")
                    .unwrap(),
                WritingPreferenceDimension::ListStructure,
            ),
            Some(WritingPreferenceValue::Bullets),
            "Clear All removes evidence/decisions but keeps reviewed user settings"
        );

        // The direct merge owner also drops an older cloud preference grant
        // when a fresh History-reviewed tone is added, while retaining style consent.
        let mut previously_consented = mapping_a.clone();
        previously_consented.preference_guidance_provider_approved = true;
        previously_consented
            .writing_preferences
            .push(WritingPreference {
                task_id: Some(WritingTaskId::Email),
                dimension: WritingPreferenceDimension::TerminalPunctuation,
                value: WritingPreferenceValue::TerminalPeriod,
                status: WritingPreferenceStatus::Active,
                version: 1,
                source: WritingPreferenceSource::Manual,
            });
        let greeting = initial_candidates
            .iter()
            .find(|draft| draft.dimension == WritingPreferenceDimension::GreetingTone)
            .expect("greeting candidate fixture");
        let revoked = merge_reviewed_preferences(&previously_consented, &[greeting]).unwrap();
        assert!(!revoked.preference_guidance_provider_approved);
        assert!(revoked.style_examples_approved);
    }
}
