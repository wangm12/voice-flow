//! Explicit fixture-only instrumentation. Normal launches have no lease or sink.
//! Events are facts supplied by their existing production owners, never probes.
//! Compiled only for tests and macOS `research-harness` builds; every other
//! build uses the inert `research_trace_inert.rs` with the same signatures.
use crate::dictation;
use crate::learning_research::LearningStage;
use crate::{lock_recover, AppState};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use std::time::Instant;
use tauri::Manager;

pub(crate) const ACTIVE: bool = true;
const CAPACITY: usize = 256;
#[cfg(all(feature = "research-harness", target_os = "macos"))]
const MAX_COMMAND_BYTES: usize = 8192;
const TRANSPORT_SECONDS: u64 = 1200;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AllowedCapture {
    /// Explicit authority for this fixture's real microphone recording only.
    pub fixture_microphone: bool,
    /// Audio identity is an opt-in hash, never audio bytes in this sink.
    pub audio_identity: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResearchRunLease {
    pub run_id: String,
    pub case_id: String,
    pub job_id: String,
    pub ttl_ms: u64,
    pub fixture_target_pid: i32,
    pub allowed_capture: AllowedCapture,
    #[serde(default)]
    pub learning: Option<crate::learning_research::LearningConsent>,
}
impl ResearchRunLease {
    fn validate(&self) -> Result<(), &'static str> {
        for id in [&self.run_id, &self.case_id, &self.job_id] {
            crate::onboarding::validate_id(id).map_err(|_| "invalid_uuid")?;
        }
        if !(1000..=TRANSPORT_SECONDS * 1000).contains(&self.ttl_ms) {
            return Err("invalid_expiry");
        }
        if self.fixture_target_pid <= 0 || !self.allowed_capture.fixture_microphone {
            return Err("fixture_authority_required");
        }
        if let Some(consent) = &self.learning {
            consent.validate()?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(crate) enum Fact {
    SessionStarted {
        trigger_source: &'static str,
    },
    InputFirstAccepted {
        samples: usize,
        source: &'static str,
    },
    StopRequested {
        trigger_source: &'static str,
    },
    CaptureClosed {
        samples: usize,
        complete: bool,
        audio_sha256: Option<String>,
    },
    CaptureDevice {
        requested_selection: Option<String>,
        clamshell_substituted: bool,
        system_default_name: Option<String>,
        effective_device_name: Option<String>,
    },
    AsrAttempt {
        requested_provider_label: String,
        requested_model_label: String,
        requested_source: &'static str,
        path: String,
        attempt: u64,
        effective_model: Option<String>,
    },
    WinnerSelected {
        origin: String,
    },
    CleanupResolved {
        stage: String,
        reason: Option<String>,
    },
    DeliveryTemporaryWrite {
        boundary: &'static str,
        outcome: &'static str,
    },
    DeliveryKeyboardMayPost,
    DeliveryTargetReadback {
        verified: Option<bool>,
    },
    DeliveryClipboardResolved {
        resolution: &'static str,
        known: bool,
    },
    CancellationRequested {
        reason: &'static str,
    },
    LearningRecord {
        record: Box<crate::learning_research::LearningRecordV1>,
    },
    SessionTerminal {
        outcome: &'static str,
        side_effect_boundary: &'static str,
        provider_termination: &'static str,
    },
}
#[derive(Serialize)]
struct Event {
    sequence: String,
    monotonic_ns: String,
    run_id: String,
    case_id: String,
    job_id: String,
    recording_generation: String,
    processing_generation: Option<String>,
    config_generation: String,
    #[serde(flatten)]
    fact: Fact,
}

/// Clipboard write association, never content equality or exclusive OS ownership.
/// Only the native restore owner supplies this from its existing snapshot and
/// actual clear/write return; this struct cannot authorize any clipboard IO.
#[derive(Clone, Serialize)]
pub(crate) struct FinalClipboardAssociation {
    pub snapshot_fixture_identity: String,
    pub snapshot_generation: String,
    pub temporary_generation: String,
    pub final_generation: String,
    pub source: &'static str,
}
#[derive(Serialize)]
struct FinalClipboardBoundary {
    artifact_type: &'static str,
    schema_version: u32,
    run_id: String,
    case_id: String,
    job_id: String,
    config_generation: String,
    recording_generation: String,
    processing_generation: Option<String>,
    fixture_target_pid: i32,
    observed_uptime_ns: String,
    expires_uptime_ns: String,
    gate: &'static str,
    clipboard_association: Option<FinalClipboardAssociation>,
    coverage: &'static str,
    independent_paste_success: Option<bool>,
    provider_termination: &'static str,
}

struct Inner {
    lease: ResearchRunLease,
    expires: Instant,
    expires_monotonic_ns: u64,
    config_generation: u64,
    recording_generation: AtomicU64,
    processing_generation: AtomicU64,
    processing_bound: AtomicBool,
    emit_gate: Mutex<()>,
    sequence: AtomicU64,
    attempts: AtomicU64,
    dropped: AtomicU64,
    revoked: AtomicBool,
    started: AtomicBool,
    terminal: AtomicBool,
    pipeline_closed: AtomicBool,
    learning_tasks: AtomicUsize,
    learning_gate: Mutex<()>,
    terminal_emitted: AtomicBool,
    capture_closed: AtomicBool,
    capture_seen: AtomicBool,
    clipboard_known: AtomicBool,
    clipboard_touched: AtomicBool,
    final_clipboard_association: Mutex<Option<FinalClipboardAssociation>>,
    keyboard_may_post: AtomicBool,
    writers: AtomicUsize,
    outcome: Mutex<&'static str>,
    audio_session: Mutex<Option<String>>,
    tx: mpsc::SyncSender<Event>,
    rx: Mutex<mpsc::Receiver<Event>>,
}
#[derive(Clone)]
pub(crate) struct Trace(Arc<Inner>);
#[derive(Default)]
pub(crate) struct Owner {
    current: Option<Trace>,
    #[cfg(all(feature = "research-harness", target_os = "macos"))]
    used_run_ids: std::collections::HashSet<String>,
}

impl Trace {
    fn new(lease: ResearchRunLease, config_generation: u64) -> Self {
        let (tx, rx) = mpsc::sync_channel(CAPACITY);
        Self(Arc::new(Inner {
            expires: Instant::now() + Duration::from_millis(lease.ttl_ms),
            expires_monotonic_ns: monotonic_ns().saturating_add(lease.ttl_ms * 1_000_000),
            lease,
            config_generation,
            recording_generation: AtomicU64::new(0),
            processing_generation: AtomicU64::new(0),
            processing_bound: AtomicBool::new(false),
            emit_gate: Mutex::new(()),
            sequence: AtomicU64::new(0),
            attempts: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            revoked: AtomicBool::new(false),
            started: AtomicBool::new(false),
            terminal: AtomicBool::new(false),
            pipeline_closed: AtomicBool::new(false),
            learning_tasks: AtomicUsize::new(0),
            learning_gate: Mutex::new(()),
            terminal_emitted: AtomicBool::new(false),
            capture_closed: AtomicBool::new(false),
            capture_seen: AtomicBool::new(false),
            clipboard_known: AtomicBool::new(true),
            clipboard_touched: AtomicBool::new(false),
            final_clipboard_association: Mutex::new(None),
            keyboard_may_post: AtomicBool::new(false),
            writers: AtomicUsize::new(0),
            outcome: Mutex::new("failed"),
            audio_session: Mutex::new(None),
            tx,
            rx: Mutex::new(rx),
        }))
    }
    pub(crate) fn allows(&self) -> bool {
        !self.0.revoked.load(Ordering::Acquire)
            && Instant::now() < self.0.expires
            && !self.0.terminal.load(Ordering::Acquire)
    }
    pub(crate) fn emit(&self, fact: Fact) {
        let Ok(_gate) = self.0.emit_gate.try_lock() else {
            self.0.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let processing = self.processing_snapshot();
        let event = Event {
            sequence: self.0.sequence.fetch_add(1, Ordering::Relaxed).to_string(),
            monotonic_ns: monotonic_ns().to_string(),
            run_id: self.0.lease.run_id.clone(),
            case_id: self.0.lease.case_id.clone(),
            job_id: self.0.lease.job_id.clone(),
            recording_generation: self
                .0
                .recording_generation
                .load(Ordering::Acquire)
                .to_string(),
            processing_generation: processing.map(|generation| generation.to_string()),
            config_generation: self.0.config_generation.to_string(),
            fact,
        };
        if self.0.tx.try_send(event).is_err() {
            self.0.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub(crate) fn bind(&self, recording_generation: u64) {
        self.0
            .recording_generation
            .store(recording_generation, Ordering::Release);
        self.0.started.store(true, Ordering::Release);
        self.emit(Fact::SessionStarted {
            trigger_source: "explicit_research_stdio",
        });
    }
    pub(crate) fn stop(&self, processing_generation: u64) {
        self.0
            .processing_generation
            .store(processing_generation, Ordering::Release);
        self.0.processing_bound.store(true, Ordering::Release);
        self.emit(Fact::StopRequested {
            trigger_source: "owner_claim_stop_action_unknown",
        });
    }
    pub(crate) fn learning_consent(&self) -> Option<&crate::learning_research::LearningConsent> {
        self.0.lease.learning.as_ref()
    }
    fn settings_match(&self, settings: &crate::store::Settings) -> bool {
        self.learning_consent().map_or_else(
            || fixture_settings(settings),
            |consent| consent.settings_match(settings),
        )
    }
    pub(crate) fn target_is_current(&self, state: &AppState) -> bool {
        self.allows()
            && self.0.config_generation
                == state
                    .processing_configuration_generation
                    .load(Ordering::Acquire)
            && self.settings_match(&lock_recover(&state.settings))
            && crate::context::frontmost_application_key().0 == self.0.lease.fixture_target_pid
    }
    pub(crate) fn bound_target_matches(&self, pid: i32) -> bool {
        pid == self.0.lease.fixture_target_pid
    }
    fn processing_snapshot(&self) -> Option<u64> {
        self.0
            .processing_bound
            .load(Ordering::Acquire)
            .then(|| self.0.processing_generation.load(Ordering::Acquire))
    }
    pub(crate) fn bind_audio(&self, session: &str) {
        *lock_recover(&self.0.audio_session) = Some(session.into());
    }
    pub(crate) fn first_input(&self, samples: usize) {
        if !self.0.capture_seen.swap(true, Ordering::AcqRel) {
            self.emit(Fact::InputFirstAccepted {
                samples,
                source: "capture_worker_16khz_samples",
            });
        }
    }
    pub(crate) fn capture_closed(&self, samples: &[f32], complete: bool) {
        use sha2::{Digest, Sha256};
        let audio_sha256 = self.0.lease.allowed_capture.audio_identity.then(|| {
            let mut hash = Sha256::new();
            hash.update(b"voiceflow-research-capture-f32le-v1\0");
            hash.update((samples.len() as u64).to_le_bytes());
            for sample in samples {
                hash.update(sample.to_bits().to_le_bytes());
            }
            hex::encode(hash.finalize())
        });
        self.0.capture_closed.store(true, Ordering::Release);
        self.emit(Fact::CaptureClosed {
            samples: samples.len(),
            complete,
            audio_sha256,
        });
    }
    pub(crate) fn capture_is_closed(&self) -> bool {
        self.0.capture_closed.load(Ordering::Acquire)
    }
    pub(crate) fn completion(&self, phase: &str) {
        *lock_recover(&self.0.outcome) = if phase == "error" {
            "failed"
        } else {
            "succeeded"
        };
    }
    pub(crate) fn revoke(&self, reason: &'static str) {
        if !self.0.revoked.swap(true, Ordering::AcqRel) {
            self.emit(Fact::CancellationRequested { reason });
        }
    }
    /// Called only after the actual start/stop/cancel owner future returns. This
    /// is local quiescence, not proof that a remote provider stopped billing.
    pub(crate) fn terminal(&self) {
        self.0.pipeline_closed.store(true, Ordering::Release);
        self.finish_if_quiescent();
    }
    fn finish_if_quiescent(&self) {
        let _gate = lock_recover(&self.0.learning_gate);
        if !self.0.pipeline_closed.load(Ordering::Acquire)
            || self.0.learning_tasks.load(Ordering::Acquire) != 0
        {
            return;
        }
        if self.0.terminal.swap(true, Ordering::AcqRel) {
            return;
        }
        let outcome = if self.0.revoked.load(Ordering::Acquire) {
            "cancelled"
        } else {
            *lock_recover(&self.0.outcome)
        };
        self.emit(Fact::SessionTerminal {
            outcome,
            side_effect_boundary: if self.0.keyboard_may_post.load(Ordering::Acquire) {
                "keyboard_may_have_posted"
            } else if self.0.clipboard_touched.load(Ordering::Acquire) {
                "clipboard_write_attempted"
            } else {
                "no_delivery_write_observed"
            },
            provider_termination: "unknown",
        });
        self.0.terminal_emitted.store(true, Ordering::Release);
    }
    pub(crate) fn begin_clipboard_write(&self) {
        *lock_recover(&self.0.final_clipboard_association) = None;
        self.0.clipboard_touched.store(true, Ordering::Release);
        self.0.clipboard_known.store(false, Ordering::Release);
        self.emit(Fact::DeliveryTemporaryWrite {
            boundary: "before_native_write",
            outcome: "unknown_until_owner_resolution",
        });
    }
    pub(crate) fn keyboard(&self) {
        self.0.keyboard_may_post.store(true, Ordering::Release);
        self.emit(Fact::DeliveryKeyboardMayPost);
    }
    pub(crate) fn clipboard_resolved(&self, resolution: &'static str, known: bool) {
        if !known || resolution != "restored" {
            *lock_recover(&self.0.final_clipboard_association) = None;
        }
        self.0.clipboard_known.store(known, Ordering::Release);
        self.emit(Fact::DeliveryClipboardResolved { resolution, known });
    }
    pub(crate) fn clipboard_restored_fixture(&self, association: FinalClipboardAssociation) {
        // Called on the existing delivery writer, before its lease is released.
        // A later temporary write clears this association rather than rebinding.
        *lock_recover(&self.0.final_clipboard_association) = Some(association);
    }
    fn final_clipboard_boundary(&self, observed: u64) -> Option<FinalClipboardBoundary> {
        // Reuse the actual lifecycle owner gate. No timer is terminal evidence;
        // expiry only denies a stale boundary. No clipboard query happens here.
        if observed == 0
            || observed >= self.0.expires_monotonic_ns
            || !self.0.started.load(Ordering::Acquire)
            || !self.final_clipboard_read_allowed()
        {
            return None;
        }
        let association = lock_recover(&self.0.final_clipboard_association).clone();
        if !self.final_clipboard_read_allowed() {
            return None;
        }
        Some(FinalClipboardBoundary {
            artifact_type: "voiceflow_final_clipboard_boundary",
            schema_version: 1,
            run_id: self.0.lease.run_id.clone(),
            case_id: self.0.lease.case_id.clone(),
            job_id: self.0.lease.job_id.clone(),
            config_generation: self.0.config_generation.to_string(),
            recording_generation: self
                .0
                .recording_generation
                .load(Ordering::Acquire)
                .to_string(),
            processing_generation: self.processing_snapshot().map(|value| value.to_string()),
            fixture_target_pid: self.0.lease.fixture_target_pid,
            observed_uptime_ns: observed.to_string(),
            expires_uptime_ns: self.0.expires_monotonic_ns.to_string(),
            gate: "research_owner_final_clipboard_read_allowed_v1",
            clipboard_association: association,
            coverage: "owner_hooks_only",
            independent_paste_success: None,
            provider_termination: "unknown",
        })
    }
    fn locally_releasable(&self) -> bool {
        self.0.terminal_emitted.load(Ordering::Acquire)
            && self.0.writers.load(Ordering::Acquire) == 0
            && self.0.learning_tasks.load(Ordering::Acquire) == 0
            && (lock_recover(&self.0.audio_session).is_none() || self.capture_is_closed())
    }
    fn final_clipboard_read_allowed(&self) -> bool {
        self.0.terminal_emitted.load(Ordering::Acquire)
            && self.0.capture_closed.load(Ordering::Acquire)
            && self.0.writers.load(Ordering::Acquire) == 0
            && self.0.learning_tasks.load(Ordering::Acquire) == 0
            && self.0.clipboard_known.load(Ordering::Acquire)
            && self.0.dropped.load(Ordering::Acquire) == 0
    }
}

tokio::task_local! { static ASYNC_TRACE: Option<Trace>; }
thread_local! { static SYNC_TRACE: RefCell<Option<Trace>> = const { RefCell::new(None) }; }
pub(crate) fn current() -> Option<Trace> {
    match ASYNC_TRACE.try_with(Clone::clone) {
        Ok(trace) => trace,
        Err(_) => SYNC_TRACE.with(|slot| slot.borrow().clone()),
    }
}
pub(crate) async fn scope<F: Future>(trace: Option<Trace>, future: F) -> F::Output {
    ASYNC_TRACE.scope(trace, future).await
}
pub(crate) fn sync_scope<T>(trace: Option<Trace>, operation: impl FnOnce() -> T) -> T {
    struct Restore(Option<Trace>);
    impl Drop for Restore {
        fn drop(&mut self) {
            SYNC_TRACE.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let previous = SYNC_TRACE.with(|slot| slot.replace(trace));
    let _restore = Restore(previous);
    operation()
}
pub(crate) fn spawn_blocking<F, R>(operation: F) -> tokio::task::JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let trace = current();
    tokio::task::spawn_blocking(move || sync_scope(trace, operation))
}
pub(crate) fn tokio_spawn<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(scope(current(), future))
}
pub(crate) fn allowed() -> bool {
    current().is_none_or(|trace| trace.allows())
}
pub(crate) fn emit(fact: Fact) {
    if let Some(trace) = current() {
        trace.emit(fact);
    }
}
pub(crate) fn capture_device_fact(
    requested_selection: Option<String>,
    clamshell_substituted: bool,
    system_default_name: Option<String>,
    effective_device_name: Option<String>,
) -> Fact {
    Fact::CaptureDevice {
        requested_selection,
        clamshell_substituted,
        system_default_name,
        effective_device_name,
    }
}
pub(crate) fn retain_operation(state: &AppState) -> bool {
    cfg!(feature = "research-harness")
        && lock_recover(&state.research_trace)
            .current
            .as_ref()
            .is_some_and(|trace| trace.0.started.load(Ordering::Acquire))
}
pub(crate) fn invalidate(state: &AppState, reason: &'static str) {
    if let Some(trace) = lock_recover(&state.research_trace).current.as_ref() {
        trace.revoke(reason);
    }
}
pub(crate) fn audio_trace(app: &tauri::AppHandle, session: &str) -> Option<Trace> {
    let state = app.state::<AppState>();
    let trace = lock_recover(&state.manager).research_trace.clone()?;
    let matches = lock_recover(&trace.0.audio_session).as_deref() == Some(session);
    matches.then_some(trace)
}
pub(crate) struct DeliveryWriter(Option<Trace>);
impl Drop for DeliveryWriter {
    fn drop(&mut self) {
        if let Some(trace) = &self.0 {
            trace.0.writers.fetch_sub(1, Ordering::AcqRel);
        }
    }
}
pub(crate) fn delivery_writer() -> DeliveryWriter {
    let trace = current();
    if let Some(trace) = &trace {
        trace.0.writers.fetch_add(1, Ordering::AcqRel);
        *lock_recover(&trace.0.final_clipboard_association) = None;
    }
    DeliveryWriter(trace)
}
/// Captured before scheduling; descendants acquire before their parent releases.
/// A run cannot end or admit a final clipboard read while these owners remain.
pub(crate) struct LearningTask(Option<Trace>);
impl Drop for LearningTask {
    fn drop(&mut self) {
        if let Some(trace) = &self.0 {
            trace.0.learning_tasks.fetch_sub(1, Ordering::AcqRel);
            trace.finish_if_quiescent();
        }
    }
}
pub(crate) fn learning_task() -> LearningTask {
    let trace = current().filter(|trace| trace.learning_consent().is_some() && trace.allows());
    if let Some(trace) = &trace {
        let _gate = lock_recover(&trace.0.learning_gate);
        trace.0.learning_tasks.fetch_add(1, Ordering::AcqRel);
    }
    LearningTask(trace)
}
pub(crate) fn learning_spawn<F>(future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    let task = learning_task();
    let trace = current();
    tauri::async_runtime::spawn(scope(trace, async move {
        let _task = task;
        future.await;
    }));
}
pub(crate) fn asr_attempt(trace: Option<&Trace>, provider: &str, model: &str, path: &str) {
    if let Some(trace) = trace {
        trace.emit(Fact::AsrAttempt {
            requested_provider_label: provider.into(),
            requested_model_label: model.into(),
            requested_source: "bounded_metrics_group_at_actual_attempt",
            path: path.into(),
            attempt: trace.0.attempts.fetch_add(1, Ordering::Relaxed) + 1,
            effective_model: None,
        });
    }
}

#[cfg(target_os = "macos")]
fn monotonic_ns() -> u64 {
    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_absolute_time() -> u64;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }
    let mut info = Timebase { numer: 0, denom: 0 };
    // Same host clock as Swift DispatchTime.uptimeNanoseconds; no wall-clock inference.
    unsafe {
        if mach_timebase_info(&mut info) != 0 || info.denom == 0 {
            return 0;
        }
        ((mach_absolute_time() as u128 * info.numer as u128) / info.denom as u128)
            .min(u64::MAX as u128) as u64
    }
}
#[cfg(not(target_os = "macos"))]
fn monotonic_ns() -> u64 {
    0
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Prepare {
        lease: Box<ResearchRunLease>,
    },
    Quit,
    Start {
        run_id: String,
    },
    Stop {
        run_id: String,
    },
    Drain {
        run_id: String,
    },
    End {
        run_id: String,
    },
    Cancel {
        run_id: String,
    },
    LearningAction {
        run_id: String,
        fixture_id: String,
        action: LearningAction,
    },
}
#[cfg(all(feature = "research-harness", target_os = "macos"))]
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LearningAction {
    Undo,
    Delete,
}
#[cfg(all(feature = "research-harness", target_os = "macos"))]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    request_id: String,
    command: Command,
}
fn fixture_settings(settings: &crate::store::Settings) -> bool {
    settings.dictionary.is_empty()
        && settings.context_mappings.is_empty()
        && settings.snippets.is_empty()
        && !settings.context_enabled
        && !settings.browser_access_enabled
        && !settings.window_ocr_enabled
        && !settings.dictionary_learn_enabled
        && !settings.always_on_microphone
}
#[cfg(all(feature = "research-harness", target_os = "macos"))]
fn get_run(state: &AppState, run_id: &str) -> Result<Trace, &'static str> {
    crate::onboarding::validate_id(run_id).map_err(|_| "invalid_uuid")?;
    lock_recover(&state.research_trace)
        .current
        .as_ref()
        .filter(|trace| trace.0.lease.run_id == run_id)
        .cloned()
        .ok_or("run_mismatch")
}
#[cfg(all(feature = "research-harness", target_os = "macos"))]
async fn cancel_owned(app: &tauri::AppHandle, trace: &Trace, reason: &'static str) {
    trace.revoke(reason);
    let state = app.state::<AppState>();
    let _gate = state.hotkey_gate.lock().await;
    let owned = lock_recover(&state.manager)
        .research_trace
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(&current.0, &trace.0));
    if !owned {
        return;
    }
    let phase = lock_recover(&state.manager).phase;
    // cancel_internal owns the gate itself; keep identity and claim atomic here.
    let claim = dictation::claim_cancel(&state);
    drop(_gate);
    dictation::execute_cancel_claim(app, &state, claim).await;
    if phase == dictation::Phase::Recording && trace.0.capture_closed.load(Ordering::Acquire) {
        trace.terminal();
    }
}
#[cfg(all(feature = "research-harness", target_os = "macos"))]
async fn command(
    app: &tauri::AppHandle,
    command: Command,
) -> Result<serde_json::Value, &'static str> {
    let state = app.state::<AppState>();
    match command {
        Command::Quit => {
            if lock_recover(&state.research_trace).current.is_some()
                || lock_recover(&state.manager).phase != dictation::Phase::Idle
            {
                return Err("end_owned_run_before_quit");
            }
            app.exit(0);
            Ok(serde_json::json!({"quit_requested": true}))
        }
        Command::Prepare { lease } => {
            let lease = *lease;
            lease.validate()?;
            let _gate = state.hotkey_gate.lock().await;
            if lock_recover(&state.manager).phase != dictation::Phase::Idle {
                return Err("busy");
            }
            if crate::onboarding::snapshot(&state, "research_stdio").is_some() {
                return Err("onboarding_lease_active");
            }
            if !lease.learning.as_ref().map_or_else(
                || fixture_settings(&lock_recover(&state.settings)),
                |consent| consent.settings_match(&lock_recover(&state.settings)),
            ) {
                return Err("fixture_settings_required");
            }
            let selected_learning_state = if let Some(consent) = &lease.learning {
                let dir = app
                    .path()
                    .app_data_dir()
                    .map_err(|_| "fixture_store_unavailable")?;
                Some(
                    crate::learning_research::selected_state_snapshot(
                        consent,
                        &dir,
                        &lock_recover(&state.settings),
                    )
                    .map_err(|_| "selected_state_identity_unavailable")?,
                )
            } else {
                None
            };
            let mut owner = lock_recover(&state.research_trace);
            if owner.current.is_some() {
                return Err("end_previous_run_first");
            }
            if !owner.used_run_ids.insert(lease.run_id.clone()) {
                return Err("run_id_reused");
            }
            let trace = Trace::new(
                lease,
                state
                    .processing_configuration_generation
                    .load(Ordering::Acquire),
            );
            owner.current = Some(trace.clone());
            let expiry_app = app.clone();
            let expiry_trace = trace.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep_until(tokio::time::Instant::from_std(expiry_trace.0.expires))
                    .await;
                cancel_owned(&expiry_app, &expiry_trace, "expired").await;
            });
            Ok(
                serde_json::json!({"lease": trace.0.lease, "selected_learning_state": selected_learning_state, "config_generation": trace.0.config_generation.to_string(), "expires_monotonic_ns": trace.0.expires_monotonic_ns.to_string()}),
            )
        }
        Command::Start { run_id } => {
            let trace = get_run(&state, &run_id)?;
            let generation = {
                let _gate = state.hotkey_gate.lock().await;
                if !trace.allows() || trace.0.started.load(Ordering::Acquire) {
                    return Err("lease_not_startable");
                }
                if trace.0.config_generation
                    != state
                        .processing_configuration_generation
                        .load(Ordering::Acquire)
                    || !trace.settings_match(&lock_recover(&state.settings))
                {
                    return Err("configuration_changed");
                }
                if crate::context::frontmost_application_key().0 != trace.0.lease.fixture_target_pid
                {
                    return Err("fixture_not_frontmost");
                }
                if crate::onboarding::snapshot(&state, "research_stdio").is_some() {
                    return Err("onboarding_lease_active");
                }
                let generation = dictation::claim_start(&state).ok_or("busy")?;
                let mut manager = lock_recover(&state.manager);
                if manager.onboarding_trial.is_some() {
                    let settlement = manager.delivery_settlement.clone();
                    dictation::reset_starting_manager(&mut manager);
                    drop(manager);
                    if let Some(settlement) = settlement {
                        state.delivery_settlements.record_terminal(settlement);
                    }
                    crate::release_operation(&state, dictation::OperationLease::LiveDictation);
                    return Err("onboarding_lease_active");
                }
                trace.bind(generation);
                manager.research_trace = Some(trace.clone());
                generation
            };
            let worker = app.clone();
            tauri::async_runtime::spawn(async move {
                let state = worker.state::<AppState>();
                if let Err(error) = crate::start_claimed(&worker, &state, generation).await {
                    crate::fail_for_generation(&worker, &state, error.message, error.generation)
                        .await;
                }
            });
            Ok(
                serde_json::json!({"recording_generation": generation.to_string(), "accepted": true}),
            )
        }
        Command::Stop { run_id } => {
            let trace = get_run(&state, &run_id)?;
            let claim = {
                let _gate = state.hotkey_gate.lock().await;
                let mut manager = lock_recover(&state.manager);
                if !manager
                    .research_trace
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(&active.0, &trace.0))
                {
                    return Err("session_mismatch");
                }
                dictation::request_stop(&mut manager)
            };
            if let Some(claim) = claim {
                let worker = app.clone();
                tauri::async_runtime::spawn(async move {
                    let state = worker.state::<AppState>();
                    let _ = crate::stop_claimed(&worker, &state, claim).await;
                });
            }
            Ok(serde_json::json!({"stop_requested": true}))
        }
        Command::Drain { run_id } => {
            let trace = get_run(&state, &run_id)?;
            let events: Vec<_> = lock_recover(&trace.0.rx)
                .try_iter()
                .take(CAPACITY)
                .collect();
            let recording_ready = {
                let manager = lock_recover(&state.manager);
                manager.phase == dictation::Phase::Recording
                    && manager
                        .research_trace
                        .as_ref()
                        .is_some_and(|active| Arc::ptr_eq(&active.0, &trace.0))
            };
            Ok(
                serde_json::json!({"run_id": trace.0.lease.run_id, "case_id": trace.0.lease.case_id, "job_id": trace.0.lease.job_id,
                "config_generation": trace.0.config_generation.to_string(),
                "recording_generation": trace.0.recording_generation.load(Ordering::Acquire).to_string(),
                "processing_generation": trace.processing_snapshot().map(|generation| generation.to_string()),
                "recording_ready": recording_ready, "capture_closed": trace.0.capture_closed.load(Ordering::Acquire), "delivery_writers": trace.0.writers.load(Ordering::Acquire),
                "learning_tasks": trace.0.learning_tasks.load(Ordering::Acquire), "pipeline_closed": trace.0.pipeline_closed.load(Ordering::Acquire),
                "clipboard_settled": trace.0.clipboard_known.load(Ordering::Acquire), "events": events, "dropped": trace.0.dropped.load(Ordering::Acquire).to_string(),
                "coverage": if trace.0.dropped.load(Ordering::Acquire) == 0 { "owner_hooks_only" } else { "partial_dropped" },
                "local_terminal": trace.0.terminal.load(Ordering::Acquire), "final_clipboard_read_allowed": trace.final_clipboard_read_allowed(),
                "independent_paste_success": null, "provider_termination": "unknown",
                "final_clipboard_boundary": trace.final_clipboard_boundary(monotonic_ns())}),
            )
        }
        Command::LearningAction {
            run_id,
            fixture_id,
            action,
        } => {
            let trace = get_run(&state, &run_id)?;
            if trace.0.started.load(Ordering::Acquire) || !trace.allows() {
                return Err("learning_action_requires_prepared_unstarted_run");
            }
            {
                let _gate = state.hotkey_gate.lock().await;
                if lock_recover(&state.manager).phase != dictation::Phase::Idle
                    || !trace.settings_match(&lock_recover(&state.settings))
                    || !crate::try_claim_operation(&state, dictation::OperationLease::LiveDictation)
                {
                    return Err("busy_or_fixture_configuration_changed");
                }
            }
            struct ActionOwner<'a>(&'a AppState);
            impl Drop for ActionOwner<'_> {
                fn drop(&mut self) {
                    crate::release_operation(self.0, dictation::OperationLease::LiveDictation);
                }
            }
            let _operation = ActionOwner(&state);
            let consent = trace
                .learning_consent()
                .ok_or("learning_consent_required")?;
            let term = consent
                .terms
                .iter()
                .find(|term| term.fixture_id == fixture_id)
                .ok_or("term_fixture_mismatch")?;
            let key = crate::dictionary_learn::pair_key(&term.before, &term.after);
            let dir = app
                .path()
                .app_data_dir()
                .map_err(|_| "fixture_store_unavailable")?;
            let before = crate::store::get_learn_pair(&dir, &key)
                .map_err(|_| "selected_pair_read_failed")?
                .ok_or("selected_pair_absent")?;
            if before.before_surface != term.before || before.after_surface != term.after {
                return Err("selected_pair_surface_mismatch");
            }
            let action_name = match action {
                LearningAction::Undo => "undo",
                LearningAction::Delete => "delete",
            };
            // Both toast undo and Dictionary "forget" use this existing owner.
            let action_completed =
                crate::dictionary_learn::undo_learn_pair(app.clone(), key.clone())
                    .await
                    .is_ok();
            let after = crate::store::get_learn_pair(&dir, &key)
                .map_err(|_| "selected_pair_read_failed")?;
            let mut record = crate::learning_research::LearningRecordV1::new(
                consent,
                LearningStage::UndoDelete,
                if action_completed {
                    "undo_learn_pair_command_completed"
                } else {
                    "undo_learn_pair_command_failed"
                },
            );
            record.term_fixture_id = Some(fixture_id);
            record.hits_before = Some(before.hits);
            record.hits_after = after.as_ref().map(|row| row.hits);
            record.promoted = after.as_ref().map(|row| row.is_live_promoted());
            record.undo_delete = Some(action_name);
            record.persistence = if action_completed {
                "selected_pair_readback_command_completed"
            } else {
                "selected_pair_readback_settings_unknown"
            };
            // Prepared action receipts are not fabricated recording events. This
            // synchronous owner has no learning descendant and closes its lease.
            let result = serde_json::json!({"action": action_name, "action_completed": action_completed, "selected_state_id": consent.selected_state_id,
                "pair_tombstoned": after.is_some_and(|row| row.tombstoned_at.is_some()),
                "dictionary_word_retained": lock_recover(&state.settings).dictionary.iter().any(|word| word == &term.after),
                "record": record, "ended": true, "recording_session": "not_started", "final_clipboard_read_allowed": false});
            lock_recover(&state.research_trace).current = None;
            Ok(result)
        }
        Command::Cancel { run_id } => {
            let trace = get_run(&state, &run_id)?;
            cancel_owned(app, &trace, "explicit_cancel").await;
            Ok(serde_json::json!({"cancel_requested": true}))
        }
        Command::End { run_id } => {
            let trace = get_run(&state, &run_id)?;
            cancel_owned(app, &trace, "explicit_end").await;
            if trace.0.started.load(Ordering::Acquire) && !trace.locally_releasable() {
                return Err("owner_not_terminal_drain_and_retry");
            }
            lock_recover(&state.research_trace).current = None;
            if trace.0.started.load(Ordering::Acquire) {
                crate::release_operation(&state, dictation::OperationLease::LiveDictation);
            }
            Ok(serde_json::json!({"ended": true}))
        }
    }
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
fn write_response(value: &serde_json::Value) -> bool {
    let Ok(mut bytes) = serde_json::to_vec(value) else {
        return false;
    };
    bytes.push(b'\n');
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut written = 0;
    while written < bytes.len() && Instant::now() < deadline {
        let count = unsafe {
            libc::write(
                libc::STDOUT_FILENO,
                bytes[written..].as_ptr().cast(),
                bytes.len() - written,
            )
        };
        if count > 0 {
            written += count as usize;
            continue;
        }
        if count == 0 {
            return false;
        }
        let error = std::io::Error::last_os_error().raw_os_error();
        if !matches!(error, Some(libc::EAGAIN) | Some(libc::EINTR)) {
            return false;
        }
        let mut descriptor = libc::pollfd {
            fd: libc::STDOUT_FILENO,
            events: libc::POLLOUT,
            revents: 0,
        };
        if unsafe { libc::poll(&mut descriptor, 1, 10) } < 0 {
            return false;
        }
    }
    written == bytes.len()
}

