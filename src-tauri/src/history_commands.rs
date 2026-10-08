//! History read, export, retry, delete, and re-clean IPC commands.

use crate::{
    asr, asr_request_snapshot_for_options_with_metrics, chrono_like_id, chunker,
    cleanup_failure_status, clipboard_text_for_snippets, context, finalize_text_for_scene,
    guard_final_output_for_scene, lexicon, llm, local_cleanup_or_raw_for_scene, lock_recover,
    metrics, paste, queue, release_operation, snippets, store, try_claim_operation, AppState,
    CleanupDecision, FinalizationContext, OperationLease, CLEANUP_STATUS_AI_SUCCESS,
    CLEANUP_STATUS_LOCAL_ONLY, CLEANUP_STATUS_PRESERVATION_GUARD, CLEANUP_STATUS_SNIPPET_BYPASS,
};
use tauri::{Emitter, Manager, State};
use tokio_util::sync::CancellationToken;

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
    let processing_guard = capture_history_processing_guard(&state);
    let raw_text = store::history_raw_text(&dir, id).map_err(|e| e.to_string())?;
    if raw_text.trim().is_empty() {
        return Err("这条历史记录没有可重新整理的原文".into());
    }
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
    let mut policy_without_examples = policy.clone();
    policy_without_examples.style_examples_approved = false;
    policy_without_examples.style_example_input = None;
    policy_without_examples.style_example_output = None;
    policy_without_examples.style_example_pairs.clear();
    let family = family_from_scene(&scene, &settings.context_mappings);
    let input_kind = policy.input_kind;
    let prepared_lexicon = crate::prepare_lexicon_transcript_with_provenance(
        Some(&dir),
        &settings.dictionary,
        &raw_text,
        family,
        input_kind,
    );
    let raw_text = prepared_lexicon.text.clone();
    let pairs_hint = prepared_lexicon.pairs_hint.clone();
    let intent = llm::CleanupIntent::selected_text(operation, &raw_text);
    if !try_claim_operation(&state, OperationLease::HistoryReclean) {
        return Err("Dictation is active; try history cleanup again after it finishes".into());
    }
    if let Err(error) = ensure_history_processing_current(&state, &processing_guard) {
        release_operation(&state, OperationLease::HistoryReclean);
        return Err(error);
    }
    let (mut final_text, mut degraded, mut degraded_reason, mut cleanup_status) =
        if !settings.cleanup_enabled {
            (raw_text.clone(), false, None, CLEANUP_STATUS_LOCAL_ONLY)
        } else {
            let cleanup_endpoint = settings.cleanup_endpoint();
            let cleanup_model = settings.cleanup_request_model();
            let cleanup_key = settings.cleanup_credential().to_owned();
            let profile_id = scene.profile_id.clone();
            let policy_base = policy_without_examples.clone();
            let app_state = state.inner();
            let cleanup_scope = crate::cleanup_quota_scope(&settings);
            let result = queue::execute_with_retry_scoped_cancelled(
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
                    let raw_text = raw_text.clone();
                    let pairs_hint = pairs_hint.clone();
                    let language = settings.language.clone();
                    let intent = intent.clone();
                    let policy_base = policy_base.clone();
                    let profile_id = profile_id.clone();
                    async move {
                        let result =
                            llm::cleanup_with_model_and_limits_and_language_and_profile_and_intent(
                                &cleanup_endpoint,
                                &cleanup_model,
                                &raw_text,
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
            .await;
            match result {
                Ok((text, limits)) if !text.trim().is_empty() => {
                    state.gate.update_llm_for(&cleanup_scope, &limits);
                    (text, false, None, CLEANUP_STATUS_AI_SUCCESS)
                }
                Ok((_, limits)) => {
                    state.gate.update_llm_for(&cleanup_scope, &limits);
                    let fallback = local_cleanup_or_raw_for_scene(&raw_text, family, input_kind);
                    (
                        fallback.clone(),
                        true,
                        Some("llm_cleanup_empty"),
                        cleanup_failure_status(&raw_text, &fallback),
                    )
                }
                Err(error) => {
                    if matches!(&error, queue::ExecuteError::Cancelled) {
                        release_operation(&state, OperationLease::HistoryReclean);
                        return Err(
                            "History cleanup cancelled because processing settings changed".into(),
                        );
                    }
                    let guard_rejected = matches!(
                        &error,
                        queue::ExecuteError::Operation(error)
                            if llm::is_preservation_guard_error(error)
                    );
                    log::warn!("history re-clean failed; using local cleanup: {error}");
                    let fallback = local_cleanup_or_raw_for_scene(&raw_text, family, input_kind);
                    (
                        fallback.clone(),
                        true,
                        Some(if guard_rejected {
                            "preservation_guard"
                        } else {
                            "llm_cleanup_failed"
                        }),
                        if guard_rejected {
                            CLEANUP_STATUS_PRESERVATION_GUARD
                        } else {
                            cleanup_failure_status(&raw_text, &fallback)
                        },
                    )
                }
            }
        };
    let (guarded, guard_rejected) = guard_final_output_for_scene(
        &raw_text,
        &final_text,
        FinalizationContext {
            family,
            input_kind,
            operation,
            revision_source: None,
            prepared_transcript: None,
            revision_authorizations: &[],
            promoted_pair_protections: &prepared_lexicon.promoted_pair_protections,
        },
    );
    final_text = guarded;
    if guard_rejected {
        degraded = true;
        degraded_reason = Some("preservation_guard");
        cleanup_status = CLEANUP_STATUS_PRESERVATION_GUARD;
    }
    if degraded_reason == Some("preservation_guard") {
        state.metrics.record_cleanup_guard_fallback();
    }
    let output_result = with_current_history_processing(&state, &processing_guard, || {
        paste::copy(&app, &final_text)
            .map_err(|_| "Failed to copy the history result to the clipboard".to_owned())?;
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
        .map_err(|error| error.to_string())?;
        store::update_history_revision_state(
            &dir,
            id,
            degraded,
            degraded_reason,
            if degraded { "degraded" } else { "copied" },
            Some(cleanup_status),
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    });
    if let Err(error) = output_result {
        release_operation(&state, OperationLease::HistoryReclean);
        return Err(error);
    }
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
    let processing_guard = capture_history_processing_guard(&state);
    if !try_claim_operation(&state, OperationLease::HistoryReclean) {
        return Err("Dictation is active; try retry again after it finishes".into());
    }
    let result = retry_dictation_inner(id, app, &state, &processing_guard).await;
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
    transcribe_complete_audio(wav, chunk_length_secs, state, request, cancellation)
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

pub(crate) async fn transcribe_complete_audio(
    wav: &[u8],
    chunk_length_secs: usize,
    state: &AppState,
    request: &crate::AsrRequestSnapshot,
    cancellation: CancellationToken,
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
        let transcript = queue::execute_with_retry_scoped_cancelled(
            &state.gate,
            queue::RequestKind::Asr,
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
) -> Result<(), FullAudioTranscriptionError> {
    let chunk_wav = chunker::encode_wav(&chunk.samples).map_err(|_| {
        FullAudioTranscriptionError::Failed("Audio chunk could not be encoded".into())
    })?;
    let transcript = queue::execute_with_retry_scoped_cancelled(
        &state.gate,
        queue::RequestKind::Asr,
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
) -> Result<(), String> {
    ensure_history_processing_current(state, processing_guard)?;
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
    if settings.output_mode != "auto" {
        retry_policy.output_mode = Some(settings.output_mode.clone());
    }
    if settings.output_mode == "translation" {
        retry_policy.translation_target_language =
            Some(settings.translation_target_language.clone());
    }
    let mut retry_policy_without_examples = retry_policy.clone();
    retry_policy_without_examples.style_examples_approved = false;
    retry_policy_without_examples.style_example_input = None;
    retry_policy_without_examples.style_example_output = None;
    retry_policy_without_examples.style_example_pairs.clear();
    let asr_request = asr_request_snapshot_for_options_with_metrics(
        &settings,
        &state.models_root,
        asr::AsrOptions {
            api_key: String::new(),
            language: None,
            prompt: asr_prompt.prompt,
            keywords: asr_prompt.keywords,
            model: String::new(),
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
    let prepared = crate::prepare_cleanup_transcript_for_scene(
        Some(&dir),
        &settings,
        &transcript.text,
        family,
        confidence,
        input_kind,
        settings.fuzzy_dictionary_enabled,
    );
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
    let cleanup_route = lexicon::decide_cleanup(
        settings.cleanup_enabled,
        crate::cleanup_intensity_for(&settings, mapping, family, input_kind),
        mapping,
        family,
        &intent,
    );
    let cleanup_decision = match cleanup_route {
        lexicon::CleanupRoute::Provider(_)
            if settings.asr_provider == crate::providers::EngineProvider::AssemblyAi
                && settings.cleanup_credential().trim().is_empty() =>
        {
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
                queue::execute_with_retry_scoped_cancelled(
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
                        let sent_signature =
                            crate::cleanup_projection_signature(None, &attempt_policy);
                        let cleanup_endpoint = cleanup_endpoint.clone();
                        let cleanup_model = cleanup_model.clone();
                        let cleanup_key = cleanup_key.clone();
                        let cleanup_input = cleanup_input.clone();
                        let pairs_hint = pairs_hint.clone();
                        let language = settings.language.clone();
                        let intent = intent.clone();
                        let policy_base = policy_base.clone();
                        let profile_id = profile_id.clone();
                        async move {
                            let result =
                            llm::cleanup_with_model_and_limits_and_language_and_profile_and_intent(
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
                Ok((text, limits)) => {
                    state.gate.update_llm_for(&cleanup_scope, &limits);
                    CleanupDecision::Provider(text)
                }
                Err(error) => {
                    log::warn!("history retry cleanup failed, copying raw transcript: {error}");
                    match error {
                        queue::ExecuteError::Cancelled => return Err("Retry was cancelled".into()),
                        queue::ExecuteError::Operation(error)
                            if llm::is_preservation_guard_error(&error) =>
                        {
                            CleanupDecision::GuardRejected
                        }
                        queue::ExecuteError::Operation(_) => CleanupDecision::Failed,
                    }
                }
            }
        }
        _ => CleanupDecision::Disabled,
    };
    let mut cleanup_status = match &cleanup_decision {
        CleanupDecision::Provider(text) if text.trim().is_empty() => cleanup_failure_status(
            &cleanup_input,
            &local_cleanup_or_raw_for_scene(&cleanup_input, family, input_kind),
        ),
        CleanupDecision::Provider(_) => CLEANUP_STATUS_AI_SUCCESS,
        CleanupDecision::Failed => cleanup_failure_status(
            &cleanup_input,
            &local_cleanup_or_raw_for_scene(&cleanup_input, family, input_kind),
        ),
        CleanupDecision::GuardRejected => CLEANUP_STATUS_PRESERVATION_GUARD,
        CleanupDecision::Disabled if snippet_expansion.is_some() => CLEANUP_STATUS_SNIPPET_BYPASS,
        CleanupDecision::Disabled => CLEANUP_STATUS_LOCAL_ONLY,
    };
    let resolved = finalize_text_for_scene(
        &cleanup_input,
        cleanup_decision,
        FinalizationContext {
            family,
            input_kind,
            operation: intent.operation,
            revision_source: Some(&prepared.revision_source),
            prepared_transcript: Some(&prepared.text),
            revision_authorizations: &prepared.revision_authorizations,
            promoted_pair_protections: &prepared.promoted_pair_protections,
        },
    )
    .map_err(|_| "No speech detected".to_owned())?;
    let final_text = resolved.text;
    let degraded = resolved.degraded;
    let degraded_reason = resolved.degraded_reason;
    if degraded_reason == Some("preservation_guard") {
        cleanup_status = CLEANUP_STATUS_PRESERVATION_GUARD;
        state.metrics.record_cleanup_guard_fallback();
    }
    // Retry is also clipboard-only: the original target guard cannot be
    // reconstructed safely from a history record.
    with_current_history_processing(state, processing_guard, || {
        paste::copy(&app, &final_text).map_err(|e| e.to_string())?;
        let _ = app.emit(
            "dictation://copied",
            serde_json::json!({ "message": "已复制，请手动粘贴" }),
        );
        store::mark_retried_with_texts(
            &dir,
            id,
            Some(&raw_text),
            Some(transcript.original_text()),
            Some(&engine_provenance),
            &final_text,
            degraded,
            degraded_reason,
            Some(cleanup_status),
        )
        .map_err(|e| e.to_string())?;
        store::remove_spool_artifact(&dir, std::path::Path::new(&path));
        Ok(())
    })?;
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
}
