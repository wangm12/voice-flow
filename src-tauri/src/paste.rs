#[cfg(target_os = "macos")]
use core_graphics::{
    event::{CGEvent, CGEventFlags, CGEventTapLocation},
    event_source::{CGEventSource, CGEventSourceStateID},
};
#[cfg(not(target_os = "macos"))]
use enigo::{Direction, Enigo, Key, Keyboard, NewConError, Settings};
use std::{panic::AssertUnwindSafe, thread, time::Duration};
use tauri::AppHandle;
use tauri_plugin_clipboard_manager::ClipboardExt;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Error)]
pub enum PasteError {
    #[error("clipboard write failed: {0}")]
    Clipboard(String),
    #[error("Accessibility permission is required to paste")]
    Accessibility,
    #[error("active target changed before paste")]
    TargetChanged,
    #[error("active target could not be identified safely")]
    TargetUnavailable,
    #[error("browser access is required to confirm the active tab")]
    BrowserAccessRequired,
    #[error("focused input is not available")]
    InputUnavailable,
    #[error("focused input changed before paste")]
    InputChanged,
    #[error("no selected text is available")]
    SelectionUnavailable,
    #[error("selected text changed before replacement")]
    SelectionChanged,
    #[error("paste failed: {0}")]
    Input(String),
    #[error("paste cancelled")]
    Cancelled,
}

/// Physical ANSI keycodes stay stable when the active input source is Chinese,
/// Japanese, or another non-Latin layout. `Key::Unicode('v')` is not safe here:
/// Enigo resolves it through the current layout and silently falls back to 0
/// when no matching key is found.
#[cfg(target_os = "macos")]
const COMMAND_COPY_KEYCODE: u16 = 0x08;
#[cfg(target_os = "macos")]
const COMMAND_PASTE_KEYCODE: u16 = 0x09;

/// The platform's "primary" modifier used for the paste shortcut
/// (Ctrl on Windows/Linux).
#[cfg(not(target_os = "macos"))]
const PRIMARY_MODIFIER: Key = Key::Control;

#[cfg(not(target_os = "macos"))]
fn map_enigo_connection_error(error: NewConError) -> PasteError {
    match error {
        NewConError::NoPermission => PasteError::Accessibility,
        other => PasteError::Input(other.to_string()),
    }
}

#[cfg(target_os = "macos")]
fn send_command_shortcut(keycode: u16) -> Result<(), PasteError> {
    if !crate::permissions::request_accessibility() {
        return Err(PasteError::Accessibility);
    }
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| PasteError::Input("failed to create macOS keyboard event source".into()))?;
    let key_down = CGEvent::new_keyboard_event(source.clone(), keycode, true).map_err(|_| {
        PasteError::Input("failed to create Command shortcut key-down event".into())
    })?;
    key_down.set_flags(CGEventFlags::CGEventFlagCommand);
    key_down.post(CGEventTapLocation::HID);

    let key_up = CGEvent::new_keyboard_event(source, keycode, false)
        .map_err(|_| PasteError::Input("failed to create Command shortcut key-up event".into()))?;
    key_up.set_flags(CGEventFlags::CGEventFlagCommand);
    key_up.post(CGEventTapLocation::HID);
    Ok(())
}

#[cfg(target_os = "macos")]
fn simulate_paste() -> Result<(), PasteError> {
    send_command_shortcut(COMMAND_PASTE_KEYCODE)
}

#[cfg(not(target_os = "macos"))]
fn simulate_paste() -> Result<(), PasteError> {
    let mut enigo = Enigo::new(&Settings::default()).map_err(map_enigo_connection_error)?;
    enigo
        .key(PRIMARY_MODIFIER, Direction::Press)
        .map_err(|error| PasteError::Input(error.to_string()))?;

    struct ModifierReleaseGuard<'a> {
        enigo: &'a mut Enigo,
        released: bool,
    }

    impl Drop for ModifierReleaseGuard<'_> {
        fn drop(&mut self) {
            if !self.released {
                let _ = self.enigo.key(PRIMARY_MODIFIER, Direction::Release);
            }
        }
    }

    let mut modifier = ModifierReleaseGuard {
        enigo: &mut enigo,
        released: false,
    };
    modifier
        .enigo
        .key(Key::Unicode('v'), Direction::Click)
        .map_err(|error| PasteError::Input(error.to_string()))?;
    modifier
        .enigo
        .key(PRIMARY_MODIFIER, Direction::Release)
        .map_err(|error| PasteError::Input(error.to_string()))?;
    modifier.released = true;
    Ok(())
}

