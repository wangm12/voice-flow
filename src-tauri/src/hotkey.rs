use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

static HOTKEYS_SUSPENDED: AtomicBool = AtomicBool::new(false);
static HYBRID_RESTART_HANDOFF_PENDING: AtomicBool = AtomicBool::new(false);
static LAST_REGISTERED: Mutex<Option<(String, String)>> = Mutex::new(None);
static CANCEL_REGISTERED: AtomicBool = AtomicBool::new(false);
static REGISTRATION_ERROR: Mutex<Option<String>> = Mutex::new(None);
static SELECTED_ACTION_REGISTERED: Mutex<Option<String>> = Mutex::new(None);
static TRANSLATION_ACTION_REGISTERED: Mutex<Option<String>> = Mutex::new(None);
static VERBATIM_ACTION_REGISTERED: Mutex<Option<String>> = Mutex::new(None);
static SCREEN_ACTION_REGISTERED: Mutex<Option<String>> = Mutex::new(None);

use crate::dictation::{HotkeySource, ModeTrigger};

static GESTURE_ID: AtomicU64 = AtomicU64::new(0);
static COMBO_GESTURES: Mutex<Vec<Arc<Mutex<ComboGesture>>>> = Mutex::new(Vec::new());
#[cfg(test)]
pub(crate) static HYBRID_HANDOFF_TEST_LOCK: Mutex<()> = Mutex::new(());

pub(crate) const HYBRID_HOLD_LOCK: &str = "hybrid_hold_lock";
const HYBRID_HOLD_THRESHOLD: Duration = Duration::from_millis(220);
const HYBRID_DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(320);
pub(crate) const CARBON_RELEASE_FALLBACK_NOTICE: &str = "combo_release_uses_carbon_fallback";

pub(crate) fn set_hybrid_restart_handoff_pending(pending: bool) {
    HYBRID_RESTART_HANDOFF_PENDING.store(pending, Ordering::Release);
}

pub(crate) fn hybrid_restart_handoff_pending() -> bool {
    HYBRID_RESTART_HANDOFF_PENDING.load(Ordering::Acquire)
}

pub(crate) fn next_gesture_id() -> u64 {
    GESTURE_ID.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
}

/// macOS Option+/ emits `÷`. muda only accepts the physical `Slash` key name.
pub(crate) fn canonicalize_hotkey(hotkey: &str) -> String {
    let mut parts: Vec<String> = hotkey
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect();
    if let Some(last) = parts.last_mut() {
        *last = match last.as_str() {
            "÷" | "/" => "Slash".into(),
            "Dead" => "Space".into(),
            "\\" => "Backslash".into(),
            _ => last.clone(),
        };
    }
    parts.join("+")
}

pub(crate) fn bindings_equal(left: &str, right: &str) -> bool {
    if crate::modifier_hotkey::is_fn_only(left) && crate::modifier_hotkey::is_fn_only(right) {
        return true;
    }
    match (
        canonicalize_hotkey(left).parse::<Shortcut>(),
        canonicalize_hotkey(right).parse::<Shortcut>(),
    ) {
        (Ok(left), Ok(right)) => left == right,
        _ => left.trim() == right.trim(),
    }
}

pub(crate) fn mode_action_binding_changed(
    previous_hotkey: &str,
    next_hotkey: &str,
    previous_mode: &str,
    next_mode: &str,
) -> bool {
    previous_mode != next_mode || !bindings_equal(previous_hotkey, next_hotkey)
}

pub(crate) fn reset_pressed_state() {
    for gesture in COMBO_GESTURES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
    {
        gesture
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .state
            .reset();
    }
}

pub fn registration_error() -> Option<String> {
    REGISTRATION_ERROR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

pub fn registration_notice() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let fallback_active = COMBO_GESTURES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|gesture| {
                gesture
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .carbon_release_fallback
            });
        fallback_active.then(|| CARBON_RELEASE_FALLBACK_NOTICE.to_owned())
    }
    #[cfg(not(target_os = "macos"))]
    None
}

fn set_registration_error(error: Option<String>) {
    *REGISTRATION_ERROR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = error;
}

fn clear_auxiliary_registration_tracking() {
    let mut gestures = COMBO_GESTURES.lock().unwrap_or_else(|e| e.into_inner());
    for gesture in gestures.iter() {
        gesture
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .state
            .reset();
    }
    gestures.clear();
    crate::modifier_hotkey::unregister();
    *TRANSLATION_ACTION_REGISTERED
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    *SELECTED_ACTION_REGISTERED
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    *SCREEN_ACTION_REGISTERED
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    *VERBATIM_ACTION_REGISTERED
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
}

pub fn set_suspended(suspended: bool) {
    HOTKEYS_SUSPENDED.store(suspended, Ordering::SeqCst);
    if suspended {
        crate::dictation::invalidate_pending_inputs();
        crate::modifier_hotkey::reset_state();
        reset_pressed_state();
    }
}

pub(crate) async fn resume_after_release() {
    crate::modifier_hotkey::wait_for_key_release().await;
    crate::modifier_hotkey::arm_after_release();
    for slot in COMBO_GESTURES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
    {
        slot.lock().unwrap_or_else(|e| e.into_inner()).state = ModeGesture::default();
    }
    set_suspended(false);
}

