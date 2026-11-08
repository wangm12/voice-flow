//! Microphone capture and crash-safe audio spooling.
use crate::chunker::{AudioChunk, Chunker, ChunkerConfig};
use crate::prefetch_asr::{PrefetchInbox, PrefetchMessage};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::StreamInstant;
use rubato::Resampler;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};
#[cfg(all(feature = "research-harness", target_os = "macos"))]
use std::{io::Read, path::Path};
use tauri::{AppHandle, Emitter, Manager};
use thiserror::Error;

const TARGET_RATE: u32 = 16_000;
const LEVEL_EMIT_INTERVAL: Duration = Duration::from_millis(33);
const CAPTURE_FRAME_FRESHNESS: Duration = Duration::from_millis(750);
const SPOOL_CHUNK_SAMPLES: usize = TARGET_RATE as usize;
const CAPTURE_CLOSE_BUDGET: Duration = Duration::from_millis(200);
const CAPTURE_CONTROL_POLL: Duration = Duration::from_millis(10);
const SPOOL_CONTROL_POLL: Duration = Duration::from_millis(20);
const CANCEL_SPOOL_CLOSE_BUDGET: Duration = Duration::from_millis(200);
const SPOOL_OWNER_QUEUE_CAPACITY: usize = 16;
// Includes queued and currently executing payloads. A 15 minute 16 kHz PCM16
// WAV is about 28.8 MB; the remaining room allows a bounded tail of f32 chunks.
pub(crate) const SPOOL_OWNER_QUEUE_BYTES: usize = 40 * 1024 * 1024;
const AUDIO_SPOOL_OWNER_CAPACITY: usize = 3;
const RETAINED_STORE_OWNER_CAPACITY: usize = 3;
/// A 15-minute 16 kHz mono PCM16 WAV is about 28.8 MB; leave small envelope
/// and header room while keeping retry reads below the retained-job byte cap.
pub(crate) const MAX_HISTORY_RETRY_WAV_BYTES: usize = 30 * 1024 * 1024;
const SPOOL_STORE_JOB_WAIT: Duration = Duration::from_secs(5);
const SPOOL_SCOPE_BUSY_RETRY: Duration = Duration::from_secs(30);
pub(crate) const OWNED_WAV_SOURCE_SHA256: &str =
    "424ba44bbb4d54d95f7b2c0c6bb24fa46daebd05a12e592be80559510d469563";
const OWNED_WAV_SHA256: &str = OWNED_WAV_SOURCE_SHA256;
#[cfg(any(test, all(feature = "research-harness", target_os = "macos")))]
const OWNED_WAV_MAX_BYTES: u64 = 1024 * 1024;
#[cfg(any(test, all(feature = "research-harness", target_os = "macos")))]
const OWNED_WAV_MAX_SECONDS: usize = 30;
#[cfg(all(feature = "research-harness", target_os = "macos"))]
const OWNED_WAV_REPLAY_CHUNK_SAMPLES: usize = 320;
/// Hard safety ceiling for a single recording. The in-memory chunker needs a
/// bounded sample buffer; keeping this at 15 minutes prevents an unattended
/// hotkey from consuming hundreds of megabytes.
pub const MAX_RECORDING_SECS: usize = 15 * 60;

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("no input device is available")]
    NoDevice,
    #[error("microphone device error: {0}")]
    Device(String),
    #[error("microphone device error: {0}")]
    ClosedLidBuiltInInput(&'static str),
    #[error("recording is not active")]
    NotRecording,
    #[error("audio encoding failed: {0}")]
    Encode(String),
    #[error("No speech detected")]
    EmptyRecording,
}

#[derive(Debug, Clone, Serialize)]
pub struct InputDeviceInfo {
    pub name: String,
    pub is_default: bool,
}

pub(crate) struct OwnedWavReplay {
    #[cfg(all(feature = "research-harness", target_os = "macos"))]
    samples: std::sync::Arc<[i16]>,
}

impl OwnedWavReplay {
    #[cfg(all(feature = "research-harness", target_os = "macos"))]
    pub(crate) fn from_path(path: &Path) -> Result<Self, &'static str> {
        if !path.is_absolute() {
            return Err("path_not_absolute");
        }
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| "source_unavailable")?;
        let metadata = file.metadata().map_err(|_| "source_unavailable")?;
        if !metadata.is_file() || metadata.len() > OWNED_WAV_MAX_BYTES {
            return Err("source_size_or_type_invalid");
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(OWNED_WAV_MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "source_unavailable")?;
        if bytes.len() as u64 > OWNED_WAV_MAX_BYTES {
            return Err("source_size_or_type_invalid");
        }
        let samples = decode_owned_wav_bytes(&bytes, OWNED_WAV_SHA256)?;
        Ok(Self {
            samples: samples.into(),
        })
    }
}

#[cfg(any(test, all(feature = "research-harness", target_os = "macos")))]
fn decode_owned_wav_bytes(bytes: &[u8], expected_sha256: &str) -> Result<Vec<i16>, &'static str> {
    use sha2::Digest;
    if bytes.is_empty() || bytes.len() as u64 > OWNED_WAV_MAX_BYTES {
        return Err("source_size_or_type_invalid");
    }
    if hex::encode(sha2::Sha256::digest(bytes)) != expected_sha256 {
        return Err("source_identity_mismatch");
    }
    let mut reader =
        hound::WavReader::new(std::io::Cursor::new(bytes)).map_err(|_| "source_format_invalid")?;
    let spec = reader.spec();
    if spec.channels != 1
        || spec.sample_rate != TARGET_RATE
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
    {
        return Err("source_format_invalid");
    }
    let samples = reader
        .samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "source_format_invalid")?;
    if samples.is_empty() || samples.len() > TARGET_RATE as usize * OWNED_WAV_MAX_SECONDS {
        return Err("source_duration_invalid");
    }
    Ok(samples)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OwnedReplayLifecycleEvent {
    pub(crate) phase: String,
    pub(crate) source_identity: String,
    pub(crate) stream_id: u64,
    pub(crate) session_generation: u64,
    pub(crate) frame_count: u64,
    pub(crate) sample_count: u64,
    pub(crate) acknowledged: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OwnedReplayStartReceipt {
    session_generation: u64,
    stream_id: u64,
    source_identity: &'static str,
}

impl OwnedReplayStartReceipt {
    pub(crate) fn session_generation(self) -> u64 {
        self.session_generation
    }

    #[cfg(any(test, all(feature = "research-harness", target_os = "macos")))]
    pub(crate) fn confirms_eof(self, event: &OwnedReplayLifecycleEvent) -> bool {
        event.phase == "eof"
            && event.acknowledged
            && event.session_generation == self.session_generation
            && event.stream_id == self.stream_id
            && event.source_identity == format!("sha256:{}", self.source_identity)
            && event.frame_count > 0
            && event.sample_count > 0
    }

    #[cfg(test)]
    pub(crate) fn test_fixture(session_generation: u64, stream_id: u64) -> Self {
        Self {
            session_generation,
            stream_id,
            source_identity: OWNED_WAV_SOURCE_SHA256,
        }
    }
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
pub(crate) struct OwnedReplayStarted {
    pub(crate) receipt: OwnedReplayStartReceipt,
    pub(crate) completion: mpsc::Receiver<OwnedReplayLifecycleEvent>,
}

struct OwnedReplayProgress {
    frame_count: AtomicU64,
    sample_count: AtomicU64,
}

impl OwnedReplayProgress {
    fn new() -> Self {
        Self {
            frame_count: AtomicU64::new(0),
            sample_count: AtomicU64::new(0),
        }
    }
}

pub fn list_input_devices() -> Result<Vec<InputDeviceInfo>, String> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|device| device.name().ok());
    let devices = host
        .input_devices()
        .map_err(|error| format!("could not enumerate input devices: {error}"))?;
    let mut names = Vec::new();
    for device in devices {
        let Ok(name) = device.name() else {
            continue;
        };
        if !names.iter().any(|known: &String| known == &name) {
            names.push(name);
        }
    }
    Ok(names
        .into_iter()
        .map(|name| InputDeviceInfo {
            is_default: default_name.as_deref() == Some(name.as_str()),
            name,
        })
        .collect())
}

pub fn selected_input_device_name(selection: &str) -> Result<String, String> {
    let host = cpal::default_host();
    resolve_input_device(&host, selection)
        .and_then(|device| {
            device
                .name()
                .map_err(|error| AudioError::Device(error.to_string()))
        })
        .map_err(|error| error.to_string())
}

enum EngineCmd {
    StartMicCheck {
        app: AppHandle,
        session_id: String,
        input_device: String,
        input_gain: f32,
        reply: mpsc::Sender<Result<crate::microphone_check::Status, String>>,
    },
    StopMicCheck {
        session_id: String,
        reply: mpsc::Sender<crate::microphone_check::StopReceipt>,
    },
    CloseMicCheck,
    Start {
        app: AppHandle,
        session: String,
        session_generation: u64,
        input_device: String,
        requested_selection: String,
        clamshell_substituted: bool,
        chunk_length_secs: usize,
        max_recording_secs: usize,
        input_gain: f32,
        prefetch_tx: PrefetchInbox,
        soniox_audio: Option<crate::soniox::SonioxAudioSender>,
        preserve_full_audio: bool,
        owned_wav: Option<OwnedWavReplay>,
        readiness: Option<mpsc::Sender<()>>,
        reply: mpsc::Sender<Result<StartHandle, AudioError>>,
    },
    #[cfg(all(feature = "research-harness", target_os = "macos"))]
    BeginOwnedSource {
        session_generation: u64,
        completion: mpsc::Sender<OwnedReplayLifecycleEvent>,
        reply: mpsc::Sender<Result<u64, String>>,
    },
    #[cfg(all(feature = "research-harness", target_os = "macos"))]
    OwnedSourceEof {
        stream_id: u64,
        session_generation: u64,
        completion: mpsc::Sender<OwnedReplayLifecycleEvent>,
    },
    ConfigureWarm {
        app: AppHandle,
        enabled: bool,
        input_device: String,
    },
    Stop {
        options: StopOptions,
        reply: StopReply,
    },
    Cancel {
        reply: mpsc::Sender<()>,
    },
    AutoStop {
        app: AppHandle,
    },
    StreamError {
        stream_id: u64,
        app: AppHandle,
        message: String,
    },
    CaptureError {
        stream_id: u64,
        session_generation: u64,
        app: AppHandle,
        message: String,
    },
    CaptureFrameObserved {
        stream_id: u64,
        session_generation: u64,
    },
}

type StopResult = Result<FinishedCapture, AudioError>;
type StopReply = mpsc::Sender<StopResult>;

pub(crate) struct FinishedCapture {
    pub(crate) wav: Vec<u8>,
    pub(crate) chunks: Vec<AudioChunk>,
    pub(crate) capture_spool: Option<CaptureSpoolLease>,
    pub(crate) input_selection: String,
    pub(crate) actual_device_name: Option<String>,
    pub(crate) device_metadata: CaptureDeviceMetadata,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CaptureDeviceMetadata {
    pub(crate) requested_selection: Option<String>,
    pub(crate) clamshell_substituted: bool,
    pub(crate) system_default_name: Option<String>,
}

impl FinishedCapture {
    #[cfg(test)]
    pub(crate) fn without_spool(wav: Vec<u8>, chunks: Vec<AudioChunk>) -> Self {
        Self {
            wav,
            chunks,
            capture_spool: None,
            input_selection: "".to_owned(),
            actual_device_name: None,
            device_metadata: CaptureDeviceMetadata::default(),
        }
    }
}

/// Transfers the live capture-spool owner to the retained History job.
/// That handoff marks the source recoverable before bounded-worker admission,
/// so admission failure or a lost caller cannot delete accepted raw samples.
/// Only a History ACK or an explicitly known no-side-effect path requests
/// completed cleanup.
pub(crate) struct CaptureSpoolLease {
    session_dir: PathBuf,
    session_id: String,
    finalizer: Option<SpoolFinalizer>,
    tail_flush: Option<CaptureSpoolFlushReceipt>,
    recover_on_drop: bool,
}

impl CaptureSpoolLease {
    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }

    pub(crate) fn recovery_audio_path(&self) -> PathBuf {
        self.session_dir.join("recovery.wav")
    }

    pub(crate) fn preserve_on_drop(&mut self) {
        self.recover_on_drop = true;
    }

    pub(crate) fn try_wait_for_durable_tail(&mut self) -> Option<(Duration, Result<(), String>)> {
        self.tail_flush.as_mut()?.try_wait()
    }

    pub(crate) fn finish(mut self, disposition: SpoolDisposition) -> Result<(), String> {
        let Some(finalizer) = self.finalizer.take() else {
            return Ok(());
        };
        finalizer.resolve(disposition);
        finalizer.wait_closed_blocking()
    }

    pub(crate) fn request_finish(mut self, disposition: SpoolDisposition) {
        if let Some(finalizer) = self.finalizer.take() {
            finalizer.resolve(disposition);
        }
    }
}

impl Drop for CaptureSpoolLease {
    fn drop(&mut self) {
        if let Some(finalizer) = self.finalizer.take() {
            finalizer.resolve(if self.recover_on_drop {
                SpoolDisposition::Recoverable
            } else {
                SpoolDisposition::Completed
            });
        }
    }
}

#[derive(Clone)]
pub(crate) struct StopOptions {
    pub extra_ms: u64,
    pub vad_enabled: bool,
    pub cancellation: tokio_util::sync::CancellationToken,
}
impl Default for StopOptions {
    fn default() -> Self {
        Self {
            extra_ms: 250,
            vad_enabled: false,
            cancellation: tokio_util::sync::CancellationToken::new(),
        }
    }
}
pub(crate) struct StartHandle {
    session_generation: u64,
    cue: Option<CaptureCueHandle>,
}

impl StartHandle {
    fn feedback_cue_handle(&self, session_generation: u64) -> Option<CaptureCueHandle> {
        (self.session_generation == session_generation)
            .then(|| self.cue.clone())
            .flatten()
    }
}

struct CallbackSession {
    state: CapturePushState,
    recycle_rx: mpsc::Receiver<Vec<f32>>,
    readiness: Option<mpsc::Sender<()>>,
}
type SessionSlot = Arc<Mutex<Option<CallbackSession>>>;
struct IdleStream {
    stream_id: u64,
    // These keep the CPAL callback and its synchronized state alive while warmed.
    _stream: cpal::Stream,
    _slot: SessionSlot,
    input_selection: String,
}
struct WarmConfig {
    app: AppHandle,
    enabled: bool,
    input_device: String,
}

pub fn configure_warm(app: AppHandle, enabled: bool, input_device: String) {
    let _ = engine().tx.send(EngineCmd::ConfigureWarm {
        app,
        enabled,
        input_device,
    });
}

struct Engine {
    tx: mpsc::Sender<EngineCmd>,
}
fn engine() -> &'static Engine {
    static ENGINE: std::sync::OnceLock<Engine> = std::sync::OnceLock::new();
    ENGINE.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || engine_loop(rx));
        Engine { tx }
    })
}

/// Streaming resampler that converts the device's native sample rate to
/// 16 kHz as audio arrives. Uses rubato's sinc resampler (with anti-aliasing)
/// for high-quality output; falls back to passing samples through unchanged
/// when the input is already 16 kHz.
struct StreamResampler {
    resampler: Option<rubato::SincFixedIn<f32>>,
    /// Staging buffer that accumulates mono input frames until we have a full
    /// resampler chunk.
    pending: Vec<f32>,
    chunk_size: usize,
    from_rate: u32,
    source_samples_seen: u64,
    output_samples_returned: usize,
    generated_pending: VecDeque<f32>,
}

impl StreamResampler {
    fn new(from_rate: u32) -> Result<Self, String> {
        if from_rate == TARGET_RATE {
            return Ok(Self {
                resampler: None,
                pending: Vec::new(),
                chunk_size: 0,
                from_rate,
                source_samples_seen: 0,
                output_samples_returned: 0,
                generated_pending: VecDeque::new(),
            });
        }
        let chunk_size = 1024;
        let params = rubato::SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            oversampling_factor: 128,
            interpolation: rubato::SincInterpolationType::Linear,
            window: rubato::WindowFunction::BlackmanHarris2,
        };
        let ratio = TARGET_RATE as f64 / from_rate as f64;
        let resampler = rubato::SincFixedIn::new(ratio, 1.0, params, chunk_size, 1)
            .map_err(|error| format!("failed to initialize audio resampler: {error}"))?;
        Ok(Self {
            resampler: Some(resampler),
            pending: Vec::with_capacity(chunk_size * 2),
            chunk_size,
            from_rate,
            source_samples_seen: 0,
            output_samples_returned: 0,
            generated_pending: VecDeque::new(),
        })
    }

    /// Feed native-rate mono samples; returns newly available 16 kHz samples.
    fn push(&mut self, input: &[f32]) -> Result<Vec<f32>, String> {
        self.source_samples_seen = self
            .source_samples_seen
            .checked_add(input.len() as u64)
            .ok_or_else(|| "audio source sample count overflowed".to_owned())?;
        let Some(resampler) = self.resampler.as_mut() else {
            return Ok(input.to_vec());
        };
        self.pending.extend_from_slice(input);
        let mut out = Vec::new();
        while self.pending.len() >= self.chunk_size {
            let block: Vec<f32> = self.pending.drain(..self.chunk_size).collect();
            let channels_in = [block];
            let processed = resampler
                .process(&channels_in, None)
                .map_err(|error| format!("audio resampling failed: {error}"))?;
            let chan = processed
                .into_iter()
                .next()
                .ok_or_else(|| "audio resampler returned no output channel".to_owned())?;
            self.generated_pending.extend(chan);
        }
        let safe_count = expected_resampled_samples(self.source_samples_seen, self.from_rate);
        let releasable = safe_count
            .saturating_sub(self.output_samples_returned)
            .min(self.generated_pending.len());
        out.extend(self.generated_pending.drain(..releasable));
        self.output_samples_returned = self.output_samples_returned.saturating_add(releasable);
        Ok(out)
    }

    /// Flush any remaining samples at end of recording.
    fn finish(&mut self) -> Result<Vec<f32>, String> {
        let expected = expected_resampled_samples(self.source_samples_seen, self.from_rate);
        let Some(resampler) = self.resampler.as_mut() else {
            return Ok(Vec::new());
        };
        let tail = std::mem::take(&mut self.pending);
        let channels_in = [tail];
        let processed = if channels_in[0].is_empty() {
            resampler.process_partial::<Vec<f32>>(None, None)
        } else {
            resampler.process_partial(Some(&channels_in), None)
        }
        .map_err(|error| format!("audio resampling failed while finishing: {error}"))?;
        let chan = processed
            .into_iter()
            .next()
            .ok_or_else(|| "audio resampler returned no output channel".to_owned())?;
        self.generated_pending.extend(chan);

        // Rubato keeps sinc lookahead in its internal buffer. A short final
        // block is zero-padded by process_partial; also issue zero blocks when
        // the source ended exactly on a chunk boundary so the filter's valid
        // tail reaches the source-duration sample count.
        let needed = expected.saturating_sub(self.output_samples_returned);
        let mut flushes = 0;
        while self.generated_pending.len() < needed && flushes < 8 {
            let processed = resampler
                .process_partial::<Vec<f32>>(None, None)
                .map_err(|error| format!("audio resampler tail flush failed: {error}"))?;
            let Some(chan) = processed.into_iter().next() else {
                return Err("audio resampler returned no output channel while flushing".into());
            };
            if chan.is_empty() {
                return Err("audio resampler produced no tail samples while flushing".into());
            }
            self.generated_pending.extend(chan);
            flushes += 1;
        }
        if self.generated_pending.len() < needed {
            return Err("audio resampler did not flush the complete source interval".into());
        }
        let out = self.generated_pending.drain(..needed).collect::<Vec<_>>();
        self.output_samples_returned = self.output_samples_returned.saturating_add(out.len());
        Ok(out)
    }
}

fn expected_resampled_samples(source_samples: u64, from_rate: u32) -> usize {
    if from_rate == TARGET_RATE {
        return usize::try_from(source_samples).unwrap_or(usize::MAX);
    }
    let numerator = u128::from(source_samples) * u128::from(TARGET_RATE);
    let rounded = (numerator + u128::from(from_rate / 2)) / u128::from(from_rate);
    usize::try_from(rounded).unwrap_or(usize::MAX)
}

struct ActiveRec {
    stream_id: u64,
    session_generation: u64,
    session_id: String,
    stream: Option<cpal::Stream>,
    slot: SessionSlot,
    input_selection: String,
    actual_device_name: Option<String>,
    actual_device_identity: Option<crate::clamshell::CoreAudioDeviceIdentity>,
    device_metadata: CaptureDeviceMetadata,
    rate: u32,
    channels: usize,
    device_failed: bool,
    first_frame_seen: bool,
    no_frame_reported: bool,
    started_at: std::time::Instant,
    last_frame_elapsed_ms: Arc<AtomicU64>,
    freshness: CaptureFreshness,
    capture: CaptureWorker,
    cue: Option<CaptureCueHandle>,
    chunk_length_secs: usize,
    app: AppHandle,
    level: Arc<AtomicU32>,
    preserve_full_audio: bool,
    owned_source: Option<OwnedWavReplay>,
    replay_progress: Option<Arc<OwnedReplayProgress>>,
    replay_cancel: Option<Arc<AtomicBool>>,
}

struct PendingSpool {
    samples: Vec<f32>,
    next_index: usize,
}

enum SpoolCommand {
    Samples {
        index: usize,
        start_secs: f32,
        end_secs: f32,
        source_start_sample: u64,
        samples: Vec<f32>,
    },
    StoreJob {
        reserved_bytes: usize,
        run: Box<dyn FnOnce(&crate::store::AudioSpoolSessionLease) + Send>,
    },
    TailBarrier {
        reply: mpsc::SyncSender<Result<(), String>>,
    },
}

struct CaptureSpoolFlushReceipt {
    receiver: Option<mpsc::Receiver<Result<(), String>>>,
    enqueued_at: std::time::Instant,
}

impl CaptureSpoolFlushReceipt {
    fn failed(error: String) -> Self {
        let (reply, receiver) = mpsc::sync_channel(1);
        let _ = reply.send(Err(error));
        Self {
            receiver: Some(receiver),
            enqueued_at: std::time::Instant::now(),
        }
    }

    fn try_wait(&mut self) -> Option<(Duration, Result<(), String>)> {
        let receiver = self.receiver.as_ref()?;
        match receiver.try_recv() {
            Ok(result) => {
                self.receiver.take();
                Some((self.enqueued_at.elapsed(), result))
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.receiver.take();
                Some((
                    self.enqueued_at.elapsed(),
                    Err("capture spool owner ended before durable tail acknowledgement".into()),
                ))
            }
        }
    }

    #[cfg(test)]
    fn wait(mut self) -> (Duration, Result<(), String>) {
        let started = self.enqueued_at;
        let result = self
            .receiver
            .take()
            .expect("tail receipt is consumed once")
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|error| {
                Err(format!(
                    "capture spool tail acknowledgement did not arrive: {error}"
                ))
            });
        (started.elapsed(), result)
    }
}

enum CaptureCommand {
    Samples(Vec<f32>),
}

enum CaptureControl {
    Finish { reply: mpsc::Sender<CaptureResult> },
    Cancel,
}

struct CaptureResult {
    samples: Vec<f32>,
    processing_error: Option<String>,
    session_dir: PathBuf,
    spool_finalizer: SpoolFinalizer,
    tail_flush: CaptureSpoolFlushReceipt,
}

struct CaptureWorker {
    tx: mpsc::SyncSender<CaptureCommand>,
    control_tx: mpsc::SyncSender<CaptureControl>,
    recycle_rx: Option<mpsc::Receiver<Vec<f32>>>,
    overflowed: Arc<AtomicBool>,
    continuity: Arc<Mutex<CaptureContinuityTracker>>,
    stale_gap_latched: Arc<AtomicBool>,
    closed_lid_latched: Arc<AtomicBool>,
    rate_drift_latched: Arc<AtomicBool>,
    rate_drift: Arc<Mutex<RateDriftTracker>>,
    limit_reached: Arc<AtomicBool>,
    started_at: std::time::Instant,
    session_id: String,
    spool_control: SpoolControl,
    terminal_requested: bool,
}

struct CapturePushState {
    stream_id: u64,
    session_generation: u64,
    first_frame_reported: Arc<AtomicBool>,
    started_at: std::time::Instant,
    ui_started_at: std::time::Instant,
    last_frame_elapsed_ms: Arc<AtomicU64>,
    continuity: Arc<Mutex<CaptureContinuityTracker>>,
    cue_handle: Option<CaptureCueHandle>,
    stale_gap_latched: Arc<AtomicBool>,
    rate_drift: Arc<Mutex<RateDriftTracker>>,
    rate_drift_latched: Arc<AtomicBool>,
    native_rate: u32,
    capture_tx: mpsc::SyncSender<CaptureCommand>,
    capture_overflowed: Arc<AtomicBool>,
    capture_limit_reached: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    app: AppHandle,
    engine_tx: mpsc::Sender<EngineCmd>,
}

const CAPTURE_RATE_DRIFT_LIMIT: Duration = Duration::from_millis(750);

#[derive(Default)]
struct RateDriftTracker {
    enabled: bool,
    baseline: Option<Duration>,
    accepted_frames_after_baseline: u64,
    direction: Option<i8>,
    sustained_since: Option<Duration>,
    latched: bool,
}

impl RateDriftTracker {
    fn accepted(&mut self, elapsed: Duration, frames: usize, rate: u32) -> bool {
        if self.latched || !self.enabled || rate == 0 {
            return self.latched;
        }
        let Some(baseline) = self.baseline else {
            self.baseline = Some(elapsed);
            // The first accepted block establishes the baseline and is excluded.
            return false;
        };
        self.accepted_frames_after_baseline = self
            .accepted_frames_after_baseline
            .saturating_add(frames as u64);
        let actual = elapsed.saturating_sub(baseline);
        let source =
            Duration::from_secs_f64(self.accepted_frames_after_baseline as f64 / rate as f64);
        let (direction, error) = if source > actual {
            (1, source - actual)
        } else {
            (-1, actual - source)
        };
        if error <= CAPTURE_RATE_DRIFT_LIMIT {
            self.direction = None;
            self.sustained_since = None;
            return false;
        }
        if self.direction != Some(direction) {
            self.direction = Some(direction);
            self.sustained_since = Some(elapsed);
            return false;
        }
        if self
            .sustained_since
            .is_some_and(|since| elapsed.saturating_sub(since) > CAPTURE_RATE_DRIFT_LIMIT)
        {
            self.latched = true;
        }
        self.latched
    }
}

