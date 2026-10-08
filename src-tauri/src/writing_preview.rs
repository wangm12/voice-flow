//! Explicit, memory-only writing-mode trials. No window capture or delivery.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::State;
use tokio_util::sync::CancellationToken;

use crate::{context, lexicon, llm, lock_recover, queue, store, AppState};

#[derive(Default)]
pub(crate) struct PreviewManager {
    active: Mutex<Option<(String, CancellationToken)>>,
    cancelled_before_start: Mutex<VecDeque<String>>,
}

impl PreviewManager {
    fn begin(&self, id: &str) -> CancellationToken {
        let mut active = lock_recover(&self.active);
        let mut cancelled = lock_recover(&self.cancelled_before_start);
        if let Some(index) = cancelled.iter().position(|previous| previous == id) {
            cancelled.remove(index);
            let token = CancellationToken::new();
            token.cancel();
            return token;
        }
        if let Some((_, previous)) = active.take() {
            previous.cancel();
        }
        let cancellation = CancellationToken::new();
        *active = Some((id.to_owned(), cancellation.clone()));
        cancellation
    }

    fn finish(&self, id: &str) {
        let mut active = lock_recover(&self.active);
        if active.as_ref().is_some_and(|(current, _)| current == id) {
            *active = None;
        }
    }

    fn cancel(&self, id: &str) {
        if id.is_empty() || id.len() > 128 {
            return;
        }
        let mut active = lock_recover(&self.active);
        if active.as_ref().is_some_and(|(current, _)| current == id) {
            if let Some((_, token)) = active.take() {
                token.cancel();
            }
        } else {
            let mut cancelled = lock_recover(&self.cancelled_before_start);
            if !cancelled.iter().any(|previous| previous == id) {
                if cancelled.len() >= 64 {
                    cancelled.pop_front();
                }
                cancelled.push_back(id.into());
            }
        }
    }
}

