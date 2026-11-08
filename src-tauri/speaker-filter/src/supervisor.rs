//! Bounded, killable host-side supervision for a streaming audio helper.
//!
//! This module deliberately knows nothing about a filtering algorithm. A `StreamProtocol`
//! adapter encodes candidate-specific requests and decodes responses; this module retains the
//! complete raw timeline first, applies process/session/sequence checks, bounds IPC memory, and
//! makes one final audio selection. The current helper's adapter is provided below, but nothing
//! here enables or endorses its filtering recipe.
//!
//! The helper owns no host audio. All host pipe descriptors are nonblocking and only touched by
//! `HostSession::poll` / `push`; a separate watchdog thread owns `Child`, uses `try_wait`, and
//! kills the helper's process group when a deadline expires. The watchdog never reads or writes a
//! pipe. A child that blocks in inference, blocks on stdin/stdout, emits a slow partial frame, or
//! exits while a descendant holds an inherited pipe therefore cannot strand a host I/O thread.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::wav::MAX_AUDIO_SAMPLES;

pub const MAX_FINISH_BUDGET: Duration = Duration::from_millis(200);
pub const MAX_AUDIO_FRAME_SAMPLES: usize = 1024;
pub const MAX_AUDIO_FRAME_BYTES: usize = 1 + 8 * 4 + 2 + MAX_AUDIO_FRAME_SAMPLES * 4;
pub const MAX_RESPONSE_FRAME_BYTES: usize = MAX_AUDIO_FRAME_BYTES;
pub const MAX_PROFILE_FRAME_BYTES: usize = 512 * 1024 + 96;
const MAX_PROTOCOL_FRAME_BYTES: usize = 1_048_576;
const MAX_RESPONSE_BYTES_PER_PUMP: usize = 64 * 1024;
const MAX_RESPONSE_FRAMES_PER_PUMP: usize = 32;
const WATCHDOG_TICK: Duration = Duration::from_millis(2);
const DEFAULT_SHUTDOWN_RESERVE: Duration = Duration::from_millis(20);

/// Identity attached to every host request and helper response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamIdentity {
    pub session_id: u64,
    pub audio_epoch: u64,
}

impl StreamIdentity {
    fn is_valid(self) -> bool {
        self.session_id != 0 && self.audio_epoch != 0
    }
}

/// A response normalized by a candidate-specific `StreamProtocol` implementation.
#[derive(Debug, PartialEq)]
pub enum ChildResponse {
    Ready {
        identity: StreamIdentity,
        proof: Vec<u8>,
    },
    Output {
        identity: StreamIdentity,
        sequence: u64,
        source_start_sample: u64,
        samples: Vec<f32>,
    },
    Finished {
        identity: StreamIdentity,
        status: u8,
        input_samples: u64,
        output_samples: u64,
    },
    Failure {
        identity: Option<StreamIdentity>,
        code: u16,
    },
}

/// Candidate-specific framing and identity proof contract.
///
/// Methods return payload bytes without the common four-byte big-endian frame length prefix.
/// Implementations must keep request payloads bounded and must not include audio or profile data
/// in error strings. The supervisor checks every normalized output independently.
pub trait StreamProtocol: Send + Sync + 'static {
    fn begin_payload(&self, identity: StreamIdentity) -> Result<Vec<u8>, ProtocolError>;

    fn audio_payload(
        &self,
        identity: StreamIdentity,
        sequence: u64,
        source_start_sample: u64,
        samples: &[f32],
    ) -> Result<Vec<u8>, ProtocolError>;

    fn finish_payload(
        &self,
        identity: StreamIdentity,
        input_samples: u64,
        next_sequence: u64,
    ) -> Result<Vec<u8>, ProtocolError>;

    fn cancel_payload(&self, identity: StreamIdentity) -> Result<Vec<u8>, ProtocolError>;

    fn decode_response(&self, payload: &[u8]) -> Result<ChildResponse, ProtocolError>;

    /// Opaque model/profile/candidate identity returned in `Ready`.
    fn expected_ready_proof(&self) -> &[u8];

    /// Maximum intentional input/output sample lag during streaming. The finish phase always has
    /// its own absolute deadline and does not use this allowance.
    fn max_lookahead_samples(&self) -> usize {
        0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FallbackReason {
    None = 0,
    InvalidSessionIdentity = 1,
    SpawnFailed = 2,
    InvalidPipe = 3,
    InvalidBeginFrame = 4,
    InputQueueFull = 5,
    InputFrameTooLarge = 6,
    RawTimelineLimit = 7,
    InvalidRawAudio = 8,
    StartupTimeout = 9,
    InferenceTimeout = 10,
    FinishTimeout = 11,
    ChildExitTimeout = 12,
    ChildExited = 13,
    ChildFailure = 14,
    MalformedResponse = 15,
    ResponseTooLarge = 16,
    ResponseTruncated = 17,
    IdentityMismatch = 18,
    InvalidReadyProof = 19,
    InvalidOutput = 20,
    NonContiguousOutput = 21,
    FinishCountMismatch = 22,
    OutputQueueFull = 23,
    PipeIo = 24,
    Cancelled = 25,
    SupervisorBusy = 26,
    ChildClosureUnresolved = 27,
    ChildReapError = 28,
}

impl FallbackReason {
    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::InvalidSessionIdentity,
            2 => Self::SpawnFailed,
            3 => Self::InvalidPipe,
            4 => Self::InvalidBeginFrame,
            5 => Self::InputQueueFull,
            6 => Self::InputFrameTooLarge,
            7 => Self::RawTimelineLimit,
            8 => Self::InvalidRawAudio,
            9 => Self::StartupTimeout,
            10 => Self::InferenceTimeout,
            11 => Self::FinishTimeout,
            12 => Self::ChildExitTimeout,
            13 => Self::ChildExited,
            14 => Self::ChildFailure,
            15 => Self::MalformedResponse,
            16 => Self::ResponseTooLarge,
            17 => Self::ResponseTruncated,
            18 => Self::IdentityMismatch,
            19 => Self::InvalidReadyProof,
            20 => Self::InvalidOutput,
            21 => Self::NonContiguousOutput,
            22 => Self::FinishCountMismatch,
            23 => Self::OutputQueueFull,
            24 => Self::PipeIo,
            25 => Self::Cancelled,
            26 => Self::SupervisorBusy,
            27 => Self::ChildClosureUnresolved,
            28 => Self::ChildReapError,
            _ => Self::ChildExited,
        }
    }
}

/// Resource and deadline limits for one child/session. Defaults cap stop/drain at 200 ms.
#[derive(Debug, Clone)]
pub struct SupervisorLimits {
    pub max_session_samples: usize,
    pub max_queued_input_frames: usize,
    pub max_queued_input_bytes: usize,
    pub max_response_frame_bytes: usize,
    pub startup_timeout: Duration,
    pub inference_timeout: Duration,
    pub finish_timeout: Duration,
    pub shutdown_reserve: Duration,
    pub graceful_exit_timeout: Duration,
}

impl Default for SupervisorLimits {
    fn default() -> Self {
        Self {
            max_session_samples: MAX_AUDIO_SAMPLES,
            max_queued_input_frames: 8,
            max_queued_input_bytes: MAX_PROFILE_FRAME_BYTES + 8 * MAX_AUDIO_FRAME_BYTES,
            max_response_frame_bytes: MAX_RESPONSE_FRAME_BYTES,
            startup_timeout: Duration::from_secs(3),
            inference_timeout: Duration::from_secs(2),
            finish_timeout: MAX_FINISH_BUDGET,
            shutdown_reserve: DEFAULT_SHUTDOWN_RESERVE,
            graceful_exit_timeout: Duration::from_millis(40),
        }
    }
}

impl SupervisorLimits {
    fn normalized(mut self) -> Self {
        self.max_session_samples = self.max_session_samples.min(MAX_AUDIO_SAMPLES);
        self.max_queued_input_frames = self.max_queued_input_frames.clamp(1, 8);
        self.max_queued_input_bytes = self.max_queued_input_bytes.clamp(
            MAX_AUDIO_FRAME_BYTES,
            MAX_PROFILE_FRAME_BYTES + 8 * MAX_AUDIO_FRAME_BYTES,
        );
        self.max_response_frame_bytes = self
            .max_response_frame_bytes
            .clamp(1, MAX_PROTOCOL_FRAME_BYTES);
        self.finish_timeout = self.finish_timeout.min(MAX_FINISH_BUDGET);
        self.shutdown_reserve = self.shutdown_reserve.min(self.finish_timeout / 2);
        self.graceful_exit_timeout = self
            .graceful_exit_timeout
            .min(self.finish_timeout.saturating_sub(self.shutdown_reserve));
        self
    }
}

/// An audio timeline stored in bounded, separately allocated blocks. The default 15-minute cap
/// is 57.6 MB of samples per timeline. Block storage avoids geometric Vec growth at that scale.
#[derive(Debug, Default)]
pub struct AudioBuffer {
    blocks: Vec<AudioBlock>,
    sample_count: usize,
}

#[derive(Debug)]
struct AudioBlock {
    samples: Box<[f32; MAX_AUDIO_FRAME_SAMPLES]>,
    len: usize,
}

impl AudioBuffer {
    pub fn len(&self) -> usize {
        self.sample_count
    }

