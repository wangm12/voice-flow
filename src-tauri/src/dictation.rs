//! Dictation phase transitions and top-level start/stop/cancel orchestration.
//!
//! The application state remains the composition root in `lib.rs`. This module
//! owns only the state machine and the short-lived claims that protect the
//! blocking recorder and provider work performed by the surrounding pipeline.

use crate::delivery_settlement::{
    DeliverySettlementOwner, DeliverySettlementRegistry, DeliveryTerminalOutcome,
    DictationTransactionId,
};
use crate::prefetch_asr;
use crate::screen_action::{clear_screen_action, clear_screen_preview};
use crate::selected_action::{clear_selected_action, clear_selected_preview};
use crate::{
    audio, cancel_audio, cancel_prefetch_asr, cancel_soniox_stream, context, emit_state,
    fail_for_generation, hotkey, island_window, lock_recover, release_operation, start_claimed,
    stop_claimed, sync_modifier_hotkey_phase, AppState,
};
use tauri::State;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Phase {
    Idle,
    Starting,
    Recording,
    Stopping,
    Processing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperationLease {
    Idle,
    LiveDictation,
    HistoryReclean,
    HistoryMutation,
    AudioRecovery,
    DataClear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HotkeySource {
    Dictation,
    Verbatim,
    Translation,
}

#[cfg(any(test, all(feature = "research-harness", target_os = "macos")))]
const OWNED_REPLAY_TTL: std::time::Duration = std::time::Duration::from_secs(180);
const OWNED_REPLAY_CHROME_BUNDLE: &str = "com.google.Chrome";
#[cfg(any(test, all(feature = "research-harness", target_os = "macos")))]
const OWNED_REPLAY_TEXTEDIT_BUNDLE: &str = "com.apple.TextEdit";
const OWNED_REPLAY_SELECTED_TEXT: &str = "REPLACE ONLY THIS";
const OWNED_REPLAY_FIELD_LIMIT_UTF16: usize = 512;

/// A private, short-lived admission for one owned fixture field. It is carried
/// with the recorder claim and is never serialized or logged.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct OwnedReplayTargetBinding {
    pub(crate) target: context::TargetAppGuard,
    expected_full_value: String,
    selection_start_utf16: i64,
    expected_document_path: Option<std::path::PathBuf>,
    expected_window_title: Option<String>,
    expires_at: std::time::Instant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OwnedReplaySourceAdmission {
    receipt: audio::OwnedReplayStartReceipt,
    configuration_generation: u64,
}

#[derive(Clone)]
pub(crate) struct OwnedReplayHistoryAuthority {
    source: OwnedReplaySourceAdmission,
    target: OwnedReplayTargetBinding,
}

impl OwnedReplayHistoryAuthority {
    pub(crate) fn matches(
        &self,
        recording_generation: u64,
        configuration_generation: u64,
        target: &OwnedReplayTargetBinding,
    ) -> bool {
        self.source.receipt.session_generation() == recording_generation
            && self.source.configuration_generation == configuration_generation
            && &self.target == target
    }
}

impl OwnedReplayTargetBinding {
    #[cfg(test)]
    pub(crate) fn from_nonce(
        target: context::TargetAppGuard,
        nonce: &str,
        now: std::time::Instant,
    ) -> Option<Self> {
        Self::from_nonce_with_document(target, nonce, None, now)
    }

    #[cfg(any(test, all(feature = "research-harness", target_os = "macos")))]
    pub(crate) fn from_nonce_with_document(
        target: context::TargetAppGuard,
        nonce: &str,
        expected_document_path: Option<std::path::PathBuf>,
        now: std::time::Instant,
    ) -> Option<Self> {
        let bundle_id = target.bundle_id.as_deref()?;
        let expected_window_title = match (bundle_id, expected_document_path.as_ref()) {
            (OWNED_REPLAY_CHROME_BUNDLE, None) => None,
            (OWNED_REPLAY_TEXTEDIT_BUNDLE, Some(path)) => {
                Some(path.file_name()?.to_str()?.to_owned())
            }
            _ => return None,
        };
        if !canonical_uuid(nonce)
            || target.pid <= 0
            || target.secure_input
            || (bundle_id == OWNED_REPLAY_TEXTEDIT_BUNDLE && target.window_id.is_none())
        {
            return None;
        }
        let prefix = format!("VoiceFlow V7 oracle {nonce}\n前缀🙂[");
        let suffix = format!("]后缀\nEnd {nonce}");
        let expected_full_value = format!("{prefix}{OWNED_REPLAY_SELECTED_TEXT}{suffix}");
        let selection_start_utf16 = prefix.encode_utf16().count() as i64;
        (expected_full_value.encode_utf16().count() <= OWNED_REPLAY_FIELD_LIMIT_UTF16).then_some(
            Self {
                target,
                expected_full_value,
                selection_start_utf16,
                expected_document_path,
                expected_window_title,
                expires_at: now + OWNED_REPLAY_TTL,
            },
        )
    }

    pub(crate) fn expected_full_value(&self) -> &str {
        &self.expected_full_value
    }

    pub(crate) fn field_limit_utf16(&self) -> usize {
        OWNED_REPLAY_FIELD_LIMIT_UTF16
    }

    pub(crate) fn target_bundle_id(&self) -> &str {
        self.target.bundle_id.as_deref().unwrap_or_default()
    }

    #[cfg(any(test, all(feature = "research-harness", target_os = "macos")))]
    pub(crate) fn expected_document_path(&self) -> Option<&std::path::Path> {
        self.expected_document_path.as_deref()
    }

    pub(crate) fn admits_native_window(
        &self,
        window_title: &str,
        document_path: Option<&std::path::Path>,
        require_initial_title: bool,
    ) -> bool {
        match (&self.expected_document_path, &self.expected_window_title) {
            (None, None) => self.target_bundle_id() == OWNED_REPLAY_CHROME_BUNDLE,
            (Some(expected_path), Some(expected_title)) => {
                document_path == Some(expected_path.as_path())
                    && (!require_initial_title || window_title == expected_title)
            }
            _ => false,
        }
    }

    pub(crate) fn is_live(&self, now: std::time::Instant) -> bool {
        now < self.expires_at
    }

    pub(crate) fn admits_capture(
        &self,
        target: &context::TargetAppGuard,
        full_value: &str,
        source: &crate::paste::CapturedTextActionSource,
        now: std::time::Instant,
    ) -> bool {
        self.is_live(now)
            && context::same_field_mismatch_reason(&self.target, target, false).is_none()
            && full_value == self.expected_full_value
            && source.kind == crate::paste::TextActionSourceKind::Selection
            && source.text == OWNED_REPLAY_SELECTED_TEXT
            && source.selection_range
                == Some((
                    self.selection_start_utf16,
                    OWNED_REPLAY_SELECTED_TEXT.encode_utf16().count() as i64,
                ))
            && source.editable
            && !source.copy_only_selection
    }
}

#[cfg(any(test, all(feature = "research-harness", target_os = "macos")))]
fn canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value != "00000000-0000-0000-0000-000000000000"
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
        && matches!(value.as_bytes().get(14), Some(b'1'..=b'8'))
        && matches!(value.as_bytes().get(19), Some(b'8' | b'9' | b'a' | b'b'))
}

#[derive(Clone)]
enum RecordingMode {
    Dictation,
    Verbatim,
    Translation(String),
}

impl RecordingMode {
    fn source(&self) -> HotkeySource {
        match self {
            Self::Dictation => HotkeySource::Dictation,
            Self::Verbatim => HotkeySource::Verbatim,
            Self::Translation(_) => HotkeySource::Translation,
        }
    }
}

/// Apply a captured session choice only to a cloned settings snapshot.
pub(crate) fn apply_session_mode(
    settings: &mut crate::store::Settings,
    skip: bool,
    translation: Option<&str>,
) {
    if skip {
        settings.cleanup_enabled = false;
    }
    if let Some(target) = translation {
        settings.output_mode = "translation".into();
        settings.translation_target_language = target.into();
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SessionLearningSnapshot {
    dictionary: Vec<String>,
    dictionary_sources: crate::store::DictionarySources,
    context_mappings: Vec<context::AppMapping>,
}

impl SessionLearningSnapshot {
    pub(crate) fn from_settings(settings: &crate::store::Settings) -> Self {
        Self {
            dictionary: settings.dictionary.clone(),
            dictionary_sources: settings.dictionary_sources.clone(),
            context_mappings: settings.context_mappings.clone(),
        }
    }

    pub(crate) fn apply_to(&self, settings: &mut crate::store::Settings) {
        settings.dictionary.clone_from(&self.dictionary);
        settings
            .dictionary_sources
            .clone_from(&self.dictionary_sources);
        settings.context_mappings.clone_from(&self.context_mappings);
    }
}

pub(crate) struct DictationManager {
    pub(crate) phase: Phase,
    pub(crate) started: std::time::Instant,
    pub(crate) session_generation: u64,
    /// Clipboard-writer ownership stays bound to the recording identity even
    /// when the manager generation advances through stop and cancellation.
    pub(crate) delivery_settlement: Option<std::sync::Arc<DeliverySettlementOwner>>,
    pub(crate) cancellation: CancellationToken,
    pub(crate) recording_context: Option<context::ContextSnapshot>,
    pub(crate) recording_learning_snapshot: Option<SessionLearningSnapshot>,
    pub(crate) recording_translation_target_language: Option<String>,
    pub(crate) hotkey_session: Option<HotkeySession>,
    pub(crate) recording_source: Option<HotkeySource>,
    pub(crate) onboarding_trial: Option<crate::onboarding::TrialSnapshot>,
    pub(crate) research_trace: Option<crate::research_trace::Trace>,
    pub(crate) stop_when_recording: bool,
    pub(crate) skip_llm_cleanup: bool,
    pub(crate) translation_target_language: Option<String>,
    pub(crate) owned_replay_learning_suppressed: bool,
    pub(crate) owned_replay_target: Option<OwnedReplayTargetBinding>,
    owned_replay_source_admission: Option<OwnedReplaySourceAdmission>,
}

impl DictationManager {
    pub(crate) fn new() -> Self {
        Self {
            phase: Phase::Idle,
            started: std::time::Instant::now(),
            session_generation: 0,
            delivery_settlement: None,
            cancellation: CancellationToken::new(),
            recording_context: None,
            recording_learning_snapshot: None,
            recording_translation_target_language: None,
            hotkey_session: None,
            recording_source: None,
            onboarding_trial: None,
            research_trace: None,
            stop_when_recording: false,
            skip_llm_cleanup: false,
            translation_target_language: None,
            owned_replay_learning_suppressed: false,
            owned_replay_target: None,
            owned_replay_source_admission: None,
        }
    }
}

/// Blocking recorder boundary. Production uses cpal through `audio::Recorder`;
/// tests can inject a deterministic recorder without constructing a Tauri app.
pub(crate) trait RecorderBackend: Send {
    fn arm_readiness(&mut self) -> Option<std::sync::mpsc::Receiver<()>> {
        None
    }
    fn feedback_cue_handle(&self, session_generation: u64) -> Option<audio::CaptureCueHandle>;
    fn start(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session: &str,
        input_device: &str,
        chunk_length_secs: usize,
        input_gain: f32,
        prefetch_tx: prefetch_asr::PrefetchInbox,
    ) -> Result<(), audio::AudioError>;

    #[allow(clippy::too_many_arguments)]
    fn start_streaming(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session: &str,
        session_generation: u64,
        input_device: &str,
        requested_selection: &str,
        clamshell_substituted: bool,
        chunk_length_secs: usize,
        input_gain: f32,
        prefetch_tx: prefetch_asr::PrefetchInbox,
        soniox_audio: Option<crate::soniox::SonioxAudioSender>,
        preserve_full_audio: bool,
        owned_wav: Option<audio::OwnedWavReplay>,
    ) -> Result<(), audio::AudioError> {
        let _ = (
            session_generation,
            requested_selection,
            clamshell_substituted,
            soniox_audio,
            preserve_full_audio,
        );
        if owned_wav.is_some() {
            return Err(audio::AudioError::Device(
                "owned replay requires the production recorder".into(),
            ));
        }
        self.start(
            app,
            session,
            input_device,
            chunk_length_secs,
            input_gain,
            prefetch_tx,
        )
    }

    fn stop_with_chunks(
        &mut self,
        app: Option<&tauri::AppHandle>,
        options: audio::StopOptions,
    ) -> Result<audio::FinishedCapture, audio::AudioError>;

    fn cancel(&mut self, app: Option<&tauri::AppHandle>);
}

impl RecorderBackend for audio::Recorder {
    fn arm_readiness(&mut self) -> Option<std::sync::mpsc::Receiver<()>> {
        Some(audio::Recorder::arm_readiness(self))
    }
    fn feedback_cue_handle(&self, session_generation: u64) -> Option<audio::CaptureCueHandle> {
        audio::Recorder::feedback_cue_handle(self, session_generation)
    }
    fn start(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session: &str,
        input_device: &str,
        chunk_length_secs: usize,
        input_gain: f32,
        prefetch_tx: prefetch_asr::PrefetchInbox,
    ) -> Result<(), audio::AudioError> {
        let app =
            app.ok_or_else(|| audio::AudioError::Device("recorder app handle is required".into()))?;
        audio::Recorder::start(
            self,
            app.clone(),
            session,
            input_device,
            chunk_length_secs,
            input_gain,
            prefetch_tx,
        )
    }

    fn start_streaming(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session: &str,
        session_generation: u64,
        input_device: &str,
        requested_selection: &str,
        clamshell_substituted: bool,
        chunk_length_secs: usize,
        input_gain: f32,
        prefetch_tx: prefetch_asr::PrefetchInbox,
        soniox_audio: Option<crate::soniox::SonioxAudioSender>,
        preserve_full_audio: bool,
        owned_wav: Option<audio::OwnedWavReplay>,
    ) -> Result<(), audio::AudioError> {
        let app =
            app.ok_or_else(|| audio::AudioError::Device("recorder app handle is required".into()))?;
        audio::Recorder::start_with_soniox(
            self,
            app.clone(),
            session,
            session_generation,
            input_device,
            requested_selection,
            clamshell_substituted,
            chunk_length_secs,
            input_gain,
            prefetch_tx,
            soniox_audio,
            preserve_full_audio,
            owned_wav,
        )
    }

    fn stop_with_chunks(
        &mut self,
        app: Option<&tauri::AppHandle>,
        options: audio::StopOptions,
    ) -> Result<audio::FinishedCapture, audio::AudioError> {
        audio::Recorder::stop_with_chunks(self, app, options)
    }

    fn cancel(&mut self, app: Option<&tauri::AppHandle>) {
        audio::Recorder::cancel(self, app);
    }
}

pub(crate) fn claim_operation(lease: &mut OperationLease, requested: OperationLease) -> bool {
    if *lease != OperationLease::Idle {
        return false;
    }
    *lease = requested;
    true
}

pub(crate) fn release_operation_lease(lease: &mut OperationLease, expected: OperationLease) {
    if *lease == expected {
        *lease = OperationLease::Idle;
    }
}

pub(crate) struct StopClaim {
    pub(crate) started: std::time::Instant,
    pub(crate) session_generation: u64,
    /// Generation assigned when this recording started, retained separately
    /// from the stop/processing generation used by HUD completion guards.
    pub(crate) recording_generation: u64,
    pub(crate) delivery_settlement: Option<std::sync::Arc<DeliverySettlementOwner>>,
    pub(crate) ended_during_start: bool,
    pub(crate) skip_llm_cleanup: bool,
    pub(crate) translation_target_language: Option<String>,
    pub(crate) owned_replay_learning_suppressed: bool,
    pub(crate) owned_replay_target: Option<OwnedReplayTargetBinding>,
    pub(crate) owned_replay_history_authority: Option<OwnedReplayHistoryAuthority>,
    pub(crate) cancellation: CancellationToken,
    pub(crate) recording_context: context::ContextSnapshot,
    pub(crate) recording_learning_snapshot: Option<SessionLearningSnapshot>,
    pub(crate) recording_translation_target_language: Option<String>,
    pub(crate) onboarding_trial: Option<crate::onboarding::TrialSnapshot>,
    pub(crate) research_trace: Option<crate::research_trace::Trace>,
}

pub(crate) fn claim_start_manager(
    manager: &mut DictationManager,
    lease: &mut OperationLease,
) -> Option<u64> {
    if manager.phase != Phase::Idle
        || manager
            .delivery_settlement
            .as_ref()
            .is_some_and(|settlement| !settlement.snapshot().writers_closed)
        || !claim_operation(lease, OperationLease::LiveDictation)
    {
        return None;
    }
    manager.skip_llm_cleanup = false;
    manager.translation_target_language = None;
    manager.owned_replay_learning_suppressed = false;
    manager.owned_replay_target = None;
    manager.owned_replay_source_admission = None;
    manager.onboarding_trial = None;
    manager.research_trace = None;
    manager.phase = Phase::Starting;
    manager.session_generation = manager.session_generation.wrapping_add(1);
    manager.delivery_settlement =
        Some(DeliverySettlementOwner::new(DictationTransactionId::fresh()));
    manager.cancellation = CancellationToken::new();
    manager.recording_context = None;
    manager.recording_learning_snapshot = None;
    manager.recording_translation_target_language = None;
    manager.stop_when_recording = false;
    clear_hotkey_session(manager);
    Some(manager.session_generation)
}

fn claim_start_manager_with_mode(
    manager: &mut DictationManager,
    lease: &mut OperationLease,
    mode: RecordingMode,
) -> Option<u64> {
    let generation = claim_start_manager(manager, lease)?;
    manager.recording_source = Some(mode.source());
    manager.skip_llm_cleanup = matches!(mode, RecordingMode::Verbatim);
    manager.translation_target_language = match mode {
        RecordingMode::Translation(language) => Some(language),
        _ => None,
    };
    Some(generation)
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
pub(crate) fn claim_start(state: &AppState) -> Option<u64> {
    claim_start_with_mode(state, RecordingMode::Dictation, "app_command")
}
pub(crate) fn claim_start_native(state: &AppState) -> Option<u64> {
    claim_start_with_mode(state, RecordingMode::Dictation, "native_hotkey")
}
fn claim_start_with_mode(
    state: &AppState,
    mode: RecordingMode,
    trigger_source: &'static str,
) -> Option<u64> {
    let trial = crate::onboarding::snapshot(state, trigger_source);
    claim_start_with_provider_read_cancel(
        &state.pending_provider_credential_read,
        &state.operation_lease,
        &state.manager,
        &state.delivery_settlements,
        mode,
        trial,
    )
}

fn claim_start_with_provider_read_cancel(
    pending: &std::sync::Mutex<Option<crate::PendingProviderCredentialRead>>,
    operation_lease: &std::sync::Mutex<OperationLease>,
    manager: &std::sync::Mutex<DictationManager>,
    delivery_settlements: &DeliverySettlementRegistry,
    mode: RecordingMode,
    trial: Option<crate::onboarding::TrialSnapshot>,
) -> Option<u64> {
    // Keep the pending-read lock through the operation and manager claim. The
    // read admission uses the same pending -> operation ordering, so a request
    // installed after an earlier pre-cancel cannot slip into this start.
    let pending = lock_recover(pending);
    let mut lease = lock_recover(operation_lease);
    let mut manager = lock_recover(manager);
    let generation = claim_start_manager_with_mode(&mut manager, &mut lease, mode)?;
    if let Some(active) = pending.as_ref() {
        // A successful claim permanently invalidates the old request even if
        // recorder startup or settlement admission later rolls the claim back.
        active
            .cancelled
            .store(true, std::sync::atomic::Ordering::Release);
    }
    manager.onboarding_trial = trial.map(|mut trial| {
        trial.recording_generation = generation;
        trial
    });
    if !admit_started_settlement(&mut manager, &mut lease, delivery_settlements) {
        return None;
    }
    Some(generation)
}

/// Every state-machine start path must admit the exact owner it just created.
/// If registry admission is inconsistent, roll the start back instead of
/// allowing an unregistered transaction to publish a later terminal.
fn admit_started_settlement(
    manager: &mut DictationManager,
    lease: &mut OperationLease,
    registry: &DeliverySettlementRegistry,
) -> bool {
    if manager
        .delivery_settlement
        .clone()
        .is_some_and(|settlement| registry.begin(settlement))
    {
        return true;
    }
    reset_starting_manager(manager);
    manager.onboarding_trial = None;
    release_operation_lease(lease, OperationLease::LiveDictation);
    false
}

pub(crate) fn claim_stop_manager(manager: &mut DictationManager) -> Option<StopClaim> {
    if manager.phase != Phase::Recording {
        return None;
    }
    let recording_generation = manager.session_generation;
    manager.session_generation = manager.session_generation.wrapping_add(1);
    let claim = StopClaim {
        started: manager.started,
        session_generation: manager.session_generation,
        recording_generation,
        delivery_settlement: manager.delivery_settlement.clone(),
        ended_during_start: manager.stop_when_recording,
        skip_llm_cleanup: manager.skip_llm_cleanup,
        translation_target_language: manager.translation_target_language.clone(),
        owned_replay_learning_suppressed: manager.owned_replay_learning_suppressed,
        owned_replay_target: manager.owned_replay_target.take(),
        owned_replay_history_authority: None,
        cancellation: manager.cancellation.clone(),
        onboarding_trial: manager.onboarding_trial.clone(),
        research_trace: manager.research_trace.clone(),
        recording_context: manager
            .recording_context
            .take()
            .unwrap_or_else(context::ContextSnapshot::general),
        recording_learning_snapshot: manager.recording_learning_snapshot.take(),
        recording_translation_target_language: manager.recording_translation_target_language.take(),
    };
    if let Some(trace) = &claim.research_trace {
        trace.stop(claim.session_generation);
    }
    manager.phase = Phase::Stopping;
    manager.hotkey_session = None;
    manager.stop_when_recording = false;
    manager.owned_replay_source_admission = None;
    Some(claim)
}

pub(crate) fn claim_stop_manager_for_generation(
    manager: &mut DictationManager,
    expected_generation: u64,
) -> Option<StopClaim> {
    if manager.phase != Phase::Recording || manager.session_generation != expected_generation {
        return None;
    }
    claim_stop_manager(manager)
}

fn hud_cancel_matches_session(phase: Phase, generation: u64, expected_generation: u64) -> bool {
    matches!(phase, Phase::Starting | Phase::Stopping | Phase::Processing)
        && generation == expected_generation
}

#[cfg(any(test, all(feature = "research-harness", target_os = "macos")))]
fn claim_owned_replay_stop_manager_for_generation(
    manager: &mut DictationManager,
    expected_generation: u64,
    event: &audio::OwnedReplayLifecycleEvent,
    current_configuration_generation: u64,
    active_configuration_generation: u64,
    now: std::time::Instant,
) -> Option<StopClaim> {
    if manager.phase != Phase::Recording || manager.session_generation != expected_generation {
        return None;
    }
    let authority = owned_replay_history_authority_for_stop(
        manager,
        expected_generation,
        event,
        current_configuration_generation,
        active_configuration_generation,
        now,
    );
    let mut claim = claim_stop_manager_for_generation(manager, expected_generation)?;
    claim.owned_replay_history_authority = authority;
    Some(claim)
}

#[cfg(any(test, all(feature = "research-harness", target_os = "macos")))]
fn claim_owned_replay_start_manager(
    manager: &mut DictationManager,
    lease: &mut OperationLease,
) -> Option<u64> {
    let generation = claim_start_manager_with_mode(manager, lease, RecordingMode::Dictation)?;
    manager.owned_replay_learning_suppressed = true;
    Some(generation)
}

#[cfg(any(test, all(feature = "research-harness", target_os = "macos")))]
fn bind_owned_replay_target_manager(
    manager: &mut DictationManager,
    expected_generation: u64,
    target: OwnedReplayTargetBinding,
) -> bool {
    if manager.phase != Phase::Starting
        || manager.session_generation != expected_generation
        || manager.cancellation.is_cancelled()
        || !manager.owned_replay_learning_suppressed
        || manager.owned_replay_target.is_some()
    {
        return false;
    }
    manager.owned_replay_target = Some(target);
    true
}

#[cfg(any(test, all(feature = "research-harness", target_os = "macos")))]
fn record_owned_replay_source_start_manager(
    manager: &mut DictationManager,
    expected_generation: u64,
    configuration_generation: u64,
    current_configuration_generation: u64,
    active_configuration_generation: u64,
    receipt: audio::OwnedReplayStartReceipt,
    now: std::time::Instant,
) -> bool {
    let Some(target) = manager.owned_replay_target.as_ref() else {
        return false;
    };
    let current_context_matches = manager
        .recording_context
        .as_ref()
        .is_some_and(|context| context.target_guard == target.target);
    if manager.phase != Phase::Recording
        || manager.session_generation != expected_generation
        || manager.cancellation.is_cancelled()
        || !manager.owned_replay_learning_suppressed
        || manager.onboarding_trial.is_some()
        || manager.research_trace.is_some()
        || manager.owned_replay_source_admission.is_some()
        || receipt.session_generation() != expected_generation
        || !target.is_live(now)
        || !current_context_matches
        || configuration_generation != current_configuration_generation
        || configuration_generation != active_configuration_generation
    {
        return false;
    }
    manager.owned_replay_source_admission = Some(OwnedReplaySourceAdmission {
        receipt,
        configuration_generation,
    });
    true
}

#[cfg(any(test, all(feature = "research-harness", target_os = "macos")))]
fn owned_replay_history_authority_for_stop(
    manager: &DictationManager,
    expected_generation: u64,
    event: &audio::OwnedReplayLifecycleEvent,
    current_configuration_generation: u64,
    active_configuration_generation: u64,
    now: std::time::Instant,
) -> Option<OwnedReplayHistoryAuthority> {
    let source = manager.owned_replay_source_admission.as_ref()?;
    let target = manager.owned_replay_target.as_ref()?;
    let current_context_matches = manager
        .recording_context
        .as_ref()
        .is_some_and(|context| context.target_guard == target.target);
    if manager.phase != Phase::Recording
        || manager.session_generation != expected_generation
        || manager.cancellation.is_cancelled()
        || !manager.owned_replay_learning_suppressed
        || manager.onboarding_trial.is_some()
        || manager.research_trace.is_some()
        || source.receipt.session_generation() != expected_generation
        || !source.receipt.confirms_eof(event)
        || !target.is_live(now)
        || !current_context_matches
        || source.configuration_generation != current_configuration_generation
        || source.configuration_generation != active_configuration_generation
    {
        return None;
    }
    Some(OwnedReplayHistoryAuthority {
        source: source.clone(),
        target: target.clone(),
    })
}

/// Complete a controlled, non-recording onboarding action only after its
/// blocking delivery owner has joined. Never finish a recorder or newer claim.
pub(crate) fn finish_onboarding_fixture_operation(
    manager: &mut DictationManager,
    expected: &crate::onboarding::TrialSnapshot,
) -> Option<u64> {
    if manager.onboarding_trial.as_ref() != Some(expected)
        || !matches!(
            manager.phase,
            Phase::Starting | Phase::Processing | Phase::Idle
        )
    {
        return None;
    }
    manager.cancellation.cancel();
    manager.phase = Phase::Idle;
    manager.skip_llm_cleanup = false;
    manager.translation_target_language = None;
    manager.owned_replay_learning_suppressed = false;
    manager.owned_replay_target = None;
    manager.owned_replay_source_admission = None;
    manager.recording_context = None;
    manager.recording_learning_snapshot = None;
    manager.recording_translation_target_language = None;
    manager.stop_when_recording = false;
    clear_hotkey_session(manager);
    Some(manager.session_generation)
}

pub(crate) fn claim_stop(state: &AppState) -> Option<StopClaim> {
    let mut manager = lock_recover(&state.manager);
    claim_stop_manager(&mut manager)
}

/// A release or second tap: stop now if recording, otherwise remember to stop once
/// `Starting` becomes `Recording`.
pub(crate) fn request_stop(manager: &mut DictationManager) -> Option<StopClaim> {
    if let Some(claim) = claim_stop_manager(manager) {
        return Some(claim);
    }
    if manager.phase == Phase::Starting {
        manager.stop_when_recording = true;
    }
    None
}

/// Commit Starting → Recording. If an end request arrived during
/// Starting, take the stop claim immediately.
pub(crate) fn enter_recording(
    manager: &mut DictationManager,
    recording_context: context::ContextSnapshot,
) -> Option<StopClaim> {
    manager.started = std::time::Instant::now();
    manager.phase = Phase::Recording;
    manager.cancellation = CancellationToken::new();
    manager.recording_context = Some(recording_context);
    if !manager.stop_when_recording {
        return None;
    }
    claim_stop_manager(manager)
}

/// Claim the onboarding external-insertion fixture without joining the
/// ordinary dictation settlement registry. That fixture has its own blocking
/// delivery owner and lifecycle; its paste result must not replace the last
/// ordinary dictation terminal.
pub(crate) async fn claim_onboarding_external_start_entry(state: &AppState) -> Option<u64> {
    let _gate = state.hotkey_gate.lock().await;
    let trial = crate::onboarding::snapshot(state, "app_command");
    let mut lease = lock_recover(&state.operation_lease);
    let mut manager = lock_recover(&state.manager);
    let generation =
        claim_start_manager_with_mode(&mut manager, &mut lease, RecordingMode::Dictation)?;
    manager.onboarding_trial = trial.map(|mut trial| {
        trial.recording_generation = generation;
        trial
    });
    manager.delivery_settlement = None;
    Some(generation)
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
pub(crate) async fn claim_owned_replay_start_entry(state: &AppState) -> Option<u64> {
    let _gate = state.hotkey_gate.lock().await;
    if lock_recover(&state.selected_action).is_some()
        || crate::screen_action::screen_action_is_active(state)
        || crate::onboarding::snapshot(state, "owned_wav_replay").is_some()
        || crate::onboarding::snapshot(state, "app_command").is_some()
    {
        return None;
    }
    let mut lease = lock_recover(&state.operation_lease);
    let mut manager = lock_recover(&state.manager);
    let generation = claim_owned_replay_start_manager(&mut manager, &mut lease)?;
    manager.onboarding_trial = None;
    if !admit_started_settlement(&mut manager, &mut lease, &state.delivery_settlements) {
        return None;
    }
    Some(generation)
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
pub(crate) fn bind_owned_replay_target(
    state: &AppState,
    expected_generation: u64,
    target: OwnedReplayTargetBinding,
) -> bool {
    let mut manager = lock_recover(&state.manager);
    bind_owned_replay_target_manager(&mut manager, expected_generation, target)
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
pub(crate) fn record_owned_replay_source_started(
    state: &AppState,
    expected_generation: u64,
    configuration_generation: u64,
    receipt: audio::OwnedReplayStartReceipt,
) -> bool {
    let current_configuration_generation = state
        .processing_configuration_generation
        .load(std::sync::atomic::Ordering::Acquire);
    let active_configuration_generation = state
        .active_processing_configuration_generation
        .load(std::sync::atomic::Ordering::Acquire);
    let mut manager = lock_recover(&state.manager);
    record_owned_replay_source_start_manager(
        &mut manager,
        expected_generation,
        configuration_generation,
        current_configuration_generation,
        active_configuration_generation,
        receipt,
        std::time::Instant::now(),
    )
}

pub(crate) async fn claim_stop_entry(state: &AppState) -> Option<StopClaim> {
    let _gate = state.hotkey_gate.lock().await;
    claim_stop(state)
}

/// Stop claim bound to the Recording generation named by an engine event, so a
/// late `audio://limit` for an older session cannot stop a newer one.
pub(crate) async fn claim_stop_entry_for_generation(
    state: &AppState,
    expected_generation: u64,
) -> Option<StopClaim> {
    let _gate = state.hotkey_gate.lock().await;
    let mut manager = lock_recover(&state.manager);
    claim_stop_manager_for_generation(&mut manager, expected_generation)
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
pub(crate) async fn claim_owned_replay_stop_entry_for_generation(
    state: &AppState,
    expected_generation: u64,
    event: &audio::OwnedReplayLifecycleEvent,
) -> Option<StopClaim> {
    let _gate = state.hotkey_gate.lock().await;
    let current_configuration_generation = state
        .processing_configuration_generation
        .load(std::sync::atomic::Ordering::Acquire);
    let active_configuration_generation = state
        .active_processing_configuration_generation
        .load(std::sync::atomic::Ordering::Acquire);
    let mut manager = lock_recover(&state.manager);
    claim_owned_replay_stop_manager_for_generation(
        &mut manager,
        expected_generation,
        event,
        current_configuration_generation,
        active_configuration_generation,
        std::time::Instant::now(),
    )
}

pub(crate) fn reset_starting_manager(manager: &mut DictationManager) -> u64 {
    if manager.phase == Phase::Starting {
        if let Some(settlement) = &manager.delivery_settlement {
            settlement.seal(DeliveryTerminalOutcome::completed(
                "error",
                "none",
                Some("start_failed"),
            ));
        }
        manager.cancellation.cancel();
        manager.phase = Phase::Idle;
        manager.skip_llm_cleanup = false;
        manager.translation_target_language = None;
        manager.owned_replay_target = None;
        manager.owned_replay_learning_suppressed = false;
        manager.owned_replay_source_admission = None;
        manager.session_generation = manager.session_generation.wrapping_add(1);
        manager.recording_context = None;
        manager.recording_learning_snapshot = None;
        manager.recording_translation_target_language = None;
        manager.stop_when_recording = false;
        clear_hotkey_session(manager);
    }
    manager.session_generation
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HotkeySession {
    source: HotkeySource,
    gesture_id: u64,
    generation: u64,
}

fn clear_hotkey_session(manager: &mut DictationManager) {
    manager.hotkey_session = None;
    manager.recording_source = None;
}

pub(crate) struct CancelClaim {
    pub(crate) phase: Phase,
    pub(crate) generation: u64,
    pub(crate) delivery_settlement: Option<std::sync::Arc<DeliverySettlementOwner>>,
    pub(crate) had_action: bool,
    pub(crate) text_action: Option<crate::TextActionIdentity>,
    pub(crate) onboarding_trial: Option<crate::onboarding::TrialSnapshot>,
    pub(crate) research_trace: Option<crate::research_trace::Trace>,
}

fn claim_cancel_for_interrupt_manager(manager: &mut DictationManager) -> Option<CancelClaim> {
    let phase = manager.phase;
    if !matches!(phase, Phase::Starting | Phase::Recording) {
        return None;
    }
    let delivery_settlement = manager.delivery_settlement.clone();
    let onboarding_trial = manager.onboarding_trial.clone();
    let research_trace = manager.research_trace.clone();
    claim_cancel_manager(manager, false);
    Some(CancelClaim {
        phase,
        generation: manager.session_generation,
        delivery_settlement,
        had_action: false,
        text_action: None,
        onboarding_trial,
        research_trace,
    })
}

pub(crate) fn claim_cancel_manager(manager: &mut DictationManager, had_preview: bool) -> Phase {
    let phase = manager.phase;
    if phase != Phase::Idle || had_preview {
        if phase != Phase::Idle {
            if let Some(settlement) = &manager.delivery_settlement {
                settlement.seal(DeliveryTerminalOutcome::cancelled());
            }
        }
        manager.cancellation.cancel();
        manager.phase = Phase::Idle;
        manager.skip_llm_cleanup = false;
        manager.translation_target_language = None;
        manager.owned_replay_learning_suppressed = false;
        manager.owned_replay_target = None;
        manager.owned_replay_source_admission = None;
        manager.session_generation = manager.session_generation.wrapping_add(1);
        manager.recording_context = None;
        manager.recording_learning_snapshot = None;
        manager.recording_translation_target_language = None;
        manager.stop_when_recording = false;
        clear_hotkey_session(manager);
    }
    phase
}

pub(crate) fn claim_cancel(state: &AppState) -> CancelClaim {
    let text_action = crate::cancel_active_text_action(state);
    claim_cancel_for_text_action(state, text_action)
}

fn claim_cancel_for_text_action(
    state: &AppState,
    text_action: Option<crate::TextActionIdentity>,
) -> CancelClaim {
    clear_selected_action(state);
    clear_screen_action(state);
    let had_selected_preview = state
        .selected_preview
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_some();
    let had_screen_preview = state
        .screen_preview
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_some();
    let had_preview = had_selected_preview || had_screen_preview;
    clear_selected_preview(state);
    clear_screen_preview(state);
    let had_action = had_preview || text_action.is_some();
    let mut manager = lock_recover(&state.manager);
    if let Some(trace) = &manager.research_trace {
        trace.revoke("production_cancel");
    }
    let delivery_settlement = manager.delivery_settlement.clone();
    let phase = claim_cancel_manager(&mut manager, had_action);
    // Explicit cancel also stops any post-paste observer that outlived a
    // successful delivery into Idle.
    crate::dictionary_learn::invalidate_learning_observers(&state.learning_observer_epoch);
    CancelClaim {
        phase,
        generation: manager.session_generation,
        delivery_settlement,
        had_action,
        text_action,
        onboarding_trial: manager.onboarding_trial.clone(),
        research_trace: manager.research_trace.clone(),
    }
}

fn claim_cancel_for_interrupt(state: &AppState) -> Option<CancelClaim> {
    let mut claim = {
        let mut manager = lock_recover(&state.manager);
        claim_cancel_for_interrupt_manager(&mut manager)?
    };

    let text_action = crate::cancel_active_text_action(state);
    clear_selected_action(state);
    clear_screen_action(state);
    let had_selected_preview = state
        .selected_preview
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_some();
    let had_screen_preview = state
        .screen_preview
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_some();
    let had_preview = had_selected_preview || had_screen_preview;
    clear_selected_preview(state);
    clear_screen_preview(state);
    if let Some(trace) = &claim.research_trace {
        trace.revoke("production_cancel");
    }
    crate::dictionary_learn::invalidate_learning_observers(&state.learning_observer_epoch);
    claim.had_action = had_preview || text_action.is_some();
    claim.text_action = text_action;
    Some(claim)
}

async fn claim_cancel_entry(state: &AppState) -> CancelClaim {
    let _gate = state.hotkey_gate.lock().await;
    claim_cancel(state)
}

async fn claim_cancel_entry_for_hud(
    state: &AppState,
    expected_generation: u64,
) -> Option<CancelClaim> {
    let _gate = state.hotkey_gate.lock().await;
    let matches = {
        let manager = lock_recover(&state.manager);
        hud_cancel_matches_session(
            manager.phase,
            manager.session_generation,
            expected_generation,
        )
    };
    matches.then(|| claim_cancel(state))
}

pub(crate) async fn cancel_text_action_by_id(
    app: &tauri::AppHandle,
    state: &AppState,
    transaction_id: &str,
    expected_kind: &str,
) -> Option<crate::PreviewCancel> {
    let (cancelled, claim) = {
        let _gate = state.hotkey_gate.lock().await;
        let outcome = crate::cancel_preview_text_action_control_for_session(
            state,
            transaction_id,
            expected_kind,
        )?;
        let crate::PreviewCancel::Cancelled(owner, _) = &outcome else {
            // The commit owner settles the preview and reports its own outcome.
            return Some(outcome);
        };
        let claim = claim_cancel_for_text_action(state, Some(owner.clone()));
        (outcome, claim)
    };
    let crate::PreviewCancel::Cancelled(identity, _) = &cancelled else {
        return None;
    };
    if claim.text_action.as_ref() != Some(identity) || identity.transaction_id != transaction_id {
        return None;
    }
    execute_cancel_claim(app, state, claim).await;
    Some(cancelled)
}

/// Finishes the cleanup a mid-commit cancel deferred to the commit owner.
/// Returns whether a deferred cancel existed.
pub(crate) async fn settle_cancel_during_commit(
    app: &tauri::AppHandle,
    state: &AppState,
    identity: &crate::TextActionIdentity,
    owner_generation: u64,
) -> bool {
    let claim = {
        let _gate = state.hotkey_gate.lock().await;
        let current_generation = {
            let manager = lock_recover(&state.manager);
            manager.session_generation
        };
        if current_generation != owner_generation
            || !crate::take_commit_cancel_pending(state, identity)
        {
            return false;
        }
        claim_cancel_for_text_action(state, Some(identity.clone()))
    };
    execute_cancel_claim(app, state, claim).await;
    true
}

/// A clipboard error is the commit's terminal result even when cancellation
/// raced it. Settle only the captured owner while holding the session gate;
/// never route this through the global cancel path, which could cancel a newer
/// recording and would emit a misleading `cancelled` lifecycle event.
pub(crate) async fn settle_failed_copy_during_commit(
    app: &tauri::AppHandle,
    state: &AppState,
    identity: &crate::TextActionIdentity,
    owner_generation: u64,
) -> bool {
    let _gate = state.hotkey_gate.lock().await;
    let current_generation = {
        let manager = lock_recover(&state.manager);
        manager.session_generation
    };
    if current_generation != owner_generation
        || !crate::settle_failed_copy_control(state, identity, owner_generation)
    {
        return false;
    }
    release_operation(state, OperationLease::LiveDictation);
    crate::emit_text_action_lifecycle(app, identity, "failed");
    true
}

async fn finish_cancel_claim(
    app: &tauri::AppHandle,
    state: &AppState,
    release_live_lease: bool,
    generation: u64,
    trial: Option<&crate::onboarding::TrialSnapshot>,
) {
    let _gate = state.hotkey_gate.lock().await;
    if lock_recover(&state.manager).session_generation != generation {
        return;
    }
    if release_live_lease {
        release_operation(state, OperationLease::LiveDictation);
    }
    sync_modifier_hotkey_phase(Phase::Idle);
    hotkey::unregister_cancel(app);
    emit_state(app, "idle");
    crate::onboarding::emit_state(app, state, "dictation://state", "idle", generation, trial);
    island_window::hide_overlay(app);
}

pub(crate) async fn execute_cancel_claim(
    app: &tauri::AppHandle,
    state: &AppState,
    claim: CancelClaim,
) {
    state.gate.set_session_generation(claim.generation);
    // The cancellation claim seals synchronously while generation changes are
    // serialized. Join outside `hotkey_gate` so a running native writer can
    // unwind without blocking unrelated state publication.
    if let Some(settlement) = &claim.delivery_settlement {
        settlement.wait_closed().await;
        if claim.phase != Phase::Idle {
            state
                .delivery_settlements
                .record_terminal(settlement.clone());
        }
    }
    match claim.phase {
        Phase::Starting => {
            finish_cancel_claim(
                app,
                state,
                false,
                claim.generation,
                claim.onboarding_trial.as_ref(),
            )
            .await
        }
        Phase::Recording => {
            cancel_prefetch_asr(state);
            cancel_soniox_stream(state, claim.generation);
            cancel_audio(state, app.clone()).await;
            finish_cancel_claim(
                app,
                state,
                true,
                claim.generation,
                claim.onboarding_trial.as_ref(),
            )
            .await;
        }
        Phase::Stopping => {
            cancel_prefetch_asr(state);
            cancel_soniox_stream(state, claim.generation);
            finish_cancel_claim(
                app,
                state,
                false,
                claim.generation,
                claim.onboarding_trial.as_ref(),
            )
            .await;
        }
        Phase::Processing => {
            finish_cancel_claim(
                app,
                state,
                true,
                claim.generation,
                claim.onboarding_trial.as_ref(),
            )
            .await
        }
        Phase::Idle if claim.had_action => {
            finish_cancel_claim(
                app,
                state,
                true,
                claim.generation,
                claim.onboarding_trial.as_ref(),
            )
            .await
        }
        Phase::Idle => {}
    }
    if let Some(trace) = &claim.research_trace {
        if claim.phase == Phase::Recording && trace.capture_is_closed() {
            trace.terminal();
        }
    }
    if let Some(identity) = claim.text_action {
        crate::clear_text_action(state, &identity);
        crate::emit_text_action_lifecycle(app, &identity, "cancelled");
    }
}

pub(crate) async fn cancel_internal(app: &tauri::AppHandle, state: &AppState) {
    let claim = claim_cancel_entry(state).await;
    execute_cancel_claim(app, state, claim).await;
}

/// Trial cleanup may only cancel the captured recording, never a newer
/// normal session or trial that started before the queued cleanup ran.
pub(crate) async fn cancel_onboarding_trial(
    app: &tauri::AppHandle,
    state: &AppState,
    expected: &crate::onboarding::TrialSnapshot,
) {
    let claim = {
        let _gate = state.hotkey_gate.lock().await;
        let current = { lock_recover(&state.manager).onboarding_trial.clone() };
        if !current.as_ref().is_some_and(|current| {
            current.lease == expected.lease
                && current.recording_generation == expected.recording_generation
        }) {
            return;
        }
        claim_cancel(state)
    };
    execute_cancel_claim(app, state, claim).await;
}

pub(crate) async fn stop_internal(app: &tauri::AppHandle, state: &AppState) -> Result<(), String> {
    if state.exit_state.load(std::sync::atomic::Ordering::Acquire) != 0 {
        return Ok(());
    }
    let Some(claim) = claim_stop_entry(state).await else {
        return Ok(());
    };
    stop_claimed(app, state, claim).await
}

async fn stop_from_hud_internal(
    app: &tauri::AppHandle,
    state: &AppState,
    expected_generation: u64,
) -> Result<(), String> {
    if state.exit_state.load(std::sync::atomic::Ordering::Acquire) != 0 {
        return Ok(());
    }
    let claim = {
        let _gate = state.hotkey_gate.lock().await;
        let mut manager = lock_recover(&state.manager);
        claim_stop_manager_for_generation(&mut manager, expected_generation)
    };
    if let Some(claim) = claim {
        stop_claimed(app, state, claim).await?;
    }
    Ok(())
}

async fn cancel_from_hud_internal(
    app: &tauri::AppHandle,
    state: &AppState,
    expected_generation: u64,
) {
    let Some(claim) = claim_cancel_entry_for_hud(state, expected_generation).await else {
        return;
    };
    execute_cancel_claim(app, state, claim).await;
}

// Stop claims intentionally carry the captured recording and its ownership
// state together. Boxing would allocate on every stop with no lifetime benefit.
#[allow(clippy::large_enum_variant)]
enum EntryClaim {
    Start(u64),
    Stop(StopClaim),
    Cancel(CancelClaim),
    Ignore,
}

async fn execute_entry_claim(app: &tauri::AppHandle, state: &AppState, claim: EntryClaim) {
    match claim {
        EntryClaim::Start(generation) => {
            if let Err(error) = start_claimed(app, state, generation).await {
                fail_for_generation(app, state, error.message, error.generation).await;
            }
        }
        EntryClaim::Stop(claim) => {
            let _ = stop_claimed(app, state, claim).await;
        }
        EntryClaim::Cancel(claim) => execute_cancel_claim(app, state, claim).await,
        EntryClaim::Ignore => {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModeTrigger {
    Press,
    Release,
    Toggle,
    Cancel,
}

// A physical press/release pair gets one id in the native adapter. Claims are
// processed in FIFO order before recorder work is spawned, so fast release
// cannot overtake the corresponding start while the microphone initializes.
#[derive(Clone, Copy)]
struct HotkeyInput {
    source: HotkeySource,
    trigger: ModeTrigger,
    gesture_id: u64,
    epoch: u64,
}

#[derive(Clone, Copy)]
enum HotkeyInputMessage {
    Gesture(HotkeyInput),
    FullCancel,
    Interrupt,
}

static INPUT_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn invalidate_pending_inputs() {
    INPUT_EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

static HOTKEY_INPUT: std::sync::OnceLock<tokio::sync::mpsc::UnboundedSender<HotkeyInputMessage>> =
    std::sync::OnceLock::new();

pub(crate) fn enqueue_hotkey(source: HotkeySource, trigger: ModeTrigger, gesture_id: u64) {
    if let Some(sender) = HOTKEY_INPUT.get() {
        let _ = sender.send(HotkeyInputMessage::Gesture(HotkeyInput {
            source,
            trigger,
            gesture_id,
            epoch: INPUT_EPOCH.load(std::sync::atomic::Ordering::SeqCst),
        }));
    }
}

pub(crate) fn enqueue_full_cancel() {
    invalidate_pending_inputs();
    if let Some(sender) = HOTKEY_INPUT.get() {
        let _ = sender.send(HotkeyInputMessage::FullCancel);
    }
}

pub(crate) fn interrupt_hotkeys() {
    invalidate_pending_inputs();
    if let Some(sender) = HOTKEY_INPUT.get() {
        let _ = sender.send(HotkeyInputMessage::Interrupt);
    }
}

#[derive(Clone, Copy)]
enum FifoCancelOrigin {
    Full,
    Interrupt,
}

fn claim_cancel_from_fifo(
    origin: FifoCancelOrigin,
    full_cancel: impl FnOnce() -> CancelClaim,
    interrupt: impl FnOnce() -> Option<CancelClaim>,
) -> EntryClaim {
    match origin {
        FifoCancelOrigin::Full => EntryClaim::Cancel(full_cancel()),
        FifoCancelOrigin::Interrupt => interrupt()
            .map(EntryClaim::Cancel)
            .unwrap_or(EntryClaim::Ignore),
    }
}

pub(crate) fn install_hotkey_dispatcher(app: tauri::AppHandle) {
    use tauri::Manager;
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    if HOTKEY_INPUT.set(sender).is_err() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        while let Some(input) = receiver.recv().await {
            let state = app.state::<AppState>();
            let claim = {
                let _gate = state.hotkey_gate.lock().await;
                match input {
                    HotkeyInputMessage::FullCancel => claim_cancel_from_fifo(
                        FifoCancelOrigin::Full,
                        || claim_cancel(&state),
                        || claim_cancel_for_interrupt(&state),
                    ),
                    HotkeyInputMessage::Interrupt => claim_cancel_from_fifo(
                        FifoCancelOrigin::Interrupt,
                        || claim_cancel(&state),
                        || claim_cancel_for_interrupt(&state),
                    ),
                    HotkeyInputMessage::Gesture(input)
                        if input.epoch == INPUT_EPOCH.load(std::sync::atomic::Ordering::SeqCst)
                            && !hotkey::is_suspended()
                            && state.exit_state.load(std::sync::atomic::Ordering::Acquire) == 0 =>
                    {
                        let mode = match input.source {
                            HotkeySource::Dictation => RecordingMode::Dictation,
                            HotkeySource::Verbatim => RecordingMode::Verbatim,
                            HotkeySource::Translation => RecordingMode::Translation(
                                lock_recover(&state.settings)
                                    .translation_target_language
                                    .clone(),
                            ),
                        };
                        let trial = crate::onboarding::snapshot(&state, "native_hotkey");
                        let claim = {
                            let mut lease = lock_recover(&state.operation_lease);
                            let mut manager = lock_recover(&state.manager);
                            let mut claim = claim_gesture(
                                &mut manager,
                                &mut lease,
                                mode,
                                input.trigger,
                                input.gesture_id,
                            );
                            if let EntryClaim::Start(generation) = &claim {
                                if admit_started_settlement(
                                    &mut manager,
                                    &mut lease,
                                    &state.delivery_settlements,
                                ) {
                                    manager.onboarding_trial = trial.map(|mut trial| {
                                        trial.recording_generation = *generation;
                                        trial
                                    });
                                } else {
                                    claim = EntryClaim::Ignore;
                                }
                            }
                            claim
                        };
                        if matches!(claim, EntryClaim::Cancel(_)) {
                            EntryClaim::Cancel(claim_cancel(&state))
                        } else {
                            claim
                        }
                    }
                    HotkeyInputMessage::Gesture(_) => EntryClaim::Ignore,
                }
            };
            if !matches!(claim, EntryClaim::Ignore) {
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    let state = handle.state::<AppState>();
                    execute_entry_claim(&handle, &state, claim).await;
                });
            }
        }
    });
}

fn claim_gesture(
    manager: &mut DictationManager,
    lease: &mut OperationLease,
    mode: RecordingMode,
    trigger: ModeTrigger,
    gesture_id: u64,
) -> EntryClaim {
    let source = mode.source();
    match trigger {
        ModeTrigger::Press | ModeTrigger::Toggle if manager.phase == Phase::Idle => {
            let Some(generation) = claim_start_manager_with_mode(manager, lease, mode) else {
                return EntryClaim::Ignore;
            };
            if trigger == ModeTrigger::Press {
                manager.hotkey_session = Some(HotkeySession {
                    source,
                    gesture_id,
                    generation,
                });
            }
            EntryClaim::Start(generation)
        }
        ModeTrigger::Toggle
            if manager.recording_source.is_none() || manager.recording_source == Some(source) =>
        {
            request_stop(manager)
                .map(EntryClaim::Stop)
                .unwrap_or(EntryClaim::Ignore)
        }
        ModeTrigger::Release | ModeTrigger::Cancel => {
            let owns_session = manager.hotkey_session.is_some_and(|owner| {
                owner.source == source
                    && owner.gesture_id == gesture_id
                    && owner.generation == manager.session_generation
            });
            if !owns_session {
                return EntryClaim::Ignore;
            }
            manager.hotkey_session = None;
            if trigger == ModeTrigger::Cancel {
                // Full cancellation (audio and pipeline) is claimed by the dispatcher.
                EntryClaim::Cancel(CancelClaim {
                    phase: manager.phase,
                    generation: manager.session_generation,
                    delivery_settlement: manager.delivery_settlement.clone(),
                    had_action: false,
                    text_action: None,
                    onboarding_trial: manager.onboarding_trial.clone(),
                    research_trace: manager.research_trace.clone(),
                })
            } else {
                request_stop(manager)
                    .map(EntryClaim::Stop)
                    .unwrap_or(EntryClaim::Ignore)
            }
        }
        _ => EntryClaim::Ignore,
    }
}

async fn handle_cli_toggle(app: &tauri::AppHandle, state: &AppState, mode: RecordingMode) {
    if state.exit_state.load(std::sync::atomic::Ordering::Acquire) != 0 || hotkey::is_suspended() {
        return;
    }
    let claim = {
        let _gate = state.hotkey_gate.lock().await;
        let mut lease = lock_recover(&state.operation_lease);
        let mut manager = lock_recover(&state.manager);
        let claim = claim_gesture(&mut manager, &mut lease, mode, ModeTrigger::Toggle, 0);
        if matches!(&claim, EntryClaim::Start(_))
            && !admit_started_settlement(&mut manager, &mut lease, &state.delivery_settlements)
        {
            EntryClaim::Ignore
        } else {
            claim
        }
    };
    execute_entry_claim(app, state, claim).await;
}

pub(crate) async fn handle_cli_toggle_verbatim(app: &tauri::AppHandle, state: &AppState) {
    handle_cli_toggle(app, state, RecordingMode::Verbatim).await;
}

pub(crate) async fn handle_hotkey_toggle(app: &tauri::AppHandle, state: &AppState) {
    handle_cli_toggle(app, state, RecordingMode::Dictation).await;
}

#[tauri::command]
pub(crate) async fn stop_dictation_from_hud(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    expected_generation: u64,
) -> Result<(), String> {
    stop_from_hud_internal(&app, &state, expected_generation).await
}

#[tauri::command]
pub(crate) async fn cancel_dictation_from_hud(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    expected_generation: u64,
) -> Result<(), String> {
    cancel_from_hud_internal(&app, &state, expected_generation).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::{AsrProvider, MockAsrProvider, RateLimits, Transcript};
    use std::time::Duration;

    #[test]
    fn direct_start_claim_cancels_request_installed_after_an_earlier_empty_precancel() {
        let pending = std::sync::Mutex::new(None);
        assert!(!crate::cancel_pending_provider_credential_read(
            &pending, None
        ));

        let request_id = "00000000-0000-4000-8000-000000000041".to_owned();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        *lock_recover(&pending) = Some(crate::PendingProviderCredentialRead {
            request_id: request_id.clone(),
            settings_generation: 9,
            cancelled: std::sync::Arc::clone(&cancelled),
            provider: crate::providers::EngineProvider::Groq,
            purpose: None,
            custom_origin: None,
        });

        let operation_lease = std::sync::Mutex::new(OperationLease::Idle);
        let manager = std::sync::Mutex::new(DictationManager::new());
        let settlements = DeliverySettlementRegistry::default();
        let generation = claim_start_with_provider_read_cancel(
            &pending,
            &operation_lease,
            &manager,
            &settlements,
            RecordingMode::Dictation,
            None,
        )
        .expect("direct recording claim");

        assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(
            *lock_recover(&operation_lease),
            OperationLease::LiveDictation
        );
        assert_eq!(lock_recover(&manager).phase, Phase::Starting);

        // A failed recorder start can return the manager to Idle, but must not
        // make the request from before that start eligible again.
        {
            let mut manager = lock_recover(&manager);
            assert_eq!(manager.session_generation, generation);
            reset_starting_manager(&mut manager);
        }
        release_operation_lease(
            &mut lock_recover(&operation_lease),
            OperationLease::LiveDictation,
        );
        assert_eq!(lock_recover(&manager).phase, Phase::Idle);
        let active = lock_recover(&pending);
        assert!(!crate::pending_provider_credential_read_matches(
            active.as_ref(),
            &request_id,
            &cancelled,
            9,
        ));
    }

    fn owned_replay_recording_fixture(
        now: std::time::Instant,
    ) -> (DictationManager, u64, OwnedReplayTargetBinding) {
        let mut manager = DictationManager::new();
        let mut lease = OperationLease::Idle;
        let generation = claim_owned_replay_start_manager(&mut manager, &mut lease)
            .expect("owned replay start claim");
        let target = context::TargetAppGuard {
            pid: 42,
            bundle_id: Some("com.google.Chrome".into()),
            browser_host: None,
            browser_target_token: None,
            browser_tab_token: None,
            window_token: Some(3),
            window_id: Some(4),
            window_identity_ambiguous: false,
            input_token: Some(5),
            secure_input: false,
        };
        let binding = OwnedReplayTargetBinding::from_nonce(
            target.clone(),
            "f45d5da3-fe13-445c-b105-acf6a1894c64",
            now,
        )
        .expect("known owned field binding");
        assert!(bind_owned_replay_target_manager(
            &mut manager,
            generation,
            binding.clone(),
        ));
        let mut recording_context = context::ContextSnapshot::general();
        recording_context.target_guard = target;
        assert!(enter_recording(&mut manager, recording_context).is_none());
        (manager, generation, binding)
    }

    fn owned_replay_eof(generation: u64, stream_id: u64) -> audio::OwnedReplayLifecycleEvent {
        audio::OwnedReplayLifecycleEvent {
            phase: "eof".into(),
            source_identity: format!("sha256:{}", audio::OWNED_WAV_SOURCE_SHA256),
            stream_id,
            session_generation: generation,
            frame_count: 3,
            sample_count: 960,
            acknowledged: true,
        }
    }

    fn admit_owned_source(
        manager: &mut DictationManager,
        generation: u64,
        configuration_generation: u64,
        stream_id: u64,
        now: std::time::Instant,
    ) -> bool {
        record_owned_replay_source_start_manager(
            manager,
            generation,
            configuration_generation,
            configuration_generation,
            configuration_generation,
            audio::OwnedReplayStartReceipt::test_fixture(generation, stream_id),
            now,
        )
    }

    #[test]
    fn owned_replay_eof_builds_history_authority_without_research_trace() {
        let now = std::time::Instant::now();
        let (mut manager, generation, binding) = owned_replay_recording_fixture(now);
        assert!(manager.research_trace.is_none());
        assert!(admit_owned_source(&mut manager, generation, 7, 44, now));

        let event = owned_replay_eof(generation, 44);
        let claim = claim_owned_replay_stop_manager_for_generation(
            &mut manager,
            generation,
            &event,
            7,
            7,
            now + Duration::from_millis(1),
        )
        .expect("matching EOF claims the ordinary stop path");
        let authority = claim
            .owned_replay_history_authority
            .as_ref()
            .expect("source, target, config, and EOF evidence compose authority");
        let target = claim.owned_replay_target.as_ref().unwrap();
        assert!(claim.research_trace.is_none());
        assert!(claim.owned_replay_learning_suppressed);
        assert!(authority.matches(claim.recording_generation, 7, target));
        assert!(target == &binding);
        assert_eq!(claim.recording_generation, generation);
    }

    #[test]
    fn owned_replay_history_authority_rejects_missing_or_stale_start_context() {
        let now = std::time::Instant::now();
        let event_generation = 9;
        let event = owned_replay_eof(event_generation, 44);

        let (mut no_source, generation, _) = owned_replay_recording_fixture(now);
        let claim = claim_owned_replay_stop_manager_for_generation(
            &mut no_source,
            generation,
            &owned_replay_eof(generation, 44),
            7,
            7,
            now,
        )
        .unwrap();
        assert!(claim.owned_replay_history_authority.is_none());

        let mut normal = DictationManager::new();
        let mut lease = OperationLease::Idle;
        let normal_generation =
            claim_start_manager_with_mode(&mut normal, &mut lease, RecordingMode::Dictation)
                .unwrap();
        assert!(enter_recording(&mut normal, context::ContextSnapshot::general()).is_none());
        assert!(!normal.owned_replay_learning_suppressed);
        let normal_claim = claim_owned_replay_stop_manager_for_generation(
            &mut normal,
            normal_generation,
            &owned_replay_eof(normal_generation, 44),
            7,
            7,
            now,
        )
        .unwrap();
        assert!(normal_claim.owned_replay_history_authority.is_none());

        let mut marker_only = DictationManager::new();
        let mut lease = OperationLease::Idle;
        let marker_generation =
            claim_owned_replay_start_manager(&mut marker_only, &mut lease).unwrap();
        marker_only.phase = Phase::Recording;
        marker_only.recording_context = Some(context::ContextSnapshot::general());
        let receipt = audio::OwnedReplayStartReceipt::test_fixture(marker_generation, 44);
        assert!(!record_owned_replay_source_start_manager(
            &mut marker_only,
            marker_generation,
            7,
            7,
            7,
            receipt,
            now,
        ));
        let marker_claim = claim_owned_replay_stop_manager_for_generation(
            &mut marker_only,
            marker_generation,
            &owned_replay_eof(marker_generation, 44),
            7,
            7,
            now,
        )
        .unwrap();
        assert!(marker_claim.owned_replay_history_authority.is_none());

        let (mut wrong_source, generation, _) = owned_replay_recording_fixture(now);
        assert!(admit_owned_source(
            &mut wrong_source,
            generation,
            7,
            43,
            now
        ));
        let claim = claim_owned_replay_stop_manager_for_generation(
            &mut wrong_source,
            generation,
            &owned_replay_eof(generation, 44),
            7,
            7,
            now,
        )
        .unwrap();
        assert!(claim.owned_replay_history_authority.is_none());

        let (mut changed_context, generation, _) = owned_replay_recording_fixture(now);
        changed_context
            .recording_context
            .as_mut()
            .unwrap()
            .target_guard
            .pid += 1;
        assert!(!admit_owned_source(
            &mut changed_context,
            generation,
            7,
            44,
            now,
        ));

        let (mut wrong_start_generation, generation, _) = owned_replay_recording_fixture(now);
        assert!(!record_owned_replay_source_start_manager(
            &mut wrong_start_generation,
            generation,
            7,
            7,
            7,
            audio::OwnedReplayStartReceipt::test_fixture(generation + 1, 44),
            now,
        ));

        let (mut changed_start_config, generation, _) = owned_replay_recording_fixture(now);
        assert!(!record_owned_replay_source_start_manager(
            &mut changed_start_config,
            generation,
            7,
            8,
            7,
            audio::OwnedReplayStartReceipt::test_fixture(generation, 44),
            now,
        ));

        let (mut onboarding, generation, _) = owned_replay_recording_fixture(now);
        assert!(admit_owned_source(&mut onboarding, generation, 7, 44, now));
        onboarding.onboarding_trial = Some(crate::onboarding::test_trial_snapshot(
            "10000000-0000-4000-8000-000000000001",
            "7",
            generation,
        ));
        let claim = claim_owned_replay_stop_manager_for_generation(
            &mut onboarding,
            generation,
            &owned_replay_eof(generation, 44),
            7,
            7,
            now,
        )
        .unwrap();
        assert!(claim.owned_replay_history_authority.is_none());

        let (mut wrong_event, generation, _) = owned_replay_recording_fixture(now);
        assert!(admit_owned_source(&mut wrong_event, generation, 7, 44, now));
        let claim = claim_owned_replay_stop_manager_for_generation(
            &mut wrong_event,
            generation,
            &event,
            7,
            7,
            now,
        )
        .unwrap();
        assert!(claim.owned_replay_history_authority.is_none());

        let (mut expired, generation, _) = owned_replay_recording_fixture(now);
        assert!(admit_owned_source(&mut expired, generation, 7, 44, now));
        let claim = claim_owned_replay_stop_manager_for_generation(
            &mut expired,
            generation,
            &owned_replay_eof(generation, 44),
            7,
            7,
            now + OWNED_REPLAY_TTL + Duration::from_millis(1),
        )
        .unwrap();
        assert!(claim.owned_replay_history_authority.is_none());

        let (mut cancelled, generation, _) = owned_replay_recording_fixture(now);
        assert!(admit_owned_source(&mut cancelled, generation, 7, 44, now));
        cancelled.cancellation.cancel();
        let claim = claim_owned_replay_stop_manager_for_generation(
            &mut cancelled,
            generation,
            &owned_replay_eof(generation, 44),
            7,
            7,
            now,
        )
        .unwrap();
        assert!(claim.owned_replay_history_authority.is_none());

        let (mut changed_config, generation, _) = owned_replay_recording_fixture(now);
        assert!(admit_owned_source(
            &mut changed_config,
            generation,
            7,
            44,
            now
        ));
        let claim = claim_owned_replay_stop_manager_for_generation(
            &mut changed_config,
            generation,
            &owned_replay_eof(generation, 44),
            8,
            8,
            now,
        )
        .unwrap();
        assert!(claim.owned_replay_history_authority.is_none());
    }

    #[test]
    fn stale_owned_replay_eof_cannot_stop_a_newer_recording() {
        let mut manager = DictationManager::new();
        manager.phase = Phase::Recording;
        manager.session_generation = 22;
        manager.owned_replay_learning_suppressed = true;

        assert!(claim_stop_manager_for_generation(&mut manager, 21).is_none());
        assert_eq!(manager.phase, Phase::Recording);
        assert_eq!(manager.session_generation, 22);

        let claim = claim_stop_manager_for_generation(&mut manager, 22)
            .expect("only the matching owned source can claim its normal stop");
        assert_eq!(claim.recording_generation, 22);
        assert!(claim.owned_replay_learning_suppressed);
        assert!(claim.owned_replay_history_authority.is_none());
        assert_eq!(manager.phase, Phase::Stopping);
    }

    #[test]
    fn hud_cancel_requires_a_matching_non_idle_session_generation() {
        assert!(!hud_cancel_matches_session(Phase::Recording, 22, 22));
        assert!(hud_cancel_matches_session(Phase::Starting, 22, 22));
        assert!(hud_cancel_matches_session(Phase::Stopping, 22, 22));
        assert!(hud_cancel_matches_session(Phase::Processing, 22, 22));
        assert!(!hud_cancel_matches_session(Phase::Idle, 22, 22));
        assert!(!hud_cancel_matches_session(Phase::Processing, 23, 22));
    }

    #[test]
    fn owned_replay_binding_requires_exact_nonce_field_and_utf16_selection() {
        let now = std::time::Instant::now();
        let target = context::TargetAppGuard {
            pid: 42,
            bundle_id: Some("com.google.Chrome".into()),
            browser_host: None,
            browser_target_token: None,
            browser_tab_token: None,
            window_token: Some(3),
            window_id: Some(4),
            window_identity_ambiguous: false,
            input_token: Some(5),
            secure_input: false,
        };
        let nonce = "f45d5da3-fe13-445c-b105-acf6a1894c64";
        let binding = OwnedReplayTargetBinding::from_nonce(target.clone(), nonce, now)
            .expect("canonical nonce and Chrome target are eligible");
        let prefix = format!("VoiceFlow V7 oracle {nonce}\n前缀🙂[");
        let source = crate::paste::CapturedTextActionSource {
            kind: crate::paste::TextActionSourceKind::Selection,
            text: OWNED_REPLAY_SELECTED_TEXT.into(),
            text_fingerprint: 1,
            field_fingerprint: Some(2),
            selection_range: Some((
                prefix.encode_utf16().count() as i64,
                OWNED_REPLAY_SELECTED_TEXT.encode_utf16().count() as i64,
            )),
            editable: true,
            copy_only_selection: false,
        };
        assert!(binding.admits_capture(
            &target,
            binding.expected_full_value(),
            &source,
            now + std::time::Duration::from_secs(1),
        ));
        assert!(!binding.admits_capture(
            &target,
            &binding
                .expected_full_value()
                .replace(nonce, "00000000-0000-4000-8000-000000000000"),
            &source,
            now,
        ));

        let mut changed_target = target.clone();
        changed_target.pid += 1;
        assert!(!binding.admits_capture(
            &changed_target,
            binding.expected_full_value(),
            &source,
            now,
        ));
        let mut changed_input = target.clone();
        changed_input.input_token = Some(6);
        assert!(!binding.admits_capture(
            &changed_input,
            binding.expected_full_value(),
            &source,
            now,
        ));

        let mut changed_source = source.clone();
        changed_source.selection_range = Some((source.selection_range.unwrap().0 + 1, 18));
        assert!(!binding.admits_capture(
            &target,
            binding.expected_full_value(),
            &changed_source,
            now,
        ));
        let mut non_editable = source.clone();
        non_editable.editable = false;
        assert!(!binding.admits_capture(
            &target,
            binding.expected_full_value(),
            &non_editable,
            now,
        ));
        assert!(!binding.is_live(now + OWNED_REPLAY_TTL));
        assert!(OwnedReplayTargetBinding::from_nonce(target, "not-a-uuid", now).is_none());
    }

    #[test]
    fn textedit_owned_replay_requires_the_nonce_named_document_and_allows_edited_title_afterward() {
        let now = std::time::Instant::now();
        let nonce = "5f1ff90d-cd1d-444d-a342-96721359c336";
        let target = context::TargetAppGuard {
            pid: 42,
            bundle_id: Some("com.apple.TextEdit".into()),
            browser_host: None,
            browser_target_token: None,
            browser_tab_token: None,
            window_token: Some(8),
            window_id: Some(4),
            window_identity_ambiguous: false,
            input_token: Some(9),
            secure_input: false,
        };
        let expected_path =
            std::path::PathBuf::from(format!("/private/owned-targets/VoiceFlow-V24-{nonce}.txt"));
        let binding = OwnedReplayTargetBinding::from_nonce_with_document(
            target.clone(),
            nonce,
            Some(expected_path.clone()),
            now,
        )
        .expect("only the bounded TextEdit fixture is admitted");
        let title = format!("VoiceFlow-V24-{nonce}.txt");
        assert!(binding.admits_native_window(&title, Some(&expected_path), true));
        assert!(binding.admits_native_window(
            "VoiceFlow-V24 fixture — Edited",
            Some(&expected_path),
            false
        ));
        assert!(!binding.admits_native_window("Other document.txt", Some(&expected_path), true));
        assert!(!binding.admits_native_window(
            &title,
            Some(std::path::Path::new("/tmp/other.txt")),
            false
        ));

        let chrome = context::TargetAppGuard {
            bundle_id: Some("com.google.Chrome".into()),
            ..target
        };
        assert!(OwnedReplayTargetBinding::from_nonce_with_document(
            chrome.clone(),
            nonce,
            None,
            now,
        )
        .is_some());
        assert!(OwnedReplayTargetBinding::from_nonce_with_document(
            chrome,
            nonce,
            Some(expected_path),
            now,
        )
        .is_none());
    }

    #[test]
    fn external_fixture_finish_never_releases_recorder_or_newer_claim() {
        let old =
            crate::onboarding::test_trial_snapshot("10000000-0000-4000-8000-000000000001", "7", 4);
        let mut manager = DictationManager::new();
        manager.onboarding_trial = Some(old.clone());
        manager.session_generation = 5;
        manager.phase = Phase::Recording;
        assert_eq!(
            finish_onboarding_fixture_operation(&mut manager, &old),
            None
        );
        assert_eq!(manager.phase, Phase::Recording);
        let mut next = old.clone();
        next.recording_generation = 6;
        manager.onboarding_trial = Some(next);
        manager.phase = Phase::Processing;
        assert_eq!(
            finish_onboarding_fixture_operation(&mut manager, &old),
            None
        );
        assert_eq!(manager.phase, Phase::Processing);
        manager.onboarding_trial = Some(old.clone());
        assert_eq!(
            finish_onboarding_fixture_operation(&mut manager, &old),
            Some(5)
        );
        assert_eq!(manager.phase, Phase::Idle);
        assert!(manager.cancellation.is_cancelled());
    }
    #[derive(Default)]
    struct MockRecorder {
        starts: usize,
        stops: usize,
        cancels: usize,
        stop_audio: Vec<u8>,
        start_entered: Option<std::sync::Arc<std::sync::Barrier>>,
        start_release: Option<std::sync::Arc<std::sync::Barrier>>,
    }

    impl RecorderBackend for MockRecorder {
        fn feedback_cue_handle(&self, _session_generation: u64) -> Option<audio::CaptureCueHandle> {
            None
        }

        fn start(
            &mut self,
            _app: Option<&tauri::AppHandle>,
            _session: &str,
            _input_device: &str,
            _chunk_length_secs: usize,
            _input_gain: f32,
            _prefetch_tx: prefetch_asr::PrefetchInbox,
        ) -> Result<(), audio::AudioError> {
            if let Some(entered) = self.start_entered.clone() {
                entered.wait();
            }
            if let Some(release) = self.start_release.clone() {
                release.wait();
            }
            self.starts += 1;
            Ok(())
        }

        fn stop_with_chunks(
            &mut self,
            _app: Option<&tauri::AppHandle>,
            _options: audio::StopOptions,
        ) -> Result<audio::FinishedCapture, audio::AudioError> {
            self.stops += 1;
            Ok(audio::FinishedCapture::without_spool(
                self.stop_audio.clone(),
                Vec::new(),
            ))
        }

        fn cancel(&mut self, _app: Option<&tauri::AppHandle>) {
            self.cancels += 1;
        }
    }

    trait EventCollector {
        fn emit(&mut self, event: &'static str, value: &str);
    }

    #[derive(Default)]
    struct Events(Vec<(&'static str, String)>);

    impl EventCollector for Events {
        fn emit(&mut self, event: &'static str, value: &str) {
            self.0.push((event, value.to_owned()));
        }
    }

    struct Harness {
        manager: DictationManager,
        lease: OperationLease,
        recorder: MockRecorder,
        events: Events,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                manager: DictationManager::new(),
                lease: OperationLease::Idle,
                recorder: MockRecorder::default(),
                events: Events::default(),
            }
        }

        fn begin_start(&mut self) -> u64 {
            claim_start_manager(&mut self.manager, &mut self.lease).expect("start claim")
        }

        fn start_recorder(&mut self) {
            let (prefetch_tx, _receiver) = prefetch_asr::PrefetchAsrSession::channel();
            let recorder: &mut dyn RecorderBackend = &mut self.recorder;
            recorder
                .start(None, "test-session", "default", 10, 1.0, prefetch_tx)
                .expect("mock recorder start");
        }

        fn stop_recorder(&mut self) -> Vec<u8> {
            let recorder: &mut dyn RecorderBackend = &mut self.recorder;
            recorder
                .stop_with_chunks(None, audio::StopOptions::default())
                .expect("mock recorder stop")
                .wav
        }

        fn cancel_recorder(&mut self) {
            let recorder: &mut dyn RecorderBackend = &mut self.recorder;
            recorder.cancel(None);
        }

        fn complete_start(&mut self, generation: u64) {
            if self.manager.phase == Phase::Starting
                && self.manager.session_generation == generation
                && !self.manager.cancellation.is_cancelled()
            {
                self.start_recorder();
                self.manager.phase = Phase::Recording;
                self.events.emit("dictation://state", "recording");
            }
        }

        async fn stop_with(&mut self, provider: &dyn AsrProvider) {
            let generation = self.manager.session_generation.wrapping_add(1);
            self.manager.session_generation = generation;
            self.manager.phase = Phase::Stopping;
            let audio = self.stop_recorder();
            self.manager.phase = Phase::Processing;
            let transcript = provider
                .transcribe_batch(audio, crate::asr::AsrOptions::default())
                .await
                .expect("mock ASR");
            if self.manager.phase == Phase::Processing
                && self.manager.session_generation == generation
            {
                self.events.emit("dictation://result", &transcript.text);
                if let Some(settlement) = &self.manager.delivery_settlement {
                    settlement.seal(DeliveryTerminalOutcome::completed("done", "none", None));
                }
                self.manager.phase = Phase::Idle;
                self.manager.skip_llm_cleanup = false;
                self.manager.translation_target_language = None;
                release_operation_lease(&mut self.lease, OperationLease::LiveDictation);
            }
        }
    }

    #[test]
    fn interrupt_during_processing_keeps_session() {
        let mut harness = Harness::new();
        let generation = harness.begin_start();
        harness.complete_start(generation);
        harness.manager.recording_context = Some(context::ContextSnapshot::general());
        let context = harness
            .manager
            .recording_context
            .clone()
            .expect("recording context exists before stop");
        let stop = claim_stop_manager(&mut harness.manager).expect("recording stop claim");
        harness.manager.phase = Phase::Processing;
        let processing_generation = harness.manager.session_generation;
        let settlement = harness
            .manager
            .delivery_settlement
            .clone()
            .expect("processing retains delivery settlement owner");
        let cancellation = harness.manager.cancellation.clone();

        assert!(claim_cancel_for_interrupt_manager(&mut harness.manager).is_none());

        assert_eq!(harness.manager.phase, Phase::Processing);
        assert_eq!(harness.manager.session_generation, processing_generation);
        assert!(!cancellation.is_cancelled());
        assert!(std::sync::Arc::ptr_eq(
            &settlement,
            harness.manager.delivery_settlement.as_ref().unwrap()
        ));
        assert_eq!(harness.lease, OperationLease::LiveDictation);
        assert_eq!(stop.recording_context, context);
        assert_eq!(settlement.snapshot().terminal_outcome, None);
    }

    fn fifo_full_cancel_for_test(manager: &mut DictationManager, had_action: bool) -> CancelClaim {
        let phase = manager.phase;
        let delivery_settlement = manager.delivery_settlement.clone();
        claim_cancel_manager(manager, had_action);
        CancelClaim {
            phase,
            generation: manager.session_generation,
            delivery_settlement,
            had_action: had_action || phase != Phase::Idle,
            text_action: None,
            onboarding_trial: manager.onboarding_trial.clone(),
            research_trace: manager.research_trace.clone(),
        }
    }

    #[test]
    fn fifo_esc_claims_full_cancel_while_generic_interrupt_keeps_phase_policy() {
        for phase in [Phase::Stopping, Phase::Processing] {
            let mut manager = DictationManager::new();
            let mut lease = OperationLease::Idle;
            claim_start_manager(&mut manager, &mut lease).expect("start claim");
            let settlement = manager.delivery_settlement.clone();
            manager.phase = phase;
            let token = manager.cancellation.clone();
            let manager = std::cell::RefCell::new(manager);
            let claim = claim_cancel_from_fifo(
                FifoCancelOrigin::Full,
                || fifo_full_cancel_for_test(&mut manager.borrow_mut(), false),
                || claim_cancel_for_interrupt_manager(&mut manager.borrow_mut()),
            );
            let manager = manager.into_inner();
            let EntryClaim::Cancel(claim) = claim else {
                panic!("FIFO Esc must select the common full-cancel claim");
            };
            assert_eq!(claim.phase, phase);
            assert_eq!(manager.phase, Phase::Idle);
            assert_eq!(manager.session_generation, claim.generation);
            assert!(token.is_cancelled());
            assert_eq!(
                settlement
                    .unwrap()
                    .snapshot()
                    .terminal_outcome
                    .as_ref()
                    .map(|outcome| outcome.kind),
                Some(crate::delivery_settlement::DeliveryOutcomeKind::Cancelled)
            );
        }

        let mut manager = DictationManager::new();
        let mut lease = OperationLease::Idle;
        claim_start_manager(&mut manager, &mut lease).expect("start claim");
        manager.phase = Phase::Idle;
        let generation = manager.session_generation;
        let manager = std::cell::RefCell::new(manager);
        let claim = claim_cancel_from_fifo(
            FifoCancelOrigin::Full,
            || fifo_full_cancel_for_test(&mut manager.borrow_mut(), true),
            || claim_cancel_for_interrupt_manager(&mut manager.borrow_mut()),
        );
        let mut manager = manager.into_inner();
        let EntryClaim::Cancel(claim) = claim else {
            panic!("queued full cancellation must retain its generic cleanup claim");
        };
        assert_eq!(claim.phase, Phase::Idle);
        assert!(claim.had_action);
        assert_eq!(claim.generation, generation.wrapping_add(1));

        manager.phase = Phase::Processing;
        let generation = manager.session_generation;
        let manager = std::cell::RefCell::new(manager);
        let claim = claim_cancel_from_fifo(
            FifoCancelOrigin::Interrupt,
            || fifo_full_cancel_for_test(&mut manager.borrow_mut(), false),
            || claim_cancel_for_interrupt_manager(&mut manager.borrow_mut()),
        );
        let manager = manager.into_inner();
        assert!(matches!(claim, EntryClaim::Ignore));
        assert_eq!(manager.phase, Phase::Processing);
        assert_eq!(manager.session_generation, generation);
    }

    #[test]
    fn interrupt_during_recording_cancels() {
        let mut manager = DictationManager::new();
        let mut lease = OperationLease::Idle;
        let generation = claim_start_manager(&mut manager, &mut lease).expect("start claim");
        let starting_cancel =
            claim_cancel_for_interrupt_manager(&mut manager).expect("starting interrupt cancels");
        assert_eq!(starting_cancel.phase, Phase::Starting);
        assert_eq!(manager.phase, Phase::Idle);
        assert_eq!(manager.session_generation, generation.wrapping_add(1));
        assert!(manager.cancellation.is_cancelled());

        let mut manager = DictationManager::new();
        let mut lease = OperationLease::Idle;
        let generation = claim_start_manager(&mut manager, &mut lease).expect("start claim");
        enter_recording(&mut manager, context::ContextSnapshot::general());
        let owner = manager
            .delivery_settlement
            .clone()
            .expect("recording settlement owner");
        let claim =
            claim_cancel_for_interrupt_manager(&mut manager).expect("recording interrupt cancels");
        assert_eq!(claim.phase, Phase::Recording);
        assert_eq!(manager.phase, Phase::Idle);
        assert_eq!(manager.session_generation, generation.wrapping_add(1));
        assert!(manager.cancellation.is_cancelled());
        assert_eq!(
            owner
                .snapshot()
                .terminal_outcome
                .as_ref()
                .map(|outcome| outcome.kind),
            Some(crate::delivery_settlement::DeliveryOutcomeKind::Cancelled)
        );
    }

    #[test]
    fn quick_hold_release_during_starting_stops_after_mic_start() {
        let mut manager = DictationManager::new();
        let mut lease = OperationLease::Idle;
        assert!(matches!(
            claim_gesture(
                &mut manager,
                &mut lease,
                RecordingMode::Dictation,
                ModeTrigger::Press,
                1
            ),
            EntryClaim::Start(1)
        ));
        assert!(matches!(
            claim_gesture(
                &mut manager,
                &mut lease,
                RecordingMode::Dictation,
                ModeTrigger::Release,
                1
            ),
            EntryClaim::Ignore
        ));
        assert!(manager.stop_when_recording);
        let claim = enter_recording(&mut manager, context::ContextSnapshot::general()).unwrap();
        assert!(claim.ended_during_start);
        assert_eq!(manager.phase, Phase::Stopping);
    }

    #[test]
    fn two_real_taps_during_startup_queue_stop_without_a_time_lock() {
        let mut manager = DictationManager::new();
        let mut lease = OperationLease::Idle;
        assert!(matches!(
            claim_gesture(
                &mut manager,
                &mut lease,
                RecordingMode::Dictation,
                ModeTrigger::Toggle,
                1
            ),
            EntryClaim::Start(_)
        ));
        claim_gesture(
            &mut manager,
            &mut lease,
            RecordingMode::Dictation,
            ModeTrigger::Toggle,
            2,
        );
        assert!(manager.stop_when_recording);
        assert!(enter_recording(&mut manager, context::ContextSnapshot::general()).is_some());
    }

    #[test]
    fn release_only_ends_its_own_source_and_gesture() {
        for (owner, other) in [
            (RecordingMode::Dictation, RecordingMode::Verbatim),
            (
                RecordingMode::Verbatim,
                RecordingMode::Translation("en".into()),
            ),
            (
                RecordingMode::Translation("zh".into()),
                RecordingMode::Dictation,
            ),
        ] {
            let mut manager = DictationManager::new();
            let mut lease = OperationLease::Idle;
            claim_gesture(
                &mut manager,
                &mut lease,
                owner.clone(),
                ModeTrigger::Press,
                10,
            );
            enter_recording(&mut manager, context::ContextSnapshot::general());
            assert!(matches!(
                claim_gesture(&mut manager, &mut lease, other, ModeTrigger::Release, 10),
                EntryClaim::Ignore
            ));
            assert!(matches!(
                claim_gesture(
                    &mut manager,
                    &mut lease,
                    owner.clone(),
                    ModeTrigger::Release,
                    9
                ),
                EntryClaim::Ignore
            ));
            assert!(matches!(
                claim_gesture(
                    &mut manager,
                    &mut lease,
                    owner.clone(),
                    ModeTrigger::Press,
                    11
                ),
                EntryClaim::Ignore
            ));
            assert_eq!(manager.phase, Phase::Recording);
            assert!(matches!(
                claim_gesture(&mut manager, &mut lease, owner, ModeTrigger::Release, 10),
                EntryClaim::Stop(_)
            ));
        }
    }

    #[test]
    fn processing_and_stopping_ignore_new_gestures() {
        for phase in [Phase::Processing, Phase::Stopping] {
            let mut manager = DictationManager::new();
            manager.phase = phase;
            let mut lease = OperationLease::LiveDictation;
            for trigger in [
                ModeTrigger::Toggle,
                ModeTrigger::Press,
                ModeTrigger::Release,
            ] {
                assert!(matches!(
                    claim_gesture(
                        &mut manager,
                        &mut lease,
                        RecordingMode::Dictation,
                        trigger,
                        1
                    ),
                    EntryClaim::Ignore
                ));
            }
            assert_eq!(manager.phase, phase);
        }
    }

    #[test]
    fn delivery_transaction_identity_survives_recording_processing_and_cancel() {
        let mut manager = DictationManager::new();
        let mut lease = OperationLease::Idle;
        let recording_generation =
            claim_start_manager(&mut manager, &mut lease).expect("start claim");
        manager.phase = Phase::Recording;
        let owner = manager
            .delivery_settlement
            .clone()
            .expect("ordinary capture has a settlement owner");
        let transaction_id = owner
            .snapshot()
            .transaction_id
            .clone()
            .expect("OS entropy yields an opaque transaction ID");

        let stop = claim_stop_manager(&mut manager).expect("stop claim");
        assert_eq!(stop.recording_generation, recording_generation);
        assert_eq!(
            stop.delivery_settlement
                .as_ref()
                .and_then(|settlement| settlement.snapshot().transaction_id),
            Some(transaction_id.clone())
        );
        assert_eq!(
            manager.session_generation,
            recording_generation.wrapping_add(1)
        );
        manager.phase = Phase::Processing;

        assert_eq!(claim_cancel_manager(&mut manager, false), Phase::Processing);
        assert_eq!(
            manager.session_generation,
            recording_generation.wrapping_add(2)
        );
        let snapshot = owner.snapshot();
        assert_eq!(snapshot.transaction_id, Some(transaction_id));
        assert!(snapshot.writer_admission_sealed);
        assert!(snapshot.writers_closed);
        assert_eq!(
            snapshot
                .terminal_outcome
                .as_ref()
                .map(|outcome| outcome.kind),
            Some(crate::delivery_settlement::DeliveryOutcomeKind::Cancelled)
        );
    }

    #[test]
    fn opaque_transaction_identity_survives_a_wrapped_manager_generation() {
        let mut manager = DictationManager::new();
        let mut lease = OperationLease::Idle;
        manager.session_generation = u64::MAX;
        let first_generation = claim_start_manager(&mut manager, &mut lease).expect("first start");
        assert_eq!(first_generation, 0);
        let first_owner = manager.delivery_settlement.clone().expect("first owner");
        let first_id = first_owner
            .snapshot()
            .transaction_id
            .expect("OS entropy yields an opaque transaction ID");

        manager.phase = Phase::Recording;
        claim_cancel_manager(&mut manager, false);
        lease = OperationLease::Idle;
        manager.session_generation = u64::MAX;
        let second_generation =
            claim_start_manager(&mut manager, &mut lease).expect("second start");
        assert_eq!(second_generation, first_generation);
        let second_id = manager
            .delivery_settlement
            .as_ref()
            .and_then(|owner| owner.snapshot().transaction_id)
            .expect("OS entropy yields an opaque transaction ID");
        assert_ne!(first_id, second_id);
    }

    #[test]
    fn new_start_waits_for_the_previous_admitted_writer_to_unwind() {
        let mut manager = DictationManager::new();
        let mut lease = OperationLease::Idle;
        let _first_generation = claim_start_manager(&mut manager, &mut lease).expect("first start");
        manager.phase = Phase::Recording;
        let owner = manager
            .delivery_settlement
            .clone()
            .expect("ordinary capture has a settlement owner");
        let first_id = owner
            .snapshot()
            .transaction_id
            .expect("OS entropy yields an opaque transaction ID");
        let writer = owner.admit_writer().expect("writer admission is open");

        assert_eq!(claim_cancel_manager(&mut manager, false), Phase::Recording);
        assert!(!owner.snapshot().writers_closed);
        lease = OperationLease::Idle;
        assert!(claim_start_manager(&mut manager, &mut lease).is_none());
        assert_eq!(
            manager
                .delivery_settlement
                .as_ref()
                .and_then(|settlement| settlement.snapshot().transaction_id),
            Some(first_id.clone())
        );

        drop(writer);
        assert!(owner.snapshot().writers_closed);
        let _second_generation =
            claim_start_manager(&mut manager, &mut lease).expect("closed permits start");
        let second_id = manager
            .delivery_settlement
            .as_ref()
            .and_then(|settlement| settlement.snapshot().transaction_id.clone())
            .expect("new owner has a fresh opaque ID");
        assert_ne!(first_id, second_id);
    }

    #[test]
    fn cancellation_invalidates_late_release_even_from_same_hotkey() {
        let mut manager = DictationManager::new();
        let mut lease = OperationLease::Idle;
        claim_gesture(
            &mut manager,
            &mut lease,
            RecordingMode::Dictation,
            ModeTrigger::Press,
            1,
        );
        claim_cancel_manager(&mut manager, false);
        lease = OperationLease::Idle;
        claim_gesture(
            &mut manager,
            &mut lease,
            RecordingMode::Dictation,
            ModeTrigger::Press,
            2,
        );
        assert!(matches!(
            claim_gesture(
                &mut manager,
                &mut lease,
                RecordingMode::Dictation,
                ModeTrigger::Release,
                1
            ),
            EntryClaim::Ignore
        ));
        assert!(!manager.stop_when_recording);
        assert_eq!(manager.phase, Phase::Starting);
    }

    #[test]
    fn idle_claim_moves_to_starting_and_reserves_live_operation() {
        let mut harness = Harness::new();
        let generation = harness.begin_start();
        assert_eq!(generation, 1);
        assert_eq!(harness.manager.phase, Phase::Starting);
        assert_eq!(harness.lease, OperationLease::LiveDictation);
        harness.complete_start(generation);
        assert_eq!(harness.manager.phase, Phase::Recording);
    }

    #[tokio::test]
    async fn stop_uses_injected_asr_and_emits_hello() {
        let mut harness = Harness::new();
        let generation = harness.begin_start();
        harness.complete_start(generation);
        harness.recorder.stop_audio = b"wav".to_vec();
        let provider = MockAsrProvider::new(
            Ok(Transcript {
                text: "hello".into(),
                asr_text: None,
                provider_cleaned_candidate: None,
                language: None,
                confidence: None,
                segments: Vec::new(),
                words: Vec::new(),
                tokens: Vec::new(),
                limits: RateLimits::default(),
            }),
            Duration::ZERO,
        );

        harness.stop_with(&provider).await;

        assert_eq!(provider.calls(), 1);
        assert!(harness
            .events
            .0
            .iter()
            .any(|(event, value)| *event == "dictation://result" && value == "hello"));
    }

    #[test]
    fn stale_start_generation_is_discarded() {
        let mut harness = Harness::new();
        let stale = harness.begin_start();
        claim_cancel_manager(&mut harness.manager, false);
        harness.complete_start(stale);
        assert_eq!(harness.manager.phase, Phase::Idle);
        assert_eq!(harness.recorder.starts, 0);
    }

    #[test]
    fn starting_cancel_completes_while_delayed_start_is_in_flight() {
        let entered = std::sync::Arc::new(std::sync::Barrier::new(2));
        let release = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut recorder = MockRecorder {
            start_entered: Some(entered.clone()),
            start_release: Some(release.clone()),
            ..MockRecorder::default()
        };
        let mut manager = DictationManager::new();
        let mut lease = OperationLease::Idle;
        let generation = claim_start_manager(&mut manager, &mut lease).expect("start claim");

        std::thread::scope(|scope| {
            scope.spawn(|| {
                let (prefetch_tx, _receiver) = prefetch_asr::PrefetchAsrSession::channel();
                let backend: &mut dyn RecorderBackend = &mut recorder;
                backend
                    .start(None, "test-session", "default", 10, 1.0, prefetch_tx)
                    .expect("delayed start");
            });
            entered.wait();
            let started = std::time::Instant::now();
            assert_eq!(claim_cancel_manager(&mut manager, false), Phase::Starting);
            assert!(started.elapsed() < std::time::Duration::from_millis(50));
            assert_eq!(manager.phase, Phase::Idle);
            assert_eq!(manager.session_generation, generation.wrapping_add(1));
            release.wait();
        });

        assert_eq!(manager.phase, Phase::Idle);
        assert_eq!(recorder.starts, 1);
        assert_eq!(recorder.cancels, 0);
        if manager.phase == Phase::Starting && manager.session_generation == generation {
            panic!("late start committed after cancel");
        }
    }

    #[test]
    fn starting_cancel_is_idempotent_and_does_not_touch_recorder() {
        let mut harness = Harness::new();
        harness.begin_start();
        assert_eq!(
            claim_cancel_manager(&mut harness.manager, false),
            Phase::Starting
        );
        assert_eq!(
            claim_cancel_manager(&mut harness.manager, false),
            Phase::Idle
        );
        assert_eq!(harness.manager.phase, Phase::Idle);
        assert_eq!(harness.recorder.cancels, 0);
    }

    #[test]
    fn recorder_backend_cancel_uses_the_injected_object() {
        let mut harness = Harness::new();
        harness.cancel_recorder();
        assert_eq!(harness.recorder.cancels, 1);
    }
}

#[cfg(test)]
mod skip_session_tests {
    use super::*;
    #[test]
    fn translation_is_captured_on_successful_start_and_never_changes_saved_settings() {
        let settings = crate::store::Settings {
            output_mode: "bullets".into(),
            translation_target_language: "ja".into(),
            ..crate::store::Settings::default()
        };
        let mut manager = DictationManager::new();
        let mut lease = OperationLease::LiveDictation;
        assert!(claim_start_manager_with_mode(
            &mut manager,
            &mut lease,
            RecordingMode::Translation("ja".into())
        )
        .is_none());
        assert!(manager.translation_target_language.is_none());
        lease = OperationLease::Idle;
        claim_start_manager_with_mode(
            &mut manager,
            &mut lease,
            RecordingMode::Translation("ja".into()),
        )
        .unwrap();
        assert!(!manager.skip_llm_cleanup);
        enter_recording(&mut manager, context::ContextSnapshot::general());
        let claim = claim_stop_manager(&mut manager).unwrap();
        let mut session_settings = settings.clone();
        apply_session_mode(
            &mut session_settings,
            claim.skip_llm_cleanup,
            claim.translation_target_language.as_deref(),
        );
        assert_eq!(session_settings.output_mode, "translation");
        assert_eq!(session_settings.translation_target_language, "ja");
        assert_eq!(settings.output_mode, "bullets");
        claim_cancel_manager(&mut manager, false);
        assert!(manager.translation_target_language.is_none());
        lease = OperationLease::Idle;
        claim_start_manager_with_mode(&mut manager, &mut lease, RecordingMode::Dictation).unwrap();
        assert!(manager.translation_target_language.is_none());
        assert_eq!(claim.translation_target_language.as_deref(), Some("ja"));
    }

    #[test]
    fn session_learning_snapshot_keeps_captured_dictionary_and_context() {
        let mapping = context::AppMapping {
            id: "editor".into(),
            label: "Editor".into(),
            family: context::ContextFamily::Document,
            mode_id: None,
            bundle_id: Some("com.example.editor".into()),
            executable: None,
            browser_host: None,
            browser_path_prefix: None,
            focused_field: None,
            source_permissions: context::ContextSourcePermissions::default(),
            style_example_input: Some("captured input".into()),
            style_example_output: Some("captured output".into()),
            style_example_pairs: vec![context::StyleExamplePair {
                input: "captured input".into(),
                output: "captured output".into(),
            }],
            style_examples_approved: true,
            enabled: true,
            cleanup_effort: None,
            cleanup_intensity: None,
            cleanup_enabled: true,
            dictionary_learn_enabled: true,
        };
        let mut captured = crate::store::Settings {
            dictionary: vec!["TypeScript".into()],
            context_mappings: vec![mapping],
            ..crate::store::Settings::default()
        };
        assert!(
            captured.add_dictionary_source("TypeScript", crate::store::DictionarySource::Manual)
        );
        let snapshot = SessionLearningSnapshot::from_settings(&captured);

        let mut updated = captured.clone();
        updated.dictionary = vec!["JavaScript".into()];
        updated.context_mappings[0].style_example_input = Some("learned input".into());
        updated.context_mappings[0].style_example_output = Some("learned output".into());
        updated.context_mappings[0].style_example_pairs = vec![context::StyleExamplePair {
            input: "learned input".into(),
            output: "learned output".into(),
        }];
        updated.output_mode = "bullets".into();
        snapshot.apply_to(&mut updated);

        assert_eq!(updated.dictionary, vec!["TypeScript"]);
        assert!(
            updated.has_dictionary_source("TypeScript", &crate::store::DictionarySource::Manual)
        );
        assert_eq!(
            updated.context_mappings[0].style_example_input.as_deref(),
            Some("captured input")
        );
        assert_eq!(updated.output_mode, "bullets");
    }

    #[test]
    fn starting_failure_clears_translation_override() {
        let mut manager = DictationManager::new();
        let mut lease = OperationLease::Idle;
        claim_start_manager_with_mode(
            &mut manager,
            &mut lease,
            RecordingMode::Translation("en".into()),
        )
        .unwrap();
        reset_starting_manager(&mut manager);
        assert!(manager.translation_target_language.is_none());
    }
    #[test]
    fn skip_policy_belongs_to_successfully_claimed_session() {
        let mut manager = DictationManager::new();
        let mut lease = OperationLease::LiveDictation;
        assert!(
            claim_start_manager_with_mode(&mut manager, &mut lease, RecordingMode::Verbatim)
                .is_none()
        );
        assert!(!manager.skip_llm_cleanup);
        assert!(manager.hotkey_session.is_none());
        lease = OperationLease::Idle;
        claim_start_manager_with_mode(&mut manager, &mut lease, RecordingMode::Verbatim).unwrap();
        enter_recording(&mut manager, context::ContextSnapshot::general());
        let claim = claim_stop_manager(&mut manager).unwrap();
        assert!(claim.skip_llm_cleanup);
        claim_cancel_manager(&mut manager, false);
        assert!(!manager.skip_llm_cleanup);
        lease = OperationLease::Idle;
        claim_start_manager_with_mode(&mut manager, &mut lease, RecordingMode::Dictation).unwrap();
        assert!(!manager.skip_llm_cleanup);
        assert!(claim.skip_llm_cleanup);
    }
}