fn observe_rate_drift(
    tracker: &Mutex<RateDriftTracker>,
    latched: &AtomicBool,
    elapsed: Duration,
    source_frames: usize,
    native_rate: u32,
    report: impl FnOnce(),
) -> bool {
    let drift = tracker.lock().unwrap_or_else(|e| e.into_inner()).accepted(
        elapsed,
        source_frames,
        native_rate,
    );
    if drift && !latched.swap(true, Ordering::AcqRel) {
        report();
        true
    } else {
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureFreshness {
    Waiting,
    Recent,
    Stale,
}

#[derive(Default)]
struct CaptureContinuityTracker {
    last_accepted: Option<Duration>,
    stale_gap: Duration,
    closed: bool,
    accepted_native_frames: u64,
    cue_window: CueWindowState,
    cue_begin_time: Option<StreamInstant>,
    cue_end_time: Option<StreamInstant>,
    cue_mapped_start: Option<usize>,
    latest_packet: Option<CuePacketSpan>,
}

#[derive(Clone, Copy)]
struct CuePacketSpan {
    capture: StreamInstant,
    frames: usize,
    before: u64,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
struct CueWindowState {
    pending_start: Option<usize>,
    confirmed: Option<(usize, usize)>,
    frozen: bool,
}

/// Recording-owned feedback cue coordinates in the accepted native source.
/// This handle carries only the small continuity state, never capture buffers.
#[derive(Clone)]
pub(crate) struct CaptureCueHandle {
    continuity: Arc<Mutex<CaptureContinuityTracker>>,
    native_rate: u32,
}

impl CaptureCueHandle {
    #[cfg(test)]
    pub(crate) fn test_handle(native_rate: u32) -> Self {
        Self {
            continuity: Arc::new(Mutex::new(CaptureContinuityTracker::default())),
            native_rate,
        }
    }

    pub(crate) fn begin(&self) {
        let Some(now) = capture_clock_now() else {
            log::warn!("feedback cue clock unavailable; keeping raw audio");
            return;
        };
        self.begin_at(now);
    }

    fn begin_at(&self, now: StreamInstant) {
        let mut state = self.continuity.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed
            || state.cue_window.frozen
            || state.cue_window.pending_start.is_some()
            || state.cue_window.confirmed.is_some()
        {
            return;
        }
        // Hold fanout from the current producer cursor until a packet timestamp
        // identifies the exact start sample. This is only a cursor, not audio.
        state.cue_window.pending_start = Some(expected_resampled_samples(
            state.accepted_native_frames,
            self.native_rate,
        ));
        state.cue_begin_time = Some(now);
        state.cue_end_time = None;
        state.cue_mapped_start = None;
    }

    pub(crate) fn finish(&self, confirmed_success: bool) {
        if !confirmed_success {
            let mut state = self.continuity.lock().unwrap_or_else(|e| e.into_inner());
            if !state.cue_window.frozen {
                state.cue_window.pending_start = None;
                state.cue_begin_time = None;
                state.cue_end_time = None;
                state.cue_mapped_start = None;
            }
            return;
        }
        let Some(now) = capture_clock_now() else {
            log::warn!("feedback cue end clock unavailable; keeping raw audio");
            let mut state = self.continuity.lock().unwrap_or_else(|e| e.into_inner());
            state.cue_window.pending_start = None;
            state.cue_begin_time = None;
            state.cue_end_time = None;
            state.cue_mapped_start = None;
            return;
        };
        self.finish_at(now);
    }

    fn finish_at(&self, now: StreamInstant) {
        let mut state = self.continuity.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed || state.cue_window.frozen || state.cue_begin_time.is_none() {
            return;
        }
        state.cue_end_time = Some(now);
        if let Some(packet) = state.latest_packet {
            self.map_packet_locked(&mut state, packet);
        }
    }

    fn accept_packet_locked(
        &self,
        state: &mut CaptureContinuityTracker,
        capture: StreamInstant,
        frames: usize,
        before: u64,
    ) {
        let packet = CuePacketSpan {
            capture,
            frames,
            before,
        };
        state.latest_packet = Some(packet);
        self.map_packet_locked(state, packet);
    }

    fn map_packet_locked(&self, state: &mut CaptureContinuityTracker, packet: CuePacketSpan) {
        if state.cue_window.frozen || state.cue_begin_time.is_none() {
            return;
        }
        let capture = packet.capture;
        let frames = packet.frames;
        let before = packet.before;
        let Some(packet_duration) = frames_to_duration_checked(frames, self.native_rate) else {
            state.cue_window.pending_start = None;
            state.cue_begin_time = None;
            log::warn!("feedback cue packet mapping failed; keeping raw audio");
            return;
        };
        let Some(packet_end) = capture.add(packet_duration) else {
            state.cue_window.pending_start = None;
            state.cue_begin_time = None;
            log::warn!("feedback cue packet timestamp overflow; keeping raw audio");
            return;
        };
        let Some(begin) = state.cue_begin_time else {
            return;
        };
        let start_clip = if begin < capture { capture } else { begin };
        if start_clip <= packet_end && state.cue_mapped_start.is_none() && start_clip >= capture {
            if let Some(offset) = start_clip.duration_since(&capture) {
                let native_offset = duration_to_frames_floor(offset, self.native_rate)
                    .min(u64::try_from(frames).unwrap_or(u64::MAX));
                state.cue_mapped_start = Some(expected_resampled_samples(
                    before.saturating_add(native_offset),
                    self.native_rate,
                ));
            }
        }
        if let (Some(end), Some(start)) = (state.cue_end_time, state.cue_mapped_start) {
            if end <= packet_end {
                let native_offset = if end <= capture {
                    0
                } else {
                    let Some(offset) = end.duration_since(&capture) else {
                        log::warn!("feedback cue packet end mapping failed; keeping raw audio");
                        state.cue_window.pending_start = None;
                        state.cue_begin_time = None;
                        state.cue_end_time = None;
                        state.cue_mapped_start = None;
                        return;
                    };
                    duration_to_frames_ceil(offset, self.native_rate)
                        .min(u64::try_from(frames).unwrap_or(u64::MAX))
                };
                let end_sample = expected_resampled_samples(
                    before.saturating_add(native_offset as u64),
                    self.native_rate,
                );
                state.cue_window.confirmed = Some((start, end_sample.max(start)));
                state.cue_window.pending_start = None;
                state.cue_begin_time = None;
                state.cue_end_time = None;
                state.cue_mapped_start = None;
            }
        }
    }

    fn snapshot(&self) -> CueWindowState {
        self.continuity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .cue_window
    }

    #[cfg(test)]
    pub(crate) fn test_observation(&self) -> (Option<usize>, Option<(usize, usize)>) {
        let state = self.snapshot();
        (state.pending_start, state.confirmed)
    }

    fn freeze(&self) -> CueWindowState {
        let mut state = self.continuity.lock().unwrap_or_else(|e| e.into_inner());
        state.cue_window.pending_start = None;
        state.cue_window.frozen = true;
        state.cue_window
    }
}

#[cfg(target_os = "macos")]
fn capture_clock_now() -> Option<StreamInstant> {
    #[repr(C)]
    struct TimebaseInfo {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_absolute_time() -> u64;
        fn mach_timebase_info(info: *mut TimebaseInfo) -> i32;
    }
    let mut info = TimebaseInfo { numer: 0, denom: 0 };
    // SAFETY: mach_timebase_info writes one ABI-compatible struct; mach_absolute_time
    // returns the same host clock used by CPAL's macOS CoreAudio backend.
    let status = unsafe { mach_timebase_info(&mut info) };
    if status != 0 || info.numer == 0 || info.denom == 0 {
        return None;
    }
    let ticks = unsafe { mach_absolute_time() } as u128;
    let nanos = ticks.checked_mul(u128::from(info.numer))? / u128::from(info.denom);
    let secs = i64::try_from(nanos / 1_000_000_000).ok()?;
    let subsec = (nanos % 1_000_000_000) as u32;
    Some(StreamInstant::new(secs, subsec))
}

#[cfg(not(target_os = "macos"))]
fn capture_clock_now() -> Option<StreamInstant> {
    None
}

fn frames_to_duration_checked(frames: usize, rate: u32) -> Option<Duration> {
    if rate == 0 {
        return None;
    }
    let nanos = (frames as u128).checked_mul(1_000_000_000)? / u128::from(rate);
    Some(Duration::from_nanos(u64::try_from(nanos).ok()?))
}

fn duration_to_frames_floor(duration: Duration, rate: u32) -> u64 {
    (duration.as_nanos().saturating_mul(u128::from(rate)) / 1_000_000_000).min(u128::from(u64::MAX))
        as u64
}

fn duration_to_frames_ceil(duration: Duration, rate: u32) -> u64 {
    let scaled = duration.as_nanos().saturating_mul(u128::from(rate));
    (scaled.saturating_add(999_999_999) / 1_000_000_000).min(u128::from(u64::MAX)) as u64
}

fn mask_cue_range(samples: &mut [f32], range: Option<(usize, usize)>, source_start: usize) {
    let Some((start, end)) = range else {
        return;
    };
    let mask_start = start.saturating_sub(source_start).min(samples.len());
    let mask_end = end.saturating_sub(source_start).min(samples.len());
    if mask_start < mask_end {
        samples[mask_start..mask_end].fill(0.0);
    }
}

#[derive(Default)]
struct CaptureFanout {
    forwarded_cursor: usize,
}

struct CaptureFanoutBatch {
    source_start: usize,
    samples: Vec<f32>,
}

impl CaptureFanout {
    fn take_ready(
        &mut self,
        accepted_samples: &[f32],
        cue: CueWindowState,
        force_pending_fallback: bool,
    ) -> Option<CaptureFanoutBatch> {
        let ready_end = match cue.pending_start {
            Some(start) if !force_pending_fallback => start.min(accepted_samples.len()),
            _ => accepted_samples.len(),
        };
        if ready_end <= self.forwarded_cursor {
            return None;
        }
        let source_start = self.forwarded_cursor;
        let mut samples = accepted_samples[source_start..ready_end].to_vec();
        mask_cue_range(&mut samples, cue.confirmed, source_start);
        self.forwarded_cursor = ready_end;
        Some(CaptureFanoutBatch {
            source_start,
            samples,
        })
    }
}

#[cfg(test)]
mod capture_cue_tests {
    use super::{
        mask_cue_range, CaptureContinuityTracker, CaptureCueHandle, CaptureFanout, TARGET_RATE,
    };
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn start_cue_window_is_masked() {
        let mut samples = vec![1.0; 12];
        mask_cue_range(&mut samples, Some((3, 8)), 0);
        assert_eq!(
            samples,
            vec![1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]
        );
    }

    #[test]
    fn first_word_after_cue_is_kept() {
        let mut samples = vec![1.0; 12];
        mask_cue_range(&mut samples, Some((3, 8)), 0);
        assert_eq!(samples[8..], [1.0; 4]);
    }

    #[test]
    fn accepted_cursor_leads_worker_without_forwarding_pending_cue() {
        let continuity = Arc::new(Mutex::new(CaptureContinuityTracker::default()));
        let cue = CaptureCueHandle {
            continuity,
            native_rate: TARGET_RATE,
        };
        let instant = |millis: u32| super::StreamInstant::new(0, millis * 1_000_000);
        cue.begin_at(instant(20));
        let mut fanout = CaptureFanout::default();
        assert!(fanout.take_ready(&[], cue.snapshot(), false).is_none());
        cue.finish_at(instant(80));
        {
            let mut state = cue.continuity.lock().unwrap();
            cue.accept_packet_locked(&mut state, instant(0), 1_600, 0);
            state.accepted_native_frames = 1_600;
        }
        let mut source = vec![0.5; 1_600];
        let batch = fanout.take_ready(&source, cue.snapshot(), false).unwrap();
        assert_eq!(batch.source_start, 0);
        assert_eq!(batch.samples.len(), 1_600);
        assert!(batch.samples[..320].iter().all(|sample| *sample == 0.5));
        assert!(batch.samples[320..1_280]
            .iter()
            .all(|sample| *sample == 0.0));
        assert!(batch.samples[1_280..].iter().all(|sample| *sample == 0.5));
        super::mask_cue_range(&mut source, cue.snapshot().confirmed, 0);
        assert!(source[..320].iter().all(|sample| *sample == 0.5));
        assert!(source[320..1_280].iter().all(|sample| *sample == 0.0));
        assert!(source[1_280..].iter().all(|sample| *sample == 0.5));
    }

    #[test]
    fn cue_spanning_packets_resolves_from_last_packet_after_wait() {
        let cue = CaptureCueHandle::test_handle(TARGET_RATE);
        let instant = |millis: u32| super::StreamInstant::new(0, millis * 1_000_000);
        cue.begin_at(instant(20));
        {
            let mut state = cue.continuity.lock().unwrap();
            cue.accept_packet_locked(&mut state, instant(0), 800, 0);
            state.accepted_native_frames = 800;
            cue.accept_packet_locked(&mut state, instant(50), 800, 800);
            state.accepted_native_frames = 1_600;
        }
        // The process wait ends after the last packet callback; the bounded
        // latest-packet span still resolves the real [20ms, 80ms) intersection.
        cue.finish_at(instant(80));
        assert_eq!(cue.snapshot().confirmed, Some((320, 1_280)));
        let mut source = vec![0.75; 1_600];
        super::mask_cue_range(&mut source, cue.snapshot().confirmed, 0);
        assert!(source[..320].iter().all(|sample| *sample == 0.75));
        assert!(source[320..1_280].iter().all(|sample| *sample == 0.0));
        assert!(source[1_280..].iter().all(|sample| *sample == 0.75));
    }

    #[test]
    fn non_16khz_cursor_uses_production_resampler_output_and_tail() {
        let mut resampler = super::StreamResampler::new(44_100).unwrap();
        let first = resampler.push(&vec![0.25; 1_024]).unwrap();
        let tail = resampler.finish().unwrap();
        let total = first.len() + tail.len();
        assert_eq!(total, 372);
        assert!(first
            .iter()
            .chain(tail.iter())
            .all(|sample| sample.is_finite()));

        let cue = CaptureCueHandle::test_handle(44_100);
        cue.begin_at(super::StreamInstant::new(0, 20_000_000));
        cue.finish_at(super::StreamInstant::new(0, 80_000_000));
        {
            let mut state = cue.continuity.lock().unwrap();
            cue.accept_packet_locked(&mut state, super::StreamInstant::new(0, 0), 4_410, 0);
            state.accepted_native_frames = 4_410;
        }
        let range = cue.snapshot().confirmed;
        assert_eq!(range, Some((320, 1_280)));
        let mut mapped_resampler = super::StreamResampler::new(44_100).unwrap();
        let mut mapped_samples = mapped_resampler.push(&vec![0.25; 4_410]).unwrap();
        mapped_samples.extend(mapped_resampler.finish().unwrap());
        assert!(mapped_samples.len() >= 1_280);
        super::mask_cue_range(&mut mapped_samples, range, 0);
        assert!(mapped_samples[..320]
            .iter()
            .all(|sample| sample.is_finite()));
        assert!(mapped_samples[320..1_280]
            .iter()
            .all(|sample| *sample == 0.0));
        assert!(mapped_samples[1_280..]
            .iter()
            .all(|sample| sample.is_finite()));
    }

    #[test]
    fn stop_first_freezes_raw_fallback_against_late_success() {
        let cue = CaptureCueHandle::test_handle(TARGET_RATE);
        cue.begin_at(super::StreamInstant::new(0, 1));
        cue.continuity.lock().unwrap().close(Duration::ZERO);
        let stopped = cue.freeze();
        cue.finish(true);
        assert_eq!(cue.snapshot(), stopped);
        assert_eq!(stopped.confirmed, None);
        let next = CaptureCueHandle::test_handle(TARGET_RATE);
        next.begin_at(super::StreamInstant::new(0, 10));
        next.finish_at(super::StreamInstant::new(0, 20));
        {
            let mut state = next.continuity.lock().unwrap();
            next.accept_packet_locked(&mut state, super::StreamInstant::new(0, 0), 100, 0);
        }
        assert_eq!(next.snapshot().confirmed, Some((0, 1)));
        assert_eq!(cue.snapshot(), stopped);
    }
}

impl CaptureContinuityTracker {
    fn accepted_frame(&mut self, elapsed: Duration) -> bool {
        if self.closed {
            return self.stale_gap > Duration::ZERO;
        }
        if let Some(previous) = self.last_accepted {
            let gap = elapsed.saturating_sub(previous);
            self.stale_gap += gap.saturating_sub(CAPTURE_FRAME_FRESHNESS);
        }
        self.last_accepted = Some(elapsed);
        self.stale_gap > Duration::ZERO
    }

    fn close(&mut self, elapsed: Duration) -> bool {
        if !self.closed {
            if let Some(previous) = self.last_accepted {
                let gap = elapsed.saturating_sub(previous);
                self.stale_gap += gap.saturating_sub(CAPTURE_FRAME_FRESHNESS);
            }
            self.closed = true;
            // Stop-first freezes an unfinished optional cue at the same
            // admission boundary; a waiter cannot revise this owner later.
            if self.cue_window.pending_start.take().is_some() {
                log::warn!("feedback cue ended after capture admission closed; keeping raw audio");
            }
            self.cue_window.frozen = true;
        }
        self.stale_gap > Duration::ZERO
    }
}

impl CaptureWorker {
    const QUEUE_CAPACITY: usize = 64;

    #[allow(clippy::too_many_arguments)]
    fn new(
        root: PathBuf,
        session_dir: PathBuf,
        spool_lease: crate::store::AudioSpoolSessionLease,
        input_rate: u32,
        monitor_rate_drift: bool,
        max_samples: usize,
        chunk_length_secs: usize,
        input_gain: f32,
        prefetch_tx: PrefetchInbox,
        soniox_audio: Option<crate::soniox::SonioxAudioSender>,
        preserve_full_audio: bool,
        stream_id: u64,
        session_generation: u64,
        auto_stop_tx: mpsc::Sender<EngineCmd>,
        app: Option<AppHandle>,
        research_trace: Option<crate::research_trace::Trace>,
        spool_io: Option<(Arc<dyn AudioSpoolIo>, Arc<SpoolOwnerRegistry>)>,
    ) -> Result<Self, AudioError> {
        let started_at = std::time::Instant::now();
        let resampler = StreamResampler::new(input_rate).map_err(AudioError::Device)?;
        let (tx, rx) = mpsc::sync_channel(Self::QUEUE_CAPACITY);
        let (control_tx, control_rx) = mpsc::sync_channel(1);
        let (recycle_tx, recycle_rx) = mpsc::sync_channel(Self::QUEUE_CAPACITY);
        for _ in 0..Self::QUEUE_CAPACITY {
            let _ = recycle_tx.try_send(Vec::with_capacity(2_048));
        }
        let overflowed = Arc::new(AtomicBool::new(false));
        let worker_overflowed = overflowed.clone();
        let continuity = Arc::new(Mutex::new(CaptureContinuityTracker::default()));
        let stale_gap_latched = Arc::new(AtomicBool::new(false));
        let worker_stale_gap_latched = stale_gap_latched.clone();
        let worker_continuity = continuity.clone();
        let worker_started_at = started_at;
        let closed_lid_latched = Arc::new(AtomicBool::new(false));
        let worker_closed_lid_latched = closed_lid_latched.clone();
        let rate_drift_latched = Arc::new(AtomicBool::new(false));
        let worker_rate_drift_latched = rate_drift_latched.clone();
        let rate_drift = Arc::new(Mutex::new(RateDriftTracker {
            enabled: monitor_rate_drift,
            ..RateDriftTracker::default()
        }));
        let limit_reached = Arc::new(AtomicBool::new(false));
        let worker_limit_reached = limit_reached.clone();
        let spool_writer = match spool_io {
            Some((io, registry)) => SpoolWriter::new_with_lease_and_io_and_registry(
                root,
                session_dir.clone(),
                spool_lease,
                io,
                registry,
            ),
            None => SpoolWriter::new(root, session_dir.clone(), spool_lease),
        }
        .map_err(AudioError::Device)?;
        let spool_control = spool_writer
            .finalizer
            .as_ref()
            .expect("new spool writer has a finalizer")
            .control
            .clone();
        spool_writer
            .finalizer
            .as_ref()
            .expect("new spool writer has a finalizer")
            .defer_recoverable_until_capture_closed();
        let worker_spool_control = spool_control.clone();
        let session_id = session_dir
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("unknown")
            .to_owned();
        let worker_session_id = session_id.clone();
        let worker_session_dir = session_dir.clone();
        let owner_registry = CAPTURE_OWNER_REGISTRY.get_or_init(Default::default);
        owner_registry
            .spawn(worker_session_id, move || {
                #[cfg(test)]
                tests::wait_for_capture_start_gate(&worker_session_dir);
                let mut resampler = resampler;
                let mut data = Vec::new();
                // Batch prefetch uploads completed files; this is not streaming ASR.
                let mut prefetch_chunker = Chunker::new(ChunkerConfig { chunk_length_secs });
                let warmup_samples = crate::prefetch_asr::WARMUP_CHUNK_SECS * TARGET_RATE as usize;
                let mut warmup_pending = Vec::with_capacity(warmup_samples);
                let mut warmup_sent = false;
                let allow_prefetch = !preserve_full_audio;
                let mut fanout = CaptureFanout::default();
                let worker_cue = CaptureCueHandle {
                    continuity: worker_continuity.clone(),
                    native_rate: input_rate,
                };
                let mut processing_error: Option<String> = None;
                let spool_pending = Arc::new(Mutex::new(PendingSpool {
                    samples: Vec::with_capacity(SPOOL_CHUNK_SAMPLES),
                    next_index: 0,
                }));
                let mut spool_writer = Some(spool_writer);
                let mut finish_reply = None;
                let mut cancelled = false;

                loop {
                    if finish_reply.is_none() {
                        match control_rx.try_recv() {
                            Ok(CaptureControl::Finish { reply }) => finish_reply = Some(reply),
                            Ok(CaptureControl::Cancel) => {
                                cancelled = true;
                                break;
                            }
                            Err(mpsc::TryRecvError::Disconnected | mpsc::TryRecvError::Empty) => {}
                        }
                    }
                    let command = if finish_reply.is_some() {
                        rx.try_recv().ok()
                    } else {
                        match rx.recv_timeout(CAPTURE_CONTROL_POLL) {
                            Ok(command) => Some(command),
                            Err(mpsc::RecvTimeoutError::Timeout) => None,
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                cancelled = true;
                                break;
                            }
                        }
                    };
                    let Some(command) = command else {
                        if finish_reply.is_some() {
                            break;
                        }
                        if let Some(batch) = fanout.take_ready(&data, worker_cue.snapshot(), false) {
                            if let Some(sender) = &soniox_audio {
                                let _ = sender.try_send_samples(u64::try_from(batch.source_start).expect("capture source cursor is bounded by the recording limit"), &batch.samples);
                            }
                            if allow_prefetch && !warmup_sent {
                                let remaining = warmup_samples.saturating_sub(warmup_pending.len());
                                warmup_pending.extend_from_slice(&batch.samples[..batch.samples.len().min(remaining)]);
                                if warmup_pending.len() == warmup_samples {
                                    let warmup = AudioChunk {
                                        index: 0,
                                        identity: crate::chunker::AudioChunkIdentity::from_samples_at(0, &warmup_pending),
                                        samples: std::mem::take(&mut warmup_pending),
                                        source_start_sample: 0,
                                        start_secs: 0.0,
                                        end_secs: crate::prefetch_asr::WARMUP_CHUNK_SECS as f32,
                                    };
                                    warmup_sent = prefetch_tx.try_send(PrefetchMessage::Warmup(warmup));
                                }
                            }
                            if allow_prefetch {
                                for chunk in prefetch_chunker.push(&batch.samples) {
                                    let _ = prefetch_tx.try_send(PrefetchMessage::Chunk(chunk));
                                }
                            }
                        }
                        continue;
                    };
                    match command {
                        CaptureCommand::Samples(mut input) => {
                            if worker_limit_reached.load(Ordering::Acquire) {
                                input.clear();
                                let _ = recycle_tx.try_send(input);
                                continue;
                            }
                            let mut produced = match resampler.push(&input) {
                                Ok(produced) => produced,
                                Err(error) => {
                                    if processing_error.is_none() {
                                        let message = error.clone();
                                        processing_error = Some(error);
                                        worker_overflowed.store(true, Ordering::Release);
                                        if let Some(app) = app.as_ref() {
                                            let _ = auto_stop_tx.send(EngineCmd::CaptureError {
                                                stream_id,
                                                session_generation,
                                                app: app.clone(),
                                                message,
                                            });
                                        }
                                    }
                                    input.clear();
                                    let _ = recycle_tx.try_send(input);
                                    continue;
                                }
                            };
                            input.clear();
                            let _ = recycle_tx.try_send(input);
                            apply_input_gain(&mut produced, input_gain);
                            if produced.is_empty() {
                                continue;
                            }
                            let accepted =
                                produced.len().min(max_samples.saturating_sub(data.len()));
                            if accepted > 0 {
                                if let Some(trace) = &research_trace {
                                    trace.first_input(accepted);
                                }
                            }
                            let (_, _accepted, hit_limit) = append_capture_samples(
                                &mut data,
                                &produced,
                                max_samples,
                                &spool_pending,
                                spool_writer.as_ref().map(|writer| &writer.sender),
                            );
                            let cue_state = worker_cue.snapshot();
                            if let Some(batch) = fanout.take_ready(&data, cue_state, false) {
                                if let Some(sender) = &soniox_audio {
                                    let _ = sender.try_send_samples(u64::try_from(batch.source_start).expect("capture source cursor is bounded by the recording limit"), &batch.samples);
                                }
                                if allow_prefetch && !warmup_sent {
                                    let remaining = warmup_samples.saturating_sub(warmup_pending.len());
                                    warmup_pending.extend_from_slice(&batch.samples[..batch.samples.len().min(remaining)]);
                                    if warmup_pending.len() == warmup_samples {
                                        let warmup = AudioChunk {
                                            index: 0,
                                            identity: crate::chunker::AudioChunkIdentity::from_samples_at(0, &warmup_pending),
                                            samples: std::mem::take(&mut warmup_pending),
                                            source_start_sample: 0,
                                            start_secs: 0.0,
                                            end_secs: crate::prefetch_asr::WARMUP_CHUNK_SECS as f32,
                                        };
                                        warmup_sent = prefetch_tx.try_send(PrefetchMessage::Warmup(warmup));
                                    }
                                }
                                if allow_prefetch {
                                    for chunk in prefetch_chunker.push(&batch.samples) {
                                        let _ = prefetch_tx.try_send(PrefetchMessage::Chunk(chunk));
                                    }
                                }
                            }
                            if hit_limit {
                                let newly_limited = close_intentional_limit(
                                    &worker_continuity,
                                    &worker_limit_reached,
                                    &worker_stale_gap_latched,
                                    worker_started_at.elapsed(),
                                );
                                if newly_limited {
                                    if let Some(app) = app.as_ref() {
                                        let _ = auto_stop_tx
                                            .send(EngineCmd::AutoStop { app: app.clone() });
                                    }
                                }
                            }
                        }
                    }
                }

                if cancelled {
                    if let Some(trace) = &research_trace {
                        trace.capture_closed(&data, false);
                    }
                    if let Some(writer) = spool_writer.take() {
                        writer.cancel();
                    }
                } else if let Some(reply) = finish_reply {
                    apply_capture_completion_failures(
                        &mut processing_error,
                        worker_overflowed.load(Ordering::Acquire),
                        worker_stale_gap_latched.load(Ordering::Acquire),
                        worker_closed_lid_latched.load(Ordering::Acquire),
                        worker_rate_drift_latched.load(Ordering::Acquire),
                    );
                    let mut trailing = match resampler.finish() {
                        Ok(trailing) => trailing,
                        Err(error) => {
                            if processing_error.is_none() {
                                processing_error = Some(error);
                            }
                            Vec::new()
                        }
                    };
                    apply_input_gain(&mut trailing, input_gain);
                    if !trailing.is_empty() {
                        let (_, _, _) = append_capture_samples(
                            &mut data,
                            &trailing,
                            max_samples,
                            &spool_pending,
                            spool_writer.as_ref().map(|writer| &writer.sender),
                        );
                    }
                    // Finish always resolves an unfinished Pending window to raw fallback.
                    if let Some(batch) = fanout.take_ready(&data, worker_cue.snapshot(), true) {
                        if let Some(sender) = &soniox_audio {
                            let _ = sender.try_send_samples(u64::try_from(batch.source_start).expect("capture source cursor is bounded by the recording limit"), &batch.samples);
                        }
                        if allow_prefetch && !warmup_sent {
                            let remaining = warmup_samples.saturating_sub(warmup_pending.len());
                            warmup_pending.extend_from_slice(&batch.samples[..batch.samples.len().min(remaining)]);
                            if warmup_pending.len() == warmup_samples {
                                let warmup = AudioChunk {
                                    index: 0,
                                    identity: crate::chunker::AudioChunkIdentity::from_samples_at(0, &warmup_pending),
                                    samples: std::mem::take(&mut warmup_pending),
                                    source_start_sample: 0,
                                    start_secs: 0.0,
                                    end_secs: crate::prefetch_asr::WARMUP_CHUNK_SECS as f32,
                                };
                                let _ = prefetch_tx.try_send(PrefetchMessage::Warmup(warmup));
                            }
                        }
                        if allow_prefetch {
                            for chunk in prefetch_chunker.push(&batch.samples) {
                                let _ = prefetch_tx.try_send(PrefetchMessage::Chunk(chunk));
                            }
                        }
                    }
                    let (spool_finalizer, tail_flush) = spool_writer
                        .take()
                        .map(|writer| writer.finish_with_pending(spool_pending))
                        .unwrap_or_else(|| {
                            (
                                SpoolFinalizer {
                                    control: worker_spool_control.clone(),
                                },
                                CaptureSpoolFlushReceipt::failed(
                                    "capture spool finalizer was unavailable".into(),
                                ),
                            )
                        });
                    if let Some(trace) = &research_trace {
                        trace.capture_closed(
                            &data,
                            processing_error.is_none()
                                && !worker_overflowed.load(Ordering::Acquire)
                                && !worker_stale_gap_latched.load(Ordering::Acquire)
                                && !worker_closed_lid_latched.load(Ordering::Acquire)
                                && !worker_rate_drift_latched.load(Ordering::Acquire),
                        );
                    }
                    let _ = reply.send(CaptureResult {
                        samples: data,
                        processing_error,
                        session_dir: worker_session_dir.clone(),
                        spool_finalizer,
                        tail_flush,
                    });
                }
            })
            .map_err(AudioError::Device)?;
        Ok(Self {
            tx,
            control_tx,
            recycle_rx: Some(recycle_rx),
            overflowed,
            continuity,
            stale_gap_latched,
            closed_lid_latched,
            rate_drift_latched,
            rate_drift,
            limit_reached,
            started_at,
            session_id,
            spool_control,
            terminal_requested: false,
        })
    }

    fn take_recycle_rx(&mut self) -> mpsc::Receiver<Vec<f32>> {
        self.recycle_rx
            .take()
            .expect("capture recycle receiver is available before stream creation")
    }

    fn finish(mut self) -> Result<CaptureResult, String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.control_tx
            .try_send(CaptureControl::Finish { reply: reply_tx })
            .map_err(|_| {
                self.terminal_requested = true;
                let _ = self.spool_control.request(SpoolDisposition::Recoverable);
                capture_owner_late_result(&self.session_id, None);
                "audio capture worker stopped".to_owned()
            })?;
        self.terminal_requested = true;
        let mut result = reply_rx.recv_timeout(CAPTURE_CLOSE_BUDGET);
        if let Ok(capture) = &mut result {
            apply_capture_completion_failures(
                &mut capture.processing_error,
                self.overflowed.load(Ordering::Acquire),
                self.stale_gap_latched.load(Ordering::Acquire),
                self.closed_lid_latched.load(Ordering::Acquire),
                self.rate_drift_latched.load(Ordering::Acquire),
            );
        }
        if result.is_err() {
            // A late capture result cannot be selected for the user's stop.
            // Preserve its already-spooled chunks asynchronously if possible.
            let _ = self.spool_control.request(SpoolDisposition::Recoverable);
        }
        capture_owner_late_result(&self.session_id, result.is_err().then_some(reply_rx));
        result.map_err(|_| "audio capture finalization is still pending".to_owned())
    }

    fn reject_closed_lid_capture(&self) {
        self.closed_lid_latched.store(true, Ordering::Release);
    }

    fn cancel(mut self) {
        self.terminal_requested = true;
        let _ = self.spool_control.request(SpoolDisposition::Cancelled);
        let _ = self.control_tx.try_send(CaptureControl::Cancel);
        capture_owner_late_result(&self.session_id, None);
    }
}

fn apply_capture_continuity_failure(processing_error: &mut Option<String>, overflowed: bool) {
    if overflowed && processing_error.is_none() {
        *processing_error =
            Some("audio capture queue overflowed; the recording is incomplete".into());
    }
}

fn apply_capture_completion_failures(
    processing_error: &mut Option<String>,
    overflowed: bool,
    stale_gap: bool,
    closed_lid: bool,
    rate_drift: bool,
) {
    apply_capture_continuity_failure(processing_error, overflowed);
    apply_capture_stale_gap_failure(processing_error, stale_gap);
    if closed_lid && processing_error.is_none() {
        *processing_error = Some(
            "built-in microphone capture stopped with the lid closed; the recording is incomplete"
                .into(),
        );
    }
    if rate_drift && processing_error.is_none() {
        *processing_error = Some(
            "audio capture rate drift exceeded the sustained 750ms tolerance; the recording is incomplete".into(),
        );
    }
}

fn apply_capture_stale_gap_failure(processing_error: &mut Option<String>, stale_gap: bool) {
    if stale_gap && processing_error.is_none() {
        *processing_error = Some(
            "audio capture callback continuity was interrupted; the recording is incomplete".into(),
        );
    }
}

impl Drop for CaptureWorker {
    fn drop(&mut self) {
        if self.terminal_requested {
            return;
        }
        self.terminal_requested = true;
        let _ = self.spool_control.request(SpoolDisposition::Cancelled);
        let _ = self.control_tx.try_send(CaptureControl::Cancel);
        capture_owner_late_result(&self.session_id, None);
    }
}

#[derive(Clone)]
struct SpoolSender {
    tx: mpsc::SyncSender<SpoolCommand>,
    queued_bytes: Arc<AtomicUsize>,
    queued_commands: Arc<AtomicUsize>,
    sealed: Arc<AtomicBool>,
    admission: Arc<Mutex<()>>,
    failed: Arc<AtomicBool>,
    writer_error: Arc<Mutex<Option<String>>>,
}

impl SpoolSender {
    fn enqueue(&self, index: usize, samples: Vec<f32>) {
        let start_secs = index as f32;
        let end_secs = start_secs + samples.len() as f32 / TARGET_RATE as f32;
        let source_start_sample = (index as u64).saturating_mul(SPOOL_CHUNK_SAMPLES as u64);
        let _ = self.enqueue_at(index, start_secs, end_secs, source_start_sample, samples);
    }

    fn enqueue_at(
        &self,
        index: usize,
        start_secs: f32,
        end_secs: f32,
        source_start_sample: u64,
        samples: Vec<f32>,
    ) -> bool {
        let _admission = self
            .admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.failed.load(Ordering::Acquire)
            || self.sealed.load(Ordering::Acquire)
            || samples.is_empty()
        {
            return false;
        }
        if !reserve_spool_queue_commands(&self.queued_commands) {
            self.latch_failure("audio spool queue command budget exceeded");
            return false;
        }
        let bytes = samples.len().saturating_mul(std::mem::size_of::<f32>());
        if !reserve_spool_queue_bytes(&self.queued_bytes, bytes) {
            self.queued_commands.fetch_sub(1, Ordering::AcqRel);
            self.latch_failure("audio spool queue byte budget exceeded");
            return false;
        }
        match self.tx.try_send(SpoolCommand::Samples {
            index,
            start_secs,
            end_secs,
            source_start_sample,
            samples,
        }) {
            Ok(()) => true,
            Err(_) => {
                self.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
                self.queued_commands.fetch_sub(1, Ordering::AcqRel);
                self.latch_failure("audio spool queue overflowed");
                false
            }
        }
    }

    fn enqueue_store_job<F>(&self, reserved_bytes: usize, run: F) -> Result<(), String>
    where
        F: FnOnce(&crate::store::AudioSpoolSessionLease) + Send + 'static,
    {
        let _admission = self
            .admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.sealed.load(Ordering::Acquire) {
            return Err("audio spool owner has closed command admission".into());
        }
        if !reserve_spool_queue_commands(&self.queued_commands) {
            return Err("audio spool owner command budget is full".into());
        }
        if !reserve_spool_queue_bytes(&self.queued_bytes, reserved_bytes) {
            self.queued_commands.fetch_sub(1, Ordering::AcqRel);
            return Err("audio spool owner queue byte budget is full".into());
        }
        match self.tx.try_send(SpoolCommand::StoreJob {
            reserved_bytes,
            run: Box::new(run),
        }) {
            Ok(()) => Ok(()),
            Err(_) => {
                self.queued_bytes
                    .fetch_sub(reserved_bytes, Ordering::AcqRel);
                self.queued_commands.fetch_sub(1, Ordering::AcqRel);
                Err("audio spool owner command queue is full".into())
            }
        }
    }

    fn enqueue_durable_tail_barrier(&self) -> CaptureSpoolFlushReceipt {
        let enqueued_at = std::time::Instant::now();
        let (reply, receiver) = mpsc::sync_channel(1);
        let error = {
            let _admission = self
                .admission
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if self.sealed.load(Ordering::Acquire) {
                Some((
                    "audio spool owner closed before tail barrier".to_owned(),
                    None,
                ))
            } else {
                match self.tx.try_send(SpoolCommand::TailBarrier { reply }) {
                    Ok(()) => None,
                    Err(mpsc::TrySendError::Full(SpoolCommand::TailBarrier { reply })) => Some((
                        "audio spool owner could not admit the durable tail barrier".to_owned(),
                        Some(reply),
                    )),
                    Err(mpsc::TrySendError::Disconnected(SpoolCommand::TailBarrier { reply })) => {
                        Some((
                            "audio spool owner ended before the durable tail barrier".to_owned(),
                            Some(reply),
                        ))
                    }
                    Err(mpsc::TrySendError::Full(_) | mpsc::TrySendError::Disconnected(_)) => {
                        Some((
                            "audio spool owner rejected the durable tail barrier".to_owned(),
                            None,
                        ))
                    }
                }
            }
        };
        if let Some((message, reply)) = error {
            self.latch_failure(&message);
            if let Some(reply) = reply {
                let _ = reply.send(Err(message));
            } else {
                let (failed_reply, failed_receiver) = mpsc::sync_channel(1);
                let _ = failed_reply.send(Err(message));
                return CaptureSpoolFlushReceipt {
                    receiver: Some(failed_receiver),
                    enqueued_at,
                };
            }
        }
        CaptureSpoolFlushReceipt {
            receiver: Some(receiver),
            enqueued_at,
        }
    }

    fn latch_failure(&self, message: &str) {
        self.failed.store(true, Ordering::Release);
        let mut error = self
            .writer_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        error.get_or_insert_with(|| message.to_owned());
    }
}

fn reserve_spool_queue_commands(used: &AtomicUsize) -> bool {
    let mut current = used.load(Ordering::Acquire);
    loop {
        let Some(next) = current.checked_add(1) else {
            return false;
        };
        if next > SPOOL_OWNER_QUEUE_CAPACITY {
            return false;
        }
        match used.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(actual) => current = actual,
        }
    }
}

fn reserve_spool_queue_bytes(used: &AtomicUsize, bytes: usize) -> bool {
    let mut current = used.load(Ordering::Acquire);
    loop {
        let Some(next) = current.checked_add(bytes) else {
            return false;
        };
        if next > SPOOL_OWNER_QUEUE_BYTES {
            return false;
        }
        match used.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(actual) => current = actual,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpoolDisposition {
    Completed,
    Recoverable,
    Degraded,
    Cancelled,
}

impl SpoolDisposition {
    fn code(self) -> u8 {
        match self {
            Self::Completed => 1,
            Self::Recoverable => 2,
            Self::Degraded => 3,
            Self::Cancelled => 4,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Completed),
            2 => Some(Self::Recoverable),
            3 => Some(Self::Degraded),
            4 => Some(Self::Cancelled),
            _ => None,
        }
    }

    fn priority(self) -> u8 {
        match self {
            Self::Recoverable => 1,
            Self::Degraded => 2,
            Self::Completed => 3,
            Self::Cancelled => 4,
        }
    }
}

#[derive(Clone)]
struct SpoolControl {
    tx: mpsc::SyncSender<SpoolDisposition>,
    requested: Arc<AtomicU8>,
    sealed: Arc<AtomicBool>,
    defer_recoverable_until_producer_closed: Arc<AtomicBool>,
    producer_closed: Arc<AtomicBool>,
    admission: Arc<Mutex<()>>,
    failed: Arc<AtomicBool>,
    writer_error: Arc<Mutex<Option<String>>>,
    completed: Arc<AtomicBool>,
    registry: Arc<SpoolOwnerRegistry>,
}

impl SpoolControl {
    fn request(&self, disposition: SpoolDisposition) -> bool {
        let _admission = self
            .admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let changed = loop {
            let current = self.requested.load(Ordering::Acquire);
            let current_priority = SpoolDisposition::from_code(current)
                .map(SpoolDisposition::priority)
                .unwrap_or(0);
            if current_priority >= disposition.priority() {
                break false;
            }
            if self
                .requested
                .compare_exchange(
                    current,
                    disposition.code(),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                break true;
            }
        };
        let should_seal = match disposition {
            SpoolDisposition::Recoverable => {
                !self
                    .defer_recoverable_until_producer_closed
                    .load(Ordering::Acquire)
                    || self.producer_closed.load(Ordering::Acquire)
            }
            SpoolDisposition::Completed
            | SpoolDisposition::Degraded
            | SpoolDisposition::Cancelled => true,
        };
        if changed && should_seal {
            self.sealed.store(true, Ordering::Release);
        }
        drop(_admission);
        // The atomic is the durable-in-memory priority signal; this channel
        // only wakes the owner early. A full channel cannot lose terminal intent.
        if changed && should_seal {
            let _ = self.tx.try_send(disposition);
        }
        changed
    }

    fn mark_capture_producer_closed(&self) {
        let _admission = self
            .admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.producer_closed.store(true, Ordering::Release);
        let requested = SpoolDisposition::from_code(self.requested.load(Ordering::Acquire));
        let should_wake = requested == Some(SpoolDisposition::Recoverable)
            && !self.sealed.swap(true, Ordering::AcqRel);
        drop(_admission);
        if should_wake {
            // Recoverable was an intent until the capture worker had drained
            // its accepted callback queue and enqueued the final tail chunk.
            let _ = self.tx.try_send(SpoolDisposition::Recoverable);
        }
    }

    fn failure(&self) -> Option<String> {
        self.writer_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .or_else(|| {
                self.failed
                    .load(Ordering::Acquire)
                    .then(|| "audio spool is incomplete".to_owned())
            })
    }
}

struct SpoolFinalizer {
    control: SpoolControl,
}

impl SpoolFinalizer {
    fn resolve(&self, disposition: SpoolDisposition) -> bool {
        let sent = self.control.request(disposition);
        self.control.registry.reap_if_finished();
        sent
    }

    fn cancel(&self) -> bool {
        self.resolve(SpoolDisposition::Cancelled)
    }

    fn defer_recoverable_until_capture_closed(&self) {
        self.control
            .defer_recoverable_until_producer_closed
            .store(true, Ordering::Release);
    }

    fn mark_capture_producer_closed(&self) {
        self.control.mark_capture_producer_closed();
    }

    fn failure(&self) -> Option<String> {
        self.control.failure()
    }

    async fn wait_closed(&self, timeout: Duration) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + timeout;
        while !self.control.completed.load(Ordering::Acquire) {
            if tokio::time::Instant::now() >= deadline {
                return Err("audio spool finalization is still pending".into());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        match self.failure() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn wait_closed_blocking(&self) -> Result<(), String> {
        while !self.control.completed.load(Ordering::Acquire) {
            thread::sleep(SPOOL_CONTROL_POLL);
        }
        match self.failure() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

pub(crate) struct LongSpoolSession {
    #[cfg(test)]
    session_dir: PathBuf,
    lease: crate::store::AudioSpoolSessionLease,
    sender: SpoolSender,
    finalizer: SpoolFinalizer,
    job_pending: Arc<AtomicBool>,
}

#[derive(Clone)]
pub(crate) struct LongSpoolEnqueue {
    sender: SpoolSender,
}

impl LongSpoolEnqueue {
    pub(crate) fn enqueue(
        &self,
        index: usize,
        start_secs: f32,
        end_secs: f32,
        source_start_sample: u64,
        samples: Vec<f32>,
    ) -> bool {
        self.sender
            .enqueue_at(index, start_secs, end_secs, source_start_sample, samples)
    }

    /// Persist one terminal recovery WAV through the same serial owner that
    /// writes this long session's chunks. The caller is a bounded retained
    /// store worker, never the UI thread; waiting here preserves FIFO ordering
    /// and avoids overlapping the shared RecoverySpool scope.
    pub(crate) fn persist_full_recovery_and_wait(
        &self,
        root: PathBuf,
        wav: Vec<u8>,
    ) -> Result<crate::store::RecoveredSpool, String> {
        let reserve = wav.len();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.sender.enqueue_store_job(reserve, move |lease| {
            let result = crate::store::persist_audio_spool_recovery_wav(lease, &root, &wav)
                .map_err(|error| error.to_string());
            let _ = reply_tx.send(result);
        })?;
        reply_rx
            .recv()
            .map_err(|_| "long audio recovery owner ended before acknowledgement".to_owned())?
    }
}

impl LongSpoolSession {
    pub(crate) fn start(root: &std::path::Path, session_id: &str) -> Result<Self, String> {
        Self::start_with_io_and_registry(
            root,
            session_id,
            Arc::new(StoreAudioSpoolIo),
            spool_owner_registry(),
        )
    }

    fn start_with_io_and_registry(
        root: &std::path::Path,
        session_id: &str,
        io: Arc<dyn AudioSpoolIo>,
        registry: Arc<SpoolOwnerRegistry>,
    ) -> Result<Self, String> {
        let (session_dir, lease) = crate::store::begin_audio_spool_session(root, session_id)
            .map_err(|error| error.to_string())?;
        let writer = SpoolWriter::new_with_lease_and_io_and_registry(
            root.to_path_buf(),
            session_dir.clone(),
            lease.clone(),
            io,
            registry,
        )?;
        let (sender, finalizer) = writer.into_parts()?;
        Ok(Self {
            #[cfg(test)]
            session_dir,
            lease,
            sender,
            finalizer,
            job_pending: Arc::new(AtomicBool::new(false)),
        })
    }

    #[cfg(test)]
    pub(crate) fn session_dir(&self) -> &std::path::Path {
        &self.session_dir
    }

    pub(crate) fn enqueue_handle(&self) -> LongSpoolEnqueue {
        LongSpoolEnqueue {
            sender: self.sender.clone(),
        }
    }

    pub(crate) async fn record_chunk_status(
        &self,
        index: usize,
        start_secs: f32,
        end_secs: f32,
        status: &'static str,
    ) -> Result<(), String> {
        self.run_store_job(0, move |lease| {
            crate::store::record_audio_spool_chunk_status(
                lease,
                lease.session_dir(),
                index,
                start_secs,
                end_secs,
                status,
            )
            .map_err(|error| error.to_string())
        })
        .await
    }

    pub(crate) async fn run_store_job<T, F>(
        &self,
        reserved_bytes: usize,
        operation: F,
    ) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&crate::store::AudioSpoolSessionLease) -> Result<T, String> + Send + 'static,
    {
        if self.lease.deletion_requested() {
            return Err("audio spool session is no longer writable".into());
        }
        if self
            .job_pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("an audio spool store operation is still pending".into());
        }
        let pending = self.job_pending.clone();
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let enqueue = self.sender.enqueue_store_job(reserved_bytes, move |lease| {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(lease)))
                    .unwrap_or_else(|_| Err("audio spool store operation failed".into()));
            pending.store(false, Ordering::Release);
            let _ = reply_tx.send(result);
        });
        if let Err(error) = enqueue {
            self.job_pending.store(false, Ordering::Release);
            return Err(error);
        }
        match tokio::time::timeout(SPOOL_STORE_JOB_WAIT, reply_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("audio spool owner ended before acknowledging the operation".into()),
            Err(_) => Err("audio spool store operation is still pending".into()),
        }
    }

    pub(crate) async fn finish(self, disposition: SpoolDisposition) -> Result<(), String> {
        self.finalizer.resolve(disposition);
        self.finalizer.wait_closed(CANCEL_SPOOL_CLOSE_BUDGET).await
    }
}

impl Drop for SpoolFinalizer {
    fn drop(&mut self) {
        let _ = self.control.request(SpoolDisposition::Recoverable);
    }
}

trait AudioSpoolIo: Send + Sync + 'static {
    fn initialize(
        &self,
        _lease: &crate::store::AudioSpoolSessionLease,
        _root: &std::path::Path,
        _session_dir: &std::path::Path,
    ) -> Result<(), String> {
        Ok(())
    }

    // This injectable I/O boundary must receive the session lease and paths
    // alongside one chunk's exact timeline and sample payload. Keeping those
    // coupled values explicit lets the storage implementation enforce the
    // existing owner/deletion checks at the write itself.
    #[allow(clippy::too_many_arguments)]
    fn write_chunk(
        &self,
        lease: &crate::store::AudioSpoolSessionLease,
        root: &std::path::Path,
        session_dir: &std::path::Path,
        index: usize,
        start_secs: f32,
        end_secs: f32,
        samples: &[f32],
    ) -> Result<(), String>;

    // The source offset is part of the same owned chunk write contract; do not
    // separate it from the lease, session paths, interval, or sample payload.
    #[allow(clippy::too_many_arguments)]
    fn write_chunk_with_source(
        &self,
        lease: &crate::store::AudioSpoolSessionLease,
        root: &std::path::Path,
        session_dir: &std::path::Path,
        index: usize,
        start_secs: f32,
        end_secs: f32,
        _source_start_sample: u64,
        samples: &[f32],
    ) -> Result<(), String> {
        self.write_chunk(
            lease,
            root,
            session_dir,
            index,
            start_secs,
            end_secs,
            samples,
        )
    }

    fn finish(
        &self,
        lease: &crate::store::AudioSpoolSessionLease,
        session_dir: &std::path::Path,
        disposition: SpoolDisposition,
        incomplete: bool,
    ) -> Result<(), String>;
}

