//! History read, export, retry, delete, and re-clean IPC commands.

use crate::mlx_diagnostics::{self, EventName, Severity, Target};
use crate::{
    asr, asr_request_snapshot_for_options_with_metrics, chrono_like_id, chunker,
    cleanup_failure_status, clipboard_text_for_snippets, context, finalize_text_for_scene, lexicon,
    llm, local_cleanup_or_raw_for_resolved_scene, lock_recover, metrics, paste, queue,
    release_operation, snippets, store, try_claim_operation, AppState, CleanupDecision,
    FinalizationContext, OperationLease, CLEANUP_STATUS_AI_SUCCESS,
    CLEANUP_STATUS_CRITICAL_MEMORY_PRESSURE, CLEANUP_STATUS_LOCAL_MEMORY_UNAVAILABLE,
    CLEANUP_STATUS_LOCAL_ONLY, CLEANUP_STATUS_PRESERVATION_GUARD, CLEANUP_STATUS_SNIPPET_BYPASS,
};
use serde::Serialize;
use std::time::Instant;
use tauri::{Emitter, Manager, State};
use tokio_util::sync::CancellationToken;

struct HistoryMutationOperationGuard {
    app: tauri::AppHandle,
}

impl Drop for HistoryMutationOperationGuard {
    fn drop(&mut self) {
        let state = self.app.state::<AppState>();
        release_operation(&state, OperationLease::HistoryMutation);
    }
}

fn claim_history_mutation(
    app: &tauri::AppHandle,
    state: &AppState,
) -> Result<HistoryMutationOperationGuard, String> {
    if !try_claim_operation(state, OperationLease::HistoryMutation) {
        return Err("History is busy while another VoiceFlow operation is active.".into());
    }
    Ok(HistoryMutationOperationGuard { app: app.clone() })
}

#[derive(Clone)]
struct HistoryProcessingGuard {
    generation: u64,
    cancellation: CancellationToken,
}

fn capture_history_processing_guard(state: &AppState) -> HistoryProcessingGuard {
    loop {
        let generation = state
            .processing_configuration_generation
            .load(std::sync::atomic::Ordering::Acquire);
        let cancellation = lock_recover(&state.history_processing_cancellation).child_token();
        if generation
            == state
                .processing_configuration_generation
                .load(std::sync::atomic::Ordering::Acquire)
            && !cancellation.is_cancelled()
        {
            return HistoryProcessingGuard {
                generation,
                cancellation,
            };
        }
        cancellation.cancel();
    }
}

fn with_current_history_processing<T>(
    state: &AppState,
    guard: &HistoryProcessingGuard,
    action: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let _active_generation = lock_recover(&state.history_processing_cancellation);
    if guard.cancellation.is_cancelled()
        || guard.generation
            != state
                .processing_configuration_generation
                .load(std::sync::atomic::Ordering::Acquire)
    {
        return Err("History operation cancelled because processing settings changed".into());
    }
    action()
}

fn ensure_history_processing_current(
    state: &AppState,
    guard: &HistoryProcessingGuard,
) -> Result<(), String> {
    with_current_history_processing(state, guard, || Ok(()))
}

#[tauri::command]
pub(crate) fn get_history(
    app: tauri::AppHandle,
    before_id: Option<i64>,
    limit: Option<i64>,
    query: Option<String>,
) -> Result<store::HistoryPage, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    store::get_history_page(&dir, limit.unwrap_or(50), before_id, query.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn export_history(app: tauri::AppHandle) -> Result<String, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let json = store::export_history_json(&dir).map_err(|e| e.to_string())?;
    let downloads = app.path().download_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&downloads).map_err(|e| e.to_string())?;
    let path = downloads.join(format!("voiceflow-history-{}.json", chrono_like_id()));
    store::write_export_file(&path, &json).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
pub(crate) fn export_gold_corpus(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let downloads = app.path().download_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&downloads).map_err(|e| e.to_string())?;
    let language = lock_recover(&state.settings).language.clone();
    let exported =
        store::export_gold_corpus(&dir, &downloads, &language).map_err(|e| e.to_string())?;
    Ok(exported.directory)
}

#[tauri::command]
pub(crate) fn repaste_history(id: i64, app: tauri::AppHandle) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let text = store::history_text(&dir, id).map_err(|e| e.to_string())?;
    if text.trim().is_empty() {
        return Err("这条历史记录没有可恢复的文字".into());
    }
    // History has no trustworthy original target guard. It is intentionally
    // clipboard-only and never injects into the currently focused app.
    paste::copy(&app, &text).map_err(|e| e.to_string())?;
    let _ = app.emit(
        "history://copied",
        serde_json::json!({ "message": "已复制，请手动粘贴" }),
    );
    Ok(())
}

fn cleanup_operation_from_name(value: &str) -> Option<llm::CleanupOperation> {
    match value {
        "cleanup" => Some(llm::CleanupOperation::Cleanup),
        "rewrite" => Some(llm::CleanupOperation::Rewrite),
        "shorten" => Some(llm::CleanupOperation::Shorten),
        "formalize" => Some(llm::CleanupOperation::Formalize),
        "casualize" => Some(llm::CleanupOperation::Casualize),
        "translate" => Some(llm::CleanupOperation::Translate),
        _ => None,
    }
}

fn apply_current_style_approval(
    policy: &mut context::ContextPolicy,
    profile_id: Option<&str>,
    settings: &store::Settings,
) {
    policy.style_examples_approved = false;
    policy.style_example_input = None;
    policy.style_example_output = None;
    policy.style_example_pairs.clear();
    if !settings.context_enabled {
        return;
    }
    let Some(mapping) = profile_id
        .and_then(|id| lexicon::mapping_for_profile(&settings.context_mappings, id))
        .filter(|mapping| {
            mapping.enabled
                && mapping.style_examples_approved
                && !lexicon::is_default_learn_off_target(
                    mapping.bundle_id.as_deref(),
                    mapping.browser_host.as_deref(),
                )
        })
    else {
        return;
    };
    policy.style_examples_approved = true;
    policy.style_example_input = mapping.style_example_input.clone();
    policy.style_example_output = mapping.style_example_output.clone();
    policy.style_example_pairs = mapping
        .style_example_pairs
        .iter()
        .take(3)
        .cloned()
        .collect();
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ManualHistorySaveResponse {
    revision_id: Option<i64>,
    changed: bool,
    learning: crate::dictionary_learn::ManualHistoryLearningStatus,
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn save_history_revision(
    id: i64,
    final_text: String,
    revision_reason: Option<String>,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<ManualHistorySaveResponse, String> {
    // Keep data-clear admission closed through the associated optional
    // dictionary-learning write and every early-return path below.
    let _operation = claim_history_mutation(&app, &state)?;
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let reason = revision_reason.as_deref().unwrap_or("manual_edit");
    if reason == "manual_edit" {
        let saved = store::save_manual_history_revision(&dir, id, &final_text)
            .map_err(|error| error.to_string())?;
        let learning = if let Some(revision_id) = saved.revision_id {
            if saved.learning_before_text == final_text {
                crate::dictionary_learn::ManualHistoryLearningStatus::NotApplicable
            } else {
                crate::dictionary_learn::learn_from_manual_history_edit(
                    &app,
                    &state,
                    id,
                    revision_id,
                    &saved.learning_before_text,
                    &final_text,
                    &saved.scene,
                )
                .await
                .map_err(|_| "history_dictionary_learning_retry".to_string())?
            }
        } else {
            crate::dictionary_learn::ManualHistoryLearningStatus::NotApplicable
        };
        Ok(ManualHistorySaveResponse {
            revision_id: saved.revision_id,
            changed: saved.changed,
            learning,
        })
    } else {
        let revision_id =
            store::save_history_revision(&dir, id, &final_text, None, None, None, None, reason)
                .map_err(|error| error.to_string())?;
        Ok(ManualHistorySaveResponse {
            revision_id: Some(revision_id),
            changed: true,
            learning: crate::dictionary_learn::ManualHistoryLearningStatus::NotApplicable,
        })
    }
}

#[tauri::command]
pub(crate) fn get_history_revisions(
    id: i64,
    app: tauri::AppHandle,
) -> Result<Vec<store::HistoryRevision>, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    store::get_history_revisions(&dir, id).map_err(|e| e.to_string())
}

fn reclean_delivery_after_finalization(
    degraded: bool,
    copy_if_successful: impl FnOnce() -> store::RecleanDelivery,
) -> Option<store::RecleanDelivery> {
    if degraded {
        None
    } else {
        Some(copy_if_successful())
    }
}

fn reclean_delivery_log_outcome(delivery: Option<store::RecleanDelivery>) -> &'static str {
    match delivery {
        Some(store::RecleanDelivery::Copied) => "copied",
        Some(store::RecleanDelivery::ClipboardChanged) => "clipboard_changed",
        Some(store::RecleanDelivery::ClipboardOwnershipUnverified) => {
            "clipboard_ownership_unverified"
        }
        Some(store::RecleanDelivery::ClipboardWriteFailed) => "clipboard_write_failed",
        None => "not_attempted_degraded",
    }
}

fn critical_retry_decision(
    error: &llm::LlmError,
    source_transcript: &str,
) -> Option<CleanupDecision> {
    llm::is_critical_memory_pressure(error)
        .then(|| CleanupDecision::CriticalMemoryPressure(source_transcript.to_owned()))
}

fn memory_unavailable_retry_decision(
    error: &llm::LlmError,
    source_transcript: &str,
) -> Option<CleanupDecision> {
    llm::is_local_memory_unavailable(error)
        .then(|| CleanupDecision::MemoryUnavailable(source_transcript.to_owned()))
}

fn is_critical_reclean_error(error: &queue::ExecuteError<llm::LlmError>) -> bool {
    matches!(error, queue::ExecuteError::Operation(error) if llm::is_critical_memory_pressure(error))
}

fn history_reclean_asr_text(dir: &std::path::Path, id: i64) -> Result<String, String> {
    store::history_asr_text(dir, id)
        .map_err(|e| e.to_string())?
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| "这条历史记录没有可重新整理的原文".to_owned())
}

