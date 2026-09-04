mod asr;
mod audio;
mod chunker;
#[cfg(test)]
mod cleanup_corpus;
mod context;
mod delivery;
mod dictation;
mod dictionary_learn;
mod engine;
mod lexicon;
mod groq;
mod history_commands;
mod hotkey;
mod input_source;
mod instance;
mod island_window;
mod keychain;
mod llm;
mod metrics;
mod modifier_hotkey;
mod notch;
mod paste;
mod permissions;
mod providers;
mod queue;
mod prefetch_asr;
mod selected_action;
mod silence;
mod snippets;
mod spoken_layout;
mod spoken_punctuation;
mod spoken_revision;
mod store;
#[cfg(test)]
mod test_http;
use futures_util::{Stream, StreamExt};
use std::future::Future;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use tauri::{Emitter, Listener, Manager, State};
use tokio_util::sync::CancellationToken;
use dictation::{
    release_operation_lease, DictationManager, OperationLease, Phase, RecorderBackend, StopClaim,
};
use selected_action::{
    clear_selected_action, clear_selected_preview, selected_preview_completion_is_current,
    SelectedActionPreview, SelectedActionSession,
};

fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn clipboard_text_for_snippets(app: &tauri::AppHandle) -> Option<String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    app.clipboard().read_text().ok()
}

fn current_asr_provider(state: &AppState) -> Arc<dyn asr::AsrProvider> {
    lock_recover(&state.asr_provider).clone()
}

fn rebuild_asr_provider(state: &AppState, endpoint: &str) {
    *lock_recover(&state.asr_provider) = Arc::new(asr::GroqAsrProvider::from_resolved_endpoint(endpoint));
}

#[derive(Debug, Clone)]
struct UndoTransaction {
    session_generation: u64,
    created_at: std::time::Instant,
    expires_at: std::time::Instant,
    target_guard: context::TargetAppGuard,
    delivery_method: String,
    post_insert_input_fingerprint: u64,
    consumed: bool,
}

pub(crate) struct AppState {
    manager: Mutex<DictationManager>,
    /// The recorder performs blocking I/O (cpal stream setup/teardown with
    /// timeouts). It lives behind its own async mutex so dictation state
    /// transitions never hold the manager lock across a blocking call.
    recorder: Arc<Mutex<Box<dyn RecorderBackend>>>,
    prefetch_asr: Mutex<Option<prefetch_asr::PrefetchAsrSession>>,
    selected_action: Mutex<Option<SelectedActionSession>>,
    selected_preview: Mutex<Option<SelectedActionPreview>>,
    undo: Mutex<Option<UndoTransaction>>,
    operation_lease: Mutex<OperationLease>,
    asr_provider: Mutex<Arc<dyn asr::AsrProvider>>,
    settings: Mutex<store::Settings>,
    context: Mutex<context::ContextState>,
    gate: Arc<queue::RequestGate>,
    metrics: metrics::Metrics,
    hotkey_gate: tokio::sync::Mutex<()>,
    settings_gate: tokio::sync::Mutex<()>,
    pending_recorder_cancel: Mutex<Option<u64>>,
    onboarding_test_mode: Mutex<bool>,
    onboarding_selected_text: Mutex<Option<String>>,
    _instance_lock: instance::InstanceLock,
}

fn try_claim_operation(state: &AppState, requested: OperationLease) -> bool {
    let mut lease = state
        .operation_lease
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    dictation::claim_operation(&mut lease, requested)
}

fn release_operation(state: &AppState, expected: OperationLease) {
    let mut lease = state
        .operation_lease
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    dictation::release_operation_lease(&mut lease, expected);
}

#[derive(Debug)]
pub(crate) struct StartError {
    pub(crate) generation: u64,
    pub(crate) message: String,
}

impl StartError {
    fn new(generation: u64, message: impl Into<String>) -> Self {
        Self {
            generation,
            message: message.into(),
        }
    }
}

static SHORT_RECOVERY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

// Audio finalization already has internal bounded waits. Keep a small outer
// bound as well so a wedged capture worker cannot leave the HUD in Processing
// forever. The blocking worker may finish later, but its stale result is
// discarded by the session-generation guard.
const AUDIO_FINALIZATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(25);
const PROCESSING_WATCHDOG_MIN_SECS: u64 = 5 * 60;
const PROCESSING_WATCHDOG_MAX_SECS: u64 = 30 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
enum CleanupDecision {
    Provider(String),
    Disabled,
    Failed,
}

const CLEANUP_STATUS_AI_SUCCESS: &str = "ai_success";
const CLEANUP_STATUS_AI_FAILED_LOCAL: &str = "ai_failed_local";
const CLEANUP_STATUS_AI_FAILED_RAW: &str = "ai_failed_raw";
const CLEANUP_STATUS_LOCAL_ONLY: &str = "local_only";
const CLEANUP_STATUS_SNIPPET_BYPASS: &str = "snippet_bypass";
const CLEANUP_STATUS_UNKNOWN: &str = "unknown";

#[derive(Debug, Clone, PartialEq, Eq)]
struct FinalText {
    text: String,
    degraded: bool,
    degraded_reason: Option<&'static str>,
}

fn finalize_text(
    raw: &str,
    decision: CleanupDecision,
    family: context::ContextFamily,
    pairs: &[store::LearnPairRecord],
    dictionary: &[String],
) -> Result<FinalText, &'static str> {
    let (candidate, mut degraded, mut degraded_reason) = match decision {
        CleanupDecision::Provider(text) => (text, false, None),
        CleanupDecision::Disabled => (local_cleanup_or_raw(raw, family), false, None),
        CleanupDecision::Failed => (
            local_cleanup_or_raw(raw, family),
            true,
            Some("llm_cleanup_failed"),
        ),
    };
    let text = if candidate.trim().is_empty() {
        degraded = true;
        degraded_reason = Some("llm_cleanup_empty");
        local_cleanup_or_raw(raw, family)
    } else {
        candidate
    };
    if text.trim().is_empty() {
        return Err("no_speech");
    }
    let text = lexicon::apply_promoted_replacements(&text, pairs, dictionary);
    if text.trim().is_empty() {
        return Err("no_speech");
    }
    Ok(FinalText {
        text,
        degraded,
        degraded_reason,
    })
}

fn cleanup_failure_status(raw: &str, final_text: &str) -> &'static str {
    if final_text.trim() != raw.trim() {
        CLEANUP_STATUS_AI_FAILED_LOCAL
    } else {
        CLEANUP_STATUS_AI_FAILED_RAW
    }
}

fn cleanup_profile_for(snapshot: &context::ContextSnapshot) -> Option<&context::ContextProfile> {
    (snapshot.profile.confidence >= 0.75).then_some(&snapshot.profile)
}

fn local_cleanup_or_raw(raw: &str, family: context::ContextFamily) -> String {
    let cleaned = llm::local_cleanup(raw);
    let text = if cleaned.trim().is_empty() {
        raw.to_owned()
    } else {
        cleaned
    };
    spoken_punctuation::ensure_terminal(&text, family)
}

fn cleanup_policy_for(
    settings: &store::Settings,
    recording_context: &context::ContextSnapshot,
) -> context::ContextPolicy {
    let mut policy = if recording_context.profile.confidence >= 0.75 {
        recording_context.policy.clone()
    } else {
        context::ContextPolicy::default()
    };
    if settings.output_mode != "auto" {
        policy.output_mode = Some(settings.output_mode.clone());
    }
    if settings.output_mode == "translation" {
        policy.translation_target_language = Some(settings.translation_target_language.clone());
    }
    policy
}

pub(crate) fn spoken_translation_target(settings: &store::Settings) -> Option<&str> {
    (settings.output_mode == "translation")
        .then_some(settings.translation_target_language.as_str())
        .filter(|value| !value.trim().is_empty() && *value != "auto")
}

fn delivery_fallback_reason(target_current: bool, paste_error: &str) -> &'static str {
    if paste_error.contains("Accessibility permission") {
        "accessibility_required"
    } else if paste_error.contains("browser access is required") {
        "browser_permission_required"
    } else if paste_error.contains("focused input is not available")
        || paste_error.contains("secure")
    {
        "input_unavailable"
    } else if paste_error.contains("focused input changed") {
        "input_changed"
    } else if paste_error.contains("target could not be identified") {
        "target_unavailable"
    } else if !target_current || paste_error.contains("target changed") {
        "target_changed"
    } else {
        "paste_failed"
    }
}

fn error_fallback_reason(message: &str) -> &'static str {
    let normalized = message.to_ascii_lowercase();
    if normalized.contains("microphone permission") {
        "microphone_required"
    } else if normalized.contains("api key") {
        "api_key_required"
    } else if normalized.contains("complete onboarding") {
        "onboarding_required"
    } else if normalized.contains("no speech") {
        "no_speech"
    } else if normalized.contains("transcription took too long") || normalized.contains("timeout") {
        "processing_timeout"
    } else if normalized.contains("microphone")
        || normalized.contains("audio")
        || normalized.contains("input device")
    {
        "audio_error"
    } else {
        "dictation_error"
    }
}

fn hud_accepts_mouse(paste_yielding: bool, learn_toast: bool) -> bool {
    !paste_yielding && learn_toast
}

fn sync_island_mouse(app: &tauri::AppHandle) {
    island_window::set_interactive(
        app,
        hud_accepts_mouse(
            island_window::is_yielding_for_paste(),
            island_window::is_learn_toast_interactive(),
        ),
    );
}

fn emit_state(app: &tauri::AppHandle, state: &str) {
    sync_island_mouse(app);
    let session_generation = current_session_generation(app);
    let _ = app.emit(
        "dictation://state",
        serde_json::json!({
            "state": state,
            "session_generation": session_generation,
        }),
    );
    if state == "idle" {
        emit_selected_action_state(app, "idle");
    }
    if state == "idle" {
        emit_progress(app, 0.0);
        emit_hud_partial(app, session_generation, "");
    }
}

/// HUD-only in-progress words. Never clipboard, History, or paste.
fn emit_hud_partial(app: &tauri::AppHandle, session_generation: u64, text: &str) {
    let _ = app.emit_to(
        "island",
        "dictation://partial",
        serde_json::json!({
            "session_generation": session_generation,
            "text": text,
        }),
    );
    let has_partial = app
        .try_state::<AppState>()
        .map(|state| {
            let manager = lock_recover(&state.manager);
            hud_partial_expands_window(
                text,
                manager.phase,
                session_generation,
                manager.session_generation,
            )
        })
        .unwrap_or(false);
    island_window::set_has_partial(app, has_partial);
}

fn hud_partial_expands_window(
    _text: &str,
    _phase: Phase,
    _event_generation: u64,
    _current_generation: u64,
) -> bool {
    false
}

fn hud_caption_expands_window(state: &str, fallback_reason: Option<&str>) -> bool {
    fallback_reason.is_some()
        || matches!(
            state,
            "error" | "degraded" | "copied" | "unverified" | "rate_limited"
        )
}

fn attach_context_fields(payload: &mut serde_json::Value, context: &context::ContextSnapshot) {
    payload["context_id"] = serde_json::json!(context.profile.id);
    payload["context_label"] = serde_json::json!(context::display_label(context));
    payload["context_app"] = match context::display_app_name(context) {
        Some(name) => serde_json::Value::from(name),
        None => serde_json::Value::Null,
    };
    payload["context_style"] = serde_json::json!(context::hud_style_id(context));
}

fn emit_selected_action_state(app: &tauri::AppHandle, state: &str) {
    let _ = app.emit(
        "selected-action://state",
        serde_json::json!({ "state": state }),
    );
}

fn current_session_generation(app: &tauri::AppHandle) -> u64 {
    app.try_state::<AppState>()
        .map(|state| lock_recover(&state.manager).session_generation)
        .unwrap_or_default()
}

fn long_chunk_progress_payload(
    session_generation: u64,
    elapsed_secs: u64,
    chunks_done: usize,
    total: usize,
    progress: f32,
) -> serde_json::Value {
    serde_json::json!({
        "session_generation": session_generation,
        "elapsed_secs": elapsed_secs,
        "chunks_done": chunks_done,
        "total": total,
        "progress": progress,
    })
}

async fn show_selected_action_error(
    app: &tauri::AppHandle,
    state: &AppState,
    message: &str,
    fallback_reason: &'static str,
) {
    show_island(app);
    let _ = app.emit("dictation://error", message);
    emit_state_with_delivery(app, "error", None, "none", Some(fallback_reason), None);
    emit_selected_action_state(app, "idle");
    tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
    if lock_recover(&state.manager).phase == Phase::Idle {
        emit_state(app, "idle");
    }
}
fn emit_progress(app: &tauri::AppHandle, progress: f32) {
    let _ = app.emit(
        "dictation://progress",
        serde_json::json!({
            "progress": progress.clamp(0.0, 1.0),
            "session_generation": current_session_generation(app),
        }),
    );
}
fn emit_state_with_context(
    app: &tauri::AppHandle,
    state: &str,
    context: Option<&context::ContextSnapshot>,
) {
    emit_state_with_delivery(app, state, context, "pending", None, None);
}
fn emit_state_with_context_and_input_device(
    app: &tauri::AppHandle,
    state: &str,
    context: Option<&context::ContextSnapshot>,
    input_device: Option<&str>,
) {
    emit_state_with_delivery_and_input_device(
        app,
        state,
        context,
        "pending",
        None,
        None,
        input_device,
    );
}
fn emit_state_with_delivery(
    app: &tauri::AppHandle,
    state: &str,
    context: Option<&context::ContextSnapshot>,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    cleanup_status: Option<&str>,
) {
    emit_state_with_delivery_and_input_device(
        app,
        state,
        context,
        delivery_method,
        fallback_reason,
        cleanup_status,
        None,
    );
}
fn emit_state_with_delivery_and_input_device(
    app: &tauri::AppHandle,
    state: &str,
    context: Option<&context::ContextSnapshot>,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    cleanup_status: Option<&str>,
    input_device: Option<&str>,
) {
    sync_island_mouse(app);
    let mut payload = serde_json::json!({
        "state": state,
        "session_generation": current_session_generation(app),
    });
    if let Some(context) = context {
        attach_context_fields(&mut payload, context);
    }
    payload["delivery_method"] = serde_json::json!(delivery_method);
    payload["phase"] = serde_json::json!(match state {
        "starting" => "finalizing_audio",
        "rate_limited" => "waiting_retry",
        "processing" => "cleanup",
        _ => "idle",
    });
    payload["fallback_reason"] = fallback_reason
        .map(serde_json::Value::from)
        .unwrap_or(serde_json::Value::Null);
    payload["cleanup_status"] = cleanup_status
        .map(serde_json::Value::from)
        .unwrap_or(serde_json::Value::Null);
    payload["undo_available"] = serde_json::json!(undo_available_from_app(app, state));
    if let Some(input_device) = input_device {
        payload["input_device"] = serde_json::Value::from(input_device);
    }
    island_window::set_has_wide_caption(
        app,
        hud_caption_expands_window(state, fallback_reason),
    );
    let _ = app.emit("dictation://state", payload);
}

fn undo_available_from_app(app: &tauri::AppHandle, hud_state: &str) -> bool {
    let Some(app_state) = app.try_state::<AppState>() else {
        return false;
    };
    let undo = lock_recover(&app_state.undo).clone();
    let generation = lock_recover(&app_state.manager).session_generation;
    undo_available_for_hud(
        hud_state,
        undo.as_ref(),
        generation,
        std::time::Instant::now(),
    )
}

/// HUD Undo is shown only when a 3s Cmd+Z transaction is actually armed.
/// Inferring from `delivery_method == "paste"` is wrong: verified AX inserts
/// also report method `"paste"` but never call `arm_undo_transaction`.
fn undo_available_for_hud(
    hud_state: &str,
    undo: Option<&UndoTransaction>,
    current_generation: u64,
    now: std::time::Instant,
) -> bool {
    matches!(hud_state, "done" | "degraded")
        && undo.is_some_and(|tx| undo_preflight(tx, current_generation, now) == "available")
}

fn emit_processing_phase(
    app: &tauri::AppHandle,
    phase: &str,
    context: Option<&context::ContextSnapshot>,
    retry_after_secs: Option<f64>,
    chunk_progress: Option<(usize, usize)>,
) {
    let mut payload = serde_json::json!({
        "state": "processing",
        "phase": phase,
        "session_generation": current_session_generation(app),
        "delivery_method": "pending",
        "fallback_reason": serde_json::Value::Null,
        "cleanup_status": serde_json::Value::Null,
    });
    if let Some(context) = context {
        attach_context_fields(&mut payload, context);
    }
    sync_island_mouse(app);
    island_window::set_has_wide_caption(app, false);
    if let Some(seconds) = retry_after_secs {
        payload["retry_after_secs"] = serde_json::json!(seconds.ceil() as u64);
    }
    if let Some((completed, total)) = chunk_progress {
        payload["completed_chunks"] = serde_json::json!(completed);
        payload["total_chunks"] = serde_json::json!(total);
    }
    let _ = app.emit("dictation://state", payload);
}
fn context_payload(snapshot: &context::ContextSnapshot) -> serde_json::Value {
    serde_json::json!({
        "profile": snapshot.profile,
        "browser_access_status": snapshot.browser_access_status,
    })
}

fn commit_context_snapshot(
    state: &mut context::ContextState,
    generation: u64,
    next: &context::ContextSnapshot,
) -> Option<bool> {
    if state.detection_generation != generation {
        return None;
    }
    let changed = state.snapshot.profile != next.profile
        || state.snapshot.browser_access_status != next.browser_access_status;
    state.snapshot = next.clone();
    Some(changed)
}

async fn refresh_context_snapshot(app: &tauri::AppHandle, state: &AppState) {
    let (enabled, browser_access_enabled, mappings, writing_modes, manual_override, generation) = {
        let mut current = lock_recover(&state.context);
        current.detection_generation = current.detection_generation.wrapping_add(1);
        (
            current.enabled,
            current.browser_access_enabled,
            current.mappings.clone(),
            current.writing_modes.clone(),
            current.manual_override,
            current.detection_generation,
        )
    };
    let next = match tokio::task::spawn_blocking(move || {
        let mut snapshot = context::detect_snapshot_for_state_with_modes(
            enabled,
            &mappings,
            browser_access_enabled,
            &writing_modes,
        );
        context::apply_manual_override_with_modes(&mut snapshot, manual_override, &writing_modes);
        snapshot
    })
    .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            log::warn!("context detection worker failed: {error}");
            return;
        }
    };
    let changed = {
        let mut current = lock_recover(&state.context);
        match commit_context_snapshot(&mut current, generation, &next) {
            Some(changed) => changed,
            None => return,
        }
    };
    if changed {
        let _ = app.emit("context://changed", context_payload(&next));
    }
}

async fn persist_context_state(
    app: &tauri::AppHandle,
    state: &AppState,
    enabled: bool,
    browser_access_enabled: bool,
    mappings: Vec<context::AppMapping>,
) -> Result<(), String> {
    let mut settings = lock_recover(&state.settings).clone();
    settings.context_enabled = enabled;
    settings.browser_access_enabled = browser_access_enabled;
    settings.context_mappings = mappings.clone();
    settings.validate().map_err(|error| error.to_string())?;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    store::save_settings(&dir, &settings).map_err(|error| error.to_string())?;
    {
        let mut current = lock_recover(&state.context);
        current.enabled = enabled;
        current.browser_access_enabled = browser_access_enabled;
        current.mappings = mappings;
    }
    *lock_recover(&state.settings) = settings;
    refresh_context_snapshot(app, state).await;
    Ok(())
}
fn sync_modifier_hotkey_phase(phase: Phase) {
    crate::modifier_hotkey::set_dictation_active(
        phase == Phase::Recording || phase == Phase::Stopping || phase == Phase::Processing,
    );
}

