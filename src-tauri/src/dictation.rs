//! Dictation phase transitions and top-level start/stop/cancel orchestration.
//!
//! The application state remains the composition root in `lib.rs`. This module
//! owns only the state machine and the short-lived claims that protect the
//! blocking recorder and provider work performed by the surrounding pipeline.

use crate::prefetch_asr;
use crate::screen_action::{clear_screen_action, clear_screen_preview};
use crate::selected_action::{clear_selected_action, clear_selected_preview};
use crate::{
    audio, cancel_audio, cancel_prefetch_asr, chunker, context, emit_state, fail_for_generation,
    hotkey, island_window, lock_recover, release_operation, start_claimed,
    start_with_error_feedback, stop_claimed, sync_modifier_hotkey_phase, AppState, StartError,
};
use tauri::{Emitter, State};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Phase {
    Idle,
    Starting,
    Recording,
    Stopping,
    Processing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToggleAction {
    Start,
    Stop,
    Cancel,
    Ignore,
}

pub(crate) fn next_toggle_action(phase: Phase) -> ToggleAction {
    match phase {
        Phase::Idle => ToggleAction::Start,
        Phase::Starting | Phase::Processing => ToggleAction::Cancel,
        Phase::Recording => ToggleAction::Stop,
        Phase::Stopping => ToggleAction::Ignore,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperationLease {
    Idle,
    LiveDictation,
    HistoryReclean,
}

pub(crate) struct DictationManager {
    pub(crate) phase: Phase,
    pub(crate) started: std::time::Instant,
    pub(crate) gesture_lock: Option<std::time::Instant>,
    pub(crate) session_generation: u64,
    pub(crate) cancellation: CancellationToken,
    pub(crate) recording_context: Option<context::ContextSnapshot>,
    pub(crate) hybrid_press_at: Option<std::time::Instant>,
    pub(crate) hybrid_started_this_press: bool,
    pub(crate) hybrid_stop_when_recording: bool,
}

impl DictationManager {
    pub(crate) fn new() -> Self {
        Self {
            phase: Phase::Idle,
            started: std::time::Instant::now(),
            gesture_lock: None,
            session_generation: 0,
            cancellation: CancellationToken::new(),
            recording_context: None,
            hybrid_press_at: None,
            hybrid_started_this_press: false,
            hybrid_stop_when_recording: false,
        }
    }
}

/// Blocking recorder boundary. Production uses cpal through `audio::Recorder`;
/// tests can inject a deterministic recorder without constructing a Tauri app.
pub(crate) trait RecorderBackend: Send {
    fn start(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session: &str,
        input_device: &str,
        chunk_length_secs: usize,
        input_gain: f32,
        prefetch_tx: prefetch_asr::PrefetchInbox,
    ) -> Result<(), audio::AudioError>;

    fn stop_with_chunks(
        &mut self,
        app: Option<&tauri::AppHandle>,
    ) -> Result<(Vec<u8>, Vec<chunker::AudioChunk>), audio::AudioError>;

    fn cancel(&mut self, app: Option<&tauri::AppHandle>);
}

impl RecorderBackend for audio::Recorder {
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

    fn stop_with_chunks(
        &mut self,
        app: Option<&tauri::AppHandle>,
    ) -> Result<(Vec<u8>, Vec<chunker::AudioChunk>), audio::AudioError> {
        audio::Recorder::stop_with_chunks(self, app)
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
    pub(crate) cancellation: CancellationToken,
    pub(crate) recording_context: context::ContextSnapshot,
}

pub(crate) fn claim_start_manager(
    manager: &mut DictationManager,
    lease: &mut OperationLease,
) -> Option<u64> {
    if manager.phase != Phase::Idle || !claim_operation(lease, OperationLease::LiveDictation) {
        return None;
    }
    manager.phase = Phase::Starting;
    manager.session_generation = manager.session_generation.wrapping_add(1);
    manager.cancellation = CancellationToken::new();
    manager.recording_context = None;
    manager.hybrid_stop_when_recording = false;
    Some(manager.session_generation)
}

pub(crate) fn claim_start(state: &AppState) -> Option<u64> {
    // Keep the global order consistent everywhere: lease before manager.
    let mut lease = lock_recover(&state.operation_lease);
    let mut manager = lock_recover(&state.manager);
    claim_start_manager(&mut manager, &mut lease)
}

pub(crate) fn claim_stop_manager(manager: &mut DictationManager) -> Option<StopClaim> {
    if manager.phase != Phase::Recording {
        return None;
    }
    manager.session_generation = manager.session_generation.wrapping_add(1);
    let claim = StopClaim {
        started: manager.started,
        session_generation: manager.session_generation,
        cancellation: manager.cancellation.clone(),
        recording_context: manager
            .recording_context
            .take()
            .unwrap_or_else(context::ContextSnapshot::general),
    };
    manager.phase = Phase::Stopping;
    manager.hybrid_stop_when_recording = false;
    Some(claim)
}

pub(crate) fn claim_stop(state: &AppState) -> Option<StopClaim> {
    let mut manager = lock_recover(&state.manager);
    claim_stop_manager(&mut manager)
}

/// Hybrid PTT release: stop now if recording, otherwise remember to stop once
/// `Starting` becomes `Recording`.
pub(crate) fn request_hybrid_stop(manager: &mut DictationManager) -> Option<StopClaim> {
    if let Some(claim) = claim_stop_manager(manager) {
        return Some(claim);
    }
    if manager.phase == Phase::Starting {
        manager.hybrid_stop_when_recording = true;
    }
    None
}

/// Commit Starting → Recording. If a hybrid PTT release arrived during
/// Starting, take the stop claim immediately.
pub(crate) fn enter_recording(
    manager: &mut DictationManager,
    recording_context: context::ContextSnapshot,
) -> Option<StopClaim> {
    manager.started = std::time::Instant::now();
    manager.phase = Phase::Recording;
    manager.cancellation = CancellationToken::new();
    manager.recording_context = Some(recording_context);
    if !manager.hybrid_stop_when_recording {
        return None;
    }
    manager.hybrid_stop_when_recording = false;
    claim_stop_manager(manager)
}

pub(crate) async fn claim_start_entry(state: &AppState) -> Option<u64> {
    let _gate = state.hotkey_gate.lock().await;
    claim_start(state)
}

pub(crate) async fn claim_stop_entry(state: &AppState) -> Option<StopClaim> {
    let _gate = state.hotkey_gate.lock().await;
    claim_stop(state)
}

pub(crate) fn reset_starting_manager(manager: &mut DictationManager) -> u64 {
    if manager.phase == Phase::Starting {
        manager.cancellation.cancel();
        manager.phase = Phase::Idle;
        manager.session_generation = manager.session_generation.wrapping_add(1);
        manager.recording_context = None;
        manager.hybrid_stop_when_recording = false;
    }
    manager.session_generation
}

pub(crate) const GESTURE_LOCK_MS: u128 = 400;

pub(crate) fn take_gesture_lock(manager: &mut DictationManager) -> bool {
    let now = std::time::Instant::now();
    if manager
        .gesture_lock
        .map(|time| now.duration_since(time).as_millis() < GESTURE_LOCK_MS)
        .unwrap_or(false)
    {
        return false;
    }
    manager.gesture_lock = Some(now);
    true
}

pub(crate) fn mark_hybrid_press(manager: &mut DictationManager, started_this_press: bool) {
    manager.hybrid_press_at = Some(std::time::Instant::now());
    manager.hybrid_started_this_press = started_this_press;
}

pub(crate) fn clear_hybrid_press(manager: &mut DictationManager) {
    manager.hybrid_press_at = None;
    manager.hybrid_started_this_press = false;
}

pub(crate) fn take_hybrid_release(
    manager: &mut DictationManager,
) -> crate::hotkey::HybridReleaseAction {
    let Some(started_at) = manager.hybrid_press_at.take() else {
        return crate::hotkey::HybridReleaseAction::Ignore;
    };
    let started_this_press = std::mem::replace(&mut manager.hybrid_started_this_press, false);
    let elapsed_ms = started_at.elapsed().as_millis() as u64;
    crate::hotkey::hybrid_release_action(elapsed_ms, started_this_press)
}

pub(crate) struct CancelClaim {
    pub(crate) phase: Phase,
    pub(crate) generation: u64,
    pub(crate) had_preview: bool,
}

pub(crate) fn claim_cancel_manager(manager: &mut DictationManager, had_preview: bool) -> Phase {
    let phase = manager.phase;
    if phase != Phase::Idle || had_preview {
        manager.cancellation.cancel();
        manager.phase = Phase::Idle;
        manager.session_generation = manager.session_generation.wrapping_add(1);
        manager.recording_context = None;
        manager.hybrid_stop_when_recording = false;
    }
    phase
}

fn claim_cancel(state: &AppState) -> CancelClaim {
    clear_selected_action(state);
    clear_screen_action(state);
    let had_preview = state
        .selected_preview
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_some();
    clear_selected_preview(state);
    clear_screen_preview(state);
    let mut manager = lock_recover(&state.manager);
    let phase = claim_cancel_manager(&mut manager, had_preview);
    CancelClaim {
        phase,
        generation: manager.session_generation,
        had_preview,
    }
}

async fn claim_cancel_entry(state: &AppState) -> CancelClaim {
    let _gate = state.hotkey_gate.lock().await;
    claim_cancel(state)
}

async fn finish_cancel_claim(app: &tauri::AppHandle, state: &AppState, release_live_lease: bool) {
    let _gate = state.hotkey_gate.lock().await;
    if release_live_lease {
        release_operation(state, OperationLease::LiveDictation);
    }
    sync_modifier_hotkey_phase(Phase::Idle);
    hotkey::unregister_cancel(app);
    emit_state(app, "idle");
    island_window::hide_overlay(app);
}

async fn execute_cancel_claim(app: &tauri::AppHandle, state: &AppState, claim: CancelClaim) {
    state.gate.set_session_generation(claim.generation);
    match claim.phase {
        Phase::Starting => finish_cancel_claim(app, state, false).await,
        Phase::Recording => {
            cancel_prefetch_asr(state);
            cancel_audio(state, app.clone()).await;
            finish_cancel_claim(app, state, true).await;
        }
        Phase::Stopping => {
            cancel_prefetch_asr(state);
            finish_cancel_claim(app, state, false).await;
        }
        Phase::Processing => finish_cancel_claim(app, state, true).await,
        Phase::Idle if claim.had_preview => finish_cancel_claim(app, state, true).await,
        Phase::Idle => {}
    }
}

pub(crate) async fn cancel_internal(app: &tauri::AppHandle, state: &AppState) {
    let claim = claim_cancel_entry(state).await;
    execute_cancel_claim(app, state, claim).await;
}

pub(crate) async fn start_internal(
    app: &tauri::AppHandle,
    state: &AppState,
) -> Result<(), StartError> {
    let Some(session_generation) = claim_start_entry(state).await else {
        let _ = app.emit(
            "dictation://error",
            "正在处理上一次结果，请稍候".to_string(),
        );
        return Ok(());
    };
    start_claimed(app, state, session_generation).await
}

pub(crate) async fn stop_internal(app: &tauri::AppHandle, state: &AppState) -> Result<(), String> {
    let Some(claim) = claim_stop_entry(state).await else {
        return Ok(());
    };
    stop_claimed(app, state, claim).await
}

enum EntryClaim {
    Start(u64),
    Stop(StopClaim),
    Cancel(CancelClaim),
    Ignore,
}

async fn claim_toggle_entry(
    state: &AppState,
    confirmed: bool,
    check_suspended: bool,
) -> EntryClaim {
    let _gate = state.hotkey_gate.lock().await;
    if !confirmed || (check_suspended && hotkey::is_suspended()) {
        return EntryClaim::Ignore;
    }
    let action = {
        let mut manager = lock_recover(&state.manager);
        let action = next_toggle_action(manager.phase);
        if action != ToggleAction::Cancel && !take_gesture_lock(&mut manager) {
            return EntryClaim::Ignore;
        }
        action
    };
    match action {
        ToggleAction::Start => claim_start(state)
            .map(EntryClaim::Start)
            .unwrap_or(EntryClaim::Ignore),
        ToggleAction::Stop => claim_stop(state)
            .map(EntryClaim::Stop)
            .unwrap_or(EntryClaim::Ignore),
        ToggleAction::Cancel => EntryClaim::Cancel(claim_cancel(state)),
        ToggleAction::Ignore => EntryClaim::Ignore,
    }
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

pub(crate) async fn handle_hotkey_toggle(app: &tauri::AppHandle, state: &AppState) {
    let claim = claim_toggle_entry(state, true, true).await;
    execute_entry_claim(app, state, claim).await;
}

pub(crate) async fn handle_hotkey_press(app: &tauri::AppHandle, state: &AppState) {
    let claim = claim_hybrid_press_entry(state).await;
    execute_entry_claim(app, state, claim).await;
}

pub(crate) async fn handle_hotkey_release(app: &tauri::AppHandle, state: &AppState) {
    let claim = {
        let _gate = state.hotkey_gate.lock().await;
        let mut manager = lock_recover(&state.manager);
        if hotkey::is_suspended() {
            clear_hybrid_press(&mut manager);
            return;
        }
        if !matches!(
            take_hybrid_release(&mut manager),
            crate::hotkey::HybridReleaseAction::Stop
        ) {
            return;
        }
        request_hybrid_stop(&mut manager)
    };
    let Some(claim) = claim else {
        return;
    };
    let _ = stop_claimed(app, state, claim).await;
}

async fn claim_hybrid_press_entry(state: &AppState) -> EntryClaim {
    let _gate = state.hotkey_gate.lock().await;
    if hotkey::is_suspended() {
        let mut manager = lock_recover(&state.manager);
        clear_hybrid_press(&mut manager);
        return EntryClaim::Ignore;
    }
    let action = {
        let mut manager = lock_recover(&state.manager);
        let action = next_toggle_action(manager.phase);
        if action != ToggleAction::Cancel && !take_gesture_lock(&mut manager) {
            return EntryClaim::Ignore;
        }
        mark_hybrid_press(&mut manager, action == ToggleAction::Start);
        action
    };
    match action {
        ToggleAction::Start => claim_start(state)
            .map(EntryClaim::Start)
            .unwrap_or(EntryClaim::Ignore),
        ToggleAction::Stop => claim_stop(state)
            .map(EntryClaim::Stop)
            .unwrap_or(EntryClaim::Ignore),
        ToggleAction::Cancel => EntryClaim::Cancel(claim_cancel(state)),
        ToggleAction::Ignore => EntryClaim::Ignore,
    }
}

pub(crate) async fn handle_double_tap_toggle(
    app: &tauri::AppHandle,
    state: &AppState,
    confirmed: bool,
) {
    let claim = claim_toggle_entry(state, confirmed, true).await;
    execute_entry_claim(app, state, claim).await;
}

#[tauri::command]
pub(crate) async fn start_dictation(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    start_with_error_feedback(&app, &state).await
}

#[tauri::command]
pub(crate) async fn stop_dictation(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    stop_internal(&app, &state).await
}

#[tauri::command]
pub(crate) async fn cancel_dictation(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    cancel_internal(&app, &state).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::{AsrProvider, MockAsrProvider, RateLimits, Transcript};
    use std::time::Duration;

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
        ) -> Result<(Vec<u8>, Vec<chunker::AudioChunk>), audio::AudioError> {
            self.stops += 1;
            Ok((self.stop_audio.clone(), Vec::new()))
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
                .stop_with_chunks(None)
                .expect("mock recorder stop")
                .0
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
                self.manager.phase = Phase::Idle;
                release_operation_lease(&mut self.lease, OperationLease::LiveDictation);
            }
        }
    }

    #[test]
    fn hybrid_stop_during_starting_sets_pending_flag() {
        let mut manager = DictationManager::new();
        manager.phase = Phase::Starting;
        manager.session_generation = 1;

        assert!(request_hybrid_stop(&mut manager).is_none());
        assert!(manager.hybrid_stop_when_recording);
        assert_eq!(manager.phase, Phase::Starting);
    }

    #[test]
    fn entering_recording_with_pending_hybrid_stop_claims_stop() {
        let mut manager = DictationManager::new();
        manager.phase = Phase::Starting;
        manager.session_generation = 1;
        manager.hybrid_stop_when_recording = true;

        let claim = enter_recording(&mut manager, context::ContextSnapshot::general());

        assert!(claim.is_some());
        assert_eq!(manager.phase, Phase::Stopping);
        assert!(!manager.hybrid_stop_when_recording);
    }

    #[test]
    fn hybrid_stop_while_recording_claims_immediately() {
        let mut manager = DictationManager::new();
        manager.phase = Phase::Recording;
        manager.session_generation = 1;

        let claim = request_hybrid_stop(&mut manager);

        assert!(claim.is_some());
        assert_eq!(manager.phase, Phase::Stopping);
        assert!(!manager.hybrid_stop_when_recording);
    }

    #[test]
    fn hybrid_hold_release_stops_and_short_tap_keeps_recording() {
        let mut manager = DictationManager::new();
        manager.hybrid_press_at = Some(std::time::Instant::now() - Duration::from_millis(400));
        manager.hybrid_started_this_press = true;
        assert_eq!(
            take_hybrid_release(&mut manager),
            crate::hotkey::HybridReleaseAction::Stop
        );
        assert!(manager.hybrid_press_at.is_none());
        assert!(!manager.hybrid_started_this_press);

        manager.hybrid_press_at = Some(std::time::Instant::now() - Duration::from_millis(80));
        manager.hybrid_started_this_press = true;
        assert_eq!(
            take_hybrid_release(&mut manager),
            crate::hotkey::HybridReleaseAction::KeepRecording
        );
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
                segments: Vec::new(),
                words: Vec::new(),
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
