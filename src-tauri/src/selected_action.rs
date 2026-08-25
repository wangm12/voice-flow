//! Selected-text hotkey and preview delivery commands.

use crate::{
    arm_undo_transaction, context, copy_text, delivery, dictionary_learn,
    emit_selected_action_state, finish_with_delivery, hotkey, lock_recover, paste, permissions,
    release_operation, start_selected_action_with_feedback, verify_delivery_target, AppState,
    CLEANUP_STATUS_AI_SUCCESS,
};
use crate::dictation::{self, OperationLease, Phase};
use tauri::State;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub(crate) struct SelectedActionSession {
    pub(crate) selected_text: String,
    pub(crate) selection_fingerprint: u64,
    pub(crate) target_guard: context::TargetAppGuard,
    pub(crate) onboarding_trial: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct SelectedActionPreview {
    pub(crate) session: SelectedActionSession,
    pub(crate) session_generation: u64,
    pub(crate) context: context::ContextSnapshot,
}

pub(crate) fn clear_selected_action(state: &AppState) {
    state
        .selected_action
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
}

pub(crate) fn clear_selected_preview(state: &AppState) {
    state
        .selected_preview
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
}

fn selected_preview_lease_is_current(state: &AppState, generation: u64) -> bool {
    let current_generation = lock_recover(&state.manager).session_generation;
    let lease = *state
        .operation_lease
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    current_generation == generation && lease == OperationLease::LiveDictation
}

pub(crate) fn selected_preview_completion_is_current(
    phase: Phase,
    current_generation: u64,
    expected_generation: u64,
    lease: OperationLease,
) -> bool {
    phase == Phase::Idle
        && current_generation == expected_generation
        && lease == OperationLease::LiveDictation
}

pub(crate) fn take_preview_if_current(
    preview: &mut Option<SelectedActionPreview>,
    current_generation: u64,
    lease: OperationLease,
) -> Result<SelectedActionPreview, String> {
    match preview.as_ref() {
        None => Err("Selected-text preview is no longer available".into()),
        Some(value)
            if current_generation != value.session_generation
                || lease != OperationLease::LiveDictation =>
        {
            Err("Selected-text preview is stale".into())
        }
        Some(_) => Ok(preview
            .take()
            .expect("selected-text preview was present after the stale check")),
    }
}

pub(crate) fn should_invalidate_selected_preview(
    had_preview: bool,
    lease: OperationLease,
    phase: Phase,
) -> bool {
    had_preview || (lease == OperationLease::LiveDictation && phase == Phase::Idle)
}

fn take_current_selected_preview(state: &AppState) -> Result<SelectedActionPreview, String> {
    let current_generation = lock_recover(&state.manager).session_generation;
    let lease = *state
        .operation_lease
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut preview = state
        .selected_preview
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    take_preview_if_current(&mut preview, current_generation, lease)
}

pub(crate) async fn handle_selected_action_hotkey(
    app: &tauri::AppHandle,
    state: &AppState,
) {
    if hotkey::is_suspended() {
        return;
    }
    let phase = lock_recover(&state.manager).phase;
    match phase {
        Phase::Idle => {
            let _ = start_selected_action_with_feedback(app, state).await;
        }
        Phase::Recording => {
            let selected = state
                .selected_action
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some();
            if selected {
                let _ = dictation::stop_internal(app, state).await;
            }
        }
        Phase::Starting | Phase::Stopping | Phase::Processing => {}
    }
}

async fn paste_selected_text(
    app: &tauri::AppHandle,
    state: &AppState,
    text: &str,
    session: &SelectedActionSession,
    accessibility: bool,
    cancellation: CancellationToken,
) -> Result<paste::InsertOutcome, String> {
    let app = app.clone();
    let text = text.to_owned();
    let expected_target = session.target_guard.clone();
    let expected_text = session.selected_text.clone();
    let expected_fingerprint = session.selection_fingerprint;
    let (mappings, browser_access_enabled) = {
        let current = lock_recover(&state.context);
        (current.mappings.clone(), current.browser_access_enabled)
    };
    let restore_pid = expected_target.pid;
    let restore_window = expected_target.window_id;
    tokio::task::spawn_blocking(move || {
        struct PasteYieldGuard;
        impl Drop for PasteYieldGuard {
            fn drop(&mut self) {
                crate::island_window::end_paste_yield();
            }
        }
        let _yield = PasteYieldGuard;
        crate::island_window::prepare_for_paste(&app);
        let _ = paste::restore_delivery_target_if_needed(restore_pid, restore_window);
        let capture_app = app.clone();
        let verify_target = move || {
            verify_delivery_target(&expected_target, &mappings, browser_access_enabled)?;
            let selection =
                paste::capture_selected_text(&capture_app, accessibility).map_err(|error| {
                    match error {
                        paste::PasteError::Accessibility => paste::PasteError::Accessibility,
                        paste::PasteError::SelectionUnavailable => {
                            paste::PasteError::SelectionChanged
                        }
                        other => other,
                    }
                })?;
            if selection.fingerprint != expected_fingerprint || selection.text != expected_text {
                return Err(paste::PasteError::SelectionChanged);
            }
            Ok(())
        };
        paste::insert(
            &app,
            &text,
            accessibility,
            cancellation,
            verify_target,
            restore_pid,
        )
    })
    .await
    .map_err(|error| format!("selected text paste worker failed: {error}"))?
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn confirm_selected_action_preview(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    final_text: String,
) -> Result<String, String> {
    let final_text = final_text.trim().to_owned();
    if final_text.is_empty() {
        return Err("Preview text cannot be empty".into());
    }
    if final_text.chars().count() > 100_000 {
        return Err("Preview text is too long".into());
    }
    let preview = take_current_selected_preview(&state)?;
    if !selected_preview_lease_is_current(&state, preview.session_generation) {
        return Err("Selected-text preview is stale".into());
    }

    let pid = preview.session.target_guard.pid;
    let window_id = preview.session.target_guard.window_id;
    let activation = tokio::task::spawn_blocking(move || {
        paste::restore_delivery_target_if_needed(pid, window_id)
    })
        .await
        .map_err(|error| format!("target activation worker failed: {error}"))
        .and_then(|result| result.map_err(|error| error.to_string()));
    if let Err(error) = activation {
        if selected_preview_lease_is_current(&state, preview.session_generation) {
            release_operation(&state, OperationLease::LiveDictation);
        }
        return Err(error);
    }
    if !selected_preview_lease_is_current(&state, preview.session_generation) {
        return Err("Selected-text preview is stale".into());
    }

    let paste_result = paste_selected_text(
        &app,
        &state,
        &final_text,
        &preview.session,
        permissions::check().accessibility,
        CancellationToken::new(),
    )
    .await;
    match paste_result {
        Ok(outcome) => {
            let result = delivery::DeliveryResult::from_insert_verified(outcome.verified);
            arm_undo_transaction(
                &state,
                preview.session_generation,
                &preview.session.target_guard,
                outcome.post_insert_input_fingerprint,
                result.method.as_str(),
                outcome.used_keyboard_paste,
            );
            dictionary_learn::maybe_observe_after_paste(
                &app,
                &state,
                outcome.value_after.as_deref(),
                outcome.verified,
                &preview.session.target_guard,
                Some(&preview.context),
            );
            emit_selected_action_state(&app, "replaced");
            finish_with_delivery(
                &app,
                &state,
                if outcome.verified { "done" } else { "copied" },
                Some(&preview.context),
                result.method.as_str(),
                result.fallback_reason,
                Some(CLEANUP_STATUS_AI_SUCCESS),
                Some(preview.session_generation),
            )
            .await;
            Ok("replaced".into())
        }
        Err(error) => {
            log::warn!("selected text replacement failed after preview confirmation: {error}");
            copy_selected_action_preview_result(
                &app,
                &state,
                &final_text,
                &preview.context,
                preview.session_generation,
            )
            .await
        }
    }
}

async fn copy_selected_action_preview_result(
    app: &tauri::AppHandle,
    state: &AppState,
    final_text: &str,
    context: &context::ContextSnapshot,
    session_generation: u64,
) -> Result<String, String> {
    if !selected_preview_lease_is_current(state, session_generation) {
        return Err("Selected-text preview is stale".into());
    }
    if let Err(error) = copy_text(app, final_text, CancellationToken::new()).await {
        if selected_preview_lease_is_current(state, session_generation) {
            release_operation(state, OperationLease::LiveDictation);
        }
        return Err(error);
    }
    if !selected_preview_lease_is_current(state, session_generation) {
        return Err("Selected-text preview is stale".into());
    }
    emit_selected_action_state(app, "copied_instead");
    finish_with_delivery(
        app,
        state,
        "copied",
        Some(context),
        delivery::DeliveryMethod::Clipboard.as_str(),
        Some("selected_action_clipboard_fallback"),
        Some(CLEANUP_STATUS_AI_SUCCESS),
        Some(session_generation),
    )
    .await;
    Ok("copied".into())
}

#[tauri::command]
pub(crate) async fn copy_selected_action_preview(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    final_text: String,
) -> Result<String, String> {
    let final_text = final_text.trim().to_owned();
    if final_text.is_empty() {
        return Err("Preview text cannot be empty".into());
    }
    let preview = take_current_selected_preview(&state)?;
    copy_selected_action_preview_result(
        &app,
        &state,
        &final_text,
        &preview.context,
        preview.session_generation,
    )
    .await
}

#[tauri::command]
pub(crate) fn cancel_selected_action_preview(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let had_preview = state
        .selected_preview
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_some();
    clear_selected_preview(&state);
    let phase = lock_recover(&state.manager).phase;
    let lease = *state
        .operation_lease
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if should_invalidate_selected_preview(had_preview, lease, phase) {
        let generation = {
            let mut manager = lock_recover(&state.manager);
            manager.session_generation = manager.session_generation.wrapping_add(1);
            manager.session_generation
        };
        state.gate.set_session_generation(generation);
        release_operation(&state, OperationLease::LiveDictation);
    }
    emit_selected_action_state(&app, "cancelled");
    Ok(())
}