struct StoreAudioSpoolIo;

fn retry_busy_audio_spool_operation<T>(
    mut operation: impl FnMut() -> Result<T, String>,
) -> Result<T, String> {
    let deadline = std::time::Instant::now() + SPOOL_SCOPE_BUSY_RETRY;
    loop {
        match operation() {
            Err(error)
                if error == "audio spool I/O is busy" && std::time::Instant::now() < deadline =>
            {
                // This wait belongs to the bounded spool owner, never the
                // capture callback. Accepted PCM remains covered by its fixed
                // command and byte reservations until the operation returns.
                thread::sleep(Duration::from_millis(10));
            }
            result => return result,
        }
    }
}

impl AudioSpoolIo for StoreAudioSpoolIo {
    fn initialize(
        &self,
        lease: &crate::store::AudioSpoolSessionLease,
        _root: &std::path::Path,
        _session_dir: &std::path::Path,
    ) -> Result<(), String> {
        retry_busy_audio_spool_operation(|| {
            crate::store::initialize_audio_spool_session(lease).map_err(|error| error.to_string())
        })
    }

    fn write_chunk(
        &self,
        lease: &crate::store::AudioSpoolSessionLease,
        root: &std::path::Path,
        session_dir: &std::path::Path,
        index: usize,
        start_secs: f32,
        end_secs: f32,
        samples: &[f32],
    ) -> Result<(), String> {
        let source_start_sample = (index as u64)
            .checked_mul(SPOOL_CHUNK_SAMPLES as u64)
            .ok_or_else(|| "audio spool sample position overflowed".to_owned())?;
        self.write_chunk_with_source(
            lease,
            root,
            session_dir,
            index,
            start_secs,
            end_secs,
            source_start_sample,
            samples,
        )
    }

    fn write_chunk_with_source(
        &self,
        lease: &crate::store::AudioSpoolSessionLease,
        root: &std::path::Path,
        session_dir: &std::path::Path,
        index: usize,
        start_secs: f32,
        end_secs: f32,
        source_start_sample: u64,
        samples: &[f32],
    ) -> Result<(), String> {
        let Some(session_id) = session_dir.file_name().and_then(|value| value.to_str()) else {
            return Err("invalid spool session path".into());
        };
        let relative = PathBuf::from(session_id)
            .join("chunks")
            .join(format!("{index:08}.f32"));
        let bytes = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>();
        retry_busy_audio_spool_operation(|| {
            crate::store::write_audio_spool_chunk_with_samples(
                lease,
                root,
                session_dir,
                &relative,
                index,
                start_secs,
                end_secs,
                "written",
                source_start_sample,
                samples.len() as u64,
                &bytes,
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
        })
    }

    fn finish(
        &self,
        lease: &crate::store::AudioSpoolSessionLease,
        session_dir: &std::path::Path,
        disposition: SpoolDisposition,
        incomplete: bool,
    ) -> Result<(), String> {
        if lease.deletion_requested() {
            return retry_busy_audio_spool_operation(|| remove_owned_spool_session(lease));
        }
        match disposition {
            SpoolDisposition::Completed | SpoolDisposition::Cancelled => {
                let status = if disposition == SpoolDisposition::Completed {
                    "completed"
                } else {
                    "cancelled"
                };
                let status_result = retry_busy_audio_spool_operation(|| {
                    crate::store::mark_audio_spool_status(lease, session_dir, status)
                        .map_err(|error| error.to_string())
                });
                match retry_busy_audio_spool_operation(|| {
                    crate::store::delete_audio_spool_session(lease)
                        .map_err(|error| error.to_string())
                }) {
                    Ok(()) => Ok(()),
                    Err(error) => Err(match status_result {
                        Ok(()) => error.to_string(),
                        Err(status_error) => {
                            format!("{status_error}; spool removal failed: {error}")
                        }
                    }),
                }
            }
            SpoolDisposition::Recoverable | SpoolDisposition::Degraded => {
                let status = if disposition == SpoolDisposition::Degraded || incomplete {
                    "degraded"
                } else {
                    "recoverable"
                };
                retry_busy_audio_spool_operation(|| {
                    crate::store::mark_audio_spool_status(lease, session_dir, status)
                        .map_err(|error| error.to_string())
                })?;
                if lease.deletion_requested() {
                    retry_busy_audio_spool_operation(|| remove_owned_spool_session(lease))
                } else {
                    Ok(())
                }
            }
        }
    }
}

fn remove_owned_spool_session(lease: &crate::store::AudioSpoolSessionLease) -> Result<(), String> {
    crate::store::delete_audio_spool_session(lease).map_err(|error| error.to_string())
}

fn finish_spool_owner(
    io: &dyn AudioSpoolIo,
    lease: &crate::store::AudioSpoolSessionLease,
    session_dir: &std::path::Path,
    disposition: SpoolDisposition,
    incomplete: bool,
) -> Result<(), String> {
    if lease.deletion_requested() {
        return remove_owned_spool_session(lease);
    }
    let result = io.finish(lease, session_dir, disposition, incomplete);
    if lease.deletion_requested() {
        // A clear/delete may race the first terminal operation. Keep the lease
        // visible through this owner-thread cleanup, then drop it afterwards.
        return remove_owned_spool_session(lease);
    }
    result
}

pub(crate) struct SpoolOwnerRegistry {
    owners: Mutex<HashMap<u64, SpoolOwner>>,
    capacity: usize,
    next_id: AtomicU64,
    queued_bytes: Arc<AtomicUsize>,
}

struct SpoolOwner {
    session_id: String,
    _session_dir: PathBuf,
    join: thread::JoinHandle<()>,
}

impl Default for SpoolOwnerRegistry {
    fn default() -> Self {
        Self::with_capacity(1)
    }
}

impl SpoolOwnerRegistry {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            owners: Mutex::new(HashMap::new()),
            capacity: capacity.max(1),
            next_id: AtomicU64::new(0),
            queued_bytes: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_test(capacity: usize) -> Arc<Self> {
        Arc::new(Self::with_capacity(capacity))
    }

    fn queued_bytes(&self) -> Arc<AtomicUsize> {
        self.queued_bytes.clone()
    }

    fn reserve_bytes(&self, bytes: usize) -> Result<SpoolOwnerByteReservation, String> {
        if bytes > SPOOL_OWNER_QUEUE_BYTES || !reserve_spool_queue_bytes(&self.queued_bytes, bytes)
        {
            return Err("retained audio store payload budget is full".into());
        }
        Ok(SpoolOwnerByteReservation {
            used: self.queued_bytes.clone(),
            bytes,
        })
    }
}

#[cfg(test)]
pub(crate) fn hold_retained_registry_admission_lock_for_test(
    registry: Arc<SpoolOwnerRegistry>,
) -> (
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::Sender<()>,
    thread::JoinHandle<()>,
) {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let join = thread::spawn(move || {
        let _guard = registry
            .owners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    (entered_rx, release_tx, join)
}

struct SpoolOwnerByteReservation {
    used: Arc<AtomicUsize>,
    bytes: usize,
}

impl Drop for SpoolOwnerByteReservation {
    fn drop(&mut self) {
        self.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

static SPOOL_OWNER_REGISTRY: OnceLock<Arc<SpoolOwnerRegistry>> = OnceLock::new();
static RETAINED_STORE_OWNER_REGISTRY: OnceLock<Arc<SpoolOwnerRegistry>> = OnceLock::new();

fn spool_owner_registry() -> Arc<SpoolOwnerRegistry> {
    SPOOL_OWNER_REGISTRY
        .get_or_init(|| {
            Arc::new(SpoolOwnerRegistry::with_capacity(
                AUDIO_SPOOL_OWNER_CAPACITY,
            ))
        })
        .clone()
}

fn retained_store_owner_registry() -> Arc<SpoolOwnerRegistry> {
    RETAINED_STORE_OWNER_REGISTRY
        .get_or_init(|| {
            Arc::new(SpoolOwnerRegistry::with_capacity(
                RETAINED_STORE_OWNER_CAPACITY,
            ))
        })
        .clone()
}

#[cfg(test)]
pub(crate) async fn wait_for_retained_spool_registry_to_exit_for_test(
    registry: Arc<SpoolOwnerRegistry>,
) {
    wait_for_retained_store_registry_to_exit_for_test(registry).await;
}

#[cfg(test)]
async fn wait_for_retained_store_registry_to_exit_for_test(registry: Arc<SpoolOwnerRegistry>) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            registry.reap_if_finished();
            let has_trace_owner = registry
                .owners
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .values()
                .any(|owner| owner.session_id.starts_with("dictation-trace"));
            if !has_trace_owner {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        let owners = registry
            .owners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        panic!(
            "owned retained trace worker exits and is reaped; remaining owners: {:?}",
            owners
                .values()
                .map(|owner| (&owner.session_id, owner.join.is_finished()))
                .collect::<Vec<_>>()
        );
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RetainedSpoolJobFailure {
    Busy,
    Pending,
    Failed(String),
}

/// A store job is admitted synchronously into the bounded owner registry.
/// Dropping this receipt only abandons the caller's acknowledgement; the
/// owner still retains its immutable payload, byte reservation, and root
/// mutation lease until the filesystem operation really returns.
pub(crate) struct RetainedSpoolJobReceipt<T> {
    receiver: Option<tokio::sync::oneshot::Receiver<Result<T, RetainedSpoolJobFailure>>>,
}

impl<T> RetainedSpoolJobReceipt<T> {
    pub(crate) async fn wait(&mut self) -> Result<T, RetainedSpoolJobFailure> {
        let receiver = self.receiver.as_mut().ok_or_else(|| {
            RetainedSpoolJobFailure::Failed("retained store job was already acknowledged".into())
        })?;
        match tokio::time::timeout(SPOOL_STORE_JOB_WAIT, receiver).await {
            Ok(Ok(result)) => {
                self.receiver.take();
                result
            }
            Ok(Err(_)) => {
                self.receiver.take();
                Err(RetainedSpoolJobFailure::Failed(
                    "retained audio spool owner ended before acknowledging the operation".into(),
                ))
            }
            Err(_) => Err(RetainedSpoolJobFailure::Pending),
        }
    }

    pub(crate) async fn wait_until_complete(&mut self) -> Result<T, RetainedSpoolJobFailure> {
        let Some(receiver) = self.receiver.take() else {
            return Err(RetainedSpoolJobFailure::Failed(
                "retained store job was already acknowledged".into(),
            ));
        };
        receiver.await.map_err(|_| {
            RetainedSpoolJobFailure::Failed(
                "retained audio spool owner ended before acknowledging the operation".into(),
            )
        })?
    }
}

/// Admit a finite operation with a caller-held root lease. This is used when
/// the lease must cover a pre-delivery journal, the native side effect, and
/// its later History acknowledgement as one clear-safe transaction.
pub(crate) fn enqueue_retained_spool_store_job_with_mutation<T, F>(
    owner_id: String,
    root: PathBuf,
    reserved_bytes: usize,
    mutation: crate::store::SpoolRootMutationLease,
    operation: F,
) -> Result<RetainedSpoolJobReceipt<T>, RetainedSpoolJobFailure>
where
    T: Send + 'static,
    F: FnOnce(&crate::store::SpoolRootMutationLease) -> Result<T, String> + Send + 'static,
{
    enqueue_retained_spool_store_job_with_registry(
        retained_store_owner_registry(),
        owner_id,
        root,
        reserved_bytes,
        mutation,
        operation,
    )
}

#[allow(
    dead_code,
    reason = "Used by the production retained trace path outside unit-test registry injection."
)]
pub(crate) fn try_enqueue_retained_spool_store_job_with_mutation<T, F>(
    owner_id: String,
    root: PathBuf,
    reserved_bytes: usize,
    mutation: crate::store::SpoolRootMutationLease,
    operation: F,
) -> Result<RetainedSpoolJobReceipt<T>, RetainedSpoolJobFailure>
where
    T: Send + 'static,
    F: FnOnce(&crate::store::SpoolRootMutationLease) -> Result<T, String> + Send + 'static,
{
    try_enqueue_retained_spool_store_job_with_registry(
        retained_store_owner_registry(),
        owner_id,
        root,
        reserved_bytes,
        mutation,
        operation,
    )
}

pub(crate) fn try_enqueue_retained_spool_store_job_with_registry<T, F>(
    registry: Arc<SpoolOwnerRegistry>,
    owner_id: String,
    root: PathBuf,
    reserved_bytes: usize,
    mutation: crate::store::SpoolRootMutationLease,
    operation: F,
) -> Result<RetainedSpoolJobReceipt<T>, RetainedSpoolJobFailure>
where
    T: Send + 'static,
    F: FnOnce(&crate::store::SpoolRootMutationLease) -> Result<T, String> + Send + 'static,
{
    if mutation.data_root() != root.as_path() {
        return Err(RetainedSpoolJobFailure::Failed(
            "retained store job lease does not own this data root".into(),
        ));
    }
    if reserved_bytes > SPOOL_OWNER_QUEUE_BYTES {
        return Err(RetainedSpoolJobFailure::Failed(
            "retained audio spool job exceeds the bounded payload budget".into(),
        ));
    }
    let byte_reservation = registry
        .reserve_bytes(reserved_bytes)
        .map_err(|_| RetainedSpoolJobFailure::Busy)?;
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    registry
        .try_spawn(owner_id, root, move || {
            let _byte_reservation = byte_reservation;
            let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                operation(&mutation)
            })) {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(error)) => Err(RetainedSpoolJobFailure::Failed(error)),
                Err(_) => Err(RetainedSpoolJobFailure::Failed(
                    "retained audio spool operation panicked".into(),
                )),
            };
            if reply_tx.send(result).is_err() {
                log::info!("retained trace write completed after its caller stopped waiting");
            }
        })
        .map_err(|_| RetainedSpoolJobFailure::Busy)?;
    Ok(RetainedSpoolJobReceipt {
        receiver: Some(reply_rx),
    })
}

pub(crate) fn enqueue_retained_spool_store_job_with_registry<T, F>(
    registry: Arc<SpoolOwnerRegistry>,
    owner_id: String,
    root: PathBuf,
    reserved_bytes: usize,
    mutation: crate::store::SpoolRootMutationLease,
    operation: F,
) -> Result<RetainedSpoolJobReceipt<T>, RetainedSpoolJobFailure>
where
    T: Send + 'static,
    F: FnOnce(&crate::store::SpoolRootMutationLease) -> Result<T, String> + Send + 'static,
{
    if mutation.data_root() != root.as_path() {
        return Err(RetainedSpoolJobFailure::Failed(
            "retained store job lease does not own this data root".into(),
        ));
    }
    if reserved_bytes > SPOOL_OWNER_QUEUE_BYTES {
        return Err(RetainedSpoolJobFailure::Failed(
            "retained audio spool job exceeds the bounded payload budget".into(),
        ));
    }
    let byte_reservation = registry
        .reserve_bytes(reserved_bytes)
        .map_err(RetainedSpoolJobFailure::Failed)?;
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let session_id = owner_id.clone();
    registry
        .spawn(owner_id, root, move || {
            let _byte_reservation = byte_reservation;
            let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                operation(&mutation)
            })) {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(error)) => Err(RetainedSpoolJobFailure::Failed(error)),
                Err(_) => Err(RetainedSpoolJobFailure::Failed(
                    "retained audio spool operation panicked".into(),
                )),
            };
            if let Err(undelivered) = reply_tx.send(result) {
                if undelivered.is_ok() {
                    log::info!(
                        "retained audio store job {session_id} completed after its caller stopped waiting"
                    );
                } else {
                    log::warn!(
                        "retained audio store job {session_id} failed after its caller stopped waiting"
                    );
                }
            }
        })
        .map_err(|error| {
            if error.contains("still pending") || error.contains("is busy") {
                RetainedSpoolJobFailure::Busy
            } else {
                RetainedSpoolJobFailure::Failed(error)
            }
        })?;
    Ok(RetainedSpoolJobReceipt {
        receiver: Some(reply_rx),
    })
}

impl std::fmt::Display for RetainedSpoolJobFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => formatter.write_str("retained audio storage is busy"),
            Self::Pending => formatter.write_str("retained audio storage is still pending"),
            Self::Failed(error) => formatter.write_str(error),
        }
    }
}

/// Variant for callers that must distinguish admission contention from a
/// running job whose acknowledgement missed the caller's deadline.
pub(crate) async fn run_retained_spool_store_job_observed<T, F>(
    owner_id: String,
    root: PathBuf,
    reserved_bytes: usize,
    operation: F,
) -> Result<T, RetainedSpoolJobFailure>
where
    T: Send + 'static,
    F: FnOnce(&crate::store::SpoolRootMutationLease) -> Result<T, String> + Send + 'static,
{
    run_retained_spool_store_job_observed_with_registry(
        retained_store_owner_registry(),
        owner_id,
        root,
        reserved_bytes,
        operation,
    )
    .await
}

async fn run_retained_spool_store_job_observed_with_registry<T, F>(
    registry: Arc<SpoolOwnerRegistry>,
    owner_id: String,
    root: PathBuf,
    reserved_bytes: usize,
    operation: F,
) -> Result<T, RetainedSpoolJobFailure>
where
    T: Send + 'static,
    F: FnOnce(&crate::store::SpoolRootMutationLease) -> Result<T, String> + Send + 'static,
{
    if reserved_bytes > SPOOL_OWNER_QUEUE_BYTES {
        return Err(RetainedSpoolJobFailure::Failed(
            "retained audio spool job exceeds the bounded payload budget".into(),
        ));
    }
    let mutation = crate::store::begin_spool_root_mutation(&root)
        .map_err(|_| RetainedSpoolJobFailure::Busy)?;
    let byte_reservation = registry
        .reserve_bytes(reserved_bytes)
        .map_err(RetainedSpoolJobFailure::Failed)?;
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let session_id = owner_id.clone();
    registry
        .spawn(owner_id, root, move || {
            let _byte_reservation = byte_reservation;
            let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                operation(&mutation)
            })) {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(error)) => Err(RetainedSpoolJobFailure::Failed(error)),
                Err(_) => Err(RetainedSpoolJobFailure::Failed(
                    "retained audio spool operation panicked".into(),
                )),
            };
            if let Err(undelivered) = reply_tx.send(result) {
                if undelivered.is_ok() {
                    log::info!(
                        "retained audio store job {session_id} completed after its caller stopped waiting"
                    );
                } else {
                    log::warn!(
                        "retained audio store job {session_id} failed after its caller stopped waiting"
                    );
                }
            }
        })
        .map_err(|error| {
            if error.contains("still pending") || error.contains("is busy") {
                RetainedSpoolJobFailure::Busy
            } else {
                RetainedSpoolJobFailure::Failed(error)
            }
        })?;
    match tokio::time::timeout(SPOOL_STORE_JOB_WAIT, reply_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(RetainedSpoolJobFailure::Failed(
            "retained audio spool owner ended before acknowledging the operation".into(),
        )),
        Err(_) => Err(RetainedSpoolJobFailure::Pending),
    }
}

/// Run a finite store operation on the same single retained I/O owner used by
/// audio sessions. A caller timeout only drops its reply receiver; the owner
/// slot and anything captured by `operation` remain live until it returns.
pub(crate) async fn run_retained_spool_store_job<T, F>(
    owner_id: String,
    root: PathBuf,
    reserved_bytes: usize,
    operation: F,
) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&crate::store::SpoolRootMutationLease) -> Result<T, String> + Send + 'static,
{
    run_retained_spool_store_job_with_registry(
        retained_store_owner_registry(),
        owner_id,
        root,
        reserved_bytes,
        operation,
    )
    .await
}

async fn run_retained_spool_store_job_with_registry<T, F>(
    registry: Arc<SpoolOwnerRegistry>,
    owner_id: String,
    root: PathBuf,
    reserved_bytes: usize,
    operation: F,
) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&crate::store::SpoolRootMutationLease) -> Result<T, String> + Send + 'static,
{
    let mutation =
        crate::store::begin_spool_root_mutation(&root).map_err(|error| error.to_string())?;
    let byte_reservation = registry
        .reserve_bytes(reserved_bytes)
        .map_err(|error| error.to_string())?;
    run_retained_spool_clear_job_with_registry(registry, owner_id, root, move || {
        let _byte_reservation = byte_reservation;
        operation(&mutation)
    })
    .await
}

/// Clear owns the exclusive root gate instead of a mutation lease. The passed
/// closure captures that clear guard, so a dropped reply cannot release it.
pub(crate) async fn run_retained_spool_clear_job<T, F>(
    owner_id: String,
    root: PathBuf,
    operation: F,
) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    run_retained_spool_clear_job_with_registry(
        retained_store_owner_registry(),
        owner_id,
        root,
        operation,
    )
    .await
}

pub(crate) async fn run_retained_spool_clear_job_with_registry<T, F>(
    registry: Arc<SpoolOwnerRegistry>,
    owner_id: String,
    root: PathBuf,
    operation: F,
) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    registry.spawn(owner_id, root, move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation))
            .unwrap_or_else(|_| Err("audio spool store operation panicked".into()));
        if let Err(undelivered) = reply_tx.send(result) {
            if undelivered.is_ok() {
                log::info!(
                    "retained audio store operation completed after its caller stopped waiting"
                );
            } else {
                log::warn!(
                    "retained audio store operation failed after its caller stopped waiting"
                );
            }
        }
    })?;
    match tokio::time::timeout(SPOOL_STORE_JOB_WAIT, reply_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => {
            Err("retained audio spool owner ended before acknowledging the operation".into())
        }
        Err(_) => Err("retained audio spool store operation is still pending".into()),
    }
}

impl SpoolOwnerRegistry {
    fn try_spawn<F>(
        &self,
        session_id: String,
        session_dir: PathBuf,
        worker: F,
    ) -> Result<(), String>
    where
        F: FnOnce() + Send + 'static,
    {
        let mut owners = self
            .owners
            .try_lock()
            .map_err(|_| "retained audio store admission is busy".to_owned())?;
        let finished = owners
            .iter()
            .filter_map(|(id, owner)| owner.join.is_finished().then_some(*id))
            .collect::<Vec<_>>();
        for id in finished {
            if let Some(owner) = owners.remove(&id) {
                drop(owner.join);
            }
        }
        if owners.len() >= self.capacity {
            return Err("retained audio store queue is full".into());
        }
        let id = self.next_id.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        let name = format!("voiceflow-spool-{session_id}");
        let join = thread::Builder::new()
            .name(name)
            .spawn(worker)
            .map_err(|error| format!("audio spool owner could not start: {error}"))?;
        owners.insert(
            id,
            SpoolOwner {
                session_id,
                _session_dir: session_dir,
                join,
            },
        );
        Ok(())
    }

    fn reap_if_finished(&self) {
        let mut owners = self
            .owners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let finished = owners
            .iter()
            .filter_map(|(id, owner)| owner.join.is_finished().then_some(*id))
            .collect::<Vec<_>>();
        for id in finished {
            if let Some(owner) = owners.remove(&id) {
                // Reap only a thread that has already returned. Dropping its
                // completed handle never waits on filesystem work.
                drop(owner.join);
            }
        }
    }

    fn ensure_available(&self) -> Result<(), String> {
        self.reap_if_finished();
        let owners = self
            .owners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(owner) = owners.values().next() {
            return Err(format!(
                "audio spool cleanup is still pending for session {}",
                owner.session_id
            ));
        }
        Ok(())
    }

    fn ensure_capacity(&self) -> Result<(), String> {
        self.reap_if_finished();
        let owners = self
            .owners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if owners.len() >= self.capacity {
            return Err("audio spool workers are still closing previous sessions".into());
        }
        Ok(())
    }

    fn spawn<F>(&self, session_id: String, session_dir: PathBuf, worker: F) -> Result<(), String>
    where
        F: FnOnce() + Send + 'static,
    {
        let mut owners = self
            .owners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let finished = owners
            .iter()
            .filter_map(|(id, owner)| owner.join.is_finished().then_some(*id))
            .collect::<Vec<_>>();
        for id in finished {
            if let Some(owner) = owners.remove(&id) {
                drop(owner.join);
            }
        }
        if owners.len() >= self.capacity {
            let owner = owners.values().next();
            return Err(format!(
                "audio spool cleanup is still pending for session {}",
                owner.map_or("unknown", |owner| owner.session_id.as_str())
            ));
        }
        let id = self.next_id.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        let name = format!("voiceflow-spool-{session_id}");
        let join = thread::Builder::new()
            .name(name)
            .spawn(worker)
            .map_err(|error| format!("audio spool worker could not start: {error}"))?;
        owners.insert(
            id,
            SpoolOwner {
                session_id,
                _session_dir: session_dir,
                join,
            },
        );
        Ok(())
    }
}

struct PendingCaptureOwner {
    session_id: String,
    join: thread::JoinHandle<()>,
    late_result: Option<mpsc::Receiver<CaptureResult>>,
}

#[derive(Default)]
struct CaptureOwnerRegistry {
    owner: Mutex<Option<PendingCaptureOwner>>,
}

static CAPTURE_OWNER_REGISTRY: OnceLock<CaptureOwnerRegistry> = OnceLock::new();

#[derive(Default)]
struct ReplayOwnerRegistry {
    owner: Mutex<Option<ReplayOwner>>,
}

struct ReplayOwner {
    session_generation: u64,
    join: thread::JoinHandle<()>,
}

static REPLAY_OWNER_REGISTRY: OnceLock<ReplayOwnerRegistry> = OnceLock::new();

impl ReplayOwnerRegistry {
    fn reap_finished(&self) {
        let mut owner = self
            .owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if owner.as_ref().is_some_and(|owner| owner.join.is_finished()) {
            owner.take();
        }
    }

    fn ensure_available(&self) -> Result<(), String> {
        self.reap_finished();
        let owner = self
            .owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(owner) = owner.as_ref() {
            return Err(format!(
                "owned replay worker is still closing session {}",
                owner.session_generation
            ));
        }
        Ok(())
    }

    #[cfg(all(feature = "research-harness", target_os = "macos"))]
    fn spawn<F>(&self, session_generation: u64, worker: F) -> Result<(), String>
    where
        F: FnOnce() + Send + 'static,
    {
        let mut owner = self
            .owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if owner.as_ref().is_some_and(|owner| owner.join.is_finished()) {
            owner.take();
        }
        if let Some(owner) = owner.as_ref() {
            return Err(format!(
                "owned replay worker is still closing session {}",
                owner.session_generation
            ));
        }
        let join = thread::Builder::new()
            .name("owned-wav-replay".to_owned())
            .spawn(worker)
            .map_err(|error| format!("owned replay worker could not start: {error}"))?;
        *owner = Some(ReplayOwner {
            session_generation,
            join,
        });
        Ok(())
    }

    fn unpark(&self, session_generation: u64) {
        let owner = self
            .owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(owner) = owner
            .as_ref()
            .filter(|owner| owner.session_generation == session_generation)
        {
            owner.join.thread().unpark();
        }
    }
}

impl CaptureOwnerRegistry {
    fn reap_finished(&self) {
        let mut owner = self
            .owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if owner.as_ref().is_some_and(|owner| owner.join.is_finished()) {
            if let Some(owner) = owner.take() {
                drop(owner.join);
                // A timed-out finish may have completed after its deadline.
                // Dropping this late result closes its spool through the
                // same asynchronous recovery path without claiming audio was
                // selected for the user's request.
                drop(
                    owner
                        .late_result
                        .and_then(|receiver| receiver.try_recv().ok()),
                );
            }
        }
    }

    fn ensure_available(&self) -> Result<(), String> {
        self.reap_finished();
        let owner = self
            .owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(owner) = owner.as_ref() {
            return Err(format!(
                "audio capture worker is still closing session {}",
                owner.session_id
            ));
        }
        Ok(())
    }

    fn spawn<F>(&self, session_id: String, worker: F) -> Result<(), String>
    where
        F: FnOnce() + Send + 'static,
    {
        let mut owner = self
            .owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if owner.as_ref().is_some_and(|owner| owner.join.is_finished()) {
            if let Some(owner) = owner.take() {
                drop(owner.join);
                drop(
                    owner
                        .late_result
                        .and_then(|receiver| receiver.try_recv().ok()),
                );
            }
        }
        if let Some(owner) = owner.as_ref() {
            return Err(format!(
                "audio capture worker is still closing session {}",
                owner.session_id
            ));
        }
        let name = format!("voiceflow-capture-{session_id}");
        let join = thread::Builder::new()
            .name(name)
            .spawn(worker)
            .map_err(|error| format!("audio capture worker could not start: {error}"))?;
        *owner = Some(PendingCaptureOwner {
            session_id,
            join,
            late_result: None,
        });
        Ok(())
    }

    fn set_late_result(
        &self,
        session_id: &str,
        late_result: Option<mpsc::Receiver<CaptureResult>>,
    ) {
        let mut owner = self
            .owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(owner) = owner
            .as_mut()
            .filter(|owner| owner.session_id == session_id)
        {
            owner.late_result = late_result;
        }
    }
}

fn ensure_audio_workers_available() -> Result<(), String> {
    CAPTURE_OWNER_REGISTRY
        .get_or_init(Default::default)
        .ensure_available()?;
    REPLAY_OWNER_REGISTRY
        .get_or_init(Default::default)
        .ensure_available()?;
    spool_owner_registry().ensure_capacity()
}

fn wait_for_audio_spool_owner(timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let registry = spool_owner_registry();
        if registry.ensure_available().is_ok() {
            return true;
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        thread::sleep(remaining.min(SPOOL_CONTROL_POLL));
    }
}

fn capture_owner_late_result(session_id: &str, late_result: Option<mpsc::Receiver<CaptureResult>>) {
    let registry = CAPTURE_OWNER_REGISTRY.get_or_init(Default::default);
    registry.set_late_result(session_id, late_result);
    registry.reap_finished();
}

struct SpoolWriter {
    sender: SpoolSender,
    finalizer: Option<SpoolFinalizer>,
}

struct SpoolOwnerCompletionGuard(Arc<AtomicBool>);

impl Drop for SpoolOwnerCompletionGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

struct SpoolCommandDrainGuard<'a> {
    receiver: &'a mpsc::Receiver<SpoolCommand>,
    queued_bytes: &'a AtomicUsize,
    queued_commands: &'a AtomicUsize,
}

impl Drop for SpoolCommandDrainGuard<'_> {
    fn drop(&mut self) {
        while let Ok(command) = self.receiver.try_recv() {
            match command {
                SpoolCommand::Samples { samples, .. } => {
                    self.queued_bytes.fetch_sub(
                        samples.len().saturating_mul(std::mem::size_of::<f32>()),
                        Ordering::AcqRel,
                    );
                    self.queued_commands.fetch_sub(1, Ordering::AcqRel);
                }
                SpoolCommand::StoreJob { reserved_bytes, .. } => {
                    self.queued_bytes
                        .fetch_sub(reserved_bytes, Ordering::AcqRel);
                    self.queued_commands.fetch_sub(1, Ordering::AcqRel);
                }
                SpoolCommand::TailBarrier { reply } => {
                    let _ = reply.send(Err(
                        "audio spool owner closed before durable tail acknowledgement".into(),
                    ));
                }
            }
        }
    }
}

struct SpoolWorkerContext<'a> {
    io: &'a dyn AudioSpoolIo,
    lease: &'a crate::store::AudioSpoolSessionLease,
    root: &'a std::path::Path,
    session_dir: &'a std::path::Path,
    queued_bytes: &'a AtomicUsize,
    queued_commands: &'a AtomicUsize,
    worker_failed: &'a AtomicBool,
    worker_error: &'a Mutex<Option<String>>,
}

fn process_spool_command(worker: &SpoolWorkerContext<'_>, command: SpoolCommand) {
    match command {
        SpoolCommand::Samples {
            index,
            start_secs,
            end_secs,
            source_start_sample,
            samples,
        } => {
            worker.queued_commands.fetch_sub(1, Ordering::AcqRel);
            let bytes = samples.len().saturating_mul(std::mem::size_of::<f32>());
            if !worker.lease.deletion_requested() {
                if let Err(error) = worker.io.write_chunk_with_source(
                    worker.lease,
                    worker.root,
                    worker.session_dir,
                    index,
                    start_secs,
                    end_secs,
                    source_start_sample,
                    &samples,
                ) {
                    worker.worker_failed.store(true, Ordering::Release);
                    let mut failure = worker
                        .worker_error
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    failure.get_or_insert(error);
                }
            }
            worker.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
        }
        SpoolCommand::StoreJob {
            reserved_bytes,
            run,
        } => {
            worker.queued_commands.fetch_sub(1, Ordering::AcqRel);
            if !worker.lease.deletion_requested()
                && std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(worker.lease)))
                    .is_err()
            {
                worker.worker_failed.store(true, Ordering::Release);
                let mut failure = worker
                    .worker_error
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                failure.get_or_insert_with(|| "audio spool store operation panicked".into());
            }
            worker
                .queued_bytes
                .fetch_sub(reserved_bytes, Ordering::AcqRel);
        }
        SpoolCommand::TailBarrier { reply } => {
            let result = if worker.lease.deletion_requested() {
                Err("capture spool was cleared before tail acknowledgement".to_owned())
            } else if worker.worker_failed.load(Ordering::Acquire) {
                Err(worker
                    .worker_error
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone()
                    .unwrap_or_else(|| "capture spool is incomplete".to_owned()))
            } else {
                Ok(())
            };
            let _ = reply.send(result);
        }
    }
}