async fn start_audio(
    state: &AppState,
    app: tauri::AppHandle,
    session: String,
    input_device: String,
    chunk_length_secs: usize,
    input_gain: f32,
    prefetch_tx: prefetch_asr::PrefetchInbox,
) -> Result<(), String> {
    let recorder = Arc::clone(&state.recorder);
    tokio::task::spawn_blocking(move || {
        let mut recorder = recorder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        recorder
            .start(
                Some(&app),
                &session,
                &input_device,
                chunk_length_secs,
                input_gain,
                prefetch_tx,
            )
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("audio start worker failed: {error}"))?
}

async fn stop_audio(
    state: &AppState,
    app: tauri::AppHandle,
) -> Result<(Vec<u8>, Vec<chunker::AudioChunk>), String> {
    let recorder = Arc::clone(&state.recorder);
    tokio::task::spawn_blocking(move || {
        let mut recorder = recorder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        recorder
            .stop_with_chunks(Some(&app))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("audio stop worker failed: {error}"))?
}

async fn cancel_audio(state: &AppState, app: tauri::AppHandle) {
    let recorder = Arc::clone(&state.recorder);
    let _ = tokio::task::spawn_blocking(move || {
        let mut recorder = recorder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        recorder.cancel(Some(&app));
    })
    .await;
}

fn cancel_prefetch_asr(state: &AppState) {
    let session = state
        .prefetch_asr
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(session) = session {
        session.cancel();
    }
}

async fn finish_prefetch_asr(state: &AppState) -> Option<prefetch_asr::PrefetchAsrResult> {
    let session = state
        .prefetch_asr
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()?;
    Some(session.finish(std::time::Duration::from_secs(3)).await)
}

async fn claim_selected_action_entry(state: &AppState) -> Option<u64> {
    let _gate = state.hotkey_gate.lock().await;
    if hotkey::is_suspended() {
        return None;
    }
    dictation::claim_start(state)
}

async fn reset_selected_action_start(
    app: &tauri::AppHandle,
    state: &AppState,
    session_generation: u64,
) -> bool {
    let _gate = state.hotkey_gate.lock().await;
    if reset_starting(state, session_generation).is_none() {
        return false;
    }
    hotkey::unregister_cancel(app);
    sync_modifier_hotkey_phase(Phase::Idle);
    true
}

async fn release_live_operation(
    app: &tauri::AppHandle,
    state: &AppState,
    expected_generation: u64,
) {
    let _gate = state.hotkey_gate.lock().await;
    if lock_recover(&state.pending_recorder_cancel).is_some() {
        return;
    }
    let owns_operation = {
        let mut lease = lock_recover(&state.operation_lease);
        let manager = lock_recover(&state.manager);
        let owns = *lease == OperationLease::LiveDictation
            && manager.phase == Phase::Idle
            && manager.session_generation == expected_generation;
        if owns {
            *lease = OperationLease::Idle;
        }
        owns
    };
    if owns_operation {
        sync_modifier_hotkey_phase(Phase::Idle);
        hotkey::unregister_cancel(app);
    }
}

async fn rollback_started_audio(
    app: &tauri::AppHandle,
    state: &AppState,
    session_generation: u64,
) {
    let owns_recorder_cancel = {
        let _gate = state.hotkey_gate.lock().await;
        let mut pending = lock_recover(&state.pending_recorder_cancel);
        let lease = lock_recover(&state.operation_lease);
        let manager = lock_recover(&state.manager);
        let owns = pending.is_none()
            && *lease == OperationLease::LiveDictation
            && manager.phase == Phase::Idle
            && manager.session_generation == session_generation.wrapping_add(1);
        if owns {
            *pending = Some(manager.session_generation);
        }
        owns
    };
    if !owns_recorder_cancel {
        return;
    }

    cancel_audio(state, app.clone()).await;
    let _gate = state.hotkey_gate.lock().await;
    let owns_stale_start = {
        let mut pending = lock_recover(&state.pending_recorder_cancel);
        let owns_pending_cancel =
            pending.take() == Some(session_generation.wrapping_add(1));
        let mut lease = lock_recover(&state.operation_lease);
        let manager = lock_recover(&state.manager);
        let stale = owns_pending_cancel
            && manager.phase == Phase::Idle
            && manager.session_generation == session_generation.wrapping_add(1);
        if stale {
            *lease = OperationLease::Idle;
        }
        stale
    };
    if owns_stale_start {
        hotkey::unregister_cancel(app);
        sync_modifier_hotkey_phase(Phase::Idle);
    }
}

async fn start_selected_session_with_feedback(
    app: &tauri::AppHandle,
    state: &AppState,
    session: SelectedActionSession,
    session_generation: u64,
) -> Result<(), String> {
    let claimed = {
        let _gate = state.hotkey_gate.lock().await;
        let manager = lock_recover(&state.manager);
        if manager.phase != Phase::Starting
            || manager.session_generation != session_generation
            || manager.cancellation.is_cancelled()
        {
            false
        } else {
            *state
                .selected_action
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(session);
            true
        }
    };
    if !claimed {
        let _ = reset_selected_action_start(app, state, session_generation).await;
        clear_selected_action(state);
        return Ok(());
    }

    match start_claimed(app, state, session_generation).await {
        Ok(()) => {
            if lock_recover(&state.manager).phase == Phase::Recording {
                emit_selected_action_state(app, "listening");
            } else {
                clear_selected_action(state);
                emit_selected_action_state(app, "idle");
            }
            Ok(())
        }
        Err(error) => {
            clear_selected_action(state);
            fail_for_generation(app, state, error.message.clone(), error.generation).await;
            Err(error.message)
        }
    }
}

async fn start_selected_action_with_feedback(
    app: &tauri::AppHandle,
    state: &AppState,
) -> Result<(), String> {
    clear_selected_preview(state);
    let Some(session_generation) = claim_selected_action_entry(state).await else {
        return Ok(());
    };
    // Escape must remain responsive while the selected text is captured.
    hotkey::register_cancel(app);
    emit_selected_action_state(app, "waiting_for_selection");
    let onboarding_selected_text = if *lock_recover(&state.onboarding_test_mode) {
        state
            .onboarding_selected_text
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .filter(|text| !text.trim().is_empty())
    } else {
        None
    };

    if let Some(selected_text) = onboarding_selected_text {
        refresh_context_snapshot(app, state).await;
        if lock_recover(&state.manager).session_generation != session_generation {
            let _ = reset_starting(state, session_generation);
            return Ok(());
        }
        let snapshot = lock_recover(&state.context).snapshot.clone();
        return start_selected_session_with_feedback(
            app,
            state,
            SelectedActionSession {
                selected_text,
                selection_fingerprint: 0,
                target_guard: snapshot.target_guard,
                onboarding_trial: true,
            },
            session_generation,
        )
        .await;
    }

    if !permissions::check().accessibility {
        emit_selected_action_state(app, "accessibility_required");
        let message = "Accessibility permission is required to read selected text".to_owned();
        if !reset_selected_action_start(app, state, session_generation).await {
            return Ok(());
        }
        show_selected_action_error(app, state, &message, "accessibility_required").await;
        return Err(message);
    }

    refresh_context_snapshot(app, state).await;
    if lock_recover(&state.manager).session_generation != session_generation {
        let _ = reset_starting(state, session_generation);
        return Ok(());
    }
    let snapshot = lock_recover(&state.context).snapshot.clone();
    if snapshot.target_guard.input_token.is_none() {
        emit_selected_action_state(app, "waiting_for_selection");
        let message = "Select editable text before starting a selected-text action".to_owned();
        if !reset_selected_action_start(app, state, session_generation).await {
            return Ok(());
        }
        show_selected_action_error(app, state, &message, "input_unavailable").await;
        return Err(message);
    }

    let app_for_capture = app.clone();
    let captured =
        match tokio::task::spawn_blocking(move || paste::capture_selected_text(&app_for_capture, true))
            .await
        {
            Ok(result) => result.map_err(|error| error.to_string()),
            Err(error) => {
                let error = format!("selection capture worker failed: {error}");
                if !reset_selected_action_start(app, state, session_generation).await {
                    return Ok(());
                }
                emit_selected_action_state(app, "waiting_for_selection");
                show_selected_action_error(app, state, &error, "input_unavailable").await;
                return Err(error);
            }
        };
    let captured = match captured {
        Ok(captured) => captured,
        Err(error) => {
            emit_selected_action_state(app, "waiting_for_selection");
            if !reset_selected_action_start(app, state, session_generation).await {
                return Ok(());
            }
            show_selected_action_error(app, state, &error, "input_unavailable").await;
            return Err(error);
        }
    };

    start_selected_session_with_feedback(
        app,
        state,
        SelectedActionSession {
            selected_text: captured.text,
            selection_fingerprint: captured.fingerprint,
            target_guard: snapshot.target_guard,
            onboarding_trial: false,
        },
        session_generation,
    )
    .await
}

pub(crate) async fn start_claimed(
    app: &tauri::AppHandle,
    state: &AppState,
    session_generation: u64,
) -> Result<(), StartError> {
    let start_is_current = {
        let manager = lock_recover(&state.manager);
        manager.phase == Phase::Starting
            && manager.session_generation == session_generation
            && !manager.cancellation.is_cancelled()
    };
    if !start_is_current {
        let _ = reset_starting(state, session_generation);
        return Ok(());
    }
    state.gate.set_session_generation(session_generation);
    clear_selected_preview(state);
    // Escape must be available during recorder setup as well as recording.
    hotkey::register_cancel(app);
    // Give the user immediate feedback while permission/context/audio setup
    // completes. The HUD must not appear to ignore a global shortcut.
    show_island(app);
    emit_state(app, "starting");
    if !permissions::check().microphone {
        let Some(failure_generation) = reset_starting(state, session_generation) else {
            return Ok(());
        };
        return Err(StartError::new(
            failure_generation,
            "Microphone permission is required",
        ));
    }
    let settings_snapshot = lock_recover(&state.settings).clone();
    let onboarding_test_mode = *lock_recover(&state.onboarding_test_mode);
    if !settings_snapshot.onboarded && !onboarding_test_mode {
        let Some(failure_generation) = reset_starting(state, session_generation) else {
            return Ok(());
        };
        return Err(StartError::new(
            failure_generation,
            "Complete onboarding before dictation can start",
        ));
    }
    if settings_snapshot.api_key.trim().is_empty() {
        let Some(failure_generation) = reset_starting(state, session_generation) else {
            return Ok(());
        };
        return Err(StartError::new(
            failure_generation,
            "A valid API key is required before dictation can start",
        ));
    }
    let asr_url = settings_snapshot.resolved_provider_base(settings_snapshot.asr_provider);
    let asr_empty_ok = settings_snapshot.asr_provider.allows_empty_key()
        && crate::providers::is_loopback_url(&asr_url);
    if settings_snapshot.asr_credential().trim().is_empty() && !asr_empty_ok {
        let Some(failure_generation) = reset_starting(state, session_generation) else {
            return Ok(());
        };
        let message = if settings_snapshot.asr_provider.is_custom() {
            "自定义 ASR 地址需要填写 ASR 密钥。"
        } else {
            "缺少所选服务商的密钥。"
        };
        return Err(StartError::new(failure_generation, message));
    }
    // Always refresh immediately before starting audio. A stale check is not
    // enough here: the user may have switched apps within the freshness
    // window.
    refresh_context_snapshot(app, state).await;
    let setup_was_cancelled = {
        let manager = lock_recover(&state.manager);
        manager.phase != Phase::Starting
            || manager.session_generation != session_generation
            || manager.cancellation.is_cancelled()
    };
    if setup_was_cancelled {
        let _ = reset_starting(state, session_generation);
        return Ok(());
    }

    let id = format!("{}", chrono_like_id());
    let chunk_length_secs = lock_recover(&state.settings).chunk_length_secs;
    let input_device = settings_snapshot.input_device.clone();
    let input_gain = settings_snapshot.input_gain;
    let (prefetch_inbox, prefetch_rx) = prefetch_asr::PrefetchAsrSession::channel();

    // Phase 2: cpal setup waits on a blocking channel, so keep it off the
    // async runtime worker and the UI-facing command path.
    if let Err(error) = start_audio(
        state,
        app.clone(),
        id,
        input_device,
        chunk_length_secs,
        input_gain,
        prefetch_inbox.clone(),
    )
    .await
    {
        let Some(failure_generation) = reset_starting(state, session_generation) else {
            return Ok(());
        };
        return Err(StartError::new(failure_generation, error));
    }
    let audio_start_was_cancelled = {
        let manager = lock_recover(&state.manager);
        manager.phase != Phase::Starting
            || manager.session_generation != session_generation
            || manager.cancellation.is_cancelled()
    };
    if audio_start_was_cancelled {
        rollback_started_audio(app, state, session_generation).await;
        return Ok(());
    }

    // Capture the target after the recorder has successfully started. This
    // narrows the race where the user changes apps while cpal is initializing.
    refresh_context_snapshot(app, state).await;
    let recording_context = lock_recover(&state.context).snapshot.clone();

    // Phase 3 (sync, short lock): commit the recording state.
    let (start_was_cancelled, pending_stop) = {
        let mut m = lock_recover(&state.manager);
        if m.phase != Phase::Starting || m.session_generation != session_generation {
            (true, None)
        } else {
            let pending = dictation::enter_recording(&mut m, recording_context.clone());
            (false, pending)
        }
    };
    if start_was_cancelled {
        // Raced with another transition; roll back the recorder we started.
        rollback_started_audio(app, state, session_generation).await;
        return Ok(());
    }
    if let Some(claim) = pending_stop {
        let _ = stop_claimed(app, state, claim).await;
        return Ok(());
    }
    let selected_action_active = state
        .selected_action
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_some();
    if !selected_action_active {
        let prefetch_cancellation = {
            let manager = lock_recover(&state.manager);
            manager.cancellation.child_token()
        };
        let asr_language =
            asr::normalize_language(Some(settings_snapshot.language.as_str())).map(str::to_owned);
        let asr_prompt = asr_prompt_for_snapshot(
            app.path().app_data_dir().ok().as_deref(),
            &settings_snapshot.dictionary,
            &recording_context,
            settings_snapshot.asr_provider,
            &settings_snapshot.asr_model,
        );
        // This is silent batch prefetch of completed files, not streaming ASR.
        let prefetch_session = prefetch_asr::PrefetchAsrSession::spawn(
            prefetch_rx,
            prefetch_inbox,
            state.gate.clone(),
            current_asr_provider(state),
            asr::AsrOptions {
                api_key: settings_snapshot.asr_credential().to_owned(),
                language: asr_language,
                prompt: asr_prompt,
                model: asr::resolve_recognition_model(
                    &settings_snapshot.asr_model,
                    Some(settings_snapshot.language.as_str()),
                )
                .to_owned(),
            },
            state.metrics.clone(),
            prefetch_cancellation,
            None,
            session_generation,
        );
        *state
            .prefetch_asr
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(prefetch_session);
    }
    hotkey::register_cancel(app);
    sync_modifier_hotkey_phase(Phase::Recording);
    show_island(app);
    let input_device = audio::selected_input_device_name(&settings_snapshot.input_device).ok();
    emit_state_with_context_and_input_device(
        app,
        "recording",
        Some(&recording_context),
        input_device.as_deref(),
    );
    Ok(())
}

fn reset_starting(state: &AppState, expected_generation: u64) -> Option<u64> {
    let mut manager = lock_recover(&state.manager);
    let was_starting =
        manager.phase == Phase::Starting && manager.session_generation == expected_generation;
    let owns_cancelled_start = manager.phase == Phase::Idle
        && manager.session_generation == expected_generation.wrapping_add(1);
    if !was_starting && !owns_cancelled_start {
        return None;
    }
    let generation = dictation::reset_starting_manager(&mut manager);
    drop(manager);
    clear_selected_action(state);
    release_operation(state, OperationLease::LiveDictation);
    was_starting.then_some(generation)
}

async fn start_with_error_feedback(app: &tauri::AppHandle, state: &AppState) -> Result<(), String> {
    match dictation::start_internal(app, state).await {
        Ok(()) => Ok(()),
        Err(error) => {
            fail_for_generation(app, state, error.message.clone(), error.generation).await;
            Err(error.message)
        }
    }
}
fn show_island(app: &tauri::AppHandle) {
    island_window::show_overlay(app);
}

#[tauri::command]
fn hide_island_if_idle(app: tauri::AppHandle, state: State<'_, AppState>) {
    island_window::set_learn_toast_interactive(&app, false);
    if lock_recover(&state.manager).phase == Phase::Idle {
        island_window::hide_overlay(&app);
    }
}

#[tauri::command]
fn set_island_learn_interactive(app: tauri::AppHandle, interactive: bool) {
    island_window::set_learn_toast_interactive(&app, interactive);
}

async fn handle_audio_error(app: &tauri::AppHandle, state: &AppState, message: String) {
    let failure_generation = {
        let _gate = state.hotkey_gate.lock().await;
        let mut manager = lock_recover(&state.manager);
        if !matches!(manager.phase, Phase::Starting | Phase::Recording) {
            None
        } else {
            manager.cancellation.cancel();
            manager.phase = Phase::Idle;
            manager.session_generation = manager.session_generation.wrapping_add(1);
            manager.recording_context = None;
            Some(manager.session_generation)
        }
    };
    let Some(failure_generation) = failure_generation else {
        return;
    };
    clear_selected_action(state);
    // The audio engine marks the active session as device_failed, so canceling
    // it here preserves durable chunks for recovery instead of deleting them.
    cancel_prefetch_asr(state);
    cancel_audio(state, app.clone()).await;
    let (keep_audio_days, keep_history_days) = {
        let settings = lock_recover(&state.settings);
        (settings.keep_audio_days, settings.keep_history_days)
    };
    recover_spool_into_history(app, keep_audio_days, keep_history_days);
    release_live_operation(app, state, failure_generation).await;
    fail_for_generation(
        app,
        state,
        format!("Microphone recording stopped: {message}"),
        failure_generation,
    )
    .await;
}

async fn handle_audio_limit(app: &tauri::AppHandle, state: &AppState) {
    let Some(claim) = dictation::claim_stop_entry(state).await else {
        return;
    };

    // The audio engine has already detached the stream. Give the user a short
    // visible explanation, then finish the captured samples automatically so
    // the 15-minute ceiling cannot leave the app stuck in Recording.
    emit_state_with_context(app, "recording_limited", Some(&claim.recording_context));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let _ = stop_claimed(app, state, claim).await;
}

async fn paste_text(
    app: &tauri::AppHandle,
    state: &AppState,
    text: &str,
    expected_target: &context::TargetAppGuard,
    accessibility: bool,
    cancellation: CancellationToken,
    recording_context: Option<&context::ContextSnapshot>,
) -> Result<paste::InsertOutcome, String> {
    let worker_app = app.clone();
    let worker_text = text.to_owned();
    let (mappings, browser_access_enabled) = {
        let current = lock_recover(&state.context);
        (current.mappings.clone(), current.browser_access_enabled)
    };
    let expected_target_owned = expected_target.clone();
    let restore_pid = expected_target.pid;
    let restore_window = expected_target.window_id;
    let outcome = tokio::task::spawn_blocking(move || {
        struct PasteYieldGuard;
        impl Drop for PasteYieldGuard {
            fn drop(&mut self) {
                crate::island_window::end_paste_yield();
            }
        }
        let _yield = PasteYieldGuard;
        crate::island_window::prepare_for_paste(&worker_app);
        let _ = paste::restore_delivery_target_if_needed(restore_pid, restore_window);
        let verify_target = move || {
            verify_delivery_target(&expected_target_owned, &mappings, browser_access_enabled)
        };
        paste::insert(
            &worker_app,
            &worker_text,
            accessibility,
            cancellation,
            verify_target,
            restore_pid,
        )
    })
    .await
    .map_err(|error| format!("paste worker failed: {error}"))?
    .map_err(|error| error.to_string())?;
    dictionary_learn::maybe_observe_after_paste(
        app,
        state,
        outcome.value_after.as_deref(),
        outcome.verified,
        expected_target,
        recording_context,
    );
    Ok(outcome)
}

async fn copy_text(
    app: &tauri::AppHandle,
    text: &str,
    cancellation: CancellationToken,
) -> Result<(), String> {
    let app = app.clone();
    let text = text.to_owned();
    tokio::task::spawn_blocking(move || paste::copy_if_not_cancelled(&app, &text, &cancellation))
        .await
        .map_err(|error| format!("clipboard worker failed: {error}"))?
        .map_err(|error| error.to_string())
}

fn arm_undo_transaction(
    state: &AppState,
    session_generation: u64,
    target_guard: &context::TargetAppGuard,
    post_insert_input_fingerprint: Option<u64>,
    delivery_method: &str,
    used_keyboard_paste: bool,
) {
    if !used_keyboard_paste || delivery_method != "paste" {
        return;
    }
    let Some(post_insert_input_fingerprint) = post_insert_input_fingerprint else {
        return;
    };
    let now = std::time::Instant::now();
    *state
        .undo
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(UndoTransaction {
        session_generation,
        created_at: now,
        expires_at: now + std::time::Duration::from_secs(3),
        target_guard: target_guard.clone(),
        delivery_method: delivery_method.to_owned(),
        post_insert_input_fingerprint,
        consumed: false,
    });
}

fn undo_preflight(
    transaction: &UndoTransaction,
    current_generation: u64,
    now: std::time::Instant,
) -> &'static str {
    if transaction.delivery_method != "paste" || now < transaction.created_at {
        "not_available"
    } else if transaction.consumed {
        "already_consumed"
    } else if now >= transaction.expires_at {
        "expired"
    } else if current_generation != transaction.session_generation {
        "stale_target"
    } else {
        "available"
    }
}

#[tauri::command]
async fn undo_last_delivery(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let generation = lock_recover(&state.manager).session_generation;
    let now = std::time::Instant::now();
    let transaction = {
        let mut undo = state
            .undo
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(transaction) = undo.as_mut() else {
            return Ok("not_available".into());
        };
        match undo_preflight(transaction, generation, now) {
            "already_consumed" => return Ok("already_consumed".into()),
            "expired" => {
                transaction.consumed = true;
                return Ok("expired".into());
            }
            "stale_target" => {
                transaction.consumed = true;
                return Ok("stale_target".into());
            }
            "not_available" => return Ok("not_available".into()),
            "available" => {}
            _ => unreachable!(),
        }
        let transaction = transaction.clone();
        // Claim the transaction before doing the blocking target probe so a
        // double click cannot send two undo shortcuts.
        if let Some(current) = undo.as_mut() {
            current.consumed = true;
        }
        transaction
    };

    let (mappings, browser_access_enabled) = {
        let current = lock_recover(&state.context);
        (current.mappings.clone(), current.browser_access_enabled)
    };
    let expected_target = transaction.target_guard.clone();
    let (current_target, current_input_fingerprint) = tokio::task::spawn_blocking(move || {
        let current_target =
            context::detect_snapshot(&mappings, browser_access_enabled).target_guard;
        let current_input_fingerprint =
            context::focused_input_value().map(|value| paste::selection_fingerprint(&value));
        (current_target, current_input_fingerprint)
    })
    .await
    .map_err(|error| format!("undo target probe failed: {error}"))?;
    if context::target_mismatch_reason(&expected_target, &current_target).is_some() {
        return Ok("stale_target".into());
    }
    if current_input_fingerprint != Some(transaction.post_insert_input_fingerprint) {
        return Ok("stale_target".into());
    }
    let accessibility = permissions::check().accessibility;
    tokio::task::spawn_blocking(move || paste::undo(accessibility))
        .await
        .map_err(|error| format!("undo worker failed: {error}"))?
        .map_err(|error| error.to_string())?;
    let _ = app.emit(
        "dictation://undo",
        serde_json::json!({ "status": "success" }),
    );
    Ok("success".into())
}

fn onboarding_delivery_target_matches(
    enabled: bool,
    _recording_context: &context::ContextSnapshot,
    _frontmost: (i32, Option<String>),
) -> bool {
    // Trial steps turn this mode on only while the onboarding window is
    // showing the try-it box. Requiring com.voiceflow.desktop fails in
    // `tauri dev`, where the process often reports as Cursor or Terminal.
    enabled
}

fn completion_hud_dwell_ms(phase: &str) -> u64 {
    match phase {
        "done" | "unverified" | "history" | "degraded" => 3_000,
        "copied" | "error" => 4_500,
        _ => 1_500,
    }
}

fn should_use_onboarding_delivery(
    state: &AppState,
    recording_context: &context::ContextSnapshot,
) -> bool {
    onboarding_delivery_target_matches(
        *lock_recover(&state.onboarding_test_mode),
        recording_context,
        context::frontmost_application_key(),
    )
}

fn emit_onboarding_result(app: &tauri::AppHandle, raw_text: &str, final_text: &str) {
    let _ = app.emit(
        "dictation://onboarding-result",
        serde_json::json!({
            "raw_text": raw_text,
            "final_text": final_text,
        }),
    );
}

pub(crate) async fn stop_claimed(
    app: &tauri::AppHandle,
    state: &AppState,
    claim: StopClaim,
) -> Result<(), String> {
    let StopClaim {
        started,
        session_generation,
        cancellation,
        recording_context,
    } = claim;
    state.gate.set_session_generation(session_generation);

    // Finalizing a long recording can take noticeable time. Show processing
    // immediately so the HUD never appears to ignore the user's stop press.
    sync_modifier_hotkey_phase(Phase::Stopping);
    emit_state_with_context(app, "processing", Some(&recording_context));
    emit_processing_phase(
        app,
        "finalizing_audio",
        Some(&recording_context),
        None,
        None,
    );
    emit_progress(app, 0.05);
    let stop_to_insert = state.metrics.timer(metrics::MetricKind::StopToInsert);

    // Phase 2: finalizing and encoding audio can take seconds for a long
    // recording, so keep it off the async runtime worker.
    let stop_result = match tokio::time::timeout(
        AUDIO_FINALIZATION_TIMEOUT,
        stop_audio(state, app.clone()),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err("audio finalization timed out".to_owned()),
    };
    let (wav, chunks) = match stop_result {
        Ok(result) => result,
        Err(error) => {
            cancel_prefetch_asr(state);
            let message = error;
            let cleanup_is_current = {
                let _gate = state.hotkey_gate.lock().await;
                let stop_was_cancelled = {
                    let m = lock_recover(&state.manager);
                    !stop_transition_is_current(
                        m.phase,
                        m.session_generation,
                        session_generation,
                        m.cancellation.is_cancelled(),
                    )
                };
                if stop_was_cancelled {
                    release_operation(state, OperationLease::LiveDictation);
                    sync_modifier_hotkey_phase(Phase::Idle);
                    hotkey::unregister_cancel(app);
                    false
                } else {
                    let mut m = lock_recover(&state.manager);
                    m.cancellation.cancel();
                    m.phase = Phase::Idle;
                    m.recording_context = None;
                    drop(m);
                    release_operation(state, OperationLease::LiveDictation);
                    sync_modifier_hotkey_phase(Phase::Idle);
                    hotkey::unregister_cancel(app);
                    true
                }
            };
            if !cleanup_is_current {
                return Ok(());
            }
            fail_for_generation(app, state, message.clone(), session_generation).await;
            return Err(message);
        }
    };

    // All capture-side files have been queued by the time finalization
    // returns. Give silent batch prefetch (not streaming ASR) a short
    // opportunity to finish; normal final ASR covers missing chunk indexes.
    let prefetch_result = finish_prefetch_asr(state).await;

    // Phase 3 (sync, short lock): claim the processing transition only if the
    // stop still belongs to this session. Cancellation is allowed while audio
    // finalization is in flight; in that case the finalized samples are simply
    // dropped and no provider or delivery work may start.
    let should_process = {
        let mut m = lock_recover(&state.manager);
        if stop_transition_is_current(
            m.phase,
            m.session_generation,
            session_generation,
            m.cancellation.is_cancelled(),
        ) {
            m.phase = Phase::Processing;
            true
        } else {
            false
        }
    };
    if !should_process {
        let current_generation = lock_recover(&state.manager).session_generation;
        release_live_operation(app, state, current_generation).await;
        return Ok(());
    }
    sync_modifier_hotkey_phase(Phase::Processing);

    let settings = lock_recover(&state.settings).clone();
    let selected_action = state
        .selected_action
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(selected_action) = selected_action {
        return process_selected_action(
            app,
            state,
            wav,
            &settings,
            &recording_context,
            selected_action,
            session_generation,
            cancellation,
            stop_to_insert,
        )
        .await;
    }
    let is_long =
        should_chunk_recording(started.elapsed().as_secs(), settings.chunk_threshold_secs);
    schedule_processing_watchdog(
        app,
        session_generation,
        started.elapsed().as_secs(),
        is_long,
    );
    if is_long {
        process_long(
            app,
            state,
            chunks,
            started,
            &settings,
            &recording_context,
            prefetch_result
                .as_ref()
                .map(|result| result.transcripts.clone()),
            session_generation,
            cancellation,
            stop_to_insert,
        )
        .await
    } else {
        process_short(
            app,
            state,
            wav,
            chunks,
            started,
            &settings,
            &recording_context,
            prefetch_result,
            session_generation,
            cancellation,
            stop_to_insert,
        )
        .await
    }
}

fn processing_watchdog_delay(recording_secs: u64, is_long: bool) -> std::time::Duration {
    let estimated_secs = if is_long {
        // Long recordings perform several provider requests. Scale the
        // emergency bound with capture length, while keeping a finite ceiling
        // for a provider or worker that never returns.
        recording_secs.saturating_mul(2).saturating_add(180)
    } else {
        // A short recording can use ASR + cleanup, each with bounded retries.
        PROCESSING_WATCHDOG_MIN_SECS
    };
    std::time::Duration::from_secs(
        estimated_secs.clamp(PROCESSING_WATCHDOG_MIN_SECS, PROCESSING_WATCHDOG_MAX_SECS),
    )
}

fn schedule_processing_watchdog(
    app: &tauri::AppHandle,
    expected_generation: u64,
    recording_secs: u64,
    is_long: bool,
) {
    let app = app.clone();
    let delay = processing_watchdog_delay(recording_secs, is_long);
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        let state = app.state::<AppState>();
        recover_processing_timeout(&app, &state, expected_generation).await;
    });
}

