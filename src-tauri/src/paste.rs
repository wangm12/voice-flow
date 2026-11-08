use crate::delivery_diagnostics::{self, DeliveryDiagnostic, PasteVerificationFact};
#[cfg(all(target_os = "macos", feature = "research-harness"))]
use crate::learning_clipboard_guard::{
    ClipboardGuardCheckStageV1, ClipboardGuardClientErrorV1, ClipboardGuardOperationV1,
    ClipboardGuardReplyV1, ClipboardGuardRpcClientV1, ClipboardGuardRpcOutcomeV1,
    ClipboardSeedFixtureV1, ClipboardWriteFixtureV1, RegisteredNamedRemoteClipboardCaseV1,
    RegisteredOwnedPasteSessionRemoteContextV1, RemoteClipboardBoardV1, RemoteProbeChildInputV1,
    RemoteProbeChildRestoreV1, RemoteProbeChildResultKindV1, RemoteProbeChildResultV1,
};
#[cfg(target_os = "macos")]
use core_graphics::{
    event::{CGEvent, CGEventFlags, CGEventTapLocation},
    event_source::{CGEventSource, CGEventSourceStateID},
};
#[cfg(not(target_os = "macos"))]
use enigo::{Direction, Enigo, Key, Keyboard, NewConError, Settings};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(all(target_os = "macos", feature = "research-harness"))]
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
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
    #[error("secure input prevents automatic paste")]
    SecureInput,
    #[error("system secure input is owned by another or unknown process")]
    SystemSecureInputOtherOwner,
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
    #[error("Accessibility edit may have changed the field and was not retried")]
    MutationUncertain,
    #[error("{source}")]
    Diagnosed {
        source: Box<PasteError>,
        diagnostic: Box<DeliveryDiagnostic>,
    },
}

impl PasteError {
    pub(crate) fn delivery_diagnostic(&self) -> Option<&DeliveryDiagnostic> {
        match self {
            Self::Diagnosed { diagnostic, .. } => Some(diagnostic.as_ref()),
            _ => None,
        }
    }
}

/// Physical ANSI keycodes stay stable when the active input source is Chinese,
/// Japanese, or another non-Latin layout. `Key::Unicode('v')` is not safe here:
/// Enigo resolves it through the current layout and silently falls back to 0
/// when no matching key is found.
#[cfg(target_os = "macos")]
const COMMAND_PASTE_KEYCODE: u16 = 0x09;
#[cfg(target_os = "macos")]
const COMMAND_UNDO_KEYCODE: u16 = 0x06;
const POST_PASTE_READBACK_DELAY_MS: u64 = 250;
const POST_PASTE_READBACK_POLL_DELAYS_MS: [u64; 10] = [0, 75, 75, 75, 75, 150, 200, 300, 400, 450];
pub(crate) const MAX_FULL_RANGE_READBACK_UTF16: usize = 65_536;
const FULL_RANGE_AX_TIMEOUT_SECONDS: f32 = 0.15;
static NEXT_ANCHOR_TICKET_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SecureInputProbeResult {
    Disabled,
    Enabled { owner_pid: Option<i32> },
}

// Apple documents IsSecureEventInputEnabled as not thread safe. Every
// production probe goes through this lock, including probes from independent
// delivery-owner threads.
static SECURE_INPUT_PROBE_LOCK: Mutex<()> = Mutex::new(());

fn serialized_secure_input_probe(
    lock: &Mutex<()>,
    cancellation: &CancellationToken,
    probe: impl FnOnce() -> SecureInputProbeResult,
) -> Result<SecureInputProbeResult, PasteError> {
    if cancellation.is_cancelled() {
        return Err(PasteError::Cancelled);
    }
    let _guard = match lock.lock() {
        Ok(guard) => guard,
        // An unknown system state must fail closed to copy/History.
        Err(_) => {
            return Ok(SecureInputProbeResult::Enabled { owner_pid: None });
        }
    };
    if cancellation.is_cancelled() {
        return Err(PasteError::Cancelled);
    }
    let result = probe();
    if cancellation.is_cancelled() {
        return Err(PasteError::Cancelled);
    }
    Ok(result)
}

fn secure_input_admission(
    probe: impl FnOnce() -> SecureInputProbeResult,
    captured_target_pid: i32,
) -> Result<(), PasteError> {
    match probe() {
        SecureInputProbeResult::Disabled => Ok(()),
        SecureInputProbeResult::Enabled { owner_pid }
            if captured_target_pid > 0 && owner_pid == Some(captured_target_pid) =>
        {
            Err(PasteError::SecureInput)
        }
        SecureInputProbeResult::Enabled { .. } => Err(PasteError::SystemSecureInputOtherOwner),
    }
}

fn secure_input_admission_with_probe(
    lock: &Mutex<()>,
    probe: impl FnOnce() -> SecureInputProbeResult,
    cancellation: &CancellationToken,
    captured_target_pid: i32,
) -> Result<(), PasteError> {
    let result = serialized_secure_input_probe(lock, cancellation, probe)?;
    secure_input_admission(|| result, captured_target_pid)
}

#[cfg(target_os = "macos")]
fn system_secure_input_probe_unserialized() -> SecureInputProbeResult {
    use core_foundation::{
        base::{CFType, TCFType},
        dictionary::{CFDictionary, CFDictionaryRef},
        number::CFNumber,
        string::CFString,
    };
    use std::ffi::c_void;

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn IsSecureEventInputEnabled() -> u8;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
    }

    if unsafe { IsSecureEventInputEnabled() } == 0 {
        return SecureInputProbeResult::Disabled;
    }

    let dictionary_ref = unsafe { CGSessionCopyCurrentDictionary() };
    if dictionary_ref.is_null() {
        return SecureInputProbeResult::Enabled { owner_pid: None };
    }
    let dictionary = unsafe {
        CFDictionary::<*const c_void, *const c_void>::wrap_under_create_rule(dictionary_ref)
    };
    let key = CFString::new("kCGSSessionSecureInputPID");
    let value_ref = unsafe {
        core_foundation::dictionary::CFDictionaryGetValue(
            dictionary.as_concrete_TypeRef(),
            key.as_CFTypeRef(),
        )
    };
    if value_ref.is_null() {
        return SecureInputProbeResult::Enabled { owner_pid: None };
    }
    let value_type = unsafe { CFType::wrap_under_get_rule(value_ref) };
    if value_type.type_of() != CFNumber::type_id() {
        return SecureInputProbeResult::Enabled { owner_pid: None };
    }
    let owner_pid = unsafe { CFNumber::wrap_under_get_rule(value_ref as _) }
        .to_i64()
        .and_then(|pid| i32::try_from(pid).ok())
        .filter(|pid| *pid > 0);
    SecureInputProbeResult::Enabled { owner_pid }
}

#[cfg(not(target_os = "macos"))]
fn system_secure_input_probe_unserialized() -> SecureInputProbeResult {
    SecureInputProbeResult::Disabled
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FullRangeReadback {
    Verified,
    Mismatch,
    Unavailable,
}

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

#[cfg(target_os = "macos")]
fn send_command_shortcut(keycode: u16) -> Result<(), PasteError> {
    let shortcut = prepare_command_shortcut(keycode)?;
    send_prepared_command_shortcut(shortcut, None, &mut || {})
}

#[cfg(target_os = "macos")]
struct PreparedCommandShortcut {
    key_down: CGEvent,
    key_up: CGEvent,
}

#[cfg(target_os = "macos")]
fn prepare_command_shortcut(keycode: u16) -> Result<PreparedCommandShortcut, PasteError> {
    if !crate::permissions::accessibility_is_trusted() {
        return Err(PasteError::Accessibility);
    }
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| PasteError::Input("failed to create macOS keyboard event source".into()))?;
    let key_down = CGEvent::new_keyboard_event(source.clone(), keycode, true).map_err(|_| {
        PasteError::Input("failed to create Command shortcut key-down event".into())
    })?;
    key_down.set_flags(CGEventFlags::CGEventFlagCommand);
    let key_up = CGEvent::new_keyboard_event(source, keycode, false)
        .map_err(|_| PasteError::Input("failed to create Command shortcut key-up event".into()))?;
    key_up.set_flags(CGEventFlags::CGEventFlagCommand);
    Ok(PreparedCommandShortcut { key_down, key_up })
}

#[cfg(target_os = "macos")]
fn send_prepared_command_shortcut(
    shortcut: PreparedCommandShortcut,
    target_pid: Option<i32>,
    on_may_have_posted: &mut dyn FnMut(),
) -> Result<(), PasteError> {
    // All fallible native event construction is complete before this callback
    // and the first irreversible post.
    on_may_have_posted();
    post_command_event(&shortcut.key_down, target_pid);
    post_command_event(&shortcut.key_up, target_pid);
    Ok(())
}

#[cfg(any(all(target_os = "macos", feature = "research-harness"), test))]
fn registered_post_target_pid(expected_pid: i32, own_pid: i32) -> Option<i32> {
    should_post_shortcut_to_pid(expected_pid, own_pid).then_some(expected_pid)
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn send_prepared_command_shortcut_to_registered_pid(
    shortcut: PreparedCommandShortcut,
    expected_pid: i32,
    cancellation: &CancellationToken,
    expires_at: std::time::Instant,
    on_may_have_posted: &mut dyn FnMut(),
) -> Result<(), PasteError> {
    let target_pid = registered_post_target_pid(expected_pid, std::process::id() as i32)
        .ok_or(PasteError::TargetChanged)?;
    // The registered research route never falls back to the ambient HID tap.
    // All event construction and target checks completed before this point.
    with_registered_post_permission(cancellation, expires_at, || {
        on_may_have_posted();
        shortcut.key_down.post_to_pid(target_pid);
        shortcut.key_up.post_to_pid(target_pid);
        Ok(())
    })
}

#[cfg(not(target_os = "macos"))]
struct PreparedCommandShortcut {
    enigo: Enigo,
}

#[cfg(not(target_os = "macos"))]
fn prepare_command_shortcut(_keycode: u16) -> Result<PreparedCommandShortcut, PasteError> {
    let enigo = Enigo::new(&Settings::default()).map_err(map_enigo_connection_error)?;
    Ok(PreparedCommandShortcut { enigo })
}

#[cfg(target_os = "macos")]
fn simulate_paste(
    expected_pid: i32,
    shortcut: PreparedCommandShortcut,
    on_may_have_posted: &mut dyn FnMut(),
) -> Result<(), PasteError> {
    send_prepared_command_shortcut(shortcut, Some(expected_pid), on_may_have_posted)?;
    thread::sleep(PASTE_CONSUME_SETTLE);
    Ok(())
}

fn simulate_paste_once(
    expected_pid: i32,
    shortcut: PreparedCommandShortcut,
    on_may_have_posted: &mut dyn FnMut(),
) -> Result<(), PasteError> {
    // A posted Cmd+V may have been accepted even when there is no immediate
    // readback. Never send a second shortcut based only on frontmost-app
    // heuristics: that can duplicate text in editors with delayed AX updates.
    simulate_paste(expected_pid, shortcut, on_may_have_posted)?;
    Ok(())
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn simulate_paste_once_to_registered_pid(
    expected_pid: i32,
    shortcut: PreparedCommandShortcut,
    cancellation: &CancellationToken,
    expires_at: std::time::Instant,
    on_may_have_posted: &mut dyn FnMut(),
) -> Result<(), PasteError> {
    send_prepared_command_shortcut_to_registered_pid(
        shortcut,
        expected_pid,
        cancellation,
        expires_at,
        on_may_have_posted,
    )?;
    thread::sleep(PASTE_CONSUME_SETTLE);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn simulate_paste(
    _expected_pid: i32,
    mut shortcut: PreparedCommandShortcut,
    on_may_have_posted: &mut dyn FnMut(),
) -> Result<(), PasteError> {
    on_may_have_posted();
    shortcut
        .enigo
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
        enigo: &mut shortcut.enigo,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextActionSourceKind {
    Selection,
    FieldText,
    EmptyComposer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CapturedTextActionSource {
    pub(crate) kind: TextActionSourceKind,
    pub(crate) text: String,
    pub(crate) text_fingerprint: u64,
    pub(crate) field_fingerprint: Option<u64>,
    /// AX text ranges use UTF-16 offsets, matching Cocoa's NSRange contract.
    pub(crate) selection_range: Option<(i64, i64)>,
    pub(crate) editable: bool,
    /// True when AX cannot expose both the exact selection range and its
    /// whole-field version. Such a source is generation-only and copy-only.
    pub(crate) copy_only_selection: bool,
}

/// A retained Accessibility element gives post-paste observation a native
/// same-field anchor. It is captured by the original paste owner and read only
/// by that owner thread; it is never recaptured from whichever app is frontmost
/// later.
pub(crate) struct FocusedFieldAnchor {
    #[cfg(target_os = "macos")]
    inner: macos_ax::FocusedFieldAnchor,
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    registered_metadata: RegisteredResearchFieldMetadata,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
struct RegisteredResearchFieldMetadata {
    window_title: Option<String>,
    control_identifier: Option<String>,
    secure_input: bool,
}

/// Transient Accessibility facts captured with the retained focused-element
/// anchor. These values stay inside the process; callers derive bounded
/// fingerprints and coarse roles without exporting field text or labels.
pub(crate) struct FocusedFieldMetadata {
    pub(crate) role: Option<String>,
    pub(crate) subrole: Option<String>,
    pub(crate) description: Option<String>,
    #[cfg(any(test, feature = "research-harness"))]
    pub(crate) identifier: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) position: Option<String>,
    pub(crate) size: Option<String>,
    pub(crate) window_title: Option<String>,
    pub(crate) window_document_path: Option<PathBuf>,
    pub(crate) window_position: Option<String>,
    pub(crate) window_size: Option<String>,
    /// Whether the focused element's AXWindow is the same object as the
    /// application's AXFocusedWindow. Unknown when either AX read is absent.
    pub(crate) ax_window_matches_app_focused_window: Option<bool>,
    pub(crate) secure_input: bool,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn registered_research_field_metadata_matches(
    observed_window_title: Option<&str>,
    observed_control_identifier: Option<&str>,
    secure_input: bool,
    expected_window_title: &str,
    expected_control_identifier: &str,
) -> bool {
    !secure_input
        && observed_window_title == Some(expected_window_title)
        && observed_control_identifier == Some(expected_control_identifier)
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn registered_remote_initial_source_matches(
    value: Option<&str>,
    selection_range: Option<(i64, i64)>,
) -> Result<(), PasteError> {
    if value != Some(REGISTERED_RESEARCH_HOST_INITIAL_FIELD) {
        return Err(PasteError::InputChanged);
    }
    if selection_range != Some(REGISTERED_RESEARCH_HOST_INITIAL_SELECTION) {
        return Err(PasteError::SelectionChanged);
    }
    Ok(())
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn with_registered_remote_initial_source<T>(
    value: Option<&str>,
    selection_range: Option<(i64, i64)>,
    on_admitted: impl FnOnce() -> T,
) -> Result<T, PasteError> {
    registered_remote_initial_source_matches(value, selection_range)?;
    Ok(on_admitted())
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn verify_registered_remote_source_on_anchor(
    anchor: Option<&FocusedFieldAnchor>,
) -> Result<(), PasteError> {
    let anchor = anchor.ok_or(PasteError::InputUnavailable)?;
    if !anchor.is_current_focus() {
        return Err(PasteError::InputChanged);
    }
    let value = anchor.read_value().ok_or(PasteError::InputUnavailable)?;
    if !anchor.is_current_focus() {
        return Err(PasteError::InputChanged);
    }
    let selection_range = anchor
        .read_selected_range()
        .ok_or(PasteError::SelectionUnavailable)?;
    if !anchor.is_current_focus() {
        return Err(PasteError::InputChanged);
    }
    registered_remote_initial_source_matches(Some(&value), Some(selection_range))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusedFieldCaptureFailure {
    ObservationUnavailable,
    #[cfg(not(target_os = "macos"))]
    UnsupportedPlatform,
}

impl FocusedFieldAnchor {
    pub(crate) fn capture(pid: i32) -> Option<Self> {
        Self::capture_with_metadata(pid)
            .ok()
            .and_then(|(anchor, metadata)| (!metadata.secure_input).then_some(anchor))
    }

    pub(crate) fn capture_with_metadata(
        pid: i32,
    ) -> Result<(Self, FocusedFieldMetadata), FocusedFieldCaptureFailure> {
        #[cfg(target_os = "macos")]
        {
            let (inner, metadata) = macos_ax::FocusedFieldAnchor::capture_with_metadata(pid)?;
            #[cfg(feature = "research-harness")]
            let registered_metadata = RegisteredResearchFieldMetadata {
                window_title: metadata.window_title.clone(),
                control_identifier: metadata.identifier.clone(),
                secure_input: metadata.secure_input,
            };
            Ok((
                Self {
                    inner,
                    #[cfg(feature = "research-harness")]
                    registered_metadata,
                },
                metadata,
            ))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = pid;
            Err(FocusedFieldCaptureFailure::UnsupportedPlatform)
        }
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    fn matches_registered_research_field(
        &self,
        window_title: &str,
        control_identifier: &str,
    ) -> bool {
        registered_research_field_metadata_matches(
            self.registered_metadata.window_title.as_deref(),
            self.registered_metadata.control_identifier.as_deref(),
            self.registered_metadata.secure_input,
            window_title,
            control_identifier,
        )
    }

    /// Distinguishes a confirmed changed anchor from a failed native AX read.
    /// The legacy boolean projection remains conservative for paste callers.
    pub(crate) fn current_focus_matches(&self) -> Option<bool> {
        #[cfg(target_os = "macos")]
        {
            self.inner.current_focus_matches()
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    pub(crate) fn is_current_focus(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.inner.is_current_focus()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    pub(crate) fn read_value(&self) -> Option<String> {
        #[cfg(target_os = "macos")]
        {
            self.inner.read_value()
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    pub(crate) fn read_value_bounded(&self, max_utf16: usize) -> Option<String> {
        #[cfg(target_os = "macos")]
        {
            self.inner.read_value_bounded(max_utf16)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = max_utf16;
            None
        }
    }

    fn read_full_range_exact(
        &self,
        expected: &str,
        expected_caret_utf16: u64,
    ) -> FullRangeReadback {
        #[cfg(target_os = "macos")]
        {
            self.inner
                .read_full_range_exact(expected, expected_caret_utf16)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (expected, expected_caret_utf16);
            FullRangeReadback::Unavailable
        }
    }

    fn read_selected_text(&self) -> Option<String> {
        #[cfg(target_os = "macos")]
        {
            self.inner.read_selected_text()
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    fn read_selected_text_bounded(&self, max_utf16: usize) -> Option<String> {
        #[cfg(target_os = "macos")]
        {
            self.inner.read_selected_text_bounded(max_utf16)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = max_utf16;
            None
        }
    }

    fn read_selected_range(&self) -> Option<(i64, i64)> {
        #[cfg(target_os = "macos")]
        {
            self.inner.read_selected_range()
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    fn can_replace_text(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.inner.can_replace_text() || self.inner.can_replace_full_field()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    fn can_replace_text_for_admitted_value(
        &self,
        field_value: &str,
        selection_range: Option<(i64, i64)>,
    ) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.inner
                .can_replace_text_for_admitted_value(field_value, selection_range)
                || self
                    .inner
                    .can_replace_full_field_for_admitted_value(field_value)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (field_value, selection_range);
            false
        }
    }
}

/// Capture only a focused AX field and its current selection. This action
/// source never reads the clipboard, so a missing selection cannot reuse an
/// unrelated previous clipboard value.
pub(crate) fn capture_text_action_source_for_target(
    accessibility: bool,
    expected_target: &crate::context::TargetAppGuard,
    mut verify_target: impl FnMut() -> Result<(), PasteError>,
) -> Result<CapturedTextActionSource, PasteError> {
    if !accessibility {
        return Err(PasteError::Accessibility);
    }
    verify_target()?;
    let before = crate::context::probe_focus_guard();
    if before.secure_input {
        return Err(PasteError::SecureInput);
    }
    verify_same_field_target(expected_target, &before)?;
    let anchor =
        FocusedFieldAnchor::capture(expected_target.pid).ok_or(PasteError::InputUnavailable)?;
    if !anchor.is_current_focus() {
        return Err(PasteError::TargetChanged);
    }
    let value = anchor.read_value();
    let selection_range = anchor.read_selected_range();
    let ax_selected = anchor.read_selected_text();
    let editable = anchor.can_replace_text();
    verify_target()?;
    let after = crate::context::probe_focus_guard();
    verify_same_field_target(expected_target, &after)?;
    if !anchor.is_current_focus() {
        return Err(PasteError::TargetChanged);
    }

    let (kind, text, stable_range) =
        resolve_text_action_source(value.as_deref(), selection_range, ax_selected)?;
    if text.len() > crate::text_action::MAX_ACTION_SOURCE_BYTES {
        return Err(PasteError::Input(
            "text action source exceeds the safe size limit".into(),
        ));
    }
    Ok(CapturedTextActionSource {
        text_fingerprint: selection_fingerprint(&text),
        field_fingerprint: value.as_deref().map(selection_fingerprint),
        kind,
        text,
        selection_range: stable_range,
        editable,
        copy_only_selection: kind == TextActionSourceKind::Selection
            && (stable_range.is_none() || value.is_none()),
    })
}

/// Read the action source from the same PID-bound focused AX element that
/// supplied the caller's native capture facts. The returned whole-field value
/// is transient and exists only so fixed-fixture owners can compare their
/// exact expected document before authorizing replacement.
pub(crate) fn capture_text_action_source_from_anchor(
    anchor: FocusedFieldAnchor,
    expected_full_value: &str,
    max_field_utf16: usize,
    mut verify_target: impl FnMut() -> Result<(), PasteError>,
) -> Result<(CapturedTextActionSource, String), PasteError> {
    verify_target()?;
    match anchor.current_focus_matches() {
        Some(true) => {}
        Some(false) => return Err(PasteError::TargetChanged),
        None => return Err(PasteError::InputUnavailable),
    }
    let (source, value) = capture_action_source_after_fixture_admission(
        expected_full_value,
        max_field_utf16,
        |limit| anchor.read_value_bounded(limit),
        || anchor.read_selected_range(),
        |limit| anchor.read_selected_text_bounded(limit),
        |field_value, selection_range| {
            anchor.can_replace_text_for_admitted_value(field_value, selection_range)
        },
    )?;
    verify_target()?;
    match anchor.current_focus_matches() {
        Some(true) => {}
        Some(false) => return Err(PasteError::TargetChanged),
        None => return Err(PasteError::InputUnavailable),
    }
    Ok((source, value))
}

fn capture_action_source_after_fixture_admission(
    expected_full_value: &str,
    max_field_utf16: usize,
    mut read_bounded_value: impl FnMut(usize) -> Option<String>,
    mut read_selection_range: impl FnMut() -> Option<(i64, i64)>,
    mut read_bounded_selected_text: impl FnMut(usize) -> Option<String>,
    mut can_replace: impl FnMut(&str, Option<(i64, i64)>) -> bool,
) -> Result<(CapturedTextActionSource, String), PasteError> {
    let value = read_bounded_value(max_field_utf16).ok_or(PasteError::InputUnavailable)?;
    if value != expected_full_value {
        return Err(PasteError::InputChanged);
    }
    // The exact bounded fixture value is admitted before reading selection or
    // editability. The same value/range facts supply editability without a
    // second, unbounded AXValue read.
    let selection_range = read_selection_range();
    let ax_selected = read_bounded_selected_text(max_field_utf16);
    let editable = can_replace(&value, selection_range);
    let (kind, text, stable_range) =
        resolve_text_action_source(Some(&value), selection_range, ax_selected)?;
    if text.len() > crate::text_action::MAX_ACTION_SOURCE_BYTES {
        return Err(PasteError::Input(
            "text action source exceeds the safe size limit".into(),
        ));
    }
    let source = CapturedTextActionSource {
        text_fingerprint: selection_fingerprint(&text),
        field_fingerprint: Some(selection_fingerprint(&value)),
        kind,
        text,
        selection_range: stable_range,
        editable,
        copy_only_selection: kind == TextActionSourceKind::Selection && stable_range.is_none(),
    };
    Ok((source, value))
}

type ResolvedTextActionSource = (TextActionSourceKind, String, Option<(i64, i64)>);

fn resolve_text_action_source(
    value: Option<&str>,
    selection_range: Option<(i64, i64)>,
    ax_selected: Option<String>,
) -> Result<ResolvedTextActionSource, PasteError> {
    let selected_from_range = selection_range
        .filter(|(location, length)| *location >= 0 && *length > 0)
        .and_then(|(location, length)| {
            value.and_then(|value| utf16_range_to_bytes(value, location, length))
        })
        .and_then(|(start, end)| value.map(|value| value[start..end].to_owned()));
    let result = match selection_range {
        Some((location, length)) if location >= 0 && length > 0 => {
            match (selected_from_range, ax_selected) {
                (Some(from_range), Some(from_ax)) if from_range != from_ax => {
                    return Err(PasteError::SelectionChanged);
                }
                (Some(from_range), _) => {
                    (TextActionSourceKind::Selection, from_range, selection_range)
                }
                (None, Some(from_ax)) if !from_ax.trim().is_empty() => {
                    // AX-selected-text without a matching field range remains
                    // useful for generation, but never authorizes replacement.
                    (TextActionSourceKind::Selection, from_ax, None)
                }
                (None, _) => return Err(PasteError::SelectionUnavailable),
            }
        }
        Some((location, length)) if location < 0 || length < 0 => {
            return Err(PasteError::SelectionUnavailable);
        }
        _ => match ax_selected {
            Some(from_ax) if !from_ax.trim().is_empty() => {
                // AX-selected-text without an exact range can be used as model
                // input, but cannot authorize replacement.
                (TextActionSourceKind::Selection, from_ax, None)
            }
            _ if value.is_some_and(|value| !value.is_empty()) => (
                TextActionSourceKind::FieldText,
                value.unwrap_or_default().to_owned(),
                selection_range.filter(|(_, length)| *length == 0),
            ),
            _ if value.is_some_and(str::is_empty) => (
                TextActionSourceKind::EmptyComposer,
                String::new(),
                selection_range,
            ),
            _ => return Err(PasteError::InputUnavailable),
        },
    };
    Ok(result)
}

pub(crate) fn text_action_source_matches(
    expected: &CapturedTextActionSource,
    current: &CapturedTextActionSource,
) -> bool {
    expected.kind == current.kind
        && expected.text == current.text
        && expected.text_fingerprint == current.text_fingerprint
        && expected.field_fingerprint == current.field_fingerprint
        && expected.selection_range == current.selection_range
        && expected.editable == current.editable
        && expected.copy_only_selection == current.copy_only_selection
}

fn utf16_range_to_bytes(text: &str, location: i64, length: i64) -> Option<(usize, usize)> {
    if location < 0 || length <= 0 {
        return None;
    }
    let start = usize::try_from(location).ok()?;
    let end = start.checked_add(usize::try_from(length).ok()?)?;
    let mut offset = 0usize;
    let mut byte_start = (start == 0).then_some(0);
    let mut byte_end = None;
    for (byte, character) in text.char_indices() {
        if offset == start {
            byte_start = Some(byte);
        }
        if offset == end {
            byte_end = Some(byte);
            break;
        }
        offset = offset.checked_add(character.len_utf16())?;
    }
    if offset == start {
        byte_start = Some(text.len());
    }
    if byte_end.is_none() && offset == end {
        byte_end = Some(text.len());
    }
    let byte_start = byte_start?;
    let byte_end = byte_end?;
    (byte_start < byte_end).then_some((byte_start, byte_end))
}

#[cfg(target_os = "macos")]
enum ClipboardSnapshotBackingV1 {
    Native(macos_pasteboard::Snapshot),
    #[cfg(feature = "research-harness")]
    RemoteResearch {
        board: RemoteClipboardBoardV1,
        session: RemoteResearchClipboardSessionV1,
        client: Arc<dyn ClipboardGuardRpcClientV1>,
        lease_id: u64,
        capture_fresh_unchanged: Mutex<Option<bool>>,
        write_id: Mutex<Option<u64>>,
        last_rpc_error: Mutex<Option<ClipboardGuardClientErrorV1>>,
        last_rpc_outcome: Box<Mutex<Option<ClipboardGuardRpcOutcomeV1>>>,
    },
}

struct ClipboardSnapshot {
    #[cfg(target_os = "macos")]
    backing: ClipboardSnapshotBackingV1,
    #[cfg(not(target_os = "macos"))]
    text: Option<String>,
}

/// Parent-owned snapshot and ownership proof for named or General research
/// clipboard guards. Contents, temporary text, token, and marker proof remain
/// private in this value and never cross the child socket protocol.
#[cfg(all(target_os = "macos", feature = "research-harness"))]
enum ResearchClipboardBoardV1 {
    Named(String),
    General,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
pub(crate) struct ResearchClipboardGuardOwnerV1 {
    board: ResearchClipboardBoardV1,
    snapshot: macos_pasteboard::Snapshot,
    temporary_text: Option<&'static str>,
    ownership_token: Option<String>,
    write_proof: Option<ClipboardWriteProof>,
    write_attempted: bool,
    restored: bool,
    ownership_changed: bool,
    closed: bool,
    #[cfg(test)]
    test_fail_current_read: bool,
    #[cfg(test)]
    test_fail_restore_prepare: bool,
    #[cfg(test)]
    test_current_observation: Option<ClipboardWriteObservation>,
    #[cfg(test)]
    test_restore_observation: Option<ClipboardWriteObservation>,
    #[cfg(test)]
    test_restore_commit_attempted: bool,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
impl ResearchClipboardGuardOwnerV1 {
    pub(crate) fn capture_named_fixture(
        seed: ClipboardSeedFixtureV1,
    ) -> Result<Self, ClipboardGuardReplyV1> {
        match macos_pasteboard::capture_named_fixture(seed) {
            Ok((named_board, snapshot)) => Ok(Self {
                board: ResearchClipboardBoardV1::Named(named_board),
                snapshot,
                temporary_text: None,
                ownership_token: None,
                write_proof: None,
                write_attempted: false,
                restored: false,
                ownership_changed: false,
                closed: false,
                #[cfg(test)]
                test_fail_current_read: false,
                #[cfg(test)]
                test_fail_restore_prepare: false,
                #[cfg(test)]
                test_current_observation: None,
                #[cfg(test)]
                test_restore_observation: None,
                #[cfg(test)]
                test_restore_commit_attempted: false,
            }),
            Err(reply) => Err(reply),
        }
    }

    pub(crate) fn capture_general() -> Result<Self, ClipboardGuardReplyV1> {
        let snapshot = macos_pasteboard::capture_general_for_guard()?;
        Ok(Self {
            board: ResearchClipboardBoardV1::General,
            snapshot,
            temporary_text: None,
            ownership_token: None,
            write_proof: None,
            write_attempted: false,
            restored: false,
            ownership_changed: false,
            closed: false,
            #[cfg(test)]
            test_fail_current_read: false,
            #[cfg(test)]
            test_fail_restore_prepare: false,
            #[cfg(test)]
            test_current_observation: None,
            #[cfg(test)]
            test_restore_observation: None,
            #[cfg(test)]
            test_restore_commit_attempted: false,
        })
    }

    pub(crate) fn write_fixture(
        &mut self,
        fixture: ClipboardWriteFixtureV1,
    ) -> ClipboardGuardReplyV1 {
        if self.closed {
            return ClipboardGuardReplyV1::Rejected;
        }
        match fixture {
            ClipboardWriteFixtureV1::ExternalCopyV1 => {
                // This fixed external-copy fixture is parent-side only; the
                // child wire parser rejects it.
                if !self.write_attempted {
                    return ClipboardGuardReplyV1::Rejected;
                }
                match &self.board {
                    ResearchClipboardBoardV1::Named(named_board) => {
                        match macos_pasteboard::write_external_fixture(named_board) {
                            Ok(()) => {
                                self.ownership_changed = true;
                                ClipboardGuardReplyV1::OwnershipChanged
                            }
                            Err(_) => ClipboardGuardReplyV1::WritePartialUnknown,
                        }
                    }
                    ResearchClipboardBoardV1::General => ClipboardGuardReplyV1::Rejected,
                }
            }
            ClipboardWriteFixtureV1::RegisteredTemporaryTextV1 => {
                self.write_registered_temporary_text_for_board()
            }
        }
    }

    /// Writes only the fixed synthetic value registered for this research
    /// component. No caller-provided text or flavor can enter here.
    pub(crate) fn write_registered_temporary_text(&mut self) -> ClipboardGuardReplyV1 {
        if !matches!(&self.board, ResearchClipboardBoardV1::General) {
            return ClipboardGuardReplyV1::Rejected;
        }
        self.write_registered_temporary_text_for_board()
    }

    pub(crate) fn inject_named_external_copy_for_probe(
        &mut self,
    ) -> Result<(), ClipboardGuardReplyV1> {
        if self.closed || self.write_attempted || self.restored {
            return Err(ClipboardGuardReplyV1::Rejected);
        }
        let ResearchClipboardBoardV1::Named(named_board) = &self.board else {
            return Err(ClipboardGuardReplyV1::Rejected);
        };
        match macos_pasteboard::write_external_fixture(named_board) {
            Ok(()) => Ok(()),
            Err(_) => Err(ClipboardGuardReplyV1::WritePartialUnknown),
        }
    }

    pub(crate) fn fresh_unchanged_since_capture(&mut self) -> Result<bool, ClipboardGuardReplyV1> {
        if self.closed || self.write_attempted || self.restored {
            return Err(ClipboardGuardReplyV1::Rejected);
        }
        if self.ownership_changed {
            return Ok(false);
        }
        match self.unchanged_since_capture() {
            Ok(true) => Ok(true),
            Ok(false) => {
                self.ownership_changed = true;
                Ok(false)
            }
            Err(_) => Err(ClipboardGuardReplyV1::ReadUnknown),
        }
    }

    fn write_registered_temporary_text_for_board(&mut self) -> ClipboardGuardReplyV1 {
        if self.closed || self.write_attempted || self.restored {
            return ClipboardGuardReplyV1::Rejected;
        }
        match self.unchanged_since_capture() {
            Ok(true) => {}
            Ok(false) => {
                self.ownership_changed = true;
                return ClipboardGuardReplyV1::OwnershipChanged;
            }
            Err(_) => return ClipboardGuardReplyV1::ReadUnknown,
        }

        self.write_attempted = true;
        let text = self.registered_temporary_text();
        let token = clipboard_ownership_token();
        self.temporary_text = Some(text);
        self.ownership_token = Some(token.clone());
        let write = match &self.board {
            ResearchClipboardBoardV1::Named(named_board) => {
                macos_pasteboard::write_named_temporary_text(
                    named_board,
                    &self.snapshot,
                    text,
                    &token,
                )
            }
            ResearchClipboardBoardV1::General => macos_pasteboard::write_temporary_text(
                text,
                &token,
                self.snapshot.change_count,
                &self.snapshot.items,
            ),
        };
        match self.capture_current_proof() {
            Ok(ResearchProofObservation::Owned) if write.is_ok() => {
                ClipboardGuardReplyV1::WriteOwned
            }
            Ok(ResearchProofObservation::Unknown) | Err(()) => ClipboardGuardReplyV1::ReadUnknown,
            Ok(ResearchProofObservation::Owned | ResearchProofObservation::Changed) => {
                ClipboardGuardReplyV1::WritePartialUnknown
            }
        }
    }

    fn registered_temporary_text(&self) -> &'static str {
        match &self.board {
            ResearchClipboardBoardV1::Named(_) => macos_pasteboard::registered_temporary_text(),
            ResearchClipboardBoardV1::General => {
                macos_pasteboard::registered_general_temporary_text()
            }
        }
    }

    fn unchanged_since_capture(&self) -> Result<bool, PasteError> {
        match &self.board {
            ResearchClipboardBoardV1::Named(named_board) => {
                macos_pasteboard::unchanged_since_capture(named_board, &self.snapshot)
            }
            ResearchClipboardBoardV1::General => {
                Ok(macos_pasteboard::change_count()? == self.snapshot.change_count)
            }
        }
    }

    #[cfg(test)]
    fn named_board_id(&self) -> Option<&str> {
        match &self.board {
            ResearchClipboardBoardV1::Named(named_board) => Some(named_board),
            ResearchClipboardBoardV1::General => None,
        }
    }

    fn release_general_owner_memory(&mut self) {
        self.snapshot.items = Vec::new();
        self.temporary_text = None;
        self.ownership_token = None;
        self.write_proof = None;
    }

    #[cfg(test)]
    fn synthetic_general_owner_for_test(
        snapshot: macos_pasteboard::Snapshot,
        proof: ClipboardWriteProof,
    ) -> Self {
        Self {
            board: ResearchClipboardBoardV1::General,
            temporary_text: Some("VoiceFlo"),
            ownership_token: Some(proof.token.clone()),
            write_proof: Some(proof),
            write_attempted: true,
            restored: false,
            ownership_changed: false,
            closed: false,
            snapshot,
            test_fail_current_read: true,
            test_fail_restore_prepare: true,
            test_current_observation: None,
            test_restore_observation: None,
            test_restore_commit_attempted: false,
        }
    }

    pub(crate) fn current_ownership(&mut self) -> ClipboardGuardReplyV1 {
        if self.closed {
            return if self.restored {
                ClipboardGuardReplyV1::Restored
            } else if self.ownership_changed {
                ClipboardGuardReplyV1::OwnershipChanged
            } else {
                ClipboardGuardReplyV1::NotWritten
            };
        }
        if self.restored {
            return ClipboardGuardReplyV1::Restored;
        }
        if self.ownership_changed {
            return ClipboardGuardReplyV1::OwnershipChanged;
        }
        if !self.write_attempted {
            return ClipboardGuardReplyV1::NotWritten;
        }
        let previously_proven = self.write_proof.is_some();
        match self.capture_current_proof() {
            Ok(ResearchProofObservation::Owned) => {
                if let Some(proof) = self.write_proof.as_ref() {
                    match self.observe_proof_state(proof) {
                        Ok(ResearchProofObservation::Owned) => ClipboardGuardReplyV1::WriteOwned,
                        Ok(ResearchProofObservation::Changed) => {
                            self.ownership_changed = true;
                            ClipboardGuardReplyV1::OwnershipChanged
                        }
                        Ok(ResearchProofObservation::Unknown) | Err(_) => {
                            ClipboardGuardReplyV1::ReadUnknown
                        }
                    }
                } else {
                    ClipboardGuardReplyV1::ReadUnknown
                }
            }
            Ok(ResearchProofObservation::Changed) => {
                if previously_proven {
                    self.ownership_changed = true;
                    ClipboardGuardReplyV1::OwnershipChanged
                } else {
                    ClipboardGuardReplyV1::ReadUnknown
                }
            }
            Ok(ResearchProofObservation::Unknown) | Err(()) => ClipboardGuardReplyV1::ReadUnknown,
        }
    }

    pub(crate) fn restore_if_owned(&mut self) -> ClipboardGuardReplyV1 {
        if self.restored {
            return ClipboardGuardReplyV1::Restored;
        }
        if self.closed {
            return if self.ownership_changed {
                ClipboardGuardReplyV1::OwnershipChanged
            } else {
                ClipboardGuardReplyV1::NotWritten
            };
        }
        let Some(proof) = self.write_proof.as_ref() else {
            return if self.ownership_changed {
                ClipboardGuardReplyV1::OwnershipChanged
            } else if self.write_attempted {
                ClipboardGuardReplyV1::RestoreUnknown
            } else {
                ClipboardGuardReplyV1::NotWritten
            };
        };
        let restore = match &self.board {
            ResearchClipboardBoardV1::Named(named_board) => {
                #[cfg(test)]
                let result = if let Some(observation) = self.test_restore_observation.clone() {
                    macos_pasteboard::restore_named_if_owned_with_observation(
                        named_board,
                        &self.snapshot,
                        proof,
                        observation,
                    )
                } else {
                    macos_pasteboard::restore_named_if_owned_with_prepare_failure(
                        named_board,
                        &self.snapshot,
                        proof,
                        self.test_fail_restore_prepare,
                    )
                    .map(|restored| {
                        if restored {
                            ResearchRestoreOutcome::Restored
                        } else {
                            ResearchRestoreOutcome::OwnershipChanged
                        }
                    })
                };
                #[cfg(not(test))]
                let result =
                    macos_pasteboard::restore_named_if_owned(named_board, &self.snapshot, proof);
                result
            }
            ResearchClipboardBoardV1::General => {
                #[cfg(test)]
                {
                    if let Some(observation) = self.test_restore_observation.clone() {
                        let mut commit_attempted = false;
                        let result = macos_pasteboard::restore_general_if_owned_with_observation(
                            &self.snapshot,
                            proof,
                            observation,
                            &mut commit_attempted,
                        );
                        self.test_restore_commit_attempted = commit_attempted;
                        result
                    } else {
                        macos_pasteboard::restore_general_if_owned_with_prepare_failure(
                            &self.snapshot,
                            proof,
                            self.test_fail_restore_prepare,
                        )
                    }
                }
                #[cfg(not(test))]
                {
                    macos_pasteboard::restore_general_if_owned(&self.snapshot, proof)
                }
            }
        };
        match restore {
            Ok(ResearchRestoreOutcome::Restored) => {
                self.restored = true;
                ClipboardGuardReplyV1::Restored
            }
            Ok(ResearchRestoreOutcome::OwnershipChanged) => {
                self.ownership_changed = true;
                ClipboardGuardReplyV1::OwnershipChanged
            }
            Ok(ResearchRestoreOutcome::Unknown) => ClipboardGuardReplyV1::RestoreUnknown,
            Err(_) => ClipboardGuardReplyV1::RestoreUnknown,
        }
    }

    /// Settles a resolved owner. Named boards are released only after restore
    /// or a known no-write result; General boards have no registration cleanup
    /// and are only marked closed in memory. Unknown states remain open.
    pub(crate) fn close_after_restore(&mut self) -> ClipboardGuardReplyV1 {
        if self.closed {
            return if self.restored {
                ClipboardGuardReplyV1::Restored
            } else if self.ownership_changed {
                ClipboardGuardReplyV1::OwnershipChanged
            } else {
                ClipboardGuardReplyV1::NotWritten
            };
        }
        if self.ownership_changed {
            if matches!(&self.board, ResearchClipboardBoardV1::General) {
                self.closed = true;
                self.release_general_owner_memory();
            }
            return ClipboardGuardReplyV1::OwnershipChanged;
        }
        if self.write_attempted && !self.restored {
            return ClipboardGuardReplyV1::RestoreUnknown;
        }
        match &self.board {
            ResearchClipboardBoardV1::Named(named_board) => {
                match macos_pasteboard::release_named_board(named_board) {
                    Ok(()) => {
                        self.closed = true;
                        if self.restored {
                            ClipboardGuardReplyV1::Restored
                        } else {
                            ClipboardGuardReplyV1::NotWritten
                        }
                    }
                    Err(_) => ClipboardGuardReplyV1::RestoreUnknown,
                }
            }
            ResearchClipboardBoardV1::General => {
                self.closed = true;
                self.release_general_owner_memory();
                if self.restored {
                    ClipboardGuardReplyV1::Restored
                } else {
                    ClipboardGuardReplyV1::NotWritten
                }
            }
        }
    }

    fn capture_current_proof(&mut self) -> Result<ResearchProofObservation, ()> {
        let (Some(text), Some(token)) = (self.temporary_text, self.ownership_token.as_deref())
        else {
            return Ok(ResearchProofObservation::Unknown);
        };
        let proof = ClipboardWriteProof {
            change_count: self.snapshot.change_count.checked_add(1).ok_or(())?,
            text: text.to_owned(),
            token: token.to_owned(),
        };
        let observation = self.current_write_observation().map_err(|_| ())?;
        let state = classify_research_proof_observation(&proof, &observation);
        if state == ResearchProofObservation::Owned {
            self.write_proof = Some(proof);
        }
        Ok(state)
    }

    fn current_write_observation(&self) -> Result<ClipboardWriteObservation, PasteError> {
        match &self.board {
            ResearchClipboardBoardV1::Named(named_board) => {
                #[cfg(test)]
                {
                    if let Some(observation) = self.test_current_observation.clone() {
                        Ok(observation)
                    } else {
                        macos_pasteboard::observe_named_write_with_failure(
                            named_board,
                            self.test_fail_current_read,
                        )
                    }
                }
                #[cfg(not(test))]
                {
                    macos_pasteboard::observe_named_write(named_board)
                }
            }
            ResearchClipboardBoardV1::General => {
                #[cfg(test)]
                {
                    if let Some(observation) = self.test_current_observation.clone() {
                        Ok(observation)
                    } else {
                        macos_pasteboard::observe_general_write_with_failure(
                            self.test_fail_current_read,
                        )
                    }
                }
                #[cfg(not(test))]
                {
                    macos_pasteboard::observe_write()
                }
            }
        }
    }

    fn observe_proof_state(
        &self,
        proof: &ClipboardWriteProof,
    ) -> Result<ResearchProofObservation, PasteError> {
        let observation = self.current_write_observation()?;
        Ok(classify_research_proof_observation(proof, &observation))
    }
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
#[derive(Clone)]
struct RemoteResearchClipboardLeaseV1 {
    client: Arc<dyn ClipboardGuardRpcClientV1>,
    lease_id: u64,
    board: RemoteClipboardBoardV1,
    session: RemoteResearchClipboardSessionV1,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
#[derive(Clone, PartialEq, Eq)]
enum RemoteResearchClipboardSessionV1 {
    RecorderOnly,
    RegisteredOwnedPasteSession {
        plan_id: String,
        phase_id: String,
        operation_id: String,
    },
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
impl RemoteResearchClipboardSessionV1 {
    fn permits_registered_target_post(&self) -> bool {
        matches!(self, Self::RegisteredOwnedPasteSession { .. })
    }
}

/// Small boundary around platform clipboard effects and delivery diagnostics.
/// Production keeps its AppHandle-backed diagnostics. On macOS, the pasteboard
/// implementation itself does not need an AppHandle; the research owner can
/// reuse this delivery core without initializing Tauri and return typed
/// diagnostics directly to its caller.
pub(crate) struct DeliveryEffects {
    app: Option<AppHandle>,
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    remote_clipboard: Option<RemoteResearchClipboardLeaseV1>,
}

impl DeliveryEffects {
    pub(crate) fn production(app: AppHandle) -> Self {
        Self {
            app: Some(app),
            #[cfg(all(target_os = "macos", feature = "research-harness"))]
            remote_clipboard: None,
        }
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    pub(crate) fn research() -> Self {
        Self {
            app: None,
            remote_clipboard: None,
        }
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    fn research_remote(
        client: Arc<dyn ClipboardGuardRpcClientV1>,
        lease_id: u64,
        board: RemoteClipboardBoardV1,
    ) -> Self {
        Self {
            app: None,
            remote_clipboard: Some(RemoteResearchClipboardLeaseV1 {
                client,
                lease_id,
                board,
                session: RemoteResearchClipboardSessionV1::RecorderOnly,
            }),
        }
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    fn research_registered_owned_remote(
        remote: RegisteredOwnedPasteSessionRemoteContextV1,
    ) -> Result<Self, PasteError> {
        if remote.lease_id == 0
            || remote.plan_id.is_empty()
            || remote.phase_id.is_empty()
            || remote.operation_id.is_empty()
        {
            return Err(PasteError::TargetUnavailable);
        }
        let mut effects = Self::research();
        effects.remote_clipboard = Some(RemoteResearchClipboardLeaseV1 {
            client: remote.client,
            lease_id: remote.lease_id,
            board: RemoteClipboardBoardV1::GeneralClipboard,
            session: RemoteResearchClipboardSessionV1::RegisteredOwnedPasteSession {
                plan_id: remote.plan_id,
                phase_id: remote.phase_id,
                operation_id: remote.operation_id,
            },
        });
        Ok(effects)
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    fn is_registered_owned_remote(&self) -> bool {
        self.remote_clipboard
            .as_ref()
            .is_some_and(|remote| remote.session.permits_registered_target_post())
    }

    #[cfg(all(target_os = "macos", feature = "research-harness", test))]
    fn research_named_remote(client: Arc<dyn ClipboardGuardRpcClientV1>, lease_id: u64) -> Self {
        Self::research_remote(client, lease_id, RemoteClipboardBoardV1::NamedFixture)
    }

    #[cfg(not(target_os = "macos"))]
    fn app(&self) -> Result<&AppHandle, PasteError> {
        self.app.as_ref().ok_or_else(|| {
            PasteError::Clipboard("platform clipboard AppHandle unavailable".to_owned())
        })
    }

    fn record_diagnostic(&self, diagnostic: &DeliveryDiagnostic) {
        if let Some(app) = self.app.as_ref() {
            delivery_diagnostics::record(app, diagnostic);
        }
    }
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
#[derive(Clone)]
pub(crate) struct RegisteredResearchHostIdentityV1 {
    pub(crate) pid: i32,
    pub(crate) parent_pid: i32,
    pub(crate) user_id: u32,
    pub(crate) start_seconds: u64,
    pub(crate) start_microseconds: u32,
    pub(crate) executable_path: PathBuf,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
const REGISTERED_RESEARCH_HOST_BUNDLE_ID: &str = "local.voiceflow.learning-oracle-fixture.v1";
#[cfg(all(target_os = "macos", feature = "research-harness"))]
const REGISTERED_RESEARCH_HOST_WINDOW_TITLE: &str =
    "Voice harness owned fixture 90000000-0000-4000-8000-000000000013";
#[cfg(all(target_os = "macos", feature = "research-harness"))]
const REGISTERED_RESEARCH_HOST_CONTROL_ID: &str =
    "vf-fixture-90000000-0000-4000-8000-000000000013-target";
#[cfg(all(target_os = "macos", feature = "research-harness"))]
const REGISTERED_RESEARCH_HOST_INITIAL_FIELD: &str = "Type: ";
#[cfg(all(target_os = "macos", feature = "research-harness"))]
const REGISTERED_RESEARCH_HOST_INITIAL_SELECTION: (i64, i64) = (6, 0);

#[cfg(all(target_os = "macos", feature = "research-harness"))]
pub(crate) fn registered_research_host_matches(
    identity: &RegisteredResearchHostIdentityV1,
) -> bool {
    use std::ffi::CStr;
    use std::path::Path;

    if identity.pid <= 0
        || identity.parent_pid <= 0
        || identity.parent_pid == identity.pid
        || identity.start_seconds == 0
        || identity.start_microseconds >= 1_000_000
        || !identity.executable_path.is_absolute()
    {
        return false;
    }
    let mut info = unsafe { std::mem::zeroed::<libc::proc_bsdinfo>() };
    let size = std::mem::size_of_val(&info) as libc::c_int;
    let read = unsafe {
        libc::proc_pidinfo(
            identity.pid,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    if read != size
        || info.pbi_pid != identity.pid as u32
        || info.pbi_ppid != identity.parent_pid as u32
        || info.pbi_uid != identity.user_id as libc::uid_t
        || info.pbi_start_tvsec != identity.start_seconds
        || info.pbi_start_tvusec != identity.start_microseconds as u64
    {
        return false;
    }
    let mut path_bytes = [0_i8; 4096];
    let path_len = unsafe {
        libc::proc_pidpath(
            identity.pid,
            path_bytes.as_mut_ptr() as *mut libc::c_void,
            path_bytes.len() as u32,
        )
    };
    if path_len <= 0 {
        return false;
    }
    let Ok(actual_path) = unsafe { CStr::from_ptr(path_bytes.as_ptr()) }.to_str() else {
        return false;
    };
    let Ok(expected_path) = identity.executable_path.canonicalize() else {
        return false;
    };
    let Ok(actual_path) = Path::new(actual_path).canonicalize() else {
        return false;
    };
    actual_path == expected_path
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
#[derive(Clone)]
struct RegisteredResearchPasteGuard {
    host_identity: RegisteredResearchHostIdentityV1,
    expected_bundle_id: String,
    expected_window_title: String,
    expected_control_identifier: String,
    expires_at: std::time::Instant,
    cancellation: CancellationToken,
    initial_target: crate::context::TargetAppGuard,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn registered_research_permission(
    cancellation: &CancellationToken,
    expires_at: std::time::Instant,
) -> Result<(), PasteError> {
    if cancellation.is_cancelled() || std::time::Instant::now() >= expires_at {
        Err(PasteError::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn with_registered_post_permission<T>(
    cancellation: &CancellationToken,
    expires_at: std::time::Instant,
    post: impl FnOnce() -> Result<T, PasteError>,
) -> Result<T, PasteError> {
    registered_research_permission(cancellation, expires_at)?;
    post()
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn capture_registered_research_target(
    host_identity: &RegisteredResearchHostIdentityV1,
    expected_bundle_id: &str,
    expected_window_title: &str,
    expected_control_identifier: &str,
    cancellation: &CancellationToken,
    expires_at: std::time::Instant,
) -> Result<crate::context::TargetAppGuard, PasteError> {
    registered_research_permission(cancellation, expires_at)?;
    if !registered_research_host_matches(host_identity) {
        return Err(PasteError::TargetChanged);
    }
    let (frontmost_pid, frontmost_bundle) = crate::context::frontmost_application_key();
    if frontmost_pid != host_identity.pid || frontmost_bundle.as_deref() != Some(expected_bundle_id)
    {
        return Err(PasteError::TargetChanged);
    }
    // This is intentionally bundle-restricted. The resulting guard is the
    // real native identity; the temporary focus anchor is never used as the
    // delivery baseline or read for field contents.
    let capture = crate::context::capture_frontmost_focus(Some(expected_bundle_id));
    if !capture.stable_frontmost_identity || capture.reason.is_some() {
        return Err(PasteError::TargetUnavailable);
    }
    if capture.target.pid != host_identity.pid
        || capture.target.bundle_id.as_deref() != Some(expected_bundle_id)
    {
        return Err(PasteError::TargetChanged);
    }
    if capture.target.secure_input {
        return Err(PasteError::SecureInput);
    }
    if capture.window_title != expected_window_title {
        return Err(PasteError::TargetChanged);
    }
    if crate::context::same_field_mismatch_reason(&capture.target, &capture.target, false).is_some()
    {
        return Err(PasteError::TargetUnavailable);
    }
    let Some(anchor) = capture.focused_anchor.as_ref() else {
        return Err(PasteError::InputUnavailable);
    };
    if !anchor.matches_registered_research_field(expected_window_title, expected_control_identifier)
    {
        return Err(PasteError::InputChanged);
    }
    if anchor.current_focus_matches() != Some(true) {
        return Err(PasteError::InputChanged);
    }
    registered_research_permission(cancellation, expires_at)?;
    if !registered_research_host_matches(host_identity) {
        return Err(PasteError::TargetChanged);
    }
    let (frontmost_pid, frontmost_bundle) = crate::context::frontmost_application_key();
    if frontmost_pid != host_identity.pid || frontmost_bundle.as_deref() != Some(expected_bundle_id)
    {
        return Err(PasteError::TargetChanged);
    }
    Ok(capture.target)
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
impl RegisteredResearchPasteGuard {
    fn check(&self) -> Result<(), PasteError> {
        let current = capture_registered_research_target(
            &self.host_identity,
            &self.expected_bundle_id,
            &self.expected_window_title,
            &self.expected_control_identifier,
            &self.cancellation,
            self.expires_at,
        )?;
        if crate::context::same_field_mismatch_reason(&self.initial_target, &current, false)
            .is_some()
        {
            return Err(PasteError::TargetChanged);
        }
        registered_research_permission(&self.cancellation, self.expires_at)?;
        if !registered_research_host_matches(&self.host_identity) {
            return Err(PasteError::TargetChanged);
        }
        let (frontmost_pid, frontmost_bundle) = crate::context::frontmost_application_key();
        if frontmost_pid != self.host_identity.pid
            || frontmost_bundle.as_deref() != Some(self.expected_bundle_id.as_str())
        {
            return Err(PasteError::TargetChanged);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetryClipboardOutcome {
    Copied,
    Changed,
    OwnershipUnverified,
    WriteFailed,
}

/// History retry may only copy through the clipboard owner's existing
/// snapshot and write-admission proof. The snapshot stays in memory and is
/// never used to restore the clipboard after a successful retry.
pub(crate) struct RetryClipboardLease {
    snapshot: Option<ClipboardSnapshot>,
}

impl RetryClipboardLease {
    pub(crate) fn capture(app: &AppHandle) -> Self {
        #[cfg(target_os = "macos")]
        let snapshot = ClipboardSnapshot::capture(&DeliveryEffects::production(app.clone())).ok();
        #[cfg(not(target_os = "macos"))]
        let snapshot = {
            let _ = app;
            // Other platform writers do not expose the native item-level
            // admission proof used by History retry, so fail closed there.
            None
        };
        Self { snapshot }
    }

    pub(crate) fn copy_if_unchanged(&self, app: &AppHandle, text: &str) -> RetryClipboardOutcome {
        copy_retry_with_snapshot(self.snapshot.as_ref(), |snapshot| {
            snapshot.copy_for_history_retry(app, text)
        })
    }
}

fn copy_retry_with_snapshot<T>(
    snapshot: Option<&T>,
    copy: impl FnOnce(&T) -> RetryClipboardOutcome,
) -> RetryClipboardOutcome {
    match snapshot {
        Some(snapshot) => copy(snapshot),
        None => RetryClipboardOutcome::OwnershipUnverified,
    }
}

impl ClipboardSnapshot {
    fn copy_for_history_retry(&self, app: &AppHandle, text: &str) -> RetryClipboardOutcome {
        let ownership_token = clipboard_ownership_token();
        #[cfg(target_os = "macos")]
        match &self.backing {
            ClipboardSnapshotBackingV1::Native(native) => {
                let _ = app;
                macos_pasteboard::write_temporary_text_for_retry(
                    text,
                    &ownership_token,
                    native.change_count,
                    &native.items,
                )
            }
            #[cfg(feature = "research-harness")]
            ClipboardSnapshotBackingV1::RemoteResearch { .. } => {
                RetryClipboardOutcome::OwnershipUnverified
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (app, text, ownership_token);
            RetryClipboardOutcome::OwnershipUnverified
        }
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ClipboardWriteProof {
    change_count: isize,
    text: String,
    token: String,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ClipboardWriteObservation {
    change_count: isize,
    text: Option<String>,
    token: Option<String>,
    has_exact_temporary_types: bool,
}

#[derive(Clone)]
enum ClipboardCopyMarker {
    #[cfg(target_os = "macos")]
    Native(ClipboardWriteProof),
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    RemoteResearch {
        board: RemoteClipboardBoardV1,
        session: RemoteResearchClipboardSessionV1,
        lease_id: u64,
        write_id: u64,
    },
    #[cfg(not(target_os = "macos"))]
    Text(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClipboardPostPermitResultV1 {
    Allowed,
    // The parent recorded a research permit; this is never authority to post
    // a real keyboard event.
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    RecordedOnly,
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    Denied,
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    AcknowledgementUnavailable,
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    ProtocolRejected,
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    Unknown,
}

impl DeliveryEffects {
    fn request_shortcut_post(&self, marker: &ClipboardCopyMarker) -> ClipboardPostPermitResultV1 {
        #[cfg(all(target_os = "macos", feature = "research-harness"))]
        if let Some(remote) = self.remote_clipboard.as_ref() {
            let ClipboardCopyMarker::RemoteResearch {
                board,
                session,
                lease_id,
                write_id,
            } = marker
            else {
                return ClipboardPostPermitResultV1::Unknown;
            };
            if *lease_id != remote.lease_id || *board != remote.board || *session != remote.session
            {
                return ClipboardPostPermitResultV1::Unknown;
            }
            return remote_post_permit_result(
                &remote.session,
                remote.client.request(
                    remote.lease_id,
                    ClipboardGuardOperationV1::PermitShortcutPost {
                        write_id: *write_id,
                    },
                ),
            );
        }
        let _ = marker;
        ClipboardPostPermitResultV1::Allowed
    }
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn remote_post_permit_result(
    session: &RemoteResearchClipboardSessionV1,
    response: Result<ClipboardGuardRpcOutcomeV1, ClipboardGuardClientErrorV1>,
) -> ClipboardPostPermitResultV1 {
    match response {
        Ok(ClipboardGuardRpcOutcomeV1::PostPermitRecordedOnly {
            parent_permit_count: 1,
        }) if !session.permits_registered_target_post() => {
            ClipboardPostPermitResultV1::RecordedOnly
        }
        Ok(ClipboardGuardRpcOutcomeV1::PostPermitAllowed {
            parent_permit_count: 1,
        }) if session.permits_registered_target_post() => ClipboardPostPermitResultV1::Allowed,
        Ok(ClipboardGuardRpcOutcomeV1::PermitDenied) => ClipboardPostPermitResultV1::Denied,
        Err(ClipboardGuardClientErrorV1::AcknowledgementUnavailable) => {
            ClipboardPostPermitResultV1::AcknowledgementUnavailable
        }
        Ok(ClipboardGuardRpcOutcomeV1::Rejected) => ClipboardPostPermitResultV1::ProtocolRejected,
        _ => ClipboardPostPermitResultV1::Unknown,
    }
}

fn clipboard_ownership_token() -> String {
    static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    format!("{:x}:{timestamp:x}:{sequence:x}", std::process::id())
}

#[cfg(any(target_os = "macos", test))]
fn prove_clipboard_write_ownership(
    previous_change_count: isize,
    expected_text: &str,
    expected_token: &str,
    observation: &ClipboardWriteObservation,
) -> Option<ClipboardWriteProof> {
    let expected_change_count = previous_change_count.checked_add(1)?;
    (observation.change_count == expected_change_count
        && observation.text.as_deref() == Some(expected_text)
        && observation.token.as_deref() == Some(expected_token)
        && observation.has_exact_temporary_types)
        .then(|| ClipboardWriteProof {
            change_count: observation.change_count,
            text: expected_text.to_owned(),
            token: expected_token.to_owned(),
        })
}

#[cfg(any(target_os = "macos", test))]
fn clipboard_write_proof_is_current(
    proof: &ClipboardWriteProof,
    observation: &ClipboardWriteObservation,
) -> bool {
    observation.change_count == proof.change_count
        && observation.text.as_deref() == Some(proof.text.as_str())
        && observation.token.as_deref() == Some(proof.token.as_str())
        && observation.has_exact_temporary_types
}

#[cfg(any(target_os = "macos", test))]
const TEMPORARY_CLIPBOARD_TYPES: [&str; 4] = [
    "public.utf8-plain-text",
    "com.voiceflow.temporary-clipboard-owner",
    "org.nspasteboard.TransientType",
    "org.nspasteboard.AutoGeneratedType",
];
#[cfg(any(target_os = "macos", test))]
const TEMPORARY_CLIPBOARD_HINT_DATA: &[u8] = &[];

#[cfg(any(target_os = "macos", test))]
fn has_exact_temporary_type_names(types: &[String]) -> bool {
    types.len() == TEMPORARY_CLIPBOARD_TYPES.len()
        && TEMPORARY_CLIPBOARD_TYPES.iter().all(|expected| {
            types
                .iter()
                .filter(|actual| actual.as_str() == *expected)
                .count()
                == 1
        })
}

#[cfg(any(target_os = "macos", test))]
trait ClipboardRestoreProtocol<Snapshot: ?Sized> {
    type Prepared;

    fn prepare_restore(&mut self, snapshot: &Snapshot) -> Result<Self::Prepared, PasteError>;
    fn observe(&mut self) -> Result<ClipboardWriteObservation, PasteError>;
    fn commit_restore(&mut self, prepared: Self::Prepared) -> Result<Option<isize>, PasteError>;
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
struct RecordingRestoreObservation<Backend> {
    inner: Backend,
    observation: Option<ClipboardWriteObservation>,
    #[cfg(test)]
    observation_override: Option<ClipboardWriteObservation>,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
impl<Snapshot: ?Sized, Backend> ClipboardRestoreProtocol<Snapshot>
    for RecordingRestoreObservation<Backend>
where
    Backend: ClipboardRestoreProtocol<Snapshot>,
{
    type Prepared = Backend::Prepared;

    fn prepare_restore(&mut self, snapshot: &Snapshot) -> Result<Self::Prepared, PasteError> {
        self.inner.prepare_restore(snapshot)
    }

    fn observe(&mut self) -> Result<ClipboardWriteObservation, PasteError> {
        #[cfg(test)]
        let observation = match self.observation_override.as_ref() {
            Some(observation) => observation.clone(),
            None => self.inner.observe()?,
        };
        #[cfg(not(test))]
        let observation = self.inner.observe()?;
        self.observation = Some(observation.clone());
        Ok(observation)
    }

    fn commit_restore(&mut self, prepared: Self::Prepared) -> Result<Option<isize>, PasteError> {
        self.inner.commit_restore(prepared)
    }
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResearchProofObservation {
    Owned,
    Changed,
    Unknown,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn classify_research_proof_observation(
    proof: &ClipboardWriteProof,
    observation: &ClipboardWriteObservation,
) -> ResearchProofObservation {
    if observation.change_count != proof.change_count || !observation.has_exact_temporary_types {
        return ResearchProofObservation::Changed;
    }
    if observation.text.is_none() || observation.token.is_none() {
        return ResearchProofObservation::Unknown;
    }
    if clipboard_write_proof_is_current(proof, observation) {
        ResearchProofObservation::Owned
    } else {
        ResearchProofObservation::Changed
    }
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResearchRestoreOutcome {
    Restored,
    OwnershipChanged,
    Unknown,
}

#[cfg(any(target_os = "macos", test))]
struct ClipboardRestoreCommit {
    restored: bool,
    generation: Option<isize>,
}

/// Shared protocol seam used by the AppKit backend and deterministic fake
/// backend tests. Preparing the old items happens before the last ownership
/// observation; restore is skipped unless that observation still proves our
/// exact write is current.
#[cfg(any(target_os = "macos", test))]
fn restore_clipboard_if_owned<Snapshot: ?Sized, Backend>(
    backend: &mut Backend,
    snapshot: &Snapshot,
    proof: Option<&ClipboardWriteProof>,
) -> Result<ClipboardRestoreCommit, PasteError>
where
    Backend: ClipboardRestoreProtocol<Snapshot>,
{
    let Some(proof) = proof else {
        return Ok(ClipboardRestoreCommit {
            restored: false,
            generation: None,
        });
    };
    let prepared = backend.prepare_restore(snapshot)?;
    let current = backend.observe()?;
    if !clipboard_write_proof_is_current(proof, &current) {
        return Ok(ClipboardRestoreCommit {
            restored: false,
            generation: None,
        });
    }
    let generation = backend.commit_restore(prepared)?;
    Ok(ClipboardRestoreCommit {
        restored: true,
        generation,
    })
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn research_restore_outcome(
    commit: &ClipboardRestoreCommit,
    observation: Option<&ClipboardWriteObservation>,
    proof: &ClipboardWriteProof,
) -> ResearchRestoreOutcome {
    if commit.restored {
        return ResearchRestoreOutcome::Restored;
    }
    match observation.map(|observed| classify_research_proof_observation(proof, observed)) {
        Some(ResearchProofObservation::Changed) => ResearchRestoreOutcome::OwnershipChanged,
        Some(ResearchProofObservation::Owned | ResearchProofObservation::Unknown) | None => {
            ResearchRestoreOutcome::Unknown
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn restore_commit_generation(
    before: Option<isize>,
    cleared: isize,
    after: Option<isize>,
) -> Option<isize> {
    (before.and_then(|value| value.checked_add(1)) == Some(cleared) && after == Some(cleared))
        .then_some(cleared)
}

/// D1 permits restoring the original clipboard only when cancellation is
/// known to precede the keyboard shortcut and the current marker was observed
/// as ours. The restore callback still performs the final ownership check.
fn restore_cancelled_clipboard_if_safe(
    trace: &mut DeliveryTrace,
    shortcut_may_have_been_posted: bool,
    restore: impl FnOnce() -> Result<ClipboardRestoreAttemptV1, PasteError>,
) {
    trace.keyboard_paste_may_have_been_posted |= shortcut_may_have_been_posted;
    if trace.keyboard_paste_may_have_been_posted || trace.clipboard_write_owned != Some(true) {
        return;
    }
    trace.clipboard_restore_attempted = true;
    match restore() {
        Ok(attempt) => {
            trace.clipboard_restored = attempt.trace_value();
            if attempt == ClipboardRestoreAttemptV1::OwnershipChanged {
                trace.clipboard_write_owned = Some(false);
            }
        }
        Err(_) => trace.clipboard_restored = None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClipboardRestoreAttemptV1 {
    NotAttempted,
    Restored,
    OwnershipChanged,
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    Deferred,
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClipboardMarkerCheckStageV1 {
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    MarkerAfterWrite,
    PreBoundary,
    PostCallback,
    PreRestore,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
impl ClipboardMarkerCheckStageV1 {
    fn rpc_stage(self) -> ClipboardGuardCheckStageV1 {
        match self {
            Self::MarkerAfterWrite => ClipboardGuardCheckStageV1::MarkerAfterWrite,
            Self::PreBoundary => ClipboardGuardCheckStageV1::PreBoundary,
            Self::PostCallback => ClipboardGuardCheckStageV1::PostCallback,
            Self::PreRestore => ClipboardGuardCheckStageV1::PreRestore,
        }
    }
}

impl ClipboardRestoreAttemptV1 {
    fn trace_value(self) -> Option<bool> {
        match self {
            Self::Restored => Some(true),
            Self::OwnershipChanged => Some(false),
            Self::NotAttempted => None,
            #[cfg(all(target_os = "macos", feature = "research-harness"))]
            Self::Deferred | Self::Unknown => None,
        }
    }
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn remote_research_request(
    client: &dyn ClipboardGuardRpcClientV1,
    lease_id: u64,
    operation: ClipboardGuardOperationV1,
    last_error: &Mutex<Option<ClipboardGuardClientErrorV1>>,
    last_outcome: &Mutex<Option<ClipboardGuardRpcOutcomeV1>>,
) -> Result<ClipboardGuardRpcOutcomeV1, PasteError> {
    match client.request(lease_id, operation) {
        Ok(outcome) => {
            if let Ok(mut stored) = last_error.lock() {
                *stored = None;
            }
            if let Ok(mut stored) = last_outcome.lock() {
                *stored = Some(outcome);
            }
            Ok(outcome)
        }
        Err(error) => {
            if let Ok(mut stored) = last_error.lock() {
                *stored = Some(error);
            }
            if let Ok(mut stored) = last_outcome.lock() {
                *stored = None;
            }
            Err(PasteError::Clipboard(
                "named clipboard owner request failed".into(),
            ))
        }
    }
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn registered_remote_temporary_text(board: RemoteClipboardBoardV1) -> &'static str {
    match board {
        RemoteClipboardBoardV1::NamedFixture => macos_pasteboard::registered_temporary_text(),
        RemoteClipboardBoardV1::GeneralClipboard => {
            macos_pasteboard::registered_general_temporary_text()
        }
    }
}

impl ClipboardSnapshot {
    fn capture(effects: &DeliveryEffects) -> Result<Self, PasteError> {
        #[cfg(all(target_os = "macos", not(feature = "research-harness")))]
        let _ = effects;
        #[cfg(target_os = "macos")]
        {
            #[cfg(feature = "research-harness")]
            if let Some(remote) = effects.remote_clipboard.as_ref() {
                return Ok(Self {
                    backing: ClipboardSnapshotBackingV1::RemoteResearch {
                        board: remote.board,
                        session: remote.session.clone(),
                        client: Arc::clone(&remote.client),
                        lease_id: remote.lease_id,
                        capture_fresh_unchanged: Mutex::new(None),
                        write_id: Mutex::new(None),
                        last_rpc_error: Mutex::new(None),
                        last_rpc_outcome: Box::new(Mutex::new(None)),
                    },
                });
            }
            Ok(Self {
                backing: ClipboardSnapshotBackingV1::Native(macos_pasteboard::Snapshot::capture()?),
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(Self {
                text: effects.app()?.clipboard().read_text().ok(),
            })
        }
    }

    fn capture_fresh_unchanged(&self) -> Result<Option<bool>, PasteError> {
        #[cfg(all(target_os = "macos", feature = "research-harness"))]
        if let ClipboardSnapshotBackingV1::RemoteResearch {
            client,
            lease_id,
            capture_fresh_unchanged,
            last_rpc_error,
            last_rpc_outcome,
            ..
        } = &self.backing
        {
            let mut cached = capture_fresh_unchanged.lock().map_err(|_| {
                PasteError::Clipboard("named clipboard snapshot state unavailable".into())
            })?;
            if let Some(unchanged) = *cached {
                return Ok(Some(unchanged));
            }
            let outcome = remote_research_request(
                client.as_ref(),
                *lease_id,
                ClipboardGuardOperationV1::CaptureFreshUnchanged,
                last_rpc_error,
                last_rpc_outcome,
            )?;
            let unchanged = match outcome {
                ClipboardGuardRpcOutcomeV1::SnapshotUnchanged => true,
                ClipboardGuardRpcOutcomeV1::OwnershipChanged => false,
                _ => {
                    return Err(PasteError::Clipboard(
                        "named clipboard snapshot freshness is unknown".into(),
                    ));
                }
            };
            *cached = Some(unchanged);
            return Ok(Some(unchanged));
        }
        Ok(None)
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    fn last_remote_client_error(&self) -> Option<ClipboardGuardClientErrorV1> {
        match &self.backing {
            ClipboardSnapshotBackingV1::RemoteResearch { last_rpc_error, .. } => {
                last_rpc_error.lock().ok().and_then(|stored| *stored)
            }
            ClipboardSnapshotBackingV1::Native(_) => None,
        }
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    fn last_remote_outcome(&self) -> Option<ClipboardGuardRpcOutcomeV1> {
        match &self.backing {
            ClipboardSnapshotBackingV1::RemoteResearch {
                last_rpc_outcome, ..
            } => last_rpc_outcome.lock().ok().and_then(|stored| *stored),
            ClipboardSnapshotBackingV1::Native(_) => None,
        }
    }

    fn unchanged_since_capture(&self, effects: &DeliveryEffects) -> Result<bool, PasteError> {
        #[cfg(target_os = "macos")]
        match &self.backing {
            ClipboardSnapshotBackingV1::Native(native) => {
                let _ = effects;
                Ok(macos_pasteboard::change_count()? == native.change_count)
            }
            #[cfg(feature = "research-harness")]
            ClipboardSnapshotBackingV1::RemoteResearch {
                client,
                lease_id,
                last_rpc_error,
                last_rpc_outcome,
                ..
            } => match remote_research_request(
                client.as_ref(),
                *lease_id,
                ClipboardGuardOperationV1::CheckUnchangedSinceCapture,
                last_rpc_error,
                last_rpc_outcome,
            )? {
                ClipboardGuardRpcOutcomeV1::SnapshotUnchanged => Ok(true),
                ClipboardGuardRpcOutcomeV1::OwnershipChanged => Ok(false),
                _ => Err(PasteError::Clipboard(
                    "named clipboard snapshot freshness is unknown".into(),
                )),
            },
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(effects.app()?.clipboard().read_text().ok() == self.text)
        }
    }

    fn write_temporary_text(
        &self,
        effects: &DeliveryEffects,
        text: &str,
        ownership_token: &str,
    ) -> Result<(), PasteError> {
        #[cfg(target_os = "macos")]
        match &self.backing {
            ClipboardSnapshotBackingV1::Native(native) => {
                let _ = effects;
                macos_pasteboard::write_temporary_text(
                    text,
                    ownership_token,
                    native.change_count,
                    &native.items,
                )
            }
            #[cfg(feature = "research-harness")]
            ClipboardSnapshotBackingV1::RemoteResearch {
                board,
                client,
                lease_id,
                write_id,
                last_rpc_error,
                last_rpc_outcome,
                ..
            } => {
                if text != registered_remote_temporary_text(*board)
                    || write_id
                        .lock()
                        .map_err(|_| {
                            PasteError::Clipboard("named clipboard write state unavailable".into())
                        })?
                        .is_some()
                {
                    return Err(PasteError::Clipboard(
                        "named clipboard write is not registered".into(),
                    ));
                }
                let _ = (effects, ownership_token);
                match remote_research_request(
                    client.as_ref(),
                    *lease_id,
                    ClipboardGuardOperationV1::WriteRegisteredTemporaryText,
                    last_rpc_error,
                    last_rpc_outcome,
                )? {
                    ClipboardGuardRpcOutcomeV1::WriteOwned { write_id: received } => {
                        *write_id.lock().map_err(|_| {
                            PasteError::Clipboard("named clipboard write state unavailable".into())
                        })? = Some(received);
                        Ok(())
                    }
                    ClipboardGuardRpcOutcomeV1::OwnershipChanged => Err(PasteError::Clipboard(
                        "named clipboard changed before registered write".into(),
                    )),
                    _ => Err(PasteError::Clipboard(
                        "named clipboard write ownership is unknown".into(),
                    )),
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (self, ownership_token);
            effects
                .app()?
                .clipboard()
                .write_text(text)
                .map_err(|error| PasteError::Clipboard(error.to_string()))
        }
    }

    fn marker_after_write(
        &self,
        effects: &DeliveryEffects,
        text: &str,
        ownership_token: &str,
    ) -> Result<Option<ClipboardCopyMarker>, PasteError> {
        #[cfg(target_os = "macos")]
        match &self.backing {
            ClipboardSnapshotBackingV1::Native(native) => {
                let _ = effects;
                let observation = macos_pasteboard::observe_write()?;
                Ok(prove_clipboard_write_ownership(
                    native.change_count,
                    text,
                    ownership_token,
                    &observation,
                )
                .map(ClipboardCopyMarker::Native))
            }
            #[cfg(feature = "research-harness")]
            ClipboardSnapshotBackingV1::RemoteResearch {
                board,
                session,
                client,
                lease_id,
                write_id,
                last_rpc_error,
                last_rpc_outcome,
                ..
            } => {
                if text != registered_remote_temporary_text(*board) {
                    return Err(PasteError::Clipboard(
                        "named clipboard marker is not registered".into(),
                    ));
                }
                let Some(write_id) = *write_id.lock().map_err(|_| {
                    PasteError::Clipboard("named clipboard write state unavailable".into())
                })?
                else {
                    return Ok(None);
                };
                let _ = (effects, ownership_token);
                match remote_research_request(
                    client.as_ref(),
                    *lease_id,
                    ClipboardGuardOperationV1::CheckCurrent {
                        write_id,
                        stage: ClipboardMarkerCheckStageV1::MarkerAfterWrite.rpc_stage(),
                        child_callback_count: 0,
                    },
                    last_rpc_error,
                    last_rpc_outcome,
                )? {
                    ClipboardGuardRpcOutcomeV1::CurrentOwned => {
                        Ok(Some(ClipboardCopyMarker::RemoteResearch {
                            board: *board,
                            session: session.clone(),
                            lease_id: *lease_id,
                            write_id,
                        }))
                    }
                    ClipboardGuardRpcOutcomeV1::OwnershipChanged => Ok(None),
                    _ => Err(PasteError::Clipboard(
                        "named clipboard marker could not be verified".into(),
                    )),
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let current = effects.app()?.clipboard().read_text().ok();
            let _ = ownership_token;
            let changed = current.as_deref() == Some(text)
                && current.as_deref().is_some_and(|current| {
                    self.text
                        .as_deref()
                        .is_none_or(|previous| previous != current)
                });
            Ok(changed.then(|| ClipboardCopyMarker::Text(text.to_owned())))
        }
    }

    fn marker_is_current(
        &self,
        effects: &DeliveryEffects,
        marker: &ClipboardCopyMarker,
        stage: ClipboardMarkerCheckStageV1,
        child_callback_count: u8,
    ) -> Result<bool, PasteError> {
        #[cfg(target_os = "macos")]
        match (&self.backing, marker) {
            (ClipboardSnapshotBackingV1::Native(_), ClipboardCopyMarker::Native(proof)) => {
                let _ = (effects, stage, child_callback_count);
                let observation = macos_pasteboard::observe_write()?;
                Ok(clipboard_write_proof_is_current(proof, &observation))
            }
            #[cfg(feature = "research-harness")]
            (
                ClipboardSnapshotBackingV1::RemoteResearch {
                    board: stored_board,
                    session: stored_session,
                    client,
                    lease_id,
                    write_id: stored_write_id,
                    last_rpc_error,
                    last_rpc_outcome,
                    ..
                },
                ClipboardCopyMarker::RemoteResearch {
                    board: marker_board,
                    session: marker_session,
                    lease_id: marker_lease_id,
                    write_id,
                },
            ) if lease_id == marker_lease_id
                && stored_board == marker_board
                && stored_session == marker_session
                && stored_write_id
                    .lock()
                    .map_err(|_| {
                        PasteError::Clipboard("named clipboard write state unavailable".into())
                    })?
                    .as_ref()
                    == Some(write_id) =>
            {
                let _ = effects;
                match remote_research_request(
                    client.as_ref(),
                    *lease_id,
                    ClipboardGuardOperationV1::CheckCurrent {
                        write_id: *write_id,
                        stage: stage.rpc_stage(),
                        child_callback_count,
                    },
                    last_rpc_error,
                    last_rpc_outcome,
                )? {
                    ClipboardGuardRpcOutcomeV1::CurrentOwned => Ok(true),
                    ClipboardGuardRpcOutcomeV1::OwnershipChanged => Ok(false),
                    _ => Err(PasteError::Clipboard(
                        "named clipboard ownership is unknown".into(),
                    )),
                }
            }
            #[cfg(feature = "research-harness")]
            (ClipboardSnapshotBackingV1::RemoteResearch { .. }, _) => Err(PasteError::Clipboard(
                "named clipboard marker does not match its lease".into(),
            )),
            #[cfg(feature = "research-harness")]
            _ => Err(PasteError::Clipboard(
                "clipboard marker does not match its snapshot".into(),
            )),
        }
        #[cfg(not(target_os = "macos"))]
        {
            let current = effects.app()?.clipboard().read_text().ok();
            let _ = (stage, child_callback_count);
            let ClipboardCopyMarker::Text(expected) = marker else {
                return Ok(false);
            };
            Ok(current.as_deref() == Some(expected.as_str()))
        }
    }

    fn restore_if_unchanged(
        &self,
        effects: &DeliveryEffects,
        marker: Option<&ClipboardCopyMarker>,
    ) -> Result<ClipboardRestoreAttemptV1, PasteError> {
        let Some(marker) = marker else {
            return Ok(ClipboardRestoreAttemptV1::NotAttempted);
        };
        #[cfg(target_os = "macos")]
        match (&self.backing, marker) {
            (ClipboardSnapshotBackingV1::Native(native), ClipboardCopyMarker::Native(proof)) => {
                let _ = effects;
                // Prepare the original items before the last ownership check so
                // the check precedes the existing non-atomic clear/write sequence.
                native.restore_if_owned(proof).map(|restored| {
                    if restored {
                        ClipboardRestoreAttemptV1::Restored
                    } else {
                        ClipboardRestoreAttemptV1::OwnershipChanged
                    }
                })
            }
            #[cfg(feature = "research-harness")]
            (
                ClipboardSnapshotBackingV1::RemoteResearch {
                    board: stored_board,
                    session: stored_session,
                    client,
                    lease_id,
                    write_id: stored_write_id,
                    last_rpc_error,
                    last_rpc_outcome,
                    ..
                },
                ClipboardCopyMarker::RemoteResearch {
                    board: marker_board,
                    session: marker_session,
                    lease_id: marker_lease_id,
                    write_id,
                },
            ) if lease_id == marker_lease_id
                && stored_board == marker_board
                && stored_session == marker_session =>
            {
                let stored_write_id = stored_write_id.lock().map_err(|_| {
                    PasteError::Clipboard("named clipboard write state unavailable".into())
                })?;
                if stored_write_id.as_ref() != Some(write_id) {
                    return Err(PasteError::Clipboard(
                        "named clipboard restore marker does not match its write".into(),
                    ));
                }
                let _ = effects;
                match remote_research_request(
                    client.as_ref(),
                    *lease_id,
                    ClipboardGuardOperationV1::RequestRestoreIfOwned {
                        write_id: *write_id,
                    },
                    last_rpc_error,
                    last_rpc_outcome,
                )? {
                    ClipboardGuardRpcOutcomeV1::RestoreDeferred => {
                        Ok(ClipboardRestoreAttemptV1::Deferred)
                    }
                    ClipboardGuardRpcOutcomeV1::RestoreUnknown => {
                        Ok(ClipboardRestoreAttemptV1::Unknown)
                    }
                    _ => Err(PasteError::Clipboard(
                        "named clipboard restore was not deferred".into(),
                    )),
                }
            }
            #[cfg(feature = "research-harness")]
            (ClipboardSnapshotBackingV1::RemoteResearch { .. }, _) => Err(PasteError::Clipboard(
                "named clipboard restore marker does not match its lease".into(),
            )),
            #[cfg(feature = "research-harness")]
            _ => Err(PasteError::Clipboard(
                "clipboard restore marker does not match its snapshot".into(),
            )),
        }
        #[cfg(not(target_os = "macos"))]
        {
            if !self.marker_is_current(
                effects,
                marker,
                ClipboardMarkerCheckStageV1::PreRestore,
                0,
            )? {
                return Ok(ClipboardRestoreAttemptV1::OwnershipChanged);
            }
            let ClipboardCopyMarker::Text(_copied_text) = marker else {
                return Ok(ClipboardRestoreAttemptV1::OwnershipChanged);
            };
            match &self.text {
                Some(previous) => effects
                    .app()?
                    .clipboard()
                    .write_text(previous)
                    .map(|()| ClipboardRestoreAttemptV1::Restored)
                    .map_err(|error| PasteError::Clipboard(error.to_string())),
                None => effects
                    .app()?
                    .clipboard()
                    .clear()
                    .map(|()| ClipboardRestoreAttemptV1::Restored)
                    .map_err(|error| PasteError::Clipboard(error.to_string())),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Utf16RangeV1 {
    pub(crate) start: u32,
    pub(crate) length: u32,
}

/// Delivery facts minted only by the original paste owner after an exact
/// primary AXValue readback. Ranges are UTF-16 code units: the replaced range
/// addresses the source field; the inserted range addresses the verified
/// post-insert field. This type intentionally has no Serialize implementation.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ConfirmedInsertionV1 {
    anchor_owner_id: u64,
    created_at: std::time::Instant,
    pub(crate) source_field_utf16_length: u32,
    pub(crate) inserted_field_utf16_length: u32,
    pub(crate) replaced_range_before: Utf16RangeV1,
    pub(crate) inserted_range_after: Utf16RangeV1,
    pub(crate) inserted_text_utf16_length: u32,
    pub(crate) target_identity: crate::context::TargetAppGuard,
    pub(crate) readback: PasteVerificationFact,
}

impl ConfirmedInsertionV1 {
    pub(crate) fn is_bound_to(&self, ticket: &LearningObservationTicket) -> bool {
        self.anchor_owner_id == ticket.inner.id
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FocusedFieldReadSampleV1 {
    pub(crate) value: Option<String>,
    pub(crate) selection_range_utf16: Option<(i64, i64)>,
    pub(crate) focus_matches: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FocusedFieldReadError {
    Cancelled,
    Expired,
    StaleEpoch,
    FocusChanged,
    FocusUnknown,
    ValueUnavailable,
    Busy,
    OwnerClosed,
    TimedOut,
    OwnerFailed,
}

// AX objects remain on the thread that captured them. Tickets carry channels
// across workers; only the original owner thread dereferences the native ref.
type AnchorOperation<A> = Box<
    dyn FnOnce(&A, &CancellationToken, std::time::Instant) -> Result<&'static str, String> + Send,
>;
type FocusedFieldReadOperation<A> = Box<
    dyn FnOnce(
            &A,
            &CancellationToken,
            std::time::Instant,
            &AtomicU64,
            u64,
        ) -> Result<FocusedFieldReadSampleV1, FocusedFieldReadError>
        + Send,
>;

struct UndoAnchorRequest<A> {
    operation: AnchorOperation<A>,
    cancellation: CancellationToken,
    deadline: std::time::Instant,
    reply: std::sync::mpsc::Sender<Result<&'static str, String>>,
}

struct FocusedFieldReadRequest<A> {
    operation: FocusedFieldReadOperation<A>,
    cancellation: CancellationToken,
    deadline: std::time::Instant,
    observer_epoch: std::sync::Arc<AtomicU64>,
    expected_epoch: u64,
    reply: std::sync::mpsc::Sender<Result<FocusedFieldReadSampleV1, FocusedFieldReadError>>,
}

enum AnchorRequest<A> {
    Undo(UndoAnchorRequest<A>),
    Read(FocusedFieldReadRequest<A>),
    ReleaseUndo {
        ticket_id: u64,
    },
    ReleaseObservation {
        ticket_id: u64,
    },
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    ReleaseUndoAndWait {
        ticket_id: u64,
        reply: std::sync::mpsc::Sender<Result<(), AnchorOwnerCloseError>>,
    },
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    ReleaseObservationAndWait {
        ticket_id: u64,
        reply: std::sync::mpsc::Sender<Result<(), AnchorOwnerCloseError>>,
    },
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnchorOwnerCloseError {
    Busy,
    Expired,
    TimedOut,
    UnknownPending,
    NotStarted,
    OwnerClosed,
    OwnerFailed,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AnchorOwnerTerminal {
    Closed,
    Failed,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
pub(crate) struct AnchorOwnerCompletion {
    receiver: Option<std::sync::mpsc::Receiver<AnchorOwnerTerminal>>,
    join_handle: Option<thread::JoinHandle<()>>,
    terminal: Option<AnchorOwnerTerminal>,
    started: bool,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
impl AnchorOwnerCompletion {
    fn started(
        receiver: std::sync::mpsc::Receiver<AnchorOwnerTerminal>,
        join_handle: thread::JoinHandle<()>,
    ) -> Self {
        Self {
            receiver: Some(receiver),
            join_handle: Some(join_handle),
            terminal: None,
            started: true,
        }
    }

    fn not_started() -> Self {
        Self {
            receiver: None,
            join_handle: None,
            terminal: None,
            started: false,
        }
    }

    pub(crate) fn wait_closed_until(
        &mut self,
        deadline: std::time::Instant,
    ) -> Result<(), AnchorOwnerCloseError> {
        if !self.started {
            return Err(AnchorOwnerCloseError::NotStarted);
        }
        match self.terminal {
            Some(AnchorOwnerTerminal::Closed) => return Ok(()),
            Some(AnchorOwnerTerminal::Failed) => {
                return Err(AnchorOwnerCloseError::OwnerFailed);
            }
            None => {}
        }
        let timeout = deadline.saturating_duration_since(std::time::Instant::now());
        let received = match self.receiver.as_ref() {
            Some(receiver) => receiver.recv_timeout(timeout),
            None => return Err(AnchorOwnerCloseError::UnknownPending),
        };
        match received {
            Ok(AnchorOwnerTerminal::Closed) => {
                self.terminal = Some(AnchorOwnerTerminal::Closed);
                self.receiver.take();
                Ok(())
            }
            Ok(AnchorOwnerTerminal::Failed) => {
                self.terminal = Some(AnchorOwnerTerminal::Failed);
                self.receiver.take();
                Err(AnchorOwnerCloseError::OwnerFailed)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                Err(AnchorOwnerCloseError::UnknownPending)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err(AnchorOwnerCloseError::UnknownPending)
            }
        }
    }

    /// Waits for the original owner thread to finish. Unlike the phase-bounded
    /// observation above, this cleanup wait has no deadline: the caller must
    /// keep the owned Host alive until the native owner has actually settled.
    pub(crate) fn settle_after_phase(
        &mut self,
    ) -> Result<AnchorOwnerTerminal, AnchorOwnerCloseError> {
        if !self.started {
            return Err(AnchorOwnerCloseError::NotStarted);
        }
        if self.join_handle.is_none() {
            return self.terminal.ok_or(AnchorOwnerCloseError::UnknownPending);
        }

        let Some(join_handle) = self.join_handle.take() else {
            return Err(AnchorOwnerCloseError::UnknownPending);
        };
        let join_result = join_handle.join();
        let explicit_terminal = self.terminal.or_else(|| {
            self.receiver
                .as_ref()
                .and_then(|receiver| receiver.try_recv().ok())
        });
        let terminal = if join_result.is_ok() {
            explicit_terminal.unwrap_or(AnchorOwnerTerminal::Failed)
        } else {
            AnchorOwnerTerminal::Failed
        };
        self.terminal = Some(terminal);
        self.receiver.take();
        Ok(terminal)
    }
}

struct AnchorTicketInner<A> {
    id: u64,
    sender: std::sync::mpsc::SyncSender<AnchorRequest<A>>,
    expires_at: std::time::Instant,
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    explicitly_released: std::sync::atomic::AtomicBool,
}

impl<A> Drop for AnchorTicketInner<A> {
    fn drop(&mut self) {
        #[cfg(all(target_os = "macos", feature = "research-harness"))]
        if self
            .explicitly_released
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        let _ = self
            .sender
            .try_send(AnchorRequest::ReleaseUndo { ticket_id: self.id });
    }
}

pub(crate) struct AnchorTicket<A> {
    inner: std::sync::Arc<AnchorTicketInner<A>>,
}

struct LearningObservationTicketInner {
    id: u64,
    sender: std::sync::mpsc::SyncSender<AnchorRequest<FocusedFieldAnchor>>,
    expires_at: std::time::Instant,
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    research_guard: Option<RegisteredResearchPasteGuard>,
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    explicitly_released: std::sync::atomic::AtomicBool,
}

impl Drop for LearningObservationTicketInner {
    fn drop(&mut self) {
        #[cfg(all(target_os = "macos", feature = "research-harness"))]
        if self
            .explicitly_released
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        let _ = self
            .sender
            .try_send(AnchorRequest::ReleaseObservation { ticket_id: self.id });
    }
}

#[derive(Clone)]
pub(crate) struct LearningObservationTicket {
    inner: std::sync::Arc<LearningObservationTicketInner>,
}

impl std::fmt::Debug for LearningObservationTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LearningObservationTicket")
    }
}

impl PartialEq for LearningObservationTicket {
    fn eq(&self, other: &Self) -> bool {
        self.inner.id == other.inner.id
    }
}

impl Eq for LearningObservationTicket {}

impl<A> Clone for AnchorTicket<A> {
    fn clone(&self) -> Self {
        Self {
            inner: std::sync::Arc::clone(&self.inner),
        }
    }
}
impl<A> std::fmt::Debug for AnchorTicket<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Do not expose native field identities or values in diagnostics.
        f.write_str("AnchorTicket")
    }
}
impl<A> PartialEq for AnchorTicket<A> {
    fn eq(&self, other: &Self) -> bool {
        self.inner.id == other.inner.id
    }
}
impl<A> Eq for AnchorTicket<A> {}

impl<A: 'static> AnchorTicket<A> {
    fn retain(anchor: A, lifetime: Duration) -> (Self, AnchorLease<A>) {
        let id = NEXT_ANCHOR_TICKET_ID.fetch_add(1, Ordering::Relaxed);
        let undo_expires_at = std::time::Instant::now() + lifetime;
        let (sender, requests) = std::sync::mpsc::sync_channel::<AnchorRequest<A>>(1);
        let ticket = Self {
            inner: std::sync::Arc::new(AnchorTicketInner {
                id,
                sender,
                expires_at: undo_expires_at,
                #[cfg(all(target_os = "macos", feature = "research-harness"))]
                explicitly_released: std::sync::atomic::AtomicBool::new(false),
            }),
        };
        (
            ticket,
            AnchorLease {
                id,
                anchor,
                requests,
                undo_expires_at: Some(undo_expires_at),
                observation_expires_at: None,
            },
        )
    }

    pub(crate) fn expires_at(&self) -> std::time::Instant {
        self.inner.expires_at
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    pub(crate) fn release_and_wait_until(
        &self,
        deadline: std::time::Instant,
    ) -> Result<(), AnchorOwnerCloseError> {
        let result = release_anchor_grant_and_wait(
            &self.inner.sender,
            |reply| AnchorRequest::ReleaseUndoAndWait {
                ticket_id: self.inner.id,
                reply,
            },
            deadline,
        );
        if result.is_ok() {
            self.inner
                .explicitly_released
                .store(true, std::sync::atomic::Ordering::Release);
        }
        result
    }

    pub(crate) fn run(
        &self,
        operation: impl FnOnce(&A, &CancellationToken, std::time::Instant) -> Result<&'static str, String>
            + Send
            + 'static,
    ) -> Result<&'static str, String> {
        self.run_with_timeout(operation, Duration::from_secs(1))
    }

    fn run_with_timeout(
        &self,
        operation: impl FnOnce(&A, &CancellationToken, std::time::Instant) -> Result<&'static str, String>
            + Send
            + 'static,
        timeout: Duration,
    ) -> Result<&'static str, String> {
        let deadline = self
            .inner
            .expires_at
            .min(std::time::Instant::now() + timeout);
        let cancellation = CancellationToken::new();
        let (reply, receiver) = std::sync::mpsc::channel();
        if self
            .inner
            .sender
            .try_send(AnchorRequest::Undo(UndoAnchorRequest {
                operation: Box::new(operation),
                cancellation: cancellation.clone(),
                deadline,
                reply,
            }))
            .is_err()
        {
            return Ok("stale_target");
        }
        match receiver.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
            Ok(result) => result,
            Err(_) => {
                cancellation.cancel();
                Ok("stale_target")
            }
        }
    }
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn release_anchor_grant_and_wait<A>(
    sender: &std::sync::mpsc::SyncSender<AnchorRequest<A>>,
    request: impl FnOnce(std::sync::mpsc::Sender<Result<(), AnchorOwnerCloseError>>) -> AnchorRequest<A>,
    deadline: std::time::Instant,
) -> Result<(), AnchorOwnerCloseError> {
    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    if remaining.is_zero() {
        return Err(AnchorOwnerCloseError::Expired);
    }
    let (reply, receiver) = std::sync::mpsc::channel();
    match sender.try_send(request(reply)) {
        Ok(()) => {}
        Err(std::sync::mpsc::TrySendError::Full(_)) => {
            return Err(AnchorOwnerCloseError::Busy);
        }
        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
            return Err(AnchorOwnerCloseError::OwnerClosed);
        }
    }
    match receiver.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(AnchorOwnerCloseError::TimedOut),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err(AnchorOwnerCloseError::OwnerFailed)
        }
    }
}

impl AnchorTicket<FocusedFieldAnchor> {
    fn retain_with_learning_observation(
        anchor: FocusedFieldAnchor,
        undo_lifetime: Duration,
        observation_lifetime: Duration,
        observation_started_at: std::time::Instant,
        #[cfg(all(target_os = "macos", feature = "research-harness"))] research_guard: Option<
            RegisteredResearchPasteGuard,
        >,
    ) -> (
        Self,
        LearningObservationTicket,
        AnchorLease<FocusedFieldAnchor>,
    ) {
        let id = NEXT_ANCHOR_TICKET_ID.fetch_add(1, Ordering::Relaxed);
        let now = std::time::Instant::now();
        let undo_expires_at = now + undo_lifetime;
        let observation_expires_at = observation_started_at + observation_lifetime;
        let (sender, requests) =
            std::sync::mpsc::sync_channel::<AnchorRequest<FocusedFieldAnchor>>(3);
        let undo_ticket = Self {
            inner: std::sync::Arc::new(AnchorTicketInner {
                id,
                sender: sender.clone(),
                expires_at: undo_expires_at,
                #[cfg(all(target_os = "macos", feature = "research-harness"))]
                explicitly_released: std::sync::atomic::AtomicBool::new(false),
            }),
        };
        let observation_ticket = LearningObservationTicket {
            inner: std::sync::Arc::new(LearningObservationTicketInner {
                id,
                sender,
                expires_at: observation_expires_at,
                #[cfg(all(target_os = "macos", feature = "research-harness"))]
                research_guard,
                #[cfg(all(target_os = "macos", feature = "research-harness"))]
                explicitly_released: std::sync::atomic::AtomicBool::new(false),
            }),
        };
        let lease = AnchorLease {
            id,
            anchor,
            requests,
            undo_expires_at: Some(undo_expires_at),
            observation_expires_at: Some(observation_expires_at),
        };
        (undo_ticket, observation_ticket, lease)
    }
}

pub(crate) type UndoFieldTicket = AnchorTicket<FocusedFieldAnchor>;

#[cfg(all(target_os = "macos", feature = "research-harness"))]
#[derive(Default)]
struct ResearchPasteTicketHandoff {
    observation_ticket: Option<LearningObservationTicket>,
    undo_ticket: Option<UndoFieldTicket>,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
pub(crate) struct RegisteredResearchPasteAttempt {
    pub(crate) attempt: Result<TimedInsertOutcome, PasteError>,
    pub(crate) owner_completion: AnchorOwnerCompletion,
    /// These are the sole ticket owners for this research attempt. They stay
    /// available even when post-write resolution returns an error.
    pub(crate) observation_ticket: Option<LearningObservationTicket>,
    pub(crate) undo_ticket: Option<UndoFieldTicket>,
}

impl LearningObservationTicket {
    pub(crate) fn expires_at(&self) -> std::time::Instant {
        self.inner.expires_at
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    pub(crate) fn release_and_wait_until(
        &self,
        deadline: std::time::Instant,
    ) -> Result<(), AnchorOwnerCloseError> {
        let result = release_anchor_grant_and_wait(
            &self.inner.sender,
            |reply| AnchorRequest::ReleaseObservationAndWait {
                ticket_id: self.inner.id,
                reply,
            },
            deadline,
        );
        if result.is_ok() {
            self.inner
                .explicitly_released
                .store(true, std::sync::atomic::Ordering::Release);
        }
        result
    }

    pub(crate) fn read_sample(
        &self,
        cancellation: &CancellationToken,
        max_utf16: usize,
        observer_epoch: std::sync::Arc<AtomicU64>,
        expected_epoch: u64,
    ) -> Result<FocusedFieldReadSampleV1, FocusedFieldReadError> {
        let now = std::time::Instant::now();
        owner_read_permit(
            cancellation,
            self.inner.expires_at,
            &observer_epoch,
            expected_epoch,
        )?;
        let deadline = self
            .inner
            .expires_at
            .min(now + Duration::from_millis(1_500));
        let request_cancellation = cancellation.child_token();
        let operation_cancellation = request_cancellation.clone();
        #[cfg(all(target_os = "macos", feature = "research-harness"))]
        let research_guard = self.inner.research_guard.clone();
        let operation: FocusedFieldReadOperation<FocusedFieldAnchor> = Box::new(
            move |anchor, cancellation, owner_deadline, observer_epoch, expected_epoch| {
                #[cfg(all(target_os = "macos", feature = "research-harness"))]
                if let Some(research_guard) = research_guard.as_ref() {
                    owner_research_ticket_guard(
                        research_guard,
                        cancellation,
                        owner_deadline,
                        observer_epoch,
                        expected_epoch,
                    )?;
                }
                let focus_before = owner_ax_read(
                    cancellation,
                    owner_deadline,
                    observer_epoch,
                    expected_epoch,
                    || anchor.current_focus_matches(),
                )?;
                match focus_before {
                    Some(true) => {}
                    Some(false) => return Err(FocusedFieldReadError::FocusChanged),
                    None => return Err(FocusedFieldReadError::FocusUnknown),
                }
                // AX calls are synchronous and cannot be interrupted after they
                // begin. Check immediately after each call so a stale read never
                // proceeds into a later value, selection, or focus request.
                let value = owner_ax_read(
                    cancellation,
                    owner_deadline,
                    observer_epoch,
                    expected_epoch,
                    || anchor.read_value_bounded(max_utf16),
                )?;
                let Some(value) = value else {
                    return Err(FocusedFieldReadError::ValueUnavailable);
                };
                let selection_range_utf16 = owner_ax_read(
                    cancellation,
                    owner_deadline,
                    observer_epoch,
                    expected_epoch,
                    || anchor.read_selected_range(),
                )?;
                let focus_after = owner_ax_read(
                    cancellation,
                    owner_deadline,
                    observer_epoch,
                    expected_epoch,
                    || anchor.current_focus_matches(),
                )?;
                #[cfg(all(target_os = "macos", feature = "research-harness"))]
                if let Some(research_guard) = research_guard.as_ref() {
                    owner_research_ticket_guard(
                        research_guard,
                        cancellation,
                        owner_deadline,
                        observer_epoch,
                        expected_epoch,
                    )?;
                }
                match focus_after {
                    Some(true) => {}
                    Some(false) => return Err(FocusedFieldReadError::FocusChanged),
                    None => return Err(FocusedFieldReadError::FocusUnknown),
                }
                Ok(FocusedFieldReadSampleV1 {
                    value: Some(value),
                    selection_range_utf16,
                    focus_matches: focus_after,
                })
            },
        );
        let (reply, receiver) = std::sync::mpsc::channel();
        if self
            .inner
            .sender
            .try_send(AnchorRequest::Read(FocusedFieldReadRequest {
                operation,
                cancellation: operation_cancellation.clone(),
                deadline,
                observer_epoch: std::sync::Arc::clone(&observer_epoch),
                expected_epoch,
                reply,
            }))
            .is_err()
        {
            return Err(FocusedFieldReadError::Busy);
        }
        match receiver.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
            Ok(result) => {
                owner_read_permit(cancellation, deadline, &observer_epoch, expected_epoch)?;
                result
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                operation_cancellation.cancel();
                if cancellation.is_cancelled() {
                    Err(FocusedFieldReadError::Cancelled)
                } else if std::time::Instant::now() >= self.inner.expires_at {
                    Err(FocusedFieldReadError::Expired)
                } else {
                    Err(FocusedFieldReadError::TimedOut)
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err(FocusedFieldReadError::OwnerClosed)
            }
        }
    }
}

fn owner_read_permit(
    cancellation: &CancellationToken,
    deadline: std::time::Instant,
    observer_epoch: &AtomicU64,
    expected_epoch: u64,
) -> Result<(), FocusedFieldReadError> {
    if cancellation.is_cancelled() {
        return Err(FocusedFieldReadError::Cancelled);
    }
    if std::time::Instant::now() >= deadline {
        return Err(FocusedFieldReadError::Expired);
    }
    if observer_epoch.load(Ordering::Acquire) != expected_epoch {
        return Err(FocusedFieldReadError::StaleEpoch);
    }
    Ok(())
}

fn owner_ax_read<T>(
    cancellation: &CancellationToken,
    deadline: std::time::Instant,
    observer_epoch: &AtomicU64,
    expected_epoch: u64,
    read: impl FnOnce() -> T,
) -> Result<T, FocusedFieldReadError> {
    owner_read_permit(cancellation, deadline, observer_epoch, expected_epoch)?;
    let value = read();
    owner_read_permit(cancellation, deadline, observer_epoch, expected_epoch)?;
    Ok(value)
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn owner_research_ticket_guard(
    registered_guard: &RegisteredResearchPasteGuard,
    cancellation: &CancellationToken,
    deadline: std::time::Instant,
    observer_epoch: &AtomicU64,
    expected_epoch: u64,
) -> Result<(), FocusedFieldReadError> {
    owner_read_permit(cancellation, deadline, observer_epoch, expected_epoch)?;
    let mut research_guard = registered_guard.clone();
    research_guard.cancellation = cancellation.clone();
    research_guard.expires_at = research_guard.expires_at.min(deadline);
    if let Err(error) = research_guard.check() {
        return Err(research_paste_read_error(error, deadline));
    }
    owner_read_permit(cancellation, deadline, observer_epoch, expected_epoch)?;
    Ok(())
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn research_paste_read_error(
    error: PasteError,
    deadline: std::time::Instant,
) -> FocusedFieldReadError {
    if std::time::Instant::now() >= deadline {
        return FocusedFieldReadError::Expired;
    }
    match error {
        PasteError::Cancelled => FocusedFieldReadError::Cancelled,
        PasteError::InputChanged | PasteError::TargetChanged | PasteError::SecureInput => {
            FocusedFieldReadError::FocusChanged
        }
        PasteError::TargetUnavailable | PasteError::InputUnavailable => {
            FocusedFieldReadError::FocusUnknown
        }
        _ => FocusedFieldReadError::OwnerFailed,
    }
}

struct AnchorLease<A> {
    id: u64,
    anchor: A,
    requests: std::sync::mpsc::Receiver<AnchorRequest<A>>,
    undo_expires_at: Option<std::time::Instant>,
    observation_expires_at: Option<std::time::Instant>,
}

impl<A> AnchorLease<A> {
    fn serve<OwnerGuard: Drop>(mut self, mut owner_guard: Option<OwnerGuard>) {
        // Both privileges share one owner thread and retained native anchor,
        // but keep their own absolute deadlines. A learning read can never
        // extend the single-use Undo window, and dropping it closes only reads.
        loop {
            let now = std::time::Instant::now();
            if self.undo_expires_at.is_some_and(|expiry| now >= expiry) {
                self.undo_expires_at = None;
            }
            if self
                .observation_expires_at
                .is_some_and(|expiry| now >= expiry)
            {
                self.observation_expires_at = None;
                drop(owner_guard.take());
            }
            let next_expiry = match (self.undo_expires_at, self.observation_expires_at) {
                (Some(undo), Some(observe)) => Some(undo.min(observe)),
                (Some(undo), None) => Some(undo),
                (None, Some(observe)) => Some(observe),
                (None, None) => None,
            };
            let Some(next_expiry) = next_expiry else {
                return;
            };
            if now >= next_expiry {
                return;
            }
            let request = match self.requests.recv_timeout(next_expiry - now) {
                Ok(request) => request,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            };
            match request {
                AnchorRequest::ReleaseUndo { ticket_id } => {
                    if ticket_id == self.id {
                        self.undo_expires_at = None;
                    }
                }
                AnchorRequest::ReleaseObservation { ticket_id } => {
                    if ticket_id == self.id {
                        self.observation_expires_at = None;
                        // The owner processes this only after any active native
                        // read returns, so pipeline quiescence cannot precede it.
                        drop(owner_guard.take());
                    }
                }
                #[cfg(all(target_os = "macos", feature = "research-harness"))]
                AnchorRequest::ReleaseUndoAndWait { ticket_id, reply } => {
                    if ticket_id == self.id {
                        self.undo_expires_at = None;
                        let _ = reply.send(Ok(()));
                    } else {
                        let _ = reply.send(Err(AnchorOwnerCloseError::OwnerFailed));
                    }
                }
                #[cfg(all(target_os = "macos", feature = "research-harness"))]
                AnchorRequest::ReleaseObservationAndWait { ticket_id, reply } => {
                    if ticket_id == self.id {
                        self.observation_expires_at = None;
                        // This ACK follows any in-flight AX read and dropping
                        // the observation owner guard; it does not mean the
                        // separate Undo grant or the owner loop has closed.
                        drop(owner_guard.take());
                        let _ = reply.send(Ok(()));
                    } else {
                        let _ = reply.send(Err(AnchorOwnerCloseError::OwnerFailed));
                    }
                }
                AnchorRequest::Undo(request) => {
                    let now = std::time::Instant::now();
                    let Some(owner_expiry) = self.undo_expires_at else {
                        let _ = request.reply.send(Ok("stale_target"));
                        continue;
                    };
                    if request.cancellation.is_cancelled()
                        || now >= request.deadline
                        || now >= owner_expiry
                    {
                        let _ = request.reply.send(Ok("stale_target"));
                        continue;
                    }
                    // Undo is single-use even while the learning read lease
                    // remains active on this same owner.
                    self.undo_expires_at = None;
                    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        (request.operation)(
                            &self.anchor,
                            &request.cancellation,
                            request.deadline.min(owner_expiry),
                        )
                    }))
                    .unwrap_or_else(|_| Err("undo anchor worker panicked".into()));
                    let _ = request.reply.send(result);
                }
                AnchorRequest::Read(request) => {
                    let now = std::time::Instant::now();
                    let Some(owner_expiry) = self.observation_expires_at else {
                        let _ = request.reply.send(Err(FocusedFieldReadError::Expired));
                        continue;
                    };
                    if request.cancellation.is_cancelled()
                        || now >= request.deadline
                        || now >= owner_expiry
                    {
                        let _ = request
                            .reply
                            .send(Err(if request.cancellation.is_cancelled() {
                                FocusedFieldReadError::Cancelled
                            } else {
                                FocusedFieldReadError::Expired
                            }));
                        continue;
                    }
                    let owner_deadline = request.deadline.min(owner_expiry);
                    if let Err(error) = owner_read_permit(
                        &request.cancellation,
                        owner_deadline,
                        &request.observer_epoch,
                        request.expected_epoch,
                    ) {
                        let _ = request.reply.send(Err(error));
                        continue;
                    }
                    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        (request.operation)(
                            &self.anchor,
                            &request.cancellation,
                            owner_deadline,
                            &request.observer_epoch,
                            request.expected_epoch,
                        )
                    }))
                    .unwrap_or(Err(FocusedFieldReadError::OwnerFailed));
                    let _ = request.reply.send(result);
                }
            }
        }
    }
}

/// Construct and retain native anchors entirely on one owner thread. Return
/// only the Send result before serving its separate bounded Undo/read leases.
fn run_delivery_owner<A: 'static, T: Send + 'static>(
    operation: impl FnOnce(&mut Option<AnchorLease<A>>) -> T + Send + 'static,
) -> Result<T, String> {
    let (result_sender, result_receiver) = std::sync::mpsc::channel();
    let research_trace = crate::research_trace::current();
    // Acquire before spawning so terminal/quiescence cannot race ahead of the
    // owner that may receive a bounded learning read request.
    let owner_learning_task = crate::research_trace::sync_scope(
        research_trace.clone(),
        crate::research_trace::learning_task,
    );
    thread::Builder::new()
        .name("voiceflow-paste-owner".into())
        .spawn(move || {
            let mut owner_learning_task = Some(owner_learning_task);
            let mut lease = None;
            let result =
                crate::research_trace::sync_scope(research_trace, || operation(&mut lease));
            if lease
                .as_ref()
                .is_none_or(|lease| lease.observation_expires_at.is_none())
            {
                // Undo can outlive the learning read grant. It must not keep
                // the research learning pipeline open on its own.
                drop(owner_learning_task.take());
            }
            if result_sender.send(result).is_err() {
                return;
            }
            if let Some(lease) = lease {
                lease.serve(owner_learning_task.take());
            }
        })
        .map_err(|error| format!("paste owner failed: {error}"))?;
    // Paste can already have posted a keyboard event; it must finish before
    // a caller chooses a fallback. Do not detach an in-flight paste on timeout.
    result_receiver
        .recv()
        .map_err(|_| "paste owner ended before readback".to_owned())
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn run_delivery_owner_with_completion<A: 'static, T: Send + 'static>(
    operation: impl FnOnce(&mut Option<AnchorLease<A>>) -> T + Send + 'static,
) -> (Result<T, String>, AnchorOwnerCompletion) {
    let (result_sender, result_receiver) = std::sync::mpsc::channel();
    let (terminal_sender, terminal_receiver) = std::sync::mpsc::channel();
    let research_trace = crate::research_trace::current();
    let owner_learning_task = crate::research_trace::sync_scope(
        research_trace.clone(),
        crate::research_trace::learning_task,
    );
    let spawned = thread::Builder::new()
        .name("voiceflow-research-paste-owner".into())
        .spawn(move || {
            let mut owner_learning_task = Some(owner_learning_task);
            let mut lease = None;
            let operation_result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                crate::research_trace::sync_scope(research_trace, || operation(&mut lease))
            }));
            let operation_failed = operation_result.is_err();
            let result =
                operation_result.map_err(|_| "research paste owner operation panicked".to_owned());
            if lease
                .as_ref()
                .is_none_or(|lease| lease.observation_expires_at.is_none())
            {
                drop(owner_learning_task.take());
            }
            let _ = result_sender.send(result);
            let serve_result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                if let Some(lease) = lease {
                    lease.serve(owner_learning_task.take());
                }
                drop(owner_learning_task.take());
            }));
            let terminal = if operation_failed || serve_result.is_err() {
                AnchorOwnerTerminal::Failed
            } else {
                AnchorOwnerTerminal::Closed
            };
            let _ = terminal_sender.send(terminal);
        });
    let join_handle = match spawned {
        Ok(join_handle) => join_handle,
        Err(error) => {
            return (
                Err(format!("research paste owner failed to start: {error}")),
                AnchorOwnerCompletion::not_started(),
            );
        }
    };
    let completion = AnchorOwnerCompletion::started(terminal_receiver, join_handle);
    let result = wait_research_owner_result(result_receiver);
    (result, completion)
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn wait_research_owner_result<T>(
    receiver: std::sync::mpsc::Receiver<Result<T, String>>,
) -> Result<T, String> {
    if unsafe { libc::pthread_main_np() } == 0 {
        return receiver
            .recv()
            .map_err(|_| "research paste owner ended before returning its outcome".to_owned())?;
    }

    loop {
        match receiver.try_recv() {
            Ok(result) => return result,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return Err("research paste owner ended before returning its outcome".into());
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        core_foundation::runloop::CFRunLoop::run_in_mode(
            // SAFETY: Core Foundation exports this process-lifetime mode constant.
            unsafe { core_foundation::runloop::kCFRunLoopDefaultMode },
            Duration::from_millis(5),
            true,
        );
    }
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
pub(crate) fn run_research_main_queue_dispatch_probe() -> Result<bool, String> {
    if unsafe { libc::pthread_main_np() } == 0 {
        return Err("research_main_queue_probe_requires_os_main_thread".into());
    }

    let (probe_result, mut owner_completion) = run_delivery_owner_with_completion::<(), _>(|_| {
        crate::input_source::research_main_queue_probe()
    });
    let main_queue_result = probe_result?;
    match owner_completion.settle_after_phase() {
        Ok(AnchorOwnerTerminal::Closed) => Ok(main_queue_result),
        Ok(AnchorOwnerTerminal::Failed) => Err("research_main_queue_probe_owner_failed".into()),
        Err(error) => Err(format!(
            "research_main_queue_probe_owner_not_settled_{error:?}"
        )),
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct InsertOutcome {
    /// The platform shortcut was posted, or a safe AX insert was applied.
    pub shortcut_sent: bool,
    /// True only if Cmd+V was posted. AX value sets are often not on the
    /// target undo stack, so a 3s Cmd+Z must never be armed for that path.
    pub used_keyboard_paste: bool,
    /// Best-effort proof that the focused field now contains the delivered text.
    /// Cmd+V or AX set alone cannot prove that the target application accepted it.
    pub verified: bool,
    /// Which readback proof authorized the result. Full-range proof confirms
    /// delivery but intentionally does not arm Undo or dictionary learning.
    pub(crate) verification: Option<PasteVerificationFact>,
    /// In-memory fingerprint of the complete focused input value immediately
    /// after a verified paste. Undo uses this to avoid undoing later user edits.
    pub post_insert_input_fingerprint: Option<u64>,
    /// Exact delivery-time field identity, kept only in memory for Undo.
    pub(crate) post_insert_target_guard: Option<crate::context::TargetAppGuard>,
    pub(crate) post_insert_field_ticket: Option<UndoFieldTicket>,
    /// Checked UTF-16 delivery coordinates from the original paste/readback owner.
    pub(crate) confirmed_insertion: Option<ConfirmedInsertionV1>,
    /// Repeated read capability for the same anchor, with a separate 12s cap.
    pub(crate) learning_observation_ticket: Option<LearningObservationTicket>,
    /// Focused field value immediately after a successful insert. Dictionary
    /// learning diffs this baseline against a later same-field read.
    pub value_after: Option<String>,
}

impl std::fmt::Debug for InsertOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InsertOutcome")
            .field("shortcut_sent", &self.shortcut_sent)
            .field("used_keyboard_paste", &self.used_keyboard_paste)
            .field("verified", &self.verified)
            .field("verification", &self.verification)
            .field(
                "post_insert_input_fingerprint",
                &self.post_insert_input_fingerprint,
            )
            .field(
                "undo_field_captured",
                &self.post_insert_field_ticket.is_some(),
            )
            .field("confirmed_insertion", &self.confirmed_insertion.is_some())
            .field(
                "learning_observer_retained",
                &self.learning_observation_ticket.is_some(),
            )
            .finish_non_exhaustive()
    }
}

/// Actual clipboard-owner outcome; sending a restore is not restoration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClipboardRestoration {
    NotNeeded,
    Restored,
    OwnershipChanged,
    Unknown,
}

fn restoration_fact(
    written: bool,
    write_failed: bool,
    owned: Option<bool>,
    attempted: bool,
    restored: Option<bool>,
) -> ClipboardRestoration {
    if !written {
        ClipboardRestoration::NotNeeded
    } else if write_failed {
        ClipboardRestoration::Unknown
    } else if attempted && restored == Some(true) {
        ClipboardRestoration::Restored
    } else if (attempted && restored == Some(false)) || owned == Some(false) {
        ClipboardRestoration::OwnershipChanged
    } else {
        ClipboardRestoration::Unknown
    }
}

impl PasteError {
    pub(crate) fn clipboard_restoration(&self) -> ClipboardRestoration {
        self.delivery_diagnostic()
            .map(|d| {
                restoration_fact(
                    d.clipboard_write_attempted,
                    d.clipboard_write_failed,
                    d.clipboard_write_owned,
                    d.clipboard_restore_attempted,
                    d.clipboard_restored,
                )
            })
            .unwrap_or(ClipboardRestoration::Unknown)
    }
    pub(crate) fn bounded_code(&self) -> &'static str {
        match self {
            Self::Clipboard(_) => "clipboard_error",
            Self::Accessibility => "accessibility_required",
            Self::TargetChanged => "target_changed",
            Self::TargetUnavailable => "target_unavailable",
            Self::BrowserAccessRequired => "browser_access_required",
            Self::InputUnavailable => "input_unavailable",
            Self::SecureInput => "secure_input",
            Self::SystemSecureInputOtherOwner => "secure_input_owner_unmatched",
            Self::InputChanged => "input_changed",
            Self::SelectionUnavailable => "selection_unavailable",
            Self::SelectionChanged => "selection_changed",
            Self::Input(_) => "input_error",
            Self::Cancelled => "cancelled",
            Self::MutationUncertain => "mutation_uncertain",
            Self::Diagnosed { source, .. } => source.bounded_code(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedInsertOutcome {
    pub(crate) clipboard_restoration: ClipboardRestoration,
    pub outcome: InsertOutcome,
    /// Time spent submitting the safe AX edit or keyboard shortcut.
    pub paste_submission: Duration,
    /// Time spent waiting for and checking the focused field after submission.
    pub readback_confirmation: Duration,
    /// Present when insertion was not confirmed. Contains only stable codes
    /// and safe delivery facts; it never contains dictated text.
    pub diagnostic: Option<DeliveryDiagnostic>,
}

#[derive(Default)]
struct DeliveryTrace {
    stage: &'static str,
    target_app: Option<String>,
    accessibility_available: bool,
    target_verified: Option<bool>,
    focused_input_available: Option<bool>,
    clipboard_snapshot_captured: bool,
    clipboard_unchanged_before_write: Option<bool>,
    clipboard_write_attempted: bool,
    clipboard_write_failed: bool,
    clipboard_write_owned: Option<bool>,
    clipboard_marker: Option<ClipboardCopyMarker>,
    keyboard_paste_attempted: bool,
    keyboard_paste_may_have_been_posted: bool,
    paste_verified: Option<bool>,
    paste_verification: Option<PasteVerificationFact>,
    paste_verification_delay_ms: Option<u64>,
    paste_readback_poll_count: Option<u8>,
    paste_readback_elapsed_ms: Option<u64>,
    paste_expected_utf16_length: Option<u32>,
    paste_actual_utf16_length: Option<u32>,
    paste_readback_mismatch_kind: Option<&'static str>,
    paste_full_range_attempted: bool,
    clipboard_restore_attempted: bool,
    clipboard_restored: Option<bool>,
}

impl DeliveryTrace {
    fn emit_research_resolution(&self) {
        let Some(research) = crate::research_trace::current() else {
            return;
        };
        research.emit(crate::research_trace::Fact::DeliveryTargetReadback {
            verified: self.paste_verified,
        });
        let (resolution, known) = if !self.clipboard_write_attempted {
            ("no_write", true)
        } else if self.clipboard_write_failed {
            ("write_failed_partial_unknown", false)
        } else if self.clipboard_restored == Some(true) {
            ("restored", true)
        } else if self.clipboard_restored == Some(false)
            || self.clipboard_write_owned == Some(false)
        {
            ("ownership_changed", true)
        } else if !self.clipboard_restore_attempted && self.clipboard_write_owned == Some(true) {
            ("retained_dictation", true)
        } else {
            ("restore_unknown", false)
        };
        research.clipboard_resolved(resolution, known);
    }
}

impl DeliveryTrace {
    fn new(accessibility_available: bool, expected_pid: i32) -> Self {
        let current = crate::context::probe_focus_guard();
        let target_app = (current.pid == expected_pid)
            .then_some(current.bundle_id)
            .flatten()
            .map(|bundle_id| {
                bundle_id
                    .chars()
                    .filter(|character| {
                        character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
                    })
                    .take(128)
                    .collect::<String>()
            })
            .filter(|bundle_id| !bundle_id.is_empty());
        Self {
            stage: "target_guard",
            target_app,
            accessibility_available,
            ..Self::default()
        }
    }

    fn diagnostic(&self, code: &'static str, user_reason: &'static str) -> DeliveryDiagnostic {
        DeliveryDiagnostic {
            code,
            user_reason,
            stage: self.stage,
            target_app: self.target_app.clone(),
            accessibility_available: self.accessibility_available,
            target_verified: self.target_verified,
            focused_input_available: self.focused_input_available,
            clipboard_snapshot_captured: self.clipboard_snapshot_captured,
            clipboard_unchanged_before_write: self.clipboard_unchanged_before_write,
            clipboard_write_attempted: self.clipboard_write_attempted,
            clipboard_write_failed: self.clipboard_write_failed,
            clipboard_write_owned: self.clipboard_write_owned,
            keyboard_paste_attempted: self.keyboard_paste_attempted,
            keyboard_paste_may_have_been_posted: self.keyboard_paste_may_have_been_posted,
            paste_verified: self.paste_verified,
            paste_verification: self.paste_verification,
            paste_verification_delay_ms: self.paste_verification_delay_ms,
            paste_readback_poll_count: self.paste_readback_poll_count,
            paste_readback_elapsed_ms: self.paste_readback_elapsed_ms,
            paste_expected_utf16_length: self.paste_expected_utf16_length,
            paste_actual_utf16_length: self.paste_actual_utf16_length,
            paste_readback_mismatch_kind: self.paste_readback_mismatch_kind,
            paste_full_range_attempted: self.paste_full_range_attempted,
            clipboard_restore_attempted: self.clipboard_restore_attempted,
            clipboard_restored: self.clipboard_restored,
        }
    }

    fn refresh_clipboard_ownership(
        &mut self,
        snapshot: &ClipboardSnapshot,
        effects: &DeliveryEffects,
    ) {
        let (stage, callback_count) = if self.clipboard_restore_attempted {
            (
                ClipboardMarkerCheckStageV1::PreRestore,
                u8::from(self.keyboard_paste_may_have_been_posted),
            )
        } else if self.keyboard_paste_may_have_been_posted {
            (ClipboardMarkerCheckStageV1::PostCallback, 1)
        } else {
            (ClipboardMarkerCheckStageV1::PreBoundary, 0)
        };
        self.refresh_clipboard_ownership_at(snapshot, effects, stage, callback_count);
    }

    fn refresh_clipboard_ownership_at(
        &mut self,
        snapshot: &ClipboardSnapshot,
        effects: &DeliveryEffects,
        stage: ClipboardMarkerCheckStageV1,
        callback_count: u8,
    ) {
        if self.clipboard_write_attempted {
            self.clipboard_write_owned = self.clipboard_marker.as_ref().and_then(|marker| {
                snapshot
                    .marker_is_current(effects, marker, stage, callback_count)
                    .ok()
            });
        }
    }

    fn for_unverified_paste(&self) -> DeliveryDiagnostic {
        let fact = self
            .paste_verification
            .filter(|fact| fact.is_unverified_attempt())
            .unwrap_or(PasteVerificationFact::ShortcutOutcomeUnknown);
        let (code, user_reason) = fact.diagnostic_copy();
        self.diagnostic(code, user_reason)
    }

    fn for_error(&self, error: &PasteError) -> DeliveryDiagnostic {
        if self.clipboard_unchanged_before_write == Some(false) {
            return self.diagnostic(
                "clipboard_changed",
                "剪贴板已被其他应用更新，已保留较新的内容；请从历史记录复制听写文字",
            );
        }
        if self.keyboard_paste_may_have_been_posted {
            return self.for_unverified_paste();
        }
        if self.clipboard_write_failed && self.clipboard_write_owned == Some(true) {
            return self.diagnostic(
                "paste_unverified",
                "无法确认自动粘贴结果，听写文字仍在剪贴板；请按 ⌘V 粘贴",
            );
        }
        if self.clipboard_write_attempted && self.clipboard_write_owned != Some(true) {
            return match self.clipboard_write_owned {
                Some(false) => self.diagnostic(
                    "clipboard_changed",
                    if self.keyboard_paste_may_have_been_posted {
                        "粘贴状态无法确认，剪贴板已被其他应用更新；请先检查输入框，再从历史记录复制听写文字"
                    } else {
                        "剪贴板已被其他应用更新，已保留较新的内容；请从历史记录复制听写文字"
                    },
                ),
                None => self.diagnostic(
                    "clipboard_ownership_unverified",
                    if self.keyboard_paste_may_have_been_posted {
                        "粘贴状态和剪贴板所有权均无法确认，未重复粘贴；请先检查输入框，再从历史记录复制听写文字"
                    } else {
                        "无法确认剪贴板内容，已保留当前内容；请从历史记录复制听写文字"
                    },
                ),
                Some(true) => unreachable!(),
            };
        }
        if matches!(
            self.stage,
            "clipboard_ownership" | "pre_paste_clipboard_guard" | "keyboard_paste"
        ) && self.clipboard_write_owned == Some(false)
        {
            return self.diagnostic(
                "clipboard_changed",
                "剪贴板已被其他应用更新，已保留较新的内容；请从历史记录复制听写文字",
            );
        }
        if self.clipboard_write_owned == Some(false) {
            return self.diagnostic(
                "clipboard_ownership_unverified",
                "无法确认剪贴板内容，已保留当前内容；请从历史记录复制听写文字",
            );
        }
        let (code, reason) = match error {
            PasteError::Accessibility => (
                "accessibility_required",
                if self.clipboard_write_owned == Some(true) {
                    "自动粘贴需要辅助功能权限，听写文字已保留在剪贴板；请按 ⌘V 粘贴"
                } else {
                    "开启辅助功能权限后才能插入"
                },
            ),
            PasteError::TargetChanged => (
                "target_changed",
                if self.clipboard_write_owned == Some(true) {
                    "输入目标已变化，听写文字已保留在剪贴板；请按 ⌘V 粘贴"
                } else {
                    "目标已变化"
                },
            ),
            PasteError::TargetUnavailable => (
                "target_unavailable",
                if self.clipboard_write_owned == Some(true) {
                    "无法确认输入目标，听写文字已保留在剪贴板；请按 ⌘V 粘贴"
                } else {
                    "无法确认输入目标"
                },
            ),
            PasteError::BrowserAccessRequired => (
                "browser_permission_required",
                if self.clipboard_write_owned == Some(true) {
                    "需要浏览器访问权限确认输入目标，听写文字已保留在剪贴板；请按 ⌘V 粘贴"
                } else {
                    "允许浏览器访问后才能安全插入"
                },
            ),
            PasteError::SecureInput => (
                "secure_input",
                if self.clipboard_write_owned == Some(true) {
                    "安全输入阻止自动粘贴，文字已复制；请手动粘贴"
                } else {
                    "安全输入阻止自动粘贴"
                },
            ),
            PasteError::SystemSecureInputOtherOwner => (
                "secure_input_owner_unmatched",
                if self.clipboard_write_owned == Some(true) {
                    "系统安全输入由其他或未知进程持有，听写文字已保留在剪贴板；请按 ⌘V 粘贴"
                } else {
                    "系统安全输入由其他或未知进程持有，听写结果保存在历史记录中"
                },
            ),
            PasteError::InputUnavailable => (
                "input_unavailable",
                if self.clipboard_write_owned == Some(true) {
                    "未找到可编辑输入框，听写文字已保留在剪贴板；请按 ⌘V 粘贴"
                } else {
                    "请将光标放在可编辑输入框中"
                },
            ),
            PasteError::InputChanged => (
                "input_changed",
                if self.clipboard_write_owned == Some(true) {
                    "输入框已变化，听写文字已保留在剪贴板；请按 ⌘V 粘贴"
                } else {
                    "输入框已变化"
                },
            ),
            PasteError::Clipboard(_) if self.stage == "clipboard_snapshot" => {
                ("clipboard_unavailable", "剪贴板不可用")
            }
            PasteError::Clipboard(_) if self.stage == "clipboard_write" => {
                ("clipboard_write_failed", "剪贴板写入失败")
            }
            PasteError::Clipboard(_) => ("clipboard_unavailable", "剪贴板不可用"),
            PasteError::MutationUncertain => (
                "paste_mutation_uncertain",
                "输入状态无法确认，未重复粘贴；请检查输入框和历史记录",
            ),
            PasteError::Cancelled => ("delivery_cancelled", "听写已取消"),
            PasteError::Diagnosed { diagnostic, .. } => {
                return diagnostic.as_ref().clone();
            }
            PasteError::SelectionUnavailable | PasteError::SelectionChanged => (
                "input_changed",
                if self.clipboard_write_owned == Some(true) {
                    "输入框已变化，听写文字已保留在剪贴板；请按 ⌘V 粘贴"
                } else {
                    "输入框已变化"
                },
            ),
            PasteError::Input(_) if self.stage == "keyboard_paste" => (
                "keyboard_paste_failed",
                if self.clipboard_write_owned == Some(true) {
                    "自动粘贴未完成，听写文字已保留在剪贴板；请按 ⌘V 粘贴"
                } else {
                    "自动粘贴未完成"
                },
            ),
            PasteError::Input(_) => (
                "delivery_failed",
                if self.clipboard_write_owned == Some(true) {
                    "自动插入未完成，听写文字已保留在剪贴板；请按 ⌘V 粘贴"
                } else {
                    "自动插入未完成"
                },
            ),
        };
        self.diagnostic(code, reason)
    }
}

pub fn selection_fingerprint(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn verify_same_field_target(
    expected: &crate::context::TargetAppGuard,
    current: &crate::context::TargetAppGuard,
) -> Result<(), PasteError> {
    match crate::context::same_field_mismatch_reason(expected, current, false) {
        None => Ok(()),
        Some("secure_input") => Err(PasteError::SecureInput),
        Some("input_unavailable") => Err(PasteError::InputUnavailable),
        Some("target_unavailable") => Err(PasteError::TargetUnavailable),
        Some(_) => Err(PasteError::TargetChanged),
    }
}

#[derive(Debug)]
pub(crate) struct PasteAttempt {
    shortcut_sent: bool,
    pub(crate) result: Result<(), PasteError>,
}

impl PasteAttempt {
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

/// Test-only legacy classifier; production restoration also covers owned
/// pre-shortcut cancellations after an ownership recheck.
/// Cancellation never restores the clipboard after a possible shortcut post.
#[cfg(test)]
fn should_restore_clipboard(attempt: &PasteAttempt) -> bool {
    !attempt.shortcut_sent
}

pub(crate) fn build_insert_outcome(
    used_keyboard_paste: bool,
    mut value_after: Option<String>,
    verification: PasteVerificationFact,
) -> InsertOutcome {
    let verified = !verification.is_unverified_attempt();
    if verification == PasteVerificationFact::VerifiedFullRange {
        // The alternate proof establishes exact delivery but does not make
        // the mismatching primary AXValue a trusted post-insert baseline.
        value_after = None;
    }
    let post_insert_input_fingerprint = if verification == PasteVerificationFact::Verified {
        value_after.as_deref().map(selection_fingerprint)
    } else {
        None
    };
    InsertOutcome {
        shortcut_sent: true,
        used_keyboard_paste,
        verified,
        verification: Some(verification),
        post_insert_input_fingerprint,
        post_insert_target_guard: None,
        post_insert_field_ticket: None,
        confirmed_insertion: None,
        learning_observation_ticket: None,
        value_after,
    }
}

fn build_verified_ax_outcome(value_after: String) -> InsertOutcome {
    InsertOutcome {
        shortcut_sent: true,
        used_keyboard_paste: false,
        verified: true,
        verification: Some(PasteVerificationFact::Verified),
        post_insert_input_fingerprint: Some(selection_fingerprint(&value_after)),
        post_insert_target_guard: None,
        post_insert_field_ticket: None,
        confirmed_insertion: None,
        learning_observation_ticket: None,
        value_after: Some(value_after),
    }
}

/// Run the irreversible keyboard injection only after the last cancellation and
/// target checks. This stays synchronous because the caller already runs inside
/// `spawn_blocking`; detaching it behind a timeout could let a timed-out paste
/// arrive after the caller had fallen back to another delivery path.
#[cfg(any(test, all(target_os = "macos", feature = "research-harness")))]
pub(crate) fn run_paste_attempt(
    cancellation: &CancellationToken,
    verify_target: impl Fn() -> Result<(), PasteError>,
    authorize_post: impl FnOnce() -> Result<(), PasteError>,
    simulate: impl FnOnce(&mut dyn FnMut()) -> Result<(), PasteError>,
) -> PasteAttempt {
    run_paste_attempt_with_post_ack_recheck(
        cancellation,
        verify_target,
        authorize_post,
        || Ok(()),
        simulate,
    )
}

fn run_paste_attempt_with_post_ack_recheck(
    cancellation: &CancellationToken,
    verify_target: impl Fn() -> Result<(), PasteError>,
    authorize_post: impl FnOnce() -> Result<(), PasteError>,
    verify_after_authorize: impl Fn() -> Result<(), PasteError>,
    simulate: impl FnOnce(&mut dyn FnMut()) -> Result<(), PasteError>,
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
        authorize_post()?;
        verify_after_authorize()?;
        if cancellation.is_cancelled() {
            return Err(PasteError::Cancelled);
        }
        let mut mark_may_have_posted = || {
            shortcut_attempted.store(true, std::sync::atomic::Ordering::Release);
        };
        simulate(&mut mark_may_have_posted)
    }))
    .unwrap_or_else(|_| Err(PasteError::Input("paste worker panicked".into())));
    let shortcut_sent = shortcut_attempted.load(std::sync::atomic::Ordering::Acquire);
    match result {
        Ok(()) => PasteAttempt {
            shortcut_sent,
            result: Ok(()),
        },
        Err(error) if shortcut_sent => PasteAttempt::failed_after_send(error),
        Err(error) => PasteAttempt::not_sent(error),
    }
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn named_remote_probe_result(
    case: RegisteredNamedRemoteClipboardCaseV1,
    result: RemoteProbeChildResultKindV1,
    child_callback_count: u8,
    restore: RemoteProbeChildRestoreV1,
) -> RemoteProbeChildResultV1 {
    RemoteProbeChildResultV1 {
        case,
        result,
        child_callback_count,
        shortcut_sent: false,
        target_dispatch_count: 0,
        restore,
    }
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn named_remote_probe_failure_kind(snapshot: &ClipboardSnapshot) -> RemoteProbeChildResultKindV1 {
    if let Some(error) = snapshot.last_remote_client_error() {
        return match error {
            ClipboardGuardClientErrorV1::AcknowledgementUnavailable => {
                RemoteProbeChildResultKindV1::AcknowledgementUnavailable
            }
            ClipboardGuardClientErrorV1::FrameOverBudget
            | ClipboardGuardClientErrorV1::MalformedFrame
            | ClipboardGuardClientErrorV1::VersionMismatch
            | ClipboardGuardClientErrorV1::LeaseMismatch
            | ClipboardGuardClientErrorV1::SequenceMismatch
            | ClipboardGuardClientErrorV1::UnexpectedReply => {
                RemoteProbeChildResultKindV1::ProtocolRejected
            }
            ClipboardGuardClientErrorV1::Io | ClipboardGuardClientErrorV1::Closed => {
                RemoteProbeChildResultKindV1::Unknown
            }
        };
    }
    match snapshot.last_remote_outcome() {
        Some(ClipboardGuardRpcOutcomeV1::Rejected) => {
            RemoteProbeChildResultKindV1::ProtocolRejected
        }
        _ => RemoteProbeChildResultKindV1::Unknown,
    }
}

/// Exercise the actual named-clipboard facade and final post-permit helper
/// without constructing an AX target or posting a CGEvent. The `simulate`
/// closure records only that the callback would run after the parent permit.
#[cfg(all(target_os = "macos", feature = "research-harness"))]
pub(crate) fn run_research_named_remote_clipboard_probe(
    input: RemoteProbeChildInputV1,
) -> RemoteProbeChildResultV1 {
    if input.board != RemoteClipboardBoardV1::NamedFixture {
        return named_remote_probe_result(
            input.case,
            RemoteProbeChildResultKindV1::ProtocolRejected,
            0,
            RemoteProbeChildRestoreV1::Unknown,
        );
    }
    run_research_remote_clipboard_probe(input)
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
pub(crate) fn run_research_remote_clipboard_probe(
    input: RemoteProbeChildInputV1,
) -> RemoteProbeChildResultV1 {
    use std::cell::Cell;

    let case = input.case;
    if input.board == RemoteClipboardBoardV1::GeneralClipboard
        && case != RegisteredNamedRemoteClipboardCaseV1::Positive
    {
        return named_remote_probe_result(
            case,
            RemoteProbeChildResultKindV1::ProtocolRejected,
            0,
            RemoteProbeChildRestoreV1::Unknown,
        );
    }
    let effects =
        DeliveryEffects::research_remote(Arc::clone(&input.client), input.lease_id, input.board);
    let snapshot = match ClipboardSnapshot::capture(&effects) {
        Ok(snapshot) => snapshot,
        Err(_) => {
            return named_remote_probe_result(
                case,
                RemoteProbeChildResultKindV1::Unknown,
                0,
                RemoteProbeChildRestoreV1::Unknown,
            );
        }
    };
    match snapshot.capture_fresh_unchanged() {
        Ok(Some(true)) => {}
        Ok(Some(false)) => {
            return named_remote_probe_result(
                case,
                RemoteProbeChildResultKindV1::PrewriteOwnershipChanged,
                0,
                RemoteProbeChildRestoreV1::NotNeeded,
            );
        }
        Ok(None) | Err(_) => {
            return named_remote_probe_result(
                case,
                named_remote_probe_failure_kind(&snapshot),
                0,
                RemoteProbeChildRestoreV1::Unknown,
            );
        }
    }
    match snapshot.unchanged_since_capture(&effects) {
        Ok(true) => {}
        Ok(false) => {
            return named_remote_probe_result(
                case,
                RemoteProbeChildResultKindV1::PrewriteOwnershipChanged,
                0,
                RemoteProbeChildRestoreV1::NotNeeded,
            );
        }
        Err(_) => {
            return named_remote_probe_result(
                case,
                named_remote_probe_failure_kind(&snapshot),
                0,
                RemoteProbeChildRestoreV1::Unknown,
            );
        }
    }

    let registered_text = registered_remote_temporary_text(input.board);
    if snapshot
        .write_temporary_text(&effects, registered_text, "")
        .is_err()
    {
        return named_remote_probe_result(
            case,
            named_remote_probe_failure_kind(&snapshot),
            0,
            RemoteProbeChildRestoreV1::Unknown,
        );
    }
    let marker = match snapshot.marker_after_write(&effects, registered_text, "") {
        Ok(Some(marker)) => marker,
        Ok(None) | Err(_) => {
            return named_remote_probe_result(
                case,
                named_remote_probe_failure_kind(&snapshot),
                0,
                RemoteProbeChildRestoreV1::Unknown,
            );
        }
    };

    let callback_count = Cell::new(0_u8);
    let permit_result = Cell::new(None);
    let cancellation = CancellationToken::new();
    let simulated_target_check = || Ok::<(), PasteError>(());
    let simulated_focus_check = || Ok::<(), PasteError>(());
    let simulated_secure_input_check = || Ok::<(), PasteError>(());
    let attempt = run_paste_attempt(
        &cancellation,
        || {
            // These gates are deliberately simulated. This probe establishes
            // the shared callback order, not registered-target authority.
            simulated_target_check()?;
            simulated_focus_check()?;
            simulated_secure_input_check()?;
            if snapshot.marker_is_current(
                &effects,
                &marker,
                ClipboardMarkerCheckStageV1::PreBoundary,
                callback_count.get(),
            )? {
                Ok(())
            } else {
                Err(PasteError::Clipboard(
                    "named clipboard changed before post boundary".into(),
                ))
            }
        },
        || {
            let result = effects.request_shortcut_post(&marker);
            permit_result.set(Some(result));
            match result {
                ClipboardPostPermitResultV1::RecordedOnly => Ok(()),
                ClipboardPostPermitResultV1::Allowed => Err(PasteError::Clipboard(
                    "named clipboard permit cannot authorize a target post".into(),
                )),
                ClipboardPostPermitResultV1::Denied => Err(PasteError::Clipboard(
                    "named clipboard post permit denied".into(),
                )),
                ClipboardPostPermitResultV1::AcknowledgementUnavailable => {
                    Err(PasteError::Clipboard(
                        "named clipboard post permit acknowledgement unavailable".into(),
                    ))
                }
                ClipboardPostPermitResultV1::ProtocolRejected => Err(PasteError::Clipboard(
                    "named clipboard post permit was rejected".into(),
                )),
                ClipboardPostPermitResultV1::Unknown => Err(PasteError::Clipboard(
                    "named clipboard post permit is unknown".into(),
                )),
            }
        },
        |_| {
            callback_count.set(callback_count.get().saturating_add(1));
            Ok(())
        },
    );

    let terminal_permit_failure = match permit_result.get() {
        Some(ClipboardPostPermitResultV1::AcknowledgementUnavailable) => {
            Some(RemoteProbeChildResultKindV1::AcknowledgementUnavailable)
        }
        Some(ClipboardPostPermitResultV1::ProtocolRejected) => {
            Some(RemoteProbeChildResultKindV1::ProtocolRejected)
        }
        Some(ClipboardPostPermitResultV1::Unknown) => Some(RemoteProbeChildResultKindV1::Unknown),
        _ => None,
    };
    if let Some(result_kind) = terminal_permit_failure {
        return named_remote_probe_result(
            case,
            result_kind,
            callback_count.get(),
            RemoteProbeChildRestoreV1::Unknown,
        );
    }

    let permit_kind = match permit_result.get() {
        Some(ClipboardPostPermitResultV1::Denied) => {
            Some(RemoteProbeChildResultKindV1::PermitDenied)
        }
        Some(ClipboardPostPermitResultV1::AcknowledgementUnavailable) => {
            Some(RemoteProbeChildResultKindV1::AcknowledgementUnavailable)
        }
        Some(ClipboardPostPermitResultV1::ProtocolRejected) => {
            Some(RemoteProbeChildResultKindV1::ProtocolRejected)
        }
        Some(ClipboardPostPermitResultV1::Unknown) => Some(RemoteProbeChildResultKindV1::Unknown),
        Some(ClipboardPostPermitResultV1::RecordedOnly) | None => None,
        Some(ClipboardPostPermitResultV1::Allowed) => {
            Some(RemoteProbeChildResultKindV1::ProtocolRejected)
        }
    };
    let mut result_kind = permit_kind.unwrap_or_else(|| {
        if attempt.result.is_ok() {
            RemoteProbeChildResultKindV1::Completed
        } else {
            named_remote_probe_failure_kind(&snapshot)
        }
    });

    let count = callback_count.get();
    for stage in [
        ClipboardMarkerCheckStageV1::PostCallback,
        ClipboardMarkerCheckStageV1::PreRestore,
    ] {
        match snapshot.marker_is_current(&effects, &marker, stage, count) {
            Ok(_) => {}
            Err(_) if result_kind == RemoteProbeChildResultKindV1::Completed => {
                result_kind = named_remote_probe_failure_kind(&snapshot);
            }
            Err(_) => {}
        }
    }

    let restore = match snapshot.restore_if_unchanged(&effects, Some(&marker)) {
        Ok(ClipboardRestoreAttemptV1::Deferred) => RemoteProbeChildRestoreV1::Deferred,
        Ok(
            ClipboardRestoreAttemptV1::NotAttempted | ClipboardRestoreAttemptV1::OwnershipChanged,
        )
        | Ok(ClipboardRestoreAttemptV1::Unknown)
        | Ok(ClipboardRestoreAttemptV1::Restored)
        | Err(_) => RemoteProbeChildRestoreV1::Unknown,
    };
    if restore == RemoteProbeChildRestoreV1::Unknown
        && result_kind == RemoteProbeChildResultKindV1::Completed
    {
        result_kind = RemoteProbeChildResultKindV1::Unknown;
    }
    if case == RegisteredNamedRemoteClipboardCaseV1::EofChildAlive
        && restore == RemoteProbeChildRestoreV1::Deferred
    {
        return named_remote_probe_result(case, result_kind, count, restore);
    }
    match input.client.request(
        input.lease_id,
        ClipboardGuardOperationV1::Close {
            child_callback_count: count,
            shortcut_sent: false,
            child_restore: restore,
        },
    ) {
        Ok(ClipboardGuardRpcOutcomeV1::CloseAcknowledged) => {}
        Err(ClipboardGuardClientErrorV1::AcknowledgementUnavailable) => {
            result_kind = RemoteProbeChildResultKindV1::AcknowledgementUnavailable;
        }
        Ok(_) => result_kind = RemoteProbeChildResultKindV1::ProtocolRejected,
        Err(
            ClipboardGuardClientErrorV1::FrameOverBudget
            | ClipboardGuardClientErrorV1::MalformedFrame
            | ClipboardGuardClientErrorV1::VersionMismatch
            | ClipboardGuardClientErrorV1::LeaseMismatch
            | ClipboardGuardClientErrorV1::SequenceMismatch
            | ClipboardGuardClientErrorV1::UnexpectedReply,
        ) => {
            result_kind = RemoteProbeChildResultKindV1::ProtocolRejected;
        }
        Err(ClipboardGuardClientErrorV1::Io | ClipboardGuardClientErrorV1::Closed) => {
            result_kind = RemoteProbeChildResultKindV1::Unknown;
        }
    }
    named_remote_probe_result(case, result_kind, count, restore)
}

fn check_before_clipboard(cancellation: &CancellationToken) -> Result<(), PasteError> {
    if cancellation.is_cancelled() || !crate::research_trace::allowed() {
        Err(PasteError::Cancelled)
    } else {
        Ok(())
    }
}

fn classify_paste_verification(
    used_keyboard_paste: bool,
    expected_after: Option<&str>,
    after: Option<&str>,
    target_confirmed: bool,
    focused_field_confirmed: bool,
) -> PasteVerificationFact {
    if !used_keyboard_paste {
        return PasteVerificationFact::NotAttempted;
    }
    if !target_confirmed {
        return PasteVerificationFact::TargetUnconfirmed;
    }
    let Some(expected) = expected_after else {
        return PasteVerificationFact::ExpectedSpliceUnavailable;
    };
    if !focused_field_confirmed {
        return PasteVerificationFact::FocusedFieldUnconfirmed;
    }
    let Some(actual) = after else {
        return PasteVerificationFact::ReadbackUnavailable;
    };
    if expected != actual {
        return PasteVerificationFact::ReadbackMismatch;
    }
    PasteVerificationFact::Verified
}

/// The splice reproduces the pre-paste value (selection replaced by identical
/// text), so AXValue equality cannot show that the paste landed. Shared by the
/// readback loop and the classifier so the two cannot drift.
fn is_identity_replacement(expected_after: Option<&str>, value_before: Option<&str>) -> bool {
    expected_after.is_some() && expected_after == value_before
}

pub(crate) fn classify_confirmed_paste_readback(
    cancellation_current: bool,
    proof: Option<PasteVerificationFact>,
    expected_after: Option<&str>,
    value_before: Option<&str>,
    value_after: Option<&str>,
    target_confirmed: bool,
    focused_field_confirmed: bool,
) -> PasteVerificationFact {
    // A late cancellation wins even if a native read returned a matching
    // value. The shortcut is irreversible, so keep the outcome unknown and
    // let the existing History/recovery path handle it without replay.
    if !cancellation_current {
        return PasteVerificationFact::ShortcutOutcomeUnknown;
    }
    match (proof, target_confirmed, focused_field_confirmed) {
        (Some(PasteVerificationFact::VerifiedFullRange), true, true) => {
            PasteVerificationFact::VerifiedFullRange
        }
        _ => {
            let fact = classify_paste_verification(
                true,
                expected_after,
                value_after,
                target_confirmed,
                focused_field_confirmed,
            );
            // Identity replacement: AXValue equality is not evidence, so a
            // would-be `Verified` without the caret proof stays unconfirmed.
            if fact == PasteVerificationFact::Verified
                && is_identity_replacement(expected_after, value_before)
            {
                PasteVerificationFact::ReadbackUnavailable
            } else {
                fact
            }
        }
    }
}

pub(crate) struct PasteReadbackConfirmation {
    pub(crate) value_after: Option<String>,
    pub(crate) proof: Option<PasteVerificationFact>,
    poll_count: u8,
    expected_utf16_length: Option<u32>,
    actual_utf16_length: Option<u32>,
    mismatch_kind: Option<&'static str>,
    full_range_attempted: bool,
    elapsed: Duration,
}

#[derive(Clone, Copy)]
pub(crate) struct PasteReadbackExpectation<'a> {
    pub(crate) expected_after: Option<&'a str>,
    pub(crate) value_before: Option<&'a str>,
    pub(crate) insertion_end_caret_utf16: Option<u64>,
}

/// Observe the same retained field after one already-posted paste shortcut.
/// The caller owns target/field guards; this consumer never retries injection.
/// Exact AXValue is the first and immediate proof, except for identity
/// replacement (expected == pre-paste value), where it proves nothing. One
/// full-range fallback is attempted only after a concrete changed-value
/// mismatch, or after the original fast polling window when AXValue is
/// unavailable. Identity replacement instead gets up to three caret-proof
/// attempts (first, fifth and last poll) and stops when the range is
/// unavailable.
pub(crate) fn confirm_keyboard_paste_readback(
    expectation: PasteReadbackExpectation<'_>,
    cancellation: &CancellationToken,
    mut current_guard: impl FnMut() -> (bool, bool),
    mut read_ax_value: impl FnMut() -> Option<String>,
    mut read_full_range: impl FnMut(&str, u64) -> FullRangeReadback,
    mut wait: impl FnMut(Duration),
) -> PasteReadbackConfirmation {
    let PasteReadbackExpectation {
        expected_after,
        value_before,
        insertion_end_caret_utf16,
    } = expectation;
    // The expected splice is meaningful only when the original complete field
    // value was observed. A later value or alternate channel cannot replace
    // missing pre-injection evidence.
    let expected_after = expected_after.filter(|_| value_before.is_some());
    let started = std::time::Instant::now();
    let expected_utf16_length =
        expected_after.and_then(|value| u32::try_from(value.encode_utf16().count()).ok());
    let schedule = if expected_after.is_some() {
        &POST_PASTE_READBACK_POLL_DELAYS_MS[..]
    } else {
        &POST_PASTE_READBACK_POLL_DELAYS_MS[..=4]
    };
    let mut value_after = None;
    let mut proof = None;
    let mut poll_count = 0u8;
    let mut mismatch_kind = None;
    let mut full_range_attempted = false;
    // Replacing a selection with identical text leaves AXValue unchanged, so
    // equality cannot prove the paste landed. Only the caret proof can.
    let identity_replacement = is_identity_replacement(expected_after, value_before);
    let last_poll = schedule.len() - 1;

    for (poll, delay_ms) in schedule.iter().copied().enumerate() {
        if delay_ms > 0 {
            wait(Duration::from_millis(delay_ms));
        }
        if cancellation.is_cancelled() {
            break;
        }
        let (target_current, field_current) = current_guard();
        if !target_current || !field_current {
            break;
        }
        if cancellation.is_cancelled() {
            break;
        }
        value_after = read_ax_value();
        poll_count = poll_count.saturating_add(1);
        if cancellation.is_cancelled() {
            break;
        }

        if !identity_replacement
            && expected_after.is_some()
            && value_after.as_deref() == expected_after
        {
            proof = Some(PasteVerificationFact::Verified);
            break;
        }

        if let (Some(expected), Some(actual)) = (expected_after, value_after.as_deref()) {
            if value_before != Some(actual) {
                mismatch_kind = Some(
                    if actual.encode_utf16().count() == expected.encode_utf16().count() {
                        "same_length_mismatch"
                    } else {
                        "length_mismatch"
                    },
                );
            }
        } else if value_after.is_none() {
            mismatch_kind = Some("value_unavailable");
        }

        let concrete_changed_mismatch = expected_after.is_some()
            && value_after.as_deref().is_some_and(|actual| {
                Some(actual) != value_before && Some(actual) != expected_after
            });
        let unavailable_after_initial_window =
            expected_after.is_some() && value_after.is_none() && poll == 4;
        if let (Some(expected), Some(expected_caret)) = (
            expected_after,
            insertion_end_caret_utf16.filter(|_| {
                if identity_replacement {
                    // Bounded caret-proof attempts: first, mid, and last poll.
                    matches!(poll, 0 | 4) || poll == last_poll
                } else {
                    !full_range_attempted
                        && (concrete_changed_mismatch || unavailable_after_initial_window)
                }
            }),
        ) {
            full_range_attempted = true;
            let (target_before_range, field_before_range) = current_guard();
            if cancellation.is_cancelled() || !target_before_range || !field_before_range {
                break;
            }
            let range_result = read_full_range(expected, expected_caret);
            let (target_after_range, field_after_range) = current_guard();
            if cancellation.is_cancelled() || !target_after_range || !field_after_range {
                break;
            }
            if range_result == FullRangeReadback::Verified {
                proof = Some(PasteVerificationFact::VerifiedFullRange);
                break;
            }
            if range_result == FullRangeReadback::Unavailable {
                mismatch_kind.get_or_insert("full_range_unavailable");
                if identity_replacement {
                    // Nothing later can prove an identity replacement.
                    break;
                }
            } else {
                mismatch_kind.get_or_insert("full_range_mismatch");
            }
        }
    }

    if identity_replacement
        && proof.is_none()
        && matches!(mismatch_kind, None | Some("full_range_mismatch"))
    {
        mismatch_kind = Some("identity_unproven");
    }
    let actual_utf16_length = value_after
        .as_deref()
        .and_then(|value| u32::try_from(value.encode_utf16().count()).ok());
    PasteReadbackConfirmation {
        value_after,
        proof,
        poll_count,
        expected_utf16_length,
        actual_utf16_length,
        mismatch_kind,
        full_range_attempted,
        elapsed: started.elapsed(),
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

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum AxReadbackState {
    Applied,
    Unchanged,
    Uncertain,
}

#[cfg(test)]
fn classify_ax_readback(
    before: &str,
    expected_after: &str,
    actual_after: Option<&str>,
) -> AxReadbackState {
    match actual_after {
        Some(actual) if actual == expected_after => AxReadbackState::Applied,
        Some(actual) if actual == before => AxReadbackState::Unchanged,
        _ => AxReadbackState::Uncertain,
    }
}

#[cfg(test)]
fn classify_ax_insert_result(
    before: &str,
    expected_after: &str,
    actual_after: Option<String>,
    setter_reported_success: bool,
    range_before: Option<(i64, i64)>,
    range_after: Option<(i64, i64)>,
) -> AxInsertAttempt {
    match classify_ax_readback(before, expected_after, actual_after.as_deref()) {
        AxReadbackState::Applied => {
            AxInsertAttempt::Verified(actual_after.expect("applied readback contains a value"))
        }
        AxReadbackState::Unchanged
            if !setter_reported_success
                && range_before.is_some()
                && range_after == range_before =>
        {
            AxInsertAttempt::NoOp
        }
        AxReadbackState::Unchanged | AxReadbackState::Uncertain => AxInsertAttempt::Uncertain,
    }
}

fn classify_ax_full_field_write(
    before: &str,
    expected_after: &str,
    actual_after: Option<String>,
    setter_reported_success: bool,
) -> AxInsertAttempt {
    match actual_after.as_deref() {
        Some(actual) if actual == expected_after => AxInsertAttempt::Verified(actual.to_owned()),
        Some(actual) if actual == before && !setter_reported_success => AxInsertAttempt::NoOp,
        _ => AxInsertAttempt::Uncertain,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AxInsertAttempt {
    NotAttempted,
    NoOp,
    Verified(String),
    Uncertain,
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

fn expected_splice_after_insert(
    before: Option<&str>,
    selected_range: Option<(i64, i64)>,
    insert: &str,
) -> Option<String> {
    let before = before?;
    let selected_range = selected_range.or_else(|| before.is_empty().then_some((0, 0)))?;
    utf16_splice(before, selected_range.0, selected_range.1, insert)
}

fn expected_insert_caret_utf16(
    before: Option<&str>,
    selected_range: Option<(i64, i64)>,
    insert: &str,
) -> Option<u64> {
    let before = before?;
    let (location, _) = selected_range.or_else(|| before.is_empty().then_some((0, 0)))?;
    // Collapsed carets and empty fields are valid splice points; validate with
    // the same splice used for the expected value (`String::from_utf16`
    // rejects a splice that leaves an unpaired surrogate).
    expected_splice_after_insert(Some(before), selected_range, insert)?;
    u64::try_from(location)
        .ok()?
        .checked_add(u64::try_from(insert.encode_utf16().count()).ok()?)
}

fn confirmed_insertion_v1(
    source_field: &str,
    selected_range: Option<(i64, i64)>,
    inserted_text: &str,
    inserted_field: &str,
    target_identity: crate::context::TargetAppGuard,
) -> Option<ConfirmedInsertionV1> {
    let replaced_range = selected_range.or_else(|| source_field.is_empty().then_some((0, 0)))?;
    let (source_start, source_replaced_length) = replaced_range;
    let start = u32::try_from(source_start).ok()?;
    let replaced_length = u32::try_from(source_replaced_length).ok()?;
    let source_length = u32::try_from(source_field.encode_utf16().count()).ok()?;
    let inserted_length = u32::try_from(inserted_text.encode_utf16().count()).ok()?;
    let inserted_field_length = u32::try_from(inserted_field.encode_utf16().count()).ok()?;
    let source_end = start.checked_add(replaced_length)?;
    let inserted_end = start.checked_add(inserted_length)?;
    let expected_after_length = source_length
        .checked_sub(replaced_length)?
        .checked_add(inserted_length)?;

    if inserted_text.is_empty()
        || expected_splice_after_insert(Some(source_field), Some(replaced_range), inserted_text)
            .as_deref()
            != Some(inserted_field)
        || source_end > source_length
        || inserted_end > inserted_field_length
        || expected_after_length != inserted_field_length
        || utf16_range_to_bytes(source_field, i64::from(start), i64::from(replaced_length))
            .is_none()
        || utf16_range_to_bytes(inserted_field, i64::from(start), i64::from(inserted_length))
            .is_none()
    {
        return None;
    }

    Some(ConfirmedInsertionV1 {
        anchor_owner_id: 0,
        created_at: std::time::Instant::now(),
        source_field_utf16_length: source_length,
        inserted_field_utf16_length: inserted_field_length,
        replaced_range_before: Utf16RangeV1 {
            start,
            length: replaced_length,
        },
        inserted_range_after: Utf16RangeV1 {
            start,
            length: inserted_length,
        },
        inserted_text_utf16_length: inserted_length,
        target_identity,
        readback: PasteVerificationFact::Verified,
    })
}

/// Recheck only AX facts that were available before clipboard staging. A
/// missing fact remains unknown; a fact that was known and then changes makes
/// the one-shot paste unsafe to send.
fn verify_known_focus_snapshot(
    expected_value: Option<&str>,
    current_value: Option<&str>,
    expected_selection: Option<(i64, i64)>,
    current_selection: Option<(i64, i64)>,
) -> Result<(), PasteError> {
    if let Some(expected) = expected_value {
        if current_value != Some(expected) {
            return Err(PasteError::InputChanged);
        }
    }
    if let Some(expected) = expected_selection {
        if current_selection != Some(expected) {
            return Err(PasteError::SelectionChanged);
        }
    }
    Ok(())
}

fn verify_focused_field_before_paste(
    anchor: Option<&FocusedFieldAnchor>,
    expected_value: Option<&str>,
    expected_selection: Option<(i64, i64)>,
) -> Result<(), PasteError> {
    let Some(anchor) = anchor else {
        // Preserve the existing keyboard-paste fallback when AX cannot supply
        // an anchor at all. The clipboard and target owners still fail closed.
        return Ok(());
    };
    if !anchor.is_current_focus() {
        return Err(PasteError::InputChanged);
    }
    let current_value = anchor.read_value();
    let current_selection = anchor.read_selected_range();
    verify_known_focus_snapshot(
        expected_value,
        current_value.as_deref(),
        expected_selection,
        current_selection,
    )
}

#[cfg(test)]
fn try_ax_insert_if_safe(
    cancellation: &CancellationToken,
    verify_target: &impl Fn() -> Result<(), PasteError>,
    verify_after: &impl Fn() -> Result<(), PasteError>,
    expected_pid: i32,
    expected_anchor: Option<&FocusedFieldAnchor>,
    text: &str,
) -> AxInsertAttempt {
    if cancellation.is_cancelled() || verify_target().is_err() || text.is_empty() {
        return AxInsertAttempt::NotAttempted;
    }
    let Some(expected_anchor) = expected_anchor.filter(|anchor| anchor.is_current_focus()) else {
        return AxInsertAttempt::NotAttempted;
    };
    let result = try_ax_insert(
        expected_pid,
        expected_anchor,
        text,
        cancellation,
        verify_target,
    );
    match result {
        AxInsertAttempt::Verified(value_after) if verify_after().is_ok() => {
            AxInsertAttempt::Verified(value_after)
        }
        AxInsertAttempt::Verified(_) => AxInsertAttempt::Uncertain,
        other => other,
    }
}

#[cfg(all(test, not(target_os = "macos")))]
fn try_ax_insert(
    _expected_pid: i32,
    _expected_anchor: &FocusedFieldAnchor,
    _text: &str,
    _cancellation: &CancellationToken,
    _verify_target: &impl Fn() -> Result<(), PasteError>,
) -> AxInsertAttempt {
    AxInsertAttempt::NotAttempted
}

#[cfg(all(test, target_os = "macos"))]
fn try_ax_insert(
    expected_pid: i32,
    expected_anchor: &FocusedFieldAnchor,
    text: &str,
    cancellation: &CancellationToken,
    verify_target: &impl Fn() -> Result<(), PasteError>,
) -> AxInsertAttempt {
    macos_ax::try_insert(
        expected_pid,
        &expected_anchor.inner,
        text,
        cancellation,
        verify_target,
    )
}

#[cfg(target_os = "macos")]
mod macos_pasteboard {
    use super::PasteError;
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::{CStr, CString};
    use std::os::raw::c_char;

    const MAX_SNAPSHOT_BYTES: usize = 128 * 1024 * 1024;
    const TEXT_TYPE: &str = "public.utf8-plain-text";
    const OWNER_MARKER_TYPE: &str = "com.voiceflow.temporary-clipboard-owner";
    const TRANSIENT_TYPE: &str = "org.nspasteboard.TransientType";
    const AUTO_GENERATED_TYPE: &str = "org.nspasteboard.AutoGeneratedType";

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Flavor {
        name: String,
        bytes: Vec<u8>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct PasteboardItemSnapshot {
        flavors: Vec<Flavor>,
    }

    pub(super) struct Snapshot {
        pub(super) change_count: isize,
        pub(super) items: Vec<PasteboardItemSnapshot>,
    }

    struct SnapshotRestoration {
        restored: bool,
        association: Option<crate::research_trace::FinalClipboardAssociation>,
    }

    // Same framing as Clipboard.swift, over bytes ALREADY captured by the
    // product snapshot owner. No additional provider/body request is introduced.
    fn snapshot_fixture_identity(items: &[PasteboardItemSnapshot]) -> String {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update(b"voiceflow-clipboard-fixture-v1\0");
        hash.update((items.len() as u64).to_le_bytes());
        for item in items {
            hash.update((item.flavors.len() as u64).to_le_bytes());
            for flavor in &item.flavors {
                hash.update((flavor.name.len() as u64).to_le_bytes());
                hash.update(flavor.name.as_bytes());
                hash.update((flavor.bytes.len() as u64).to_le_bytes());
                hash.update(&flavor.bytes);
            }
        }
        hex::encode(hash.finalize())
    }

    impl Snapshot {
        // The same snapshot/protocol producer serves native restoration and
        // deterministic protocol fixtures. Commit facts are returned once;
        // no backend side field or second association interpreter is consulted.
        fn restore_using<Backend>(
            &self,
            backend: &mut Backend,
            proof: &super::ClipboardWriteProof,
        ) -> Result<SnapshotRestoration, PasteError>
        where
            Backend: super::ClipboardRestoreProtocol<[PasteboardItemSnapshot]>,
        {
            let commit = super::restore_clipboard_if_owned(backend, &self.items[..], Some(proof))?;
            let association = commit
                .generation
                .filter(|generation| {
                    commit.restored
                        && self.change_count.checked_add(1) == Some(proof.change_count)
                        && proof.change_count.checked_add(1) == Some(*generation)
                })
                .map(
                    |generation| crate::research_trace::FinalClipboardAssociation {
                        snapshot_fixture_identity: snapshot_fixture_identity(&self.items),
                        snapshot_generation: self.change_count.to_string(),
                        temporary_generation: proof.change_count.to_string(),
                        final_generation: generation.to_string(),
                        source: "macos_restore_owner_clear_write_sampled_generation_v1",
                    },
                );
            Ok(SnapshotRestoration {
                restored: commit.restored,
                association,
            })
        }

        pub(super) fn capture() -> Result<Self, PasteError> {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(capture_inner)).unwrap_or_else(
                |_| Err(PasteError::Clipboard("pasteboard snapshot panicked".into())),
            )
        }

        pub(super) fn restore_if_owned(
            &self,
            proof: &super::ClipboardWriteProof,
        ) -> Result<bool, PasteError> {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
                let pasteboard = general_pasteboard()?;
                let mut backend = MacRestoreProtocol { pasteboard };
                let restored = self.restore_using(&mut backend, proof)?;
                if let (Some(trace), Some(association)) =
                    (crate::research_trace::current(), restored.association)
                {
                    trace.clipboard_restored_fixture(association);
                }
                Ok(restored.restored)
            }))
            .unwrap_or_else(|_| Err(PasteError::Clipboard("pasteboard restore panicked".into())))
        }
    }

    pub(super) fn write_temporary_text(
        text: &str,
        ownership_token: &str,
        previous_change_count: isize,
        original_items: &[PasteboardItemSnapshot],
    ) -> Result<(), PasteError> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            write_temporary_text_inner(text, ownership_token, previous_change_count, original_items)
        }))
        .unwrap_or_else(|_| {
            Err(PasteError::Clipboard(
                "temporary pasteboard write panicked".into(),
            ))
        })
    }

    pub(super) fn write_temporary_text_for_retry(
        text: &str,
        ownership_token: &str,
        previous_change_count: isize,
        original_items: &[PasteboardItemSnapshot],
    ) -> super::RetryClipboardOutcome {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let pasteboard = general_pasteboard().map_err(TemporaryWriteFailure::unverified)?;
            write_temporary_to_with_outcome(
                pasteboard,
                text,
                ownership_token,
                previous_change_count,
                original_items,
            )
        })) {
            Ok(Ok(())) => super::RetryClipboardOutcome::Copied,
            Ok(Err(failure)) => failure.outcome,
            Err(_) => super::RetryClipboardOutcome::OwnershipUnverified,
        }
    }

    pub(super) fn observe_write() -> Result<super::ClipboardWriteObservation, PasteError> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(observe_write_inner)).unwrap_or_else(
            |_| {
                Err(PasteError::Clipboard(
                    "pasteboard ownership check panicked".into(),
                ))
            },
        )
    }

    #[cfg(all(test, feature = "research-harness"))]
    pub(super) fn observe_general_write_with_failure(
        fail_read: bool,
    ) -> Result<super::ClipboardWriteObservation, PasteError> {
        if fail_read {
            return Err(PasteError::Clipboard(
                "injected General pasteboard read failure".into(),
            ));
        }
        observe_write()
    }

    pub(super) fn change_count() -> Result<isize, PasteError> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let pasteboard = general_pasteboard()?;
            let count: isize = msg_send![pasteboard, changeCount];
            Ok(count)
        }))
        .unwrap_or_else(|_| {
            Err(PasteError::Clipboard(
                "pasteboard change-count read panicked".into(),
            ))
        })
    }

    #[cfg(feature = "research-harness")]
    pub(super) fn capture_general_for_guard() -> Result<Snapshot, super::ClipboardGuardReplyV1> {
        // This owner captures directly from AppKit and does not use
        // ClipboardSnapshot::capture or the research-trace fixture identity.
        let capture = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let pasteboard = general_pasteboard()?;
            capture_from(pasteboard)
        }));
        match capture {
            Ok(Ok(snapshot)) => Ok(snapshot),
            Ok(Err(error)) => Err(general_capture_failure_reply(&error)),
            Err(_) => Err(super::ClipboardGuardReplyV1::CaptureUnknown),
        }
    }

    #[cfg(feature = "research-harness")]
    fn general_capture_failure_reply(error: &PasteError) -> super::ClipboardGuardReplyV1 {
        match error {
            PasteError::Clipboard(reason)
                if reason == "pasteboard is too large to preserve safely" =>
            {
                super::ClipboardGuardReplyV1::CaptureOverBudget
            }
            _ => super::ClipboardGuardReplyV1::CaptureUnknown,
        }
    }

    #[cfg(feature = "research-harness")]
    pub(super) fn capture_named_fixture(
        seed: super::ClipboardSeedFixtureV1,
    ) -> Result<(String, Snapshot), super::ClipboardGuardReplyV1> {
        let items = research_seed_items(seed)?;
        let (named_board, pasteboard) = unsafe {
            unique_research_pasteboard()
                .map_err(|_| super::ClipboardGuardReplyV1::CaptureUnknown)?
        };
        let seed_write = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            write_seed_items_to(pasteboard, &items)
        }));
        if !matches!(seed_write, Ok(Ok(()))) {
            unsafe { release_named_pasteboard(pasteboard) };
            return Err(super::ClipboardGuardReplyV1::CaptureUnknown);
        }
        let snapshot = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            capture_from(pasteboard)
        })) {
            Ok(Ok(snapshot)) => snapshot,
            Ok(Err(_)) | Err(_) => {
                unsafe { release_named_pasteboard(pasteboard) };
                return Err(super::ClipboardGuardReplyV1::CaptureUnknown);
            }
        };
        if snapshot.items != items {
            unsafe { release_named_pasteboard(pasteboard) };
            return Err(super::ClipboardGuardReplyV1::CaptureUnknown);
        }
        Ok((named_board, snapshot))
    }

    #[cfg(feature = "research-harness")]
    pub(super) fn unchanged_since_capture(
        named_board: &str,
        snapshot: &Snapshot,
    ) -> Result<bool, PasteError> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let pasteboard = named_research_pasteboard(named_board)?;
            let count: isize = msg_send![pasteboard, changeCount];
            Ok(count == snapshot.change_count)
        }))
        .unwrap_or_else(|_| {
            Err(PasteError::Clipboard(
                "named pasteboard change-count read panicked".into(),
            ))
        })
    }

    #[cfg(feature = "research-harness")]
    pub(super) fn registered_temporary_text() -> &'static str {
        "VoiceFlow research synthetic clipboard payload"
    }

    #[cfg(feature = "research-harness")]
    pub(super) fn registered_general_temporary_text() -> &'static str {
        "VoiceFlo"
    }

    #[cfg(feature = "research-harness")]
    pub(super) fn write_named_temporary_text(
        named_board: &str,
        snapshot: &Snapshot,
        text: &str,
        ownership_token: &str,
    ) -> Result<(), PasteError> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let pasteboard = named_research_pasteboard(named_board)?;
            write_temporary_to_with_outcome(
                pasteboard,
                text,
                ownership_token,
                snapshot.change_count,
                &snapshot.items,
            )
            .map_err(|failure| failure.error)
        }))
        .unwrap_or_else(|_| {
            Err(PasteError::Clipboard(
                "named temporary pasteboard write panicked".into(),
            ))
        })
    }

    #[cfg(feature = "research-harness")]
    pub(super) fn observe_named_write(
        named_board: &str,
    ) -> Result<super::ClipboardWriteObservation, PasteError> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let pasteboard = named_research_pasteboard(named_board)?;
            observe_write_from(pasteboard)
        }))
        .unwrap_or_else(|_| {
            Err(PasteError::Clipboard(
                "named pasteboard ownership check panicked".into(),
            ))
        })
    }

    #[cfg(all(test, feature = "research-harness"))]
    pub(super) fn observe_named_write_with_failure(
        named_board: &str,
        fail_read: bool,
    ) -> Result<super::ClipboardWriteObservation, PasteError> {
        if fail_read {
            return Err(PasteError::Clipboard(
                "injected named pasteboard read failure".into(),
            ));
        }
        observe_named_write(named_board)
    }

    #[cfg(all(test, feature = "research-harness"))]
    pub(super) fn marker_is_current(
        named_board: &str,
        proof: &super::ClipboardWriteProof,
    ) -> Result<bool, PasteError> {
        let observation = observe_named_write(named_board)?;
        Ok(super::clipboard_write_proof_is_current(proof, &observation))
    }

    #[cfg(all(not(test), feature = "research-harness"))]
    pub(super) fn restore_named_if_owned(
        named_board: &str,
        snapshot: &Snapshot,
        proof: &super::ClipboardWriteProof,
    ) -> Result<super::ResearchRestoreOutcome, PasteError> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let pasteboard = named_research_pasteboard(named_board)?;
            let mut backend = super::RecordingRestoreObservation {
                inner: MacRestoreProtocol { pasteboard },
                observation: None,
                #[cfg(test)]
                observation_override: None,
            };
            let commit =
                super::restore_clipboard_if_owned(&mut backend, &snapshot.items[..], Some(proof))?;
            Ok(super::research_restore_outcome(
                &commit,
                backend.observation.as_ref(),
                proof,
            ))
        }))
        .unwrap_or_else(|_| {
            Err(PasteError::Clipboard(
                "named pasteboard restore panicked".into(),
            ))
        })
    }

    #[cfg(all(test, feature = "research-harness"))]
    pub(super) fn restore_named_if_owned_with_observation(
        named_board: &str,
        snapshot: &Snapshot,
        proof: &super::ClipboardWriteProof,
        observation_override: super::ClipboardWriteObservation,
    ) -> Result<super::ResearchRestoreOutcome, PasteError> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let pasteboard = named_research_pasteboard(named_board)?;
            let mut backend = super::RecordingRestoreObservation {
                inner: MacRestoreProtocol { pasteboard },
                observation: None,
                observation_override: Some(observation_override),
            };
            let commit =
                super::restore_clipboard_if_owned(&mut backend, &snapshot.items[..], Some(proof))?;
            Ok(super::research_restore_outcome(
                &commit,
                backend.observation.as_ref(),
                proof,
            ))
        }))
        .unwrap_or_else(|_| {
            Err(PasteError::Clipboard(
                "named pasteboard restore panicked".into(),
            ))
        })
    }

    #[cfg(all(not(test), feature = "research-harness"))]
    pub(super) fn restore_general_if_owned(
        snapshot: &Snapshot,
        proof: &super::ClipboardWriteProof,
    ) -> Result<super::ResearchRestoreOutcome, PasteError> {
        // Keep the General restoration on the shared ownership protocol while
        // avoiding Snapshot::restore_using's research-trace identity path.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let pasteboard = general_pasteboard()?;
            let mut backend = super::RecordingRestoreObservation {
                inner: MacRestoreProtocol { pasteboard },
                observation: None,
                #[cfg(test)]
                observation_override: None,
            };
            let commit =
                super::restore_clipboard_if_owned(&mut backend, &snapshot.items[..], Some(proof))?;
            Ok(super::research_restore_outcome(
                &commit,
                backend.observation.as_ref(),
                proof,
            ))
        }))
        .unwrap_or_else(|_| {
            Err(PasteError::Clipboard(
                "General pasteboard restore panicked".into(),
            ))
        })
    }

    #[cfg(all(test, feature = "research-harness"))]
    pub(super) fn restore_named_if_owned_with_prepare_failure(
        named_board: &str,
        snapshot: &Snapshot,
        proof: &super::ClipboardWriteProof,
        fail_prepare: bool,
    ) -> Result<bool, PasteError> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let pasteboard = named_research_pasteboard(named_board)?;
            let mut backend = RestorePrepareFailureProtocol {
                inner: Some(MacRestoreProtocol { pasteboard }),
                fail_prepare,
            };
            let commit =
                super::restore_clipboard_if_owned(&mut backend, &snapshot.items[..], Some(proof))?;
            Ok(commit.restored)
        }))
        .unwrap_or_else(|_| {
            Err(PasteError::Clipboard(
                "named pasteboard restore panicked".into(),
            ))
        })
    }

    #[cfg(all(test, feature = "research-harness"))]
    pub(super) fn restore_general_if_owned_with_prepare_failure(
        snapshot: &Snapshot,
        proof: &super::ClipboardWriteProof,
        fail_prepare: bool,
    ) -> Result<super::ResearchRestoreOutcome, PasteError> {
        if !fail_prepare {
            return Err(PasteError::Clipboard(
                "General test backend requires the injected failure".into(),
            ));
        }
        let mut backend = RestorePrepareFailureProtocol {
            inner: None,
            fail_prepare: true,
        };
        let commit =
            super::restore_clipboard_if_owned(&mut backend, &snapshot.items[..], Some(proof))?;
        Ok(super::research_restore_outcome(&commit, None, proof))
    }

    #[cfg(all(test, feature = "research-harness"))]
    pub(super) fn restore_general_if_owned_with_observation(
        snapshot: &Snapshot,
        proof: &super::ClipboardWriteProof,
        observation: super::ClipboardWriteObservation,
        commit_attempted: &mut bool,
    ) -> Result<super::ResearchRestoreOutcome, PasteError> {
        let inner = InjectedObservationRestoreProtocol {
            observation,
            commit_attempted,
        };
        let mut backend = super::RecordingRestoreObservation {
            inner,
            observation: None,
            observation_override: None,
        };
        let commit =
            super::restore_clipboard_if_owned(&mut backend, &snapshot.items[..], Some(proof))?;
        Ok(super::research_restore_outcome(
            &commit,
            backend.observation.as_ref(),
            proof,
        ))
    }

    #[cfg(feature = "research-harness")]
    pub(super) fn write_external_fixture(named_board: &str) -> Result<(), PasteError> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let items = research_external_items();
            let pasteboard = named_research_pasteboard(named_board)?;
            write_seed_items_to(pasteboard, &items)
        }))
        .unwrap_or_else(|_| {
            Err(PasteError::Clipboard(
                "named external-copy fixture write panicked".into(),
            ))
        })
    }

    #[cfg(feature = "research-harness")]
    pub(super) fn release_named_board(named_board: &str) -> Result<(), PasteError> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let pasteboard = named_research_pasteboard(named_board)?;
            release_named_pasteboard(pasteboard);
            Ok(())
        }))
        .unwrap_or_else(|_| {
            Err(PasteError::Clipboard(
                "named pasteboard release panicked".into(),
            ))
        })
    }

    #[cfg(feature = "research-harness")]
    fn research_seed_items(
        seed: super::ClipboardSeedFixtureV1,
    ) -> Result<Vec<PasteboardItemSnapshot>, super::ClipboardGuardReplyV1> {
        use super::ClipboardSeedFixtureV1;

        let flavor = |name: &str, bytes: &[u8]| Flavor {
            name: name.to_owned(),
            bytes: bytes.to_vec(),
        };
        let item = |flavors| PasteboardItemSnapshot { flavors };
        match seed {
            ClipboardSeedFixtureV1::Empty => Ok(Vec::new()),
            ClipboardSeedFixtureV1::OneItemMultiFlavor => Ok(vec![item(vec![
                flavor(TEXT_TYPE, b"research seed text one"),
                flavor(
                    "com.voiceflow.research.clipboard-guard-fixture",
                    b"research seed flavor one",
                ),
            ])]),
            ClipboardSeedFixtureV1::MultipleItems => Ok(vec![
                item(vec![
                    flavor(TEXT_TYPE, b"research seed item one"),
                    flavor(
                        "com.voiceflow.research.clipboard-guard-fixture",
                        b"research seed item one flavor",
                    ),
                ]),
                item(vec![
                    flavor(TEXT_TYPE, b"research seed item two"),
                    flavor(
                        "com.voiceflow.research.clipboard-guard-fixture",
                        b"research seed item two flavor",
                    ),
                ]),
            ]),
            ClipboardSeedFixtureV1::Over128MiB => {
                Err(super::ClipboardGuardReplyV1::CaptureOverBudget)
            }
        }
    }

    #[cfg(feature = "research-harness")]
    fn research_external_items() -> Vec<PasteboardItemSnapshot> {
        vec![PasteboardItemSnapshot {
            flavors: vec![Flavor {
                name: TEXT_TYPE.to_owned(),
                bytes: b"synthetic external copy".to_vec(),
            }],
        }]
    }

    fn snapshot_total_bytes_exceed_limit(total_bytes: usize, next_length: usize) -> bool {
        total_bytes.saturating_add(next_length) > MAX_SNAPSHOT_BYTES
    }

    fn capture_inner() -> Result<Snapshot, PasteError> {
        let pasteboard = unsafe { general_pasteboard()? };
        capture_from(pasteboard)
    }

    fn capture_from(pasteboard: *mut Object) -> Result<Snapshot, PasteError> {
        unsafe {
            let change_count: isize = msg_send![pasteboard, changeCount];
            let native_items: *mut Object = msg_send![pasteboard, pasteboardItems];
            if native_items.is_null() {
                let types: *mut Object = msg_send![pasteboard, types];
                if !types.is_null() {
                    let type_count: usize = msg_send![types, count];
                    if type_count > 0 {
                        return Err(PasteError::Clipboard(
                            "pasteboard items could not be preserved".into(),
                        ));
                    }
                }
                return Ok(Snapshot {
                    change_count,
                    items: Vec::new(),
                });
            }
            let item_count: usize = msg_send![native_items, count];
            let mut items = Vec::with_capacity(item_count);
            let mut total_bytes = 0usize;
            for item_index in 0..item_count {
                let native_item: *mut Object = msg_send![native_items, objectAtIndex: item_index];
                if native_item.is_null() {
                    return Err(PasteError::Clipboard(
                        "pasteboard item could not be read".into(),
                    ));
                }
                let types: *mut Object = msg_send![native_item, types];
                if types.is_null() {
                    return Err(PasteError::Clipboard(
                        "pasteboard item types unavailable".into(),
                    ));
                }
                let type_count: usize = msg_send![types, count];
                let mut flavors = Vec::with_capacity(type_count);
                for type_index in 0..type_count {
                    let type_object: *mut Object = msg_send![types, objectAtIndex: type_index];
                    let type_name = object_string(type_object)?;
                    let data: *mut Object = msg_send![native_item, dataForType: type_object];
                    if data.is_null() {
                        return Err(PasteError::Clipboard(format!(
                            "could not preserve pasteboard flavor {type_name}"
                        )));
                    }
                    let length: usize = msg_send![data, length];
                    if snapshot_total_bytes_exceed_limit(total_bytes, length) {
                        return Err(PasteError::Clipboard(
                            "pasteboard is too large to preserve safely".into(),
                        ));
                    }
                    total_bytes = total_bytes.saturating_add(length);
                    let bytes: *const u8 = msg_send![data, bytes];
                    if length > 0 && bytes.is_null() {
                        return Err(PasteError::Clipboard(format!(
                            "pasteboard flavor {type_name} could not be read"
                        )));
                    }
                    let owned = if length == 0 {
                        Vec::new()
                    } else {
                        std::slice::from_raw_parts(bytes, length).to_vec()
                    };
                    flavors.push(Flavor {
                        name: type_name,
                        bytes: owned,
                    });
                }
                items.push(PasteboardItemSnapshot { flavors });
            }
            let after_snapshot: isize = msg_send![pasteboard, changeCount];
            if after_snapshot != change_count {
                return Err(PasteError::Clipboard(
                    "pasteboard changed while it was being preserved".into(),
                ));
            }
            Ok(Snapshot {
                change_count,
                items,
            })
        }
    }

    struct PreparedNativeRestore {
        items: *mut Object,
        clear: bool,
    }

    impl Drop for PreparedNativeRestore {
        fn drop(&mut self) {
            if !self.items.is_null() {
                unsafe {
                    let _: () = msg_send![self.items, release];
                }
            }
        }
    }

    unsafe fn prepare_native_restore(
        items: &[PasteboardItemSnapshot],
    ) -> Result<PreparedNativeRestore, PasteError> {
        if items.is_empty() {
            return Ok(PreparedNativeRestore {
                items: std::ptr::null_mut(),
                clear: true,
            });
        }
        let native_items: *mut Object =
            msg_send![class!(NSMutableArray), arrayWithCapacity: items.len()];
        if native_items.is_null() {
            return Err(PasteError::Clipboard(
                "pasteboard item list allocation failed".into(),
            ));
        }
        let _: *mut Object = msg_send![native_items, retain];
        let prepared = PreparedNativeRestore {
            items: native_items,
            clear: false,
        };
        for item in items {
            let native_item = item_to_native(item)?;
            let _: () = msg_send![prepared.items, addObject: native_item];
            let _: () = msg_send![native_item, release];
        }
        Ok(prepared)
    }

    unsafe fn commit_native_restore(
        pasteboard: *mut Object,
        prepared: PreparedNativeRestore,
    ) -> Result<Option<isize>, PasteError> {
        // Ordinary launches gain no research generation queries or body reads.
        let observing = crate::research_trace::current().is_some();
        let before = observing.then(|| msg_send![pasteboard, changeCount]);
        let cleared: isize = msg_send![pasteboard, clearContents];
        if !prepared.clear {
            let restored: bool = msg_send![pasteboard, writeObjects: prepared.items];
            if !restored {
                return Err(PasteError::Clipboard(
                    "pasteboard items could not be restored".into(),
                ));
            }
        }
        let after = observing.then(|| msg_send![pasteboard, changeCount]);
        // Existing bool/error semantics are unchanged. An uncertain generation
        // supplies no observation association, with no retry or restoration.
        Ok(super::restore_commit_generation(before, cleared, after))
    }

    unsafe fn restore_declared_stage_if_owned(
        pasteboard: *mut Object,
        prepared: PreparedNativeRestore,
        declared_count: isize,
        expected_token: &str,
        expected_text: &str,
    ) -> Result<bool, PasteError> {
        let observation = observe_write_from(pasteboard)?;
        let token_is_ours = observation
            .token
            .as_deref()
            .is_none_or(|token| token == expected_token);
        let text_is_ours = observation
            .text
            .as_deref()
            .is_none_or(|text| text == expected_text);
        if observation.change_count != declared_count
            || !observation.has_exact_temporary_types
            || !token_is_ours
            || !text_is_ours
        {
            return Ok(false);
        }
        commit_native_restore(pasteboard, prepared)?;
        Ok(true)
    }

    struct MacRestoreProtocol {
        pasteboard: *mut Object,
    }

    #[cfg(all(test, feature = "research-harness"))]
    struct RestorePrepareFailureProtocol {
        inner: Option<MacRestoreProtocol>,
        fail_prepare: bool,
    }

    #[cfg(all(test, feature = "research-harness"))]
    struct InjectedObservationRestoreProtocol<'a> {
        observation: super::ClipboardWriteObservation,
        commit_attempted: &'a mut bool,
    }

    #[cfg(all(test, feature = "research-harness"))]
    impl super::ClipboardRestoreProtocol<[PasteboardItemSnapshot]>
        for InjectedObservationRestoreProtocol<'_>
    {
        type Prepared = ();

        fn prepare_restore(
            &mut self,
            _snapshot: &[PasteboardItemSnapshot],
        ) -> Result<Self::Prepared, PasteError> {
            Ok(())
        }

        fn observe(&mut self) -> Result<super::ClipboardWriteObservation, PasteError> {
            Ok(self.observation.clone())
        }

        fn commit_restore(
            &mut self,
            _prepared: Self::Prepared,
        ) -> Result<Option<isize>, PasteError> {
            *self.commit_attempted = true;
            Ok(None)
        }
    }

    #[cfg(all(test, feature = "research-harness"))]
    impl super::ClipboardRestoreProtocol<[PasteboardItemSnapshot]> for RestorePrepareFailureProtocol {
        type Prepared = PreparedNativeRestore;

        fn prepare_restore(
            &mut self,
            snapshot: &[PasteboardItemSnapshot],
        ) -> Result<Self::Prepared, PasteError> {
            if self.fail_prepare {
                return Err(PasteError::Clipboard(
                    "injected pasteboard restore preparation failure".into(),
                ));
            }
            let Some(inner) = self.inner.as_mut() else {
                return Err(PasteError::Clipboard(
                    "restore test backend has no native board".into(),
                ));
            };
            super::ClipboardRestoreProtocol::prepare_restore(inner, snapshot)
        }

        fn observe(&mut self) -> Result<super::ClipboardWriteObservation, PasteError> {
            let Some(inner) = self.inner.as_mut() else {
                return Err(PasteError::Clipboard(
                    "restore test backend has no native board".into(),
                ));
            };
            super::ClipboardRestoreProtocol::observe(inner)
        }

        fn commit_restore(
            &mut self,
            prepared: Self::Prepared,
        ) -> Result<Option<isize>, PasteError> {
            let Some(inner) = self.inner.as_mut() else {
                return Err(PasteError::Clipboard(
                    "restore test backend has no native board".into(),
                ));
            };
            super::ClipboardRestoreProtocol::commit_restore(inner, prepared)
        }
    }

    impl super::ClipboardRestoreProtocol<[PasteboardItemSnapshot]> for MacRestoreProtocol {
        type Prepared = PreparedNativeRestore;

        fn prepare_restore(
            &mut self,
            snapshot: &[PasteboardItemSnapshot],
        ) -> Result<Self::Prepared, PasteError> {
            unsafe { prepare_native_restore(snapshot) }
        }

        fn observe(&mut self) -> Result<super::ClipboardWriteObservation, PasteError> {
            observe_write_from(self.pasteboard)
        }

        fn commit_restore(
            &mut self,
            prepared: Self::Prepared,
        ) -> Result<Option<isize>, PasteError> {
            unsafe { commit_native_restore(self.pasteboard, prepared) }
        }
    }

    unsafe fn write_temporary_text_inner(
        text: &str,
        ownership_token: &str,
        previous_change_count: isize,
        original_items: &[PasteboardItemSnapshot],
    ) -> Result<(), PasteError> {
        let pasteboard = general_pasteboard()?;
        write_temporary_to(
            pasteboard,
            text,
            ownership_token,
            previous_change_count,
            original_items,
        )
    }

    unsafe fn write_temporary_to(
        pasteboard: *mut Object,
        text: &str,
        ownership_token: &str,
        previous_change_count: isize,
        original_items: &[PasteboardItemSnapshot],
    ) -> Result<(), PasteError> {
        write_temporary_to_with_outcome(
            pasteboard,
            text,
            ownership_token,
            previous_change_count,
            original_items,
        )
        .map_err(|failure| failure.error)
    }

    #[derive(Debug)]
    struct TemporaryWriteFailure {
        outcome: super::RetryClipboardOutcome,
        error: PasteError,
    }

    impl TemporaryWriteFailure {
        fn changed(error: PasteError) -> Self {
            Self {
                outcome: super::RetryClipboardOutcome::Changed,
                error,
            }
        }

        fn unverified(error: PasteError) -> Self {
            Self {
                outcome: super::RetryClipboardOutcome::OwnershipUnverified,
                error,
            }
        }

        fn write_failed(error: PasteError) -> Self {
            Self {
                outcome: super::RetryClipboardOutcome::WriteFailed,
                error,
            }
        }
    }

    impl From<PasteError> for TemporaryWriteFailure {
        fn from(error: PasteError) -> Self {
            Self::write_failed(error)
        }
    }

    unsafe fn write_temporary_to_with_outcome(
        pasteboard: *mut Object,
        text: &str,
        ownership_token: &str,
        previous_change_count: isize,
        original_items: &[PasteboardItemSnapshot],
    ) -> Result<(), TemporaryWriteFailure> {
        let before: isize = msg_send![pasteboard, changeCount];
        if before != previous_change_count {
            return Err(TemporaryWriteFailure::changed(PasteError::Clipboard(
                "clipboard changed before temporary paste".into(),
            )));
        }
        // Allocate all data/type objects and the recovery snapshot before
        // taking pasteboard ownership. This keeps failures after declaration
        // limited to the native setData calls, which can be rolled back
        // only while the returned declaration count and exact staged item
        // still prove that no newer clipboard was supplied.
        let prepared_original = prepare_native_restore(original_items)?;
        let text_type = string_object(TEXT_TYPE)?;
        let marker_type = string_object(OWNER_MARKER_TYPE)?;
        let transient_type = string_object(TRANSIENT_TYPE)?;
        let auto_generated_type = string_object(AUTO_GENERATED_TYPE)?;
        let native_types: *mut Object =
            msg_send![class!(NSMutableArray), arrayWithCapacity: 4usize];
        if native_types.is_null() {
            return Err(PasteError::Clipboard(
                "temporary pasteboard type list allocation failed".into(),
            )
            .into());
        }
        let _: () = msg_send![native_types, addObject: text_type];
        let _: () = msg_send![native_types, addObject: marker_type];
        let _: () = msg_send![native_types, addObject: transient_type];
        let _: () = msg_send![native_types, addObject: auto_generated_type];
        let text_data: *mut Object = msg_send![
            class!(NSData),
            dataWithBytes: text.as_ptr()
            length: text.len()
        ];
        let marker_data: *mut Object = msg_send![
            class!(NSData),
            dataWithBytes: ownership_token.as_ptr()
            length: ownership_token.len()
        ];
        let empty_data: *mut Object = msg_send![
            class!(NSData),
            dataWithBytes: super::TEMPORARY_CLIPBOARD_HINT_DATA.as_ptr()
            length: super::TEMPORARY_CLIPBOARD_HINT_DATA.len()
        ];
        if text_data.is_null() || marker_data.is_null() || empty_data.is_null() {
            return Err(PasteError::Clipboard(
                "temporary pasteboard data allocation failed".into(),
            )
            .into());
        }

        // AppKit `writeObjects:` can append items without changing
        // `changeCount`. Re-read every item/type/data after allocating all
        // fallible native objects, immediately before declaration, so an
        // appended clipboard item cannot be mistaken for the original A.
        // AppKit provides no compare-and-swap; another writer can still race
        // between this final observation and `declareTypes:`.
        let latest = capture_from(pasteboard).map_err(TemporaryWriteFailure::unverified)?;
        if latest.change_count != previous_change_count || latest.items != original_items {
            return Err(TemporaryWriteFailure::changed(PasteError::Clipboard(
                "clipboard contents changed before temporary write".into(),
            )));
        }

        let declared_count: isize = msg_send![
            pasteboard,
            declareTypes: native_types
            owner: std::ptr::null_mut::<Object>()
        ];
        let expected_count = before.checked_add(1).ok_or_else(|| {
            TemporaryWriteFailure::unverified(PasteError::Clipboard(
                "pasteboard change count is out of range".into(),
            ))
        })?;
        let after_declare: isize = msg_send![pasteboard, changeCount];
        if declared_count != expected_count || after_declare != declared_count {
            return Err(TemporaryWriteFailure::unverified(PasteError::Clipboard(
                "clipboard changed while declaring temporary contents".into(),
            )));
        }

        let marker_written: bool = msg_send![pasteboard, setData: marker_data forType: marker_type];
        let after_marker: isize = msg_send![pasteboard, changeCount];
        if !marker_written || after_marker != declared_count {
            let _ = restore_declared_stage_if_owned(
                pasteboard,
                prepared_original,
                declared_count,
                ownership_token,
                text,
            );
            let error = PasteError::Clipboard(
                "temporary pasteboard ownership marker could not be written".into(),
            );
            return Err(if after_marker == declared_count {
                TemporaryWriteFailure::write_failed(error)
            } else {
                TemporaryWriteFailure::unverified(error)
            });
        }

        let text_written: bool = msg_send![pasteboard, setData: text_data forType: text_type];
        let transient_written: bool =
            msg_send![pasteboard, setData: empty_data forType: transient_type];
        let auto_generated_written: bool =
            msg_send![pasteboard, setData: empty_data forType: auto_generated_type];
        let observation = match observe_write_from(pasteboard) {
            Ok(observation) => observation,
            Err(error) => {
                drop(prepared_original);
                return Err(TemporaryWriteFailure::unverified(error));
            }
        };
        if !text_written
            || !transient_written
            || !auto_generated_written
            || super::prove_clipboard_write_ownership(
                previous_change_count,
                text,
                ownership_token,
                &observation,
            )
            .is_none()
        {
            let _ = restore_declared_stage_if_owned(
                pasteboard,
                prepared_original,
                declared_count,
                ownership_token,
                text,
            );
            let error =
                PasteError::Clipboard("temporary pasteboard contents could not be verified".into());
            return Err(
                if !text_written || !transient_written || !auto_generated_written {
                    TemporaryWriteFailure::write_failed(error)
                } else {
                    TemporaryWriteFailure::unverified(error)
                },
            );
        }
        drop(prepared_original);
        Ok(())
    }

    fn observe_write_inner() -> Result<super::ClipboardWriteObservation, PasteError> {
        let pasteboard = unsafe { general_pasteboard()? };
        observe_write_from(pasteboard)
    }

    fn observe_write_from(
        pasteboard: *mut Object,
    ) -> Result<super::ClipboardWriteObservation, PasteError> {
        unsafe {
            let before: isize = msg_send![pasteboard, changeCount];
            let native_items: *mut Object = msg_send![pasteboard, pasteboardItems];
            let item_count: usize = if native_items.is_null() {
                0
            } else {
                msg_send![native_items, count]
            };
            let mut text = None;
            let mut token = None;
            let mut has_exact_temporary_types = false;
            if item_count == 1 {
                let native_item: *mut Object = msg_send![native_items, objectAtIndex: 0usize];
                if !native_item.is_null() {
                    let types: *mut Object = msg_send![native_item, types];
                    let type_count: usize = if types.is_null() {
                        0
                    } else {
                        msg_send![types, count]
                    };
                    if type_count == 4 {
                        let mut type_names = Vec::with_capacity(type_count);
                        for index in 0..type_count {
                            let type_object: *mut Object = msg_send![types, objectAtIndex: index];
                            type_names.push(object_string(type_object)?);
                        }
                        has_exact_temporary_types =
                            super::has_exact_temporary_type_names(&type_names);
                    }
                    if has_exact_temporary_types {
                        let text_type = string_object(TEXT_TYPE)?;
                        let marker_type = string_object(OWNER_MARKER_TYPE)?;
                        text = string_data_for_type(native_item, text_type);
                        token = string_data_for_type(native_item, marker_type);
                    }
                }
            }
            let after: isize = msg_send![pasteboard, changeCount];
            if before != after {
                return Err(PasteError::Clipboard(
                    "pasteboard changed while checking write ownership".into(),
                ));
            }
            Ok(super::ClipboardWriteObservation {
                change_count: after,
                text,
                token,
                has_exact_temporary_types,
            })
        }
    }

    unsafe fn string_data_for_type(item: *mut Object, type_object: *mut Object) -> Option<String> {
        let data: *mut Object = msg_send![item, dataForType: type_object];
        if data.is_null() {
            return None;
        }
        let length: usize = msg_send![data, length];
        if length > MAX_SNAPSHOT_BYTES {
            return None;
        }
        let bytes: *const u8 = msg_send![data, bytes];
        if length > 0 && bytes.is_null() {
            return None;
        }
        let bytes = if length == 0 {
            &[][..]
        } else {
            std::slice::from_raw_parts(bytes, length)
        };
        std::str::from_utf8(bytes).ok().map(str::to_owned)
    }

    unsafe fn item_to_native(item: &PasteboardItemSnapshot) -> Result<*mut Object, PasteError> {
        let native_item: *mut Object = msg_send![class!(NSPasteboardItem), new];
        if native_item.is_null() {
            return Err(PasteError::Clipboard(
                "pasteboard item allocation failed".into(),
            ));
        }
        for flavor in &item.flavors {
            let name = match c_string(&flavor.name) {
                Ok(name) => name,
                Err(error) => {
                    let _: () = msg_send![native_item, release];
                    return Err(error);
                }
            };
            let type_object: *mut Object =
                msg_send![class!(NSString), stringWithUTF8String: name.as_ptr()];
            if type_object.is_null() {
                let _: () = msg_send![native_item, release];
                return Err(PasteError::Clipboard(
                    "pasteboard type allocation failed".into(),
                ));
            }
            let data: *mut Object = msg_send![
                class!(NSData),
                dataWithBytes: flavor.bytes.as_ptr()
                length: flavor.bytes.len()
            ];
            if data.is_null() {
                let _: () = msg_send![native_item, release];
                return Err(PasteError::Clipboard(
                    "pasteboard flavor allocation failed".into(),
                ));
            }
            let restored: bool = msg_send![native_item, setData: data forType: type_object];
            if !restored {
                let _: () = msg_send![native_item, release];
                return Err(PasteError::Clipboard(format!(
                    "pasteboard flavor {} could not be restored",
                    flavor.name
                )));
            }
        }
        Ok(native_item)
    }

    unsafe fn string_object(value: &str) -> Result<*mut Object, PasteError> {
        let value = c_string(value)?;
        let object: *mut Object = msg_send![class!(NSString), stringWithUTF8String: value.as_ptr()];
        if object.is_null() {
            Err(PasteError::Clipboard(
                "pasteboard type allocation failed".into(),
            ))
        } else {
            Ok(object)
        }
    }

    unsafe fn general_pasteboard() -> Result<*mut Object, PasteError> {
        let pasteboard: *mut Object = msg_send![class!(NSPasteboard), generalPasteboard];
        if pasteboard.is_null() {
            Err(PasteError::Clipboard(
                "general pasteboard unavailable".into(),
            ))
        } else {
            Ok(pasteboard)
        }
    }

    #[cfg(feature = "research-harness")]
    unsafe fn unique_research_pasteboard() -> Result<(String, *mut Object), PasteError> {
        let pasteboard: *mut Object = msg_send![class!(NSPasteboard), pasteboardWithUniqueName];
        if pasteboard.is_null() {
            return Err(PasteError::Clipboard(
                "named research pasteboard unavailable".into(),
            ));
        }
        let name_object: *mut Object = msg_send![pasteboard, name];
        let name = match object_string(name_object) {
            Ok(name) => name,
            Err(error) => {
                release_named_pasteboard(pasteboard);
                return Err(error);
            }
        };
        Ok((name, pasteboard))
    }

    #[cfg(feature = "research-harness")]
    unsafe fn named_research_pasteboard(name: &str) -> Result<*mut Object, PasteError> {
        let name_object = string_object(name)?;
        let pasteboard: *mut Object =
            msg_send![class!(NSPasteboard), pasteboardWithName: name_object];
        if pasteboard.is_null() {
            Err(PasteError::Clipboard(
                "named research pasteboard unavailable".into(),
            ))
        } else {
            Ok(pasteboard)
        }
    }

    #[cfg(feature = "research-harness")]
    unsafe fn release_named_pasteboard(pasteboard: *mut Object) {
        let _: () = msg_send![pasteboard, releaseGlobally];
    }

    #[cfg(feature = "research-harness")]
    unsafe fn write_seed_items_to(
        pasteboard: *mut Object,
        items: &[PasteboardItemSnapshot],
    ) -> Result<(), PasteError> {
        let _: isize = msg_send![pasteboard, clearContents];
        if items.is_empty() {
            return Ok(());
        }
        let native_items: *mut Object =
            msg_send![class!(NSMutableArray), arrayWithCapacity: items.len()];
        if native_items.is_null() {
            let _: isize = msg_send![pasteboard, clearContents];
            return Err(PasteError::Clipboard(
                "named fixture item list unavailable".into(),
            ));
        }
        let mut owned_items = Vec::with_capacity(items.len());
        for item in items {
            match item_to_native(item) {
                Ok(native_item) => {
                    let _: () = msg_send![native_items, addObject: native_item];
                    owned_items.push(native_item);
                }
                Err(error) => {
                    for native_item in owned_items {
                        let _: () = msg_send![native_item, release];
                    }
                    let _: isize = msg_send![pasteboard, clearContents];
                    return Err(error);
                }
            }
        }
        let wrote: bool = msg_send![pasteboard, writeObjects: native_items];
        for native_item in owned_items {
            let _: () = msg_send![native_item, release];
        }
        if !wrote {
            let _: isize = msg_send![pasteboard, clearContents];
            return Err(PasteError::Clipboard(
                "named fixture items could not be written".into(),
            ));
        }
        Ok(())
    }

    unsafe fn object_string(value: *mut Object) -> Result<String, PasteError> {
        if value.is_null() {
            return Err(PasteError::Clipboard("pasteboard type was empty".into()));
        }
        let bytes: *const c_char = msg_send![value, UTF8String];
        if bytes.is_null() {
            return Err(PasteError::Clipboard(
                "pasteboard type was not UTF-8".into(),
            ));
        }
        CStr::from_ptr(bytes)
            .to_str()
            .map(str::to_owned)
            .map_err(|_| PasteError::Clipboard("pasteboard type was not UTF-8".into()))
    }

    fn c_string(value: &str) -> Result<CString, PasteError> {
        CString::new(value)
            .map_err(|_| PasteError::Clipboard("pasteboard type contains a null byte".into()))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::{AtomicU64, Ordering};

        #[test]
        fn snapshot_size_guard_accepts_limit_and_rejects_overflow() {
            assert!(!snapshot_total_bytes_exceed_limit(
                MAX_SNAPSHOT_BYTES - 1,
                1
            ));
            assert!(snapshot_total_bytes_exceed_limit(MAX_SNAPSHOT_BYTES, 1));
            assert!(snapshot_total_bytes_exceed_limit(usize::MAX, 1));
        }

        #[cfg(feature = "research-harness")]
        #[test]
        fn named_research_seeds_preserve_empty_single_and_multiple_item_shapes() {
            for (seed, expected_items, expected_flavors) in [
                (super::super::ClipboardSeedFixtureV1::Empty, 0, 0),
                (
                    super::super::ClipboardSeedFixtureV1::OneItemMultiFlavor,
                    1,
                    2,
                ),
                (super::super::ClipboardSeedFixtureV1::MultipleItems, 2, 2),
            ] {
                let (named_board, snapshot) = capture_named_fixture(seed).unwrap();
                assert_eq!(snapshot.items.len(), expected_items);
                assert!(snapshot
                    .items
                    .iter()
                    .all(|item| item.flavors.len() == expected_flavors));
                release_named_board(&named_board).unwrap();
            }
            assert!(matches!(
                capture_named_fixture(super::super::ClipboardSeedFixtureV1::Over128MiB),
                Err(super::super::ClipboardGuardReplyV1::CaptureOverBudget)
            ));
        }

        #[cfg(feature = "research-harness")]
        #[test]
        fn general_capture_failures_project_to_fixed_codes_only() {
            assert_eq!(
                general_capture_failure_reply(&PasteError::Clipboard(
                    "pasteboard is too large to preserve safely".into()
                )),
                super::super::ClipboardGuardReplyV1::CaptureOverBudget
            );
            assert_eq!(
                general_capture_failure_reply(&PasteError::Clipboard(
                    "could not preserve pasteboard flavor secret-type-name".into()
                )),
                super::super::ClipboardGuardReplyV1::CaptureUnknown
            );
        }

        #[cfg(feature = "research-harness")]
        #[test]
        fn named_research_owner_restores_only_its_proven_write() {
            for seed in [
                super::super::ClipboardSeedFixtureV1::Empty,
                super::super::ClipboardSeedFixtureV1::MultipleItems,
            ] {
                let mut owner =
                    super::super::ResearchClipboardGuardOwnerV1::capture_named_fixture(seed)
                        .unwrap();
                let original_items = owner.snapshot.items.clone();

                assert_eq!(
                    owner.write_fixture(
                        super::super::ClipboardWriteFixtureV1::RegisteredTemporaryTextV1
                    ),
                    super::super::ClipboardGuardReplyV1::WriteOwned
                );
                assert_eq!(
                    owner.current_ownership(),
                    super::super::ClipboardGuardReplyV1::WriteOwned
                );
                assert_eq!(
                    owner.restore_if_owned(),
                    super::super::ClipboardGuardReplyV1::Restored
                );

                let restored = unsafe {
                    capture_from(
                        named_research_pasteboard(owner.named_board_id().unwrap()).unwrap(),
                    )
                    .unwrap()
                };
                assert_eq!(restored.items, original_items);
                assert_eq!(
                    owner.close_after_restore(),
                    super::super::ClipboardGuardReplyV1::Restored
                );
                assert_eq!(
                    owner.current_ownership(),
                    super::super::ClipboardGuardReplyV1::Restored
                );
            }
        }

        #[cfg(feature = "research-harness")]
        #[test]
        fn named_research_owner_preserves_a_new_external_copy() {
            let mut owner = super::super::ResearchClipboardGuardOwnerV1::capture_named_fixture(
                super::super::ClipboardSeedFixtureV1::Empty,
            )
            .unwrap();
            let named_board = owner.named_board_id().unwrap().to_owned();

            assert_eq!(
                owner.write_fixture(
                    super::super::ClipboardWriteFixtureV1::RegisteredTemporaryTextV1
                ),
                super::super::ClipboardGuardReplyV1::WriteOwned
            );
            assert_eq!(
                owner.write_fixture(super::super::ClipboardWriteFixtureV1::ExternalCopyV1),
                super::super::ClipboardGuardReplyV1::OwnershipChanged
            );
            assert_eq!(
                owner.restore_if_owned(),
                super::super::ClipboardGuardReplyV1::OwnershipChanged
            );
            assert_eq!(
                owner.close_after_restore(),
                super::super::ClipboardGuardReplyV1::OwnershipChanged
            );
            let observed_external_copy =
                unsafe { capture_from(named_research_pasteboard(&named_board).unwrap()).unwrap() };
            assert_eq!(observed_external_copy.items, research_external_items());

            // Test cleanup releases only this synthetic named board after the
            // preservation assertion; the owner never restores over it.
            release_named_board(&named_board).unwrap();
        }

        #[cfg(feature = "research-harness")]
        #[test]
        fn prewrite_external_copy_is_latched_before_no_write_can_be_reported() {
            let mut owner = super::super::ResearchClipboardGuardOwnerV1::capture_named_fixture(
                super::super::ClipboardSeedFixtureV1::Empty,
            )
            .unwrap();
            let named_board = owner.named_board_id().unwrap().to_owned();

            assert_eq!(owner.inject_named_external_copy_for_probe(), Ok(()));
            assert_eq!(owner.fresh_unchanged_since_capture(), Ok(false));
            assert_eq!(
                owner.current_ownership(),
                super::super::ClipboardGuardReplyV1::OwnershipChanged
            );
            assert_eq!(
                owner.restore_if_owned(),
                super::super::ClipboardGuardReplyV1::OwnershipChanged
            );
            assert_eq!(
                owner.close_after_restore(),
                super::super::ClipboardGuardReplyV1::OwnershipChanged
            );

            let observed_external_copy =
                unsafe { capture_from(named_research_pasteboard(&named_board).unwrap()).unwrap() };
            assert_eq!(observed_external_copy.items, research_external_items());
            release_named_board(&named_board).unwrap();
        }

        #[cfg(feature = "research-harness")]
        #[test]
        fn named_research_owner_keeps_unproven_partial_write_snapshot_open() {
            let mut owner = super::super::ResearchClipboardGuardOwnerV1::capture_named_fixture(
                super::super::ClipboardSeedFixtureV1::OneItemMultiFlavor,
            )
            .unwrap();
            assert_eq!(
                owner.current_ownership(),
                super::super::ClipboardGuardReplyV1::NotWritten
            );
            // Model the exact owner state after a partial attempt whose marker
            // cannot be proven. The full Snapshot stays attached for recovery.
            owner.write_attempted = true;
            owner.temporary_text = Some(super::registered_temporary_text());
            owner.ownership_token = Some("synthetic-unproven-owner-token".to_owned());
            assert_eq!(
                owner.current_ownership(),
                super::super::ClipboardGuardReplyV1::ReadUnknown
            );
            assert_eq!(
                owner.restore_if_owned(),
                super::super::ClipboardGuardReplyV1::RestoreUnknown
            );
            assert_eq!(
                owner.close_after_restore(),
                super::super::ClipboardGuardReplyV1::RestoreUnknown
            );
            assert!(!owner.closed);
            assert_eq!(owner.snapshot.items.len(), 1);

            release_named_board(owner.named_board_id().unwrap()).unwrap();
        }

        #[cfg(feature = "research-harness")]
        #[test]
        fn named_research_owner_keeps_snapshot_after_live_read_and_restore_failures() {
            let mut owner = super::super::ResearchClipboardGuardOwnerV1::capture_named_fixture(
                super::super::ClipboardSeedFixtureV1::OneItemMultiFlavor,
            )
            .unwrap();
            assert_eq!(
                owner.write_fixture(
                    super::super::ClipboardWriteFixtureV1::RegisteredTemporaryTextV1
                ),
                super::super::ClipboardGuardReplyV1::WriteOwned
            );
            let original_items = owner.snapshot.items.clone();
            let proof = owner.write_proof.clone().unwrap();

            // Preserve the original snapshot when an ordinary read fails.
            owner.test_fail_current_read = true;
            assert_eq!(
                owner.current_ownership(),
                super::super::ClipboardGuardReplyV1::ReadUnknown
            );
            assert!(marker_is_current(owner.named_board_id().unwrap(), &proof).unwrap());

            // The actual named restore backend receives this observation
            // after prepare_restore. Missing marker data at the proven
            // generation and exact shape is unknown, so restore must not
            // commit or release the saved items.
            let incomplete_observation = super::super::ClipboardWriteObservation {
                change_count: proof.change_count,
                text: None,
                token: None,
                has_exact_temporary_types: true,
            };
            owner.test_fail_current_read = false;
            owner.test_current_observation = Some(incomplete_observation.clone());
            assert_eq!(
                owner.current_ownership(),
                super::super::ClipboardGuardReplyV1::ReadUnknown
            );
            owner.test_current_observation = None;
            owner.test_restore_observation = Some(incomplete_observation);
            assert_eq!(
                owner.restore_if_owned(),
                super::super::ClipboardGuardReplyV1::RestoreUnknown
            );
            assert!(marker_is_current(owner.named_board_id().unwrap(), &proof).unwrap());
            assert_eq!(
                owner.close_after_restore(),
                super::super::ClipboardGuardReplyV1::RestoreUnknown
            );
            assert!(!owner.closed);
            assert!(!owner.restored);
            assert_eq!(owner.snapshot.items, original_items);

            owner.test_fail_current_read = false;
            owner.test_restore_observation = None;
            assert_eq!(
                owner.current_ownership(),
                super::super::ClipboardGuardReplyV1::WriteOwned
            );
            assert_eq!(
                owner.restore_if_owned(),
                super::super::ClipboardGuardReplyV1::Restored
            );
            let restored = unsafe {
                capture_from(named_research_pasteboard(owner.named_board_id().unwrap()).unwrap())
                    .unwrap()
            };
            assert_eq!(restored.items, original_items);
            assert_eq!(
                owner.close_after_restore(),
                super::super::ClipboardGuardReplyV1::Restored
            );
        }

        #[cfg(feature = "research-harness")]
        #[test]
        fn named_research_proof_classification_keeps_unknown_distinct_from_changes() {
            let proof = super::super::ClipboardWriteProof {
                change_count: 11,
                text: "VoiceFlo".to_owned(),
                token: "synthetic-owner-token".to_owned(),
            };
            let observations = [
                (
                    super::super::ClipboardWriteObservation {
                        change_count: 11,
                        text: None,
                        token: None,
                        has_exact_temporary_types: true,
                    },
                    super::super::ResearchProofObservation::Unknown,
                ),
                (
                    super::super::ClipboardWriteObservation {
                        change_count: 12,
                        text: None,
                        token: None,
                        has_exact_temporary_types: false,
                    },
                    super::super::ResearchProofObservation::Changed,
                ),
                (
                    super::super::ClipboardWriteObservation {
                        change_count: 11,
                        text: None,
                        token: None,
                        has_exact_temporary_types: false,
                    },
                    super::super::ResearchProofObservation::Changed,
                ),
                (
                    super::super::ClipboardWriteObservation {
                        change_count: 11,
                        text: Some("different complete value".to_owned()),
                        token: Some("different-complete-marker".to_owned()),
                        has_exact_temporary_types: true,
                    },
                    super::super::ResearchProofObservation::Changed,
                ),
            ];
            for (observation, expected) in observations {
                assert_eq!(
                    super::super::classify_research_proof_observation(&proof, &observation),
                    expected
                );
            }
        }

        #[cfg(feature = "research-harness")]
        #[test]
        fn general_owner_fault_mapping_never_reads_or_writes_general_pasteboard() {
            let snapshot = Snapshot {
                change_count: 41,
                items: vec![PasteboardItemSnapshot {
                    flavors: vec![Flavor {
                        name: TEXT_TYPE.to_owned(),
                        bytes: b"synthetic original".to_vec(),
                    }],
                }],
            };
            let original_items = snapshot.items.clone();
            let proof = super::super::ClipboardWriteProof {
                change_count: 42,
                text: "VoiceFlo".to_owned(),
                token: "synthetic-general-owner-token".to_owned(),
            };
            let mut owner =
                super::super::ResearchClipboardGuardOwnerV1::synthetic_general_owner_for_test(
                    snapshot, proof,
                );

            assert_eq!(super::registered_general_temporary_text(), "VoiceFlo");
            assert_eq!(
                owner.write_fixture(super::super::ClipboardWriteFixtureV1::ExternalCopyV1),
                super::super::ClipboardGuardReplyV1::Rejected
            );
            assert_eq!(
                owner.current_ownership(),
                super::super::ClipboardGuardReplyV1::ReadUnknown
            );
            assert_eq!(
                owner.restore_if_owned(),
                super::super::ClipboardGuardReplyV1::RestoreUnknown
            );
            assert_eq!(
                owner.close_after_restore(),
                super::super::ClipboardGuardReplyV1::RestoreUnknown
            );
            assert!(!owner.closed);
            assert!(!owner.restored);
            assert_eq!(owner.snapshot.items, original_items);

            owner.ownership_changed = true;
            assert_eq!(
                owner.close_after_restore(),
                super::super::ClipboardGuardReplyV1::OwnershipChanged
            );
            assert!(owner.closed);
            assert!(owner.snapshot.items.is_empty());
            assert!(owner.temporary_text.is_none());
            assert!(owner.ownership_token.is_none());
            assert!(owner.write_proof.is_none());
        }

        #[cfg(feature = "research-harness")]
        #[test]
        fn general_missing_marker_fields_are_unknown_and_never_commit_restore() {
            let snapshot = Snapshot {
                change_count: 41,
                items: vec![PasteboardItemSnapshot {
                    flavors: vec![Flavor {
                        name: TEXT_TYPE.to_owned(),
                        bytes: b"synthetic original".to_vec(),
                    }],
                }],
            };
            let original_items = snapshot.items.clone();
            let proof = super::super::ClipboardWriteProof {
                change_count: 42,
                text: "VoiceFlo".to_owned(),
                token: "synthetic-general-owner-token".to_owned(),
            };
            let owned_observation = super::super::ClipboardWriteObservation {
                change_count: proof.change_count,
                text: Some(proof.text.clone()),
                token: Some(proof.token.clone()),
                has_exact_temporary_types: true,
            };
            let incomplete_observation = super::super::ClipboardWriteObservation {
                change_count: proof.change_count,
                text: None,
                token: None,
                has_exact_temporary_types: true,
            };
            let external_observation = super::super::ClipboardWriteObservation {
                change_count: proof.change_count + 1,
                text: Some("external clipboard value".to_owned()),
                token: None,
                has_exact_temporary_types: false,
            };
            let complete_external_observation = super::super::ClipboardWriteObservation {
                change_count: proof.change_count,
                text: Some("different complete value".to_owned()),
                token: Some("different-complete-marker".to_owned()),
                has_exact_temporary_types: true,
            };
            let mut owner =
                super::super::ResearchClipboardGuardOwnerV1::synthetic_general_owner_for_test(
                    snapshot,
                    proof.clone(),
                );
            owner.test_fail_current_read = false;
            owner.test_fail_restore_prepare = false;
            owner.test_current_observation = Some(incomplete_observation.clone());
            owner.test_restore_observation = Some(incomplete_observation);

            // This is a real observation value at the owner/protocol seam:
            // generation and shape match, but AppKit could not return marker
            // data. It cannot be evidence that a different clipboard owns it.
            assert_eq!(
                owner.current_ownership(),
                super::super::ClipboardGuardReplyV1::ReadUnknown
            );
            assert_eq!(
                owner.restore_if_owned(),
                super::super::ClipboardGuardReplyV1::RestoreUnknown
            );
            assert!(!owner.test_restore_commit_attempted);
            assert_eq!(
                owner.close_after_restore(),
                super::super::ClipboardGuardReplyV1::RestoreUnknown
            );
            assert!(!owner.closed);
            assert!(!owner.restored);
            assert_eq!(owner.snapshot.items, original_items);

            // A complete same-owner observation can recover the retained
            // snapshot through the same restore protocol without touching the
            // system General pasteboard in this test.
            owner.test_current_observation = Some(owned_observation.clone());
            assert_eq!(
                owner.current_ownership(),
                super::super::ClipboardGuardReplyV1::WriteOwned
            );
            owner.test_restore_observation = Some(owned_observation);
            assert_eq!(
                owner.restore_if_owned(),
                super::super::ClipboardGuardReplyV1::Restored
            );
            assert!(owner.test_restore_commit_attempted);
            assert_eq!(
                owner.close_after_restore(),
                super::super::ClipboardGuardReplyV1::Restored
            );

            // A proven generation change remains ownership-changed even when
            // the new provider omitted its marker fields.
            let mut external_owner =
                super::super::ResearchClipboardGuardOwnerV1::synthetic_general_owner_for_test(
                    Snapshot {
                        change_count: 41,
                        items: original_items.clone(),
                    },
                    proof.clone(),
                );
            external_owner.test_fail_current_read = false;
            external_owner.test_current_observation = Some(external_observation.clone());
            assert_eq!(
                external_owner.current_ownership(),
                super::super::ClipboardGuardReplyV1::OwnershipChanged
            );

            let mut external_restore_owner =
                super::super::ResearchClipboardGuardOwnerV1::synthetic_general_owner_for_test(
                    Snapshot {
                        change_count: 41,
                        items: original_items.clone(),
                    },
                    proof.clone(),
                );
            external_restore_owner.test_fail_restore_prepare = false;
            external_restore_owner.test_restore_observation = Some(external_observation);
            assert_eq!(
                external_restore_owner.restore_if_owned(),
                super::super::ClipboardGuardReplyV1::OwnershipChanged
            );
            assert!(!external_restore_owner.test_restore_commit_attempted);
            assert_eq!(
                external_restore_owner.close_after_restore(),
                super::super::ClipboardGuardReplyV1::OwnershipChanged
            );

            let mut complete_external_owner =
                super::super::ResearchClipboardGuardOwnerV1::synthetic_general_owner_for_test(
                    Snapshot {
                        change_count: 41,
                        items: original_items,
                    },
                    proof,
                );
            complete_external_owner.test_fail_current_read = false;
            complete_external_owner.test_current_observation =
                Some(complete_external_observation.clone());
            assert_eq!(
                complete_external_owner.current_ownership(),
                super::super::ClipboardGuardReplyV1::OwnershipChanged
            );

            let mut complete_external_restore_owner =
                super::super::ResearchClipboardGuardOwnerV1::synthetic_general_owner_for_test(
                    Snapshot {
                        change_count: 41,
                        items: vec![PasteboardItemSnapshot {
                            flavors: vec![Flavor {
                                name: TEXT_TYPE.to_owned(),
                                bytes: b"synthetic original".to_vec(),
                            }],
                        }],
                    },
                    super::super::ClipboardWriteProof {
                        change_count: 42,
                        text: "VoiceFlo".to_owned(),
                        token: "synthetic-general-owner-token".to_owned(),
                    },
                );
            complete_external_restore_owner.test_fail_restore_prepare = false;
            complete_external_restore_owner.test_restore_observation =
                Some(complete_external_observation);
            assert_eq!(
                complete_external_restore_owner.restore_if_owned(),
                super::super::ClipboardGuardReplyV1::OwnershipChanged
            );
            assert!(!complete_external_restore_owner.test_restore_commit_attempted);
        }

        // Scripted IO at the actual Snapshot restore producer; no AppKit call.
        // The same fixed bytes/digest are consumed by the Swift declaration owner.
        #[test]
        fn snapshot_restore_producer_serializes_shared_wire_and_abstains_on_drift() {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct WireFlavor {
                #[serde(rename = "type")]
                name: String,
                bytes: Vec<u8>,
            }
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct WireFixture {
                artifact_type: String,
                schema_version: u32,
                items: Vec<Vec<WireFlavor>>,
                expected_identity: String,
                expected_association: serde_json::Value,
                framing: String,
                scope: String,
            }
            struct ScriptedRestore {
                observation: super::super::ClipboardWriteObservation,
                committed_items: Option<Vec<PasteboardItemSnapshot>>,
                before: Option<isize>,
                cleared: isize,
                after: Option<isize>,
            }
            impl super::super::ClipboardRestoreProtocol<[PasteboardItemSnapshot]> for ScriptedRestore {
                type Prepared = Vec<PasteboardItemSnapshot>;
                fn prepare_restore(
                    &mut self,
                    items: &[PasteboardItemSnapshot],
                ) -> Result<Self::Prepared, PasteError> {
                    Ok(items.to_vec())
                }
                fn observe(
                    &mut self,
                ) -> Result<super::super::ClipboardWriteObservation, PasteError> {
                    Ok(self.observation.clone())
                }
                fn commit_restore(
                    &mut self,
                    items: Self::Prepared,
                ) -> Result<Option<isize>, PasteError> {
                    self.committed_items = Some(items);
                    Ok(super::super::restore_commit_generation(
                        self.before,
                        self.cleared,
                        self.after,
                    ))
                }
            }
            let fixture: WireFixture =
                serde_json::from_str(include_str!("fixtures/clipboard-wire-v1.json")).unwrap();
            assert_eq!(fixture.artifact_type, "voiceflow_clipboard_wire_fixture");
            assert_eq!(fixture.schema_version, 1);
            assert!(!fixture.framing.is_empty() && !fixture.scope.is_empty());
            let mut snapshot = Snapshot {
                change_count: 10,
                items: fixture
                    .items
                    .into_iter()
                    .map(|flavors| PasteboardItemSnapshot {
                        flavors: flavors
                            .into_iter()
                            .map(|flavor| Flavor {
                                name: flavor.name,
                                bytes: flavor.bytes,
                            })
                            .collect(),
                    })
                    .collect(),
            };
            let proof = super::super::ClipboardWriteProof {
                change_count: 11,
                text: "CONTROLLED_WIRE27".into(),
                token: "fixture-token27".into(),
            };
            let observation = super::super::ClipboardWriteObservation {
                change_count: 11,
                text: Some(proof.text.clone()),
                token: Some(proof.token.clone()),
                has_exact_temporary_types: true,
            };
            let backend = |before, after| ScriptedRestore {
                observation: observation.clone(),
                committed_items: None,
                before,
                cleared: 12,
                after,
            };
            let mut exact = backend(Some(11), Some(12));
            let result = snapshot.restore_using(&mut exact, &proof).unwrap();
            assert!(result.restored);
            assert_eq!(exact.committed_items.as_ref(), Some(&snapshot.items));
            let association = result
                .association
                .expect("actual producer must bind exact commit to original bytes");
            assert_eq!(
                association.snapshot_fixture_identity,
                fixture.expected_identity
            );
            assert_eq!(
                serde_json::to_value(association).unwrap(),
                fixture.expected_association
            );

            // Successful native-write return and a changed/unknown commit sample
            // are distinct facts: preserve restored, withhold wire association.
            for (before, after) in [(Some(11), Some(13)), (None, Some(12)), (Some(11), None)] {
                let mut drift = backend(before, after);
                let result = snapshot.restore_using(&mut drift, &proof).unwrap();
                assert!(result.restored && result.association.is_none());
                assert_eq!(drift.committed_items.as_ref(), Some(&snapshot.items));
            }
            snapshot.change_count = 9;
            let mut wrong_source = backend(Some(11), Some(12));
            let result = snapshot.restore_using(&mut wrong_source, &proof).unwrap();
            assert!(result.restored && result.association.is_none());
            snapshot.change_count = 10;
            let mut stale = backend(Some(11), Some(12));
            stale.observation.change_count = 12;
            let result = snapshot.restore_using(&mut stale, &proof).unwrap();
            assert!(!result.restored && result.association.is_none());
            assert!(stale.committed_items.is_none());
        }

        fn with_named_scratch_pasteboard(test: impl FnOnce(*mut Object)) {
            static NEXT_NAME: AtomicU64 = AtomicU64::new(1);
            let name = format!(
                "com.voiceflow.tests.pasteboard.{}.{}",
                std::process::id(),
                NEXT_NAME.fetch_add(1, Ordering::Relaxed)
            );
            unsafe {
                let name = c_string(&name).unwrap();
                let name_object: *mut Object =
                    msg_send![class!(NSString), stringWithUTF8String: name.as_ptr()];
                let pasteboard: *mut Object =
                    msg_send![class!(NSPasteboard), pasteboardWithName: name_object];
                assert!(!pasteboard.is_null());
                let scratch = ScratchPasteboard(pasteboard);
                test(scratch.0);
            }
        }

        struct ScratchPasteboard(*mut Object);

        impl Drop for ScratchPasteboard {
            fn drop(&mut self) {
                unsafe {
                    let _: isize = msg_send![self.0, clearContents];
                    let _: () = msg_send![self.0, releaseGlobally];
                }
            }
        }

        fn write_fixture_items(pasteboard: *mut Object, items: &[&[u8]]) {
            unsafe {
                let objects: *mut Object =
                    msg_send![class!(NSMutableArray), arrayWithCapacity: items.len()];
                let mut allocated = Vec::new();
                for bytes in items {
                    let item = item_to_native(&PasteboardItemSnapshot {
                        flavors: vec![Flavor {
                            name: "com.voiceflow.tests.duplicate-flavor".into(),
                            bytes: bytes.to_vec(),
                        }],
                    })
                    .unwrap();
                    let _: () = msg_send![objects, addObject: item];
                    allocated.push(item);
                }
                let written: bool = msg_send![pasteboard, writeObjects: objects];
                for item in allocated {
                    let _: () = msg_send![item, release];
                }
                assert!(written);
            }
        }

        unsafe fn read_item_flavor(
            pasteboard: *mut Object,
            item_index: usize,
            flavor_name: &str,
        ) -> Vec<u8> {
            let native_items: *mut Object = msg_send![pasteboard, pasteboardItems];
            let native_item: *mut Object = msg_send![native_items, objectAtIndex: item_index];
            let type_object = string_object(flavor_name).unwrap();
            let data: *mut Object = msg_send![native_item, dataForType: type_object];
            assert!(!data.is_null());
            let length: usize = msg_send![data, length];
            let bytes: *const u8 = msg_send![data, bytes];
            if length == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(bytes, length).to_vec()
            }
        }

        unsafe fn declare_temporary_types(pasteboard: *mut Object) -> isize {
            let text_type = string_object(TEXT_TYPE).unwrap();
            let marker_type = string_object(OWNER_MARKER_TYPE).unwrap();
            let transient_type = string_object(TRANSIENT_TYPE).unwrap();
            let auto_generated_type = string_object(AUTO_GENERATED_TYPE).unwrap();
            let types: *mut Object = msg_send![class!(NSMutableArray), arrayWithCapacity: 4usize];
            let _: () = msg_send![types, addObject: text_type];
            let _: () = msg_send![types, addObject: marker_type];
            let _: () = msg_send![types, addObject: transient_type];
            let _: () = msg_send![types, addObject: auto_generated_type];
            msg_send![
                pasteboard,
                declareTypes: types
                owner: std::ptr::null_mut::<Object>()
            ]
        }

        #[test]
        fn snapshot_restore_preserves_two_items_with_the_same_type() {
            with_named_scratch_pasteboard(|pasteboard| unsafe {
                write_fixture_items(pasteboard, &[b"first item", b"second item"]);
                let snapshot = capture_from(pasteboard).unwrap();
                assert_eq!(snapshot.items.len(), 2);
                assert_eq!(snapshot.items[0].flavors[0].bytes.as_slice(), b"first item");
                assert_eq!(
                    snapshot.items[1].flavors[0].bytes.as_slice(),
                    b"second item"
                );

                // Do not clear the scratch pasteboard here: this exercises
                // replacing A with B and restoring A through the production
                // ownership protocol without relying on writeObjects replacing
                // existing NSPasteboard contents (it appends instead).
                write_temporary_to(
                    pasteboard,
                    "synthetic temporary B",
                    "synthetic-unique-owner-token",
                    snapshot.change_count,
                    &snapshot.items[..],
                )
                .unwrap();
                let observation = observe_write_from(pasteboard).unwrap();
                assert_eq!(observation.text.as_deref(), Some("synthetic temporary B"));
                assert_eq!(
                    observation.token.as_deref(),
                    Some("synthetic-unique-owner-token")
                );
                assert!(observation.has_exact_temporary_types);
                let proof = super::super::prove_clipboard_write_ownership(
                    snapshot.change_count,
                    "synthetic temporary B",
                    "synthetic-unique-owner-token",
                    &observation,
                )
                .unwrap();
                let mut backend = MacRestoreProtocol { pasteboard };
                assert!(
                    super::super::restore_clipboard_if_owned(
                        &mut backend,
                        &snapshot.items[..],
                        Some(&proof)
                    )
                    .unwrap()
                    .restored
                );
                let restored = capture_from(pasteboard).unwrap();
                assert_eq!(restored.items, snapshot.items);
            });
        }

        #[test]
        fn temporary_write_declines_same_count_external_append_before_declaration() {
            with_named_scratch_pasteboard(|pasteboard| unsafe {
                write_fixture_items(pasteboard, &[b"synthetic A"]);
                let snapshot = capture_from(pasteboard).unwrap();
                write_fixture_items(pasteboard, &[b"synthetic C"]);
                let after_append: isize = msg_send![pasteboard, changeCount];
                assert_eq!(after_append, snapshot.change_count);

                let failure = write_temporary_to_with_outcome(
                    pasteboard,
                    "synthetic temporary B",
                    "synthetic-owner-token",
                    snapshot.change_count,
                    &snapshot.items,
                )
                .unwrap_err();
                assert_eq!(
                    failure.outcome,
                    super::super::RetryClipboardOutcome::Changed
                );
                let current = capture_from(pasteboard).unwrap();
                assert_eq!(current.items.len(), 2);
                assert_eq!(current.items[0].flavors[0].bytes, b"synthetic A");
                assert_eq!(current.items[1].flavors[0].bytes, b"synthetic C");
            });
        }

        #[test]
        fn temporary_write_declines_different_count_external_replacement() {
            with_named_scratch_pasteboard(|pasteboard| unsafe {
                write_fixture_items(pasteboard, &[b"synthetic A"]);
                let snapshot = capture_from(pasteboard).unwrap();
                let _: isize = msg_send![pasteboard, clearContents];
                write_fixture_items(pasteboard, &[b"synthetic C"]);
                let changed = capture_from(pasteboard).unwrap();
                assert_ne!(changed.change_count, snapshot.change_count);

                let failure = write_temporary_to_with_outcome(
                    pasteboard,
                    "synthetic temporary B",
                    "synthetic-owner-token",
                    snapshot.change_count,
                    &snapshot.items,
                )
                .unwrap_err();
                assert_eq!(
                    failure.outcome,
                    super::super::RetryClipboardOutcome::Changed
                );
                assert_eq!(capture_from(pasteboard).unwrap().items, changed.items);
            });
        }

        #[test]
        fn temporary_write_returns_its_exact_change_count_and_marker() {
            with_named_scratch_pasteboard(|pasteboard| unsafe {
                write_fixture_items(pasteboard, &[b"synthetic A"]);
                let snapshot = capture_from(pasteboard).unwrap();
                let before: isize = msg_send![pasteboard, changeCount];
                let token = "synthetic-unique-owner-token";
                write_temporary_to_with_outcome(
                    pasteboard,
                    "synthetic temporary text",
                    token,
                    before,
                    &snapshot.items,
                )
                .unwrap();
                let observation = observe_write_from(pasteboard).unwrap();
                let proof = super::super::prove_clipboard_write_ownership(
                    before,
                    "synthetic temporary text",
                    token,
                    &observation,
                )
                .unwrap();
                assert_eq!(proof.change_count, before + 1);
                assert!(super::super::clipboard_write_proof_is_current(
                    &proof,
                    &observation
                ));
                assert_eq!(proof.change_count, before + 1);
                assert_eq!(read_item_flavor(pasteboard, 0, TRANSIENT_TYPE), b"");
                assert_eq!(read_item_flavor(pasteboard, 0, AUTO_GENERATED_TYPE), b"");

                write_fixture_items(pasteboard, &[b"external C"]);
                let external = observe_write_from(pasteboard).unwrap();
                assert_eq!(external.change_count, proof.change_count);
                assert_eq!(external.text, None);
                assert!(!external.has_exact_temporary_types);
                assert!(!super::super::clipboard_write_proof_is_current(
                    &proof, &external
                ));
                let mut backend = MacRestoreProtocol { pasteboard };
                assert!(
                    !super::super::restore_clipboard_if_owned(
                        &mut backend,
                        &snapshot.items,
                        Some(&proof)
                    )
                    .unwrap()
                    .restored
                );
                let current_items: *mut Object = msg_send![pasteboard, pasteboardItems];
                let current_count: usize = msg_send![current_items, count];
                assert_eq!(current_count, 2);
                assert_eq!(
                    read_item_flavor(pasteboard, 1, "com.voiceflow.tests.duplicate-flavor"),
                    b"external C"
                );
            });
        }

        #[test]
        fn partial_temporary_write_restores_original_after_our_declaration() {
            with_named_scratch_pasteboard(|pasteboard| unsafe {
                write_fixture_items(pasteboard, &[b"original A", b"second original item"]);
                let snapshot = capture_from(pasteboard).unwrap();
                let before = snapshot.change_count;
                let prepared_original = prepare_native_restore(&snapshot.items).unwrap();
                let declared_count = declare_temporary_types(pasteboard);
                assert_eq!(declared_count, before + 1);

                // Model a failure before either data flavor can be written.
                // The declaration receipt and exact empty declared item prove
                // this is our partial write, so the old two-item clipboard is
                // restored without deleting an unrelated clipboard.
                let partial = observe_write_from(pasteboard).unwrap();
                assert_eq!(partial.change_count, declared_count);
                assert!(partial.has_exact_temporary_types);
                assert_eq!(partial.text, None);
                assert_eq!(partial.token, None);
                assert!(restore_declared_stage_if_owned(
                    pasteboard,
                    prepared_original,
                    declared_count,
                    "synthetic-owner",
                    "synthetic-B",
                )
                .unwrap());
                assert_eq!(capture_from(pasteboard).unwrap().items, snapshot.items);
            });
        }

        #[test]
        fn partial_temporary_write_preserves_external_append_at_same_change_count() {
            with_named_scratch_pasteboard(|pasteboard| unsafe {
                write_fixture_items(pasteboard, &[b"original A"]);
                let snapshot = capture_from(pasteboard).unwrap();
                let prepared_original = prepare_native_restore(&snapshot.items).unwrap();
                let declared_count = declare_temporary_types(pasteboard);
                write_fixture_items(pasteboard, &[b"external C"]);
                let after_append: isize = msg_send![pasteboard, changeCount];
                assert_eq!(after_append, declared_count);
                let partial = observe_write_from(pasteboard).unwrap();
                assert_eq!(partial.change_count, declared_count);
                assert!(!partial.has_exact_temporary_types);

                assert!(!restore_declared_stage_if_owned(
                    pasteboard,
                    prepared_original,
                    declared_count,
                    "synthetic-owner",
                    "synthetic-B",
                )
                .unwrap());
                let current_items: *mut Object = msg_send![pasteboard, pasteboardItems];
                let current_count: usize = msg_send![current_items, count];
                assert_eq!(current_count, 2);
                assert_eq!(
                    read_item_flavor(pasteboard, 1, "com.voiceflow.tests.duplicate-flavor"),
                    b"external C"
                );
            });
        }
    }
}

#[cfg(target_os = "macos")]
mod macos_ax {
    use super::{
        ax_insert_decision, classify_ax_full_field_write, AxInsertAttempt, AxInsertDecision,
        CancellationToken, CapturedTextActionSource, PasteError,
    };
    #[cfg(test)]
    use super::{classify_ax_insert_result, utf16_splice};
    use core::ffi::c_void;
    use core_foundation::base::{
        CFEqual, CFRange, CFRelease, CFType, CFTypeID, CFTypeRef, TCFType,
    };
    use core_foundation::number::CFNumber;
    use core_foundation::string::{CFString, CFStringRef};

    type AXUIElementRef = *const c_void;
    type AXValueRef = *const c_void;
    const AX_SUCCESS: i32 = 0;
    const AX_VALUE_CGPOINT: u32 = 1;
    const AX_VALUE_CGSIZE: u32 = 2;
    const AX_VALUE_CF_RANGE: u32 = 4;

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
        fn AXUIElementCopyParameterizedAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            parameter: CFTypeRef,
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
        fn AXValueGetType(value: AXValueRef) -> u32;
        fn AXValueGetTypeID() -> CFTypeID;
    }

    struct AxElement(AXUIElementRef);

    struct AxValue(AXValueRef);

    impl Drop for AxValue {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0 as CFTypeRef) };
                self.0 = std::ptr::null();
            }
        }
    }

    impl Drop for AxElement {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0 as CFTypeRef) };
                self.0 = std::ptr::null();
            }
        }
    }

    pub(super) struct FocusedFieldAnchor {
        pid: i32,
        focused: AxElement,
    }

    impl FocusedFieldAnchor {
        pub(super) fn capture_with_metadata(
            pid: i32,
        ) -> Result<(Self, super::FocusedFieldMetadata), super::FocusedFieldCaptureFailure>
        {
            if pid <= 0 || !crate::permissions::accessibility_is_trusted() {
                return Err(super::FocusedFieldCaptureFailure::ObservationUnavailable);
            }
            let captured = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let app = AxElement(unsafe { AXUIElementCreateApplication(pid) });
                if app.0.is_null() {
                    return None;
                }
                unsafe {
                    let _ = AXUIElementSetMessagingTimeout(app.0, 0.35);
                }
                let focused = copy_element_attr(&app, "AXFocusedUIElement")?;
                unsafe {
                    let _ = AXUIElementSetMessagingTimeout(focused.0, 0.35);
                }
                let role = copy_string_attr(&focused, "AXRole");
                let subrole = copy_string_attr(&focused, "AXSubrole");
                let description = copy_string_attr(&focused, "AXDescription");
                #[cfg(any(test, feature = "research-harness"))]
                let identifier = copy_string_attr(&focused, "AXIdentifier");
                let title = copy_string_attr(&focused, "AXTitle");
                let position = copy_point_attr(&focused, "AXPosition")
                    .map(|point| format!("{},{}", point.x, point.y));
                let size = copy_size_attr(&focused, "AXSize")
                    .map(|size| format!("{},{}", size.width, size.height));
                let window = copy_element_attr(&focused, "AXWindow");
                let app_focused_window = copy_element_attr(&app, "AXFocusedWindow");
                let ax_window_matches_app_focused_window =
                    match (window.as_ref(), app_focused_window.as_ref()) {
                        (Some(field_window), Some(app_window)) => Some(unsafe {
                            CFEqual(field_window.0 as CFTypeRef, app_window.0 as CFTypeRef) != 0
                        }),
                        _ => None,
                    };
                let (window_title, window_position, window_size) = window
                    .as_ref()
                    .map(|window| {
                        (
                            copy_string_attr(window, "AXTitle"),
                            copy_point_attr(window, "AXPosition")
                                .map(|point| format!("{},{}", point.x, point.y)),
                            copy_size_attr(window, "AXSize")
                                .map(|size| format!("{},{}", size.width, size.height)),
                        )
                    })
                    .unwrap_or_default();
                let window_document_path = window
                    .as_ref()
                    .and_then(|window| copy_file_url_attr(window, "AXDocument"));
                let role_blob = format!(
                    "{} {}",
                    role.as_deref().unwrap_or_default(),
                    subrole.as_deref().unwrap_or_default()
                )
                .to_ascii_lowercase();
                let secure_input = role_blob.contains("securetextfield")
                    || role_blob.contains("secure text field");
                Some((
                    Self { pid, focused },
                    super::FocusedFieldMetadata {
                        role,
                        subrole,
                        description,
                        #[cfg(any(test, feature = "research-harness"))]
                        identifier,
                        title,
                        position,
                        size,
                        window_title,
                        window_document_path,
                        window_position,
                        window_size,
                        ax_window_matches_app_focused_window,
                        secure_input,
                    },
                ))
            }))
            .ok()
            .flatten()
            .ok_or(super::FocusedFieldCaptureFailure::ObservationUnavailable)?;
            Ok(captured)
        }

        pub(super) fn is_current_focus(&self) -> bool {
            self.current_focus_matches().unwrap_or(false)
        }

        pub(super) fn current_focus_matches(&self) -> Option<bool> {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let current = focused_element_for_pid(self.pid)?;
                Some(unsafe { CFEqual(self.focused.0 as CFTypeRef, current.0 as CFTypeRef) != 0 })
            }))
            .ok()
            .flatten()
        }

        pub(super) fn read_value(&self) -> Option<String> {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                copy_string_attr(&self.focused, "AXValue")
            }))
            .ok()
            .flatten()
        }

        pub(super) fn read_value_bounded(&self, max_utf16: usize) -> Option<String> {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                copy_string_attr_bounded(&self.focused, "AXValue", max_utf16)
            }))
            .ok()
            .flatten()
        }

        pub(super) fn read_full_range_exact(
            &self,
            expected: &str,
            expected_caret_utf16: u64,
        ) -> super::FullRangeReadback {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                read_full_range_exact(&self.focused, expected, expected_caret_utf16)
            }))
            .unwrap_or(super::FullRangeReadback::Unavailable)
        }

        pub(super) fn read_selected_range(&self) -> Option<(i64, i64)> {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                copy_range_attr(&self.focused, "AXSelectedTextRange")
            }))
            .ok()
            .flatten()
        }

        pub(super) fn read_selected_text(&self) -> Option<String> {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                copy_string_attr(&self.focused, "AXSelectedText")
            }))
            .ok()
            .flatten()
        }

        pub(super) fn read_selected_text_bounded(&self, max_utf16: usize) -> Option<String> {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                copy_string_attr_bounded(&self.focused, "AXSelectedText", max_utf16)
            }))
            .ok()
            .flatten()
        }

        pub(super) fn can_replace_text(&self) -> bool {
            if !self.is_current_focus() {
                return false;
            }
            let role = copy_string_attr(&self.focused, "AXRole").unwrap_or_default();
            let subrole = copy_string_attr(&self.focused, "AXSubrole").unwrap_or_default();
            let selected_text_settable = is_settable(&self.focused, "AXSelectedText");
            let value_settable = is_settable(&self.focused, "AXValue");
            let Some(value) = copy_string_attr(&self.focused, "AXValue") else {
                return false;
            };
            matches!(
                ax_insert_decision(
                    &role,
                    &subrole,
                    selected_text_settable,
                    value_settable,
                    copy_range_attr(&self.focused, "AXSelectedTextRange"),
                    value.is_empty(),
                ),
                AxInsertDecision::AttemptSelectedText | AxInsertDecision::AttemptValueSplice
            )
        }

        pub(super) fn can_replace_text_for_admitted_value(
            &self,
            value: &str,
            selection_range: Option<(i64, i64)>,
        ) -> bool {
            if !self.is_current_focus() {
                return false;
            }
            let role = copy_string_attr(&self.focused, "AXRole").unwrap_or_default();
            let subrole = copy_string_attr(&self.focused, "AXSubrole").unwrap_or_default();
            let selected_text_settable = is_settable(&self.focused, "AXSelectedText");
            let value_settable = is_settable(&self.focused, "AXValue");
            matches!(
                ax_insert_decision(
                    &role,
                    &subrole,
                    selected_text_settable,
                    value_settable,
                    selection_range,
                    value.is_empty(),
                ),
                AxInsertDecision::AttemptSelectedText | AxInsertDecision::AttemptValueSplice
            )
        }

        pub(super) fn can_replace_full_field(&self) -> bool {
            if !self.is_current_focus() || !is_settable(&self.focused, "AXValue") {
                return false;
            }
            let role = copy_string_attr(&self.focused, "AXRole").unwrap_or_default();
            let subrole = copy_string_attr(&self.focused, "AXSubrole").unwrap_or_default();
            let role_blob = format!("{role} {subrole}").to_ascii_lowercase();
            ["textfield", "textarea", "combobox", "searchfield"]
                .iter()
                .any(|marker| role_blob.contains(marker))
                && !role_blob.contains("securetextfield")
                && !role_blob.contains("secure text field")
                && copy_string_attr(&self.focused, "AXValue").is_some()
        }

        pub(super) fn can_replace_full_field_for_admitted_value(&self, value: &str) -> bool {
            if !self.is_current_focus() || !is_settable(&self.focused, "AXValue") {
                return false;
            }
            let role = copy_string_attr(&self.focused, "AXRole").unwrap_or_default();
            let subrole = copy_string_attr(&self.focused, "AXSubrole").unwrap_or_default();
            let role_blob = format!("{role} {subrole}").to_ascii_lowercase();
            !value.is_empty()
                && ["textfield", "textarea", "combobox", "searchfield"]
                    .iter()
                    .any(|marker| role_blob.contains(marker))
                && !role_blob.contains("securetextfield")
                && !role_blob.contains("secure text field")
        }

        fn matches_element(&self, element: &AxElement) -> bool {
            self.pid > 0
                && unsafe { CFEqual(self.focused.0 as CFTypeRef, element.0 as CFTypeRef) != 0 }
        }
    }

    fn focused_element_for_pid(pid: i32) -> Option<AxElement> {
        if pid <= 0 || !crate::permissions::accessibility_is_trusted() {
            return None;
        }
        let app = AxElement(unsafe { AXUIElementCreateApplication(pid) });
        if app.0.is_null() {
            return None;
        }
        unsafe {
            let _ = AXUIElementSetMessagingTimeout(app.0, 0.35);
        }
        let focused = copy_element_attr(&app, "AXFocusedUIElement")?;
        unsafe {
            let _ = AXUIElementSetMessagingTimeout(focused.0, 0.35);
        }
        Some(focused)
    }

    #[cfg(test)]
    pub(super) fn try_insert(
        expected_pid: i32,
        expected_anchor: &FocusedFieldAnchor,
        text: &str,
        cancellation: &CancellationToken,
        verify_target: &impl Fn() -> Result<(), PasteError>,
    ) -> AxInsertAttempt {
        if !crate::permissions::accessibility_is_trusted() {
            return AxInsertAttempt::NotAttempted;
        }
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            try_insert_inner(
                expected_pid,
                expected_anchor,
                text,
                cancellation,
                verify_target,
            )
        }))
        .unwrap_or(AxInsertAttempt::Uncertain)
    }

    #[cfg(test)]
    fn try_insert_inner(
        expected_pid: i32,
        expected_anchor: &FocusedFieldAnchor,
        text: &str,
        cancellation: &CancellationToken,
        verify_target: &impl Fn() -> Result<(), PasteError>,
    ) -> AxInsertAttempt {
        if expected_anchor.pid != expected_pid || !expected_anchor.is_current_focus() {
            return AxInsertAttempt::NotAttempted;
        }
        let Some(focused) = focused_element_for_pid(expected_pid) else {
            return AxInsertAttempt::NotAttempted;
        };
        if !expected_anchor.matches_element(&focused) {
            return AxInsertAttempt::NotAttempted;
        }
        let role = copy_string_attr(&focused, "AXRole").unwrap_or_default();
        let subrole = copy_string_attr(&focused, "AXSubrole").unwrap_or_default();
        let selected_text_settable = is_settable(&focused, "AXSelectedText");
        let value_settable = is_settable(&focused, "AXValue");
        let Some(current_value) = copy_string_attr(&focused, "AXValue") else {
            return AxInsertAttempt::NotAttempted;
        };
        let selected_range = copy_range_attr(&focused, "AXSelectedTextRange");
        let effective_range = selected_range.or_else(|| current_value.is_empty().then_some((0, 0)));
        let field_empty = current_value.is_empty();

        match ax_insert_decision(
            &role,
            &subrole,
            selected_text_settable,
            value_settable,
            effective_range,
            field_empty,
        ) {
            AxInsertDecision::AttemptSelectedText => try_replace_selection(
                expected_pid,
                expected_anchor,
                &focused,
                &current_value,
                effective_range,
                "AXSelectedText",
                text,
                cancellation,
                verify_target,
            ),
            AxInsertDecision::AttemptValueSplice => try_replace_selection(
                expected_pid,
                expected_anchor,
                &focused,
                &current_value,
                effective_range,
                "AXValue",
                text,
                cancellation,
                verify_target,
            ),
            _ => AxInsertAttempt::NotAttempted,
        }
    }

    pub(super) fn try_replace_full_field(
        expected_pid: i32,
        expected_anchor: &FocusedFieldAnchor,
        source: &CapturedTextActionSource,
        text: &str,
        cancellation: &CancellationToken,
        verify_target: &impl Fn() -> Result<(), PasteError>,
    ) -> AxInsertAttempt {
        if !crate::permissions::accessibility_is_trusted() {
            return AxInsertAttempt::NotAttempted;
        }
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            try_replace_full_field_inner(
                expected_pid,
                expected_anchor,
                source,
                text,
                cancellation,
                verify_target,
            )
        }))
        .unwrap_or(AxInsertAttempt::Uncertain)
    }

    fn try_replace_full_field_inner(
        expected_pid: i32,
        expected_anchor: &FocusedFieldAnchor,
        source: &CapturedTextActionSource,
        text: &str,
        cancellation: &CancellationToken,
        verify_target: &impl Fn() -> Result<(), PasteError>,
    ) -> AxInsertAttempt {
        if expected_anchor.pid != expected_pid || !expected_anchor.is_current_focus() {
            return AxInsertAttempt::NotAttempted;
        }
        let Some(focused) = focused_element_for_pid(expected_pid) else {
            return AxInsertAttempt::NotAttempted;
        };
        if !expected_anchor.matches_element(&focused) {
            return AxInsertAttempt::NotAttempted;
        }
        let role = copy_string_attr(&focused, "AXRole").unwrap_or_default();
        let subrole = copy_string_attr(&focused, "AXSubrole").unwrap_or_default();
        let role_blob = format!("{role} {subrole}").to_ascii_lowercase();
        let is_text_field = ["textfield", "textarea", "combobox", "searchfield"]
            .iter()
            .any(|marker| role_blob.contains(marker));
        if !is_text_field
            || role_blob.contains("securetextfield")
            || role_blob.contains("secure text field")
            || !is_settable(&focused, "AXValue")
        {
            return AxInsertAttempt::NotAttempted;
        }
        let Some(before) = copy_string_attr(&focused, "AXValue") else {
            return AxInsertAttempt::NotAttempted;
        };
        if before != source.text
            || copy_range_attr(&focused, "AXSelectedTextRange") != source.selection_range
            || !expected_anchor.is_current_focus()
            || !is_same_focused_element(&focused, expected_pid)
        {
            return AxInsertAttempt::NotAttempted;
        }
        if before == text {
            return AxInsertAttempt::NoOp;
        }
        if cancellation.is_cancelled()
            || verify_target().is_err()
            || !expected_anchor.is_current_focus()
            || !is_same_focused_element(&focused, expected_pid)
        {
            return AxInsertAttempt::NotAttempted;
        }
        let setter_reported_success = set_string_attr(&focused, "AXValue", text);
        let after = copy_string_attr(&focused, "AXValue");
        if !expected_anchor.is_current_focus() || !is_same_focused_element(&focused, expected_pid) {
            return AxInsertAttempt::Uncertain;
        }
        classify_ax_full_field_write(&before, text, after, setter_reported_success)
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    fn try_replace_selection(
        expected_pid: i32,
        expected_anchor: &FocusedFieldAnchor,
        focused: &AxElement,
        before: &str,
        selected_range: Option<(i64, i64)>,
        attribute: &str,
        text: &str,
        cancellation: &CancellationToken,
        verify_target: &impl Fn() -> Result<(), PasteError>,
    ) -> AxInsertAttempt {
        let Some((location, length)) = selected_range else {
            return AxInsertAttempt::NotAttempted;
        };
        let Some(expected_after) = utf16_splice(before, location, length, text) else {
            return AxInsertAttempt::NotAttempted;
        };
        if expected_anchor.pid != expected_pid
            || !expected_anchor.matches_element(focused)
            || !expected_anchor.is_current_focus()
            || !is_same_focused_element(focused, expected_pid)
        {
            return AxInsertAttempt::NotAttempted;
        }
        if expected_after == before {
            return AxInsertAttempt::NoOp;
        }
        if cancellation.is_cancelled()
            || verify_target().is_err()
            || !expected_anchor.is_current_focus()
            || !is_same_focused_element(focused, expected_pid)
        {
            return AxInsertAttempt::NotAttempted;
        }

        let setter_reported_success = if attribute == "AXSelectedText" {
            set_string_attr(focused, attribute, text)
        } else {
            set_string_attr(focused, attribute, &expected_after)
        };
        let after = copy_string_attr(focused, "AXValue");
        let after_range = copy_range_attr(focused, "AXSelectedTextRange");
        if !expected_anchor.is_current_focus() || !is_same_focused_element(focused, expected_pid) {
            return AxInsertAttempt::Uncertain;
        }
        let outcome = classify_ax_insert_result(
            before,
            &expected_after,
            after,
            setter_reported_success,
            Some((location, length)),
            after_range,
        );
        if matches!(outcome, AxInsertAttempt::Verified(_)) && attribute == "AXValue" {
            let caret = location.saturating_add(text.encode_utf16().count() as i64);
            let _ = set_range_attr(focused, "AXSelectedTextRange", caret, 0);
        }
        outcome
    }

    fn is_same_focused_element(focused: &AxElement, expected_pid: i32) -> bool {
        let Some(current) = focused_element_for_pid(expected_pid) else {
            return false;
        };
        unsafe { CFEqual(focused.0 as CFTypeRef, current.0 as CFTypeRef) != 0 }
    }

    fn copy_element_attr(element: &AxElement, name: &str) -> Option<AxElement> {
        let value = copy_raw_attr(element, name)?;
        Some(AxElement(value as AXUIElementRef))
    }

    fn copy_file_url_attr(element: &AxElement, name: &str) -> Option<std::path::PathBuf> {
        use core_foundation::base::CFType;
        let value = copy_raw_attr(element, name)?;
        let cf_type = unsafe { CFType::wrap_under_create_rule(value) };
        file_url_representation_to_path(&cf_type)
    }

    const MAX_AX_FILE_URL_UTF16: usize = 16 * 1024;
    const MAX_AX_FILE_URL_BYTES: usize = 64 * 1024;

    fn file_url_representation_to_path(value: &CFType) -> Option<std::path::PathBuf> {
        use core_foundation::{
            string::{CFString, CFStringGetLength},
            url::CFURL,
        };

        let url_text = if value.type_of() == CFURL::type_id() {
            unsafe { CFURL::wrap_under_get_rule(value.as_CFTypeRef() as _) }.get_string()
        } else if value.type_of() == CFString::type_id() {
            unsafe { CFString::wrap_under_get_rule(value.as_CFTypeRef() as _) }
        } else {
            return None;
        };

        let length = unsafe { CFStringGetLength(url_text.as_concrete_TypeRef()) };
        if length < 0 || length as usize > MAX_AX_FILE_URL_UTF16 {
            return None;
        }
        file_url_text_to_path(&url_text.to_string())
    }

    fn file_url_text_to_path(value: &str) -> Option<std::path::PathBuf> {
        if !has_explicit_absolute_local_file_url(value)
            || value.is_empty()
            || value.len() > MAX_AX_FILE_URL_BYTES
            || value
                .bytes()
                .any(|byte| byte == 0 || byte.is_ascii_control() || byte == b' ' || byte == b'\\')
            || !has_well_formed_percent_escapes(value)
        {
            return None;
        }

        let url = reqwest::Url::parse(value).ok()?;
        if url.scheme() != "file"
            || url.host_str().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return None;
        }

        let path = url.to_file_path().ok()?;
        let path_text = path.to_str()?;
        (path.is_absolute() && !path_text.contains('\0')).then_some(path)
    }

    fn has_explicit_absolute_local_file_url(value: &str) -> bool {
        let Some((scheme, remainder)) = value.split_once(':') else {
            return false;
        };
        scheme.eq_ignore_ascii_case("file")
            && remainder.starts_with("///")
            && !remainder.starts_with("////")
    }

    fn has_well_formed_percent_escapes(value: &str) -> bool {
        let bytes = value.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' {
                if index + 2 >= bytes.len()
                    || !bytes[index + 1].is_ascii_hexdigit()
                    || !bytes[index + 2].is_ascii_hexdigit()
                {
                    return false;
                }
                index += 3;
            } else {
                index += 1;
            }
        }
        true
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
        copy_string_attr_bounded(element, name, usize::MAX)
    }
    fn copy_string_attr_bounded(
        element: &AxElement,
        name: &str,
        max_utf16: usize,
    ) -> Option<String> {
        let value = copy_raw_attr(element, name)?;
        let cf_type = unsafe { CFType::wrap_under_create_rule(value) };
        if cf_type.type_of() != CFString::type_id() {
            return None;
        }
        let value = unsafe { CFString::wrap_under_get_rule(cf_type.as_CFTypeRef() as _) };
        let length =
            unsafe { core_foundation::string::CFStringGetLength(value.as_concrete_TypeRef()) };
        if length < 0 || length as usize > max_utf16 {
            return None;
        }
        Some(value.to_string())
    }

    fn copy_ax_character_count(element: &AxElement) -> Option<usize> {
        let raw = copy_raw_attr(element, "AXNumberOfCharacters")?;
        let value = unsafe { CFType::wrap_under_create_rule(raw) };
        if value.type_of() != CFNumber::type_id() {
            return None;
        }
        let number = unsafe { CFNumber::wrap_under_get_rule(value.as_CFTypeRef() as _) };
        let count = usize::try_from(number.to_i64()?).ok()?;
        (count <= super::MAX_FULL_RANGE_READBACK_UTF16).then_some(count)
    }

    fn read_full_range_exact(
        element: &AxElement,
        expected: &str,
        expected_caret_utf16: u64,
    ) -> super::FullRangeReadback {
        let expected_units = expected.encode_utf16().count();
        if expected_units > super::MAX_FULL_RANGE_READBACK_UTF16
            || expected_caret_utf16 > expected_units as u64
        {
            return super::FullRangeReadback::Unavailable;
        }

        // Keep a single bounded fallback attempt from consuming several full
        // default AX message timeouts. The normal fast-path AXValue polling
        // retains its existing timeout and cadence.
        unsafe {
            let _ = AXUIElementSetMessagingTimeout(element.0, super::FULL_RANGE_AX_TIMEOUT_SECONDS);
        }
        let result =
            read_full_range_exact_inner(element, expected, expected_units, expected_caret_utf16);
        unsafe {
            let _ = AXUIElementSetMessagingTimeout(element.0, 0.35);
        }
        match result {
            Some(true) => super::FullRangeReadback::Verified,
            Some(false) => super::FullRangeReadback::Mismatch,
            None => super::FullRangeReadback::Unavailable,
        }
    }

    fn read_full_range_exact_inner(
        element: &AxElement,
        expected: &str,
        expected_units: usize,
        expected_caret_utf16: u64,
    ) -> Option<bool> {
        let before_count = copy_ax_character_count(element)?;
        if before_count != expected_units {
            return Some(false);
        }
        let length = isize::try_from(before_count).ok()?;
        let range = CFRange {
            location: 0,
            length,
        };
        let range_value = AxValue(unsafe {
            AXValueCreate(AX_VALUE_CF_RANGE, &range as *const _ as *const c_void)
        });
        if range_value.0.is_null() {
            return None;
        }
        let attribute = CFString::new("AXStringForRange");
        let mut raw_value: CFTypeRef = std::ptr::null();
        let result = unsafe {
            AXUIElementCopyParameterizedAttributeValue(
                element.0,
                attribute.as_concrete_TypeRef(),
                range_value.0 as CFTypeRef,
                &mut raw_value,
            )
        };
        if result != AX_SUCCESS || raw_value.is_null() {
            return None;
        }
        let value = unsafe { CFType::wrap_under_create_rule(raw_value) };
        if value.type_of() != CFString::type_id() {
            return Some(false);
        }
        let string = unsafe { CFString::wrap_under_get_rule(value.as_CFTypeRef() as _) };
        let string_units =
            unsafe { core_foundation::string::CFStringGetLength(string.as_concrete_TypeRef()) };
        if string_units < 0 || string_units as usize != before_count {
            return Some(false);
        }
        let full_range_text = string.to_string();
        let caret = copy_range_attr(element, "AXSelectedTextRange");
        let after_count = copy_ax_character_count(element);
        let expected_caret = i64::try_from(expected_caret_utf16).ok()?;
        Some(
            full_range_text == expected
                && full_range_text.encode_utf16().count() == before_count
                && caret == Some((expected_caret, 0))
                && after_count == Some(before_count),
        )
    }

    fn copy_range_attr(element: &AxElement, name: &str) -> Option<(i64, i64)> {
        let value = unsafe { CFType::wrap_under_create_rule(copy_raw_attr(element, name)?) };
        if value.type_of() != unsafe { AXValueGetTypeID() } {
            return None;
        }
        let ax_value = value.as_CFTypeRef() as AXValueRef;
        if unsafe { AXValueGetType(ax_value) } != AX_VALUE_CF_RANGE {
            return None;
        }
        let mut range = CFRange {
            location: 0,
            length: 0,
        };
        let ok = unsafe {
            AXValueGetValue(
                ax_value,
                AX_VALUE_CF_RANGE,
                &mut range as *mut _ as *mut c_void,
            )
        };
        if ok == 0 {
            return None;
        }
        Some((
            i64::try_from(range.location).ok()?,
            i64::try_from(range.length).ok()?,
        ))
    }

    fn copy_point_attr(element: &AxElement, name: &str) -> Option<CgPoint> {
        let value = copy_raw_attr(element, name)?;
        let value = unsafe { CFType::wrap_under_create_rule(value) };
        let mut point = CgPoint { x: 0.0, y: 0.0 };
        let ok = unsafe {
            AXValueGetValue(
                value.as_CFTypeRef() as AXValueRef,
                AX_VALUE_CGPOINT,
                &mut point as *mut _ as *mut c_void,
            )
        };
        (ok != 0).then_some(point)
    }

    fn copy_size_attr(element: &AxElement, name: &str) -> Option<CgSize> {
        let value = copy_raw_attr(element, name)?;
        let value = unsafe { CFType::wrap_under_create_rule(value) };
        let mut size = CgSize {
            width: 0.0,
            height: 0.0,
        };
        let ok = unsafe {
            AXValueGetValue(
                value.as_CFTypeRef() as AXValueRef,
                AX_VALUE_CGSIZE,
                &mut size as *mut _ as *mut c_void,
            )
        };
        (ok != 0).then_some(size)
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

    #[cfg(test)]
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

    #[cfg(test)]
    mod document_url_tests {
        use super::{file_url_representation_to_path, file_url_text_to_path};
        use core_foundation::{
            base::{CFType, TCFType},
            number::CFNumber,
            url::CFURL,
        };
        use std::path::PathBuf;

        #[test]
        fn accepts_cfstring_and_cfurl_for_the_same_encoded_local_document() {
            let expected = PathBuf::from("/tmp/VoiceFlow V24 - 世界.txt");
            let url = CFURL::from_path(&expected, false).expect("fixture path is representable");
            let string = url.get_string();
            let url_value = unsafe { CFType::wrap_under_get_rule(url.as_CFTypeRef()) };
            let string_value = unsafe { CFType::wrap_under_get_rule(string.as_CFTypeRef()) };

            assert_eq!(
                file_url_representation_to_path(&url_value),
                Some(expected.clone())
            );
            assert_eq!(
                file_url_representation_to_path(&string_value),
                Some(expected)
            );
        }

        #[test]
        fn rejects_other_cf_types_and_mismatched_expected_document_paths() {
            let number = CFNumber::from(7);
            let number_value = unsafe { CFType::wrap_under_get_rule(number.as_CFTypeRef()) };
            assert_eq!(file_url_representation_to_path(&number_value), None);

            let actual = file_url_text_to_path("file:///tmp/VoiceFlow-V24-other.txt").unwrap();
            let expected = PathBuf::from("/tmp/VoiceFlow-V24-owned.txt");
            assert_ne!(actual, expected);
        }

        #[test]
        fn rejects_nonlocal_or_ambiguous_file_url_text() {
            for value in [
                "https://example.com/file.txt",
                "/tmp/file.txt",
                "file:relative.txt",
                "file://localhost/tmp/file.txt",
                "file://example.com/tmp/file.txt",
                "file://user:secret@host/tmp/file.txt",
                "file:///tmp/file.txt?mode=1",
                "file:///tmp/file.txt#section",
                "file:///tmp/%00name.txt",
                "file:///tmp/bad%2.txt",
                "file:///tmp/bad%GG.txt",
                "file:///tmp/raw\\separator.txt",
                "file:///tmp/raw space.txt",
            ] {
                assert_eq!(file_url_text_to_path(value), None, "accepted {value:?}");
            }
        }

        #[test]
        fn rejects_oversized_url_representations_before_path_conversion() {
            let oversized = format!("file:///tmp/{}", "x".repeat(super::MAX_AX_FILE_URL_BYTES));
            assert_eq!(file_url_text_to_path(&oversized), None);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::super::{
            confirm_keyboard_paste_readback, delivery_target_is_frontmost, FullRangeReadback,
            PasteReadbackExpectation, PasteVerificationFact, POST_PASTE_READBACK_DELAY_MS,
        };
        use super::FocusedFieldAnchor;
        use serde::Deserialize;
        use sha2::Digest;
        use std::ffi::CStr;
        use std::fs::{self, OpenOptions};
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        use std::path::{Path, PathBuf};
        use std::time::{Duration, Instant};
        use tokio_util::sync::CancellationToken;

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Lease {
            schema_version: u32,
            nonce: String,
            token: String,
            fixture_pid: i32,
            executable_path: PathBuf,
            executable_sha256: String,
            birth_seconds: u64,
            birth_microseconds: u64,
            window_title: String,
            field_identifier: String,
            field_description: String,
            other_field_identifier: String,
            before: String,
            expected_after: String,
            selection_utf16: Utf16Range,
            expected_caret_utf16: u64,
            mode: String,
            delay_ms: u64,
            arm_request_path: PathBuf,
            arm_ack_path: PathBuf,
            settled_path: PathBuf,
            result_path: PathBuf,
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Utf16Range {
            location: i64,
            length: i64,
        }

        fn read_json(path: &Path) -> serde_json::Value {
            serde_json::from_slice(&fs::read(path).expect("read fixture receipt"))
                .expect("parse fixture receipt")
        }

        fn fixture_process_matches(lease: &Lease) -> bool {
            if lease.fixture_pid <= 0 || !lease.executable_path.is_absolute() {
                return false;
            }
            let mut info = unsafe { std::mem::zeroed::<libc::proc_bsdinfo>() };
            let size = std::mem::size_of_val(&info) as libc::c_int;
            let read = unsafe {
                libc::proc_pidinfo(
                    lease.fixture_pid,
                    libc::PROC_PIDTBSDINFO,
                    0,
                    &mut info as *mut _ as *mut libc::c_void,
                    size,
                )
            };
            if read != size
                || info.pbi_pid != lease.fixture_pid as u32
                || info.pbi_start_tvsec != lease.birth_seconds
                || info.pbi_start_tvusec != lease.birth_microseconds
            {
                return false;
            }

            let mut path_bytes = [0_i8; 4096];
            let path_len = unsafe {
                libc::proc_pidpath(
                    lease.fixture_pid,
                    path_bytes.as_mut_ptr() as *mut libc::c_void,
                    path_bytes.len() as u32,
                )
            };
            if path_len <= 0 {
                return false;
            }
            let Ok(actual_path) = unsafe { CStr::from_ptr(path_bytes.as_ptr()) }.to_str() else {
                return false;
            };
            let expected_path = match lease.executable_path.canonicalize() {
                Ok(path) => path,
                Err(_) => return false,
            };
            let actual_path = match Path::new(actual_path).canonicalize() {
                Ok(path) => path,
                Err(_) => return false,
            };
            if actual_path != expected_path {
                return false;
            }
            let Ok(bytes) = fs::read(&expected_path) else {
                return false;
            };
            hex::encode(sha2::Sha256::digest(bytes)) == lease.executable_sha256
        }

        fn load_lease() -> (PathBuf, Lease) {
            let path = std::env::var_os("VOICEFLOW_OWNED_AX_LEASE")
                .map(PathBuf::from)
                .expect("VOICEFLOW_OWNED_AX_LEASE must name a private v1 lease");
            assert!(path.is_absolute());
            let metadata = fs::metadata(&path).expect("read private fixture lease metadata");
            assert!(metadata.is_file());
            assert_eq!(metadata.permissions().mode() & 0o077, 0);
            let path = path.canonicalize().expect("canonicalize lease path");
            let lease: Lease = serde_json::from_slice(&fs::read(&path).expect("read lease"))
                .expect("parse exact v1 lease schema");
            assert_eq!(lease.schema_version, 1);
            assert_eq!(lease.nonce.len(), 36);
            assert!(lease
                .nonce
                .chars()
                .all(|character| character.is_ascii_hexdigit() || character == '-'));
            assert_eq!(lease.token.len(), 64);
            assert!(lease
                .token
                .chars()
                .all(|character| character.is_ascii_hexdigit()));
            assert!(lease.executable_path.is_absolute());
            assert_eq!(
                lease.window_title,
                format!("VoiceFlow Owned AX {}", lease.nonce)
            );
            assert_eq!(
                lease.field_identifier,
                format!("voiceflow-owned-{}-target", lease.nonce)
            );
            assert_eq!(lease.field_description, lease.field_identifier);
            assert_eq!(
                lease.other_field_identifier,
                format!("voiceflow-owned-{}-other", lease.nonce)
            );
            assert!(matches!(
                lease.mode.as_str(),
                "direct" | "alternate" | "presentation-mismatch" | "missing-before"
            ));
            assert!((700..=1_500).contains(&lease.delay_ms));
            assert!(lease.selection_utf16.location >= 0);
            assert!(lease.selection_utf16.length >= 0);
            assert!(
                lease.expected_caret_utf16 <= lease.expected_after.encode_utf16().count() as u64
            );
            assert!(
                lease.expected_after.encode_utf16().count()
                    <= super::super::MAX_FULL_RANGE_READBACK_UTF16
            );
            let pinned_executable = std::env::var_os("VOICEFLOW_OWNED_AX_FIXTURE_EXECUTABLE")
                .map(PathBuf::from)
                .expect("runner must provide the pinned owned fixture executable");
            assert!(pinned_executable.is_absolute());
            assert_eq!(
                lease
                    .executable_path
                    .canonicalize()
                    .expect("canonicalize leased executable"),
                pinned_executable
                    .canonicalize()
                    .expect("canonicalize pinned fixture executable"),
                "lease must name the runner-pinned owned helper before any AX capture"
            );
            let directory = path.parent().expect("lease directory");
            for (candidate, name) in [
                (&lease.arm_request_path, "arm-request.json"),
                (&lease.arm_ack_path, "arm-ack.json"),
                (&lease.settled_path, "settled.json"),
                (&lease.result_path, "native-test-result.json"),
            ] {
                assert!(candidate.is_absolute());
                assert_eq!(
                    candidate
                        .parent()
                        .and_then(|parent| parent.canonicalize().ok()),
                    Some(directory.to_path_buf())
                );
                assert_eq!(
                    candidate.file_name().and_then(|item| item.to_str()),
                    Some(name)
                );
            }
            assert!(
                fixture_process_matches(&lease),
                "fixture process lease is stale"
            );
            (path, lease)
        }

        fn wait_for_json(path: &Path, lease: &Lease, timeout: Duration) -> serde_json::Value {
            let deadline = Instant::now() + timeout;
            loop {
                assert!(
                    fixture_process_matches(lease),
                    "fixture process identity changed"
                );
                if path.is_file() {
                    let receipt = read_json(path);
                    assert_eq!(receipt["nonce"], lease.nonce);
                    assert_eq!(receipt["token"], lease.token);
                    assert_eq!(receipt["pid"], lease.fixture_pid);
                    return receipt;
                }
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for fixture receipt"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        fn arm_owned_fixture(lease: &Lease) {
            let request = serde_json::json!({
                "schema_version": 1,
                "nonce": lease.nonce,
                "token": lease.token,
                "op": "arm"
            });
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lease.arm_request_path)
                .expect("exclusively create authenticated arm request");
            file.write_all(request.to_string().as_bytes())
                .expect("write arm request");
            file.sync_all().expect("sync arm request");
            let ack = wait_for_json(&lease.arm_ack_path, lease, Duration::from_secs(5));
            assert_eq!(ack["schema_version"], 1);
            assert_eq!(ack["op"], "arm");
        }

        #[test]
        #[ignore = "requires root-leased owned native AX fixture and explicit RUN handshake"]
        fn leased_fixture_full_range_readback_proves_exact_delayed_text() {
            let (_lease_path, lease) = load_lease();
            let (anchor, metadata) = FocusedFieldAnchor::capture_with_metadata(lease.fixture_pid)
                .expect("capture only the leased fixture's focused AX field");
            assert!(!metadata.secure_input);
            assert_eq!(
                metadata.window_title.as_deref(),
                Some(lease.window_title.as_str())
            );
            assert_eq!(
                metadata.identifier.as_deref(),
                Some(lease.field_identifier.as_str())
            );
            assert_eq!(
                metadata.description.as_deref(),
                Some(lease.field_description.as_str())
            );
            assert_eq!(metadata.ax_window_matches_app_focused_window, Some(true));
            assert_eq!(
                anchor.read_selected_range(),
                Some((lease.selection_utf16.location, lease.selection_utf16.length))
            );
            assert!(anchor.is_current_focus());
            assert!(delivery_target_is_frontmost(lease.fixture_pid));
            match lease.mode.as_str() {
                "direct" | "presentation-mismatch" => {
                    assert_eq!(anchor.read_value().as_deref(), Some(lease.before.as_str()));
                }
                "alternate" | "missing-before" => assert_eq!(anchor.read_value(), None),
                _ => unreachable!("lease validates the mode"),
            }

            arm_owned_fixture(&lease);
            if lease.mode == "alternate" {
                let settled = wait_for_json(&lease.settled_path, &lease, Duration::from_secs(5));
                assert_eq!(
                    settled["after_sha256"],
                    hex::encode(sha2::Sha256::digest(lease.expected_after.as_bytes()))
                );
                assert_eq!(settled["caret_utf16"], lease.expected_caret_utf16);
                assert_eq!(settled["other_unchanged"], true);
                assert!(anchor.is_current_focus());
                assert_eq!(anchor.read_value(), None);
                assert_eq!(
                    anchor.read_full_range_exact(
                        &lease.expected_after,
                        lease.expected_caret_utf16,
                    ),
                    FullRangeReadback::Verified,
                    "native full-range API can read the exact own-field value while AXValue is absent"
                );
                assert!(fixture_process_matches(&lease));
                write_native_test_receipt(
                    &lease,
                    "alternate_api_only",
                    None,
                    true,
                    anchor.read_selected_range(),
                );
                return;
            }

            std::thread::sleep(Duration::from_millis(POST_PASTE_READBACK_DELAY_MS));
            let expected_before = (lease.mode != "missing-before").then_some(lease.before.as_str());
            let mut full_range_calls = 0usize;
            let result = confirm_keyboard_paste_readback(
                PasteReadbackExpectation {
                    expected_after: Some(&lease.expected_after),
                    value_before: expected_before,
                    insertion_end_caret_utf16: Some(lease.expected_caret_utf16),
                },
                &CancellationToken::new(),
                || {
                    let target = fixture_process_matches(&lease)
                        && delivery_target_is_frontmost(lease.fixture_pid);
                    let field = target && anchor.current_focus_matches() == Some(true);
                    (target, field)
                },
                || anchor.read_value(),
                |expected, expected_caret| {
                    full_range_calls += 1;
                    anchor.read_full_range_exact(expected, expected_caret)
                },
                std::thread::sleep,
            );
            match lease.mode.as_str() {
                "presentation-mismatch" => {
                    assert_eq!(result.proof, Some(PasteVerificationFact::VerifiedFullRange));
                    assert!(result.full_range_attempted);
                    assert_eq!(full_range_calls, 1);
                    assert_eq!(
                        result.value_after.as_deref(),
                        Some(format!("{}\n", lease.expected_after).as_str()),
                        "primary AXValue is kept as observed; the LF is not normalized away"
                    );
                }
                "direct" => {
                    assert_eq!(result.proof, Some(PasteVerificationFact::Verified));
                    assert!(!result.full_range_attempted);
                    assert_eq!(full_range_calls, 0);
                }
                "missing-before" => {
                    assert_eq!(result.proof, None);
                    assert!(!result.full_range_attempted);
                    assert_eq!(full_range_calls, 0);
                }
                _ => unreachable!("lease validates the consumer mode"),
            }
            let settled = wait_for_json(&lease.settled_path, &lease, Duration::from_secs(5));
            assert_eq!(
                settled["after_sha256"],
                hex::encode(sha2::Sha256::digest(lease.expected_after.as_bytes()))
            );
            assert_eq!(settled["caret_utf16"], lease.expected_caret_utf16);
            assert_eq!(settled["other_unchanged"], true);
            assert!(fixture_process_matches(&lease));
            assert!(anchor.is_current_focus());
            let primary_matches = result
                .value_after
                .as_deref()
                .map(|value| value == lease.expected_after);
            write_native_test_receipt(
                &lease,
                match result.proof {
                    Some(PasteVerificationFact::Verified) => "verified_ax_value",
                    Some(PasteVerificationFact::VerifiedFullRange) => "verified_full_range",
                    _ => "unverified_missing_before",
                },
                primary_matches,
                result.full_range_attempted,
                anchor.read_selected_range(),
            );
        }

        fn write_native_test_receipt(
            lease: &Lease,
            outcome: &str,
            primary_matches_expected: Option<bool>,
            full_range_attempted: bool,
            selected_caret_utf16: Option<(i64, i64)>,
        ) {
            let receipt = serde_json::json!({
                "schema_version": 1,
                "nonce": lease.nonce,
                "token": lease.token,
                "pid": lease.fixture_pid,
                "mode": lease.mode,
                "outcome": outcome,
                "primary_matches_expected": primary_matches_expected,
                "full_range_attempted": full_range_attempted,
                "selected_caret_utf16": selected_caret_utf16,
                "no_keyboard_or_clipboard_action": true,
            });
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lease.result_path)
                .expect("exclusively create native test result receipt");
            file.write_all(receipt.to_string().as_bytes())
                .expect("write native test result");
            file.sync_all().expect("sync native test result");
        }
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

/// Recovery is a caller-selected delivery intent, not a permission inferred
/// from an error string or a target callback. Guarded fixtures may only write
/// through successful current admission; recoverable text retains ordinary UX.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecoveryIntent {
    PreserveText,
    NoImplicitCopy,
}
/// Guard polled during post-paste readback. It must stay cheap (no browser
/// queries); `after` runs once after the last read.
pub(crate) type ReadbackGuard =
    std::sync::Arc<dyn Fn() -> Result<(), PasteError> + Send + Sync + 'static>;

pub(crate) struct DeliveryPolicy<Before, After> {
    pub(crate) before: Before,
    pub(crate) after: After,
    /// Lightweight guard for the readback polling loop. `None` polls with
    /// `after`, which keeps owned replay, onboarding and other callers as is.
    pub(crate) readback: Option<ReadbackGuard>,
    pub(crate) recovery: RecoveryIntent,
}

/// The shared owner runs the attempt and decides whether its *prewrite* error
/// authorizes recovery effects. Cancellation, existing writes and current-copy
/// conflicts are never turned into a fresh implicit copy by this boundary.
fn attempt_with_recovery<T>(
    intent: RecoveryIntent,
    cancellation: &CancellationToken,
    trace: &mut DeliveryTrace,
    attempt: impl FnOnce(&mut DeliveryTrace) -> Result<T, PasteError>,
    recover: impl FnOnce(&mut DeliveryTrace),
) -> Result<T, PasteError> {
    let result = attempt(trace);
    if matches!(intent, RecoveryIntent::PreserveText)
        && !trace.clipboard_write_attempted
        && !cancellation.is_cancelled()
        && result
            .as_ref()
            .is_err_and(|error| !matches!(error, PasteError::Cancelled))
    {
        recover(trace);
    }
    result
}

fn admit_before_clipboard_write(
    accessibility: bool,
    cancellation: &CancellationToken,
    verify_target: impl FnOnce() -> Result<(), PasteError>,
    trace: &mut DeliveryTrace,
) -> Result<(), PasteError> {
    check_before_clipboard(cancellation)?;
    if !accessibility {
        trace.stage = "accessibility_guard";
        return Err(PasteError::Accessibility);
    }
    trace.stage = "target_guard";
    if let Err(error) = verify_target() {
        trace.target_verified = Some(false);
        return Err(error);
    }
    trace.target_verified = Some(true);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_timed_traced(
    app: &AppHandle,
    text: &str,
    accessibility: bool,
    cancellation: CancellationToken,
    verify_target: impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    verify_after: impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    verify_readback: impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    expected_pid: i32,
    trace: Option<crate::metrics::TraceHandle>,
) -> Result<TimedInsertOutcome, PasteError> {
    insert_with_post_verify_timed_traced(
        app,
        text,
        accessibility,
        cancellation,
        DeliveryPolicy {
            before: verify_target,
            after: verify_after,
            readback: Some(std::sync::Arc::new(verify_readback)),
            recovery: RecoveryIntent::PreserveText,
        },
        expected_pid,
        trace,
    )
}

pub(crate) fn insert_with_post_verify(
    app: &AppHandle,
    text: &str,
    accessibility: bool,
    cancellation: CancellationToken,
    verify_target: impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    verify_after: impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    expected_pid: i32,
) -> Result<InsertOutcome, PasteError> {
    Ok(insert_with_post_verify_timed(
        app,
        text,
        accessibility,
        cancellation,
        DeliveryPolicy {
            before: verify_target,
            after: verify_after,
            readback: None,
            recovery: RecoveryIntent::PreserveText,
        },
        expected_pid,
    )?
    .outcome)
}

pub(crate) fn insert_with_post_verify_timed(
    app: &AppHandle,
    text: &str,
    accessibility: bool,
    cancellation: CancellationToken,
    policy: DeliveryPolicy<
        impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
        impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    >,
    expected_pid: i32,
) -> Result<TimedInsertOutcome, PasteError> {
    insert_with_post_verify_timed_traced(
        app,
        text,
        accessibility,
        cancellation,
        policy,
        expected_pid,
        None,
    )
}

pub(crate) fn insert_with_post_verify_timed_traced(
    app: &AppHandle,
    text: &str,
    accessibility: bool,
    cancellation: CancellationToken,
    policy: DeliveryPolicy<
        impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
        impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    >,
    expected_pid: i32,
    trace: Option<crate::metrics::TraceHandle>,
) -> Result<TimedInsertOutcome, PasteError> {
    insert_with_post_verify_timed_effects(
        DeliveryEffects::production(app.clone()),
        text,
        accessibility,
        cancellation,
        policy,
        expected_pid,
        trace,
    )
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
struct RegisteredResearchPasteRequestV1 {
    host_identity: RegisteredResearchHostIdentityV1,
    expected_bundle_id: String,
    expected_window_title: String,
    expected_control_identifier: String,
    expires_at: std::time::Instant,
    cancellation: CancellationToken,
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
pub(crate) fn insert_registered_research_owned_paste_remote(
    text: &str,
    host_identity: RegisteredResearchHostIdentityV1,
    remote: RegisteredOwnedPasteSessionRemoteContextV1,
    expires_at: std::time::Instant,
    cancellation: CancellationToken,
) -> RegisteredResearchPasteAttempt {
    if text != registered_remote_temporary_text(RemoteClipboardBoardV1::GeneralClipboard) {
        return RegisteredResearchPasteAttempt {
            attempt: Err(PasteError::TargetUnavailable),
            owner_completion: AnchorOwnerCompletion::not_started(),
            observation_ticket: None,
            undo_ticket: None,
        };
    }
    let effects = match DeliveryEffects::research_registered_owned_remote(remote) {
        Ok(effects) => effects,
        Err(error) => {
            return RegisteredResearchPasteAttempt {
                attempt: Err(error),
                owner_completion: AnchorOwnerCompletion::not_started(),
                observation_ticket: None,
                undo_ticket: None,
            };
        }
    };
    insert_registered_research_owned_paste_with_effects(
        text,
        RegisteredResearchPasteRequestV1 {
            host_identity,
            expected_bundle_id: REGISTERED_RESEARCH_HOST_BUNDLE_ID.to_owned(),
            expected_window_title: REGISTERED_RESEARCH_HOST_WINDOW_TITLE.to_owned(),
            expected_control_identifier: REGISTERED_RESEARCH_HOST_CONTROL_ID.to_owned(),
            expires_at,
            cancellation,
        },
        effects,
    )
}

#[cfg(all(target_os = "macos", feature = "research-harness"))]
fn insert_registered_research_owned_paste_with_effects(
    text: &str,
    request: RegisteredResearchPasteRequestV1,
    effects: DeliveryEffects,
) -> RegisteredResearchPasteAttempt {
    let RegisteredResearchPasteRequestV1 {
        host_identity,
        expected_bundle_id,
        expected_window_title,
        expected_control_identifier,
        expires_at,
        cancellation,
    } = request;
    let invalid_request = host_identity.pid <= 0
        || host_identity.parent_pid <= 0
        || host_identity.parent_pid == host_identity.pid
        || host_identity.executable_path.as_os_str().is_empty()
        || !host_identity.executable_path.is_absolute()
        || expected_bundle_id != REGISTERED_RESEARCH_HOST_BUNDLE_ID
        || expected_window_title != REGISTERED_RESEARCH_HOST_WINDOW_TITLE
        || expected_control_identifier != REGISTERED_RESEARCH_HOST_CONTROL_ID;
    if invalid_request {
        return RegisteredResearchPasteAttempt {
            attempt: Err(PasteError::TargetUnavailable),
            owner_completion: AnchorOwnerCompletion::not_started(),
            observation_ticket: None,
            undo_ticket: None,
        };
    }
    if let Err(error) = registered_research_permission(&cancellation, expires_at) {
        return RegisteredResearchPasteAttempt {
            attempt: Err(error),
            owner_completion: AnchorOwnerCompletion::not_started(),
            observation_ticket: None,
            undo_ticket: None,
        };
    }

    let text = text.to_owned();
    let host_identity_for_owner = host_identity.clone();
    let owner_effects = effects;
    let owner_cancellation = cancellation.clone();
    let ticket_handoff = std::sync::Arc::new(Mutex::new(ResearchPasteTicketHandoff::default()));
    let owner_ticket_handoff = std::sync::Arc::clone(&ticket_handoff);
    let (owner_result, owner_completion) = run_delivery_owner_with_completion(move |undo_lease| {
        let initial_target = capture_registered_research_target(
            &host_identity_for_owner,
            &expected_bundle_id,
            &expected_window_title,
            &expected_control_identifier,
            &owner_cancellation,
            expires_at,
        )?;
        let research_guard = RegisteredResearchPasteGuard {
            host_identity: host_identity_for_owner.clone(),
            expected_bundle_id: expected_bundle_id.clone(),
            expected_window_title: expected_window_title.clone(),
            expected_control_identifier: expected_control_identifier.clone(),
            expires_at,
            cancellation: owner_cancellation.clone(),
            initial_target,
        };
        let before_guard = research_guard.clone();
        let after_guard = research_guard.clone();
        let readback_guard = research_guard.clone();
        insert_with_post_verify_timed_on_owner(
            &owner_effects,
            &text,
            true,
            owner_cancellation,
            DeliveryPolicy {
                before: move || before_guard.check(),
                after: move || after_guard.check(),
                readback: Some(std::sync::Arc::new(move || readback_guard.check())),
                recovery: RecoveryIntent::NoImplicitCopy,
            },
            undo_lease,
            TimedInsertOwnerMetadata {
                expected_pid: host_identity_for_owner.pid,
                trace_handle: None,
                research_guard: Some(research_guard),
                ticket_handoff: Some(owner_ticket_handoff),
            },
        )
    });
    let attempt = match owner_result {
        Ok(attempt) => attempt,
        Err(error) => Err(PasteError::Input(error)),
    };
    let mut ticket_handoff = ticket_handoff
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    RegisteredResearchPasteAttempt {
        attempt,
        owner_completion,
        observation_ticket: ticket_handoff.observation_ticket.take(),
        undo_ticket: ticket_handoff.undo_ticket.take(),
    }
}

pub(crate) fn insert_with_post_verify_timed_effects(
    effects: DeliveryEffects,
    text: &str,
    accessibility: bool,
    cancellation: CancellationToken,
    policy: DeliveryPolicy<
        impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
        impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    >,
    expected_pid: i32,
    trace: Option<crate::metrics::TraceHandle>,
) -> Result<TimedInsertOutcome, PasteError> {
    let text = text.to_owned();
    run_delivery_owner(move |undo_lease| {
        insert_with_post_verify_timed_on_owner(
            &effects,
            &text,
            accessibility,
            cancellation,
            policy,
            undo_lease,
            TimedInsertOwnerMetadata {
                expected_pid,
                trace_handle: trace,
                #[cfg(all(target_os = "macos", feature = "research-harness"))]
                research_guard: None,
                #[cfg(all(target_os = "macos", feature = "research-harness"))]
                ticket_handoff: None,
            },
        )
    })
    .map_err(PasteError::Input)?
}

struct TimedInsertOwnerMetadata {
    expected_pid: i32,
    trace_handle: Option<crate::metrics::TraceHandle>,
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    research_guard: Option<RegisteredResearchPasteGuard>,
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    ticket_handoff: Option<std::sync::Arc<Mutex<ResearchPasteTicketHandoff>>>,
}

fn insert_with_post_verify_timed_on_owner(
    effects: &DeliveryEffects,
    text: &str,
    accessibility: bool,
    cancellation: CancellationToken,
    policy: DeliveryPolicy<
        impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
        impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    >,
    undo_lease: &mut Option<AnchorLease<FocusedFieldAnchor>>,
    metadata: TimedInsertOwnerMetadata,
) -> Result<TimedInsertOutcome, PasteError> {
    let TimedInsertOwnerMetadata {
        expected_pid,
        trace_handle,
        #[cfg(all(target_os = "macos", feature = "research-harness"))]
        research_guard,
        #[cfg(all(target_os = "macos", feature = "research-harness"))]
        ticket_handoff,
    } = metadata;
    let mut trace = DeliveryTrace::new(accessibility, expected_pid);
    let result = (|| {
        #[cfg(all(target_os = "macos", feature = "research-harness"))]
        if research_guard.is_some() != ticket_handoff.is_some() {
            return Err(PasteError::Input(
                "research ticket handoff is unavailable".to_owned(),
            ));
        }
        #[cfg(all(target_os = "macos", feature = "research-harness"))]
        if let Some(research_guard) = research_guard.as_ref() {
            research_guard.check()?;
        }
        let cancellation_for_resolution = cancellation.clone();
        let clipboard_snapshot = match ClipboardSnapshot::capture(effects) {
            Ok(snapshot) => {
                trace.clipboard_snapshot_captured = true;
                snapshot
            }
            Err(error) => {
                trace.stage = "clipboard_snapshot";
                let diagnostic = trace.for_error(&error);
                effects.record_diagnostic(&diagnostic);
                return Err(PasteError::Diagnosed {
                    source: Box::new(error),
                    diagnostic: Box::new(diagnostic),
                });
            }
        };
        match clipboard_snapshot.capture_fresh_unchanged() {
            Ok(Some(false)) => {
                trace.stage = "clipboard_snapshot";
                trace.clipboard_unchanged_before_write = Some(false);
                let error = PasteError::Clipboard("named clipboard changed before paste".into());
                let diagnostic = trace.for_error(&error);
                effects.record_diagnostic(&diagnostic);
                return Err(PasteError::Diagnosed {
                    source: Box::new(error),
                    diagnostic: Box::new(diagnostic),
                });
            }
            Ok(Some(true) | None) => {}
            Err(error) => {
                trace.stage = "clipboard_snapshot";
                let diagnostic = trace.for_error(&error);
                effects.record_diagnostic(&diagnostic);
                return Err(PasteError::Diagnosed {
                    source: Box::new(error),
                    diagnostic: Box::new(diagnostic),
                });
            }
        }
        let result = attempt_with_recovery(
            policy.recovery,
            &cancellation_for_resolution,
            &mut trace,
            |trace| {
                insert_with_post_verify_timed_inner(
                    effects,
                    text,
                    accessibility,
                    cancellation,
                    policy.before,
                    policy.after,
                    policy.readback,
                    expected_pid,
                    &clipboard_snapshot,
                    trace,
                    undo_lease,
                    trace_handle.clone(),
                    #[cfg(all(target_os = "macos", feature = "research-harness"))]
                    research_guard.as_ref(),
                    #[cfg(all(target_os = "macos", feature = "research-harness"))]
                    ticket_handoff.clone(),
                )
            },
            |trace| {
                let unchanged = clipboard_snapshot
                    .unchanged_since_capture(effects)
                    .unwrap_or(false);
                trace.clipboard_unchanged_before_write = Some(unchanged);
                if unchanged && crate::research_trace::allowed() {
                    trace.clipboard_write_attempted = true;
                    if let Some(research) = crate::research_trace::current() {
                        research.begin_clipboard_write();
                    }
                    let ownership_token = clipboard_ownership_token();
                    if clipboard_snapshot
                        .write_temporary_text(effects, text, &ownership_token)
                        .is_ok()
                    {
                        trace.clipboard_marker = clipboard_snapshot
                            .marker_after_write(effects, text, &ownership_token)
                            .ok()
                            .flatten();
                        trace.refresh_clipboard_ownership(&clipboard_snapshot, effects);
                    } else {
                        trace.clipboard_write_failed = true;
                        trace.clipboard_write_owned = Some(false);
                    }
                }
            },
        );
        match result {
            Ok(mut outcome) => {
                outcome.clipboard_restoration = restoration_fact(
                    trace.clipboard_write_attempted,
                    trace.clipboard_write_failed,
                    trace.clipboard_write_owned,
                    trace.clipboard_restore_attempted,
                    trace.clipboard_restored,
                );
                trace.paste_verified = Some(outcome.outcome.verified);
                if cancellation_for_resolution.is_cancelled() && trace.clipboard_write_attempted {
                    let (error, diagnostic) = resolve_post_write_cancellation(
                        effects,
                        &clipboard_snapshot,
                        &mut trace,
                        outcome.outcome.shortcut_sent,
                    );
                    if !matches!(&error, PasteError::Cancelled) {
                        effects.record_diagnostic(&diagnostic);
                    }
                    return Err(error);
                }
                if !outcome.outcome.verified {
                    trace.stage = "paste_verification";
                    trace.refresh_clipboard_ownership(&clipboard_snapshot, effects);
                    let diagnostic = trace.for_unverified_paste();
                    effects.record_diagnostic(&diagnostic);
                    outcome.diagnostic = Some(diagnostic);
                }
                Ok(outcome)
            }
            Err(error)
                if cancellation_for_resolution.is_cancelled()
                    || matches!(&error, PasteError::Cancelled) =>
            {
                if !trace.clipboard_write_attempted {
                    return Err(PasteError::Cancelled);
                }
                let shortcut_may_have_been_posted = trace.keyboard_paste_may_have_been_posted;
                let (error, diagnostic) = resolve_post_write_cancellation(
                    effects,
                    &clipboard_snapshot,
                    &mut trace,
                    shortcut_may_have_been_posted,
                );
                if !matches!(&error, PasteError::Cancelled) {
                    effects.record_diagnostic(&diagnostic);
                }
                Err(error)
            }
            Err(error) => {
                trace.refresh_clipboard_ownership(&clipboard_snapshot, effects);
                let diagnostic = trace.for_error(&error);
                effects.record_diagnostic(&diagnostic);
                Err(PasteError::Diagnosed {
                    source: Box::new(error),
                    diagnostic: Box::new(diagnostic),
                })
            }
        }
    })();
    trace.emit_research_resolution();
    result
}

fn resolve_post_write_cancellation(
    effects: &DeliveryEffects,
    clipboard_snapshot: &ClipboardSnapshot,
    trace: &mut DeliveryTrace,
    shortcut_may_have_been_posted: bool,
) -> (PasteError, DeliveryDiagnostic) {
    trace.stage = "cancelled_after_clipboard_write";
    trace.refresh_clipboard_ownership_at(
        clipboard_snapshot,
        effects,
        if shortcut_may_have_been_posted {
            ClipboardMarkerCheckStageV1::PostCallback
        } else {
            ClipboardMarkerCheckStageV1::PreRestore
        },
        u8::from(shortcut_may_have_been_posted),
    );
    let marker = trace.clipboard_marker.clone();
    restore_cancelled_clipboard_if_safe(trace, shortcut_may_have_been_posted, || {
        clipboard_snapshot.restore_if_unchanged(effects, marker.as_ref())
    });
    let diagnostic = if trace.keyboard_paste_may_have_been_posted {
        if trace.paste_verified == Some(true) {
            trace.diagnostic(
                "delivery_cancelled",
                "粘贴已确认，听写结果已保存在历史记录中",
            )
        } else {
            trace.for_unverified_paste()
        }
    } else if trace.clipboard_restored == Some(true) {
        trace.diagnostic(
            "delivery_cancelled",
            "听写已取消，原剪贴板已恢复；听写文字已保存在历史记录中",
        )
    } else if trace.clipboard_restore_attempted && trace.clipboard_restored.is_none() {
        trace.diagnostic(
            "clipboard_ownership_unverified",
            "无法确认剪贴板是否恢复；请从历史记录复制听写文字",
        )
    } else {
        match trace.clipboard_write_owned {
            Some(false) => trace.diagnostic(
                "clipboard_changed",
                "剪贴板已被其他应用更新，已保留较新的内容；请从历史记录复制听写文字",
            ),
            None => trace.diagnostic(
                "clipboard_ownership_unverified",
                "无法确认剪贴板内容；请从历史记录复制听写文字",
            ),
            Some(true) => trace.diagnostic(
                "paste_unverified",
                "听写已取消，文字仍在剪贴板并已保存在历史记录中",
            ),
        }
    };
    (
        PasteError::Diagnosed {
            source: Box::new(PasteError::Cancelled),
            diagnostic: Box::new(diagnostic.clone()),
        },
        diagnostic,
    )
}

/// Run one paste boundary through the shared timing owner used by native
/// submission/readback and fake-effect tests. It measures only the supplied
/// callback; it does not claim an offset on any child or external clock.
pub(crate) fn run_traced_boundary<T>(
    trace: Option<&crate::metrics::TraceHandle>,
    stage: crate::metrics::Stage,
    operation: impl FnOnce() -> T,
) -> (T, Duration) {
    let started = std::time::Instant::now();
    let mut span = trace.map(|trace| trace.begin(stage));
    let result = operation();
    if let Some(span) = &mut span {
        span.finish();
    }
    (result, started.elapsed())
}

pub(crate) fn run_traced_confirmation<T, E>(
    trace: Option<&crate::metrics::TraceHandle>,
    submitted: Result<(), E>,
    readback: impl FnOnce() -> Result<T, E>,
) -> (Result<T, E>, Duration) {
    // A confirmation boundary exists only once the submitted shortcut was
    // accepted and the readback callback is actually reached.
    match submitted {
        Ok(()) => run_traced_boundary(trace, crate::metrics::Stage::TargetConfirmation, readback),
        Err(error) => (Err(error), Duration::ZERO),
    }
}

/// Polling guard choice: the lightweight readback guard when the caller
/// supplied one, otherwise the final `after` guard.
fn readback_guard_passes(
    readback: Option<&(dyn Fn() -> Result<(), PasteError> + Send + Sync)>,
    after: &impl Fn() -> Result<(), PasteError>,
) -> bool {
    match readback {
        Some(guard) => guard().is_ok(),
        None => after().is_ok(),
    }
}

#[allow(clippy::too_many_arguments)]
fn insert_with_post_verify_timed_inner(
    effects: &DeliveryEffects,
    text: &str,
    accessibility: bool,
    cancellation: CancellationToken,
    verify_target: impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    verify_after: impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    verify_readback: Option<ReadbackGuard>,
    expected_pid: i32,
    clipboard_snapshot: &ClipboardSnapshot,
    trace: &mut DeliveryTrace,
    undo_lease: &mut Option<AnchorLease<FocusedFieldAnchor>>,
    trace_handle: Option<crate::metrics::TraceHandle>,
    #[cfg(all(target_os = "macos", feature = "research-harness"))] research_guard: Option<
        &RegisteredResearchPasteGuard,
    >,
    #[cfg(all(target_os = "macos", feature = "research-harness"))] ticket_handoff: Option<
        std::sync::Arc<Mutex<ResearchPasteTicketHandoff>>,
    >,
) -> Result<TimedInsertOutcome, PasteError> {
    // A clipboard write is itself a user-visible delivery side effect. Do not
    // let a failed permission or target check overwrite the user's existing
    // clipboard before the keyboard-injection guard runs.
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    let registered_remote_post = effects.is_registered_owned_remote();
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    if registered_remote_post && research_guard.is_none() {
        return Err(PasteError::TargetUnavailable);
    }
    admit_before_clipboard_write(accessibility, &cancellation, &verify_target, trace)?;
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    let mut focus_anchor = if let Some(research_guard) = research_guard {
        let (anchor, metadata) = FocusedFieldAnchor::capture_with_metadata(expected_pid)
            .map_err(|_| PasteError::InputUnavailable)?;
        if !registered_research_field_metadata_matches(
            metadata.window_title.as_deref(),
            metadata.identifier.as_deref(),
            metadata.secure_input,
            &research_guard.expected_window_title,
            &research_guard.expected_control_identifier,
        ) {
            return Err(if metadata.secure_input {
                PasteError::SecureInput
            } else {
                PasteError::InputChanged
            });
        }
        // This is the sole AX anchor retained for the source value, paste
        // readback, ConfirmedInsertion, and both bounded tickets.
        research_guard.check()?;
        if anchor.current_focus_matches() != Some(true) {
            return Err(PasteError::InputChanged);
        }
        Some(anchor)
    } else {
        FocusedFieldAnchor::capture(expected_pid)
    };
    #[cfg(not(all(target_os = "macos", feature = "research-harness")))]
    let mut focus_anchor = FocusedFieldAnchor::capture(expected_pid);
    trace.focused_input_available = Some(
        focus_anchor
            .as_ref()
            .is_some_and(FocusedFieldAnchor::is_current_focus),
    );
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    let (value_before, selection_range) = if research_guard.is_some() {
        let anchor = focus_anchor.as_ref().ok_or(PasteError::InputUnavailable)?;
        if anchor.current_focus_matches() != Some(true) {
            return Err(PasteError::InputChanged);
        }
        let value_before = anchor.read_value().ok_or(PasteError::InputUnavailable)?;
        if anchor.current_focus_matches() != Some(true) {
            return Err(PasteError::InputChanged);
        }
        if let Some(research_guard) = research_guard {
            research_guard.check()?;
        }
        let selection_range = anchor.read_selected_range();
        if anchor.current_focus_matches() != Some(true) {
            return Err(PasteError::InputChanged);
        }
        if let Some(research_guard) = research_guard {
            research_guard.check()?;
        }
        (Some(value_before), selection_range)
    } else {
        (
            focus_anchor
                .as_ref()
                .and_then(FocusedFieldAnchor::read_value),
            focus_anchor
                .as_ref()
                .and_then(FocusedFieldAnchor::read_selected_range),
        )
    };
    #[cfg(not(all(target_os = "macos", feature = "research-harness")))]
    let (value_before, selection_range) = (
        focus_anchor
            .as_ref()
            .and_then(FocusedFieldAnchor::read_value),
        focus_anchor
            .as_ref()
            .and_then(FocusedFieldAnchor::read_selected_range),
    );
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    if registered_remote_post {
        with_registered_remote_initial_source(value_before.as_deref(), selection_range, || ())?;
    }
    let expected_after =
        expected_splice_after_insert(value_before.as_deref(), selection_range, text);
    // An empty insert cannot be told apart from "nothing happened".
    let expected_caret_utf16 =
        expected_insert_caret_utf16(value_before.as_deref(), selection_range, text)
            .filter(|_| !(text.is_empty() && expected_after.as_deref() == value_before.as_deref()));
    check_before_clipboard(&cancellation)?;
    if cancellation.is_cancelled() {
        return Err(PasteError::Cancelled);
    }
    if focus_anchor
        .as_ref()
        .is_some_and(|anchor| !anchor.is_current_focus())
    {
        return Err(PasteError::InputChanged);
    }
    // Preserve every pasteboard flavor before writing the temporary text. If
    // another process changed it since the snapshot, stop rather than
    // overwrite the newer clipboard contents.
    trace.stage = "clipboard_snapshot";
    if !clipboard_snapshot.unchanged_since_capture(effects)? {
        trace.clipboard_unchanged_before_write = Some(false);
        return Err(PasteError::Clipboard(
            "clipboard changed before paste".into(),
        ));
    }
    trace.clipboard_unchanged_before_write = Some(true);
    check_before_clipboard(&cancellation)?;
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    if let Some(research_guard) = research_guard {
        research_guard.check()?;
    }
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    if registered_remote_post {
        verify_registered_remote_source_on_anchor(focus_anchor.as_ref())?;
        research_guard
            .ok_or(PasteError::TargetUnavailable)?
            .check()?;
    }
    let ownership_token = clipboard_ownership_token();
    trace.stage = "clipboard_write";
    trace.clipboard_write_attempted = true;
    if let Some(research) = crate::research_trace::current() {
        research.begin_clipboard_write();
    }
    if let Err(error) = clipboard_snapshot.write_temporary_text(effects, text, &ownership_token) {
        trace.clipboard_write_failed = true;
        trace.clipboard_marker = clipboard_snapshot
            .marker_after_write(effects, text, &ownership_token)
            .ok()
            .flatten();
        trace.refresh_clipboard_ownership(clipboard_snapshot, effects);
        return Err(error);
    }
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    if let Some(research_guard) = research_guard {
        research_guard.check()?;
    }
    trace.stage = "clipboard_ownership";
    let clipboard_marker = clipboard_snapshot
        .marker_after_write(effects, text, &ownership_token)?
        .ok_or_else(|| {
            PasteError::Clipboard("clipboard write ownership could not be verified".into())
        })?;
    trace.clipboard_write_owned = Some(true);
    trace.clipboard_marker = Some(clipboard_marker.clone());
    thread::sleep(Duration::from_millis(100));
    if cancellation.is_cancelled() {
        return Err(PasteError::Cancelled);
    }
    // Fall through to Cmd+V with a CJK→ABC input-source switch.
    // Suppress the modifier event-tap while we synthesize the paste keystroke:
    // our own keystroke must not be read as a physical hotkey tap, and re-entrant
    // event delivery during the paste aborts the main runloop (uncaught
    // NSException -> SIGABRT). Enigo is called synchronously here, but this
    // function is reached from `paste_text`'s `spawn_blocking` worker, never from
    // the AppKit/tao main-thread callback.
    trace.stage = "pre_paste_guard";
    let failed_pre_paste_stage = std::cell::Cell::new("shortcut_preparation");
    let verify_target_before_keyboard = || {
        failed_pre_paste_stage.set("target_recheck");
        verify_target()?;
        failed_pre_paste_stage.set("focused_input_recheck");
        verify_focused_field_before_paste(
            focus_anchor.as_ref(),
            value_before.as_deref(),
            selection_range,
        )?;
        failed_pre_paste_stage.set("pre_paste_clipboard_guard");
        if !clipboard_snapshot.marker_is_current(
            effects,
            &clipboard_marker,
            ClipboardMarkerCheckStageV1::PreBoundary,
            0,
        )? {
            return Err(PasteError::Clipboard(
                "clipboard changed before keyboard paste".into(),
            ));
        }
        failed_pre_paste_stage.set("system_secure_input_guard");
        secure_input_admission_with_probe(
            &SECURE_INPUT_PROBE_LOCK,
            system_secure_input_probe_unserialized,
            &cancellation,
            expected_pid,
        )?;
        Ok(())
    };
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    let verify_registered_target_after_post_ack = || {
        if registered_remote_post {
            verify_registered_remote_source_on_anchor(focus_anchor.as_ref())?;
            return research_guard.ok_or(PasteError::TargetUnavailable)?.check();
        }
        Ok(())
    };
    #[cfg(not(all(target_os = "macos", feature = "research-harness")))]
    let verify_registered_target_after_post_ack = || Ok(());
    trace.stage = "keyboard_paste";
    let (attempt, paste_submission) =
        run_traced_boundary(trace_handle.as_ref(), crate::metrics::Stage::Paste, || {
            // Let the input-source owner settle before the final target,
            // focused-field, selection, and clipboard admission performed
            // inside run_paste_attempt. Acquiring inside simulate_paste would
            // leave admission before the layout switch and create a race.
            #[cfg(target_os = "macos")]
            let latin_layout = crate::input_source::AbcLayoutGuard::acquire();
            #[cfg(target_os = "macos")]
            let prepared_shortcut = prepare_command_shortcut(COMMAND_PASTE_KEYCODE);
            #[cfg(not(target_os = "macos"))]
            let prepared_shortcut = prepare_command_shortcut(0);
            let attempt = match prepared_shortcut {
                Err(error) => PasteAttempt::not_sent(error),
                Ok(prepared_shortcut) => run_paste_attempt_with_post_ack_recheck(
                    &cancellation,
                    verify_target_before_keyboard,
                    || match effects.request_shortcut_post(&clipboard_marker) {
                        ClipboardPostPermitResultV1::Allowed => Ok(()),
                        #[cfg(all(target_os = "macos", feature = "research-harness"))]
                        ClipboardPostPermitResultV1::RecordedOnly => Err(PasteError::Clipboard(
                            "recorded-only permit cannot authorize a target post".into(),
                        )),
                        #[cfg(all(target_os = "macos", feature = "research-harness"))]
                        ClipboardPostPermitResultV1::Denied => Err(PasteError::Clipboard(
                            "named clipboard post permit denied".into(),
                        )),
                        #[cfg(all(target_os = "macos", feature = "research-harness"))]
                        ClipboardPostPermitResultV1::AcknowledgementUnavailable => {
                            Err(PasteError::Clipboard(
                                "named clipboard post permit acknowledgement unavailable".into(),
                            ))
                        }
                        #[cfg(all(target_os = "macos", feature = "research-harness"))]
                        ClipboardPostPermitResultV1::ProtocolRejected => {
                            Err(PasteError::Clipboard(
                                "named clipboard post permit was rejected".into(),
                            ))
                        }
                        #[cfg(all(target_os = "macos", feature = "research-harness"))]
                        ClipboardPostPermitResultV1::Unknown => Err(PasteError::Clipboard(
                            "named clipboard post permit is unknown".into(),
                        )),
                    },
                    verify_registered_target_after_post_ack,
                    move |mark_may_have_posted| {
                        if !crate::research_trace::allowed() {
                            return Err(PasteError::Cancelled);
                        }
                        #[cfg(all(target_os = "macos", feature = "research-harness"))]
                        if registered_remote_post {
                            let registered_guard =
                                research_guard.ok_or(PasteError::TargetUnavailable)?;
                            let mut mark_registered_post = || {
                                if let Some(research) = crate::research_trace::current() {
                                    research.keyboard();
                                }
                                mark_may_have_posted();
                            };
                            return simulate_paste_once_to_registered_pid(
                                expected_pid,
                                prepared_shortcut,
                                &registered_guard.cancellation,
                                registered_guard.expires_at,
                                &mut mark_registered_post,
                            );
                        }
                        if let Some(research) = crate::research_trace::current() {
                            research.keyboard();
                        }
                        simulate_paste_once(expected_pid, prepared_shortcut, mark_may_have_posted)
                    },
                ),
            };
            #[cfg(target_os = "macos")]
            drop(latin_layout);
            attempt
        });
    trace.stage = if attempt.shortcut_sent {
        "keyboard_paste"
    } else {
        failed_pre_paste_stage.get()
    };
    trace.keyboard_paste_attempted = attempt.shortcut_sent;
    trace.keyboard_paste_may_have_been_posted = attempt.shortcut_sent;
    if attempt.shortcut_sent {
        // An invoked shortcut is an irreversible attempt. Keep its result
        // unknown until the single readback below proves an exact splice.
        trace.paste_verification = Some(PasteVerificationFact::ShortcutOutcomeUnknown);
    }
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    if let Some(research_guard) = research_guard {
        research_guard.check()?;
    }
    trace.clipboard_write_owned = clipboard_snapshot
        .marker_is_current(
            effects,
            &clipboard_marker,
            ClipboardMarkerCheckStageV1::PostCallback,
            u8::from(attempt.shortcut_sent),
        )
        .ok();
    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    if let Some(research_guard) = research_guard {
        research_guard.check()?;
    }
    if let Err(error) = &attempt.result {
        match error {
            PasteError::TargetChanged
            | PasteError::TargetUnavailable
            | PasteError::BrowserAccessRequired => trace.target_verified = Some(false),
            PasteError::InputUnavailable
            | PasteError::InputChanged
            | PasteError::SelectionUnavailable
            | PasteError::SelectionChanged => trace.focused_input_available = Some(false),
            _ => {}
        }
    }
    let (readback_result, readback_confirmation) =
        run_traced_confirmation(trace_handle.as_ref(), attempt.result, || {
            trace.stage = "paste_verification";
            trace.paste_verification_delay_ms = Some(POST_PASTE_READBACK_DELAY_MS);
            thread::sleep(Duration::from_millis(POST_PASTE_READBACK_DELAY_MS));
            Ok({
                let readback = confirm_keyboard_paste_readback(
                    PasteReadbackExpectation {
                        expected_after: expected_after.as_deref(),
                        value_before: value_before.as_deref(),
                        insertion_end_caret_utf16: expected_caret_utf16,
                    },
                    &cancellation,
                    || {
                        let target_matches = delivery_target_is_frontmost(expected_pid)
                            && readback_guard_passes(verify_readback.as_deref(), &verify_after);
                        let focused_matches = target_matches
                            && focus_anchor
                                .as_ref()
                                .is_some_and(FocusedFieldAnchor::is_current_focus);
                        (target_matches, focused_matches)
                    },
                    || {
                        focus_anchor
                            .as_ref()
                            .filter(|anchor| anchor.is_current_focus())
                            .and_then(FocusedFieldAnchor::read_value)
                    },
                    |expected, expected_caret| {
                        focus_anchor
                            .as_ref()
                            .map(|anchor| anchor.read_full_range_exact(expected, expected_caret))
                            .unwrap_or(FullRangeReadback::Unavailable)
                    },
                    thread::sleep,
                );
                trace.paste_readback_poll_count = Some(readback.poll_count);
                trace.paste_readback_elapsed_ms =
                    Some(u64::try_from(readback.elapsed.as_millis()).unwrap_or(u64::MAX));
                trace.paste_expected_utf16_length = readback.expected_utf16_length;
                trace.paste_actual_utf16_length = readback.actual_utf16_length;
                trace.paste_readback_mismatch_kind = readback.mismatch_kind;
                trace.paste_full_range_attempted = readback.full_range_attempted;
                // Recheck both identities after the last read. A late value must not
                // authorize delivery if focus or the target moved during observation.
                let target_still_matches =
                    delivery_target_is_frontmost(expected_pid) && verify_after().is_ok();
                let focused_field_still_same = target_still_matches
                    && focus_anchor
                        .as_ref()
                        .is_some_and(FocusedFieldAnchor::is_current_focus);
                trace.target_verified = Some(target_still_matches);
                trace.focused_input_available = Some(focused_field_still_same);
                let verification = classify_confirmed_paste_readback(
                    !cancellation.is_cancelled(),
                    readback.proof,
                    expected_after.as_deref(),
                    value_before.as_deref(),
                    readback.value_after.as_deref(),
                    target_still_matches,
                    focused_field_still_same,
                );
                trace.paste_verification = Some(verification);
                let mut outcome = build_insert_outcome(true, readback.value_after, verification);
                // Bind Undo to the field that accepted Cmd+V, not the field focused
                // when recording started. The original AX anchor brackets the probe
                // so moving focus during readback cannot arm a different field.
                if outcome.verification == Some(PasteVerificationFact::Verified) {
                    if let Some(anchor) = focus_anchor.as_ref().filter(|a| a.is_current_focus()) {
                        #[cfg(all(target_os = "macos", feature = "research-harness"))]
                        let guard = research_guard
                            .map(|research_guard| research_guard.initial_target.clone())
                            .unwrap_or_else(crate::context::probe_focus_guard);
                        #[cfg(not(all(target_os = "macos", feature = "research-harness")))]
                        let guard = crate::context::probe_focus_guard();
                        let fingerprint = anchor.read_value().as_deref().map(selection_fingerprint);
                        #[cfg(all(target_os = "macos", feature = "research-harness"))]
                        let research_after_fingerprint =
                            research_guard.is_none_or(|_| verify_after().is_ok());
                        #[cfg(not(all(target_os = "macos", feature = "research-harness")))]
                        let research_after_fingerprint = true;
                        #[cfg(all(target_os = "macos", feature = "research-harness"))]
                        let research_before_ticket = research_guard
                            .is_none_or(|research_guard| research_guard.check().is_ok());
                        #[cfg(not(all(target_os = "macos", feature = "research-harness")))]
                        let research_before_ticket = true;
                        let focus_still_current_before_ticket = anchor.is_current_focus();
                        if focus_still_current_before_ticket
                            && fingerprint == outcome.post_insert_input_fingerprint
                            && guard.pid == expected_pid
                            && crate::context::same_field_mismatch_reason(&guard, &guard, false)
                                .is_none()
                            && research_after_fingerprint
                            && research_before_ticket
                        {
                            let confirmed = value_before
                                .as_deref()
                                .zip(outcome.value_after.as_deref())
                                .and_then(|(before, after)| {
                                    confirmed_insertion_v1(
                                        before,
                                        selection_range,
                                        text,
                                        after,
                                        guard.clone(),
                                    )
                                });
                            // Move the exact original AX reference into the owner
                            // loop; never recapture focus. The short single-use
                            // Undo ticket and repeated read capability have
                            // separate absolute deadlines.
                            if let Some(original_anchor) = focus_anchor.take() {
                                let (ticket, observation, lease) = if let Some(mut confirmed) =
                                    confirmed
                                {
                                    #[cfg(all(target_os = "macos", feature = "research-harness"))]
                                    let (undo_lifetime, observation_lifetime) = if let Some(guard) =
                                        research_guard
                                    {
                                        let observation_remaining = guard
                                            .expires_at
                                            .saturating_duration_since(confirmed.created_at);
                                        let undo_remaining = guard
                                            .expires_at
                                            .saturating_duration_since(std::time::Instant::now());
                                        (
                                            Duration::from_secs(3).min(undo_remaining),
                                            Duration::from_secs(12).min(observation_remaining),
                                        )
                                    } else {
                                        (Duration::from_secs(3), Duration::from_secs(12))
                                    };
                                    #[cfg(not(all(
                                        target_os = "macos",
                                        feature = "research-harness"
                                    )))]
                                    let (undo_lifetime, observation_lifetime) =
                                        (Duration::from_secs(3), Duration::from_secs(12));
                                    let (undo, observation, lease) =
                                        AnchorTicket::retain_with_learning_observation(
                                            original_anchor,
                                            undo_lifetime,
                                            observation_lifetime,
                                            confirmed.created_at,
                                            #[cfg(all(
                                                target_os = "macos",
                                                feature = "research-harness"
                                            ))]
                                            research_guard.cloned(),
                                        );
                                    confirmed.anchor_owner_id = observation.inner.id;
                                    outcome.confirmed_insertion = Some(confirmed);
                                    (undo, Some(observation), lease)
                                } else {
                                    let undo_lifetime = {
                                        #[cfg(all(
                                            target_os = "macos",
                                            feature = "research-harness"
                                        ))]
                                        {
                                            research_guard.map_or(Duration::from_secs(3), |guard| {
                                                Duration::from_secs(3).min(
                                                    guard.expires_at.saturating_duration_since(
                                                        std::time::Instant::now(),
                                                    ),
                                                )
                                            })
                                        }
                                        #[cfg(not(all(
                                            target_os = "macos",
                                            feature = "research-harness"
                                        )))]
                                        {
                                            Duration::from_secs(3)
                                        }
                                    };
                                    let (undo, lease) =
                                        AnchorTicket::retain(original_anchor, undo_lifetime);
                                    (undo, None, lease)
                                };
                                outcome.post_insert_target_guard = Some(guard);
                                #[cfg(all(target_os = "macos", feature = "research-harness"))]
                                if let Some(handoff) = ticket_handoff.as_ref() {
                                    let mut handoff = handoff
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                                    handoff.undo_ticket = Some(ticket);
                                    handoff.observation_ticket = observation;
                                } else {
                                    outcome.post_insert_field_ticket = Some(ticket);
                                    outcome.learning_observation_ticket = observation;
                                }
                                #[cfg(not(all(
                                    target_os = "macos",
                                    feature = "research-harness"
                                )))]
                                {
                                    outcome.post_insert_field_ticket = Some(ticket);
                                    outcome.learning_observation_ticket = observation;
                                }
                                *undo_lease = Some(lease);
                            }
                        }
                    }
                }
                trace.clipboard_write_owned = clipboard_snapshot
                    .marker_is_current(
                        effects,
                        &clipboard_marker,
                        ClipboardMarkerCheckStageV1::PreRestore,
                        u8::from(attempt.shortcut_sent),
                    )
                    .ok();
                if outcome.verified {
                    #[cfg(all(target_os = "macos", feature = "research-harness"))]
                    if research_guard.is_some() {
                        verify_after()?;
                    }
                    trace.clipboard_restore_attempted = true;
                    trace.stage = "clipboard_restore";
                    trace.clipboard_restored = clipboard_snapshot
                        .restore_if_unchanged(effects, Some(&clipboard_marker))
                        .ok()
                        .and_then(ClipboardRestoreAttemptV1::trace_value);
                    #[cfg(all(target_os = "macos", feature = "research-harness"))]
                    if research_guard.is_some() {
                        verify_after()?;
                    }
                }
                TimedInsertOutcome {
                    clipboard_restoration: ClipboardRestoration::Unknown,
                    outcome,
                    paste_submission,
                    readback_confirmation: Duration::ZERO,
                    diagnostic: None,
                }
            })
        });
    readback_result.map(|mut outcome| {
        outcome.readback_confirmation = readback_confirmation;
        outcome
    })
}

/// Apply a selected-action result to the exact captured source. Selection and
/// empty-composer targets use the Phase 1 UTF-16 splice/keyboard path. A full
/// field source is replaced only through a verified AX value update; it never
/// falls through to a paste at the current caret.
#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_captured_text_action(
    app: &AppHandle,
    text: &str,
    source: &CapturedTextActionSource,
    accessibility: bool,
    cancellation: CancellationToken,
    verify_target: impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    verify_after: impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    expected_pid: i32,
) -> Result<InsertOutcome, PasteError> {
    if source.kind != TextActionSourceKind::FieldText {
        return insert_with_post_verify(
            app,
            text,
            accessibility,
            cancellation,
            verify_target,
            verify_after,
            expected_pid,
        );
    }
    check_before_clipboard(&cancellation)?;
    if !accessibility {
        return Err(PasteError::Accessibility);
    }
    verify_target()?;
    let focus_anchor =
        FocusedFieldAnchor::capture(expected_pid).ok_or(PasteError::InputUnavailable)?;
    if !focus_anchor.is_current_focus()
        || focus_anchor.read_value().as_deref() != Some(source.text.as_str())
        || focus_anchor.read_selected_range() != source.selection_range
    {
        return Err(PasteError::SelectionChanged);
    }
    if cancellation.is_cancelled() {
        return Err(PasteError::Cancelled);
    }
    match try_ax_replace_full_field(
        &cancellation,
        &verify_target,
        &verify_after,
        expected_pid,
        &focus_anchor,
        source,
        text,
    ) {
        AxInsertAttempt::Verified(value_after) => Ok(build_verified_ax_outcome(value_after)),
        AxInsertAttempt::Uncertain => Err(PasteError::MutationUncertain),
        AxInsertAttempt::NoOp | AxInsertAttempt::NotAttempted => Err(PasteError::InputUnavailable),
    }
}

fn try_ax_replace_full_field(
    cancellation: &CancellationToken,
    verify_target: &impl Fn() -> Result<(), PasteError>,
    verify_after: &impl Fn() -> Result<(), PasteError>,
    expected_pid: i32,
    expected_anchor: &FocusedFieldAnchor,
    source: &CapturedTextActionSource,
    text: &str,
) -> AxInsertAttempt {
    if cancellation.is_cancelled() || verify_target().is_err() || text.is_empty() {
        return AxInsertAttempt::NotAttempted;
    }
    if !expected_anchor.is_current_focus()
        || expected_anchor.read_value().as_deref() != Some(source.text.as_str())
        || expected_anchor.read_selected_range() != source.selection_range
    {
        return AxInsertAttempt::NotAttempted;
    }
    #[cfg(target_os = "macos")]
    let result = macos_ax::try_replace_full_field(
        expected_pid,
        &expected_anchor.inner,
        source,
        text,
        cancellation,
        verify_target,
    );
    #[cfg(not(target_os = "macos"))]
    let result = {
        let _ = (expected_pid, expected_anchor, source, text);
        AxInsertAttempt::NotAttempted
    };
    match result {
        AxInsertAttempt::Verified(value_after) if verify_after().is_ok() => {
            AxInsertAttempt::Verified(value_after)
        }
        AxInsertAttempt::Verified(_) => AxInsertAttempt::Uncertain,
        other => other,
    }
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
    let trace = crate::research_trace::current();
    if let Some(trace) = &trace {
        trace.begin_clipboard_write();
    }
    let result = copy(app, text);
    if let Some(trace) = trace {
        trace.clipboard_resolved(
            if result.is_ok() {
                "retained_dictation"
            } else {
                "copy_failed_unknown"
            },
            result.is_ok(),
        );
    }
    result
}

/// Copy only while a preview's lease is current. The validation runs in the
/// same blocking worker as the clipboard write, closing the race between an
/// async generation check and this side effect.
pub(crate) fn copy_if_valid(
    app: &AppHandle,
    text: &str,
    validate: impl FnOnce() -> Result<(), PasteError>,
) -> Result<(), PasteError> {
    validate()?;
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

/// Final fail-closed gate shared by the real AX ticket and regression harness.
/// The callback is the retained AX anchor's CFEqual focus check in production.
pub(crate) fn run_guarded_undo(
    cancellation: &CancellationToken,
    deadline: std::time::Instant,
    same_field: impl FnOnce() -> bool,
    submit: impl FnOnce() -> Result<(), PasteError>,
) -> Result<&'static str, String> {
    if !same_field() || cancellation.is_cancelled() || std::time::Instant::now() >= deadline {
        return Ok("stale_target");
    }
    submit().map_err(|error| error.to_string())?;
    Ok("success")
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

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    struct FakeNamedRemoteClientV1 {
        operations: Mutex<Vec<ClipboardGuardOperationV1>>,
        closed: Mutex<bool>,
        capture_outcome: ClipboardGuardRpcOutcomeV1,
        permit_outcome: ClipboardGuardRpcOutcomeV1,
        close_outcome: ClipboardGuardRpcOutcomeV1,
        fail_operation: Option<ClipboardGuardOperationV1>,
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    impl FakeNamedRemoteClientV1 {
        fn new() -> Self {
            Self {
                operations: Mutex::new(Vec::new()),
                closed: Mutex::new(false),
                capture_outcome: ClipboardGuardRpcOutcomeV1::SnapshotUnchanged,
                permit_outcome: ClipboardGuardRpcOutcomeV1::PostPermitRecordedOnly {
                    parent_permit_count: 1,
                },
                close_outcome: ClipboardGuardRpcOutcomeV1::CloseAcknowledged,
                fail_operation: None,
            }
        }

        fn with_capture_outcome(mut self, outcome: ClipboardGuardRpcOutcomeV1) -> Self {
            self.capture_outcome = outcome;
            self
        }

        fn with_permit_outcome(mut self, outcome: ClipboardGuardRpcOutcomeV1) -> Self {
            self.permit_outcome = outcome;
            self
        }

        fn with_close_outcome(mut self, outcome: ClipboardGuardRpcOutcomeV1) -> Self {
            self.close_outcome = outcome;
            self
        }

        fn failing(mut self, operation: ClipboardGuardOperationV1) -> Self {
            self.fail_operation = Some(operation);
            self
        }
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    impl ClipboardGuardRpcClientV1 for FakeNamedRemoteClientV1 {
        fn request(
            &self,
            lease_id: u64,
            operation: ClipboardGuardOperationV1,
        ) -> Result<ClipboardGuardRpcOutcomeV1, ClipboardGuardClientErrorV1> {
            if lease_id != 17 {
                return Err(ClipboardGuardClientErrorV1::LeaseMismatch);
            }
            self.operations.lock().unwrap().push(operation);
            let closed = *self.closed.lock().unwrap();
            if closed {
                return Err(ClipboardGuardClientErrorV1::Closed);
            }
            if self.fail_operation == Some(operation) {
                *self.closed.lock().unwrap() = true;
                return Err(ClipboardGuardClientErrorV1::AcknowledgementUnavailable);
            }
            match operation {
                ClipboardGuardOperationV1::CaptureFreshUnchanged => Ok(self.capture_outcome),
                ClipboardGuardOperationV1::CheckUnchangedSinceCapture => {
                    Ok(ClipboardGuardRpcOutcomeV1::SnapshotUnchanged)
                }
                ClipboardGuardOperationV1::WriteRegisteredTemporaryText => {
                    Ok(ClipboardGuardRpcOutcomeV1::WriteOwned { write_id: 29 })
                }
                ClipboardGuardOperationV1::CheckCurrent { .. } => {
                    Ok(ClipboardGuardRpcOutcomeV1::CurrentOwned)
                }
                ClipboardGuardOperationV1::PermitShortcutPost { .. } => Ok(self.permit_outcome),
                ClipboardGuardOperationV1::ApplyDeclaredEdit { .. }
                | ClipboardGuardOperationV1::CloseRegisteredOwnedPasteSession { .. } => {
                    Ok(ClipboardGuardRpcOutcomeV1::Rejected)
                }
                ClipboardGuardOperationV1::RequestRestoreIfOwned { .. } => {
                    Ok(ClipboardGuardRpcOutcomeV1::RestoreDeferred)
                }
                ClipboardGuardOperationV1::Close { .. } => Ok(self.close_outcome),
            }
        }
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    fn named_remote_input(
        client: Arc<dyn ClipboardGuardRpcClientV1>,
        case: RegisteredNamedRemoteClipboardCaseV1,
    ) -> RemoteProbeChildInputV1 {
        RemoteProbeChildInputV1 {
            client,
            lease_id: 17,
            case,
            board: RemoteClipboardBoardV1::NamedFixture,
        }
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    #[test]
    fn general_remote_probe_uses_recorder_only_flow_and_deferred_restore() {
        let client = Arc::new(FakeNamedRemoteClientV1::new());
        let operations = Arc::clone(&client);
        let result = run_research_remote_clipboard_probe(RemoteProbeChildInputV1 {
            client,
            lease_id: 17,
            case: RegisteredNamedRemoteClipboardCaseV1::Positive,
            board: RemoteClipboardBoardV1::GeneralClipboard,
        });

        assert_eq!(result.result, RemoteProbeChildResultKindV1::Completed);
        assert_eq!(result.child_callback_count, 1);
        assert!(!result.shortcut_sent);
        assert_eq!(result.target_dispatch_count, 0);
        assert_eq!(result.restore, RemoteProbeChildRestoreV1::Deferred);
        let operations = operations.operations.lock().unwrap();
        assert_eq!(operations.len(), 10);
        assert_eq!(
            operations[2],
            ClipboardGuardOperationV1::WriteRegisteredTemporaryText
        );
        assert!(matches!(
            operations[5],
            ClipboardGuardOperationV1::PermitShortcutPost { .. }
        ));
        assert!(matches!(
            operations[8],
            ClipboardGuardOperationV1::RequestRestoreIfOwned { .. }
        ));
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    #[test]
    fn named_remote_probe_uses_facade_defers_restore_and_never_dispatches() {
        let client = Arc::new(FakeNamedRemoteClientV1::new());
        let operations = Arc::clone(&client);
        let result = run_research_named_remote_clipboard_probe(named_remote_input(
            client,
            RegisteredNamedRemoteClipboardCaseV1::Positive,
        ));

        assert_eq!(result.result, RemoteProbeChildResultKindV1::Completed);
        assert_eq!(result.child_callback_count, 1);
        assert!(!result.shortcut_sent);
        assert_eq!(result.target_dispatch_count, 0);
        assert_eq!(result.restore, RemoteProbeChildRestoreV1::Deferred);
        let operations = operations.operations.lock().unwrap();
        assert_eq!(operations.len(), 10);
        assert_eq!(
            operations[0],
            ClipboardGuardOperationV1::CaptureFreshUnchanged
        );
        assert_eq!(
            operations[1],
            ClipboardGuardOperationV1::CheckUnchangedSinceCapture
        );
        assert_eq!(
            operations[2],
            ClipboardGuardOperationV1::WriteRegisteredTemporaryText
        );
        assert_eq!(
            operations[3],
            ClipboardGuardOperationV1::CheckCurrent {
                write_id: 29,
                stage: ClipboardGuardCheckStageV1::MarkerAfterWrite,
                child_callback_count: 0,
            }
        );
        assert_eq!(
            operations[4],
            ClipboardGuardOperationV1::CheckCurrent {
                write_id: 29,
                stage: ClipboardGuardCheckStageV1::PreBoundary,
                child_callback_count: 0,
            }
        );
        assert_eq!(
            operations[5],
            ClipboardGuardOperationV1::PermitShortcutPost { write_id: 29 }
        );
        assert_eq!(
            operations[6],
            ClipboardGuardOperationV1::CheckCurrent {
                write_id: 29,
                stage: ClipboardGuardCheckStageV1::PostCallback,
                child_callback_count: 1,
            }
        );
        assert_eq!(
            operations[7],
            ClipboardGuardOperationV1::CheckCurrent {
                write_id: 29,
                stage: ClipboardGuardCheckStageV1::PreRestore,
                child_callback_count: 1,
            }
        );
        assert_eq!(
            operations[8],
            ClipboardGuardOperationV1::RequestRestoreIfOwned { write_id: 29 }
        );
        assert_eq!(
            operations[9],
            ClipboardGuardOperationV1::Close {
                child_callback_count: 1,
                shortcut_sent: false,
                child_restore: RemoteProbeChildRestoreV1::Deferred,
            }
        );
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    #[test]
    fn named_remote_permit_ack_loss_has_no_callback_or_fallback() {
        let client = Arc::new(
            FakeNamedRemoteClientV1::new()
                .failing(ClipboardGuardOperationV1::PermitShortcutPost { write_id: 29 }),
        );
        let operations = Arc::clone(&client);
        let result = run_research_named_remote_clipboard_probe(named_remote_input(
            client,
            RegisteredNamedRemoteClipboardCaseV1::PermitAckLost,
        ));

        assert_eq!(
            result.result,
            RemoteProbeChildResultKindV1::AcknowledgementUnavailable
        );
        assert_eq!(result.child_callback_count, 0);
        assert!(!result.shortcut_sent);
        assert_eq!(result.target_dispatch_count, 0);
        assert_eq!(result.restore, RemoteProbeChildRestoreV1::Unknown);
        let operations = operations.operations.lock().unwrap();
        assert_eq!(operations.len(), 6);
        assert_eq!(
            operations.last(),
            Some(&ClipboardGuardOperationV1::PermitShortcutPost { write_id: 29 })
        );
        assert!(!operations.iter().any(|operation| matches!(
            operation,
            ClipboardGuardOperationV1::CheckCurrent {
                stage: ClipboardGuardCheckStageV1::PostCallback
                    | ClipboardGuardCheckStageV1::PreRestore,
                ..
            } | ClipboardGuardOperationV1::RequestRestoreIfOwned { .. }
                | ClipboardGuardOperationV1::Close { .. }
        )));
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    #[test]
    fn named_remote_permit_rejection_stops_without_cleanup_rpc() {
        let client = Arc::new(
            FakeNamedRemoteClientV1::new()
                .with_permit_outcome(ClipboardGuardRpcOutcomeV1::Rejected),
        );
        let operations = Arc::clone(&client);
        let result = run_research_named_remote_clipboard_probe(named_remote_input(
            client,
            RegisteredNamedRemoteClipboardCaseV1::Positive,
        ));

        assert_eq!(
            result.result,
            RemoteProbeChildResultKindV1::ProtocolRejected
        );
        assert_eq!(result.child_callback_count, 0);
        assert_eq!(result.restore, RemoteProbeChildRestoreV1::Unknown);
        let operations = operations.operations.lock().unwrap();
        assert_eq!(operations.len(), 6);
        assert_eq!(
            operations.last(),
            Some(&ClipboardGuardOperationV1::PermitShortcutPost { write_id: 29 })
        );
        assert!(!operations.iter().any(|operation| matches!(
            operation,
            ClipboardGuardOperationV1::CheckCurrent {
                stage: ClipboardGuardCheckStageV1::PostCallback
                    | ClipboardGuardCheckStageV1::PreRestore,
                ..
            } | ClipboardGuardOperationV1::RequestRestoreIfOwned { .. }
                | ClipboardGuardOperationV1::Close { .. }
        )));
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    #[test]
    fn named_remote_close_rejects_non_close_acknowledgement() {
        let client = Arc::new(
            FakeNamedRemoteClientV1::new()
                .with_close_outcome(ClipboardGuardRpcOutcomeV1::NotWritten),
        );
        let operations = Arc::clone(&client);
        let result = run_research_named_remote_clipboard_probe(named_remote_input(
            client,
            RegisteredNamedRemoteClipboardCaseV1::Positive,
        ));

        assert_eq!(
            result.result,
            RemoteProbeChildResultKindV1::ProtocolRejected
        );
        assert_eq!(result.child_callback_count, 1);
        assert_eq!(result.restore, RemoteProbeChildRestoreV1::Deferred);
        let operations = operations.operations.lock().unwrap();
        assert_eq!(operations.len(), 10);
        assert!(matches!(
            operations.last(),
            Some(ClipboardGuardOperationV1::Close { .. })
        ));
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    #[test]
    fn named_remote_permit_denial_runs_only_deferred_cleanup() {
        let client = Arc::new(
            FakeNamedRemoteClientV1::new()
                .with_permit_outcome(ClipboardGuardRpcOutcomeV1::PermitDenied),
        );
        let operations = Arc::clone(&client);
        let result = run_research_named_remote_clipboard_probe(named_remote_input(
            client,
            RegisteredNamedRemoteClipboardCaseV1::PermitDenied,
        ));

        assert_eq!(result.result, RemoteProbeChildResultKindV1::PermitDenied);
        assert_eq!(result.child_callback_count, 0);
        assert!(!result.shortcut_sent);
        assert_eq!(result.target_dispatch_count, 0);
        assert_eq!(result.restore, RemoteProbeChildRestoreV1::Deferred);
        assert!(operations
            .operations
            .lock()
            .unwrap()
            .iter()
            .any(|operation| matches!(
                operation,
                ClipboardGuardOperationV1::Close {
                    child_callback_count: 0,
                    shortcut_sent: false,
                    child_restore: RemoteProbeChildRestoreV1::Deferred,
                }
            )));
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    #[test]
    fn named_remote_write_ack_loss_never_retries_or_falls_back() {
        let client = Arc::new(
            FakeNamedRemoteClientV1::new()
                .failing(ClipboardGuardOperationV1::WriteRegisteredTemporaryText),
        );
        let operations = Arc::clone(&client);
        let result = run_research_named_remote_clipboard_probe(named_remote_input(
            client,
            RegisteredNamedRemoteClipboardCaseV1::WriteAckLost,
        ));

        assert_eq!(
            result.result,
            RemoteProbeChildResultKindV1::AcknowledgementUnavailable
        );
        assert_eq!(result.child_callback_count, 0);
        assert_eq!(result.restore, RemoteProbeChildRestoreV1::Unknown);
        let operations = operations.operations.lock().unwrap();
        assert_eq!(
            operations
                .iter()
                .filter(|operation| **operation
                    == ClipboardGuardOperationV1::WriteRegisteredTemporaryText)
                .count(),
            1
        );
        assert!(!operations.iter().any(|operation| matches!(
            operation,
            ClipboardGuardOperationV1::PermitShortcutPost { .. }
                | ClipboardGuardOperationV1::Close { .. }
        )));
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    #[test]
    fn named_remote_eof_case_does_not_report_restore_before_parent_close() {
        let client = Arc::new(FakeNamedRemoteClientV1::new());
        let operations = Arc::clone(&client);
        let result = run_research_named_remote_clipboard_probe(named_remote_input(
            client,
            RegisteredNamedRemoteClipboardCaseV1::EofChildAlive,
        ));

        assert_eq!(result.restore, RemoteProbeChildRestoreV1::Deferred);
        assert_eq!(result.child_callback_count, 1);
        let operations = operations.operations.lock().unwrap();
        assert!(matches!(
            operations.last(),
            Some(ClipboardGuardOperationV1::RequestRestoreIfOwned { write_id: 29 })
        ));
        assert!(!operations
            .iter()
            .any(|operation| matches!(operation, ClipboardGuardOperationV1::Close { .. })));
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    #[test]
    fn named_remote_prewrite_change_stops_before_any_write() {
        let client = Arc::new(
            FakeNamedRemoteClientV1::new()
                .with_capture_outcome(ClipboardGuardRpcOutcomeV1::OwnershipChanged),
        );
        let operations = Arc::clone(&client);
        let result = run_research_named_remote_clipboard_probe(named_remote_input(
            client,
            RegisteredNamedRemoteClipboardCaseV1::PrewriteExternalCopy,
        ));

        assert_eq!(
            result.result,
            RemoteProbeChildResultKindV1::PrewriteOwnershipChanged
        );
        assert_eq!(result.child_callback_count, 0);
        assert_eq!(result.restore, RemoteProbeChildRestoreV1::NotNeeded);
        assert_eq!(
            *operations.operations.lock().unwrap(),
            [ClipboardGuardOperationV1::CaptureFreshUnchanged]
        );
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    #[test]
    fn named_remote_cancel_keeps_deferred_restore_unknown_in_trace() {
        let client = Arc::new(FakeNamedRemoteClientV1::new());
        let effects = DeliveryEffects::research_named_remote(client.clone(), 17);
        let snapshot = ClipboardSnapshot::capture(&effects).unwrap();
        assert_eq!(snapshot.capture_fresh_unchanged().unwrap(), Some(true));
        assert!(snapshot.unchanged_since_capture(&effects).unwrap());
        snapshot
            .write_temporary_text(&effects, macos_pasteboard::registered_temporary_text(), "")
            .unwrap();
        let marker = snapshot
            .marker_after_write(&effects, macos_pasteboard::registered_temporary_text(), "")
            .unwrap()
            .unwrap();

        let mut trace = DeliveryTrace::new(false, 0);
        trace.clipboard_snapshot_captured = true;
        trace.clipboard_write_attempted = true;
        trace.clipboard_write_owned = Some(true);
        trace.clipboard_marker = Some(marker);
        let _ = resolve_post_write_cancellation(&effects, &snapshot, &mut trace, false);

        assert!(trace.clipboard_restore_attempted);
        assert_eq!(trace.clipboard_restored, None);
        assert_eq!(trace.clipboard_write_owned, Some(true));
        assert_eq!(
            restoration_fact(
                trace.clipboard_write_attempted,
                trace.clipboard_write_failed,
                trace.clipboard_write_owned,
                trace.clipboard_restore_attempted,
                trace.clipboard_restored,
            ),
            ClipboardRestoration::Unknown
        );
        assert!(client
            .operations
            .lock()
            .unwrap()
            .iter()
            .any(|operation| matches!(
                operation,
                ClipboardGuardOperationV1::RequestRestoreIfOwned { write_id: 29 }
            )));
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    #[test]
    fn named_remote_not_written_never_means_unchanged() {
        let client = Arc::new(
            FakeNamedRemoteClientV1::new()
                .with_capture_outcome(ClipboardGuardRpcOutcomeV1::NotWritten),
        );
        let operations = Arc::clone(&client);
        let result = run_research_named_remote_clipboard_probe(named_remote_input(
            client,
            RegisteredNamedRemoteClipboardCaseV1::Positive,
        ));

        assert_eq!(result.result, RemoteProbeChildResultKindV1::Unknown);
        assert_eq!(result.child_callback_count, 0);
        assert_eq!(result.restore, RemoteProbeChildRestoreV1::Unknown);
        assert_eq!(
            *operations.operations.lock().unwrap(),
            [ClipboardGuardOperationV1::CaptureFreshUnchanged]
        );
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    #[test]
    fn named_remote_write_rejects_unregistered_text_without_sending_it() {
        let client = Arc::new(FakeNamedRemoteClientV1::new());
        let effects = DeliveryEffects::research_named_remote(client.clone(), 17);
        let snapshot = ClipboardSnapshot::capture(&effects).unwrap();
        assert_eq!(snapshot.capture_fresh_unchanged().unwrap(), Some(true));

        assert!(snapshot
            .write_temporary_text(&effects, "caller supplied text", "")
            .is_err());
        assert_eq!(
            *client.operations.lock().unwrap(),
            [ClipboardGuardOperationV1::CaptureFreshUnchanged]
        );
    }

    #[test]
    fn fake_submission_and_readback_use_the_production_traced_boundary() {
        let mut timer = crate::metrics::Metrics::default().stop_to_insert(
            crate::metrics::MetricGroup::unknown("paste_trace_test"),
            CancellationToken::new(),
        );
        let trace = timer.bind_trace(29);
        let cancellation = CancellationToken::new();
        let (attempt, _submit_duration) =
            run_traced_boundary(Some(&trace), crate::metrics::Stage::Paste, || {
                run_paste_attempt(
                    &cancellation,
                    || Ok(()),
                    || Ok(()),
                    |mark| {
                        mark();
                        Ok(())
                    },
                )
            });
        assert!(attempt.shortcut_sent);
        assert!(attempt.result.is_ok());
        let expected = "before inserted";
        let before = "before";
        let (confirmed, _confirmation_duration) =
            run_traced_confirmation(Some(&trace), attempt.result, || {
                Ok::<_, PasteError>(confirm_keyboard_paste_readback(
                    PasteReadbackExpectation {
                        expected_after: Some(expected),
                        value_before: Some(before),
                        insertion_end_caret_utf16: None,
                    },
                    &cancellation,
                    || (true, true),
                    || Some(expected.to_owned()),
                    |_, _| FullRangeReadback::Unavailable,
                    |_| {},
                ))
            });
        let readback = confirmed.expect("production confirmation callback was reached");
        let verification = classify_confirmed_paste_readback(
            !cancellation.is_cancelled(),
            readback.proof,
            Some(expected),
            Some(before),
            readback.value_after.as_deref(),
            true,
            true,
        );
        let outcome = build_insert_outcome(true, readback.value_after, verification);
        assert!(outcome.verified);
        assert_eq!(outcome.verification, Some(PasteVerificationFact::Verified));
        let (unconfirmed_attempt, _) =
            run_traced_boundary(Some(&trace), crate::metrics::Stage::Paste, || {
                run_paste_attempt(
                    &cancellation,
                    || Ok(()),
                    || Ok(()),
                    |mark| {
                        mark();
                        Ok(())
                    },
                )
            });
        let (unconfirmed_readback, _) =
            run_traced_confirmation(Some(&trace), unconfirmed_attempt.result, || {
                Ok::<_, PasteError>(confirm_keyboard_paste_readback(
                    PasteReadbackExpectation {
                        expected_after: Some(expected),
                        value_before: Some(before),
                        insertion_end_caret_utf16: None,
                    },
                    &cancellation,
                    || (false, true),
                    || Some(expected.to_owned()),
                    |_, _| FullRangeReadback::Unavailable,
                    |_| {},
                ))
            });
        let unconfirmed_readback = unconfirmed_readback.expect("readback is classified");
        let unconfirmed_fact = classify_confirmed_paste_readback(
            !cancellation.is_cancelled(),
            unconfirmed_readback.proof,
            Some(expected),
            Some(before),
            unconfirmed_readback.value_after.as_deref(),
            false,
            true,
        );
        let unconfirmed =
            build_insert_outcome(true, unconfirmed_readback.value_after, unconfirmed_fact);
        assert!(!unconfirmed.verified);
        assert_eq!(
            unconfirmed.verification,
            Some(PasteVerificationFact::TargetUnconfirmed)
        );
        let snapshot = trace.snapshot();
        assert_eq!(snapshot.session_generation, 29);
        assert_eq!(
            snapshot
                .stages
                .iter()
                .map(|(stage, _, _)| *stage)
                .collect::<Vec<_>>(),
            vec![
                crate::metrics::Stage::Paste,
                crate::metrics::Stage::TargetConfirmation,
                crate::metrics::Stage::Paste,
                crate::metrics::Stage::TargetConfirmation,
            ]
        );
        timer.finish(crate::metrics::DeliveryOutcome::PasteUnconfirmed, None);
        assert_eq!(trace.snapshot().stages, snapshot.stages);
    }

    #[test]
    fn failed_submission_does_not_enter_target_confirmation() {
        let mut timer = crate::metrics::Metrics::default().stop_to_insert(
            crate::metrics::MetricGroup::unknown("paste_failed_submission_trace_test"),
            CancellationToken::new(),
        );
        let trace = timer.bind_trace(30);
        let readback_called = std::sync::atomic::AtomicBool::new(false);
        let (result, _) =
            run_traced_confirmation::<(), _>(Some(&trace), Err(PasteError::Cancelled), || {
                readback_called.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            });
        assert!(result.is_err());
        assert!(!readback_called.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!trace
            .snapshot()
            .stages
            .iter()
            .any(|(stage, _, _)| { *stage == crate::metrics::Stage::TargetConfirmation }));

        let (unconfirmed, _) =
            run_traced_confirmation(Some(&trace), Ok(()), || Ok::<_, PasteError>(false));
        assert!(matches!(unconfirmed, Ok(false)));
        assert_eq!(
            trace
                .snapshot()
                .stages
                .iter()
                .filter(|(stage, _, _)| *stage == crate::metrics::Stage::TargetConfirmation)
                .count(),
            1,
            "successful submission reaches readback even when proof is absent"
        );
        timer.finish(crate::metrics::DeliveryOutcome::PasteUnconfirmed, None);
    }

    #[test]
    fn temporary_clipboard_type_matcher_requires_exact_four_types() {
        assert!(TEMPORARY_CLIPBOARD_HINT_DATA.is_empty());
        let expected = TEMPORARY_CLIPBOARD_TYPES.map(str::to_owned);
        assert!(has_exact_temporary_type_names(&expected));

        let reordered = vec![
            expected[3].clone(),
            expected[1].clone(),
            expected[0].clone(),
            expected[2].clone(),
        ];
        assert!(has_exact_temporary_type_names(&reordered));

        let mut foreign = expected.to_vec();
        foreign.push("foreign.type".into());
        let mut duplicate = expected.to_vec();
        duplicate.push(expected[0].clone());
        for invalid in [expected[..3].to_vec(), foreign, duplicate] {
            assert!(!has_exact_temporary_type_names(&invalid));
        }
    }

    #[test]
    fn expected_insert_caret_accepts_collapsed_caret_and_empty_field() {
        assert_eq!(
            expected_insert_caret_utf16(Some("a🙂b"), Some((3, 0)), "你"),
            Some(4)
        );
        assert_eq!(expected_insert_caret_utf16(Some(""), None, "x"), Some(1));
        assert_eq!(
            expected_insert_caret_utf16(Some("🙂"), Some((1, 0)), "x"),
            None
        );
    }

    fn run_identity(
        caret: Option<u64>,
        results: Vec<FullRangeReadback>,
    ) -> (PasteReadbackConfirmation, usize) {
        let text = "same";
        let mut results = results.into_iter();
        let mut reads = 0;
        let r = confirm_keyboard_paste_readback(
            PasteReadbackExpectation {
                expected_after: Some(text),
                value_before: Some(text),
                insertion_end_caret_utf16: caret,
            },
            &CancellationToken::new(),
            || (true, true),
            || Some(text.to_owned()),
            |_, _| {
                reads += 1;
                results.next().unwrap_or(FullRangeReadback::Mismatch)
            },
            |_| {},
        );
        (r, reads)
    }

    #[test]
    fn identity_replacement_requires_caret_proof() {
        let (r, reads) = run_identity(Some(4), vec![]);
        assert_eq!(r.proof, None);
        assert_eq!(r.poll_count, 10);
        assert_eq!(reads, 3, "poll 0, 4 and the last poll only");
        assert_eq!(r.mismatch_kind, Some("identity_unproven"));

        let (r, reads) = run_identity(
            Some(4),
            vec![FullRangeReadback::Mismatch, FullRangeReadback::Verified],
        );
        assert_eq!(r.proof, Some(PasteVerificationFact::VerifiedFullRange));
        assert_eq!(reads, 2);

        let (r, reads) = run_identity(Some(4), vec![FullRangeReadback::Unavailable]);
        assert_eq!(r.proof, None);
        assert_eq!(reads, 1, "stop after Unavailable");
        assert_eq!(r.poll_count, 1, "loop stops, no 1.8 s of useless polling");
        assert_eq!(r.mismatch_kind, Some("full_range_unavailable"));

        let (r, reads) = run_identity(None, vec![]);
        assert_eq!(r.proof, None, "empty-insert identity never verifies");
        assert_eq!(reads, 0);
    }

    #[test]
    fn collapsed_caret_insertion_reaches_full_range_proof_after_changed_mismatch() {
        let before = "a🙂b";
        let expected = "a🙂你b";
        let caret = expected_insert_caret_utf16(Some(before), Some((3, 0)), "你").unwrap();
        let mut values = [Some(before.to_owned()), Some("a🙂你b\n".to_owned())].into_iter();
        let mut reads = 0;
        let r = confirm_keyboard_paste_readback(
            PasteReadbackExpectation {
                expected_after: Some(expected),
                value_before: Some(before),
                insertion_end_caret_utf16: Some(caret),
            },
            &CancellationToken::new(),
            || (true, true),
            || values.next().flatten(),
            |text, c| {
                reads += 1;
                assert_eq!((text, c), (expected, 4));
                FullRangeReadback::Verified
            },
            |_| {},
        );
        assert_eq!(r.proof, Some(PasteVerificationFact::VerifiedFullRange));
        assert_eq!(reads, 1);
    }

    #[test]
    fn never_landed_identity_replacement_is_not_classified_as_verified() {
        let text = "same";
        let (r, _) = run_identity(Some(4), vec![]);
        let fact = classify_confirmed_paste_readback(
            true,
            r.proof,
            Some(text),
            Some(text),
            r.value_after.as_deref(),
            true,
            true,
        );
        assert_ne!(fact, PasteVerificationFact::Verified);
        assert_eq!(fact, PasteVerificationFact::ReadbackUnavailable);
    }

    #[test]
    fn identity_classifier_needs_caret_proof_and_confirmed_identities() {
        let t = "same";
        let c = |proof, target, focus| {
            classify_confirmed_paste_readback(true, proof, Some(t), Some(t), Some(t), target, focus)
        };
        assert_eq!(
            c(None, true, true),
            PasteVerificationFact::ReadbackUnavailable
        );
        assert_eq!(
            c(Some(PasteVerificationFact::Verified), true, true),
            PasteVerificationFact::ReadbackUnavailable
        );
        assert_eq!(
            c(Some(PasteVerificationFact::VerifiedFullRange), true, true),
            PasteVerificationFact::VerifiedFullRange
        );
        assert_eq!(
            c(Some(PasteVerificationFact::VerifiedFullRange), false, true),
            PasteVerificationFact::TargetUnconfirmed
        );
        assert_eq!(
            c(Some(PasteVerificationFact::VerifiedFullRange), true, false),
            PasteVerificationFact::FocusedFieldUnconfirmed
        );
        // Non-identity: exact AXValue still verifies.
        assert_eq!(
            classify_confirmed_paste_readback(
                true,
                Some(PasteVerificationFact::Verified),
                Some("new"),
                Some("old"),
                Some("new"),
                true,
                true
            ),
            PasteVerificationFact::Verified
        );
    }

    #[test]
    fn identity_polling_stops_on_cancellation_and_guard_change_without_range_reads() {
        let text = "same";
        let expectation = PasteReadbackExpectation {
            expected_after: Some(text),
            value_before: Some(text),
            insertion_end_caret_utf16: Some(4),
        };
        // Cancelled before the first poll's read.
        let token = CancellationToken::new();
        token.cancel();
        let mut reads = 0;
        let r = confirm_keyboard_paste_readback(
            expectation,
            &token,
            || (true, true),
            || Some(text.to_owned()),
            |_, _| {
                reads += 1;
                FullRangeReadback::Verified
            },
            |_| {},
        );
        assert_eq!((r.proof, reads), (None, 0));

        // Focus lost: the guard fails before any range read.
        let mut reads = 0;
        let r = confirm_keyboard_paste_readback(
            expectation,
            &CancellationToken::new(),
            || (true, false),
            || Some(text.to_owned()),
            |_, _| {
                reads += 1;
                FullRangeReadback::Verified
            },
            |_| {},
        );
        assert_eq!((r.proof, reads), (None, 0));

        // Focus changes between the pre-read and post-read guard: no proof.
        let mut guards = 0;
        let mut reads = 0;
        let r = confirm_keyboard_paste_readback(
            expectation,
            &CancellationToken::new(),
            || {
                guards += 1;
                (true, guards < 3)
            },
            || Some(text.to_owned()),
            |_, _| {
                reads += 1;
                FullRangeReadback::Verified
            },
            |_| {},
        );
        assert_eq!((r.proof, reads), (None, 1));
    }

    #[test]
    fn readback_confirms_a_late_exact_unicode_splice_without_resending() {
        let before = "前缀🙂[替换]后缀";
        let expected = "前缀🙂[第一段：API v1.2\n路径 /tmp/owned.rs\n结束🙂]后缀";
        let mut values = [
            Some(before.to_owned()),
            Some(before.to_owned()),
            Some(before.to_owned()),
            Some(before.to_owned()),
            Some(before.to_owned()),
            Some("前缀🙂[第一段：API v1.2\n路径 /tmp/owned.rs]后缀".to_owned()),
            Some("前缀🙂[第一段：API v1.2\n路径 /tmp/owned.rs]后缀".to_owned()),
            Some(expected.to_owned()),
        ]
        .into_iter();
        let mut elapsed = Duration::ZERO;
        let mut range_reads = 0;
        let result = confirm_keyboard_paste_readback(
            PasteReadbackExpectation {
                expected_after: Some(expected),
                value_before: Some(before),
                insertion_end_caret_utf16: expected_insert_caret_utf16(
                    Some(before),
                    Some((5, 2)),
                    "第一段：API v1.2\n路径 /tmp/owned.rs\n结束🙂",
                ),
            },
            &CancellationToken::new(),
            || (true, true),
            || {
                values
                    .next()
                    .unwrap_or_else(|| Some("unexpected duplicate".into()))
            },
            |full_text, caret| {
                range_reads += 1;
                assert_eq!(full_text, expected);
                assert_eq!(
                    caret,
                    expected_insert_caret_utf16(
                        Some(before),
                        Some((5, 2)),
                        "第一段：API v1.2\n路径 /tmp/owned.rs\n结束🙂"
                    )
                    .unwrap()
                );
                FullRangeReadback::Mismatch
            },
            |delay| elapsed += delay,
        );

        assert_eq!(result.proof, Some(PasteVerificationFact::Verified));
        assert_eq!(result.value_after.as_deref(), Some(expected));
        assert_eq!(result.poll_count, 8);
        assert_eq!(elapsed, Duration::from_millis(950));
        assert_eq!(
            result.expected_utf16_length,
            Some(expected.encode_utf16().count() as u32)
        );
        assert_eq!(range_reads, 1, "only one strict fallback read is allowed");
    }

    #[test]
    fn readback_uses_one_exact_full_range_proof_for_representation_mismatch() {
        let before = "前🙂[替换]后";
        let insert = "新😀\n";
        let expected = "前🙂[新😀\n]后";
        let expected_caret =
            expected_insert_caret_utf16(Some(before), Some((4, 2)), insert).unwrap();
        assert_eq!(expected_caret, 8);
        assert!(expected_caret < expected.encode_utf16().count() as u64);
        let mut values = [Some(before.to_owned()), Some("前🙂[新😀\n]后\n".to_owned())].into_iter();
        let mut range_reads = 0;
        let result = confirm_keyboard_paste_readback(
            PasteReadbackExpectation {
                expected_after: Some(expected),
                value_before: Some(before),
                insertion_end_caret_utf16: Some(expected_caret),
            },
            &CancellationToken::new(),
            || (true, true),
            || values.next().flatten(),
            |full_text, caret| {
                range_reads += 1;
                assert_eq!(full_text, expected);
                assert_eq!(caret, expected_caret);
                FullRangeReadback::Verified
            },
            |_| {},
        );

        assert_eq!(result.proof, Some(PasteVerificationFact::VerifiedFullRange));
        assert_eq!(result.value_after.as_deref(), Some("前🙂[新😀\n]后\n"));
        assert_eq!(result.expected_utf16_length, Some(10));
        assert_eq!(result.actual_utf16_length, Some(11));
        assert_eq!(range_reads, 1);
        assert!(result.full_range_attempted);
        let outcome = build_insert_outcome(
            true,
            result.value_after,
            result
                .proof
                .expect("the full-range proof should be retained"),
        );
        assert!(outcome.verified);
        assert!(outcome.value_after.is_none());
        assert!(outcome.post_insert_input_fingerprint.is_none());
        assert!(outcome.post_insert_field_ticket.is_none());
    }

    #[test]
    fn missing_initial_ax_value_never_becomes_a_full_range_success() {
        let mut values = [
            None,
            None,
            None,
            None,
            Some("eventual exact text".to_owned()),
        ]
        .into_iter();
        let mut range_reads = 0;
        let result = confirm_keyboard_paste_readback(
            PasteReadbackExpectation {
                expected_after: Some("eventual exact text"),
                value_before: None,
                insertion_end_caret_utf16: Some(19),
            },
            &CancellationToken::new(),
            || (true, true),
            || values.next().flatten(),
            |_, _| {
                range_reads += 1;
                FullRangeReadback::Verified
            },
            |_| {},
        );

        assert_eq!(result.proof, None);
        assert_eq!(result.poll_count, 5);
        assert!(!result.full_range_attempted);
        assert_eq!(range_reads, 0);
    }

    #[test]
    fn persistent_duplicate_or_wrong_readback_remains_unverified_at_the_poll_cap() {
        let before = "original";
        let wrong = "original inserted inserted";
        let mut reads = 0;
        let mut range_reads = 0;
        let result = confirm_keyboard_paste_readback(
            PasteReadbackExpectation {
                expected_after: Some("original inserted"),
                value_before: Some(before),
                insertion_end_caret_utf16: Some("original inserted".encode_utf16().count() as u64),
            },
            &CancellationToken::new(),
            || (true, true),
            || {
                reads += 1;
                Some(if reads == 1 { before } else { wrong }.to_owned())
            },
            |_, _| {
                range_reads += 1;
                FullRangeReadback::Mismatch
            },
            |_| {},
        );

        assert_eq!(result.proof, None);
        assert_eq!(
            result.poll_count,
            POST_PASTE_READBACK_POLL_DELAYS_MS.len() as u8
        );
        assert_eq!(reads, POST_PASTE_READBACK_POLL_DELAYS_MS.len());
        assert_eq!(range_reads, 1);
        assert!(result.full_range_attempted);
    }

    #[test]
    fn cancel_or_focus_drift_never_accepts_a_late_or_alternate_readback() {
        let before = "before";
        let expected = "after";
        let cancellation = CancellationToken::new();
        let cancel_in_wait = cancellation.clone();
        let mut range_reads = 0;
        let cancelled = confirm_keyboard_paste_readback(
            PasteReadbackExpectation {
                expected_after: Some(expected),
                value_before: Some(before),
                insertion_end_caret_utf16: Some(expected.encode_utf16().count() as u64),
            },
            &cancellation,
            || (true, true),
            || Some(before.into()),
            |_, _| {
                range_reads += 1;
                FullRangeReadback::Verified
            },
            move |_| cancel_in_wait.cancel(),
        );
        assert_eq!(cancelled.proof, None);
        assert_eq!(cancelled.poll_count, 1);
        assert_eq!(range_reads, 0);

        let guard_calls = std::cell::Cell::new(0);
        let mut range_reads = 0;
        let before_range_drift = confirm_keyboard_paste_readback(
            PasteReadbackExpectation {
                expected_after: Some(expected),
                value_before: Some(before),
                insertion_end_caret_utf16: Some(expected.encode_utf16().count() as u64),
            },
            &CancellationToken::new(),
            || {
                guard_calls.set(guard_calls.get() + 1);
                if guard_calls.get() == 3 {
                    (false, true)
                } else {
                    (true, true)
                }
            },
            || {
                Some(
                    if guard_calls.get() == 1 {
                        before
                    } else {
                        "wrong"
                    }
                    .into(),
                )
            },
            |_, _| {
                range_reads += 1;
                FullRangeReadback::Verified
            },
            |_| {},
        );
        assert_eq!(before_range_drift.proof, None);
        assert_eq!(range_reads, 0);

        let guard_calls = std::cell::Cell::new(0);
        let after_range_drift = confirm_keyboard_paste_readback(
            PasteReadbackExpectation {
                expected_after: Some(expected),
                value_before: Some(before),
                insertion_end_caret_utf16: Some(expected.encode_utf16().count() as u64),
            },
            &CancellationToken::new(),
            || {
                guard_calls.set(guard_calls.get() + 1);
                (true, guard_calls.get() != 4)
            },
            || {
                Some(
                    if guard_calls.get() == 1 {
                        before
                    } else {
                        "wrong"
                    }
                    .into(),
                )
            },
            |_, _| FullRangeReadback::Verified,
            |_| {},
        );
        assert_eq!(after_range_drift.proof, None);
    }

    #[test]
    fn exact_ax_value_fast_path_returns_without_waiting_or_full_range() {
        let mut waits = 0;
        let mut range_reads = 0;
        let result = confirm_keyboard_paste_readback(
            PasteReadbackExpectation {
                expected_after: Some("exact"),
                value_before: Some("before"),
                insertion_end_caret_utf16: Some(5),
            },
            &CancellationToken::new(),
            || (true, true),
            || Some("exact".into()),
            |_, _| {
                range_reads += 1;
                FullRangeReadback::Unavailable
            },
            |_| waits += 1,
        );
        assert_eq!(result.proof, Some(PasteVerificationFact::Verified));
        assert_eq!(result.poll_count, 1);
        assert_eq!(waits, 0);
        assert_eq!(range_reads, 0);
    }

    #[test]
    fn final_cancellation_keeps_exact_or_full_range_readback_unverified() {
        let before = "before";
        let expected = "after";

        let exact = confirm_keyboard_paste_readback(
            PasteReadbackExpectation {
                expected_after: Some(expected),
                value_before: Some(before),
                insertion_end_caret_utf16: Some(expected.encode_utf16().count() as u64),
            },
            &CancellationToken::new(),
            || (true, true),
            || Some(expected.into()),
            |_, _| FullRangeReadback::Unavailable,
            |_| {},
        );
        assert_eq!(exact.proof, Some(PasteVerificationFact::Verified));
        assert_eq!(
            classify_confirmed_paste_readback(
                false,
                exact.proof,
                Some(expected),
                Some(before),
                exact.value_after.as_deref(),
                true,
                true,
            ),
            PasteVerificationFact::ShortcutOutcomeUnknown
        );

        let alternate = confirm_keyboard_paste_readback(
            PasteReadbackExpectation {
                expected_after: Some(expected),
                value_before: Some(before),
                insertion_end_caret_utf16: Some(expected.encode_utf16().count() as u64),
            },
            &CancellationToken::new(),
            || (true, true),
            || Some("presentation mismatch".into()),
            |_, _| FullRangeReadback::Verified,
            |_| {},
        );
        assert_eq!(
            alternate.proof,
            Some(PasteVerificationFact::VerifiedFullRange)
        );
        assert_eq!(
            classify_confirmed_paste_readback(
                false,
                alternate.proof,
                Some(expected),
                Some(before),
                alternate.value_after.as_deref(),
                true,
                true,
            ),
            PasteVerificationFact::ShortcutOutcomeUnknown
        );
    }

    #[test]
    fn fixture_source_admission_stops_before_selection_or_editability_reads() {
        for observed in [None, Some("different fixture value".to_owned())] {
            let reads = Arc::new(Mutex::new(Vec::new()));
            let value_reads = reads.clone();
            let range_reads = reads.clone();
            let selected_reads = reads.clone();
            let editability_reads = reads.clone();
            let result = capture_action_source_after_fixture_admission(
                "owned fixture",
                13,
                move |limit| {
                    value_reads.lock().unwrap().push(("value", limit));
                    observed.clone()
                },
                move || {
                    range_reads.lock().unwrap().push(("range", 0));
                    Some((0, 1))
                },
                move |limit| {
                    selected_reads.lock().unwrap().push(("selected", limit));
                    Some("x".into())
                },
                move |_, _| {
                    editability_reads.lock().unwrap().push(("editable", 0));
                    true
                },
            );
            assert!(matches!(
                result,
                Err(PasteError::InputUnavailable | PasteError::InputChanged)
            ));
            assert_eq!(*reads.lock().unwrap(), vec![("value", 13)]);
        }
    }

    #[test]
    fn fixture_source_admission_bounds_later_reads_on_the_same_known_value() {
        let reads = Arc::new(Mutex::new(Vec::new()));
        let value_reads = reads.clone();
        let range_reads = reads.clone();
        let selected_reads = reads.clone();
        let editability_reads = reads.clone();
        let (source, value) = capture_action_source_after_fixture_admission(
            "left REPLACE ONLY THIS right",
            29,
            move |limit| {
                value_reads.lock().unwrap().push(("value", limit));
                Some("left REPLACE ONLY THIS right".into())
            },
            move || {
                range_reads.lock().unwrap().push(("range", 0));
                Some((5, 17))
            },
            move |limit| {
                selected_reads.lock().unwrap().push(("selected", limit));
                Some("REPLACE ONLY THIS".into())
            },
            move |field_value, range| {
                editability_reads.lock().unwrap().push(("editable", 0));
                assert_eq!(field_value, "left REPLACE ONLY THIS right");
                assert_eq!(range, Some((5, 17)));
                true
            },
        )
        .unwrap();
        assert_eq!(value, "left REPLACE ONLY THIS right");
        assert_eq!(source.text, "REPLACE ONLY THIS");
        assert!(source.editable);
        assert_eq!(
            *reads.lock().unwrap(),
            vec![
                ("value", 29),
                ("range", 0),
                ("selected", 29),
                ("editable", 0)
            ]
        );
    }

    struct TestAnchor {
        identity: u64,
        // Mirror AX's !Send ownership rather than testing a freely movable object.
        _owner_local: std::rc::Rc<()>,
        owner_thread: std::thread::ThreadId,
        current: Arc<AtomicU64>,
        released: std::sync::mpsc::Sender<()>,
    }
    impl Drop for TestAnchor {
        fn drop(&mut self) {
            assert_eq!(self.owner_thread, std::thread::current().id());
            let _ = self.released.send(());
        }
    }

    fn test_anchor_ticket(
        lifetime: Duration,
    ) -> (
        AnchorTicket<TestAnchor>,
        Arc<AtomicU64>,
        std::sync::mpsc::Receiver<()>,
    ) {
        let current = Arc::new(AtomicU64::new(1));
        let native_current = current.clone();
        let (released, receiver) = std::sync::mpsc::channel();
        let ticket = run_delivery_owner(move |slot| {
            let anchor = TestAnchor {
                identity: 1,
                _owner_local: std::rc::Rc::new(()),
                owner_thread: std::thread::current().id(),
                current: native_current,
                released,
            };
            let (ticket, lease) = AnchorTicket::retain(anchor, lifetime);
            *slot = Some(lease);
            ticket
        })
        .unwrap();
        (ticket, current, receiver)
    }

    #[test]
    fn retained_owner_consumes_one_undo_then_keeps_serving_the_read_ticket() {
        let id = NEXT_ANCHOR_TICKET_ID.fetch_add(1, Ordering::Relaxed);
        let now = std::time::Instant::now();
        let deadline = now + Duration::from_secs(2);
        let (sender, requests) = std::sync::mpsc::sync_channel(4);
        let lease = AnchorLease {
            id,
            anchor: (),
            requests,
            undo_expires_at: Some(deadline),
            observation_expires_at: Some(deadline),
        };
        let undo_calls = Arc::new(AtomicU64::new(0));
        let read_calls = Arc::new(AtomicU64::new(0));

        let (first_reply, first_result) = std::sync::mpsc::channel();
        let first_calls = Arc::clone(&undo_calls);
        sender
            .send(AnchorRequest::Undo(UndoAnchorRequest {
                operation: Box::new(move |_, _, _| {
                    first_calls.fetch_add(1, Ordering::Relaxed);
                    Ok("success")
                }),
                cancellation: CancellationToken::new(),
                deadline,
                reply: first_reply,
            }))
            .unwrap();

        let (second_reply, second_result) = std::sync::mpsc::channel();
        let second_calls = Arc::clone(&undo_calls);
        sender
            .send(AnchorRequest::Undo(UndoAnchorRequest {
                operation: Box::new(move |_, _, _| {
                    second_calls.fetch_add(1, Ordering::Relaxed);
                    Ok("success")
                }),
                cancellation: CancellationToken::new(),
                deadline,
                reply: second_reply,
            }))
            .unwrap();

        let (read_reply, read_result) = std::sync::mpsc::channel();
        let observed_reads = Arc::clone(&read_calls);
        sender
            .send(AnchorRequest::Read(FocusedFieldReadRequest {
                operation: Box::new(move |_, _, _, _, _| {
                    observed_reads.fetch_add(1, Ordering::Relaxed);
                    Ok(FocusedFieldReadSampleV1 {
                        value: Some("retained field".into()),
                        selection_range_utf16: Some((4, 0)),
                        focus_matches: Some(true),
                    })
                }),
                cancellation: CancellationToken::new(),
                deadline,
                observer_epoch: Arc::new(AtomicU64::new(9)),
                expected_epoch: 9,
                reply: read_reply,
            }))
            .unwrap();
        sender
            .send(AnchorRequest::ReleaseObservation { ticket_id: id })
            .unwrap();

        let owner = thread::spawn(move || lease.serve(None::<crate::research_trace::LearningTask>));
        owner.join().unwrap();

        assert_eq!(first_result.recv().unwrap(), Ok("success"));
        assert_eq!(second_result.recv().unwrap(), Ok("stale_target"));
        assert_eq!(
            read_result.recv().unwrap(),
            Ok(FocusedFieldReadSampleV1 {
                value: Some("retained field".into()),
                selection_range_utf16: Some((4, 0)),
                focus_matches: Some(true),
            })
        );
        assert_eq!(undo_calls.load(Ordering::Relaxed), 1);
        assert_eq!(read_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn owner_rejects_a_queued_read_after_its_epoch_changes() {
        let id = NEXT_ANCHOR_TICKET_ID.fetch_add(1, Ordering::Relaxed);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let (sender, requests) = std::sync::mpsc::sync_channel(2);
        let lease = AnchorLease {
            id,
            anchor: (),
            requests,
            undo_expires_at: None,
            observation_expires_at: Some(deadline),
        };
        let observer_epoch = Arc::new(AtomicU64::new(9));
        let native_reads = Arc::new(AtomicU64::new(0));
        let read_counter = Arc::clone(&native_reads);
        let owner_epoch = Arc::clone(&observer_epoch);
        let (reply, result) = std::sync::mpsc::channel();
        sender
            .send(AnchorRequest::Read(FocusedFieldReadRequest {
                operation: Box::new(move |_, cancellation, owner_deadline, epoch, expected| {
                    owner_ax_read(cancellation, owner_deadline, epoch, expected, || {
                        read_counter.fetch_add(1, Ordering::Relaxed);
                        FocusedFieldReadSampleV1 {
                            value: Some("must not be read".into()),
                            selection_range_utf16: None,
                            focus_matches: Some(true),
                        }
                    })
                }),
                cancellation: CancellationToken::new(),
                deadline,
                observer_epoch: owner_epoch,
                expected_epoch: 9,
                reply,
            }))
            .unwrap();
        sender
            .send(AnchorRequest::ReleaseObservation { ticket_id: id })
            .unwrap();
        observer_epoch.store(10, Ordering::Release);

        let owner = thread::spawn(move || lease.serve(None::<crate::research_trace::LearningTask>));
        owner.join().unwrap();

        assert_eq!(
            result.recv().unwrap(),
            Err(FocusedFieldReadError::StaleEpoch)
        );
        assert_eq!(native_reads.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn invalidation_after_a_read_stops_before_the_next_native_read() {
        let cancellation = CancellationToken::new();
        let observer_epoch = AtomicU64::new(9);
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let value_reads = AtomicU64::new(0);
        let selection_reads = AtomicU64::new(0);

        let value = owner_ax_read(&cancellation, deadline, &observer_epoch, 9, || {
            value_reads.fetch_add(1, Ordering::Relaxed);
            observer_epoch.store(10, Ordering::Release);
            Some("value".to_owned())
        });
        assert_eq!(value, Err(FocusedFieldReadError::StaleEpoch));
        let selection = owner_ax_read(&cancellation, deadline, &observer_epoch, 9, || {
            selection_reads.fetch_add(1, Ordering::Relaxed);
            Some((0, 0))
        });
        assert_eq!(selection, Err(FocusedFieldReadError::StaleEpoch));
        assert_eq!(value_reads.load(Ordering::Relaxed), 1);
        assert_eq!(selection_reads.load(Ordering::Relaxed), 0);

        let cancellation = CancellationToken::new();
        let value_reads = AtomicU64::new(0);
        let selection_reads = AtomicU64::new(0);
        let epoch = AtomicU64::new(11);
        let value = owner_ax_read(&cancellation, deadline, &epoch, 11, || {
            value_reads.fetch_add(1, Ordering::Relaxed);
            cancellation.cancel();
            Some("value".to_owned())
        });
        assert_eq!(value, Err(FocusedFieldReadError::Cancelled));
        let selection = owner_ax_read(&cancellation, deadline, &epoch, 11, || {
            selection_reads.fetch_add(1, Ordering::Relaxed);
            Some((0, 0))
        });
        assert_eq!(selection, Err(FocusedFieldReadError::Cancelled));
        assert_eq!(value_reads.load(Ordering::Relaxed), 1);
        assert_eq!(selection_reads.load(Ordering::Relaxed), 0);
    }

    struct DropCounter(Arc<AtomicU64>);

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    struct DropSignal(std::sync::mpsc::Sender<()>);

    impl Drop for DropSignal {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }

    #[test]
    fn read_guard_expires_independently_while_undo_remains_available() {
        let id = NEXT_ANCHOR_TICKET_ID.fetch_add(1, Ordering::Relaxed);
        let now = std::time::Instant::now();
        let undo_deadline = now + Duration::from_secs(2);
        let (sender, requests) = std::sync::mpsc::sync_channel(1);
        let lease = AnchorLease {
            id,
            anchor: (),
            requests,
            undo_expires_at: Some(undo_deadline),
            observation_expires_at: Some(now + Duration::from_millis(100)),
        };
        let (guard_dropped, guard_dropped_receiver) = std::sync::mpsc::channel();
        let (owner_exited, owner_exited_receiver) = std::sync::mpsc::channel();
        let owner = thread::spawn(move || {
            lease.serve(Some(DropSignal(guard_dropped)));
            owner_exited.send(()).unwrap();
        });

        guard_dropped_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        let (undo_reply, undo_result) = std::sync::mpsc::channel();
        sender
            .send(AnchorRequest::Undo(UndoAnchorRequest {
                operation: Box::new(|_, _, _| Ok("success")),
                cancellation: CancellationToken::new(),
                deadline: undo_deadline,
                reply: undo_reply,
            }))
            .unwrap();
        assert_eq!(undo_result.recv().unwrap(), Ok("success"));
        owner_exited_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        owner.join().unwrap();
    }

    #[test]
    fn observation_owner_guard_waits_for_an_inflight_read_to_return() {
        let id = NEXT_ANCHOR_TICKET_ID.fetch_add(1, Ordering::Relaxed);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let (sender, requests) = std::sync::mpsc::sync_channel(2);
        let lease = AnchorLease {
            id,
            anchor: (),
            requests,
            undo_expires_at: None,
            observation_expires_at: Some(deadline),
        };
        let (read_started, read_started_receiver) = std::sync::mpsc::channel();
        let (finish_read, finish_read_receiver) = std::sync::mpsc::channel();
        let (read_reply, read_result) = std::sync::mpsc::channel();
        let cancellation = CancellationToken::new();
        sender
            .send(AnchorRequest::Read(FocusedFieldReadRequest {
                operation: Box::new(move |_, cancellation, _, _, _| {
                    read_started.send(()).unwrap();
                    finish_read_receiver.recv().unwrap();
                    if cancellation.is_cancelled() {
                        Err(FocusedFieldReadError::Cancelled)
                    } else {
                        Ok(FocusedFieldReadSampleV1 {
                            value: Some("late value".into()),
                            selection_range_utf16: None,
                            focus_matches: Some(true),
                        })
                    }
                }),
                cancellation: cancellation.clone(),
                deadline,
                observer_epoch: Arc::new(AtomicU64::new(1)),
                expected_epoch: 1,
                reply: read_reply,
            }))
            .unwrap();
        let (owner_exited, owner_exited_receiver) = std::sync::mpsc::channel();
        let owner_guard_drops = Arc::new(AtomicU64::new(0));
        let owner_guard = DropCounter(Arc::clone(&owner_guard_drops));
        let owner = thread::spawn(move || {
            lease.serve(Some(owner_guard));
            owner_exited.send(()).unwrap();
        });

        read_started_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        cancellation.cancel();
        drop(read_result); // The caller timed out and no longer accepts a reply.
        sender
            .send(AnchorRequest::ReleaseObservation { ticket_id: id })
            .unwrap();
        assert_eq!(owner_guard_drops.load(Ordering::Relaxed), 0);
        assert!(owner_exited_receiver.try_recv().is_err());

        finish_read.send(()).unwrap();
        owner_exited_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        owner.join().unwrap();
        assert_eq!(owner_guard_drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn undo_anchor_ticket_releases_on_last_drop_without_join() {
        let (ticket, _, released) = test_anchor_ticket(Duration::from_secs(3));
        let second_owner = ticket.clone();
        drop(ticket);
        assert!(released.try_recv().is_err());
        drop(second_owner);
        released.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn undo_anchor_ticket_expires_and_rejects_later_requests() {
        let (ticket, _, released) = test_anchor_ticket(Duration::from_millis(30));
        released.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            ticket.run(|_, _, _| panic!("expired anchor must not run")),
            Ok("stale_target")
        );
    }

    #[test]
    fn undo_anchor_ticket_rejects_replacement_field_with_identical_value() {
        let (ticket, current, released) = test_anchor_ticket(Duration::from_secs(3));
        // Same app, rectangle and text may describe a different AX element.
        // The retained anchor's actual identity must still agree with focus.
        current.store(2, Ordering::Release);
        let submissions = Arc::new(AtomicU64::new(0));
        let worker_submissions = submissions.clone();
        assert_eq!(
            ticket.run(move |anchor, cancellation, deadline| {
                run_guarded_undo(
                    cancellation,
                    deadline,
                    || anchor.identity == anchor.current.load(Ordering::Acquire),
                    || {
                        worker_submissions.fetch_add(1, Ordering::Relaxed);
                        Ok(())
                    },
                )
            }),
            Ok("stale_target")
        );
        assert_eq!(submissions.load(Ordering::Relaxed), 0);
        released.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn undo_original_owner_rejects_capture_period_aba_with_same_value_and_geometry() {
        let current = Arc::new(AtomicU64::new(1));
        let native_current = current.clone();
        let captures = Arc::new(AtomicU64::new(0));
        let native_captures = captures.clone();
        let (released, release_receiver) = std::sync::mpsc::channel();
        let ticket = run_delivery_owner(move |slot| {
            native_captures.fetch_add(1, Ordering::Relaxed);
            let original = TestAnchor {
                identity: native_current.load(Ordering::Acquire),
                _owner_local: std::rc::Rc::new(()),
                owner_thread: std::thread::current().id(),
                current: native_current.clone(),
                released,
            };
            assert_eq!(original.identity, 1); // A accepted the paste/readback.
                                              // A -> B while the old implementation would recapture focus.
                                              // Both controls have exactly the same text/geometry metadata.
            native_current.store(2, Ordering::Release);
            assert_ne!(original.identity, native_current.load(Ordering::Acquire));
            // Return to A so the old original-anchor post-check would pass.
            native_current.store(1, Ordering::Release);
            assert_eq!(original.identity, native_current.load(Ordering::Acquire));
            let (ticket, lease) = AnchorTicket::retain(original, Duration::from_secs(3));
            *slot = Some(lease);
            ticket
        })
        .unwrap();
        // Switch to B before Undo. A recaptured B ticket would wrongly allow it.
        current.store(2, Ordering::Release);
        let submissions = Arc::new(AtomicU64::new(0));
        let native_submissions = submissions.clone();
        assert_eq!(
            ticket.run(move |anchor, cancellation, deadline| {
                assert_eq!(anchor.identity, 1);
                assert_eq!(anchor.owner_thread, std::thread::current().id());
                run_guarded_undo(
                    cancellation,
                    deadline,
                    || anchor.identity == anchor.current.load(Ordering::Acquire),
                    || {
                        native_submissions.fetch_add(1, Ordering::Relaxed);
                        Ok(())
                    },
                )
            }),
            Ok("stale_target")
        );
        assert_eq!(captures.load(Ordering::Relaxed), 1);
        assert_eq!(submissions.load(Ordering::Relaxed), 0);
        release_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
    }

    #[test]
    fn undo_anchor_ticket_submits_once_for_the_retained_field() {
        let (ticket, _, released) = test_anchor_ticket(Duration::from_secs(3));
        let submissions = Arc::new(AtomicU64::new(0));
        let worker_submissions = submissions.clone();
        assert_eq!(
            ticket.run(move |anchor, cancellation, deadline| {
                run_guarded_undo(
                    cancellation,
                    deadline,
                    || anchor.identity == anchor.current.load(Ordering::Acquire),
                    || {
                        worker_submissions.fetch_add(1, Ordering::Relaxed);
                        Ok(())
                    },
                )
            }),
            Ok("success")
        );
        released.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(submissions.load(Ordering::Relaxed), 1);
        assert_eq!(
            ticket.run(|_, _, _| panic!("consumed native anchor must not be reused")),
            Ok("stale_target")
        );
    }

    #[test]
    fn undo_anchor_ticket_timeout_cancels_work_before_submission() {
        let (ticket, _, released) = test_anchor_ticket(Duration::from_secs(3));
        let (started, start_receiver) = std::sync::mpsc::channel();
        let submissions = Arc::new(AtomicU64::new(0));
        let worker_submissions = submissions.clone();
        assert_eq!(
            ticket.run_with_timeout(
                move |_, cancellation, deadline| {
                    started.send(()).unwrap();
                    while !cancellation.is_cancelled() && std::time::Instant::now() < deadline {
                        thread::yield_now();
                    }
                    run_guarded_undo(
                        cancellation,
                        deadline,
                        || true,
                        || {
                            worker_submissions.fetch_add(1, Ordering::Relaxed);
                            Ok(())
                        },
                    )
                },
                Duration::from_millis(40)
            ),
            Ok("stale_target")
        );
        start_receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        released.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(submissions.load(Ordering::Relaxed), 0);
    }

    struct FakeClipboardRestore {
        items: Vec<String>,
        observation: ClipboardWriteObservation,
        external_write_before_observe: Option<(Vec<String>, ClipboardWriteObservation)>,
        restore_count: usize,
    }

    impl ClipboardRestoreProtocol<Vec<String>> for FakeClipboardRestore {
        type Prepared = Vec<String>;

        fn prepare_restore(
            &mut self,
            snapshot: &Vec<String>,
        ) -> Result<Self::Prepared, PasteError> {
            Ok(snapshot.clone())
        }

        fn observe(&mut self) -> Result<ClipboardWriteObservation, PasteError> {
            if let Some((items, observation)) = self.external_write_before_observe.take() {
                self.items = items;
                self.observation = observation;
            }
            Ok(self.observation.clone())
        }

        fn commit_restore(
            &mut self,
            prepared: Self::Prepared,
        ) -> Result<Option<isize>, PasteError> {
            self.items = prepared;
            self.restore_count += 1;
            Ok(None)
        }
    }

    #[test]
    fn external_receipt_restoration_comes_from_actual_commit_not_scheduling() {
        let proof = owned_synthetic_proof();
        let mut backend = FakeClipboardRestore {
            items: vec!["temporary".into()],
            observation: ClipboardWriteObservation {
                change_count: 11,
                text: Some("synthetic B".into()),
                token: Some("voiceflow-token-1".into()),
                has_exact_temporary_types: true,
            },
            external_write_before_observe: None,
            restore_count: 0,
        };
        let snapshot = vec!["fixture-start clipboard".into()];
        assert_eq!(
            restoration_fact(true, false, Some(true), true, None),
            ClipboardRestoration::Unknown
        );
        let restored = restore_clipboard_if_owned(&mut backend, &snapshot, Some(&proof))
            .unwrap()
            .restored;
        assert_eq!(
            restoration_fact(true, false, Some(true), true, Some(restored)),
            ClipboardRestoration::Restored
        );
        assert_eq!(backend.items, snapshot);
        assert_eq!(backend.restore_count, 1);
        backend.observation.change_count += 1;
        let restored = restore_clipboard_if_owned(&mut backend, &snapshot, Some(&proof))
            .unwrap()
            .restored;
        assert_eq!(
            restoration_fact(true, false, Some(true), true, Some(restored)),
            ClipboardRestoration::OwnershipChanged
        );
        assert_eq!(backend.restore_count, 1);
    }

    #[test]
    fn controlled_rejection_never_recovers_but_ordinary_delivery_retains_one_copy() {
        for target_changed in [true, false] {
            for intent in [RecoveryIntent::NoImplicitCopy, RecoveryIntent::PreserveText] {
                let observation = run_delivery_owner::<(), _>(move |_| {
                    let cancellation = CancellationToken::new();
                    let mut trace = DeliveryTrace::new(true, 42);
                    let calls = std::cell::Cell::new(0);
                    let writes = std::cell::Cell::new(0);
                    let keyboard = std::cell::Cell::new(0);
                    let clipboard = std::cell::RefCell::new("owned original".to_owned());
                    // The same real admission owner admits the initial captured
                    // fixture, then the current callback reports user drift.
                    admit_before_clipboard_write(
                        true,
                        &cancellation,
                        || {
                            calls.set(calls.get() + 1);
                            Ok(())
                        },
                        &mut trace,
                    )
                    .unwrap();
                    let result: Result<(), PasteError> = attempt_with_recovery(
                        intent,
                        &cancellation,
                        &mut trace,
                        |trace| {
                            admit_before_clipboard_write(
                                true,
                                &cancellation,
                                || {
                                    calls.set(calls.get() + 1);
                                    if target_changed {
                                        Err(PasteError::TargetChanged)
                                    } else {
                                        Err(PasteError::SelectionChanged)
                                    }
                                },
                                trace,
                            )?;
                            writes.set(writes.get() + 1);
                            trace.clipboard_write_attempted = true;
                            keyboard.set(keyboard.get() + 1);
                            Ok(())
                        },
                        |trace| {
                            writes.set(writes.get() + 1);
                            trace.clipboard_write_attempted = true;
                            *clipboard.borrow_mut() = "recoverable text".into();
                        },
                    );
                    let original_error = if target_changed {
                        matches!(result, Err(PasteError::TargetChanged))
                    } else {
                        matches!(result, Err(PasteError::SelectionChanged))
                    };
                    (
                        calls.get(),
                        writes.get(),
                        keyboard.get(),
                        clipboard.into_inner(),
                        original_error,
                    )
                })
                .unwrap();
                assert_eq!(observation.0, 2);
                assert_eq!(observation.2, 0);
                assert!(
                    observation.4,
                    "recovery must preserve the original tagged failure"
                );
                match intent {
                    RecoveryIntent::NoImplicitCopy => {
                        assert_eq!(observation.1, 0);
                        assert_eq!(observation.3, "owned original");
                    }
                    RecoveryIntent::PreserveText => {
                        assert_eq!(observation.1, 1);
                        assert_eq!(observation.3, "recoverable text");
                    }
                }
            }
        }
    }
    #[test]
    fn controlled_success_keeps_once_only_delivery_and_actual_restore_commit_fact() {
        let observed = run_delivery_owner::<(), _>(move |_| {
            let cancellation = CancellationToken::new();
            let mut trace = DeliveryTrace::new(true, 42);
            let writes = std::cell::Cell::new(0);
            let keyboard = std::cell::Cell::new(0);
            let outcome = attempt_with_recovery(
                RecoveryIntent::NoImplicitCopy,
                &cancellation,
                &mut trace,
                |trace| {
                    admit_before_clipboard_write(true, &cancellation, || Ok(()), trace)?;
                    writes.set(writes.get() + 1);
                    trace.clipboard_write_attempted = true;
                    keyboard.set(keyboard.get() + 1);
                    let proof = owned_synthetic_proof();
                    let mut backend = FakeClipboardRestore {
                        items: vec!["temporary".into()],
                        observation: ClipboardWriteObservation {
                            change_count: 11,
                            text: Some("synthetic B".into()),
                            token: Some("voiceflow-token-1".into()),
                            has_exact_temporary_types: true,
                        },
                        external_write_before_observe: None,
                        restore_count: 0,
                    };
                    let snapshot = vec!["owned original".into()];
                    let restored =
                        restore_clipboard_if_owned(&mut backend, &snapshot, Some(&proof))?.restored;
                    trace.clipboard_restore_attempted = true;
                    trace.clipboard_restored = Some(restored);
                    Ok((backend.items, backend.restore_count))
                },
                |_| panic!("successful controlled delivery must not recover"),
            );
            (
                writes.get(),
                keyboard.get(),
                outcome.unwrap(),
                restoration_fact(
                    trace.clipboard_write_attempted,
                    trace.clipboard_write_failed,
                    trace.clipboard_write_owned,
                    trace.clipboard_restore_attempted,
                    trace.clipboard_restored,
                ),
            )
        })
        .unwrap();
        assert_eq!(observed.0, 1);
        assert_eq!(observed.1, 1);
        assert_eq!(observed.2, (vec!["owned original".to_owned()], 1));
        assert_eq!(observed.3, ClipboardRestoration::Restored);
    }
    #[test]
    fn recovery_intent_never_changes_cancel_or_postwrite_new_copy_ownership() {
        for intent in [RecoveryIntent::NoImplicitCopy, RecoveryIntent::PreserveText] {
            let observed = run_delivery_owner::<(), _>(move |_| {
                let cancellation = CancellationToken::new();
                let mut trace = DeliveryTrace::new(true, 42);
                admit_before_clipboard_write(true, &cancellation, || Ok(()), &mut trace).unwrap();
                cancellation.cancel();
                let result: Result<(), PasteError> = attempt_with_recovery(
                    intent,
                    &cancellation,
                    &mut trace,
                    |trace| {
                        admit_before_clipboard_write(
                            true,
                            &cancellation,
                            || panic!("cancelled before callback"),
                            trace,
                        )
                    },
                    |_| panic!("cancellation cannot request recovery"),
                );
                assert!(matches!(result, Err(PasteError::Cancelled)));
                assert!(!trace.clipboard_write_attempted);
                // A newer user copy remains protected by the unchanged real
                // restoration protocol, regardless of delivery intent.
                let proof = owned_synthetic_proof();
                let mut backend = FakeClipboardRestore {
                    items: vec!["new user copy".into()],
                    observation: ClipboardWriteObservation {
                        change_count: 12,
                        text: Some("new user copy".into()),
                        token: None,
                        has_exact_temporary_types: false,
                    },
                    external_write_before_observe: None,
                    restore_count: 0,
                };
                let restored =
                    restore_clipboard_if_owned(&mut backend, &vec!["old".into()], Some(&proof))
                        .unwrap()
                        .restored;
                (restored, backend.restore_count, backend.items)
            })
            .unwrap();
            assert_eq!(observed, (false, 0, vec!["new user copy".to_owned()]));
        }
    }

    fn owned_synthetic_proof() -> ClipboardWriteProof {
        prove_clipboard_write_ownership(
            10,
            "synthetic B",
            "voiceflow-token-1",
            &ClipboardWriteObservation {
                change_count: 11,
                text: Some("synthetic B".into()),
                token: Some("voiceflow-token-1".into()),
                has_exact_temporary_types: true,
            },
        )
        .unwrap()
    }

    #[test]
    fn clipboard_restore_requires_the_exact_write_count_text_and_unique_marker() {
        let proof_observation = ClipboardWriteObservation {
            change_count: 11,
            text: Some("synthetic B".into()),
            token: Some("voiceflow-token-1".into()),
            has_exact_temporary_types: true,
        };
        let proof = prove_clipboard_write_ownership(
            10,
            "synthetic B",
            "voiceflow-token-1",
            &proof_observation,
        )
        .unwrap();
        assert!(clipboard_write_proof_is_current(&proof, &proof_observation));

        let newer_c = ClipboardWriteObservation {
            change_count: 12,
            text: Some("synthetic C".into()),
            token: None,
            has_exact_temporary_types: false,
        };
        assert!(
            !clipboard_write_proof_is_current(&proof, &newer_c),
            "a newer external clipboard is never treated as VoiceFlow's write"
        );

        let same_text_external_write = ClipboardWriteObservation {
            change_count: 11,
            text: Some("synthetic B".into()),
            token: None,
            has_exact_temporary_types: false,
        };
        assert!(
            prove_clipboard_write_ownership(
                10,
                "synthetic B",
                "voiceflow-token-1",
                &same_text_external_write
            )
            .is_none(),
            "text equality alone cannot claim clipboard ownership"
        );

        let failed_write_race = ClipboardWriteObservation {
            change_count: 12,
            text: Some("synthetic C".into()),
            token: None,
            has_exact_temporary_types: false,
        };
        let mut restored = false;
        if let Some(proof) = prove_clipboard_write_ownership(
            10,
            "synthetic B",
            "voiceflow-token-1",
            &failed_write_race,
        ) {
            restored = clipboard_write_proof_is_current(&proof, &failed_write_race);
        }
        assert!(!restored, "failed-write/cancel race preserves external C");
    }

    #[test]
    fn external_write_before_marker_acquisition_is_not_claimed_or_restored() {
        let external = ClipboardWriteObservation {
            change_count: 12,
            text: Some("synthetic C".into()),
            token: None,
            has_exact_temporary_types: false,
        };
        let proof =
            prove_clipboard_write_ownership(10, "synthetic B", "voiceflow-token-1", &external);
        assert!(proof.is_none());

        let mut backend = FakeClipboardRestore {
            items: vec!["synthetic C".into()],
            observation: external,
            external_write_before_observe: None,
            restore_count: 0,
        };
        assert!(
            !restore_clipboard_if_owned(&mut backend, &vec!["synthetic A".into()], proof.as_ref())
                .unwrap()
                .restored
        );
        assert_eq!(backend.items, ["synthetic C"]);
        assert_eq!(backend.restore_count, 0);
    }

    #[test]
    fn cancellation_restore_rechecks_ownership_after_snapshot_preparation() {
        let proof = owned_synthetic_proof();
        let mut backend = FakeClipboardRestore {
            items: vec!["synthetic B".into()],
            observation: ClipboardWriteObservation {
                change_count: 11,
                text: Some("synthetic B".into()),
                token: Some("voiceflow-token-1".into()),
                has_exact_temporary_types: true,
            },
            external_write_before_observe: Some((
                vec!["synthetic C".into()],
                ClipboardWriteObservation {
                    change_count: 12,
                    text: Some("synthetic C".into()),
                    token: None,
                    has_exact_temporary_types: false,
                },
            )),
            restore_count: 0,
        };

        assert!(
            !restore_clipboard_if_owned(&mut backend, &vec!["synthetic A".into()], Some(&proof))
                .unwrap()
                .restored
        );
        assert_eq!(backend.items, ["synthetic C"]);
        assert_eq!(backend.restore_count, 0);
    }

    #[test]
    fn cancel_before_shortcut_restores_owned_clipboard() {
        let proof = owned_synthetic_proof();
        let snapshot = vec!["synthetic original A".to_owned()];
        let mut backend = FakeClipboardRestore {
            items: vec!["synthetic temporary B".into()],
            observation: ClipboardWriteObservation {
                change_count: 11,
                text: Some("synthetic B".into()),
                token: Some("voiceflow-token-1".into()),
                has_exact_temporary_types: true,
            },
            external_write_before_observe: None,
            restore_count: 0,
        };
        let mut trace = DeliveryTrace {
            clipboard_write_attempted: true,
            clipboard_write_owned: Some(true),
            ..DeliveryTrace::default()
        };

        restore_cancelled_clipboard_if_safe(&mut trace, false, || {
            restore_clipboard_if_owned(&mut backend, &snapshot, Some(&proof)).map(|result| {
                if result.restored {
                    ClipboardRestoreAttemptV1::Restored
                } else {
                    ClipboardRestoreAttemptV1::OwnershipChanged
                }
            })
        });

        assert_eq!(backend.items, snapshot);
        assert_eq!(backend.restore_count, 1);
        assert!(trace.clipboard_restore_attempted);
        assert_eq!(trace.clipboard_restored, Some(true));
        assert_eq!(
            restoration_fact(
                trace.clipboard_write_attempted,
                trace.clipboard_write_failed,
                trace.clipboard_write_owned,
                trace.clipboard_restore_attempted,
                trace.clipboard_restored,
            ),
            ClipboardRestoration::Restored
        );
    }

    #[test]
    fn cancellation_does_not_restore_after_possible_shortcut_or_unknown_ownership() {
        let cases = [(true, Some(true)), (false, None), (false, Some(false))];
        for (shortcut_may_have_been_posted, owned) in cases {
            let mut trace = DeliveryTrace {
                clipboard_write_attempted: true,
                clipboard_write_owned: owned,
                ..DeliveryTrace::default()
            };
            restore_cancelled_clipboard_if_safe(&mut trace, shortcut_may_have_been_posted, || {
                panic!("unsafe cancellation must not call restore")
            });
            assert!(!trace.clipboard_restore_attempted);
            assert_eq!(trace.clipboard_restored, None);
            assert_eq!(
                trace.keyboard_paste_may_have_been_posted,
                shortcut_may_have_been_posted
            );
        }
    }

    #[test]
    fn failed_write_restore_never_overwrites_an_unattributed_external_clipboard() {
        let external = ClipboardWriteObservation {
            change_count: 12,
            text: Some("synthetic C".into()),
            token: None,
            has_exact_temporary_types: false,
        };
        let unproven =
            prove_clipboard_write_ownership(10, "synthetic B", "voiceflow-token-1", &external);
        let mut backend = FakeClipboardRestore {
            items: vec!["synthetic C".into()],
            observation: external,
            external_write_before_observe: None,
            restore_count: 0,
        };
        assert!(
            !restore_clipboard_if_owned(
                &mut backend,
                &vec!["synthetic A".into()],
                unproven.as_ref()
            )
            .unwrap()
            .restored
        );
        assert_eq!(backend.items, ["synthetic C"]);
        assert_eq!(backend.restore_count, 0);
    }

    fn action_source(
        text: &str,
        field: &str,
        range: Option<(i64, i64)>,
    ) -> CapturedTextActionSource {
        CapturedTextActionSource {
            kind: TextActionSourceKind::Selection,
            text: text.to_owned(),
            text_fingerprint: selection_fingerprint(text),
            field_fingerprint: Some(selection_fingerprint(field)),
            selection_range: range,
            editable: true,
            copy_only_selection: range.is_none(),
        }
    }

    #[test]
    fn invalid_utf16_selection_does_not_fall_back_to_the_whole_field() {
        assert!(matches!(
            resolve_text_action_source(Some("甲😀乙"), Some((2, 1)), None),
            Err(PasteError::SelectionUnavailable)
        ));
    }

    #[test]
    fn ax_selection_without_field_range_remains_generation_only() {
        let resolved = resolve_text_action_source(None, None, Some("selected".into())).unwrap();
        assert_eq!(resolved.0, TextActionSourceKind::Selection);
        assert_eq!(resolved.1, "selected");
        assert_eq!(resolved.2, None);
    }

    #[test]
    fn full_field_ax_readback_never_retries_an_unexpected_mutation() {
        assert_eq!(
            classify_ax_full_field_write("before", "after", Some("before".into()), false),
            AxInsertAttempt::NoOp
        );
        assert_eq!(
            classify_ax_full_field_write("before", "after", Some("after".into()), false),
            AxInsertAttempt::Verified("after".into())
        );
        assert_eq!(
            classify_ax_full_field_write("before", "after", Some("unexpected".into()), false),
            AxInsertAttempt::Uncertain
        );
        assert_eq!(
            classify_ax_full_field_write("before", "after", None, false),
            AxInsertAttempt::Uncertain
        );
    }

    #[test]
    fn source_snapshot_rejects_the_same_text_at_a_different_occurrence() {
        let field = "same then same";
        let first = action_source("same", field, Some((0, 4)));
        let second = action_source("same", field, Some((10, 4)));
        assert!(!text_action_source_matches(&first, &second));
    }

    #[test]
    fn source_snapshot_rejects_a_changed_field_version_with_same_selection() {
        let expected = action_source("same", "same before", Some((0, 4)));
        let current = action_source("same", "same after", Some((0, 4)));
        assert!(!text_action_source_matches(&expected, &current));
    }

    #[test]
    fn source_snapshot_binds_the_caret_for_full_field_actions() {
        let source = "rewrite this whole field";
        let field_fingerprint = selection_fingerprint(source);
        let expected = CapturedTextActionSource {
            kind: TextActionSourceKind::FieldText,
            text: source.into(),
            text_fingerprint: field_fingerprint,
            field_fingerprint: Some(field_fingerprint),
            selection_range: Some((3, 0)),
            editable: true,
            copy_only_selection: false,
        };
        let mut current = expected.clone();
        current.selection_range = Some((8, 0));
        assert!(!text_action_source_matches(&expected, &current));
    }

    #[test]
    fn source_snapshot_accepts_the_same_selection_and_field_version() {
        let expected = action_source("same", "same before", Some((0, 4)));
        let current = action_source("same", "same before", Some((0, 4)));
        assert!(text_action_source_matches(&expected, &current));
    }

    #[test]
    fn utf16_range_to_bytes_handles_cjk_and_supplementary_characters() {
        assert_eq!(utf16_range_to_bytes("甲😀乙", 0, 1), Some((0, 3)));
        assert_eq!(utf16_range_to_bytes("甲😀乙", 1, 2), Some((3, 7)));
        assert_eq!(utf16_range_to_bytes("甲😀乙", 3, 1), Some((7, 10)));
        assert_eq!(utf16_range_to_bytes("甲😀乙", 2, 1), None);
    }

    #[test]
    fn confirmed_insertion_records_checked_utf16_ranges_only_for_exact_splices() {
        let target = crate::context::TargetAppGuard {
            pid: 42,
            bundle_id: Some("com.example.editor".into()),
            browser_host: None,
            browser_target_token: None,
            browser_tab_token: None,
            window_token: Some(1),
            window_id: Some(1),
            window_identity_ambiguous: false,
            input_token: Some(1),
            secure_input: false,
        };
        let confirmed = confirmed_insertion_v1("A🙂x", Some((3, 1)), "xy", "A🙂xy", target.clone())
            .expect("the exact verified splice has valid UTF-16 boundaries");
        assert_eq!(confirmed.source_field_utf16_length, 4);
        assert_eq!(confirmed.inserted_field_utf16_length, 5);
        assert_eq!(
            confirmed.replaced_range_before,
            Utf16RangeV1 {
                start: 3,
                length: 1
            }
        );
        assert_eq!(
            confirmed.inserted_range_after,
            Utf16RangeV1 {
                start: 3,
                length: 2
            }
        );
        assert_eq!(confirmed.inserted_text_utf16_length, 2);
        assert_eq!(confirmed.target_identity, target);
        assert_eq!(confirmed.readback, PasteVerificationFact::Verified);

        assert!(
            confirmed_insertion_v1("A🙂x", Some((2, 1)), "z", "A🙂zx", target.clone()).is_none(),
            "a range that cuts the surrogate pair cannot mint a learning fact"
        );
        assert!(
            confirmed_insertion_v1("A🙂x", Some((3, 1)), "xy", "A🙂xz", target).is_none(),
            "a mismatched post-insert readback cannot mint a learning fact"
        );
    }

    #[test]
    fn cancelled_attempt_never_runs_keyboard_injection() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let called = Arc::new(Mutex::new(false));
        let called_in_simulate = called.clone();

        let result = run_paste_attempt(
            &cancellation,
            || Ok(()),
            || Ok(()),
            move |_| {
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
    fn post_permit_is_after_final_check_and_failure_never_simulates() {
        let cancellation = CancellationToken::new();
        let calls = std::cell::RefCell::new(Vec::new());
        let simulated = std::cell::Cell::new(false);

        let result = run_paste_attempt(
            &cancellation,
            || {
                calls.borrow_mut().push("final_check");
                Ok(())
            },
            || {
                calls.borrow_mut().push("post_permit");
                Err(PasteError::Clipboard("synthetic permit denied".into()))
            },
            |_| {
                simulated.set(true);
                Ok(())
            },
        );

        assert_eq!(*calls.borrow(), ["final_check", "post_permit"]);
        assert!(!simulated.get());
        assert!(!result.shortcut_sent);
        assert!(matches!(result.result, Err(PasteError::Clipboard(_))));
    }

    #[test]
    fn registered_post_rechecks_same_target_after_ack_before_simulation() {
        let cancellation = CancellationToken::new();
        let calls = std::cell::RefCell::new(Vec::new());
        let result = run_paste_attempt_with_post_ack_recheck(
            &cancellation,
            || {
                calls.borrow_mut().push("pre_ack_target_check");
                Ok(())
            },
            || {
                calls.borrow_mut().push("remote_post_ack");
                Ok(())
            },
            || {
                calls.borrow_mut().push("registered_target_recheck");
                Ok(())
            },
            |_| {
                calls.borrow_mut().push("irreversible_post");
                Ok(())
            },
        );

        assert!(result.result.is_ok());
        assert_eq!(
            *calls.borrow(),
            [
                "pre_ack_target_check",
                "remote_post_ack",
                "registered_target_recheck",
                "irreversible_post"
            ]
        );
    }

    #[test]
    fn registered_post_recheck_failure_stops_before_simulation() {
        let cancellation = CancellationToken::new();
        let simulated = std::cell::Cell::new(false);
        let result = run_paste_attempt_with_post_ack_recheck(
            &cancellation,
            || Ok(()),
            || Ok(()),
            || Err(PasteError::TargetChanged),
            |_| {
                simulated.set(true);
                Ok(())
            },
        );

        assert!(!result.shortcut_sent);
        assert!(matches!(result.result, Err(PasteError::TargetChanged)));
        assert!(!simulated.get());
    }

    #[test]
    fn registered_post_cancellation_after_ack_stops_before_simulation() {
        let cancellation = CancellationToken::new();
        let cancellation_in_recheck = cancellation.clone();
        let simulated = std::cell::Cell::new(false);
        let result = run_paste_attempt_with_post_ack_recheck(
            &cancellation,
            || Ok(()),
            || Ok(()),
            move || {
                cancellation_in_recheck.cancel();
                Ok(())
            },
            |_| {
                simulated.set(true);
                Ok(())
            },
        );

        assert!(!result.shortcut_sent);
        assert!(matches!(result.result, Err(PasteError::Cancelled)));
        assert!(!simulated.get());
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    #[test]
    fn registered_remote_initial_source_rejects_missing_or_wrong_facts_before_effects() {
        let clipboard_writes = std::cell::Cell::new(0);
        let posts = std::cell::Cell::new(0);
        let rejected = [
            (
                None,
                Some(REGISTERED_RESEARCH_HOST_INITIAL_SELECTION),
                true,
                false,
            ),
            (
                Some(REGISTERED_RESEARCH_HOST_INITIAL_FIELD),
                None,
                false,
                true,
            ),
            (
                Some(REGISTERED_RESEARCH_HOST_INITIAL_FIELD),
                Some((5, 0)),
                false,
                true,
            ),
            (
                Some("TypeX: "),
                Some(REGISTERED_RESEARCH_HOST_INITIAL_SELECTION),
                true,
                false,
            ),
        ];

        for (value, selection_range, input_changed, selection_changed) in rejected {
            let result = with_registered_remote_initial_source(value, selection_range, || {
                clipboard_writes.set(clipboard_writes.get() + 1);
                posts.set(posts.get() + 1);
            });
            if input_changed {
                assert!(matches!(result, Err(PasteError::InputChanged)));
            } else if selection_changed {
                assert!(matches!(result, Err(PasteError::SelectionChanged)));
            } else {
                panic!("registered source fixture expected rejection");
            }
            assert_eq!(clipboard_writes.get(), 0);
            assert_eq!(posts.get(), 0);
        }

        let admitted = with_registered_remote_initial_source(
            Some(REGISTERED_RESEARCH_HOST_INITIAL_FIELD),
            Some(REGISTERED_RESEARCH_HOST_INITIAL_SELECTION),
            || {
                clipboard_writes.set(clipboard_writes.get() + 1);
            },
        );
        assert!(admitted.is_ok());
        assert_eq!(clipboard_writes.get(), 1);
        assert_eq!(posts.get(), 0);
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    #[test]
    fn registered_remote_post_rechecks_declared_selection_after_ack() {
        let cancellation = CancellationToken::new();
        let simulated = std::cell::Cell::new(false);
        let result = run_paste_attempt_with_post_ack_recheck(
            &cancellation,
            || Ok(()),
            || Ok(()),
            || {
                registered_remote_initial_source_matches(
                    Some(REGISTERED_RESEARCH_HOST_INITIAL_FIELD),
                    Some((5, 0)),
                )
            },
            |_| {
                simulated.set(true);
                Ok(())
            },
        );

        assert!(!result.shortcut_sent);
        assert!(matches!(result.result, Err(PasteError::SelectionChanged)));
        assert!(!simulated.get());
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    #[test]
    fn registered_post_rechecks_expiry_after_live_permit_before_native_post() {
        let cancellation = CancellationToken::new();
        let expires_at = std::cell::Cell::new(std::time::Instant::now() + Duration::from_secs(1));
        let permit_was_live = std::cell::Cell::new(false);
        let native_post = std::cell::Cell::new(false);
        let result = run_paste_attempt_with_post_ack_recheck(
            &cancellation,
            || Ok(()),
            || {
                registered_research_permission(&cancellation, expires_at.get())?;
                permit_was_live.set(true);
                // Model the bounded authorization/readback work crossing the
                // original deadline without sleeping or issuing a native event.
                expires_at.set(std::time::Instant::now());
                Ok(())
            },
            || Ok(()),
            |_| {
                with_registered_post_permission(&cancellation, expires_at.get(), || {
                    native_post.set(true);
                    Ok(())
                })
            },
        );

        assert!(permit_was_live.get());
        assert!(!result.shortcut_sent);
        assert!(matches!(result.result, Err(PasteError::Cancelled)));
        assert!(!native_post.get());
    }

    #[test]
    fn registered_post_target_requires_a_positive_non_self_pid() {
        assert_eq!(registered_post_target_pid(4242, 99), Some(4242));
        assert_eq!(registered_post_target_pid(99, 99), None);
        assert_eq!(registered_post_target_pid(0, 99), None);
        assert_eq!(registered_post_target_pid(-3, 99), None);
    }

    #[cfg(all(target_os = "macos", feature = "research-harness"))]
    #[test]
    fn recorder_only_and_registered_post_permits_are_not_interchangeable() {
        let recorder = RemoteResearchClipboardSessionV1::RecorderOnly;
        let registered = RemoteResearchClipboardSessionV1::RegisteredOwnedPasteSession {
            plan_id: "plan".into(),
            phase_id: "phase".into(),
            operation_id: "operation".into(),
        };
        let recorded_only = Ok(ClipboardGuardRpcOutcomeV1::PostPermitRecordedOnly {
            parent_permit_count: 1,
        });
        let target_allowed = Ok(ClipboardGuardRpcOutcomeV1::PostPermitAllowed {
            parent_permit_count: 1,
        });

        assert_eq!(
            remote_post_permit_result(&recorder, recorded_only),
            ClipboardPostPermitResultV1::RecordedOnly
        );
        assert_eq!(
            remote_post_permit_result(&recorder, target_allowed),
            ClipboardPostPermitResultV1::Unknown
        );
        assert_eq!(
            remote_post_permit_result(&registered, target_allowed),
            ClipboardPostPermitResultV1::Allowed
        );
        assert_eq!(
            remote_post_permit_result(&registered, recorded_only),
            ClipboardPostPermitResultV1::Unknown
        );
        assert_eq!(
            remote_post_permit_result(
                &registered,
                Ok(ClipboardGuardRpcOutcomeV1::PostPermitAllowed {
                    parent_permit_count: 2,
                }),
            ),
            ClipboardPostPermitResultV1::Unknown
        );
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
            || Ok(()),
            move |_| {
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
            || Ok(()),
            move |_| {
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
            || Ok(()),
            |mark_may_have_posted| -> Result<(), PasteError> {
                mark_may_have_posted();
                panic!("synthetic input failure")
            },
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
        let result = run_paste_attempt(
            &cancellation,
            || Ok(()),
            || Ok(()),
            |mark_may_have_posted| {
                mark_may_have_posted();
                Ok(())
            },
        );
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

    #[test]
    fn selection_fingerprint_changes_when_selected_text_changes() {
        assert_eq!(selection_fingerprint("same"), selection_fingerprint("same"));
        assert_ne!(
            selection_fingerprint("first"),
            selection_fingerprint("second")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_paste_shortcut_uses_layout_independent_ansi_keycode() {
        assert_eq!(COMMAND_PASTE_KEYCODE, 0x09);
    }

    #[test]
    fn command_shortcuts_target_the_recorded_pid() {
        assert!(should_post_shortcut_to_pid(4242, 99));
        assert!(!should_post_shortcut_to_pid(99, 99));
        assert!(!should_post_shortcut_to_pid(0, 99));
        assert!(!should_post_shortcut_to_pid(-3, 99));
    }

    #[test]
    fn verified_ax_insert_does_not_claim_a_keyboard_paste() {
        let verified_ax = build_verified_ax_outcome("hello".into());
        assert!(!verified_ax.used_keyboard_paste);
        assert!(verified_ax.verified);
        assert_eq!(verified_ax.value_after.as_deref(), Some("hello"));
    }

    #[test]
    fn ax_readback_distinguishes_applied_no_op_and_uncertain_mutation() {
        assert_eq!(
            classify_ax_readback("hello", "hello!", Some("hello!")),
            AxReadbackState::Applied
        );
        assert_eq!(
            classify_ax_readback("hello", "hello!", Some("hello")),
            AxReadbackState::Unchanged
        );
        assert_eq!(
            classify_ax_readback("hello", "hello!", Some("other")),
            AxReadbackState::Uncertain
        );
        assert_eq!(
            classify_ax_readback("hello", "hello!", None),
            AxReadbackState::Uncertain
        );
    }

    #[test]
    fn ax_insert_result_only_retries_when_mutation_is_proven_noop() {
        assert_eq!(
            classify_ax_insert_result(
                "hello",
                "hello!",
                Some("hello!".into()),
                true,
                Some((5, 0)),
                Some((5, 0)),
            ),
            AxInsertAttempt::Verified("hello!".into())
        );
        assert_eq!(
            classify_ax_insert_result(
                "hello",
                "hello!",
                Some("hello".into()),
                false,
                Some((5, 0)),
                Some((5, 0)),
            ),
            AxInsertAttempt::NoOp
        );
        assert_eq!(
            classify_ax_insert_result(
                "hello",
                "hello!",
                Some("hello".into()),
                true,
                Some((5, 0)),
                Some((5, 0)),
            ),
            AxInsertAttempt::Uncertain
        );
        assert_eq!(
            classify_ax_insert_result(
                "hello",
                "hello!",
                Some("hello".into()),
                false,
                Some((5, 0)),
                Some((5, 1)),
            ),
            AxInsertAttempt::Uncertain
        );
        assert_eq!(
            classify_ax_insert_result("hello", "hello!", None, false, Some((5, 0)), None),
            AxInsertAttempt::Uncertain
        );
    }

    #[test]
    fn utf16_splice_replaces_the_exact_selected_range_including_surrogate_pairs() {
        assert_eq!(
            utf16_splice("hello world", 6, 5, "你好").as_deref(),
            Some("hello 你好")
        );
        assert_eq!(utf16_splice("😀a", 0, 2, "x").as_deref(), Some("xa"));
        assert!(utf16_splice("😀a", 1, 1, "x").is_none());
        assert!(utf16_splice("text", -1, 0, "x").is_none());
    }

    #[test]
    fn expected_splice_requires_both_field_value_and_known_selection() {
        assert_eq!(
            expected_splice_after_insert(Some("hello world"), Some((6, 5)), "你好").as_deref(),
            Some("hello 你好")
        );
        assert_eq!(
            expected_splice_after_insert(Some(""), None, "hello").as_deref(),
            Some("hello")
        );
        assert!(expected_splice_after_insert(Some("hello"), None, "!").is_none());
        assert!(expected_splice_after_insert(None, Some((0, 0)), "hello").is_none());
    }

    #[test]
    fn pre_paste_guard_rejects_selection_drift_during_clipboard_staging() {
        assert!(matches!(
            verify_known_focus_snapshot(Some("A🙂中"), Some("A🙂中"), Some((1, 2)), Some((4, 1)),),
            Err(PasteError::SelectionChanged)
        ));
    }

    #[test]
    fn pre_paste_guard_rejects_field_value_drift_with_same_selection() {
        assert!(matches!(
            verify_known_focus_snapshot(
                Some("hello world"),
                Some("hello world!"),
                Some((5, 0)),
                Some((5, 0)),
            ),
            Err(PasteError::InputChanged)
        ));
    }

    #[test]
    fn pre_paste_guard_accepts_unchanged_unicode_value_and_utf16_selection() {
        assert!(matches!(
            verify_known_focus_snapshot(Some("A🙂中"), Some("A🙂中"), Some((1, 2)), Some((1, 2)),),
            Ok(())
        ));
    }

    #[test]
    fn pre_paste_guard_keeps_unknown_ax_facts_unknown() {
        assert!(matches!(
            verify_known_focus_snapshot(None, Some("changed value"), None, Some((9, 0))),
            Ok(())
        ));
        assert!(matches!(
            verify_known_focus_snapshot(Some("known"), None, Some((2, 0)), Some((2, 0))),
            Err(PasteError::InputChanged)
        ));
        assert!(matches!(
            verify_known_focus_snapshot(Some("known"), Some("known"), Some((2, 0)), None),
            Err(PasteError::SelectionChanged)
        ));
    }

    #[test]
    fn paste_verification_records_the_exact_reason_without_claiming_insert_failure() {
        let no_readback_fact = classify_paste_verification(true, None, None, true, true);
        let mismatch_fact =
            classify_paste_verification(true, Some("hello!"), Some("hello!!"), true, true);
        let moved_focus_fact =
            classify_paste_verification(true, Some("hello!"), Some("hello!"), true, false);
        let no_readback = build_insert_outcome(true, None, no_readback_fact);
        let wrong_splice = build_insert_outcome(true, Some("hello!!".into()), mismatch_fact);
        let moved_focus = build_insert_outcome(true, Some("hello!".into()), moved_focus_fact);
        assert_eq!(
            no_readback_fact,
            PasteVerificationFact::ExpectedSpliceUnavailable
        );
        assert_eq!(mismatch_fact, PasteVerificationFact::ReadbackMismatch);
        assert_eq!(
            moved_focus_fact,
            PasteVerificationFact::FocusedFieldUnconfirmed
        );
        assert!(no_readback.used_keyboard_paste);
        assert!(!no_readback.verified);
        assert!(!wrong_splice.verified);
        assert!(!moved_focus.verified);
        assert!(no_readback.post_insert_input_fingerprint.is_none());
    }

    #[test]
    fn paste_verification_keeps_failed_target_and_focus_reads_unknown() {
        assert_eq!(
            classify_paste_verification(true, Some("x"), Some("x"), false, true),
            PasteVerificationFact::TargetUnconfirmed
        );
        assert_eq!(
            classify_paste_verification(true, Some("x"), None, true, true),
            PasteVerificationFact::ReadbackUnavailable
        );
        assert_eq!(
            classify_paste_verification(true, Some("x"), Some("x"), true, false),
            PasteVerificationFact::FocusedFieldUnconfirmed
        );
        assert_eq!(
            classify_paste_verification(true, None, None, true, false),
            PasteVerificationFact::ExpectedSpliceUnavailable
        );
        assert_eq!(
            classify_paste_verification(false, Some("x"), Some("x"), true, true),
            PasteVerificationFact::NotAttempted
        );
        assert_eq!(POST_PASTE_READBACK_DELAY_MS, 250);
    }

    #[test]
    fn exact_cmd_v_splice_readback_and_stable_target_is_verified() {
        let verification =
            classify_paste_verification(true, Some("hello 你好"), Some("hello 你好"), true, true);
        let outcome = build_insert_outcome(true, Some("hello 你好".into()), verification);
        assert_eq!(verification, PasteVerificationFact::Verified);
        assert!(outcome.used_keyboard_paste);
        assert!(outcome.verified);
        assert!(outcome.post_insert_input_fingerprint.is_some());
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
        assert_eq!(
            try_ax_insert_if_safe(&cancellation, &|| Ok(()), &|| Ok(()), 42, None, "hello"),
            AxInsertAttempt::NotAttempted
        );

        let cancellation = CancellationToken::new();
        assert_eq!(
            try_ax_insert_if_safe(
                &cancellation,
                &|| Err(PasteError::TargetChanged),
                &|| Ok(()),
                42,
                None,
                "hello"
            ),
            AxInsertAttempt::NotAttempted
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
            || Ok(()),
            |mark_may_have_posted| {
                saw_suppressed_cmd_v = is_paste_suppressed();
                mark_may_have_posted();
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

#[cfg(all(test, target_os = "macos", feature = "research-harness"))]
mod registered_research_paste_contract_tests {
    use super::*;
    use std::rc::Rc;
    use std::sync::mpsc;
    use std::thread::ThreadId;
    use std::time::Instant;

    struct OwnerLocalAnchor {
        owner_thread: ThreadId,
        _not_send: Rc<()>,
        dropped: mpsc::Sender<()>,
    }

    impl Drop for OwnerLocalAnchor {
        fn drop(&mut self) {
            assert_eq!(self.owner_thread, std::thread::current().id());
            let _ = self.dropped.send(());
        }
    }

    struct BlockingOwnerGuard {
        entered: mpsc::Sender<()>,
        resume: mpsc::Receiver<()>,
    }

    impl Drop for BlockingOwnerGuard {
        fn drop(&mut self) {
            let _ = self.entered.send(());
            let _ = self.resume.recv_timeout(Duration::from_secs(2));
        }
    }

    struct ResumeOwner(mpsc::Sender<()>);

    impl Drop for ResumeOwner {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }

    #[test]
    fn research_ticket_ack_and_owner_completion_are_separate_facts() {
        let id = NEXT_ANCHOR_TICKET_ID.fetch_add(1, Ordering::Relaxed);
        let expires_at = Instant::now() + Duration::from_secs(3);
        let (sender, requests) = mpsc::sync_channel(3);
        let ticket = AnchorTicket {
            inner: std::sync::Arc::new(AnchorTicketInner {
                id,
                sender,
                expires_at,
                explicitly_released: std::sync::atomic::AtomicBool::new(false),
            }),
        };
        let (dropped, drop_receiver) = mpsc::channel();
        let (guard_entered, guard_entered_receiver) = mpsc::channel();
        let (resume, resume_receiver) = mpsc::channel();
        let resume_on_drop = ResumeOwner(resume.clone());
        let (terminal_sender, terminal_receiver) = mpsc::channel();
        let owner_thread = std::thread::spawn(move || {
            let anchor = OwnerLocalAnchor {
                owner_thread: std::thread::current().id(),
                _not_send: Rc::new(()),
                dropped,
            };
            let lease = AnchorLease {
                id,
                anchor,
                requests,
                undo_expires_at: Some(expires_at),
                observation_expires_at: None,
            };
            lease.serve(Some(BlockingOwnerGuard {
                entered: guard_entered,
                resume: resume_receiver,
            }));
            let _ = terminal_sender.send(AnchorOwnerTerminal::Closed);
        });
        let mut completion = AnchorOwnerCompletion::started(terminal_receiver, owner_thread);
        let deadline = Instant::now() + Duration::from_secs(3);
        assert_eq!(ticket.release_and_wait_until(deadline), Ok(()));
        guard_entered_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("release ACK is emitted before the owner guard is dropped");
        assert_eq!(
            completion.wait_closed_until(Instant::now() + Duration::from_millis(10)),
            Err(AnchorOwnerCloseError::UnknownPending)
        );

        let (settlement_started, settlement_started_receiver) = mpsc::channel();
        let (settlement_result, settlement_result_receiver) = mpsc::channel();
        let (host_close_marker, host_close_marker_receiver) = mpsc::channel();
        let settlement_thread = std::thread::spawn(move || {
            let _ = settlement_started.send(());
            let result = completion.settle_after_phase();
            if result.is_ok() {
                let _ = host_close_marker.send(());
            }
            let _ = settlement_result.send(result);
        });
        settlement_started_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("cleanup wait starts while the owner is still held");
        assert!(matches!(
            settlement_result_receiver.recv_timeout(Duration::from_millis(10)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(matches!(
            host_close_marker_receiver.recv_timeout(Duration::from_millis(10)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        drop(resume_on_drop);
        assert_eq!(
            settlement_result_receiver.recv_timeout(Duration::from_secs(1)),
            Ok(Ok(AnchorOwnerTerminal::Closed))
        );
        host_close_marker_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("Host teardown is enabled only after owner join");
        settlement_thread
            .join()
            .expect("cleanup waiter returns after the owner thread joins");
        drop_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("owner drops its native anchor before the close ACK");
    }

    #[test]
    fn research_owner_without_a_started_thread_is_not_reported_closed() {
        let mut completion = AnchorOwnerCompletion::not_started();
        assert_eq!(
            completion.wait_closed_until(Instant::now() + Duration::from_secs(1)),
            Err(AnchorOwnerCloseError::NotStarted)
        );
    }

    #[test]
    fn research_owner_runner_reports_terminal_after_an_empty_owner_loop() {
        let (result, mut completion) = run_delivery_owner_with_completion::<(), _>(|lease| {
            assert!(lease.is_none());
            17
        });
        assert_eq!(result, Ok(17));
        assert_eq!(
            completion.wait_closed_until(Instant::now() + Duration::from_secs(1)),
            Ok(())
        );
        assert_eq!(
            completion.settle_after_phase(),
            Ok(AnchorOwnerTerminal::Closed)
        );
    }

    #[test]
    fn main_queue_dispatch_probe_rejects_non_main_test_threads() {
        if unsafe { libc::pthread_main_np() } != 0 {
            return;
        }
        assert_eq!(
            run_research_main_queue_dispatch_probe(),
            Err("research_main_queue_probe_requires_os_main_thread".into())
        );
    }

    #[test]
    fn registered_field_metadata_requires_exact_window_and_control_without_secure_input() {
        assert!(registered_research_field_metadata_matches(
            Some("registered fixture"),
            Some("fixture-control"),
            false,
            "registered fixture",
            "fixture-control",
        ));
        assert!(!registered_research_field_metadata_matches(
            Some("other window"),
            Some("fixture-control"),
            false,
            "registered fixture",
            "fixture-control",
        ));
        assert!(!registered_research_field_metadata_matches(
            Some("registered fixture"),
            Some("other-control"),
            false,
            "registered fixture",
            "fixture-control",
        ));
        assert!(!registered_research_field_metadata_matches(
            Some("registered fixture"),
            Some("fixture-control"),
            true,
            "registered fixture",
            "fixture-control",
        ));
    }
}

#[cfg(test)]
mod secure_copy_regressions {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    #[test]
    fn secure_input_policy_allows_disabled_probe_and_falls_back_for_unknown_owner() {
        assert!(secure_input_admission(|| SecureInputProbeResult::Disabled, 42).is_ok());
        assert!(matches!(
            secure_input_admission(|| SecureInputProbeResult::Enabled { owner_pid: None }, 42),
            Err(PasteError::SystemSecureInputOtherOwner)
        ));
        for target_pid in [0, -1] {
            assert!(matches!(
                secure_input_admission(
                    || SecureInputProbeResult::Enabled {
                        owner_pid: Some(target_pid)
                    },
                    target_pid
                ),
                Err(PasteError::SystemSecureInputOtherOwner)
            ));
        }
    }

    #[test]
    fn secure_input_queries_are_serialized_across_delivery_threads() {
        let lock = std::sync::Arc::new(Mutex::new(()));
        let active = std::sync::Arc::new(AtomicUsize::new(0));
        let maximum_active = std::sync::Arc::new(AtomicUsize::new(0));
        let (first_entered_tx, first_entered_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let (second_started_tx, second_started_rx) = mpsc::channel();
        let (second_entered_tx, second_entered_rx) = mpsc::channel();
        let first = {
            let lock = lock.clone();
            let active = active.clone();
            let maximum_active = maximum_active.clone();
            std::thread::spawn(move || {
                serialized_secure_input_probe(&lock, &CancellationToken::new(), || {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum_active.fetch_max(now, Ordering::SeqCst);
                    first_entered_tx.send(()).unwrap();
                    release_first_rx.recv().unwrap();
                    active.fetch_sub(1, Ordering::SeqCst);
                    SecureInputProbeResult::Disabled
                })
            })
        };
        first_entered_rx.recv().unwrap();
        let second = {
            let lock = lock.clone();
            let active = active.clone();
            let maximum_active = maximum_active.clone();
            std::thread::spawn(move || {
                second_started_tx.send(()).unwrap();
                serialized_secure_input_probe(&lock, &CancellationToken::new(), || {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum_active.fetch_max(now, Ordering::SeqCst);
                    second_entered_tx.send(()).unwrap();
                    active.fetch_sub(1, Ordering::SeqCst);
                    SecureInputProbeResult::Disabled
                })
            })
        };
        second_started_rx.recv().unwrap();
        assert!(second_entered_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err());
        release_first_tx.send(()).unwrap();
        assert!(first.join().unwrap().is_ok());
        assert!(second.join().unwrap().is_ok());
        second_entered_rx.recv().unwrap();
        assert_eq!(maximum_active.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn secure_input_cancellation_while_waiting_skips_the_probe() {
        let lock = std::sync::Arc::new(Mutex::new(()));
        let guard = lock.lock().unwrap();
        let cancellation = CancellationToken::new();
        let probe_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (started_tx, started_rx) = mpsc::channel();
        let worker = {
            let lock = lock.clone();
            let cancellation = cancellation.clone();
            let probe_called = probe_called.clone();
            std::thread::spawn(move || {
                started_tx.send(()).unwrap();
                secure_input_admission_with_probe(
                    &lock,
                    || {
                        probe_called.store(true, Ordering::SeqCst);
                        SecureInputProbeResult::Disabled
                    },
                    &cancellation,
                    42,
                )
            })
        };
        started_rx.recv().unwrap();
        cancellation.cancel();
        drop(guard);
        assert!(matches!(worker.join().unwrap(), Err(PasteError::Cancelled)));
        assert!(!probe_called.load(Ordering::SeqCst));
    }

    #[test]
    fn secure_input_lock_poison_fails_closed_to_copy_history() {
        let lock = std::sync::Arc::new(Mutex::new(()));
        let poison_lock = lock.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poison_lock.lock().unwrap();
            panic!("poison probe lock for test");
        })
        .join();
        let probe_called = std::cell::Cell::new(false);
        assert!(matches!(
            secure_input_admission_with_probe(
                &lock,
                || {
                    probe_called.set(true);
                    SecureInputProbeResult::Disabled
                },
                &CancellationToken::new(),
                42,
            ),
            Err(PasteError::SystemSecureInputOtherOwner)
        ));
        assert!(!probe_called.get());
    }

    #[test]
    fn secure_input_owned_by_target_refuses_without_shortcut() {
        let cancellation = CancellationToken::new();
        let shortcut_invoked = std::cell::Cell::new(false);
        let attempt = run_paste_attempt(
            &cancellation,
            || {
                secure_input_admission(
                    || SecureInputProbeResult::Enabled {
                        owner_pid: Some(42),
                    },
                    42,
                )
            },
            || Ok(()),
            |mark_may_have_posted| {
                shortcut_invoked.set(true);
                mark_may_have_posted();
                Ok(())
            },
        );

        assert!(matches!(attempt.result, Err(PasteError::SecureInput)));
        assert!(!attempt.shortcut_sent);
        assert!(!shortcut_invoked.get());
    }

    #[test]
    fn secure_input_owned_by_other_app_falls_back_per_policy() {
        let cancellation = CancellationToken::new();
        let shortcut_invoked = std::cell::Cell::new(false);
        let attempt = run_paste_attempt(
            &cancellation,
            || {
                secure_input_admission(
                    || SecureInputProbeResult::Enabled {
                        owner_pid: Some(77),
                    },
                    42,
                )
            },
            || Ok(()),
            |mark_may_have_posted| {
                shortcut_invoked.set(true);
                mark_may_have_posted();
                Ok(())
            },
        );

        assert!(matches!(
            attempt.result,
            Err(PasteError::SystemSecureInputOtherOwner)
        ));
        assert!(!attempt.shortcut_sent);
        assert!(!shortcut_invoked.get());
        let diagnostic = DeliveryTrace {
            clipboard_write_attempted: true,
            clipboard_write_owned: Some(true),
            ..Default::default()
        }
        .for_error(&PasteError::SystemSecureInputOtherOwner);
        assert_eq!(diagnostic.code, "secure_input_owner_unmatched");
        assert_eq!(diagnostic.fallback_delivery_method(), "clipboard");
    }

    #[test]
    fn secure_input_unknown_owner_falls_back_without_shortcut() {
        let cancellation = CancellationToken::new();
        let shortcut_invoked = std::cell::Cell::new(false);
        let attempt = run_paste_attempt(
            &cancellation,
            || {
                // Missing or malformed owner data and a failed owner lookup
                // are represented as an enabled state with no known PID.
                secure_input_admission(|| SecureInputProbeResult::Enabled { owner_pid: None }, 42)
            },
            || Ok(()),
            |mark_may_have_posted| {
                shortcut_invoked.set(true);
                mark_may_have_posted();
                Ok(())
            },
        );

        assert!(matches!(
            attempt.result,
            Err(PasteError::SystemSecureInputOtherOwner)
        ));
        assert!(!attempt.shortcut_sent);
        assert!(!shortcut_invoked.get());
    }

    #[test]
    fn secure_copy_message_requires_actual_clipboard_ownership() {
        let trace = DeliveryTrace {
            clipboard_write_attempted: true,
            clipboard_write_owned: Some(true),
            ..Default::default()
        };
        let diagnostic = trace.for_error(&PasteError::SecureInput);
        assert_eq!(diagnostic.code, "secure_input");
        assert!(diagnostic.user_reason.contains("已复制"));
        let trace = DeliveryTrace {
            clipboard_write_attempted: true,
            clipboard_write_owned: None,
            ..Default::default()
        };
        let diagnostic = trace.for_error(&PasteError::SecureInput);
        assert_eq!(diagnostic.code, "clipboard_ownership_unverified");
        assert!(!diagnostic.user_reason.contains("已复制"));
        let trace = DeliveryTrace {
            stage: "clipboard_write",
            clipboard_write_attempted: true,
            clipboard_write_failed: true,
            clipboard_write_owned: Some(false),
            ..Default::default()
        };
        let diagnostic = trace.for_error(&PasteError::Clipboard("denied".into()));
        assert_ne!(diagnostic.code, "secure_input");
        assert!(!diagnostic.user_reason.contains("已复制"));
    }

    #[test]
    fn target_change_does_not_claim_a_copy_before_clipboard_ownership_is_proven() {
        let no_write = DeliveryTrace {
            stage: "target_guard",
            target_verified: Some(false),
            ..Default::default()
        }
        .for_error(&PasteError::TargetChanged);
        assert_eq!(no_write.code, "target_changed");
        assert_eq!(no_write.stage, "target_guard");
        assert_eq!(no_write.user_reason, "目标已变化");
        assert!(!no_write.clipboard_write_attempted);
        assert!(!no_write.user_reason.contains("剪贴板"));
        assert!(!no_write.user_reason.contains("历史记录"));
        assert!(!no_write.user_reason.contains("已复制"));

        let owned_copy = DeliveryTrace {
            stage: "target_guard",
            target_verified: Some(false),
            clipboard_write_attempted: true,
            clipboard_write_owned: Some(true),
            ..Default::default()
        }
        .for_error(&PasteError::TargetChanged);
        assert_eq!(owned_copy.code, "target_changed");
        assert!(owned_copy.user_reason.contains("剪贴板"));
        assert!(owned_copy.user_reason.contains("⌘V"));
    }

    #[test]
    fn errors_without_proven_copy_do_not_direct_manual_paste() {
        let cases = [
            PasteError::TargetChanged,
            PasteError::TargetUnavailable,
            PasteError::BrowserAccessRequired,
            PasteError::SecureInput,
            PasteError::InputChanged,
            PasteError::SelectionUnavailable,
            PasteError::SelectionChanged,
            PasteError::InputUnavailable,
            PasteError::Input("failed".into()),
            PasteError::Clipboard("unavailable".into()),
        ];

        for error in cases {
            let diagnostic = DeliveryTrace::default().for_error(&error);
            assert!(
                !diagnostic.user_reason.contains("请手动粘贴")
                    && !diagnostic.user_reason.contains("按 ⌘V")
                    && !diagnostic.user_reason.contains("已复制")
                    && !diagnostic.user_reason.contains("已保存"),
                "{} must describe the cause without assuming a clipboard copy",
                diagnostic.code
            );
        }

        let clipboard_write = DeliveryTrace {
            stage: "clipboard_write",
            ..Default::default()
        }
        .for_error(&PasteError::Clipboard("write failed".into()));
        assert_eq!(clipboard_write.code, "clipboard_write_failed");
        assert_eq!(clipboard_write.user_reason, "剪贴板写入失败");

        let keyboard_paste = DeliveryTrace {
            stage: "keyboard_paste",
            ..Default::default()
        }
        .for_error(&PasteError::Input("failed".into()));
        assert_eq!(keyboard_paste.code, "keyboard_paste_failed");
        assert_eq!(keyboard_paste.user_reason, "自动粘贴未完成");
    }
}

#[cfg(test)]
mod final_clipboard_commit_source_fixtures {
    #[test]
    fn only_the_exact_observed_clear_commit_can_supply_an_association() {
        assert_eq!(
            super::restore_commit_generation(Some(10), 11, Some(11)),
            Some(11)
        );
        assert_eq!(
            super::restore_commit_generation(Some(10), 12, Some(12)),
            None
        );
        assert_eq!(
            super::restore_commit_generation(Some(10), 11, Some(12)),
            None
        );
        assert_eq!(super::restore_commit_generation(None, 11, Some(11)), None);
        assert_eq!(
            super::restore_commit_generation(Some(isize::MAX), isize::MIN, Some(isize::MIN)),
            None
        );
    }
}

#[cfg(test)]
mod history_retry_clipboard_tests {
    use super::*;

    #[test]
    fn retry_without_a_captured_snapshot_abstains_without_copying() {
        let mut attempted_copy = false;
        let outcome = copy_retry_with_snapshot(None::<&()>, |_| {
            attempted_copy = true;
            RetryClipboardOutcome::Copied
        });
        assert_eq!(outcome, RetryClipboardOutcome::OwnershipUnverified);
        assert!(!attempted_copy);
    }

    #[test]
    fn readback_polling_uses_lightweight_guard_not_after() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let after_calls = AtomicUsize::new(0);
        let readback_calls = AtomicUsize::new(0);
        let after = || {
            after_calls.fetch_add(1, Ordering::SeqCst);
            Err(PasteError::TargetChanged)
        };
        let readback = || {
            readback_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        assert!(readback_guard_passes(Some(&readback), &after));
        assert_eq!(after_calls.load(Ordering::SeqCst), 0);
        assert_eq!(readback_calls.load(Ordering::SeqCst), 1);
        // Without a readback guard, polling falls back to `after`.
        assert!(!readback_guard_passes(None, &after));
        assert_eq!(after_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn readback_rejects_same_host_different_tab() {
        // A failed final target check can never classify as Verified.
        let verification = classify_confirmed_paste_readback(
            true,
            Some(PasteVerificationFact::Verified),
            Some("hello"),
            Some(""),
            Some("hello"),
            false,
            false,
        );
        assert_ne!(verification, PasteVerificationFact::Verified);
    }
}