pub(crate) fn invalidate_registration_cache() {
    *LAST_REGISTERED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub async fn pause_for_capture(app: &AppHandle) -> Result<(), String> {
    set_suspended(true);
    unregister_cancel(app);
    crate::modifier_hotkey::unregister();
    let dispatcher = app.clone();
    let main_app = app.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    dispatcher
        .run_on_main_thread(move || {
            let result = main_app
                .global_shortcut()
                .unregister_all()
                .map_err(|error| error.to_string());
            if result.is_ok() {
                clear_auxiliary_registration_tracking();
            }
            let _ = tx.send(result);
        })
        .map_err(|error| {
            set_suspended(false);
            format!("hotkey pause dispatch failed: {error}")
        })?;
    rx.await
        .map_err(|_| "hotkey pause dispatch was cancelled".to_owned())??;
    CANCEL_REGISTERED.store(false, Ordering::SeqCst);
    if let Ok(mut last) = LAST_REGISTERED.lock() {
        *last = None;
    }
    Ok(())
}

/// Register a global Escape cancel shortcut **only while a recording is
/// active**. Escape must NOT be registered for the app's whole lifetime, or it
/// would be swallowed even when the user types in other apps. Registration and
/// unregistration are dispatched to the main thread because Carbon hotkey
/// registration off the main runloop throws an NSException and aborts the
/// process.
pub fn register_cancel(app: &AppHandle) {
    if CANCEL_REGISTERED.swap(true, Ordering::SeqCst) {
        return;
    }
    let dispatcher = app.clone();
    let app = app.clone();
    let _ = dispatcher.run_on_main_thread(move || {
        let shortcut: Shortcut = match "Escape".parse() {
            Ok(shortcut) => shortcut,
            Err(error) => {
                CANCEL_REGISTERED.store(false, Ordering::SeqCst);
                log::error!("failed to parse Escape shortcut: {error}");
                return;
            }
        };
        if let Err(error) = app.global_shortcut().on_shortcut(shortcut, |_, _, event| {
            if !is_suspended() && event.state == ShortcutState::Pressed {
                crate::modifier_hotkey::reset_state();
                reset_pressed_state();
                crate::dictation::enqueue_full_cancel();
            }
        }) {
            CANCEL_REGISTERED.store(false, Ordering::SeqCst);
            log::error!("failed to register cancel shortcut: {error}");
        }
    });
}

/// Unregister the Escape shortcut (called when a recording finishes/cancels).
/// Also dispatched to the main thread for the same NSException reason.
pub fn unregister_cancel(app: &AppHandle) {
    if !CANCEL_REGISTERED.swap(false, Ordering::SeqCst) {
        return;
    }
    let dispatcher = app.clone();
    let app = app.clone();
    let _ = dispatcher.run_on_main_thread(move || {
        if let Ok(shortcut) = "Escape".parse::<Shortcut>() {
            let _ = app.global_shortcut().unregister(shortcut);
        }
    });
}

pub fn is_suspended() -> bool {
    HOTKEYS_SUSPENDED.load(Ordering::SeqCst)
}

fn should_dispatch_action_hotkey(suspended: bool, state: ShortcutState) -> bool {
    !suspended && state == ShortcutState::Pressed
}

pub fn register(app: &AppHandle, hotkey: &str, activation_mode: &str) -> Result<(), String> {
    let hotkey = canonicalize_hotkey(hotkey);
    let hotkey = hotkey.as_str();
    let signature = (hotkey.to_string(), activation_mode.to_string());
    if LAST_REGISTERED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        == Some(&signature)
    {
        return Ok(());
    }

    let plugin = app.global_shortcut();
    plugin.unregister_all().map_err(map_register_error(app))?;
    clear_auxiliary_registration_tracking();
    CANCEL_REGISTERED.store(false, Ordering::SeqCst);
    if crate::modifier_hotkey::is_modifier_only(hotkey)
        && !crate::modifier_hotkey::is_fn_only(hotkey)
    {
        *LAST_REGISTERED.lock().unwrap_or_else(|e| e.into_inner()) = Some(signature);
        set_registration_error(Some(
            "旧的单独修饰键已暂停。请选择 Fn 或组合快捷键。".into(),
        ));
        return Ok(());
    }
    let registration_result = if crate::modifier_hotkey::is_fn_only(hotkey) {
        crate::modifier_hotkey::register(app, HotkeySource::Dictation, activation_mode)
    } else {
        register_combo(app, hotkey, activation_mode, HotkeySource::Dictation)
    };
    if let Err(error) = registration_result {
        set_registration_error(Some(error.clone()));
        return Err(error);
    }

    *LAST_REGISTERED.lock().unwrap_or_else(|e| e.into_inner()) = Some(signature);
    set_registration_error(None);
    Ok(())
}

fn map_register_error(app: &AppHandle) -> impl Fn(tauri_plugin_global_shortcut::Error) -> String {
    let app = app.clone();
    move |error| {
        let message = format!("failed to register global hotkey: {error}");
        set_registration_error(Some(message.clone()));
        let _ = app.emit("dictation://error", message.clone());
        log::error!("{message}");
        message
    }
}

async fn register_on_main(app: &AppHandle, hotkey: &str, mode: &str) -> Result<(), String> {
    let hotkey = hotkey.to_string();
    let mode = mode.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let app_for_thread = app.clone();
    let app_for_task = app.clone();
    app_for_thread
        .run_on_main_thread(move || {
            let result = register(&app_for_task, &hotkey, &mode);
            let _ = tx.send(result);
        })
        .map_err(|error| error.to_string())?;
    rx.await
        .map_err(|_| "hotkey register dispatch failed".to_string())?
}

pub async fn apply_settings_hotkey(
    app: &AppHandle,
    hotkey: &str,
    mode: &str,
) -> Result<(), String> {
    let previous = LAST_REGISTERED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let requested = (canonicalize_hotkey(hotkey), mode.to_owned());
    // `register` unregisters the current binding before installing the new
    // one. Clear the cache first so a failed install can actually restore the
    // old binding instead of being short-circuited by the cache.
    if previous.as_ref() != Some(&requested) {
        *LAST_REGISTERED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
    if let Err(error) = register_on_main(app, hotkey, mode).await {
        if let Some((old_hotkey, old_mode)) = previous {
            if let Err(restore_error) = register_on_main(app, &old_hotkey, &old_mode).await {
                log::error!(
                    "failed to restore previous hotkey after registration error: {restore_error}"
                );
            }
        }
        return Err(error);
    }
    // Note: we deliberately do NOT re-register the Escape cancel shortcut here.
    // Escape is only registered while a recording is active (see
    // `register_cancel` callers in the dictation entry path), so applying settings while
    // idle must leave Escape unregistered.
    Ok(())
}

fn unregister_selected_action_on_main(app: &AppHandle) {
    let previous = SELECTED_ACTION_REGISTERED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(previous) = previous {
        if let Ok(shortcut) = previous.parse::<Shortcut>() {
            let _ = app.global_shortcut().unregister(shortcut);
        }
    }
}

/// Register the optional selected-text action shortcut. It deliberately uses
/// a separate global shortcut slot and never calls `unregister_all`, so it
/// cannot replace the ordinary dictation shortcut.
pub fn register_selected_action(app: &AppHandle, hotkey: &str) -> Result<(), String> {
    unregister_selected_action_on_main(app);
    let hotkey = canonicalize_hotkey(hotkey);
    if hotkey.trim().is_empty() {
        return Ok(());
    }
    if crate::modifier_hotkey::is_modifier_only(&hotkey) {
        return Ok(());
    }
    let shortcut: Shortcut = hotkey
        .parse()
        .map_err(|error| format!("invalid selected action hotkey `{hotkey}`: {error}"))?;
    app.global_shortcut()
        .on_shortcut(shortcut, move |app, _, event| {
            if should_dispatch_action_hotkey(is_suspended(), event.state) {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let state = app.state::<crate::AppState>();
                    crate::selected_action::handle_selected_action_hotkey(&app, &state).await;
                });
            }
        })
        .map_err(|error| format!("failed to register selected action hotkey: {error}"))?;
    *SELECTED_ACTION_REGISTERED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(hotkey.to_owned());
    Ok(())
}

