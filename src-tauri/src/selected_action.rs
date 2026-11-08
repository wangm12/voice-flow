//! Selected-text hotkey and preview delivery commands.

use crate::dictation::{self, OperationLease, Phase};
use crate::{
    arm_undo_transaction, context, delivery, emit_selected_action_state, finish_with_delivery,
    hotkey, lock_recover, paste, permissions, release_operation,
    start_selected_action_with_feedback, verify_delivery_target, AppState,
    CLEANUP_STATUS_AI_SUCCESS,
};
use tauri::{Manager, State};
use tokio_util::sync::CancellationToken;

/// How a selected-text / look-at-screen replace attempt must be reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextActionDelivery {
    /// Insert verified: emit `replaced`, phase `done`.
    Replaced,
    /// Cmd+V may have been posted (or insert unverified): never copy, phase
    /// `unverified`; the text stays recoverable through History.
    Unverified,
    /// The shortcut was definitely not posted: copy fallback is safe.
    CopyFallback,
    Cancelled,
    MutationUncertain,
}

/// Pure decision shared by both text actions. `result` is `Ok(verified)` or the
/// paste error.
pub(crate) fn text_action_delivery_terminal(
    result: Result<bool, &paste::PasteError>,
) -> TextActionDelivery {
    match result {
        Ok(true) => TextActionDelivery::Replaced,
        Ok(false) => TextActionDelivery::Unverified,
        Err(paste::PasteError::Cancelled) => TextActionDelivery::Cancelled,
        Err(paste::PasteError::MutationUncertain) => TextActionDelivery::MutationUncertain,
        Err(error) if paste_error_may_have_posted_shortcut(error) => TextActionDelivery::Unverified,
        Err(_) => TextActionDelivery::CopyFallback,
    }
}

fn paste_error_may_have_posted_shortcut(error: &paste::PasteError) -> bool {
    error
        .delivery_diagnostic()
        .is_some_and(|diagnostic| diagnostic.keyboard_paste_may_have_been_posted)
}