fn apply_current_history_retry_output_mode(
    policy: &mut context::ContextPolicy,
    settings: &store::Settings,
) {
    policy.output_mode = (settings.output_mode != "auto").then(|| settings.output_mode.clone());
    policy.translation_target_language = (settings.output_mode == "translation")
        .then(|| settings.translation_target_language.clone());
    policy.cleanup_output_plan = None;
}

struct HistoryRetryOutputProjectionInput<'a> {
    settings: &'a store::Settings,
    profile_id: Option<&'a str>,
    family: context::ContextFamily,
    input_kind: context::FocusKind,
    scene: crate::ResolvedCleanupScene,
    intent: &'a llm::CleanupIntent,
    route_effort: llm::CleanupEffort,
    source: &'a str,
}

fn history_retry_output_projection(
    input: HistoryRetryOutputProjectionInput<'_>,
) -> Option<crate::cleanup_output_resolver::ResolvedCleanupProjection> {
    let HistoryRetryOutputProjectionInput {
        settings,
        profile_id,
        family,
        input_kind,
        scene,
        intent,
        route_effort,
        source,
    } = input;
    use crate::cleanup_output_resolver::{CleanupOutputFacts, OutputOperation};
    use crate::cleanup_prompt::OutputMode;

    let operation = match intent.operation {
        llm::CleanupOperation::Cleanup => OutputOperation::OrdinaryCleanup,
        llm::CleanupOperation::Translate => OutputOperation::AuthorizedTranslation,
        _ => OutputOperation::Other,
    };
    if operation == OutputOperation::Other {
        return None;
    }

    // Normal retry routes already carry the resolved scene strength. Spoken
    // cleanup commands use Command as their route effort, so consult the same
    // saved scene with an implicit cleanup only to recover its existing
    // strength; this does not change route eligibility or operation intent.
    let intensity = route_effort.as_intensity().or_else(|| {
        let implicit = llm::CleanupIntent::implicit(source);
        match crate::cleanup_route_for_resolved_scene(settings, scene, &implicit) {
            lexicon::CleanupRoute::Provider(effort) => effort.as_intensity(),
            lexicon::CleanupRoute::LocalOnly => None,
        }
    })?;
    let output_mode = OutputMode::parse(&settings.output_mode).unwrap_or(OutputMode::Auto);
    let auto_scene = crate::cleanup_output_resolver::auto_scene_for_context(
        profile_id.unwrap_or_default(),
        family,
        input_kind,
    );
    let facts = CleanupOutputFacts {
        intensity,
        output_mode,
        auto_scene,
        operation,
        model_request_eligible: true,
        // Detached History no longer has an observed AX target or multiline
        // capability. Keep the Auto add-on and decline all inferred layouts.
        output_mode_compatible: false,
        multiline_capable: false,
        focus_kind: input_kind,
    };
    let resolved = crate::cleanup_output_resolver::resolve_cleanup_projection(facts, source)
        .ok()
        .flatten()?;
    (resolved.projection.text == source).then_some(resolved)
}

