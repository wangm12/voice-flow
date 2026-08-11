mod asr;
mod audio;
mod chunker;
#[cfg(test)]
mod cleanup_corpus;
mod context;
mod groq;
mod hotkey;
mod instance;
mod island_window;
mod keychain;
mod llm;
mod metrics;
mod modifier_hotkey;
mod notch;
mod paste;
mod permissions;
mod queue;
mod realtime_asr;
mod snippets;
mod store;
#[cfg(test)]
mod test_http;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Listener, Manager, State};
use tokio_util::sync::CancellationToken;
#[derive(Clone, Copy, PartialEq, Debug)]
enum Phase {
    Idle,
    Starting,
    Recording,
    Stopping,
    Processing,
}
struct DictationManager {
    phase: Phase,
    started: std::time::Instant,
    gesture_lock: Option<std::time::Instant>,
    session_generation: u64,
    cancellation: CancellationToken,
    recording_context: Option<context::ContextSnapshot>,
}

#[derive(Debug, Clone)]
struct SelectedActionSession {
    selected_text: String,
    selection_fingerprint: u64,
    target_guard: context::TargetAppGuard,
    onboarding_trial: bool,
}

struct AppState {
    manager: Mutex<DictationManager>,
    /// The recorder performs blocking I/O (cpal stream setup/teardown with
    /// timeouts). It lives behind its own async mutex so dictation state
    /// transitions never hold the manager lock across a blocking call.
    recorder: Arc<Mutex<audio::Recorder>>,
    realtime_asr: Mutex<Option<realtime_asr::RealtimeAsrSession>>,
    selected_action: Mutex<Option<SelectedActionSession>>,
    asr_provider: Arc<dyn asr::AsrProvider>,
    settings: Mutex<store::Settings>,
    context: Mutex<context::ContextState>,
    gate: Arc<queue::RequestGate>,
    metrics: metrics::Metrics,
    hotkey_gate: tokio::sync::Mutex<()>,
    onboarding_test_mode: Mutex<bool>,
    onboarding_selected_text: Mutex<Option<String>>,
    _instance_lock: instance::InstanceLock,
}

#[derive(Debug)]
struct StartError {
    generation: u64,
    message: String,
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
const CLEANUP_STATUS_AI_FAILED: &str = "ai_failed";
const CLEANUP_STATUS_LOCAL_ONLY: &str = "local_only";
const CLEANUP_STATUS_SNIPPET_BYPASS: &str = "snippet_bypass";
const CLEANUP_STATUS_UNKNOWN: &str = "unknown";

#[derive(Debug, Clone, PartialEq, Eq)]
struct FinalText {
    text: String,
    degraded: bool,
    degraded_reason: Option<&'static str>,
}

fn finalize_text(raw: &str, decision: CleanupDecision) -> Result<FinalText, &'static str> {
    let (candidate, mut degraded, mut degraded_reason) = match decision {
        CleanupDecision::Provider(text) => (text, false, None),
        CleanupDecision::Disabled => (local_cleanup_or_raw(raw), false, None),
        CleanupDecision::Failed => (raw.to_owned(), true, Some("llm_cleanup_failed")),
    };
    let text = if candidate.trim().is_empty() {
        degraded = true;
        degraded_reason = Some("llm_cleanup_empty");
        raw.to_owned()
    } else {
        candidate
    };
    if text.trim().is_empty() {
        return Err("no_speech");
    }
    Ok(FinalText {
        text,
        degraded,
        degraded_reason,
    })
}

fn local_cleanup_or_raw(raw: &str) -> String {
    let cleaned = llm::local_cleanup(raw);
    if cleaned.trim().is_empty() {
        raw.to_owned()
    } else {
        cleaned
    }
}

fn cleanup_policy_for(
    settings: &store::Settings,
    recording_context: &context::ContextSnapshot,
) -> context::ContextPolicy {
    let mut policy = recording_context.policy.clone();
    if settings.output_mode != "auto" {
        policy.output_mode = Some(settings.output_mode.clone());
    }
    if settings.output_mode == "translation" {
        policy.translation_target_language = Some(settings.translation_target_language.clone());
    }
    policy
}