fn prefetch_short_tail_wav(chunks: &[chunker::AudioChunk]) -> Option<Vec<u8>> {
    let chunk = chunks.iter().find(|chunk| chunk.index == 0)?;
    let tail_start =
        prefetch_asr::WARMUP_CHUNK_SECS.saturating_sub(prefetch_asr::WARMUP_OVERLAP_SECS) * 16_000;
    if chunk.samples.len() <= tail_start {
        return None;
    }
    chunker::encode_wav(&chunk.samples[tail_start..]).ok()
}

fn selected_target_error(reason: &'static str) -> paste::PasteError {
    match reason {
        "browser_permission_required" => paste::PasteError::BrowserAccessRequired,
        "input_unavailable" => paste::PasteError::InputUnavailable,
        "input_changed" => paste::PasteError::InputChanged,
        "secure_input" => paste::PasteError::InputUnavailable,
        "target_unavailable" => paste::PasteError::TargetUnavailable,
        _ => paste::PasteError::TargetChanged,
    }
}

const TARGET_PROBE_RETRIES: usize = 3;
const TARGET_PROBE_RETRY_DELAY_MS: u64 = 60;

/// macOS can briefly report no frontmost application while switching Spaces,
/// activating a browser tab, or handing focus back after a global shortcut.
/// Retry only that transient "unavailable" result. A concrete app/window/input
/// mismatch remains fail-closed and falls back to the clipboard.
fn target_guard_mismatch_with_retry(
    expected: &context::TargetAppGuard,
    mut detect: impl FnMut() -> context::TargetAppGuard,
) -> Option<&'static str> {
    for attempt in 0..TARGET_PROBE_RETRIES {
        let current = detect();
        let reason = context::target_mismatch_reason(expected, &current)?;
        if reason == "target_unavailable" && attempt + 1 < TARGET_PROBE_RETRIES {
            std::thread::sleep(std::time::Duration::from_millis(
                TARGET_PROBE_RETRY_DELAY_MS,
            ));
            continue;
        }
        return Some(reason);
    }
    Some("target_unavailable")
}

