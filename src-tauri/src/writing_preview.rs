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
    Off,
    LocalOnly,
    ProviderFallback,
    GuardFallback,
}

#[derive(Serialize)]
pub(crate) struct PreviewResult {
    text: String,
    status: PreviewStatus,
    elapsed_ms: u64,
    /// Only the actual shared adapter's accepted reply, before final guard.
    /// None means unavailable, not proof that a provider was never attempted.
    model_candidate: Option<String>,
    /// Complete parsed provider text before shared cleanup transformations.
    /// This is preview-only and is never added to normal dictation history.
    provider_candidate: Option<String>,
    cleanup_transformations: Vec<String>,
    cleanup_rejection: Option<llm::CleanupRejectionFact>,
    provider_error: Option<llm::LlmError>,
    final_guard_rejected: bool,
}

impl PreviewResult {
    fn local(text: String, off: bool) -> Self {
        Self {
            text,
            status: if off {
                PreviewStatus::Off
            } else {
                PreviewStatus::LocalOnly
            },
            elapsed_ms: 0,
            model_candidate: None,
            provider_candidate: None,
            cleanup_transformations: Vec::new(),
            cleanup_rejection: None,
            provider_error: None,
            final_guard_rejected: false,
        }
    }
    fn provider(
        result: Result<llm::AcceptedCleanupReply, queue::ExecuteError<llm::LlmError>>,
        local: impl FnOnce() -> String,
    ) -> Result<Self, String> {
        match result {
            Ok(reply) => Ok(Self {
                model_candidate: Some(reply.accepted_text.clone()),
                provider_candidate: Some(reply.provider_candidate),
                cleanup_transformations: reply.transformations,
                cleanup_rejection: None,
                text: reply.accepted_text,
                status: PreviewStatus::Model,
                elapsed_ms: 0,
                provider_error: None,
                final_guard_rejected: false,
            }),
            Err(queue::ExecuteError::Cancelled) => Err("preview_cancelled".into()),
            Err(queue::ExecuteError::Operation(error)) => {
                let status = if llm::is_preservation_guard_error(&error) {
                    PreviewStatus::GuardFallback
                } else {
                    PreviewStatus::ProviderFallback
                };
                Ok(Self {
                    text: local(),
                    status,
                    elapsed_ms: 0,
                    model_candidate: None,
                    provider_candidate: None,
                    cleanup_transformations: Vec::new(),
                    cleanup_rejection: llm::cleanup_rejection(&error),
                    provider_error: Some(error),
                    final_guard_rejected: false,
                })
            }
        }
    }
}

#[derive(Serialize)]
pub(crate) struct PreviewResponse {
    saved: Option<PreviewResult>,
    draft: PreviewResult,
}

