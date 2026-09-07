//! History read, export, retry, delete, and re-clean IPC commands.

use crate::{
    asr, chrono_like_id, cleanup_failure_status, clipboard_text_for_snippets,
    context, current_asr_provider, finalize_text, lexicon, llm, local_cleanup_or_raw, lock_recover,
    metrics, paste, queue, snippets, spoken_translation_target, store, try_claim_operation,
    release_operation, AppState,
    CleanupDecision, OperationLease, CLEANUP_STATUS_AI_SUCCESS, CLEANUP_STATUS_LOCAL_ONLY,
    CLEANUP_STATUS_SNIPPET_BYPASS,
};
use tauri::{Emitter, Manager, State};

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
    let exported = store::export_gold_corpus(&dir, &downloads, &language).map_err(|e| e.to_string())?;
    Ok(exported.directory)
}

#[tauri::command]
pub(crate) fn get_history_audio(id: i64, app: tauri::AppHandle) -> Result<Vec<u8>, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    store::history_audio_bytes(&dir, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn save_verbatim(
    id: i64,
    text: String,
    reviewed: bool,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    store::save_verbatim(&dir, id, &text, reviewed).map_err(|e| e.to_string())
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

#[tauri::command]
pub(crate) fn save_history_revision(
    id: i64,
    final_text: String,
    revision_reason: Option<String>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let reason = revision_reason.as_deref().unwrap_or("manual_edit");
    store::save_history_revision(&dir, id, &final_text, None, None, None, None, reason)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn get_history_revisions(
    id: i64,
    app: tauri::AppHandle,
) -> Result<Vec<store::HistoryRevision>, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    store::get_history_revisions(&dir, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn reclean_history(
    id: i64,
    operation: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let raw_text = store::history_raw_text(&dir, id).map_err(|e| e.to_string())?;
    if raw_text.trim().is_empty() {
        return Err("这条历史记录没有可重新整理的原文".into());
    }
    let operation =
        cleanup_operation_from_name(&operation).ok_or_else(|| "不支持的重新整理模式".to_owned())?;
    let settings = lock_recover(&state.settings).clone();
    let scene = store::history_scene(&dir, id).map_err(|e| e.to_string())?;
    let mut policy = scene.policy.clone().unwrap_or_default();
    if settings.output_mode != "auto" {
        policy.output_mode = Some(settings.output_mode.clone());
    }
    if settings.output_mode == "translation" {
        policy.translation_target_language = Some(settings.translation_target_language.clone());
    }
    let family = family_from_scene(&scene, &settings.context_mappings);
    let (raw_text, pairs_hint) =
        crate::prepare_lexicon_transcript(Some(&dir), &settings.dictionary, &raw_text);
    let intent = llm::CleanupIntent::selected_text(operation, &raw_text);
    if !try_claim_operation(&state, OperationLease::HistoryReclean) {
        return Err("Dictation is active; try history cleanup again after it finishes".into());
    }
    let (final_text, degraded, degraded_reason, cleanup_status) = if !settings.cleanup_enabled {
        (
            local_cleanup_or_raw(&raw_text, family),
            false,
            None,
            CLEANUP_STATUS_LOCAL_ONLY,
        )
    } else {
        let cleanup_endpoint = settings.cleanup_endpoint();
        let cleanup_model = settings.cleanup_request_model();
        let cleanup_key = settings.cleanup_credential().to_owned();
        let result = queue::execute_with_retry(&state.gate, queue::RequestKind::HistoryLlm, || {
            llm::cleanup_with_model_and_limits_and_language_and_profile_and_intent(
                &cleanup_endpoint,
                &cleanup_model,
                &raw_text,
                &cleanup_key,
                &[],
                None,
                Some(&policy),
                Some(settings.language.as_str()),
                None,
                Some(&intent),
                pairs_hint.as_deref(),
                llm::CleanupEffort::Command,
                None,
            )
        })
        .await;
        match result {
            Ok((text, limits)) if !text.trim().is_empty() => {
                state.gate.update_llm(&limits);
                (text, false, None, CLEANUP_STATUS_AI_SUCCESS)
            }
            Ok((_, limits)) => {
                state.gate.update_llm(&limits);
                let fallback = local_cleanup_or_raw(&raw_text, family);
                (
                    fallback.clone(),
                    true,
                    Some("llm_cleanup_empty"),
                    cleanup_failure_status(&raw_text, &fallback),
                )
            }
            Err(error) => {
                log::warn!("history re-clean failed; using local cleanup: {error}");
                let fallback = local_cleanup_or_raw(&raw_text, family);
                (
                    fallback.clone(),
                    true,
                    Some("llm_cleanup_failed"),
                    cleanup_failure_status(&raw_text, &fallback),
                )
            }
        }
    };
    if paste::copy(&app, &final_text).is_err() {
        release_operation(&state, OperationLease::HistoryReclean);
        return Err("Failed to copy the history result to the clipboard".into());
    }
    store::save_history_revision(
        &dir,
        id,
        &final_text,
        Some(cleanup_status),
        Some(&intent),
        Some(&settings.cleanup_model),
        Some(&policy),
        "ai_reclean",
    )
    .map_err(|error| {
        release_operation(&state, OperationLease::HistoryReclean);
        error.to_string()
    })?;
    store::update_history_revision_state(
        &dir,
        id,
        degraded,
        degraded_reason,
        if degraded { "degraded" } else { "copied" },
        Some(cleanup_status),
    )
    .map_err(|error| {
        release_operation(&state, OperationLease::HistoryReclean);
        error.to_string()
    })?;
    let _ = app.emit(
        "history://recleaned",
        serde_json::json!({ "dictation_id": id, "degraded": degraded }),
    );
    release_operation(&state, OperationLease::HistoryReclean);
    Ok(())
}

#[tauri::command]
pub(crate) fn delete_history(id: i64, app: tauri::AppHandle) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    store::delete_history(&dir, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn retry_dictation(
    id: i64,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if !try_claim_operation(&state, OperationLease::HistoryReclean) {
        return Err("Dictation is active; try retry again after it finishes".into());
    }
    let result = retry_dictation_inner(id, app, &state).await;
    release_operation(&state, OperationLease::HistoryReclean);
    result
}

async fn retry_dictation_inner(
    id: i64,
    app: tauri::AppHandle,
    state: &AppState,
) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let path = store::failed_spool(&dir, id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Audio spool is no longer available".to_owned())?;
    let wav = store::read_spool_file(std::path::Path::new(&path))
        .map_err(|_| "Audio spool is no longer available".to_owned())?;
    let settings = lock_recover(&state.settings).clone();
    let scene = store::history_scene(&dir, id).map_err(|e| e.to_string())?;
    let pairs = store::list_learn_pairs(&dir).unwrap_or_default();
    let scope = lexicon::PromptScope::from_history(
        scene.profile_id.as_deref(),
        scene.family.as_deref(),
        scene.browser_host.as_deref(),
    );
    let asr_prompt = lexicon::build_asr_prompt_shaped(
        &settings.dictionary,
        scene.policy.as_ref(),
        &pairs,
        Some(&scope),
        lexicon::asr_prompt_shape_for(settings.asr_provider, &settings.asr_model),
        None,
    );
    let mut retry_policy = scene.policy.clone().unwrap_or_default();
    if settings.output_mode != "auto" {
        retry_policy.output_mode = Some(settings.output_mode.clone());
    }
    if settings.output_mode == "translation" {
        retry_policy.translation_target_language =
            Some(settings.translation_target_language.clone());
    }
    let provider = current_asr_provider(state);
    let options = asr::AsrOptions {
        api_key: settings.asr_credential().to_owned(),
        language: asr::normalize_language(Some(settings.language.as_str())).map(str::to_owned),
        prompt: asr_prompt,
        model: asr::resolve_recognition_model(
            &settings.asr_model,
            Some(settings.language.as_str()),
        )
        .to_owned(),
    };
    let transcript = {
        let _latency = state.metrics.timer(metrics::MetricKind::FinalAsr);
        queue::execute_with_retry(&state.gate, queue::RequestKind::Asr, || {
            provider.transcribe_batch(wav.clone(), options.clone())
        })
        .await
        .map_err(|e| e.to_string())?
    };
    state.gate.update_asr(&transcript.limits);
    let family = family_from_scene(&scene, &settings.context_mappings);
    let mapping = scene
        .profile_id
        .as_deref()
        .and_then(|id| lexicon::mapping_for_profile(&settings.context_mappings, id));
    let confidence = if scene.profile_id.is_some() { 0.88 } else { 0.0 };
    let (raw_text, pairs_hint) = crate::prepare_lexicon_transcript(
        Some(&dir),
        &settings.dictionary,
        &crate::prepare_spoken_transcript(&transcript.text, family, confidence),
    );
    let clipboard = snippets::read_clipboard_if_needed(&settings.snippets, &raw_text, || {
        clipboard_text_for_snippets(&app)
    });
    let snippet_expansion =
        snippets::resolve_exact_with_clipboard(&settings.snippets, &raw_text, clipboard.as_deref());
    let intent = llm::parse_cleanup_intent(&raw_text, spoken_translation_target(&settings));
    let cleanup_input = snippet_expansion
        .clone()
        .unwrap_or_else(|| intent.content.clone());
    let cleanup_route = lexicon::decide_cleanup(
        settings.cleanup_enabled,
        llm::CleanupIntensity::parse(&settings.cleanup_intensity)
            .unwrap_or(llm::CleanupIntensity::Heavy),
        mapping,
        family,
        &intent,
    );
    let cleanup_decision = match cleanup_route {
        lexicon::CleanupRoute::Provider(effort) if snippet_expansion.is_none() => {
            let cleanup_result = {
                let _latency = state.metrics.timer(metrics::MetricKind::Cleanup);
                let cleanup_endpoint = settings.cleanup_endpoint();
                let cleanup_model = settings.cleanup_request_model();
                let cleanup_key = settings.cleanup_credential().to_owned();
                queue::execute_with_retry(&state.gate, queue::RequestKind::Llm, || {
                    llm::cleanup_with_model_and_limits_and_language_and_profile_and_intent(
                        &cleanup_endpoint,
                        &cleanup_model,
                        &cleanup_input,
                        &cleanup_key,
                        &[],
                        None,
                        Some(&retry_policy),
                        Some(settings.language.as_str()),
                        None,
                        Some(&intent),
                        pairs_hint.as_deref(),
                        effort,
                        None,
                    )
                })
                .await
            };
            match cleanup_result {
                Ok((text, limits)) => {
                    state.gate.update_llm(&limits);
                    CleanupDecision::Provider(text)
                }
                Err(error) => {
                    log::warn!("history retry cleanup failed, copying raw transcript: {error}");
                    CleanupDecision::Failed
                }
            }
        }
        _ => CleanupDecision::Disabled,
    };
    let cleanup_status = match &cleanup_decision {
        CleanupDecision::Provider(text) if text.trim().is_empty() => {
            cleanup_failure_status(&cleanup_input, &local_cleanup_or_raw(&cleanup_input, family))
        }
        CleanupDecision::Provider(_) => CLEANUP_STATUS_AI_SUCCESS,
        CleanupDecision::Failed => {
            cleanup_failure_status(&cleanup_input, &local_cleanup_or_raw(&cleanup_input, family))
        }
        CleanupDecision::Disabled if snippet_expansion.is_some() => CLEANUP_STATUS_SNIPPET_BYPASS,
        CleanupDecision::Disabled => CLEANUP_STATUS_LOCAL_ONLY,
    };
    let resolved =
        finalize_text(
            &cleanup_input,
            cleanup_decision,
            family,
            &crate::load_learn_pairs(Some(&dir)),
            &settings.dictionary,
        )
        .map_err(|_| "No speech detected".to_owned())?;
    let final_text = resolved.text;
    let degraded = resolved.degraded;
    let degraded_reason = resolved.degraded_reason;
    // Retry is also clipboard-only: the original target guard cannot be
    // reconstructed safely from a history record.
    paste::copy(&app, &final_text).map_err(|e| e.to_string())?;
    let _ = app.emit(
        "dictation://copied",
        serde_json::json!({ "message": "已复制，请手动粘贴" }),
    );
    store::mark_retried_with_texts(
        &dir,
        id,
        Some(&raw_text),
        &final_text,
        degraded,
        degraded_reason,
        Some(cleanup_status),
    )
    .map_err(|e| e.to_string())?;
    store::remove_spool_artifact(&dir, std::path::Path::new(&path));
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
}
