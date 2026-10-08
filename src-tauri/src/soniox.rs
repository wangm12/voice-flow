//! Soniox's fixed WebSocket streaming adapter.
//!
//! Live audio is supplied as ordered, mono 16 kHz floating-point samples.
//! Only finalized Soniox tokens enter the returned transcript. Provisional
//! tokens are deliberately discarded by this module.

use crate::asr::{AsrError, AsrToken, RateLimits, Transcript};
use crate::metrics::{AsrRequestDiagnostics, AsrStreamAttemptGuard};
use futures_util::{SinkExt, StreamExt};
use hound::{SampleFormat, WavReader};
use serde::Deserialize;
use serde_json::json;
use std::io::Cursor;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;
use tokio::time::{interval_at, timeout, Instant as TokioInstant, MissedTickBehavior};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as WebSocketError, Message};
use tokio_tungstenite::{connect_async_with_config, MaybeTlsStream, WebSocketStream};
use tokio_util::sync::CancellationToken;

pub const SONIOX_WEBSOCKET_ENDPOINT: &str = "wss://stt-rt.soniox.com/transcribe-websocket";
pub const SONIOX_MODEL: &str = "stt-rt-v5";

const SAMPLE_RATE: u64 = 16_000;
const CHANNEL_COUNT: u16 = 1;
const QUEUE_CAPACITY: usize = 32;
const AUDIO_FRAME_SAMPLES: usize = 4_096;
const REPLAY_FRAME_SAMPLES: usize = 1_600;
const MAX_STREAM_SAMPLES: u64 = 300 * 60 * SAMPLE_RATE;
const MAX_TRANSCRIPT_CHARS: usize = 1_000_000;
const MAX_FINAL_TOKENS: usize = 100_000;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const CONFIG_TIMEOUT: Duration = Duration::from_secs(8);
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
const QUEUE_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
const FINALIZE_TIMEOUT: Duration = Duration::from_secs(60);
const FINISH_WAIT_TIMEOUT: Duration = Duration::from_secs(120);
const SHUTDOWN_WAIT_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_FRAME_BYTES: usize = 1_048_576;

type SonioxSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug, Clone, Error)]
#[error("{error}")]
pub struct SonioxStreamFailure {
    pub error: AsrError,
    /// Whether callers may recover by replaying the complete captured audio
    /// once to this same endpoint.
    pub replayable: bool,
    /// A bounded, non-sensitive diagnostic category.
    pub failure_code: &'static str,
    /// Successfully queued to the WebSocket sink before this attempt ended.
    pub audio_seconds_sent: f64,
}