fn delivery_fallback_reason(target_current: bool, paste_error: &str) -> &'static str {
    if paste_error.contains("Accessibility permission") {
        "accessibility_required"
    } else if paste_error.contains("browser access is required") {
        "browser_permission_required"
    } else if paste_error.contains("focused input is not available") {
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

fn emit_state(app: &tauri::AppHandle, state: &str) {
    island_window::set_interactive(
        app,
        matches!(
            state,
            "starting" | "recording" | "recording_limited" | "processing" | "rate_limited"
        ),
    );
    let _ = app.emit("dictation://state", serde_json::json!({ "state": state }));
    if state == "idle" {
        emit_selected_action_state(app, "idle");
    }
    if state == "idle" {
        emit_progress(app, 0.0);
    }
}

fn emit_selected_action_state(app: &tauri::AppHandle, state: &str) {
    let _ = app.emit(
        "selected-action://state",
        serde_json::json!({ "state": state }),
    );
}

fn clear_selected_action(state: &AppState) {
    state
        .selected_action
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
}

async fn show_selected_action_error(
    app: &tauri::AppHandle,
    state: &AppState,
    message: &str,
    fallback_reason: &'static str,
) {
    show_island(app);
    let _ = app.emit("dictation://error", message);
    emit_state_with_delivery(app, "error", None, "none", Some(fallback_reason));
    emit_selected_action_state(app, "idle");
    tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
    if state.manager.lock().unwrap().phase == Phase::Idle {
        emit_state(app, "idle");
    }
}
fn emit_progress(app: &tauri::AppHandle, progress: f32) {
    let _ = app.emit(
        "dictation://progress",
        serde_json::json!({ "progress": progress.clamp(0.0, 1.0) }),
    );
}
fn emit_state_with_context(
    app: &tauri::AppHandle,
    state: &str,
    context: Option<&context::ContextSnapshot>,
) {
    emit_state_with_delivery(app, state, context, "pending", None);
}
fn emit_state_with_context_and_input_device(
    app: &tauri::AppHandle,
    state: &str,
    context: Option<&context::ContextSnapshot>,
    input_device: Option<&str>,
) {
    emit_state_with_delivery_and_input_device(app, state, context, "pending", None, input_device);
}
fn emit_state_with_delivery(
    app: &tauri::AppHandle,
    state: &str,
    context: Option<&context::ContextSnapshot>,
    delivery_method: &str,
    fallback_reason: Option<&str>,
) {
    emit_state_with_delivery_and_input_device(
        app,
        state,
        context,
        delivery_method,
        fallback_reason,
        None,
    );
}
fn emit_state_with_delivery_and_input_device(
    app: &tauri::AppHandle,
    state: &str,
    context: Option<&context::ContextSnapshot>,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    input_device: Option<&str>,
) {
    island_window::set_interactive(
        app,
        matches!(
            state,
            "starting" | "recording" | "recording_limited" | "processing" | "rate_limited"
        ),
    );
    let mut payload = serde_json::json!({ "state": state });
    if let Some(context) = context {
        payload["context_id"] = serde_json::json!(context.profile.id);
        payload["context_label"] = serde_json::json!(context::display_label(context));
    }
    payload["delivery_method"] = serde_json::json!(delivery_method);
    payload["fallback_reason"] = fallback_reason
        .map(serde_json::Value::from)
        .unwrap_or(serde_json::Value::Null);
    if let Some(input_device) = input_device {
        payload["input_device"] = serde_json::Value::from(input_device);
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
        let mut current = state.context.lock().unwrap();
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
        let mut current = state.context.lock().unwrap();
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
    let mut settings = state.settings.lock().unwrap().clone();
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
        let mut current = state.context.lock().unwrap();
        current.enabled = enabled;
        current.browser_access_enabled = browser_access_enabled;
        current.mappings = mappings;
    }
    *state.settings.lock().unwrap() = settings;
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
    realtime_tx: tokio::sync::mpsc::Sender<realtime_asr::RealtimeMessage>,
) -> Result<(), String> {
    let recorder = Arc::clone(&state.recorder);
    tokio::task::spawn_blocking(move || {
        let mut recorder = recorder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        recorder
            .start(app, &session, &input_device, chunk_length_secs, realtime_tx)
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

fn cancel_realtime_asr(state: &AppState) {
    let session = state
        .realtime_asr
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(session) = session {
        session.cancel();
    }
}

async fn finish_realtime_asr(state: &AppState) -> Option<realtime_asr::RealtimeAsrResult> {
    let session = state
        .realtime_asr
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()?;
    Some(session.finish(std::time::Duration::from_secs(3)).await)
}

#[tauri::command]
async fn start_dictation(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    start_with_error_feedback(&app, &state).await
}

async fn start_selected_action_with_feedback(
    app: &tauri::AppHandle,
    state: &AppState,
) -> Result<(), String> {
    emit_selected_action_state(app, "waiting_for_selection");
    let onboarding_selected_text = if *state.onboarding_test_mode.lock().unwrap() {
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
        let snapshot = state.context.lock().unwrap().snapshot.clone();
        *state
            .selected_action
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(SelectedActionSession {
            selected_text,
            selection_fingerprint: 0,
            target_guard: snapshot.target_guard,
            onboarding_trial: true,
        });

        return match start_internal(app, state).await {
            Ok(()) => {
                if state.manager.lock().unwrap().phase == Phase::Recording {
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
        };
    }

    if !permissions::check().accessibility {
        emit_selected_action_state(app, "accessibility_required");
        let message = "Accessibility permission is required to read selected text".to_owned();
        show_selected_action_error(app, state, &message, "accessibility_required").await;
        return Err(message);
    }

    refresh_context_snapshot(app, state).await;
    let snapshot = state.context.lock().unwrap().snapshot.clone();
    if snapshot.target_guard.input_token.is_none() {
        emit_selected_action_state(app, "waiting_for_selection");
        let message = "Select editable text before starting a selected-text action".to_owned();
        show_selected_action_error(app, state, &message, "input_unavailable").await;
        return Err(message);
    }

    let app_for_capture = app.clone();
    let captured =
        tokio::task::spawn_blocking(move || paste::capture_selected_text(&app_for_capture, true))
            .await
            .map_err(|error| format!("selection capture worker failed: {error}"))?
            .map_err(|error| error.to_string());
    let captured = match captured {
        Ok(captured) => captured,
        Err(error) => {
            emit_selected_action_state(app, "waiting_for_selection");
            show_selected_action_error(app, state, &error, "input_unavailable").await;
            return Err(error);
        }
    };

    *state
        .selected_action
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(SelectedActionSession {
        selected_text: captured.text,
        selection_fingerprint: captured.fingerprint,
        target_guard: snapshot.target_guard,
        onboarding_trial: false,
    });

    match start_internal(app, state).await {
        Ok(()) => {
            let recording_started = state.manager.lock().unwrap().phase == Phase::Recording;
            if recording_started {
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

async fn handle_selected_action_hotkey(app: &tauri::AppHandle, state: &AppState) {
    if hotkey::is_suspended() {
        return;
    }
    let phase = state.manager.lock().unwrap().phase;
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
                let _ = stop_internal(app, state).await;
            }
        }
        Phase::Starting | Phase::Stopping | Phase::Processing => {}
    }
}

async fn start_internal(app: &tauri::AppHandle, state: &AppState) -> Result<(), StartError> {
    // Claim the start transition before preflight or context detection. This
    // prevents a second command from resetting the first command while it is
    // waiting on permissions or the frontmost-app probe.
    {
        let mut m = state.manager.lock().unwrap();
        if m.phase != Phase::Idle {
            return Ok(());
        }
        m.phase = Phase::Starting;
        m.session_generation = m.session_generation.wrapping_add(1);
        m.cancellation = CancellationToken::new();
        m.recording_context = None;
    }
    // Give the user immediate feedback while permission/context/audio setup
    // completes. The HUD must not appear to ignore a global shortcut.
    show_island(app);
    emit_state(app, "starting");
    if !permissions::check().microphone {
        let failure_generation = reset_starting(state);
        return Err(StartError::new(
            failure_generation,
            "Microphone permission is required",
        ));
    }
    let settings_snapshot = state.settings.lock().unwrap().clone();
    let onboarding_test_mode = *state.onboarding_test_mode.lock().unwrap();
    if !settings_snapshot.onboarded && !onboarding_test_mode {
        let failure_generation = reset_starting(state);
        return Err(StartError::new(
            failure_generation,
            "Complete onboarding before dictation can start",
        ));
    }
    if settings_snapshot.api_key.trim().is_empty() {
        let failure_generation = reset_starting(state);
        return Err(StartError::new(
            failure_generation,
            "A valid API key is required before dictation can start",
        ));
    }
    // Always refresh immediately before starting audio. A stale check is not
    // enough here: the user may have switched apps within the freshness
    // window.
    refresh_context_snapshot(app, state).await;

    let id = format!("{}", chrono_like_id());
    let chunk_length_secs = state.settings.lock().unwrap().chunk_length_secs;
    let input_device = settings_snapshot.input_device.clone();
    let (realtime_tx, realtime_rx) = realtime_asr::RealtimeAsrSession::channel();

    // Phase 2: cpal setup waits on a blocking channel, so keep it off the
    // async runtime worker and the UI-facing command path.
    if let Err(error) = start_audio(
        state,
        app.clone(),
        id,
        input_device,
        chunk_length_secs,
        realtime_tx.clone(),
    )
    .await
    {
        let failure_generation = reset_starting(state);
        return Err(StartError::new(failure_generation, error));
    }

    // Capture the target after the recorder has successfully started. This
    // narrows the race where the user changes apps while cpal is initializing.
    refresh_context_snapshot(app, state).await;
    let recording_context = state.context.lock().unwrap().snapshot.clone();

    // Phase 3 (sync, short lock): commit the recording state.
    let start_was_cancelled = {
        let mut m = state.manager.lock().unwrap();
        if m.phase != Phase::Starting {
            true
        } else {
            m.started = std::time::Instant::now();
            m.phase = Phase::Recording;
            m.cancellation = CancellationToken::new();
            m.recording_context = Some(recording_context.clone());
            false
        }
    };
    if start_was_cancelled {
        // Raced with another transition; roll back the recorder we started.
        cancel_audio(state, app.clone()).await;
        return Ok(());
    }
    let selected_action_active = state
        .selected_action
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_some();
    if !selected_action_active {
        let realtime_cancellation = {
            let manager = state.manager.lock().unwrap();
            manager.cancellation.child_token()
        };
        let asr_language =
            asr::normalize_language(Some(settings_snapshot.language.as_str())).map(str::to_owned);
        let asr_prompt = build_asr_prompt(
            &settings_snapshot.dictionary,
            Some(&recording_context.policy),
        );
        let realtime_session = realtime_asr::RealtimeAsrSession::spawn(
            realtime_rx,
            realtime_tx,
            state.gate.clone(),
            state.asr_provider.clone(),
            asr::AsrOptions {
                api_key: settings_snapshot.api_key.clone(),
                language: asr_language,
                prompt: asr_prompt,
            },
            state.metrics.clone(),
            realtime_cancellation,
        );
        *state
            .realtime_asr
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(realtime_session);
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

fn reset_starting_manager(manager: &mut DictationManager) -> u64 {
    if manager.phase == Phase::Starting {
        manager.cancellation.cancel();
        manager.phase = Phase::Idle;
        manager.session_generation = manager.session_generation.wrapping_add(1);
        manager.recording_context = None;
    }
    manager.session_generation
}

fn reset_starting(state: &AppState) -> u64 {
    let mut manager = state.manager.lock().unwrap();
    let generation = reset_starting_manager(&mut manager);
    drop(manager);
    clear_selected_action(state);
    generation
}

async fn start_with_error_feedback(app: &tauri::AppHandle, state: &AppState) -> Result<(), String> {
    match start_internal(app, state).await {
        Ok(()) => Ok(()),
        Err(error) => {
            fail_for_generation(app, state, error.message.clone(), error.generation).await;
            Err(error.message)
        }
    }
}
const GESTURE_LOCK_MS: u128 = 400;
fn take_gesture_lock(manager: &mut DictationManager) -> bool {
    let now = std::time::Instant::now();
    if manager
        .gesture_lock
        .map(|t| now.duration_since(t).as_millis() < GESTURE_LOCK_MS)
        .unwrap_or(false)
    {
        return false;
    }
    manager.gesture_lock = Some(now);
    true
}

fn show_island(app: &tauri::AppHandle) {
    island_window::show_overlay(app);
}

async fn handle_audio_error(app: &tauri::AppHandle, state: &AppState, message: String) {
    let failure_generation = {
        let mut manager = state.manager.lock().unwrap();
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
    cancel_realtime_asr(state);
    cancel_audio(state, app.clone()).await;
    let (keep_audio_days, keep_history_days) = {
        let settings = state.settings.lock().unwrap();
        (settings.keep_audio_days, settings.keep_history_days)
    };
    recover_spool_into_history(app, keep_audio_days, keep_history_days);
    sync_modifier_hotkey_phase(Phase::Idle);
    hotkey::unregister_cancel(app);
    fail_for_generation(
        app,
        state,
        format!("Microphone recording stopped: {message}"),
        failure_generation,
    )
    .await;
}

async fn handle_audio_limit(app: &tauri::AppHandle, state: &AppState) {
    let recording_context = {
        let manager = state.manager.lock().unwrap();
        if manager.phase != Phase::Recording {
            return;
        }
        manager.recording_context.clone()
    };

    // The audio engine has already detached the stream. Give the user a short
    // visible explanation, then finish the captured samples automatically so
    // the 15-minute ceiling cannot leave the app stuck in Recording.
    emit_state_with_context(app, "recording_limited", recording_context.as_ref());
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let _guard = state.hotkey_gate.lock().await;
    if state.manager.lock().unwrap().phase == Phase::Recording {
        let _ = stop_internal(app, state).await;
    }
}

async fn paste_text(
    app: &tauri::AppHandle,
    state: &AppState,
    text: &str,
    expected_target: &context::TargetAppGuard,
    accessibility: bool,
    cancellation: CancellationToken,
) -> Result<(), String> {
    let app = app.clone();
    let text = text.to_owned();
    let (mappings, browser_access_enabled) = {
        let current = state.context.lock().unwrap();
        (current.mappings.clone(), current.browser_access_enabled)
    };
    let expected_target = expected_target.clone();
    tokio::task::spawn_blocking(move || {
        let verify_target = move || {
            let current = context::detect_snapshot(&mappings, browser_access_enabled);
            match context::target_mismatch_reason(&expected_target, &current.target_guard) {
                None => Ok(()),
                Some("browser_permission_required") => {
                    Err(paste::PasteError::BrowserAccessRequired)
                }
                Some("input_unavailable") => Err(paste::PasteError::InputUnavailable),
                Some("input_changed") => Err(paste::PasteError::InputChanged),
                Some("target_unavailable") => Err(paste::PasteError::TargetUnavailable),
                Some(_) => Err(paste::PasteError::TargetChanged),
            }
        };
        paste::insert(&app, &text, accessibility, cancellation, verify_target)
    })
    .await
    .map_err(|error| format!("paste worker failed: {error}"))?
    .map_err(|error| error.to_string())
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

fn onboarding_delivery_target_matches(
    enabled: bool,
    recording_context: &context::ContextSnapshot,
    frontmost: (i32, Option<String>),
) -> bool {
    if !enabled {
        return false;
    }
    if recording_context.target_guard.bundle_id.as_deref() != Some("com.voiceflow.desktop") {
        return false;
    }
    let (pid, bundle_id) = frontmost;
    pid == recording_context.target_guard.pid
        && bundle_id.as_deref() == Some("com.voiceflow.desktop")
}

fn should_use_onboarding_delivery(
    state: &AppState,
    recording_context: &context::ContextSnapshot,
) -> bool {
    onboarding_delivery_target_matches(
        *state.onboarding_test_mode.lock().unwrap(),
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

#[tauri::command]
async fn stop_dictation(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    stop_internal(&app, &state).await
}
async fn stop_internal(app: &tauri::AppHandle, state: &AppState) -> Result<(), String> {
    // Phase 1 (sync, short lock): validate recording and claim the session.
    let (started, session_generation, cancellation, recording_context) = {
        let mut m = state.manager.lock().unwrap();
        if m.phase != Phase::Recording {
            return Ok(());
        }
        let started = m.started;
        m.session_generation = m.session_generation.wrapping_add(1);
        let session_generation = m.session_generation;
        let recording_context = m
            .recording_context
            .take()
            .unwrap_or_else(context::ContextSnapshot::general);
        // Claim the stop transition before finalizing audio. A second hotkey
        // event must not start another stop worker while the first one owns
        // the recorder mutex.
        m.phase = Phase::Stopping;
        (
            started,
            session_generation,
            m.cancellation.clone(),
            recording_context,
        )
    };

    // Finalizing a long recording can take noticeable time. Show processing
    // immediately so the HUD never appears to ignore the user's stop press.
    sync_modifier_hotkey_phase(Phase::Stopping);
    emit_state_with_context(app, "processing", Some(&recording_context));
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
            cancel_realtime_asr(state);
            let stop_was_cancelled = {
                let m = state.manager.lock().unwrap();
                !stop_transition_is_current(
                    m.phase,
                    m.session_generation,
                    session_generation,
                    m.cancellation.is_cancelled(),
                )
            };
            if stop_was_cancelled {
                return Ok(());
            }
            let message = error;
            {
                let mut m = state.manager.lock().unwrap();
                m.cancellation.cancel();
                m.phase = Phase::Idle;
                m.recording_context = None;
            }
            sync_modifier_hotkey_phase(Phase::Idle);
            hotkey::unregister_cancel(app);
            fail_for_generation(app, state, message.clone(), session_generation).await;
            return Err(message);
        }
    };

    // All capture-side chunks have been queued by the time finalization
    // returns. Give completed background ASR requests a short opportunity to
    // finish; a slow request is cancelled and the normal post-stop path will
    // transcribe the missing chunk instead.
    let realtime_prefetch = finish_realtime_asr(state).await;

    // Phase 3 (sync, short lock): claim the processing transition only if the
    // stop still belongs to this session. Cancellation is allowed while audio
    // finalization is in flight; in that case the finalized samples are simply
    // dropped and no provider or delivery work may start.
    let should_process = {
        let mut m = state.manager.lock().unwrap();
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
        return Ok(());
    }
    sync_modifier_hotkey_phase(Phase::Processing);

    let settings = state.settings.lock().unwrap().clone();
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
            realtime_prefetch
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
            realtime_prefetch,
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

fn realtime_short_tail_wav(chunks: &[chunker::AudioChunk]) -> Option<Vec<u8>> {
    let chunk = chunks.iter().find(|chunk| chunk.index == 0)?;
    let tail_start =
        realtime_asr::WARMUP_CHUNK_SECS.saturating_sub(realtime_asr::WARMUP_OVERLAP_SECS) * 16_000;
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
        "target_unavailable" => paste::PasteError::TargetUnavailable,
        _ => paste::PasteError::TargetChanged,
    }
}

async fn paste_selected_text(
    app: &tauri::AppHandle,
    state: &AppState,
    text: &str,
    session: &SelectedActionSession,
    accessibility: bool,
    cancellation: CancellationToken,
) -> Result<(), String> {
    let app = app.clone();
    let text = text.to_owned();
    let expected_target = session.target_guard.clone();
    let expected_text = session.selected_text.clone();
    let expected_fingerprint = session.selection_fingerprint;
    let (mappings, browser_access_enabled) = {
        let current = state.context.lock().unwrap();
        (current.mappings.clone(), current.browser_access_enabled)
    };
    tokio::task::spawn_blocking(move || {
        let capture_app = app.clone();
        let verify_target = move || {
            let current = context::detect_snapshot(&mappings, browser_access_enabled);
            if let Some(reason) =
                context::target_mismatch_reason(&expected_target, &current.target_guard)
            {
                return Err(selected_target_error(reason));
            }
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
        paste::insert(&app, &text, accessibility, cancellation, verify_target)
    })
    .await
    .map_err(|error| format!("selected text paste worker failed: {error}"))?
    .map_err(|error| error.to_string())
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
    let provider = state.asr_provider.clone();
    let options = asr::AsrOptions {
        api_key: settings.api_key.clone(),
        language: asr::normalize_language(Some(settings.language.as_str())).map(str::to_owned),
        prompt: build_asr_prompt(&settings.dictionary, Some(&recording_context.policy)),
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

    let cleanup = {
        let _latency = state.metrics.timer(metrics::MetricKind::Cleanup);
        queue::execute_with_retry_cancelled(
            &state.gate,
            queue::RequestKind::Llm,
            || {
                llm::selected_text_action_with_limits(
                    &settings.cleanup_model,
                    &selected_action.selected_text,
                    &transcript,
                    &settings.api_key,
                    Some(&recording_context.policy),
                    Some(&recording_context.profile),
                    Some(settings.translation_target_language.as_str()),
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
            Some(session_generation),
        )
        .await;
        return Ok(());
    }

    emit_selected_action_state(app, "ready_to_replace");
    let paste_result = {
        let _latency = state.metrics.timer(metrics::MetricKind::Paste);
        paste_selected_text(
            app,
            state,
            &final_text,
            &selected_action,
            permissions::check().accessibility,
            cancellation.clone(),
        )
        .await
    };
    match paste_result {
        Ok(()) => {
            stop_to_insert.finish();
            finish_with_delivery(
                app,
                state,
                "done",
                Some(recording_context),
                "paste",
                None,
                Some(session_generation),
            )
            .await;
            Ok(())
        }
        Err(_error)
            if cancellation.is_cancelled() || processing_aborted(state, session_generation) =>
        {
            Ok(())
        }
        Err(error) => {
            log::warn!("selected text replacement failed; copying result instead: {error}");
            if let Err(copy_error) = copy_text(app, &final_text, cancellation.clone()).await {
                emit_selected_action_state(app, "idle");
                fail_for_generation(app, state, copy_error.to_string(), session_generation).await;
                return Err(copy_error);
            }
            emit_selected_action_state(
                app,
                if error.contains("selected text changed") {
                    "selection_changed"
                } else {
                    "copied_instead"
                },
            );
            stop_to_insert.finish();
            finish_with_delivery(
                app,
                state,
                "copied",
                Some(recording_context),
                "clipboard",
                Some("selected_action_clipboard_fallback"),
                Some(session_generation),
            )
            .await;
            Ok(())
        }
    }
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
    realtime_prefetch: Option<realtime_asr::RealtimeAsrResult>,
    session_generation: u64,
    cancellation: CancellationToken,
    stop_to_insert: metrics::LatencyTimer,
) -> Result<(), String> {
    let language = asr::normalize_language(Some(settings.language.as_str())).map(str::to_owned);
    let asr_prompt = build_asr_prompt(&settings.dictionary, Some(&recording_context.policy));
    let asr_provider = state.asr_provider.clone();
    let asr_options = asr::AsrOptions {
        api_key: settings.api_key.clone(),
        language,
        prompt: asr_prompt,
    };
    let cleanup_policy = cleanup_policy_for(settings, recording_context);
    let mut raw = None;
    if let Some(warmup) = realtime_prefetch
        .as_ref()
        .and_then(|result| result.warmup.as_deref())
    {
        let total_samples = chunks.first().map(|chunk| chunk.samples.len()).unwrap_or(0);
        if total_samples <= realtime_asr::WARMUP_CHUNK_SECS * 16_000 {
            raw = Some(warmup.to_owned());
        } else if let Some(tail_wav) = realtime_short_tail_wav(&chunks) {
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
                        "realtime ASR tail failed; falling back to the complete recording: {error}"
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
    let spoken_raw = raw.clone();
    let snippet_expansion = snippets::resolve_exact(&settings.snippets, &raw);
    let raw = snippet_expansion.clone().unwrap_or(raw);
    let snippet_expanded = snippet_expansion.is_some();
    if processing_aborted(state, session_generation) {
        return Ok(());
    }
    if raw.trim().is_empty() {
        let message = "No speech detected".to_string();
        fail_for_generation(app, state, message.clone(), session_generation).await;
        return Err(message);
    }
    emit_progress(app, 0.45);
    let cleanup_decision = if settings.cleanup_enabled && !snippet_expanded {
        let cleanup_result = {
            let _latency = state.metrics.timer(metrics::MetricKind::Cleanup);
            queue::execute_with_retry_cancelled(
                &state.gate,
                queue::RequestKind::Llm,
                || {
                    llm::cleanup_with_model_and_limits_and_language_and_profile(
                        &settings.cleanup_model,
                        &raw,
                        &settings.api_key,
                        &settings.dictionary,
                        None,
                        Some(&cleanup_policy),
                        Some(settings.language.as_str()),
                        Some(&recording_context.profile),
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
    } else {
        CleanupDecision::Disabled
    };
    let cleanup_status = match &cleanup_decision {
        CleanupDecision::Provider(text) if text.trim().is_empty() => CLEANUP_STATUS_AI_FAILED,
        CleanupDecision::Provider(_) => CLEANUP_STATUS_AI_SUCCESS,
        CleanupDecision::Failed => CLEANUP_STATUS_AI_FAILED,
        CleanupDecision::Disabled if snippet_expanded => CLEANUP_STATUS_SNIPPET_BYPASS,
        CleanupDecision::Disabled => CLEANUP_STATUS_LOCAL_ONLY,
    };
    if processing_aborted(state, session_generation) {
        return Ok(());
    }
    emit_progress(app, 0.70);
    let resolved_text = match finalize_text(&raw, cleanup_decision) {
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
    let (pasted, fallback_reason) = {
        if processing_aborted(state, session_generation) {
            return Ok(());
        }
        emit_progress(app, 0.88);
        let paste_result = {
            let _latency = state.metrics.timer(metrics::MetricKind::Paste);
            if should_use_onboarding_delivery(state, recording_context) {
                emit_onboarding_result(app, &spoken_raw, &final_text);
                Ok(())
            } else {
                paste_text(
                    app,
                    state,
                    &final_text,
                    &recording_context.target_guard,
                    permissions::check().accessibility,
                    cancellation.clone(),
                )
                .await
            }
        };
        match paste_result {
            Ok(()) => (true, None),
            Err(e) => {
                if processing_aborted(state, session_generation) || cancellation.is_cancelled() {
                    discard_short_recovery_audio(app, recovery_spool.as_deref());
                    return Ok(());
                }
                log::warn!("paste failed, falling back to clipboard: {e}");
                if let Err(copy_err) = copy_text(app, &final_text, cancellation.clone()).await {
                    if processing_aborted(state, session_generation) || cancellation.is_cancelled()
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
                    fail_for_generation(app, state, copy_err.to_string(), session_generation).await;
                    return Err(copy_err);
                }
                (false, Some(delivery_fallback_reason(true, &e)))
            }
        }
    };
    // Cancellation can arrive after delivery returns but before durable History
    // persistence. Do not let an aborted processing generation leave a late
    // result behind as if the user had completed the dictation.
    if processing_aborted(state, session_generation) || cancellation.is_cancelled() {
        discard_short_recovery_audio(app, recovery_spool.as_deref());
        return Ok(());
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
            } else if pasted {
                "ok"
            } else {
                "copied"
            },
            if pasted { "paste" } else { "clipboard" },
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
    if pasted {
        stop_to_insert.finish();
        finish_with_delivery(
            app,
            state,
            "done",
            Some(recording_context),
            "paste",
            None,
            Some(session_generation),
        )
        .await;
    } else {
        stop_to_insert.finish();
        let _ = app.emit(
            "dictation://copied",
            serde_json::json!({ "message": "已复制，请手动粘贴" }),
        );
        finish_with_delivery(
            app,
            state,
            "copied",
            Some(recording_context),
            "clipboard",
            fallback_reason,
            Some(session_generation),
        )
        .await;
    }
    Ok(())
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

#[allow(clippy::too_many_arguments)]
async fn process_long(
    app: &tauri::AppHandle,
    state: &AppState,
    chunks: Vec<chunker::AudioChunk>,
    started: std::time::Instant,
    settings: &store::Settings,
    recording_context: &context::ContextSnapshot,
    realtime_prefetch: Option<std::collections::HashMap<usize, String>>,
    session_generation: u64,
    cancellation: CancellationToken,
    stop_to_insert: metrics::LatencyTimer,
) -> Result<(), String> {
    let total = chunks.len();
    emit_progress(app, 0.05);
    let mut jobs = Vec::new();
    let dir = app.path().app_data_dir().ok().map(|p| {
        p.join("spool")
            .join(format!("session-{}", chrono_like_id()))
    });
    if let Some(session_dir) = &dir {
        if let Ok(root) = app.path().app_data_dir() {
            if let Some(session_id) = session_dir.file_name().and_then(|name| name.to_str()) {
                if let Err(error) = store::begin_spool_session(&root, session_id) {
                    log::warn!("failed to create long-recording manifest: {error}");
                }
            }
        }
    }
    let mut chunk_times = std::collections::HashMap::new();
    let asr_provider = state.asr_provider.clone();
    let asr_options = asr::AsrOptions {
        api_key: settings.api_key.clone(),
        language: asr::normalize_language(Some(settings.language.as_str())).map(str::to_owned),
        prompt: build_asr_prompt(&settings.dictionary, Some(&recording_context.policy)),
    };
    let cleanup_policy = cleanup_policy_for(settings, recording_context);
    for chunk in chunks {
        let chunk_start_secs = chunk.start_secs;
        let chunk_end_secs = chunk.end_secs;
        chunk_times.insert(chunk.index, (chunk_start_secs, chunk_end_secs));
        let spool_bytes = chunk
            .samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>();
        let wav = match chunker::encode_wav(&chunk.samples) {
            Ok(wav) => wav,
            Err(error) => {
                let message = format!("long-recording audio encoding failed: {error}");
                mark_spool_degraded(dir.as_deref());
                fail_for_generation(app, state, message.clone(), session_generation).await;
                return Err(message);
            }
        };
        if let Some(d) = &dir {
            let session_name = d
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("session");
            let relative = std::path::PathBuf::from(session_name)
                .join("chunks")
                .join(format!("{:08}.f32", chunk.index));
            if let Ok(root) = app.path().app_data_dir() {
                match store::write_spool_file(&root, &relative, &spool_bytes) {
                    Ok(_) => {
                        if let Err(error) = store::record_spool_chunk(
                            d,
                            chunk.index,
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
        }
        let prefetched = realtime_prefetch
            .as_ref()
            .and_then(|transcripts| transcripts.get(&chunk.index))
            .cloned();
        if let Some(prefetched) = prefetched {
            jobs.push(tokio::spawn(async move { (chunk.index, Ok(prefetched)) }));
        } else {
            let gate = state.gate.clone();
            let metrics = state.metrics.clone();
            let provider = asr_provider.clone();
            let options = asr_options.clone();
            let cancellation = cancellation.clone();
            jobs.push(tokio::spawn(async move {
                let r = {
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
                (chunk.index, r)
            }));
        }
    }
    let mut raw_texts = Vec::new();
    let mut failed_chunks = 0usize;
    let mut cleanup_failure_reason: Option<&'static str> = None;
    for (done, job) in jobs.into_iter().enumerate() {
        if processing_aborted(state, session_generation) {
            if let Some(d) = &dir {
                let _ = std::fs::remove_dir_all(d);
            }
            return Ok(());
        }
        let (index, result) = match job.await {
            Ok(result) => result,
            Err(error) => {
                let message = format!("long-recording transcription task failed: {error}");
                mark_spool_degraded(dir.as_deref());
                fail_for_generation(app, state, message.clone(), session_generation).await;
                return Err(message);
            }
        };
        let (chunk_start_secs, chunk_end_secs) =
            chunk_times.get(&index).copied().unwrap_or((0.0, 0.0));
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
            Err(_) => {
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
                let placeholder = format!("[此处约{}秒识别失败]", settings.chunk_length_secs);
                raw_texts.push((index, placeholder));
            }
        }
        let progress = if total == 0 {
            0.1
        } else {
            0.1 + 0.72 * (done + 1) as f32 / total as f32
        };
        let _ = app.emit(
            "dictation://progress",
            serde_json::json!({
                "elapsed_secs": started.elapsed().as_secs(),
                "chunks_done": done + 1,
                "total": total,
                "progress": progress.clamp(0.0, 0.84),
            }),
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
    let raw_text = chunker::merge_transcripts(raw_texts);
    let cleanup_status;
    let final_text = if let Some(expansion) = snippets::resolve_exact(&settings.snippets, &raw_text)
    {
        cleanup_status = CLEANUP_STATUS_SNIPPET_BYPASS;
        expansion
    } else if settings.cleanup_enabled {
        // Long recordings are cleaned only after every ASR chunk has been
        // merged. This gives the model the complete spoken structure instead
        // of asking it to make independent decisions at chunk boundaries.
        emit_progress(app, 0.86);
        let cleanup_result = {
            let _latency = state.metrics.timer(metrics::MetricKind::Cleanup);
            queue::execute_with_retry_cancelled(
                &state.gate,
                queue::RequestKind::Llm,
                || {
                    llm::cleanup_with_model_and_limits_and_language_and_profile(
                        &settings.cleanup_model,
                        &raw_text,
                        &settings.api_key,
                        &settings.dictionary,
                        None,
                        Some(&cleanup_policy),
                        Some(settings.language.as_str()),
                        Some(&recording_context.profile),
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
                cleanup_status = CLEANUP_STATUS_AI_FAILED;
                cleanup_failure_reason = Some("llm_cleanup_empty");
                raw_text.clone()
            }
            Err(queue::ExecuteError::Cancelled) => {
                discard_spool(dir.as_deref());
                return Ok(());
            }
            Err(error) => {
                cleanup_status = CLEANUP_STATUS_AI_FAILED;
                cleanup_failure_reason = Some("llm_cleanup_failed");
                log::warn!(
                    "long-recording LLM cleanup failed after transcript merge, keeping raw transcript: {error:?}"
                );
                raw_text.clone()
            }
        }
    } else {
        cleanup_status = CLEANUP_STATUS_LOCAL_ONLY;
        local_cleanup_or_raw(&raw_text)
    };
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
    let mut delivered_via_paste = false;
    let mut delivery_method = "history";
    let mut fallback_reason = None;
    if settings.long_output_mode == "paste" {
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
                )
                .await
            };
            match paste_result {
                Ok(()) => {
                    delivered_via_paste = true;
                    delivery_method = "paste";
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
    } else if settings.long_output_mode == "clipboard" {
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
    let degraded_spool = degraded_spool_path.as_deref();
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
            } else {
                "ok"
            },
            delivery_method,
            fallback_reason,
            recording_context,
            degraded_spool,
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
    let copied_fallback = settings.long_output_mode == "paste" && !delivered_via_paste;
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
            long_completion_state(degraded, true),
            Some(recording_context),
            "clipboard",
            fallback_reason,
            Some(session_generation),
        )
        .await;
    } else {
        stop_to_insert.finish();
        finish_with_delivery(
            app,
            state,
            long_completion_state(degraded, false),
            Some(recording_context),
            delivery_method,
            fallback_reason,
            Some(session_generation),
        )
        .await;
    }
    Ok(())
}
fn processing_aborted(state: &AppState, session_generation: u64) -> bool {
    let m = state.manager.lock().unwrap();
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
        let mut manager = state.manager.lock().unwrap();
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

fn long_completion_state(degraded: bool, copied_fallback: bool) -> &'static str {
    if degraded {
        "degraded"
    } else if copied_fallback {
        "copied"
    } else {
        "done"
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

fn build_asr_prompt(
    dictionary: &[String],
    policy: Option<&context::ContextPolicy>,
) -> Option<String> {
    let mut hints = dictionary
        .iter()
        .map(|word| word.trim())
        .filter(|word| !word.is_empty())
        .take(32)
        .collect::<Vec<_>>();
    if policy.is_some_and(|policy| policy.preserve_technical_tokens) {
        hints.push("preserve technical terms, identifiers, paths, commands, URLs, and versions");
    }
    if hints.is_empty() {
        None
    } else {
        let prompt = format!(
            "Recognize these terms exactly when spoken: {}",
            hints.join(", ")
        );
        Some(prompt.chars().take(2_000).collect())
    }
}
fn abort_processing(app: &tauri::AppHandle, state: &AppState) {
    let mut m = state.manager.lock().unwrap();
    if m.phase != Phase::Processing {
        return;
    }
    m.cancellation.cancel();
    m.session_generation = m.session_generation.wrapping_add(1);
    m.phase = Phase::Idle;
    sync_modifier_hotkey_phase(Phase::Idle);
    hotkey::unregister_cancel(app);
    emit_state(app, "idle");
    island_window::hide_overlay(app);
}
async fn fail_for_generation(
    app: &tauri::AppHandle,
    state: &AppState,
    message: String,
    expected_generation: u64,
) {
    let is_current = {
        let manager = state.manager.lock().unwrap();
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
        Some(expected_generation),
    )
    .await;
}
async fn finish_with_delivery(
    app: &tauri::AppHandle,
    state: &AppState,
    phase: &str,
    context: Option<&context::ContextSnapshot>,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    expected_generation: Option<u64>,
) {
    let completion_generation = {
        let mut m = state.manager.lock().unwrap();
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
                )
            };
            if !is_current {
                return;
            }
        }
        m.cancellation.cancel();
        m.phase = Phase::Idle;
        m.recording_context = None;
        m.session_generation
    };
    // Only the current processing generation may release the active cancel
    // shortcut. A stale completion can arrive after the user cancelled and
    // started a new recording; unregistering here would otherwise remove the
    // new recording's Escape handler.
    hotkey::unregister_cancel(app);
    sync_modifier_hotkey_phase(Phase::Idle);
    if phase == "error" {
        show_island(app);
    }
    if matches!(phase, "done" | "copied" | "degraded") {
        emit_progress(app, 1.0);
    }
    if let Some(context) = context {
        emit_state_with_delivery(app, phase, Some(context), delivery_method, fallback_reason);
    } else {
        emit_state_with_delivery(app, phase, None, delivery_method, fallback_reason);
    }
    let dwell_ms = match phase {
        "done" => 550,
        "copied" => 1800,
        "degraded" => 2200,
        _ => 1500,
    };
    tokio::time::sleep(std::time::Duration::from_millis(dwell_ms)).await;
    let should_hide = {
        let m = state.manager.lock().unwrap();
        m.phase == Phase::Idle && m.session_generation == completion_generation
    };
    if should_hide {
        emit_state(app, "idle");
        // Let the WebView finish its short opacity exit before hiding the
        // native panel. Hiding the panel in the same turn as the idle event
        // cuts off copied/degraded/error fades and reads as a dropped frame.
        tokio::time::sleep(std::time::Duration::from_millis(140)).await;
        let should_hide_after_fade = {
            let m = state.manager.lock().unwrap();
            m.phase == Phase::Idle && m.session_generation == completion_generation
        };
        if should_hide_after_fade {
            island_window::hide_overlay(app);
        }
    }
}
#[tauri::command]
async fn cancel_dictation(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    cancel_internal(&app, &state).await;
    Ok(())
}
async fn cancel_internal(app: &tauri::AppHandle, state: &AppState) {
    let phase = state.manager.lock().unwrap().phase;
    clear_selected_action(state);
    match phase {
        Phase::Starting => {
            let mut m = state.manager.lock().unwrap();
            m.cancellation.cancel();
            m.phase = Phase::Idle;
            m.session_generation = m.session_generation.wrapping_add(1);
            m.recording_context = None;
            drop(m);
            sync_modifier_hotkey_phase(Phase::Idle);
            hotkey::unregister_cancel(app);
            emit_state(app, "idle");
            island_window::hide_overlay(app);
        }
        Phase::Recording => {
            {
                let mut m = state.manager.lock().unwrap();
                m.cancellation.cancel();
                m.phase = Phase::Idle;
                m.session_generation = m.session_generation.wrapping_add(1);
            }
            cancel_realtime_asr(state);
            // Blocking recorder cancel runs outside the manager lock.
            cancel_audio(state, app.clone()).await;
            sync_modifier_hotkey_phase(Phase::Idle);
            hotkey::unregister_cancel(app);
            emit_state(app, "idle");
            island_window::hide_overlay(app);
        }
        Phase::Stopping => {
            // Audio finalization owns the recorder mutex, so do not enqueue a
            // competing recorder cancel here. Mark the session cancelled and
            // let stop_internal drop its finalized result before providers or
            // paste can start.
            let mut m = state.manager.lock().unwrap();
            m.cancellation.cancel();
            m.phase = Phase::Idle;
            m.session_generation = m.session_generation.wrapping_add(1);
            m.recording_context = None;
            drop(m);
            cancel_realtime_asr(state);
            sync_modifier_hotkey_phase(Phase::Idle);
            hotkey::unregister_cancel(app);
            emit_state(app, "idle");
            island_window::hide_overlay(app);
        }
        Phase::Processing => abort_processing(app, state),
        Phase::Idle => {}
    }
}
async fn handle_hotkey_toggle(app: &tauri::AppHandle, state: &AppState) {
    if hotkey::is_suspended() {
        return;
    }
    if !take_gesture_lock(&mut state.manager.lock().unwrap()) {
        return;
    }
    let phase = state.manager.lock().unwrap().phase;
    match phase {
        Phase::Idle => {
            let _ = start_with_error_feedback(app, state).await;
        }
        Phase::Starting => {}
        Phase::Recording => {
            let _ = stop_internal(app, state).await;
        }
        Phase::Stopping => {}
        Phase::Processing => abort_processing(app, state),
    }
}
async fn handle_double_tap_toggle(app: &tauri::AppHandle, state: &AppState, confirmed: bool) {
    if !confirmed || !take_gesture_lock(&mut state.manager.lock().unwrap()) {
        return;
    }
    let phase = state.manager.lock().unwrap().phase;
    match phase {
        Phase::Recording => {
            let _ = stop_internal(app, state).await;
        }
        Phase::Idle => {
            let _ = start_with_error_feedback(app, state).await;
        }
        Phase::Starting => {}
        Phase::Stopping => {}
        Phase::Processing => {
            abort_processing(app, state);
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

    let _guard = state.hotkey_gate.lock().await;
    let previous = state.settings.lock().unwrap().clone();
    let mut settings = previous.clone();
    if let Some(hotkey) = captured_hotkey {
        if capture_target.as_deref() == Some("selected_action") {
            settings.selected_action_hotkey = hotkey;
            settings.selected_actions_enabled = true;
        } else {
            settings.hotkey = hotkey;
        }
    }
    if let Some(mode) = captured_activation_mode {
        if matches!(mode.as_str(), "tap" | "double_tap") {
            settings.activation_mode = mode;
        }
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
    *state.settings.lock().unwrap() = settings;
    hotkey::set_suspended(false);
    Ok(())
}
#[tauri::command]
fn set_onboarding_test_mode(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    *state.onboarding_test_mode.lock().unwrap() = enabled;
    if !enabled {
        *state.onboarding_selected_text.lock().unwrap() = None;
    }
    Ok(())
}

#[tauri::command]
fn set_onboarding_selected_text(state: State<'_, AppState>, text: String) -> Result<(), String> {
    if !*state.onboarding_test_mode.lock().unwrap() {
        return Err("onboarding test mode is not active".into());
    }
    *state.onboarding_selected_text.lock().unwrap() = (!text.trim().is_empty()).then_some(text);
    Ok(())
}
#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> store::SettingsView {
    store::SettingsView::from(&*state.settings.lock().unwrap())
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
    state.context.lock().unwrap().snapshot.clone()
}
#[tauri::command]
fn get_context_mappings(state: State<'_, AppState>) -> Vec<context::AppMapping> {
    state.context.lock().unwrap().mappings.clone()
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
    state.context.lock().unwrap().manual_override
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
    let _guard = state.hotkey_gate.lock().await;
    let (enabled, browser_access_enabled, mut mappings) = {
        let current = state.context.lock().unwrap();
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
    let _guard = state.hotkey_gate.lock().await;
    let (enabled, browser_access_enabled, mut mappings) = {
        let current = state.context.lock().unwrap();
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
    let _guard = state.hotkey_gate.lock().await;
    let (browser_access_enabled, mappings) = {
        let current = state.context.lock().unwrap();
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
    let _guard = state.hotkey_gate.lock().await;
    {
        let mut current = state.context.lock().unwrap();
        current.manual_override = family;
    }
    refresh_context_snapshot(&app, &state).await;
    Ok(state.context.lock().unwrap().snapshot.clone())
}
#[tauri::command]
async fn request_browser_access(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let _guard = state.hotkey_gate.lock().await;
    let (enabled, mappings) = {
        let current = state.context.lock().unwrap();
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
    settings.validate().map_err(|error| error.to_string())?;
    let prev = state.settings.lock().unwrap().clone();
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
    if prev.keep_history_days != settings.keep_history_days {
        if let Err(error) = store::purge_history(&dir, settings.keep_history_days) {
            log::warn!("history retention cleanup after settings change failed: {error}");
        }
    }
    *state.settings.lock().unwrap() = settings;
    if tray_visibility_changed {
        if let Some(tray) = app.tray_by_id("voiceflow-status") {
            if let Err(error) = tray.set_visible(tray_visible) {
                log::warn!("failed to update tray icon visibility: {error}");
            }
        }
    }
    if context_changed {
        {
            let mut current = state.context.lock().unwrap();
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
    let _guard = state.hotkey_gate.lock().await;
    apply_settings(app, &state, settings).await
}

#[tauri::command]
async fn update_settings_patch(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    patch: serde_json::Value,
) -> Result<(), String> {
    let _guard = state.hotkey_gate.lock().await;
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
        "long_output_mode",
        "keep_audio_days",
        "keep_history_days",
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
        "input_device",
    ];
    if let Some(unknown) = object.keys().find(|key| !ALLOWED.contains(&key.as_str())) {
        return Err(format!("unsupported settings field: {unknown}"));
    }
    let current = state.settings.lock().unwrap().clone();
    let mut merged = serde_json::to_value(current).map_err(|error| error.to_string())?;
    let merged_object = merged
        .as_object_mut()
        .ok_or_else(|| "settings serialization failed".to_owned())?;
    for (key, value) in object {
        merged_object.insert(key.clone(), value.clone());
    }
    let settings = serde_json::from_value(merged).map_err(|error| error.to_string())?;
    apply_settings(app, &state, settings).await
}

#[tauri::command]
async fn remove_api_key(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<store::SettingsView, String> {
    let _guard = state.hotkey_gate.lock().await;
    keychain::set_api_key("")
        .map_err(|error| format!("failed to remove API key securely: {error}"))?;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let mut settings = state.settings.lock().unwrap().clone();
    settings.api_key.clear();
    settings.onboarded = false;
    store::save_settings(&dir, &settings).map_err(|error| error.to_string())?;
    *state.settings.lock().unwrap() = settings.clone();
    Ok(store::SettingsView::from(&settings))
}

fn clamp_double_tap_activation(settings: &mut store::Settings) {
    let modifier_only = crate::modifier_hotkey::is_modifier_only(&settings.hotkey);
    if modifier_only && settings.activation_mode != "double_tap" {
        log::warn!(
            "modifier-only hotkeys require double_tap activation; using double_tap semantics"
        );
        settings.activation_mode = "double_tap".into();
    } else if !modifier_only && matches!(settings.activation_mode.as_str(), "hold" | "double_tap") {
        log::warn!(
            "double_tap activation is only supported for modifier-only hotkeys; using tap semantics"
        );
        settings.activation_mode = "tap".into();
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
fn get_history(
    app: tauri::AppHandle,
    before_id: Option<i64>,
    limit: Option<i64>,
) -> Result<store::HistoryPage, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    store::get_history_page(&dir, limit.unwrap_or(50), before_id).map_err(|e| e.to_string())
}
#[tauri::command]
fn export_history(app: tauri::AppHandle) -> Result<String, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let json = store::export_history_json(&dir).map_err(|e| e.to_string())?;
    let downloads = app.path().download_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&downloads).map_err(|e| e.to_string())?;
    let path = downloads.join(format!("voiceflow-history-{}.json", chrono_like_id()));
    store::write_export_file(&path, &json).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}
#[tauri::command]
fn clear_all_data(app: tauri::AppHandle) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    store::clear_all_data(&dir).map_err(|e| e.to_string())
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
fn repaste_history(id: i64, app: tauri::AppHandle) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let text = store::history_text(&dir, id).map_err(|e| e.to_string())?;
    if text.trim().is_empty() {
        return Err("这条历史记录没有可恢复的文字".into());
    }
    paste::copy(&app, &text).map_err(|e| e.to_string())?;
    let _ = app.emit(
        "dictation://copied",
        serde_json::json!({ "message": "已复制，请手动粘贴" }),
    );
    Ok(())
}
#[tauri::command]
fn delete_history(id: i64, app: tauri::AppHandle) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    store::delete_history(&dir, id).map_err(|e| e.to_string())
}
#[tauri::command]
async fn retry_dictation(
    id: i64,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let path = store::failed_spool(&dir, id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Audio spool is no longer available".to_owned())?;
    let wav = std::fs::read(&path).map_err(|_| "Audio spool is no longer available".to_owned())?;
    let settings = state.settings.lock().unwrap().clone();
    let history_policy = store::history_context(&dir, id).map_err(|e| e.to_string())?;
    let asr_prompt = build_asr_prompt(&settings.dictionary, history_policy.as_ref());
    let mut retry_policy = history_policy.clone().unwrap_or_default();
    if settings.output_mode != "auto" {
        retry_policy.output_mode = Some(settings.output_mode.clone());
    }
    if settings.output_mode == "translation" {
        retry_policy.translation_target_language =
            Some(settings.translation_target_language.clone());
    }
    let provider = state.asr_provider.clone();
    let options = asr::AsrOptions {
        api_key: settings.api_key.clone(),
        language: asr::normalize_language(Some(settings.language.as_str())).map(str::to_owned),
        prompt: asr_prompt,
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
    let snippet_expansion = snippets::resolve_exact(&settings.snippets, &transcript.text);
    let raw_text = transcript.text.clone();
    let cleanup_input = snippet_expansion
        .clone()
        .unwrap_or_else(|| transcript.text.clone());
    let cleanup_decision = if settings.cleanup_enabled && snippet_expansion.is_none() {
        let cleanup_result = {
            let _latency = state.metrics.timer(metrics::MetricKind::Cleanup);
            queue::execute_with_retry(&state.gate, queue::RequestKind::Llm, || {
                llm::cleanup_with_model_and_limits_and_language(
                    &settings.cleanup_model,
                    &cleanup_input,
                    &settings.api_key,
                    &settings.dictionary,
                    None,
                    Some(&retry_policy),
                    Some(settings.language.as_str()),
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
    } else {
        CleanupDecision::Disabled
    };
    let cleanup_status = match &cleanup_decision {
        CleanupDecision::Provider(text) if text.trim().is_empty() => CLEANUP_STATUS_AI_FAILED,
        CleanupDecision::Provider(_) => CLEANUP_STATUS_AI_SUCCESS,
        CleanupDecision::Failed => CLEANUP_STATUS_AI_FAILED,
        CleanupDecision::Disabled if snippet_expansion.is_some() => CLEANUP_STATUS_SNIPPET_BYPASS,
        CleanupDecision::Disabled => CLEANUP_STATUS_LOCAL_ONLY,
    };
    let resolved = finalize_text(&cleanup_input, cleanup_decision)
        .map_err(|_| "No speech detected".to_owned())?;
    let final_text = resolved.text;
    let degraded = resolved.degraded;
    let degraded_reason = resolved.degraded_reason;
    // A retry no longer has a trustworthy original target guard. Never inject
    // into whichever app happens to be active now; copy for an explicit manual
    // paste instead.
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
                manager: Mutex::new(DictationManager {
                    phase: Phase::Idle,
                    started: std::time::Instant::now(),
                    gesture_lock: None,
                    session_generation: 0,
                    cancellation: CancellationToken::new(),
                    recording_context: None,
                }),
                recorder: Arc::new(Mutex::new(audio::Recorder::new())),
                realtime_asr: Mutex::new(None),
                selected_action: Mutex::new(None),
                asr_provider: Arc::new(asr::GroqAsrProvider::default()),
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
                    let accessibility = permissions::request_accessibility();
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
                    let _guard = state.hotkey_gate.lock().await;
                    cancel_internal(&h, &state).await;
                });
            });
            let h = app.handle().clone();
            app.listen("hotkey://toggle", move |_| {
                let h = h.clone();
                tauri::async_runtime::spawn(async move {
                    let state = h.state::<AppState>();
                    {
                        let _guard = state.hotkey_gate.lock().await;
                        if hotkey::is_suspended() {
                            return;
                        }
                    }
                    handle_hotkey_toggle(&h, &state).await;
                });
            });
            let h = app.handle().clone();
            app.listen("hotkey://double_tap", move |_| {
                let h = h.clone();
                tauri::async_runtime::spawn(async move {
                    let state = h.state::<AppState>();
                    {
                        let _guard = state.hotkey_gate.lock().await;
                        if hotkey::is_suspended() {
                            return;
                        }
                    }
                    handle_double_tap_toggle(&h, &state, true).await;
                });
            });
            let h = app.handle().clone();
            app.listen("hotkey://selected-action", move |_| {
                let h = h.clone();
                tauri::async_runtime::spawn(async move {
                    let state = h.state::<AppState>();
                    let _guard = state.hotkey_gate.lock().await;
                    handle_selected_action_hotkey(&h, &state).await;
                });
            });
            let h = app.handle().clone();
            app.listen("audio://error", move |event| {
                let h = h.clone();
                let message = event.payload().to_owned();
                tauri::async_runtime::spawn(async move {
                    let state = h.state::<AppState>();
                    let _guard = state.hotkey_gate.lock().await;
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
            start_dictation,
            stop_dictation,
            cancel_dictation,
            get_settings,
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
            get_usage,
            get_latency_metrics,
            get_history,
            export_history,
            clear_all_data,
            retry_dictation,
            validate_api_key,
            validate_configured_api_key,
            repaste_history,
            delete_history,
            check_permissions,
            get_audio_input_devices,
            get_audio_input_device,
            request_microphone_permission,
            open_privacy_settings,
            request_accessibility_permission,
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
        claim_processing_timeout, context, delivery_fallback_reason, error_completion_is_current,
        error_fallback_reason, finalize_text, long_completion_state,
        processing_completion_is_current, processing_watchdog_delay, read_dictionary_file_contents,
        reset_starting_manager, should_chunk_recording, stop_transition_is_current,
        CleanupDecision, DictationManager, Phase, MAX_DICTIONARY_FILE_BYTES,
    };
    use crate::store;
    use std::fs;
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
        assert_eq!(long_completion_state(true, false), "degraded");
        assert_eq!(long_completion_state(true, true), "degraded");
        assert_eq!(long_completion_state(false, true), "copied");
        assert_eq!(long_completion_state(false, false), "done");
    }

    #[test]
    fn cleanup_failure_preserves_raw_transcript_and_marks_degraded() {
        let result =
            finalize_text("uh deploy v2 /Users/mingjie/app", CleanupDecision::Failed).unwrap();
        assert_eq!(result.text, "uh deploy v2 /Users/mingjie/app");
        assert!(result.degraded);
        assert_eq!(result.degraded_reason, Some("llm_cleanup_failed"));
    }

    #[test]
    fn disabled_cleanup_never_drops_a_filler_only_transcript() {
        let result = finalize_text("嗯 uh", CleanupDecision::Disabled).unwrap();
        assert_eq!(result.text, "嗯 uh");
        assert!(!result.degraded);
    }

    #[test]
    fn provider_cleanup_is_used_when_non_empty() {
        let result = finalize_text("uh hello", CleanupDecision::Provider("hello".into())).unwrap();
        assert_eq!(result.text, "hello");
        assert!(!result.degraded);
    }

    #[test]
    fn empty_provider_cleanup_falls_back_to_raw_and_is_degraded() {
        let result = finalize_text("uh hello", CleanupDecision::Provider("  ".into())).unwrap();
        assert_eq!(result.text, "uh hello");
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
    fn onboarding_delivery_only_targets_the_frontmost_voiceflow_window() {
        let mut snapshot = context::ContextSnapshot::general();
        snapshot.target_guard.pid = 42;
        snapshot.target_guard.bundle_id = Some("com.voiceflow.desktop".into());

        assert!(super::onboarding_delivery_target_matches(
            true,
            &snapshot,
            (42, Some("com.voiceflow.desktop".into()))
        ));
        assert!(!super::onboarding_delivery_target_matches(
            false,
            &snapshot,
            (42, Some("com.voiceflow.desktop".into()))
        ));
        assert!(!super::onboarding_delivery_target_matches(
            true,
            &snapshot,
            (42, Some("com.example.other-app".into()))
        ));
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
        };
        assert!(super::take_gesture_lock(&mut manager));
        assert!(!super::take_gesture_lock(&mut manager));
        manager.gesture_lock = Some(
            std::time::Instant::now()
                - std::time::Duration::from_millis(super::GESTURE_LOCK_MS as u64 + 1),
        );
        assert!(super::take_gesture_lock(&mut manager));
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
}