impl Drop for SpoolWriter {
    fn drop(&mut self) {
        if let Some(finalizer) = self.finalizer.take() {
            // A writer abandoned before ownership transfers to the capture
            // result is a failed start/cancel, not a recoverable recording.
            // Its owner cleans only a session directory it initialized.
            let _ = finalizer.cancel();
        }
    }
}

impl SpoolWriter {
    fn new(
        root: PathBuf,
        session_dir: PathBuf,
        lease: crate::store::AudioSpoolSessionLease,
    ) -> Result<Self, String> {
        Self::new_with_lease_and_io_and_registry(
            root,
            session_dir,
            lease,
            Arc::new(StoreAudioSpoolIo),
            spool_owner_registry(),
        )
    }

    #[cfg(test)]
    fn new_with_io_and_registry(
        root: PathBuf,
        session_dir: PathBuf,
        io: Arc<dyn AudioSpoolIo>,
        registry: Arc<SpoolOwnerRegistry>,
    ) -> Result<Self, String> {
        let lease =
            crate::store::lease_spool_session(&session_dir).map_err(|error| error.to_string())?;
        Self::new_with_lease_and_io_and_registry(root, session_dir, lease, io, registry)
    }

    fn new_with_lease_and_io_and_registry(
        root: PathBuf,
        session_dir: PathBuf,
        store_lease: crate::store::AudioSpoolSessionLease,
        io: Arc<dyn AudioSpoolIo>,
        registry: Arc<SpoolOwnerRegistry>,
    ) -> Result<Self, String> {
        if store_lease.session_dir() != session_dir.as_path() {
            return Err("audio spool lease does not match the writer session".into());
        }
        let session_id = session_dir
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "invalid spool session path".to_owned())?
            .to_owned();
        // One command slot is reserved for the terminal FIFO tail barrier;
        // ordinary payload admission remains capped at the prior queue size.
        let (tx, rx) = mpsc::sync_channel(SPOOL_OWNER_QUEUE_CAPACITY + 1);
        let (control_tx, control_rx) = mpsc::sync_channel(1);
        let failed = Arc::new(AtomicBool::new(false));
        let worker_failed = failed.clone();
        let writer_error = Arc::new(Mutex::new(None));
        let worker_error = writer_error.clone();
        let requested = Arc::new(AtomicU8::new(0));
        let worker_requested = requested.clone();
        let sealed = Arc::new(AtomicBool::new(false));
        let worker_sealed = sealed.clone();
        let admission = Arc::new(Mutex::new(()));
        let queued_bytes = registry.queued_bytes();
        let worker_queued_bytes = queued_bytes.clone();
        let queued_commands = Arc::new(AtomicUsize::new(0));
        let worker_queued_commands = queued_commands.clone();
        let completed = Arc::new(AtomicBool::new(false));
        let worker_completed = completed.clone();
        let defer_recoverable_until_producer_closed = Arc::new(AtomicBool::new(false));
        let producer_closed = Arc::new(AtomicBool::new(false));
        let worker_session_dir = session_dir.clone();
        registry.spawn(session_id, session_dir, move || {
            let _completion = SpoolOwnerCompletionGuard(worker_completed);
            let store_lease = store_lease;
            let _queued_command_cleanup = SpoolCommandDrainGuard {
                receiver: &rx,
                queued_bytes: &worker_queued_bytes,
                queued_commands: &worker_queued_commands,
            };
            let mut disposition = control_rx
                .try_recv()
                .ok()
                .or_else(|| SpoolDisposition::from_code(worker_requested.load(Ordering::Acquire)));
            // A cancellation or clear can arrive after memory reservation but
            // before this owner starts. In that case no directory was created
            // by this owner, so release the reservation without touching a
            // possible pre-existing recovery session.
            if store_lease.deletion_requested()
                || matches!(disposition, Some(SpoolDisposition::Cancelled))
            {
                return;
            }
            let worker = SpoolWorkerContext {
                io: io.as_ref(),
                lease: &store_lease,
                root: &root,
                session_dir: &worker_session_dir,
                queued_bytes: &worker_queued_bytes,
                queued_commands: &worker_queued_commands,
                worker_failed: &worker_failed,
                worker_error: &worker_error,
            };
            if store_lease.needs_initialization() {
                if let Err(error) = io.initialize(&store_lease, &root, &worker_session_dir) {
                    worker_failed.store(true, Ordering::Release);
                    *worker_error
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error);
                    // Initialization failure is not allowed to remove a
                    // pre-existing path. The initializer owns any partial cleanup.
                    return;
                }
            }
            // The spool owner may have been initializing while capture finished
            // or data was cleared. Observe those terminal intents before
            // replaying any queued samples into the path.
            disposition = SpoolDisposition::from_code(worker_requested.load(Ordering::Acquire))
                .or_else(|| control_rx.try_recv().ok())
                .or(disposition);
            if store_lease.deletion_requested() {
                let _ = finish_spool_owner(
                    io.as_ref(),
                    &store_lease,
                    &worker_session_dir,
                    SpoolDisposition::Cancelled,
                    true,
                );
                return;
            }
            if matches!(disposition, Some(SpoolDisposition::Cancelled)) {
                let _ = finish_spool_owner(
                    io.as_ref(),
                    &store_lease,
                    &worker_session_dir,
                    disposition.unwrap_or(SpoolDisposition::Recoverable),
                    worker_failed.load(Ordering::Acquire)
                        || worker_queued_bytes.load(Ordering::Acquire) > 0,
                );
                return;
            }
            loop {
                disposition = SpoolDisposition::from_code(worker_requested.load(Ordering::Acquire))
                    .or_else(|| control_rx.try_recv().ok())
                    .or(disposition);
                if store_lease.deletion_requested() {
                    disposition = Some(SpoolDisposition::Cancelled);
                    break;
                }
                if disposition == Some(SpoolDisposition::Cancelled) {
                    break;
                }
                if worker_sealed.load(Ordering::Acquire) {
                    while let Ok(command) = rx.try_recv() {
                        process_spool_command(&worker, command);
                    }
                    break;
                }
                match rx.recv_timeout(SPOOL_CONTROL_POLL) {
                    Ok(command) => process_spool_command(&worker, command),
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        // A dropped data sender only seals the sample stream.
                        // Keep the owner alive until the finalizer selects a
                        // disposition; otherwise it can claim Recoverable
                        // before the caller's Completed/Cancelled request is
                        // processed. If the finalizer itself is gone, its
                        // control sender disconnects and the owner can close.
                        match control_rx.recv_timeout(SPOOL_CONTROL_POLL) {
                            Ok(requested) => {
                                disposition = Some(requested);
                                break;
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    }
                }
            }
            let disposition = SpoolDisposition::from_code(worker_requested.load(Ordering::Acquire))
                .or(disposition)
                .unwrap_or(SpoolDisposition::Recoverable);
            if let Err(error) = finish_spool_owner(
                io.as_ref(),
                &store_lease,
                &worker_session_dir,
                disposition,
                worker_failed.load(Ordering::Acquire),
            ) {
                worker_failed.store(true, Ordering::Release);
                let mut failure = worker_error
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                failure.get_or_insert(error);
            }
        })?;
        let control = SpoolControl {
            tx: control_tx,
            requested,
            sealed: sealed.clone(),
            defer_recoverable_until_producer_closed,
            producer_closed,
            admission: admission.clone(),
            failed: failed.clone(),
            writer_error: writer_error.clone(),
            completed,
            registry,
        };
        Ok(Self {
            sender: SpoolSender {
                tx,
                queued_bytes,
                queued_commands,
                sealed,
                admission,
                failed,
                writer_error,
            },
            finalizer: Some(SpoolFinalizer { control }),
        })
    }

    fn finish_with_pending(
        mut self,
        pending: Arc<Mutex<PendingSpool>>,
    ) -> (SpoolFinalizer, CaptureSpoolFlushReceipt) {
        let (index, samples) = {
            let mut pending = pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let index = pending.next_index;
            pending.next_index = pending.next_index.wrapping_add(1);
            (index, std::mem::take(&mut pending.samples))
        };
        self.sender.enqueue(index, samples);
        let tail_flush = self.sender.enqueue_durable_tail_barrier();
        let finalizer = self
            .finalizer
            .take()
            .expect("spool finalizer transfers exactly once");
        finalizer.mark_capture_producer_closed();
        (finalizer, tail_flush)
    }

    fn into_parts(mut self) -> Result<(SpoolSender, SpoolFinalizer), String> {
        let finalizer = self
            .finalizer
            .take()
            .ok_or_else(|| "audio spool finalizer was not created".to_owned())?;
        Ok((self.sender.clone(), finalizer))
    }

    fn cancel(mut self) {
        if let Some(finalizer) = self.finalizer.take() {
            let _ = finalizer.cancel();
        }
    }
}

fn append_spool(pending: &Arc<Mutex<PendingSpool>>, sender: Option<&SpoolSender>, samples: &[f32]) {
    let Some(sender) = sender else {
        return;
    };
    let mut pending = pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    pending.samples.extend_from_slice(samples);
    while pending.samples.len() >= SPOOL_CHUNK_SAMPLES {
        let chunk = pending.samples.drain(..SPOOL_CHUNK_SAMPLES).collect();
        let index = pending.next_index;
        pending.next_index = pending.next_index.wrapping_add(1);
        sender.enqueue(index, chunk);
    }
}