impl Drop for PreviewManager {
    fn drop(&mut self) {
        if let Some((_, token)) = lock_recover(&self.active).take() {
            token.cancel();
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct PreviewRequest {
    request_id: String,
    text: String,
    mode: context::WritingMode,
    #[serde(default)]
    compare_saved: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PreviewStatus {
    Model,
    LocalOnly,
    ProviderFallback,
    GuardFallback,
}

#[derive(Serialize)]
pub(crate) struct PreviewResult {
    text: String,
    status: PreviewStatus,
    elapsed_ms: u64,
}

#[derive(Serialize)]
pub(crate) struct PreviewResponse {
    saved: Option<PreviewResult>,
    draft: PreviewResult,
}

fn validate_request(request: &PreviewRequest) -> Result<(), String> {
    if request.request_id.is_empty() || request.request_id.len() > 128 {
        return Err("preview_invalid_request".into());
    }
    if request.text.trim().is_empty() || request.text.len() > 16 * 1024 {
        return Err("preview_invalid_text".into());
    }
    context::validate_writing_modes(std::slice::from_ref(&request.mode))
        .map_err(|_| "preview_invalid_mode".to_owned())
}

fn trial_context(mode: &context::WritingMode) -> context::ContextSnapshot {
    let mut snapshot = context::ContextSnapshot::general();
    snapshot.profile.family = mode.family;
    snapshot.profile.confidence = 1.0;
    snapshot.profile.id = "writing_preview".into();
    snapshot.policy = context::ContextPolicy::for_family(mode.family);
    snapshot.policy.writing_prompt = Some(mode.prompt.clone());
    snapshot.policy.input_kind = match mode.family {
        context::ContextFamily::PromptOrCode => context::FocusKind::CodingPrompt,
        context::ContextFamily::Terminal => context::FocusKind::Terminal,
        context::ContextFamily::FormFilling => context::FocusKind::Form,
        context::ContextFamily::BrowserSearch => context::FocusKind::Search,
        _ => context::FocusKind::Unknown,
    };
    snapshot
}

fn settings_are_current(state: &AppState, generation: u64) -> bool {
    state
        .processing_configuration_generation
        .load(Ordering::Acquire)
        == generation
        && state.exit_state.load(Ordering::Acquire) == 0
}

async fn run_trial(
    state: &AppState,
    settings: &store::Settings,
    generation: u64,
    mode: &context::WritingMode,
    text: &str,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, String> {
    let started = Instant::now();
    if !settings_are_current(state, generation) || cancellation.is_cancelled() {
        return Err("preview_cancelled".into());
    }
    let snapshot = trial_context(mode);
    // A tone trial has no actual App target, context grants, or clipboard.
    // Reuse production preparation, routing, provider adapter and final guard.
    let prepared = crate::prepare_cleanup_transcript_for_scene(
        None,
        settings,
        text,
        mode.family,
        1.0,
        snapshot.policy.input_kind,
        settings.fuzzy_dictionary_enabled,
    );
    let input = &prepared.intent.content;
    let fallback =
        || crate::local_cleanup_or_raw_for_scene(input, mode.family, snapshot.policy.input_kind);
    let (candidate, mut status) =
        match crate::cleanup_route_for(settings, Some(&snapshot), &prepared.intent) {
            lexicon::CleanupRoute::Provider(effort) => {
                let scope = crate::cleanup_quota_scope(settings);
                let result = queue::execute_with_retry_scoped_cancelled(
                    &state.gate,
                    queue::RequestKind::WritingPreview,
                    &scope,
                    || async {
                        if !settings_are_current(state, generation) {
                            return Err(llm::LlmError::ContextAuthorizationChanged);
                        }
                        llm::cleanup_with_model_and_limits_and_language_and_profile_and_intent(
                            &settings.cleanup_endpoint(),
                            &settings.cleanup_request_model(),
                            input,
                            settings.cleanup_credential(),
                            &[],
                            None,
                            Some(&snapshot.policy),
                            Some(&settings.language),
                            Some(&snapshot.profile),
                            Some(&prepared.intent),
                            prepared.pairs_hint.as_deref(),
                            effort,
                            None,
                        )
                        .await
                    },
                    cancellation.clone(),
                )
                .await;
                match result {
                    Ok((text, limits)) if !text.trim().is_empty() => {
                        state.gate.update_llm_for(&scope, &limits);
                        (text, PreviewStatus::Model)
                    }
                    Ok((_, limits)) => {
                        state.gate.update_llm_for(&scope, &limits);
                        (fallback(), PreviewStatus::ProviderFallback)
                    }
                    Err(queue::ExecuteError::Cancelled) => {
                        return Err("preview_cancelled".into());
                    }
                    Err(queue::ExecuteError::Operation(ref error))
                        if llm::is_preservation_guard_error(error) =>
                    {
                        (fallback(), PreviewStatus::GuardFallback)
                    }
                    Err(_) => (fallback(), PreviewStatus::ProviderFallback),
                }
            }
            _ => (fallback(), PreviewStatus::LocalOnly),
        };
    if !settings_are_current(state, generation) || cancellation.is_cancelled() {
        return Err("preview_cancelled".into());
    }
    let (text, guard_rejected) = crate::guard_final_output_for_scene(
        input,
        &candidate,
        crate::FinalizationContext {
            family: mode.family,
            input_kind: snapshot.policy.input_kind,
            operation: prepared.intent.operation,
            revision_source: Some(&prepared.revision_source),
            prepared_transcript: Some(&prepared.text),
            revision_authorizations: &prepared.revision_authorizations,
            promoted_pair_protections: &prepared.promoted_pair_protections,
        },
    );
    if guard_rejected {
        status = PreviewStatus::GuardFallback;
    }
    Ok(PreviewResult {
        text,
        status,
        elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
    })
}

#[tauri::command]
pub(crate) async fn preview_writing_mode(
    state: State<'_, AppState>,
    request: PreviewRequest,
) -> Result<PreviewResponse, String> {
    validate_request(&request)?;
    let (mut settings, generation) = {
        let settings = lock_recover(&state.settings);
        (
            settings.clone(),
            state
                .processing_configuration_generation
                .load(Ordering::Acquire),
        )
    };
    let saved_mode = settings
        .writing_modes
        .iter()
        .find(|mode| mode.id == request.mode.id)
        .cloned();
    // Test the selected tone, independent of a global formatting override.
    settings.output_mode = "auto".into();
    settings.snippets.clear();
    settings.context_mappings.clear();
    let cancellation = state.writing_preview.begin(&request.request_id);
    let result = tokio::time::timeout(Duration::from_secs(90), async {
        let saved = if request.compare_saved {
            if let Some(mode) = saved_mode.as_ref() {
                Some(
                    run_trial(
                        &state,
                        &settings,
                        generation,
                        mode,
                        &request.text,
                        &cancellation,
                    )
                    .await?,
                )
            } else {
                None
            }
        } else {
            None
        };
        let draft = run_trial(
            &state,
            &settings,
            generation,
            &request.mode,
            &request.text,
            &cancellation,
        )
        .await?;
        Ok(PreviewResponse { saved, draft })
    })
    .await
    .unwrap_or_else(|_| {
        cancellation.cancel();
        Err("preview_timeout".into())
    });
    state.writing_preview.finish(&request.request_id);
    // Deliberately no History, learner, clipboard or target-App writes.
    result
}

#[tauri::command]
pub(crate) fn cancel_writing_preview(state: State<'_, AppState>, request_id: String) {
    state.writing_preview.cancel(&request_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_before_command_dispatch_prevents_a_late_request() {
        let manager = PreviewManager::default();
        manager.cancel("pending");
        assert!(manager.begin("pending").is_cancelled());
        assert!(!manager.begin("current").is_cancelled());
    }

    #[test]
    fn trials_have_no_context_text_or_style_examples() {
        let mode = context::builtin_writing_modes().remove(0);
        let snapshot = trial_context(&mode);
        assert_eq!(
            snapshot.policy.writing_prompt.as_deref(),
            Some(mode.prompt.as_str())
        );
        assert!(!snapshot.policy.style_examples_approved);
        assert!(snapshot.policy.style_example_pairs.is_empty());
        assert!(snapshot.policy.style_example_input.is_none());
    }

    #[test]
    fn old_cancellation_and_completion_cannot_clear_a_new_trial() {
        let manager = PreviewManager::default();
        let old = manager.begin("old");
        let current = manager.begin("new");
        assert!(old.is_cancelled());
        manager.cancel("old");
        manager.finish("old");
        assert!(!current.is_cancelled());
        manager.cancel("new");
        assert!(current.is_cancelled());
    }

    #[test]
    fn validates_text_bytes_and_prompt_before_any_provider_request() {
        let mut request = PreviewRequest {
            request_id: "trial".into(),
            text: "测试一下 API".into(),
            mode: context::builtin_writing_modes().remove(0),
            compare_saved: false,
        };
        assert!(validate_request(&request).is_ok());
        request.text = "中".repeat(6000);
        assert_eq!(
            validate_request(&request).unwrap_err(),
            "preview_invalid_text"
        );
        request.text = "测试".into();
        request.mode.prompt.clear();
        assert_eq!(
            validate_request(&request).unwrap_err(),
            "preview_invalid_mode"
        );
    }
}