#[derive(Debug, Clone)]
pub struct SonioxStreamOptions {
    pub api_key: String,
    /// Saved app language (`auto`, `zh`, or `en`). Unknown values leave the
    /// provider's own automatic language detection enabled.
    pub language: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SonioxAudioSendError {
    SequenceMismatch { expected: u64, received: u64 },
    QueueFull,
    Closed,
    InvalidAudio,
    SampleIndexOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SonioxAudioCoverageStatus {
    /// First sample index not yet accepted by the bounded sender queue.
    pub next_sample: u64,
    /// Total samples accepted from sample index zero.
    pub accepted_samples: u64,
    pub sealed: bool,
    pub failed: bool,
    pub closed: bool,
}

#[derive(Debug, Clone)]
struct AudioFrame {
    start_sample: u64,
    sample_count: u64,
    bytes: Arc<[u8]>,
}

#[derive(Debug, Clone)]
enum ProducerFailure {
    SequenceMismatch { expected: u64, received: u64 },
    QueueFull,
    Closed,
    InvalidAudio,
    SampleIndexOverflow,
}

impl From<&ProducerFailure> for SonioxAudioSendError {
    fn from(value: &ProducerFailure) -> Self {
        match value {
            ProducerFailure::SequenceMismatch { expected, received } => Self::SequenceMismatch {
                expected: *expected,
                received: *received,
            },
            ProducerFailure::QueueFull => Self::QueueFull,
            ProducerFailure::Closed => Self::Closed,
            ProducerFailure::InvalidAudio => Self::InvalidAudio,
            ProducerFailure::SampleIndexOverflow => Self::SampleIndexOverflow,
        }
    }
}

#[derive(Default)]
struct ProducerState {
    next_sample: u64,
    sealed: bool,
    closed: bool,
    failure: Option<ProducerFailure>,
}

/// Cloneable, nonblocking producer handle for the live microphone path.
///
/// Samples must start at the exact next accepted index. A gap, duplicate,
/// queue overflow, or invalid sample poisons the attempt so the caller can
/// recover with the complete locally captured WAV.
#[derive(Clone)]
pub struct SonioxAudioSender {
    queue: mpsc::Sender<AudioFrame>,
    state: Arc<Mutex<ProducerState>>,
    stop: CancellationToken,
    failure_notify: Arc<Notify>,
}

impl SonioxAudioSender {
    /// Tries to enqueue ordered mono 16 kHz samples without waiting for network
    /// capacity. The samples are encoded as signed 16-bit little-endian PCM.
    pub fn try_send_samples(
        &self,
        start_sample: u64,
        samples: &[f32],
    ) -> Result<SonioxAudioCoverageStatus, SonioxAudioSendError> {
        if samples.is_empty() {
            return Ok(self.coverage_status());
        }

        let mut state = lock_state(&self.state);
        if let Some(failure) = state.failure.as_ref() {
            return Err(failure.into());
        }
        if state.sealed || state.closed || self.stop.is_cancelled() {
            return Err(SonioxAudioSendError::Closed);
        }
        if start_sample != state.next_sample {
            let failure = ProducerFailure::SequenceMismatch {
                expected: state.next_sample,
                received: start_sample,
            };
            state.failure = Some(failure.clone());
            drop(state);
            self.signal_failure();
            return Err((&failure).into());
        }
        if samples.iter().any(|sample| !sample.is_finite()) {
            let failure = ProducerFailure::InvalidAudio;
            state.failure = Some(failure.clone());
            drop(state);
            self.signal_failure();
            return Err((&failure).into());
        }
        let input_count = match u64::try_from(samples.len()) {
            Ok(count) => count,
            Err(_) => {
                let failure = ProducerFailure::SampleIndexOverflow;
                state.failure = Some(failure.clone());
                drop(state);
                self.signal_failure();
                return Err((&failure).into());
            }
        };
        if start_sample
            .checked_add(input_count)
            .is_none_or(|end| end > MAX_STREAM_SAMPLES)
        {
            let failure = ProducerFailure::SampleIndexOverflow;
            state.failure = Some(failure.clone());
            drop(state);
            self.signal_failure();
            return Err((&failure).into());
        }

        let accepted_start = state.next_sample;
        for part in samples.chunks(AUDIO_FRAME_SAMPLES) {
            let mut pcm = Vec::with_capacity(part.len() * 2);
            for sample in part {
                pcm.extend_from_slice(&sample_to_i16(*sample).to_le_bytes());
            }
            let sample_count = part.len() as u64;
            let frame = AudioFrame {
                start_sample: state.next_sample,
                sample_count,
                bytes: Arc::from(pcm),
            };
            match self.queue.try_send(frame) {
                Ok(()) => {
                    state.next_sample += sample_count;
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    let failure = ProducerFailure::QueueFull;
                    state.failure = Some(failure.clone());
                    drop(state);
                    self.signal_failure();
                    return Err((&failure).into());
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    let failure = ProducerFailure::Closed;
                    state.failure = Some(failure.clone());
                    drop(state);
                    self.signal_failure();
                    return Err((&failure).into());
                }
            }
        }

        let status = SonioxAudioCoverageStatus {
            next_sample: state.next_sample,
            accepted_samples: state.next_sample,
            sealed: state.sealed,
            failed: state.failure.is_some(),
            closed: state.closed,
        };
        debug_assert_eq!(state.next_sample - accepted_start, input_count);
        Ok(status)
    }

    /// Returns the exact producer-side coverage accepted by the bounded queue.
    pub fn coverage_status(&self) -> SonioxAudioCoverageStatus {
        let state = lock_state(&self.state);
        SonioxAudioCoverageStatus {
            next_sample: state.next_sample,
            accepted_samples: state.next_sample,
            sealed: state.sealed,
            failed: state.failure.is_some(),
            closed: state.closed,
        }
    }

    /// Queues complete-WAV replay audio with bounded async backpressure.
    /// Unlike the microphone callback path, recovery may wait briefly for
    /// sender capacity while preserving real-time audio cadence.
    async fn send_samples_bounded(
        &self,
        start_sample: u64,
        samples: &[f32],
        cancellation: &CancellationToken,
    ) -> Result<SonioxAudioCoverageStatus, SonioxAudioSendError> {
        if samples.is_empty() {
            return Ok(self.coverage_status());
        }
        for (part_index, part) in samples.chunks(AUDIO_FRAME_SAMPLES).enumerate() {
            let part_offset = (part_index * AUDIO_FRAME_SAMPLES) as u64;
            self.send_frame_bounded(start_sample + part_offset, part, cancellation)
                .await?;
        }
        Ok(self.coverage_status())
    }

    async fn send_frame_bounded(
        &self,
        start_sample: u64,
        samples: &[f32],
        cancellation: &CancellationToken,
    ) -> Result<(), SonioxAudioSendError> {
        let input_count = u64::try_from(samples.len())
            .map_err(|_| self.fail_producer(ProducerFailure::SampleIndexOverflow))?;
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err(self.fail_producer(ProducerFailure::InvalidAudio));
        }
        let sequence_mismatch = {
            let state = lock_state(&self.state);
            if let Some(failure) = state.failure.as_ref() {
                return Err(failure.into());
            }
            if state.sealed || state.closed || self.stop.is_cancelled() {
                return Err(SonioxAudioSendError::Closed);
            }
            if start_sample != state.next_sample {
                Some(state.next_sample)
            } else {
                None
            }
        };
        if let Some(expected) = sequence_mismatch {
            return Err(self.fail_producer(ProducerFailure::SequenceMismatch {
                expected,
                received: start_sample,
            }));
        }
        if start_sample
            .checked_add(input_count)
            .is_none_or(|end| end > MAX_STREAM_SAMPLES)
        {
            return Err(self.fail_producer(ProducerFailure::SampleIndexOverflow));
        }

        let mut pcm = Vec::with_capacity(samples.len() * 2);
        for sample in samples {
            pcm.extend_from_slice(&sample_to_i16(*sample).to_le_bytes());
        }
        let frame = AudioFrame {
            start_sample,
            sample_count: input_count,
            bytes: Arc::from(pcm),
        };
        let permit = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(SonioxAudioSendError::Closed),
            _ = self.stop.cancelled() => return Err(self.current_error()),
            result = timeout(QUEUE_WAIT_TIMEOUT, self.queue.reserve()) => match result {
                Err(_) => return Err(self.fail_producer(ProducerFailure::QueueFull)),
                Ok(Err(_)) => return Err(self.fail_producer(ProducerFailure::Closed)),
                Ok(Ok(permit)) => permit,
            }
        };
        let mut state = lock_state(&self.state);
        if let Some(failure) = state.failure.as_ref() {
            return Err(failure.into());
        }
        if state.sealed || state.closed || self.stop.is_cancelled() {
            return Err(SonioxAudioSendError::Closed);
        }
        if state.next_sample != start_sample {
            let expected = state.next_sample;
            drop(state);
            return Err(self.fail_producer(ProducerFailure::SequenceMismatch {
                expected,
                received: start_sample,
            }));
        }
        permit.send(frame);
        state.next_sample += input_count;
        Ok(())
    }