fn engine_loop(rx: mpsc::Receiver<EngineCmd>) {
    let mut active = None;
    let mut idle = None;
    let mut warm = None;
    let mut pending = std::collections::VecDeque::new();
    let mut check = MicCheckState::default();
    loop {
        let command = pending
            .pop_front()
            .map(Ok)
            .unwrap_or_else(|| rx.recv_timeout(LEVEL_EMIT_INTERVAL));
        match command {
            Ok(cmd) => handle_cmd(
                cmd,
                &mut active,
                &mut idle,
                &mut warm,
                &rx,
                &mut pending,
                &mut check,
            ),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(rec) = active.as_mut() {
                    if (rec.stream.is_some() || rec.replay_progress.is_some()) && !rec.device_failed
                    {
                        let elapsed_ms =
                            rec.started_at.elapsed().as_millis().min(u64::MAX as u128) as u64;
                        let last_frame_elapsed_ms =
                            rec.last_frame_elapsed_ms.load(Ordering::Acquire);
                        let freshness = capture_freshness(
                            rec.first_frame_seen,
                            elapsed_ms,
                            last_frame_elapsed_ms,
                        );
                        if freshness == CaptureFreshness::Stale {
                            rec.level.store(0.0_f32.to_bits(), Ordering::Release);
                        }
                        emit_audio_level(
                            &rec.app,
                            &rec.level,
                            rec.session_generation,
                            freshness == CaptureFreshness::Recent,
                        );
                        match freshness {
                            CaptureFreshness::Waiting => {
                                if no_audio_notice_due(
                                    rec.first_frame_seen,
                                    rec.no_frame_reported,
                                    rec.started_at.elapsed(),
                                ) {
                                    rec.no_frame_reported = true;
                                    emit_capture_state(
                                        &rec.app,
                                        rec.session_generation,
                                        "no_audio_yet",
                                        &rec.input_selection,
                                        rec.actual_device_name.as_deref(),
                                        &rec.device_metadata,
                                        (rec.rate, rec.channels),
                                    );
                                }
                            }
                            CaptureFreshness::Recent => {
                                if rec.freshness != CaptureFreshness::Recent {
                                    rec.freshness = CaptureFreshness::Recent;
                                    emit_capture_state(
                                        &rec.app,
                                        rec.session_generation,
                                        "receiving_audio",
                                        &rec.input_selection,
                                        rec.actual_device_name.as_deref(),
                                        &rec.device_metadata,
                                        (rec.rate, rec.channels),
                                    );
                                }
                            }
                            CaptureFreshness::Stale => {
                                if rec.freshness != CaptureFreshness::Stale {
                                    rec.freshness = CaptureFreshness::Stale;
                                    emit_capture_state(
                                        &rec.app,
                                        rec.session_generation,
                                        "no_recent_audio",
                                        &rec.input_selection,
                                        rec.actual_device_name.as_deref(),
                                        &rec.device_metadata,
                                        (rec.rate, rec.channels),
                                    );
                                }
                            }
                        }
                    }
                }
                if let Some(meter) = check.active.as_ref() {
                    if let Some(terminal) = meter.terminal_state() {
                        finish_mic_check(&mut check, terminal);
                        restore_warm_after_check(&mut idle, &warm);
                    } else {
                        meter.emit("running");
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[derive(Default)]
struct MicCheckState {
    active: Option<crate::microphone_check::Check>,
    cancellations: crate::microphone_check::Cancellations,
}

fn finish_mic_check(check: &mut MicCheckState, reason: &str) {
    if let Some(meter) = check.active.take() {
        meter.finish(reason);
    }
}

fn restore_warm_after_check(idle: &mut Option<IdleStream>, warm: &Option<WarmConfig>) {
    if idle.is_none() && crate::permissions::check().microphone {
        if let Some(config) = warm.as_ref().filter(|config| config.enabled) {
            *idle = open_idle_stream(&config.app, &config.input_device).ok();
        }
    }
}

pub fn start_microphone_check(
    app: AppHandle,
    session_id: String,
    input_device: String,
    input_gain: f32,
) -> Result<crate::microphone_check::Status, String> {
    let (reply, receiver) = mpsc::channel();
    engine()
        .tx
        .send(EngineCmd::StartMicCheck {
            app,
            session_id: session_id.clone(),
            input_device,
            input_gain,
            reply,
        })
        .map_err(|error| error.to_string())?;
    let result = receiver
        .recv_timeout(Duration::from_secs(5))
        .map_err(|error| error.to_string());
    if result.is_err() {
        if let Err(error) = stop_microphone_check(session_id) {
            log::warn!("microphone check start failed; stop remains unconfirmed: {error}");
        }
    }
    result?
}

pub fn stop_microphone_check(
    session_id: String,
) -> Result<crate::microphone_check::StopReceipt, String> {
    request_microphone_stop(&engine().tx, session_id, Duration::from_secs(5))
}

fn request_microphone_stop(
    tx: &mpsc::Sender<EngineCmd>,
    session_id: String,
    timeout: Duration,
) -> Result<crate::microphone_check::StopReceipt, String> {
    let (reply, receiver) = mpsc::channel();
    tx.send(EngineCmd::StopMicCheck { session_id, reply })
        .map_err(|error| format!("microphone_check_stop_enqueue_failed: {error}"))?;
    receiver
        .recv_timeout(timeout)
        .map_err(|error| format!("microphone_check_stop_ack_unconfirmed: {error}"))
}

pub fn close_microphone_check() {
    let _ = engine().tx.send(EngineCmd::CloseMicCheck);
}

fn detach_capture(rec: &mut ActiveRec, idle: &mut Option<IdleStream>, warm: &Option<WarmConfig>) {
    // Serialize with the callback before flushing the worker. Idle callbacks drop their input.
    close_capture_continuity(rec);
    stop_owned_replay(rec);
    if !rec.device_failed
        && warm
            .as_ref()
            .is_some_and(|config| config.enabled && config.input_device == rec.input_selection)
    {
        if let Some(stream) = rec.stream.take() {
            *idle = Some(IdleStream {
                stream_id: rec.stream_id,
                _stream: stream,
                _slot: rec.slot.clone(),
                input_selection: rec.input_selection.clone(),
            });
        }
    } else {
        drop(rec.stream.take());
    }
}

fn close_capture_continuity(rec: &ActiveRec) {
    close_capture_continuity_at(&rec.capture, &rec.slot, std::time::Instant::now());
}

fn close_capture_continuity_at<S>(
    capture: &CaptureWorker,
    slot: &Mutex<Option<S>>,
    now: std::time::Instant,
) {
    close_capture_admission(
        slot,
        &capture.continuity,
        &capture.stale_gap_latched,
        now.saturating_duration_since(capture.started_at),
    );
}

fn close_intentional_limit(
    continuity: &Mutex<CaptureContinuityTracker>,
    limit_reached: &AtomicBool,
    stale_gap_latched: &AtomicBool,
    elapsed: Duration,
) -> bool {
    let mut continuity = continuity.lock().unwrap_or_else(|e| e.into_inner());
    if limit_reached.swap(true, Ordering::AcqRel) {
        return false;
    }
    if continuity.close(elapsed) {
        stale_gap_latched.store(true, Ordering::Release);
    }
    true
}

fn close_capture_admission<S>(
    slot: &Mutex<Option<S>>,
    continuity: &Mutex<CaptureContinuityTracker>,
    stale_gap_latched: &AtomicBool,
    elapsed: Duration,
) {
    let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
    slot.take();
    let stale = continuity
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .close(elapsed);
    if stale {
        stale_gap_latched.store(true, Ordering::Release);
    }
}

enum TailCommand<T> {
    Pending(T),
    Stop(bool),
}

fn wait_tail_commands<T>(
    options: &StopOptions,
    rx: &mpsc::Receiver<T>,
    pending: &mut std::collections::VecDeque<T>,
    mut classify: impl FnMut(T) -> TailCommand<T>,
) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(options.extra_ms.min(2000));
    while std::time::Instant::now() < deadline {
        if options.cancellation.is_cancelled() {
            return false;
        }
        let wait = deadline
            .saturating_duration_since(std::time::Instant::now())
            .min(Duration::from_millis(25));
        match rx.recv_timeout(wait) {
            Ok(command) => match classify(command) {
                TailCommand::Pending(command) => pending.push_back(command),
                TailCommand::Stop(completed) => {
                    return completed && !options.cancellation.is_cancelled()
                }
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    !options.cancellation.is_cancelled()
}

fn wait_recording_tail(
    options: &StopOptions,
    rec: &mut ActiveRec,
    rx: &mpsc::Receiver<EngineCmd>,
    pending: &mut std::collections::VecDeque<EngineCmd>,
) -> bool {
    if rec.stream.is_none() {
        return !options.cancellation.is_cancelled();
    }
    wait_tail_commands(options, rx, pending, |command| match command {
        EngineCmd::Cancel { reply } => {
            let _ = reply.send(());
            TailCommand::Stop(false)
        }
        EngineCmd::StreamError {
            stream_id,
            app,
            message,
        } => {
            if !stream_failure_matches(rec.stream_id, stream_id) {
                return TailCommand::Pending(EngineCmd::StreamError {
                    stream_id,
                    app,
                    message,
                });
            }
            fail_recording(rec, &app, &message);
            TailCommand::Stop(true)
        }
        EngineCmd::CaptureError {
            stream_id,
            session_generation,
            app,
            message,
        } => {
            if !capture_session_matches(
                rec.stream_id,
                rec.session_generation,
                stream_id,
                Some(session_generation),
            ) {
                return TailCommand::Pending(EngineCmd::CaptureError {
                    stream_id,
                    session_generation,
                    app,
                    message,
                });
            }
            fail_recording(rec, &app, &message);
            TailCommand::Stop(true)
        }
        EngineCmd::AutoStop { app } => {
            close_capture_continuity(rec);
            drop(rec.stream.take());
            let _ = app.emit(
                "audio://limit",
                serde_json::json!({
                    "max_recording_secs": MAX_RECORDING_SECS,
                    "session_generation": rec.session_generation,
                }),
            );
            TailCommand::Stop(true)
        }
        command => TailCommand::Pending(command),
    })
}

// Holding this gate until the push finishes lets stop detach before flushing;
// an idle callback never reaches capture workers, spool, or provider senders.
fn with_capture_session<S, R>(
    slot: &Mutex<Option<S>>,
    push: impl FnOnce(&mut S) -> R,
) -> Option<R> {
    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    guard.as_mut().map(push)
}

fn emit_audio_level(
    app: &AppHandle,
    level: &Arc<AtomicU32>,
    session_generation: u64,
    frame_fresh: bool,
) {
    let level = f32::from_bits(level.load(Ordering::Acquire)).clamp(0.0, 1.0);
    let _ = app.emit(
        "audio://level",
        serde_json::json!({
            "level": level,
            "session_generation": session_generation,
            "frame_fresh": frame_fresh,
        }),
    );
}

fn reset_audio_level(app: &AppHandle, level: &Arc<AtomicU32>, session_generation: u64) {
    level.store(0.0_f32.to_bits(), Ordering::Release);
    emit_audio_level(app, level, session_generation, false);
}

fn emit_capture_state(
    app: &AppHandle,
    session_generation: u64,
    status: &str,
    input_selection: &str,
    actual_device_name: Option<&str>,
    device_metadata: &CaptureDeviceMetadata,
    format: (u32, usize),
) {
    let _ = app.emit(
        "audio://capture",
        capture_state_payload(
            session_generation,
            status,
            input_selection,
            actual_device_name,
            device_metadata,
            format.0,
            format.1,
        ),
    );
}

fn capture_state_payload(
    session_generation: u64,
    status: &str,
    input_selection: &str,
    actual_device_name: Option<&str>,
    device_metadata: &CaptureDeviceMetadata,
    sample_rate: u32,
    channels: usize,
) -> serde_json::Value {
    serde_json::json!({
        "session_generation": session_generation,
        "status": status,
        "input_selection": input_selection,
        "device_name": actual_device_name,
        "requested_selection": device_metadata.requested_selection.as_deref(),
        "clamshell_substituted": device_metadata.clamshell_substituted,
        "system_default_name": device_metadata.system_default_name.as_deref(),
        "sample_rate": sample_rate,
        "channels": channels,
    })
}

fn emit_owned_replay_event(
    app: &AppHandle,
    phase: &str,
    stream_id: u64,
    session_generation: u64,
    progress: &OwnedReplayProgress,
    acknowledged: bool,
) -> OwnedReplayLifecycleEvent {
    let event =
        owned_replay_lifecycle_event(phase, stream_id, session_generation, progress, acknowledged);
    let _ = app.emit("audio://owned-replay", &event);
    if let Ok(payload) = serde_json::to_string(&event) {
        log::info!("owned_replay_lifecycle={payload}");
    }
    event
}

fn owned_replay_lifecycle_event(
    phase: &str,
    stream_id: u64,
    session_generation: u64,
    progress: &OwnedReplayProgress,
    acknowledged: bool,
) -> OwnedReplayLifecycleEvent {
    OwnedReplayLifecycleEvent {
        phase: phase.to_owned(),
        source_identity: format!("sha256:{OWNED_WAV_SHA256}"),
        stream_id,
        session_generation,
        frame_count: progress.frame_count.load(Ordering::Acquire),
        sample_count: progress.sample_count.load(Ordering::Acquire),
        acknowledged,
    }
}

fn capture_session_matches(
    expected_stream_id: u64,
    expected_generation: u64,
    event_stream_id: u64,
    event_generation: Option<u64>,
) -> bool {
    event_stream_id == expected_stream_id && event_generation == Some(expected_generation)
}

fn stream_failure_matches(expected_stream_id: u64, event_stream_id: u64) -> bool {
    event_stream_id == expected_stream_id
}

fn capture_freshness(
    first_frame_seen: bool,
    elapsed_ms: u64,
    last_frame_elapsed_ms: u64,
) -> CaptureFreshness {
    if !first_frame_seen {
        CaptureFreshness::Waiting
    } else if elapsed_ms.saturating_sub(last_frame_elapsed_ms)
        <= CAPTURE_FRAME_FRESHNESS.as_millis() as u64
    {
        CaptureFreshness::Recent
    } else {
        CaptureFreshness::Stale
    }
}

fn fail_recording(rec: &mut ActiveRec, app: &AppHandle, message: &str) {
    if rec.device_failed {
        return;
    }
    rec.device_failed = true;
    rec.slot.lock().unwrap_or_else(|e| e.into_inner()).take();
    stop_owned_replay(rec);
    drop(rec.stream.take());
    reset_audio_level(&rec.app, &rec.level, rec.session_generation);
    emit_capture_state(
        &rec.app,
        rec.session_generation,
        "device_error",
        &rec.input_selection,
        rec.actual_device_name.as_deref(),
        &rec.device_metadata,
        (rec.rate, rec.channels),
    );
    let _ = app.emit(
        "audio://error",
        serde_json::json!({
            "session_generation": rec.session_generation,
            "session_id": rec.session_id,
            "message": message,
        }),
    );
}

fn no_audio_notice_due(first_frame_seen: bool, already_reported: bool, elapsed: Duration) -> bool {
    !first_frame_seen && !already_reported && elapsed >= Duration::from_secs(5)
}

fn handle_cmd(
    cmd: EngineCmd,
    active: &mut Option<ActiveRec>,
    idle: &mut Option<IdleStream>,
    warm: &mut Option<WarmConfig>,
    rx: &mpsc::Receiver<EngineCmd>,
    pending: &mut std::collections::VecDeque<EngineCmd>,
    check: &mut MicCheckState,
) {
    match cmd {
        EngineCmd::CloseMicCheck => {
            if check.active.is_some() {
                finish_mic_check(check, "interrupted");
                if active.is_none() {
                    restore_warm_after_check(idle, warm);
                }
            }
        }
        EngineCmd::StartMicCheck {
            app,
            session_id,
            input_device,
            input_gain,
            reply,
        } => {
            if check.cancellations.take(&session_id) {
                let _ = reply.send(Err("microphone_check_cancelled".into()));
                return;
            }
            if active.is_some()
                || crate::lock_recover(&app.state::<crate::AppState>().manager).phase
                    != crate::Phase::Idle
                || app
                    .state::<crate::AppState>()
                    .exit_state
                    .load(Ordering::Acquire)
                    != 0
                || !app.get_webview_window("main").is_some_and(|window| {
                    window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false)
                })
            {
                let _ = reply.send(Err("microphone_check_busy".into()));
                return;
            }
            if !crate::permissions::check().microphone {
                let _ = reply.send(Err("microphone_check_permission".into()));
                return;
            }
            finish_mic_check(check, "interrupted");
            *idle = None;
            match crate::microphone_check::Check::open(app, session_id, &input_device, input_gain) {
                Ok(meter) => {
                    let status = meter.status("running");
                    meter.emit("running");
                    check.active = Some(meter);
                    let _ = reply.send(Ok(status));
                }
                Err(error) => {
                    restore_warm_after_check(idle, warm);
                    let _ = reply.send(Err(error));
                }
            }
        }
        EngineCmd::StopMicCheck { session_id, reply } => {
            let stopped = check
                .active
                .as_ref()
                .is_some_and(|meter| meter.session_id == session_id);
            let state = if stopped {
                finish_mic_check(check, "stopped");
                // The matching Check/stream has dropped before this receipt.
                crate::microphone_check::StopState::Stopped
            } else {
                check.cancellations.remember(session_id.clone());
                crate::microphone_check::StopState::InactiveCancelled
            };
            let _ = reply.send(crate::microphone_check::StopReceipt { session_id, state });
            if stopped {
                restore_warm_after_check(idle, warm);
            }
        }
        EngineCmd::ConfigureWarm {
            app,
            enabled,
            input_device,
        } => {
            finish_mic_check(check, "interrupted");
            *warm = Some(WarmConfig {
                app,
                enabled,
                input_device,
            });
            if active.is_none() {
                let config = warm.as_ref().expect("warm configuration was set");
                if !config.enabled {
                    *idle = None;
                } else if idle
                    .as_ref()
                    .is_none_or(|stream| stream.input_selection != config.input_device)
                {
                    *idle = None;
                    match open_idle_stream(&config.app, &config.input_device) {
                        Ok(stream) => *idle = Some(stream),
                        Err(error) => log::warn!(
                            "microphone prewarm failed; using on-demand capture: {error}"
                        ),
                    }
                }
            }
        }
        EngineCmd::Start {
            app,
            session,
            session_generation,
            input_device,
            requested_selection,
            clamshell_substituted,
            chunk_length_secs,
            max_recording_secs,
            input_gain,
            prefetch_tx,
            soniox_audio,
            preserve_full_audio,
            owned_wav,
            readiness,
            reply,
        } => {
            let requested_metadata = CaptureDeviceMetadata {
                requested_selection: Some(requested_selection.clone()),
                clamshell_substituted,
                system_default_name: None,
            };
            finish_mic_check(check, "interrupted");
            if active.is_some() {
                let _ = reply.send(Err(AudioError::Device("recording already active".into())));
                return;
            }
            let build_result = if let Some(source) = owned_wav {
                build_owned_stream(
                    &app,
                    &session,
                    session_generation,
                    chunk_length_secs,
                    max_recording_secs,
                    input_gain,
                    prefetch_tx,
                    preserve_full_audio,
                    readiness,
                    source,
                    idle,
                )
            } else {
                build_stream(
                    &app,
                    &session,
                    session_generation,
                    &input_device,
                    &requested_selection,
                    clamshell_substituted,
                    chunk_length_secs,
                    max_recording_secs,
                    input_gain,
                    prefetch_tx,
                    soniox_audio,
                    preserve_full_audio,
                    idle,
                    readiness,
                )
            };
            match build_result {
                Ok(rec) => {
                    let handle = StartHandle {
                        session_generation,
                        cue: rec.cue.clone(),
                    };
                    let mut rec = rec;
                    let play_result = rec.stream.as_ref().map(|stream| stream.play()).transpose();
                    if let Err(error) = play_result {
                        rec.slot.lock().unwrap_or_else(|e| e.into_inner()).take();
                        drop(rec.stream.take());
                        rec.capture.cancel();
                        emit_capture_state(
                            &app,
                            session_generation,
                            "unavailable",
                            &rec.input_selection,
                            rec.actual_device_name.as_deref(),
                            &rec.device_metadata,
                            (rec.rate, rec.channels),
                        );
                        let _ = reply.send(Err(AudioError::Device(error.to_string())));
                    } else {
                        emit_capture_state(
                            &app,
                            session_generation,
                            "waiting_first_frame",
                            &rec.input_selection,
                            rec.actual_device_name.as_deref(),
                            &rec.device_metadata,
                            (rec.rate, rec.channels),
                        );
                        *active = Some(rec);
                        let _ = reply.send(Ok(handle));
                    }
                }
                Err(e) => {
                    emit_capture_state(
                        &app,
                        session_generation,
                        "unavailable",
                        &input_device,
                        None,
                        &requested_metadata,
                        (0, 0),
                    );
                    let _ = reply.send(Err(e));
                }
            }
        }
        #[cfg(all(feature = "research-harness", target_os = "macos"))]
        EngineCmd::BeginOwnedSource {
            session_generation,
            completion,
            reply,
        } => {
            let result = match active.as_mut() {
                Some(rec)
                    if rec.session_generation == session_generation
                        && rec.replay_progress.is_some() =>
                {
                    begin_owned_replay(rec, completion)
                }
                _ => Err("owned_source_generation_not_active".to_owned()),
            };
            let _ = reply.send(result);
        }
        #[cfg(all(feature = "research-harness", target_os = "macos"))]
        EngineCmd::OwnedSourceEof {
            stream_id,
            session_generation,
            completion,
        } => {
            if let Some(rec) = active.as_mut() {
                if capture_session_matches(
                    rec.stream_id,
                    rec.session_generation,
                    stream_id,
                    Some(session_generation),
                ) {
                    if let Some(progress) = rec.replay_progress.as_ref() {
                        let event = emit_owned_replay_event(
                            &rec.app,
                            "eof",
                            rec.stream_id,
                            rec.session_generation,
                            progress,
                            true,
                        );
                        let _ = completion.send(event);
                    }
                }
            }
        }
        EngineCmd::Stop { options, reply } => {
            let mut replay_receipt = None;
            let result = match active.take() {
                Some(mut rec) => {
                    let stop_lid_state = rec
                        .actual_device_identity
                        .as_ref()
                        .filter(|identity| {
                            rec.replay_progress.is_none()
                                && identity.transport == crate::clamshell::DeviceTransport::BuiltIn
                        })
                        .map(|_| crate::clamshell::current_lid_state())
                        .unwrap_or(crate::clamshell::LidState::Unknown);
                    reset_audio_level(&rec.app, &rec.level, rec.session_generation);
                    // Explicit stop/cancel must stop the owned feeder before
                    // draining the ordinary capture worker. Otherwise the
                    // optional post-stop buffer window could accept more WAV
                    // frames after the user's stop edge.
                    if rec.replay_progress.is_some() {
                        stop_owned_replay(&mut rec);
                    }
                    let completed = wait_recording_tail(&options, &mut rec, rx, pending);
                    detach_capture(&mut rec, idle, warm);
                    replay_receipt = rec.replay_progress.as_ref().map(|progress| {
                        (
                            rec.app.clone(),
                            rec.stream_id,
                            rec.session_generation,
                            progress.clone(),
                        )
                    });
                    match stop_capture_disposition(
                        rec.device_failed,
                        completed,
                        stop_lid_state,
                        rec.actual_device_identity.as_ref(),
                        rec.replay_progress.is_some(),
                    ) {
                        StopCaptureDisposition::PreserveDeviceFailure => {
                            // A cancellation racing device failure must retain recovery
                            // chunks, matching the ordinary engine cancel path.
                            preserve_partial_recording(rec);
                            Err(AudioError::Device(
                                "microphone device failed; recovery status may still be pending"
                                    .into(),
                            ))
                        }
                        StopCaptureDisposition::PreserveClosedLid => {
                            rec.capture.reject_closed_lid_capture();
                            preserve_partial_recording(rec);
                            Err(AudioError::ClosedLidBuiltInInput(
                                crate::clamshell::CLOSED_LID_BUILTIN_INPUT_MESSAGE,
                            ))
                        }
                        StopCaptureDisposition::Cancel => {
                            rec.capture.cancel();
                            Err(AudioError::Device("recording cancelled".into()))
                        }
                        StopCaptureDisposition::Finalize => finalize(rec, options.vad_enabled),
                    }
                }
                None => Err(AudioError::NotRecording),
            };
            if let Some((app, stream_id, session_generation, progress)) = replay_receipt {
                emit_owned_replay_event(
                    &app,
                    "stop_acknowledged",
                    stream_id,
                    session_generation,
                    &progress,
                    result.is_ok(),
                );
            }
            let _ = reply.send(result);
        }
        EngineCmd::Cancel { reply } => {
            if let Some(mut rec) = active.take() {
                detach_capture(&mut rec, idle, warm);
                reset_audio_level(&rec.app, &rec.level, rec.session_generation);
                if rec.device_failed {
                    preserve_partial_recording(rec);
                } else {
                    drop(rec.stream);
                    rec.capture.cancel();
                }
            }
            let _ = reply.send(());
        }
        EngineCmd::AutoStop { app } => {
            if let Some(rec) = active.as_mut() {
                close_capture_continuity(rec);
                if rec.stream.take().is_some() {
                    reset_audio_level(&app, &rec.level, rec.session_generation);
                    let _ = app.emit(
                        "audio://limit",
                        serde_json::json!({
                            "max_recording_secs": MAX_RECORDING_SECS,
                            "session_generation": rec.session_generation,
                        }),
                    );
                }
            }
        }
        EngineCmd::StreamError {
            stream_id,
            app,
            message,
        } => {
            if let Some(rec) = active.as_mut() {
                if stream_failure_matches(rec.stream_id, stream_id) {
                    fail_recording(rec, &app, &message);
                }
            }
            if idle
                .as_ref()
                .is_some_and(|idle| stream_failure_matches(idle.stream_id, stream_id))
            {
                *idle = None;
            }
        }
        EngineCmd::CaptureError {
            stream_id,
            session_generation,
            app,
            message,
        } => {
            if let Some(rec) = active.as_mut() {
                if capture_session_matches(
                    rec.stream_id,
                    rec.session_generation,
                    stream_id,
                    Some(session_generation),
                ) {
                    fail_recording(rec, &app, &message);
                }
            }
        }
        EngineCmd::CaptureFrameObserved {
            stream_id,
            session_generation,
        } => {
            if let Some(rec) = active.as_mut() {
                if capture_session_matches(
                    rec.stream_id,
                    rec.session_generation,
                    stream_id,
                    Some(session_generation),
                ) && !rec.first_frame_seen
                    && !rec.device_failed
                {
                    rec.first_frame_seen = true;
                    rec.freshness = CaptureFreshness::Recent;
                    emit_capture_state(
                        &rec.app,
                        rec.session_generation,
                        "receiving_audio",
                        &rec.input_selection,
                        rec.actual_device_name.as_deref(),
                        &rec.device_metadata,
                        (rec.rate, rec.channels),
                    );
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_stream(
    app: &AppHandle,
    session: &str,
    session_generation: u64,
    input_device: &str,
    requested_selection: &str,
    clamshell_substituted: bool,
    chunk_length_secs: usize,
    max_recording_secs: usize,
    input_gain: f32,
    prefetch_tx: PrefetchInbox,
    soniox_audio: Option<crate::soniox::SonioxAudioSender>,
    preserve_full_audio: bool,
    idle: &mut Option<IdleStream>,
    readiness: Option<mpsc::Sender<()>>,
) -> Result<ActiveRec, AudioError> {
    let host = cpal::default_host();
    let system_default_name = host
        .default_input_device()
        .and_then(|device| device.name().ok());
    let device = match resolve_input_device(&host, input_device) {
        Ok(device) => device,
        Err(error @ AudioError::ClosedLidBuiltInInput(_)) => {
            // A previously open always-on stream may still own the built-in
            // microphone. Release it before returning the actionable error.
            drop(idle.take());
            return Err(error);
        }
        Err(error) => return Err(error),
    };
    let supported = device
        .default_input_config()
        .map_err(|e| AudioError::Device(e.to_string()))?;
    let actual_device_name = device.name().ok();
    let config = supported.config();
    let channels = config.channels as usize;
    let rate = config.sample_rate.0;
    let max_samples = max_recording_secs.saturating_mul(TARGET_RATE as usize);
    ensure_audio_workers_available().map_err(AudioError::Device)?;
    let root = app
        .path()
        .app_data_dir()
        .map_err(|e| AudioError::Device(e.to_string()))?;
    let (dir, spool_lease) = crate::store::begin_audio_spool_session(&root, session)
        .map_err(|error| AudioError::Device(error.to_string()))?;
    let auto_stop_tx = engine().tx.clone();
    let engine_tx = auto_stop_tx.clone();
    // CPAL Device has no stable identity/equality contract. Do not rebind an
    // idle callback stream merely because its display name/configuration match.
    drop(idle.take());
    let stream_id = next_stream_id();
    let mut capture = match CaptureWorker::new(
        root.clone(),
        dir.clone(),
        spool_lease,
        rate,
        true,
        max_samples,
        chunk_length_secs,
        input_gain,
        prefetch_tx,
        soniox_audio.clone(),
        preserve_full_audio,
        stream_id,
        session_generation,
        auto_stop_tx.clone(),
        Some(app.clone()),
        crate::research_trace::audio_trace(app, session),
        None,
    ) {
        Ok(capture) => capture,
        Err(error) => {
            return Err(error);
        }
    };
    let level = Arc::new(AtomicU32::new(0.0_f32.to_bits()));
    let first_frame_reported = Arc::new(AtomicBool::new(false));
    let started_at = capture.started_at;
    let ui_started_at = std::time::Instant::now();
    let last_frame_elapsed_ms = Arc::new(AtomicU64::new(0));
    let callback = CallbackSession {
        state: CapturePushState {
            stream_id,
            session_generation,
            first_frame_reported,
            started_at,
            ui_started_at,
            last_frame_elapsed_ms: last_frame_elapsed_ms.clone(),
            continuity: capture.continuity.clone(),
            cue_handle: Some(CaptureCueHandle {
                continuity: capture.continuity.clone(),
                native_rate: rate,
            }),
            stale_gap_latched: capture.stale_gap_latched.clone(),
            rate_drift: capture.rate_drift.clone(),
            rate_drift_latched: capture.rate_drift_latched.clone(),
            native_rate: rate,
            capture_tx: capture.tx.clone(),
            capture_overflowed: capture.overflowed.clone(),
            capture_limit_reached: capture.limit_reached.clone(),
            level: level.clone(),
            app: app.clone(),
            engine_tx,
        },
        recycle_rx: capture.take_recycle_rx(),
        readiness,
    };
    let slot = Arc::new(Mutex::new(Some(callback)));
    let stream = match make_input_stream(app, &device, &supported, slot.clone(), stream_id) {
        Ok(stream) => stream,
        Err(error) => {
            capture.cancel();
            return Err(error);
        }
    };
    Ok(ActiveRec {
        stream_id,
        session_generation,
        session_id: session.to_owned(),
        stream: Some(stream),
        slot,
        input_selection: input_device.to_owned(),
        actual_device_name: actual_device_name.clone(),
        actual_device_identity: actual_device_name.as_deref().and_then(|name| {
            crate::clamshell::actual_coreaudio_identity_for_stable_selection(name, || {
                input_selection_matches_current(&host, input_device, &device)
            })
        }),
        device_metadata: CaptureDeviceMetadata {
            requested_selection: Some(requested_selection.to_owned()),
            clamshell_substituted,
            system_default_name,
        },
        rate,
        channels,
        device_failed: false,
        first_frame_seen: false,
        no_frame_reported: false,
        started_at: ui_started_at,
        last_frame_elapsed_ms,
        freshness: CaptureFreshness::Waiting,
        cue: Some(CaptureCueHandle {
            continuity: capture.continuity.clone(),
            native_rate: rate,
        }),
        capture,
        chunk_length_secs,
        app: app.clone(),
        level,
        preserve_full_audio,
        owned_source: None,
        replay_progress: None,
        replay_cancel: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_owned_stream(
    app: &AppHandle,
    session: &str,
    session_generation: u64,
    chunk_length_secs: usize,
    max_recording_secs: usize,
    input_gain: f32,
    prefetch_tx: PrefetchInbox,
    preserve_full_audio: bool,
    readiness: Option<mpsc::Sender<()>>,
    source: OwnedWavReplay,
    idle: &mut Option<IdleStream>,
) -> Result<ActiveRec, AudioError> {
    drop(idle.take());
    let max_samples = max_recording_secs.saturating_mul(TARGET_RATE as usize);
    ensure_audio_workers_available().map_err(AudioError::Device)?;
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| AudioError::Device(error.to_string()))?;
    let (dir, spool_lease) = crate::store::begin_audio_spool_session(&root, session)
        .map_err(|error| AudioError::Device(error.to_string()))?;
    let auto_stop_tx = engine().tx.clone();
    let stream_id = next_stream_id();
    let mut capture = match CaptureWorker::new(
        root.clone(),
        dir.clone(),
        spool_lease,
        TARGET_RATE,
        false,
        max_samples,
        chunk_length_secs,
        input_gain,
        prefetch_tx,
        None,
        preserve_full_audio,
        stream_id,
        session_generation,
        auto_stop_tx.clone(),
        Some(app.clone()),
        crate::research_trace::audio_trace(app, session),
        None,
    ) {
        Ok(capture) => capture,
        Err(error) => {
            return Err(error);
        }
    };
    let level = Arc::new(AtomicU32::new(0.0_f32.to_bits()));
    let first_frame_reported = Arc::new(AtomicBool::new(false));
    let started_at = capture.started_at;
    let ui_started_at = std::time::Instant::now();
    let last_frame_elapsed_ms = Arc::new(AtomicU64::new(0));
    let callback = CallbackSession {
        state: CapturePushState {
            stream_id,
            session_generation,
            first_frame_reported,
            started_at,
            ui_started_at,
            last_frame_elapsed_ms: last_frame_elapsed_ms.clone(),
            continuity: capture.continuity.clone(),
            cue_handle: None,
            stale_gap_latched: capture.stale_gap_latched.clone(),
            rate_drift: capture.rate_drift.clone(),
            rate_drift_latched: capture.rate_drift_latched.clone(),
            native_rate: TARGET_RATE,
            capture_tx: capture.tx.clone(),
            capture_overflowed: capture.overflowed.clone(),
            capture_limit_reached: capture.limit_reached.clone(),
            level: level.clone(),
            app: app.clone(),
            engine_tx: auto_stop_tx,
        },
        recycle_rx: capture.take_recycle_rx(),
        readiness,
    };
    let slot = Arc::new(Mutex::new(Some(callback)));
    let replay_progress = Arc::new(OwnedReplayProgress::new());
    Ok(ActiveRec {
        stream_id,
        session_generation,
        session_id: session.to_owned(),
        stream: None,
        slot,
        input_selection: "owned-wav-replay".to_owned(),
        actual_device_name: None,
        actual_device_identity: None,
        device_metadata: CaptureDeviceMetadata::default(),
        rate: TARGET_RATE,
        channels: 1,
        device_failed: false,
        first_frame_seen: false,
        no_frame_reported: false,
        started_at: ui_started_at,
        last_frame_elapsed_ms,
        freshness: CaptureFreshness::Waiting,
        cue: None,
        capture,
        chunk_length_secs,
        app: app.clone(),
        level,
        preserve_full_audio,
        owned_source: Some(source),
        replay_progress: Some(replay_progress),
        replay_cancel: None,
    })
}

fn next_stream_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
fn begin_owned_replay(
    rec: &mut ActiveRec,
    completion: mpsc::Sender<OwnedReplayLifecycleEvent>,
) -> Result<u64, String> {
    let source = rec
        .owned_source
        .take()
        .ok_or_else(|| "owned_source_missing".to_owned())?;
    let progress = rec
        .replay_progress
        .clone()
        .ok_or_else(|| "owned_source_progress_missing".to_owned())?;
    let stream_id = rec.stream_id;
    let session_generation = rec.session_generation;
    let app = rec.app.clone();
    let slot = rec.slot.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    let thread_cancelled = cancelled.clone();
    let engine_tx = engine().tx.clone();
    emit_owned_replay_event(
        &app,
        "source_started",
        stream_id,
        session_generation,
        &progress,
        true,
    );
    REPLAY_OWNER_REGISTRY
        .get_or_init(Default::default)
        .spawn(session_generation, move || {
            let started = std::time::Instant::now();
            let mut sample_count = 0_u64;
            for frame in source.samples.chunks(OWNED_WAV_REPLAY_CHUNK_SAMPLES) {
                if thread_cancelled.load(Ordering::Acquire) {
                    return;
                }
                let accepted = with_capture_session(&slot, |callback| {
                    let accepted = push::<i16>(
                        frame,
                        1,
                        StreamInstant::new(0, 0),
                        &mut callback.recycle_rx,
                        &callback.state,
                    );
                    if accepted {
                        if let Some(readiness) = callback.readiness.take() {
                            let _ = readiness.send(());
                        }
                    }
                    accepted
                })
                .unwrap_or(false);
                if !accepted {
                    emit_owned_replay_event(
                        &app,
                        "capture_failed",
                        stream_id,
                        session_generation,
                        &progress,
                        false,
                    );
                    return;
                }
                let frame_count = progress.frame_count.fetch_add(1, Ordering::AcqRel) + 1;
                sample_count = sample_count.saturating_add(frame.len() as u64);
                progress.sample_count.store(sample_count, Ordering::Release);
                if frame_count == 1 {
                    emit_owned_replay_event(
                        &app,
                        "first_frame",
                        stream_id,
                        session_generation,
                        &progress,
                        true,
                    );
                }
                let elapsed_micros = sample_count.saturating_mul(1_000_000) / TARGET_RATE as u64;
                let deadline = started + Duration::from_micros(elapsed_micros);
                if let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now())
                {
                    thread::park_timeout(remaining);
                }
            }
            if !thread_cancelled.load(Ordering::Acquire) {
                let _ = engine_tx.send(EngineCmd::OwnedSourceEof {
                    stream_id,
                    session_generation,
                    completion,
                });
            }
        })?;
    rec.replay_cancel = Some(cancelled);
    Ok(stream_id)
}

fn stop_owned_replay(rec: &mut ActiveRec) {
    if let Some(cancelled) = rec.replay_cancel.take() {
        cancelled.store(true, Ordering::Release);
        REPLAY_OWNER_REGISTRY
            .get_or_init(Default::default)
            .unpark(rec.session_generation);
    }
    rec.owned_source.take();
}

fn make_input_stream(
    app: &AppHandle,
    device: &cpal::Device,
    supported: &cpal::SupportedStreamConfig,
    slot: SessionSlot,
    stream_id: u64,
) -> Result<cpal::Stream, AudioError> {
    let config = supported.config();
    let channels = config.channels as usize;
    let app = app.clone();
    let err_fn = move |error: cpal::StreamError| {
        let _ = engine().tx.send(EngineCmd::StreamError {
            stream_id,
            app: app.clone(),
            message: error.to_string(),
        });
    };
    macro_rules! build {
        ($ty:ty) => {
            device.build_input_stream(
                &config,
                move |data: &[$ty], info: &cpal::InputCallbackInfo| {
                    let capture_time = info.timestamp().capture;
                    with_capture_session(&slot, |callback| {
                        if push(
                            data,
                            channels,
                            capture_time,
                            &mut callback.recycle_rx,
                            &callback.state,
                        ) {
                            if let Some(ready) = callback.readiness.take() {
                                let _ = ready.send(());
                            }
                        }
                    });
                },
                err_fn,
                None,
            )
        };
    }
    match supported.sample_format() {
        cpal::SampleFormat::F32 => build!(f32),
        cpal::SampleFormat::I16 => build!(i16),
        cpal::SampleFormat::U16 => build!(u16),
        _ => Err(cpal::BuildStreamError::StreamConfigNotSupported),
    }
    .map_err(|error| AudioError::Device(error.to_string()))
}

fn open_idle_stream(app: &AppHandle, input_device: &str) -> Result<IdleStream, AudioError> {
    let host = cpal::default_host();
    let device = resolve_input_device(&host, input_device)?;
    let supported = device
        .default_input_config()
        .map_err(|e| AudioError::Device(e.to_string()))?;
    let slot = Arc::new(Mutex::new(None));
    let stream_id = next_stream_id();
    let stream = make_input_stream(app, &device, &supported, slot.clone(), stream_id)?;
    stream
        .play()
        .map_err(|e| AudioError::Device(e.to_string()))?;
    Ok(IdleStream {
        stream_id,
        _stream: stream,
        _slot: slot,
        input_selection: input_device.to_owned(),
    })
}

pub(crate) fn resolve_input_device(
    host: &cpal::Host,
    selected_name: &str,
) -> Result<cpal::Device, AudioError> {
    let device = select_input_device(host, selected_name)?;

    if let Ok(name) = device.name() {
        if let Err(message) = crate::clamshell::ensure_input_device_available(&name, || {
            input_selection_matches_current(host, selected_name, &device)
        }) {
            return Err(AudioError::ClosedLidBuiltInInput(message));
        }
    }
    Ok(device)
}

fn select_input_device(host: &cpal::Host, selected_name: &str) -> Result<cpal::Device, AudioError> {
    if selected_name.trim().is_empty() {
        host.default_input_device().ok_or(AudioError::NoDevice)
    } else {
        let selected_name = selected_name.trim();
        let devices = host.input_devices().map_err(|error| {
            AudioError::Device(format!("could not enumerate input devices: {error}"))
        })?;
        devices
            .into_iter()
            .find(|device| device.name().ok().as_deref() == Some(selected_name))
            .ok_or_else(|| {
                AudioError::Device(format!(
                    "selected input device is unavailable: {selected_name}"
                ))
            })
    }
}

#[cfg(target_os = "macos")]
fn input_selection_matches_current(
    host: &cpal::Host,
    selected_name: &str,
    selected_device: &cpal::Device,
) -> bool {
    use cpal::platform::DeviceInner;

    let current_device = select_input_device(host, selected_name).ok();
    let Some(current_device) = current_device else {
        return false;
    };
    match (selected_device.as_inner(), current_device.as_inner()) {
        (DeviceInner::CoreAudio(selected), DeviceInner::CoreAudio(current)) => {
            crate::clamshell::selection_snapshot_matches(Some(selected), Some(current))
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn input_selection_matches_current(
    _host: &cpal::Host,
    _selected_name: &str,
    _selected_device: &cpal::Device,
) -> bool {
    true
}

/// Preserve the chunks already written when the input device disappears. The
/// in-memory recording may be incomplete, but the durable chunks remain
/// retryable after the next launch instead of being deleted as a cancellation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StopCaptureDisposition {
    PreserveDeviceFailure,
    PreserveClosedLid,
    Cancel,
    Finalize,
}

fn stop_capture_disposition(
    device_failed: bool,
    completed: bool,
    lid: crate::clamshell::LidState,
    actual_device: Option<&crate::clamshell::CoreAudioDeviceIdentity>,
    owned_replay: bool,
) -> StopCaptureDisposition {
    if device_failed {
        StopCaptureDisposition::PreserveDeviceFailure
    } else if !completed {
        StopCaptureDisposition::Cancel
    } else if !owned_replay && crate::clamshell::should_reject_closed_lid_input(lid, actual_device)
    {
        StopCaptureDisposition::PreserveClosedLid
    } else {
        StopCaptureDisposition::Finalize
    }
}

fn preserve_partial_recording(mut rec: ActiveRec) {
    close_capture_continuity(&rec);
    drop(rec.stream.take());
    match rec.capture.finish() {
        Ok(result) => {
            let disposition = if result.processing_error.is_none()
                && result.spool_finalizer.failure().is_none()
            {
                SpoolDisposition::Recoverable
            } else {
                SpoolDisposition::Degraded
            };
            let _ = result.spool_finalizer.resolve(disposition);
        }
        Err(error) => {
            log::warn!(
                "audio capture close is pending; recovery status remains unresolved: {error}"
            );
        }
    }
}

fn finalize(rec: ActiveRec, vad_enabled: bool) -> Result<FinishedCapture, AudioError> {
    close_capture_continuity(&rec);
    let cue_range = rec
        .cue
        .as_ref()
        .map(CaptureCueHandle::freeze)
        .and_then(|state| state.confirmed);
    let app = rec.app.clone();
    let session_generation = rec.session_generation;
    let input_selection = rec.input_selection.clone();
    let actual_device_name = rec.actual_device_name.clone();
    let device_metadata = rec.device_metadata.clone();
    let rate = rec.rate;
    let channels = rec.channels;
    let ActiveRec {
        stream,
        device_failed: _,
        capture,
        chunk_length_secs,
        preserve_full_audio,
        ..
    } = rec;
    drop(stream); // no callback can append after the stream is dropped.
    let capture = capture.finish().map_err(AudioError::Device)?;
    let CaptureResult {
        samples: mut captured_samples,
        processing_error,
        session_dir,
        spool_finalizer,
        tail_flush,
    } = capture;
    mask_cue_range(&mut captured_samples, cue_range, 0);
    if let Some(error) = processing_error {
        let _ = spool_finalizer.resolve(SpoolDisposition::Degraded);
        return Err(AudioError::Device(error));
    }
    if let Some(error) = spool_finalizer.failure() {
        log::warn!("audio spool finalization was partial: {error}");
    }
    let samples = if preserve_full_audio {
        captured_samples
    } else {
        crate::vad::filter_speech(&captured_samples, vad_enabled)
    };
    if samples.is_empty() {
        let _ = spool_finalizer.resolve(SpoolDisposition::Completed);
        emit_capture_state(
            &app,
            session_generation,
            "no_speech",
            &input_selection,
            actual_device_name.as_deref(),
            &device_metadata,
            (rate, channels),
        );
        return Err(AudioError::EmptyRecording);
    }
    // Samples are already at 16 kHz (resampled inline during capture).
    let mut chunker = Chunker::new(ChunkerConfig { chunk_length_secs });
    let mut chunks = chunker.push(&samples);
    if let Some(last) = chunker.finish() {
        chunks.push(last);
    }
    let wav = match encode(samples) {
        Ok(wav) => wav,
        Err(error) => {
            let _ = spool_finalizer.resolve(SpoolDisposition::Recoverable);
            return Err(error);
        }
    };
    let session_id = session_dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| AudioError::Device("invalid capture spool session path".into()))?
        .to_owned();
    Ok(FinishedCapture {
        wav,
        chunks,
        device_metadata,
        input_selection,
        actual_device_name,
        capture_spool: Some(CaptureSpoolLease {
            session_dir,
            session_id,
            finalizer: Some(spool_finalizer),
            tail_flush: Some(tail_flush),
            recover_on_drop: false,
        }),
    })
}

pub struct Recorder {
    active: Option<StartHandle>,
    readiness: Option<mpsc::Sender<()>>,
}
impl Recorder {
    pub fn new() -> Self {
        Self {
            active: None,
            readiness: None,
        }
    }

    pub(crate) fn feedback_cue_handle(&self, session_generation: u64) -> Option<CaptureCueHandle> {
        self.active
            .as_ref()?
            .feedback_cue_handle(session_generation)
    }
    pub fn arm_readiness(&mut self) -> mpsc::Receiver<()> {
        let (tx, rx) = mpsc::channel();
        self.readiness = Some(tx);
        rx
    }
    pub fn start(
        &mut self,
        app: AppHandle,
        session: &str,
        input_device: &str,
        chunk_length_secs: usize,
        input_gain: f32,
        prefetch_tx: PrefetchInbox,
    ) -> Result<(), AudioError> {
        self.start_with_soniox(
            app,
            session,
            0,
            input_device,
            input_device,
            false,
            chunk_length_secs,
            input_gain,
            prefetch_tx,
            None,
            false,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start_with_soniox(
        &mut self,
        app: AppHandle,
        session: &str,
        session_generation: u64,
        input_device: &str,
        requested_selection: &str,
        clamshell_substituted: bool,
        chunk_length_secs: usize,
        input_gain: f32,
        prefetch_tx: PrefetchInbox,
        soniox_audio: Option<crate::soniox::SonioxAudioSender>,
        preserve_full_audio: bool,
        owned_wav: Option<OwnedWavReplay>,
    ) -> Result<(), AudioError> {
        if self.active.is_some() {
            return Err(AudioError::Device("recording already active".into()));
        }
        let (tx, rx) = mpsc::channel();
        engine()
            .tx
            .send(EngineCmd::Start {
                app: app.clone(),
                session: session.to_string(),
                session_generation,
                input_device: input_device.to_owned(),
                requested_selection: requested_selection.to_owned(),
                clamshell_substituted,
                chunk_length_secs,
                max_recording_secs: MAX_RECORDING_SECS,
                input_gain,
                prefetch_tx,
                soniox_audio,
                preserve_full_audio,
                owned_wav,
                readiness: self.readiness.take(),
                reply: tx,
            })
            .map_err(|_| AudioError::Device("audio engine stopped".into()))?;
        let handle = match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(result) => result?,
            Err(_) => {
                let (cancel_tx, cancel_rx) = mpsc::channel();
                let _ = engine().tx.send(EngineCmd::Cancel { reply: cancel_tx });
                let _ = cancel_rx.recv_timeout(Duration::from_secs(5));
                return Err(AudioError::Device("audio engine start timed out".into()));
            }
        };
        self.active = Some(handle);
        Ok(())
    }
    pub fn stop_with_chunks(
        &mut self,
        _app: Option<&AppHandle>,
        options: StopOptions,
    ) -> Result<FinishedCapture, AudioError> {
        let handle = self.active.take().ok_or(AudioError::NotRecording)?;
        let (tx, rx) = mpsc::channel::<StopResult>();
        if engine()
            .tx
            .send(EngineCmd::Stop { options, reply: tx })
            .is_err()
        {
            return Err(AudioError::Device("audio engine stopped".into()));
        }
        match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(result) => {
                let _ = handle;
                result
            }
            Err(_) => {
                let (cancel_tx, cancel_rx) = mpsc::channel();
                let _ = engine().tx.send(EngineCmd::Cancel { reply: cancel_tx });
                let _ = cancel_rx.recv_timeout(Duration::from_secs(5));
                Err(AudioError::Device("audio engine stop timed out".into()))
            }
        }
    }
    pub fn cancel(&mut self, _app: Option<&AppHandle>) {
        if self.active.take().is_some() {
            let (tx, rx) = mpsc::channel();
            if engine().tx.send(EngineCmd::Cancel { reply: tx }).is_ok() {
                // Device-failure recovery may still be finishing one queued
                // spool write. Give the owner a short bounded window before
                // callers inspect recovery, while leaving the audio engine
                // free and reporting unresolved storage as unavailable.
                if rx.recv_timeout(Duration::from_millis(500)).is_ok() {
                    let _ = wait_for_audio_spool_owner(CANCEL_SPOOL_CLOSE_BUDGET);
                }
            }
        }
    }
}

#[cfg(all(feature = "research-harness", target_os = "macos"))]
pub(crate) fn begin_owned_source(session_generation: u64) -> Result<OwnedReplayStarted, String> {
    let (reply, receiver) = mpsc::channel();
    let (completion, completion_receiver) = mpsc::channel();
    engine()
        .tx
        .send(EngineCmd::BeginOwnedSource {
            session_generation,
            completion,
            reply,
        })
        .map_err(|_| "audio_engine_stopped".to_owned())?;
    let stream_id = receiver
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "owned_source_start_ack_timeout".to_owned())??;
    Ok(OwnedReplayStarted {
        receipt: OwnedReplayStartReceipt {
            session_generation,
            stream_id,
            source_identity: OWNED_WAV_SOURCE_SHA256,
        },
        completion: completion_receiver,
    })
}

fn push<T: Copy + cpal::Sample>(
    input: &[T],
    channels: usize,
    capture_time: StreamInstant,
    recycle_rx: &mut mpsc::Receiver<Vec<f32>>,
    state: &CapturePushState,
) -> bool
where
    f32: cpal::FromSample<T>,
{
    if input.is_empty() {
        return false;
    }
    if state.capture_overflowed.load(Ordering::Acquire)
        || state.capture_limit_reached.load(Ordering::Acquire)
    {
        return false;
    }
    // Down-mix to mono at the native rate.
    let expected_frame_count = input.len() / channels.max(1);
    let mut mono = recycle_rx
        .try_recv()
        .unwrap_or_else(|_| Vec::with_capacity(expected_frame_count));
    mono.clear();
    if mono.capacity() < expected_frame_count {
        mono.reserve(expected_frame_count - mono.capacity());
    }
    for frame in input.chunks(channels.max(1)) {
        let v = frame.iter().map(|v| (*v).to_sample::<f32>()).sum::<f32>() / frame.len() as f32;
        mono.push(v);
    }
    let frame_count = mono.len();
    let raw_level = normalized_audio_level(&mono);
    let previous_level = f32::from_bits(state.level.load(Ordering::Relaxed));
    let smoothed_level = smooth_audio_level(previous_level, raw_level);
    state
        .level
        .store(smoothed_level.to_bits(), Ordering::Relaxed);
    let (newly_overflowed, accepted, stale, now_elapsed) = {
        // Serialize callback admission and its clock accounting with the
        // intentional-limit endpoint. A callback that wins this lock is fully
        // enqueued and timestamped before the worker can close admission.
        let mut continuity = state.continuity.lock().unwrap_or_else(|e| e.into_inner());
        if continuity.closed || state.capture_limit_reached.load(Ordering::Acquire) {
            // This terminal block was never accepted into the capture queue.
            // Drop its owned buffer; the receiver cannot return it to the pool.
            drop(mono);
            return false;
        }
        let newly_overflowed = match try_enqueue_capture_command(
            &state.capture_tx,
            &state.capture_overflowed,
            CaptureCommand::Samples(mono),
        ) {
            Ok(()) => false,
            Err(newly_overflowed) => newly_overflowed,
        };
        let accepted = !newly_overflowed && !state.capture_overflowed.load(Ordering::Acquire);
        let native_before = continuity.accepted_native_frames;
        let now_elapsed = state.started_at.elapsed();
        let stale = accepted && {
            continuity.accepted_native_frames = continuity
                .accepted_native_frames
                .saturating_add(frame_count as u64);
            if let Some(cue) = &state.cue_handle {
                cue.accept_packet_locked(&mut continuity, capture_time, frame_count, native_before);
            }
            continuity.accepted_frame(now_elapsed)
        };
        (newly_overflowed, accepted, stale, now_elapsed)
    };
    if stale {
        state.stale_gap_latched.store(true, Ordering::Release);
    }
    if accepted && !state.first_frame_reported.swap(true, Ordering::AcqRel) {
        let _ = state.engine_tx.send(EngineCmd::CaptureFrameObserved {
            stream_id: state.stream_id,
            session_generation: state.session_generation,
        });
    }
    if accepted {
        let elapsed_ms = state
            .ui_started_at
            .elapsed()
            .as_millis()
            .min(u64::MAX as u128) as u64;
        observe_rate_drift(
            &state.rate_drift,
            &state.rate_drift_latched,
            now_elapsed,
            frame_count,
            state.native_rate,
            || {
                let _ = state.engine_tx.send(EngineCmd::CaptureError {
                    stream_id: state.stream_id,
                    session_generation: state.session_generation,
                    app: state.app.clone(),
                    message: "audio capture rate drift exceeded the sustained 750ms tolerance; the recording is incomplete".into(),
                });
            },
        );
        state
            .last_frame_elapsed_ms
            .store(elapsed_ms, Ordering::Release);
    }
    if newly_overflowed {
        let _ = state.engine_tx.send(EngineCmd::CaptureError {
            stream_id: state.stream_id,
            session_generation: state.session_generation,
            app: state.app.clone(),
            message: "audio processing queue overflowed".into(),
        });
    }
    accepted
}

/// The callback queue is a finite capture boundary. A failed enqueue latches a
/// continuity error that survives a stop racing the UI error event.
fn try_enqueue_capture_command(
    tx: &mpsc::SyncSender<CaptureCommand>,
    overflowed: &AtomicBool,
    command: CaptureCommand,
) -> Result<(), bool> {
    match tx.try_send(command) {
        Ok(()) => Ok(()),
        Err(mpsc::TrySendError::Full(_) | mpsc::TrySendError::Disconnected(_)) => {
            Err(!overflowed.swap(true, Ordering::AcqRel))
        }
    }
}

fn normalized_audio_level(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sum_squares: f32 = 0.0;
    let mut peak: f32 = 0.0;
    for sample in samples {
        sum_squares += sample * sample;
        peak = peak.max(sample.abs());
    }
    let rms = (sum_squares / samples.len() as f32).sqrt();
    let amplitude = rms * 0.8 + peak * 0.2;
    const NOISE_FLOOR: f32 = 0.008;
    const SPEECH_CEILING: f32 = 0.20;
    ((amplitude - NOISE_FLOOR) / (SPEECH_CEILING - NOISE_FLOOR)).clamp(0.0, 1.0)
}

fn smooth_audio_level(previous: f32, current: f32) -> f32 {
    let previous = previous.clamp(0.0, 1.0);
    let current = current.clamp(0.0, 1.0);
    let response = if current > previous { 0.48 } else { 0.20 };
    previous + (current - previous) * response
}

fn apply_input_gain(samples: &mut [f32], gain: f32) {
    // Run unity gain through the limiter too: input samples can still contain
    // non-finite values or exceed full scale, and bypassing here made the
    // transfer function jump backwards when gain moved just above 1.0.
    let gain = if gain.is_finite() {
        gain.clamp(0.0, 4.0)
    } else {
        1.0
    };
    for sample in samples.iter_mut() {
        *sample = soft_limit_sample(*sample * gain);
    }
}

fn soft_limit_sample(sample: f32) -> f32 {
    if sample.is_nan() {
        return 0.0;
    }
    if sample.is_infinite() {
        return sample.signum();
    }
    const SOFT_KNEE: f32 = 0.9;
    if sample.abs() <= SOFT_KNEE {
        return sample;
    }
    let sign = sample.signum();
    let span = 1.0 - SOFT_KNEE;
    let over = (sample.abs() - SOFT_KNEE) / span;
    let compressed = SOFT_KNEE + span * -(-over).exp_m1();
    sign * compressed
}

fn append_bounded(samples: &mut Vec<f32>, produced: &[f32], max_samples: usize) -> bool {
    let remaining = max_samples.saturating_sub(samples.len());
    samples.extend_from_slice(&produced[..produced.len().min(remaining)]);
    produced.len() > remaining
}

fn append_capture_samples(
    data: &mut Vec<f32>,
    produced: &[f32],
    max_samples: usize,
    spool_pending: &Arc<Mutex<PendingSpool>>,
    spool_sender: Option<&SpoolSender>,
) -> (u64, usize, bool) {
    let start_sample = data.len() as u64;
    let accepted = produced.len().min(max_samples.saturating_sub(data.len()));
    let hit_limit = append_bounded(data, produced, max_samples);
    // Keep the complete in-memory capture authoritative before best-effort
    // spool admission. A blocked or full disk worker cannot drop this timeline.
    append_spool(spool_pending, spool_sender, &produced[..accepted]);
    (start_sample, accepted, hit_limit)
}

fn encode(input: Vec<f32>) -> Result<Vec<u8>, AudioError> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer =
        hound::WavWriter::new(&mut bytes, spec).map_err(|e| AudioError::Encode(e.to_string()))?;
    for x in input {
        writer
            .write_sample((x.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .map_err(|e| AudioError::Encode(e.to_string()))?;
    }
    writer
        .finalize()
        .map_err(|e| AudioError::Encode(e.to_string()))?;
    Ok(bytes.into_inner())
}

#[cfg(test)]
mod tests {
    use super::{
        append_bounded, apply_capture_continuity_failure, apply_input_gain, capture_state_payload,
        decode_owned_wav_bytes, expected_resampled_samples, normalized_audio_level,
        smooth_audio_level, soft_limit_sample, AudioError, CaptureContinuityTracker,
        CaptureDeviceMetadata, OwnedReplayLifecycleEvent, OwnedReplayStartReceipt,
        RateDriftTracker, StreamResampler, TARGET_RATE,
    };
    use sha2::Digest;
    use std::sync::atomic::Ordering;
    use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
    use std::time::Duration;

    struct CaptureStartGate {
        session_dir: std::path::PathBuf,
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    }

    static CAPTURE_START_GATE: OnceLock<Mutex<Option<CaptureStartGate>>> = OnceLock::new();
    static CAPTURE_TEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[test]
    fn stop_lid_check_rejects_only_the_actual_builtin_capture_and_preserves_first_error() {
        use super::StopCaptureDisposition as Stop;
        use crate::clamshell::{CoreAudioDeviceIdentity, DeviceTransport, LidState};

        let builtin = CoreAudioDeviceIdentity {
            device_id: 1,
            name: "Internal Mic".into(),
            uid: "builtin-uid".into(),
            transport: DeviceTransport::BuiltIn,
        };
        let external = CoreAudioDeviceIdentity {
            device_id: 2,
            name: "USB Mic".into(),
            uid: "usb-uid".into(),
            transport: DeviceTransport::NonBuiltIn,
        };
        let unknown = CoreAudioDeviceIdentity {
            device_id: 3,
            name: "Unknown Mic".into(),
            uid: "unknown-uid".into(),
            transport: DeviceTransport::Unknown,
        };
        assert_eq!(
            super::stop_capture_disposition(false, true, LidState::Closed, Some(&builtin), false),
            Stop::PreserveClosedLid
        );
        assert_eq!(
            super::stop_capture_disposition(false, true, LidState::Open, Some(&builtin), false),
            Stop::Finalize
        );
        assert_eq!(
            super::stop_capture_disposition(false, true, LidState::Closed, Some(&external), false),
            Stop::Finalize
        );
        assert_eq!(
            super::stop_capture_disposition(false, true, LidState::Closed, Some(&unknown), false),
            Stop::Finalize
        );
        assert_eq!(
            super::stop_capture_disposition(false, true, LidState::Closed, None, false),
            Stop::Finalize
        );
        assert_eq!(
            super::stop_capture_disposition(false, true, LidState::Closed, Some(&builtin), true),
            Stop::Finalize
        );
        // A prior capture failure wins over a newly observed closed lid.
        assert_eq!(
            super::stop_capture_disposition(true, true, LidState::Closed, Some(&builtin), false),
            Stop::PreserveDeviceFailure
        );
        assert_eq!(
            super::stop_capture_disposition(false, false, LidState::Open, Some(&builtin), false),
            Stop::Cancel
        );
    }

    #[test]
    fn closed_lid_completion_is_incomplete_without_overwriting_an_earlier_capture_error() {
        let mut error = None;
        super::apply_capture_completion_failures(&mut error, false, false, true, false);
        assert!(error
            .as_deref()
            .is_some_and(|value| value.contains("lid closed")));
        let mut first_error = Some("resampler failed".to_owned());
        super::apply_capture_completion_failures(&mut first_error, false, false, true, false);
        assert_eq!(first_error.as_deref(), Some("resampler failed"));
    }

    #[test]
    fn capture_state_payload_preserves_requested_and_effective_devices() {
        let payload = capture_state_payload(
            17,
            "waiting_first_frame",
            "external:usb-mic",
            Some("USB Microphone"),
            &CaptureDeviceMetadata {
                requested_selection: Some("built-in-microphone".to_owned()),
                clamshell_substituted: true,
                system_default_name: Some("USB Microphone".to_owned()),
            },
            48_000,
            2,
        );

        assert_eq!(payload["session_generation"], 17);
        assert_eq!(payload["input_selection"], "external:usb-mic");
        assert_eq!(payload["device_name"], "USB Microphone");
        assert_eq!(payload["requested_selection"], "built-in-microphone");
        assert_eq!(payload["clamshell_substituted"], true);
        assert_eq!(payload["system_default_name"], "USB Microphone");
        assert_eq!(payload["sample_rate"], 48_000);
        assert_eq!(payload["channels"], 2);
    }

    #[test]
    fn capture_state_payload_keeps_default_distinct_from_selected_device() {
        let payload = capture_state_payload(
            18,
            "receiving_audio",
            "external:chosen-mic",
            Some("Chosen Microphone"),
            &CaptureDeviceMetadata {
                requested_selection: Some("system-default".to_owned()),
                clamshell_substituted: false,
                system_default_name: Some("System Default Microphone".to_owned()),
            },
            44_100,
            1,
        );

        assert_eq!(payload["requested_selection"], "system-default");
        assert_eq!(payload["input_selection"], "external:chosen-mic");
        assert_eq!(payload["device_name"], "Chosen Microphone");
        assert_eq!(payload["system_default_name"], "System Default Microphone");
    }

    pub(super) fn wait_for_capture_start_gate(session_dir: &std::path::Path) {
        let gate = {
            let mut slot = CAPTURE_START_GATE
                .get_or_init(Default::default)
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot
                .as_ref()
                .is_some_and(|gate| gate.session_dir == session_dir)
            {
                slot.take()
            } else {
                None
            }
        };
        if let Some(gate) = gate {
            let _ = gate.entered.send(());
            let _ = gate
                .release
                .recv_timeout(std::time::Duration::from_secs(10));
        }
    }

    struct BlockingSpoolIo {
        initialize_entered: Mutex<Option<mpsc::Sender<()>>>,
        initialize_release: Option<Arc<(Mutex<bool>, Condvar)>>,
        store_initialize: bool,
        initialize_complete: Mutex<Option<mpsc::Sender<bool>>>,
        entered: Mutex<Option<mpsc::Sender<()>>>,
        release: Arc<(Mutex<bool>, Condvar)>,
        finish_entered: Mutex<Option<mpsc::Sender<()>>>,
        finish_release: Option<Arc<(Mutex<bool>, Condvar)>>,
        finished: mpsc::Sender<(super::SpoolDisposition, bool)>,
    }

    impl super::AudioSpoolIo for BlockingSpoolIo {
        fn initialize(
            &self,
            lease: &crate::store::AudioSpoolSessionLease,
            _root: &std::path::Path,
            _session_dir: &std::path::Path,
        ) -> Result<(), String> {
            if let Some(entered) = self
                .initialize_entered
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
            {
                let _ = entered.send(());
            }
            let initialized = if self.store_initialize {
                crate::store::initialize_audio_spool_session(lease)
                    .map_err(|error| error.to_string())
            } else {
                Ok(())
            };
            if let Some(complete) = self
                .initialize_complete
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
            {
                let _ = complete.send(initialized.is_ok());
            }
            if let Some(release) = &self.initialize_release {
                let (released, condition) = &**release;
                let mut released = released
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                while !*released {
                    released = condition
                        .wait(released)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
            }
            initialized
        }

        fn write_chunk(
            &self,
            _lease: &crate::store::AudioSpoolSessionLease,
            _root: &std::path::Path,
            _session_dir: &std::path::Path,
            _index: usize,
            _start_secs: f32,
            _end_secs: f32,
            _samples: &[f32],
        ) -> Result<(), String> {
            if let Some(entered) = self
                .entered
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
            {
                let _ = entered.send(());
            }
            let (released, condition) = &*self.release;
            let mut released = released
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            while !*released {
                released = condition
                    .wait(released)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            Ok(())
        }

        fn finish(
            &self,
            lease: &crate::store::AudioSpoolSessionLease,
            session_dir: &std::path::Path,
            disposition: super::SpoolDisposition,
            incomplete: bool,
        ) -> Result<(), String> {
            if let Some(release) = &self.finish_release {
                if let Some(entered) = self
                    .finish_entered
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take()
                {
                    let _ = entered.send(());
                }
                let (released, condition) = &**release;
                let mut released = released
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                while !*released {
                    released = condition
                        .wait(released)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
            }
            if lease.deletion_requested()
                || matches!(
                    disposition,
                    super::SpoolDisposition::Cancelled | super::SpoolDisposition::Completed
                )
            {
                match std::fs::remove_dir_all(session_dir) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
            self.finished
                .send((disposition, incomplete))
                .map_err(|_| "test finalizer observer closed".to_owned())
        }
    }

    fn spool_test_session(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "voiceflow-audio-lifecycle-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("test spool root");
        let session = crate::store::begin_spool_session(&root, name).expect("test session");
        (root, session)
    }

    fn await_spool_registry(registry: &super::SpoolOwnerRegistry) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while registry.ensure_available().is_err() {
            assert!(
                std::time::Instant::now() < deadline,
                "spool owner did not close"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    fn release_spool_io(release: &Arc<(Mutex<bool>, Condvar)>) {
        let (released, condition) = &**release;
        *released
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        condition.notify_all();
    }

    fn wait_for_audio_owners() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while super::CAPTURE_OWNER_REGISTRY
            .get_or_init(Default::default)
            .ensure_available()
            .is_err()
            || super::REPLAY_OWNER_REGISTRY
                .get_or_init(Default::default)
                .ensure_available()
                .is_err()
            || super::spool_owner_registry().ensure_available().is_err()
        {
            assert!(
                std::time::Instant::now() < deadline,
                "audio owners did not close after releasing the injected gate"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canceled_recovery_copy_claim_releases_for_next_real_capture_worker() {
        use super::{CaptureCommand, CaptureWorker, SpoolDisposition, TARGET_RATE};
        use crate::dictation::OperationLease;

        let delivery_owner = crate::DeliverySettlementOwner::new(
            crate::delivery_settlement::DictationTransactionId::fresh(),
        );
        let delivery_transaction_id = delivery_owner
            .transaction_token()
            .expect("the recovery ticket has an opaque delivery identity");
        let history_transaction_id = "history-copy-abort-test".to_owned();
        let ticket = crate::HistoryRecoveryTicket {
            revision: 11,
            session_generation: 919,
            delivery_transaction_id: delivery_transaction_id.clone(),
            history_transaction_id: history_transaction_id.clone(),
            delivery_owner: delivery_owner.clone(),
            history_state: crate::HistoryRecoveryStage::Pending,
            copy_status: crate::HistoryRecoveryCopyStatus::Copying,
            copy_attempt: 3,
            copy_revision: Some(11),
            visible: true,
        };
        let copy_claim = crate::HistoryRecoveryCopyClaim::from_ticket(&ticket);
        let history_ticket = Arc::new(Mutex::new(Some(ticket)));
        let operation = Arc::new(Mutex::new(OperationLease::Idle));
        assert!(crate::dictation::claim_operation(
            &mut operation.lock().unwrap(),
            OperationLease::HistoryMutation,
        ));
        let guard_operation = operation.clone();
        let mut guard = crate::HistoryRecoveryOperationGuard::with_release(move || {
            crate::dictation::release_operation_lease(
                &mut guard_operation.lock().unwrap(),
                OperationLease::HistoryMutation,
            );
        });
        let rollback_ticket = Arc::clone(&history_ticket);
        guard.set_pre_worker_rollback(move || {
            crate::rollback_history_recovery_copy_before_worker(
                &mut rollback_ticket.lock().unwrap(),
                &copy_claim,
                12,
            );
        });
        let copy_wait = tokio::spawn(async move {
            let _guard = guard;
            std::future::pending::<()>().await
        });
        tokio::task::yield_now().await;
        assert_eq!(*operation.lock().unwrap(), OperationLease::HistoryMutation);
        copy_wait.abort();
        let _ = copy_wait.await;
        assert_eq!(
            *operation.lock().unwrap(),
            OperationLease::Idle,
            "cancelling before clipboard admission releases HistoryMutation"
        );
        let rolled_back_ticket = history_ticket
            .lock()
            .unwrap()
            .clone()
            .expect("the same recovery ticket remains available after pre-worker cancellation");
        assert_eq!(
            rolled_back_ticket.copy_status,
            crate::HistoryRecoveryCopyStatus::Failed,
            "a pre-worker abort reports a known no-side-effect retryable result"
        );
        assert_eq!(rolled_back_ticket.revision, 12);
        assert_eq!(rolled_back_ticket.copy_attempt, 3);
        assert_eq!(rolled_back_ticket.copy_revision, None);
        assert!(rolled_back_ticket.visible);
        let retry_request = crate::HistoryRecoveryCopyRequest {
            ticket_revision: rolled_back_ticket.revision,
            session_generation: rolled_back_ticket.session_generation,
            delivery_transaction_id: delivery_transaction_id.clone(),
            history_transaction_id: history_transaction_id.clone(),
        };
        assert!(rolled_back_ticket.admits_copy(&retry_request, &delivery_owner));
        assert!(crate::dictation::claim_operation(
            &mut operation.lock().unwrap(),
            OperationLease::LiveDictation,
        ));

        let _serial = CAPTURE_TEST_SERIAL.lock().await;
        wait_for_audio_owners();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "voiceflow-copy-cancel-next-capture-{}-{nonce}",
            std::process::id()
        ));
        let session_id = format!("after-copy-cancel-{nonce}");
        let (session_dir, lease) = crate::store::begin_audio_spool_session(&root, &session_id)
            .expect("the next capture reserves its own durable audio session");
        let (prefetch_tx, _prefetch_rx) = crate::prefetch_asr::PrefetchAsrSession::channel();
        let (event_tx, _event_rx) = mpsc::channel();
        let capture = CaptureWorker::new(
            root.clone(),
            session_dir,
            lease,
            TARGET_RATE,
            false,
            super::MAX_RECORDING_SECS * TARGET_RATE as usize,
            30,
            1.0,
            prefetch_tx,
            None,
            true,
            919,
            920,
            event_tx,
            None,
            None,
            None,
        )
        .expect("the actual CaptureWorker is admitted after copy cancellation");
        let pcm = vec![0.375; TARGET_RATE as usize];
        capture
            .tx
            .try_send(CaptureCommand::Samples(pcm.clone()))
            .expect("the capture queue accepts the full fake PCM block");
        let result = capture.finish().expect("the capture worker closes cleanly");
        assert_eq!(result.samples.len(), pcm.len());
        assert!(result.samples.iter().all(|sample| *sample == 0.375));
        assert!(
            result.tail_flush.wait().1.is_ok(),
            "the accepted capture tail reaches the spool writer"
        );
        assert!(result
            .spool_finalizer
            .resolve(SpoolDisposition::Recoverable));
        result
            .spool_finalizer
            .wait_closed_blocking()
            .expect("the next capture's spool writer closes after the tail ACK");
        crate::dictation::release_operation_lease(
            &mut operation.lock().unwrap(),
            OperationLease::LiveDictation,
        );
        assert_eq!(*operation.lock().unwrap(), OperationLease::Idle);
        crate::store::remove_spool_artifact(&root, &root.join("spool").join(&session_id));
        crate::store::clear_all_data(&root).expect("clear the isolated test root");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn admitted_recovery_copy_worker_keeps_claim_until_its_terminal_result() {
        use crate::dictation::OperationLease;

        let delivery_owner = crate::DeliverySettlementOwner::new(
            crate::delivery_settlement::DictationTransactionId::fresh(),
        );
        let ticket = crate::HistoryRecoveryTicket {
            revision: 21,
            session_generation: 921,
            delivery_transaction_id: delivery_owner
                .transaction_token()
                .expect("the recovery ticket has an opaque delivery identity"),
            history_transaction_id: "history-copy-admitted-test".to_owned(),
            delivery_owner,
            history_state: crate::HistoryRecoveryStage::Pending,
            copy_status: crate::HistoryRecoveryCopyStatus::Copying,
            copy_attempt: 2,
            copy_revision: Some(21),
            visible: true,
        };
        let copy_claim = crate::HistoryRecoveryCopyClaim::from_ticket(&ticket);
        let history_ticket = Arc::new(Mutex::new(Some(ticket)));
        let operation = Arc::new(Mutex::new(OperationLease::Idle));
        assert!(crate::dictation::claim_operation(
            &mut operation.lock().unwrap(),
            OperationLease::HistoryMutation,
        ));
        let guard_operation = Arc::clone(&operation);
        let mut guard = crate::HistoryRecoveryOperationGuard::with_release(move || {
            crate::dictation::release_operation_lease(
                &mut guard_operation.lock().unwrap(),
                OperationLease::HistoryMutation,
            );
        });
        let rollback_ticket = Arc::clone(&history_ticket);
        let rollback_claim = copy_claim.clone();
        guard.set_pre_worker_rollback(move || {
            crate::rollback_history_recovery_copy_before_worker(
                &mut rollback_ticket.lock().unwrap(),
                &rollback_claim,
                23,
            );
        });

        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker_ticket = Arc::clone(&history_ticket);
        let worker = tokio::task::spawn_blocking(move || {
            let mut operation_guard = guard;
            operation_guard.mark_worker_started();
            entered_tx
                .send(())
                .expect("the test observes the admitted worker");
            release_rx
                .recv()
                .expect("the test releases the blocked admitted worker");
            assert!(crate::finish_history_recovery_copy_attempt(
                &mut worker_ticket.lock().unwrap(),
                &copy_claim,
                crate::HistoryRecoveryCopyStatus::Unknown,
                22,
            )
            .is_some());
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the blocking closure has crossed worker admission");
        assert_eq!(*operation.lock().unwrap(), OperationLease::HistoryMutation);
        let pending_ticket = history_ticket.lock().unwrap().clone().unwrap();
        assert_eq!(
            pending_ticket.copy_status,
            crate::HistoryRecoveryCopyStatus::Copying
        );
        assert_eq!(pending_ticket.copy_revision, Some(21));
        assert_eq!(pending_ticket.revision, 21);

        release_tx.send(()).unwrap();
        worker
            .await
            .expect("the admitted worker returns its terminal result");
        assert_eq!(*operation.lock().unwrap(), OperationLease::Idle);
        let terminal_ticket = history_ticket.lock().unwrap().clone().unwrap();
        assert_eq!(
            terminal_ticket.copy_status,
            crate::HistoryRecoveryCopyStatus::Unknown
        );
        assert_eq!(terminal_ticket.copy_revision, None);
        assert_eq!(terminal_ticket.revision, 22);
    }

    #[test]
    fn pre_worker_copy_rollback_does_not_rewrite_a_hidden_or_replaced_ticket() {
        let owner_a = crate::DeliverySettlementOwner::new(
            crate::delivery_settlement::DictationTransactionId::fresh(),
        );
        let ticket_a = crate::HistoryRecoveryTicket {
            revision: 31,
            session_generation: 931,
            delivery_transaction_id: owner_a.transaction_token().unwrap(),
            history_transaction_id: "history-copy-ticket-a".to_owned(),
            delivery_owner: owner_a,
            history_state: crate::HistoryRecoveryStage::Pending,
            copy_status: crate::HistoryRecoveryCopyStatus::Copying,
            copy_attempt: 4,
            copy_revision: Some(31),
            visible: true,
        };
        let claim_a = crate::HistoryRecoveryCopyClaim::from_ticket(&ticket_a);

        let owner_b = crate::DeliverySettlementOwner::new(
            crate::delivery_settlement::DictationTransactionId::fresh(),
        );
        let ticket_b = crate::HistoryRecoveryTicket {
            revision: 32,
            session_generation: 932,
            delivery_transaction_id: owner_b.transaction_token().unwrap(),
            history_transaction_id: "history-copy-ticket-b".to_owned(),
            delivery_owner: owner_b.clone(),
            history_state: crate::HistoryRecoveryStage::Failed,
            copy_status: crate::HistoryRecoveryCopyStatus::Ready,
            copy_attempt: 0,
            copy_revision: None,
            visible: true,
        };
        let newer_ticket = Some(ticket_b);
        let mut current = newer_ticket.clone();
        assert!(
            crate::rollback_history_recovery_copy_before_worker(&mut current, &claim_a, 33,)
                .is_none()
        );
        let preserved = current.unwrap();
        assert_eq!(preserved.session_generation, 932);
        assert_eq!(preserved.revision, 32);
        assert_eq!(
            preserved.copy_status,
            crate::HistoryRecoveryCopyStatus::Ready
        );
        assert!(Arc::ptr_eq(&preserved.delivery_owner, &owner_b));

        let mut hidden = ticket_a.clone();
        hidden.visible = false;
        let mut hidden_ticket = Some(hidden);
        assert!(crate::rollback_history_recovery_copy_before_worker(
            &mut hidden_ticket,
            &claim_a,
            34,
        )
        .is_none());
        let hidden_ticket = hidden_ticket.unwrap();
        assert!(!hidden_ticket.visible);
        assert_eq!(hidden_ticket.revision, 31);
        assert_eq!(
            hidden_ticket.copy_status,
            crate::HistoryRecoveryCopyStatus::Copying
        );
        assert_eq!(hidden_ticket.copy_revision, Some(31));
    }

    #[test]
    fn actual_capture_worker_persists_pcm_while_another_session_holds_spool_io_and_quota() {
        use super::{
            CaptureCommand, CaptureWorker, SpoolDisposition, SpoolOwnerRegistry, StoreAudioSpoolIo,
        };
        use std::time::{Duration, Instant};

        let _serial = CAPTURE_TEST_SERIAL.blocking_lock();
        wait_for_audio_owners();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "voiceflow-live-session-spool-isolation-{}-{nonce}",
            std::process::id()
        ));
        let old_id = format!("older-spool-owner-{nonce}");
        let (old_session, old_lease) = crate::store::begin_audio_spool_session(&root, &old_id)
            .expect("old owner reserves a real session path");
        crate::store::initialize_audio_spool_session(&old_lease)
            .expect("old session has a durable manifest");

        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        crate::store::pause_next_spool_write_after_quota_reservation_for_test(
            &old_session,
            entered_tx,
            release_rx,
        );
        let old_root = root.clone();
        let old_session_id = old_id.clone();
        let old_writer_lease = old_lease.clone();
        let old_writer = std::thread::spawn(move || {
            let bytes = b"older retained spool write";
            crate::store::write_audio_spool_file(
                &old_writer_lease,
                &old_root,
                &std::path::PathBuf::from(old_session_id).join("recovery.wav"),
                bytes,
            )
        });
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("old job pauses after the actual session permit and quota reservation");
        assert!(
            crate::store::recovery_spool_reserved_bytes_for_test(&root) > 0,
            "the old writer keeps its immutable quota charge while the write is paused"
        );

        let new_id = format!("new-capture-spool-{nonce}");
        let (new_session, new_lease) = crate::store::begin_audio_spool_session(&root, &new_id)
            .expect("new capture reserves a separate live session");
        let registry = Arc::new(SpoolOwnerRegistry::with_capacity(2));
        let (prefetch_tx, _prefetch_rx) = crate::prefetch_asr::PrefetchAsrSession::channel();
        let (event_tx, _event_rx) = mpsc::channel();
        let capture = CaptureWorker::new(
            root.clone(),
            new_session.clone(),
            new_lease,
            TARGET_RATE,
            false,
            60 * TARGET_RATE as usize,
            30,
            1.0,
            prefetch_tx,
            None,
            true,
            183,
            184,
            event_tx,
            None,
            None,
            Some((Arc::new(StoreAudioSpoolIo), registry.clone())),
        )
        .expect("a real capture worker starts with store-backed spool I/O");
        let expected_samples = vec![0.375; TARGET_RATE as usize];
        capture
            .tx
            .try_send(CaptureCommand::Samples(expected_samples.clone()))
            .expect("fake PCM is admitted by the actual capture queue");
        let capture_result = capture
            .finish()
            .expect("capture closes without waiting for the older session's I/O");
        let mut tail_flush = capture_result.tail_flush;
        let deadline = Instant::now() + Duration::from_secs(3);
        let tail_ack = loop {
            if let Some(result) = tail_flush.try_wait() {
                break Some(result);
            }
            if Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        let old_writer_still_holds_scope = !old_writer.is_finished();
        let quota_still_reserved = crate::store::recovery_spool_reserved_bytes_for_test(&root) > 0;

        let _ = release_tx.send(());
        let old_result = old_writer
            .join()
            .expect("old store owner thread completes after release");
        assert!(old_result.is_ok());
        drop(old_lease);

        assert!(
            old_writer_still_holds_scope,
            "A remained paused during B's tail ACK"
        );
        assert!(
            quota_still_reserved,
            "A's quota reservation remained live through B"
        );
        assert!(
            tail_ack.is_some_and(|(_, result)| result.is_ok()),
            "B's durable tail receipt must ACK while A still owns its separate session scope"
        );
        assert_eq!(capture_result.samples, expected_samples);
        assert!(capture_result
            .spool_finalizer
            .resolve(SpoolDisposition::Recoverable));
        capture_result
            .spool_finalizer
            .wait_closed_blocking()
            .expect("B's status manifest commits before its lease closes");
        await_spool_registry(&registry);

        let chunk_path = new_session.join("chunks/00000000.f32");
        let chunk_bytes = std::fs::read(&chunk_path).expect("B's accepted PCM chunk is durable");
        let samples = chunk_bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|sample| f32::from_le_bytes(*sample))
            .collect::<Vec<_>>();
        assert_eq!(samples, expected_samples);
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(new_session.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["chunks"][0]["source_start_sample"], 0);
        assert_eq!(manifest["chunks"][0]["sample_count"], TARGET_RATE as u64);
        assert_eq!(
            crate::store::recovery_spool_reserved_bytes_for_test(&root),
            0
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn actual_capture_worker_retries_bounded_root_maintenance_without_dropping_queued_pcm() {
        use super::{
            CaptureCommand, CaptureWorker, SpoolDisposition, SpoolOwnerRegistry, StoreAudioSpoolIo,
        };
        use std::time::{Duration, Instant};

        let _serial = CAPTURE_TEST_SERIAL.blocking_lock();
        wait_for_audio_owners();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "voiceflow-live-root-maintenance-retry-{}-{nonce}",
            std::process::id()
        ));
        let session_id = format!("capture-during-root-maintenance-{nonce}");
        let (session_dir, lease) = crate::store::begin_audio_spool_session(&root, &session_id)
            .expect("capture admission reserves memory only");
        let maintenance = crate::store::try_spool_io_commit_scope(
            &root,
            crate::store::SpoolIoScope::RecoverySpool,
        )
        .expect("whole-tree maintenance holds the legacy root scope");
        let registry = Arc::new(SpoolOwnerRegistry::with_capacity(2));
        let (prefetch_tx, _prefetch_rx) = crate::prefetch_asr::PrefetchAsrSession::channel();
        let (event_tx, _event_rx) = mpsc::channel();
        let capture = CaptureWorker::new(
            root.clone(),
            session_dir.clone(),
            lease,
            TARGET_RATE,
            false,
            60 * TARGET_RATE as usize,
            30,
            1.0,
            prefetch_tx,
            None,
            true,
            185,
            186,
            event_tx,
            None,
            None,
            Some((Arc::new(StoreAudioSpoolIo), registry.clone())),
        )
        .expect("capture worker keeps initialization on its bounded spool owner");
        let expected_samples = vec![0.625; TARGET_RATE as usize];
        capture
            .tx
            .try_send(CaptureCommand::Samples(expected_samples.clone()))
            .expect("fake PCM is accepted while root-wide storage is busy");
        let capture_result = capture
            .finish()
            .expect("finish returns while the spool worker retries maintenance contention");
        let mut tail_flush = capture_result.tail_flush;
        std::thread::sleep(Duration::from_millis(75));
        let no_false_ack = tail_flush.try_wait().is_none();
        drop(maintenance);

        let deadline = Instant::now() + Duration::from_secs(3);
        let tail_ack = loop {
            if let Some(result) = tail_flush.try_wait() {
                break Some(result);
            }
            if Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        assert!(no_false_ack, "queue admission is not a durable tail ACK");
        assert!(capture_result.processing_error.is_none());
        assert!(tail_ack.is_some_and(|(_, result)| result.is_ok()));
        assert!(capture_result
            .spool_finalizer
            .resolve(SpoolDisposition::Recoverable));
        capture_result
            .spool_finalizer
            .wait_closed_blocking()
            .expect("the retried owner commits its recoverable status");
        await_spool_registry(&registry);

        let chunk_path = session_dir.join("chunks/00000000.f32");
        let chunk_bytes = std::fs::read(&chunk_path).expect("queued accepted PCM is persisted");
        let actual_samples = chunk_bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|sample| f32::from_le_bytes(*sample))
            .collect::<Vec<_>>();
        assert_eq!(actual_samples, expected_samples);
        assert_eq!(capture_result.samples, expected_samples);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn actual_capture_worker_tail_barrier_waits_for_queued_spool_io() {
        use super::{
            AudioSpoolIo, CaptureCommand, CaptureWorker, SpoolDisposition, SpoolOwnerRegistry,
            TARGET_RATE,
        };
        use std::time::{Duration, Instant};

        let _serial = CAPTURE_TEST_SERIAL.blocking_lock();
        wait_for_audio_owners();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "voiceflow-capture-tail-barrier-{}-{nonce}",
            std::process::id()
        ));
        let session_id = format!("tail-barrier-{nonce}");
        let (session_dir, lease) = crate::store::begin_audio_spool_session(&root, &session_id)
            .expect("actual capture reserves a real recovery session");
        let registry = Arc::new(SpoolOwnerRegistry::default());
        let (entered_tx, entered_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let io: Arc<dyn AudioSpoolIo> = Arc::new(BlockingSpoolIo {
            initialize_entered: Mutex::new(None),
            initialize_release: None,
            store_initialize: true,
            initialize_complete: Mutex::new(None),
            entered: Mutex::new(Some(entered_tx)),
            release: release.clone(),
            finish_entered: Mutex::new(None),
            finish_release: None,
            finished: finished_tx,
        });
        let (prefetch_tx, _prefetch_rx) = crate::prefetch_asr::PrefetchAsrSession::channel();
        let (event_tx, _event_rx) = mpsc::channel();
        let capture = CaptureWorker::new(
            root.clone(),
            session_dir.clone(),
            lease,
            TARGET_RATE,
            false,
            60 * TARGET_RATE as usize,
            30,
            1.0,
            prefetch_tx,
            None,
            true,
            181,
            182,
            event_tx,
            None,
            None,
            Some((io, registry.clone())),
        )
        .expect("the actual CaptureWorker owns the injected serial spool worker");
        capture
            .tx
            .try_send(CaptureCommand::Samples(vec![0.25; TARGET_RATE as usize]))
            .expect("fake PCM enters the actual bounded capture queue");
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("the real spool owner pauses while processing the accepted PCM chunk");

        let finish_started = Instant::now();
        let capture_result = capture
            .finish()
            .expect("the capture worker drains PCM without awaiting filesystem I/O");
        assert!(finish_started.elapsed() < Duration::from_secs(1));
        assert_eq!(capture_result.samples.len(), TARGET_RATE as usize);
        assert!(capture_result
            .samples
            .iter()
            .all(|sample| (*sample - 0.25).abs() < 0.0001));
        let mut tail_flush = capture_result.tail_flush;
        assert!(
            tail_flush.try_wait().is_none(),
            "enqueue is not reported as a durable acknowledgement while the FIFO write is paused"
        );

        std::thread::sleep(Duration::from_millis(25));
        release_spool_io(&release);
        let deadline = Instant::now() + Duration::from_secs(2);
        let (elapsed, result) = loop {
            if let Some(result) = tail_flush.try_wait() {
                break result;
            }
            assert!(
                Instant::now() < deadline,
                "tail barrier did not receive an ACK"
            );
            std::thread::sleep(Duration::from_millis(1));
        };
        assert!(
            result.is_ok(),
            "writer ACK follows completion of prior chunk writes"
        );
        assert!(elapsed >= Duration::from_millis(1));
        assert!(capture_result
            .spool_finalizer
            .resolve(SpoolDisposition::Recoverable));
        capture_result
            .spool_finalizer
            .wait_closed_blocking()
            .expect("the capture spool owner closes after its barrier ACK");
        assert_eq!(
            finished_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            (SpoolDisposition::Recoverable, false)
        );
        await_spool_registry(&registry);
        wait_for_audio_owners();
        crate::store::clear_all_data(&root).expect("clear the isolated capture test root");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn capture_timeout_keeps_recoverable_intent_until_accepted_pcm_is_drained() {
        use super::{CaptureCommand, CaptureWorker, TARGET_RATE};
        use std::time::{Duration, Instant};

        let _serial = CAPTURE_TEST_SERIAL.blocking_lock();

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "voiceflow-capture-timeout-recovery-{}-{nonce}",
            std::process::id()
        ));
        let session_id = format!("late-capture-{nonce}");
        let (session_dir, lease) = crate::store::begin_audio_spool_session(&root, &session_id)
            .expect("capture reserves a fresh recovery path");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        *CAPTURE_START_GATE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(CaptureStartGate {
            session_dir: session_dir.clone(),
            entered: entered_tx,
            release: release_rx,
        });
        let (prefetch_tx, _prefetch_rx) = crate::prefetch_asr::PrefetchAsrSession::channel();
        let (event_tx, _event_rx) = mpsc::channel();
        let capture = CaptureWorker::new(
            root.clone(),
            session_dir,
            lease,
            TARGET_RATE,
            false,
            super::MAX_RECORDING_SECS * TARGET_RATE as usize,
            30,
            1.0,
            prefetch_tx,
            None,
            true,
            917,
            918,
            event_tx,
            None,
            None,
            None,
        )
        .expect("production capture worker starts");
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("capture processing waits behind the injected gate");
        for index in 0..3 {
            capture
                .tx
                .try_send(CaptureCommand::Samples(vec![
                    0.1 + index as f32 * 0.1;
                    16_000
                ]))
                .unwrap_or_else(|_| panic!("accepted capture command {index}"));
        }

        let started = Instant::now();
        let result = capture.finish();
        assert!(
            result.is_err(),
            "the deliberate gate exceeds the close budget"
        );
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(
            crate::store::recover_spool_session(&root, &session_id, 7).is_err(),
            "the same session remains owned until the accepted PCM is drained"
        );

        release_tx.send(()).expect("release the capture owner");
        wait_for_audio_owners();
        let recovered = crate::store::recover_spool_session(&root, &session_id, 7)
            .expect("completed late recovery path is readable")
            .expect("recoverable manifest includes its full WAV");
        let wav = crate::store::read_spool_file(&recovered.audio_path).expect("read recovered WAV");
        let mut reader = hound::WavReader::new(std::io::Cursor::new(wav)).expect("valid PCM WAV");
        assert_eq!(reader.spec().sample_rate, TARGET_RATE);
        assert_eq!(reader.spec().channels, 1);
        let actual = reader
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .expect("decode every recovered sample");
        assert_eq!(actual.len(), 48_000);
        for (index, samples) in actual.as_chunks::<16_000>().0.iter().enumerate() {
            let expected = ((0.1 + index as f32 * 0.1) * i16::MAX as f32) as i16;
            assert!(samples.iter().all(|sample| *sample == expected));
        }
        crate::store::remove_spool_artifact(&root, &recovered.audio_path);
        crate::store::clear_all_data(&root).expect("clear the isolated test root");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn paused_history_commit_allows_next_pending_journal_and_real_capture_pcm() {
        use super::{
            enqueue_retained_spool_store_job_with_mutation, CaptureCommand, CaptureWorker,
            RetainedSpoolJobFailure, TARGET_RATE,
        };
        use crate::store::{PendingHistoryIntent, SpoolIoScope};
        use std::time::{Duration, Instant};

        let _serial = CAPTURE_TEST_SERIAL.lock().await;
        wait_for_audio_owners();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "voiceflow-postdelivery-capture-admission-{}-{nonce}",
            std::process::id()
        ));
        // Keep A's actual source-spool owner alive after its capture worker
        // has drained. The retained History transaction below must not be the
        // only old owner in this acceptance case: B must pass real capture
        // admission while both A's SQLite commit and its recovery spool are
        // pending.
        let old_session_id = format!("old-capture-{nonce}");
        let (old_session_dir, old_spool_lease) =
            crate::store::begin_audio_spool_session(&root, &old_session_id)
                .expect("old capture reserves its source recovery session");
        let (old_prefetch_tx, _old_prefetch_rx) =
            crate::prefetch_asr::PrefetchAsrSession::channel();
        let (old_event_tx, _old_event_rx) = mpsc::channel();
        let old_capture = CaptureWorker::new(
            root.clone(),
            old_session_dir,
            old_spool_lease,
            TARGET_RATE,
            false,
            super::MAX_RECORDING_SECS * TARGET_RATE as usize,
            30,
            1.0,
            old_prefetch_tx,
            None,
            true,
            991,
            992,
            old_event_tx,
            None,
            None,
            None,
        )
        .expect("old actual CaptureWorker starts");
        for index in 0..3 {
            old_capture
                .tx
                .try_send(CaptureCommand::Samples(vec![
                    0.15 + index as f32 * 0.1;
                    16_000
                ]))
                .unwrap_or_else(|_| panic!("old capture accepted PCM block {index}"));
        }
        let old_capture_result = old_capture
            .finish()
            .expect("old capture worker drains accepted PCM");
        assert!(old_capture_result.processing_error.is_none());
        assert_eq!(old_capture_result.samples.len(), 48_000);
        let (_, old_tail_result) = old_capture_result.tail_flush.wait();
        old_tail_result
            .expect("the old capture spool acknowledges all accepted chunks before handoff");
        let old_spool_finalizer = old_capture_result.spool_finalizer;
        assert!(
            crate::store::recover_spool_session(&root, &old_session_id, 7).is_err(),
            "A's source recovery remains owned until the History ACK is settled"
        );

        let old_intent_id = "dictation-paused-history-owner";
        let mut old_intent: PendingHistoryIntent = crate::store::pending_history_intent_for_test(
            old_intent_id,
            "ok",
            "paste",
            "earlier acknowledged delivery",
        );
        old_intent.spool_path = None;
        old_intent.capture_spool_session_id = Some(old_session_id.clone());
        let old_intent_owner = crate::store::register_pending_history_owner(&root, old_intent_id)
            .expect("reserve A's durable History identity");
        crate::store::write_pending_history_intent(&root, &old_intent, &old_intent_owner)
            .expect("write the old transaction's durable intent");

        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        crate::store::pause_next_history_promotion_for_test(&root, entered_tx, release_rx);
        let old_root = root.clone();
        let mutation = crate::store::begin_spool_root_mutation(&root)
            .expect("old retained job owns the clear/deletion fence");
        let old_owner_for_promotion = old_intent_owner.clone();
        let mut old_receipt = enqueue_retained_spool_store_job_with_mutation(
            "postdelivery-test-old-history".into(),
            root.clone(),
            0,
            mutation,
            move |lease| {
                if lease.data_root() != old_root.as_path() {
                    return Err("old retained History job has the wrong root".into());
                }
                crate::store::promote_pending_history_intent(
                    &old_root,
                    old_intent_id,
                    &old_owner_for_promotion,
                )
                .map(|()| false)
                .map_err(|error| error.to_string())
            },
        )
        .expect("old History promotion is retained before it starts");
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("the job paused inside its real SQLite promotion transaction");

        let wait_started = Instant::now();
        assert_eq!(
            old_receipt.wait().await,
            Err(RetainedSpoolJobFailure::Pending)
        );
        assert!(
            wait_started.elapsed() >= Duration::from_secs(5),
            "the previous History transaction remains paused beyond the caller's storage wait"
        );
        assert!(
            crate::store::spool_io_scope_is_busy_for_test(&root, SpoolIoScope::HistoryDatabase,),
            "the injected owner holds the actual History commit scope"
        );
        crate::store::list_learn_pairs(&root)
            .expect("B's dictionary/preparation read remains available during A's write txn");

        let session_id = format!("next-capture-{nonce}");
        let next_intent_id = format!("dictation-next-{nonce}");
        let mut next_intent = crate::store::pending_history_intent_for_test(
            &next_intent_id,
            "delivery_pending",
            "unknown",
            "next actual capture transcript",
        );
        next_intent.capture_spool_session_id = Some(session_id.clone());
        let next_intent_owner =
            crate::store::register_pending_history_owner(&root, &next_intent_id)
                .expect("reserve B's durable History identity");
        crate::store::write_pending_history_intent(&root, &next_intent, &next_intent_owner)
            .expect("B's pre-delivery journal uses the disjoint pending-intent scope");

        super::ensure_audio_workers_available()
            .expect("a retained History worker does not consume active capture admission");
        let (session_dir, spool_lease) =
            crate::store::begin_audio_spool_session(&root, &session_id)
                .expect("actual next capture reserves a separate recovery session");
        let (prefetch_tx, _prefetch_rx) = crate::prefetch_asr::PrefetchAsrSession::channel();
        let (event_tx, _event_rx) = mpsc::channel();
        let capture = CaptureWorker::new(
            root.clone(),
            session_dir,
            spool_lease,
            TARGET_RATE,
            false,
            super::MAX_RECORDING_SECS * TARGET_RATE as usize,
            30,
            1.0,
            prefetch_tx,
            None,
            true,
            993,
            994,
            event_tx,
            None,
            None,
            None,
        )
        .expect("the actual CaptureWorker starts while A's History transaction is paused");
        for index in 0..3 {
            capture
                .tx
                .try_send(CaptureCommand::Samples(vec![
                    0.1 + index as f32 * 0.1;
                    16_000
                ]))
                .unwrap_or_else(|_| panic!("next capture accepted PCM block {index}"));
        }
        let capture_result = capture
            .finish()
            .expect("next capture drains accepted PCM without waiting for History");
        assert!(capture_result.processing_error.is_none());
        assert_eq!(capture_result.samples.len(), 48_000);
        let (_, next_tail_result) = capture_result.tail_flush.wait();
        next_tail_result.expect("the next actual capture acknowledges its durable FIFO tail");
        for (index, samples) in capture_result
            .samples
            .as_chunks::<16_000>()
            .0
            .iter()
            .enumerate()
        {
            let expected = 0.1 + index as f32 * 0.1;
            assert!(samples
                .iter()
                .all(|sample| (*sample - expected).abs() < 0.0001));
        }
        assert!(capture_result
            .spool_finalizer
            .resolve(super::SpoolDisposition::Recoverable));
        capture_result
            .spool_finalizer
            .wait_closed(Duration::from_secs(3))
            .await
            .expect("capture spool owner durably closes while History remains locked");
        let recovered = crate::store::recover_spool_session(&root, &session_id, 7)
            .expect("next capture spool is readable")
            .expect("the accepted capture has a full recovery WAV");
        let wav = crate::store::read_spool_file(&recovered.audio_path)
            .expect("read the actual capture's recovery audio");
        let mut reader = hound::WavReader::new(std::io::Cursor::new(wav))
            .expect("recovery audio is a valid PCM WAV");
        assert_eq!(reader.spec().sample_rate, TARGET_RATE);
        let samples = reader
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .expect("decode the entire recovered capture");
        assert_eq!(samples.len(), 48_000);
        for (index, block) in samples.as_chunks::<16_000>().0.iter().enumerate() {
            let expected = ((0.1 + index as f32 * 0.1) * i16::MAX as f32) as i16;
            assert!(block.iter().all(|sample| *sample == expected));
        }

        next_intent.status = "ok".into();
        next_intent.delivery_method = "paste".into();
        next_intent.spool_path = Some(recovered.audio_path.clone());
        crate::store::write_pending_history_intent(&root, &next_intent, &next_intent_owner)
            .expect("the exact pending transcript and delivered outcome remain durable");
        assert!(
            crate::store::promote_pending_history_intent(
                &root,
                &next_intent_id,
                &next_intent_owner,
            )
            .is_err(),
            "B retains its delivery result while A owns the History database commit"
        );
        let next_intent_path = root
            .join("history-pending")
            .join(&next_intent_id)
            .join("intent.json");
        assert!(next_intent_path.is_file());
        let pending_bytes =
            std::fs::read(&next_intent_path).expect("read the retained B recovery intent");
        let pending: PendingHistoryIntent =
            serde_json::from_slice(&pending_bytes).expect("B's exact intent remains valid");
        assert_eq!(pending.status, "ok");
        assert_eq!(pending.delivery_method, "paste");
        assert_eq!(pending.final_text, "next actual capture transcript");

        release_tx
            .send(())
            .expect("release old History transaction");
        assert_eq!(
            old_receipt.wait_until_complete().await,
            Ok(false),
            "the old History owner receives its real SQLite acknowledgement"
        );
        let old_intent_path = root
            .join("history-pending")
            .join(old_intent_id)
            .join("intent.json");
        assert!(
            old_intent_path.is_file(),
            "the acknowledged History row does not discard its intent while the capture owner remains live"
        );
        assert!(old_spool_finalizer.resolve(super::SpoolDisposition::Completed));
        old_spool_finalizer
            .wait_closed(Duration::from_secs(3))
            .await
            .expect("A's source spool closes after its History ACK");
        crate::store::remove_pending_history_intent(&root, old_intent_id)
            .expect("remove A's intent only after its source spool is settled");
        crate::store::promote_pending_history_intent(&root, &next_intent_id, &next_intent_owner)
            .expect(
                "B's already-recorded delivery result promotes after the old transaction commits",
            );
        let rows = crate::store::get_history(&root, 10).expect("read both committed History rows");
        let row = rows
            .iter()
            .find(|row| row.raw_text == "next actual capture transcript")
            .expect("the next capture is represented in History");
        assert_eq!(row.status, "ok");
        assert_eq!(row.delivery_method.as_deref(), Some("paste"));
        assert!(
            row.has_audio,
            "the committed row references retained capture audio"
        );
        let saved = crate::store::history_audio_bytes(&root, row.id)
            .expect("History audio remains readable after its ACK");
        let mut saved_reader = hound::WavReader::new(std::io::Cursor::new(saved))
            .expect("History retains a valid recovery WAV");
        assert_eq!(saved_reader.samples::<i16>().count(), 48_000);

        wait_for_audio_owners();
        crate::store::clear_all_data(&root).expect("clear only the isolated test root");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn blocked_spool_initialization_keeps_clear_cancel_and_same_path_reservation_bounded() {
        use super::{
            AudioSpoolIo, PendingSpool, SpoolDisposition, SpoolOwnerRegistry, SpoolWriter,
        };
        use std::time::{Duration, Instant};

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "voiceflow-audio-init-{}-{nonce}",
            std::process::id()
        ));
        let registry = Arc::new(SpoolOwnerRegistry::default());
        let (session_dir, lease) = crate::store::begin_audio_spool_session(&root, "blocked-init")
            .expect("reserve a new session without filesystem work");
        assert!(!root.exists());

        let (initialize_entered_tx, initialize_entered_rx) = mpsc::channel();
        let (write_entered_tx, write_entered_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let initialize_release = Arc::new((Mutex::new(false), Condvar::new()));
        let io: Arc<dyn AudioSpoolIo> = Arc::new(BlockingSpoolIo {
            initialize_entered: Mutex::new(Some(initialize_entered_tx)),
            initialize_release: Some(initialize_release.clone()),
            store_initialize: false,
            initialize_complete: Mutex::new(None),
            entered: Mutex::new(Some(write_entered_tx)),
            release: Arc::new((Mutex::new(true), Condvar::new())),
            finish_entered: Mutex::new(None),
            finish_release: None,
            finished: finished_tx,
        });
        let writer = SpoolWriter::new_with_lease_and_io_and_registry(
            root.clone(),
            session_dir.clone(),
            lease,
            io,
            registry.clone(),
        )
        .expect("bounded owner starts");
        writer.sender.enqueue(0, vec![0.5; 32]);
        let (finalizer, _tail_flush) =
            writer.finish_with_pending(Arc::new(Mutex::new(PendingSpool {
                samples: Vec::new(),
                next_index: 1,
            })));
        initialize_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("injected initialization blocks on the owner thread");

        assert!(!session_dir.exists());
        let started = Instant::now();
        assert!(crate::store::clear_all_data(&root)
            .unwrap_err()
            .to_string()
            .contains("cleanup is still pending"));
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(!root.join("spool").exists());
        assert!(crate::store::recover_spool(&root, 7).is_err());
        assert!(crate::store::recover_spool_session(&root, "blocked-init", 0).is_err());
        assert!(crate::store::begin_audio_spool_session(&root, "blocked-init").is_err());
        std::fs::create_dir_all(root.join("spool")).unwrap();
        assert!(
            crate::store::recover_spool(&root, 7).is_err(),
            "list recovery also sees a reservation whose directory is not created yet"
        );
        let started = Instant::now();
        assert!(finalizer.cancel());
        assert!(started.elapsed() < Duration::from_millis(100));

        release_spool_io(&initialize_release);
        assert_eq!(
            finished_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            (SpoolDisposition::Cancelled, true)
        );
        await_spool_registry(&registry);
        assert!(!session_dir.exists());
        std::fs::remove_dir_all(root.join("spool")).unwrap();
        assert!(matches!(
            write_entered_rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        let (next_path, next_lease) =
            crate::store::begin_audio_spool_session(&root, "blocked-init")
                .expect("same path becomes available after owner closure");
        drop(next_lease);
        assert!(!next_path.exists());
        assert!(!root.join("spool").exists());
        crate::store::clear_all_data(&root).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn failed_spool_init_with_cancel_preserves_preexisting_directory_contents() {
        use super::{AudioSpoolIo, SpoolOwnerRegistry, SpoolWriter};
        use std::time::{Duration, Instant};

        let (root, session_dir) = spool_test_session("preexisting-init");
        let marker = session_dir.join("marker.bin");
        std::fs::write(&marker, b"preexisting recovery data").unwrap();
        let marker_hash = sha2::Sha256::digest(std::fs::read(&marker).unwrap());
        let manifest = session_dir.join("manifest.json");
        let manifest_hash = sha2::Sha256::digest(std::fs::read(&manifest).unwrap());
        let (reserved, lease) =
            crate::store::begin_audio_spool_session(&root, "preexisting-init").unwrap();
        assert_eq!(reserved, session_dir);

        let registry = Arc::new(SpoolOwnerRegistry::default());
        let (initialize_entered_tx, initialize_entered_rx) = mpsc::channel();
        let (initialize_complete_tx, initialize_complete_rx) = mpsc::channel();
        let (finished_tx, _finished_rx) = mpsc::channel();
        let initialize_release = Arc::new((Mutex::new(false), Condvar::new()));
        let io: Arc<dyn AudioSpoolIo> = Arc::new(BlockingSpoolIo {
            initialize_entered: Mutex::new(Some(initialize_entered_tx)),
            initialize_release: Some(initialize_release.clone()),
            store_initialize: true,
            initialize_complete: Mutex::new(Some(initialize_complete_tx)),
            entered: Mutex::new(None),
            release: Arc::new((Mutex::new(true), Condvar::new())),
            finish_entered: Mutex::new(None),
            finish_release: None,
            finished: finished_tx,
        });
        let writer = SpoolWriter::new_with_lease_and_io_and_registry(
            root.clone(),
            session_dir.clone(),
            lease,
            io,
            registry.clone(),
        )
        .expect("owner starts for the reserved path");
        initialize_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("initialization was attempted");
        assert!(
            !initialize_complete_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            "exclusive create rejects the pre-existing directory"
        );

        assert!(crate::store::clear_all_data(&root)
            .unwrap_err()
            .to_string()
            .contains("cleanup is still pending"));
        let started = Instant::now();
        drop(writer);
        assert!(started.elapsed() < Duration::from_millis(100));
        release_spool_io(&initialize_release);
        await_spool_registry(&registry);

        assert_eq!(
            marker_hash,
            sha2::Sha256::digest(std::fs::read(&marker).unwrap())
        );
        assert_eq!(
            manifest_hash,
            sha2::Sha256::digest(std::fs::read(&manifest).unwrap())
        );
        crate::store::clear_all_data(&root).unwrap();
        assert!(!session_dir.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn blocked_spool_owner_keeps_raw_audio_and_defers_cancel_and_reuse() {
        use super::{
            append_capture_samples, AudioSpoolIo, PendingSpool, SpoolDisposition,
            SpoolOwnerRegistry, SpoolWriter, SPOOL_CHUNK_SAMPLES,
        };
        use std::time::{Duration, Instant};

        let registry = Arc::new(SpoolOwnerRegistry::default());
        let (root, session_dir) = spool_test_session("blocked-recoverable");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let io: Arc<dyn AudioSpoolIo> = Arc::new(BlockingSpoolIo {
            initialize_entered: Mutex::new(None),
            initialize_release: None,
            store_initialize: false,
            initialize_complete: Mutex::new(None),
            entered: Mutex::new(Some(entered_tx)),
            release: release.clone(),
            finish_entered: Mutex::new(None),
            finish_release: None,
            finished: finished_tx,
        });
        let writer = SpoolWriter::new_with_io_and_registry(
            root.clone(),
            session_dir.clone(),
            io,
            registry.clone(),
        )
        .expect("writer starts");
        let pending = Arc::new(Mutex::new(PendingSpool {
            samples: Vec::with_capacity(SPOOL_CHUNK_SAMPLES),
            next_index: 0,
        }));
        let mut raw_samples = Vec::new();
        for chunk_index in 0..18 {
            let source = vec![0.25_f32; SPOOL_CHUNK_SAMPLES];
            let (start, accepted, hit_limit) = append_capture_samples(
                &mut raw_samples,
                &source,
                20 * SPOOL_CHUNK_SAMPLES,
                &pending,
                Some(&writer.sender),
            );
            assert_eq!(start, (chunk_index * SPOOL_CHUNK_SAMPLES) as u64);
            assert_eq!(accepted, SPOOL_CHUNK_SAMPLES);
            assert!(!hit_limit);
            if chunk_index == 0 {
                entered_rx
                    .recv_timeout(Duration::from_secs(1))
                    .expect("injected filesystem write blocks");
            }
        }
        assert_eq!(raw_samples.len(), 18 * SPOOL_CHUNK_SAMPLES);
        assert!(raw_samples.iter().all(|sample| *sample == 0.25));
        assert!(writer
            .sender
            .failed
            .load(std::sync::atomic::Ordering::Acquire));
        let (finalizer, _tail_flush) = writer.finish_with_pending(pending);
        assert!(finalizer.failure().is_some());
        let started = Instant::now();
        assert!(finalizer.resolve(SpoolDisposition::Recoverable));
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(!finalizer.resolve(SpoolDisposition::Recoverable));
        assert!(registry.ensure_available().is_err());
        assert!(crate::store::recover_spool_session(&root, "blocked-recoverable", 7).is_err());
        assert!(crate::store::begin_audio_spool_session(&root, "blocked-recoverable").is_err());
        crate::store::remove_spool_artifact(&root, &session_dir);
        assert!(session_dir.exists(), "removal waits for the active owner");

        release_spool_io(&release);
        await_spool_registry(&registry);
        assert!(
            matches!(finished_rx.try_recv(), Err(mpsc::TryRecvError::Disconnected)),
            "deferred deletion closes the old owner without publishing a recoverable terminal write"
        );
        assert!(!session_dir.exists());
        let (reused, lease) = crate::store::begin_audio_spool_session(&root, "blocked-recoverable")
            .expect("same ID is reusable only after old owner closure");
        crate::store::initialize_audio_spool_session(&lease).unwrap();
        drop(lease);
        assert!(reused.join("manifest.json").exists());
        let following_chunk = vec![0.125_f32; TARGET_RATE as usize];
        let following_relative = std::path::Path::new("blocked-recoverable/chunks/00000000.f32");
        crate::store::write_spool_file(
            &root,
            following_relative,
            &following_chunk
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        crate::store::record_spool_chunk_with_samples(
            &reused,
            0,
            0.0,
            1.0,
            "written",
            0,
            following_chunk.len() as u64,
        )
        .unwrap();
        let recovered = crate::store::recover_spool_session(&root, "blocked-recoverable", 7)
            .unwrap()
            .expect("the following session remains independently recoverable");
        assert_eq!(recovered.duration_secs, 1.0);
        crate::store::remove_spool_artifact(&root, &reused);
        let _ = std::fs::remove_dir_all(&root);

        let (cancel_root, cancel_dir) = spool_test_session("blocked-cancel");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let io: Arc<dyn AudioSpoolIo> = Arc::new(BlockingSpoolIo {
            initialize_entered: Mutex::new(None),
            initialize_release: None,
            store_initialize: false,
            initialize_complete: Mutex::new(None),
            entered: Mutex::new(Some(entered_tx)),
            release: release.clone(),
            finish_entered: Mutex::new(None),
            finish_release: None,
            finished: finished_tx,
        });
        let writer = SpoolWriter::new_with_io_and_registry(
            cancel_root.clone(),
            cancel_dir.clone(),
            io,
            registry.clone(),
        )
        .expect("cancel writer starts after first owner closes");
        writer.sender.enqueue(0, vec![0.5; 32]);
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("second injected filesystem write blocks");
        let (finalizer, _tail_flush) =
            writer.finish_with_pending(Arc::new(Mutex::new(PendingSpool {
                samples: Vec::new(),
                next_index: 1,
            })));
        let started = Instant::now();
        assert!(finalizer.cancel());
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(registry.ensure_available().is_err());
        assert!(crate::store::begin_audio_spool_session(&cancel_root, "blocked-cancel").is_err());
        crate::store::remove_spool_artifact(&cancel_root, &cancel_dir);
        release_spool_io(&release);
        await_spool_registry(&registry);
        assert!(
            matches!(
                finished_rx.try_recv(),
                Err(mpsc::TryRecvError::Disconnected)
            ),
            "cancel plus deferred deletion closes the old owner without a late finalizer write"
        );
        assert!(!cancel_dir.exists());
        let (reused, lease) =
            crate::store::begin_audio_spool_session(&cancel_root, "blocked-cancel")
                .expect("late cancellation completed before session reuse");
        crate::store::initialize_audio_spool_session(&lease).unwrap();
        drop(lease);
        assert!(reused.join("manifest.json").exists());
        crate::store::remove_spool_artifact(&cancel_root, &reused);
        let _ = std::fs::remove_dir_all(&cancel_root);
    }

    #[test]
    fn blocked_spool_finalization_keeps_session_owned_until_terminal_io_returns() {
        use super::{
            AudioSpoolIo, PendingSpool, SpoolDisposition, SpoolOwnerRegistry, SpoolWriter,
        };
        use std::time::{Duration, Instant};

        let registry = Arc::new(SpoolOwnerRegistry::default());
        let (root, session_dir) = spool_test_session("blocked-terminal");
        let (finish_entered_tx, finish_entered_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let finish_release = Arc::new((Mutex::new(false), Condvar::new()));
        let io: Arc<dyn AudioSpoolIo> = Arc::new(BlockingSpoolIo {
            initialize_entered: Mutex::new(None),
            initialize_release: None,
            store_initialize: false,
            initialize_complete: Mutex::new(None),
            entered: Mutex::new(None),
            release: Arc::new((Mutex::new(true), Condvar::new())),
            finish_entered: Mutex::new(Some(finish_entered_tx)),
            finish_release: Some(finish_release.clone()),
            finished: finished_tx,
        });
        let writer = SpoolWriter::new_with_io_and_registry(
            root.clone(),
            session_dir.clone(),
            io,
            registry.clone(),
        )
        .expect("writer starts");
        let (finalizer, _tail_flush) =
            writer.finish_with_pending(Arc::new(Mutex::new(PendingSpool {
                samples: Vec::new(),
                next_index: 0,
            })));
        let started = Instant::now();
        assert!(finalizer.resolve(SpoolDisposition::Completed));
        assert!(started.elapsed() < Duration::from_millis(100));
        finish_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("injected terminal filesystem operation blocks");

        assert!(registry.ensure_available().is_err());
        assert!(crate::store::begin_audio_spool_session(&root, "blocked-terminal").is_err());
        assert!(crate::store::recover_spool_session(&root, "blocked-terminal", 7).is_err());
        crate::store::remove_spool_artifact(&root, &session_dir);
        assert!(
            session_dir.exists(),
            "terminal I/O still owns the directory"
        );

        release_spool_io(&finish_release);
        assert_eq!(
            finished_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            (SpoolDisposition::Completed, false)
        );
        await_spool_registry(&registry);
        assert!(!session_dir.exists());
        let (reused, lease) = crate::store::begin_audio_spool_session(&root, "blocked-terminal")
            .expect("session path is reusable after terminal I/O and lease release");
        crate::store::initialize_audio_spool_session(&lease).unwrap();
        drop(lease);
        crate::store::remove_spool_artifact(&root, &reused);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn timed_out_long_store_job_retains_owner_and_same_path_until_real_completion() {
        use super::{AudioSpoolIo, LongSpoolSession, SpoolOwnerRegistry};
        use std::time::{Duration, Instant};

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "voiceflow-long-owner-job-{}-{nonce}",
            std::process::id()
        ));
        let session_id = "timed-out-long-job";
        let registry = Arc::new(SpoolOwnerRegistry::default());
        let (initialize_complete_tx, initialize_complete_rx) = mpsc::channel();
        let (finished_tx, _finished_rx) = mpsc::channel();
        let io: Arc<dyn AudioSpoolIo> = Arc::new(BlockingSpoolIo {
            initialize_entered: Mutex::new(None),
            initialize_release: None,
            store_initialize: false,
            initialize_complete: Mutex::new(Some(initialize_complete_tx)),
            entered: Mutex::new(None),
            release: Arc::new((Mutex::new(true), Condvar::new())),
            finish_entered: Mutex::new(None),
            finish_release: None,
            finished: finished_tx,
        });
        let owner =
            LongSpoolSession::start_with_io_and_registry(&root, session_id, io, registry.clone())
                .expect("long spool owner starts");
        assert!(initialize_complete_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("injected owner initialization returns"));

        let (job_entered_tx, job_entered_rx) = mpsc::channel();
        let job_release = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_release = job_release.clone();
        let started = Instant::now();
        let timed_out = tokio::time::timeout(
            Duration::from_millis(50),
            owner.run_store_job(128, move |_| {
                job_entered_tx
                    .send(())
                    .map_err(|_| "test job observer closed".to_owned())?;
                let (released, condition) = &*worker_release;
                let mut released = released
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                while !*released {
                    released = condition
                        .wait(released)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                Ok::<(), String>(())
            }),
        )
        .await;
        assert!(timed_out.is_err(), "caller wait should be bounded");
        assert!(started.elapsed() < Duration::from_millis(300));
        job_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("actual retained store closure is blocked on the owner");

        assert!(owner
            .run_store_job(0, |_| Ok::<(), String>(()))
            .await
            .unwrap_err()
            .contains("still pending"));
        let session_dir = owner.session_dir().to_path_buf();
        assert!(crate::store::clear_all_data(&root)
            .unwrap_err()
            .to_string()
            .contains("cleanup is still pending"));
        assert!(
            !owner.lease.deletion_requested(),
            "a Busy clear cannot revoke a retryable long-session owner"
        );
        assert!(crate::store::begin_audio_spool_session(&root, session_id).is_err());
        assert!(owner.finalizer.cancel());
        assert!(registry.ensure_available().is_err());

        release_spool_io(&job_release);
        owner
            .finalizer
            .wait_closed(Duration::from_secs(1))
            .await
            .expect("owner reports terminal completion after injected operation returns");
        drop(owner);
        await_spool_registry(&registry);
        assert!(!session_dir.exists());

        let (next_session, next_lease) = crate::store::begin_audio_spool_session(&root, session_id)
            .expect("same path becomes reusable only after actual owner completion");
        crate::store::initialize_audio_spool_session(&next_lease).unwrap();
        drop(next_lease);
        assert!(next_session.join("manifest.json").exists());
        crate::store::remove_spool_artifact(&root, &next_session);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn long_recoverable_finish_seals_stale_sender_after_draining_accepted_audio() {
        use super::{LongSpoolSession, SpoolDisposition, SpoolOwnerRegistry, StoreAudioSpoolIo};

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "voiceflow-long-recoverable-seal-{}-{nonce}",
            std::process::id()
        ));
        let session_id = format!("stale-long-sender-{nonce}");
        let registry = Arc::new(SpoolOwnerRegistry::default());
        let owner = LongSpoolSession::start_with_io_and_registry(
            &root,
            &session_id,
            Arc::new(StoreAudioSpoolIo),
            registry.clone(),
        )
        .expect("long spool session starts");
        let stale_sender = owner.enqueue_handle();
        assert!(stale_sender.enqueue(0, 0.0, 0.25, 0, vec![0.25; 4_000]));
        let session_dir = owner.session_dir().to_path_buf();

        let finish_result = owner.finish(SpoolDisposition::Recoverable).await;
        if let Err(error) = finish_result {
            assert_eq!(error, "audio spool finalization is still pending");
        }
        await_spool_registry(&registry);
        registry
            .ensure_available()
            .expect("long spool owner has fully exited");
        assert!(
            !stale_sender.enqueue(1, 0.25, 0.5, 4_000, vec![0.5; 4_000]),
            "a sender retained by a late task cannot append after finalization"
        );
        let recovered = crate::store::recover_spool_session(&root, &session_id, 7)
            .expect("finished recoverable session is readable")
            .expect("recoverable manifest includes its WAV");
        assert!((recovered.duration_secs - 0.25).abs() < 0.0001);
        let wav = crate::store::read_spool_file(&recovered.audio_path)
            .expect("the completed owner leaves its recovery WAV readable");
        let mut reader = hound::WavReader::new(std::io::Cursor::new(wav)).expect("valid WAV");
        let samples = reader
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .expect("decode the complete accepted PCM payload");
        let expected = (0.25 * i16::MAX as f32) as i16;
        assert_eq!(samples.len(), 4_000);
        assert!(samples.iter().all(|sample| *sample == expected));
        assert!(session_dir.exists());

        crate::store::remove_spool_artifact(&root, &session_dir);
        assert!(!session_dir.exists());
        let (reused_dir, reused_lease) =
            crate::store::begin_audio_spool_session(&root, &session_id)
                .expect("the completed owner released the exact session lease");
        crate::store::initialize_audio_spool_session(&reused_lease).unwrap();
        drop(reused_lease);
        assert!(reused_dir.join("manifest.json").exists());
        crate::store::remove_spool_artifact(&root, &reused_dir);
        assert!(!reused_dir.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn generic_store_job_keeps_root_lease_after_caller_abort_and_clear_sees_it() {
        use super::{run_retained_spool_store_job_with_registry, SpoolOwnerRegistry};
        use std::time::Duration;

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "voiceflow-generic-root-lease-{}-{nonce}",
            std::process::id()
        ));
        let registry = Arc::new(SpoolOwnerRegistry::default());
        let (entered_tx, entered_rx) = mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_release = release.clone();
        let worker_root = root.clone();
        let expected_root = root.clone();
        let worker_registry = registry.clone();
        let caller = tokio::spawn(async move {
            run_retained_spool_store_job_with_registry(
                worker_registry,
                "generic-history-save".into(),
                worker_root,
                64,
                move |lease| {
                    if lease.data_root() != expected_root.as_path() {
                        return Err("wrong root mutation lease".into());
                    }
                    entered_tx
                        .send(())
                        .map_err(|_| "test owner observer closed".to_owned())?;
                    let (released, condition) = &*worker_release;
                    let mut released = released
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    while !*released {
                        released = condition
                            .wait(released)
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                    }
                    Ok::<(), String>(())
                },
            )
            .await
        });
        let owner_entered = entered_rx.recv_timeout(Duration::from_secs(1));

        let observe_clear = |root: std::path::PathBuf| {
            let (response_tx, response_rx) = mpsc::channel();
            let observer = std::thread::spawn(move || {
                let response = crate::store::begin_clear_all_data(&root)
                    .map(drop)
                    .map_err(|error| error.to_string());
                let _ = response_tx.send(response);
            });
            (response_rx, observer)
        };
        let mut observers = Vec::new();
        let (before_abort, duplicate_owner) = if owner_entered.is_ok() {
            let (response_rx, observer) = observe_clear(root.clone());
            observers.push(observer);
            let response = response_rx.recv_timeout(Duration::from_secs(2));
            let busy = matches!(
                &response,
                Ok(Err(error)) if error.contains("cleanup is still pending")
            );
            let duplicate = if busy {
                run_retained_spool_store_job_with_registry(
                    registry.clone(),
                    "duplicate-history-save".into(),
                    root.clone(),
                    0,
                    |_| Ok::<(), String>(()),
                )
                .await
                .err()
            } else {
                None
            };
            (Some(response), duplicate)
        } else {
            (None, None)
        };

        caller.abort();
        let caller_abort = caller.await;
        let after_abort = if duplicate_owner.is_some() {
            let (response_rx, observer) = observe_clear(root.clone());
            observers.push(observer);
            Some(response_rx.recv_timeout(Duration::from_secs(2)))
        } else {
            None
        };

        // Always release the actual writer and reap every observer before assertions.
        release_spool_io(&release);
        let observer_joins = observers
            .into_iter()
            .map(|observer| observer.join())
            .collect::<Vec<_>>();
        await_spool_registry(&registry);

        assert!(
            owner_entered.is_ok(),
            "generic History write must enter owner"
        );
        assert!(
            observer_joins.iter().all(Result::is_ok),
            "clear observer panicked"
        );
        assert!(
            matches!(
                before_abort,
                Some(Ok(Err(ref error))) if error.contains("cleanup is still pending")
            ),
            "clear must report Busy before writer release"
        );
        assert!(
            duplicate_owner
                .as_deref()
                .is_some_and(|error| error.contains("cleanup is still pending")),
            "duplicate owner admission must remain Busy"
        );
        assert!(caller_abort.is_err(), "caller task must be aborted");
        assert!(
            matches!(
                after_abort,
                Some(Ok(Err(ref error))) if error.contains("cleanup is still pending")
            ),
            "clear must remain Busy after caller abort"
        );

        let clear = crate::store::begin_clear_all_data(&root)
            .expect("root admission releases only after the actual owner returns");
        let rejected = run_retained_spool_store_job_with_registry(
            registry.clone(),
            "after-clear-admission".into(),
            root.clone(),
            0,
            |_| Ok::<(), String>(()),
        )
        .await
        .unwrap_err();
        assert!(rejected.contains("data root is being cleared"));
        drop(clear);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn lost_capture_reply_keeps_one_bounded_owner_until_worker_exit() {
        use super::CaptureOwnerRegistry;
        use std::time::{Duration, Instant};

        let registry = CaptureOwnerRegistry::default();
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        registry
            .spawn("lost-reply".into(), move || {
                let _ = release_rx.recv();
            })
            .unwrap();
        let (_reply_tx, reply_rx) = mpsc::channel::<super::CaptureResult>();
        drop(_reply_tx);
        registry.set_late_result("lost-reply", Some(reply_rx));
        let start = Instant::now();
        assert!(registry.ensure_available().is_err());
        assert!(start.elapsed() < Duration::from_millis(100));
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while registry.ensure_available().is_err() {
            assert!(Instant::now() < deadline, "lost reply owner was not reaped");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn owned_replay_start_receipt_accepts_only_its_acknowledged_eof() {
        let receipt = OwnedReplayStartReceipt::test_fixture(12, 44);
        let event = OwnedReplayLifecycleEvent {
            phase: "eof".into(),
            source_identity: format!("sha256:{}", super::OWNED_WAV_SOURCE_SHA256),
            stream_id: 44,
            session_generation: 12,
            frame_count: 2,
            sample_count: 640,
            acknowledged: true,
        };
        assert!(receipt.confirms_eof(&event));

        let mut invalid = event.clone();
        invalid.stream_id += 1;
        assert!(!receipt.confirms_eof(&invalid));
        let mut invalid = event.clone();
        invalid.session_generation += 1;
        assert!(!receipt.confirms_eof(&invalid));
        let mut invalid = event.clone();
        invalid.source_identity = "sha256:unknown".into();
        assert!(!receipt.confirms_eof(&invalid));
        let mut invalid = event.clone();
        invalid.phase = "capture_failed".into();
        assert!(!receipt.confirms_eof(&invalid));
        let mut invalid = event.clone();
        invalid.acknowledged = false;
        assert!(!receipt.confirms_eof(&invalid));
        let mut invalid = event.clone();
        invalid.frame_count = 0;
        assert!(!receipt.confirms_eof(&invalid));
        let mut invalid = event;
        invalid.sample_count = 0;
        assert!(!receipt.confirms_eof(&invalid));
    }

    #[test]
    fn owned_wav_decoder_checks_hash_pcm_contract_and_sample_bound() {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(
                &mut cursor,
                hound::WavSpec {
                    channels: 1,
                    sample_rate: TARGET_RATE,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                },
            )
            .expect("wav writer");
            for sample in [7_i16, -9, 42] {
                writer.write_sample(sample).expect("sample");
            }
            writer.finalize().expect("wav finalize");
        }
        let bytes = cursor.into_inner();
        let digest = hex::encode(sha2::Sha256::digest(&bytes));
        assert_eq!(
            decode_owned_wav_bytes(&bytes, &digest).expect("valid owned-format wav"),
            [7, -9, 42]
        );
        assert_eq!(
            decode_owned_wav_bytes(&bytes, "00"),
            Err("source_identity_mismatch")
        );
        assert_eq!(
            decode_owned_wav_bytes(&[], &digest),
            Err("source_size_or_type_invalid")
        );
    }

    #[cfg(all(feature = "research-harness", target_os = "macos"))]
    #[test]
    fn owned_wav_source_rejects_fifo_without_waiting_for_a_writer() {
        use std::os::unix::ffi::OsStrExt;

        let path = std::env::temp_dir().join(format!(
            "voiceflow-owned-wav-fifo-{}-{}.wav",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos(),
        ));
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("path bytes");
        // SAFETY: this creates one uniquely named FIFO in the temporary
        // directory; it does not open or read from the FIFO.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let started = std::time::Instant::now();
        let result = super::OwnedWavReplay::from_path(&path);
        let elapsed = started.elapsed();
        let _ = std::fs::remove_file(path);
        assert_eq!(result.err(), Some("source_size_or_type_invalid"));
        assert!(elapsed < std::time::Duration::from_secs(1));
    }

    #[test]
    fn microphone_stop_distinguishes_enqueue_failure_and_missing_thread_ack() {
        let (tx, rx) = std::sync::mpsc::channel();
        drop(rx);
        let error = super::request_microphone_stop(&tx, "owned".into(), std::time::Duration::ZERO)
            .unwrap_err();
        assert!(error.starts_with("microphone_check_stop_enqueue_failed:"));

        let (tx, rx) = std::sync::mpsc::channel();
        let error = super::request_microphone_stop(&tx, "owned".into(), std::time::Duration::ZERO)
            .unwrap_err();
        assert!(error.starts_with("microphone_check_stop_ack_unconfirmed:"));
        match rx.try_recv().unwrap() {
            super::EngineCmd::StopMicCheck { session_id, reply } => {
                assert_eq!(session_id, "owned");
                // Enqueue succeeded, yet no terminal fact was delivered.
                assert!(reply
                    .send(crate::microphone_check::StopReceipt {
                        session_id,
                        state: crate::microphone_check::StopState::InactiveCancelled,
                    })
                    .is_err());
            }
            _ => panic!("wrong engine command"),
        }
    }

    #[test]
    fn microphone_stop_receipt_comes_from_the_real_audio_command_owner() {
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut check = super::MicCheckState::default();
            let mut active = None;
            let mut idle = None;
            let mut warm = None;
            let mut pending = std::collections::VecDeque::new();
            let command = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            super::handle_cmd(
                command,
                &mut active,
                &mut idle,
                &mut warm,
                &rx,
                &mut pending,
                &mut check,
            );
            assert!(check.active.is_none());
            assert!(check.cancellations.take("owned"));
            assert!(!check.cancellations.take("newer"));
            assert!(idle.is_none());
        });
        let receipt =
            super::request_microphone_stop(&tx, "owned".into(), std::time::Duration::from_secs(5))
                .unwrap();
        assert_eq!(receipt.session_id, "owned");
        assert_eq!(
            receipt.state,
            crate::microphone_check::StopState::InactiveCancelled
        );
        worker.join().unwrap();
    }

    #[test]
    fn input_gain_multiplies_samples() {
        let mut samples = vec![0.2, -0.1, 0.0];
        apply_input_gain(&mut samples, 2.0);
        assert_eq!(samples, vec![0.4, -0.2, 0.0]);
        apply_input_gain(&mut samples, 1.0);
        assert_eq!(samples, vec![0.4, -0.2, 0.0]);
    }

    #[test]
    fn input_gain_soft_limits_samples_above_full_scale() {
        let mut samples = vec![0.6, -0.8];
        apply_input_gain(&mut samples, 4.0);
        assert!(samples[0] > 0.9 && samples[0] <= 1.0, "{:?}", samples[0]);
        assert!(samples[1] < -0.9 && samples[1] >= -1.0, "{:?}", samples[1]);
    }

    #[test]
    fn soft_limiter_is_continuous_and_monotonic_across_full_scale() {
        for sign in [-1.0, 1.0] {
            let before = soft_limit_sample(sign * 0.999).abs();
            let at = soft_limit_sample(sign).abs();
            let after = soft_limit_sample(sign * 1.001).abs();
            assert!(before <= at && at <= after, "{before}, {at}, {after}");
            assert!(
                after - before < 0.002,
                "limiter step was {before} to {after}"
            );

            let mut previous = 0.0;
            for magnitude in [0.0, 0.1, 0.5, 0.9, 0.999, 1.0, 1.001, 1.5, 4.0, 32.0] {
                let current = soft_limit_sample(sign * magnitude).abs();
                assert!(current >= previous, "not monotonic at {magnitude}");
                assert!(current <= 1.0, "exceeded full scale: {current}");
                previous = current;
            }
        }
    }

    #[test]
    fn soft_limiter_returns_only_finite_bounded_samples() {
        for sample in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::MAX,
            f32::MIN,
            -1.001,
            1.001,
            0.0,
        ] {
            let limited = soft_limit_sample(sample);
            assert!(limited.is_finite(), "{sample} became {limited}");
            assert!(limited.abs() <= 1.0, "{sample} became {limited}");
        }
    }

    #[test]
    fn input_gain_preserves_low_volume_linear_gain() {
        let mut samples = vec![0.001, -0.02, 0.1];
        apply_input_gain(&mut samples, 1.5);
        assert_eq!(samples, vec![0.0015, -0.03, 0.15]);
    }

    #[test]
    fn input_gain_is_finite_and_nondecreasing_across_unity() {
        for sample in [-1.0, -0.91, -0.2, 0.2, 0.91, 1.0] {
            let outputs = [0.999, 1.0, 1.001].map(|gain| {
                let mut input = [sample];
                apply_input_gain(&mut input, gain);
                input[0]
            });
            if sample < 0.0 {
                assert!(
                    outputs.windows(2).all(|pair| pair[0] >= pair[1]),
                    "{sample}: {outputs:?}"
                );
            } else {
                assert!(
                    outputs.windows(2).all(|pair| pair[0] <= pair[1]),
                    "{sample}: {outputs:?}"
                );
            }
        }

        let mut unity = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.0, -1.0];
        apply_input_gain(&mut unity, 1.0);
        assert!(unity
            .iter()
            .all(|sample| sample.is_finite() && sample.abs() <= 1.0));
        assert!(unity[0].is_sign_positive());
        assert!(unity[1] > 0.0);
        assert!(unity[2] < 0.0);
    }

    #[test]
    fn pass_through_resampler_keeps_native_target_rate_samples() {
        let mut resampler = StreamResampler::new(TARGET_RATE).expect("target-rate resampler");
        let input = [0.1_f32, -0.2, 0.3];
        assert_eq!(resampler.push(&input).expect("pass-through samples"), input);
        assert!(resampler.finish().expect("finish pass-through").is_empty());
    }

    #[test]
    fn capture_queue_overflow_is_a_terminal_continuity_error() {
        let mut complete = None;
        apply_capture_continuity_failure(&mut complete, false);
        assert!(complete.is_none());

        apply_capture_continuity_failure(&mut complete, true);
        assert_eq!(
            complete.as_deref(),
            Some("audio capture queue overflowed; the recording is incomplete")
        );

        let mut resampler_error = Some("audio resampling failed".to_owned());
        apply_capture_continuity_failure(&mut resampler_error, true);
        assert_eq!(resampler_error.as_deref(), Some("audio resampling failed"));
    }

    #[test]
    fn real_capture_finish_latches_queue_overflow_before_asr_selection() {
        use super::{
            try_enqueue_capture_command, CaptureCommand, CaptureControl, CaptureResult,
            CaptureWorker, SpoolControl, SpoolFinalizer, SpoolOwnerRegistry,
        };
        use std::sync::atomic::{AtomicBool, AtomicU8};

        let (capture_tx, capture_rx) = mpsc::sync_channel(CaptureWorker::QUEUE_CAPACITY);
        let overflowed = Arc::new(AtomicBool::new(false));
        let worker_overflowed = overflowed.clone();
        for _ in 0..CaptureWorker::QUEUE_CAPACITY {
            try_enqueue_capture_command(
                &capture_tx,
                &overflowed,
                CaptureCommand::Samples(vec![0.25; 320]),
            )
            .expect("the 64 accepted callback blocks fit the real bounded queue");
        }
        assert_eq!(
            try_enqueue_capture_command(
                &capture_tx,
                &overflowed,
                CaptureCommand::Samples(vec![0.5; 320]),
            ),
            Err(true),
            "the next native callback block latches continuity loss"
        );
        assert!(overflowed.load(Ordering::Acquire));

        let (control_tx, control_rx) = mpsc::sync_channel(1);
        let (spool_control_tx, spool_control_rx) = mpsc::sync_channel(1);
        drop(spool_control_rx);
        let spool_control = SpoolControl {
            tx: spool_control_tx,
            requested: Arc::new(AtomicU8::new(0)),
            sealed: Arc::new(AtomicBool::new(false)),
            defer_recoverable_until_producer_closed: Arc::new(AtomicBool::new(false)),
            producer_closed: Arc::new(AtomicBool::new(false)),
            admission: Arc::new(Mutex::new(())),
            failed: Arc::new(AtomicBool::new(false)),
            writer_error: Arc::new(Mutex::new(None)),
            completed: Arc::new(AtomicBool::new(false)),
            registry: Arc::new(SpoolOwnerRegistry::default()),
        };
        let reply_control = spool_control.clone();
        let worker = std::thread::spawn(move || {
            if let Ok(CaptureControl::Finish { reply }) = control_rx.recv() {
                let mut samples = Vec::new();
                while let Ok(CaptureCommand::Samples(block)) = capture_rx.try_recv() {
                    samples.extend(block);
                }
                assert!(worker_overflowed.load(Ordering::Acquire));
                let _ = reply.send(CaptureResult {
                    samples,
                    processing_error: None,
                    session_dir: std::path::PathBuf::from("overflow-finish-test"),
                    spool_finalizer: SpoolFinalizer {
                        control: reply_control,
                    },
                    tail_flush: super::CaptureSpoolFlushReceipt::failed(
                        "overflow test has no spool owner".into(),
                    ),
                });
            }
        });
        let capture = CaptureWorker {
            tx: capture_tx,
            control_tx,
            recycle_rx: None,
            overflowed,
            continuity: Arc::new(Mutex::new(CaptureContinuityTracker::default())),
            stale_gap_latched: Arc::new(AtomicBool::new(false)),
            closed_lid_latched: Arc::new(AtomicBool::new(false)),
            rate_drift_latched: Arc::new(AtomicBool::new(false)),
            rate_drift: Arc::new(Mutex::new(RateDriftTracker::default())),
            limit_reached: Arc::new(AtomicBool::new(false)),
            started_at: std::time::Instant::now(),
            session_id: "overflow-finish-test".into(),
            spool_control,
            terminal_requested: false,
        }
        .finish()
        .expect("worker sends the finite accepted callback history");
        worker.join().unwrap();

        assert_eq!(capture.samples.len(), CaptureWorker::QUEUE_CAPACITY * 320);
        assert!(capture
            .processing_error
            .as_deref()
            .is_some_and(|error| { error.contains("capture queue overflowed") }));
        // `finalize` checks this field before encoding WAV or starting ASR; a
        // UI CaptureError event is not required to arrive first.
        assert!(capture.processing_error.is_some());
    }

    #[test]
    fn rate_drift_raises_capture_error_and_real_finish_preserves_pcm() {
        use super::{
            observe_rate_drift, CaptureCommand, CaptureControl, CaptureResult, CaptureWorker,
            RateDriftTracker, SpoolControl, SpoolFinalizer, SpoolOwnerRegistry,
            CAPTURE_RATE_DRIFT_LIMIT,
        };
        use std::sync::atomic::{AtomicBool, AtomicU8};

        let baseline = Mutex::new(RateDriftTracker {
            enabled: true,
            ..RateDriftTracker::default()
        });
        let report_latched = AtomicBool::new(false);
        let mut reported = 0;
        assert!(!observe_rate_drift(
            &baseline,
            &report_latched,
            Duration::from_secs(30),
            48_000,
            48_000,
            || reported += 1,
        ));
        assert!(
            !observe_rate_drift(
                &baseline,
                &report_latched,
                Duration::from_secs(31),
                12_000,
                48_000,
                || reported += 1,
            ),
            "exactly 750ms of absolute drift is allowed"
        );

        let mut baseline = RateDriftTracker {
            enabled: true,
            ..RateDriftTracker::default()
        };
        assert!(!baseline.accepted(Duration::ZERO, 48_000, 48_000));
        assert!(!baseline.accepted(Duration::from_secs(2), 12_000, 48_000));
        assert!(
            !baseline.accepted(Duration::from_millis(2_750), 12_000, 48_000,),
            "exactly 750ms above threshold duration is allowed"
        );
        assert!(
            baseline.accepted(Duration::from_nanos(2_750_000_001), 1, 48_000,),
            "same-direction drift must exceed both strict boundaries"
        );

        let mut recover = RateDriftTracker {
            enabled: true,
            ..RateDriftTracker::default()
        };
        assert!(!recover.accepted(Duration::ZERO, 48_000, 48_000));
        assert!(!recover.accepted(Duration::from_secs(2), 12_000, 48_000));
        assert!(!recover.accepted(Duration::from_millis(2_100), 96_000, 48_000));
        assert_eq!(
            recover.sustained_since, None,
            "returning within tolerance resets the window"
        );
        assert!(!recover.accepted(Duration::from_secs(4), 0, 48_000));
        assert!(!recover.accepted(Duration::from_millis(4_100), 150_000, 48_000));
        assert_eq!(
            recover.direction,
            Some(1),
            "a direction change starts a fresh window"
        );
        assert_eq!(recover.sustained_since, Some(Duration::from_millis(4_100)));
        assert!(
            !recover.accepted(Duration::from_millis(4_850), 36_000, 48_000,),
            "exactly 750ms of sustained same-direction drift is allowed"
        );

        let mut cadence = RateDriftTracker {
            enabled: true,
            ..RateDriftTracker::default()
        };
        assert!(!cadence.accepted(Duration::ZERO, 480, 48_000));
        for step in 1..=100 {
            assert!(!cadence.accepted(Duration::from_millis(step * 10), 480, 48_000,));
        }
        let owned = Mutex::new(RateDriftTracker::default());
        assert!(
            !observe_rate_drift(
                &owned,
                &report_latched,
                Duration::from_secs(20),
                0,
                48_000,
                || reported += 1,
            ),
            "owned-WAV replay has no hardware wall-clock drift monitor"
        );
        assert_eq!(reported, 0);
        assert_eq!(CAPTURE_RATE_DRIFT_LIMIT, Duration::from_millis(750));

        let (capture_tx, capture_rx) = mpsc::sync_channel(2);
        capture_tx
            .send(CaptureCommand::Samples(vec![0.125; 32]))
            .unwrap();
        let (control_tx, control_rx) = mpsc::sync_channel(1);
        let (spool_tx, spool_rx) = mpsc::sync_channel(1);
        drop(spool_rx);
        let spool_control = SpoolControl {
            tx: spool_tx,
            requested: Arc::new(AtomicU8::new(0)),
            sealed: Arc::new(AtomicBool::new(false)),
            defer_recoverable_until_producer_closed: Arc::new(AtomicBool::new(false)),
            producer_closed: Arc::new(AtomicBool::new(false)),
            admission: Arc::new(Mutex::new(())),
            failed: Arc::new(AtomicBool::new(false)),
            writer_error: Arc::new(Mutex::new(None)),
            completed: Arc::new(AtomicBool::new(false)),
            registry: Arc::new(SpoolOwnerRegistry::default()),
        };
        let reply_control = spool_control.clone();
        let worker = std::thread::spawn(move || {
            if let Ok(CaptureControl::Finish { reply }) = control_rx.recv() {
                let mut samples = Vec::new();
                while let Ok(CaptureCommand::Samples(block)) = capture_rx.try_recv() {
                    samples.extend(block);
                }
                let _ = reply.send(CaptureResult {
                    samples,
                    processing_error: None,
                    session_dir: std::path::PathBuf::from("rate-drift-finish-test"),
                    spool_finalizer: SpoolFinalizer {
                        control: reply_control,
                    },
                    tail_flush: super::CaptureSpoolFlushReceipt::failed(
                        "rate drift fixture has no spool owner".into(),
                    ),
                });
            }
        });
        let drift_latched = Arc::new(AtomicBool::new(false));
        let mut drift_monitor = RateDriftTracker {
            enabled: true,
            ..RateDriftTracker::default()
        };
        assert!(!drift_monitor.accepted(Duration::ZERO, 48_000, 48_000));
        assert!(!drift_monitor.accepted(Duration::from_secs(2), 12_000, 48_000));
        assert!(!drift_monitor.accepted(Duration::from_millis(2_750), 12_000, 48_000));
        assert!(observe_rate_drift(
            &Mutex::new(drift_monitor),
            &drift_latched,
            Duration::from_nanos(2_750_000_001),
            1,
            48_000,
            || reported += 1,
        ));
        assert_eq!(reported, 1, "production latch dispatch reports once");
        let capture = CaptureWorker {
            tx: capture_tx,
            control_tx,
            recycle_rx: None,
            overflowed: Arc::new(AtomicBool::new(false)),
            continuity: Arc::new(Mutex::new(super::CaptureContinuityTracker::default())),
            stale_gap_latched: Arc::new(AtomicBool::new(false)),
            closed_lid_latched: Arc::new(AtomicBool::new(false)),
            rate_drift_latched: drift_latched,
            rate_drift: Arc::new(Mutex::new(RateDriftTracker::default())),
            limit_reached: Arc::new(AtomicBool::new(false)),
            started_at: std::time::Instant::now(),
            session_id: "rate-drift-finish-test".into(),
            spool_control,
            terminal_requested: false,
        }
        .finish()
        .expect("real CaptureWorker finish consumes the incomplete capture");
        worker.join().unwrap();
        assert_eq!(capture.samples, vec![0.125; 32]);
        assert!(capture
            .processing_error
            .as_deref()
            .is_some_and(|error| { error.contains("rate drift") && error.contains("incomplete") }));
        let mut first_error = Some("resampler failed".to_owned());
        super::apply_capture_completion_failures(&mut first_error, false, false, false, true);
        assert_eq!(first_error.as_deref(), Some("resampler failed"));
        // finalize checks processing_error before WAV encoding and ASR selection.
    }

    #[test]
    fn stale_gap_after_first_frame_marks_capture_incomplete() {
        use super::{
            CaptureCommand, CaptureContinuityTracker, CaptureControl, CaptureResult, CaptureWorker,
            SpoolControl, SpoolFinalizer, SpoolOwnerRegistry,
        };
        use std::sync::atomic::{AtomicBool, AtomicU8};

        let freshness = super::CAPTURE_FRAME_FRESHNESS;
        let mut resumed = CaptureContinuityTracker::default();
        assert!(
            !resumed.close(Duration::from_secs(2)),
            "no first frame is waiting"
        );
        let mut continuous = CaptureContinuityTracker::default();
        assert!(!continuous.accepted_frame(Duration::ZERO));
        assert!(!continuous.accepted_frame(Duration::from_millis(500)));
        assert!(!continuous.close(Duration::from_millis(1_000)));
        let mut resumed = CaptureContinuityTracker::default();
        assert!(!resumed.accepted_frame(Duration::ZERO));
        assert!(resumed.accepted_frame(freshness + Duration::from_millis(300)));
        assert!(resumed.accepted_frame(freshness + Duration::from_millis(400)));
        let mut trailing = CaptureContinuityTracker::default();
        assert!(!trailing.accepted_frame(Duration::ZERO));
        assert!(trailing.close(freshness + Duration::from_millis(1)));

        let (capture_tx, capture_rx) = mpsc::sync_channel(2);
        capture_tx
            .send(CaptureCommand::Samples(vec![0.125; 32]))
            .unwrap();
        let (control_tx, control_rx) = mpsc::sync_channel(1);
        let stale_gap_latched = Arc::new(AtomicBool::new(false));
        let tracker = Arc::new(Mutex::new(CaptureContinuityTracker::default()));
        {
            let mut tracker = tracker.lock().unwrap();
            assert!(!tracker.accepted_frame(Duration::ZERO));
            let stale = tracker.close(freshness + Duration::from_millis(1));
            stale_gap_latched.store(stale, Ordering::Release);
        }
        let (spool_tx, spool_rx) = mpsc::sync_channel(1);
        drop(spool_rx);
        let spool_control = SpoolControl {
            tx: spool_tx,
            requested: Arc::new(AtomicU8::new(0)),
            sealed: Arc::new(AtomicBool::new(false)),
            defer_recoverable_until_producer_closed: Arc::new(AtomicBool::new(false)),
            producer_closed: Arc::new(AtomicBool::new(false)),
            admission: Arc::new(Mutex::new(())),
            failed: Arc::new(AtomicBool::new(false)),
            writer_error: Arc::new(Mutex::new(None)),
            completed: Arc::new(AtomicBool::new(false)),
            registry: Arc::new(SpoolOwnerRegistry::default()),
        };
        let reply_control = spool_control.clone();
        let worker = std::thread::spawn(move || {
            if let Ok(CaptureControl::Finish { reply }) = control_rx.recv() {
                let mut samples = Vec::new();
                while let Ok(CaptureCommand::Samples(block)) = capture_rx.try_recv() {
                    samples.extend(block);
                }
                let _ = reply.send(CaptureResult {
                    samples,
                    processing_error: None,
                    session_dir: std::path::PathBuf::from("stale-gap-finish-test"),
                    spool_finalizer: SpoolFinalizer {
                        control: reply_control,
                    },
                    tail_flush: super::CaptureSpoolFlushReceipt::failed(
                        "stale gap fixture has no spool owner".into(),
                    ),
                });
            }
        });
        let capture = CaptureWorker {
            tx: capture_tx,
            control_tx,
            recycle_rx: None,
            overflowed: Arc::new(AtomicBool::new(false)),
            continuity: tracker,
            stale_gap_latched,
            closed_lid_latched: Arc::new(AtomicBool::new(false)),
            rate_drift_latched: Arc::new(AtomicBool::new(false)),
            rate_drift: Arc::new(Mutex::new(RateDriftTracker::default())),
            limit_reached: Arc::new(AtomicBool::new(false)),
            started_at: std::time::Instant::now(),
            session_id: "stale-gap-finish-test".into(),
            spool_control,
            terminal_requested: false,
        }
        .finish()
        .expect("real CaptureWorker finish returns retained accepted samples");
        worker.join().unwrap();
        assert_eq!(capture.samples, vec![0.125; 32]);
        assert!(capture
            .processing_error
            .as_deref()
            .is_some_and(|error| { error.contains("callback continuity was interrupted") }));
        // finalize checks processing_error before WAV encoding and ASR selection.
    }

    #[test]
    fn normal_capture_close_uses_capture_epoch_when_ui_epoch_differs() {
        use super::{
            close_capture_continuity_at, CaptureCommand, CaptureContinuityTracker, CaptureControl,
            CaptureWorker, RateDriftTracker, SpoolControl, SpoolOwnerRegistry,
        };
        use std::sync::atomic::AtomicBool;

        let capture_started_at = std::time::Instant::now();
        let ui_started_at = capture_started_at + Duration::from_millis(100);
        let (tx, _rx) = mpsc::sync_channel::<CaptureCommand>(1);
        let (control_tx, _control_rx) = mpsc::sync_channel::<CaptureControl>(1);
        let (spool_tx, _spool_rx) = mpsc::sync_channel(1);
        let mut capture = CaptureWorker {
            tx,
            control_tx,
            recycle_rx: None,
            overflowed: Arc::new(AtomicBool::new(false)),
            continuity: Arc::new(Mutex::new(CaptureContinuityTracker::default())),
            stale_gap_latched: Arc::new(AtomicBool::new(false)),
            closed_lid_latched: Arc::new(AtomicBool::new(false)),
            rate_drift_latched: Arc::new(AtomicBool::new(false)),
            rate_drift: Arc::new(Mutex::new(RateDriftTracker::default())),
            limit_reached: Arc::new(AtomicBool::new(false)),
            started_at: capture_started_at,
            session_id: "epoch-close-test".into(),
            spool_control: SpoolControl {
                tx: spool_tx,
                requested: Arc::new(std::sync::atomic::AtomicU8::new(0)),
                sealed: Arc::new(AtomicBool::new(false)),
                defer_recoverable_until_producer_closed: Arc::new(AtomicBool::new(false)),
                producer_closed: Arc::new(AtomicBool::new(false)),
                admission: Arc::new(Mutex::new(())),
                failed: Arc::new(AtomicBool::new(false)),
                writer_error: Arc::new(Mutex::new(None)),
                completed: Arc::new(AtomicBool::new(false)),
                registry: Arc::new(SpoolOwnerRegistry::default()),
            },
            terminal_requested: false,
        };
        assert!(ui_started_at > capture.started_at);
        // Production callback timestamps from capture.started_at: accepted at 110ms.
        capture
            .continuity
            .lock()
            .unwrap()
            .accepted_frame(Duration::from_millis(110));
        let slot = Mutex::new(Some(()));
        // Normal Stop/detach closes at wall 910ms. The UI epoch would report 810ms,
        // incorrectly reducing the terminal gap to 700ms; the capture epoch reports 910ms.
        close_capture_continuity_at(
            &capture,
            &slot,
            capture_started_at + Duration::from_millis(910),
        );
        assert!(slot.lock().unwrap().is_none());
        assert!(capture.stale_gap_latched.load(Ordering::Acquire));

        // Exactly 750ms of terminal silence remains within the allowed boundary.
        capture.continuity = Arc::new(Mutex::new(CaptureContinuityTracker::default()));
        capture.stale_gap_latched = Arc::new(AtomicBool::new(false));
        capture
            .continuity
            .lock()
            .unwrap()
            .accepted_frame(Duration::from_millis(110));
        close_capture_continuity_at(
            &capture,
            &Mutex::new(Some(())),
            capture_started_at + Duration::from_millis(860),
        );
        assert!(!capture.stale_gap_latched.load(Ordering::Acquire));
    }

    #[test]
    fn autostop_closes_continuity_before_delayed_stop() {
        use super::{close_capture_admission, CaptureContinuityTracker, CAPTURE_FRAME_FRESHNESS};
        use std::sync::atomic::AtomicBool;

        let slot = Mutex::new(Some(()));
        let continuity = Mutex::new(CaptureContinuityTracker::default());
        let stale_latched = AtomicBool::new(false);
        {
            let mut tracker = continuity.lock().unwrap();
            assert!(!tracker.accepted_frame(Duration::from_secs(899)));
        }

        // This is the AutoStop admission edge; the subsequent Stop is delayed
        // well beyond freshness but must not count time after capture closed.
        let auto_stop_at = Duration::from_secs(899) + CAPTURE_FRAME_FRESHNESS;
        close_capture_admission(&slot, &continuity, &stale_latched, auto_stop_at);
        assert!(slot.lock().unwrap().is_none());
        close_capture_admission(
            &slot,
            &continuity,
            &stale_latched,
            auto_stop_at + Duration::from_secs(2),
        );
        assert!(!stale_latched.load(Ordering::Acquire));
        assert!(!continuity
            .lock()
            .unwrap()
            .close(auto_stop_at + Duration::from_secs(2)));

        let gap = Mutex::new(CaptureContinuityTracker::default());
        let gap_slot = Mutex::new(Some(()));
        let gap_latched = AtomicBool::new(false);
        {
            let mut tracker = gap.lock().unwrap();
            assert!(!tracker.accepted_frame(Duration::ZERO));
        }
        close_capture_admission(
            &gap_slot,
            &gap,
            &gap_latched,
            CAPTURE_FRAME_FRESHNESS + Duration::from_millis(1),
        );
        assert!(gap_latched.load(Ordering::Acquire));
    }

    #[test]
    fn intentional_limit_closes_at_worker_endpoint_before_delayed_autostop() {
        use super::{close_capture_admission, close_intentional_limit, CaptureContinuityTracker};
        use std::sync::atomic::AtomicBool;

        let slot = Mutex::new(Some(()));
        let continuity = Mutex::new(CaptureContinuityTracker::default());
        let limit_reached = AtomicBool::new(false);
        let stale_latched = AtomicBool::new(false);
        {
            let mut tracker = continuity.lock().unwrap();
            assert!(!tracker.accepted_frame(Duration::from_secs(899)));
        }

        // Worker limit processing is the intentional endpoint. AutoStop may
        // sit in the engine queue for longer than the freshness window.
        let endpoint = Duration::from_secs(899) + super::CAPTURE_FRAME_FRESHNESS;
        assert!(close_intentional_limit(
            &continuity,
            &limit_reached,
            &stale_latched,
            endpoint,
        ));
        assert!(!stale_latched.load(Ordering::Acquire));
        let closed_snapshot = continuity.lock().unwrap().stale_gap;

        let delayed_autostop = endpoint + Duration::from_secs(2);
        close_capture_admission(&slot, &continuity, &stale_latched, delayed_autostop);
        assert!(slot.lock().unwrap().is_none());
        assert!(!stale_latched.load(Ordering::Acquire));
        assert_eq!(continuity.lock().unwrap().stale_gap, closed_snapshot);
        assert!(!close_intentional_limit(
            &continuity,
            &limit_reached,
            &stale_latched,
            delayed_autostop + Duration::from_secs(2),
        ));
        assert_eq!(continuity.lock().unwrap().stale_gap, closed_snapshot);

        // Closing at the limit preserves a real gap already observed before
        // the worker reaches its intentional endpoint.
        let prior_gap = Mutex::new(CaptureContinuityTracker::default());
        let prior_limit = AtomicBool::new(false);
        let prior_stale = AtomicBool::new(false);
        {
            let mut tracker = prior_gap.lock().unwrap();
            assert!(!tracker.accepted_frame(Duration::ZERO));
            assert!(tracker.accepted_frame(Duration::from_secs(2)));
        }
        assert!(close_intentional_limit(
            &prior_gap,
            &prior_limit,
            &prior_stale,
            Duration::from_secs(2) + super::CAPTURE_FRAME_FRESHNESS,
        ));
        assert!(prior_stale.load(Ordering::Acquire));
    }

    #[test]
    fn intentional_limit_waits_for_inflight_callback_clock_accounting() {
        use super::{close_intentional_limit, CaptureContinuityTracker};
        use std::sync::atomic::AtomicBool;

        let continuity = Arc::new(Mutex::new(CaptureContinuityTracker::default()));
        let limit_reached = Arc::new(AtomicBool::new(false));
        let stale_latched = Arc::new(AtomicBool::new(false));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (samples_tx, samples_rx) = mpsc::sync_channel(1);

        let callback_continuity = continuity.clone();
        let callback = std::thread::spawn(move || {
            let mut tracker = callback_continuity.lock().unwrap();
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            samples_tx.send(vec![0.25_f32; 4]).unwrap();
            assert!(!tracker.accepted_frame(Duration::from_secs(1)));
        });
        entered_rx.recv().unwrap();

        let worker_continuity = continuity.clone();
        let worker_limit = limit_reached.clone();
        let worker_stale = stale_latched.clone();
        let endpoint = Duration::from_secs(1) + super::CAPTURE_FRAME_FRESHNESS;
        let worker = std::thread::spawn(move || {
            close_intentional_limit(&worker_continuity, &worker_limit, &worker_stale, endpoint)
        });

        // The callback owns the admission/clock mutex until its already
        // enqueued block has also been accounted. The worker endpoint waits.
        release_tx.send(()).unwrap();
        callback.join().unwrap();
        assert!(worker.join().unwrap());
        assert_eq!(samples_rx.recv().unwrap(), vec![0.25_f32; 4]);
        assert!(!stale_latched.load(Ordering::Acquire));
        assert!(continuity.lock().unwrap().closed);
        assert!(!continuity
            .lock()
            .unwrap()
            .accepted_frame(endpoint + Duration::from_secs(2)));
    }

    #[test]
    fn sinc_resampler_zero_flush_covers_exact_rational_source_duration() {
        for rate in [44_100_u32, 48_000] {
            for source_count in [
                1_023_usize,
                1_024,
                1_025,
                30 * 1_024,
                30 * 1_024 + 1,
                rate as usize,
            ] {
                let mut input = vec![0.0_f32; source_count];
                if let Some(first) = input.first_mut() {
                    *first = 0.75;
                }
                if let Some(last) = input.last_mut() {
                    *last = -0.5;
                }
                let mut resampler = StreamResampler::new(rate).expect("native-rate resampler");
                let mut output = Vec::new();
                for block in input.chunks(257) {
                    output.extend(resampler.push(block).expect("resampler push"));
                }
                output.extend(resampler.finish().expect("explicit sinc zero flush"));

                assert_eq!(
                    output.len(),
                    expected_resampled_samples(source_count as u64, rate),
                    "rate={rate}, source_count={source_count}"
                );
                assert!(
                    output.iter().take(128).any(|sample| sample.abs() > 1.0e-7),
                    "start impulse was absent: rate={rate}, source_count={source_count}"
                );
                assert!(
                    output
                        .iter()
                        .rev()
                        .take(128)
                        .any(|sample| sample.abs() > 1.0e-7),
                    "end impulse tail was absent: rate={rate}, source_count={source_count}"
                );
            }
        }
    }

    #[test]
    fn resampler_flush_is_callback_shape_independent_at_chunk_boundaries() {
        fn resample(input: &[f32], rate: u32, callback_size: usize) -> Vec<f32> {
            let mut resampler = StreamResampler::new(rate).expect("resampler");
            let mut output = Vec::new();
            for block in input.chunks(callback_size) {
                output.extend(resampler.push(block).expect("resampler push"));
            }
            output.extend(resampler.finish().expect("tail flush"));
            output
        }

        for rate in [16_000_u32, 44_100, 48_000] {
            for source_count in [1_023_usize, 1_024, 1_025, 30 * 1_024, 30 * 1_024 + 1] {
                let mut input = (0..source_count)
                    .map(|index| {
                        (index as f32 * std::f32::consts::TAU * 700.0 / rate as f32).sin() * 0.2
                    })
                    .collect::<Vec<_>>();
                if let Some(first) = input.first_mut() {
                    *first += 0.6;
                }
                if let Some(last) = input.last_mut() {
                    *last -= 0.4;
                }

                let reference = resample(&input, rate, 257);
                assert_eq!(
                    reference.len(),
                    expected_resampled_samples(source_count as u64, rate),
                    "rate={rate}, source_count={source_count}"
                );
                for callback_size in [1, 1_023, 1_024, 1_025] {
                    let actual = resample(&input, rate, callback_size);
                    assert_eq!(
                        actual, reference,
                        "callback_size={callback_size}, rate={rate}, source_count={source_count}"
                    );
                }
                assert!(
                    reference.iter().rev().take(128).any(|sample| sample.abs() > 1.0e-7),
                    "the valid end impulse tail was trimmed: rate={rate}, source_count={source_count}"
                );
            }
        }
    }

    #[test]
    fn bounded_append_reports_recording_limit() {
        let mut samples = vec![0.0; 4];
        assert!(!append_bounded(&mut samples, &[1.0, 2.0], 8));
        assert_eq!(samples.len(), 6);
        assert!(append_bounded(&mut samples, &[3.0, 4.0, 5.0], 8));
        assert_eq!(samples.len(), 8);
    }

    #[test]
    fn audio_level_gates_silence_and_clamps_loud_input() {
        assert_eq!(normalized_audio_level(&[0.0; 32]), 0.0);
        assert_eq!(normalized_audio_level(&[1.0; 32]), 1.0);
        assert!(normalized_audio_level(&[0.08; 32]) > 0.2);
    }

    #[test]
    fn audio_level_rises_quickly_and_decays_smoothly() {
        let rising = smooth_audio_level(0.0, 1.0);
        let falling = smooth_audio_level(1.0, 0.0);
        assert!(rising < 1.0);
        assert!(falling > 0.0);
        assert!(falling > rising);
    }

    #[test]
    fn empty_recording_uses_the_same_no_speech_copy_as_asr() {
        assert_eq!(AudioError::EmptyRecording.to_string(), "No speech detected");
    }

    #[test]
    fn capture_events_are_bound_to_the_open_session_generation() {
        assert!(super::capture_session_matches(12, 7, 12, Some(7)));
        assert!(!super::capture_session_matches(12, 7, 12, Some(6)));
        assert!(!super::capture_session_matches(12, 7, 13, Some(7)));
        assert!(!super::capture_session_matches(12, 7, 12, None));
        assert!(super::stream_failure_matches(12, 12));
        assert!(!super::stream_failure_matches(12, 13));
    }

    #[test]
    fn meter_freshness_expires_then_recovers_only_after_a_new_accepted_frame() {
        use super::CaptureFreshness::{Recent, Stale, Waiting};

        assert_eq!(super::capture_freshness(false, 5_000, 0), Waiting);
        assert_eq!(super::capture_freshness(true, 10_750, 10_000), Recent);
        assert_eq!(super::capture_freshness(true, 10_751, 10_000), Stale);
        assert_eq!(super::capture_freshness(true, 20_100, 20_000), Recent);
    }

    #[test]
    fn no_audio_notice_waits_for_five_seconds_without_accepted_frames() {
        use std::time::Duration;

        assert!(!super::no_audio_notice_due(
            false,
            false,
            Duration::from_millis(4_999),
        ));
        assert!(super::no_audio_notice_due(
            false,
            false,
            Duration::from_secs(5),
        ));
        assert!(!super::no_audio_notice_due(
            true,
            false,
            Duration::from_secs(10),
        ));
        assert!(!super::no_audio_notice_due(
            false,
            true,
            Duration::from_secs(10),
        ));
    }
}

#[cfg(test)]
mod tail_and_idle_regressions {
    use super::*;
    #[test]
    fn ordinary_commands_preserve_order_and_do_not_shorten_buffer() {
        let (tx, rx) = mpsc::channel();
        for command in 0..8 {
            tx.send(command).unwrap();
        }
        let mut pending = std::collections::VecDeque::new();
        let start = std::time::Instant::now();
        assert!(wait_tail_commands(
            &StopOptions {
                extra_ms: 80,
                ..StopOptions::default()
            },
            &rx,
            &mut pending,
            TailCommand::Pending
        ));
        assert!(start.elapsed() >= Duration::from_millis(75));
        assert_eq!(
            pending.into_iter().chain(rx.try_iter()).collect::<Vec<_>>(),
            (0..8).collect::<Vec<_>>()
        );
    }
    #[test]
    fn cancellation_does_not_wait_for_two_second_buffer() {
        let (_tx, rx) = mpsc::channel::<u8>();
        let token = tokio_util::sync::CancellationToken::new();
        let cancel = token.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            cancel.cancel();
        });
        let start = std::time::Instant::now();
        assert!(!wait_tail_commands(
            &StopOptions {
                extra_ms: 2000,
                cancellation: token,
                ..StopOptions::default()
            },
            &rx,
            &mut Default::default(),
            TailCommand::Pending
        ));
        assert!(start.elapsed() < Duration::from_millis(500));
        thread.join().unwrap();
    }
    #[test]
    fn priority_stop_preempts_buffer_without_discarding_pending() {
        let (tx, rx) = mpsc::channel();
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        tx.send(99).unwrap();
        let mut pending = Default::default();
        assert!(wait_tail_commands(
            &StopOptions {
                extra_ms: 2000,
                ..StopOptions::default()
            },
            &rx,
            &mut pending,
            |command| if command == 99 {
                TailCommand::Stop(true)
            } else {
                TailCommand::Pending(command)
            }
        ));
        assert_eq!(pending.into_iter().collect::<Vec<_>>(), vec![1, 2]);
    }
    #[test]
    fn reused_callback_gate_discards_idle_samples_across_three_sessions() {
        let slot = Mutex::new(None::<Vec<i32>>);
        assert!(with_capture_session(&slot, |samples| samples.push(99)).is_none());
        for session in 0..3 {
            *slot.lock().unwrap() = Some(Vec::new());
            with_capture_session(&slot, |samples| samples.push(session));
            assert_eq!(slot.lock().unwrap().take(), Some(vec![session]));
            assert!(with_capture_session(&slot, |samples| samples.push(99)).is_none());
        }
    }
}