/// True when the commit must be treated as terminal: the target may already
/// have been mutated (or Cmd+V posted), so nothing may retry or copy.
pub(crate) fn text_action_commit_is_terminal<T>(result: &Result<T, paste::PasteError>) -> bool {
    match result {
        Ok(_) | Err(paste::PasteError::MutationUncertain) => true,
        Err(error) => paste_error_may_have_posted_shortcut(error),
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SelectedActionSession {
    pub(crate) selected_text: String,
    pub(crate) source: paste::CapturedTextActionSource,
    pub(crate) target_guard: context::TargetAppGuard,
    pub(crate) identity: crate::TextActionIdentity,
    pub(crate) delivery_replace_allowed: bool,
    pub(crate) context_policy_revision: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct SelectedActionPreview {
    pub(crate) session: SelectedActionSession,
    pub(crate) session_generation: u64,
    pub(crate) context: context::ContextSnapshot,
}

pub(crate) fn cancelled_preview_event_metadata(
    owner_session_generation: u64,
    cancelled_identity: &crate::TextActionIdentity,
) -> (u64, u64) {
    (owner_session_generation, cancelled_identity.action_sequence)
}

pub(crate) async fn settle_cancelled_commit(
    app: &tauri::AppHandle,
    state: &AppState,
    identity: &crate::TextActionIdentity,
    session_generation: u64,
) {
    if dictation::settle_cancel_during_commit(app, state, identity, session_generation).await {
        let (event_generation, action_sequence) =
            cancelled_preview_event_metadata(session_generation, identity);
        emit_selected_action_state(app, "cancelled", event_generation, Some(action_sequence));
    }
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
    preview_lease_matches(current_generation, generation, lease)
}

pub(crate) fn preview_lease_is_current(state: &AppState, generation: u64) -> bool {
    selected_preview_lease_is_current(state, generation)
}

fn preview_lease_matches(
    current_generation: u64,
    expected_generation: u64,
    lease: OperationLease,
) -> bool {
    current_generation == expected_generation && lease == OperationLease::LiveDictation
}

#[derive(Debug)]
pub(crate) enum PreviewRestoreError {
    Stale,
    Activation(String),
    Target(String),
}

/// Restore focus for either kind of preview, then recheck the live operation
/// lease and strict target identity before allowing any insertion.
pub(crate) async fn restore_preview_target_and_validate(
    app: &tauri::AppHandle,
    target_guard: &context::TargetAppGuard,
    session_generation: u64,
) -> Result<(), PreviewRestoreError> {
    let app = app.clone();
    let target_guard = target_guard.clone();
    tokio::task::spawn_blocking(move || {
        let state = app.state::<AppState>();
        restore_preview_target_and_validate_blocking(&state, &target_guard, session_generation)
    })
    .await
    .map_err(|error| {
        PreviewRestoreError::Activation(format!("target activation worker failed: {error}"))
    })?
}

pub(crate) fn restore_preview_target_and_validate_blocking(
    state: &AppState,
    target_guard: &context::TargetAppGuard,
    session_generation: u64,
) -> Result<(), PreviewRestoreError> {
    if !selected_preview_lease_is_current(state, session_generation) {
        return Err(PreviewRestoreError::Stale);
    }
    paste::restore_delivery_target_if_needed(target_guard.pid, target_guard.window_id)
        .map_err(|error| PreviewRestoreError::Activation(error.to_string()))?;
    if !selected_preview_lease_is_current(state, session_generation) {
        return Err(PreviewRestoreError::Stale);
    }
    let (mappings, browser_access_enabled) = {
        let current = lock_recover(&state.context);
        (current.mappings.clone(), current.browser_access_enabled)
    };
    let require_browser_identity =
        context::is_browser_application(target_guard.bundle_id.as_deref());
    let current = if browser_access_enabled {
        context::detect_snapshot(&mappings, true).target_guard
    } else {
        context::probe_focus_guard()
    };
    let mismatch =
        context::same_field_mismatch_reason(target_guard, &current, require_browser_identity);
    if let Some(reason) = mismatch {
        let error = match reason {
            "input_unavailable" => paste::PasteError::InputUnavailable,
            "input_changed" => paste::PasteError::InputChanged,
            "secure_input" => paste::PasteError::InputUnavailable,
            "target_unavailable" => paste::PasteError::TargetUnavailable,
            _ => paste::PasteError::TargetChanged,
        };
        return Err(PreviewRestoreError::Target(error.to_string()));
    }
    if !selected_preview_lease_is_current(state, session_generation) {
        return Err(PreviewRestoreError::Stale);
    }
    Ok(())
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

fn take_selected_preview_for_transaction(
    state: &AppState,
    transaction_id: &str,
) -> Result<SelectedActionPreview, String> {
    let current_generation = lock_recover(&state.manager).session_generation;
    let lease = *lock_recover(&state.operation_lease);
    let mut preview = lock_recover(&state.selected_preview);
    take_selected_preview_for_transaction_control(
        &mut preview,
        current_generation,
        lease,
        transaction_id,
        |identity| crate::text_action_is_current(state, identity),
    )
}

pub(crate) fn take_selected_preview_for_transaction_control(
    preview: &mut Option<SelectedActionPreview>,
    current_generation: u64,
    lease: OperationLease,
    transaction_id: &str,
    is_current: impl Fn(&crate::TextActionIdentity) -> bool,
) -> Result<SelectedActionPreview, String> {
    let Some(current) = preview.as_ref() else {
        return Err("Selected action preview is no longer available".into());
    };
    if current.session.identity.transaction_id != transaction_id {
        return Err("Selected action preview is stale".into());
    }
    if !is_current(&current.session.identity) {
        return Err("Selected action preview is stale".into());
    }
    take_preview_if_current(preview, current_generation, lease)
}

pub(crate) async fn handle_selected_action_hotkey(app: &tauri::AppHandle, state: &AppState) {
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

#[allow(clippy::too_many_arguments)]
async fn paste_selected_text(
    app: &tauri::AppHandle,
    state: &AppState,
    text: &str,
    session: &SelectedActionSession,
    identity: &crate::TextActionIdentity,
    session_generation: u64,
    accessibility: bool,
    cancellation: CancellationToken,
) -> Result<paste::InsertOutcome, paste::PasteError> {
    let app = app.clone();
    let text = text.to_owned();
    let expected_target = session.target_guard.clone();
    let identity = identity.clone();
    let expected_source = session.source.clone();
    let lease_app = app.clone();
    let (mappings, browser_access_enabled) = {
        let current = lock_recover(&state.context);
        (current.mappings.clone(), current.browser_access_enabled)
    };
    let restore_pid = expected_target.pid;
    tokio::task::spawn_blocking(move || {
        struct PasteYieldGuard;
        impl Drop for PasteYieldGuard {
            fn drop(&mut self) {
                crate::island_window::end_paste_yield();
            }
        }
        let _yield = PasteYieldGuard;
        crate::island_window::prepare_for_paste(&app);
        let app_state = lease_app.state::<AppState>();
        restore_preview_target_and_validate_blocking(
            &app_state,
            &expected_target,
            session_generation,
        )
        .map_err(|error| match error {
            PreviewRestoreError::Stale => paste::PasteError::Cancelled,
            PreviewRestoreError::Activation(message) | PreviewRestoreError::Target(message) => {
                paste::PasteError::Input(message)
            }
        })?;
        let after_target = expected_target.clone();
        let after_mappings = mappings.clone();
        let after_lease_app = lease_app.clone();
        let verify_after = move || {
            let app_state = after_lease_app.state::<AppState>();
            if !preview_lease_is_current(&app_state, session_generation) {
                return Err(paste::PasteError::Cancelled);
            }
            crate::verify_text_action_target(&after_target, &after_mappings, browser_access_enabled)
        };
        let source_for_verify = expected_source.clone();
        let validation_app = lease_app.clone();
        let verify_target = move || {
            let app_state = validation_app.state::<AppState>();
            if !preview_lease_is_current(&app_state, session_generation) {
                return Err(paste::PasteError::Cancelled);
            }
            crate::verify_text_action_target(&expected_target, &mappings, browser_access_enabled)?;
            let source_validation_app = validation_app.clone();
            let current_source = paste::capture_text_action_source_for_target(
                accessibility,
                &expected_target,
                || {
                    let app_state = source_validation_app.state::<AppState>();
                    if !preview_lease_is_current(&app_state, session_generation) {
                        return Err(paste::PasteError::Cancelled);
                    }
                    crate::verify_text_action_target(
                        &expected_target,
                        &mappings,
                        browser_access_enabled,
                    )
                },
            )?;
            if !paste::text_action_source_matches(&source_for_verify, &current_source) {
                return Err(paste::PasteError::SelectionChanged);
            }
            let app_state = validation_app.state::<AppState>();
            if !preview_lease_is_current(&app_state, session_generation) {
                return Err(paste::PasteError::Cancelled);
            }
            Ok(())
        };
        crate::with_text_action_commit(&app_state, &identity, || {
            let result = paste::insert_captured_text_action(
                &app,
                &text,
                &expected_source,
                accessibility,
                cancellation,
                verify_target,
                verify_after,
                restore_pid,
            );
            let terminal = text_action_commit_is_terminal(&result);
            (result, terminal)
        })
        .map_err(|_| paste::PasteError::Cancelled)?
    })
    .await
    .map_err(|error| {
        paste::PasteError::Input(format!("selected text paste worker failed: {error}"))
    })?
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn confirm_selected_action_preview(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    transaction_id: String,
    final_text: String,
) -> Result<String, String> {
    if final_text.trim().is_empty() {
        return Err("Preview text cannot be empty".into());
    }
    if final_text.chars().count() > 100_000 {
        return Err("Preview text is too long".into());
    }
    let preview = take_selected_preview_for_transaction(&state, &transaction_id)?;
    let identity = preview.session.identity.clone();
    if !selected_preview_lease_is_current(&state, preview.session_generation) {
        return Err("Selected-text preview is stale".into());
    }
    if let Err(error) = restore_preview_target_and_validate(
        &app,
        &preview.session.target_guard,
        preview.session_generation,
    )
    .await
    {
        match error {
            PreviewRestoreError::Stale => return Err("Selected-text preview is stale".into()),
            PreviewRestoreError::Activation(_) | PreviewRestoreError::Target(_) => {
                return copy_selected_action_preview_result(
                    &app,
                    &state,
                    &final_text,
                    &preview.context,
                    preview.session_generation,
                    &identity,
                    true,
                )
                .await;
            }
        }
    }

    if let Err(error) = verify_source_snapshot(&app, &state, &preview.session).await {
        return match error {
            paste::PasteError::Cancelled => Err("Selected-text preview is stale".into()),
            _ => {
                copy_selected_action_preview_result(
                    &app,
                    &state,
                    &final_text,
                    &preview.context,
                    preview.session_generation,
                    &identity,
                    true,
                )
                .await
            }
        };
    }

    if !preview.session.delivery_replace_allowed {
        return copy_selected_action_preview_result(
            &app,
            &state,
            &final_text,
            &preview.context,
            preview.session_generation,
            &identity,
            false,
        )
        .await;
    }

    let cancellation = crate::text_action_cancellation(&state, &identity)
        .ok_or_else(|| "Selected-text preview is stale".to_owned())?;
    let paste_result = paste_selected_text(
        &app,
        &state,
        &final_text,
        &preview.session,
        &identity,
        preview.session_generation,
        permissions::check().accessibility,
        cancellation,
    )
    .await;
    match paste_result {
        Ok(outcome) => {
            let result = delivery::DeliveryResult::from_insert_verified(outcome.verified);
            arm_undo_transaction(
                &state,
                preview.session_generation,
                outcome.post_insert_target_guard.as_ref(),
                outcome.post_insert_input_fingerprint,
                outcome.post_insert_field_ticket.as_ref(),
                result.method.as_str(),
                outcome.used_keyboard_paste,
            );
            if outcome.verified {
                emit_selected_action_state(
                    &app,
                    "replaced",
                    preview.session_generation,
                    Some(preview.session.identity.action_sequence),
                );
            }
            finish_with_delivery(
                &app,
                &state,
                if outcome.verified {
                    "done"
                } else {
                    "unverified"
                },
                Some(&preview.context),
                result.method.as_str(),
                result.fallback_reason,
                Some(CLEANUP_STATUS_AI_SUCCESS),
                Some(preview.session_generation),
                None,
            )
            .await;
            crate::clear_text_action(&state, &identity);
            crate::emit_text_action_lifecycle(&app, &identity, "completed");
            Ok(if outcome.verified {
                "replaced"
            } else {
                "unverified"
            }
            .into())
        }
        Err(paste::PasteError::Cancelled) => {
            settle_cancelled_commit(&app, &state, &identity, preview.session_generation).await;
            Err("Selected-text preview is stale".into())
        }
        Err(paste::PasteError::MutationUncertain) => {
            crate::clear_text_action(&state, &identity);
            release_operation(&state, OperationLease::LiveDictation);
            crate::emit_text_action_lifecycle(&app, &identity, "failed");
            Err("Text delivery could not be verified".into())
        }
        Err(error)
            if text_action_delivery_terminal(Err(&error)) == TextActionDelivery::Unverified =>
        {
            // Cmd+V may already have been posted: a copy fallback invites a
            // second paste.
            let result = crate::delivery_result_for_paste(false, error.delivery_diagnostic());
            finish_with_delivery(
                &app,
                &state,
                "unverified",
                Some(&preview.context),
                result.method.as_str(),
                result.fallback_reason,
                Some(CLEANUP_STATUS_AI_SUCCESS),
                Some(preview.session_generation),
                None,
            )
            .await;
            crate::clear_text_action(&state, &identity);
            crate::emit_text_action_lifecycle(&app, &identity, "completed");
            Ok("unverified".into())
        }
        Err(_) => {
            copy_selected_action_preview_result(
                &app,
                &state,
                &final_text,
                &preview.context,
                preview.session_generation,
                &identity,
                true,
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
    identity: &crate::TextActionIdentity,
    target_changed: bool,
) -> Result<String, String> {
    let worker_app = app.clone();
    let text = final_text.to_owned();
    let identity = identity.clone();
    let commit_identity = identity.clone();
    let copied = tokio::task::spawn_blocking(move || {
        let app_state = worker_app.state::<AppState>();
        crate::with_text_action_copy_commit(&app_state, &commit_identity, || {
            paste::copy_if_valid(&worker_app, &text, || {
                if preview_lease_is_current(&app_state, session_generation) {
                    Ok(())
                } else {
                    Err(paste::PasteError::Cancelled)
                }
            })
        })
    })
    .await
    .map_err(|error| format!("selected preview clipboard worker failed: {error}"))?;
    let Ok(copied) = copied else {
        settle_cancelled_commit(app, state, &identity, session_generation).await;
        return Err("Selected-text preview is stale".into());
    };
    let copied = copied.map_err(|error| error.to_string());
    if let Err(error) = copied {
        dictation::settle_failed_copy_during_commit(app, state, &identity, session_generation)
            .await;
        return Err(error);
    }
    emit_selected_action_state(
        app,
        "copied_instead",
        session_generation,
        Some(identity.action_sequence),
    );
    finish_with_delivery(
        app,
        state,
        "copied",
        Some(context),
        delivery::DeliveryMethod::Clipboard.as_str(),
        Some("selected_action_clipboard_fallback"),
        Some(CLEANUP_STATUS_AI_SUCCESS),
        Some(session_generation),
        None,
    )
    .await;
    crate::clear_text_action(state, &identity);
    crate::emit_text_action_lifecycle(app, &identity, "completed");
    Ok(if target_changed {
        "copied_target_changed"
    } else {
        "copied"
    }
    .into())
}

pub(crate) async fn verify_source_snapshot(
    _app: &tauri::AppHandle,
    state: &AppState,
    session: &SelectedActionSession,
) -> Result<(), paste::PasteError> {
    let expected_target = session.target_guard.clone();
    let expected_source = session.source.clone();
    let (mappings, browser_access_enabled) = {
        let context = lock_recover(&state.context);
        (context.mappings.clone(), context.browser_access_enabled)
    };
    tokio::task::spawn_blocking(move || {
        crate::verify_text_action_target(&expected_target, &mappings, browser_access_enabled)?;
        let current = paste::capture_text_action_source_for_target(
            permissions::check().accessibility,
            &expected_target,
            || {
                crate::verify_text_action_target(
                    &expected_target,
                    &mappings,
                    browser_access_enabled,
                )
            },
        )?;
        if paste::text_action_source_matches(&expected_source, &current) {
            Ok(())
        } else {
            Err(paste::PasteError::SelectionChanged)
        }
    })
    .await
    .map_err(|error| {
        paste::PasteError::Input(format!("source validation worker failed: {error}"))
    })?
}

/// A synthetic onboarding fixture is verified locally by its captured owner.
/// Native target/selection proof belongs exclusively to normal selected actions.
fn verify_trial_source(
    session: &SelectedActionSession,
    current: bool,
) -> Result<(), paste::PasteError> {
    let text = &session.selected_text;
    let fingerprint = paste::selection_fingerprint(text);
    let source = &session.source;
    if !current
        || text.is_empty()
        || source.kind != paste::TextActionSourceKind::Selection
        || source.text != *text
        || source.text_fingerprint != fingerprint
        || source.field_fingerprint != Some(fingerprint)
        || source.selection_range != Some((0, text.encode_utf16().count() as i64))
        || source.editable
        || source.copy_only_selection
        || session.delivery_replace_allowed
    {
        return Err(paste::PasteError::SelectionChanged);
    }
    Ok(())
}
fn trial_source_validation(
    session: &SelectedActionSession,
    current: impl FnOnce() -> bool,
) -> Option<Result<(), paste::PasteError>> {
    session
        .identity
        .onboarding_trial
        .as_ref()
        .map(|_| verify_trial_source(session, current()))
}

pub(crate) async fn verify_request_source_snapshot(
    _app: &tauri::AppHandle,
    state: &AppState,
    session: &SelectedActionSession,
) -> Result<(), paste::PasteError> {
    if let Some(result) = trial_source_validation(session, || {
        let trial = session
            .identity
            .onboarding_trial
            .as_ref()
            .expect("trial route selected");
        crate::onboarding::is_current(state, trial)
            && crate::text_action_is_current(state, &session.identity)
            && crate::onboarding::trial_operation_matches(
                &lock_recover(&state.manager),
                trial,
                &session.identity,
            )
    }) {
        return result;
    }
    let expected_target = session.target_guard.clone();
    let expected_source = session.source.clone();
    let (mappings, browser_access_enabled) = {
        let context = lock_recover(&state.context);
        (context.mappings.clone(), context.browser_access_enabled)
    };
    tokio::task::spawn_blocking(move || {
        verify_delivery_target(&expected_target, &mappings, browser_access_enabled)?;
        let current = paste::capture_text_action_source_for_target(
            permissions::check().accessibility,
            &expected_target,
            || verify_delivery_target(&expected_target, &mappings, browser_access_enabled),
        )?;
        if paste::text_action_source_matches(&expected_source, &current) {
            Ok(())
        } else {
            Err(paste::PasteError::SelectionChanged)
        }
    })
    .await
    .map_err(|error| {
        paste::PasteError::Input(format!("source validation worker failed: {error}"))
    })?
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn copy_selected_action_preview(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    transaction_id: String,
    final_text: String,
) -> Result<String, String> {
    if final_text.trim().is_empty() {
        return Err("Preview text cannot be empty".into());
    }
    if final_text.chars().count() > 100_000 {
        return Err("Preview text is too long".into());
    }
    let preview = take_selected_preview_for_transaction(&state, &transaction_id)?;
    copy_selected_action_preview_result(
        &app,
        &state,
        &final_text,
        &preview.context,
        preview.session_generation,
        &preview.session.identity,
        false,
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn cancel_selected_action_preview(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    transaction_id: String,
) -> Result<(), String> {
    let Some(cancel) =
        dictation::cancel_text_action_by_id(&app, &state, &transaction_id, "selected").await
    else {
        return Err("Selected-text preview is stale".into());
    };
    let Some((event_generation, action_sequence)) = cancel.cancelled_event_metadata() else {
        return Err("Text action is committing; its result will be reported".into());
    };
    emit_selected_action_state(&app, "cancelled", event_generation, Some(action_sequence));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        cancelled_preview_event_metadata, preview_lease_matches, text_action_commit_is_terminal,
        text_action_delivery_terminal, TextActionDelivery,
    };
    use crate::dictation::OperationLease;
    use crate::paste::PasteError;

    fn diagnosed(posted: bool) -> PasteError {
        let diagnostic = crate::delivery_diagnostics::DeliveryDiagnostic {
            code: "paste_failed",
            user_reason: "test",
            stage: "paste_shortcut",
            target_app: None,
            accessibility_available: true,
            target_verified: Some(true),
            focused_input_available: Some(true),
            clipboard_snapshot_captured: true,
            clipboard_unchanged_before_write: Some(true),
            clipboard_write_attempted: true,
            clipboard_write_failed: false,
            clipboard_write_owned: Some(true),
            keyboard_paste_attempted: posted,
            keyboard_paste_may_have_been_posted: posted,
            paste_verified: None,
            paste_verification: None,
            paste_verification_delay_ms: None,
            paste_readback_poll_count: None,
            paste_readback_elapsed_ms: None,
            paste_expected_utf16_length: None,
            paste_actual_utf16_length: None,
            paste_readback_mismatch_kind: None,
            paste_full_range_attempted: false,
            clipboard_restore_attempted: false,
            clipboard_restored: None,
        };
        PasteError::Diagnosed {
            source: Box::new(PasteError::Input("x".into())),
            diagnostic: Box::new(diagnostic),
        }
    }

    #[test]
    fn posted_shortcut_error_never_falls_back_to_copy() {
        let posted = diagnosed(true);
        assert_eq!(
            text_action_delivery_terminal(Err(&posted)),
            TextActionDelivery::Unverified
        );
        assert!(text_action_commit_is_terminal::<()>(&Err(posted)));

        let not_posted = diagnosed(false);
        assert_eq!(
            text_action_delivery_terminal(Err(&not_posted)),
            TextActionDelivery::CopyFallback
        );
        assert!(!text_action_commit_is_terminal::<()>(&Err(not_posted)));
        assert_eq!(
            text_action_delivery_terminal(Err(&PasteError::Input("x".into()))),
            TextActionDelivery::CopyFallback
        );

        assert_eq!(
            text_action_delivery_terminal(Err(&PasteError::Cancelled)),
            TextActionDelivery::Cancelled
        );
        assert_eq!(
            text_action_delivery_terminal(Err(&PasteError::MutationUncertain)),
            TextActionDelivery::MutationUncertain
        );
        assert!(text_action_commit_is_terminal::<()>(&Err(
            PasteError::MutationUncertain
        )));
        assert_eq!(
            text_action_delivery_terminal(Ok(true)),
            TextActionDelivery::Replaced
        );
        // Unverified Ok is never reported as replaced.
        assert_eq!(
            text_action_delivery_terminal(Ok(false)),
            TextActionDelivery::Unverified
        );
    }

    #[test]
    fn preview_restore_requires_the_same_generation_and_live_lease() {
        assert!(preview_lease_matches(17, 17, OperationLease::LiveDictation));
        assert!(!preview_lease_matches(
            18,
            17,
            OperationLease::LiveDictation
        ));
        assert!(!preview_lease_matches(
            17,
            17,
            OperationLease::HistoryReclean
        ));
        assert!(!preview_lease_matches(17, 17, OperationLease::Idle));
    }

    #[test]
    fn cancelled_preview_uses_processing_owner_generation_not_capture_generation() {
        let identity = crate::TextActionIdentity {
            transaction_id: "owned-preview".into(),
            action_sequence: 8,
            recording_generation: 41,
            onboarding_trial: None,
        };
        assert_eq!(cancelled_preview_event_metadata(42, &identity), (42, 8),);
        assert_ne!(identity.recording_generation, 42);
    }

    #[test]
    fn synthetic_trial_source_uses_local_proof_and_never_external_selection_capture() {
        use super::{trial_source_validation, SelectedActionSession};
        use crate::{context, onboarding, paste, TextActionIdentity};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let text = "受控 fixture 🧪".to_owned();
        let fingerprint = paste::selection_fingerprint(&text);
        let trial =
            onboarding::test_trial_snapshot("10000000-0000-4000-8000-000000000001", "7", 41);
        let mut session = SelectedActionSession {
            selected_text: text.clone(),
            source: paste::CapturedTextActionSource {
                kind: paste::TextActionSourceKind::Selection,
                text: text.clone(),
                text_fingerprint: fingerprint,
                field_fingerprint: Some(fingerprint),
                selection_range: Some((0, text.encode_utf16().count() as i64)),
                editable: false,
                copy_only_selection: false,
            },
            target_guard: context::ContextSnapshot::general().target_guard,
            identity: TextActionIdentity {
                transaction_id: "trial-41".into(),
                action_sequence: 41,
                recording_generation: 41,
                onboarding_trial: Some(trial),
            },
            delivery_replace_allowed: false,
            context_policy_revision: 0,
        };
        let external_capture = AtomicUsize::new(0);
        let decision = trial_source_validation(&session, || true).unwrap_or_else(|| {
            external_capture.fetch_add(1, Ordering::SeqCst);
            Err(paste::PasteError::TargetChanged)
        });
        assert!(decision.is_ok());
        assert_eq!(external_capture.load(Ordering::SeqCst), 0);
        let stale = trial_source_validation(&session, || false).unwrap_or_else(|| {
            external_capture.fetch_add(1, Ordering::SeqCst);
            Err(paste::PasteError::TargetChanged)
        });
        assert!(stale.is_err());
        assert_eq!(external_capture.load(Ordering::SeqCst), 0);
        session.source.text.push('x');
        assert!(trial_source_validation(&session, || true).unwrap().is_err());
        session.identity.onboarding_trial = None;
        assert!(trial_source_validation(&session, || panic!(
            "normal source must use native proof"
        ))
        .is_none());
    }
}