    pub fn is_empty(&self) -> bool {
        self.sample_count == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = &f32> {
        self.blocks
            .iter()
            .flat_map(|block| block.samples[..block.len].iter())
    }

    fn append(&mut self, samples: &[f32], max_samples: usize) -> Result<(), FallbackReason> {
        let new_count = self
            .sample_count
            .checked_add(samples.len())
            .filter(|count| *count <= max_samples)
            .ok_or(FallbackReason::RawTimelineLimit)?;
        let block_count = new_count.div_ceil(MAX_AUDIO_FRAME_SAMPLES);
        self.blocks
            .try_reserve(block_count.saturating_sub(self.blocks.len()))
            .map_err(|_| FallbackReason::RawTimelineLimit)?;

        let mut copied = 0;
        while copied < samples.len() {
            if self
                .blocks
                .last()
                .is_none_or(|block| block.len == MAX_AUDIO_FRAME_SAMPLES)
            {
                self.blocks.push(AudioBlock {
                    samples: Box::new([0.0; MAX_AUDIO_FRAME_SAMPLES]),
                    len: 0,
                });
            }
            let block = self.blocks.last_mut().expect("a writable block was added");
            let count = (MAX_AUDIO_FRAME_SAMPLES - block.len).min(samples.len() - copied);
            block.samples[block.len..block.len + count]
                .copy_from_slice(&samples[copied..copied + count]);
            block.len += count;
            copied += count;
        }
        self.sample_count = new_count;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    Filtered,
    RawFallback(FallbackReason),
}

/// The single timeline selected after `finish` consumes the session.
#[derive(Debug)]
pub struct AudioSelection {
    pub identity: StreamIdentity,
    pub kind: SelectionKind,
    pub samples: AudioBuffer,
    pub finish_elapsed: Duration,
    /// False means the helper received kill but the OS did not confirm its reap before deadline.
    pub child_reaped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureError {
    RawTimelineLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalizeError {
    EmptyAudio,
    RawTimelineIncomplete,
    InvalidRawAudio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionProgress {
    pub ready: bool,
    pub fallback: Option<FallbackReason>,
    pub captured_samples: usize,
    pub filtered_samples: usize,
    pub child_reaped: bool,
    pub child_closure_unresolved: bool,
}

/// Nonblocking child control seam. Implementations of `try_wait_nonblocking` must return
/// promptly; a permanent error is surfaced as unresolved closure and does not trigger a poll loop.
pub trait ChildProcessControl: Send + Sync + 'static {
    /// Must not block. A permanent error is treated as unresolved closure.
    fn try_wait_nonblocking(&self, child: &mut Child) -> io::Result<Option<ExitStatus>>;
    /// Must signal promptly; process-group and direct-child kill are expected to be best-effort.
    fn kill(&self, child: &mut Child, process_id: u32);
}

#[derive(Debug, Default)]
pub struct SystemChildProcessControl;

impl ChildProcessControl for SystemChildProcessControl {
    fn try_wait_nonblocking(&self, child: &mut Child) -> io::Result<Option<ExitStatus>> {
        child.try_wait()
    }

    fn kill(&self, child: &mut Child, process_id: u32) {
        kill_process_group(process_id);
        let _ = child.kill();
    }
}

/// A reusable factory for one isolated helper process per session.
#[derive(Clone)]
pub struct HostSupervisor {
    limits: SupervisorLimits,
    process_control: Arc<dyn ChildProcessControl>,
}

impl Default for HostSupervisor {
    fn default() -> Self {
        Self::new(SupervisorLimits::default())
    }
}

impl HostSupervisor {
    pub fn new(limits: SupervisorLimits) -> Self {
        Self {
            limits: limits.normalized(),
            process_control: Arc::new(SystemChildProcessControl),
        }
    }

    pub fn with_process_control(
        limits: SupervisorLimits,
        process_control: Arc<dyn ChildProcessControl>,
    ) -> Self {
        Self {
            limits: limits.normalized(),
            process_control,
        }
    }

    /// Start a fresh process/session without waiting for model load or inference. If spawning or
    /// Begin setup fails, the returned session still accepts and retains raw audio for fallback.
    pub fn start(
        &self,
        command: Command,
        identity: StreamIdentity,
        protocol: impl StreamProtocol,
    ) -> HostSession {
        HostSession::start(
            command,
            identity,
            Box::new(protocol),
            self.limits.clone(),
            Arc::clone(&self.process_control),
        )
    }
}

/// Host-side raw capture, helper input, filtered timeline, watchdog, and one-shot finalization.
pub struct HostSession {
    identity: StreamIdentity,
    protocol: Box<dyn StreamProtocol>,
    limits: SupervisorLimits,
    origin: Instant,
    control: Arc<WatchdogControl>,
    watchdog: Option<JoinHandle<()>>,
    process_id: Option<u32>,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    raw: AudioBuffer,
    filtered: AudioBuffer,
    outbound: VecDeque<PendingWrite>,
    outbound_bytes: usize,
    next_input_sequence: u64,
    next_output_sequence: u64,
    ready: bool,
    finished: bool,
    finishing: bool,
    finish_command_sent: bool,
    raw_complete: bool,
    raw_valid: bool,
    response_buffer: Vec<u8>,
    finish_deadline_ns: u64,
}

struct PendingWrite {
    frame: Vec<u8>,
    offset: usize,
    kind: OutboundKind,
}

impl PendingWrite {
    fn write_once(&mut self, writer: &mut impl Write) -> io::Result<bool> {
        let written = writer.write(&self.frame[self.offset..])?;
        if written == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        self.offset += written;
        Ok(self.offset == self.frame.len())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutboundKind {
    Begin,
    Audio,
    Finish,
    Cancel,
}

struct WatchdogControl {
    deadline_ns: AtomicU64,
    deadline_reason: AtomicU8,
    fallback_reason: AtomicU8,
    stop_requested: AtomicBool,
    graceful_expected: AtomicBool,
    reaped: AtomicBool,
    exit_success: AtomicBool,
    reap_unresolved: AtomicBool,
    origin: Instant,
}

struct ActiveChildLease {
    control: Arc<WatchdogControl>,
    child_slot: Option<ChildSlot>,
    process_id: Option<u32>,
    process_control: Arc<dyn ChildProcessControl>,
}

type ChildSlot = Arc<Mutex<Option<Child>>>;
type ChildRegistration = Option<(ChildSlot, u32)>;

static ACTIVE_CHILD_LEASE: OnceLock<Mutex<Option<ActiveChildLease>>> = OnceLock::new();

fn active_child_lease() -> &'static Mutex<Option<ActiveChildLease>> {
    ACTIVE_CHILD_LEASE.get_or_init(|| Mutex::new(None))
}

fn reserve_active_child(
    control: Arc<WatchdogControl>,
    process_control: Arc<dyn ChildProcessControl>,
) -> bool {
    let mut active = active_child_lease()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let clear_existing = {
        if let Some(lease) = active.as_ref() {
            if lease.control.reaped.load(Ordering::Acquire) {
                true
            } else {
                let Some(child_slot) = lease.child_slot.as_ref() else {
                    return false;
                };
                let mut child_guard = child_slot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let Some(child) = child_guard.as_mut() else {
                    return false;
                };
                match lease.process_control.try_wait_nonblocking(child) {
                    Ok(Some(status)) => {
                        if lease.control.graceful_expected.load(Ordering::Acquire)
                            && status.success()
                            && !lease.control.stop_requested.load(Ordering::Acquire)
                        {
                            lease.control.exit_success.store(true, Ordering::Release);
                        } else {
                            lease.control.latch_fallback(FallbackReason::ChildExited);
                        }
                        if let Some(pid) = lease.process_id {
                            kill_process_group(pid);
                        }
                        child_guard.take();
                        lease.control.reaped.store(true, Ordering::Release);
                        lease
                            .control
                            .reap_unresolved
                            .store(false, Ordering::Release);
                        true
                    }
                    Ok(None) => return false,
                    Err(_) => {
                        lease.control.reap_unresolved.store(true, Ordering::Release);
                        return false;
                    }
                }
            }
        } else {
            false
        }
    };
    if clear_existing {
        *active = None;
    }
    if active.is_some() {
        return false;
    }
    *active = Some(ActiveChildLease {
        control,
        child_slot: None,
        process_id: None,
        process_control,
    });
    true
}

fn attach_child_lease(
    control: &Arc<WatchdogControl>,
    child_slot: ChildSlot,
    process_id: u32,
) -> bool {
    let mut active = active_child_lease()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(lease) = active.as_mut() else {
        return false;
    };
    if !Arc::ptr_eq(&lease.control, control) {
        return false;
    }
    lease.child_slot = Some(child_slot);
    lease.process_id = Some(process_id);
    true
}

fn release_active_child(control: &Arc<WatchdogControl>) {
    let mut active = active_child_lease()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if active
        .as_ref()
        .is_some_and(|lease| Arc::ptr_eq(&lease.control, control))
    {
        *active = None;
    }
}

impl WatchdogControl {
    fn set_deadline(&self, deadline: Option<Instant>, reason: FallbackReason) {
        if let Some(deadline) = deadline {
            self.deadline_reason.store(reason as u8, Ordering::Release);
            self.deadline_ns
                .store(instant_ns(self.origin, deadline), Ordering::Release);
        } else {
            self.deadline_ns.store(0, Ordering::Release);
        }
    }

    fn set_deadline_ns(&self, deadline_ns: u64, reason: FallbackReason) {
        self.deadline_reason.store(reason as u8, Ordering::Release);
        self.deadline_ns.store(deadline_ns, Ordering::Release);
    }

    fn latch_fallback(&self, reason: FallbackReason) {
        if reason == FallbackReason::None {
            return;
        }
        let _ = self.fallback_reason.compare_exchange(
            FallbackReason::None as u8,
            reason as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        self.stop_requested.store(true, Ordering::Release);
    }

    fn fallback_reason(&self) -> Option<FallbackReason> {
        match self.fallback_reason.load(Ordering::Acquire) {
            0 => None,
            reason => Some(FallbackReason::from_code(reason)),
        }
    }
}

impl HostSession {
    fn start(
        mut command: Command,
        identity: StreamIdentity,
        protocol: Box<dyn StreamProtocol>,
        limits: SupervisorLimits,
        process_control: Arc<dyn ChildProcessControl>,
    ) -> Self {
        let origin = Instant::now();
        let control = Arc::new(WatchdogControl {
            deadline_ns: AtomicU64::new(0),
            deadline_reason: AtomicU8::new(FallbackReason::StartupTimeout as u8),
            fallback_reason: AtomicU8::new(FallbackReason::None as u8),
            stop_requested: AtomicBool::new(false),
            graceful_expected: AtomicBool::new(false),
            reaped: AtomicBool::new(false),
            exit_success: AtomicBool::new(false),
            reap_unresolved: AtomicBool::new(false),
            origin,
        });
        let mut session = Self {
            identity,
            protocol,
            limits,
            origin,
            control: Arc::clone(&control),
            watchdog: None,
            process_id: None,
            stdin: None,
            stdout: None,
            raw: AudioBuffer::default(),
            filtered: AudioBuffer::default(),
            outbound: VecDeque::new(),
            outbound_bytes: 0,
            next_input_sequence: 0,
            next_output_sequence: 0,
            ready: false,
            finished: false,
            finishing: false,
            finish_command_sent: false,
            raw_complete: true,
            raw_valid: true,
            response_buffer: Vec::new(),
            finish_deadline_ns: 0,
        };

        if !identity.is_valid() {
            session.latch_fallback(FallbackReason::InvalidSessionIdentity);
            session.control.reaped.store(true, Ordering::Release);
            return session;
        }
        let begin = match session.protocol.begin_payload(identity) {
            Ok(frame) if !frame.is_empty() && frame.len() <= MAX_PROFILE_FRAME_BYTES => frame,
            _ => {
                session.latch_fallback(FallbackReason::InvalidBeginFrame);
                session.control.reaped.store(true, Ordering::Release);
                return session;
            }
        };
        if !reserve_active_child(Arc::clone(&control), Arc::clone(&process_control)) {
            session.latch_fallback(FallbackReason::SupervisorBusy);
            session.control.reaped.store(true, Ordering::Release);
            return session;
        }
        let (child_sender, child_receiver) = sync_channel(1);
        let watchdog_control = Arc::clone(&control);
        let watchdog_reaper = Arc::clone(&process_control);
        let watchdog_limits = session.limits.clone();
        let watchdog = thread::Builder::new()
            .name("voiceflow-filter-watchdog".to_string())
            .spawn(move || {
                watch_child_receiver(
                    child_receiver,
                    watchdog_control,
                    watchdog_reaper,
                    watchdog_limits,
                )
            });
        match watchdog {
            Ok(watchdog) => session.watchdog = Some(watchdog),
            Err(_) => {
                session.latch_fallback(FallbackReason::SpawnFailed);
                session.control.reaped.store(true, Ordering::Release);
                release_active_child(&control);
                return session;
            }
        }

        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => {
                session.latch_fallback(FallbackReason::SpawnFailed);
                let _ = child_sender.send(None);
                return session;
            }
        };
        session.process_id = Some(child.id());
        session.stdin = child.stdin.take();
        session.stdout = child.stdout.take();
        let pipes_ok = session
            .stdin
            .as_ref()
            .is_some_and(|stdin| set_nonblocking(stdin).is_ok())
            && session
                .stdout
                .as_ref()
                .is_some_and(|stdout| set_nonblocking(stdout).is_ok());
        if !pipes_ok {
            session.latch_fallback(FallbackReason::InvalidPipe);
        }
        let child_pid = child.id();
        let child_slot = Arc::new(Mutex::new(Some(child)));
        if !attach_child_lease(&control, Arc::clone(&child_slot), child_pid) {
            session.latch_fallback(FallbackReason::SupervisorBusy);
        }
        if child_sender
            .send(Some((Arc::clone(&child_slot), child_pid)))
            .is_err()
        {
            session.latch_fallback(FallbackReason::ChildClosureUnresolved);
            session
                .control
                .reap_unresolved
                .store(true, Ordering::Release);
            if let Some(child) = child_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_mut()
            {
                process_control.kill(child, child_pid);
            }
            return session;
        }

        if pipes_ok {
            if !session.queue_payload(begin, OutboundKind::Begin) {
                session.latch_fallback(FallbackReason::InvalidBeginFrame);
            } else {
                session.control.set_deadline(
                    Some(origin + session.limits.startup_timeout),
                    FallbackReason::StartupTimeout,
                );
                session.flush_outbound();
            }
        }
        session
    }

    /// Retain raw samples before any process or IPC work. Input is split into ≤1,024-sample
    /// frames; on IPC failure subsequent calls still append to the raw timeline.
    pub fn push(&mut self, samples: &[f32]) -> Result<(), CaptureError> {
        if samples.is_empty() {
            return Ok(());
        }
        if self
            .raw
            .append(samples, self.limits.max_session_samples)
            .is_err()
        {
            self.raw_complete = false;
            self.latch_fallback(FallbackReason::RawTimelineLimit);
            return Err(CaptureError::RawTimelineLimit);
        }
        if samples.iter().any(|sample| !sample.is_finite()) {
            self.raw_valid = false;
            self.latch_fallback(FallbackReason::InvalidRawAudio);
            return Ok(());
        }
        if self.control.reaped.load(Ordering::Acquire) {
            self.latch_fallback(FallbackReason::ChildExited);
        }
        if self.control.fallback_reason().is_some() || self.finished {
            return Ok(());
        }

        let mut source_start = self.raw.len() - samples.len();
        for chunk in samples.chunks(MAX_AUDIO_FRAME_SAMPLES) {
            let payload = match self.protocol.audio_payload(
                self.identity,
                self.next_input_sequence,
                source_start as u64,
                chunk,
            ) {
                Ok(payload) if !payload.is_empty() => payload,
                _ => {
                    self.latch_fallback(FallbackReason::InvalidBeginFrame);
                    return Ok(());
                }
            };
            self.next_input_sequence = self.next_input_sequence.saturating_add(1);
            if !self.queue_payload(payload, OutboundKind::Audio) {
                self.latch_fallback(FallbackReason::InputQueueFull);
                return Ok(());
            }
            source_start += chunk.len();
        }
        if self.ready && !self.finishing {
            let allowed_lag = self.protocol.max_lookahead_samples();
            if self.raw.len().saturating_sub(self.filtered.len()) > allowed_lag {
                let deadline = Instant::now() + self.limits.inference_timeout;
                if self.control.deadline_ns.load(Ordering::Acquire) == 0 {
                    self.control
                        .set_deadline(Some(deadline), FallbackReason::InferenceTimeout);
                }
            }
        }
        self.flush_outbound();
        Ok(())
    }

    /// Process any currently available child output without waiting. Call during capture to keep
    /// the helper output pipe flowing. If this is not called, OS pipe capacity remains bounded and
    /// the independent watchdog still stops a stalled child.
    pub fn poll(&mut self) -> SessionProgress {
        self.pump_io(0);
        self.progress()
    }

    pub fn progress(&self) -> SessionProgress {
        SessionProgress {
            ready: self.ready,
            fallback: self.control.fallback_reason(),
            captured_samples: self.raw.len(),
            filtered_samples: self.filtered.len(),
            child_reaped: self.control.reaped.load(Ordering::Acquire),
            child_closure_unresolved: self.control.reap_unresolved.load(Ordering::Acquire),
        }
    }

    /// Finish within a single absolute budget capped at 200 ms. With a complete finite raw
    /// timeline, a successful result contains the complete filtered timeline and every process,
    /// protocol, drain, or identity failure selects the complete raw timeline once. Incomplete or
    /// non-finite raw input returns a typed error with no audio selection. `child_reaped` reports
    /// whether the OS confirmed closure before the budget. An unresolved child remains in the
    /// global one-child lease and blocks a later helper launch until a nonblocking reap check
    /// confirms exit.
    pub fn finish(mut self) -> Result<AudioSelection, FinalizeError> {
        let start = Instant::now();
        if self.raw.is_empty() {
            self.force_stop(FallbackReason::Cancelled);
            return Err(FinalizeError::EmptyAudio);
        }
        if !self.raw_complete {
            self.force_stop(FallbackReason::RawTimelineLimit);
            return Err(FinalizeError::RawTimelineIncomplete);
        }
        if !self.raw_valid {
            self.force_stop(FallbackReason::InvalidRawAudio);
            return Err(FinalizeError::InvalidRawAudio);
        }
        self.finishing = true;
        let finish_budget = self.limits.finish_timeout.min(MAX_FINISH_BUDGET);
        let deadline = start + finish_budget;
        self.finish_deadline_ns = instant_ns(self.origin, deadline);
        let drain_deadline = deadline
            .checked_sub(self.limits.shutdown_reserve)
            .unwrap_or(start);
        self.control
            .set_deadline(Some(drain_deadline), FallbackReason::FinishTimeout);

        if self.control.fallback_reason().is_none() && !self.finished {
            match self.protocol.finish_payload(
                self.identity,
                self.raw.len() as u64,
                self.next_input_sequence,
            ) {
                Ok(payload) if !payload.is_empty() => {
                    if !self.queue_payload(payload, OutboundKind::Finish) {
                        self.latch_fallback(FallbackReason::InputQueueFull);
                    }
                }
                _ => self.latch_fallback(FallbackReason::InvalidBeginFrame),
            }
            self.flush_outbound();
        }

        while !self.control.reaped.load(Ordering::Acquire)
            && self.control.fallback_reason().is_none()
            && Instant::now() < drain_deadline
        {
            let remaining = drain_deadline.saturating_duration_since(Instant::now());
            let wait_ms = remaining.as_millis().min(5) as i32;
            self.pump_io(wait_ms.max(0));
        }

        if self.control.fallback_reason().is_none() && !self.finished {
            self.latch_fallback(FallbackReason::FinishTimeout);
        }
        if self.control.fallback_reason().is_none() && self.finished {
            // The helper has sent Finished. Closing stdin lets the standalone serve loop exit;
            // its child process has only the remaining finish budget to exit cleanly.
            self.control
                .graceful_expected
                .store(true, Ordering::Release);
            self.close_stdin_if_drained();
            let grace_deadline = (Instant::now() + self.limits.graceful_exit_timeout).min(deadline);
            self.control
                .set_deadline(Some(grace_deadline), FallbackReason::ChildExitTimeout);
            while !self.control.reaped.load(Ordering::Acquire)
                && self.control.fallback_reason().is_none()
                && Instant::now() < grace_deadline
            {
                let remaining = grace_deadline.saturating_duration_since(Instant::now());
                self.pump_io(remaining.as_millis().min(5) as i32);
            }
            if !self.control.reaped.load(Ordering::Acquire)
                && self.control.fallback_reason().is_none()
            {
                self.latch_fallback(FallbackReason::ChildExitTimeout);
            }
        }

        if self.control.fallback_reason().is_some() {
            self.kill_group_now();
        }
        self.wait_for_reap(deadline);
        self.pump_io(0);
        let elapsed = start.elapsed();
        let reason = self.control.fallback_reason();
        let child_reaped = self.control.reaped.load(Ordering::Acquire);
        let (kind, samples) = if reason.is_none()
            && self.finished
            && self.control.exit_success.load(Ordering::Acquire)
            && self.filtered.len() == self.raw.len()
        {
            (SelectionKind::Filtered, std::mem::take(&mut self.filtered))
        } else {
            let reason = reason.unwrap_or(FallbackReason::ChildExited);
            (
                SelectionKind::RawFallback(reason),
                std::mem::take(&mut self.raw),
            )
        };
        Ok(AudioSelection {
            identity: self.identity,
            kind,
            samples,
            finish_elapsed: elapsed,
            child_reaped,
        })
    }

    /// Cancel and discard this session. Cancellation never returns an audio selection.
    pub fn cancel(mut self) {
        if self.control.fallback_reason().is_none() {
            if let Ok(payload) = self.protocol.cancel_payload(self.identity) {
                let _ = self.queue_payload(payload, OutboundKind::Cancel);
                self.flush_outbound();
                self.pump_io(2);
            }
        }
        self.force_stop(FallbackReason::Cancelled);
    }

    fn queue_payload(&mut self, payload: Vec<u8>, kind: OutboundKind) -> bool {
        let max_payload = if kind == OutboundKind::Begin {
            MAX_PROFILE_FRAME_BYTES
        } else {
            MAX_AUDIO_FRAME_BYTES
        };
        if payload.is_empty() || payload.len() > max_payload {
            self.latch_fallback(if kind == OutboundKind::Begin {
                FallbackReason::InvalidBeginFrame
            } else {
                FallbackReason::InputFrameTooLarge
            });
            return false;
        }
        let Some(frame) = add_frame_prefix(payload) else {
            self.latch_fallback(FallbackReason::InputFrameTooLarge);
            return false;
        };
        let queue_full = self.outbound.len() >= self.limits.max_queued_input_frames
            || self.outbound_bytes.saturating_add(frame.len()) > self.limits.max_queued_input_bytes;
        if queue_full {
            return false;
        }
        self.outbound_bytes += frame.len();
        self.outbound.push_back(PendingWrite {
            frame,
            offset: 0,
            kind,
        });
        true
    }

    fn flush_outbound(&mut self) {
        let Some(stdin) = self.stdin.as_mut() else {
            if !self.outbound.is_empty()
                && self.control.fallback_reason().is_none()
                && !self.control.reaped.load(Ordering::Acquire)
            {
                self.latch_fallback(FallbackReason::PipeIo);
            }
            return;
        };
        while let Some(front) = self.outbound.front_mut() {
            match front.write_once(stdin) {
                Ok(true) => {
                    let frame = self.outbound.pop_front().expect("front exists");
                    self.outbound_bytes = self.outbound_bytes.saturating_sub(frame.frame.len());
                    if frame.kind == OutboundKind::Finish {
                        self.finish_command_sent = true;
                    }
                }
                Ok(false) => continue,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    self.latch_fallback(FallbackReason::PipeIo);
                    break;
                }
            }
        }
        if self.finishing && self.finished && self.outbound.is_empty() {
            self.stdin.take();
        }
    }

    fn close_stdin_if_drained(&mut self) {
        if self.outbound.is_empty() {
            self.stdin.take();
        }
    }

    fn pump_io(&mut self, timeout_ms: i32) {
        self.observe_watchdog();
        if self.control.fallback_reason().is_some() {
            self.close_pipes_if_reaped();
            return;
        }
        self.flush_outbound();
        let Some(stdout_fd) = self.stdout.as_ref().map(raw_fd) else {
            if !self.finished {
                self.latch_fallback(FallbackReason::PipeIo);
            } else if timeout_ms > 0 {
                thread::sleep(Duration::from_millis(timeout_ms.clamp(0, 10) as u64));
            }
            self.observe_watchdog();
            return;
        };
        let stdin_fd = self.stdin.as_ref().map(raw_fd);
        let mut descriptors = Vec::with_capacity(2);
        descriptors.push(libc::pollfd {
            fd: stdout_fd,
            events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
            revents: 0,
        });
        if let Some(fd) = stdin_fd {
            descriptors.push(libc::pollfd {
                fd,
                events: if self.outbound.is_empty() {
                    0
                } else {
                    libc::POLLOUT
                },
                revents: 0,
            });
        }
        let result = unsafe {
            libc::poll(
                descriptors.as_mut_ptr(),
                descriptors.len() as libc::nfds_t,
                timeout_ms.clamp(0, 10),
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                self.latch_fallback(FallbackReason::PipeIo);
            }
        } else {
            if descriptors[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
                self.read_stdout_available();
            }
            if descriptors.len() > 1
                && descriptors[1].revents & (libc::POLLOUT | libc::POLLHUP | libc::POLLERR) != 0
            {
                self.flush_outbound();
            }
        }
        self.observe_watchdog();
        if self.control.reaped.load(Ordering::Acquire) {
            // Read anything already available from the direct child, then close the descriptor.
            self.read_stdout_available();
            self.close_pipes_if_reaped();
            if !self.finished && self.control.fallback_reason().is_none() {
                self.latch_fallback(FallbackReason::ChildExited);
            }
        }
    }

    fn read_stdout_available(&mut self) {
        if self.stdout.is_none() {
            return;
        }
        let mut bytes = [0_u8; 4096];
        let mut bytes_left = MAX_RESPONSE_BYTES_PER_PUMP;
        let mut frames_left = MAX_RESPONSE_FRAMES_PER_PUMP;
        while bytes_left > 0 && frames_left > 0 {
            let read_len = bytes.len().min(bytes_left);
            let read_result = self
                .stdout
                .as_mut()
                .expect("checked above")
                .read(&mut bytes[..read_len]);
            match read_result {
                Ok(0) => {
                    self.stdout.take();
                    if !self.finished && self.control.fallback_reason().is_none() {
                        let reason = if self.response_buffer.is_empty() {
                            FallbackReason::ChildExited
                        } else {
                            FallbackReason::ResponseTruncated
                        };
                        self.latch_fallback(reason);
                    }
                    break;
                }
                Ok(read) => {
                    bytes_left -= read;
                    self.response_buffer.extend_from_slice(&bytes[..read]);
                    let Some(decoded) = self.decode_buffered_responses(frames_left) else {
                        return;
                    };
                    frames_left -= decoded;
                    if frames_left == 0 {
                        return;
                    }
                    if self.response_buffer.len() > self.limits.max_response_frame_bytes + 4 {
                        self.latch_fallback(FallbackReason::ResponseTooLarge);
                        return;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return,
                Err(_) => {
                    self.latch_fallback(FallbackReason::PipeIo);
                    return;
                }
            }
        }
    }

    fn decode_buffered_responses(&mut self, max_frames: usize) -> Option<usize> {
        let mut decoded = 0;
        loop {
            if self.response_buffer.len() < 4 {
                return Some(decoded);
            }
            let frame_len =
                u32::from_be_bytes(self.response_buffer[..4].try_into().unwrap()) as usize;
            if frame_len == 0 || frame_len > self.limits.max_response_frame_bytes {
                self.latch_fallback(FallbackReason::ResponseTooLarge);
                return None;
            }
            let total_len = frame_len + 4;
            if self.response_buffer.len() < total_len {
                return Some(decoded);
            }
            if decoded == max_frames {
                return Some(decoded);
            }
            let payload = self.response_buffer[4..total_len].to_vec();
            self.response_buffer.drain(..total_len);
            let response = match self.protocol.decode_response(&payload) {
                Ok(response) => response,
                Err(_) => {
                    self.latch_fallback(FallbackReason::MalformedResponse);
                    return None;
                }
            };
            if !self.accept_response(response) {
                return None;
            }
            decoded += 1;
        }
    }

    fn accept_response(&mut self, response: ChildResponse) -> bool {
        if self.finished {
            self.latch_fallback(FallbackReason::MalformedResponse);
            return false;
        }
        match response {
            ChildResponse::Ready { identity, proof } => {
                if self.ready || self.raw.is_empty() && self.finishing {
                    self.latch_fallback(FallbackReason::MalformedResponse);
                    return false;
                }
                if identity != self.identity {
                    self.latch_fallback(FallbackReason::IdentityMismatch);
                    return false;
                }
                if proof.as_slice() != self.protocol.expected_ready_proof() {
                    self.latch_fallback(FallbackReason::InvalidReadyProof);
                    return false;
                }
                self.ready = true;
                let allowed_lag = self.protocol.max_lookahead_samples();
                if self.raw.len().saturating_sub(self.filtered.len()) > allowed_lag {
                    self.control.set_deadline(
                        Some(Instant::now() + self.limits.inference_timeout),
                        FallbackReason::InferenceTimeout,
                    );
                } else {
                    self.control
                        .set_deadline(None, FallbackReason::InferenceTimeout);
                }
            }
            ChildResponse::Output {
                identity,
                sequence,
                source_start_sample,
                samples,
            } => {
                if !self.ready || identity != self.identity {
                    self.latch_fallback(if identity != self.identity {
                        FallbackReason::IdentityMismatch
                    } else {
                        FallbackReason::MalformedResponse
                    });
                    return false;
                }
                if samples.is_empty()
                    || samples.len() > MAX_AUDIO_FRAME_SAMPLES
                    || samples.iter().any(|sample| !sample.is_finite())
                {
                    self.latch_fallback(FallbackReason::InvalidOutput);
                    return false;
                }
                if sequence != self.next_output_sequence
                    || source_start_sample != self.filtered.len() as u64
                    || self.filtered.len().saturating_add(samples.len()) > self.raw.len()
                    || self.filtered.len().saturating_add(samples.len())
                        > self.limits.max_session_samples
                {
                    self.latch_fallback(FallbackReason::NonContiguousOutput);
                    return false;
                }
                if self
                    .filtered
                    .append(&samples, self.limits.max_session_samples)
                    .is_err()
                {
                    self.latch_fallback(FallbackReason::InvalidOutput);
                    return false;
                }
                self.next_output_sequence = self.next_output_sequence.saturating_add(1);
                if !self.finishing {
                    let allowed_lag = self.protocol.max_lookahead_samples();
                    if self.raw.len().saturating_sub(self.filtered.len()) > allowed_lag {
                        self.control.set_deadline(
                            Some(Instant::now() + self.limits.inference_timeout),
                            FallbackReason::InferenceTimeout,
                        );
                    } else {
                        self.control
                            .set_deadline(None, FallbackReason::InferenceTimeout);
                    }
                }
            }
            ChildResponse::Finished {
                identity,
                status,
                input_samples,
                output_samples,
            } => {
                if !self.finishing || !self.finish_command_sent {
                    self.latch_fallback(FallbackReason::MalformedResponse);
                    return false;
                }
                if identity != self.identity {
                    self.latch_fallback(FallbackReason::IdentityMismatch);
                    return false;
                }
                if !self.ready || status != 0 {
                    self.latch_fallback(FallbackReason::ChildFailure);
                    return false;
                }
                if input_samples != self.raw.len() as u64
                    || output_samples != self.filtered.len() as u64
                    || output_samples != input_samples
                {
                    self.latch_fallback(FallbackReason::FinishCountMismatch);
                    return false;
                }
                self.finished = true;
                self.control
                    .graceful_expected
                    .store(true, Ordering::Release);
                let graceful_deadline = Instant::now() + self.limits.graceful_exit_timeout;
                let finish_deadline = self.finish_deadline_ns;
                let graceful_ns = instant_ns(self.origin, graceful_deadline).min(finish_deadline);
                self.control
                    .set_deadline_ns(graceful_ns, FallbackReason::ChildExitTimeout);
                self.close_stdin_if_drained();
            }
            ChildResponse::Failure { identity, .. } => {
                if identity.is_some_and(|received| received != self.identity) {
                    self.latch_fallback(FallbackReason::IdentityMismatch);
                } else {
                    self.latch_fallback(FallbackReason::ChildFailure);
                }
                return false;
            }
        }
        true
    }

    fn observe_watchdog(&mut self) {
        if let Some(reason) = self.control.fallback_reason() {
            if reason != FallbackReason::None {
                self.close_pipes_if_reaped();
            }
        } else if self.control.reaped.load(Ordering::Acquire) && !self.finished {
            self.latch_fallback(FallbackReason::ChildExited);
        }
    }

    fn latch_fallback(&self, reason: FallbackReason) {
        self.control.latch_fallback(reason);
    }

    fn kill_group_now(&mut self) {
        self.control.stop_requested.store(true, Ordering::Release);
        if let Some(pid) = self.process_id {
            kill_process_group(pid);
        }
    }

    fn force_stop(&mut self, reason: FallbackReason) {
        if self.control.fallback_reason().is_none() {
            self.control.latch_fallback(reason);
        } else {
            self.control.stop_requested.store(true, Ordering::Release);
        }
        self.kill_group_now();
        let deadline = Instant::now() + self.limits.finish_timeout;
        self.wait_for_reap(deadline);
        self.close_pipes_if_reaped();
    }

    fn wait_for_reap(&mut self, deadline: Instant) {
        while !self.control.reaped.load(Ordering::Acquire) && Instant::now() < deadline {
            if self.control.fallback_reason().is_some() {
                thread::sleep(WATCHDOG_TICK);
            } else {
                self.pump_io(1);
            }
        }
        if !self.control.reaped.load(Ordering::Acquire) {
            if let Some(pid) = self.process_id {
                kill_process_group(pid);
            }
            self.control.stop_requested.store(true, Ordering::Release);
            self.control
                .latch_fallback(FallbackReason::ChildClosureUnresolved);
            self.control.reap_unresolved.store(true, Ordering::Release);
        }
        if let Some(watchdog) = self.watchdog.as_ref() {
            while !watchdog.is_finished() && Instant::now() < deadline {
                thread::yield_now();
            }
        }
        // The watchdog never reads or writes pipes and is never joined here. On successful reap it
        // exits immediately. If closure is unresolved, its bounded attempt ends and the static
        // one-child lease retains Child ownership; a later session can proceed only after one
        // nonblocking reap check confirms that the prior process exited.
        self.watchdog.take();
        self.close_pipes_if_reaped();
    }

    fn close_pipes_if_reaped(&mut self) {
        if self.control.reaped.load(Ordering::Acquire) {
            self.stdin.take();
            self.stdout.take();
            self.outbound.clear();
            self.outbound_bytes = 0;
        }
    }
}

impl Drop for HostSession {
    fn drop(&mut self) {
        if self.watchdog.is_some() {
            self.force_stop(FallbackReason::Cancelled);
        }
    }
}

fn add_frame_prefix(mut payload: Vec<u8>) -> Option<Vec<u8>> {
    let length = u32::try_from(payload.len()).ok()?;
    let mut frame = Vec::with_capacity(payload.len().saturating_add(4));
    frame.extend_from_slice(&length.to_be_bytes());
    frame.append(&mut payload);
    Some(frame)
}

fn instant_ns(origin: Instant, deadline: Instant) -> u64 {
    deadline
        .saturating_duration_since(origin)
        .as_nanos()
        .min(u64::MAX as u128) as u64
}

fn elapsed_ns(origin: Instant) -> u64 {
    origin.elapsed().as_nanos().min(u64::MAX as u128) as u64
}

fn watch_child_receiver(
    receiver: Receiver<ChildRegistration>,
    control: Arc<WatchdogControl>,
    process_control: Arc<dyn ChildProcessControl>,
    limits: SupervisorLimits,
) {
    match receiver.recv() {
        Ok(Some((child_slot, process_id))) => {
            watch_child(child_slot, process_id, control, process_control, limits)
        }
        Ok(None) | Err(_) => {
            control.reaped.store(true, Ordering::Release);
            release_active_child(&control);
        }
    }
}

fn watch_child(
    child_slot: ChildSlot,
    process_id: u32,
    control: Arc<WatchdogControl>,
    process_control: Arc<dyn ChildProcessControl>,
    limits: SupervisorLimits,
) {
    let mut kill_sent = false;
    let mut reap_deadline = None;
    loop {
        if control.reaped.load(Ordering::Acquire) {
            return;
        }
        let requested = control.stop_requested.load(Ordering::Acquire);
        let deadline_ns = control.deadline_ns.load(Ordering::Acquire);
        let timed_out = deadline_ns != 0 && elapsed_ns(control.origin) >= deadline_ns;
        if !kill_sent && (requested || timed_out) {
            if timed_out {
                let reason =
                    FallbackReason::from_code(control.deadline_reason.load(Ordering::Acquire));
                control.latch_fallback(reason);
            }
            if let Some(child) = child_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_mut()
            {
                process_control.kill(child, process_id);
            }
            kill_process_group(process_id);
            kill_sent = true;
            reap_deadline = Some(Instant::now() + limits.shutdown_reserve);
        }

        let wait_result = {
            let mut child = child_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match child.as_mut() {
                Some(child) => process_control.try_wait_nonblocking(child),
                None if control.reaped.load(Ordering::Acquire) => return,
                None => {
                    control.reap_unresolved.store(true, Ordering::Release);
                    control.latch_fallback(FallbackReason::ChildClosureUnresolved);
                    return;
                }
            }
        };

        match wait_result {
            Ok(Some(status)) => {
                let expected = control.graceful_expected.load(Ordering::Acquire);
                if expected && status.success() && !control.stop_requested.load(Ordering::Acquire) {
                    control.exit_success.store(true, Ordering::Release);
                } else {
                    control.latch_fallback(FallbackReason::ChildExited);
                }
                kill_process_group(process_id);
                child_slot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take();
                control.reaped.store(true, Ordering::Release);
                control.reap_unresolved.store(false, Ordering::Release);
                release_active_child(&control);
                return;
            }
            Ok(None) => {}
            Err(_) => {
                control.latch_fallback(FallbackReason::ChildReapError);
                if let Some(child) = child_slot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_mut()
                {
                    process_control.kill(child, process_id);
                }
                kill_process_group(process_id);
                control.reap_unresolved.store(true, Ordering::Release);
                return;
            }
        }

        if kill_sent && reap_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            control.latch_fallback(FallbackReason::ChildClosureUnresolved);
            control.reap_unresolved.store(true, Ordering::Release);
            return;
        }
        thread::sleep(WATCHDOG_TICK);
    }
}

#[cfg(unix)]
fn raw_fd<T: std::os::fd::AsRawFd>(stream: &T) -> libc::c_int {
    stream.as_raw_fd()
}

#[cfg(unix)]
fn set_nonblocking<T: std::os::fd::AsRawFd>(stream: &T) -> io::Result<()> {
    // fcntl changes the parent-side pipe endpoint only. The child retains normal blocking stdio.
    let fd = stream.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let result = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(unix))]
fn raw_fd<T>(_stream: &T) -> libc::c_int {
    -1
}

#[cfg(not(unix))]
fn set_nonblocking<T>(_stream: &T) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "nonblocking child pipes require a Unix host",
    ))
}

fn kill_process_group(process_id: u32) {
    #[cfg(unix)]
    {
        if let Ok(group) = i32::try_from(process_id) {
            let _ = unsafe { libc::kill(-group, libc::SIGKILL) };
        }
        // The watchdog also signals the direct child and reaps it with `try_wait`.
    }
}

/// Adapter for the existing standalone helper protocol. The adapter carries only identity and
/// framing facts; it does not select or enable a recipe.
pub struct ExistingHelperProtocol {
    model_digest: [u8; 32],
    profile_digest: [u8; 32],
    profile_bytes: Vec<u8>,
    recipe: u8,
    ready_proof: Vec<u8>,
    lookahead_samples: usize,
}

impl ExistingHelperProtocol {
    pub fn new(
        model_digest: [u8; 32],
        profile_digest: [u8; 32],
        profile_bytes: Vec<u8>,
        recipe: u8,
    ) -> Result<Self, ProtocolError> {
        if profile_bytes.is_empty() || profile_bytes.len() > 512 * 1024 || recipe > 1 {
            return Err(ProtocolError);
        }
        let mut ready_proof = Vec::with_capacity(65);
        ready_proof.extend_from_slice(&model_digest);
        ready_proof.extend_from_slice(&profile_digest);
        ready_proof.push(recipe);
        Ok(Self {
            model_digest,
            profile_digest,
            profile_bytes,
            recipe,
            ready_proof,
            lookahead_samples: 256,
        })
    }
}

impl StreamProtocol for ExistingHelperProtocol {
    fn begin_payload(&self, identity: StreamIdentity) -> Result<Vec<u8>, ProtocolError> {
        let profile_len = u32::try_from(self.profile_bytes.len()).map_err(|_| ProtocolError)?;
        let mut payload =
            Vec::with_capacity(1 + 8 + 8 + 1 + 32 + 32 + 4 + self.profile_bytes.len());
        payload.push(1);
        payload.extend_from_slice(&identity.session_id.to_be_bytes());
        payload.extend_from_slice(&identity.audio_epoch.to_be_bytes());
        payload.push(self.recipe);
        payload.extend_from_slice(&self.model_digest);
        payload.extend_from_slice(&self.profile_digest);
        payload.extend_from_slice(&profile_len.to_be_bytes());
        payload.extend_from_slice(&self.profile_bytes);
        Ok(payload)
    }