    fn current_error(&self) -> SonioxAudioSendError {
        lock_state(&self.state)
            .failure
            .as_ref()
            .map(Into::into)
            .unwrap_or(SonioxAudioSendError::Closed)
    }

    fn fail_producer(&self, failure: ProducerFailure) -> SonioxAudioSendError {
        let failure = {
            let mut state = lock_state(&self.state);
            match state.failure.as_ref() {
                Some(existing) => existing.clone(),
                None => {
                    state.failure = Some(failure.clone());
                    failure
                }
            }
        };
        self.signal_failure();
        (&failure).into()
    }

    fn signal_failure(&self) {
        self.failure_notify.notify_one();
        self.stop.cancel();
    }
}

pub struct SonioxStreamSession {
    finish_sender: Option<mpsc::Sender<u64>>,
    result_task: Option<JoinHandle<Result<Transcript, SonioxStreamFailure>>>,
    state: Arc<Mutex<ProducerState>>,
    stop: CancellationToken,
    cancel_on_drop: bool,
}

impl SonioxStreamSession {
    /// Creates the bounded producer immediately and performs the fixed
    /// endpoint connection/config handshake in a background task. Capture can
    /// begin while the service connects, and `finish` may be queued before
    /// startup completes. Any startup failure is returned by `finish`.
    pub fn start(
        options: SonioxStreamOptions,
        cancellation: CancellationToken,
        diagnostics: Option<AsrRequestDiagnostics>,
    ) -> (Self, SonioxAudioSender) {
        let attempt_started = Instant::now();
        let diagnostics_guard = diagnostics.map(|diagnostics| diagnostics.begin_stream());
        let (mut session, sender, queue, finish) = new_session_shell();
        let task_state = session.state.clone();
        let task_stop = session.stop.clone();
        let task_failure_notify = sender.failure_notify.clone();
        let task_cancellation = cancellation.clone();
        let task_policy_cancellation = crate::network_policy::cloud_request_token();
        session.result_task = Some(tokio::spawn(async move {
            let connection = tokio::select! {
                biased;
                _ = task_stop.cancelled() => {
                    let failure = lock_state(&task_state)
                        .failure
                        .as_ref()
                        .map(|failure| producer_failure(failure, 0))
                        .unwrap_or_else(|| cancelled_failure(0));
                    Err(failure)
                }
                _ = task_cancellation.cancelled() => Err(cancelled_failure(0)),
                _ = task_policy_cancellation.cancelled() => Err(cancelled_failure(0)),
                result = open_socket_and_config(options, task_cancellation.clone()) => result,
            };
            let socket = match connection {
                Ok(socket) => socket,
                Err(failure) => {
                    lock_state(&task_state).closed = true;
                    complete_guard(
                        diagnostics_guard,
                        Some(failure.failure_code),
                        attempt_started.elapsed(),
                        0.0,
                        0.0,
                    );
                    return Err(failure);
                }
            };
            tokio::select! {
                biased;
                _ = task_policy_cancellation.cancelled() => Err(cancelled_failure(0)),
                result = run_stream_actor(
                    socket,
                    queue,
                    finish,
                    task_state,
                    task_stop,
                    task_failure_notify,
                    task_cancellation,
                    attempt_started,
                    diagnostics_guard,
                    false,
                ) => result,
            }
        }));
        (session, sender)
    }

    async fn connect_inner(
        options: SonioxStreamOptions,
        cancellation: CancellationToken,
        diagnostics: Option<AsrRequestDiagnostics>,
        is_replay: bool,
    ) -> Result<(Self, SonioxAudioSender), SonioxStreamFailure> {
        let attempt_started = Instant::now();
        let diagnostics_guard = diagnostics.map(|diagnostics| diagnostics.begin_stream());
        let socket = match open_socket_and_config(options, cancellation.clone()).await {
            Ok(socket) => socket,
            Err(failure) => {
                complete_guard(
                    diagnostics_guard,
                    Some(failure.failure_code),
                    attempt_started.elapsed(),
                    0.0,
                    0.0,
                );
                return Err(failure);
            }
        };
        let (mut session, sender, queue, finish) = new_session_shell();
        let task_state = session.state.clone();
        let task_stop = session.stop.clone();
        let task_failure_notify = sender.failure_notify.clone();
        session.result_task = Some(tokio::spawn(async move {
            run_stream_actor(
                socket,
                queue,
                finish,
                task_state,
                task_stop,
                task_failure_notify,
                cancellation,
                attempt_started,
                diagnostics_guard,
                is_replay,
            )
            .await
        }));
        Ok((session, sender))
    }

    /// Seals the producer at the exact expected sample count, drains every
    /// accepted frame, sends an empty binary finalize frame, and waits for the
    /// server's explicit `finished: true` response.
    pub async fn finish(
        mut self,
        expected_samples: u64,
    ) -> Result<Transcript, SonioxStreamFailure> {
        {
            let mut state = lock_state(&self.state);
            state.sealed = true;
        }

        if let Some(sender) = self.finish_sender.take() {
            if timeout(CONFIG_TIMEOUT, sender.send(expected_samples))
                .await
                .is_err()
            {
                self.stop.cancel();
            }
        }

        let Some(mut task) = self.result_task.take() else {
            return Err(protocol_failure("stream_task_missing", 0));
        };
        let outcome = match timeout(FINISH_WAIT_TIMEOUT, &mut task).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(protocol_failure("stream_task_failed", 0)),
            Err(_) => {
                self.stop.cancel();
                if timeout(SHUTDOWN_WAIT_TIMEOUT, &mut task).await.is_err() {
                    task.abort();
                    let _ = task.await;
                }
                Err(timeout_failure("finish_timeout", 0))
            }
        };
        self.cancel_on_drop = false;
        outcome
    }
}

