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

pub fn should_post_shortcut_to_pid(expected_pid: i32, own_pid: i32) -> bool {
    expected_pid > 0 && expected_pid != own_pid
}

pub fn ax_insert_is_final(outcome: &InsertOutcome) -> bool {
    outcome.verified
}

#[cfg(target_os = "macos")]
fn send_command_shortcut(keycode: u16) -> Result<(), PasteError> {
    send_command_shortcut_targeting(keycode, None)
}

#[cfg(target_os = "macos")]
fn send_command_shortcut_targeting(
    keycode: u16,
    target_pid: Option<i32>,
) -> Result<(), PasteError> {
    if !crate::permissions::accessibility_is_trusted() {
        return Err(PasteError::Accessibility);
    }
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| PasteError::Input("failed to create macOS keyboard event source".into()))?;
    let key_down = CGEvent::new_keyboard_event(source.clone(), keycode, true).map_err(|_| {
        PasteError::Input("failed to create Command shortcut key-down event".into())
    })?;
    key_down.set_flags(CGEventFlags::CGEventFlagCommand);
    post_command_event(&key_down, target_pid);

    let key_up = CGEvent::new_keyboard_event(source, keycode, false)
        .map_err(|_| PasteError::Input("failed to create Command shortcut key-up event".into()))?;
    key_up.set_flags(CGEventFlags::CGEventFlagCommand);
    post_command_event(&key_up, target_pid);
    Ok(())
}

#[cfg(target_os = "macos")]
fn post_command_event(event: &CGEvent, target_pid: Option<i32>) {
    let own_pid = std::process::id() as i32;
    if let Some(pid) = target_pid.filter(|pid| should_post_shortcut_to_pid(*pid, own_pid)) {
        event.post_to_pid(pid);
    } else {
        event.post(CGEventTapLocation::HID);
    }
}

/// Hold ABC until the target can consume Cmd+V. `CGEvent::post` is async; the
/// pre-switch settle is 30 ms, so the post-paste hold stays in the same band.
#[cfg(target_os = "macos")]
const PASTE_CONSUME_SETTLE: Duration = Duration::from_millis(40);

/// VoiceInk-style pause after activating the recorded app/window.
const DELIVERY_FOCUS_SETTLE: Duration = Duration::from_millis(120);

#[cfg(target_os = "macos")]
const APPLESCRIPT_PASTE: &str =
    r#"tell application "System Events" to key code 9 using command down"#;

#[cfg(target_os = "macos")]
fn simulate_paste(expected_pid: i32) -> Result<(), PasteError> {
    let _latin_layout = crate::input_source::AbcLayoutGuard::acquire();
    send_command_shortcut_targeting(COMMAND_PASTE_KEYCODE, Some(expected_pid))?;
    thread::sleep(PASTE_CONSUME_SETTLE);
    Ok(())
}