fn unregister_screen_action_on_main(app: &AppHandle) {
    let previous = SCREEN_ACTION_REGISTERED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(previous) = previous {
        if let Ok(shortcut) = previous.parse::<Shortcut>() {
            let _ = app.global_shortcut().unregister(shortcut);
        }
    }
}

/// Register the optional look-at-screen shortcut. Empty means off.
pub fn register_screen_action(app: &AppHandle, hotkey: &str) -> Result<(), String> {
    unregister_screen_action_on_main(app);
    let hotkey = canonicalize_hotkey(hotkey);
    if hotkey.trim().is_empty() {
        return Ok(());
    }
    if crate::modifier_hotkey::is_modifier_only(&hotkey) {
        return Ok(());
    }
    let shortcut: Shortcut = hotkey
        .parse()
        .map_err(|error| format!("invalid look-at-screen hotkey `{hotkey}`: {error}"))?;
    app.global_shortcut()
        .on_shortcut(shortcut, move |app, _, event| {
            if should_dispatch_action_hotkey(is_suspended(), event.state) {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let state = app.state::<crate::AppState>();
                    crate::screen_action::handle_screen_action_hotkey(&app, &state).await;
                });
            }
        })
        .map_err(|error| format!("failed to register look-at-screen hotkey: {error}"))?;
    *SCREEN_ACTION_REGISTERED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(hotkey.to_owned());
    Ok(())
}

fn unregister_mode_action_on_main(app: &AppHandle, registered: &Mutex<Option<String>>) {
    let previous = registered
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(previous) = previous {
        if let Ok(shortcut) = previous.parse::<Shortcut>() {
            let _ = app.global_shortcut().unregister(shortcut);
        }
    }
}