fn new_session_shell() -> (
    SonioxStreamSession,
    SonioxAudioSender,
    mpsc::Receiver<AudioFrame>,
    mpsc::Receiver<u64>,
) {
    let (queue_sender, queue_receiver) = mpsc::channel(QUEUE_CAPACITY);
    let (finish_sender, finish_receiver) = mpsc::channel(1);
    let state = Arc::new(Mutex::new(ProducerState::default()));
    let stop = CancellationToken::new();
    let failure_notify = Arc::new(Notify::new());
    let sender = SonioxAudioSender {
        queue: queue_sender,
        state: state.clone(),
        stop: stop.clone(),
        failure_notify,
    };
    let session = SonioxStreamSession {
        finish_sender: Some(finish_sender),
        result_task: None,
        state,
        stop,
        cancel_on_drop: true,
    };
    (session, sender, queue_receiver, finish_receiver)
}

async fn open_socket_and_config(
    options: SonioxStreamOptions,
    cancellation: CancellationToken,
) -> Result<SonioxSocket, SonioxStreamFailure> {
    if options.api_key.trim().is_empty() {
        return Err(local_failure(
            AsrError::Unauthorized("Soniox API key is not configured".to_owned()),
            false,
            "missing_credentials",
            0,
        ));
    }
    if cancellation.is_cancelled() {
        return Err(cancelled_failure(0));
    }

    // The URL is fixed. `tokio-tungstenite` does not follow HTTP redirects,
    // so a redirect response is rejected rather than sent to another host.
    let websocket_config = WebSocketConfig::default()
        .max_frame_size(Some(MAX_FRAME_BYTES))
        .max_message_size(Some(MAX_FRAME_BYTES));
    let policy_cancellation = crate::network_policy::cloud_request_token();
    let connected = tokio::select! {
        biased;
        _ = policy_cancellation.cancelled() => return Err(cancelled_failure(0)),
        _ = cancellation.cancelled() => return Err(cancelled_failure(0)),
        result = timeout(
            CONNECT_TIMEOUT,
            connect_async_with_config(SONIOX_WEBSOCKET_ENDPOINT, Some(websocket_config), false),
        ) => result,
    };
    let (mut socket, _) = match connected {
        Err(_) => return Err(timeout_failure("connect_timeout", 0)),
        Ok(Err(error)) => return Err(websocket_connect_failure(error)),
        Ok(Ok(connection)) => connection,
    };

    let mut config = json!({
        "api_key": options.api_key,
        "model": SONIOX_MODEL,
        "audio_format": "pcm_s16le",
        "sample_rate": SAMPLE_RATE,
        "num_channels": CHANNEL_COUNT,
    });
    if let Some(hints) = language_hints(options.language.as_deref()) {
        config["language_hints"] = json!(hints);
    }
    let config_result = tokio::select! {
        biased;
        _ = policy_cancellation.cancelled() => return Err(cancelled_failure(0)),
        _ = cancellation.cancelled() => return Err(cancelled_failure(0)),
        result = timeout(CONFIG_TIMEOUT, socket.send(Message::Text(config.to_string().into()))) => result,
    };
    match config_result {
        Err(_) => Err(timeout_failure("config_timeout", 0)),
        Ok(Err(error)) => Err(websocket_transport_failure(error, 0)),
        Ok(Ok(())) => Ok(socket),
    }
}

impl Drop for SonioxStreamSession {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            self.stop.cancel();
        }
    }
}