fn verify_delivery_target(
    expected: &context::TargetAppGuard,
    mappings: &[context::AppMapping],
    browser_access_enabled: bool,
) -> Result<(), paste::PasteError> {
    match target_guard_mismatch_with_retry(expected, || {
        context::detect_snapshot(mappings, browser_access_enabled).target_guard
    }) {
        None => Ok(()),
        Some(reason) => Err(selected_target_error(reason)),
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_selected_action(
    app: &tauri::AppHandle,
    state: &AppState,
    wav: Vec<u8>,
    settings: &store::Settings,
    recording_context: &context::ContextSnapshot,
    selected_action: SelectedActionSession,
    session_generation: u64,
    cancellation: CancellationToken,
    stop_to_insert: metrics::LatencyTimer,
) -> Result<(), String> {
    emit_selected_action_state(app, "preparing_rewrite");
    let provider = current_asr_provider(state);
    let options = asr::AsrOptions {
        api_key: settings.asr_credential().to_owned(),
        language: asr::normalize_language(Some(settings.language.as_str())).map(str::to_owned),
        prompt: asr_prompt_for_snapshot(
            app.path().app_data_dir().ok().as_deref(),
            &settings.dictionary,
            recording_context,
            settings.asr_provider,
            &settings.asr_model,
        ),
        model: asr::resolve_recognition_model(
            &settings.asr_model,
            Some(settings.language.as_str()),
        )
        .to_owned(),
    };
    let transcript = {
        let _latency = state.metrics.timer(metrics::MetricKind::FinalAsr);
        queue::execute_with_retry_cancelled(
            &state.gate,
            queue::RequestKind::Asr,
            || provider.transcribe_batch(wav.clone(), options.clone()),
            cancellation.clone(),
        )
        .await
    };
    let transcript = match transcript {
        Ok(transcript) => {
            state.gate.update_asr(&transcript.limits);
            transcript.text
        }
        Err(queue::ExecuteError::Cancelled) => return Ok(()),
        Err(queue::ExecuteError::Operation(error)) => {
            emit_selected_action_state(app, "idle");
            let message = error.to_string();
            fail_for_generation(app, state, message.clone(), session_generation).await;
            return Err(message);
        }
    };
    if processing_aborted(state, session_generation) {
        return Ok(());
    }
    if transcript.trim().is_empty() {
        emit_selected_action_state(app, "idle");
        let message = "No speech detected".to_owned();
        fail_for_generation(app, state, message.clone(), session_generation).await;
        return Err(message);
    }

    let cleanup_endpoint = settings.cleanup_endpoint();
    let cleanup_model = settings.cleanup_request_model();
    let cleanup_key = settings.cleanup_credential().to_owned();
    let cleanup = {
        let _latency = state.metrics.timer(metrics::MetricKind::Cleanup);
        queue::execute_with_retry_cancelled(
            &state.gate,
            queue::RequestKind::Llm,
            || {
                llm::selected_text_action_with_limits(
                    &cleanup_endpoint,
                    &cleanup_model,
                    &selected_action.selected_text,
                    &transcript,
                    &cleanup_key,
                    Some(&recording_context.policy),
                    Some(&recording_context.profile),
                    spoken_translation_target(settings),
                )
            },
            cancellation.clone(),
        )
        .await
    };
    let final_text = match cleanup {
        Ok((text, limits)) => {
            state.gate.update_llm(&limits);
            text
        }
        Err(queue::ExecuteError::Cancelled) => return Ok(()),
        Err(queue::ExecuteError::Operation(error)) => {
            emit_selected_action_state(app, "idle");
            let message = error.to_string();
            fail_for_generation(app, state, message.clone(), session_generation).await;
            return Err(message);
        }
    };
    if processing_aborted(state, session_generation) {
        return Ok(());
    }
    if final_text.trim().is_empty() {
        emit_selected_action_state(app, "idle");
        let message = "Selected text action returned no result".to_owned();
        fail_for_generation(app, state, message.clone(), session_generation).await;
        return Err(message);
    }

    if selected_action.onboarding_trial {
        let _ = app.emit(
            "selected-action://onboarding-result",
            serde_json::json!({
                "raw_text": transcript,
                "final_text": final_text,
            }),
        );
        finish_with_delivery(
            app,
            state,
            "done",
            Some(recording_context),
            "onboarding",
            None,
            Some(CLEANUP_STATUS_AI_SUCCESS),
            Some(session_generation),
        )
        .await;
        return Ok(());
    }

    let preview = SelectedActionPreview {
        session: selected_action,
        session_generation,
        context: recording_context.clone(),
    };
    *state
        .selected_preview
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(preview);
    stop_to_insert.finish();
    if move_processing_to_selected_preview(state, session_generation) {
        hotkey::unregister_cancel(app);
        sync_modifier_hotkey_phase(Phase::Idle);
        emit_progress(app, 0.0);
        emit_selected_action_state(app, "preview_ready");
        let _ = app.emit(
            "selected-action://preview",
            serde_json::json!({
                "selected_text": state
                    .selected_preview
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_ref()
                    .map(|preview| preview.session.selected_text.clone())
                    .unwrap_or_default(),
                "transcript": transcript,
                "final_text": final_text,
            }),
        );
        island_window::hide_overlay(app);
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }
    Ok(())
}

fn move_processing_to_selected_preview(state: &AppState, expected_generation: u64) -> bool {
    let mut manager = lock_recover(&state.manager);
    if manager.phase != Phase::Processing
        || manager.session_generation != expected_generation
        || manager.cancellation.is_cancelled()
    {
        return false;
    }
    manager.cancellation.cancel();
    manager.phase = Phase::Idle;
    manager.recording_context = None;
    true
}

#[allow(clippy::too_many_arguments)]
async fn process_short(
    app: &tauri::AppHandle,
    state: &AppState,
    wav: Vec<u8>,
    chunks: Vec<chunker::AudioChunk>,
    started: std::time::Instant,
    settings: &store::Settings,
    recording_context: &context::ContextSnapshot,
    prefetch_result: Option<prefetch_asr::PrefetchAsrResult>,
    session_generation: u64,
    cancellation: CancellationToken,
    stop_to_insert: metrics::LatencyTimer,
) -> Result<(), String> {
    emit_processing_phase(app, "asr", Some(recording_context), None, None);
    let language = asr::normalize_language(Some(settings.language.as_str())).map(str::to_owned);
    let asr_prompt = asr_prompt_for_snapshot(
        app.path().app_data_dir().ok().as_deref(),
        &settings.dictionary,
        recording_context,
        settings.asr_provider,
        &settings.asr_model,
    );
    let asr_provider = current_asr_provider(state);
    let asr_options = asr::AsrOptions {
        api_key: settings.asr_credential().to_owned(),
        language,
        prompt: asr_prompt,
        model: asr::resolve_recognition_model(
            &settings.asr_model,
            Some(settings.language.as_str()),
        )
        .to_owned(),
    };
    let cleanup_policy = cleanup_policy_for(settings, recording_context);
    let mut raw = None;
    if let Some(warmup) = prefetch_result
        .as_ref()
        .and_then(|result| result.warmup.as_deref())
    {
        let total_samples = chunks.first().map(|chunk| chunk.samples.len()).unwrap_or(0);
        if total_samples <= prefetch_asr::WARMUP_CHUNK_SECS * 16_000 {
            raw = Some(warmup.to_owned());
        } else if let Some(tail_wav) = prefetch_short_tail_wav(&chunks) {
            let tail_result = {
                let _latency = state.metrics.timer(metrics::MetricKind::FinalAsr);
                queue::execute_with_retry_cancelled(
                    &state.gate,
                    queue::RequestKind::Asr,
                    || asr_provider.transcribe_batch(tail_wav.clone(), asr_options.clone()),
                    cancellation.clone(),
                )
                .await
            };
            match tail_result {
                Ok(tail) => {
                    state.gate.update_asr(&tail.limits);
                    raw = Some(chunker::merge_transcripts(vec![
                        (0, warmup.to_owned()),
                        (1, tail.text),
                    ]));
                }
                Err(queue::ExecuteError::Cancelled) => return Ok(()),
                Err(queue::ExecuteError::Operation(error)) => {
                    log::warn!(
                        "batch-prefetch tail ASR failed; falling back to the complete recording: {error}"
                    );
                }
            }
        }
    }
    let raw = if let Some(raw) = raw {
        raw
    } else {
        let audio_for_retry = wav.clone();
        let transcript = {
            let _latency = state.metrics.timer(metrics::MetricKind::FinalAsr);
            match queue::execute_with_retry_cancelled(
                &state.gate,
                queue::RequestKind::Asr,
                || asr_provider.transcribe_batch(audio_for_retry.clone(), asr_options.clone()),
                cancellation.clone(),
            )
            .await
            {
                Err(queue::ExecuteError::Cancelled) => return Ok(()),
                Err(queue::ExecuteError::Operation(e)) => {
                    if processing_aborted(state, session_generation) {
                        return Ok(());
                    }
                    let message = e.to_string();
                    if let Ok(dir) = app.path().app_data_dir() {
                        let relative =
                            std::path::PathBuf::from(format!("failed-{}.wav", chrono_like_id()));
                        match store::write_spool_file(&dir, &relative, &wav) {
                            Ok(path) => {
                                if let Err(history_error) =
                                    store::insert_failed_history_with_context(
                                        &dir,
                                        "",
                                        started.elapsed().as_secs_f64(),
                                        Some(&path),
                                        recording_context,
                                    )
                                {
                                    log::warn!(
                                        "failed to record ASR retry history: {history_error}"
                                    );
                                }
                            }
                            Err(spool_error) => {
                                log::warn!("failed to preserve retry audio: {spool_error}");
                            }
                        }
                    }
                    fail_for_generation(app, state, message.clone(), session_generation).await;
                    return Err(message);
                }
                Ok(v) => {
                    state.gate.update_asr(&v.limits);
                    v
                }
            }
        };
        transcript.text
    };
    let raw = prepare_spoken_transcript(
        &raw,
        recording_context.profile.family,
        recording_context.profile.confidence,
    );
    let app_dir = app.path().app_data_dir().ok();
    let (raw, pairs_hint) =
        prepare_lexicon_transcript(app_dir.as_deref(), &settings.dictionary, &raw);
    let spoken_raw = raw.clone();
    let intent = llm::parse_cleanup_intent(&raw, spoken_translation_target(settings));
    let clipboard = snippets::read_clipboard_if_needed(&settings.snippets, &raw, || {
        clipboard_text_for_snippets(app)
    });
    let snippet_expansion =
        snippets::resolve_exact_with_clipboard(&settings.snippets, &raw, clipboard.as_deref());
    let snippet_expanded = snippet_expansion.is_some();
    let raw = snippet_expansion.clone().unwrap_or(raw);
    let cleanup_input = if snippet_expanded {
        raw.clone()
    } else {
        intent.content.clone()
    };
    if processing_aborted(state, session_generation) {
        return Ok(());
    }
    if raw.trim().is_empty() {
        let message = "No speech detected".to_string();
        fail_for_generation(app, state, message.clone(), session_generation).await;
        return Err(message);
    }
    emit_progress(app, 0.45);
    let cleanup_route = cleanup_route_for(settings, Some(recording_context), &intent);
    let cleanup_decision = if !snippet_expanded {
        match cleanup_route {
        lexicon::CleanupRoute::LocalOnly => CleanupDecision::Disabled,
        lexicon::CleanupRoute::Provider(effort) => {
        emit_processing_phase(app, "cleanup", Some(recording_context), None, None);
        let pairs_hint = pairs_hint.clone();
        let cleanup_endpoint = settings.cleanup_endpoint();
        let cleanup_model = settings.cleanup_request_model();
        let cleanup_key = settings.cleanup_credential().to_owned();
        let cleanup_result = {
            let _latency = state.metrics.timer(metrics::MetricKind::Cleanup);
            queue::execute_with_retry_cancelled(
                &state.gate,
                queue::RequestKind::Llm,
                || {
                    llm::cleanup_with_model_and_limits_and_language_and_profile_and_intent(
                        &cleanup_endpoint,
                        &cleanup_model,
                        &cleanup_input,
                        &cleanup_key,
                        &[],
                        None,
                        Some(&cleanup_policy),
                        Some(settings.language.as_str()),
                        cleanup_profile_for(recording_context),
                        Some(&intent),
                        pairs_hint.as_deref(),
                        effort,
                    )
                },
                cancellation.clone(),
            )
            .await
        };
        match cleanup_result {
            Ok((text, limits)) => {
                state.gate.update_llm(&limits);
                CleanupDecision::Provider(text)
            }
            Err(queue::ExecuteError::Cancelled) => return Ok(()),
            Err(queue::ExecuteError::Operation(error)) => {
                log::warn!("LLM cleanup failed, using the raw transcript: {error}");
                CleanupDecision::Failed
            }
        }
        }
        }
    } else {
        CleanupDecision::Disabled
    };
    let cleanup_status = match &cleanup_decision {
        CleanupDecision::Provider(text) if text.trim().is_empty() => {
            cleanup_failure_status(
                &cleanup_input,
                &local_cleanup_or_raw(&cleanup_input, recording_context.profile.family),
            )
        }
        CleanupDecision::Provider(_) => CLEANUP_STATUS_AI_SUCCESS,
        CleanupDecision::Failed => {
            cleanup_failure_status(
                &cleanup_input,
                &local_cleanup_or_raw(&cleanup_input, recording_context.profile.family),
            )
        }
        CleanupDecision::Disabled if snippet_expanded => CLEANUP_STATUS_SNIPPET_BYPASS,
        CleanupDecision::Disabled => CLEANUP_STATUS_LOCAL_ONLY,
    };
    if processing_aborted(state, session_generation) {
        return Ok(());
    }
    emit_progress(app, 0.70);
    let resolved_text = match finalize_text(
        &cleanup_input,
        cleanup_decision,
        recording_context.profile.family,
        &load_learn_pairs(app_dir.as_deref()),
        &settings.dictionary,
    ) {
        Ok(value) => value,
        Err("no_speech") => {
            let message = "No speech detected".to_string();
            fail_for_generation(app, state, message.clone(), session_generation).await;
            return Err(message);
        }
        Err(_) => unreachable!("finalize_text only returns no_speech"),
    };
    let final_text = resolved_text.text;
    let degraded = resolved_text.degraded;
    let degraded_reason = resolved_text.degraded_reason;
    let mut recovery_spool = if degraded {
        persist_short_recovery_audio(app, &wav)
    } else {
        None
    };
    if processing_aborted(state, session_generation) {
        discard_short_recovery_audio(app, recovery_spool.as_deref());
        return Ok(());
    }
    // Paste with graceful fallback: if the simulated keystroke fails (no
    // accessibility permission, timeout, panic), the text is already on the
    // clipboard from `insert`'s first step — but we also explicitly copy it so
    // the user can paste manually, and surface a "copied" state on the Island.
    // `paste_text` performs the fail-closed target check twice: once before
    // preparing the clipboard and once immediately before keyboard injection.
    // Avoid an extra detector pass here; browser AppleScript/System Events can
    // each take hundreds of milliseconds, and the inner checks are the safety
    // boundary that matters for the irreversible keystroke.
    let delivery_policy = delivery::DeliveryPolicy::parse(&settings.delivery_policy);
    let (pasted, fallback_reason, delivery_method) = {
        if processing_aborted(state, session_generation) {
            return Ok(());
        }
        emit_progress(app, 0.88);
        emit_processing_phase(app, "delivery", Some(recording_context), None, None);
        if matches!(delivery_policy, delivery::DeliveryPolicy::HistoryOnly) {
            (false, None, delivery::DeliveryMethod::History.as_str())
        } else if matches!(delivery_policy, delivery::DeliveryPolicy::ClipboardOnly) {
            if let Err(copy_err) = copy_text(app, &final_text, cancellation.clone()).await {
                let message = copy_err.to_string();
                record_delivery_failure(
                    app,
                    &spoken_raw,
                    &final_text,
                    started.elapsed().as_secs_f64(),
                    recording_context,
                    recovery_spool.as_deref(),
                    cleanup_status,
                );
                fail_for_generation(app, state, message.clone(), session_generation).await;
                return Err(message);
            }
            (false, None, delivery::DeliveryMethod::Clipboard.as_str())
        } else {
            let paste_result = {
                let _latency = state.metrics.timer(metrics::MetricKind::Paste);
                if should_use_onboarding_delivery(state, recording_context) {
                    emit_onboarding_result(app, &spoken_raw, &final_text);
                    Ok(paste::InsertOutcome {
                        shortcut_sent: true,
                        used_keyboard_paste: false,
                        verified: true,
                        post_insert_input_fingerprint: None,
                        value_after: None,
                    })
                } else {
                    paste_text(
                        app,
                        state,
                        &final_text,
                        &recording_context.target_guard,
                        permissions::check().accessibility,
                        cancellation.clone(),
                        Some(recording_context),
                    )
                    .await
                }
            };
            match paste_result {
                Ok(outcome) => {
                    debug_assert!(outcome.shortcut_sent);
                    let result = delivery::DeliveryResult::from_insert_verified(outcome.verified);
                    arm_undo_transaction(
                        state,
                        session_generation,
                        &recording_context.target_guard,
                        outcome.post_insert_input_fingerprint,
                        result.method.as_str(),
                        outcome.used_keyboard_paste,
                    );
                    (
                        outcome.verified,
                        result.fallback_reason,
                        result.method.as_str(),
                    )
                }
                Err(e) => {
                    if processing_aborted(state, session_generation) || cancellation.is_cancelled()
                    {
                        discard_short_recovery_audio(app, recovery_spool.as_deref());
                        return Ok(());
                    }
                    log::warn!("paste failed, falling back to clipboard: {e}");
                    if let Err(copy_err) = copy_text(app, &final_text, cancellation.clone()).await {
                        if processing_aborted(state, session_generation)
                            || cancellation.is_cancelled()
                        {
                            discard_short_recovery_audio(app, recovery_spool.as_deref());
                            return Ok(());
                        }
                        // Total failure: neither paste nor clipboard worked.
                        if recovery_spool.is_none() {
                            recovery_spool = persist_short_recovery_audio(app, &wav);
                        }
                        record_delivery_failure(
                            app,
                            &spoken_raw,
                            &final_text,
                            started.elapsed().as_secs_f64(),
                            recording_context,
                            recovery_spool.as_deref(),
                            cleanup_status,
                        );
                        fail_for_generation(app, state, copy_err.to_string(), session_generation)
                            .await;
                        return Err(copy_err);
                    }
                    (
                        false,
                        Some(delivery_fallback_reason(true, &e)),
                        delivery::DeliveryMethod::Clipboard.as_str(),
                    )
                }
            }
        }
    };
    let delivery_result = delivery::DeliveryResult::for_method(delivery_method, fallback_reason);
    let mut fallback_reason = delivery_result.fallback_reason;
    let delivery_method = delivery_result.method.as_str();
    if degraded && fallback_reason.is_none() {
        fallback_reason = degraded_reason;
    }
    // Cancellation can arrive after delivery returns but before durable History
    // persistence. Do not let an aborted processing generation leave a late
    // result behind as if the user had completed the dictation.
    if processing_aborted(state, session_generation) || cancellation.is_cancelled() {
        discard_short_recovery_audio(app, recovery_spool.as_deref());
        return Ok(());
    }
    if recovery_spool.is_none() {
        recovery_spool =
            persist_success_gold_audio(app, settings, recording_context, &wav, degraded);
    }
    if let Ok(dir) = app.path().app_data_dir() {
        if let Err(history_error) = store::insert_history_with_delivery_and_spool_and_cleanup(
            &dir,
            &spoken_raw,
            &final_text,
            started.elapsed().as_secs_f64(),
            degraded,
            degraded_reason,
            if degraded {
                "degraded"
            } else if delivery_method == "paste_unverified" {
                "unverified"
            } else if pasted {
                "ok"
            } else {
                "copied"
            },
            delivery_method,
            fallback_reason,
            recording_context,
            recovery_spool.as_deref(),
            cleanup_status,
        ) {
            log::warn!("failed to record dictation history: {history_error}");
        }
    }
    if processing_aborted(state, session_generation) {
        return Ok(());
    }
    emit_progress(app, 1.0);
    let completion = completion_state_for_result(degraded, &delivery_result);
    if pasted {
        stop_to_insert.finish();
        finish_with_delivery(
            app,
            state,
            completion,
            Some(recording_context),
            delivery_method,
            fallback_reason,
            Some(cleanup_status),
            Some(session_generation),
        )
        .await;
    } else if delivery_method == delivery::DeliveryMethod::Clipboard.as_str() {
        stop_to_insert.finish();
        let _ = app.emit(
            "dictation://copied",
            serde_json::json!({ "message": "已复制，请手动粘贴" }),
        );
        finish_with_delivery(
            app,
            state,
            completion,
            Some(recording_context),
            delivery_method,
            fallback_reason,
            Some(cleanup_status),
            Some(session_generation),
        )
        .await;
    } else {
        stop_to_insert.finish();
        finish_with_delivery(
            app,
            state,
            completion,
            Some(recording_context),
            delivery_method,
            fallback_reason,
            Some(cleanup_status),
            Some(session_generation),
        )
        .await;
    }
    Ok(())
}

fn should_keep_success_audio(
    settings: &store::Settings,
    recording_context: &context::ContextSnapshot,
) -> bool {
    if !settings.keep_success_audio || settings.keep_audio_days == 0 {
        return false;
    }
    if recording_context.target_guard.secure_input {
        return false;
    }
    lexicon::scene_allows_learn(
        &settings.context_mappings,
        &recording_context.profile.id,
        recording_context.target_guard.bundle_id.as_deref(),
        recording_context.target_guard.browser_host.as_deref(),
    )
}

fn persist_gold_wav(dir: &Path, wav: &[u8]) -> Option<std::path::PathBuf> {
    let sequence = SHORT_RECOVERY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let file_name = format!("gold-{}-{sequence}.wav", chrono_like_id());
    match store::write_gold_file(dir, &file_name, wav) {
        Ok(path) => Some(path),
        Err(error) => {
            log::warn!("failed to keep success audio for later review: {error}");
            None
        }
    }
}

fn persist_success_gold_audio(
    app: &tauri::AppHandle,
    settings: &store::Settings,
    recording_context: &context::ContextSnapshot,
    wav: &[u8],
    degraded: bool,
) -> Option<std::path::PathBuf> {
    if degraded || !should_keep_success_audio(settings, recording_context) {
        return None;
    }
    let dir = match app.path().app_data_dir() {
        Ok(dir) => dir,
        Err(error) => {
            log::warn!("failed to resolve app data directory for gold audio: {error}");
            return None;
        }
    };
    persist_gold_wav(&dir, wav)
}

fn persist_long_success_gold(
    app: &tauri::AppHandle,
    settings: &store::Settings,
    recording_context: &context::ContextSnapshot,
    session_dir: Option<&Path>,
) -> Option<std::path::PathBuf> {
    if !should_keep_success_audio(settings, recording_context) {
        return None;
    }
    let session_dir = session_dir?;
    let recovery = match store::rebuild_spool_recovery(session_dir) {
        Ok(recovery) => recovery,
        Err(error) => {
            log::warn!("failed to rebuild success audio for gold keep: {error}");
            return None;
        }
    };
    let wav = match store::read_spool_file(&recovery.audio_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            log::warn!("failed to read rebuilt success audio for gold keep: {error}");
            return None;
        }
    };
    let dir = match app.path().app_data_dir() {
        Ok(dir) => dir,
        Err(error) => {
            log::warn!("failed to resolve app data directory for gold audio: {error}");
            return None;
        }
    };
    persist_gold_wav(&dir, &wav)
}

fn persist_short_recovery_audio(app: &tauri::AppHandle, wav: &[u8]) -> Option<std::path::PathBuf> {
    let dir = match app.path().app_data_dir() {
        Ok(dir) => dir,
        Err(error) => {
            log::warn!("failed to resolve app data directory for retry audio: {error}");
            return None;
        }
    };
    let sequence = SHORT_RECOVERY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let relative = std::path::PathBuf::from(format!("failed-{}-{sequence}.wav", chrono_like_id()));
    match store::write_spool_file(&dir, &relative, wav) {
        Ok(path) => Some(path),
        Err(error) => {
            log::warn!("failed to preserve short recording for retry: {error}");
            None
        }
    }
}

fn discard_short_recovery_audio(app: &tauri::AppHandle, path: Option<&std::path::Path>) {
    let Some(path) = path else {
        return;
    };
    if let Ok(dir) = app.path().app_data_dir() {
        store::remove_spool_artifact(&dir, path);
    }
}

const LONG_ASR_WORK_WINDOW: usize = 2;

fn process_bounded_chunk_jobs<I, F, Fut, T>(items: I, worker: F) -> impl Stream<Item = T>
where
    I: IntoIterator,
    F: FnMut(I::Item) -> Fut,
    Fut: Future<Output = T>,
{
    futures_util::stream::iter(items)
        .map(worker)
        .buffer_unordered(LONG_ASR_WORK_WINDOW)
}

enum LongChunkJobResult {
    Completed {
        index: usize,
        start_secs: f32,
        end_secs: f32,
        transcript: Result<String, queue::ExecuteError<asr::AsrError>>,
    },
    EncodingFailed(String),
}

#[allow(clippy::too_many_arguments)]
async fn process_long(
    app: &tauri::AppHandle,
    state: &AppState,
    chunks: Vec<chunker::AudioChunk>,
    started: std::time::Instant,
    settings: &store::Settings,
    recording_context: &context::ContextSnapshot,
    prefetched_transcripts: Option<std::collections::HashMap<usize, String>>,
    session_generation: u64,
    cancellation: CancellationToken,
    stop_to_insert: metrics::LatencyTimer,
) -> Result<(), String> {
    let total = chunks.len();
    emit_processing_phase(app, "asr", Some(recording_context), None, Some((0, total)));
    emit_progress(app, 0.05);
    let app_data_root = app.path().app_data_dir().ok();
    let dir = app_data_root.as_ref().map(|p| {
        p.join("spool")
            .join(format!("session-{}", chrono_like_id()))
    });
    if let Some(session_dir) = &dir {
        if let Some(root) = &app_data_root {
            if let Some(session_id) = session_dir.file_name().and_then(|name| name.to_str()) {
                if let Err(error) = store::begin_spool_session(root, session_id) {
                    log::warn!("failed to create long-recording manifest: {error}");
                }
            }
        }
    }
    let asr_options = asr::AsrOptions {
        api_key: settings.asr_credential().to_owned(),
        language: asr::normalize_language(Some(settings.language.as_str())).map(str::to_owned),
        prompt: asr_prompt_for_snapshot(
            app.path().app_data_dir().ok().as_deref(),
            &settings.dictionary,
            recording_context,
            settings.asr_provider,
            &settings.asr_model,
        ),
        model: asr::resolve_recognition_model(
            &settings.asr_model,
            Some(settings.language.as_str()),
        )
        .to_owned(),
    };
    let cleanup_policy = cleanup_policy_for(settings, recording_context);
    let worker_prefetch = prefetched_transcripts.unwrap_or_default();
    let worker_gate = state.gate.clone();
    let worker_metrics = state.metrics.clone();
    let worker_provider = current_asr_provider(state);
    let worker_cancellation = cancellation.clone();
    let worker_session_dir = dir.clone();
    let worker_spool_root = app_data_root.clone();
    let jobs = process_bounded_chunk_jobs(chunks, move |chunk| {
        let prefetched = worker_prefetch.get(&chunk.index).cloned();
        let gate = worker_gate.clone();
        let metrics = worker_metrics.clone();
        let provider = worker_provider.clone();
        let options = asr_options.clone();
        let cancellation = worker_cancellation.clone();
        let session_dir = worker_session_dir.clone();
        let spool_root = worker_spool_root.clone();
        async move {
            let index = chunk.index;
            let chunk_start_secs = chunk.start_secs;
            let chunk_end_secs = chunk.end_secs;
            if cancellation.is_cancelled() {
                return LongChunkJobResult::Completed {
                    index,
                    start_secs: chunk_start_secs,
                    end_secs: chunk_end_secs,
                    transcript: Err(queue::ExecuteError::Cancelled),
                };
            }
            if let (Some(d), Some(root)) = (&session_dir, &spool_root) {
                let spool_bytes = chunk
                    .samples
                    .iter()
                    .flat_map(|sample| sample.to_le_bytes())
                    .collect::<Vec<_>>();
                let session_name = d
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("session");
                let relative = std::path::PathBuf::from(session_name)
                    .join("chunks")
                    .join(format!("{index:08}.f32"));
                match store::write_spool_file(root, &relative, &spool_bytes) {
                    Ok(_) => {
                        if let Err(error) = store::record_spool_chunk(
                            d,
                            index,
                            chunk_start_secs,
                            chunk_end_secs,
                            "written",
                        ) {
                            log::warn!("failed to update long-recording manifest: {error}");
                        }
                    }
                    Err(error) => {
                        log::warn!("failed to preserve long-recording chunk: {error}");
                    }
                }
            }
            let transcript = if let Some(prefetched) = prefetched {
                Ok(prefetched)
            } else {
                let wav = match chunker::encode_wav(&chunk.samples) {
                    Ok(wav) => wav,
                    Err(error) => {
                        return LongChunkJobResult::EncodingFailed(error.to_string());
                    }
                };
                let _latency = metrics.timer(metrics::MetricKind::FinalAsr);
                queue::execute_with_retry_cancelled(
                    &gate,
                    queue::RequestKind::Asr,
                    || provider.transcribe_batch(wav.clone(), options.clone()),
                    cancellation,
                )
                .await
                .map(|transcript| transcript.text)
            };
            LongChunkJobResult::Completed {
                index,
                start_secs: chunk_start_secs,
                end_secs: chunk_end_secs,
                transcript,
            }
        }
    });
    futures_util::pin_mut!(jobs);
    let mut raw_texts = Vec::new();
    let mut failed_chunks = 0usize;
    let mut cleanup_failure_reason: Option<&'static str> = None;
    let mut done = 0usize;
    while let Some(job) = jobs.next().await {
        if processing_aborted(state, session_generation) {
            discard_spool(dir.as_deref());
            return Ok(());
        }
        let (index, chunk_start_secs, chunk_end_secs, result) = match job {
            LongChunkJobResult::Completed {
                index,
                start_secs,
                end_secs,
                transcript,
            } => (index, start_secs, end_secs, transcript),
            LongChunkJobResult::EncodingFailed(error) => {
                let message = format!("long-recording audio encoding failed: {error}");
                mark_spool_degraded(dir.as_deref());
                fail_for_generation(app, state, message.clone(), session_generation).await;
                return Err(message);
            }
        };
        match result {
            Ok(raw) => {
                if let Some(session_dir) = &dir {
                    let _ = store::record_spool_chunk(
                        session_dir,
                        index,
                        chunk_start_secs,
                        chunk_end_secs,
                        "transcribed",
                    );
                }
                raw_texts.push((index, raw.clone()));
            }
            Err(queue::ExecuteError::Cancelled) => {
                discard_spool(dir.as_deref());
                return Ok(());
            }
            Err(queue::ExecuteError::Operation(_)) => {
                failed_chunks += 1;
                if let Some(session_dir) = &dir {
                    let _ = store::record_spool_chunk(
                        session_dir,
                        index,
                        chunk_start_secs,
                        chunk_end_secs,
                        "asr_failed",
                    );
                }
            }
        }
        done += 1;
        let progress = if total == 0 {
            0.1
        } else {
            0.1 + 0.72 * done as f32 / total as f32
        };
        let _ = app.emit(
            "dictation://progress",
            long_chunk_progress_payload(
                session_generation,
                started.elapsed().as_secs(),
                done,
                total,
                progress.clamp(0.0, 0.84),
            ),
        );
        emit_processing_phase(
            app,
            "asr",
            Some(recording_context),
            None,
            Some((done, total)),
        );
    }
    if processing_aborted(state, session_generation) {
        if let Some(d) = dir {
            let _ = std::fs::remove_dir_all(d);
        }
        return Ok(());
    }
    if total > 0 && failed_chunks == total {
        mark_spool_degraded(dir.as_deref());
        let recovery = dir
            .as_deref()
            .and_then(|session_dir| store::rebuild_spool_recovery(session_dir).ok());
        record_delivery_failure(
            app,
            "",
            "",
            started.elapsed().as_secs_f64(),
            recording_context,
            recovery.as_ref().map(|item| item.audio_path.as_path()),
            CLEANUP_STATUS_UNKNOWN,
        );
        let message = "语音转写失败，音频已保留在历史记录中，可重试".to_string();
        fail_for_generation(app, state, message.clone(), session_generation).await;
        return Err(message);
    }
    let app_dir = app.path().app_data_dir().ok();
    let (raw_text, pairs_hint) = prepare_lexicon_transcript(
        app_dir.as_deref(),
        &settings.dictionary,
        &prepare_spoken_transcript(
            &chunker::merge_transcripts(raw_texts),
            recording_context.profile.family,
            recording_context.profile.confidence,
        ),
    );
    let intent = llm::parse_cleanup_intent(&raw_text, spoken_translation_target(settings));
    let clipboard = snippets::read_clipboard_if_needed(&settings.snippets, &raw_text, || {
        clipboard_text_for_snippets(app)
    });
    let snippet_expansion = snippets::resolve_exact_with_clipboard(
        &settings.snippets,
        &raw_text,
        clipboard.as_deref(),
    );
    let cleanup_input = snippet_expansion
        .clone()
        .unwrap_or_else(|| intent.content.clone());
    let cleanup_status;
    let cleanup_route = cleanup_route_for(settings, Some(recording_context), &intent);
    let final_text = if let Some(expansion) = snippet_expansion {
        cleanup_status = CLEANUP_STATUS_SNIPPET_BYPASS;
        expansion
    } else if let lexicon::CleanupRoute::Provider(effort) = cleanup_route {
        // Long recordings are cleaned only after every ASR chunk has been
        // merged. This gives the model the complete spoken structure instead
        // of asking it to make independent decisions at chunk boundaries.
        emit_progress(app, 0.86);
        emit_processing_phase(
            app,
            "cleanup",
            Some(recording_context),
            None,
            Some((total, total)),
        );
        let pairs_hint = pairs_hint.clone();
        let cleanup_endpoint = settings.cleanup_endpoint();
        let cleanup_model = settings.cleanup_request_model();
        let cleanup_key = settings.cleanup_credential().to_owned();
        let cleanup_result = {
            let _latency = state.metrics.timer(metrics::MetricKind::Cleanup);
            queue::execute_with_retry_cancelled(
                &state.gate,
                queue::RequestKind::Llm,
                || {
                    llm::cleanup_with_model_and_limits_and_language_and_profile_and_intent(
                        &cleanup_endpoint,
                        &cleanup_model,
                        &cleanup_input,
                        &cleanup_key,
                        &[],
                        None,
                        Some(&cleanup_policy),
                        Some(settings.language.as_str()),
                        cleanup_profile_for(recording_context),
                        Some(&intent),
                        pairs_hint.as_deref(),
                        effort,
                    )
                },
                cancellation.clone(),
            )
            .await
        };
        match cleanup_result {
            Ok((text, limits)) if !text.trim().is_empty() => {
                state.gate.update_llm(&limits);
                cleanup_status = CLEANUP_STATUS_AI_SUCCESS;
                text
            }
            Ok((_text, limits)) => {
                state.gate.update_llm(&limits);
                cleanup_failure_reason = Some("llm_cleanup_empty");
                let fallback = local_cleanup_or_raw(&cleanup_input, recording_context.profile.family);
                cleanup_status = cleanup_failure_status(&cleanup_input, &fallback);
                fallback
            }
            Err(queue::ExecuteError::Cancelled) => {
                discard_spool(dir.as_deref());
                return Ok(());
            }
            Err(error) => {
                cleanup_failure_reason = Some("llm_cleanup_failed");
                log::warn!(
                    "long-recording LLM cleanup failed after transcript merge, using local cleanup: {error:?}"
                );
                let fallback = local_cleanup_or_raw(&cleanup_input, recording_context.profile.family);
                cleanup_status = cleanup_failure_status(&cleanup_input, &fallback);
                fallback
            }
        }
    } else {
        cleanup_status = CLEANUP_STATUS_LOCAL_ONLY;
        local_cleanup_or_raw(&cleanup_input, recording_context.profile.family)
    };
    let final_text = lexicon::apply_promoted_replacements(
        &final_text,
        &load_learn_pairs(app_dir.as_deref()),
        &settings.dictionary,
    );
    if final_text.trim().is_empty() {
        let message = "No speech detected".to_string();
        mark_spool_degraded(dir.as_deref());
        if let Some(d) = &dir {
            let _ = std::fs::remove_dir_all(d);
        }
        fail_for_generation(app, state, message.clone(), session_generation).await;
        return Err(message);
    }
    // Output with graceful fallback to clipboard so the text is never lost.
    emit_progress(app, 0.88);
    emit_processing_phase(
        app,
        "delivery",
        Some(recording_context),
        None,
        Some((total, total)),
    );
    let delivery_policy = delivery::DeliveryPolicy::parse(&settings.delivery_policy);
    let mut delivered_via_paste = false;
    let mut delivery_method = "history";
    let mut fallback_reason = None;
    if delivery_policy.attempts_paste() {
        delivery_method = "clipboard";
        if failed_chunks > 0 {
            if processing_aborted(state, session_generation) {
                discard_spool(dir.as_deref());
                return Ok(());
            }
            if let Err(error) = copy_text(app, &final_text, cancellation.clone()).await {
                if processing_aborted(state, session_generation) || cancellation.is_cancelled() {
                    discard_spool(dir.as_deref());
                    return Ok(());
                }
                let message = error.to_string();
                mark_spool_degraded(dir.as_deref());
                let recovery = dir
                    .as_deref()
                    .and_then(|session_dir| store::rebuild_spool_recovery(session_dir).ok());
                record_delivery_failure(
                    app,
                    &raw_text,
                    &final_text,
                    started.elapsed().as_secs_f64(),
                    recording_context,
                    recovery.as_ref().map(|item| item.audio_path.as_path()),
                    cleanup_status,
                );
                fail_for_generation(app, state, message.clone(), session_generation).await;
                return Err(message);
            }
            fallback_reason = Some("partial_asr_failure");
        } else if should_use_onboarding_delivery(state, recording_context) {
            emit_onboarding_result(app, &raw_text, &final_text);
            delivered_via_paste = true;
            delivery_method = "paste";
        } else {
            if processing_aborted(state, session_generation) {
                discard_spool(dir.as_deref());
                return Ok(());
            }
            let paste_result = {
                let _latency = state.metrics.timer(metrics::MetricKind::Paste);
                paste_text(
                    app,
                    state,
                    &final_text,
                    &recording_context.target_guard,
                    permissions::check().accessibility,
                    cancellation.clone(),
                    Some(recording_context),
                )
                .await
            };
            match paste_result {
                Ok(outcome) => {
                    debug_assert!(outcome.shortcut_sent);
                    let result = delivery::DeliveryResult::from_insert_verified(outcome.verified);
                    arm_undo_transaction(
                        state,
                        session_generation,
                        &recording_context.target_guard,
                        outcome.post_insert_input_fingerprint,
                        result.method.as_str(),
                        outcome.used_keyboard_paste,
                    );
                    delivered_via_paste = outcome.verified;
                    delivery_method = result.method.as_str();
                    if fallback_reason.is_none() {
                        fallback_reason = result.fallback_reason;
                    }
                }
                Err(e) => {
                    if processing_aborted(state, session_generation) || cancellation.is_cancelled()
                    {
                        discard_spool(dir.as_deref());
                        return Ok(());
                    }
                    log::warn!("long-recording paste failed, falling back to clipboard: {e}");
                    if let Err(copy_err) = copy_text(app, &final_text, cancellation.clone()).await {
                        if processing_aborted(state, session_generation)
                            || cancellation.is_cancelled()
                        {
                            discard_spool(dir.as_deref());
                            return Ok(());
                        }
                        let message = copy_err.to_string();
                        mark_spool_degraded(dir.as_deref());
                        let recovery = dir.as_deref().and_then(|session_dir| {
                            store::rebuild_spool_recovery(session_dir).ok()
                        });
                        record_delivery_failure(
                            app,
                            &raw_text,
                            &final_text,
                            started.elapsed().as_secs_f64(),
                            recording_context,
                            recovery.as_ref().map(|item| item.audio_path.as_path()),
                            cleanup_status,
                        );
                        fail_for_generation(app, state, message.clone(), session_generation).await;
                        return Err(message);
                    }
                    fallback_reason = Some(delivery_fallback_reason(true, &e));
                }
            }
        }
    } else if matches!(delivery_policy, delivery::DeliveryPolicy::ClipboardOnly) {
        delivery_method = "clipboard";
        if processing_aborted(state, session_generation) {
            discard_spool(dir.as_deref());
            return Ok(());
        }
        if let Err(e) = copy_text(app, &final_text, cancellation.clone()).await {
            if processing_aborted(state, session_generation) || cancellation.is_cancelled() {
                discard_spool(dir.as_deref());
                return Ok(());
            }
            let message = e.to_string();
            mark_spool_degraded(dir.as_deref());
            let recovery = dir
                .as_deref()
                .and_then(|session_dir| store::rebuild_spool_recovery(session_dir).ok());
            record_delivery_failure(
                app,
                &raw_text,
                &final_text,
                started.elapsed().as_secs_f64(),
                recording_context,
                recovery.as_ref().map(|item| item.audio_path.as_path()),
                cleanup_status,
            );
            fail_for_generation(app, state, message.clone(), session_generation).await;
            return Err(message);
        }
    }
    let degraded = failed_chunks > 0 || cleanup_failure_reason.is_some();
    let degraded_reason = match (failed_chunks > 0, cleanup_failure_reason) {
        (true, Some(_)) => Some("partial_asr_and_llm_failure"),
        (true, None) => Some("partial_asr_failure"),
        (false, Some(reason)) => Some(reason),
        (false, None) => None,
    };
    if degraded_reason.is_some() && fallback_reason.is_none() {
        fallback_reason = degraded_reason;
    }
    if let Some(session_dir) = &dir {
        let _ =
            store::mark_spool_status(session_dir, if degraded { "degraded" } else { "completed" });
    }
    if processing_aborted(state, session_generation) {
        discard_spool(dir.as_deref());
        return Ok(());
    }
    emit_progress(app, 1.0);
    let degraded_spool_path = if degraded {
        dir.as_deref().and_then(
            |session_dir| match store::rebuild_spool_recovery(session_dir) {
                Ok(recovery) => Some(recovery.audio_path),
                Err(error) => {
                    log::warn!("failed to rebuild degraded recording for retry: {error}");
                    None
                }
            },
        )
    } else {
        None
    };
    let success_gold_path = if degraded {
        None
    } else {
        persist_long_success_gold(app, settings, recording_context, dir.as_deref())
    };
    let insert_spool = degraded_spool_path
        .as_deref()
        .or(success_gold_path.as_deref());
    if let Ok(app_data_dir) = app.path().app_data_dir() {
        if let Err(history_error) = store::insert_history_with_delivery_and_spool_and_cleanup(
            &app_data_dir,
            &raw_text,
            &final_text,
            started.elapsed().as_secs_f64(),
            degraded,
            degraded_reason,
            if degraded {
                "degraded"
            } else if delivery_method == "clipboard" {
                "copied"
            } else if delivery_method == "paste_unverified" {
                "unverified"
            } else {
                "ok"
            },
            delivery_method,
            fallback_reason,
            recording_context,
            insert_spool,
            cleanup_status,
        ) {
            log::warn!("failed to record long dictation history: {history_error}");
        }
    }
    if !degraded {
        if let Some(d) = dir {
            let _ = std::fs::remove_dir_all(d);
        }
    }
    let copied_fallback = delivery_policy.attempts_paste() && !delivered_via_paste;
    if copied_fallback {
        stop_to_insert.finish();
        if !degraded {
            let _ = app.emit(
                "dictation://copied",
                serde_json::json!({ "message": "已复制，请手动粘贴" }),
            );
        }
        finish_with_delivery(
            app,
            state,
            long_completion_state(degraded, delivery_method, true),
            Some(recording_context),
            "clipboard",
            fallback_reason,
            Some(cleanup_status),
            Some(session_generation),
        )
        .await;
    } else {
        stop_to_insert.finish();
        finish_with_delivery(
            app,
            state,
            long_completion_state(degraded, delivery_method, false),
            Some(recording_context),
            delivery_method,
            fallback_reason,
            Some(cleanup_status),
            Some(session_generation),
        )
        .await;
    }
    Ok(())
}
fn processing_aborted(state: &AppState, session_generation: u64) -> bool {
    let m = lock_recover(&state.manager);
    m.phase != Phase::Processing || m.session_generation != session_generation
}

fn claim_processing_timeout(
    manager: &mut DictationManager,
    expected_generation: u64,
) -> Option<u64> {
    if manager.phase != Phase::Processing || manager.session_generation != expected_generation {
        return None;
    }
    manager.cancellation.cancel();
    manager.phase = Phase::Idle;
    manager.recording_context = None;
    manager.session_generation = manager.session_generation.wrapping_add(1);
    Some(manager.session_generation)
}

async fn recover_processing_timeout(
    app: &tauri::AppHandle,
    state: &AppState,
    expected_generation: u64,
) {
    let Some(completion_generation) = ({
        let mut manager = lock_recover(&state.manager);
        claim_processing_timeout(&mut manager, expected_generation)
    }) else {
        return;
    };

    log::error!("processing watchdog recovered session generation {expected_generation}");
    sync_modifier_hotkey_phase(Phase::Idle);
    hotkey::unregister_cancel(app);
    show_island(app);
    let _ = app.emit(
        "dictation://error",
        "Transcription took too long and was cancelled. Please try again.",
    );
    finish_with_delivery(
        app,
        state,
        "error",
        None,
        "none",
        Some("processing_timeout"),
        None,
        Some(completion_generation),
    )
    .await;
}

fn processing_completion_is_current(
    phase: Phase,
    current_generation: u64,
    expected_generation: u64,
    cancelled: bool,
) -> bool {
    phase == Phase::Processing && current_generation == expected_generation && !cancelled
}

fn error_completion_is_current(
    phase: Phase,
    current_generation: u64,
    expected_generation: u64,
    cancelled: bool,
) -> bool {
    (phase == Phase::Processing && current_generation == expected_generation && !cancelled)
        || (phase == Phase::Idle && current_generation == expected_generation)
}

fn stop_transition_is_current(
    phase: Phase,
    current_generation: u64,
    expected_generation: u64,
    cancelled: bool,
) -> bool {
    phase == Phase::Stopping && current_generation == expected_generation && !cancelled
}

fn should_chunk_recording(elapsed_secs: u64, threshold_secs: u64) -> bool {
    // Groq's upload limit makes a 15-minute 16 kHz PCM WAV too large for a
    // single request (~28.8 MB). Keep the user-configured threshold, but force
    // chunking before a direct ASR upload can exceed a conservative 10-minute
    // budget.
    const MAX_DIRECT_ASR_SECS: u64 = 10 * 60;
    elapsed_secs >= threshold_secs || elapsed_secs >= MAX_DIRECT_ASR_SECS
}

fn should_refresh_context_preview(
    same_application: bool,
    browser_refresh: bool,
    last_accessibility: Option<bool>,
    accessibility: bool,
) -> bool {
    !same_application || browser_refresh || last_accessibility != Some(accessibility)
}

fn long_completion_state(
    degraded: bool,
    delivery_method: &str,
    copied_fallback: bool,
) -> &'static str {
    if copied_fallback {
        completion_state(degraded, false)
    } else {
        completion_state_for_delivery(degraded, delivery_method)
    }
}