fn remove_combo_gestures_for_source(source: HotkeySource) {
    let mut gestures = COMBO_GESTURES.lock().unwrap_or_else(|e| e.into_inner());
    gestures.retain(|slot| {
        let mut gesture = slot.lock().unwrap_or_else(|e| e.into_inner());
        if gesture.source == source {
            gesture.state.reset();
            false
        } else {
            true
        }
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PressKind {
    Tap,
    StartRecording,
    ConfirmLock,
    EndLock,
    Suppressed,
}

#[derive(Clone, Copy)]
struct ActivePress {
    gesture_id: u64,
    started_at: Instant,
    kind: PressKind,
}

#[derive(Clone, Copy)]
struct PendingHybridTap {
    gesture_id: u64,
    deadline: Instant,
    timeout_token: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GestureAction {
    Trigger(ModeTrigger, u64),
    HybridCancel {
        gesture_id: u64,
    },
    RestartAfterCancel {
        cancelled_gesture_id: u64,
        gesture_id: u64,
    },
    HybridTimeout {
        gesture_id: u64,
        timeout_token: u64,
        deadline: Instant,
    },
}

#[derive(Default)]
pub(crate) struct ModeGesture {
    pressed: Option<ActivePress>,
    pending_hybrid_tap: Option<PendingHybridTap>,
    awaiting_hybrid_cancel: Option<u64>,
    locked_gesture_id: Option<u64>,
    blocked_until_release: bool,
    timeout_token: u64,
}

impl ModeGesture {
    fn next_timeout_token(&mut self) -> u64 {
        self.timeout_token = self.timeout_token.wrapping_add(1);
        self.timeout_token
    }

    pub(crate) fn pressed(
        &mut self,
        mode: &str,
        chorded: bool,
        toggle_on_press: bool,
        now: Instant,
    ) -> Vec<GestureAction> {
        if self.pressed.is_some() || self.blocked_until_release {
            return Vec::new();
        }

        let mut actions = Vec::new();
        if let Some(pending) = self.pending_hybrid_tap.take() {
            self.next_timeout_token();
            if mode == HYBRID_HOLD_LOCK && !chorded && now < pending.deadline {
                self.locked_gesture_id = Some(pending.gesture_id);
                self.pressed = Some(ActivePress {
                    gesture_id: pending.gesture_id,
                    started_at: now,
                    kind: PressKind::ConfirmLock,
                });
                return actions;
            }
            if mode == HYBRID_HOLD_LOCK && !chorded {
                let gesture_id = next_gesture_id();
                self.pressed = Some(ActivePress {
                    gesture_id,
                    started_at: now,
                    kind: PressKind::StartRecording,
                });
                actions.push(GestureAction::RestartAfterCancel {
                    cancelled_gesture_id: pending.gesture_id,
                    gesture_id,
                });
                return actions;
            }
            actions.push(GestureAction::Trigger(
                ModeTrigger::Cancel,
                pending.gesture_id,
            ));
        }

        if mode == HYBRID_HOLD_LOCK {
            if let Some(cancelled_gesture_id) = self.awaiting_hybrid_cancel.take() {
                if !chorded {
                    let gesture_id = next_gesture_id();
                    self.pressed = Some(ActivePress {
                        gesture_id,
                        started_at: now,
                        kind: PressKind::StartRecording,
                    });
                    actions.push(GestureAction::RestartAfterCancel {
                        cancelled_gesture_id,
                        gesture_id,
                    });
                    return actions;
                }
                self.pressed = Some(ActivePress {
                    gesture_id: next_gesture_id(),
                    started_at: now,
                    kind: PressKind::Suppressed,
                });
                return actions;
            }
        }

        if mode == HYBRID_HOLD_LOCK {
            if let Some(gesture_id) = self.locked_gesture_id {
                if chorded {
                    self.pressed = Some(ActivePress {
                        gesture_id: next_gesture_id(),
                        started_at: now,
                        kind: PressKind::Suppressed,
                    });
                    return actions;
                }
                self.pressed = Some(ActivePress {
                    gesture_id,
                    started_at: now,
                    kind: PressKind::EndLock,
                });
                return actions;
            }
        }

        let gesture_id = next_gesture_id();
        let kind = if chorded {
            PressKind::Suppressed
        } else if mode == "tap" {
            PressKind::Tap
        } else {
            PressKind::StartRecording
        };
        self.pressed = Some(ActivePress {
            gesture_id,
            started_at: now,
            kind,
        });
        if chorded {
            return actions;
        }

        if mode == "tap" {
            if toggle_on_press {
                actions.push(GestureAction::Trigger(ModeTrigger::Toggle, gesture_id));
            }
        } else {
            actions.push(GestureAction::Trigger(ModeTrigger::Press, gesture_id));
        }
        actions
    }

    pub(crate) fn released(
        &mut self,
        mode: &str,
        toggle_on_release: bool,
        now: Instant,
    ) -> Vec<GestureAction> {
        if self.blocked_until_release {
            self.blocked_until_release = false;
            self.pressed = None;
            return Vec::new();
        }
        let Some(press) = self.pressed.take() else {
            return Vec::new();
        };
        match press.kind {
            PressKind::Tap if mode == "tap" && toggle_on_release => {
                vec![GestureAction::Trigger(
                    ModeTrigger::Toggle,
                    press.gesture_id,
                )]
            }
            PressKind::StartRecording if mode == "hold_to_talk" => {
                vec![GestureAction::Trigger(
                    ModeTrigger::Release,
                    press.gesture_id,
                )]
            }
            PressKind::StartRecording if mode == HYBRID_HOLD_LOCK => {
                if now.saturating_duration_since(press.started_at) >= HYBRID_HOLD_THRESHOLD {
                    vec![GestureAction::Trigger(
                        ModeTrigger::Release,
                        press.gesture_id,
                    )]
                } else {
                    let timeout_token = self.next_timeout_token();
                    let deadline = now + HYBRID_DOUBLE_TAP_WINDOW;
                    self.pending_hybrid_tap = Some(PendingHybridTap {
                        gesture_id: press.gesture_id,
                        deadline,
                        timeout_token,
                    });
                    vec![GestureAction::HybridTimeout {
                        gesture_id: press.gesture_id,
                        timeout_token,
                        deadline,
                    }]
                }
            }
            PressKind::EndLock if mode == HYBRID_HOLD_LOCK => {
                self.locked_gesture_id = None;
                vec![GestureAction::Trigger(
                    ModeTrigger::Release,
                    press.gesture_id,
                )]
            }
            PressKind::ConfirmLock
            | PressKind::EndLock
            | PressKind::Suppressed
            | PressKind::Tap
            | PressKind::StartRecording => Vec::new(),
        }
    }

    pub(crate) fn chorded(&mut self, mode: &str) -> Vec<GestureAction> {
        let Some(press) = self.pressed.as_mut() else {
            return Vec::new();
        };
        if press.kind == PressKind::Suppressed {
            return Vec::new();
        }
        let gesture_id = press.gesture_id;
        let should_cancel = matches!(press.kind, PressKind::StartRecording)
            && matches!(mode, "hold_to_talk" | HYBRID_HOLD_LOCK);
        let cancel_confirmed_lock = press.kind == PressKind::ConfirmLock;
        if cancel_confirmed_lock {
            self.locked_gesture_id = None;
        }
        press.kind = PressKind::Suppressed;
        if should_cancel || cancel_confirmed_lock {
            vec![GestureAction::Trigger(ModeTrigger::Cancel, gesture_id)]
        } else {
            Vec::new()
        }
    }

    pub(crate) fn modifiers_released(&mut self, mode: &str, now: Instant) -> Vec<GestureAction> {
        if self.pressed.is_none() {
            return Vec::new();
        }
        let actions = self.released(mode, false, now);
        if matches!(mode, "hold_to_talk" | HYBRID_HOLD_LOCK) {
            self.blocked_until_release = true;
        }
        actions
    }

    pub(crate) fn expire_hybrid_tap(
        &mut self,
        gesture_id: u64,
        timeout_token: u64,
        now: Instant,
    ) -> Option<GestureAction> {
        let pending = self.pending_hybrid_tap?;
        if pending.gesture_id != gesture_id
            || pending.timeout_token != timeout_token
            || now < pending.deadline
        {
            return None;
        }
        self.pending_hybrid_tap = None;
        self.awaiting_hybrid_cancel = Some(gesture_id);
        self.next_timeout_token();
        Some(GestureAction::HybridCancel { gesture_id })
    }

    pub(crate) fn reset(&mut self) {
        self.blocked_until_release |= self.pressed.take().is_some();
        self.pending_hybrid_tap = None;
        self.awaiting_hybrid_cancel = None;
        self.locked_gesture_id = None;
        self.next_timeout_token();
    }
}

struct ComboGesture {
    state: ModeGesture,
    mode: String,
    source: HotkeySource,
    required_flags: u64,
    carbon_release_fallback: bool,
}

pub(crate) fn dispatch_gesture_triggers(
    source: HotkeySource,
    actions: Vec<GestureAction>,
) -> Vec<GestureAction> {
    let mut timeouts = Vec::new();
    for action in actions {
        match action {
            GestureAction::Trigger(trigger, gesture_id) => {
                crate::dictation::enqueue_hotkey(source, trigger, gesture_id);
            }
            GestureAction::HybridCancel { gesture_id } => {
                crate::dictation::enqueue_hybrid_cancel(source, gesture_id);
            }
            GestureAction::RestartAfterCancel {
                cancelled_gesture_id,
                gesture_id,
            } => crate::dictation::enqueue_hybrid_restart(source, cancelled_gesture_id, gesture_id),
            timeout @ GestureAction::HybridTimeout { .. } => timeouts.push(timeout),
        }
    }
    timeouts
}

fn schedule_combo_hybrid_timeout(gesture: Arc<Mutex<ComboGesture>>, timeout: GestureAction) {
    let GestureAction::HybridTimeout {
        gesture_id,
        timeout_token,
        deadline,
    } = timeout
    else {
        return;
    };
    let source = gesture.lock().unwrap_or_else(|e| e.into_inner()).source;
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(deadline.saturating_duration_since(Instant::now())).await;
        let mut gesture = gesture.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(GestureAction::HybridCancel { gesture_id }) =
            gesture
                .state
                .expire_hybrid_tap(gesture_id, timeout_token, Instant::now())
        {
            crate::dictation::enqueue_hybrid_cancel(source, gesture_id);
        }
    });
}

pub(crate) fn schedule_mode_hybrid_timeout(
    gesture: Arc<Mutex<ModeGesture>>,
    source: HotkeySource,
    timeout: GestureAction,
) {
    let GestureAction::HybridTimeout {
        gesture_id,
        timeout_token,
        deadline,
    } = timeout
    else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(deadline.saturating_duration_since(Instant::now())).await;
        let mut gesture = gesture.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(GestureAction::HybridCancel { gesture_id }) =
            gesture.expire_hybrid_tap(gesture_id, timeout_token, Instant::now())
        {
            crate::dictation::enqueue_hybrid_cancel(source, gesture_id);
        }
    });
}