/// Transcribes a complete captured WAV through one Soniox WebSocket session.
///
/// This function performs one provider attempt only. Callers may invoke it at
/// most once after a replayable live failure, with `is_replay` set to true; it
/// never retries, concatenates prefixes, changes providers, or changes the
/// provider endpoint. A replay is paced at approximately the audio's duration
/// to preserve the streaming API's intended input cadence.
pub async fn transcribe_complete_wav(
    wav: Vec<u8>,
    options: SonioxStreamOptions,
    cancellation: CancellationToken,
    diagnostics: Option<AsrRequestDiagnostics>,
    is_replay: bool,
) -> Result<Transcript, SonioxStreamFailure> {
    let samples = decode_captured_wav(&wav)?;
    if samples.is_empty() {
        return Err(local_failure(
            AsrError::EmptyResult,
            false,
            "empty_audio",
            0,
        ));
    }
    let sample_count = u64::try_from(samples.len()).map_err(|_| {
        local_failure(
            AsrError::Other("Captured Soniox audio exceeded the supported duration".to_owned()),
            false,
            "duration_limit",
            0,
        )
    })?;
    if sample_count > MAX_STREAM_SAMPLES {
        return Err(local_failure(
            AsrError::Other("Captured Soniox audio exceeded the supported duration".to_owned()),
            false,
            "duration_limit",
            0,
        ));
    }

    let (session, sender) =
        SonioxStreamSession::connect_inner(options, cancellation.clone(), diagnostics, is_replay)
            .await?;
    for (frame_index, frame) in samples.chunks(REPLAY_FRAME_SAMPLES).enumerate() {
        if cancellation.is_cancelled() {
            break;
        }
        let start_sample = frame_index as u64 * REPLAY_FRAME_SAMPLES as u64;
        if sender
            .send_samples_bounded(start_sample, frame, &cancellation)
            .await
            .is_err()
        {
            break;
        }
        if (frame_index + 1) * REPLAY_FRAME_SAMPLES < samples.len() {
            tokio::select! {
                _ = cancellation.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
    }
    session.finish(sample_count).await
}

#[allow(clippy::too_many_arguments)]
async fn run_stream_actor(
    socket: SonioxSocket,
    queue: mpsc::Receiver<AudioFrame>,
    finish: mpsc::Receiver<u64>,
    state: Arc<Mutex<ProducerState>>,
    stop: CancellationToken,
    failure_notify: Arc<Notify>,
    cancellation: CancellationToken,
    attempt_started: Instant,
    diagnostics_guard: Option<AsrStreamAttemptGuard>,
    is_replay: bool,
) -> Result<Transcript, SonioxStreamFailure> {
    let outcome = run_stream_actor_inner(
        socket,
        queue,
        finish,
        state.clone(),
        stop,
        failure_notify,
        cancellation,
    )
    .await;
    {
        let mut producer = lock_state(&state);
        producer.closed = true;
    }

    let result = match outcome {
        Ok(success) => {
            complete_guard(
                diagnostics_guard,
                None,
                attempt_started.elapsed(),
                samples_to_seconds(success.sent_samples),
                if is_replay {
                    samples_to_seconds(success.sent_samples)
                } else {
                    0.0
                },
            );
            return Ok(success.transcript);
        }
        Err(failure) => failure,
    };
    complete_guard(
        diagnostics_guard,
        Some(result.failure.failure_code),
        attempt_started.elapsed(),
        samples_to_seconds(result.sent_samples),
        if is_replay {
            samples_to_seconds(result.sent_samples)
        } else {
            0.0
        },
    );
    Err(result
        .failure
        .with_audio_seconds(samples_to_seconds(result.sent_samples)))
}

struct ActorSuccess {
    transcript: Transcript,
    sent_samples: u64,
}

struct ActorFailure {
    failure: SonioxStreamFailure,
    sent_samples: u64,
}

impl SonioxStreamFailure {
    fn with_audio_seconds(mut self, audio_seconds_sent: f64) -> Self {
        self.audio_seconds_sent = audio_seconds_sent;
        self
    }
}

async fn run_stream_actor_inner(
    socket: SonioxSocket,
    mut queue: mpsc::Receiver<AudioFrame>,
    mut finish: mpsc::Receiver<u64>,
    state: Arc<Mutex<ProducerState>>,
    stop: CancellationToken,
    failure_notify: Arc<Notify>,
    cancellation: CancellationToken,
) -> Result<ActorSuccess, ActorFailure> {
    let (mut sink, mut stream) = socket.split();
    let started = TokioInstant::now();
    let maximum_deadline = started + Duration::from_secs(300 * 60);
    let mut keepalive = interval_at(started + KEEPALIVE_INTERVAL, KEEPALIVE_INTERVAL);
    keepalive.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_outbound = started;
    let mut final_deadline: Option<TokioInstant> = None;
    let mut expected_samples: Option<u64> = None;
    let mut finalize_sent = false;
    let mut sent_samples = 0u64;
    let mut next_audio_send_at: Option<TokioInstant> = None;
    let mut pending_audio: Option<AudioFrame> = None;
    let mut transcript_builder = TranscriptBuilder::default();

    loop {
        let stream_deadline = final_deadline.unwrap_or(maximum_deadline);
        tokio::select! {
            biased;
            _ = stop.cancelled() => {
                let failure = lock_state(&state)
                    .failure
                    .as_ref()
                    .map(|failure| producer_failure(failure, sent_samples))
                    .unwrap_or_else(|| cancelled_failure(sent_samples));
                return Err(ActorFailure { failure, sent_samples });
            }
            _ = cancellation.cancelled() => {
                let failure = cancelled_failure(sent_samples);
                return Err(ActorFailure { failure, sent_samples });
            }
            _ = failure_notify.notified() => {
                if let Some(failure) = lock_state(&state).failure.as_ref() {
                    return Err(ActorFailure {
                        failure: producer_failure(failure, sent_samples),
                        sent_samples,
                    });
                }
            }
            _ = tokio::time::sleep_until(stream_deadline) => {
                let failure = if finalize_sent {
                    timeout_failure("finalize_timeout", sent_samples)
                } else {
                    timeout_failure("stream_timeout", sent_samples)
                };
                return Err(ActorFailure { failure, sent_samples });
            }
            requested = finish.recv(), if expected_samples.is_none() => {
                let Some(expected) = requested else {
                    let failure = cancelled_failure(sent_samples);
                    return Err(ActorFailure { failure, sent_samples });
                };
                expected_samples = Some(expected);
                let accepted = lock_state(&state).next_sample;
                if expected == 0 {
                    return Err(ActorFailure {
                        failure: local_failure(AsrError::EmptyResult, false, "empty_audio", sent_samples),
                        sent_samples,
                    });
                }
                if expected > MAX_STREAM_SAMPLES {
                    return Err(ActorFailure {
                        failure: local_failure(
                            AsrError::Other("Soniox stream exceeded the supported duration".to_owned()),
                            false,
                            "duration_limit",
                            sent_samples,
                        ),
                        sent_samples,
                    });
                }
                if accepted != expected {
                    let failure = coverage_failure(sent_samples);
                    return Err(ActorFailure { failure, sent_samples });
                }
                queue.close();
            }
            _ = tokio::time::sleep_until(
                next_audio_send_at.unwrap_or_else(TokioInstant::now),
            ), if pending_audio.is_some() => {
                let Some(frame) = pending_audio.take() else {
                    continue;
                };
                if frame.start_sample != sent_samples {
                    let failure = coverage_failure(sent_samples);
                    return Err(ActorFailure { failure, sent_samples });
                }
                let next_sent = sent_samples.checked_add(frame.sample_count);
                if next_sent.is_none_or(|next| next > MAX_STREAM_SAMPLES) {
                    let failure = local_failure(
                        AsrError::Other("Soniox stream exceeded the supported duration".to_owned()),
                        false,
                        "duration_limit",
                        sent_samples,
                    );
                    return Err(ActorFailure { failure, sent_samples });
                }
                let message = Message::Binary(frame.bytes.to_vec().into());
                match send_message(&mut sink, message, &stop, &cancellation).await {
                    Ok(()) => {
                        sent_samples = next_sent.unwrap_or(sent_samples);
                        last_outbound = TokioInstant::now();
                        let frame_duration = Duration::from_secs_f64(
                            frame.sample_count as f64 / SAMPLE_RATE as f64,
                        );
                        next_audio_send_at = Some(TokioInstant::now() + frame_duration);
                    }
                    Err(SendFailure::Stopped) => {
                        let failure = lock_state(&state)
                            .failure
                            .as_ref()
                            .map(|failure| producer_failure(failure, sent_samples))
                            .unwrap_or_else(|| cancelled_failure(sent_samples));
                        return Err(ActorFailure { failure, sent_samples });
                    }
                    Err(SendFailure::Timeout) => {
                        let failure = timeout_failure("send_timeout", sent_samples);
                        return Err(ActorFailure { failure, sent_samples });
                    }
                    Err(SendFailure::WebSocket(error)) => {
                        let failure = websocket_transport_failure(*error, sent_samples);
                        return Err(ActorFailure { failure, sent_samples });
                    }
                }
            }
            frame = queue.recv(), if !finalize_sent && pending_audio.is_none() => {
                match frame {
                    Some(frame) => {
                        pending_audio = Some(frame);
                    }
                    None if expected_samples.is_some() => {
                        if sent_samples != expected_samples.unwrap_or_default() {
                            let failure = coverage_failure(sent_samples);
                            return Err(ActorFailure { failure, sent_samples });
                        }
                        match send_message(
                            &mut sink,
                            Message::Binary(Vec::<u8>::new().into()),
                            &stop,
                            &cancellation,
                        )
                        .await
                        {
                            Ok(()) => {
                                finalize_sent = true;
                                final_deadline = Some(TokioInstant::now() + FINALIZE_TIMEOUT);
                                last_outbound = TokioInstant::now();
                            }
                            Err(SendFailure::Stopped) => {
                                let failure = lock_state(&state)
                                    .failure
                                    .as_ref()
                                    .map(|failure| producer_failure(failure, sent_samples))
                                    .unwrap_or_else(|| cancelled_failure(sent_samples));
                                return Err(ActorFailure { failure, sent_samples });
                            }
                            Err(SendFailure::Timeout) => {
                                let failure = timeout_failure("finalize_send_timeout", sent_samples);
                                return Err(ActorFailure { failure, sent_samples });
                            }
                            Err(SendFailure::WebSocket(error)) => {
                                let failure = websocket_transport_failure(*error, sent_samples);
                                return Err(ActorFailure { failure, sent_samples });
                            }
                        }
                    }
                    None => {
                        let failure = coverage_failure(sent_samples);
                        return Err(ActorFailure { failure, sent_samples });
                    }
                }
            }
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let response = match serde_json::from_str::<ServerResponse>(text.as_ref()) {
                            Ok(response) => response,
                            Err(_) => {
                                let failure = protocol_failure("invalid_server_message", sent_samples);
                                return Err(ActorFailure { failure, sent_samples });
                            }
                        };
                        if response.error_code.is_some() || response.error_type.is_some() {
                            let failure = provider_response_failure(
                                response.error_code,
                                response.error_type.as_deref(),
                                sent_samples,
                            );
                            return Err(ActorFailure { failure, sent_samples });
                        }
                        if let Err(failure) = transcript_builder.append_final_tokens(response.tokens) {
                            return Err(ActorFailure { failure: failure.with_audio_seconds(samples_to_seconds(sent_samples)), sent_samples });
                        }
                        if response.finished == Some(true) {
                            if !finalize_sent {
                                let failure = protocol_failure("premature_finished", sent_samples);
                                return Err(ActorFailure { failure, sent_samples });
                            }
                            return Ok(ActorSuccess {
                                transcript: transcript_builder.finish(),
                                sent_samples,
                            });
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        match send_message(
                            &mut sink,
                            Message::Pong(payload),
                            &stop,
                            &cancellation,
                        )
                        .await
                        {
                            Ok(()) => last_outbound = TokioInstant::now(),
                            Err(SendFailure::Stopped) => {
                                let failure = cancelled_failure(sent_samples);
                                return Err(ActorFailure { failure, sent_samples });
                            }
                            Err(SendFailure::Timeout) => {
                                let failure = timeout_failure("send_timeout", sent_samples);
                                return Err(ActorFailure { failure, sent_samples });
                            }
                            Err(SendFailure::WebSocket(error)) => {
                                let failure = websocket_transport_failure(*error, sent_samples);
                                return Err(ActorFailure { failure, sent_samples });
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        let failure = transport_failure("closed_before_finished", sent_samples);
                        return Err(ActorFailure { failure, sent_samples });
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Binary(_))) | Some(Ok(Message::Frame(_))) => {
                        let failure = protocol_failure("unexpected_server_frame", sent_samples);
                        return Err(ActorFailure { failure, sent_samples });
                    }
                    Some(Err(error)) => {
                        let failure = websocket_transport_failure(error, sent_samples);
                        return Err(ActorFailure { failure, sent_samples });
                    }
                }
            }
            _ = keepalive.tick() => {
                if TokioInstant::now().duration_since(last_outbound) >= KEEPALIVE_INTERVAL {
                    match send_message(
                        &mut sink,
                        Message::Text(json!({"type": "keepalive"}).to_string().into()),
                        &stop,
                        &cancellation,
                    )
                    .await
                    {
                        Ok(()) => last_outbound = TokioInstant::now(),
                        Err(SendFailure::Stopped) => {
                            let failure = lock_state(&state)
                                .failure
                                .as_ref()
                                .map(|failure| producer_failure(failure, sent_samples))
                                .unwrap_or_else(|| cancelled_failure(sent_samples));
                            return Err(ActorFailure { failure, sent_samples });
                        }
                        Err(SendFailure::Timeout) => {
                            let failure = timeout_failure("keepalive_timeout", sent_samples);
                            return Err(ActorFailure { failure, sent_samples });
                        }
                        Err(SendFailure::WebSocket(error)) => {
                            let failure = websocket_transport_failure(*error, sent_samples);
                            return Err(ActorFailure { failure, sent_samples });
                        }
                    }
                }
            }
        }
    }
}

enum SendFailure {
    Stopped,
    Timeout,
    WebSocket(Box<WebSocketError>),
}

async fn send_message<S>(
    sink: &mut S,
    message: Message,
    stop: &CancellationToken,
    cancellation: &CancellationToken,
) -> Result<(), SendFailure>
where
    S: futures_util::Sink<Message, Error = WebSocketError> + Unpin,
{
    tokio::select! {
        biased;
        _ = stop.cancelled() => Err(SendFailure::Stopped),
        _ = cancellation.cancelled() => Err(SendFailure::Stopped),
        result = timeout(SEND_TIMEOUT, sink.send(message)) => match result {
            Err(_) => Err(SendFailure::Timeout),
            Ok(Err(error)) => Err(SendFailure::WebSocket(Box::new(error))),
            Ok(Ok(())) => Ok(()),
        }
    }
}

#[derive(Deserialize)]
struct ServerResponse {
    #[serde(default)]
    tokens: Vec<ServerToken>,
    #[serde(default)]
    finished: Option<bool>,
    #[serde(default)]
    error_code: Option<u32>,
    #[serde(default)]
    error_type: Option<String>,
}

#[derive(Deserialize)]
struct ServerToken {
    #[serde(default)]
    text: String,
    #[serde(default)]
    is_final: bool,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    start_ms: Option<f64>,
    #[serde(default)]
    end_ms: Option<f64>,
}

#[derive(Default)]
struct TranscriptBuilder {
    text: String,
    tokens: Vec<AsrToken>,
    language: Option<String>,
    mixed_languages: bool,
}

impl TranscriptBuilder {
    fn append_final_tokens(
        &mut self,
        incoming: Vec<ServerToken>,
    ) -> Result<(), SonioxStreamFailure> {
        for token in incoming.into_iter().filter(|token| token.is_final) {
            if matches!(token.text.as_str(), "<end>" | "<fin>") {
                continue;
            }
            if self.tokens.len() >= MAX_FINAL_TOKENS
                || self
                    .text
                    .chars()
                    .count()
                    .saturating_add(token.text.chars().count())
                    > MAX_TRANSCRIPT_CHARS
            {
                return Err(local_failure(
                    AsrError::Other(
                        "Soniox transcript exceeded the supported response size".to_owned(),
                    ),
                    false,
                    "response_limit",
                    0,
                ));
            }
            if let Some(language) = token.language.as_deref().and_then(safe_language_code) {
                match self.language.as_deref() {
                    None if !self.mixed_languages => self.language = Some(language.to_owned()),
                    Some(existing) if existing != language => {
                        self.language = None;
                        self.mixed_languages = true;
                    }
                    _ => {}
                }
            }
            self.text.push_str(&token.text);
            self.tokens.push(AsrToken {
                text: token.text,
                language: token
                    .language
                    .and_then(|value| safe_language_code(&value).map(str::to_owned)),
                confidence: token
                    .confidence
                    .filter(|value| value.is_finite() && (0.0..=1.0).contains(value)),
                start: millis_to_seconds(token.start_ms),
                end: millis_to_seconds(token.end_ms),
            });
        }
        Ok(())
    }

    fn finish(self) -> Transcript {
        Transcript {
            text: self.text.clone(),
            asr_text: Some(self.text),
            provider_cleaned_candidate: None,
            language: self.language,
            confidence: None,
            segments: Vec::new(),
            words: Vec::new(),
            tokens: self.tokens,
            limits: RateLimits::default(),
        }
    }
}

fn language_hints(language: Option<&str>) -> Option<Vec<&'static str>> {
    match language
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None | Some("") | Some("auto") => Some(vec!["zh", "en"]),
        Some("zh") => Some(vec!["zh"]),
        Some("en") => Some(vec!["en"]),
        _ => None,
    }
}