    fn audio_payload(
        &self,
        identity: StreamIdentity,
        sequence: u64,
        source_start_sample: u64,
        samples: &[f32],
    ) -> Result<Vec<u8>, ProtocolError> {
        if samples.is_empty() || samples.len() > MAX_AUDIO_FRAME_SAMPLES {
            return Err(ProtocolError);
        }
        let mut payload = Vec::with_capacity(MAX_AUDIO_FRAME_BYTES);
        payload.push(2);
        payload.extend_from_slice(&identity.session_id.to_be_bytes());
        payload.extend_from_slice(&identity.audio_epoch.to_be_bytes());
        payload.extend_from_slice(&sequence.to_be_bytes());
        payload.extend_from_slice(&source_start_sample.to_be_bytes());
        payload.extend_from_slice(&(samples.len() as u16).to_be_bytes());
        for sample in samples {
            payload.extend_from_slice(&sample.to_le_bytes());
        }
        Ok(payload)
    }

    fn finish_payload(
        &self,
        identity: StreamIdentity,
        input_samples: u64,
        next_sequence: u64,
    ) -> Result<Vec<u8>, ProtocolError> {
        let mut payload = Vec::with_capacity(33);
        payload.push(3);
        payload.extend_from_slice(&identity.session_id.to_be_bytes());
        payload.extend_from_slice(&identity.audio_epoch.to_be_bytes());
        payload.extend_from_slice(&input_samples.to_be_bytes());
        payload.extend_from_slice(&next_sequence.to_be_bytes());
        Ok(payload)
    }