fn mode_requires_modifier_observation(mode: &str) -> bool {
    matches!(mode, "hold_to_talk" | HYBRID_HOLD_LOCK)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModifierObservationPlan {
    NotNeeded,
    Observe,
    CarbonFallback,
}

fn modifier_observation_plan(mode: &str, permission_available: bool) -> ModifierObservationPlan {
    if !mode_requires_modifier_observation(mode) {
        ModifierObservationPlan::NotNeeded
    } else if permission_available {
        ModifierObservationPlan::Observe
    } else {
        ModifierObservationPlan::CarbonFallback
    }
}

#[cfg(any(target_os = "macos", test))]
fn modifier_listener_error_uses_carbon_fallback(error: &str) -> bool {
    matches!(
        error,
        "input_monitoring_required" | "modifier_event_tap_unavailable"
    )
}

fn required_flags(shortcut: &Shortcut) -> u64 {
    let mut flags = 0;
    for (modifier, flag) in [
        (Modifiers::SHIFT, 1 << 17),
        (Modifiers::CONTROL, 1 << 18),
        (Modifiers::ALT, 1 << 19),
        (Modifiers::SUPER, 1 << 20),
    ] {
        if shortcut.mods.contains(modifier) {
            flags |= flag;
        }
    }
    flags
}

// Observe modifier state only. Releasing either part of a held combination
// ends its own recording even if Carbon delays HotKeyReleased until main-key up.
pub(crate) fn modifiers_changed(flags: u64) {
    let gestures = COMBO_GESTURES.lock().unwrap_or_else(|e| e.into_inner());
    for slot in gestures.iter() {
        let (source, actions) = {
            let mut gesture = slot.lock().unwrap_or_else(|e| e.into_inner());
            if gesture.carbon_release_fallback
                || !mode_requires_modifier_observation(&gesture.mode)
                || flags & gesture.required_flags == gesture.required_flags
            {
                continue;
            }
            let mode = gesture.mode.clone();
            let source = gesture.source;
            (
                source,
                gesture.state.modifiers_released(&mode, Instant::now()),
            )
        };
        for timeout in dispatch_gesture_triggers(source, actions) {
            schedule_combo_hybrid_timeout(slot.clone(), timeout);
        }
    }
}

fn register_combo(
    app: &AppHandle,
    hotkey: &str,
    mode: &str,
    source: HotkeySource,
) -> Result<(), String> {
    let shortcut: Shortcut = hotkey
        .parse()
        .map_err(|error| format!("invalid hotkey `{hotkey}`: {error}"))?;
    let modifier_observation_enabled = match modifier_observation_plan(
        mode,
        crate::modifier_hotkey::input_monitoring_is_allowed(),
    ) {
        ModifierObservationPlan::NotNeeded | ModifierObservationPlan::CarbonFallback => false,
        ModifierObservationPlan::Observe => {
            #[cfg(target_os = "macos")]
            match crate::modifier_hotkey::ensure_listener(app.clone()) {
                Ok(()) => true,
                Err(error) if modifier_listener_error_uses_carbon_fallback(&error) => false,
                Err(error) => return Err(error),
            }
            #[cfg(not(target_os = "macos"))]
            false
        }
    };
    let carbon_release_fallback = cfg!(target_os = "macos")
        && mode_requires_modifier_observation(mode)
        && !modifier_observation_enabled;
    let gesture = Arc::new(Mutex::new(ComboGesture {
        state: ModeGesture::default(),
        mode: mode.into(),
        source,
        required_flags: required_flags(&shortcut),
        carbon_release_fallback,
    }));
    let callback_gesture = gesture.clone();
    app.global_shortcut()
        .on_shortcut(shortcut, move |_, _, event| {
            if is_suspended() || crate::modifier_hotkey::is_paste_suppressed() {
                return;
            }
            let actions = {
                let mut gesture = callback_gesture.lock().unwrap_or_else(|e| e.into_inner());
                let mode = gesture.mode.clone();
                match event.state {
                    ShortcutState::Pressed => {
                        gesture.state.pressed(&mode, false, true, Instant::now())
                    }
                    ShortcutState::Released => gesture.state.released(&mode, false, Instant::now()),
                }
            };
            for timeout in dispatch_gesture_triggers(source, actions) {
                schedule_combo_hybrid_timeout(callback_gesture.clone(), timeout);
            }
        })
        .map_err(map_register_error(app))?;
    COMBO_GESTURES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(gesture);
    Ok(())
}

/// Register the optional verbatim shortcut. Empty means off. Uses the same
/// activation semantics as the main dictation hotkey but never calls
/// `unregister_all`.
pub fn register_verbatim_action(
    app: &AppHandle,
    hotkey: &str,
    activation_mode: &str,
) -> Result<(), String> {
    register_mode_action(
        app,
        hotkey,
        activation_mode,
        HotkeySource::Verbatim,
        &VERBATIM_ACTION_REGISTERED,
    )
}

pub fn register_translation_action(
    app: &AppHandle,
    hotkey: &str,
    activation_mode: &str,
) -> Result<(), String> {
    register_mode_action(
        app,
        hotkey,
        activation_mode,
        HotkeySource::Translation,
        &TRANSLATION_ACTION_REGISTERED,
    )
}

fn register_mode_action(
    app: &AppHandle,
    hotkey: &str,
    activation_mode: &str,
    source: HotkeySource,
    registered: &Mutex<Option<String>>,
) -> Result<(), String> {
    unregister_mode_action_on_main(app, registered);
    crate::modifier_hotkey::unregister_source(source);
    remove_combo_gestures_for_source(source);
    let hotkey = canonicalize_hotkey(hotkey);
    if hotkey.trim().is_empty() {
        return Ok(());
    }
    if crate::modifier_hotkey::is_fn_only(&hotkey) {
        crate::modifier_hotkey::register(app, source, activation_mode)?;
    } else if !crate::modifier_hotkey::is_modifier_only(&hotkey) {
        register_combo(app, &hotkey, activation_mode, source)?;
    }
    *registered.lock().unwrap_or_else(|e| e.into_inner()) = Some(hotkey);
    Ok(())
}

pub async fn apply_verbatim_action_hotkey(
    app: &AppHandle,
    hotkey: &str,
    activation_mode: &str,
) -> Result<(), String> {
    apply_mode_action_hotkey(app, hotkey, activation_mode, register_verbatim_action).await
}

pub async fn apply_translation_action_hotkey(
    app: &AppHandle,
    hotkey: &str,
    activation_mode: &str,
) -> Result<(), String> {
    apply_mode_action_hotkey(app, hotkey, activation_mode, register_translation_action).await
}

async fn apply_mode_action_hotkey(
    app: &AppHandle,
    hotkey: &str,
    activation_mode: &str,
    register: fn(&AppHandle, &str, &str) -> Result<(), String>,
) -> Result<(), String> {
    let hotkey = hotkey.to_owned();
    let mode = activation_mode.to_owned();
    let dispatcher = app.clone();
    let app_for_thread = app.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    dispatcher
        .run_on_main_thread(move || {
            let result = register(&app_for_thread, &hotkey, &mode);
            let _ = tx.send(result);
        })
        .map_err(|error| format!("mode hotkey dispatch failed: {error}"))?;
    rx.await
        .map_err(|_| "mode hotkey dispatch was cancelled".to_owned())?
}

pub async fn apply_screen_action_hotkey(app: &AppHandle, hotkey: &str) -> Result<(), String> {
    let hotkey = hotkey.to_owned();
    let dispatcher = app.clone();
    let app_for_thread = app.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    dispatcher
        .run_on_main_thread(move || {
            let result = register_screen_action(&app_for_thread, &hotkey);
            let _ = tx.send(result);
        })
        .map_err(|error| format!("look-at-screen hotkey dispatch failed: {error}"))?;
    rx.await
        .map_err(|_| "look-at-screen hotkey dispatch was cancelled".to_owned())?
}

pub async fn apply_selected_action_hotkey(
    app: &AppHandle,
    hotkey: &str,
    enabled: bool,
) -> Result<(), String> {
    let hotkey = if enabled {
        hotkey.to_owned()
    } else {
        String::new()
    };
    let dispatcher = app.clone();
    let app_for_thread = app.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    dispatcher
        .run_on_main_thread(move || {
            let result = register_selected_action(&app_for_thread, &hotkey);
            let _ = tx.send(result);
        })
        .map_err(|error| format!("selected action hotkey dispatch failed: {error}"))?;
    rx.await
        .map_err(|_| "selected action hotkey dispatch was cancelled".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;

    struct HandoffFlagGuard;

    impl Drop for HandoffFlagGuard {
        fn drop(&mut self) {
            set_hybrid_restart_handoff_pending(false);
        }
    }

    struct TestComboSlot(Arc<Mutex<ComboGesture>>);

    impl Drop for TestComboSlot {
        fn drop(&mut self) {
            COMBO_GESTURES
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|slot| !Arc::ptr_eq(slot, &self.0));
        }
    }

    #[test]
    fn selected_and_screen_action_shortcuts_dispatch_only_unsuspended_presses() {
        assert!(should_dispatch_action_hotkey(false, ShortcutState::Pressed));
        assert!(!should_dispatch_action_hotkey(
            false,
            ShortcutState::Released
        ));
        assert!(!should_dispatch_action_hotkey(true, ShortcutState::Pressed));
        assert!(!should_dispatch_action_hotkey(
            true,
            ShortcutState::Released
        ));
    }

    #[test]
    fn option_slash_is_canonicalized() {
        assert_eq!(
            canonicalize_hotkey("CmdOrControl+Alt+÷"),
            "CmdOrControl+Alt+Slash"
        );
        assert!(canonicalize_hotkey("CmdOrControl+Alt+÷")
            .parse::<Shortcut>()
            .is_ok());
    }

    fn at(start: Instant, milliseconds: u64) -> Instant {
        start + Duration::from_millis(milliseconds)
    }

    fn trigger(action: GestureAction) -> (ModeTrigger, u64) {
        match action {
            GestureAction::Trigger(trigger, gesture_id) => (trigger, gesture_id),
            _ => panic!("expected a trigger"),
        }
    }

    #[test]
    fn legacy_tap_toggles_on_press_and_deduplicates_repeat() {
        let now = Instant::now();
        let mut gesture = ModeGesture::default();
        let first = gesture.pressed("tap", false, true, now);
        let (first_trigger, first_id) = trigger(first[0]);
        assert_eq!(first_trigger, ModeTrigger::Toggle);
        assert!(gesture.pressed("tap", false, true, at(now, 1)).is_empty());
        assert!(gesture.released("tap", false, at(now, 2)).is_empty());
        let second = gesture.pressed("tap", false, true, at(now, 3));
        assert_eq!(trigger(second[0]).0, ModeTrigger::Toggle);
        assert_ne!(first_id, trigger(second[0]).1);
    }

    #[test]
    fn fn_tap_keeps_its_release_trigger_and_does_not_toggle_chords() {
        let now = Instant::now();
        let mut gesture = ModeGesture::default();
        assert!(gesture.pressed("tap", false, false, now).is_empty());
        let toggled = gesture.released("tap", true, at(now, 1));
        assert_eq!(trigger(toggled[0]).0, ModeTrigger::Toggle);

        assert!(gesture.pressed("tap", true, false, at(now, 2)).is_empty());
        assert!(gesture.released("tap", true, at(now, 3)).is_empty());
    }

    #[test]
    fn legacy_hold_starts_on_press_and_stops_on_release() {
        let now = Instant::now();
        let mut gesture = ModeGesture::default();
        assert!(gesture.released("hold_to_talk", false, now).is_empty());
        let started = gesture.pressed("hold_to_talk", false, true, at(now, 1));
        let (start_trigger, id) = trigger(started[0]);
        assert_eq!(start_trigger, ModeTrigger::Press);
        assert!(gesture
            .pressed("hold_to_talk", false, true, at(now, 2))
            .is_empty());
        let ended = gesture.released("hold_to_talk", false, at(now, 3));
        assert_eq!(trigger(ended[0]), (ModeTrigger::Release, id));
    }

    #[test]
    fn modifier_first_release_ends_hold_once_and_waits_for_main_key_up() {
        let now = Instant::now();
        let mut gesture = ModeGesture::default();
        let started = gesture.pressed("hold_to_talk", false, true, now);
        let id = trigger(started[0]).1;
        let ended = gesture.modifiers_released("hold_to_talk", at(now, 20));
        assert_eq!(trigger(ended[0]), (ModeTrigger::Release, id));
        assert!(gesture
            .pressed("hold_to_talk", false, true, at(now, 21))
            .is_empty());
        assert!(gesture
            .released("hold_to_talk", false, at(now, 22))
            .is_empty());
        assert!(!gesture
            .pressed("hold_to_talk", false, true, at(now, 23))
            .is_empty());
    }

    #[test]
    fn hybrid_hold_releases_after_the_220ms_threshold() {
        let now = Instant::now();
        let mut gesture = ModeGesture::default();
        let started = gesture.pressed(HYBRID_HOLD_LOCK, false, true, now);
        let id = trigger(started[0]).1;
        let ended = gesture.released(HYBRID_HOLD_LOCK, false, at(now, 220));
        assert_eq!(trigger(ended[0]), (ModeTrigger::Release, id));
    }

    #[test]
    fn one_short_hybrid_tap_is_cancelled_after_the_double_tap_window() {
        let now = Instant::now();
        let mut gesture = ModeGesture::default();
        let started = gesture.pressed(HYBRID_HOLD_LOCK, false, true, now);
        let id = trigger(started[0]).1;
        let timeout = gesture.released(HYBRID_HOLD_LOCK, false, at(now, 100));
        let GestureAction::HybridTimeout {
            gesture_id,
            timeout_token,
            deadline,
        } = timeout[0]
        else {
            panic!("short release must wait for a second press");
        };
        assert_eq!(gesture_id, id);
        assert_eq!(deadline, at(now, 420));
        assert_eq!(
            gesture.expire_hybrid_tap(gesture_id, timeout_token, at(now, 419)),
            None
        );
        assert_eq!(
            gesture.expire_hybrid_tap(gesture_id, timeout_token, at(now, 420)),
            Some(GestureAction::HybridCancel { gesture_id: id })
        );
        assert_eq!(
            gesture.expire_hybrid_tap(gesture_id, timeout_token, at(now, 500)),
            None
        );
    }

    #[test]
    fn hybrid_short_press_after_deadline_restarts_after_the_prior_cancel() {
        let now = Instant::now();
        let mut gesture = ModeGesture::default();
        let started = gesture.pressed(HYBRID_HOLD_LOCK, false, true, now);
        let id = trigger(started[0]).1;
        let timeout = gesture.released(HYBRID_HOLD_LOCK, false, at(now, 80));
        let GestureAction::HybridTimeout {
            gesture_id,
            timeout_token,
            deadline,
        } = timeout[0]
        else {
            panic!("first short release must be pending");
        };
        let second_press = gesture.pressed(HYBRID_HOLD_LOCK, false, true, at(now, 400));
        let GestureAction::RestartAfterCancel {
            cancelled_gesture_id,
            gesture_id: next_id,
        } = second_press[0]
        else {
            panic!("a press at the 320ms deadline must wait for cancellation");
        };
        assert_eq!(cancelled_gesture_id, id);
        assert_eq!(
            gesture.expire_hybrid_tap(gesture_id, timeout_token, deadline),
            None,
            "confirming the lock invalidates the old timer"
        );

        let ended = gesture.released(HYBRID_HOLD_LOCK, false, at(now, 620));
        assert_eq!(trigger(ended[0]), (ModeTrigger::Release, next_id));
    }

    #[test]
    fn hybrid_timer_first_press_after_deadline_attaches_to_the_inflight_cancel() {
        let now = Instant::now();
        let mut gesture = ModeGesture::default();
        let old_id = trigger(gesture.pressed(HYBRID_HOLD_LOCK, false, true, now)[0]).1;
        let timeout = gesture.released(HYBRID_HOLD_LOCK, false, at(now, 40));
        let GestureAction::HybridTimeout {
            gesture_id,
            timeout_token,
            deadline,
        } = timeout[0]
        else {
            panic!("a short press should schedule its cancel");
        };
        assert_eq!(deadline, at(now, 360));
        assert_eq!(
            gesture.expire_hybrid_tap(gesture_id, timeout_token, deadline),
            Some(GestureAction::HybridCancel { gesture_id: old_id })
        );

        let next = gesture.pressed(HYBRID_HOLD_LOCK, false, true, at(now, 361));
        let GestureAction::RestartAfterCancel {
            cancelled_gesture_id,
            gesture_id: next_id,
        } = next[0]
        else {
            panic!("a timer-first press must attach to the same cancel handoff");
        };
        assert_eq!(cancelled_gesture_id, old_id);
        let released = gesture.released(HYBRID_HOLD_LOCK, false, at(now, 581));
        assert_eq!(trigger(released[0]), (ModeTrigger::Release, next_id));
    }

    #[test]
    fn chord_after_confirming_hybrid_lock_cancels_and_clears_original_owner() {
        let now = Instant::now();
        let mut gesture = ModeGesture::default();
        let started = gesture.pressed(HYBRID_HOLD_LOCK, false, true, now);
        let id = trigger(started[0]).1;
        gesture.released(HYBRID_HOLD_LOCK, false, at(now, 60));
        gesture.pressed(HYBRID_HOLD_LOCK, false, true, at(now, 100));

        let chord = gesture.chorded(HYBRID_HOLD_LOCK);
        assert_eq!(trigger(chord[0]), (ModeTrigger::Cancel, id));
        assert!(gesture
            .released(HYBRID_HOLD_LOCK, false, at(now, 110))
            .is_empty());
        let next = gesture.pressed(HYBRID_HOLD_LOCK, false, true, at(now, 200));
        assert_eq!(trigger(next[0]).0, ModeTrigger::Press);
        assert_ne!(trigger(next[0]).1, id);
    }

    #[test]
    fn chord_during_hybrid_lock_end_does_not_toggle_off_the_latched_recording() {
        let now = Instant::now();
        let mut gesture = ModeGesture::default();
        let id = trigger(gesture.pressed(HYBRID_HOLD_LOCK, false, true, now)[0]).1;
        gesture.released(HYBRID_HOLD_LOCK, false, at(now, 50));
        gesture.pressed(HYBRID_HOLD_LOCK, false, true, at(now, 80));
        gesture.released(HYBRID_HOLD_LOCK, false, at(now, 90));

        assert!(gesture
            .pressed(HYBRID_HOLD_LOCK, false, true, at(now, 200))
            .is_empty());
        assert!(gesture.chorded(HYBRID_HOLD_LOCK).is_empty());
        assert!(gesture
            .released(HYBRID_HOLD_LOCK, false, at(now, 210))
            .is_empty());
        assert!(gesture
            .pressed(HYBRID_HOLD_LOCK, false, true, at(now, 300))
            .is_empty());
        let ended = gesture.released(HYBRID_HOLD_LOCK, false, at(now, 310));
        assert_eq!(trigger(ended[0]), (ModeTrigger::Release, id));
    }

    #[test]
    fn hybrid_modifier_release_uses_same_id_and_waits_for_carbon_key_release() {
        let now = Instant::now();
        let mut gesture = ModeGesture::default();
        let id = trigger(gesture.pressed(HYBRID_HOLD_LOCK, false, true, now)[0]).1;
        let pending = gesture.modifiers_released(HYBRID_HOLD_LOCK, at(now, 100));
        let GestureAction::HybridTimeout { gesture_id, .. } = pending[0] else {
            panic!("short modifier release should await the second tap");
        };
        assert_eq!(gesture_id, id);
        assert!(gesture
            .pressed(HYBRID_HOLD_LOCK, false, true, at(now, 120))
            .is_empty());
        assert!(gesture
            .released(HYBRID_HOLD_LOCK, false, at(now, 130))
            .is_empty());
    }

    #[test]
    fn reset_cancels_timeout_and_requires_release_before_a_new_press() {
        let now = Instant::now();
        let mut gesture = ModeGesture::default();
        let started = gesture.pressed(HYBRID_HOLD_LOCK, false, true, now);
        let id = trigger(started[0]).1;
        let pending = gesture.released(HYBRID_HOLD_LOCK, false, at(now, 40));
        let GestureAction::HybridTimeout {
            gesture_id,
            timeout_token,
            deadline,
        } = pending[0]
        else {
            panic!("short release should be pending");
        };
        gesture.reset();
        assert_eq!(
            gesture.expire_hybrid_tap(gesture_id, timeout_token, deadline),
            None
        );
        let next = gesture.pressed(HYBRID_HOLD_LOCK, false, true, at(now, 100));
        assert_eq!(trigger(next[0]).0, ModeTrigger::Press);
        assert_ne!(trigger(next[0]).1, id);

        gesture.reset();
        assert!(gesture
            .pressed(HYBRID_HOLD_LOCK, false, true, at(now, 110))
            .is_empty());
        assert!(gesture
            .released(HYBRID_HOLD_LOCK, false, at(now, 120))
            .is_empty());
        assert_eq!(
            trigger(gesture.pressed(HYBRID_HOLD_LOCK, false, true, at(now, 130))[0]).0,
            ModeTrigger::Press
        );
    }

    #[test]
    fn hold_gesture_selects_modifier_observation_or_carbon_fallback() {
        assert_eq!(
            modifier_observation_plan("tap", false),
            ModifierObservationPlan::NotNeeded
        );
        assert_eq!(
            modifier_observation_plan("hold_to_talk", true),
            ModifierObservationPlan::Observe
        );
        assert_eq!(
            modifier_observation_plan(HYBRID_HOLD_LOCK, false),
            ModifierObservationPlan::CarbonFallback
        );
        assert!(modifier_listener_error_uses_carbon_fallback(
            "modifier_event_tap_unavailable"
        ));
        assert!(!modifier_listener_error_uses_carbon_fallback(
            "unexpected_runloop_failure"
        ));
    }

    #[test]
    fn equivalent_bindings_include_fn_aliases_and_modifier_aliases() {
        assert!(bindings_equal("Fn", "Globe"));
        assert!(bindings_equal("CmdOrControl+Alt+Space", "Super+Alt+Space"));
        assert!(!bindings_equal("Fn", "Shift"));
    }

    #[test]
    fn mode_action_registration_changes_only_with_its_binding_or_recording_mode() {
        assert!(!mode_action_binding_changed(
            "CmdOrControl+Alt+Space",
            "Super+Alt+Space",
            HYBRID_HOLD_LOCK,
            HYBRID_HOLD_LOCK,
        ));
        assert!(!mode_action_binding_changed("Fn", "Globe", "tap", "tap",));
        assert!(mode_action_binding_changed(
            "CmdOrControl+Alt+Space",
            "CmdOrControl+Alt+Enter",
            HYBRID_HOLD_LOCK,
            HYBRID_HOLD_LOCK,
        ));
        assert!(mode_action_binding_changed(
            "CmdOrControl+Alt+Space",
            "CmdOrControl+Alt+Space",
            "tap",
            HYBRID_HOLD_LOCK,
        ));
    }

    #[test]
    fn idle_phase_preserves_a_held_combo_during_hybrid_restart_handoff() {
        let _handoff_test_lock = HYBRID_HANDOFF_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let slot = Arc::new(Mutex::new(ComboGesture {
            state: ModeGesture::default(),
            mode: HYBRID_HOLD_LOCK.into(),
            source: HotkeySource::Verbatim,
            required_flags: 0,
            carbon_release_fallback: false,
        }));
        COMBO_GESTURES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(slot.clone());
        let _remove_slot = TestComboSlot(slot.clone());
        let _clear_handoff_flag = HandoffFlagGuard;

        let pressed = slot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .state
            .pressed(HYBRID_HOLD_LOCK, false, true, now);
        let (start_trigger, gesture_id) = trigger(pressed[0]);
        assert_eq!(start_trigger, ModeTrigger::Press);

        set_hybrid_restart_handoff_pending(true);
        crate::modifier_hotkey::set_dictation_active(false);
        let released = slot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .state
            .released(HYBRID_HOLD_LOCK, false, now + HYBRID_HOLD_THRESHOLD);

        assert_eq!(
            released,
            vec![GestureAction::Trigger(ModeTrigger::Release, gesture_id)]
        );
    }
}