fn safe_language_code(language: &str) -> Option<&str> {
    let trimmed = language.trim();
    (!trimmed.is_empty()
        && trimmed.len() <= 16
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
    .then_some(trimmed)
}

fn millis_to_seconds(milliseconds: Option<f64>) -> Option<f32> {
    milliseconds
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|value| (value / 1_000.0) as f32)
        .filter(|value| value.is_finite())
}

fn sample_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

fn decode_captured_wav(wav: &[u8]) -> Result<Vec<f32>, SonioxStreamFailure> {
    let mut reader = WavReader::new(Cursor::new(wav)).map_err(|_| {
        local_failure(
            AsrError::Other("Captured Soniox audio was not a valid WAV".to_owned()),
            false,
            "invalid_wav",
            0,
        )
    })?;
    let spec = reader.spec();
    if spec.channels != CHANNEL_COUNT
        || u64::from(spec.sample_rate) != SAMPLE_RATE
        || spec.sample_format != SampleFormat::Int
        || spec.bits_per_sample != 16
    {
        return Err(local_failure(
            AsrError::Other("Captured Soniox audio must be mono 16 kHz PCM16".to_owned()),
            false,
            "unsupported_wav_format",
            0,
        ));
    }
    let samples = reader
        .samples::<i16>()
        .map(|sample| {
            sample
                // `audio::encode` quantizes with `i16::MAX`; dividing by that
                // same scale preserves every sample when a recovery WAV is
                // sent through this adapter again.
                .map(|sample| f32::from(sample) / i16::MAX as f32)
                .map_err(|_| {
                    local_failure(
                        AsrError::Other("Captured Soniox WAV data could not be decoded".to_owned()),
                        false,
                        "invalid_wav_audio",
                        0,
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(samples)
}

fn producer_failure(failure: &ProducerFailure, sent_samples: u64) -> SonioxStreamFailure {
    match failure {
        ProducerFailure::SequenceMismatch { .. } => coverage_failure(sent_samples),
        ProducerFailure::QueueFull => local_failure(
            AsrError::Network(
                "Soniox audio queue could not preserve complete sample coverage".to_owned(),
            ),
            true,
            "queue_overflow",
            sent_samples,
        ),
        ProducerFailure::Closed => transport_failure("stream_closed", sent_samples),
        ProducerFailure::InvalidAudio => local_failure(
            AsrError::Other("Soniox received invalid local audio samples".to_owned()),
            false,
            "invalid_audio",
            sent_samples,
        ),
        ProducerFailure::SampleIndexOverflow => local_failure(
            AsrError::Other("Soniox stream exceeded the supported duration".to_owned()),
            false,
            "duration_limit",
            sent_samples,
        ),
    }
}

fn websocket_connect_failure(error: WebSocketError) -> SonioxStreamFailure {
    if let WebSocketError::Http(response) = error {
        let status = response.status().as_u16();
        return provider_response_failure(Some(u32::from(status)), None, 0);
    }
    transport_failure("connect_failed", 0)
}

fn websocket_transport_failure(error: WebSocketError, sent_samples: u64) -> SonioxStreamFailure {
    if let WebSocketError::Http(response) = error {
        let status = response.status().as_u16();
        return provider_response_failure(Some(u32::from(status)), None, sent_samples);
    }
    transport_failure("transport_error", sent_samples)
}

fn provider_response_failure(
    code: Option<u32>,
    error_type: Option<&str>,
    sent_samples: u64,
) -> SonioxStreamFailure {
    let inferred_code = code.or_else(|| match error_type.map(str::trim) {
        Some("unauthenticated") => Some(401),
        Some("organization_balance_exhausted")
        | Some("organization_monthly_budget_exhausted")
        | Some("project_monthly_budget_exhausted") => Some(402),
        Some("request_timeout") => Some(408),
        Some("rate_limited") | Some("too_many_requests") => Some(429),
        Some("internal_error") | Some("server_error") => Some(500),
        Some("invalid_request") | Some("model_not_available") => Some(400),
        _ => None,
    });
    match inferred_code {
        Some(401 | 403) => local_failure(
            AsrError::Unauthorized("Soniox rejected the configured API key".to_owned()),
            false,
            "provider_auth",
            sent_samples,
        ),
        Some(402) => local_failure(
            AsrError::Other("Soniox account billing limits rejected the stream".to_owned()),
            false,
            "provider_billing",
            sent_samples,
        ),
        Some(408) => timeout_failure("provider_timeout", sent_samples),
        Some(429) => local_failure(
            AsrError::RateLimited(" by Soniox".to_owned()),
            true,
            "provider_rate_limited",
            sent_samples,
        ),
        Some(500..=599) => local_failure(
            AsrError::RetryableServer {
                message: "Soniox streaming service returned a server error".to_owned(),
                retry_after: None,
            },
            true,
            "provider_server",
            sent_samples,
        ),
        Some(400..=499) => local_failure(
            AsrError::Other(format!(
                "Soniox rejected the stream request (HTTP {}).",
                inferred_code.unwrap_or_default()
            )),
            false,
            "provider_request",
            sent_samples,
        ),
        Some(300..=399) => local_failure(
            AsrError::Other("Soniox WebSocket endpoint redirect was rejected".to_owned()),
            false,
            "redirect_rejected",
            sent_samples,
        ),
        _ => protocol_failure("provider_error_response", sent_samples),
    }
}

fn complete_guard(
    guard: Option<AsrStreamAttemptGuard>,
    failure: Option<&'static str>,
    connection_duration: Duration,
    audio_seconds: f64,
    reprocessed_audio_seconds: f64,
) {
    if let Some(guard) = guard {
        guard.complete(
            failure,
            connection_duration,
            audio_seconds,
            reprocessed_audio_seconds,
        );
    }
}

fn local_failure(
    error: AsrError,
    replayable: bool,
    failure_code: &'static str,
    sent_samples: u64,
) -> SonioxStreamFailure {
    SonioxStreamFailure {
        error,
        replayable,
        failure_code,
        audio_seconds_sent: samples_to_seconds(sent_samples),
    }
}

fn cancelled_failure(sent_samples: u64) -> SonioxStreamFailure {
    local_failure(
        AsrError::Other("Soniox streaming transcription was cancelled".to_owned()),
        false,
        "cancelled",
        sent_samples,
    )
}

fn timeout_failure(code: &'static str, sent_samples: u64) -> SonioxStreamFailure {
    local_failure(AsrError::Timeout, true, code, sent_samples)
}

fn transport_failure(code: &'static str, sent_samples: u64) -> SonioxStreamFailure {
    local_failure(
        AsrError::Network("Soniox WebSocket stream ended before finalization".to_owned()),
        true,
        code,
        sent_samples,
    )
}

fn coverage_failure(sent_samples: u64) -> SonioxStreamFailure {
    local_failure(
        AsrError::Network("Soniox stream did not preserve complete audio coverage".to_owned()),
        true,
        "coverage_failure",
        sent_samples,
    )
}

fn protocol_failure(code: &'static str, sent_samples: u64) -> SonioxStreamFailure {
    local_failure(
        AsrError::Network("Soniox returned an incomplete or invalid stream response".to_owned()),
        true,
        code,
        sent_samples,
    )
}

fn samples_to_seconds(samples: u64) -> f64 {
    samples as f64 / SAMPLE_RATE as f64
}

fn lock_state(state: &Mutex<ProducerState>) -> MutexGuard<'_, ProducerState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
