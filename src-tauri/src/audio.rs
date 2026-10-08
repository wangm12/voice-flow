//! Microphone capture and crash-safe audio spooling.
use crate::chunker::{AudioChunk, Chunker, ChunkerConfig};
use crate::prefetch_asr::{PrefetchInbox, PrefetchMessage};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rubato::Resampler;
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager};
use thiserror::Error;

const TARGET_RATE: u32 = 16_000;
const LEVEL_EMIT_INTERVAL: Duration = Duration::from_millis(33);
const SPOOL_CHUNK_SAMPLES: usize = TARGET_RATE as usize;
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
    },
    CloseMicCheck,
    Start {
        app: AppHandle,
        session: String,
        input_device: String,
        chunk_length_secs: usize,
        max_recording_secs: usize,
        input_gain: f32,
        prefetch_tx: PrefetchInbox,
        soniox_audio: Option<crate::soniox::SonioxAudioSender>,
        preserve_full_audio: bool,
        readiness: Option<mpsc::Sender<()>>,
        reply: mpsc::Sender<Result<StartHandle, AudioError>>,
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
    DeviceError {
        stream_id: u64,
        app: AppHandle,
        message: String,
    },
}

type StopResult = Result<(Vec<u8>, Vec<AudioChunk>), AudioError>;
type StopReply = mpsc::Sender<StopResult>;

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
struct StartHandle {}

struct CallbackSession {
    state: CapturePushState,
    recycle_rx: mpsc::Receiver<Vec<f32>>,
    readiness: Option<mpsc::Sender<()>>,
}
type SessionSlot = Arc<Mutex<Option<CallbackSession>>>;
struct IdleStream {
    stream_id: u64,
    stream: cpal::Stream,
    slot: SessionSlot,
    input_device: String,
    rate: u32,
    channels: usize,
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
}

impl StreamResampler {
    fn new(from_rate: u32) -> Result<Self, String> {
        if from_rate == TARGET_RATE {
            return Ok(Self {
                resampler: None,
                pending: Vec::new(),
                chunk_size: 0,
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
        })
    }

    /// Feed native-rate mono samples; returns newly available 16 kHz samples.
    fn push(&mut self, input: &[f32]) -> Result<Vec<f32>, String> {
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
            out.extend_from_slice(&chan);
        }
        Ok(out)
    }

    /// Flush any remaining samples at end of recording.
    fn finish(&mut self) -> Result<Vec<f32>, String> {
        let Some(resampler) = self.resampler.as_mut() else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        if !self.pending.is_empty() {
            let block: Vec<f32> = std::mem::take(&mut self.pending);
            let channels_in = [block];
            let processed = resampler
                .process_partial(Some(&channels_in), None)
                .map_err(|error| format!("audio resampling failed while finishing: {error}"))?;
            let chan = processed
                .into_iter()
                .next()
                .ok_or_else(|| "audio resampler returned no output channel".to_owned())?;
            out.extend_from_slice(&chan);
        }
        Ok(out)
    }
}

struct ActiveRec {
    stream_id: u64,
    stream: Option<cpal::Stream>,
    slot: SessionSlot,
    input_device: String,
    rate: u32,
    channels: usize,
    reused_stream: bool,
    device_failed: bool,
    capture: CaptureWorker,
    encode_spool: PathBuf,
    chunk_length_secs: usize,
    app: AppHandle,
    level: Arc<AtomicU32>,
    preserve_full_audio: bool,
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
        samples: Vec<f32>,
    },
    Finish {
        reply: mpsc::Sender<Result<(), String>>,
    },
    Cancel,
}

enum CaptureCommand {
    Samples(Vec<f32>),
    Finish { reply: mpsc::Sender<CaptureResult> },
    Cancel,
}

struct CaptureResult {
    samples: Vec<f32>,
    spool_result: Result<(), String>,
    processing_error: Option<String>,
}