// Single route/typed-result owner, also usable with a bounded real HTTP fixture.
// The provider closure is never invoked for a local/Off route.
async fn execute_preview_route<F, Fut>(
    route: lexicon::CleanupRoute,
    off: bool,
    provider: F,
    local: impl FnOnce() -> String,
) -> Result<PreviewResult, String>
where
    F: FnOnce(llm::CleanupEffort) -> Fut,
    Fut: std::future::Future<
        Output = Result<llm::AcceptedCleanupReply, queue::ExecuteError<llm::LlmError>>,
    >,
{
    match route {
        lexicon::CleanupRoute::Provider(effort) => {
            PreviewResult::provider(provider(effort).await, local)
        }
        lexicon::CleanupRoute::LocalOnly => Ok(PreviewResult::local(local(), off)),
    }
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

fn finalize_trial_result(
    mut result: PreviewResult,
    source: &str,
    context: crate::FinalizationContext<'_>,
) -> PreviewResult {
    let (text, guard_reason, rejected_candidate) =
        crate::guard_final_output_with_reason(source, &result.text, context);
    let guard_rejected = guard_reason.is_some();
    if guard_rejected {
        result.status = PreviewStatus::GuardFallback;
        if result.cleanup_rejection.is_none() {
            if let (Some(candidate), Some(reason)) = (rejected_candidate, guard_reason) {
                if result.model_candidate.is_some() {
                    result.cleanup_rejection = Some(llm::CleanupRejectionFact {
                        candidate,
                        reason,
                        stage: llm::CleanupRejectionStage::FinalOutput,
                    });
                }
            }
        }
    }
    result.text = text;
    result.final_guard_rejected = guard_rejected;
    result
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
    let scene = crate::resolve_cleanup_scene(settings, Some(&snapshot));
    let prepared = crate::prepare_cleanup_transcript_for_resolved_scene(
        None,
        settings,
        text,
        scene,
        1.0,
        None,
        settings.fuzzy_dictionary_enabled,
    );
    let input = &prepared.intent.content;
    let fallback = || crate::local_cleanup_or_raw_for_resolved_scene(input, prepared.scene);
    let route = crate::cleanup_route_for_resolved_scene(settings, prepared.scene, &prepared.intent);
    let off = route == lexicon::CleanupRoute::LocalOnly;
    let request_snapshot = &snapshot;
    let request_prepared = &prepared;
    let mut result = execute_preview_route(
        route,
        off,
        |effort| async move {
            let scope = crate::cleanup_quota_scope(settings);
            let result = queue::execute_with_retry_scoped_cancelled(
                &state.gate,
                queue::RequestKind::WritingPreview,
                &scope,
                || async {
                    if !settings_are_current(state, generation) {
                        return Err(llm::LlmError::ContextAuthorizationChanged);
                    }
                    crate::with_cleanup_warmup_model_demand(
                        state,
                        settings.cleanup_provider,
                        llm::cleanup_with_provider_and_local_runtimes_and_intent_and_source(
                            settings.cleanup_provider,
                            &state.mlx_cleanup_runtime,
                            Some(&state.llama_cleanup_runtime),
                            &settings.cleanup_endpoint(),
                            &settings.cleanup_request_model(),
                            input,
                            settings.cleanup_credential(),
                            None,
                            Some(&request_snapshot.policy),
                            Some(&request_snapshot.profile),
                            Some(&request_prepared.intent),
                            request_prepared.pairs_hint.as_deref(),
                            effort,
                            None,
                            Some(text),
                            None,
                            (settings.cleanup_provider == crate::providers::EngineProvider::Custom)
                                .then_some(&settings.custom_cleanup_parameters),
                        ),
                    )
                    .await
                },
                cancellation.clone(),
            )
            .await;
            result.map(|(reply, limits)| {
                state.gate.update_llm_for(&scope, &limits);
                reply
            })
        },
        fallback,
    )
    .await?;
    if !settings_are_current(state, generation) || cancellation.is_cancelled() {
        return Err("preview_cancelled".into());
    }
    result = finalize_trial_result(
        result,
        input,
        crate::FinalizationContext {
            scene: prepared.scene,
            operation: prepared.intent.operation,
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
    );
    result.elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    Ok(result)
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
    #[tokio::test]
    async fn off_route_never_invokes_the_actual_provider_boundary() {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let result = execute_preview_route(
            lexicon::CleanupRoute::LocalOnly,
            true,
            |_| {
                calls.fetch_add(1, Ordering::Relaxed);
                async { panic!("Off invoked provider") }
            },
            || "local".into(),
        )
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert_eq!(result.status, PreviewStatus::Off);
        assert!(result.model_candidate.is_none());
        assert!(result.provider_error.is_none());
    }

    #[tokio::test]
    async fn real_shared_parser_empty_length_and_stop_reach_typed_preview_owner() {
        let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
        for (finish, content, guard) in [
            ("stop", "", Some(llm::GuardReason::EmptyCompletion)),
            ("length", "partial", Some(llm::GuardReason::Truncated)),
            ("stop", "hello", None),
        ] {
            let body = format!(
                "data: {}\n\ndata: [DONE]\n\n",
                serde_json::json!({
                "choices":[{"delta":{"content":content},"finish_reason":finish}]})
            );
            let mut server =
                crate::test_http::spawn_scripted(vec![crate::test_http::ResponseStep {
                    status: 200,
                    body,
                    delay: Duration::ZERO,
                }])
                .await;
            let endpoint = format!("{}/v1/chat/completions", server.url);
            let result = execute_preview_route(
                lexicon::CleanupRoute::Provider(llm::CleanupEffort::Standard),
                false,
                |effort| async move {
                    llm::cleanup_with_model_and_limits_and_language_and_profile_and_intent(
                        &endpoint,
                        "fixture",
                        "hello",
                        "fixture-key",
                        None,
                        None,
                        None,
                        None,
                        None,
                        effort,
                        None,
                    )
                    .await
                    .map(|(text, _)| llm::AcceptedCleanupReply {
                        provider_candidate: text.clone(),
                        accepted_text: text,
                        transformations: Vec::new(),
                        native_request_id: None,
                    })
                    .map_err(queue::ExecuteError::Operation)
                },
                || "local fallback".into(),
            )
            .await
            .unwrap();
            assert!(
                tokio::time::timeout(Duration::from_secs(2), server.receipts.recv())
                    .await
                    .unwrap()
                    .is_some()
            );
            if let Some(reason) = guard {
                assert_eq!(result.status, PreviewStatus::ProviderFallback);
                assert!(
                    matches!(result.provider_error, Some(llm::LlmError::Guard(actual)) if actual == reason)
                );
                assert_eq!(result.text, "local fallback");
                assert!(result.model_candidate.is_none());
            } else {
                assert_eq!(result.status, PreviewStatus::Model);
                assert_eq!(result.model_candidate.as_deref(), Some("hello"));
                assert_eq!(result.provider_candidate.as_deref(), Some("hello"));
                assert!(result.cleanup_transformations.is_empty());
            }
        }
    }

    #[test]
    fn shared_error_serialization_keeps_variants_without_transport_payloads() {
        let value = serde_json::to_value(llm::LlmError::Network("private transport detail".into()))
            .unwrap();
        assert_eq!(
            value,
            serde_json::json!({"kind":"network","guard":null,"native_failure":null})
        );
        let value =
            serde_json::to_value(llm::LlmError::Guard(llm::GuardReason::Truncated)).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"kind":"guard","guard":"truncated","native_failure":null})
        );
        assert_eq!(
            PreviewResult::provider(Err(queue::ExecuteError::Cancelled), || panic!(
                "cancel fallback"
            ))
            .err()
            .as_deref(),
            Some("preview_cancelled")
        );
    }

    #[test]
    fn real_provider_and_fallback_rejections_match_the_shared_preview_wire_fixture() {
        const SOURCE: &str = "Keep API v2.";
        const DAMAGED_CANDIDATE: &str = "Keep API v3.";

        let provider_error = llm::accept_cleanup_reply_with_facts(
            DAMAGED_CANDIDATE.to_owned(),
            &llm::CleanupIntent::implicit(SOURCE),
        )
        .expect_err("the shared cleanup acceptance owner rejects the changed API version");
        let provider_fact = llm::cleanup_rejection(&provider_error)
            .expect("the provider-stage rejection is produced by the shared guard");
        assert_eq!(provider_fact.candidate, DAMAGED_CANDIDATE);
        assert_eq!(provider_fact.reason, llm::GuardReason::ProtectedToken);
        assert_eq!(
            provider_fact.stage,
            llm::CleanupRejectionStage::ProviderOutput
        );

        let local_called = std::cell::Cell::new(false);
        let mut result =
            PreviewResult::provider(Err(queue::ExecuteError::Operation(provider_error)), || {
                local_called.set(true);
                DAMAGED_CANDIDATE.to_owned()
            })
            .expect("provider failure falls back through the real preview owner");
        assert!(local_called.get());
        assert!(result.model_candidate.is_none());

        result = finalize_trial_result(
            result,
            SOURCE,
            crate::FinalizationContext {
                scene: crate::ResolvedCleanupScene::observed(
                    context::ContextFamily::General,
                    context::FocusKind::Editable,
                ),
                operation: llm::CleanupOperation::Cleanup,
                revision_source: Some(SOURCE),
                prepared_transcript: Some(SOURCE),
                revision_authorizations: &[],
                promoted_pair_protections: &[],
                list_revision_proof: None,
                structure_safe_source: None,
                output_projection_list_revision_proof: None,
                layout_source: None,
                exact_fallback: false,
            },
        );
        assert_eq!(result.status, PreviewStatus::GuardFallback);
        assert_eq!(
            result.text, SOURCE,
            "the final guard retains the prepared source"
        );
        assert!(result.final_guard_rejected);
        let rejection = result
            .cleanup_rejection
            .as_ref()
            .expect("provider rejection remains visible after fallback rejection");
        assert_eq!(rejection.stage, llm::CleanupRejectionStage::ProviderOutput);
        assert_eq!(rejection.candidate, DAMAGED_CANDIDATE);

        result.elapsed_ms = 17;
        let actual = serde_json::to_value(PreviewResponse {
            saved: None,
            draft: result,
        })
        .expect("the actual typed preview response serializes");
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../src/test/fixtures/writing-preview-rejected-fallback.json"
        ))
        .expect("shared checked fixture is valid JSON");
        assert_eq!(
            actual, fixture,
            "Rust serde and TypeScript use one wire fixture"
        );
    }
}
