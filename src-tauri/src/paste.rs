use crate::delivery_diagnostics::{self, DeliveryDiagnostic};
#[cfg(target_os = "macos")]
use core_graphics::{
    event::{CGEvent, CGEventFlags, CGEventTapLocation},
    event_source::{CGEventSource, CGEventSourceStateID},
};
#[cfg(not(target_os = "macos"))]
use enigo::{Direction, Enigo, Key, Keyboard, NewConError, Settings};
use std::sync::atomic::{AtomicU64, Ordering};
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
        diagnostic: DeliveryDiagnostic,
    },
}

impl PasteError {
    pub(crate) fn delivery_diagnostic(&self) -> Option<&DeliveryDiagnostic> {
        match self {
            Self::Diagnosed { diagnostic, .. } => Some(diagnostic),
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
fn simulate_paste(expected_pid: i32) -> Result<(), PasteError> {
    let _latin_layout = crate::input_source::AbcLayoutGuard::acquire();
    send_command_shortcut_targeting(COMMAND_PASTE_KEYCODE, Some(expected_pid))?;
    thread::sleep(PASTE_CONSUME_SETTLE);
    Ok(())
}

fn simulate_paste_once(expected_pid: i32) -> Result<(), PasteError> {
    // A posted Cmd+V may have been accepted even when there is no immediate
    // readback. Never send a second shortcut based only on frontmost-app
    // heuristics: that can duplicate text in editors with delayed AX updates.
    simulate_paste(expected_pid)?;
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
/// same-field anchor. It is captured and used inside one blocking worker; the
/// field value is read from this element, never from whichever app happens to
/// be frontmost later.
pub(crate) struct FocusedFieldAnchor {
    #[cfg(target_os = "macos")]
    inner: macos_ax::FocusedFieldAnchor,
}

impl FocusedFieldAnchor {
    pub(crate) fn capture(pid: i32) -> Option<Self> {
        #[cfg(target_os = "macos")]
        {
            macos_ax::FocusedFieldAnchor::capture(pid).map(|inner| Self { inner })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = pid;
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

struct ClipboardSnapshot {
    #[cfg(target_os = "macos")]
    native: macos_pasteboard::Snapshot,
    #[cfg(not(target_os = "macos"))]
    text: Option<String>,
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
    #[cfg(not(target_os = "macos"))]
    Text(String),
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
trait ClipboardRestoreProtocol<Snapshot: ?Sized> {
    type Prepared;

    fn prepare_restore(&mut self, snapshot: &Snapshot) -> Result<Self::Prepared, PasteError>;
    fn observe(&mut self) -> Result<ClipboardWriteObservation, PasteError>;
    fn commit_restore(&mut self, prepared: Self::Prepared) -> Result<(), PasteError>;
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
) -> Result<bool, PasteError>
where
    Backend: ClipboardRestoreProtocol<Snapshot>,
{
    let Some(proof) = proof else {
        return Ok(false);
    };
    let prepared = backend.prepare_restore(snapshot)?;
    let current = backend.observe()?;
    if !clipboard_write_proof_is_current(proof, &current) {
        return Ok(false);
    }
    backend.commit_restore(prepared)?;
    Ok(true)
}

impl ClipboardSnapshot {
    fn capture(app: &AppHandle) -> Result<Self, PasteError> {
        #[cfg(target_os = "macos")]
        {
            let _ = app;
            Ok(Self {
                native: macos_pasteboard::Snapshot::capture()?,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(Self {
                text: app.clipboard().read_text().ok(),
            })
        }
    }

    fn unchanged_since_capture(&self, app: &AppHandle) -> Result<bool, PasteError> {
        #[cfg(target_os = "macos")]
        {
            let _ = app;
            Ok(macos_pasteboard::change_count()? == self.native.change_count)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(app.clipboard().read_text().ok() == self.text)
        }
    }

    fn write_temporary_text(
        &self,
        app: &AppHandle,
        text: &str,
        ownership_token: &str,
    ) -> Result<(), PasteError> {
        #[cfg(target_os = "macos")]
        {
            let _ = app;
            macos_pasteboard::write_temporary_text(
                text,
                ownership_token,
                self.native.change_count,
                &self.native.items,
            )
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (self, ownership_token);
            app.clipboard()
                .write_text(text)
                .map_err(|error| PasteError::Clipboard(error.to_string()))
        }
    }

    fn marker_after_write(
        &self,
        app: &AppHandle,
        text: &str,
        ownership_token: &str,
    ) -> Result<Option<ClipboardCopyMarker>, PasteError> {
        #[cfg(target_os = "macos")]
        {
            let _ = app;
            let observation = macos_pasteboard::observe_write()?;
            Ok(prove_clipboard_write_ownership(
                self.native.change_count,
                text,
                ownership_token,
                &observation,
            )
            .map(ClipboardCopyMarker::Native))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let current = app.clipboard().read_text().ok();
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
        app: &AppHandle,
        marker: &ClipboardCopyMarker,
    ) -> Result<bool, PasteError> {
        #[cfg(target_os = "macos")]
        {
            let _ = (self, app);
            let ClipboardCopyMarker::Native(proof) = marker;
            let observation = macos_pasteboard::observe_write()?;
            Ok(clipboard_write_proof_is_current(proof, &observation))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let current = app.clipboard().read_text().ok();
            let ClipboardCopyMarker::Text(expected) = marker else {
                return Ok(false);
            };
            Ok(current.as_deref() == Some(expected.as_str()))
        }
    }

    fn restore_if_unchanged(
        &self,
        app: &AppHandle,
        marker: Option<&ClipboardCopyMarker>,
    ) -> Result<bool, PasteError> {
        let Some(marker) = marker else {
            return Ok(false);
        };
        #[cfg(target_os = "macos")]
        {
            let ClipboardCopyMarker::Native(proof) = marker;
            let _ = app;
            // Prepare the original items before the last ownership check so
            // the check is immediately adjacent to the atomic writeObjects call.
            self.native.restore_if_owned(proof)
        }
        #[cfg(not(target_os = "macos"))]
        {
            if !self.marker_is_current(app, marker)? {
                return Ok(false);
            }
            let ClipboardCopyMarker::Text(_copied_text) = marker else {
                return Ok(false);
            };
            match &self.text {
                Some(previous) => app
                    .clipboard()
                    .write_text(previous)
                    .map(|()| true)
                    .map_err(|error| PasteError::Clipboard(error.to_string())),
                None => app
                    .clipboard()
                    .clear()
                    .map(|()| true)
                    .map_err(|error| PasteError::Clipboard(error.to_string())),
            }
        }
    }
}

// AX objects remain on the thread that captured them. A ticket carries only
// channels across workers, avoiding an unsafe Send implementation for AX refs.
type AnchorOperation<A> = Box<
    dyn FnOnce(&A, &CancellationToken, std::time::Instant) -> Result<&'static str, String> + Send,
>;
struct AnchorRequest<A> {
    operation: AnchorOperation<A>,
    cancellation: CancellationToken,
    deadline: std::time::Instant,
    reply: std::sync::mpsc::Sender<Result<&'static str, String>>,
}

pub(crate) struct AnchorTicket<A> {
    id: u64,
    sender: std::sync::mpsc::SyncSender<AnchorRequest<A>>,
    expires_at: std::time::Instant,
}

impl<A> Clone for AnchorTicket<A> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            sender: self.sender.clone(),
            expires_at: self.expires_at,
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
        self.id == other.id
    }
}
impl<A> Eq for AnchorTicket<A> {}

impl<A: 'static> AnchorTicket<A> {
    fn retain(anchor: A, lifetime: Duration) -> (Self, AnchorLease<A>) {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let expires_at = std::time::Instant::now() + lifetime;
        let (sender, requests) = std::sync::mpsc::sync_channel::<AnchorRequest<A>>(1);
        let ticket = Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            sender,
            expires_at,
        };
        (
            ticket,
            AnchorLease {
                anchor,
                requests,
                expires_at,
            },
        )
    }

    pub(crate) fn expires_at(&self) -> std::time::Instant {
        self.expires_at
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
        let deadline = self.expires_at.min(std::time::Instant::now() + timeout);
        let cancellation = CancellationToken::new();
        let (reply, receiver) = std::sync::mpsc::channel();
        if self
            .sender
            .try_send(AnchorRequest {
                operation: Box::new(operation),
                cancellation: cancellation.clone(),
                deadline,
                reply,
            })
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

pub(crate) type UndoFieldTicket = AnchorTicket<FocusedFieldAnchor>;

struct AnchorLease<A> {
    anchor: A,
    requests: std::sync::mpsc::Receiver<AnchorRequest<A>>,
    expires_at: std::time::Instant,
}

impl<A> AnchorLease<A> {
    fn serve(self) {
        let remaining = self
            .expires_at
            .saturating_duration_since(std::time::Instant::now());
        // The original paste/readback owner keeps the actual AX reference.
        // No new focus capture, native object transfer, or hash is involved.
        // Last-ticket drop disconnects the channel and releases it promptly.
        if let Ok(request) = self.requests.recv_timeout(remaining) {
            if request.cancellation.is_cancelled() || std::time::Instant::now() >= request.deadline
            {
                return;
            }
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                (request.operation)(&self.anchor, &request.cancellation, request.deadline)
            }))
            .unwrap_or_else(|_| Err("undo anchor worker panicked".into()));
            let _ = request.reply.send(result);
        }
    }
}

/// Construct and retain native anchors entirely on one owner thread. Return
/// only the Send result before serving the optional short-lived Undo lease.
fn run_delivery_owner<A: 'static, T: Send + 'static>(
    operation: impl FnOnce(&mut Option<AnchorLease<A>>) -> T + Send + 'static,
) -> Result<T, String> {
    let (result_sender, result_receiver) = std::sync::mpsc::channel();
    thread::Builder::new()
        .name("voiceflow-paste-owner".into())
        .spawn(move || {
            let mut lease = None;
            let result = operation(&mut lease);
            if result_sender.send(result).is_err() {
                return;
            }
            if let Some(lease) = lease {
                lease.serve();
            }
        })
        .map_err(|error| format!("paste owner failed: {error}"))?;
    // Paste can already have posted a keyboard event; it must finish before
    // a caller chooses a fallback. Do not detach an in-flight paste on timeout.
    result_receiver
        .recv()
        .map_err(|_| "paste owner ended before readback".to_owned())
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
    /// In-memory fingerprint of the complete focused input value immediately
    /// after a verified paste. Undo uses this to avoid undoing later user edits.
    pub post_insert_input_fingerprint: Option<u64>,
    /// Exact delivery-time field identity, kept only in memory for Undo.
    pub(crate) post_insert_target_guard: Option<crate::context::TargetAppGuard>,
    pub(crate) post_insert_field_ticket: Option<UndoFieldTicket>,
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
            .field(
                "post_insert_input_fingerprint",
                &self.post_insert_input_fingerprint,
            )
            .field(
                "undo_field_captured",
                &self.post_insert_field_ticket.is_some(),
            )
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedInsertOutcome {
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
    clipboard_restore_attempted: bool,
    clipboard_restored: Option<bool>,
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
            clipboard_restore_attempted: self.clipboard_restore_attempted,
            clipboard_restored: self.clipboard_restored,
        }
    }

    fn refresh_clipboard_ownership(&mut self, snapshot: &ClipboardSnapshot, app: &AppHandle) {
        if self.clipboard_write_attempted {
            self.clipboard_write_owned = self
                .clipboard_marker
                .as_ref()
                .and_then(|marker| snapshot.marker_is_current(app, marker).ok());
        }
    }

    fn for_unverified_paste(&self) -> DeliveryDiagnostic {
        match self.clipboard_write_owned {
            Some(true) if self.keyboard_paste_may_have_been_posted => self.diagnostic(
                "paste_mutation_uncertain",
                "输入状态无法确认，未重复粘贴；请检查输入框和历史记录",
            ),
            Some(true) => self.diagnostic(
                "paste_unverified",
                "无法确认自动粘贴结果，听写文字仍在剪贴板；请按 ⌘V 粘贴",
            ),
            Some(false) => self.diagnostic(
                "clipboard_changed",
                "无法确认自动粘贴结果，已保留其他应用更新的剪贴板；请先检查输入框，再从历史记录复制听写文字",
            ),
            None => self.diagnostic(
                "clipboard_ownership_unverified",
                "无法确认自动粘贴或剪贴板内容；请从历史记录复制听写文字",
            ),
        }
    }

    fn for_error(&self, error: &PasteError) -> DeliveryDiagnostic {
        if self.clipboard_unchanged_before_write == Some(false) {
            return self.diagnostic(
                "clipboard_changed",
                "剪贴板已被其他应用更新，已保留较新的内容；请从历史记录复制听写文字",
            );
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
        if self.keyboard_paste_may_have_been_posted {
            return self.diagnostic(
                "paste_mutation_uncertain",
                "输入状态无法确认，未重复粘贴；请检查输入框和历史记录",
            );
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
                "自动粘贴需要辅助功能权限，听写文字已保留在剪贴板；请按 ⌘V 粘贴",
            ),
            PasteError::TargetChanged => (
                "target_changed",
                "输入目标已变化，听写文字已保留在剪贴板；请按 ⌘V 粘贴",
            ),
            PasteError::TargetUnavailable => (
                "target_unavailable",
                "无法确认输入目标，听写文字已保留在剪贴板；请按 ⌘V 粘贴",
            ),
            PasteError::BrowserAccessRequired => (
                "browser_permission_required",
                "需要浏览器访问权限确认输入目标，听写文字已保留在剪贴板；请按 ⌘V 粘贴",
            ),
            PasteError::SecureInput => (
                "secure_input",
                if self.clipboard_write_owned == Some(true) {
                    "安全输入阻止自动粘贴，文字已复制；请手动粘贴"
                } else {
                    "安全输入阻止自动粘贴，请从历史记录复制文字"
                },
            ),
            PasteError::InputUnavailable => (
                "input_unavailable",
                "未找到可编辑输入框，听写文字已保留在剪贴板；请按 ⌘V 粘贴",
            ),
            PasteError::InputChanged => (
                "input_changed",
                "输入框已变化，听写文字已保留在剪贴板；请按 ⌘V 粘贴",
            ),
            PasteError::Clipboard(_) if self.stage == "clipboard_snapshot" => (
                "clipboard_unavailable",
                "无法读取剪贴板，听写文字已保存在历史记录中",
            ),
            PasteError::Clipboard(_) if self.stage == "clipboard_write" => (
                "clipboard_write_failed",
                "无法准备剪贴板，听写文字已保存在历史记录中",
            ),
            PasteError::Clipboard(_) => (
                "clipboard_unavailable",
                "无法安全使用剪贴板，听写文字已保存在历史记录中",
            ),
            PasteError::MutationUncertain => (
                "paste_mutation_uncertain",
                "输入状态无法确认，未重复粘贴；请检查输入框和历史记录",
            ),
            PasteError::Cancelled => ("delivery_cancelled", "听写已取消"),
            PasteError::Diagnosed { diagnostic, .. } => {
                return diagnostic.clone();
            }
            PasteError::SelectionUnavailable | PasteError::SelectionChanged => (
                "input_changed",
                "输入框已变化，听写文字已保留在剪贴板；请按 ⌘V 粘贴",
            ),
            PasteError::Input(_) if self.stage == "keyboard_paste" => (
                "keyboard_paste_failed",
                "自动粘贴未完成，听写文字已保留在剪贴板；请按 ⌘V 粘贴",
            ),
            PasteError::Input(_) => (
                "delivery_failed",
                "自动插入未完成，听写文字已保留在剪贴板；请按 ⌘V 粘贴",
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

/// Test-only legacy classifier; production clipboard restoration is limited to
/// verified paste paths that still own the temporary clipboard marker.
#[cfg(test)]
fn should_restore_clipboard(attempt: &PasteAttempt) -> bool {
    !attempt.shortcut_sent
}

fn build_insert_outcome(
    used_keyboard_paste: bool,
    expected_after: Option<&str>,
    value_after: Option<String>,
    target_still_frontmost: bool,
    focused_field_still_same: bool,
) -> InsertOutcome {
    let verified = insert_is_verified(
        used_keyboard_paste,
        expected_after,
        value_after.as_deref(),
        target_still_frontmost,
        focused_field_still_same,
    );
    InsertOutcome {
        shortcut_sent: true,
        used_keyboard_paste,
        verified,
        post_insert_input_fingerprint: verified
            .then(|| value_after.as_deref().map(selection_fingerprint))
            .flatten(),
        post_insert_target_guard: None,
        post_insert_field_ticket: None,
        value_after,
    }
}

fn build_verified_ax_outcome(value_after: String) -> InsertOutcome {
    InsertOutcome {
        shortcut_sent: true,
        used_keyboard_paste: false,
        verified: true,
        post_insert_input_fingerprint: Some(selection_fingerprint(&value_after)),
        post_insert_target_guard: None,
        post_insert_field_ticket: None,
        value_after: Some(value_after),
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
    expected_after: Option<&str>,
    after: Option<&str>,
    target_still_frontmost: bool,
    focused_field_still_same: bool,
) -> bool {
    if !used_keyboard_paste || !target_still_frontmost || !focused_field_still_same {
        return false;
    }
    matches!((expected_after, after), (Some(expected), Some(actual)) if expected == actual)
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

    impl Snapshot {
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
                super::restore_clipboard_if_owned(&mut backend, &self.items, Some(proof))
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

    pub(super) fn observe_write() -> Result<super::ClipboardWriteObservation, PasteError> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(observe_write_inner)).unwrap_or_else(
            |_| {
                Err(PasteError::Clipboard(
                    "pasteboard ownership check panicked".into(),
                ))
            },
        )
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
                    total_bytes = total_bytes.saturating_add(length);
                    if total_bytes > MAX_SNAPSHOT_BYTES {
                        return Err(PasteError::Clipboard(
                            "pasteboard is too large to preserve safely".into(),
                        ));
                    }
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
    ) -> Result<(), PasteError> {
        if prepared.clear {
            let _: isize = msg_send![pasteboard, clearContents];
            return Ok(());
        }
        let _: isize = msg_send![pasteboard, clearContents];
        let restored: bool = msg_send![pasteboard, writeObjects: prepared.items];
        if !restored {
            return Err(PasteError::Clipboard(
                "pasteboard items could not be restored".into(),
            ));
        }
        Ok(())
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

        fn commit_restore(&mut self, prepared: Self::Prepared) -> Result<(), PasteError> {
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
        let before: isize = msg_send![pasteboard, changeCount];
        if before != previous_change_count {
            return Err(PasteError::Clipboard(
                "clipboard changed before temporary paste".into(),
            ));
        }
        // Allocate all data/type objects and the recovery snapshot before
        // taking pasteboard ownership. This keeps failures after declaration
        // limited to the two native setData calls, which can be rolled back
        // only while the returned declaration count and exact staged item
        // still prove that no newer clipboard was supplied.
        let prepared_original = prepare_native_restore(original_items)?;
        let text_type = string_object(TEXT_TYPE)?;
        let marker_type = string_object(OWNER_MARKER_TYPE)?;
        let native_types: *mut Object =
            msg_send![class!(NSMutableArray), arrayWithCapacity: 2usize];
        if native_types.is_null() {
            return Err(PasteError::Clipboard(
                "temporary pasteboard type list allocation failed".into(),
            ));
        }
        let _: () = msg_send![native_types, addObject: text_type];
        let _: () = msg_send![native_types, addObject: marker_type];
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
        if text_data.is_null() || marker_data.is_null() {
            return Err(PasteError::Clipboard(
                "temporary pasteboard data allocation failed".into(),
            ));
        }

        // AppKit `writeObjects:` can append items without changing
        // `changeCount`. Re-read every item/type/data after allocating all
        // fallible native objects, immediately before declaration, so an
        // appended clipboard item cannot be mistaken for the original A.
        // AppKit provides no compare-and-swap; another writer can still race
        // between this final observation and `declareTypes:`.
        let latest = capture_from(pasteboard)?;
        if latest.change_count != previous_change_count || latest.items != original_items {
            return Err(PasteError::Clipboard(
                "clipboard contents changed before temporary write".into(),
            ));
        }

        let declared_count: isize = msg_send![
            pasteboard,
            declareTypes: native_types
            owner: std::ptr::null_mut::<Object>()
        ];
        let expected_count = before.checked_add(1).ok_or_else(|| {
            PasteError::Clipboard("pasteboard change count is out of range".into())
        })?;
        let after_declare: isize = msg_send![pasteboard, changeCount];
        if declared_count != expected_count || after_declare != declared_count {
            return Err(PasteError::Clipboard(
                "clipboard changed while declaring temporary contents".into(),
            ));
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
            return Err(PasteError::Clipboard(
                "temporary pasteboard ownership marker could not be written".into(),
            ));
        }

        let text_written: bool = msg_send![pasteboard, setData: text_data forType: text_type];
        let observation = match observe_write_from(pasteboard) {
            Ok(observation) => observation,
            Err(error) => {
                drop(prepared_original);
                return Err(error);
            }
        };
        if !text_written
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
            return Err(PasteError::Clipboard(
                "temporary pasteboard text could not be verified".into(),
            ));
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
                    if type_count == 2 {
                        let first: *mut Object = msg_send![types, objectAtIndex: 0usize];
                        let second: *mut Object = msg_send![types, objectAtIndex: 1usize];
                        let first = object_string(first)?;
                        let second = object_string(second)?;
                        has_exact_temporary_types = (first == TEXT_TYPE
                            && second == OWNER_MARKER_TYPE)
                            || (first == OWNER_MARKER_TYPE && second == TEXT_TYPE);
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
            std::slice::from_raw_parts(bytes, length).to_vec()
        }

        unsafe fn declare_temporary_types(pasteboard: *mut Object) -> isize {
            let text_type = string_object(TEXT_TYPE).unwrap();
            let marker_type = string_object(OWNER_MARKER_TYPE).unwrap();
            let types: *mut Object = msg_send![class!(NSMutableArray), arrayWithCapacity: 2usize];
            let _: () = msg_send![types, addObject: text_type];
            let _: () = msg_send![types, addObject: marker_type];
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
                    &snapshot.items,
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
                assert!(super::super::restore_clipboard_if_owned(
                    &mut backend,
                    &snapshot.items,
                    Some(&proof)
                )
                .unwrap());
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

                assert!(write_temporary_to(
                    pasteboard,
                    "synthetic temporary B",
                    "synthetic-owner-token",
                    snapshot.change_count,
                    &snapshot.items,
                )
                .is_err());
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

                assert!(write_temporary_to(
                    pasteboard,
                    "synthetic temporary B",
                    "synthetic-owner-token",
                    snapshot.change_count,
                    &snapshot.items,
                )
                .is_err());
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
                write_temporary_to(
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

                write_fixture_items(pasteboard, &[b"external C"]);
                let external = observe_write_from(pasteboard).unwrap();
                assert_eq!(external.change_count, proof.change_count);
                assert_eq!(external.text, None);
                assert!(!external.has_exact_temporary_types);
                assert!(!super::super::clipboard_write_proof_is_current(
                    &proof, &external
                ));
                let mut backend = MacRestoreProtocol { pasteboard };
                assert!(!super::super::restore_clipboard_if_owned(
                    &mut backend,
                    &snapshot.items,
                    Some(&proof)
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
    use core_foundation::base::{CFEqual, CFRange, CFRelease, CFType, CFTypeRef, TCFType};
    use core_foundation::string::{CFString, CFStringRef};

    type AXUIElementRef = *const c_void;
    type AXValueRef = *const c_void;
    const AX_SUCCESS: i32 = 0;
    const AX_VALUE_CF_RANGE: u32 = 4;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
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
        #[cfg(test)]
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

    pub(super) struct FocusedFieldAnchor {
        pid: i32,
        focused: AxElement,
    }

    impl FocusedFieldAnchor {
        pub(super) fn capture(pid: i32) -> Option<Self> {
            if pid <= 0 || !crate::permissions::accessibility_is_trusted() {
                return None;
            }
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
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
                let role = copy_string_attr(&focused, "AXRole").unwrap_or_default();
                let subrole = copy_string_attr(&focused, "AXSubrole").unwrap_or_default();
                let role_blob = format!("{role} {subrole}").to_ascii_lowercase();
                if role_blob.contains("securetextfield") || role_blob.contains("secure text field")
                {
                    return None;
                }
                Some(Self { pid, focused })
            }))
            .ok()
            .flatten()
        }

        pub(super) fn is_current_focus(&self) -> bool {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let current = focused_element_for_pid(self.pid)?;
                Some(unsafe { CFEqual(self.focused.0 as CFTypeRef, current.0 as CFTypeRef) != 0 })
            }))
            .ok()
            .flatten()
            .unwrap_or(false)
        }

        pub(super) fn read_value(&self) -> Option<String> {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                copy_string_attr(&self.focused, "AXValue")
            }))
            .ok()
            .flatten()
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

pub fn insert_timed(
    app: &AppHandle,
    text: &str,
    accessibility: bool,
    cancellation: CancellationToken,
    verify_target: impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    expected_pid: i32,
) -> Result<TimedInsertOutcome, PasteError> {
    let verify_target: std::sync::Arc<dyn Fn() -> Result<(), PasteError> + Send + Sync + 'static> =
        std::sync::Arc::new(verify_target);
    let verify_before = verify_target.clone();
    let verify_after = verify_target.clone();
    insert_with_post_verify_timed(
        app,
        text,
        accessibility,
        cancellation,
        move || verify_before(),
        move || verify_after(),
        expected_pid,
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
        verify_target,
        verify_after,
        expected_pid,
    )?
    .outcome)
}

pub(crate) fn insert_with_post_verify_timed(
    app: &AppHandle,
    text: &str,
    accessibility: bool,
    cancellation: CancellationToken,
    verify_target: impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    verify_after: impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    expected_pid: i32,
) -> Result<TimedInsertOutcome, PasteError> {
    let app = app.clone();
    let text = text.to_owned();
    run_delivery_owner(move |undo_lease| {
        insert_with_post_verify_timed_on_owner(
            &app,
            &text,
            accessibility,
            cancellation,
            verify_target,
            verify_after,
            expected_pid,
            undo_lease,
        )
    })
    .map_err(PasteError::Input)?
}

#[allow(clippy::too_many_arguments)]
fn insert_with_post_verify_timed_on_owner(
    app: &AppHandle,
    text: &str,
    accessibility: bool,
    cancellation: CancellationToken,
    verify_target: impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    verify_after: impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    expected_pid: i32,
    undo_lease: &mut Option<AnchorLease<FocusedFieldAnchor>>,
) -> Result<TimedInsertOutcome, PasteError> {
    let mut trace = DeliveryTrace::new(accessibility, expected_pid);
    let cancellation_for_resolution = cancellation.clone();
    let clipboard_snapshot = match ClipboardSnapshot::capture(app) {
        Ok(snapshot) => {
            trace.clipboard_snapshot_captured = true;
            snapshot
        }
        Err(error) => {
            trace.stage = "clipboard_snapshot";
            let diagnostic = trace.for_error(&error);
            delivery_diagnostics::record(app, &diagnostic);
            return Err(PasteError::Diagnosed {
                source: Box::new(error),
                diagnostic,
            });
        }
    };
    let result = insert_with_post_verify_timed_inner(
        app,
        text,
        accessibility,
        cancellation,
        verify_target,
        verify_after,
        expected_pid,
        &clipboard_snapshot,
        &mut trace,
        undo_lease,
    );
    match result {
        Ok(mut outcome) => {
            trace.paste_verified = Some(outcome.outcome.verified);
            if cancellation_for_resolution.is_cancelled() && trace.clipboard_write_attempted {
                let (error, diagnostic) = resolve_post_write_cancellation(
                    app,
                    &clipboard_snapshot,
                    &mut trace,
                    outcome.outcome.shortcut_sent,
                );
                if !matches!(&error, PasteError::Cancelled) {
                    delivery_diagnostics::record(app, &diagnostic);
                }
                return Err(error);
            }
            if !outcome.outcome.verified {
                trace.stage = "paste_verification";
                trace.refresh_clipboard_ownership(&clipboard_snapshot, app);
                let diagnostic = trace.for_unverified_paste();
                delivery_diagnostics::record(app, &diagnostic);
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
                app,
                &clipboard_snapshot,
                &mut trace,
                shortcut_may_have_been_posted,
            );
            if !matches!(&error, PasteError::Cancelled) {
                delivery_diagnostics::record(app, &diagnostic);
            }
            Err(error)
        }
        Err(error) => {
            if !trace.clipboard_write_attempted {
                let unchanged = clipboard_snapshot
                    .unchanged_since_capture(app)
                    .unwrap_or(false);
                trace.clipboard_unchanged_before_write = Some(unchanged);
                if unchanged {
                    trace.clipboard_write_attempted = true;
                    let ownership_token = clipboard_ownership_token();
                    if clipboard_snapshot
                        .write_temporary_text(app, text, &ownership_token)
                        .is_ok()
                    {
                        trace.clipboard_marker = clipboard_snapshot
                            .marker_after_write(app, text, &ownership_token)
                            .ok()
                            .flatten();
                        trace.refresh_clipboard_ownership(&clipboard_snapshot, app);
                    } else {
                        trace.clipboard_write_failed = true;
                        trace.clipboard_write_owned = Some(false);
                    }
                }
            }
            trace.refresh_clipboard_ownership(&clipboard_snapshot, app);
            let diagnostic = trace.for_error(&error);
            delivery_diagnostics::record(app, &diagnostic);
            Err(PasteError::Diagnosed {
                source: Box::new(error),
                diagnostic,
            })
        }
    }
}

fn resolve_post_write_cancellation(
    app: &AppHandle,
    clipboard_snapshot: &ClipboardSnapshot,
    trace: &mut DeliveryTrace,
    shortcut_may_have_been_posted: bool,
) -> (PasteError, DeliveryDiagnostic) {
    trace.stage = "cancelled_after_clipboard_write";
    trace.keyboard_paste_may_have_been_posted |= shortcut_may_have_been_posted;
    trace.refresh_clipboard_ownership(clipboard_snapshot, app);
    // A cancellation after the temporary write is beyond the delivery side
    // effect boundary. Keep the dictated text when the marker is still ours;
    // if ownership changed, never overwrite the newer clipboard. Only the
    // verified-paste branch in the normal success path may restore the prior
    // clipboard.
    let diagnostic = if trace.keyboard_paste_may_have_been_posted {
        if trace.paste_verified == Some(true) {
            trace.diagnostic(
                "delivery_cancelled",
                "粘贴已确认，听写结果已保存在历史记录中",
            )
        } else {
            trace.diagnostic(
                "paste_mutation_uncertain",
                "取消发生在键盘粘贴尝试后，未重复粘贴；请检查输入框和历史记录",
            )
        }
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
            diagnostic: diagnostic.clone(),
        },
        diagnostic,
    )
}

#[allow(clippy::too_many_arguments)]
fn insert_with_post_verify_timed_inner(
    app: &AppHandle,
    text: &str,
    accessibility: bool,
    cancellation: CancellationToken,
    verify_target: impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    verify_after: impl Fn() -> Result<(), PasteError> + Send + Sync + 'static,
    expected_pid: i32,
    clipboard_snapshot: &ClipboardSnapshot,
    trace: &mut DeliveryTrace,
    undo_lease: &mut Option<AnchorLease<FocusedFieldAnchor>>,
) -> Result<TimedInsertOutcome, PasteError> {
    // A clipboard write is itself a user-visible delivery side effect. Do not
    // let a failed permission or target check overwrite the user's existing
    // clipboard before the keyboard-injection guard runs.
    check_before_clipboard(&cancellation)?;
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
    let mut focus_anchor = FocusedFieldAnchor::capture(expected_pid);
    trace.focused_input_available = Some(
        focus_anchor
            .as_ref()
            .is_some_and(FocusedFieldAnchor::is_current_focus),
    );
    let value_before = focus_anchor
        .as_ref()
        .and_then(FocusedFieldAnchor::read_value);
    let selection_range = focus_anchor
        .as_ref()
        .and_then(FocusedFieldAnchor::read_selected_range);
    let expected_after =
        expected_splice_after_insert(value_before.as_deref(), selection_range, text);
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
    if !clipboard_snapshot.unchanged_since_capture(app)? {
        trace.clipboard_unchanged_before_write = Some(false);
        return Err(PasteError::Clipboard(
            "clipboard changed before paste".into(),
        ));
    }
    trace.clipboard_unchanged_before_write = Some(true);
    check_before_clipboard(&cancellation)?;
    let ownership_token = clipboard_ownership_token();
    trace.stage = "clipboard_write";
    trace.clipboard_write_attempted = true;
    if let Err(error) = clipboard_snapshot.write_temporary_text(app, text, &ownership_token) {
        trace.clipboard_write_failed = true;
        trace.clipboard_marker = clipboard_snapshot
            .marker_after_write(app, text, &ownership_token)
            .ok()
            .flatten();
        trace.refresh_clipboard_ownership(clipboard_snapshot, app);
        return Err(error);
    }
    trace.stage = "clipboard_ownership";
    let clipboard_marker = clipboard_snapshot
        .marker_after_write(app, text, &ownership_token)?
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
    let failed_pre_paste_stage = std::cell::Cell::new("target_recheck");
    let verify_target_before_keyboard = || {
        verify_target()?;
        failed_pre_paste_stage.set("focused_input_recheck");
        if focus_anchor
            .as_ref()
            .is_some_and(|anchor| !anchor.is_current_focus())
        {
            return Err(PasteError::InputChanged);
        }
        failed_pre_paste_stage.set("pre_paste_clipboard_guard");
        if !clipboard_snapshot.marker_is_current(app, &clipboard_marker)? {
            return Err(PasteError::Clipboard(
                "clipboard changed before keyboard paste".into(),
            ));
        }
        Ok(())
    };
    trace.stage = "keyboard_paste";
    let submission_started = std::time::Instant::now();
    let attempt = run_paste_attempt(&cancellation, verify_target_before_keyboard, || {
        simulate_paste_once(expected_pid)
    });
    let paste_submission = submission_started.elapsed();
    trace.stage = if attempt.shortcut_sent {
        "keyboard_paste"
    } else {
        failed_pre_paste_stage.get()
    };
    trace.keyboard_paste_attempted = attempt.shortcut_sent;
    trace.keyboard_paste_may_have_been_posted = attempt.shortcut_sent;
    trace.clipboard_write_owned = clipboard_snapshot
        .marker_is_current(app, &clipboard_marker)
        .ok();
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
    let readback_confirmation_started = std::time::Instant::now();
    if attempt.result.is_ok() {
        trace.stage = "paste_verification";
        thread::sleep(Duration::from_millis(250));
    }
    attempt.result.map(|()| {
        let focused_field_still_same = focus_anchor
            .as_ref()
            .is_some_and(FocusedFieldAnchor::is_current_focus);
        let value_after = focus_anchor
            .as_ref()
            .filter(|_| focused_field_still_same)
            .and_then(FocusedFieldAnchor::read_value);
        let target_still_matches =
            delivery_target_is_frontmost(expected_pid) && verify_after().is_ok();
        trace.target_verified = Some(target_still_matches);
        trace.focused_input_available = Some(focused_field_still_same);
        let mut outcome = build_insert_outcome(
            true,
            expected_after.as_deref(),
            value_after,
            target_still_matches,
            focused_field_still_same,
        );
        // Bind Undo to the field that accepted Cmd+V, not the field focused
        // when recording started. The original AX anchor brackets the probe
        // so moving focus during readback cannot arm a different field.
        if outcome.verified {
            if let Some(anchor) = focus_anchor.as_ref().filter(|a| a.is_current_focus()) {
                let guard = crate::context::probe_focus_guard();
                let fingerprint = anchor.read_value().as_deref().map(selection_fingerprint);
                if anchor.is_current_focus()
                    && fingerprint == outcome.post_insert_input_fingerprint
                    && guard.pid == expected_pid
                    && crate::context::same_field_mismatch_reason(&guard, &guard, false).is_none()
                {
                    // Move the exact original AX reference into a lease on
                    // this same owner thread; never capture focus a second time.
                    // ABA switching cannot turn this retained A anchor into B.
                    if let Some(original_anchor) = focus_anchor.take() {
                        let (ticket, lease) =
                            UndoFieldTicket::retain(original_anchor, Duration::from_secs(3));
                        outcome.post_insert_target_guard = Some(guard);
                        outcome.post_insert_field_ticket = Some(ticket);
                        *undo_lease = Some(lease);
                    }
                }
            }
        }
        trace.clipboard_write_owned = clipboard_snapshot
            .marker_is_current(app, &clipboard_marker)
            .ok();
        if outcome.verified {
            trace.clipboard_restore_attempted = true;
            trace.stage = "clipboard_restore";
            trace.clipboard_restored = clipboard_snapshot
                .restore_if_unchanged(app, Some(&clipboard_marker))
                .ok();
        }
        TimedInsertOutcome {
            outcome,
            paste_submission,
            readback_confirmation: readback_confirmation_started.elapsed(),
            diagnostic: None,
        }
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
    copy(app, text)
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

        fn commit_restore(&mut self, prepared: Self::Prepared) -> Result<(), PasteError> {
            self.items = prepared;
            self.restore_count += 1;
            Ok(())
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
        assert!(!restore_clipboard_if_owned(
            &mut backend,
            &vec!["synthetic A".into()],
            proof.as_ref()
        )
        .unwrap());
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

        assert!(!restore_clipboard_if_owned(
            &mut backend,
            &vec!["synthetic A".into()],
            Some(&proof)
        )
        .unwrap());
        assert_eq!(backend.items, ["synthetic C"]);
        assert_eq!(backend.restore_count, 0);
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
        assert!(!restore_clipboard_if_owned(
            &mut backend,
            &vec!["synthetic A".into()],
            unproven.as_ref()
        )
        .unwrap());
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
    fn posting_cmd_v_without_exact_readback_is_not_verified() {
        let no_readback = build_insert_outcome(true, None, None, true, true);
        let wrong_splice =
            build_insert_outcome(true, Some("hello!"), Some("hello!!".into()), true, true);
        let moved_focus =
            build_insert_outcome(true, Some("hello!"), Some("hello!".into()), true, false);
        assert!(no_readback.used_keyboard_paste);
        assert!(!no_readback.verified);
        assert!(!wrong_splice.verified);
        assert!(!moved_focus.verified);
        assert!(no_readback.post_insert_input_fingerprint.is_none());
    }

    #[test]
    fn exact_cmd_v_splice_readback_and_stable_target_is_verified() {
        let outcome = build_insert_outcome(
            true,
            Some("hello 你好"),
            Some("hello 你好".into()),
            true,
            true,
        );
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

#[cfg(test)]
mod secure_copy_regressions {
    use super::*;
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
}