struct CaptureWorker {
    tx: mpsc::SyncSender<CaptureCommand>,
    recycle_rx: Option<mpsc::Receiver<Vec<f32>>>,
    cancelled: Arc<AtomicBool>,
    overflowed: Arc<AtomicBool>,
    limit_reached: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

struct CapturePushState {
    stream_id: u64,
    capture_tx: mpsc::SyncSender<CaptureCommand>,
    capture_overflowed: Arc<AtomicBool>,
    capture_limit_reached: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    app: AppHandle,
    device_error_tx: mpsc::Sender<EngineCmd>,
}

impl CaptureWorker {
    const QUEUE_CAPACITY: usize = 64;

    #[allow(clippy::too_many_arguments)]
    fn new(
        root: PathBuf,
        session_dir: PathBuf,
        input_rate: u32,
        max_samples: usize,
        chunk_length_secs: usize,
        input_gain: f32,
        prefetch_tx: PrefetchInbox,
        soniox_audio: Option<crate::soniox::SonioxAudioSender>,
        preserve_full_audio: bool,
        stream_id: u64,
        auto_stop_tx: mpsc::Sender<EngineCmd>,
        app: AppHandle,
    ) -> Result<Self, AudioError> {
        let resampler = StreamResampler::new(input_rate).map_err(AudioError::Device)?;
        let (tx, rx) = mpsc::sync_channel(Self::QUEUE_CAPACITY);
        let (recycle_tx, recycle_rx) = mpsc::sync_channel(Self::QUEUE_CAPACITY);
        for _ in 0..Self::QUEUE_CAPACITY {
            let _ = recycle_tx.try_send(Vec::with_capacity(2_048));
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let overflowed = Arc::new(AtomicBool::new(false));
        let worker_overflowed = overflowed.clone();
        let limit_reached = Arc::new(AtomicBool::new(false));
        let worker_limit_reached = limit_reached.clone();
        let join = thread::spawn(move || {
            let mut resampler = resampler;
            let mut data = Vec::new();
            // Batch prefetch uploads completed files; this is not streaming ASR.
            let mut prefetch_chunker = Chunker::new(ChunkerConfig { chunk_length_secs });
            let warmup_samples = crate::prefetch_asr::WARMUP_CHUNK_SECS * TARGET_RATE as usize;
            let mut warmup_pending = Vec::with_capacity(warmup_samples);
            let mut warmup_sent = false;
            let allow_prefetch = !preserve_full_audio;
            let mut processing_error: Option<String> = None;
            let spool_pending = Arc::new(Mutex::new(PendingSpool {
                samples: Vec::with_capacity(SPOOL_CHUNK_SAMPLES),
                next_index: 0,
            }));
            let mut spool_writer = Some(SpoolWriter::new(root, session_dir));

            while let Ok(command) = rx.recv() {
                if worker_cancelled.load(Ordering::Acquire) {
                    if let Some(writer) = spool_writer.take() {
                        writer.cancel();
                    }
                    break;
                }
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
                                    let _ = auto_stop_tx.send(EngineCmd::DeviceError {
                                        stream_id,
                                        app: app.clone(),
                                        message,
                                    });
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
                        let remaining = max_samples.saturating_sub(data.len());
                        let accepted = produced.len().min(remaining);
                        let start_sample = data.len() as u64;
                        let hit_limit = append_bounded(&mut data, &produced, max_samples);
                        if let Some(sender) = &soniox_audio {
                            let _ = sender.try_send_samples(start_sample, &produced[..accepted]);
                        }
                        if let Some(writer) = spool_writer.as_ref() {
                            append_spool(
                                &spool_pending,
                                Some(&writer.sender),
                                &produced[..accepted],
                            );
                        }
                        if allow_prefetch && !warmup_sent {
                            let remaining = warmup_samples.saturating_sub(warmup_pending.len());
                            warmup_pending.extend_from_slice(&produced[..accepted.min(remaining)]);
                            if warmup_pending.len() == warmup_samples {
                                let warmup = AudioChunk {
                                    index: 0,
                                    identity: crate::chunker::AudioChunkIdentity::from_samples_at(
                                        0,
                                        &warmup_pending,
                                    ),
                                    samples: std::mem::take(&mut warmup_pending),
                                    source_start_sample: 0,
                                    start_secs: 0.0,
                                    end_secs: crate::prefetch_asr::WARMUP_CHUNK_SECS as f32,
                                };
                                warmup_sent = prefetch_tx.try_send(PrefetchMessage::Warmup(warmup));
                            }
                        }
                        if allow_prefetch {
                            for chunk in prefetch_chunker.push(&produced[..accepted]) {
                                let _ = prefetch_tx.try_send(PrefetchMessage::Chunk(chunk));
                            }
                        }
                        if hit_limit && !worker_limit_reached.swap(true, Ordering::AcqRel) {
                            let _ = auto_stop_tx.send(EngineCmd::AutoStop { app: app.clone() });
                        }
                    }
                    CaptureCommand::Finish { reply } => {
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
                            let remaining = max_samples.saturating_sub(data.len());
                            let accepted = trailing.len().min(remaining);
                            let start_sample = data.len() as u64;
                            let _ = append_bounded(&mut data, &trailing, max_samples);
                            if let Some(sender) = &soniox_audio {
                                let _ =
                                    sender.try_send_samples(start_sample, &trailing[..accepted]);
                            }
                            if let Some(writer) = spool_writer.as_ref() {
                                append_spool(
                                    &spool_pending,
                                    Some(&writer.sender),
                                    &trailing[..accepted],
                                );
                            }
                            if allow_prefetch && !warmup_sent {
                                let remaining = warmup_samples.saturating_sub(warmup_pending.len());
                                warmup_pending
                                    .extend_from_slice(&trailing[..accepted.min(remaining)]);
                                if warmup_pending.len() == warmup_samples {
                                    let warmup = AudioChunk {
                                        index: 0,
                                        identity:
                                            crate::chunker::AudioChunkIdentity::from_samples_at(
                                                0,
                                                &warmup_pending,
                                            ),
                                        samples: std::mem::take(&mut warmup_pending),
                                        source_start_sample: 0,
                                        start_secs: 0.0,
                                        end_secs: crate::prefetch_asr::WARMUP_CHUNK_SECS as f32,
                                    };
                                    let _ = prefetch_tx.try_send(PrefetchMessage::Warmup(warmup));
                                }
                            }
                            if allow_prefetch {
                                for chunk in prefetch_chunker.push(&trailing[..accepted]) {
                                    let _ = prefetch_tx.try_send(PrefetchMessage::Chunk(chunk));
                                }
                            }
                        }
                        let spool_result = spool_writer
                            .take()
                            .map(|writer| writer.finish_with_pending(spool_pending))
                            .unwrap_or(Ok(()));
                        let _ = reply.send(CaptureResult {
                            samples: data,
                            spool_result,
                            processing_error,
                        });
                        break;
                    }
                    CaptureCommand::Cancel => {
                        if let Some(writer) = spool_writer.take() {
                            writer.cancel();
                        }
                        break;
                    }
                }
            }
        });
        Ok(Self {
            tx,
            recycle_rx: Some(recycle_rx),
            cancelled,
            overflowed,
            limit_reached,
            join: Some(join),
        })
    }

    fn take_recycle_rx(&mut self) -> mpsc::Receiver<Vec<f32>> {
        self.recycle_rx
            .take()
            .expect("capture recycle receiver is available before stream creation")
    }

    fn finish(mut self) -> Result<CaptureResult, String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(CaptureCommand::Finish { reply: reply_tx })
            .map_err(|_| "audio capture worker stopped".to_owned())?;
        let result = reply_rx
            .recv_timeout(Duration::from_secs(15))
            .map_err(|_| "audio capture worker timed out".to_owned());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        result
    }

    fn cancel(mut self) {
        self.cancelled.store(true, Ordering::Release);
        let _ = self.tx.send(CaptureCommand::Cancel);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[derive(Clone)]
struct SpoolSender {
    tx: mpsc::SyncSender<SpoolCommand>,
    failed: Arc<AtomicBool>,
}

impl SpoolSender {
    fn enqueue(&self, index: usize, samples: Vec<f32>) {
        if self.failed.load(Ordering::Acquire) || samples.is_empty() {
            return;
        }
        let start_secs = index as f32;
        let end_secs = start_secs + samples.len() as f32 / TARGET_RATE as f32;
        if self
            .tx
            .try_send(SpoolCommand::Samples {
                index,
                start_secs,
                end_secs,
                samples,
            })
            .is_err()
        {
            // The callback must never block on disk I/O. The in-memory copy
            // remains authoritative for the current recording; this flag
            // only indicates that crash recovery may be partial.
            self.failed.store(true, Ordering::Release);
        }
    }
}

struct SpoolWriter {
    sender: SpoolSender,
    join: Option<thread::JoinHandle<()>>,
}

impl SpoolWriter {
    fn new(root: PathBuf, session_dir: PathBuf) -> Self {
        let (tx, rx) = mpsc::sync_channel(16);
        let failed = Arc::new(AtomicBool::new(false));
        let worker_failed = failed.clone();
        let join = thread::spawn(move || {
            let mut worker_error: Option<String> = None;
            while let Ok(command) = rx.recv() {
                match command {
                    SpoolCommand::Samples {
                        index,
                        start_secs,
                        end_secs,
                        samples,
                    } => {
                        if worker_error.is_some() {
                            continue;
                        }
                        let Some(session_id) =
                            session_dir.file_name().and_then(|value| value.to_str())
                        else {
                            worker_error = Some("invalid spool session path".into());
                            worker_failed.store(true, Ordering::Release);
                            continue;
                        };
                        let relative = PathBuf::from(session_id)
                            .join("chunks")
                            .join(format!("{index:08}.f32"));
                        let bytes = samples
                            .iter()
                            .flat_map(|sample| sample.to_le_bytes())
                            .collect::<Vec<_>>();
                        if let Err(error) = crate::store::write_spool_file(&root, &relative, &bytes)
                            .and_then(|_| {
                                crate::store::record_spool_chunk(
                                    &session_dir,
                                    index,
                                    start_secs,
                                    end_secs,
                                    "written",
                                )
                            })
                        {
                            worker_failed.store(true, Ordering::Release);
                            worker_error = Some(error.to_string());
                        }
                    }
                    SpoolCommand::Finish { reply } => {
                        let result = if worker_failed.load(Ordering::Acquire) {
                            Err(worker_error
                                .unwrap_or_else(|| "audio spool queue overflowed".into()))
                        } else {
                            Ok(())
                        };
                        let _ = reply.send(result);
                        break;
                    }
                    SpoolCommand::Cancel => break,
                }
            }
        });
        Self {
            sender: SpoolSender { tx, failed },
            join: Some(join),
        }
    }

    fn finish_with_pending(mut self, pending: Arc<Mutex<PendingSpool>>) -> Result<(), String> {
        let (index, samples) = {
            let mut pending = pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let index = pending.next_index;
            pending.next_index = pending.next_index.wrapping_add(1);
            (index, std::mem::take(&mut pending.samples))
        };
        self.sender.enqueue(index, samples);
        let (reply_tx, reply_rx) = mpsc::channel();
        let result = self
            .sender
            .tx
            .send(SpoolCommand::Finish { reply: reply_tx })
            .map_err(|_| "audio spool worker stopped".to_owned())
            .and_then(|_| {
                reply_rx
                    .recv_timeout(Duration::from_secs(15))
                    .map_err(|_| "audio spool worker timed out".to_owned())
            });
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        result.and_then(|inner| inner)
    }

    fn cancel(mut self) {
        let _ = self.sender.tx.send(SpoolCommand::Cancel);
        if let Some(join) = self.join.take() {
            let _ = join.join();
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
                if let Some(rec) = active.as_ref() {
                    if rec.stream.is_some() && !rec.device_failed {
                        emit_audio_level(&rec.app, &rec.level);
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
        meter.emit(reason);
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
        stop_microphone_check(session_id);
    }
    result?
}

pub fn stop_microphone_check(session_id: String) {
    let _ = engine().tx.send(EngineCmd::StopMicCheck { session_id });
}

pub fn close_microphone_check() {
    let _ = engine().tx.send(EngineCmd::CloseMicCheck);
}

fn detach_capture(rec: &mut ActiveRec, idle: &mut Option<IdleStream>, warm: &Option<WarmConfig>) {
    // Serialize with the callback before flushing the worker. Idle callbacks drop their input.
    rec.slot.lock().unwrap_or_else(|e| e.into_inner()).take();
    if !rec.device_failed
        && warm
            .as_ref()
            .is_some_and(|config| config.enabled && config.input_device == rec.input_device)
    {
        if let Some(stream) = rec.stream.take() {
            *idle = Some(IdleStream {
                stream_id: rec.stream_id,
                stream,
                slot: rec.slot.clone(),
                input_device: rec.input_device.clone(),
                rate: rec.rate,
                channels: rec.channels,
            });
        }
    } else {
        drop(rec.stream.take());
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
        EngineCmd::DeviceError {
            stream_id,
            app,
            message,
        } => {
            if stream_id != rec.stream_id {
                return TailCommand::Pending(EngineCmd::DeviceError {
                    stream_id,
                    app,
                    message,
                });
            }
            rec.device_failed = true;
            rec.slot.lock().unwrap_or_else(|e| e.into_inner()).take();
            drop(rec.stream.take());
            let _ = app.emit("audio://error", message);
            TailCommand::Stop(true)
        }
        EngineCmd::AutoStop { app } => {
            rec.slot.lock().unwrap_or_else(|e| e.into_inner()).take();
            drop(rec.stream.take());
            let _ = app.emit(
                "audio://limit",
                serde_json::json!({ "max_recording_secs": MAX_RECORDING_SECS }),
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

fn emit_audio_level(app: &AppHandle, level: &Arc<AtomicU32>) {
    let level = f32::from_bits(level.load(Ordering::Acquire)).clamp(0.0, 1.0);
    let _ = app.emit("audio://level", serde_json::json!({ "level": level }));
}

fn reset_audio_level(app: &AppHandle, level: &Arc<AtomicU32>) {
    level.store(0.0_f32.to_bits(), Ordering::Release);
    emit_audio_level(app, level);
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
        EngineCmd::StopMicCheck { session_id } => {
            if check
                .active
                .as_ref()
                .is_some_and(|meter| meter.session_id == session_id)
            {
                finish_mic_check(check, "stopped");
                restore_warm_after_check(idle, warm);
            } else {
                check.cancellations.remember(session_id);
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
                    .is_none_or(|stream| stream.input_device != config.input_device)
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
            input_device,
            chunk_length_secs,
            max_recording_secs,
            input_gain,
            prefetch_tx,
            soniox_audio,
            preserve_full_audio,
            readiness,
            reply,
        } => {
            finish_mic_check(check, "interrupted");
            if active.is_some() {
                let _ = reply.send(Err(AudioError::Device("recording already active".into())));
                return;
            }
            match build_stream(
                &app,
                &session,
                &input_device,
                chunk_length_secs,
                max_recording_secs,
                input_gain,
                prefetch_tx,
                soniox_audio,
                preserve_full_audio,
                idle,
                readiness,
            ) {
                Ok(rec) => {
                    let handle = StartHandle {};
                    let mut rec = rec;
                    if let Err(e) = if rec.reused_stream {
                        Ok(())
                    } else {
                        rec.stream.as_ref().expect("stream is present").play()
                    } {
                        drop(rec.stream.take());
                        rec.capture.cancel();
                        if let Some(dir) = rec.encode_spool.parent() {
                            let _ = std::fs::remove_dir_all(dir);
                        }
                        let _ = reply.send(Err(AudioError::Device(e.to_string())));
                    } else {
                        *active = Some(rec);
                        let _ = reply.send(Ok(handle));
                    }
                }
                Err(e) => {
                    let _ = reply.send(Err(e));
                }
            }
        }
        EngineCmd::Stop { options, reply } => {
            let result = match active.take() {
                Some(mut rec) => {
                    reset_audio_level(&rec.app, &rec.level);
                    let completed = wait_recording_tail(&options, &mut rec, rx, pending);
                    detach_capture(&mut rec, idle, warm);
                    if rec.device_failed {
                        // A cancellation racing device failure must retain recovery
                        // chunks, matching the ordinary engine cancel path.
                        preserve_partial_recording(rec);
                        Err(AudioError::Device(
                            "microphone device failed; recording preserved".into(),
                        ))
                    } else if !completed {
                        let dir = rec.encode_spool.parent().map(PathBuf::from);
                        rec.capture.cancel();
                        if let Some(dir) = dir {
                            let _ = std::fs::remove_dir_all(dir);
                        }
                        Err(AudioError::Device("recording cancelled".into()))
                    } else {
                        finalize(rec, options.vad_enabled)
                    }
                }
                None => Err(AudioError::NotRecording),
            };
            let _ = reply.send(result);
        }
        EngineCmd::Cancel { reply } => {
            if let Some(mut rec) = active.take() {
                detach_capture(&mut rec, idle, warm);
                reset_audio_level(&rec.app, &rec.level);
                if rec.device_failed {
                    preserve_partial_recording(rec);
                } else {
                    let dir = rec.encode_spool.parent().map(PathBuf::from);
                    drop(rec.stream);
                    rec.capture.cancel();
                    if let Some(dir) = dir {
                        let _ = std::fs::remove_dir_all(dir);
                    }
                }
            }
            let _ = reply.send(());
        }
        EngineCmd::AutoStop { app } => {
            if let Some(rec) = active.as_mut() {
                rec.slot.lock().unwrap_or_else(|e| e.into_inner()).take();
                if rec.stream.take().is_some() {
                    reset_audio_level(&app, &rec.level);
                    let _ = app.emit(
                        "audio://limit",
                        serde_json::json!({ "max_recording_secs": MAX_RECORDING_SECS }),
                    );
                }
            }
        }
        EngineCmd::DeviceError {
            stream_id,
            app,
            message,
        } => {
            if let Some(rec) = active.as_mut() {
                if stream_id == rec.stream_id && !rec.device_failed {
                    rec.device_failed = true;
                    rec.slot.lock().unwrap_or_else(|e| e.into_inner()).take();
                    drop(rec.stream.take());
                    reset_audio_level(&app, &rec.level);
                    let _ = app.emit("audio://error", message);
                }
            }
            if active.is_none()
                && idle
                    .as_ref()
                    .is_some_and(|idle| idle.stream_id == stream_id)
            {
                *idle = None;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_stream(
    app: &AppHandle,
    session: &str,
    input_device: &str,
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
    let device = resolve_input_device(&host, input_device)?;
    let supported = device
        .default_input_config()
        .map_err(|e| AudioError::Device(e.to_string()))?;
    let config = supported.config();
    let channels = config.channels as usize;
    let rate = config.sample_rate.0;
    let max_samples = max_recording_secs.saturating_mul(TARGET_RATE as usize);
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| AudioError::Device(e.to_string()))?
        .join("spool")
        .join(session);
    let root = app
        .path()
        .app_data_dir()
        .map_err(|e| AudioError::Device(e.to_string()))?;
    crate::store::begin_spool_session(&root, session)
        .map_err(|e| AudioError::Device(e.to_string()))?;
    let encode_spool = dir.join("recording.raw");
    let auto_stop_tx = engine().tx.clone();
    let device_error_tx = auto_stop_tx.clone();
    let reusable = idle.take().filter(|stream| {
        stream.input_device == input_device && stream.rate == rate && stream.channels == channels
    });
    let stream_id = reusable
        .as_ref()
        .map(|stream| stream.stream_id)
        .unwrap_or_else(next_stream_id);
    let mut capture = match CaptureWorker::new(
        root,
        dir.clone(),
        rate,
        max_samples,
        chunk_length_secs,
        input_gain,
        prefetch_tx,
        soniox_audio.clone(),
        preserve_full_audio,
        stream_id,
        auto_stop_tx.clone(),
        app.clone(),
    ) {
        Ok(capture) => capture,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(error);
        }
    };
    let level = Arc::new(AtomicU32::new(0.0_f32.to_bits()));
    let callback = CallbackSession {
        state: CapturePushState {
            stream_id,
            capture_tx: capture.tx.clone(),
            capture_overflowed: capture.overflowed.clone(),
            capture_limit_reached: capture.limit_reached.clone(),
            level: level.clone(),
            app: app.clone(),
            device_error_tx,
        },
        recycle_rx: capture.take_recycle_rx(),
        readiness,
    };
    let reused_stream = reusable.is_some();
    let (stream, slot) = if let Some(stream) = reusable {
        *stream.slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(callback);
        (stream.stream, stream.slot)
    } else {
        let slot = Arc::new(Mutex::new(Some(callback)));
        match make_input_stream(app, &device, &supported, slot.clone(), stream_id) {
            Ok(stream) => (stream, slot),
            Err(error) => {
                capture.cancel();
                let _ = std::fs::remove_dir_all(&dir);
                return Err(error);
            }
        }
    };
    Ok(ActiveRec {
        stream_id,
        stream: Some(stream),
        slot,
        input_device: input_device.to_owned(),
        rate,
        channels,
        reused_stream,
        device_failed: false,
        capture,
        encode_spool,
        chunk_length_secs,
        app: app.clone(),
        level,
        preserve_full_audio,
    })
}

fn next_stream_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
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
        let _ = engine().tx.send(EngineCmd::DeviceError {
            stream_id,
            app: app.clone(),
            message: error.to_string(),
        });
    };
    macro_rules! build {
        ($ty:ty) => {
            device.build_input_stream(
                &config,
                move |data: &[$ty], _| {
                    with_capture_session(&slot, |callback| {
                        if push(data, channels, &mut callback.recycle_rx, &callback.state) {
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
        stream,
        slot,
        input_device: input_device.to_owned(),
        rate: supported.sample_rate().0,
        channels: supported.channels() as usize,
    })
}

pub(crate) fn resolve_input_device(
    host: &cpal::Host,
    selected_name: &str,
) -> Result<cpal::Device, AudioError> {
    if selected_name.trim().is_empty() {
        return host.default_input_device().ok_or(AudioError::NoDevice);
    }
    let selected_name = selected_name.trim();
    let devices = host.input_devices().map_err(|error| {
        AudioError::Device(format!("could not enumerate input devices: {error}"))
    })?;
    for device in devices {
        if device.name().ok().as_deref() == Some(selected_name) {
            return Ok(device);
        }
    }
    Err(AudioError::Device(format!(
        "selected input device is unavailable: {selected_name}"
    )))
}

/// Preserve the chunks already written when the input device disappears. The
/// in-memory recording may be incomplete, but the durable chunks remain
/// retryable after the next launch instead of being deleted as a cancellation.
fn preserve_partial_recording(mut rec: ActiveRec) {
    let session_dir = rec.encode_spool.parent().map(PathBuf::from);
    drop(rec.stream.take());
    let capture_result = rec.capture.finish();
    if let Some(dir) = session_dir {
        let status = match capture_result {
            Ok(result) if result.processing_error.is_none() && result.spool_result.is_ok() => {
                "recoverable"
            }
            _ => "degraded",
        };
        let _ = crate::store::mark_spool_status(&dir, status);
    }
}

fn finalize(rec: ActiveRec, vad_enabled: bool) -> Result<(Vec<u8>, Vec<AudioChunk>), AudioError> {
    let ActiveRec {
        stream,
        device_failed: _,
        capture,
        encode_spool,
        chunk_length_secs,
        preserve_full_audio,
        ..
    } = rec;
    drop(stream); // no callback can append after the stream is dropped.
    let capture = capture.finish().map_err(AudioError::Device)?;
    if let Some(error) = capture.processing_error {
        return Err(AudioError::Device(error));
    }
    if let Err(error) = capture.spool_result {
        log::warn!("audio spool finalization was partial: {error}");
    }
    let samples = if preserve_full_audio {
        capture.samples
    } else {
        crate::vad::filter_speech(&capture.samples, vad_enabled)
    };
    if samples.is_empty() {
        return Err(AudioError::EmptyRecording);
    }
    // Samples are already at 16 kHz (resampled inline during capture).
    let mut chunker = Chunker::new(ChunkerConfig { chunk_length_secs });
    let mut chunks = chunker.push(&samples);
    if let Some(last) = chunker.finish() {
        chunks.push(last);
    }
    let wav = encode(samples, encode_spool)?;
    Ok((wav, chunks))
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
            input_device,
            chunk_length_secs,
            input_gain,
            prefetch_tx,
            None,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start_with_soniox(
        &mut self,
        app: AppHandle,
        session: &str,
        input_device: &str,
        chunk_length_secs: usize,
        input_gain: f32,
        prefetch_tx: PrefetchInbox,
        soniox_audio: Option<crate::soniox::SonioxAudioSender>,
        preserve_full_audio: bool,
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
                input_device: input_device.to_owned(),
                chunk_length_secs,
                max_recording_secs: MAX_RECORDING_SECS,
                input_gain,
                prefetch_tx,
                soniox_audio,
                preserve_full_audio,
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
    ) -> Result<(Vec<u8>, Vec<AudioChunk>), AudioError> {
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
                // Device-failure recovery flushes the capture and spool
                // workers before acknowledging the cancel. Their bounded
                // finish path allows up to 15 seconds, so returning after
                // five could make the caller scan the spool too early and
                // miss the just-recorded recovery item.
                let _ = rx.recv_timeout(Duration::from_secs(20));
            }
        }
    }
}

fn push<T: Copy + cpal::Sample>(
    input: &[T],
    channels: usize,
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
    let frame_count = input.len() / channels.max(1);
    let mut mono = recycle_rx
        .try_recv()
        .unwrap_or_else(|_| Vec::with_capacity(frame_count));
    mono.clear();
    if mono.capacity() < frame_count {
        mono.reserve(frame_count - mono.capacity());
    }
    for frame in input.chunks(channels.max(1)) {
        let v = frame.iter().map(|v| (*v).to_sample::<f32>()).sum::<f32>() / frame.len() as f32;
        mono.push(v);
    }
    let raw_level = normalized_audio_level(&mono);
    let previous_level = f32::from_bits(state.level.load(Ordering::Relaxed));
    let smoothed_level = smooth_audio_level(previous_level, raw_level);
    state
        .level
        .store(smoothed_level.to_bits(), Ordering::Relaxed);
    let accepted = state
        .capture_tx
        .try_send(CaptureCommand::Samples(mono))
        .is_ok();
    if !accepted && !state.capture_overflowed.swap(true, Ordering::AcqRel) {
        let _ = state.device_error_tx.send(EngineCmd::DeviceError {
            stream_id: state.stream_id,
            app: state.app.clone(),
            message: "audio processing queue overflowed".into(),
        });
    }
    accepted
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

fn encode(input: Vec<f32>, spool: PathBuf) -> Result<Vec<u8>, AudioError> {
    std::fs::write(
        &spool,
        input
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect::<Vec<_>>(),
    )
    .map_err(|e| AudioError::Encode(e.to_string()))?;
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
    std::fs::remove_file(&spool).map_err(|e| AudioError::Encode(e.to_string()))?;
    if let Some(session_dir) = spool.parent() {
        let _ = crate::store::mark_spool_status(session_dir, "completed");
        let _ = std::fs::remove_dir_all(session_dir);
    }
    Ok(bytes.into_inner())
}

#[cfg(test)]
mod tests {
    use super::{
        append_bounded, apply_input_gain, normalized_audio_level, smooth_audio_level,
        soft_limit_sample, AudioError, StreamResampler, TARGET_RATE,
    };

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
            pending.into_iter().collect::<Vec<_>>(),
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
