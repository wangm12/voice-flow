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
#[cfg(target_os = "macos")]
const COMMAND_UNDO_KEYCODE: u16 = 0x06;

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
    if !crate::permissions::accessibility_is_trusted() {
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

#[cfg(target_os = "macos")]
fn simulate_undo() -> Result<(), PasteError> {
    send_command_shortcut(COMMAND_UNDO_KEYCODE)
}

#[cfg(not(target_os = "macos"))]
fn simulate_undo() -> Result<(), PasteError> {
    let mut enigo = Enigo::new(&Settings::default()).map_err(map_enigo_connection_error)?;
    enigo
        .key(PRIMARY_MODIFIER, Direction::Press)
        .map_err(|error| PasteError::Input(error.to_string()))?;
    enigo
        .key(Key::Unicode('z'), Direction::Click)
        .map_err(|error| PasteError::Input(error.to_string()))?;
    enigo
        .key(PRIMARY_MODIFIER, Direction::Release)
        .map_err(|error| PasteError::Input(error.to_string()))?;
    Ok(())
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InsertOutcome {
    /// The platform shortcut was posted successfully.
    pub shortcut_sent: bool,
    /// Reserved for a future AX direct-insertion adapter. Cmd+V alone cannot
    /// prove that the target application accepted the clipboard contents.
    pub verified: bool,
    /// In-memory fingerprint of the complete focused input value immediately
    /// after a verified paste. Undo uses this to avoid undoing later user edits.
    pub post_insert_input_fingerprint: Option<u64>,
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

#[derive(Debug)]
struct PasteAttempt {
    shortcut_sent: bool,
    result: Result<(), PasteError>,
}

impl PasteAttempt {
    fn sent() -> Self {
        Self {
            shortcut_sent: true,
            result: Ok(()),
        }
    }

    fn not_sent(error: PasteError) -> Self {
        Self {
            shortcut_sent: false,
            result: Err(error),
        }
    }

    fn failed_after_send(error: PasteError) -> Self {
        Self {
            shortcut_sent: true,
            result: Err(error),
        }
    }
}

fn should_restore_clipboard(attempt: &PasteAttempt) -> bool {
    !attempt.shortcut_sent
}

/// Run the irreversible keyboard injection only after the last cancellation and
/// target checks. This stays synchronous because the caller already runs inside
/// `spawn_blocking`; detaching it behind a timeout could let a timed-out paste
/// arrive after the caller had fallen back to another delivery path.
fn run_paste_attempt(
    cancellation: &CancellationToken,
    verify_target: impl Fn() -> Result<(), PasteError>,
    simulate: impl FnOnce() -> Result<(), PasteError>,
) -> PasteAttempt {
    if cancellation.is_cancelled() {
        return PasteAttempt::not_sent(PasteError::Cancelled);
    }

    crate::modifier_hotkey::set_paste_suppressed(true);
    struct PasteSuppressionGuard;
    impl Drop for PasteSuppressionGuard {
        fn drop(&mut self) {
            crate::modifier_hotkey::set_paste_suppressed(false);
        }
    }

    let shortcut_attempted = std::sync::atomic::AtomicBool::new(false);
    let _suppression_guard = PasteSuppressionGuard;
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        if cancellation.is_cancelled() {
            return Err(PasteError::Cancelled);
        }
        // Keep the final target check inside the suppression window so the
        // focused app cannot change between verification and Cmd+V.
        verify_target()?;
        if cancellation.is_cancelled() {
            return Err(PasteError::Cancelled);
        }
        shortcut_attempted.store(true, std::sync::atomic::Ordering::Release);
        simulate()
    }))
    .unwrap_or_else(|_| Err(PasteError::Input("paste worker panicked".into())));
    let shortcut_sent = shortcut_attempted.load(std::sync::atomic::Ordering::Acquire);
    match result {
        Ok(()) => PasteAttempt::sent(),
        Err(error) if shortcut_sent => PasteAttempt::failed_after_send(error),
        Err(error) => PasteAttempt::not_sent(error),
    }
}