#[cfg(target_os = "macos")]
fn simulate_copy() -> Result<(), PasteError> {
    send_command_shortcut(COMMAND_COPY_KEYCODE)
}

#[cfg(not(target_os = "macos"))]
fn simulate_copy() -> Result<(), PasteError> {
    let mut enigo = Enigo::new(&Settings::default()).map_err(map_enigo_connection_error)?;
    enigo
        .key(PRIMARY_MODIFIER, Direction::Press)
        .map_err(|error| PasteError::Input(error.to_string()))?;

    struct ModifierReleaseGuard<'a> {
        enigo: &'a mut Enigo,
        released: bool,
    }

    impl Drop for ModifierReleaseGuard<'_> {
        fn drop(&mut self) {
            if !self.released {
                let _ = self.enigo.key(PRIMARY_MODIFIER, Direction::Release);
            }
        }
    }

    let mut modifier = ModifierReleaseGuard {
        enigo: &mut enigo,
        released: false,
    };
    modifier
        .enigo
        .key(Key::Unicode('c'), Direction::Click)
        .map_err(|error| PasteError::Input(error.to_string()))?;
    modifier
        .enigo
        .key(PRIMARY_MODIFIER, Direction::Release)
        .map_err(|error| PasteError::Input(error.to_string()))?;
    modifier.released = true;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedSelection {
    pub text: String,
    pub fingerprint: u64,
}

pub fn selection_fingerprint(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Read the current selection without leaving the temporary Cmd-C result in
/// the user's clipboard. The clipboard is restored before this function
/// returns, including when the selected action is cancelled later.
pub fn capture_selected_text(
    app: &AppHandle,
    accessibility: bool,
) -> Result<CapturedSelection, PasteError> {
    if !accessibility {
        return Err(PasteError::Accessibility);
    }
    let previous = app.clipboard().read_text().ok();
    crate::modifier_hotkey::set_paste_suppressed(true);
    let copy_result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        simulate_copy()?;
        thread::sleep(Duration::from_millis(80));
        let selected = app
            .clipboard()
            .read_text()
            .map_err(|error| PasteError::Clipboard(error.to_string()))?;
        if selected.trim().is_empty() {
            return Err(PasteError::SelectionUnavailable);
        }
        Ok(selected)
    }))
    .unwrap_or_else(|_| Err(PasteError::Input("selection capture panicked".into())));
    crate::modifier_hotkey::set_paste_suppressed(false);

    match previous {
        Some(previous) => {
            let _ = app.clipboard().write_text(&previous);
        }
        None => {
            let _ = app.clipboard().clear();
        }
    }

    let text = copy_result?;
    let fingerprint = selection_fingerprint(&text);
    Ok(CapturedSelection { text, fingerprint })
}

/// Run the irreversible keyboard injection only after the last cancellation and
/// target checks. This stays synchronous because the caller already runs inside
/// `spawn_blocking`; detaching it behind a timeout could let a timed-out paste
/// arrive after the caller had fallen back to another delivery path.
fn run_paste_attempt(
    cancellation: &CancellationToken,
    verify_target: impl Fn() -> Result<(), PasteError>,
    simulate: impl FnOnce() -> Result<(), PasteError>,
) -> Result<(), PasteError> {
    if cancellation.is_cancelled() {
        return Err(PasteError::Cancelled);
    }
    verify_target()?;

    crate::modifier_hotkey::set_paste_suppressed(true);
    struct PasteSuppressionGuard;
    impl Drop for PasteSuppressionGuard {
        fn drop(&mut self) {
            crate::modifier_hotkey::set_paste_suppressed(false);
        }
    }

    let _suppression_guard = PasteSuppressionGuard;
    std::panic::catch_unwind(AssertUnwindSafe(|| {
        if cancellation.is_cancelled() {
            return Err(PasteError::Cancelled);
        }
        verify_target()?;
        simulate()
    }))
    .unwrap_or_else(|_| Err(PasteError::Input("paste worker panicked".into())))
}

fn check_before_clipboard(cancellation: &CancellationToken) -> Result<(), PasteError> {
    if cancellation.is_cancelled() {
        Err(PasteError::Cancelled)
    } else {
        Ok(())
    }
}