fn completion_state(degraded: bool, pasted: bool) -> &'static str {
    if degraded {
        "degraded"
    } else if pasted {
        "done"
    } else {
        "copied"
    }
}

fn completion_state_for_delivery(degraded: bool, delivery_method: &str) -> &'static str {
    if degraded {
        "degraded"
    } else if delivery_method == "paste_unverified" {
        "unverified"
    } else if delivery_method == "clipboard" {
        "copied"
    } else if delivery_method == "history" {
        "history"
    } else {
        "done"
    }
}

fn completion_state_for_result(
    degraded: bool,
    delivery_result: &delivery::DeliveryResult,
) -> &'static str {
    if degraded {
        "degraded"
    } else if !delivery_result.verified {
        "unverified"
    } else {
        completion_state_for_delivery(degraded, delivery_result.method.as_str())
    }
}

fn mark_spool_degraded(dir: Option<&std::path::Path>) {
    if let Some(dir) = dir {
        let _ = store::mark_spool_status(dir, "degraded");
    }
}

fn discard_spool(dir: Option<&std::path::Path>) {
    if let Some(dir) = dir {
        let _ = std::fs::remove_dir_all(dir);
    }
}

fn record_delivery_failure(
    app: &tauri::AppHandle,
    raw: &str,
    final_text: &str,
    duration: f64,
    context: &context::ContextSnapshot,
    spool: Option<&std::path::Path>,
    cleanup_status: &str,
) {
    if let Ok(dir) = app.path().app_data_dir() {
        if let Err(history_error) = store::insert_history_with_delivery_and_spool_and_cleanup(
            &dir,
            raw,
            final_text,
            duration,
            true,
            Some("delivery_failed"),
            "failed",
            "none",
            Some("delivery_failed"),
            context,
            spool,
            cleanup_status,
        ) {
            log::warn!("failed to record delivery failure history: {history_error}");
        }
    }
}

pub(crate) fn load_learn_pairs(dir: Option<&Path>) -> Vec<store::LearnPairRecord> {
    dir.and_then(|path| store::list_learn_pairs(path).ok())
        .unwrap_or_default()
}

pub(crate) fn prepare_spoken_transcript(
    raw: &str,
    family: context::ContextFamily,
    confidence: f32,
) -> String {
    spoken_revision::apply(&spoken_layout::apply_after_punctuation(
        raw, family, confidence,
    ))
}

pub(crate) fn prepare_lexicon_transcript(
    dir: Option<&Path>,
    dictionary: &[String],
    raw: &str,
) -> (String, Option<String>) {
    let pairs = load_learn_pairs(dir);
    let replaceable = lexicon::replaceable_pairs(&pairs, dictionary);
    let hits = lexicon::hit_pairs(raw, &replaceable, dictionary);
    let replaced = lexicon::apply_lexicon_replacements(raw, &replaceable, dictionary);
    if let Some(path) = dir {
        let used = lexicon::used_pair_keys(&replaced, &pairs);
        let _ = store::bump_learn_pairs_used(path, &used);
    }
    (replaced, lexicon::format_cleanup_pairs(&hits))
}

fn asr_prompt_for_snapshot(
    dir: Option<&Path>,
    dictionary: &[String],
    snapshot: &context::ContextSnapshot,
    asr_provider: crate::providers::EngineProvider,
    asr_model: &str,
) -> Option<String> {
    let pairs = load_learn_pairs(dir);
    let scope = lexicon::PromptScope::from_snapshot(snapshot);
    lexicon::build_asr_prompt_shaped(
        dictionary,
        Some(&snapshot.policy),
        &pairs,
        Some(&scope),
        lexicon::asr_prompt_shape_for(asr_provider, asr_model),
    )
}

fn cleanup_route_for(
    settings: &store::Settings,
    recording_context: Option<&context::ContextSnapshot>,
    intent: &llm::CleanupIntent,
) -> lexicon::CleanupRoute {
    let mapping = recording_context.and_then(|snapshot| {
        lexicon::mapping_for_profile(&settings.context_mappings, &snapshot.profile.id)
    });
    let family = recording_context
        .map(|snapshot| snapshot.profile.family)
        .unwrap_or(context::ContextFamily::General);
    let confidence = recording_context
        .map(|snapshot| snapshot.profile.confidence)
        .unwrap_or(0.0);
    lexicon::decide_cleanup(
        settings.cleanup_enabled,
        mapping,
        family,
        intent,
        confidence,
    )
}
#[cfg(test)]
fn abort_processing_manager(manager: &mut DictationManager) -> bool {
    if manager.phase != Phase::Processing {
        return false;
    }
    manager.cancellation.cancel();
    manager.session_generation = manager.session_generation.wrapping_add(1);
    manager.phase = Phase::Idle;
    manager.recording_context = None;
    true
}

#[cfg(test)]
fn apply_processing_abort(manager: &mut DictationManager, lease: &mut OperationLease) -> bool {
    if !abort_processing_manager(manager) {
        return false;
    }
    release_operation_lease(lease, OperationLease::LiveDictation);
    true
}
async fn fail_for_generation(
    app: &tauri::AppHandle,
    state: &AppState,
    message: String,
    expected_generation: u64,
) {
    let is_current = {
        let manager = lock_recover(&state.manager);
        error_completion_is_current(
            manager.phase,
            manager.session_generation,
            expected_generation,
            manager.cancellation.is_cancelled(),
        )
    };
    if !is_current {
        return;
    }

    show_island(app);
    let error_reason = error_fallback_reason(&message);
    let _ = app.emit("dictation://error", &message);
    finish_with_delivery(
        app,
        state,
        "error",
        None,
        "none",
        Some(error_reason),
        None,
        Some(expected_generation),
    )
    .await;
}
#[allow(clippy::too_many_arguments)]
async fn finish_with_delivery(
    app: &tauri::AppHandle,
    state: &AppState,
    phase: &str,
    context: Option<&context::ContextSnapshot>,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    cleanup_status: Option<&str>,
    expected_generation: Option<u64>,
) {
    // Keep the global lock order consistent with start claims: operation lease
    // before manager. Never wait on the lease while holding the manager lock.
    let completion_generation = {
        let _gate = state.hotkey_gate.lock().await;
        let defer_operation_release = expected_generation.is_some_and(|generation| {
            *lock_recover(&state.pending_recorder_cancel) == Some(generation)
        });
        let lease = expected_generation.map(|_| *lock_recover(&state.operation_lease));
        let mut m = lock_recover(&state.manager);
        if let Some(expected_generation) = expected_generation {
            let is_current = if phase == "error" {
                error_completion_is_current(
                    m.phase,
                    m.session_generation,
                    expected_generation,
                    m.cancellation.is_cancelled(),
                )
            } else {
                processing_completion_is_current(
                    m.phase,
                    m.session_generation,
                    expected_generation,
                    m.cancellation.is_cancelled(),
                ) || selected_preview_completion_is_current(
                    m.phase,
                    m.session_generation,
                    expected_generation,
                    lease.unwrap_or(OperationLease::Idle),
                )
            };
            if !is_current {
                return;
            }
        }
        m.cancellation.cancel();
        m.phase = Phase::Idle;
        m.recording_context = None;
        let completion_generation = m.session_generation;
        drop(m);
        if !defer_operation_release {
            release_operation(state, OperationLease::LiveDictation);
            // Only the current processing generation may release the active
            // cancel shortcut. A stale completion can arrive after the user
            // cancelled and started a new recording; unregistering here would
            // otherwise remove the new recording's Escape handler.
            hotkey::unregister_cancel(app);
            sync_modifier_hotkey_phase(Phase::Idle);
        }
        if phase == "error" {
            show_island(app);
        }
        if matches!(
            phase,
            "done" | "unverified" | "copied" | "degraded" | "history"
        ) {
            emit_progress(app, 1.0);
        }
        if let Some(context) = context {
            emit_state_with_delivery(
                app,
                phase,
                Some(context),
                delivery_method,
                fallback_reason,
                cleanup_status,
            );
        } else {
            emit_state_with_delivery(
                app,
                phase,
                None,
                delivery_method,
                fallback_reason,
                cleanup_status,
            );
        }
        completion_generation
    };
    let dwell_ms = completion_hud_dwell_ms(phase);
    tokio::time::sleep(std::time::Duration::from_millis(dwell_ms)).await;
    let should_hide = {
        let m = lock_recover(&state.manager);
        m.phase == Phase::Idle && m.session_generation == completion_generation
    };
    if should_hide {
        emit_state(app, "idle");
        // Let the WebView finish its short opacity exit before hiding the
        // native panel. Hiding the panel in the same turn as the idle event
        // cuts off copied/degraded/error fades and reads as a dropped frame.
        tokio::time::sleep(std::time::Duration::from_millis(140)).await;
        let should_hide_after_fade = {
            let m = lock_recover(&state.manager);
            m.phase == Phase::Idle && m.session_generation == completion_generation
        };
        if should_hide_after_fade {
            island_window::hide_overlay(app);
        }
    }
}
#[tauri::command]
async fn set_hotkeys_suspended(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    suspended: bool,
    captured_hotkey: Option<String>,
    captured_activation_mode: Option<String>,
    capture_target: Option<String>,
) -> Result<(), String> {
    if suspended {
        hotkey::pause_for_capture(&app).await?;
        return Ok(());
    }

    let _guard = state.settings_gate.lock().await;
    let previous = lock_recover(&state.settings).clone();
    let mut settings = previous.clone();
    if let Some(hotkey) = captured_hotkey {
        let hotkey = hotkey::canonicalize_hotkey(&hotkey);
        if capture_target.as_deref() == Some("selected_action") {
            settings.selected_action_hotkey = hotkey;
            settings.selected_actions_enabled = true;
        } else {
            settings.hotkey = hotkey;
        }
    }
    if let Some(mode) = captured_activation_mode {
        apply_captured_activation_mode(&mut settings, &mode);
    }
    clamp_double_tap_activation(&mut settings);
    if let Err(error) = settings.validate() {
        let _ =
            hotkey::apply_settings_hotkey(&app, &previous.hotkey, &previous.activation_mode).await;
        let _ = hotkey::apply_selected_action_hotkey(
            &app,
            &previous.selected_action_hotkey,
            previous.selected_actions_enabled,
        )
        .await;
        hotkey::set_suspended(false);
        return Err(error.to_string());
    }

    // Re-register before committing the new settings. The hotkey adapter
    // restores the previous binding if registration fails.
    if let Err(error) =
        hotkey::apply_settings_hotkey(&app, &settings.hotkey, &settings.activation_mode).await
    {
        let _ = hotkey::apply_selected_action_hotkey(
            &app,
            &previous.selected_action_hotkey,
            previous.selected_actions_enabled,
        )
        .await;
        hotkey::set_suspended(false);
        return Err(error);
    }
    if let Err(error) = hotkey::apply_selected_action_hotkey(
        &app,
        &settings.selected_action_hotkey,
        settings.selected_actions_enabled,
    )
    .await
    {
        let _ =
            hotkey::apply_settings_hotkey(&app, &previous.hotkey, &previous.activation_mode).await;
        let _ = hotkey::apply_selected_action_hotkey(
            &app,
            &previous.selected_action_hotkey,
            previous.selected_actions_enabled,
        )
        .await;
        hotkey::set_suspended(false);
        return Err(error);
    }
    let dir = match app.path().app_data_dir() {
        Ok(dir) => dir,
        Err(error) => {
            let _ =
                hotkey::apply_settings_hotkey(&app, &previous.hotkey, &previous.activation_mode)
                    .await;
            let _ = hotkey::apply_selected_action_hotkey(
                &app,
                &previous.selected_action_hotkey,
                previous.selected_actions_enabled,
            )
            .await;
            hotkey::set_suspended(false);
            return Err(error.to_string());
        }
    };
    if let Err(error) = store::save_settings(&dir, &settings) {
        let _ =
            hotkey::apply_settings_hotkey(&app, &previous.hotkey, &previous.activation_mode).await;
        let _ = hotkey::apply_selected_action_hotkey(
            &app,
            &previous.selected_action_hotkey,
            previous.selected_actions_enabled,
        )
        .await;
        hotkey::set_suspended(false);
        return Err(error.to_string());
    }
    *lock_recover(&state.settings) = settings;
    hotkey::set_suspended(false);
    Ok(())
}
#[tauri::command]
fn set_onboarding_test_mode(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    *lock_recover(&state.onboarding_test_mode) = enabled;
    if !enabled {
        *lock_recover(&state.onboarding_selected_text) = None;
    }
    Ok(())
}