fn check_before_clipboard(cancellation: &CancellationToken) -> Result<(), PasteError> {
    if cancellation.is_cancelled() {
        Err(PasteError::Cancelled)
    } else {
        Ok(())
    }
}

fn input_value_verifies_delivery(
    before: Option<&str>,
    after: Option<&str>,
    expected: &str,
) -> bool {
    if expected.is_empty() || expected.chars().count() < 2 {
        return false;
    }
    let Some(after) = after else {
        return false;
    };
    let after_count = after.matches(expected).count();
    if after_count == 0 {
        return false;
    }
    let expected_chars = expected.chars().count();
    let after_chars = after.chars().count();
    match before {
        Some(before) => {
            after_count > before.matches(expected).count()
                && after_chars >= before.chars().count().saturating_add(expected_chars)
        }
        None => after_chars >= expected_chars,
    }
}

fn restore_clipboard(app: &AppHandle, previous: Option<&str>) -> Result<(), PasteError> {
    let Some(previous) = previous else {
        return Ok(());
    };
    app.clipboard()
        .write_text(previous)
        .map_err(|error| PasteError::Clipboard(error.to_string()))
}

pub fn insert(
    app: &AppHandle,
    text: &str,
    accessibility: bool,
    cancellation: CancellationToken,
    verify_target: impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
) -> Result<InsertOutcome, PasteError> {
    // A clipboard write is itself a user-visible delivery side effect. Do not
    // let a failed permission or target check overwrite the user's existing
    // clipboard before the keyboard-injection guard runs.
    check_before_clipboard(&cancellation)?;
    if !accessibility {
        return Err(PasteError::Accessibility);
    }
    verify_target()?;
    let value_before = crate::context::focused_input_value();
    check_before_clipboard(&cancellation)?;
    // Images and files cannot be restored as text. Continue the paste instead
    // of aborting the whole delivery because the previous clipboard was not
    // a string.
    let previous_clipboard = app.clipboard().read_text().ok();
    check_before_clipboard(&cancellation)?;
    app.clipboard()
        .write_text(text)
        .map_err(|error| PasteError::Clipboard(error.to_string()))?;
    thread::sleep(Duration::from_millis(100));
    if cancellation.is_cancelled() {
        let _ = restore_clipboard(app, previous_clipboard.as_deref());
        return Err(PasteError::Cancelled);
    }
    // Suppress the modifier event-tap while we synthesize the paste keystroke:
    // our own keystroke must not be read as a physical hotkey tap, and re-entrant
    // event delivery during the paste aborts the main runloop (uncaught
    // NSException -> SIGABRT). Enigo is called synchronously here, but this
    // function is reached from `paste_text`'s `spawn_blocking` worker, never from
    // the AppKit/tao main-thread callback.
    let attempt = run_paste_attempt(&cancellation, verify_target, simulate_paste);
    if should_restore_clipboard(&attempt) {
        // Restore only when Cmd+V was never posted. Once the shortcut is sent,
        // keep VoiceFlow's text available as the manual fallback.
        let _ = restore_clipboard(app, previous_clipboard.as_deref());
    }
    if attempt.result.is_ok() {
        thread::sleep(Duration::from_millis(250));
    }
    attempt.result.map(|()| {
        let value_after = crate::context::focused_input_value();
        let verified =
            input_value_verifies_delivery(value_before.as_deref(), value_after.as_deref(), text);
        InsertOutcome {
            shortcut_sent: true,
            verified,
            post_insert_input_fingerprint: verified
                .then(|| value_after.as_deref().map(selection_fingerprint))
                .flatten(),
        }
    })
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

/// Bring the original target app back to the front before a user confirms a
/// selected-text preview. The preview window necessarily took focus, so the
/// target guard must be checked again after activation.
pub fn activate_target(pid: i32) -> Result<(), PasteError> {
    #[cfg(target_os = "macos")]
    {
        unsafe {
            use objc::{class, msg_send, sel, sel_impl};
            let application: *mut objc::runtime::Object = msg_send![
                class!(NSRunningApplication),
                runningApplicationWithProcessIdentifier: pid
            ];
            if application.is_null() {
                return Err(PasteError::TargetUnavailable);
            }
            let activated: bool = msg_send![application, activateWithOptions: (1u64 << 1)];
            if !activated {
                return Err(PasteError::TargetUnavailable);
            }
        }
        thread::sleep(Duration::from_millis(120));
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        Ok(())
    }
}

/// Undo is deliberately a separate primitive from paste. Callers must verify
/// the target immediately before invoking it; this function only performs the
/// platform shortcut after the permission check.
pub fn undo(accessibility: bool) -> Result<(), PasteError> {
    if !accessibility {
        return Err(PasteError::Accessibility);
    }
    std::panic::catch_unwind(AssertUnwindSafe(simulate_undo))
        .unwrap_or_else(|_| Err(PasteError::Input("undo worker panicked".into())))
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

        assert!(!result.shortcut_sent);
        assert!(should_restore_clipboard(&result));
        assert!(matches!(result.result, Err(PasteError::Cancelled)));
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
    fn cancellation_after_target_check_never_runs_keyboard_injection() {
        let cancellation = CancellationToken::new();
        let called = Arc::new(Mutex::new(false));
        let called_in_simulate = called.clone();
        let cancellation_in_verify = cancellation.clone();

        let result = run_paste_attempt(
            &cancellation,
            move || {
                cancellation_in_verify.cancel();
                Ok(())
            },
            move || {
                *called_in_simulate.lock().unwrap() = true;
                Ok::<(), PasteError>(())
            },
        );

        assert!(!result.shortcut_sent);
        assert!(should_restore_clipboard(&result));
        assert!(matches!(result.result, Err(PasteError::Cancelled)));
        assert!(!*called.lock().unwrap());
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

        assert!(!result.shortcut_sent);
        assert!(should_restore_clipboard(&result));
        assert!(matches!(result.result, Err(PasteError::TargetChanged)));
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

        assert!(result.shortcut_sent);
        assert!(!should_restore_clipboard(&result));
        assert!(
            matches!(result.result, Err(PasteError::Input(message)) if message.contains("panicked"))
        );
    }

    #[test]
    fn successful_shortcut_keeps_dictation_clipboard() {
        let cancellation = CancellationToken::new();
        let result = run_paste_attempt(&cancellation, || Ok(()), || Ok(()));
        assert!(result.shortcut_sent);
        assert!(!should_restore_clipboard(&result));
        assert!(result.result.is_ok());
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

    #[test]
    fn input_verification_requires_a_new_occurrence_of_the_delivered_text() {
        assert!(input_value_verifies_delivery(
            Some("before"),
            Some("before + inserted"),
            "inserted"
        ));
        assert!(!input_value_verifies_delivery(
            Some("already inserted"),
            Some("already inserted"),
            "inserted"
        ));
        assert!(!input_value_verifies_delivery(
            Some("before inserted"),
            Some("before changed"),
            "inserted"
        ));
        assert!(!input_value_verifies_delivery(None, None, "inserted"));
        assert!(!input_value_verifies_delivery(
            Some("aa"),
            Some("aa"),
            "a"
        ));
        assert!(!input_value_verifies_delivery(
            Some("aa"),
            Some("aaa"),
            "a"
        ));
        assert!(input_value_verifies_delivery(
            Some("hi"),
            Some("hi hello"),
            "hello"
        ));
        assert!(!input_value_verifies_delivery(
            None,
            Some("inserted"),
            "i"
        ));
    }
}
