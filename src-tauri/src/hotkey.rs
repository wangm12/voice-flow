use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Emitter};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

static HOTKEYS_SUSPENDED: AtomicBool = AtomicBool::new(false);
static LAST_REGISTERED: Mutex<Option<(String, String)>> = Mutex::new(None);
static CANCEL_REGISTERED: AtomicBool = AtomicBool::new(false);
static REGISTRATION_ERROR: Mutex<Option<String>> = Mutex::new(None);
static SELECTED_ACTION_REGISTERED: Mutex<Option<String>> = Mutex::new(None);

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

pub fn set_suspended(suspended: bool) {
    HOTKEYS_SUSPENDED.store(suspended, Ordering::SeqCst);
    if suspended {
        crate::modifier_hotkey::reset_state();
    }
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
            let _ = main_app.global_shortcut().unregister_all();
            let _ = tx.send(());
        })
        .map_err(|error| {
            set_suspended(false);
            format!("hotkey pause dispatch failed: {error}")
        })?;
    if rx.await.is_err() {
        set_suspended(false);
        return Err("hotkey pause dispatch was cancelled".to_owned());
    }
    CANCEL_REGISTERED.store(false, Ordering::SeqCst);
    if let Ok(mut last) = LAST_REGISTERED.lock() {
        *last = None;
    }
    Ok(())
}

#[allow(dead_code)]
pub fn unregister_all(app: &AppHandle) {
    unregister_cancel(app);
    crate::modifier_hotkey::unregister();
    let _ = app.global_shortcut().unregister_all();
    CANCEL_REGISTERED.store(false, Ordering::SeqCst);
    if let Ok(mut last) = LAST_REGISTERED.lock() {
        *last = None;
    }
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
        if let Err(error) = app
            .global_shortcut()
            .on_shortcut(shortcut, |app, _, event| {
                if !is_suspended() && event.state == ShortcutState::Pressed {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = app.emit("hotkey://cancel", ());
                    });
                }
            })
        {
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
    let signature = (hotkey.to_string(), activation_mode.to_string());
    if LAST_REGISTERED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        == Some(&signature)
    {
        return Ok(());
    }

    if crate::modifier_hotkey::is_modifier_only(hotkey) {
        let plugin = app.global_shortcut();
        let _ = plugin.unregister_all();
        CANCEL_REGISTERED.store(false, Ordering::SeqCst);
        if let Err(error) = crate::modifier_hotkey::register(app, hotkey, activation_mode) {
            set_registration_error(Some(error.clone()));
            let _ = app.emit("dictation://error", error.clone());
            return Err(error);
        }
    } else {
        crate::modifier_hotkey::unregister();
        let plugin = app.global_shortcut();
        let _ = plugin.unregister_all();
        CANCEL_REGISTERED.store(false, Ordering::SeqCst);

        let shortcut: Shortcut = match hotkey.parse() {
            Ok(shortcut) => shortcut,
            Err(error) => {
                let message = format!("invalid hotkey `{hotkey}`: {error}");
                set_registration_error(Some(message.clone()));
                return Err(message);
            }
        };

        plugin
            .on_shortcut(shortcut, move |app, _, event| {
                if !is_suspended() && event.state == ShortcutState::Pressed {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = app.emit("hotkey://toggle", ());
                    });
                }
            })
            .map_err(map_register_error(app))?;
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
    let requested = (hotkey.to_owned(), mode.to_owned());
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
    if hotkey.trim().is_empty() {
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