#[tauri::command]
fn set_onboarding_selected_text(state: State<'_, AppState>, text: String) -> Result<(), String> {
    if !*lock_recover(&state.onboarding_test_mode) {
        return Err("onboarding test mode is not active".into());
    }
    *lock_recover(&state.onboarding_selected_text) = (!text.trim().is_empty()).then_some(text);
    Ok(())
}
#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> store::SettingsView {
    store::SettingsView::from(&*lock_recover(&state.settings))
}
const MAX_DICTIONARY_FILE_BYTES: u64 = 1024 * 1024;

fn read_dictionary_file_contents(path: &Path) -> Result<String, String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    if !matches!(extension.as_deref(), Some("csv" | "txt" | "tsv")) {
        return Err("只支持 CSV、TXT 或 TSV 文件".into());
    }

    let metadata = std::fs::metadata(path).map_err(|_| "无法读取词典文件".to_owned())?;
    if !metadata.is_file() {
        return Err("词典路径不是文件".into());
    }
    if metadata.len() > MAX_DICTIONARY_FILE_BYTES {
        return Err("词典文件不能超过 1 MB".into());
    }

    let bytes = std::fs::read(path).map_err(|_| "无法读取词典文件".to_owned())?;
    String::from_utf8(bytes).map_err(|_| "词典文件必须使用 UTF-8 编码".into())
}

#[tauri::command]
fn read_dictionary_file(path: String) -> Result<String, String> {
    read_dictionary_file_contents(Path::new(&path))
}

#[tauri::command]
fn get_context_snapshot(state: State<'_, AppState>) -> context::ContextSnapshot {
    lock_recover(&state.context).snapshot.clone()
}
#[tauri::command]
fn get_context_mappings(state: State<'_, AppState>) -> Vec<context::AppMapping> {
    lock_recover(&state.context).mappings.clone()
}
#[tauri::command]
fn get_available_applications() -> Vec<context::ApplicationOption> {
    context::available_applications()
}
#[tauri::command]
fn get_application_from_path(path: String) -> Result<context::ApplicationOption, String> {
    context::application_from_path(&path)
}
#[tauri::command]
fn get_context_override(state: State<'_, AppState>) -> Option<context::ContextFamily> {
    lock_recover(&state.context).manual_override
}
#[tauri::command]
async fn save_context_mapping(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    mut mapping: context::AppMapping,
) -> Result<Vec<context::AppMapping>, String> {
    if let Some(host) = &mapping.browser_host {
        mapping.browser_host = context::normalize_host(host);
    }
    mapping.validate()?;
    let _guard = state.settings_gate.lock().await;
    let (enabled, browser_access_enabled, mut mappings) = {
        let current = lock_recover(&state.context);
        (
            current.enabled,
            current.browser_access_enabled,
            current.mappings.clone(),
        )
    };
    if let Some(existing) = mappings.iter_mut().find(|item| item.id == mapping.id) {
        *existing = mapping;
    } else {
        mappings.push(mapping);
    }
    persist_context_state(
        &app,
        &state,
        enabled,
        browser_access_enabled,
        mappings.clone(),
    )
    .await?;
    Ok(mappings)
}
#[tauri::command]
async fn delete_context_mapping(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<context::AppMapping>, String> {
    let _guard = state.settings_gate.lock().await;
    let (enabled, browser_access_enabled, mut mappings) = {
        let current = lock_recover(&state.context);
        (
            current.enabled,
            current.browser_access_enabled,
            current.mappings.clone(),
        )
    };
    mappings.retain(|mapping| mapping.id != id);
    persist_context_state(
        &app,
        &state,
        enabled,
        browser_access_enabled,
        mappings.clone(),
    )
    .await?;
    Ok(mappings)
}
#[tauri::command]
async fn set_context_enabled(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    let _guard = state.settings_gate.lock().await;
    let (browser_access_enabled, mappings) = {
        let current = lock_recover(&state.context);
        (current.browser_access_enabled, current.mappings.clone())
    };
    persist_context_state(&app, &state, enabled, browser_access_enabled, mappings).await
}
#[tauri::command]
async fn set_context_override(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    family: Option<context::ContextFamily>,
) -> Result<context::ContextSnapshot, String> {
    let _guard = state.settings_gate.lock().await;
    {
        let mut current = lock_recover(&state.context);
        current.manual_override = family;
    }
    refresh_context_snapshot(&app, &state).await;
    Ok(lock_recover(&state.context).snapshot.clone())
}
#[tauri::command]
async fn request_browser_access(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let _guard = state.settings_gate.lock().await;
    let (enabled, mappings) = {
        let current = lock_recover(&state.context);
        (current.enabled, current.mappings.clone())
    };
    persist_context_state(&app, &state, enabled, true, mappings).await?;
    // The OS automation prompt can only be triggered by querying a browser.
    // Settings is usually the frontmost app when this button is clicked, so
    // enable the user's intent here and let the next browser-focused refresh
    // perform the actual, scoped query.
    Ok("pending".into())
}
async fn apply_settings(
    app: tauri::AppHandle,
    state: &AppState,
    mut settings: store::Settings,
) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    clamp_double_tap_activation(&mut settings);
    settings.normalize();
    let prev = lock_recover(&state.settings).clone();
    let incoming_asr_key = settings.asr_api_key.clone();
    settings.asr_api_key = store::bind_asr_key_to_host(
        &prev.asr_base_url,
        &settings.asr_base_url,
        &incoming_asr_key,
        &prev.asr_api_key,
    );
    let incoming_cleanup_key = settings.cleanup_api_key.clone();
    settings.cleanup_api_key = store::bind_cleanup_key_to_host(
        &prev.cleanup_base_url,
        &settings.cleanup_base_url,
        &incoming_cleanup_key,
        &prev.cleanup_api_key,
    );
    let asr_host_changed = crate::asr::asr_host_changed(&prev.asr_base_url, &settings.asr_base_url);
    let asr_key_cleared = asr_host_changed && incoming_asr_key.trim().is_empty();
    let cleanup_host_changed =
        crate::llm::chat_host_changed(&prev.cleanup_base_url, &settings.cleanup_base_url);
    let cleanup_key_cleared = cleanup_host_changed && incoming_cleanup_key.trim().is_empty();
    settings.repair_incomplete_engine_sides();
    settings.validate().map_err(|error| error.to_string())?;
    let needs_api_key_validation =
        settings.onboarded && (!prev.onboarded || prev.api_key != settings.api_key);
    if needs_api_key_validation {
        let validation = groq::validate_key(&settings.api_key).await;
        if validation != "valid" {
            return Err(format!("API key validation failed: {validation}"));
        }
    }
    let hotkey = settings.hotkey.clone();
    let mode = settings.activation_mode.clone();
    let selected_hotkey = settings.selected_action_hotkey.clone();
    let selected_enabled = settings.selected_actions_enabled;
    let hotkey_changed = prev.hotkey != hotkey || prev.activation_mode != mode;
    let selected_hotkey_changed = prev.selected_action_hotkey != selected_hotkey
        || prev.selected_actions_enabled != selected_enabled;
    let context_changed = prev.context_enabled != settings.context_enabled
        || prev.browser_access_enabled != settings.browser_access_enabled
        || prev.context_mappings != settings.context_mappings
        || prev.writing_modes != settings.writing_modes;
    let context_enabled = settings.context_enabled;
    let browser_access_enabled = settings.browser_access_enabled;
    let context_mappings = settings.context_mappings.clone();
    let writing_modes = settings.writing_modes.clone();
    let tray_visible = settings.show_tray_icon;
    let tray_visibility_changed = prev.show_tray_icon != tray_visible;

    if !hotkey::is_suspended() && hotkey_changed {
        if let Err(error) = hotkey::apply_settings_hotkey(&app, &hotkey, &mode).await {
            let _ = hotkey::apply_selected_action_hotkey(
                &app,
                &prev.selected_action_hotkey,
                prev.selected_actions_enabled,
            )
            .await;
            return Err(error);
        }
    }
    if !hotkey::is_suspended() && (hotkey_changed || selected_hotkey_changed) {
        if let Err(error) =
            hotkey::apply_selected_action_hotkey(&app, &selected_hotkey, selected_enabled).await
        {
            if hotkey_changed {
                let _ =
                    hotkey::apply_settings_hotkey(&app, &prev.hotkey, &prev.activation_mode).await;
            }
            let _ = hotkey::apply_selected_action_hotkey(
                &app,
                &prev.selected_action_hotkey,
                prev.selected_actions_enabled,
            )
            .await;
            return Err(error);
        }
    }
    if let Err(error) = store::save_settings(&dir, &settings) {
        if !hotkey::is_suspended() && hotkey_changed {
            let _ = hotkey::apply_settings_hotkey(&app, &prev.hotkey, &prev.activation_mode).await;
        }
        if !hotkey::is_suspended() && (hotkey_changed || selected_hotkey_changed) {
            let _ = hotkey::apply_selected_action_hotkey(
                &app,
                &prev.selected_action_hotkey,
                prev.selected_actions_enabled,
            )
            .await;
        }
        return Err(error.to_string());
    }
    if asr_key_cleared {
        crate::keychain::set_asr_api_key("")
            .map_err(|error| format!("failed to remove ASR API key securely: {error}"))?;
    }
    if cleanup_key_cleared {
        crate::keychain::set_cleanup_api_key("")
            .map_err(|error| format!("failed to remove cleanup API key securely: {error}"))?;
    }
    if prev.keep_history_days != settings.keep_history_days {
        if let Err(error) = store::purge_history(&dir, settings.keep_history_days) {
            log::warn!("history retention cleanup after settings change failed: {error}");
        }
    }
    if prev.keep_audio_days != settings.keep_audio_days {
        if let Err(error) = store::purge_gold_audio(&dir, settings.keep_audio_days) {
            log::warn!("gold audio retention cleanup after settings change failed: {error}");
        }
    }
    let asr_provider_changed = prev.asr_provider != settings.asr_provider
        || prev.asr_base_url != settings.asr_base_url
        || prev.custom_base_url != settings.custom_base_url
        || prev.local_whisper_base_url != settings.local_whisper_base_url
        || prev.asr_api_key != settings.asr_api_key
        || prev.asr_endpoint() != settings.asr_endpoint();
    let next_asr_endpoint = settings.asr_endpoint();
    let settings_view = store::SettingsView::from(&settings);
    *lock_recover(&state.settings) = settings;
    if asr_key_cleared || cleanup_key_cleared {
        let _ = app.emit("settings://changed", settings_view);
    }
    if asr_provider_changed {
        rebuild_asr_provider(state, &next_asr_endpoint);
    }
    if tray_visibility_changed {
        if let Some(tray) = app.tray_by_id("voiceflow-status") {
            if let Err(error) = tray.set_visible(tray_visible) {
                log::warn!("failed to update tray icon visibility: {error}");
            }
        }
    }
    if context_changed {
        {
            let mut current = lock_recover(&state.context);
            current.enabled = context_enabled;
            current.browser_access_enabled = browser_access_enabled;
            current.mappings = context_mappings;
            current.writing_modes = writing_modes;
        }
        refresh_context_snapshot(&app, state).await;
    }
    if hotkey::is_suspended() {
        return Ok(());
    }
    Ok(())
}

#[tauri::command]
async fn set_settings(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    settings: store::Settings,
) -> Result<(), String> {
    let _guard = state.settings_gate.lock().await;
    apply_settings(app, &state, settings).await
}

#[tauri::command]
async fn update_settings_patch(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    patch: serde_json::Value,
) -> Result<(), String> {
    let _guard = state.settings_gate.lock().await;
    let object = patch
        .as_object()
        .ok_or_else(|| "settings patch must be an object".to_owned())?;
    const ALLOWED: &[&str] = &[
        "api_key",
        "language",
        "ui_language",
        "theme",
        "dictionary",
        "hotkey",
        "activation_mode",
        "chunk_threshold_secs",
        "chunk_length_secs",
        "delivery_policy",
        "keep_audio_days",
        "keep_history_days",
        "keep_success_audio",
        "onboarded",
        "cleanup_enabled",
        "cleanup_model",
        "show_tray_icon",
        "context_enabled",
        "browser_access_enabled",
        "context_mappings",
        "writing_modes",
        "snippets",
        "output_mode",
        "translation_target_language",
        "selected_action_hotkey",
        "selected_actions_enabled",
        "dictionary_learn_enabled",
        "input_device",
        "input_gain",
        "asr_base_url",
        "asr_api_key",
        "asr_model",
        "asr_provider",
        "cleanup_provider",
        "cleanup_base_url",
        "cleanup_api_key",
        "custom_base_url",
        "custom_asr",
        "custom_llm",
        "ollama_base_url",
        "local_whisper_base_url",
        "provider_keys",
        "provider_api_keys",
    ];
    if let Some(unknown) = object.keys().find(|key| !ALLOWED.contains(&key.as_str())) {
        return Err(format!("unsupported settings field: {unknown}"));
    }
    let current = lock_recover(&state.settings).clone();
    let mut merged = serde_json::to_value(&current).map_err(|error| error.to_string())?;
    let merged_object = merged
        .as_object_mut()
        .ok_or_else(|| "settings serialization failed".to_owned())?;
    for (key, value) in object {
        merged_object.insert(key.clone(), value.clone());
    }
    if object.contains_key("asr_base_url") && !object.contains_key("asr_api_key") {
        let next_url = merged_object
            .get("asr_base_url")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if crate::asr::asr_host_changed(&current.asr_base_url, next_url) {
            merged_object.insert(
                "asr_api_key".into(),
                serde_json::Value::String(String::new()),
            );
        }
    }
    if object.contains_key("cleanup_base_url") && !object.contains_key("cleanup_api_key") {
        let next_url = merged_object
            .get("cleanup_base_url")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if crate::llm::chat_host_changed(&current.cleanup_base_url, next_url) {
            merged_object.insert(
                "cleanup_api_key".into(),
                serde_json::Value::String(String::new()),
            );
        }
    }
    if let Some(incoming) = object.get("provider_keys").or_else(|| object.get("provider_api_keys"))
    {
        let mut keys = current.provider_api_keys.clone();
        if let Some(map) = incoming.as_object() {
            for (id, value) in map {
                if let Some(secret) = value.as_str().filter(|item| !item.trim().is_empty()) {
                    keys.insert(id.clone(), secret.to_owned());
                } else if let Some(secret) = value.get("key").and_then(|item| item.as_str()) {
                    if !secret.trim().is_empty() {
                        keys.insert(id.clone(), secret.to_owned());
                    }
                }
            }
        }
        merged_object.insert(
            "provider_api_keys".into(),
            serde_json::to_value(keys).unwrap_or(serde_json::json!({})),
        );
        merged_object.remove("provider_keys");
    }
    let mut settings: store::Settings =
        serde_json::from_value(merged).map_err(|error| error.to_string())?;
    if let Some(groq) = settings.provider_api_keys.get("groq").cloned() {
        if !groq.trim().is_empty() {
            settings.api_key = groq;
        }
    }
    if let Some(custom) = settings.provider_api_keys.get("custom").cloned() {
        if !custom.trim().is_empty() {
            settings.asr_api_key = custom.clone();
            settings.cleanup_api_key = custom;
        }
    }
    apply_settings(app, &state, settings).await
}