    fn cancel_payload(&self, identity: StreamIdentity) -> Result<Vec<u8>, ProtocolError> {
        let mut payload = Vec::with_capacity(17);
        payload.push(4);
        payload.extend_from_slice(&identity.session_id.to_be_bytes());
        payload.extend_from_slice(&identity.audio_epoch.to_be_bytes());
        Ok(payload)
    }

    fn decode_response(&self, payload: &[u8]) -> Result<ChildResponse, ProtocolError> {
        let tag = *payload.first().ok_or(ProtocolError)?;
        match tag {
            0x80 if payload.len() == 82 => Ok(ChildResponse::Ready {
                identity: read_identity(payload, 1)?,
                proof: payload[17..].to_vec(),
            }),
            0x81 if payload.len() >= 35 => {
                let identity = read_identity(payload, 1)?;
                let sequence = read_u64(payload, 17)?;
                let source_start_sample = read_u64(payload, 25)?;
                let count = read_u16(payload, 33)? as usize;
                if count == 0 || count > MAX_AUDIO_FRAME_SAMPLES || payload.len() != 35 + count * 4
                {
                    return Err(ProtocolError);
                }
                let mut samples = Vec::with_capacity(count);
                for bytes in payload[35..].as_chunks::<4>().0 {
                    samples.push(f32::from_le_bytes(*bytes));
                }
                Ok(ChildResponse::Output {
                    identity,
                    sequence,
                    source_start_sample,
                    samples,
                })
            }
            0x82 if payload.len() == 34 => Ok(ChildResponse::Finished {
                identity: read_identity(payload, 1)?,
                status: payload[17],
                input_samples: read_u64(payload, 18)?,
                output_samples: read_u64(payload, 26)?,
            }),
            0x83 if payload.len() == 19 => {
                let session_id = read_u64(payload, 1)?;
                let audio_epoch = read_u64(payload, 9)?;
                Ok(ChildResponse::Failure {
                    identity: (session_id != 0 && audio_epoch != 0).then_some(StreamIdentity {
                        session_id,
                        audio_epoch,
                    }),
                    code: read_u16(payload, 17)?,
                })
            }
            _ => Err(ProtocolError),
        }
    }