/// Explicit owned-launch transport; no socket/server, env configuration, or
/// product file writes. The launcher must retain process identity and stdout.
#[cfg(all(feature = "research-harness", target_os = "macos"))]
pub(crate) fn start_stdio(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let started = Instant::now();
        let mut line = Vec::new();
        let mut count = 0;
        let mut request_ids = std::collections::HashSet::new();
        let flags = unsafe { libc::fcntl(libc::STDOUT_FILENO, libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(libc::STDOUT_FILENO, libc::F_SETFL, flags | libc::O_NONBLOCK) }
                < 0
        {
            app.exit(2);
            return;
        }
        let hello = serde_json::json!({"protocol": "voiceflow_research_stdio_v1", "pid": std::process::id(),
            "clock": "mach_absolute_time_nanoseconds_same_host", "capacity": CAPACITY,
            "executable_identity": "launcher_must_verify_owned_binary", "feature": "research-harness"});
        if !write_response(&hello) {
            app.exit(2);
            return;
        }
        while started.elapsed() < Duration::from_secs(TRANSPORT_SECONDS) && count < 256 {
            let mut descriptor = libc::pollfd {
                fd: libc::STDIN_FILENO,
                events: libc::POLLIN,
                revents: 0,
            };
            // Poll each byte, including partial lines; an unclosed line cannot
            // retain the lease beyond the transport's finite deadline.
            let ready = unsafe { libc::poll(&mut descriptor, 1, 100) };
            if ready < 0 {
                break;
            } else if ready == 0 {
                continue;
            }
            let mut byte = [0_u8; 1];
            if unsafe { libc::read(libc::STDIN_FILENO, byte.as_mut_ptr().cast(), 1) } != 1 {
                break;
            }
            if byte[0] != b'\n' {
                line.push(byte[0]);
                if line.len() > MAX_COMMAND_BYTES {
                    break;
                }
                continue;
            }
            count += 1;
            let result = serde_json::from_slice::<Request>(&line)
                .map_err(|_| "invalid_command")
                .and_then(|request| {
                    crate::onboarding::validate_id(&request.request_id)
                        .map_err(|_| "invalid_request_uuid")?;
                    if !request_ids.insert(request.request_id.clone()) {
                        return Err("request_id_reused");
                    }
                    Ok(request)
                });
            line.clear();
            let response = match result {
                Ok(request) => match tauri::async_runtime::block_on(command(&app, request.command))
                {
                    Ok(value) => {
                        serde_json::json!({"request_id": request.request_id, "ok": true, "result": value})
                    }
                    Err(reason) => {
                        serde_json::json!({"request_id": request.request_id, "ok": false, "reason": reason})
                    }
                },
                Err(reason) => serde_json::json!({"ok": false, "reason": reason}),
            };
            if !write_response(&response) {
                break;
            }
        }
        let trace = lock_recover(&app.state::<AppState>().research_trace)
            .current
            .clone();
        if let Some(trace) = trace {
            tauri::async_runtime::block_on(cancel_owned(&app, &trace, "transport_closed"));
        }
        // This function exists only in this explicitly owned feature launch.
        // Use the existing bounded application exit/cancellation owner.
        app.exit(0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_device_fact_serializes_only_device_selection_metadata() {
        let fact = capture_device_fact(
            Some("built-in-microphone".to_owned()),
            true,
            Some("MacBook Pro Microphone".to_owned()),
            Some("USB Microphone".to_owned()),
        );
        let value = serde_json::to_value(fact).expect("capture device fact serializes");
        assert_eq!(value["event"], "capture_device");
        assert_eq!(value["requested_selection"], "built-in-microphone");
        assert_eq!(value["clamshell_substituted"], true);
        assert_eq!(value["system_default_name"], "MacBook Pro Microphone");
        assert_eq!(value["effective_device_name"], "USB Microphone");
        assert!(value.get("audio").is_none());
    }

    fn lease() -> ResearchRunLease {
        ResearchRunLease {
            run_id: "10000000-0000-4000-8000-000000000001".into(),
            case_id: "10000000-0000-4000-8000-000000000002".into(),
            job_id: "10000000-0000-4000-8000-000000000003".into(),
            ttl_ms: 1000,
            fixture_target_pid: 1,
            allowed_capture: AllowedCapture {
                fixture_microphone: true,
                audio_identity: false,
            },
            learning: None,
        }
    }
    #[test]
    fn strict_authority_before_start() {
        let mut fixture = lease();
        assert!(fixture.validate().is_ok());
        fixture.ttl_ms = 999;
        assert_eq!(fixture.validate(), Err("invalid_expiry"));
        fixture.ttl_ms = 1000;
        fixture.allowed_capture.fixture_microphone = false;
        assert_eq!(fixture.validate(), Err("fixture_authority_required"));
        #[cfg(all(feature = "research-harness", target_os = "macos"))]
        {
            assert!(
                serde_json::from_str::<Command>(r#"{"op":"drain","run_id":"x","extra":1}"#)
                    .is_err()
            );
            let valid_request =
                r#"{"request_id":"10000000-0000-4000-8000-000000000004","command":{"op":"quit"}}"#;
            assert!(serde_json::from_str::<Request>(valid_request).is_ok());
            let request_with_extra = r#"{"request_id":"10000000-0000-4000-8000-000000000004","command":{"op":"quit"},"extra":1}"#;
            assert!(serde_json::from_str::<Request>(request_with_extra).is_err());
        }
    }
    #[test]
    fn dropped_or_unsettled_owner_never_authorizes_clipboard_read() {
        let trace = Trace::new(lease(), 7);
        trace.bind(u64::MAX);
        trace.capture_closed(&[], false);
        trace.terminal();
        assert!(trace.final_clipboard_read_allowed());
        trace.begin_clipboard_write();
        assert!(!trace.final_clipboard_read_allowed());
        trace.clipboard_resolved("ownership_changed", true);
        assert!(trace.final_clipboard_read_allowed());
        for _ in 0..CAPACITY {
            trace.emit(Fact::StopRequested {
                trigger_source: "fixture",
            });
        }
        assert!(trace.0.dropped.load(Ordering::Acquire) > 0);
        assert!(!trace.final_clipboard_read_allowed());
        let event = lock_recover(&trace.0.rx).try_recv().unwrap();
        assert_eq!(event.recording_generation, u64::MAX.to_string());
    }
    #[tokio::test]
    async fn revoked_a_scope_cannot_become_b_or_inherit_ordinary_scope() {
        let a = Trace::new(lease(), 1);
        let b = Trace::new(lease(), 2);
        scope(Some(a.clone()), async {
            a.revoke("explicit_cancel");
            assert!(!allowed());
            scope(Some(b.clone()), async {
                assert!(allowed());
            })
            .await;
            assert!(!allowed());
        })
        .await;
        assert!(current().is_none());
    }
}

#[cfg(test)]
mod production_claim_fixtures {
    use super::*;
    fn fixture() -> Trace {
        Trace::new(
            ResearchRunLease {
                run_id: "20000000-0000-4000-8000-000000000001".into(),
                case_id: "20000000-0000-4000-8000-000000000002".into(),
                job_id: "20000000-0000-4000-8000-000000000003".into(),
                ttl_ms: 1000,
                fixture_target_pid: 1,
                allowed_capture: AllowedCapture {
                    fixture_microphone: true,
                    audio_identity: false,
                },
                learning: None,
            },
            42,
        )
    }
    #[tokio::test]
    async fn real_stop_claim_keeps_a_when_ordinary_b_claim_clears_manager_route() {
        let a = fixture();
        let mut manager = dictation::DictationManager::new();
        let mut lease = dictation::OperationLease::Idle;
        let recording = dictation::claim_start_manager(&mut manager, &mut lease).unwrap();
        a.bind(recording);
        manager.research_trace = Some(a.clone());
        manager.phase = dictation::Phase::Recording;
        let a_settlement = manager
            .delivery_settlement
            .clone()
            .expect("ordinary recording owns its settlement");
        let stop = dictation::claim_stop_manager(&mut manager).unwrap();
        assert!(std::sync::Arc::ptr_eq(
            stop.delivery_settlement
                .as_ref()
                .expect("stop retains recording A's settlement"),
            &a_settlement
        ));
        a.revoke("fixture_a_cancelled");
        assert!(!a_settlement.snapshot().writer_admission_sealed);
        assert_eq!(
            dictation::claim_cancel_manager(&mut manager, false),
            dictation::Phase::Stopping
        );
        let cancelled_a = a_settlement.snapshot();
        assert!(cancelled_a.writer_admission_sealed);
        assert_eq!(cancelled_a.admitted_writers, 0);
        assert!(cancelled_a.writers_closed);
        assert_eq!(
            cancelled_a
                .terminal_outcome
                .as_ref()
                .map(|outcome| outcome.kind),
            Some(crate::delivery_settlement::DeliveryOutcomeKind::Cancelled)
        );
        assert_eq!(manager.phase, dictation::Phase::Idle);
        dictation::release_operation_lease(&mut lease, dictation::OperationLease::LiveDictation);
        assert!(a_settlement.snapshot().writers_closed);
        let b = dictation::claim_start_manager(&mut manager, &mut lease).unwrap();
        assert!(b > recording);
        assert!(manager.research_trace.is_none());
        assert_eq!(stop.recording_generation, recording);
        assert_eq!(stop.session_generation, recording + 1);
        scope(stop.research_trace, async {
            assert!(!allowed());
            emit(Fact::CleanupResolved {
                stage: "late_a_rejected".into(),
                reason: None,
            });
        })
        .await;
        let events: Vec<_> = lock_recover(&a.0.rx).try_iter().collect();
        assert!(events
            .iter()
            .all(|event| event.recording_generation == recording.to_string()));
        assert_eq!(
            events.last().unwrap().processing_generation,
            Some((recording + 1).to_string())
        );
    }
    #[tokio::test]
    async fn terminal_cannot_admit_read_while_delivery_is_still_queued() {
        let a = fixture();
        a.bind(1);
        a.capture_closed(&[], false);
        scope(Some(a.clone()), async {
            let queued_writer = delivery_writer();
            a.terminal();
            assert!(!a.final_clipboard_read_allowed());
            drop(queued_writer);
            assert!(a.final_clipboard_read_allowed());
        })
        .await;
    }
}

#[cfg(test)]
mod learning_owner_fixtures {
    use super::*;
    use crate::dictionary_learn::{self, ObserveLimits, ObserveOutcome};
    use crate::learning_research::{LearningConsent, LearningPurpose, TermFixture};
    fn consent(before: &str, after: &str) -> LearningConsent {
        LearningConsent {
            selected_state_id: "30000000-0000-4000-8000-000000000001".into(),
            baseline_run: None,
            edit_fixture_id: "30000000-0000-4000-8000-000000000002".into(),
            purpose: LearningPurpose::Correction,
            mapping_id: "controlled-learning".into(),
            bundle_id: "com.example.controlled-learning".into(),
            observer_enabled: true,
            post_insert_field: before.into(),
            settled_field: after.into(),
            terms: vec![TermFixture {
                fixture_id: "30000000-0000-4000-8000-000000000003".into(),
                before: before.into(),
                after: after.into(),
            }],
        }
    }
    fn trace(consent: LearningConsent) -> Trace {
        Trace::new(
            ResearchRunLease {
                run_id: "30000000-0000-4000-8000-000000000004".into(),
                case_id: "30000000-0000-4000-8000-000000000005".into(),
                job_id: "30000000-0000-4000-8000-000000000006".into(),
                ttl_ms: 60_000,
                fixture_target_pid: 1,
                allowed_capture: AllowedCapture {
                    fixture_microphone: true,
                    audio_identity: false,
                },
                learning: Some(consent),
            },
            7,
        )
    }
    /// MAX may retain these real production-serde bytes for the independent
    /// Swift/schema consumer. This is a serializer fixture, not live learning.
    #[test]
    fn production_learning_wire_records_keep_typed_stages_and_explicit_nulls() {
        let selected = consent("cobolt", "cobalt");
        println!(
            "\nVF12_RECORD_CONSENT\t{}",
            serde_json::to_string(&selected).unwrap()
        );
        let a = trace(selected);
        a.bind(1);
        let _started = lock_recover(&a.0.rx).try_recv().unwrap();
        for stage in [
            LearningStage::SelectedState,
            LearningStage::SelectedPairRead,
            LearningStage::ObserverRegistration,
            LearningStage::Observation,
            LearningStage::Classification,
            LearningStage::CandidateStore,
            LearningStage::StyleCandidateStore,
            LearningStage::StoreGuard,
            LearningStage::Persistence,
            LearningStage::SettingsPersistence,
            LearningStage::StyleStore,
            LearningStage::LocalPrepareUsage,
            LearningStage::UndoDelete,
        ] {
            sync_scope(Some(a.clone()), || {
                crate::learning_research::stage(
                    stage,
                    if stage == LearningStage::Persistence {
                        "stale_or_cancelled"
                    } else {
                        "serializer_fixture_not_live_observation"
                    },
                )
            });
            let event = lock_recover(&a.0.rx).try_recv().unwrap();
            let Fact::LearningRecord { record } = event.fact else {
                panic!("expected real learning emitter")
            };
            assert_eq!(record.stage, stage);
            let value = serde_json::to_value(&record).unwrap();
            assert_eq!(value.as_object().unwrap().len(), 23);
            for key in [
                "baseline_run",
                "term_fixture_id",
                "candidate_class",
                "persistence_version",
                "undo_delete",
                "eligible",
                "promoted",
                "hint_used",
                "replacement_used",
                "hits_before",
                "hits_after",
                "promote_threshold",
            ] {
                assert_eq!(value.get(key), Some(&serde_json::Value::Null));
            }
            // Extract bytes after this exact TAB prefix; never reconstruct the
            // record by hand. Keep complete command stdout/source identity too.
            println!(
                "VF12_RECORD_WIRE\t{}",
                serde_json::to_string(&record).unwrap()
            );
        }
    }
    fn target() -> crate::context::TargetAppGuard {
        crate::context::TargetAppGuard {
            pid: 1,
            bundle_id: Some("com.example.controlled-learning".into()),
            browser_host: None,
            browser_target_token: None,
            browser_tab_token: None,
            window_token: Some(1),
            window_id: Some(1),
            window_identity_ambiguous: false,
            input_token: Some(1),
            secure_input: false,
        }
    }
    struct OwnedDir(std::path::PathBuf);
    impl OwnedDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let name = format!(
                "voiceflow-vf12-{}-{}-{}",
                std::process::id(),
                monotonic_ns(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let path = std::env::temp_dir().join(name);
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for OwnedDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn actual_observer_store_replacement_and_receipts_correction_matrix() {
        // This is controlled local replay, not GUI, provider, capture or accuracy evidence.
        for (corrections, before, after) in [
            (0, "cobolt", "cobalt"),
            (1, "ambr", "amber"),
            (2, "quortz", "quartz"),
            (3, "obsidion", "obsidian"),
            (5, "vermilin", "vermilion"),
        ] {
            let dir = OwnedDir::new();
            let a = trace(consent(before, after));
            let mut dictionary = Vec::new();
            a.bind(1);
            sync_scope(Some(a.clone()), || {
                for _ in 0..corrections {
                    let expected = target();
                    let outcome = dictionary_learn::observe_after_paste(
                        before,
                        &expected,
                        true,
                        ObserveLimits::default(),
                        || Some(after.into()),
                        || expected.clone(),
                        |_| {},
                    );
                    let ObserveOutcome::SingleToken { before_span, after } = outcome else {
                        panic!("fixture did not reach actual lexeme observer")
                    };
                    dictionary_learn::record_test_proof_pair(
                        &dir.0,
                        &mut dictionary,
                        &before_span,
                        &after,
                        None,
                    )
                    .unwrap();
                }
                let selected = crate::store::LearnPairScope::default()
                    .applicable_promoted_records(&crate::learning_research::selected_pairs(&dir.0));
                let follow_up = format!("new sentence {before} today");
                let (text, usage) = crate::lexicon::apply_promoted_replacements_with_usage(
                    &follow_up,
                    &selected,
                    &dictionary,
                );
                crate::learning_research::usage(&usage, false);
                assert_eq!(text.contains(after), corrections >= 3);
                assert_eq!(dictionary.contains(&after.to_string()), corrections >= 3);
                if corrections >= 3 {
                    let key = dictionary_learn::pair_key(before, after);
                    assert!(crate::store::tombstone_learn_pair(&dir.0, &key).unwrap());
                    let after_undo = crate::store::LearnPairScope::default()
                        .applicable_promoted_records(&crate::learning_research::selected_pairs(
                            &dir.0,
                        ));
                    let (after_undo_text, after_undo_usage) =
                        crate::lexicon::apply_promoted_replacements_with_usage(
                            &follow_up,
                            &after_undo,
                            &dictionary,
                        );
                    assert_eq!(after_undo_text, follow_up);
                    assert!(after_undo_usage.is_empty());
                }
                let row = crate::store::get_learn_pair(
                    &dir.0,
                    &dictionary_learn::pair_key(before, after),
                )
                .unwrap();
                assert_eq!(
                    row.map(|row| row.hits),
                    if corrections == 0 {
                        None
                    } else {
                        // Promotion threshold controls activation, not the count of later independent origins.
                        Some(corrections)
                    }
                );
            });
            let events: Vec<_> = lock_recover(&a.0.rx).try_iter().collect();
            assert!(events.iter().all(|event| event.run_id == a.0.lease.run_id));
            assert!(events.iter().any(|event| matches!(&event.fact, Fact::LearningRecord { record } if record.stage == LearningStage::LocalPrepareUsage && record.replacement_used == Some(corrections >= 3))));
        }
    }
    #[test]
    fn actual_name_threshold_and_tombstone_store_owner_remain_authoritative() {
        let dir = OwnedDir::new();
        let a = trace(consent("王小名", "王小明"));
        let mut dictionary = Vec::new();
        sync_scope(Some(a.clone()), || {
            for _ in 0..2 {
                dictionary_learn::record_test_proof_pair(
                    &dir.0,
                    &mut dictionary,
                    "王小名",
                    "王小明",
                    None,
                )
                .unwrap();
            }
            let key = dictionary_learn::pair_key("王小名", "王小明");
            assert!(dictionary.contains(&"王小明".into()));
            crate::store::tombstone_learn_pair(&dir.0, &key).unwrap();
            dictionary_learn::record_test_proof_pair(
                &dir.0,
                &mut dictionary,
                "王小名",
                "王小明",
                None,
            )
            .unwrap();
            let row = crate::store::get_learn_pair(&dir.0, &key).unwrap().unwrap();
            assert!(row.tombstoned_at.is_some());
            assert!(!row.is_live_promoted());
        });
        let events: Vec<_> = lock_recover(&a.0.rx).try_iter().collect();
        assert!(events.iter().any(|event| matches!(&event.fact,Fact::LearningRecord { record } if record.promote_threshold == Some(2) && record.hits_after == Some(2))));
        assert!(events.iter().any(|event| matches!(&event.fact,Fact::LearningRecord { record } if record.guard_effect == "tombstone_or_ignored_preserved")));
    }
    #[tokio::test]
    async fn actual_descendant_guards_close_a_before_b_and_never_admit_early_clipboard_read() {
        let a = trace(consent("cobolt", "cobalt"));
        a.bind(1);
        a.capture_closed(&[], false);
        scope(Some(a.clone()), async {
            let parent = learning_task();
            a.terminal();
            assert!(!a.locally_releasable());
            assert!(!a.final_clipboard_read_allowed());
            let descendant = learning_task();
            drop(parent);
            a.revoke("fixture_cancelled");
            assert!(!allowed());
            assert!(!a.locally_releasable());
            drop(descendant);
            assert!(a.locally_releasable());
        })
        .await;
        let events: Vec<_> = lock_recover(&a.0.rx).try_iter().collect();
        assert!(matches!(
            &events.last().unwrap().fact,
            Fact::SessionTerminal {
                outcome: "cancelled",
                ..
            }
        ));
        let b = trace(consent("ambr", "amber"));
        scope(Some(b.clone()), async {
            assert!(allowed());
        })
        .await;
        assert_eq!(b.0.sequence.load(Ordering::Acquire), 0);
    }
    #[test]
    fn final_boundary_requires_real_owner_quiescence_and_clears_on_late_writer() {
        let trace = trace(consent("cobolt", "cobalt"));
        trace.bind(1);
        sync_scope(Some(trace.clone()), || {
            let writer = delivery_writer();
            let learning = learning_task();
            trace.capture_closed(&[], true);
            trace.clipboard_restored_fixture(FinalClipboardAssociation {
                snapshot_fixture_identity: "a".repeat(64),
                snapshot_generation: "10".into(),
                temporary_generation: "11".into(),
                final_generation: "12".into(),
                source: "macos_restore_owner_clear_write_sampled_generation_v1",
            });
            trace.clipboard_resolved("restored", true);
            trace.terminal();
            assert!(!trace.0.terminal_emitted.load(Ordering::Acquire));
            assert!(trace.final_clipboard_boundary(1).is_none());
            drop(learning);
            assert!(trace.0.terminal_emitted.load(Ordering::Acquire));
            assert!(trace.final_clipboard_boundary(1).is_none());
            drop(writer);
            let boundary = trace
                .final_clipboard_boundary(1)
                .expect("actual owner terminal gate");
            assert!(boundary.clipboard_association.is_some());
            assert!(trace.final_clipboard_boundary(0).is_none());
            assert!(trace
                .final_clipboard_boundary(trace.0.expires_monotonic_ns)
                .is_none());

            let late_writer = delivery_writer();
            assert!(trace.final_clipboard_boundary(1).is_none());
            assert!(!allowed());
            drop(late_writer);
        });
        assert!(trace
            .final_clipboard_boundary(1)
            .unwrap()
            .clipboard_association
            .is_none());
        trace.0.dropped.store(1, Ordering::Release);
        assert!(trace.final_clipboard_boundary(1).is_none());
    }
}