#[tauri::command]
async fn remove_api_key(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<store::SettingsView, String> {
    let _guard = state.settings_gate.lock().await;
    keychain::set_api_key("")
        .map_err(|error| format!("failed to remove API key securely: {error}"))?;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let mut settings = lock_recover(&state.settings).clone();
    settings.api_key.clear();
    settings.onboarded = false;
    store::save_settings(&dir, &settings).map_err(|error| error.to_string())?;
    *lock_recover(&state.settings) = settings.clone();
    Ok(store::SettingsView::from(&settings))
}

#[tauri::command]
async fn remove_asr_api_key(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<store::SettingsView, String> {
    let _guard = state.settings_gate.lock().await;
    keychain::set_asr_api_key("")
        .map_err(|error| format!("failed to remove ASR API key securely: {error}"))?;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let mut settings = lock_recover(&state.settings).clone();
    settings.asr_api_key.clear();
    store::save_settings(&dir, &settings).map_err(|error| error.to_string())?;
    let asr_endpoint = settings.asr_endpoint();
    *lock_recover(&state.settings) = settings.clone();
    rebuild_asr_provider(&state, &asr_endpoint);
    Ok(store::SettingsView::from(&settings))
}

#[tauri::command]
async fn remove_cleanup_api_key(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<store::SettingsView, String> {
    let _guard = state.settings_gate.lock().await;
    keychain::set_cleanup_api_key("")
        .map_err(|error| format!("failed to remove cleanup API key securely: {error}"))?;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let mut settings = lock_recover(&state.settings).clone();
    settings.cleanup_api_key.clear();
    store::save_settings(&dir, &settings).map_err(|error| error.to_string())?;
    *lock_recover(&state.settings) = settings.clone();
    Ok(store::SettingsView::from(&settings))
}

#[tauri::command]
async fn remove_provider_key(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    provider: String,
) -> Result<store::SettingsView, String> {
    let _guard = state.settings_gate.lock().await;
    let parsed = crate::providers::EngineProvider::parse(&provider)
        .ok_or_else(|| format!("unknown provider: {provider}"))?;
    keychain::set_provider_api_key(parsed, "")
        .map_err(|error| format!("failed to remove provider key securely: {error}"))?;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let mut settings = lock_recover(&state.settings).clone();
    settings.provider_api_keys.remove(parsed.as_str());
    if parsed.is_groq() {
        settings.api_key.clear();
    }
    if parsed.is_custom() {
        settings.asr_api_key.clear();
        settings.cleanup_api_key.clear();
    }
    store::save_settings(&dir, &settings).map_err(|error| error.to_string())?;
    *lock_recover(&state.settings) = settings.clone();
    Ok(store::SettingsView::from(&settings))
}

fn apply_captured_activation_mode(settings: &mut store::Settings, captured: &str) {
    if !matches!(captured, "tap" | "double_tap" | "hybrid") {
        return;
    }
    let keep_hybrid = settings.activation_mode == "hybrid"
        && captured == "tap"
        && !crate::modifier_hotkey::is_modifier_only(&settings.hotkey);
    if !keep_hybrid {
        settings.activation_mode = captured.to_owned();
    }
}

fn clamp_double_tap_activation(settings: &mut store::Settings) {
    let modifier_only = crate::modifier_hotkey::is_modifier_only(&settings.hotkey);
    if modifier_only && settings.activation_mode != "double_tap" {
        log::warn!(
            "modifier-only hotkeys require double_tap activation; using double_tap semantics"
        );
        settings.activation_mode = "double_tap".into();
    } else if !modifier_only && settings.activation_mode == "double_tap" {
        log::warn!(
            "double_tap activation is only supported for modifier-only hotkeys; using tap semantics"
        );
        settings.activation_mode = "tap".into();
    } else if !modifier_only && settings.activation_mode == "hold" {
        settings.activation_mode = "hybrid".into();
    }
}
#[tauri::command]
fn get_usage(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<store::Usage, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    store::get_usage(&dir, state.gate.snapshots()).map_err(|e| e.to_string())
}
#[tauri::command]
fn get_latency_metrics(state: State<'_, AppState>) -> metrics::LatencyMetrics {
    state.metrics.snapshot()
}
#[tauri::command]
fn clear_all_data(app: tauri::AppHandle) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    store::clear_all_data(&dir).map_err(|e| e.to_string())
}
#[tauri::command]
async fn probe_engine_draft(
    state: State<'_, AppState>,
    draft: engine::EngineDraft,
) -> Result<engine::ProbeResult, String> {
    let stored = lock_recover(&state.settings).clone();
    Ok(engine::probe_engine_draft(&draft, &stored).await)
}

#[tauri::command]
async fn validate_api_key(key: String) -> Result<String, String> {
    Ok(groq::validate_key(&key).await)
}
#[tauri::command]
async fn validate_configured_api_key() -> Result<String, String> {
    let key = keychain::get_api_key().ok_or_else(|| "No API key is configured".to_owned())?;
    Ok(groq::validate_key(&key).await)
}
#[tauri::command]
fn check_permissions() -> permissions::PermissionStatus {
    permissions::check()
}
#[tauri::command]
fn get_audio_input_devices() -> Result<Vec<audio::InputDeviceInfo>, String> {
    audio::list_input_devices()
}
#[tauri::command]
fn get_audio_input_device() -> Result<String, String> {
    audio::default_input_device_name()
}
#[tauri::command]
#[cfg(target_os = "macos")]
async fn request_microphone_permission() -> Result<bool, String> {
    match permissions::check().microphone_status.as_str() {
        "authorized" => Ok(true),
        "denied" | "restricted" => Ok(false),
        "not_determined" => {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            permissions::request_microphone(sender)?;
            receiver
                .await
                .map_err(|_| "microphone permission request was cancelled".to_owned())
        }
        _ => Err("microphone permission status could not be determined".into()),
    }
}
#[tauri::command]
#[cfg(not(target_os = "macos"))]
async fn request_microphone_permission() -> Result<bool, String> {
    Ok(true)
}
#[tauri::command]
fn open_privacy_settings(pane: String) -> Result<(), String> {
    permissions::open_privacy_settings(&pane)
}
#[tauri::command]
fn request_accessibility_permission() -> bool {
    permissions::request_accessibility()
}
fn chrono_like_id() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn recover_spool_into_history(
    app: &tauri::AppHandle,
    keep_audio_days: u64,
    keep_history_days: u64,
) {
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    match store::recover_spool(&dir, keep_audio_days) {
        Ok(recovered) => {
            for recovery in recovered {
                if let Err(error) = store::record_recovered_history(&dir, &recovery) {
                    log::warn!("failed to record recovered audio: {error}");
                }
            }
        }
        Err(error) => log::warn!("spool recovery failed: {error}"),
    }
    if let Err(error) = store::purge_history(&dir, keep_history_days) {
        log::warn!("history retention cleanup failed: {error}");
    }
    if let Err(error) = store::purge_gold_audio(&dir, keep_audio_days) {
        log::warn!("gold audio retention cleanup failed: {error}");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init());
    #[cfg(target_os = "macos")]
    {
        builder = builder.plugin(tauri_nspanel::init());
    }
    builder
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(|app| {
            let instance_lock = instance::acquire_or_exit();
            let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
            let (settings, migrated) = store::load_settings(&dir);
            // Persist key migrations and other startup normalization (for
            // example, clearing stale onboarded=true when no key is readable).
            if migrated {
                let _ = store::save_settings(&dir, &settings);
            }
            recover_spool_into_history(
                app.handle(),
                settings.keep_audio_days,
                settings.keep_history_days,
            );
            app.manage(AppState {
                manager: Mutex::new(DictationManager::new()),
                recorder: Arc::new(Mutex::new(Box::new(audio::Recorder::new()))),
                prefetch_asr: Mutex::new(None),
                selected_action: Mutex::new(None),
                selected_preview: Mutex::new(None),
                undo: Mutex::new(None),
                operation_lease: Mutex::new(OperationLease::Idle),
                asr_provider: Mutex::new(Arc::new(asr::GroqAsrProvider::from_resolved_endpoint(
                    settings.asr_endpoint(),
                ))),
                settings: Mutex::new(settings.clone()),
                context: Mutex::new(context::ContextState::new_with_modes(
                    settings.context_enabled,
                    settings.browser_access_enabled,
                    settings.context_mappings.clone(),
                    settings.writing_modes.clone(),
                )),
                gate: Arc::new(queue::RequestGate::new(Some(app.handle().clone()))),
                metrics: metrics::Metrics::default(),
                hotkey_gate: tokio::sync::Mutex::new(()),
                settings_gate: tokio::sync::Mutex::new(()),
                pending_recorder_cancel: Mutex::new(None),
                onboarding_test_mode: Mutex::new(false),
                onboarding_selected_text: Mutex::new(None),
                _instance_lock: instance_lock,
            });
            let context_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // AX/System Events probing is intentionally conservative: the
                // recording and paste paths perform their own fresh checks,
                // while the background preview does not need sub-second churn.
                // Accessibility + browser AppleScript probes are intentionally
                // throttled while idle. Native apps use the cheap NSWorkspace
                // key to avoid repeated probes; an authorized browser is
                // refreshed even when its PID/bundle stays the same so a Tab
                // switch updates the preview. Recording start still performs a
                // fresh target capture, so this preview optimization does not
                // weaken delivery safety or accuracy.
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
                let mut last_application = None;
                let mut last_accessibility = None;
                loop {
                    interval.tick().await;
                    let application = context::frontmost_application_key();
                    let state = context_handle.state::<AppState>();
                    let browser_access_enabled = state
                        .context
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .browser_access_enabled;
                    let accessibility = permissions::accessibility_is_trusted();
                    let same_application = last_application.as_ref() == Some(&application);
                    let browser_refresh = same_application
                        && context::is_browser_application(application.1.as_deref())
                        && browser_access_enabled;
                    if !should_refresh_context_preview(
                        same_application,
                        browser_refresh,
                        last_accessibility,
                        accessibility,
                    ) {
                        continue;
                    }
                    last_application = Some(application);
                    last_accessibility = Some(accessibility);
                    refresh_context_snapshot(&context_handle, &state).await;
                }
            });
            if let Some(window) = app.get_webview_window("island") {
                island_window::ensure_panel(&window);
                let placement = notch::placement_for_cursor_screen(app.handle());
                let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
                    x: placement.x as i32,
                    y: placement.y as i32,
                }));
                let _ = window.set_size(tauri::Size::Physical(tauri::PhysicalSize {
                    width: placement.width as u32,
                    height: placement.height as u32,
                }));
            }
            let open_settings = tauri::menu::MenuItem::with_id(
                app,
                "open-settings",
                "打开设置",
                true,
                None::<&str>,
            )?;
            let quit =
                tauri::menu::MenuItem::with_id(app, "quit", "退出 VoiceFlow", true, None::<&str>)?;
            let tray_menu = tauri::menu::Menu::with_items(app, &[&open_settings, &quit])?;
            let tray_icon =
                tauri::image::Image::from_bytes(include_bytes!("../icons/tray-icon.png"))?;
            let tray = tauri::tray::TrayIconBuilder::with_id("voiceflow-status")
                .menu(&tray_menu)
                .icon(tray_icon)
                .icon_as_template(true)
                .tooltip("VoiceFlow")
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open-settings" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                });
            tray.build(app)?;
            if let Some(tray) = app.tray_by_id("voiceflow-status") {
                tray.set_visible(settings.show_tray_icon)?;
            }
            // Escape is NOT registered here at startup. It is only registered
            // while a recording is active (see start_internal), so it is never
            // swallowed when the user is typing in other apps.
            let h = app.handle().clone();
            app.listen("hotkey://cancel", move |_| {
                let h = h.clone();
                tauri::async_runtime::spawn(async move {
                    let state = h.state::<AppState>();
                    dictation::cancel_internal(&h, &state).await;
                });
            });
            let h = app.handle().clone();
            app.listen("hotkey://toggle", move |_| {
                let h = h.clone();
                tauri::async_runtime::spawn(async move {
                    let state = h.state::<AppState>();
                    dictation::handle_hotkey_toggle(&h, &state).await;
                });
            });
            let h = app.handle().clone();
            app.listen("hotkey://press", move |_| {
                let h = h.clone();
                tauri::async_runtime::spawn(async move {
                    let state = h.state::<AppState>();
                    dictation::handle_hotkey_press(&h, &state).await;
                });
            });
            let h = app.handle().clone();
            app.listen("hotkey://release", move |_| {
                let h = h.clone();
                tauri::async_runtime::spawn(async move {
                    let state = h.state::<AppState>();
                    dictation::handle_hotkey_release(&h, &state).await;
                });
            });
            let h = app.handle().clone();
            app.listen("hotkey://double_tap", move |_| {
                let h = h.clone();
                tauri::async_runtime::spawn(async move {
                    let state = h.state::<AppState>();
                    dictation::handle_double_tap_toggle(&h, &state, true).await;
                });
            });
            let h = app.handle().clone();
            app.listen("hotkey://selected-action", move |_| {
                let h = h.clone();
                tauri::async_runtime::spawn(async move {
                    let state = h.state::<AppState>();
                    selected_action::handle_selected_action_hotkey(&h, &state).await;
                });
            });
            let h = app.handle().clone();
            app.listen("audio://error", move |event| {
                let h = h.clone();
                let message = event.payload().to_owned();
                tauri::async_runtime::spawn(async move {
                    let state = h.state::<AppState>();
                    handle_audio_error(&h, &state, message).await;
                });
            });
            let h = app.handle().clone();
            app.listen("audio://limit", move |_| {
                let h = h.clone();
                tauri::async_runtime::spawn(async move {
                    let state = h.state::<AppState>();
                    handle_audio_limit(&h, &state).await;
                });
            });
            // Register after listeners are installed so a startup failure is
            // observable by the UI instead of being emitted into the void.
            let h = app.handle().clone();
            let hotkey = settings.hotkey.clone();
            let mode = settings.activation_mode.clone();
            if let Err(error) = hotkey::register(&h, &hotkey, &mode) {
                log::warn!("hotkey registration failed: {error}");
                let _ = h.emit("dictation://error", error);
            }
            let selected_hotkey = settings.selected_action_hotkey.clone();
            let selected_enabled = settings.selected_actions_enabled;
            let selected_handle = h.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = hotkey::apply_selected_action_hotkey(
                    &selected_handle,
                    &selected_hotkey,
                    selected_enabled,
                )
                .await
                {
                    log::warn!("selected action hotkey registration failed: {error}");
                    let _ = selected_handle.emit("dictation://error", error);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            dictation::start_dictation,
            dictation::stop_dictation,
            dictation::cancel_dictation,
            get_settings,
            dictionary_learn::suggest_dictionary_entries,
            dictionary_learn::add_dictionary_entries,
            dictionary_learn::remove_dictionary_word,
            dictionary_learn::list_learn_pairs,
            dictionary_learn::promote_learn_pair,
            dictionary_learn::ignore_learn_pair,
            dictionary_learn::undo_learn_pair,
            dictionary_learn::pin_dictionary_term,
            dictionary_learn::list_pinned_terms,
            dictionary_learn::list_style_drafts,
            dictionary_learn::confirm_style_draft,
            dictionary_learn::dismiss_style_draft,
            read_dictionary_file,
            get_context_snapshot,
            get_context_mappings,
            get_available_applications,
            get_application_from_path,
            get_context_override,
            save_context_mapping,
            delete_context_mapping,
            set_context_enabled,
            set_context_override,
            request_browser_access,
            set_settings,
            update_settings_patch,
            remove_api_key,
            remove_asr_api_key,
            remove_cleanup_api_key,
            remove_provider_key,
            get_usage,
            get_latency_metrics,
            history_commands::get_history,
            history_commands::export_history,
            history_commands::export_gold_corpus,
            history_commands::get_history_audio,
            history_commands::save_verbatim,
            clear_all_data,
            history_commands::retry_dictation,
            probe_engine_draft,
            validate_api_key,
            validate_configured_api_key,
            history_commands::repaste_history,
            history_commands::save_history_revision,
            history_commands::get_history_revisions,
            history_commands::reclean_history,
            undo_last_delivery,
            selected_action::confirm_selected_action_preview,
            selected_action::copy_selected_action_preview,
            selected_action::cancel_selected_action_preview,
            history_commands::delete_history,
            check_permissions,
            get_audio_input_devices,
            get_audio_input_device,
            request_microphone_permission,
            open_privacy_settings,
            request_accessibility_permission,
            hide_island_if_idle,
            set_island_learn_interactive,
            set_hotkeys_suspended,
            set_onboarding_test_mode,
            set_onboarding_selected_text,
            notch::island_placement
        ])
        .build(tauri::generate_context!())
        .expect("error while building VoiceFlow")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            }

            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}

#[cfg(test)]
mod tests {
    use super::{
        claim_processing_timeout, completion_state, completion_state_for_delivery, context,
        hud_accepts_mouse,
        delivery_fallback_reason, error_completion_is_current, error_fallback_reason,
        finalize_text, long_completion_state, process_bounded_chunk_jobs,
        processing_completion_is_current,
        processing_watchdog_delay, read_dictionary_file_contents, should_chunk_recording,
        stop_transition_is_current, target_guard_mismatch_with_retry, undo_available_for_hud,
        undo_preflight,
        CleanupDecision, DictationManager, OperationLease, Phase, UndoTransaction,
        MAX_DICTIONARY_FILE_BYTES,
    };
    use super::dictation::{
        next_toggle_action, reset_starting_manager, take_gesture_lock, ToggleAction, GESTURE_LOCK_MS,
    };
    use crate::store;
    use futures_util::StreamExt;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Instant;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio_util::sync::CancellationToken;

    #[test]
    fn short_hotkey_recordings_use_the_short_path() {
        assert!(!should_chunk_recording(3, 25));
        assert!(should_chunk_recording(25, 25));
        assert!(should_chunk_recording(600, 3_600));
        assert!(!should_chunk_recording(599, 3_600));
    }

    #[tokio::test]
    async fn long_chunk_jobs_never_exceed_the_two_item_window() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let jobs = process_bounded_chunk_jobs(0..6, {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            move |index| {
                let active = Arc::clone(&active);
                let peak = Arc::clone(&peak);
                async move {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(current, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    index
                }
            }
        });

        let completed = jobs.collect::<Vec<_>>().await;

        assert_eq!(completed.len(), 6);
        assert_eq!(peak.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn dictionary_file_reader_validates_type_size_and_encoding() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("voiceflow-dictionary-{stamp}"));
        let valid_path = root.with_extension("csv");
        let invalid_encoding_path = root.with_extension("txt");
        let oversized_path = root.with_extension("tsv");

        fs::write(&valid_path, "term\nVoiceFlow\n").unwrap();
        assert_eq!(
            read_dictionary_file_contents(&valid_path).unwrap(),
            "term\nVoiceFlow\n"
        );

        fs::write(&invalid_encoding_path, [0xff, 0xfe]).unwrap();
        assert_eq!(
            read_dictionary_file_contents(&invalid_encoding_path).unwrap_err(),
            "词典文件必须使用 UTF-8 编码"
        );

        fs::write(
            &oversized_path,
            vec![b'x'; MAX_DICTIONARY_FILE_BYTES as usize + 1],
        )
        .unwrap();
        assert_eq!(
            read_dictionary_file_contents(&oversized_path).unwrap_err(),
            "词典文件不能超过 1 MB"
        );

        let unsupported_path = root.with_extension("json");
        fs::write(&unsupported_path, "[]").unwrap();
        assert_eq!(
            read_dictionary_file_contents(&unsupported_path).unwrap_err(),
            "只支持 CSV、TXT 或 TSV 文件"
        );

        for path in [
            valid_path,
            invalid_encoding_path,
            oversized_path,
            unsupported_path,
        ] {
            let _ = fs::remove_file(path);
        }
    }

    #[test]
    fn hud_caption_expands_window_for_delivery_failures() {
        assert!(super::hud_caption_expands_window("copied", Some("paste_failed")));
        assert!(super::hud_caption_expands_window("degraded", Some("target_changed")));
        assert!(super::hud_caption_expands_window("error", None));
        assert!(super::hud_caption_expands_window("copied", None));
        assert!(!super::hud_caption_expands_window("recording", None));
        assert!(!super::hud_caption_expands_window("idle", None));
    }

    #[test]
    fn hud_partial_never_expands_the_window() {
        assert!(!super::hud_partial_expands_window(
            "你好世界",
            Phase::Recording,
            4,
            4
        ));
        assert!(!super::hud_partial_expands_window(
            "hello",
            Phase::Processing,
            4,
            4
        ));
        assert!(!super::hud_partial_expands_window(
            "你好世界",
            Phase::Idle,
            4,
            4
        ));
        assert!(!super::hud_partial_expands_window("", Phase::Recording, 4, 4));
    }

    #[test]
    fn context_preview_refreshes_when_accessibility_changes() {
        assert!(!super::should_refresh_context_preview(
            true,
            false,
            Some(false),
            false
        ));
        assert!(super::should_refresh_context_preview(
            true,
            false,
            Some(false),
            true
        ));
        assert!(super::should_refresh_context_preview(
            true,
            true,
            Some(true),
            true
        ));
        assert!(super::should_refresh_context_preview(
            false,
            false,
            Some(true),
            true
        ));
    }

    #[test]
    fn cancelled_stop_cannot_claim_processing() {
        assert!(stop_transition_is_current(Phase::Stopping, 4, 4, false));
        assert!(!stop_transition_is_current(Phase::Stopping, 5, 4, false));
        assert!(!stop_transition_is_current(Phase::Stopping, 4, 4, true));
        assert!(!stop_transition_is_current(Phase::Idle, 4, 4, false));
    }

    #[test]
    fn toggle_actions_cancel_starting_and_ignore_stopping() {
        assert_eq!(next_toggle_action(Phase::Idle), ToggleAction::Start);
        assert_eq!(
            next_toggle_action(Phase::Starting),
            ToggleAction::Cancel
        );
        assert_eq!(next_toggle_action(Phase::Recording), ToggleAction::Stop);
        assert_eq!(next_toggle_action(Phase::Stopping), ToggleAction::Ignore);
        assert_eq!(
            next_toggle_action(Phase::Processing),
            ToggleAction::Cancel
        );
    }

    #[test]
    fn starting_cancellation_claim_does_not_wait_for_audio_setup() {
        let cancellation = CancellationToken::new();
        let mut manager = DictationManager {
            phase: Phase::Starting,
            started: Instant::now(),
            gesture_lock: None,
            session_generation: 11,
            cancellation: cancellation.clone(),
            recording_context: None,
            ..DictationManager::new()
        };

        assert_eq!(
            super::dictation::claim_cancel_manager(&mut manager, false),
            Phase::Starting
        );
        assert_eq!(manager.phase, Phase::Idle);
        assert_eq!(manager.session_generation, 12);
        assert!(cancellation.is_cancelled());
    }