pub fn insert(
    app: &AppHandle,
    text: &str,
    accessibility: bool,
    cancellation: CancellationToken,
    verify_target: impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
) -> Result<(), PasteError> {
    // A clipboard write is itself a user-visible delivery side effect. Do not
    // let a cancellation that arrived while processing overwrite the user's
    // existing clipboard before the keyboard-injection guard runs.
    check_before_clipboard(&cancellation)?;
    app.clipboard()
        .write_text(text)
        .map_err(|error| PasteError::Clipboard(error.to_string()))?;
    thread::sleep(Duration::from_millis(100));
    if cancellation.is_cancelled() {
        return Err(PasteError::Cancelled);
    }
    if !accessibility {
        return Err(PasteError::Accessibility);
    }
    verify_target()?;

    // Suppress the modifier event-tap while we synthesize the paste keystroke:
    // our own keystroke must not be read as a physical hotkey tap, and re-entrant
    // event delivery during the paste aborts the main runloop (uncaught
    // NSException -> SIGABRT). Enigo is called synchronously here, but this
    // function is reached from `paste_text`'s `spawn_blocking` worker, never from
    // the AppKit/tao main-thread callback.
    let result = run_paste_attempt(&cancellation, verify_target, simulate_paste);
    if result.is_ok() {
        thread::sleep(Duration::from_millis(250));
    }
    result
}

pub fn copy(app: &AppHandle, text: &str) -> Result<(), PasteError> {
    app.clipboard()
        .write_text(text)
        .map_err(|error| PasteError::Clipboard(error.to_string()))
}

/// Copy only while the processing session is still active. The cancellation
/// check runs in the same blocking worker as the clipboard write, preventing
/// a late processing result from overwriting the user's clipboard after
/// cancellation has been requested.
pub fn copy_if_not_cancelled(
    app: &AppHandle,
    text: &str,
    cancellation: &CancellationToken,
) -> Result<(), PasteError> {
    check_before_clipboard(cancellation)?;
    copy(app, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn cancelled_attempt_never_runs_keyboard_injection() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let called = Arc::new(Mutex::new(false));
        let called_in_simulate = called.clone();

        let result = run_paste_attempt(
            &cancellation,
            || Ok(()),
            move || {
                *called_in_simulate.lock().unwrap() = true;
                Ok::<(), PasteError>(())
            },
        );

        assert!(matches!(result, Err(PasteError::Cancelled)));
        assert!(!*called.lock().unwrap());
    }

    #[test]
    fn cancelled_delivery_is_rejected_before_clipboard_stage() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        assert!(matches!(
            check_before_clipboard(&cancellation),
            Err(PasteError::Cancelled)
        ));
    }

    #[test]
    fn changed_target_never_runs_keyboard_injection() {
        let cancellation = CancellationToken::new();
        let called = Arc::new(Mutex::new(false));
        let called_in_simulate = called.clone();

        let result = run_paste_attempt(
            &cancellation,
            || Err(PasteError::TargetChanged),
            move || {
                *called_in_simulate.lock().unwrap() = true;
                Ok::<(), PasteError>(())
            },
        );

        assert!(matches!(result, Err(PasteError::TargetChanged)));
        assert!(!*called.lock().unwrap());
    }

    #[test]
    fn panicking_keyboard_injection_is_recovered() {
        let cancellation = CancellationToken::new();
        let result = run_paste_attempt(
            &cancellation,
            || Ok(()),
            || -> Result<(), PasteError> { panic!("synthetic input failure") },
        );

        assert!(matches!(result, Err(PasteError::Input(message)) if message.contains("panicked")));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn enigo_permission_error_is_reported_as_accessibility() {
        assert!(matches!(
            map_enigo_connection_error(NewConError::NoPermission),
            PasteError::Accessibility
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_shortcuts_use_layout_independent_ansi_keycodes() {
        assert_eq!(COMMAND_COPY_KEYCODE, 0x08);
        assert_eq!(COMMAND_PASTE_KEYCODE, 0x09);
    }

    #[test]
    fn selection_fingerprint_changes_when_selected_text_changes() {
        assert_eq!(selection_fingerprint("same"), selection_fingerprint("same"));
        assert_ne!(
            selection_fingerprint("first"),
            selection_fingerprint("second")
        );
    }
}