fn simulate_paste_with_fallback(expected_pid: i32) -> Result<(), PasteError> {
    simulate_paste(expected_pid)?;
    let own_pid = std::process::id() as i32;
    if should_retry_paste_with_applescript(current_frontmost_pid(), expected_pid, own_pid) {
        let _ = activate_target_now(expected_pid);
        #[cfg(target_os = "macos")]
        {
            simulate_paste_applescript()?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn simulate_paste_applescript() -> Result<(), PasteError> {
    if !crate::permissions::accessibility_is_trusted() {
        return Err(PasteError::Accessibility);
    }
    let status = std::process::Command::new("osascript")
        .args(["-e", APPLESCRIPT_PASTE])
        .status()
        .map_err(|error| PasteError::Input(error.to_string()))?;
    if !status.success() {
        return Err(PasteError::Input("AppleScript paste failed".into()));
    }
    thread::sleep(PASTE_CONSUME_SETTLE);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn simulate_paste(_expected_pid: i32) -> Result<(), PasteError> {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsertOutcome {
    /// The platform shortcut was posted, or a safe AX insert was applied.
    pub shortcut_sent: bool,
    /// True only if Cmd+V was posted. AX value sets are often not on the
    /// target undo stack, so a 3s Cmd+Z must never be armed for that path.
    pub used_keyboard_paste: bool,
    /// Best-effort proof that the focused field now contains the delivered text.
    /// Cmd+V or AX set alone cannot prove that the target application accepted it.
    pub verified: bool,
    /// In-memory fingerprint of the complete focused input value immediately
    /// after a verified paste. Undo uses this to avoid undoing later user edits.
    pub post_insert_input_fingerprint: Option<u64>,
    /// Focused field value immediately after a successful insert. Dictionary
    /// learning diffs this baseline against a later same-field read.
    pub value_after: Option<String>,
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
    let _suppress = crate::modifier_hotkey::PasteSuppressGuard::new();
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
    drop(_suppress);

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

/// AX insert that we could not verify must still leave the text copyable.
/// Verified AX must not touch the clipboard. Keyboard paste already wrote it.
fn should_copy_clipboard_fallback(outcome: &InsertOutcome) -> bool {
    !outcome.used_keyboard_paste && !outcome.verified
}

fn should_restore_clipboard(attempt: &PasteAttempt) -> bool {
    // Restore whenever Cmd+V was never posted (cancelled, target changed, or
    // any other path that did not inject the shortcut).
    !attempt.shortcut_sent
}

fn build_insert_outcome(
    used_keyboard_paste: bool,
    value_before: Option<&str>,
    value_after: Option<String>,
    expected: &str,
    target_still_frontmost: bool,
) -> InsertOutcome {
    let verified = insert_is_verified(
        used_keyboard_paste,
        value_before,
        value_after.as_deref(),
        expected,
        target_still_frontmost,
    );
    InsertOutcome {
        shortcut_sent: true,
        used_keyboard_paste,
        verified,
        post_insert_input_fingerprint: verified
            .then(|| value_after.as_deref().map(selection_fingerprint))
            .flatten(),
        value_after,
    }
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

    let shortcut_attempted = std::sync::atomic::AtomicBool::new(false);
    let _suppression_guard = crate::modifier_hotkey::PasteSuppressGuard::new();
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

fn insert_is_verified(
    used_keyboard_paste: bool,
    before: Option<&str>,
    after: Option<&str>,
    expected: &str,
    target_still_frontmost: bool,
) -> bool {
    if input_value_verifies_delivery(before, after, expected) {
        return true;
    }
    used_keyboard_paste && target_still_frontmost && !expected.is_empty()
}

fn input_value_verifies_delivery(
    before: Option<&str>,
    after: Option<&str>,
    expected: &str,
) -> bool {
    if expected.is_empty() {
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
            let before_count = before.matches(expected).count();
            if expected_chars == 1 && before_count > 0 {
                return false;
            }
            after_count > before_count
                && after_chars >= before.chars().count().saturating_add(expected_chars)
        }
        None => {
            if expected_chars == 1 {
                false
            } else {
                after_chars >= expected_chars
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AxInsertDecision {
    SkipSecure,
    SkipWrongRole,
    SkipWouldReplaceAll,
    SkipNeedsKeystrokeTyper,
    AttemptSelectedText,
    AttemptValueSplice,
}

fn ax_insert_decision(
    role: &str,
    subrole: &str,
    selected_text_settable: bool,
    value_settable: bool,
    selected_range: Option<(i64, i64)>,
    field_empty: bool,
) -> AxInsertDecision {
    let role_blob = format!("{role} {subrole}").to_ascii_lowercase();
    if role_blob.contains("securetextfield") || role_blob.contains("secure text field") {
        return AxInsertDecision::SkipSecure;
    }
    let editable = ["textfield", "textarea", "combobox", "searchfield"]
        .iter()
        .any(|marker| role_blob.contains(marker));
    if !editable {
        return AxInsertDecision::SkipWrongRole;
    }
    if selected_text_settable {
        return AxInsertDecision::AttemptSelectedText;
    }
    if value_settable && (selected_range.is_some() || field_empty) {
        return AxInsertDecision::AttemptValueSplice;
    }
    if value_settable {
        return AxInsertDecision::SkipWouldReplaceAll;
    }
    AxInsertDecision::SkipNeedsKeystrokeTyper
}

/// AX selected-text ranges are UTF-16 units. Refuse out-of-range edits rather
/// than guessing a byte or `char` index, which would corrupt CJK text.
fn utf16_splice(current: &str, location: i64, length: i64, insert: &str) -> Option<String> {
    if location < 0 || length < 0 {
        return None;
    }
    let units: Vec<u16> = current.encode_utf16().collect();
    let start = usize::try_from(location).ok()?;
    let span = usize::try_from(length).ok()?;
    let end = start.checked_add(span)?;
    if end > units.len() {
        return None;
    }
    let mut next = Vec::with_capacity(units.len() - span + insert.encode_utf16().count());
    next.extend_from_slice(&units[..start]);
    next.extend(insert.encode_utf16());
    next.extend_from_slice(&units[end..]);
    String::from_utf16(&next).ok()
}

fn try_ax_insert_if_safe(
    cancellation: &CancellationToken,
    verify_target: impl Fn() -> Result<(), PasteError>,
    text: &str,
) -> Option<String> {
    if cancellation.is_cancelled() || verify_target().is_err() || text.is_empty() {
        return None;
    }
    try_ax_insert(text)
}

#[cfg(not(target_os = "macos"))]
fn try_ax_insert(_text: &str) -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
fn try_ax_insert(text: &str) -> Option<String> {
    macos_ax::try_insert(text)
}

#[cfg(target_os = "macos")]
mod macos_ax {
    use super::{ax_insert_decision, utf16_splice, AxInsertDecision};
    use core::ffi::c_void;
    use core_foundation::base::{CFRange, CFRelease, CFType, CFTypeRef, TCFType};
    use core_foundation::string::{CFString, CFStringRef};

    type AXUIElementRef = *const c_void;
    type AXValueRef = *const c_void;
    const AX_SUCCESS: i32 = 0;
    const AX_VALUE_CF_RANGE: u32 = 4;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXUIElementCreateSystemWide() -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> i32;
        fn AXUIElementSetAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: CFTypeRef,
        ) -> i32;
        fn AXUIElementIsAttributeSettable(
            element: AXUIElementRef,
            attribute: CFStringRef,
            settable: *mut u8,
        ) -> i32;
        fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout_in_seconds: f32) -> i32;
        fn AXValueCreate(the_type: u32, value_ptr: *const c_void) -> AXValueRef;
        fn AXValueGetValue(value: AXValueRef, the_type: u32, value_ptr: *mut c_void) -> u8;
    }

    struct AxElement(AXUIElementRef);

    impl Drop for AxElement {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0 as CFTypeRef) };
                self.0 = std::ptr::null();
            }
        }
    }

    pub(super) fn try_insert(text: &str) -> Option<String> {
        if !crate::permissions::accessibility_is_trusted() {
            return None;
        }
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| try_insert_inner(text)))
            .unwrap_or(None)
    }

    fn try_insert_inner(text: &str) -> Option<String> {
        let system = AxElement(unsafe { AXUIElementCreateSystemWide() });
        if system.0.is_null() {
            return None;
        }
        unsafe {
            let _ = AXUIElementSetMessagingTimeout(system.0, 0.35);
        }
        let Some(focused) = copy_element_attr(&system, "AXFocusedUIElement") else {
            return None;
        };
        unsafe {
            let _ = AXUIElementSetMessagingTimeout(focused.0, 0.35);
        }

        let role = copy_string_attr(&focused, "AXRole").unwrap_or_default();
        let subrole = copy_string_attr(&focused, "AXSubrole").unwrap_or_default();
        let selected_text_settable = is_settable(&focused, "AXSelectedText");
        let value_settable = is_settable(&focused, "AXValue");
        let current_value = copy_string_attr(&focused, "AXValue");
        let selected_range = copy_range_attr(&focused, "AXSelectedTextRange");
        let field_empty = current_value.as_deref().is_none_or(str::is_empty);

        match ax_insert_decision(
            &role,
            &subrole,
            selected_text_settable,
            value_settable,
            selected_range,
            field_empty,
        ) {
            AxInsertDecision::AttemptSelectedText => {
                if !set_string_attr(&focused, "AXSelectedText", text) {
                    return None;
                }
                Some(copy_string_attr(&focused, "AXValue").unwrap_or_default())
            }
            AxInsertDecision::AttemptValueSplice => {
                let current = current_value.unwrap_or_default();
                let (location, length) = selected_range.unwrap_or((0, 0));
                let Some(next) = utf16_splice(&current, location, length, text) else {
                    return None;
                };
                if !set_string_attr(&focused, "AXValue", &next) {
                    return None;
                }
                let caret = location.saturating_add(text.encode_utf16().count() as i64);
                let _ = set_range_attr(&focused, "AXSelectedTextRange", caret, 0);
                Some(copy_string_attr(&focused, "AXValue").unwrap_or_default())
            }
            _ => None,
        }
    }

    fn copy_element_attr(element: &AxElement, name: &str) -> Option<AxElement> {
        let value = copy_raw_attr(element, name)?;
        Some(AxElement(value as AXUIElementRef))
    }

    fn copy_raw_attr(element: &AxElement, name: &str) -> Option<CFTypeRef> {
        let attr = CFString::new(name);
        let mut value: CFTypeRef = std::ptr::null();
        let err = unsafe {
            AXUIElementCopyAttributeValue(element.0, attr.as_concrete_TypeRef(), &mut value)
        };
        if err != AX_SUCCESS || value.is_null() {
            None
        } else {
            Some(value)
        }
    }

    fn copy_string_attr(element: &AxElement, name: &str) -> Option<String> {
        let value = copy_raw_attr(element, name)?;
        let cf_type = unsafe { CFType::wrap_under_create_rule(value) };
        if cf_type.type_of() != CFString::type_id() {
            return None;
        }
        Some(unsafe { CFString::wrap_under_get_rule(cf_type.as_CFTypeRef() as _) }.to_string())
    }

    fn copy_range_attr(element: &AxElement, name: &str) -> Option<(i64, i64)> {
        let value = copy_raw_attr(element, name)?;
        let mut range = CFRange {
            location: 0,
            length: 0,
        };
        let ok = unsafe {
            AXValueGetValue(
                value as AXValueRef,
                AX_VALUE_CF_RANGE,
                &mut range as *mut _ as *mut c_void,
            )
        };
        unsafe { CFRelease(value) };
        if ok == 0 {
            return None;
        }
        Some((range.location as i64, range.length as i64))
    }

    fn is_settable(element: &AxElement, name: &str) -> bool {
        let attr = CFString::new(name);
        let mut settable: u8 = 0;
        let err = unsafe {
            AXUIElementIsAttributeSettable(element.0, attr.as_concrete_TypeRef(), &mut settable)
        };
        err == AX_SUCCESS && settable != 0
    }

    fn set_string_attr(element: &AxElement, name: &str, text: &str) -> bool {
        let attr = CFString::new(name);
        let value = CFString::new(text);
        unsafe {
            AXUIElementSetAttributeValue(
                element.0,
                attr.as_concrete_TypeRef(),
                value.as_CFTypeRef(),
            ) == AX_SUCCESS
        }
    }

    fn set_range_attr(element: &AxElement, name: &str, location: i64, length: i64) -> bool {
        let range = CFRange {
            location: location as isize,
            length: length as isize,
        };
        let value =
            unsafe { AXValueCreate(AX_VALUE_CF_RANGE, &range as *const _ as *const c_void) };
        if value.is_null() {
            return false;
        }
        let attr = CFString::new(name);
        let ok = unsafe {
            AXUIElementSetAttributeValue(element.0, attr.as_concrete_TypeRef(), value as CFTypeRef)
                == AX_SUCCESS
        };
        unsafe { CFRelease(value as CFTypeRef) };
        ok
    }
}

#[cfg(target_os = "macos")]
mod macos_raise {
    use core::ffi::c_void;
    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::{CFRelease, CFRetain, CFTypeRef, TCFType};
    use core_foundation::number::CFNumber;
    use core_foundation::string::{CFString, CFStringRef};

    type AXUIElementRef = *const c_void;
    type AXValueRef = *const c_void;
    const AX_SUCCESS: i32 = 0;
    const AX_VALUE_CGPOINT: u32 = 1;
    const AX_VALUE_CGSIZE: u32 = 2;

    #[repr(C)]
    struct CgPoint {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    struct CgSize {
        width: f64,
        height: f64,
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> i32;
        fn AXUIElementPerformAction(element: AXUIElementRef, action: CFStringRef) -> i32;
        fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout_in_seconds: f32) -> i32;
        fn AXValueGetValue(value: AXValueRef, the_type: u32, value_ptr: *mut c_void) -> u8;
        fn _AXUIElementGetWindow(element: AXUIElementRef, identifier: *mut u32) -> i32;
    }

    struct AxElement(AXUIElementRef);

    impl Drop for AxElement {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0 as CFTypeRef) };
                self.0 = std::ptr::null();
            }
        }
    }

    pub(super) fn raise_window(pid: i32, window_id: u64) {
        if pid <= 0 || window_id == 0 || window_id > u32::MAX as u64 {
            return;
        }
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            raise_window_inner(pid, window_id as u32);
        }));
    }

    fn raise_window_inner(pid: i32, window_id: u32) {
        if !crate::permissions::accessibility_is_trusted() {
            return;
        }
        let app = AxElement(unsafe { AXUIElementCreateApplication(pid) });
        if app.0.is_null() {
            return;
        }
        unsafe {
            let _ = AXUIElementSetMessagingTimeout(app.0, 0.35);
        }
        let windows = copy_windows(&app);
        let bounds = crate::context::window_bounds_for_id(pid, window_id as u64);
        for window in windows {
            if window_matches(&window, window_id, bounds) {
                let action = CFString::new("AXRaise");
                unsafe {
                    let _ = AXUIElementPerformAction(window.0, action.as_concrete_TypeRef());
                }
                return;
            }
        }
    }

    fn window_matches(
        window: &AxElement,
        window_id: u32,
        bounds: Option<(f64, f64, f64, f64)>,
    ) -> bool {
        if ax_window_id(window) == Some(window_id) {
            return true;
        }
        if copy_number_attr(window, "AXWindowNumber")
            .or_else(|| copy_number_attr(window, "_AXWindowNumber"))
            == Some(i64::from(window_id))
        {
            return true;
        }
        let Some((x, y, width, height)) = bounds else {
            return false;
        };
        let Some(position) = copy_point_attr(window, "AXPosition") else {
            return false;
        };
        let Some(size) = copy_size_attr(window, "AXSize") else {
            return false;
        };
        (position.x - x).abs() <= 2.0
            && (position.y - y).abs() <= 2.0
            && (size.width - width).abs() <= 2.0
            && (size.height - height).abs() <= 2.0
    }

    fn ax_window_id(window: &AxElement) -> Option<u32> {
        let mut identifier = 0u32;
        let err = unsafe { _AXUIElementGetWindow(window.0, &mut identifier) };
        (err == AX_SUCCESS && identifier > 0).then_some(identifier)
    }

    fn copy_windows(app: &AxElement) -> Vec<AxElement> {
        let attr = CFString::new("AXWindows");
        let mut value: CFTypeRef = std::ptr::null();
        let err =
            unsafe { AXUIElementCopyAttributeValue(app.0, attr.as_concrete_TypeRef(), &mut value) };
        if err != AX_SUCCESS || value.is_null() {
            return Vec::new();
        }
        let array =
            unsafe { CFArray::<*const c_void>::wrap_under_create_rule(value as CFArrayRef) };
        let mut windows = Vec::new();
        for index in 0..array.len() {
            let Some(item) = array.get(index) else {
                continue;
            };
            if item.is_null() {
                continue;
            }
            unsafe { CFRetain(*item) };
            windows.push(AxElement(*item));
        }
        windows
    }

    fn copy_raw_attr(element: &AxElement, name: &str) -> Option<CFTypeRef> {
        let attr = CFString::new(name);
        let mut value: CFTypeRef = std::ptr::null();
        let err = unsafe {
            AXUIElementCopyAttributeValue(element.0, attr.as_concrete_TypeRef(), &mut value)
        };
        if err != AX_SUCCESS || value.is_null() {
            None
        } else {
            Some(value)
        }
    }

    fn copy_number_attr(element: &AxElement, name: &str) -> Option<i64> {
        let value = copy_raw_attr(element, name)?;
        let number = unsafe { CFNumber::wrap_under_create_rule(value as _) };
        number.to_i64()
    }

    fn copy_point_attr(element: &AxElement, name: &str) -> Option<CgPoint> {
        let value = copy_raw_attr(element, name)?;
        let mut point = CgPoint { x: 0.0, y: 0.0 };
        let ok = unsafe {
            AXValueGetValue(
                value as AXValueRef,
                AX_VALUE_CGPOINT,
                &mut point as *mut _ as *mut c_void,
            )
        };
        unsafe { CFRelease(value) };
        (ok != 0).then_some(point)
    }

    fn copy_size_attr(element: &AxElement, name: &str) -> Option<CgSize> {
        let value = copy_raw_attr(element, name)?;
        let mut size = CgSize {
            width: 0.0,
            height: 0.0,
        };
        let ok = unsafe {
            AXValueGetValue(
                value as AXValueRef,
                AX_VALUE_CGSIZE,
                &mut size as *mut _ as *mut c_void,
            )
        };
        unsafe { CFRelease(value) };
        (ok != 0).then_some(size)
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
    expected_pid: i32,
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
    // Prefer a safe in-process AX insert before touching the clipboard. Verified
    // AX success must not leave VoiceFlow text on the system clipboard, and AX
    // must not be treated as a posted Cmd+V (no 3s undo). Unverified AX copies
    // the text as the fail-closed manual fallback.
    if let Some(ax_value) = try_ax_insert_if_safe(&cancellation, &verify_target, text) {
        let outcome = build_insert_outcome(
            false,
            value_before.as_deref(),
            Some(ax_value),
            text,
            delivery_target_is_frontmost(expected_pid),
        );
        if ax_insert_is_final(&outcome) {
            return Ok(outcome);
        }
        // Cursor/VS Code often expose a settable AX stub that does not update
        // the visible Monaco input. Fall through to a process-targeted Cmd+V.
    }
    if cancellation.is_cancelled() {
        return Err(PasteError::Cancelled);
    }
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
    // Fall through to Cmd+V with a CJK→ABC input-source switch.
    // Suppress the modifier event-tap while we synthesize the paste keystroke:
    // our own keystroke must not be read as a physical hotkey tap, and re-entrant
    // event delivery during the paste aborts the main runloop (uncaught
    // NSException -> SIGABRT). Enigo is called synchronously here, but this
    // function is reached from `paste_text`'s `spawn_blocking` worker, never from
    // the AppKit/tao main-thread callback.
    let attempt = run_paste_attempt(&cancellation, verify_target, || {
        simulate_paste_with_fallback(expected_pid)
    });
    if should_restore_clipboard(&attempt) {
        // Restore only when Cmd+V was never posted. Once the shortcut is sent,
        // keep VoiceFlow's text available as the manual fallback.
        let _ = restore_clipboard(app, previous_clipboard.as_deref());
    }
    if attempt.result.is_ok() {
        thread::sleep(Duration::from_millis(250));
    }
    attempt.result.map(|()| {
        build_insert_outcome(
            true,
            value_before.as_deref(),
            crate::context::focused_input_value(),
            text,
            delivery_target_is_frontmost(expected_pid),
        )
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

/// Restore the recorded target only when VoiceFlow (or a missing frontmost
/// app) is in front. If the user switched to a different app, stay fail-closed.
pub fn should_restore_delivery_target(current_pid: i32, expected_pid: i32, own_pid: i32) -> bool {
    current_pid <= 0 || current_pid == expected_pid || current_pid == own_pid
}

pub fn restore_delivery_target_if_needed(
    expected_pid: i32,
    window_id: Option<u64>,
) -> Result<(), PasteError> {
    let current_pid = current_frontmost_pid();
    let own_pid = std::process::id() as i32;
    if should_restore_delivery_target(current_pid, expected_pid, own_pid) {
        activate_target_now(expected_pid)?;
        raise_target_window(expected_pid, window_id);
        thread::sleep(DELIVERY_FOCUS_SETTLE);
        Ok(())
    } else {
        Ok(())
    }
}

/// Bring the original target app back to the front before a user confirms a
/// selected-text preview. The preview window necessarily took focus, so the
/// target guard must be checked again after activation.
#[allow(dead_code)]
pub fn activate_target(pid: i32) -> Result<(), PasteError> {
    activate_target_now(pid)?;
    thread::sleep(DELIVERY_FOCUS_SETTLE);
    Ok(())
}

fn activate_target_now(pid: i32) -> Result<(), PasteError> {
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
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        Ok(())
    }
}

fn current_frontmost_pid() -> i32 {
    crate::context::frontmost_application_key().0
}

pub fn target_is_frontmost_for_delivery(current_pid: i32, expected_pid: i32, own_pid: i32) -> bool {
    expected_pid > 0 && expected_pid != own_pid && current_pid == expected_pid
}

fn delivery_target_is_frontmost(expected_pid: i32) -> bool {
    target_is_frontmost_for_delivery(
        current_frontmost_pid(),
        expected_pid,
        std::process::id() as i32,
    )
}

/// Retry the paste shortcut only while VoiceFlow (or no app) is still in front.
/// Never spray Cmd+V into a different user app.
pub fn should_retry_paste_with_applescript(
    current_pid: i32,
    expected_pid: i32,
    own_pid: i32,
) -> bool {
    expected_pid > 0
        && expected_pid != own_pid
        && current_pid != expected_pid
        && (current_pid <= 0 || current_pid == own_pid)
}

fn raise_target_window(pid: i32, window_id: Option<u64>) {
    let Some(window_id) = window_id.filter(|id| *id > 0) else {
        return;
    };
    #[cfg(target_os = "macos")]
    {
        macos_raise::raise_window(pid, window_id);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (pid, window_id);
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
        assert_eq!(
            APPLESCRIPT_PASTE,
            r#"tell application "System Events" to key code 9 using command down"#
        );
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
        assert!(!input_value_verifies_delivery(Some("aa"), Some("aa"), "a"));
        assert!(!input_value_verifies_delivery(Some("aa"), Some("aaa"), "a"));
        assert!(input_value_verifies_delivery(
            Some("hi"),
            Some("hi hello"),
            "hello"
        ));
    }

    #[test]
    fn input_verification_accepts_single_ascii_and_cjk_characters() {
        assert!(input_value_verifies_delivery(
            Some("before"),
            Some("beforex"),
            "x"
        ));
        assert!(input_value_verifies_delivery(
            Some("开头"),
            Some("开头字"),
            "字"
        ));
        assert!(!input_value_verifies_delivery(None, Some("inserted"), "i"));
        assert!(!input_value_verifies_delivery(None, Some("x"), "x"));
        assert!(!input_value_verifies_delivery(None, Some("字"), "字"));
        assert!(input_value_verifies_delivery(
            None,
            Some("inserted"),
            "inserted"
        ));
    }

    #[test]
    fn unverified_ax_insert_is_not_final_so_keyboard_paste_can_still_run() {
        let unverified = build_insert_outcome(false, None, None, "hello", true);
        assert!(!ax_insert_is_final(&unverified));
        let stub_empty = build_insert_outcome(false, Some(""), Some(String::new()), "hello", true);
        assert!(!ax_insert_is_final(&stub_empty));
        let verified = build_insert_outcome(false, Some(""), Some("hello".into()), "hello", false);
        assert!(ax_insert_is_final(&verified));
    }

    #[test]
    fn command_shortcuts_target_the_recorded_pid() {
        assert!(should_post_shortcut_to_pid(4242, 99));
        assert!(!should_post_shortcut_to_pid(99, 99));
        assert!(!should_post_shortcut_to_pid(0, 99));
        assert!(!should_post_shortcut_to_pid(-3, 99));
    }

    #[test]
    fn unverified_ax_insert_copies_clipboard_fallback_without_arming_undo() {
        let verified_ax =
            build_insert_outcome(false, Some(""), Some("hello".into()), "hello", false);
        assert!(!verified_ax.used_keyboard_paste);
        assert!(verified_ax.verified);
        assert!(!should_copy_clipboard_fallback(&verified_ax));

        let unverified_ax = build_insert_outcome(false, None, None, "hello", true);
        assert!(!unverified_ax.used_keyboard_paste);
        assert!(!unverified_ax.verified);
        assert!(should_copy_clipboard_fallback(&unverified_ax));

        let keyboard = build_insert_outcome(true, Some(""), Some("hello".into()), "hello", true);
        assert!(keyboard.used_keyboard_paste);
        assert!(!should_copy_clipboard_fallback(&keyboard));

        let keyboard_unverified = build_insert_outcome(true, None, Some("x".into()), "x", false);
        assert!(keyboard_unverified.used_keyboard_paste);
        assert!(!keyboard_unverified.verified);
        assert!(!should_copy_clipboard_fallback(&keyboard_unverified));
    }

    #[test]
    fn ax_success_outcome_is_not_a_keyboard_paste() {
        let ax = build_insert_outcome(false, Some(""), Some("hello".into()), "hello", false);
        assert!(ax.shortcut_sent);
        assert!(!ax.used_keyboard_paste);
        assert!(ax.verified);
        assert_eq!(ax.value_after.as_deref(), Some("hello"));

        let keyboard = build_insert_outcome(true, Some(""), Some("hello".into()), "hello", true);
        assert!(keyboard.used_keyboard_paste);
        assert!(keyboard.verified);
    }

    #[test]
    fn ax_path_verifies_against_the_returned_ax_value() {
        let ax_read = Some("你好世界".into());
        let outcome = build_insert_outcome(false, Some("你好"), ax_read.clone(), "世界", false);
        assert!(!outcome.used_keyboard_paste);
        assert!(outcome.verified);
        assert_eq!(outcome.value_after, ax_read);
        assert!(!should_copy_clipboard_fallback(&outcome));
    }

    #[test]
    fn empty_ax_reread_is_unverified_and_copies_clipboard() {
        let outcome = build_insert_outcome(false, Some(""), Some(String::new()), "hello", true);
        assert!(!outcome.used_keyboard_paste);
        assert!(!outcome.verified);
        assert!(should_copy_clipboard_fallback(&outcome));
    }

    #[test]
    fn single_char_unknown_before_is_unverified_even_for_keyboard_paste() {
        let outcome = build_insert_outcome(true, None, Some("x".into()), "x", false);
        assert!(outcome.used_keyboard_paste);
        assert!(!outcome.verified);
        assert!(outcome.post_insert_input_fingerprint.is_none());
    }

    #[test]
    fn keyboard_paste_is_verified_when_the_target_app_stays_frontmost() {
        let unread = build_insert_outcome(true, None, None, "hello there", true);
        assert!(unread.used_keyboard_paste);
        assert!(unread.verified);
        assert!(!should_copy_clipboard_fallback(&unread));

        let empty = build_insert_outcome(true, Some(""), Some(String::new()), "hello there", true);
        assert!(empty.used_keyboard_paste);
        assert!(empty.verified);
        assert!(!should_copy_clipboard_fallback(&empty));
    }

    #[test]
    fn keyboard_paste_is_not_verified_when_voiceflow_or_another_app_is_frontmost() {
        let voiceflow = build_insert_outcome(true, None, None, "hello there", false);
        assert!(voiceflow.used_keyboard_paste);
        assert!(!voiceflow.verified);
        assert!(!should_copy_clipboard_fallback(&voiceflow));
    }

    #[test]
    fn delivery_target_is_restored_only_when_voiceflow_or_the_original_app_is_frontmost() {
        assert!(should_restore_delivery_target(42, 42, 7));
        assert!(should_restore_delivery_target(7, 42, 7));
        assert!(should_restore_delivery_target(0, 42, 7));
        assert!(!should_restore_delivery_target(99, 42, 7));
    }

    #[test]
    fn delivery_is_verified_only_when_the_recorded_user_app_is_frontmost() {
        assert!(target_is_frontmost_for_delivery(42, 42, 7));
        assert!(!target_is_frontmost_for_delivery(7, 42, 7));
        assert!(!target_is_frontmost_for_delivery(99, 42, 7));
        assert!(!target_is_frontmost_for_delivery(7, 7, 7));
        assert!(!target_is_frontmost_for_delivery(0, 42, 7));
    }

    #[test]
    fn applescript_paste_retries_only_while_voiceflow_still_owns_frontmost() {
        assert!(should_retry_paste_with_applescript(7, 42, 7));
        assert!(should_retry_paste_with_applescript(0, 42, 7));
        assert!(!should_retry_paste_with_applescript(42, 42, 7));
        assert!(!should_retry_paste_with_applescript(99, 42, 7));
        assert!(!should_retry_paste_with_applescript(7, 7, 7));
    }

    #[test]
    fn delivery_focus_settle_matches_voiceink_pre_paste_delay() {
        assert!(DELIVERY_FOCUS_SETTLE >= Duration::from_millis(100));
        assert!(DELIVERY_FOCUS_SETTLE <= Duration::from_millis(150));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn paste_layout_is_held_until_cmd_v_can_be_consumed() {
        assert!(PASTE_CONSUME_SETTLE >= Duration::from_millis(30));
        assert!(PASTE_CONSUME_SETTLE <= Duration::from_millis(50));
    }

    #[test]
    fn ax_insert_skips_secure_and_non_text_roles() {
        assert_eq!(
            ax_insert_decision(
                "AXTextField",
                "AXSecureTextField",
                true,
                true,
                Some((0, 0)),
                true
            ),
            AxInsertDecision::SkipSecure
        );
        assert_eq!(
            ax_insert_decision("AXWebArea", "", true, true, Some((0, 0)), true),
            AxInsertDecision::SkipWrongRole
        );
        assert_eq!(
            ax_insert_decision("AXGroup", "", false, false, None, false),
            AxInsertDecision::SkipWrongRole
        );
    }

    #[test]
    fn ax_insert_uses_selected_text_when_settable() {
        assert_eq!(
            ax_insert_decision("AXTextField", "", true, false, None, false),
            AxInsertDecision::AttemptSelectedText
        );
        assert_eq!(
            ax_insert_decision("AXTextArea", "", true, true, Some((2, 0)), false),
            AxInsertDecision::AttemptSelectedText
        );
        assert_eq!(
            ax_insert_decision("AXComboBox", "", true, true, None, false),
            AxInsertDecision::AttemptSelectedText
        );
        assert_eq!(
            ax_insert_decision("AXSearchField", "", true, false, Some((0, 0)), true),
            AxInsertDecision::AttemptSelectedText
        );
    }

    #[test]
    fn ax_insert_splices_value_only_with_a_known_range() {
        assert_eq!(
            ax_insert_decision("AXTextField", "", false, true, Some((1, 0)), false),
            AxInsertDecision::AttemptValueSplice
        );
        assert_eq!(
            ax_insert_decision("AXTextArea", "", false, true, None, false),
            AxInsertDecision::SkipWouldReplaceAll
        );
        assert_eq!(
            ax_insert_decision("AXTextField", "", false, true, None, true),
            AxInsertDecision::AttemptValueSplice
        );
    }

    #[test]
    fn ax_insert_does_not_fall_back_to_a_keystroke_typer() {
        assert_eq!(
            ax_insert_decision("AXTextField", "", false, false, Some((0, 0)), false),
            AxInsertDecision::SkipNeedsKeystrokeTyper
        );
    }

    #[test]
    fn utf16_splice_inserts_cjk_at_utf16_range() {
        assert_eq!(
            utf16_splice("hello", 5, 0, "世界").as_deref(),
            Some("hello世界")
        );
        assert_eq!(
            utf16_splice("你好", 2, 0, "世界").as_deref(),
            Some("你好世界")
        );
        assert_eq!(utf16_splice("hello", 0, 5, "hi").as_deref(), Some("hi"));
        assert_eq!(utf16_splice("hello", 1, 3, "i").as_deref(), Some("hio"));
        assert_eq!(utf16_splice("hello", 6, 0, "x"), None);
        assert_eq!(utf16_splice("hello", 2, 10, "x"), None);
        assert_eq!(utf16_splice("hello", -1, 0, "x"), None);
    }

    #[test]
    fn ax_insert_is_skipped_when_cancelled_or_target_changed() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(try_ax_insert_if_safe(&cancellation, || Ok(()), "hello").is_none());

        let cancellation = CancellationToken::new();
        assert!(
            try_ax_insert_if_safe(&cancellation, || Err(PasteError::TargetChanged), "hello")
                .is_none()
        );
    }

    #[test]
    fn nested_selection_capture_does_not_clear_outer_paste_suppress() {
        use crate::modifier_hotkey::{is_paste_suppressed, PasteSuppressGuard};
        let cancellation = CancellationToken::new();
        let mut saw_suppressed_cmd_v = false;
        let attempt = run_paste_attempt(
            &cancellation,
            || {
                let _inner = PasteSuppressGuard::new();
                assert!(is_paste_suppressed());
                Ok(())
            },
            || {
                saw_suppressed_cmd_v = is_paste_suppressed();
                Ok(())
            },
        );
        assert!(attempt.result.is_ok());
        assert!(
            saw_suppressed_cmd_v,
            "Cmd+V must stay suppressed after nested capture"
        );
    }
}