    #[test]
    fn production_lock_helper_recovers_poisoned_state() {
        let value = std::sync::Arc::new(std::sync::Mutex::new(9));
        let poisoned = value.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.lock().expect("initial lock");
            panic!("poison application state");
        })
        .join();

        assert_eq!(*super::lock_recover(&value), 9);
    }

    #[test]
    fn cancelled_processing_cannot_claim_success_completion() {
        assert!(processing_completion_is_current(
            Phase::Processing,
            4,
            4,
            false
        ));
        assert!(!processing_completion_is_current(
            Phase::Processing,
            5,
            4,
            false
        ));
        assert!(!processing_completion_is_current(
            Phase::Processing,
            4,
            4,
            true
        ));
        assert!(!processing_completion_is_current(Phase::Idle, 4, 4, false));
    }

    #[test]
    fn processing_watchdog_is_bounded_and_scales_for_long_recordings() {
        assert_eq!(processing_watchdog_delay(3, false).as_secs(), 300);
        assert_eq!(processing_watchdog_delay(600, true).as_secs(), 1_380);
        assert_eq!(processing_watchdog_delay(3_600, true).as_secs(), 1_800);
    }

    #[test]
    fn processing_timeout_claim_returns_manager_to_idle_once() {
        let cancellation = CancellationToken::new();
        let mut manager = DictationManager {
            phase: Phase::Processing,
            started: Instant::now(),
            gesture_lock: None,
            session_generation: 7,
            cancellation: cancellation.clone(),
            recording_context: Some(context::ContextSnapshot::general()),
            ..DictationManager::new()
        };

        assert_eq!(claim_processing_timeout(&mut manager, 7), Some(8));
        assert_eq!(manager.phase, Phase::Idle);
        assert_eq!(manager.session_generation, 8);
        assert!(manager.recording_context.is_none());
        assert!(cancellation.is_cancelled());
        assert_eq!(claim_processing_timeout(&mut manager, 7), None);
    }

    #[test]
    fn failed_start_resets_to_idle_and_advances_generation() {
        let cancellation = CancellationToken::new();
        let mut manager = DictationManager {
            phase: Phase::Starting,
            started: Instant::now(),
            gesture_lock: None,
            session_generation: 7,
            cancellation: cancellation.clone(),
            recording_context: Some(context::ContextSnapshot::general()),
            ..DictationManager::new()
        };

        let failure_generation = reset_starting_manager(&mut manager);

        assert_eq!(failure_generation, 8);
        assert_eq!(manager.phase, Phase::Idle);
        assert_eq!(manager.session_generation, 8);
        assert!(manager.recording_context.is_none());
        assert!(cancellation.is_cancelled());

        // A completion retry must not invalidate the generation again.
        assert_eq!(reset_starting_manager(&mut manager), 8);
    }

    #[test]
    fn startup_error_completion_accepts_failed_generation_but_rejects_stale_one() {
        assert!(error_completion_is_current(Phase::Idle, 8, 8, false));
        assert!(!error_completion_is_current(Phase::Idle, 8, 7, false));
    }

    #[test]
    fn stale_error_cannot_claim_a_new_generation() {
        assert!(error_completion_is_current(Phase::Processing, 4, 4, false));
        assert!(error_completion_is_current(Phase::Idle, 4, 4, false));
        assert!(!error_completion_is_current(Phase::Processing, 5, 4, false));
        assert!(!error_completion_is_current(Phase::Starting, 4, 4, false));
        assert!(!error_completion_is_current(Phase::Recording, 4, 4, false));
        assert!(!error_completion_is_current(Phase::Stopping, 4, 4, false));
        assert!(!error_completion_is_current(Phase::Processing, 4, 4, true));
    }

    #[test]
    fn degraded_long_recording_never_reports_success() {
        assert_eq!(
            long_completion_state(true, "paste_unverified", false),
            "degraded"
        );
        assert_eq!(long_completion_state(true, "clipboard", true), "degraded");
        assert_eq!(long_completion_state(false, "clipboard", true), "copied");
        assert_eq!(long_completion_state(false, "paste", false), "done");
    }

    #[test]
    fn hud_stays_click_through_during_processing_and_paste_yield() {
        assert!(!hud_accepts_mouse(false, false));
        assert!(!hud_accepts_mouse(true, false));
        assert!(!hud_accepts_mouse(true, true));
        assert!(hud_accepts_mouse(false, true));
    }

    #[test]
    fn completion_state_prioritizes_degraded_over_delivery_method() {
        assert_eq!(completion_state(true, true), "degraded");
        assert_eq!(completion_state(true, false), "degraded");
        assert_eq!(completion_state(false, true), "done");
        assert_eq!(completion_state(false, false), "copied");
        assert_eq!(
            completion_state_for_delivery(false, "paste_unverified"),
            "unverified"
        );
        assert_eq!(completion_state_for_delivery(false, "paste"), "done");
        assert_eq!(completion_state_for_delivery(false, "history"), "history");
    }

    #[test]
    fn operation_lease_serializes_live_and_history_operations() {
        let mut lease = OperationLease::Idle;
        assert!(super::dictation::claim_operation(
            &mut lease,
            OperationLease::LiveDictation
        ));
        assert!(!super::dictation::claim_operation(
            &mut lease,
            OperationLease::HistoryReclean
        ));
        super::release_operation_lease(&mut lease, OperationLease::HistoryReclean);
        assert_eq!(lease, OperationLease::LiveDictation);
        super::release_operation_lease(&mut lease, OperationLease::LiveDictation);
        assert_eq!(lease, OperationLease::Idle);

        assert!(super::dictation::claim_operation(
            &mut lease,
            OperationLease::HistoryReclean
        ));
        assert!(!super::dictation::claim_operation(
            &mut lease,
            OperationLease::LiveDictation
        ));
        super::release_operation_lease(&mut lease, OperationLease::HistoryReclean);
        assert_eq!(lease, OperationLease::Idle);
    }

    #[test]
    fn selected_action_entry_cannot_claim_during_normal_dictation() {
        for (phase, initial_lease) in [
            (Phase::Starting, OperationLease::Idle),
            (Phase::Recording, OperationLease::Idle),
            (Phase::Idle, OperationLease::LiveDictation),
        ] {
            let mut manager = DictationManager {
                phase,
                started: Instant::now(),
                gesture_lock: None,
                session_generation: 3,
                cancellation: CancellationToken::new(),
                recording_context: None,
                ..DictationManager::new()
            };
            let mut lease = initial_lease;

            assert_eq!(
                super::dictation::claim_start_manager(&mut manager, &mut lease),
                None
            );
            assert_eq!(manager.phase, phase);
            assert_eq!(manager.session_generation, 3);
            assert_eq!(lease, initial_lease);
        }
    }

    #[test]
    fn abort_from_processing_releases_live_dictation_lease() {
        let cancellation = CancellationToken::new();
        let mut manager = DictationManager {
            phase: Phase::Processing,
            started: Instant::now(),
            gesture_lock: None,
            session_generation: 4,
            cancellation: cancellation.clone(),
            recording_context: Some(context::ContextSnapshot::general()),
            ..DictationManager::new()
        };
        let mut lease = OperationLease::LiveDictation;

        assert!(super::apply_processing_abort(&mut manager, &mut lease));
        assert_eq!(manager.phase, Phase::Idle);
        assert_eq!(manager.session_generation, 5);
        assert!(manager.recording_context.is_none());
        assert!(cancellation.is_cancelled());
        assert_eq!(lease, OperationLease::Idle);
        assert!(super::dictation::claim_operation(
            &mut lease,
            OperationLease::LiveDictation
        ));
    }

    #[test]
    fn abort_from_idle_does_not_clear_an_unrelated_lease() {
        let mut manager = DictationManager {
            phase: Phase::Idle,
            started: Instant::now(),
            gesture_lock: None,
            session_generation: 2,
            cancellation: CancellationToken::new(),
            recording_context: None,
            ..DictationManager::new()
        };
        let mut lease = OperationLease::HistoryReclean;
        assert!(!super::apply_processing_abort(&mut manager, &mut lease));
        assert_eq!(lease, OperationLease::HistoryReclean);
        assert_eq!(manager.session_generation, 2);
    }

    fn sample_preview(generation: u64) -> super::SelectedActionPreview {
        super::SelectedActionPreview {
            session: super::SelectedActionSession {
                selected_text: "hello".into(),
                selection_fingerprint: 1,
                target_guard: context::TargetAppGuard {
                    pid: 1,
                    bundle_id: Some("com.example.editor".into()),
                    browser_host: None,
                    browser_target_token: None,
                    window_token: None,
                    window_id: None,
                    input_token: None,
                    secure_input: false,
                },
                onboarding_trial: false,
            },
            session_generation: generation,
            context: context::ContextSnapshot::general(),
        }
    }

    #[test]
    fn selected_preview_take_keeps_the_preview_when_the_lease_is_gone() {
        let mut preview = Some(sample_preview(4));
        let result =
            super::selected_action::take_preview_if_current(&mut preview, 4, OperationLease::Idle);
        assert_eq!(result.unwrap_err(), "Selected-text preview is stale");
        assert!(preview.is_some());
    }

    #[test]
    fn selected_preview_take_keeps_the_preview_when_generation_moved() {
        let mut preview = Some(sample_preview(4));
        let result =
            super::selected_action::take_preview_if_current(
                &mut preview,
                5,
                OperationLease::LiveDictation,
            );
        assert_eq!(result.unwrap_err(), "Selected-text preview is stale");
        assert!(preview.is_some());
    }

    #[test]
    fn selected_preview_take_removes_only_a_current_lease() {
        let mut preview = Some(sample_preview(4));
        let taken =
            super::selected_action::take_preview_if_current(
                &mut preview,
                4,
                OperationLease::LiveDictation,
            )
            .unwrap();
        assert_eq!(taken.session_generation, 4);
        assert!(preview.is_none());
    }

    #[test]
    fn selected_preview_completion_requires_idle_matching_generation_and_lease() {
        assert!(super::selected_preview_completion_is_current(
            Phase::Idle,
            4,
            4,
            OperationLease::LiveDictation
        ));
        assert!(!super::selected_preview_completion_is_current(
            Phase::Recording,
            4,
            4,
            OperationLease::LiveDictation
        ));
        assert!(!super::selected_preview_completion_is_current(
            Phase::Idle,
            5,
            4,
            OperationLease::LiveDictation
        ));
        assert!(!super::selected_preview_completion_is_current(
            Phase::Idle,
            4,
            4,
            OperationLease::Idle
        ));
    }

    #[test]
    fn selected_preview_cancel_invalidates_an_in_flight_confirm() {
        assert!(super::selected_action::should_invalidate_selected_preview(
            false,
            OperationLease::LiveDictation,
            Phase::Idle
        ));
        assert!(super::selected_action::should_invalidate_selected_preview(
            true,
            OperationLease::Idle,
            Phase::Idle
        ));
        assert!(!super::selected_action::should_invalidate_selected_preview(
            false,
            OperationLease::Idle,
            Phase::Idle
        ));
        assert!(!super::selected_action::should_invalidate_selected_preview(
            false,
            OperationLease::LiveDictation,
            Phase::Recording
        ));
    }

    #[test]
    fn long_chunk_progress_includes_session_generation() {
        let payload = super::long_chunk_progress_payload(9, 12, 2, 4, 0.46);
        assert_eq!(payload["session_generation"], 9);
        assert_eq!(payload["chunks_done"], 2);
        assert_eq!(payload["total"], 4);
        assert!((payload["progress"].as_f64().unwrap() - 0.46).abs() < 1e-6);
    }

    fn sample_undo_transaction(now: Instant) -> UndoTransaction {
        UndoTransaction {
            session_generation: 4,
            created_at: now,
            expires_at: now + std::time::Duration::from_secs(3),
            target_guard: context::TargetAppGuard {
                pid: 1,
                bundle_id: Some("com.example.editor".into()),
                browser_host: None,
                browser_target_token: None,
                window_token: None,
                window_id: None,
                input_token: None,
                secure_input: false,
            },
            delivery_method: "paste".into(),
            post_insert_input_fingerprint: 1,
            consumed: false,
        }
    }

    #[test]
    fn undo_available_matches_armed_unexpired_transaction_not_paste_method() {
        let now = Instant::now();
        let armed = sample_undo_transaction(now);

        assert!(undo_available_for_hud("done", Some(&armed), 4, now));
        assert!(undo_available_for_hud("degraded", Some(&armed), 4, now));
        // Verified AX reports method "paste" / state "done" but never arms undo.
        assert!(!undo_available_for_hud("done", None, 4, now));
        assert!(!undo_available_for_hud("degraded", None, 4, now));
        assert!(!undo_available_for_hud("unverified", Some(&armed), 4, now));
        assert!(!undo_available_for_hud("copied", Some(&armed), 4, now));
        assert!(!undo_available_for_hud(
            "done",
            Some(&armed),
            4,
            now + std::time::Duration::from_secs(3)
        ));
        assert!(!undo_available_for_hud("done", Some(&armed), 5, now));
        let consumed = UndoTransaction {
            consumed: true,
            ..armed.clone()
        };
        assert!(!undo_available_for_hud("done", Some(&consumed), 4, now));
    }

    #[test]
    fn undo_preflight_rejects_expired_stale_and_consumed_transactions() {
        let now = Instant::now();
        let transaction = sample_undo_transaction(now);
        assert_eq!(undo_preflight(&transaction, 4, now), "available");
        assert_eq!(
            undo_preflight(&transaction, 4, now + std::time::Duration::from_secs(3)),
            "expired"
        );
        assert_eq!(undo_preflight(&transaction, 5, now), "stale_target");

        let consumed = UndoTransaction {
            consumed: true,
            ..transaction
        };
        assert_eq!(undo_preflight(&consumed, 4, now), "already_consumed");
    }

    #[test]
    fn cleanup_failure_preserves_raw_transcript_and_marks_degraded() {
        let result =
            finalize_text(
                "uh deploy v2 /Users/mingjie/app",
                CleanupDecision::Failed,
                context::ContextFamily::PromptOrCode,
                &[],
                &[],
            )
            .unwrap();
        assert_eq!(result.text, "deploy v2 /Users/mingjie/app");
        assert!(result.degraded);
        assert_eq!(result.degraded_reason, Some("llm_cleanup_failed"));
    }

    #[test]
    fn disabled_cleanup_never_drops_a_filler_only_transcript() {
        let result = finalize_text(
            "嗯 uh",
            CleanupDecision::Disabled,
            context::ContextFamily::PersonalChat,
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(result.text, "嗯 uh");
        assert!(!result.degraded);
    }

    #[test]
    fn provider_cleanup_is_used_when_non_empty() {
        let result = finalize_text(
            "uh hello",
            CleanupDecision::Provider("hello".into()),
            context::ContextFamily::General,
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(result.text, "hello");
        assert!(!result.degraded);
    }

    #[test]
    fn provider_cleanup_cannot_reintroduce_a_promoted_before() {
        let pair = store::LearnPairRecord {
            pair_key: crate::dictionary_learn::pair_key("知呼", "知乎"),
            before_surface: "知呼".into(),
            after_surface: "知乎".into(),
            hits: 3,
            promoted: true,
            last_at: "2026-01-01".into(),
            family: Some("personal_chat".into()),
            mapping_id: None,
            browser_host: None,
            native_bundle: None,
            last_used_at: None,
            pinned: false,
            tombstoned_at: None,
            ignored: false,
            promote_hits: 0,
        };
        let result = finalize_text(
            "今天去知乎看看",
            CleanupDecision::Provider("今天去知呼看看".into()),
            context::ContextFamily::PersonalChat,
            &[pair],
            &["知乎".into()],
        )
        .unwrap();
        assert_eq!(result.text, "今天去知乎看看");
        assert!(!result.degraded);
    }

    #[test]
    fn empty_provider_cleanup_falls_back_to_raw_and_is_degraded() {
        let result = finalize_text(
            "uh hello",
            CleanupDecision::Provider("  ".into()),
            context::ContextFamily::General,
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(result.text, "hello");
        assert!(result.degraded);
        assert_eq!(result.degraded_reason, Some("llm_cleanup_empty"));
    }

    #[test]
    fn delivery_fallback_is_fail_closed() {
        assert_eq!(
            delivery_fallback_reason(false, "paste failed"),
            "target_changed"
        );
        assert_eq!(
            delivery_fallback_reason(true, "active target changed before paste"),
            "target_changed"
        );
        assert_eq!(
            delivery_fallback_reason(true, "Accessibility permission is required"),
            "accessibility_required"
        );
        assert_eq!(
            delivery_fallback_reason(true, "browser access is required to confirm the active tab"),
            "browser_permission_required"
        );
        assert_eq!(
            delivery_fallback_reason(true, "focused input is not available"),
            "input_unavailable"
        );
    }

    #[test]
    fn transient_target_probe_failure_is_retried_but_real_change_is_not() {
        let expected = context::TargetAppGuard {
            pid: 42,
            bundle_id: Some("com.example.editor".into()),
            browser_host: None,
            browser_target_token: None,
            window_token: Some(1),
            window_id: Some(2),
            input_token: Some(3),
            secure_input: false,
        };
        let unavailable = context::TargetAppGuard {
            pid: 0,
            bundle_id: None,
            ..expected.clone()
        };
        let mut probes = vec![unavailable, expected.clone()].into_iter();
        assert_eq!(
            target_guard_mismatch_with_retry(&expected, || probes.next().unwrap()),
            None
        );

        let changed = context::TargetAppGuard {
            pid: 43,
            ..expected.clone()
        };
        assert_eq!(
            target_guard_mismatch_with_retry(&expected, || changed.clone()),
            Some("target_changed")
        );
    }

    #[test]
    fn onboarding_delivery_follows_test_mode_regardless_of_frontmost_app() {
        let mut snapshot = context::ContextSnapshot::general();
        snapshot.target_guard.pid = 42;
        snapshot.target_guard.bundle_id = Some("com.todesktop.230313mzl4w4u92".into());

        assert!(super::onboarding_delivery_target_matches(
            true,
            &snapshot,
            (99, Some("com.todesktop.230313mzl4w4u92".into()))
        ));
        assert!(super::onboarding_delivery_target_matches(true, &snapshot, (1, None)));
        assert!(!super::onboarding_delivery_target_matches(
            false,
            &snapshot,
            (42, Some("com.voiceflow.desktop".into()))
        ));
    }

    #[test]
    fn completion_hud_dwells_long_enough_to_read_copied_and_error() {
        assert!(super::completion_hud_dwell_ms("copied") >= 4_000);
        assert!(super::completion_hud_dwell_ms("error") >= 4_000);
        assert_eq!(super::completion_hud_dwell_ms("done"), 3_000);
        assert_eq!(super::completion_hud_dwell_ms("degraded"), 3_000);
    }

    #[test]
    fn error_fallback_reason_does_not_label_every_error_as_microphone_failure() {
        assert_eq!(
            error_fallback_reason("Microphone permission is required"),
            "microphone_required"
        );
        assert_eq!(
            error_fallback_reason("A valid API key is required before dictation can start"),
            "api_key_required"
        );
        assert_eq!(error_fallback_reason("No speech detected"), "no_speech");
        assert_eq!(
            error_fallback_reason("Transcription took too long and was cancelled."),
            "processing_timeout"
        );
        assert_eq!(
            error_fallback_reason("provider request failed"),
            "dictation_error"
        );
    }

    #[test]
    fn repeated_gesture_is_ignored_until_the_lock_expires() {
        let mut manager = super::DictationManager {
            phase: Phase::Idle,
            started: std::time::Instant::now(),
            gesture_lock: None,
            session_generation: 0,
            cancellation: tokio_util::sync::CancellationToken::new(),
            recording_context: None,
            ..DictationManager::new()
        };
        assert!(take_gesture_lock(&mut manager));
        assert!(!take_gesture_lock(&mut manager));
        manager.gesture_lock = Some(
            std::time::Instant::now()
                - std::time::Duration::from_millis(GESTURE_LOCK_MS as u64 + 1),
        );
        assert!(take_gesture_lock(&mut manager));
    }

    #[test]
    fn stale_context_detection_cannot_replace_a_newer_snapshot() {
        let mut state = context::ContextState::new(true, false, Vec::new());
        state.snapshot.profile.id = "newer.profile".into();
        state.detection_generation = 2;

        let stale = context::ContextSnapshot::general();
        assert_eq!(super::commit_context_snapshot(&mut state, 1, &stale), None);
        assert_eq!(state.snapshot.profile.id, "newer.profile");
    }

    #[test]
    fn current_context_detection_replaces_snapshot_and_reports_change() {
        let mut state = context::ContextState::new(true, false, Vec::new());
        state.detection_generation = 3;

        let mut next = context::ContextSnapshot::general();
        next.profile.id = "current.profile".into();
        assert_eq!(
            super::commit_context_snapshot(&mut state, 3, &next),
            Some(true)
        );
        assert_eq!(state.snapshot.profile.id, "current.profile");
    }

    #[test]
    fn context_ipc_payload_excludes_target_identity() {
        let mut snapshot = context::ContextSnapshot::general();
        snapshot.target_guard = context::TargetAppGuard {
            pid: 1234,
            bundle_id: Some("com.example.private".into()),
            browser_host: Some("private.example".into()),
            browser_target_token: Some(1),
            window_token: Some(2),
            window_id: Some(3),
            input_token: Some(4),
            secure_input: false,
        };

        let payload = serde_json::to_string(&super::context_payload(&snapshot)).unwrap();
        assert!(!payload.contains("1234"));
        assert!(!payload.contains("private.example"));
        assert!(!payload.contains("window_token"));
        assert!(!payload.contains("input_token"));
    }

    #[test]
    fn modifier_only_hotkeys_cannot_be_left_with_dead_tap_semantics() {
        let mut settings = store::Settings {
            hotkey: "Shift".into(),
            activation_mode: "tap".into(),
            ..store::Settings::default()
        };
        super::clamp_double_tap_activation(&mut settings);
        assert_eq!(settings.activation_mode, "double_tap");

        let mut combo = store::Settings {
            hotkey: "CmdOrControl+Shift+Space".into(),
            activation_mode: "double_tap".into(),
            ..store::Settings::default()
        };
        super::clamp_double_tap_activation(&mut combo);
        assert_eq!(combo.activation_mode, "tap");
    }

    #[test]
    fn combo_hold_clamps_to_hybrid_and_hybrid_is_kept() {
        let mut hold = store::Settings {
            hotkey: "CmdOrControl+Shift+Space".into(),
            activation_mode: "hold".into(),
            ..store::Settings::default()
        };
        super::clamp_double_tap_activation(&mut hold);
        assert_eq!(hold.activation_mode, "hybrid");

        let mut hybrid = store::Settings {
            hotkey: "CmdOrControl+Shift+Space".into(),
            activation_mode: "hybrid".into(),
            ..store::Settings::default()
        };
        super::clamp_double_tap_activation(&mut hybrid);
        assert_eq!(hybrid.activation_mode, "hybrid");
    }

    #[test]
    fn modifier_only_cannot_keep_hybrid() {
        let mut settings = store::Settings {
            hotkey: "Shift".into(),
            activation_mode: "hybrid".into(),
            ..store::Settings::default()
        };
        super::clamp_double_tap_activation(&mut settings);
        assert_eq!(settings.activation_mode, "double_tap");
    }

    #[test]
    fn captured_tap_does_not_wipe_hybrid_combo() {
        let mut hybrid = store::Settings {
            hotkey: "Command+Shift+Space".into(),
            activation_mode: "hybrid".into(),
            ..store::Settings::default()
        };
        super::apply_captured_activation_mode(&mut hybrid, "tap");
        assert_eq!(hybrid.activation_mode, "hybrid");

        let mut tap = store::Settings {
            hotkey: "Command+Shift+Space".into(),
            activation_mode: "tap".into(),
            ..store::Settings::default()
        };
        super::apply_captured_activation_mode(&mut tap, "tap");
        assert_eq!(tap.activation_mode, "tap");

        let mut modifier = store::Settings {
            hotkey: "Fn".into(),
            activation_mode: "hybrid".into(),
            ..store::Settings::default()
        };
        super::apply_captured_activation_mode(&mut modifier, "double_tap");
        assert_eq!(modifier.activation_mode, "double_tap");
    }

    #[test]
    fn automatic_output_mode_keeps_app_context_while_explicit_mode_overrides_it() {
        let snapshot = context::ContextSnapshot::general();
        let mut settings = store::Settings::default();

        assert_eq!(
            super::cleanup_policy_for(&settings, &snapshot).output_mode,
            None
        );

        settings.output_mode = "email".into();
        assert_eq!(
            super::cleanup_policy_for(&settings, &snapshot).output_mode,
            Some("email".into())
        );
    }

    #[test]
    fn automatic_output_mode_does_not_arm_a_default_english_translation() {
        let snapshot = context::ContextSnapshot::general();
        let settings = store::Settings::default();
        assert_eq!(settings.output_mode, "auto");
        assert_eq!(settings.translation_target_language, "en");
        assert_eq!(
            super::cleanup_policy_for(&settings, &snapshot).translation_target_language,
            None
        );
        assert_eq!(super::spoken_translation_target(&settings), None);

        let mut translating = settings;
        translating.output_mode = "translation".into();
        assert_eq!(
            super::spoken_translation_target(&translating),
            Some("en")
        );
    }

    #[test]
    fn success_audio_keep_is_opt_in_and_skips_secure_or_learn_off_targets() {
        let mut settings = store::Settings::default();
        let mut context = context::ContextSnapshot::general();
        assert!(!settings.keep_success_audio);
        assert!(!super::should_keep_success_audio(&settings, &context));

        settings.keep_success_audio = true;
        settings.keep_audio_days = 7;
        assert!(super::should_keep_success_audio(&settings, &context));

        context.target_guard.secure_input = true;
        assert!(!super::should_keep_success_audio(&settings, &context));

        context.target_guard.secure_input = false;
        context.target_guard.bundle_id = Some("com.1password.1password".into());
        assert!(!super::should_keep_success_audio(&settings, &context));

        context.target_guard.bundle_id = None;
        settings.keep_audio_days = 0;
        assert!(!super::should_keep_success_audio(&settings, &context));
    }
}