    fn expected_ready_proof(&self) -> &[u8] {
        &self.ready_proof
    }

    fn max_lookahead_samples(&self) -> usize {
        self.lookahead_samples
    }
}

fn read_identity(payload: &[u8], offset: usize) -> Result<StreamIdentity, ProtocolError> {
    let identity = StreamIdentity {
        session_id: read_u64(payload, offset)?,
        audio_epoch: read_u64(payload, offset + 8)?,
    };
    if identity.is_valid() {
        Ok(identity)
    } else {
        Err(ProtocolError)
    }
}

fn read_u64(payload: &[u8], offset: usize) -> Result<u64, ProtocolError> {
    Ok(u64::from_be_bytes(
        payload
            .get(offset..offset + 8)
            .ok_or(ProtocolError)?
            .try_into()
            .map_err(|_| ProtocolError)?,
    ))
}

fn read_u16(payload: &[u8], offset: usize) -> Result<u16, ProtocolError> {
    Ok(u16::from_be_bytes(
        payload
            .get(offset..offset + 2)
            .ok_or(ProtocolError)?
            .try_into()
            .map_err(|_| ProtocolError)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::atomic::AtomicU8;

    static TEST_CASE_LOCK: Mutex<()> = Mutex::new(());

    fn serialize_test_case() -> std::sync::MutexGuard<'static, ()> {
        TEST_CASE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    const CHILD_FIXTURE: &str = r#"
import os, struct, sys, time

mode = sys.argv[1]

def read_exact(count):
    out = bytearray()
    while len(out) < count:
        chunk = sys.stdin.buffer.read(count - len(out))
        if not chunk:
            return None if not out else bytes(out)
        out.extend(chunk)
    return bytes(out)

def read_frame():
    head = read_exact(4)
    if head is None or len(head) != 4:
        return None
    size = struct.unpack('>I', head)[0]
    body = read_exact(size)
    if body is None or len(body) != size:
        return None
    return body

def emit(payload, fragment=0, delay=0):
    frame = struct.pack('>I', len(payload)) + payload
    if not fragment:
        sys.stdout.buffer.write(frame)
        sys.stdout.buffer.flush()
        return
    for at in range(0, len(frame), fragment):
        sys.stdout.buffer.write(frame[at:at + fragment])
        sys.stdout.buffer.flush()
        if delay:
            time.sleep(delay)

def response_ready(sid, epoch, wrong=False):
    if wrong:
        epoch += 1
    emit(bytes([0x80]) + struct.pack('>QQ', sid, epoch) + b'\xa5',
         3 if mode == 'fragmented' else 0, 0.001)

def output_payload(sid, epoch, sequence, offset, values, wrong_epoch=False, wrong_offset=False, wrong_seq=False):
    if wrong_epoch:
        epoch += 1
    if wrong_offset:
        offset += 1
    if wrong_seq:
        sequence += 1
    data = b''.join(struct.pack('<f', value) for value in values)
    return (bytes([0x81]) + struct.pack('>QQQQH', sid, epoch, sequence, offset, len(values)) + data)

if mode == 'no_read':
    time.sleep(30)
    sys.exit(0)

sid = epoch = 0
out_sequence = 0
input_count = 0
output_count = 0
stored = []

while True:
    body = read_frame()
    if body is None:
        break
    tag = body[0]
    if tag == 1:
        sid, epoch = struct.unpack('>QQ', body[1:17])
        if mode == 'partial_max':
            sys.stdout.buffer.write(struct.pack('>I', 4131) + b'\x80')
            sys.stdout.buffer.flush()
            time.sleep(30)
            os._exit(0)
        if mode == 'slow_ready':
            ready = bytes([0x80]) + struct.pack('>QQ', sid, epoch) + b'\xa5'
            emit(ready, 1, 0.025)
        elif mode == 'delayed_ready':
            pass
        else:
            response_ready(sid, epoch, mode == 'stale_ready')
        if mode == 'duplicate_ready':
            response_ready(sid, epoch)
        if mode == 'descendant':
            child = os.fork()
            if child == 0:
                while True:
                    time.sleep(1)
        if mode == 'flood':
            seq = 0
            while True:
                emit(output_payload(sid, epoch, seq, 0, [0.0] * 1024))
                seq += 1
    elif tag == 2:
        if mode == 'hang' or mode == 'descendant':
            time.sleep(30)
        if mode == 'crash':
            os._exit(7)
        sequence, offset, count = struct.unpack('>QQH', body[17:35])
        values = [struct.unpack('<f', body[35 + at * 4:39 + at * 4])[0] for at in range(count)]
        input_count += count
        if mode in ('tail', 'delayed_ready'):
            stored.append((sequence, offset, values))
            continue
        if mode == 'no_output':
            continue
        response = output_payload(
            sid, epoch, out_sequence, offset, values,
            wrong_epoch=(mode == 'stale_output'),
            wrong_offset=(mode == 'wrong_offset'),
            wrong_seq=(mode == 'wrong_sequence'))
        emit(response, 3 if mode == 'fragmented' else 0, 0.001)
        out_sequence += 1
        output_count += count
        if mode == 'early_finished':
            emit(bytes([0x82]) + struct.pack('>QQBQQ', sid, epoch, 0, input_count, output_count))
        if mode == 'ready_after_output':
            response_ready(sid, epoch)
    elif tag == 3:
        expected_samples, expected_sequence = struct.unpack('>QQ', body[17:33])
        if mode == 'delayed_ready':
            response_ready(sid, epoch)
        if mode in ('tail', 'delayed_ready'):
            for _, _, values in stored:
                emit(output_payload(sid, epoch, out_sequence, output_count, values))
                out_sequence += 1
                output_count += len(values)
        finish_epoch = epoch + (mode == 'stale_finish')
        finish_input_count = input_count + (mode == 'stale_finish_count')
        finished = bytes([0x82]) + struct.pack(
            '>QQBQQ', sid, finish_epoch, 0, finish_input_count, output_count)
        emit(finished, 3 if mode == 'fragmented' else 0, 0.001)
        if mode == 'duplicate_finished':
            emit(finished)
        if mode == 'after_finished_output':
            emit(output_payload(sid, epoch, out_sequence, output_count, [0.1]))
        if mode == 'ready_after_finished':
            response_ready(sid, epoch)
        if mode == 'after_finished_hang':
            time.sleep(30)
        else:
            while read_frame() is not None:
                pass
        break
    elif tag == 4:
        break
sys.exit(0)
"#;

    #[derive(Debug)]
    struct TestProtocol;

    impl StreamProtocol for TestProtocol {
        fn begin_payload(&self, identity: StreamIdentity) -> Result<Vec<u8>, ProtocolError> {
            let mut payload = vec![1];
            payload.extend_from_slice(&identity.session_id.to_be_bytes());
            payload.extend_from_slice(&identity.audio_epoch.to_be_bytes());
            Ok(payload)
        }

        fn audio_payload(
            &self,
            identity: StreamIdentity,
            sequence: u64,
            source_start_sample: u64,
            samples: &[f32],
        ) -> Result<Vec<u8>, ProtocolError> {
            if samples.is_empty() || samples.len() > MAX_AUDIO_FRAME_SAMPLES {
                return Err(ProtocolError);
            }
            let mut payload = Vec::new();
            payload.push(2);
            payload.extend_from_slice(&identity.session_id.to_be_bytes());
            payload.extend_from_slice(&identity.audio_epoch.to_be_bytes());
            payload.extend_from_slice(&sequence.to_be_bytes());
            payload.extend_from_slice(&source_start_sample.to_be_bytes());
            payload.extend_from_slice(&(samples.len() as u16).to_be_bytes());
            for sample in samples {
                payload.extend_from_slice(&sample.to_le_bytes());
            }
            Ok(payload)
        }

        fn finish_payload(
            &self,
            identity: StreamIdentity,
            input_samples: u64,
            next_sequence: u64,
        ) -> Result<Vec<u8>, ProtocolError> {
            let mut payload = vec![3];
            payload.extend_from_slice(&identity.session_id.to_be_bytes());
            payload.extend_from_slice(&identity.audio_epoch.to_be_bytes());
            payload.extend_from_slice(&input_samples.to_be_bytes());
            payload.extend_from_slice(&next_sequence.to_be_bytes());
            Ok(payload)
        }

        fn cancel_payload(&self, identity: StreamIdentity) -> Result<Vec<u8>, ProtocolError> {
            let mut payload = vec![4];
            payload.extend_from_slice(&identity.session_id.to_be_bytes());
            payload.extend_from_slice(&identity.audio_epoch.to_be_bytes());
            Ok(payload)
        }

        fn decode_response(&self, payload: &[u8]) -> Result<ChildResponse, ProtocolError> {
            match payload.first().copied() {
                Some(0x80) if payload.len() == 18 => Ok(ChildResponse::Ready {
                    identity: read_identity(payload, 1)?,
                    proof: payload[17..].to_vec(),
                }),
                Some(0x81) if payload.len() >= 35 => {
                    let count = read_u16(payload, 33)? as usize;
                    if count == 0
                        || count > MAX_AUDIO_FRAME_SAMPLES
                        || payload.len() != 35 + 4 * count
                    {
                        return Err(ProtocolError);
                    }
                    let samples = payload[35..]
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|chunk| f32::from_le_bytes(*chunk))
                        .collect();
                    Ok(ChildResponse::Output {
                        identity: read_identity(payload, 1)?,
                        sequence: read_u64(payload, 17)?,
                        source_start_sample: read_u64(payload, 25)?,
                        samples,
                    })
                }
                Some(0x82) if payload.len() == 34 => Ok(ChildResponse::Finished {
                    identity: read_identity(payload, 1)?,
                    status: payload[17],
                    input_samples: read_u64(payload, 18)?,
                    output_samples: read_u64(payload, 26)?,
                }),
                Some(0x83) if payload.len() == 19 => Ok(ChildResponse::Failure {
                    identity: Some(read_identity(payload, 1)?),
                    code: read_u16(payload, 17)?,
                }),
                _ => Err(ProtocolError),
            }
        }

        fn expected_ready_proof(&self) -> &[u8] {
            &[0xa5]
        }
    }

    fn test_limits() -> SupervisorLimits {
        SupervisorLimits {
            startup_timeout: Duration::from_millis(500),
            inference_timeout: Duration::from_millis(300),
            finish_timeout: Duration::from_millis(160),
            shutdown_reserve: Duration::from_millis(20),
            graceful_exit_timeout: Duration::from_millis(40),
            ..SupervisorLimits::default()
        }
    }

    fn command(mode: &str) -> Command {
        let mut command = Command::new("python3");
        command.args(["-u", "-c", CHILD_FIXTURE, mode]);
        command
    }

    fn make_session(mode: &str, identity: StreamIdentity) -> HostSession {
        start_session(HostSupervisor::new(test_limits()), command(mode), identity)
    }

    fn start_session(
        supervisor: HostSupervisor,
        command: Command,
        identity: StreamIdentity,
    ) -> HostSession {
        supervisor.start(command, identity, TestProtocol)
    }

    fn selection_samples(selection: &AudioSelection) -> Vec<f32> {
        selection.samples.iter().copied().collect()
    }

    struct PartialWriter {
        max_write: usize,
        written: Vec<u8>,
    }

    impl Write for PartialWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            let count = buffer.len().min(self.max_write);
            self.written.extend_from_slice(&buffer[..count]);
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn finish_command_is_incomplete_until_the_last_partial_write() {
        let frame = (0..11).collect::<Vec<u8>>();
        let mut pending = PendingWrite {
            frame: frame.clone(),
            offset: 0,
            kind: OutboundKind::Finish,
        };
        let mut writer = PartialWriter {
            max_write: 4,
            written: Vec::new(),
        };

        assert!(!pending.write_once(&mut writer).unwrap());
        assert_eq!(pending.offset, 4);
        assert!(!pending.write_once(&mut writer).unwrap());
        assert_eq!(pending.offset, 8);
        assert!(pending.write_once(&mut writer).unwrap());
        assert_eq!(pending.offset, frame.len());
        assert_eq!(writer.written, frame);
    }

    fn wait_until_ready(session: &mut HostSession) {
        let deadline = Instant::now() + Duration::from_millis(300);
        while !session.progress().ready && Instant::now() < deadline {
            session.poll();
            thread::sleep(Duration::from_millis(1));
        }
        assert!(session.progress().ready);
    }

    fn assert_raw(selection: AudioSelection, reason: FallbackReason, expected: &[f32]) {
        assert_eq!(selection.kind, SelectionKind::RawFallback(reason));
        assert_eq!(selection_samples(&selection), expected);
    }

    struct InjectedReaper {
        mode: AtomicU8,
        allow_reap: AtomicBool,
    }

    impl InjectedReaper {
        const NEVER: u8 = 0;
        const REAL: u8 = 1;
        const ERROR: u8 = 2;

        fn never() -> Self {
            Self {
                mode: AtomicU8::new(Self::NEVER),
                allow_reap: AtomicBool::new(false),
            }
        }
    }

    impl Drop for InjectedReaper {
        fn drop(&mut self) {
            self.mode.store(Self::REAL, Ordering::Release);
            self.allow_reap.store(true, Ordering::Release);
        }
    }

    impl ChildProcessControl for InjectedReaper {
        fn try_wait_nonblocking(&self, child: &mut Child) -> io::Result<Option<ExitStatus>> {
            match self.mode.load(Ordering::Acquire) {
                Self::NEVER if !self.allow_reap.load(Ordering::Acquire) => Ok(None),
                Self::ERROR => Err(io::Error::other("injected wait error")),
                _ => child.try_wait(),
            }
        }

        fn kill(&self, child: &mut Child, process_id: u32) {
            kill_process_group(process_id);
            let _ = child.kill();
        }
    }

    #[test]
    fn tail_flush_selects_the_complete_filtered_timeline_once() {
        let _case = serialize_test_case();
        let identity = StreamIdentity {
            session_id: 11,
            audio_epoch: 1,
        };
        let mut session = make_session("tail", identity);
        let raw = [0.1, -0.2, 0.3, -0.4, 0.5];
        session.push(&raw).unwrap();
        let selection = session.finish().unwrap();
        assert_eq!(selection.identity, identity);
        assert_eq!(selection.kind, SelectionKind::Filtered);
        assert_eq!(selection_samples(&selection), raw);
        assert!(selection.finish_elapsed <= MAX_FINISH_BUDGET);
    }

    #[test]
    fn fragmented_pipe_frames_are_reassembled_before_validation() {
        let _case = serialize_test_case();
        let identity = StreamIdentity {
            session_id: 12,
            audio_epoch: 4,
        };
        let mut session = make_session("fragmented", identity);
        let raw = [0.2, 0.3, -0.7];
        session.push(&raw).unwrap();
        let selection = session.finish().unwrap();
        assert_eq!(selection.kind, SelectionKind::Filtered);
        assert_eq!(selection_samples(&selection), raw);
    }

    #[test]
    fn stale_epoch_and_offset_results_fall_back_to_complete_raw_audio() {
        let _case = serialize_test_case();
        let raw = [0.11, 0.22, 0.33];
        let identity = StreamIdentity {
            session_id: 13,
            audio_epoch: 5,
        };
        let mut stale = make_session("stale_output", identity);
        stale.push(&raw).unwrap();
        assert_raw(
            stale.finish().unwrap(),
            FallbackReason::IdentityMismatch,
            &raw,
        );

        let mut offset = make_session(
            "wrong_offset",
            StreamIdentity {
                session_id: 13,
                audio_epoch: 6,
            },
        );
        offset.push(&raw).unwrap();
        assert_raw(
            offset.finish().unwrap(),
            FallbackReason::NonContiguousOutput,
            &raw,
        );
    }

    #[test]
    fn non_finite_raw_audio_never_becomes_a_selection_and_capture_continues() {
        let _case = serialize_test_case();
        let invalid_samples = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
        let mut identity = 300;

        for invalid in invalid_samples {
            for invalid_first in [true, false] {
                let mut session = make_session(
                    "good",
                    StreamIdentity {
                        session_id: identity,
                        audio_epoch: identity + 1,
                    },
                );
                identity += 2;

                if invalid_first {
                    session.push(&[invalid]).unwrap();
                    session.push(&[0.11, 0.22]).unwrap();
                } else {
                    session.push(&[0.11]).unwrap();
                    session.push(&[invalid]).unwrap();
                    session.push(&[0.22]).unwrap();
                }

                assert_eq!(
                    session.progress().fallback,
                    Some(FallbackReason::InvalidRawAudio)
                );
                assert_eq!(session.progress().captured_samples, 3);
                assert!(matches!(
                    session.finish(),
                    Err(FinalizeError::InvalidRawAudio)
                ));
            }
        }
    }

    #[test]
    fn unsolicited_finished_during_capture_cannot_select_filtered_audio() {
        let _case = serialize_test_case();
        let identity = StreamIdentity {
            session_id: 33,
            audio_epoch: 43,
        };
        let raw = [0.1, 0.2];
        let mut session = make_session("early_finished", identity);
        session.push(&raw).unwrap();
        wait_until_ready(&mut session);

        let deadline = Instant::now() + Duration::from_millis(100);
        while session.progress().fallback.is_none() && Instant::now() < deadline {
            session.poll();
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            session.progress().fallback,
            Some(FallbackReason::MalformedResponse)
        );
        assert_raw(
            session.finish().unwrap(),
            FallbackReason::MalformedResponse,
            &raw,
        );
    }

    #[test]
    fn delayed_ready_is_allowed_after_finish_write_but_bad_finish_lifecycle_is_rejected() {
        let _case = serialize_test_case();
        let raw = [0.15, 0.25, 0.35];
        let mut delayed_ready = make_session(
            "delayed_ready",
            StreamIdentity {
                session_id: 34,
                audio_epoch: 44,
            },
        );
        delayed_ready.push(&raw).unwrap();
        let selection = delayed_ready.finish().unwrap();
        assert_eq!(selection.kind, SelectionKind::Filtered);
        assert_eq!(selection_samples(&selection), raw);

        for (offset, mode) in [
            "stale_finish",
            "stale_finish_count",
            "duplicate_finished",
            "after_finished_output",
            "duplicate_ready",
            "ready_after_output",
            "ready_after_finished",
        ]
        .into_iter()
        .enumerate()
        {
            let mut session = make_session(
                mode,
                StreamIdentity {
                    session_id: 40 + offset as u64,
                    audio_epoch: 50 + offset as u64,
                },
            );
            session.push(&raw).unwrap();
            let selection = session.finish().unwrap();
            assert_ne!(selection.kind, SelectionKind::Filtered, "accepted {mode}");
            assert_eq!(selection_samples(&selection), raw);
        }
    }

    #[test]
    fn blocked_child_stdin_overflows_a_bounded_queue_but_raw_capture_continues() {
        let _case = serialize_test_case();
        let identity = StreamIdentity {
            session_id: 14,
            audio_epoch: 7,
        };
        let mut session = make_session("no_read", identity);
        let chunk = [0.25_f32; MAX_AUDIO_FRAME_SAMPLES];
        for _ in 0..40 {
            session.push(&chunk).unwrap();
        }
        assert_eq!(
            session.progress().fallback,
            Some(FallbackReason::InputQueueFull)
        );
        let tail = [0.6, 0.7];
        session.push(&tail).unwrap();
        let selection = session.finish().unwrap();
        assert_eq!(
            selection.kind,
            SelectionKind::RawFallback(FallbackReason::InputQueueFull)
        );
        assert_eq!(
            selection.samples.len(),
            40 * MAX_AUDIO_FRAME_SAMPLES + tail.len()
        );
        assert_eq!(
            &selection_samples(&selection)[selection.samples.len() - 2..],
            &tail
        );
    }

    #[test]
    fn never_returning_inference_is_killed_and_reaped_within_finish_budget() {
        let _case = serialize_test_case();
        let identity = StreamIdentity {
            session_id: 15,
            audio_epoch: 8,
        };
        let mut session = make_session("hang", identity);
        let raw = [0.9, 0.8, 0.7];
        session.push(&raw).unwrap();
        let started = Instant::now();
        let selection = session.finish().unwrap();
        let child_reaped = selection.child_reaped;
        assert_raw(selection, FallbackReason::FinishTimeout, &raw);
        assert!(child_reaped);
        assert!(started.elapsed() <= MAX_FINISH_BUDGET + Duration::from_millis(40));
    }

    #[test]
    fn independent_watchdog_stops_a_hung_inference_without_host_polling() {
        let _case = serialize_test_case();
        let mut session = make_session(
            "hang",
            StreamIdentity {
                session_id: 151,
                audio_epoch: 81,
            },
        );
        session.push(&[0.9, 0.8, 0.7]).unwrap();
        wait_until_ready(&mut session);
        let timeout = Duration::from_millis(350);
        thread::sleep(timeout);
        assert_eq!(
            session.progress().fallback,
            Some(FallbackReason::InferenceTimeout)
        );
        session.cancel();
    }

    #[test]
    fn cancel_terminates_and_reaps_a_hung_child_within_bound() {
        let _case = serialize_test_case();
        let mut session = make_session(
            "hang",
            StreamIdentity {
                session_id: 152,
                audio_epoch: 82,
            },
        );
        session.push(&[0.9, 0.8, 0.7]).unwrap();
        wait_until_ready(&mut session);
        let control = Arc::clone(&session.control);
        let started = Instant::now();
        session.cancel();
        assert!(control.reaped.load(Ordering::Acquire));
        assert!(started.elapsed() <= MAX_FINISH_BUDGET + Duration::from_millis(40));
    }

    #[test]
    fn unresolved_reap_returns_raw_and_blocks_new_helper_until_closure_is_confirmed() {
        let _case = serialize_test_case();
        let reaper = Arc::new(InjectedReaper::never());
        let supervisor = HostSupervisor::with_process_control(
            test_limits(),
            Arc::clone(&reaper) as Arc<dyn ChildProcessControl>,
        );
        let raw = [0.41, 0.42, 0.43];
        let mut first = start_session(
            supervisor.clone(),
            command("hang"),
            StreamIdentity {
                session_id: 153,
                audio_epoch: 83,
            },
        );
        first.push(&raw).unwrap();
        wait_until_ready(&mut first);
        let first_selection = first.finish().unwrap();
        assert_eq!(selection_samples(&first_selection), raw);
        assert!(!first_selection.child_reaped);
        assert!(first_selection.finish_elapsed <= MAX_FINISH_BUDGET);

        let mut blocked = start_session(
            supervisor.clone(),
            command("good"),
            StreamIdentity {
                session_id: 154,
                audio_epoch: 84,
            },
        );
        blocked.push(&raw).unwrap();
        let blocked_selection = blocked.finish().unwrap();
        assert_eq!(
            blocked_selection.kind,
            SelectionKind::RawFallback(FallbackReason::SupervisorBusy)
        );
        assert!(blocked_selection.child_reaped);

        reaper.allow_reap.store(true, Ordering::Release);
        let mut recovered = None;
        for attempt in 0..100 {
            let mut session = start_session(
                supervisor.clone(),
                command("good"),
                StreamIdentity {
                    session_id: 200 + attempt,
                    audio_epoch: 100 + attempt,
                },
            );
            session.push(&raw).unwrap();
            let selection = session.finish().unwrap();
            if selection.kind == SelectionKind::Filtered {
                recovered = Some(selection);
                break;
            }
            assert_eq!(
                selection.kind,
                SelectionKind::RawFallback(FallbackReason::SupervisorBusy)
            );
            thread::sleep(Duration::from_millis(2));
        }
        let recovered = recovered.expect("the confirmed prior child should release the lease");
        assert!(recovered.child_reaped);
        assert_eq!(selection_samples(&recovered), raw);
    }

    #[test]
    fn permanent_reap_error_returns_without_waiting_or_repeated_polling() {
        let _case = serialize_test_case();
        let reaper = Arc::new(InjectedReaper::never());
        reaper.mode.store(InjectedReaper::ERROR, Ordering::Release);
        let supervisor = HostSupervisor::with_process_control(
            test_limits(),
            Arc::clone(&reaper) as Arc<dyn ChildProcessControl>,
        );
        let raw = [0.51, 0.52];
        let mut session = start_session(
            supervisor.clone(),
            command("hang"),
            StreamIdentity {
                session_id: 155,
                audio_epoch: 85,
            },
        );
        session.push(&raw).unwrap();
        let started = Instant::now();
        let selection = session.finish().unwrap();
        let child_reaped = selection.child_reaped;
        assert_raw(selection, FallbackReason::ChildReapError, &raw);
        assert!(started.elapsed() <= MAX_FINISH_BUDGET + Duration::from_millis(40));
        assert!(!child_reaped);

        reaper.mode.store(InjectedReaper::REAL, Ordering::Release);
        let mut recovered = None;
        for attempt in 0..100 {
            let mut session = start_session(
                supervisor.clone(),
                command("good"),
                StreamIdentity {
                    session_id: 156 + attempt,
                    audio_epoch: 86 + attempt,
                },
            );
            session.push(&raw).unwrap();
            let selection = session.finish().unwrap();
            if selection.kind == SelectionKind::Filtered {
                recovered = Some(selection);
                break;
            }
            assert_eq!(
                selection.kind,
                SelectionKind::RawFallback(FallbackReason::SupervisorBusy)
            );
            thread::sleep(Duration::from_millis(2));
        }
        let recovered = recovered.expect("the successful reap check should release the lease");
        assert!(recovered.child_reaped);
    }

    #[test]
    fn child_crash_falls_back_and_the_following_epoch_recovers() {
        let _case = serialize_test_case();
        let raw = [0.4, 0.5, 0.6];
        let mut crashed = make_session(
            "crash",
            StreamIdentity {
                session_id: 16,
                audio_epoch: 9,
            },
        );
        crashed.push(&raw).unwrap();
        let crash_deadline = Instant::now() + Duration::from_millis(100);
        while crashed.progress().fallback.is_none() && Instant::now() < crash_deadline {
            crashed.poll();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            crashed.progress().fallback,
            Some(FallbackReason::ChildExited)
        );
        assert_raw(crashed.finish().unwrap(), FallbackReason::ChildExited, &raw);

        let next_identity = StreamIdentity {
            session_id: 17,
            audio_epoch: 10,
        };
        let mut recovered = make_session("good", next_identity);
        recovered.push(&raw).unwrap();
        let selection = recovered.finish().unwrap();
        assert_eq!(selection.identity, next_identity);
        assert_eq!(selection.kind, SelectionKind::Filtered);
        assert_eq!(selection_samples(&selection), raw);
    }

    #[test]
    fn partial_maximum_frame_and_slow_bytes_cannot_extend_the_stop_deadline() {
        let _case = serialize_test_case();
        let raw = [0.33];
        let mut partial = make_session(
            "partial_max",
            StreamIdentity {
                session_id: 18,
                audio_epoch: 11,
            },
        );
        partial.push(&raw).unwrap();
        let started = Instant::now();
        let partial_selection = partial.finish().unwrap();
        assert_raw(partial_selection, FallbackReason::FinishTimeout, &raw);
        assert!(started.elapsed() <= MAX_FINISH_BUDGET + Duration::from_millis(40));

        let mut slow = make_session(
            "slow_ready",
            StreamIdentity {
                session_id: 19,
                audio_epoch: 12,
            },
        );
        slow.push(&raw).unwrap();
        let started = Instant::now();
        let slow_selection = slow.finish().unwrap();
        assert_raw(slow_selection, FallbackReason::FinishTimeout, &raw);
        assert!(started.elapsed() <= MAX_FINISH_BUDGET + Duration::from_millis(40));
    }

    #[test]
    fn descendant_holding_inherited_pipes_does_not_block_kill_and_reap() {
        let _case = serialize_test_case();
        let raw = [0.44];
        let mut session = make_session(
            "descendant",
            StreamIdentity {
                session_id: 20,
                audio_epoch: 13,
            },
        );
        session.push(&raw).unwrap();
        let started = Instant::now();
        let selection = session.finish().unwrap();
        let child_reaped = selection.child_reaped;
        assert_raw(selection, FallbackReason::FinishTimeout, &raw);
        assert!(child_reaped);
        assert!(started.elapsed() <= MAX_FINISH_BUDGET + Duration::from_millis(40));
    }

    #[test]
    fn child_output_pipe_flood_is_bounded_and_rejected_without_partial_selection() {
        let _case = serialize_test_case();
        let raw = [0.55];
        let mut session = make_session(
            "flood",
            StreamIdentity {
                session_id: 21,
                audio_epoch: 14,
            },
        );
        session.push(&raw).unwrap();
        let selection = session.finish().unwrap();
        assert_raw(selection, FallbackReason::NonContiguousOutput, &raw);
    }

    #[test]
    fn audio_buffer_coalesces_tiny_appends_and_refuses_overflow_atomically() {
        let mut buffer = AudioBuffer::default();
        let max_samples = 2 * MAX_AUDIO_FRAME_SAMPLES + 3;
        for sample in 0..max_samples {
            buffer
                .append(&[sample as f32], max_samples)
                .expect("each sample remains within the cap");
        }

        assert_eq!(buffer.len(), max_samples);
        assert_eq!(buffer.blocks.len(), 3);
        assert_eq!(buffer.iter().copied().count(), max_samples);
        assert_eq!(buffer.iter().next(), Some(&0.0));
        assert_eq!(buffer.iter().last(), Some(&((max_samples - 1) as f32)));

        assert_eq!(
            buffer.append(&[99.0], max_samples),
            Err(FallbackReason::RawTimelineLimit)
        );
        assert_eq!(buffer.len(), max_samples);
        assert_eq!(buffer.blocks.len(), 3);
    }

    #[test]
    fn existing_helper_adapter_keeps_model_profile_and_epoch_identity_in_begin_ready() {
        let model = [0x11; 32];
        let profile_digest = [0x22; 32];
        let profile = vec![7, 8, 9];
        let adapter =
            ExistingHelperProtocol::new(model, profile_digest, profile.clone(), 1).unwrap();
        let identity = StreamIdentity {
            session_id: 31,
            audio_epoch: 42,
        };
        let begin = adapter.begin_payload(identity).unwrap();
        assert_eq!(begin[0], 1);
        assert_eq!(&begin[1..9], &31_u64.to_be_bytes());
        assert_eq!(&begin[9..17], &42_u64.to_be_bytes());
        assert_eq!(&begin[18..50], &model);
        assert_eq!(&begin[50..82], &profile_digest);
        assert_eq!(
            u32::from_be_bytes(begin[82..86].try_into().unwrap()) as usize,
            profile.len()
        );
        assert_eq!(&begin[86..], &profile);
        assert_eq!(adapter.expected_ready_proof().len(), 65);
    }
}
