use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

static HOTKEYS_SUSPENDED: AtomicBool = AtomicBool::new(false);
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

fn set_registration_error(error: Option<String>) {
    *REGISTRATION_ERROR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = error;
}

fn clear_auxiliary_registration_tracking() {
    COMBO_GESTURES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
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
                crate::dictation::interrupt_hotkeys();
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
    if crate::modifier_hotkey::is_fn_only(hotkey) {
        crate::modifier_hotkey::register(app, HotkeySource::Dictation, activation_mode)?;
    } else {
        register_combo(app, hotkey, activation_mode, HotkeySource::Dictation)?;
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
    // `register_cancel` callers in start_internal), so applying settings while
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
            if !is_suspended() && event.state == ShortcutState::Pressed {
                let _ = app.emit("hotkey://selected-action", ());
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
            if !is_suspended() && event.state == ShortcutState::Pressed {
                let _ = app.emit("hotkey://screen-action", ());
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

#[derive(Default)]
struct ModeGesture {
    pressed: Option<u64>,
    blocked_until_release: bool,
}
impl ModeGesture {
    fn event(&mut self, mode: &str, state: ShortcutState) -> Option<(ModeTrigger, u64)> {
        if state == ShortcutState::Released {
            self.blocked_until_release = false;
            let id = self.pressed.take()?;
            return (mode == "hold_to_talk").then_some((ModeTrigger::Release, id));
        }
        if self.pressed.is_some() || self.blocked_until_release {
            return None;
        }
        let id = next_gesture_id();
        self.pressed = Some(id);
        Some((
            if mode == "hold_to_talk" {
                ModeTrigger::Press
            } else {
                ModeTrigger::Toggle
            },
            id,
        ))
    }

    fn reset(&mut self) {
        self.blocked_until_release |= self.pressed.take().is_some();
    }

    fn modifiers_released(&mut self, mode: &str) -> Option<(ModeTrigger, u64)> {
        let id = self.pressed.take()?;
        self.blocked_until_release = true;
        (mode == "hold_to_talk").then_some((ModeTrigger::Release, id))
    }
}

struct ComboGesture {
    state: ModeGesture,
    mode: String,
    source: HotkeySource,
    required_flags: u64,
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
    for slot in COMBO_GESTURES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
    {
        let mut gesture = slot.lock().unwrap_or_else(|e| e.into_inner());
        if flags & gesture.required_flags != gesture.required_flags {
            let mode = gesture.mode.clone();
            if let Some((trigger, id)) = gesture.state.modifiers_released(&mode) {
                crate::dictation::enqueue_hotkey(gesture.source, trigger, id);
            }
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
    if mode == "hold_to_talk" {
        crate::modifier_hotkey::ensure_listener(app.clone())?;
    }
    let gesture = Arc::new(Mutex::new(ComboGesture {
        state: ModeGesture::default(),
        mode: mode.into(),
        source,
        required_flags: required_flags(&shortcut),
    }));
    let callback_gesture = gesture.clone();
    app.global_shortcut()
        .on_shortcut(shortcut, move |_, _, event| {
            if is_suspended() || crate::modifier_hotkey::is_paste_suppressed() {
                return;
            }
            let mut gesture = callback_gesture.lock().unwrap_or_else(|e| e.into_inner());
            let mode = gesture.mode.clone();
            if let Some((trigger, id)) = gesture.state.event(&mode, event.state) {
                crate::dictation::enqueue_hotkey(source, trigger, id);
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
    COMBO_GESTURES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|slot| slot.lock().unwrap_or_else(|e| e.into_inner()).source != source);
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
    #[test]
    fn tap_deduplicates_repeat_but_accepts_a_second_real_press_immediately() {
        let mut gesture = ModeGesture::default();
        let first = gesture.event("tap", ShortcutState::Pressed).unwrap();
        assert_eq!(first.0, ModeTrigger::Toggle);
        assert!(gesture.event("tap", ShortcutState::Pressed).is_none());
        assert!(gesture.event("tap", ShortcutState::Released).is_none());
        let second = gesture.event("tap", ShortcutState::Pressed).unwrap();
        assert_eq!(second.0, ModeTrigger::Toggle);
        assert_ne!(first.1, second.1);
    }
    #[test]
    fn hold_release_always_ends_the_matching_press() {
        let mut gesture = ModeGesture::default();
        assert!(gesture
            .event("hold_to_talk", ShortcutState::Released)
            .is_none());
        let (trigger, id) = gesture
            .event("hold_to_talk", ShortcutState::Pressed)
            .unwrap();
        assert_eq!(trigger, ModeTrigger::Press);
        assert!(gesture
            .event("hold_to_talk", ShortcutState::Pressed)
            .is_none());
        assert_eq!(
            gesture.event("hold_to_talk", ShortcutState::Released),
            Some((ModeTrigger::Release, id))
        );
    }
    #[test]
    fn modifier_first_release_ends_hold_once_and_waits_for_main_key_up() {
        let mut gesture = ModeGesture::default();
        let (_, id) = gesture
            .event("hold_to_talk", ShortcutState::Pressed)
            .unwrap();
        assert_eq!(
            gesture.modifiers_released("hold_to_talk"),
            Some((ModeTrigger::Release, id))
        );
        assert!(gesture
            .event("hold_to_talk", ShortcutState::Pressed)
            .is_none());
        assert!(gesture
            .event("hold_to_talk", ShortcutState::Released)
            .is_none());
        assert!(gesture
            .event("hold_to_talk", ShortcutState::Pressed)
            .is_some());
    }
    #[test]
    fn cancel_does_not_allow_a_held_key_repeat_to_start_another_session() {
        let mut gesture = ModeGesture::default();
        gesture.event("tap", ShortcutState::Pressed);
        gesture.reset();
        assert!(gesture.event("tap", ShortcutState::Pressed).is_none());
        gesture.event("tap", ShortcutState::Released);
        assert!(gesture.event("tap", ShortcutState::Pressed).is_some());
    }
    #[test]
    fn equivalent_bindings_include_fn_aliases_and_modifier_aliases() {
        assert!(bindings_equal("Fn", "Globe"));
        assert!(bindings_equal("CmdOrControl+Alt+Space", "Super+Alt+Space"));
        assert!(!bindings_equal("Fn", "Shift"));
    }
}