#[tauri::command]
pub(crate) async fn reclean_history(
    id: i64,
    operation: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let reclean_started = Instant::now();
    mlx_diagnostics::event(EventName::HistoryReclean, Severity::Info, Target::History)
        .source_history_id(Some(id))
        .phase("begin")
        .emit();
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let processing_guard = capture_history_processing_guard(&state);
    let raw_text = history_reclean_asr_text(&dir, id)?;
    let operation =
        cleanup_operation_from_name(&operation).ok_or_else(|| "不支持的重新整理模式".to_owned())?;
    let settings = lock_recover(&state.settings).clone();
    let scene = store::history_scene(&dir, id).map_err(|e| e.to_string())?;
    let mut policy = scene.policy.clone().unwrap_or_default();
    apply_current_style_approval(&mut policy, scene.profile_id.as_deref(), &settings);
    if settings.output_mode != "auto" {
        policy.output_mode = Some(settings.output_mode.clone());
    }
    if settings.output_mode == "translation" {
        policy.translation_target_language = Some(settings.translation_target_language.clone());
    }
    let family = family_from_scene(&scene, &settings.context_mappings);
    let input_kind = policy.input_kind;
    let resolved_scene = crate::ResolvedCleanupScene::observed(family, input_kind)
        .with_codex_transcript_prompt(
            scene.profile_id.as_deref(),
            scene.native_bundle.as_deref(),
            &raw_text,
        );
    crate::apply_resolved_scene_to_policy(&mut policy, resolved_scene);
    let mut policy_without_examples = policy.clone();
    policy_without_examples.style_examples_approved = false;
    policy_without_examples.style_example_input = None;
    policy_without_examples.style_example_output = None;
    policy_without_examples.style_example_pairs.clear();
    let profile_confidence = if scene.profile_id.is_some() {
        0.88
    } else {
        0.0
    };
    let mut prepared = crate::prepare_cleanup_transcript_for_resolved_scene(
        Some(&dir),
        &settings,
        &raw_text,
        resolved_scene,
        profile_confidence,
        Some(&scene.learn_scope()),
        false,
    );
    // Reclean starts from the provider's original ASR transcript. The local
    // preparation owner still consumes it as selected text, so spoken command
    // cues are not reinterpreted during a History operation.
    prepared.intent = llm::CleanupIntent::selected_text(operation, &prepared.text);
    let prepared_text = prepared.text.clone();
    let pairs_hint = prepared.pairs_hint.clone();
    let intent = prepared.intent.clone();
    if !try_claim_operation(&state, OperationLease::HistoryReclean) {
        return Err("Dictation is active; try history cleanup again after it finishes".into());
    }
    if let Err(error) = ensure_history_processing_current(&state, &processing_guard) {
        release_operation(&state, OperationLease::HistoryReclean);
        return Err(error);
    }
    // Capture before the provider wait. Reclean may only copy if the
    // pasteboard is still owned by this operation when its result is ready.
    let clipboard_lease = paste::RetryClipboardLease::capture(&app);
    let mut native_input_capacity = None;
    let mut native_request_id = None;
    let mut cleanup_attempt_kind = Some("not_called".to_owned());
    let mut cleanup_transformations = Some(Vec::new());
    let mut cleanup_failure_reason: Option<String> = None;
    let cleanup_decision = if !settings.cleanup_enabled {
        CleanupDecision::Disabled
    } else {
        let cleanup_endpoint = settings.cleanup_endpoint();
        let cleanup_model = settings.cleanup_request_model();
        let cleanup_key = settings.cleanup_credential().to_owned();
        let profile_id = scene.profile_id.clone();
        let policy_base = policy_without_examples.clone();
        let app_state = state.inner();
        let cleanup_scope = crate::cleanup_quota_scope(&settings);
        let result = queue::execute_with_retry_scoped_cancelled_background(
            &app_state.gate,
            queue::RequestKind::HistoryLlm,
            &cleanup_scope,
            || {
                let current_settings = lock_recover(&app_state.settings).clone();
                let mut attempt_policy = policy_base.clone();
                apply_current_style_approval(
                    &mut attempt_policy,
                    profile_id.as_deref(),
                    &current_settings,
                );
                let sent_signature = crate::cleanup_projection_signature(None, &attempt_policy);
                let cleanup_endpoint = cleanup_endpoint.clone();
                let cleanup_model = cleanup_model.clone();
                let cleanup_key = cleanup_key.clone();
                let cleanup_provider = settings.cleanup_provider;
                let custom_cleanup_parameters = (cleanup_provider
                    == crate::providers::EngineProvider::Custom)
                    .then(|| current_settings.custom_cleanup_parameters.clone());
                let prepared_text = prepared_text.clone();
                let pairs_hint = pairs_hint.clone();
                let language = settings.language.clone();
                let intent = intent.clone();
                let policy_base = policy_base.clone();
                let profile_id = profile_id.clone();
                async move {
                    // The selected raw row is already represented by the
                    // prepared transcript and its local proof vector. Do
                    // not send it a second time as an untrusted reference:
                    // that would spend prompt budget and reintroduce text
                    // the preparation owner intentionally resolved.
                    let result = crate::with_cleanup_warmup_model_demand(
                        app_state,
                        cleanup_provider,
                        llm::cleanup_with_provider_and_native_runtime_and_intent_and_source(
                            cleanup_provider,
                            &app_state.mlx_cleanup_runtime,
                            &cleanup_endpoint,
                            &cleanup_model,
                            &prepared_text,
                            &cleanup_key,
                            &[],
                            None,
                            Some(&attempt_policy),
                            Some(language.as_str()),
                            None,
                            Some(&intent),
                            pairs_hint.as_deref(),
                            llm::CleanupEffort::Command,
                            None,
                            None,
                            Some(id),
                            custom_cleanup_parameters.as_ref(),
                        ),
                    )
                    .await
                    .map(|(reply, limits)| {
                        (
                            reply.accepted_text,
                            reply.native_request_id,
                            reply.transformations,
                            limits,
                        )
                    });
                    if result.is_ok() {
                        let latest_settings = lock_recover(&app_state.settings).clone();
                        let mut latest_policy = policy_base.clone();
                        apply_current_style_approval(
                            &mut latest_policy,
                            profile_id.as_deref(),
                            &latest_settings,
                        );
                        if crate::cleanup_projection_signature(None, &latest_policy)
                            != sent_signature
                        {
                            return Err(llm::LlmError::ContextAuthorizationChanged);
                        }
                    }
                    result
                }
            },
            processing_guard.cancellation.clone(),
        )
        .await;
        match result {
            Ok((text, request_id, transformations, limits)) => {
                state.gate.update_llm_for(&cleanup_scope, &limits);
                native_request_id = request_id;
                cleanup_attempt_kind = Some("success".to_owned());
                cleanup_transformations = Some(transformations);
                CleanupDecision::Provider(text)
            }
            Err(error) => {
                if matches!(&error, queue::ExecuteError::Cancelled) {
                    release_operation(&state, OperationLease::HistoryReclean);
                    return Err(
                        "History cleanup cancelled because processing settings changed".into(),
                    );
                }
                if is_critical_reclean_error(&error) {
                    release_operation(&state, OperationLease::HistoryReclean);
                    return Err(
                        "System memory pressure is critical; selected text and existing History were left unchanged"
                            .into(),
                    );
                }
                if matches!(&error, queue::ExecuteError::Operation(error) if llm::is_local_memory_unavailable(error))
                {
                    release_operation(&state, OperationLease::HistoryReclean);
                    return Err(
                        "Local memory budget is unavailable; selected text and existing History were left unchanged".into(),
                    );
                }
                native_input_capacity = match &error {
                    queue::ExecuteError::Operation(llm::LlmError::NativeLocal(failure)) => {
                        failure.input_capacity()
                    }
                    _ => None,
                };
                if let queue::ExecuteError::Operation(operation_error) = &error {
                    cleanup_attempt_kind = Some(operation_error.history_attempt_kind().to_owned());
                    cleanup_failure_reason = Some(operation_error.history_failure_reason());
                    cleanup_transformations = None;
                }
                let cleanup_rejection = match &error {
                    queue::ExecuteError::Operation(error) => llm::cleanup_rejection(error),
                    queue::ExecuteError::Cancelled => None,
                };
                let guard_rejected = matches!(
                    &error,
                    queue::ExecuteError::Operation(error)
                        if llm::is_preservation_guard_error(error)
                );
                log::warn!(
                    "history re-clean failed; using local cleanup: {}",
                    match &error {
                        queue::ExecuteError::Operation(error) => error.log_summary(),
                        queue::ExecuteError::Cancelled => "cancelled".to_owned(),
                    }
                );
                if let Some(fact) = cleanup_rejection {
                    CleanupDecision::CleanupRejected(fact)
                } else if guard_rejected {
                    CleanupDecision::GuardRejected
                } else {
                    CleanupDecision::Failed
                }
            }
        }
    };
    let finalizer_started = Instant::now();
    let finalized = match crate::finalize_text_for_scene(
        &prepared_text,
        cleanup_decision,
        FinalizationContext {
            scene: prepared.scene,
            operation,
            revision_source: Some(&prepared.revision_source),
            prepared_transcript: Some(&prepared.text),
            revision_authorizations: &prepared.revision_authorizations,
            promoted_pair_protections: &prepared.promoted_pair_protections,
            list_revision_proof: prepared.list_revision_proof.as_ref(),
            structure_safe_source: None,
            output_projection_list_revision_proof: None,
            layout_source: None,
            exact_fallback: false,
        },
    ) {
        Ok(finalized) => {
            mlx_diagnostics::event(
                EventName::HistoryRecleanFinalizer,
                Severity::Info,
                Target::History,
            )
            .request_id(native_request_id.as_deref().unwrap_or(""))
            .source_history_id(Some(id))
            .outcome("completed")
            .degraded(finalized.degraded)
            .elapsed_ms(finalizer_started.elapsed().as_secs_f64() * 1_000.0)
            .emit();
            finalized
        }
        Err(_) => {
            mlx_diagnostics::event(
                EventName::HistoryRecleanFinalizer,
                Severity::Warn,
                Target::History,
            )
            .request_id(native_request_id.as_deref().unwrap_or(""))
            .source_history_id(Some(id))
            .outcome("failed")
            .elapsed_ms(finalizer_started.elapsed().as_secs_f64() * 1_000.0)
            .emit();
            release_operation(&state, OperationLease::HistoryReclean);
            return Err("History cleanup returned no usable text".into());
        }
    };
    let final_text = finalized.text;
    let degraded = finalized.degraded;
    let degraded_reason = if native_input_capacity.is_some() {
        Some("native_input_too_large")
    } else {
        finalized.degraded_reason
    };
    let cleanup_final_guard_reason = finalized
        .final_guard_reason
        .map(|reason| llm::CleanupRejectionStage::FinalOutput.storage_code(reason));
    if cleanup_failure_reason.is_none() && finalized.degraded_reason == Some("llm_cleanup_empty") {
        cleanup_failure_reason = Some("empty_completion".to_owned());
    }
    if cleanup_failure_reason.is_none() {
        cleanup_failure_reason = cleanup_final_guard_reason.clone();
    }
    let cleanup_rejection = finalized.cleanup_rejection;
    if cleanup_rejection
        .as_ref()
        .is_some_and(|fact| matches!(fact.stage, llm::CleanupRejectionStage::FinalOutput))
        && cleanup_attempt_kind.as_deref() == Some("success")
    {
        cleanup_attempt_kind = Some("final_guard".to_owned());
    }
    let cleanup_status = if !settings.cleanup_enabled {
        CLEANUP_STATUS_LOCAL_ONLY
    } else if !degraded {
        CLEANUP_STATUS_AI_SUCCESS
    } else if degraded_reason == Some("preservation_guard") {
        CLEANUP_STATUS_PRESERVATION_GUARD
    } else {
        cleanup_failure_status(&prepared_text, &final_text)
    };
    if degraded_reason == Some("preservation_guard") {
        state.metrics.record_cleanup_guard_fallback();
    }
    let mut persisted_revision_id = None;
    let mut clipboard_elapsed_seconds = None;
    let mut revision_state_elapsed_seconds = None;
    let output_result = with_current_history_processing(&state, &processing_guard, || {
        let revision_started = Instant::now();
        let cleanup_guard_reason = cleanup_rejection
            .as_ref()
            .map(|fact| fact.stage.storage_code(fact.reason));
        let revision_id = store::save_reclean_revision_with_facts(
            &dir,
            store::RecleanHistorySave {
                dictation_id: id,
                final_text: &final_text,
                cleanup_status,
                intent: &intent,
                model: &settings.cleanup_model,
                context_policy: &policy,
                cleanup_rejected_candidate: cleanup_rejection
                    .as_ref()
                    .map(|fact| fact.candidate.as_str()),
                cleanup_guard_reason: cleanup_guard_reason.as_deref(),
                cleanup_attempt_kind: cleanup_attempt_kind.as_deref(),
                cleanup_transformations: cleanup_transformations.as_deref(),
                cleanup_final_guard_reason: cleanup_final_guard_reason.as_deref(),
                cleanup_failure_reason: cleanup_failure_reason.as_deref(),
            },
        )
        .map_err(|error| error.to_string())?;
        persisted_revision_id = Some(revision_id);
        mlx_diagnostics::event(
            EventName::HistoryRecleanRevision,
            Severity::Info,
            Target::History,
        )
        .request_id(native_request_id.as_deref().unwrap_or(""))
        .source_history_id(Some(id))
        .revision_id(revision_id)
        .outcome("saved")
        .db_write_ms(revision_started.elapsed().as_secs_f64() * 1_000.0)
        .history_timestamp_facts()
        .emit();
        let clipboard_started = Instant::now();
        let delivery = reclean_delivery_after_finalization(degraded, || {
            match clipboard_lease.copy_if_unchanged(&app, &final_text) {
                paste::RetryClipboardOutcome::Copied => store::RecleanDelivery::Copied,
                paste::RetryClipboardOutcome::Changed => store::RecleanDelivery::ClipboardChanged,
                paste::RetryClipboardOutcome::OwnershipUnverified => {
                    store::RecleanDelivery::ClipboardOwnershipUnverified
                }
                paste::RetryClipboardOutcome::WriteFailed => {
                    store::RecleanDelivery::ClipboardWriteFailed
                }
            }
        });
        clipboard_elapsed_seconds = delivery.map(|_| clipboard_started.elapsed().as_secs_f64());
        mlx_diagnostics::event(
            EventName::HistoryRecleanClipboard,
            Severity::Info,
            Target::History,
        )
        .request_id(native_request_id.as_deref().unwrap_or(""))
        .source_history_id(Some(id))
        .revision_id(revision_id)
        .delivery_outcome(reclean_delivery_log_outcome(delivery))
        .clipboard_ms(clipboard_elapsed_seconds.map(|seconds| seconds * 1_000.0))
        .emit();
        let revision_state_started = Instant::now();
        store::update_history_revision_state_with_reclean_delivery(
            &dir,
            id,
            degraded,
            degraded_reason,
            Some(cleanup_status),
            cleanup_rejection
                .as_ref()
                .map(|fact| fact.candidate.as_str()),
            cleanup_guard_reason.as_deref(),
            delivery,
            cleanup_attempt_kind.as_deref(),
            cleanup_transformations.as_deref(),
            cleanup_final_guard_reason.as_deref(),
            cleanup_failure_reason.as_deref(),
        )
        .map_err(|error| error.to_string())?;
        revision_state_elapsed_seconds = Some(revision_state_started.elapsed().as_secs_f64());
        mlx_diagnostics::event(
            EventName::HistoryRecleanRevisionState,
            Severity::Info,
            Target::History,
        )
        .request_id(native_request_id.as_deref().unwrap_or(""))
        .source_history_id(Some(id))
        .revision_id(revision_id)
        .outcome("updated")
        .db_write_ms(revision_state_started.elapsed().as_secs_f64() * 1_000.0)
        .emit();
        Ok(())
    });
    if let Err(error) = output_result {
        mlx_diagnostics::event(
            EventName::HistoryRecleanTerminal,
            Severity::Warn,
            Target::History,
        )
        .request_id(native_request_id.as_deref().unwrap_or(""))
        .source_history_id(Some(id))
        .optional_revision_id(persisted_revision_id)
        .outcome("persistence_failed")
        .clipboard_ms(clipboard_elapsed_seconds.map(|seconds| seconds * 1_000.0))
        .state_write_ms(revision_state_elapsed_seconds.map(|seconds| seconds * 1_000.0))
        .request_total_ms(reclean_started.elapsed().as_secs_f64() * 1_000.0)
        .emit();
        release_operation(&state, OperationLease::HistoryReclean);
        return Err(error);
    }
    let _ = app.emit(
        "history://recleaned",
        serde_json::json!({ "dictation_id": id, "degraded": degraded }),
    );
    mlx_diagnostics::event(
        EventName::HistoryRecleanTerminal,
        Severity::Info,
        Target::History,
    )
    .request_id(native_request_id.as_deref().unwrap_or(""))
    .source_history_id(Some(id))
    .optional_revision_id(persisted_revision_id)
    .outcome("completed")
    .clipboard_ms(clipboard_elapsed_seconds.map(|seconds| seconds * 1_000.0))
    .state_write_ms(revision_state_elapsed_seconds.map(|seconds| seconds * 1_000.0))
    .degraded(degraded)
    .cleanup_status(cleanup_status)
    .request_total_ms(reclean_started.elapsed().as_secs_f64() * 1_000.0)
    .emit();
    release_operation(&state, OperationLease::HistoryReclean);
    if let Some((input_bytes, max_bytes)) = native_input_capacity {
        return Err(format!(
            "history_native_input_too_large:{input_bytes}:{max_bytes}"
        ));
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn delete_history(
    id: i64,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let _operation = claim_history_mutation(&app, &state)?;
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    store::delete_history(&dir, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn retry_dictation(
    id: i64,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let processing_guard = capture_history_processing_guard(&state);
    if !try_claim_operation(&state, OperationLease::HistoryReclean) {
        return Err("Dictation is active; try retry again after it finishes".into());
    }
    let clipboard_lease = paste::RetryClipboardLease::capture(&app);
    let result = retry_dictation_inner(id, app, &state, &processing_guard, &clipboard_lease).await;
    release_operation(&state, OperationLease::HistoryReclean);
    result
}

async fn transcribe_history_audio(
    wav: &[u8],
    chunk_length_secs: usize,
    state: &AppState,
    request: &crate::AsrRequestSnapshot,
    cancellation: CancellationToken,
) -> Result<asr::Transcript, String> {
    let _latency = state.metrics.timer(metrics::MetricKind::FinalAsr);
    transcribe_complete_audio_inner(
        wav,
        chunk_length_secs,
        state,
        request,
        cancellation,
        AsrHud::Background,
    )
    .await
    .map_err(|error| match error {
        FullAudioTranscriptionError::Cancelled => "request cancelled".to_owned(),
        FullAudioTranscriptionError::Failed(message) => message,
    })
}

#[derive(Debug)]
pub(crate) enum FullAudioTranscriptionError {
    Cancelled,
    Failed(String),
}

/// Whether an ASR request may drive the recording/processing HUD. Live flows
/// (selected/screen action) own the Processing HUD; History retries do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AsrHud {
    Live,
    Background,
}

async fn execute_asr_request<F, Fut, T, E>(
    gate: &queue::RequestGate,
    hud: AsrHud,
    scope: &queue::RequestScope,
    operation: F,
    cancellation: CancellationToken,
) -> Result<T, queue::ExecuteError<E>>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: queue::RetryError,
{
    match hud {
        AsrHud::Live => {
            queue::execute_with_retry_scoped_cancelled(
                gate,
                queue::RequestKind::Asr,
                scope,
                operation,
                cancellation,
            )
            .await
        }
        AsrHud::Background => {
            queue::execute_with_retry_scoped_cancelled_background(
                gate,
                queue::RequestKind::Asr,
                scope,
                operation,
                cancellation,
            )
            .await
        }
    }
}

/// Live entry point (selected-action and screen-action flows): keeps HUD retry
/// events while the manager is Processing for the request's generation.
pub(crate) async fn transcribe_complete_audio(
    wav: &[u8],
    chunk_length_secs: usize,
    state: &AppState,
    request: &crate::AsrRequestSnapshot,
    cancellation: CancellationToken,
) -> Result<asr::Transcript, FullAudioTranscriptionError> {
    transcribe_complete_audio_inner(
        wav,
        chunk_length_secs,
        state,
        request,
        cancellation,
        AsrHud::Live,
    )
    .await
}

async fn transcribe_complete_audio_inner(
    wav: &[u8],
    chunk_length_secs: usize,
    state: &AppState,
    request: &crate::AsrRequestSnapshot,
    cancellation: CancellationToken,
    hud: AsrHud,
) -> Result<asr::Transcript, FullAudioTranscriptionError> {
    if cancellation.is_cancelled() {
        return Err(FullAudioTranscriptionError::Cancelled);
    }
    let duration = asr::wav_duration_seconds(wav).ok_or_else(|| {
        FullAudioTranscriptionError::Failed("Audio is not a readable WAV file".into())
    })?;
    let capabilities = request
        .provider
        .capabilities_for_model(&request.options.model);
    let direct_audio_limit = capabilities
        .max_audio_duration_secs
        .map(|seconds| seconds as f64)
        .unwrap_or(asr::MAX_DIRECT_REQUEST_DURATION_SECS as f64)
        .min(asr::MAX_DIRECT_REQUEST_DURATION_SECS as f64);
    if duration <= direct_audio_limit {
        let transcript = execute_asr_request(
            &state.gate,
            hud,
            &request.quota_scope,
            || {
                request
                    .provider
                    .transcribe_batch(wav.to_vec(), request.options.clone())
            },
            cancellation,
        )
        .await
        .map_err(|error| match error {
            queue::ExecuteError::Cancelled => FullAudioTranscriptionError::Cancelled,
            queue::ExecuteError::Operation(error) => {
                FullAudioTranscriptionError::Failed(error.to_string())
            }
        })?;
        state
            .gate
            .update_asr_for(&request.quota_scope, &transcript.limits);
        return Ok(transcript);
    }

    let reader = hound::WavReader::new(std::io::Cursor::new(wav)).map_err(|_| {
        FullAudioTranscriptionError::Failed("Audio is not a readable WAV file".into())
    })?;
    let spec = reader.spec();
    if spec.channels != 1
        || spec.sample_rate != 16_000
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
    {
        return Err(FullAudioTranscriptionError::Failed(
            "Long audio must be 16 kHz mono PCM".into(),
        ));
    }
    let samples = reader
        .into_samples::<i16>()
        .map(|sample| {
            sample
                .map(|value| value as f32 / i16::MAX as f32)
                .map_err(|_| {
                    FullAudioTranscriptionError::Failed("Audio contains invalid PCM samples".into())
                })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let chunk_length_secs = if direct_audio_limit <= 20.0 {
        15
    } else {
        chunk_length_secs
    };
    let mut planner = chunker::Chunker::new(chunker::ChunkerConfig { chunk_length_secs });
    let mut recognized_chunks = Vec::new();
    let mut original_chunks = Vec::new();
    let mut language = None;
    let mut limits = asr::RateLimits::default();
    for sample_block in samples.chunks(30 * 16_000) {
        for chunk in planner.push(sample_block) {
            append_history_transcript_chunk(
                chunk,
                state,
                request,
                cancellation.clone(),
                &mut recognized_chunks,
                &mut original_chunks,
                &mut language,
                &mut limits,
                hud,
            )
            .await?;
        }
    }
    if let Some(chunk) = planner.finish() {
        append_history_transcript_chunk(
            chunk,
            state,
            request,
            cancellation,
            &mut recognized_chunks,
            &mut original_chunks,
            &mut language,
            &mut limits,
            hud,
        )
        .await?;
    }

    if recognized_chunks.is_empty() {
        return Err(FullAudioTranscriptionError::Failed(
            "Audio contains no transcribable content".into(),
        ));
    }
    Ok(asr::Transcript {
        text: chunker::merge_transcripts_with_timing(recognized_chunks),
        asr_text: Some(chunker::merge_transcripts_with_timing(original_chunks)),
        provider_cleaned_candidate: None,
        language,
        confidence: None,
        segments: Vec::new(),
        words: Vec::new(),
        tokens: Vec::new(),
        limits,
    })
}

#[allow(
    clippy::too_many_arguments,
    reason = "Keep chunk source bounds, provider request, cancellation, and whole-recording merge state explicit."
)]
async fn append_history_transcript_chunk(
    chunk: chunker::AudioChunk,
    state: &AppState,
    request: &crate::AsrRequestSnapshot,
    cancellation: CancellationToken,
    recognized_chunks: &mut Vec<chunker::TimedTranscriptChunk>,
    original_chunks: &mut Vec<chunker::TimedTranscriptChunk>,
    language: &mut Option<String>,
    limits: &mut asr::RateLimits,
    hud: AsrHud,
) -> Result<(), FullAudioTranscriptionError> {
    let chunk_wav = chunker::encode_wav(&chunk.samples).map_err(|_| {
        FullAudioTranscriptionError::Failed("Audio chunk could not be encoded".into())
    })?;
    let transcript = execute_asr_request(
        &state.gate,
        hud,
        &request.quota_scope,
        || {
            request
                .provider
                .transcribe_batch(chunk_wav.clone(), request.options.clone())
        },
        cancellation,
    )
    .await
    .map_err(|error| match error {
        queue::ExecuteError::Cancelled => FullAudioTranscriptionError::Cancelled,
        queue::ExecuteError::Operation(error) => {
            FullAudioTranscriptionError::Failed(error.to_string())
        }
    })?;
    state
        .gate
        .update_asr_for(&request.quota_scope, &transcript.limits);
    if language.is_none() {
        *language = transcript.language.clone();
    }
    *limits = transcript.limits.clone();
    original_chunks.push(crate::timed_transcript_chunk(
        chunk.index,
        transcript.original_text().to_owned(),
        chunk.start_secs,
        chunk.end_secs,
        &transcript.words,
    ));
    recognized_chunks.push(crate::timed_transcript_chunk(
        chunk.index,
        transcript.text,
        chunk.start_secs,
        chunk.end_secs,
        &transcript.words,
    ));
    Ok(())
}

async fn retry_dictation_inner(
    id: i64,
    app: tauri::AppHandle,
    state: &AppState,
    processing_guard: &HistoryProcessingGuard,
    clipboard_lease: &paste::RetryClipboardLease,
) -> Result<(), String> {
    ensure_history_processing_current(state, processing_guard)?;
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let expected_root = dir.clone();
    let read_cancellation = processing_guard.cancellation.clone();
    let retry_audio = crate::audio::run_retained_spool_store_job(
        format!("history-retry-read-{id}"),
        dir.clone(),
        crate::audio::MAX_HISTORY_RETRY_WAV_BYTES + 4096,
        move |lease| {
            if lease.data_root() != expected_root.as_path() {
                return Err("History retry read does not own the data root".into());
            }
            if read_cancellation.is_cancelled() {
                return Err("History retry was cancelled before reading audio".into());
            }
            let path = store::failed_spool(lease.data_root(), id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Audio spool is no longer available".to_owned())?;
            let wav = store::read_spool_file_limited(
                std::path::Path::new(&path),
                crate::audio::MAX_HISTORY_RETRY_WAV_BYTES,
            )
            .map_err(|error| error.to_string())?;
            if read_cancellation.is_cancelled() {
                return Err("History retry was cancelled after reading audio".into());
            }
            Ok((path, wav))
        },
    )
    .await
    .map_err(|_| {
        if processing_guard.cancellation.is_cancelled() {
            "request cancelled".to_owned()
        } else {
            "Audio spool is no longer available".to_owned()
        }
    })?;
    ensure_history_processing_current(state, processing_guard)?;
    let (path, wav) = retry_audio;
    let settings = lock_recover(&state.settings).clone();
    let scene = store::history_scene(&dir, id).map_err(|e| e.to_string())?;
    let pairs = store::list_learn_pairs(&dir).unwrap_or_default();
    let scope = lexicon::PromptScope::from_history(
        scene.profile_id.as_deref(),
        scene.family.as_deref(),
        scene.browser_host.as_deref(),
    );
    let asr_prompt = lexicon::build_asr_prompt_bundle(
        &settings.dictionary,
        scene.policy.as_ref(),
        &pairs,
        Some(&scope),
        lexicon::asr_prompt_shape_for(settings.asr_provider, &settings.asr_model),
        None,
    );
    let mut retry_policy = scene.policy.clone().unwrap_or_default();
    apply_current_style_approval(&mut retry_policy, scene.profile_id.as_deref(), &settings);
    apply_current_history_retry_output_mode(&mut retry_policy, &settings);
    let asr_request = asr_request_snapshot_for_options_with_metrics(
        &settings,
        &state.models_root,
        asr::AsrOptions {
            api_key: String::new(),
            language: None,
            prompt: asr_prompt.prompt,
            keywords: asr_prompt.keywords,
            model: String::new(),
            session_generation: None,
            trace: None,
        },
        &state.metrics,
        "history_retry",
    );
    let engine_provenance = asr_request.provenance.clone();
    let transcript = transcribe_history_audio(
        &wav,
        settings.chunk_length_secs,
        state,
        &asr_request,
        processing_guard.cancellation.clone(),
    )
    .await?;
    ensure_history_processing_current(state, processing_guard)?;
    let source_transcript = transcript.original_text().to_owned();
    let family = family_from_scene(&scene, &settings.context_mappings);
    let mapping = scene
        .profile_id
        .as_deref()
        .and_then(|id| lexicon::mapping_for_profile(&settings.context_mappings, id));
    let confidence = if scene.profile_id.is_some() {
        0.88
    } else {
        0.0
    };
    let input_kind = retry_policy.input_kind;
    let retry_scene =
        crate::ResolvedCleanupScene::observed_for_history_retry(family, input_kind, mapping)
            .with_codex_transcript_prompt(
                scene.profile_id.as_deref(),
                scene.native_bundle.as_deref(),
                &source_transcript,
            );
    crate::apply_resolved_scene_to_policy(&mut retry_policy, retry_scene);
    let mut retry_policy_without_examples = retry_policy.clone();
    retry_policy_without_examples.style_examples_approved = false;
    retry_policy_without_examples.style_example_input = None;
    retry_policy_without_examples.style_example_output = None;
    retry_policy_without_examples.style_example_pairs.clear();
    let prepared = if settings.output_mode == "code" {
        crate::prepare_exact_code_transcript(retry_scene, &source_transcript)
    } else {
        crate::prepare_cleanup_transcript_for_resolved_scene(
            Some(&dir),
            &settings,
            &transcript.text,
            retry_scene,
            confidence,
            Some(&scene.learn_scope()),
            settings.fuzzy_dictionary_enabled,
        )
    };
    let raw_text = prepared.text.clone();
    let pairs_hint = prepared.pairs_hint.clone();
    let clipboard =
        snippets::read_clipboard_if_needed(&settings.snippets, &prepared.snippet_input, || {
            clipboard_text_for_snippets(&app)
        });
    let snippet_expansion = snippets::resolve_exact_with_clipboard(
        &settings.snippets,
        &prepared.snippet_input,
        clipboard.as_deref(),
    );
    let intent = prepared.intent.clone();
    let cleanup_input = snippet_expansion
        .clone()
        .unwrap_or_else(|| intent.content.clone());
    let cleanup_route = crate::cleanup_route_for_resolved_scene(&settings, prepared.scene, &intent);
    let cleanup_output_projection = match cleanup_route {
        lexicon::CleanupRoute::Provider(effort) if snippet_expansion.is_none() => {
            history_retry_output_projection(HistoryRetryOutputProjectionInput {
                settings: &settings,
                profile_id: scene.profile_id.as_deref(),
                family,
                input_kind,
                scene: retry_scene,
                intent: &intent,
                route_effort: effort,
                source: &cleanup_input,
            })
        }
        _ => None,
    };
    let cleanup_output_plan = cleanup_output_projection
        .as_ref()
        .map(|resolved| resolved.plan);
    let structure_safe_source = cleanup_output_plan
        .filter(|plan| plan.edit_budget == crate::cleanup_prompt::EditBudget::StructureSafe)
        .map(|_| cleanup_input.clone());
    let mut cleanup_attempt_kind = Some("not_called".to_owned());
    let mut cleanup_transformations = Some(Vec::new());
    let mut cleanup_failure_reason: Option<String> = None;
    let cleanup_decision = match cleanup_route {
        lexicon::CleanupRoute::Provider(_)
            if settings.asr_provider == crate::providers::EngineProvider::AssemblyAi
                && settings.cleanup_credential().trim().is_empty() =>
        {
            cleanup_attempt_kind = Some("credentials_unavailable".to_owned());
            cleanup_failure_reason = Some("credentials_unavailable".to_owned());
            CleanupDecision::Failed
        }
        lexicon::CleanupRoute::Provider(effort) if snippet_expansion.is_none() => {
            let cleanup_scope = crate::cleanup_quota_scope(&settings);
            let cleanup_result = {
                let _latency = state.metrics.timer(metrics::MetricKind::Cleanup);
                let cleanup_endpoint = settings.cleanup_endpoint();
                let cleanup_model = settings.cleanup_request_model();
                let cleanup_key = settings.cleanup_credential().to_owned();
                let profile_id = scene.profile_id.clone();
                let policy_base = retry_policy_without_examples.clone();
                let app_state = state;
                queue::execute_with_retry_scoped_cancelled_background(
                    &app_state.gate,
                    queue::RequestKind::Llm,
                    &cleanup_scope,
                    || {
                        let current_settings = lock_recover(&app_state.settings).clone();
                        let mut attempt_policy = policy_base.clone();
                        apply_current_style_approval(
                            &mut attempt_policy,
                            profile_id.as_deref(),
                            &current_settings,
                        );
                        attempt_policy.cleanup_output_plan = cleanup_output_plan;
                        let sent_signature =
                            crate::cleanup_projection_signature(None, &attempt_policy);
                        let cleanup_endpoint = cleanup_endpoint.clone();
                        let cleanup_model = cleanup_model.clone();
                        let cleanup_key = cleanup_key.clone();
                        let cleanup_provider = settings.cleanup_provider;
                        let custom_cleanup_parameters = (cleanup_provider
                            == crate::providers::EngineProvider::Custom)
                            .then(|| current_settings.custom_cleanup_parameters.clone());
                        let cleanup_input = cleanup_input.clone();
                        let source_transcript = source_transcript.clone();
                        let pairs_hint = pairs_hint.clone();
                        let language = settings.language.clone();
                        let intent = intent.clone();
                        let policy_base = policy_base.clone();
                        let profile_id = profile_id.clone();
                        async move {
                            let result = crate::with_cleanup_warmup_model_demand(
                                app_state,
                                cleanup_provider,
                                llm::cleanup_with_provider_and_native_runtime_and_intent_and_source(
                                    cleanup_provider,
                                    &app_state.mlx_cleanup_runtime,
                                    &cleanup_endpoint,
                                    &cleanup_model,
                                    &cleanup_input,
                                    &cleanup_key,
                                    &[],
                                    None,
                                    Some(&attempt_policy),
                                    Some(language.as_str()),
                                    None,
                                    Some(&intent),
                                    pairs_hint.as_deref(),
                                    effort,
                                    None,
                                    Some(&source_transcript),
                                    Some(id),
                                    custom_cleanup_parameters.as_ref(),
                                ),
                            )
                            .await;
                            if result.is_ok() {
                                let latest_settings = lock_recover(&app_state.settings).clone();
                                let mut latest_policy = policy_base.clone();
                                apply_current_style_approval(
                                    &mut latest_policy,
                                    profile_id.as_deref(),
                                    &latest_settings,
                                );
                                if crate::cleanup_projection_signature(None, &latest_policy)
                                    != sent_signature
                                {
                                    return Err(llm::LlmError::ContextAuthorizationChanged);
                                }
                            }
                            result
                        }
                    },
                    processing_guard.cancellation.clone(),
                )
                .await
            };
            match cleanup_result {
                Ok((reply, limits)) => {
                    state.gate.update_llm_for(&cleanup_scope, &limits);
                    cleanup_attempt_kind = Some("success".to_owned());
                    cleanup_transformations = Some(reply.transformations.clone());
                    CleanupDecision::Provider(reply.accepted_text)
                }
                Err(error) => {
                    log::warn!(
                        "history retry cleanup failed, copying raw transcript: {}",
                        match &error {
                            queue::ExecuteError::Operation(error) => error.log_summary(),
                            queue::ExecuteError::Cancelled => "cancelled".to_owned(),
                        }
                    );
                    match error {
                        queue::ExecuteError::Cancelled => return Err("Retry was cancelled".into()),
                        queue::ExecuteError::Operation(error) => {
                            cleanup_attempt_kind = Some(error.history_attempt_kind().to_owned());
                            cleanup_failure_reason = Some(error.history_failure_reason());
                            cleanup_transformations = None;
                            if let Some(decision) =
                                critical_retry_decision(&error, &source_transcript)
                            {
                                decision
                            } else if let Some(decision) =
                                memory_unavailable_retry_decision(&error, &source_transcript)
                            {
                                decision
                            } else if let Some(fact) = llm::cleanup_rejection(&error) {
                                CleanupDecision::CleanupRejected(fact)
                            } else if llm::is_preservation_guard_error(&error) {
                                CleanupDecision::GuardRejected
                            } else {
                                CleanupDecision::Failed
                            }
                        }
                    }
                }
            }
        }
        _ => CleanupDecision::Disabled,
    };
    let local_failure_fallback = if structure_safe_source.is_some() {
        cleanup_input.clone()
    } else {
        local_cleanup_or_raw_for_resolved_scene(&cleanup_input, prepared.scene)
    };
    let mut cleanup_status = match &cleanup_decision {
        CleanupDecision::Provider(text) if text.trim().is_empty() => {
            cleanup_failure_status(&cleanup_input, &local_failure_fallback)
        }
        CleanupDecision::Provider(_) => CLEANUP_STATUS_AI_SUCCESS,
        CleanupDecision::CriticalMemoryPressure(_) => CLEANUP_STATUS_CRITICAL_MEMORY_PRESSURE,
        CleanupDecision::MemoryUnavailable(_) => CLEANUP_STATUS_LOCAL_MEMORY_UNAVAILABLE,
        CleanupDecision::Failed => cleanup_failure_status(&cleanup_input, &local_failure_fallback),
        CleanupDecision::GuardRejected => CLEANUP_STATUS_PRESERVATION_GUARD,
        CleanupDecision::CleanupRejected(_) => CLEANUP_STATUS_PRESERVATION_GUARD,
        CleanupDecision::Disabled if snippet_expansion.is_some() => CLEANUP_STATUS_SNIPPET_BYPASS,
        CleanupDecision::Disabled => CLEANUP_STATUS_LOCAL_ONLY,
    };
    let resolved = finalize_text_for_scene(
        &cleanup_input,
        cleanup_decision,
        FinalizationContext {
            scene: prepared.scene,
            operation: intent.operation,
            revision_source: Some(&prepared.revision_source),
            prepared_transcript: Some(&prepared.text),
            revision_authorizations: &prepared.revision_authorizations,
            promoted_pair_protections: &prepared.promoted_pair_protections,
            list_revision_proof: prepared.list_revision_proof.as_ref(),
            structure_safe_source: structure_safe_source.as_deref(),
            output_projection_list_revision_proof: None,
            layout_source: None,
            exact_fallback: false,
        },
    )
    .map_err(|_| "No speech detected".to_owned())?;
    let final_text = resolved.text;
    let degraded = resolved.degraded;
    let degraded_reason = resolved.degraded_reason;
    let cleanup_final_guard_reason = resolved
        .final_guard_reason
        .map(|reason| llm::CleanupRejectionStage::FinalOutput.storage_code(reason));
    let cleanup_rejection = resolved.cleanup_rejection;
    if cleanup_failure_reason.is_none() && degraded_reason == Some("llm_cleanup_empty") {
        cleanup_failure_reason = Some("empty_completion".to_owned());
    }
    if cleanup_failure_reason.is_none() {
        cleanup_failure_reason = cleanup_final_guard_reason.clone();
    }
    if cleanup_rejection
        .as_ref()
        .is_some_and(|fact| matches!(fact.stage, llm::CleanupRejectionStage::FinalOutput))
        && cleanup_attempt_kind.as_deref() == Some("success")
    {
        cleanup_attempt_kind = Some("final_guard".to_owned());
    }
    if degraded_reason == Some("preservation_guard") {
        cleanup_status = CLEANUP_STATUS_PRESERVATION_GUARD;
        state.metrics.record_cleanup_guard_fallback();
    }
    // A retry has no trustworthy original target guard. The pasteboard owner
    // admits the write only when its complete native snapshot still matches;
    // if that proof changes or is unavailable, keep the transcript in History.
    let copy_outcome = with_current_history_processing(state, processing_guard, || {
        let copy_outcome = clipboard_lease.copy_if_unchanged(&app, &final_text);
        let delivery = match copy_outcome {
            paste::RetryClipboardOutcome::Copied => store::RetryDelivery::Copied,
            paste::RetryClipboardOutcome::Changed => store::RetryDelivery::ClipboardChanged,
            paste::RetryClipboardOutcome::OwnershipUnverified => {
                store::RetryDelivery::ClipboardOwnershipUnverified
            }
            paste::RetryClipboardOutcome::WriteFailed => store::RetryDelivery::ClipboardWriteFailed,
        };
        let cleanup_guard_reason = cleanup_rejection
            .as_ref()
            .map(|fact| fact.stage.storage_code(fact.reason));
        store::mark_retried_with_texts_and_cleanup_rejection(
            &dir,
            id,
            Some(&raw_text),
            Some(transcript.original_text()),
            Some(&engine_provenance),
            &final_text,
            degraded,
            degraded_reason,
            Some(cleanup_status),
            delivery,
            cleanup_rejection
                .as_ref()
                .map(|fact| fact.candidate.as_str()),
            cleanup_guard_reason.as_deref(),
            cleanup_attempt_kind.as_deref(),
            cleanup_transformations.as_deref(),
            cleanup_final_guard_reason.as_deref(),
            cleanup_failure_reason.as_deref(),
        )
        .map_err(|e| e.to_string())?;
        Ok(copy_outcome)
    })?;
    let expected_root = dir.clone();
    let cleanup_path = std::path::PathBuf::from(path);
    if let Err(error) = crate::audio::run_retained_spool_store_job(
        format!("history-retry-cleanup-{id}"),
        dir,
        cleanup_path.as_os_str().len().saturating_add(512),
        move |lease| {
            if lease.data_root() != expected_root.as_path() {
                return Err("History retry cleanup does not own the data root".into());
            }
            store::remove_spool_artifact(lease.data_root(), &cleanup_path);
            Ok(())
        },
    )
    .await
    {
        log::debug!("History retry audio cleanup was not confirmed: {error}");
    }
    if copy_outcome == paste::RetryClipboardOutcome::Copied {
        let _ = app.emit(
            "dictation://copied",
            serde_json::json!({ "message": "已复制，请手动粘贴" }),
        );
    }
    Ok(())
}

fn family_from_scene(
    scene: &store::HistoryScene,
    mappings: &[context::AppMapping],
) -> context::ContextFamily {
    scene
        .family
        .as_deref()
        .and_then(context::builtin_family_for_id)
        .unwrap_or_else(|| {
            scene
                .profile_id
                .as_deref()
                .map(|id| lexicon::family_from_profile_id(id, mappings))
                .unwrap_or(context::ContextFamily::General)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct RateLimitedOnce;

    impl queue::RetryError for RateLimitedOnce {
        fn retry_kind(&self) -> queue::RetryClass {
            queue::RetryClass::RateLimited(0.0)
        }
    }

    async fn rate_limited_asr_events(hud: AsrHud) -> Vec<String> {
        let gate = queue::RequestGate::new(None);
        gate.set_session_generation(7);
        gate.set_test_manager_state(Some((crate::Phase::Processing, 7)));
        let scope = queue::RequestScope::new("test", Some("https://test.invalid"), "m", "hud");
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let result = execute_asr_request(
            &gate,
            hud,
            &scope,
            || async {
                if calls.fetch_add(1, std::sync::atomic::Ordering::AcqRel) == 0 {
                    Err(RateLimitedOnce)
                } else {
                    Ok("transcript")
                }
            },
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_ok());
        gate.emitted_event_names()
    }

    #[tokio::test]
    async fn live_asr_entry_keeps_hud_retry_events_and_history_entry_suppresses_them() {
        // `transcribe_complete_audio` (live) maps to AsrHud::Live and
        // `transcribe_history_audio` to AsrHud::Background; AppState cannot be
        // built in a unit test, so the dispatch layer is tested directly.
        let live = rate_limited_asr_events(AsrHud::Live).await;
        assert!(live.iter().any(|event| event == "dictation://state"));
        assert!(live.iter().any(|event| event == "quota://retrying"));
        let background = rate_limited_asr_events(AsrHud::Background).await;
        assert!(!background
            .iter()
            .any(|event| event == "dictation://state" || event.starts_with("quota://")));
    }

    fn history_style_mapping(approved: bool, enabled: bool) -> context::AppMapping {
        context::AppMapping {
            id: "history-app".into(),
            label: "History App".into(),
            family: context::ContextFamily::Document,
            mode_id: None,
            bundle_id: Some("com.example.Editor".into()),
            executable: None,
            browser_host: None,
            browser_path_prefix: None,
            focused_field: None,
            source_permissions: context::ContextSourcePermissions::default(),
            style_example_input: Some("retained source input".into()),
            style_example_output: Some("retained source output".into()),
            style_example_pairs: vec![context::StyleExamplePair {
                input: "retained source input".into(),
                output: "retained source output".into(),
            }],
            style_examples_approved: approved,
            enabled,
            cleanup_effort: None,
            cleanup_intensity: None,
            cleanup_enabled: true,
            dictionary_learn_enabled: true,
        }
    }

    fn temp_history_dir(name: &str) -> std::path::PathBuf {
        static NEXT_TEMP_HISTORY_DIR: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        let unique_id = NEXT_TEMP_HISTORY_DIR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "voiceflow-history-reclean-{name}-{}-{unique_id}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).expect("create isolated History fixture directory");
        dir
    }

    #[test]
    fn history_reclean_prepares_and_adapts_the_stored_asr_not_processed_text() {
        let dir = temp_history_dir("asr-source");
        let asr = "第一个是准确性，第二个是稳定性。";
        store::insert_history_with_asr_and_delivery_and_spool_and_cleanup(
            &dir,
            "processed draft that must not be reused",
            Some(asr),
            "previous final output",
            1.0,
            false,
            None,
            "ok",
            "history",
            None,
            &context::ContextSnapshot::general(),
            None,
            "fixture:asr",
            "local_only",
        )
        .expect("insert isolated production-shaped History record");

        let input = history_reclean_asr_text(&dir, 1).expect("stored ASR is available");
        assert_eq!(input, asr);
        assert_ne!(input, "processed draft that must not be reused");
        let prepared = crate::prepare_cleanup_transcript_for_scene(
            Some(&dir),
            &store::Settings::default(),
            &input,
            context::ContextFamily::General,
            0.9,
            context::FocusKind::Editable,
            None,
            false,
        );
        let intent =
            llm::CleanupIntent::selected_text(llm::CleanupOperation::Cleanup, &prepared.text);
        assert_eq!(intent.content, prepared.text);
        assert!(!intent.content.contains("processed draft"));
        let adapted = llm::accept_cleanup_reply(intent.content.clone(), &intent)
            .expect("prepared ASR follows the shared cleanup adapter");
        assert_eq!(adapted, intent.content);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_reclean_requires_nonempty_asr_but_keeps_missing_record_error() {
        let dir = temp_history_dir("missing-asr");
        for (index, asr) in [None, Some(""), Some("  \n")].into_iter().enumerate() {
            store::insert_history_with_asr_and_delivery_and_spool_and_cleanup(
                &dir,
                "processed content is not an ASR fallback",
                asr,
                "final",
                1.0,
                false,
                None,
                "ok",
                "history",
                None,
                &context::ContextSnapshot::general(),
                None,
                "fixture:asr",
                "local_only",
            )
            .expect("insert isolated legacy-shaped History record");
            let id = index as i64 + 1;
            assert_eq!(
                history_reclean_asr_text(&dir, id),
                Err("这条历史记录没有可重新整理的原文".to_owned())
            );
        }
        assert!(store::history_asr_text(&dir, 999).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_retry_auto_clears_saved_output_mode_and_target() {
        let mut policy = context::ContextPolicy::for_family(context::ContextFamily::Email);
        policy.output_mode = Some("email".into());
        policy.translation_target_language = Some("French".into());
        let settings = store::Settings::default();

        apply_current_history_retry_output_mode(&mut policy, &settings);

        assert!(policy.output_mode.is_none());
        assert!(policy.translation_target_language.is_none());
        assert!(policy.cleanup_output_plan.is_none());

        let settings = store::Settings {
            output_mode: "bullets".into(),
            ..store::Settings::default()
        };
        apply_current_history_retry_output_mode(&mut policy, &settings);
        assert_eq!(policy.output_mode.as_deref(), Some("bullets"));
    }

    #[test]
    fn detached_history_keeps_current_manual_mode_from_inventing_layout() {
        let settings = store::Settings {
            output_mode: "bullets".into(),
            ..store::Settings::default()
        };
        let source = "Please review this note, then tell me whether you agree.";
        let intent = llm::CleanupIntent::implicit(source);
        let scene = crate::ResolvedCleanupScene::observed(
            context::ContextFamily::Document,
            context::FocusKind::Document,
        );

        let resolved = history_retry_output_projection(HistoryRetryOutputProjectionInput {
            settings: &settings,
            profile_id: Some("docs.work"),
            family: context::ContextFamily::Document,
            input_kind: context::FocusKind::Document,
            scene,
            intent: &intent,
            route_effort: llm::CleanupEffort::Standard,
            source,
        })
        .expect("eligible retry keeps a shared output plan");

        assert_eq!(
            resolved.plan.output_add_on,
            crate::cleanup_prompt::OutputAddOn::AutoDocument
        );
        assert_eq!(
            resolved.plan.mode_source,
            crate::cleanup_prompt::ModeSource::AutoScene
        );
        assert_eq!(resolved.projection.text, source);
        assert!(resolved.projection.edits.is_empty());
    }

    #[test]
    fn detached_history_marks_existing_structure_safe_without_layout_projection() {
        let settings = store::Settings::default();
        let source = "1. Check the logs.\n2. Restart the service.";
        let intent = llm::CleanupIntent::implicit(source);
        let scene = crate::ResolvedCleanupScene::observed(
            context::ContextFamily::Document,
            context::FocusKind::Document,
        );

        let resolved = history_retry_output_projection(HistoryRetryOutputProjectionInput {
            settings: &settings,
            profile_id: Some("docs.work"),
            family: context::ContextFamily::Document,
            input_kind: context::FocusKind::Document,
            scene,
            intent: &intent,
            route_effort: llm::CleanupEffort::Heavy,
            source,
        })
        .expect("eligible retry keeps a shared output plan");

        assert_eq!(
            resolved.plan.edit_budget,
            crate::cleanup_prompt::EditBudget::StructureSafe
        );
        assert_eq!(resolved.projection.text, source);
        assert!(resolved.projection.edits.is_empty());
    }

    #[test]
    fn explicit_code_retry_preparation_keeps_raw_sync_bytes() {
        let source = "print('hello')\r\n\r\n  # literal\n";
        let scene = crate::ResolvedCleanupScene::observed(
            context::ContextFamily::PromptOrCode,
            context::FocusKind::Code,
        );
        let prepared = crate::prepare_exact_code_transcript(scene, source);

        assert_eq!(prepared.text, source);
        assert_eq!(prepared.revision_source, source);
        assert_eq!(prepared.original_asr_source, source);
        assert!(prepared.revision_authorizations.is_empty());
        assert!(prepared.list_revision_proof.is_none());
    }

    #[test]
    fn reclean_uses_stored_history_family_not_general() {
        let wechat = store::HistoryScene {
            family: Some("personal_chat".into()),
            profile_id: Some("chat.personal".into()),
            ..Default::default()
        };
        assert_eq!(
            family_from_scene(&wechat, &[]),
            context::ContextFamily::PersonalChat
        );

        let terminal = store::HistoryScene {
            family: Some("terminal".into()),
            ..Default::default()
        };
        assert_eq!(
            family_from_scene(&terminal, &[]),
            context::ContextFamily::Terminal
        );
    }

    #[test]
    fn degraded_reclean_does_not_attempt_a_clipboard_copy() {
        let mut copy_attempted = false;
        let delivery = reclean_delivery_after_finalization(true, || {
            copy_attempted = true;
            store::RecleanDelivery::Copied
        });
        assert!(!copy_attempted);
        assert_eq!(delivery, None);
    }

    #[test]
    fn critical_retry_decision_keeps_the_exact_original_asr_text() {
        let error = llm::map_on_device_mlx_error(
            crate::mlx_cleanup_runtime::RuntimeError::CriticalMemoryPressure,
        );
        let original = "um exact ASR\nsecond line";
        let decision = critical_retry_decision(&error, original)
            .expect("typed critical pressure uses the raw ASR route");
        assert!(
            matches!(decision, CleanupDecision::CriticalMemoryPressure(text) if text == original)
        );
    }

    #[test]
    fn local_memory_retry_decision_keeps_the_exact_source_transcript() {
        let error = llm::map_on_device_mlx_error(
            crate::mlx_cleanup_runtime::RuntimeError::MemoryUnavailable(
                crate::model_resources::MemoryUnavailableReason::BudgetExceeded,
            ),
        );
        let original = "um exact ASR\n\nsecond line";
        let decision = memory_unavailable_retry_decision(&error, original)
            .expect("memory refusal routes History retry to the source transcript");
        assert!(matches!(decision, CleanupDecision::MemoryUnavailable(text) if text == original));
    }

    #[test]
    fn critical_reclean_error_is_detected_before_selected_text_revision_write() {
        let error = queue::ExecuteError::Operation(llm::map_on_device_mlx_error(
            crate::mlx_cleanup_runtime::RuntimeError::CriticalMemoryPressure,
        ));
        assert!(is_critical_reclean_error(&error));
        let ordinary_error = queue::ExecuteError::Operation(llm::LlmError::Other("offline".into()));
        assert!(!is_critical_reclean_error(&ordinary_error));
    }

    #[test]
    fn history_rehydrates_only_currently_approved_enabled_style_examples() {
        let mut saved = context::ContextPolicy::for_family(context::ContextFamily::Document);
        saved.style_examples_approved = true;
        saved.style_example_input = Some("old approved input".into());
        saved.style_example_output = Some("old approved output".into());
        saved.style_example_pairs = vec![context::StyleExamplePair {
            input: "old approved input".into(),
            output: "old approved output".into(),
        }];

        let approved_settings = store::Settings {
            context_enabled: true,
            context_mappings: vec![history_style_mapping(true, true)],
            ..store::Settings::default()
        };
        let mut approved = saved.clone();
        apply_current_style_approval(&mut approved, Some("user.history-app"), &approved_settings);
        assert!(approved.style_examples_approved);
        assert_eq!(
            approved.style_example_pairs[0].input,
            "retained source input"
        );
        assert!(!approved.style_example_pairs[0]
            .input
            .contains("old approved"));

        let revoked_settings = store::Settings {
            context_enabled: true,
            context_mappings: vec![history_style_mapping(false, true)],
            ..store::Settings::default()
        };
        let mut revoked = saved.clone();
        apply_current_style_approval(&mut revoked, Some("user.history-app"), &revoked_settings);
        assert!(!revoked.style_examples_approved);
        assert!(revoked.style_example_pairs.is_empty());
        assert!(revoked.style_example_input.is_none());

        let disabled_settings = store::Settings {
            context_enabled: true,
            context_mappings: vec![history_style_mapping(true, false)],
            ..store::Settings::default()
        };
        let mut disabled = saved;
        apply_current_style_approval(&mut disabled, Some("user.history-app"), &disabled_settings);
        assert!(!disabled.style_examples_approved);
        assert!(disabled.style_example_pairs.is_empty());
    }

    #[test]
    fn save_history_revision_ipc_decodes_the_history_ui_snake_case_payload() {
        let app = tauri::test::mock_builder()
            .invoke_handler(|invoke| super::__cmd__save_history_revision!(test_decode_stub, invoke))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let invoke = |body, callback| {
            tauri::test::get_ipc_response(
                &webview,
                tauri::webview::InvokeRequest {
                    cmd: "save_history_revision".into(),
                    callback: tauri::ipc::CallbackFn(callback),
                    error: tauri::ipc::CallbackFn(callback + 1),
                    url: if cfg!(any(windows, target_os = "android")) {
                        "http://tauri.localhost"
                    } else {
                        "tauri://localhost"
                    }
                    .parse()
                    .unwrap(),
                    body: tauri::ipc::InvokeBody::Json(body),
                    headers: Default::default(),
                    invoke_key: tauri::test::INVOKE_KEY.to_string(),
                },
            )
        };

        // State is intentionally unmanaged so this exercises Tauri's generated
        // argument decoder without reading or writing app storage.
        let snake_case_error = invoke(
            serde_json::json!({
                "id": 2,
                "final_text": "edited text",
                "revision_reason": "manual_edit",
            }),
            0,
        )
        .unwrap_err()
        .to_string();
        assert!(
            snake_case_error.contains("state not managed for field `state`"),
            "the History payload should decode before state lookup: {snake_case_error}"
        );

        let camel_case_error = invoke(
            serde_json::json!({
                "id": 2,
                "finalText": "edited text",
                "revisionReason": "manual_edit",
            }),
            2,
        )
        .unwrap_err()
        .to_string();
        assert!(
            camel_case_error.contains("missing required key final_text"),
            "the command must retain the snake_case request contract: {camel_case_error}"
        );
    }

    async fn test_decode_stub(
        id: i64,
        final_text: String,
        revision_reason: Option<String>,
        app: tauri::AppHandle<tauri::test::MockRuntime>,
        state: State<'_, AppState>,
    ) -> Result<super::ManualHistorySaveResponse, String> {
        let _ = (id, final_text, revision_reason, app, state);
        panic!("the generated IPC decoder must reject unmanaged state before calling the body")
    }

    #[test]
    fn manual_history_save_response_serializes_frontend_field_names() {
        let response = super::ManualHistorySaveResponse {
            revision_id: Some(41),
            changed: true,
            learning: crate::dictionary_learn::ManualHistoryLearningStatus::Pending {
                hits: 1,
                required: 3,
            },
        };
        assert_eq!(
            serde_json::to_value(response).unwrap(),
            serde_json::json!({
                "revision_id": 41,
                "changed": true,
                "learning": {
                    "status": "pending",
                    "hits": 1,
                    "required": 3,
                },
            })
        );
    }
}
