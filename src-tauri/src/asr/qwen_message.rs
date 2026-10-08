//! Completed-audio adapter for the raw-capable Qwen Audio 3.1 Message API.
//!
//! This adapter deliberately returns only sentence-final ASR text. It does not
//! expose interim output or the provider's optional polishing path.

use crate::asr::{AsrError, RateLimits, Segment, Transcript, Word};
use crate::metrics::{AsrRequestDiagnostics, AsrUsageUnit};
use chacha20poly1305::aead::{rand_core::RngCore, OsRng};
use futures_util::{stream::SplitStream, SinkExt, StreamExt};
use hound::{SampleFormat, WavReader};
use reqwest::Url;
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
use std::io::Cursor;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::time::Instant as TokioInstant;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as WebSocketError, Message};
use tokio_tungstenite::{connect_async_with_config, MaybeTlsStream, WebSocketStream};
use tokio_util::sync::CancellationToken;

pub const QWEN_MESSAGE_MODEL: &str = "qwen-audio-3.1-asr-flash-message";

const INFERENCE_PATH: &str = "/api-ws/v1/inference";
const BEIJING_HOST: &str = "dashscope.aliyuncs.com";
const SINGAPORE_HOST: &str = "dashscope-intl.aliyuncs.com";
const SAMPLE_RATE: u32 = 16_000;
const CHANNELS: u16 = 1;
const MAX_AUDIO_BLOCK_BYTES: usize = 12_800;
const MAX_AUDIO_DURATION_SECS: u64 = 20;
const MAX_AUDIO_BYTES: usize = 1024 * 1024;
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
const MAX_TRANSCRIPT_BYTES: usize = 1024 * 1024;
const MAX_FINAL_SENTENCES: usize = 10_000;
const MAX_FINAL_WORDS: usize = 100_000;
const MAX_VOCABULARY_TERMS: usize = 100;
const MAX_VOCABULARY_CHARS: usize = 8_000;
const MAX_VOCABULARY_TERM_CHARS: usize = 256;
const MAX_AUTH_KEY_BYTES: usize = 4_096;
const MAX_PENDING_PONGS: usize = 8;
const OUTPUT_TOKEN_BUDGET: f64 = 1_024.0;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const TASK_START_TIMEOUT: Duration = Duration::from_secs(15);
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
const FINAL_TIMEOUT: Duration = Duration::from_secs(90);
const SESSION_TIMEOUT: Duration = Duration::from_secs(180);

type QwenSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Settings for one completed-audio request.
///
/// The endpoint must be an official Beijing or Singapore DashScope WSS URL.
/// The model is pinned by this adapter; callers cannot select a different
/// protocol model or add language hints that this Message API does not expose.
pub struct QwenMessageOptions {
    pub endpoint: String,
    pub api_key: String,
    /// Permission-scoped instant vocabulary. Weights must be in the provider's
    /// documented 1–5 range.
    pub instant_vocabulary: BTreeMap<String, u8>,
}

struct AdapterFailure {
    error: AsrError,
    failure_code: &'static str,
    audio_seconds_sent: f64,
}

impl AdapterFailure {
    fn other(code: &'static str, message: &'static str, audio_seconds_sent: f64) -> Self {
        Self {
            error: AsrError::Other(message.to_owned()),
            failure_code: code,
            audio_seconds_sent,
        }
    }

    fn timeout(code: &'static str, audio_seconds_sent: f64) -> Self {
        Self {
            error: AsrError::Timeout,
            failure_code: code,
            audio_seconds_sent,
        }
    }

    fn network(code: &'static str, audio_seconds_sent: f64) -> Self {
        Self {
            error: AsrError::Network("Qwen Message connection failed".to_owned()),
            failure_code: code,
            audio_seconds_sent,
        }
    }

    fn cancelled(audio_seconds_sent: f64) -> Self {
        Self::other(
            "cancelled",
            "Qwen Message transcription cancelled",
            audio_seconds_sent,
        )
    }
}

struct SessionSuccess {
    transcript: Transcript,
    audio_seconds_sent: f64,
}

struct ValidatedAudio {
    wav: Vec<u8>,
    duration_secs: f64,
}

#[derive(Default)]
struct ReceiveState {
    sentences: BTreeMap<u64, FinalSentence>,
    transcript_bytes: usize,
    word_count: usize,
    word_bytes: usize,
    finished: bool,
}

#[derive(Clone, PartialEq)]
struct FinalSentence {
    text: String,
    start: Option<f32>,
    end: Option<f32>,
    language: Option<String>,
    words: Vec<Word>,
}

struct ServerMessage {
    event: String,
    task_id: String,
    payload: Value,
}

/// Transcribe a complete mono 16 kHz PCM16 WAV through one Message task.
///
/// Only explicit `sentence_end` results are assembled. The active socket stays
/// inside this future, so cancellation drops the socket and aborts the request
/// without returning a partial transcript.
pub async fn transcribe_complete_wav(
    wav: Vec<u8>,
    options: QwenMessageOptions,
    cancellation: CancellationToken,
    diagnostics: Option<AsrRequestDiagnostics>,
) -> Result<Transcript, AsrError> {
    let started = Instant::now();
    let diagnostics_guard = diagnostics
        .as_ref()
        .map(AsrRequestDiagnostics::begin_stream);
    let result = transcribe_inner(wav, options, cancellation, diagnostics.as_ref()).await;

    let (failure, audio_seconds_sent) = match &result {
        Ok(success) => (None, success.audio_seconds_sent),
        Err(failure) => (Some(failure.failure_code), failure.audio_seconds_sent),
    };
    if let Some(guard) = diagnostics_guard {
        guard.complete(failure, started.elapsed(), audio_seconds_sent, 0.0);
    }

    result
        .map(|success| success.transcript)
        .map_err(|failure| failure.error)
}

async fn transcribe_inner(
    wav: Vec<u8>,
    options: QwenMessageOptions,
    cancellation: CancellationToken,
    diagnostics: Option<&AsrRequestDiagnostics>,
) -> Result<SessionSuccess, AdapterFailure> {
    let policy_cancellation = crate::network_policy::cloud_request_token();
    if cancellation.is_cancelled() || policy_cancellation.is_cancelled() {
        return Err(AdapterFailure::cancelled(0.0));
    }

    let endpoint = validate_endpoint(&options.endpoint)?;
    validate_api_key(&options.api_key)?;
    let vocabulary = validate_vocabulary(options.instant_vocabulary)?;
    let audio = validate_wav(wav)?;
    let task_id = new_task_id()?;

    let request = endpoint.as_str().into_client_request().map_err(|_| {
        AdapterFailure::other("invalid_endpoint", "Invalid Qwen Message endpoint", 0.0)
    })?;
    let mut request = request;
    let authorization = HeaderValue::from_str(&format!("Bearer {}", options.api_key.trim()))
        .map_err(|_| {
            AdapterFailure::other(
                "invalid_credentials",
                "Invalid Qwen Message credentials",
                0.0,
            )
        })?;
    let _ = request.headers_mut().insert(AUTHORIZATION, authorization);

    let websocket_config = WebSocketConfig::default()
        .max_frame_size(Some(MAX_MESSAGE_BYTES))
        .max_message_size(Some(MAX_MESSAGE_BYTES));
    // tokio-tungstenite rejects redirect handshakes instead of following them.
    // The fixed host/path allowlist above prevents credentials from escaping.
    let connected = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(AdapterFailure::cancelled(0.0)),
        _ = policy_cancellation.cancelled() => return Err(AdapterFailure::cancelled(0.0)),
        result = tokio::time::timeout(
            CONNECT_TIMEOUT,
            connect_async_with_config(request, Some(websocket_config), false),
        ) => result,
    };
    let (mut socket, _) = match connected {
        Err(_) => return Err(AdapterFailure::timeout("connect_timeout", 0.0)),
        Ok(Err(error)) => return Err(handshake_failure(error)),
        Ok(Ok(connection)) => connection,
    };

    let session_deadline = TokioInstant::now() + SESSION_TIMEOUT;
    let start_message = run_task_message(&task_id, &vocabulary);
    send_control_message(
        &mut socket,
        Message::Text(start_message.to_string().into()),
        &cancellation,
        &policy_cancellation,
        session_deadline,
        0.0,
        "start_send_timeout",
    )
    .await?;
    wait_for_task_started(
        &mut socket,
        &task_id,
        &cancellation,
        &policy_cancellation,
        session_deadline,
    )
    .await?;

    let (mut sink, mut stream) = socket.split();
    let mut received = ReceiveState::default();
    let mut sent_bytes = 0usize;
    let mut pending_pongs: VecDeque<Vec<u8>> = VecDeque::new();

    for block in audio.wav.chunks(MAX_AUDIO_BLOCK_BYTES) {
        send_message_and_drain(
            &mut sink,
            &mut stream,
            Message::Binary(block.to_vec().into()),
            &task_id,
            false,
            &mut received,
            &mut pending_pongs,
            &cancellation,
            &policy_cancellation,
            session_deadline,
            0.0,
            diagnostics,
        )
        .await?;
        sent_bytes = sent_bytes.saturating_add(block.len());
        let uploaded_audio_seconds = if sent_bytes == audio.wav.len() {
            audio.duration_secs
        } else {
            0.0
        };
        flush_pending_pongs(
            &mut sink,
            &mut pending_pongs,
            &cancellation,
            &policy_cancellation,
            session_deadline,
            uploaded_audio_seconds,
        )
        .await?;
        // Match the official SDK's cooperative yield without audio-duration pacing.
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(AdapterFailure::cancelled(uploaded_audio_seconds)),
            _ = policy_cancellation.cancelled() => return Err(AdapterFailure::cancelled(uploaded_audio_seconds)),
            _ = tokio::time::sleep(Duration::from_micros(1)) => {}
        }
    }

    let complete_audio_seconds = audio.duration_secs;
    let finish_message = finish_task_message(&task_id);
    send_message_and_drain(
        &mut sink,
        &mut stream,
        Message::Text(finish_message.to_string().into()),
        &task_id,
        true,
        &mut received,
        &mut pending_pongs,
        &cancellation,
        &policy_cancellation,
        session_deadline,
        complete_audio_seconds,
        diagnostics,
    )
    .await?;
    flush_pending_pongs(
        &mut sink,
        &mut pending_pongs,
        &cancellation,
        &policy_cancellation,
        session_deadline,
        complete_audio_seconds,
    )
    .await?;

    let final_deadline = TokioInstant::now() + FINAL_TIMEOUT;
    while !received.finished {
        let incoming = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(AdapterFailure::cancelled(complete_audio_seconds)),
            _ = policy_cancellation.cancelled() => return Err(AdapterFailure::cancelled(complete_audio_seconds)),
            _ = tokio::time::sleep_until(session_deadline) => {
                return Err(AdapterFailure::timeout("session_timeout", complete_audio_seconds));
            }
            _ = tokio::time::sleep_until(final_deadline) => {
                return Err(AdapterFailure::timeout("task_finished_timeout", complete_audio_seconds));
            }
            incoming = stream.next() => incoming,
        };
        let incoming = incoming.ok_or_else(|| {
            AdapterFailure::network("closed_before_task_finished", complete_audio_seconds)
        })?;
        match incoming {
            Ok(Message::Text(text)) => {
                process_active_text(
                    text.as_ref(),
                    &task_id,
                    true,
                    &mut received,
                    diagnostics,
                    complete_audio_seconds,
                )?;
            }
            Ok(Message::Ping(payload)) => {
                send_control_message(
                    &mut sink,
                    Message::Pong(payload),
                    &cancellation,
                    &policy_cancellation,
                    session_deadline,
                    complete_audio_seconds,
                    "pong_send_timeout",
                )
                .await?;
            }
            Ok(Message::Pong(_)) => {}
            Ok(Message::Close(_)) | Err(_) | Ok(Message::Binary(_)) | Ok(Message::Frame(_)) => {
                return Err(AdapterFailure::network(
                    "transport_closed_or_invalid",
                    complete_audio_seconds,
                ));
            }
        }
    }

    if sent_bytes != audio.wav.len() {
        return Err(AdapterFailure::other(
            "audio_coverage_mismatch",
            "Qwen Message audio upload was incomplete",
            0.0,
        ));
    }
    let transcript = build_transcript(received, complete_audio_seconds)?;
    Ok(SessionSuccess {
        transcript,
        audio_seconds_sent: complete_audio_seconds,
    })
}

fn validate_endpoint(endpoint: &str) -> Result<Url, AdapterFailure> {
    if endpoint.trim() != endpoint || endpoint.len() > 512 {
        return Err(AdapterFailure::other(
            "invalid_endpoint",
            "Invalid Qwen Message endpoint",
            0.0,
        ));
    }
    let url = Url::parse(endpoint).map_err(|_| {
        AdapterFailure::other("invalid_endpoint", "Invalid Qwen Message endpoint", 0.0)
    })?;
    let host = url.host_str();
    let allowed_host = host == Some(BEIJING_HOST) || host == Some(SINGAPORE_HOST);
    if url.scheme() != "wss"
        || !allowed_host
        || url.path() != INFERENCE_PATH
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AdapterFailure::other(
            "invalid_endpoint",
            "Qwen Message requires an official regional inference endpoint",
            0.0,
        ));
    }
    Ok(url)
}

fn validate_api_key(api_key: &str) -> Result<(), AdapterFailure> {
    let trimmed = api_key.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_AUTH_KEY_BYTES
        || trimmed.chars().any(char::is_control)
    {
        return Err(AdapterFailure {
            error: AsrError::Unauthorized("Qwen Message API key is not configured".to_owned()),
            failure_code: "invalid_credentials",
            audio_seconds_sent: 0.0,
        });
    }
    Ok(())
}

fn validate_vocabulary(
    input: BTreeMap<String, u8>,
) -> Result<BTreeMap<String, u8>, AdapterFailure> {
    if input.len() > MAX_VOCABULARY_TERMS {
        return Err(AdapterFailure::other(
            "vocabulary_limit",
            "Qwen Message vocabulary exceeds its size limit",
            0.0,
        ));
    }
    let mut result = BTreeMap::new();
    let mut total_chars = 0usize;
    for (raw_term, weight) in input {
        let term = raw_term.trim();
        let term_chars = term.chars().count();
        total_chars = total_chars.saturating_add(term_chars);
        if term.is_empty()
            || term_chars > MAX_VOCABULARY_TERM_CHARS
            || total_chars > MAX_VOCABULARY_CHARS
            || !(1..=5).contains(&weight)
            || term.chars().any(char::is_control)
            || result.insert(term.to_owned(), weight).is_some()
        {
            return Err(AdapterFailure::other(
                "invalid_vocabulary",
                "Qwen Message vocabulary is invalid",
                0.0,
            ));
        }
    }
    Ok(result)
}

fn validate_wav(wav: Vec<u8>) -> Result<ValidatedAudio, AdapterFailure> {
    if wav.is_empty() {
        return Err(AdapterFailure {
            error: AsrError::EmptyResult,
            failure_code: "empty_audio",
            audio_seconds_sent: 0.0,
        });
    }
    if wav.len() > MAX_AUDIO_BYTES {
        return Err(AdapterFailure::other(
            "audio_byte_limit",
            "Qwen Message audio exceeds the supported size",
            0.0,
        ));
    }
    let mut reader = WavReader::new(Cursor::new(wav.as_slice())).map_err(|_| {
        AdapterFailure::other("invalid_audio", "Invalid Qwen Message WAV audio", 0.0)
    })?;
    let spec = reader.spec();
    if spec.sample_rate != SAMPLE_RATE
        || spec.channels != CHANNELS
        || spec.bits_per_sample != 16
        || spec.sample_format != SampleFormat::Int
    {
        return Err(AdapterFailure::other(
            "unsupported_audio_format",
            "Qwen Message requires mono 16 kHz PCM16 WAV audio",
            0.0,
        ));
    }
    let sample_count = u64::from(reader.duration());
    if sample_count == 0 {
        return Err(AdapterFailure {
            error: AsrError::EmptyResult,
            failure_code: "empty_audio",
            audio_seconds_sent: 0.0,
        });
    }
    if sample_count > MAX_AUDIO_DURATION_SECS * u64::from(SAMPLE_RATE) {
        return Err(AdapterFailure::other(
            "audio_duration_limit",
            "Qwen Message audio exceeds the supported duration",
            0.0,
        ));
    }
    for sample in reader.samples::<i16>() {
        if sample.is_err() {
            return Err(AdapterFailure::other(
                "invalid_audio",
                "Invalid Qwen Message WAV audio",
                0.0,
            ));
        }
    }
    Ok(ValidatedAudio {
        wav,
        duration_secs: sample_count as f64 / f64::from(SAMPLE_RATE),
    })
}

fn run_task_message(task_id: &str, vocabulary: &BTreeMap<String, u8>) -> Value {
    json!({
        "header": {
            "action": "run-task",
            "task_id": task_id,
            "streaming": "duplex"
        },
        "payload": {
            "task_group": "audio",
            "task": "asr",
            "function": "recognition",
            "model": QWEN_MESSAGE_MODEL,
            "parameters": {
                "format": "wav",
                "sample_rate": SAMPLE_RATE,
                "heartbeat": true,
                "disfluency_removal_enabled": false,
                "intermediate_result_enabled": false,
                "keep_dialect": true,
                "vocabulary": vocabulary
            },
            "input": {}
        }
    })
}

fn finish_task_message(task_id: &str) -> Value {
    json!({
        "header": {
            "action": "finish-task",
            "task_id": task_id,
            "streaming": "duplex"
        },
        "payload": { "input": {} }
    })
}

async fn wait_for_task_started(
    socket: &mut QwenSocket,
    task_id: &str,
    cancellation: &CancellationToken,
    policy_cancellation: &CancellationToken,
    session_deadline: TokioInstant,
) -> Result<(), AdapterFailure> {
    let task_start_deadline = TokioInstant::now() + TASK_START_TIMEOUT;
    loop {
        let incoming = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(AdapterFailure::cancelled(0.0)),
            _ = policy_cancellation.cancelled() => return Err(AdapterFailure::cancelled(0.0)),
            _ = tokio::time::sleep_until(session_deadline) => {
                return Err(AdapterFailure::timeout("session_timeout", 0.0));
            }
            _ = tokio::time::sleep_until(task_start_deadline) => {
                return Err(AdapterFailure::timeout("task_started_timeout", 0.0));
            }
            incoming = socket.next() => incoming,
        };
        let incoming =
            incoming.ok_or_else(|| AdapterFailure::network("closed_before_task_started", 0.0))?;
        match incoming {
            Ok(Message::Text(text)) => {
                let message = parse_server_message(text.as_ref(), 0.0)?;
                check_task_binding(&message, task_id, 0.0)?;
                match message.event.as_str() {
                    "task-started" => return Ok(()),
                    "task-failed" => return Err(provider_failure(0.0)),
                    _ => {
                        return Err(AdapterFailure::other(
                            "unexpected_prestart_event",
                            "Qwen Message returned an unexpected task event",
                            0.0,
                        ));
                    }
                }
            }
            Ok(Message::Ping(payload)) => {
                send_control_message(
                    socket,
                    Message::Pong(payload),
                    cancellation,
                    policy_cancellation,
                    session_deadline,
                    0.0,
                    "pong_send_timeout",
                )
                .await?;
            }
            Ok(Message::Pong(_)) => {}
            Ok(Message::Close(_)) | Err(_) | Ok(Message::Binary(_)) | Ok(Message::Frame(_)) => {
                return Err(AdapterFailure::network("invalid_task_start_stream", 0.0));
            }
        }
    }
}

async fn send_control_message<S>(
    sink: &mut S,
    message: Message,
    cancellation: &CancellationToken,
    policy_cancellation: &CancellationToken,
    session_deadline: TokioInstant,
    audio_seconds_sent: f64,
    timeout_code: &'static str,
) -> Result<(), AdapterFailure>
where
    S: futures_util::Sink<Message, Error = WebSocketError> + Unpin,
{
    let deadline = (TokioInstant::now() + SEND_TIMEOUT).min(session_deadline);
    let sent = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(AdapterFailure::cancelled(audio_seconds_sent)),
        _ = policy_cancellation.cancelled() => return Err(AdapterFailure::cancelled(audio_seconds_sent)),
        _ = tokio::time::sleep_until(session_deadline) => {
            return Err(AdapterFailure::timeout("session_timeout", audio_seconds_sent));
        }
        _ = tokio::time::sleep_until(deadline) => return Err(AdapterFailure::timeout(timeout_code, audio_seconds_sent)),
        sent = sink.send(message) => sent,
    };
    sent.map_err(|_| AdapterFailure::network("send_failed", audio_seconds_sent))
}

#[allow(clippy::too_many_arguments)]
async fn send_message_and_drain(
    sink: &mut futures_util::stream::SplitSink<QwenSocket, Message>,
    stream: &mut SplitStream<QwenSocket>,
    message: Message,
    task_id: &str,
    allow_task_finished: bool,
    received: &mut ReceiveState,
    pending_pongs: &mut VecDeque<Vec<u8>>,
    cancellation: &CancellationToken,
    policy_cancellation: &CancellationToken,
    session_deadline: TokioInstant,
    audio_seconds_sent: f64,
    diagnostics: Option<&AsrRequestDiagnostics>,
) -> Result<(), AdapterFailure> {
    let send_deadline = (TokioInstant::now() + SEND_TIMEOUT).min(session_deadline);
    let mut send_future = Box::pin(sink.send(message));
    loop {
        let event = tokio::select! {
            _ = cancellation.cancelled() => return Err(AdapterFailure::cancelled(audio_seconds_sent)),
            _ = policy_cancellation.cancelled() => return Err(AdapterFailure::cancelled(audio_seconds_sent)),
            _ = tokio::time::sleep_until(session_deadline) => {
                return Err(AdapterFailure::timeout("session_timeout", audio_seconds_sent));
            }
            _ = tokio::time::sleep_until(send_deadline) => {
                return Err(AdapterFailure::timeout("send_timeout", audio_seconds_sent));
            }
            sent = &mut send_future => {
                sent.map_err(|_| AdapterFailure::network("send_failed", audio_seconds_sent))?;
                break;
            }
            incoming = stream.next() => incoming,
        };
        let incoming = event
            .ok_or_else(|| AdapterFailure::network("closed_during_upload", audio_seconds_sent))?;
        match incoming {
            Ok(Message::Text(text)) => process_active_text(
                text.as_ref(),
                task_id,
                allow_task_finished,
                received,
                diagnostics,
                audio_seconds_sent,
            )?,
            Ok(Message::Ping(payload)) => {
                if pending_pongs.len() == MAX_PENDING_PONGS {
                    return Err(AdapterFailure::other(
                        "provider_ping_limit",
                        "Qwen Message sent too many control frames",
                        audio_seconds_sent,
                    ));
                }
                pending_pongs.push_back(payload.to_vec());
            }
            Ok(Message::Pong(_)) => {}
            Ok(Message::Close(_)) | Err(_) | Ok(Message::Binary(_)) | Ok(Message::Frame(_)) => {
                return Err(AdapterFailure::network(
                    "invalid_provider_frame",
                    audio_seconds_sent,
                ));
            }
        }
    }
    Ok(())
}

async fn flush_pending_pongs(
    sink: &mut futures_util::stream::SplitSink<QwenSocket, Message>,
    pending_pongs: &mut VecDeque<Vec<u8>>,
    cancellation: &CancellationToken,
    policy_cancellation: &CancellationToken,
    session_deadline: TokioInstant,
    audio_seconds_sent: f64,
) -> Result<(), AdapterFailure> {
    while let Some(payload) = pending_pongs.pop_front() {
        let deadline = (TokioInstant::now() + SEND_TIMEOUT).min(session_deadline);
        let sent = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(AdapterFailure::cancelled(audio_seconds_sent)),
            _ = policy_cancellation.cancelled() => return Err(AdapterFailure::cancelled(audio_seconds_sent)),
            _ = tokio::time::sleep_until(session_deadline) => {
                return Err(AdapterFailure::timeout("session_timeout", audio_seconds_sent));
            }
            _ = tokio::time::sleep_until(deadline) => {
                return Err(AdapterFailure::timeout("pong_send_timeout", audio_seconds_sent));
            }
            sent = sink.send(Message::Pong(payload.into())) => sent,
        };
        sent.map_err(|_| AdapterFailure::network("pong_send_failed", audio_seconds_sent))?;
    }
    Ok(())
}

fn process_active_text(
    text: &str,
    task_id: &str,
    allow_task_finished: bool,
    received: &mut ReceiveState,
    diagnostics: Option<&AsrRequestDiagnostics>,
    audio_seconds_sent: f64,
) -> Result<(), AdapterFailure> {
    let message = parse_server_message(text, audio_seconds_sent)?;
    check_task_binding(&message, task_id, audio_seconds_sent)?;
    if received.finished {
        return Err(AdapterFailure::other(
            "event_after_task_finished",
            "Qwen Message sent an event after task completion",
            audio_seconds_sent,
        ));
    }
    match message.event.as_str() {
        "result-generated" => {
            append_result(&message.payload, received, audio_seconds_sent)?;
        }
        "task-finished" => {
            if !allow_task_finished {
                return Err(AdapterFailure::other(
                    "premature_task_finished",
                    "Qwen Message finished before the complete audio was sent",
                    audio_seconds_sent,
                ));
            }
            if message
                .payload
                .get("output")
                .and_then(|output| output.get("sentence"))
                .is_some()
            {
                append_result(&message.payload, received, audio_seconds_sent)?;
            }
            let usage = message.payload.get("usage");
            if let (Some(diagnostics), Some(usage)) = (diagnostics, usage) {
                record_final_usage(diagnostics, usage);
            }
            if usage
                .and_then(|value| usage_number(value, "output_tokens"))
                .is_some_and(|output_tokens| output_tokens >= OUTPUT_TOKEN_BUDGET)
            {
                return Err(AdapterFailure::other(
                    "output_budget_exhausted",
                    "Qwen Message output reached its 1024-token limit",
                    audio_seconds_sent,
                ));
            }
            received.finished = true;
        }
        "task-failed" => return Err(provider_failure(audio_seconds_sent)),
        "task-started" => {
            return Err(AdapterFailure::other(
                "duplicate_task_started",
                "Qwen Message returned an unexpected task event",
                audio_seconds_sent,
            ));
        }
        _ => {
            return Err(AdapterFailure::other(
                "unknown_provider_event",
                "Qwen Message returned an unknown task event",
                audio_seconds_sent,
            ));
        }
    }
    Ok(())
}

fn append_result(
    payload: &Value,
    received: &mut ReceiveState,
    audio_seconds_sent: f64,
) -> Result<(), AdapterFailure> {
    let output = payload.get("output").ok_or_else(|| {
        AdapterFailure::other(
            "invalid_result",
            "Qwen Message returned an invalid result",
            audio_seconds_sent,
        )
    })?;
    let sentence = output.get("sentence").ok_or_else(|| {
        AdapterFailure::other(
            "invalid_result",
            "Qwen Message returned an invalid result",
            audio_seconds_sent,
        )
    })?;
    let sentence_id = sentence.get("sentence_id").and_then(Value::as_u64);
    if sentence.get("heartbeat").and_then(Value::as_bool) == Some(true) || sentence_id == Some(0) {
        return Ok(());
    }
    let sentence_id = sentence_id.ok_or_else(|| {
        AdapterFailure::other(
            "invalid_sentence_id",
            "Qwen Message returned an invalid sentence id",
            audio_seconds_sent,
        )
    })?;
    if sentence_id == 0 || sentence_id > MAX_FINAL_SENTENCES as u64 {
        return Err(AdapterFailure::other(
            "sentence_limit",
            "Qwen Message returned an unsupported sentence id",
            audio_seconds_sent,
        ));
    }
    let is_final = sentence
        .get("sentence_end")
        .or_else(|| output.get("sentence_end"))
        .and_then(Value::as_bool)
        == Some(true);
    if !is_final {
        return Ok(());
    }
    let text = sentence
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            AdapterFailure::other(
                "invalid_final_text",
                "Qwen Message returned an invalid final sentence",
                audio_seconds_sent,
            )
        })?;
    if text.len() > MAX_TRANSCRIPT_BYTES {
        return Err(AdapterFailure::other(
            "transcript_limit",
            "Qwen Message transcript exceeds the supported size",
            audio_seconds_sent,
        ));
    }
    let (words, added_word_count, added_word_bytes) = parse_words(sentence, audio_seconds_sent)?;
    let language = sentence
        .get("language")
        .and_then(Value::as_str)
        .filter(|value| value.len() <= 64 && !value.chars().any(char::is_control))
        .map(str::to_owned);
    let final_sentence = FinalSentence {
        text: text.to_owned(),
        start: milliseconds_to_seconds(sentence.get("begin_time")),
        end: milliseconds_to_seconds(sentence.get("end_time")),
        language,
        words,
    };
    if let Some(existing) = received.sentences.get(&sentence_id) {
        if existing != &final_sentence {
            return Err(AdapterFailure::other(
                "conflicting_final_sentence",
                "Qwen Message returned conflicting final sentences",
                audio_seconds_sent,
            ));
        }
        return Ok(());
    }
    let updated_bytes = received.transcript_bytes.saturating_add(text.len());
    if updated_bytes > MAX_TRANSCRIPT_BYTES {
        return Err(AdapterFailure::other(
            "transcript_limit",
            "Qwen Message transcript exceeds the supported size",
            audio_seconds_sent,
        ));
    }
    if received.sentences.len() >= MAX_FINAL_SENTENCES {
        return Err(AdapterFailure::other(
            "sentence_limit",
            "Qwen Message transcript has too many sentences",
            audio_seconds_sent,
        ));
    }
    if received.word_count.saturating_add(added_word_count) > MAX_FINAL_WORDS
        || received.word_bytes.saturating_add(added_word_bytes) > MAX_TRANSCRIPT_BYTES
    {
        return Err(AdapterFailure::other(
            "word_limit",
            "Qwen Message word metadata exceeds the supported size",
            audio_seconds_sent,
        ));
    }
    received.transcript_bytes = updated_bytes;
    received.word_count = received.word_count.saturating_add(added_word_count);
    received.word_bytes = received.word_bytes.saturating_add(added_word_bytes);
    let _ = received.sentences.insert(sentence_id, final_sentence);
    Ok(())
}

fn parse_words(
    sentence: &Value,
    audio_seconds_sent: f64,
) -> Result<(Vec<Word>, usize, usize), AdapterFailure> {
    let Some(words) = sentence.get("words").and_then(Value::as_array) else {
        return Ok((Vec::new(), 0, 0));
    };
    if words.len() > MAX_FINAL_WORDS {
        return Err(AdapterFailure::other(
            "word_limit",
            "Qwen Message transcript has too many word timings",
            audio_seconds_sent,
        ));
    }
    let mut parsed = Vec::with_capacity(words.len());
    let mut parsed_bytes = 0usize;
    for item in words {
        let Some(word) = item
            .get("text")
            .or_else(|| item.get("word"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        parsed_bytes = parsed_bytes.saturating_add(word.len());
        if parsed_bytes > MAX_TRANSCRIPT_BYTES {
            return Err(AdapterFailure::other(
                "word_text_limit",
                "Qwen Message word metadata exceeds the supported size",
                audio_seconds_sent,
            ));
        }
        parsed.push(Word {
            word: word.to_owned(),
            start: milliseconds_to_seconds(item.get("begin_time")),
            end: milliseconds_to_seconds(item.get("end_time")),
            confidence: None,
        });
    }
    let parsed_count = parsed.len();
    Ok((parsed, parsed_count, parsed_bytes))
}

fn milliseconds_to_seconds(value: Option<&Value>) -> Option<f32> {
    let milliseconds = value?.as_f64()?;
    if !milliseconds.is_finite() || milliseconds < 0.0 {
        return None;
    }
    let seconds = milliseconds / 1_000.0;
    (seconds <= f64::from(f32::MAX)).then_some(seconds as f32)
}

fn build_transcript(
    received: ReceiveState,
    audio_seconds_sent: f64,
) -> Result<Transcript, AdapterFailure> {
    if received.sentences.is_empty() {
        return Err(AdapterFailure {
            error: AsrError::EmptyResult,
            failure_code: "empty_result",
            audio_seconds_sent,
        });
    }
    let mut text = String::with_capacity(received.transcript_bytes);
    let mut segments = Vec::with_capacity(received.sentences.len());
    let mut words = Vec::with_capacity(received.word_count);
    let mut language: Option<String> = None;
    let mut language_is_consistent = true;
    for (index, (sentence_id, sentence)) in received.sentences.into_iter().enumerate() {
        if sentence_id != index as u64 + 1 {
            return Err(AdapterFailure::other(
                "sentence_sequence_gap",
                "Qwen Message final sentence sequence was incomplete",
                audio_seconds_sent,
            ));
        }
        text.push_str(&sentence.text);
        segments.push(Segment {
            text: sentence.text,
            start: sentence.start,
            end: sentence.end,
            avg_logprob: None,
            no_speech_prob: None,
        });
        words.extend(sentence.words);
        if let Some(found) = sentence.language {
            let is_mismatch = language.as_ref().is_some_and(|existing| existing != &found);
            if is_mismatch {
                language_is_consistent = false;
            } else if language.is_none() {
                language = Some(found);
            }
        } else {
            language_is_consistent = false;
        }
    }
    if text.trim().is_empty() {
        return Err(AdapterFailure {
            error: AsrError::EmptyResult,
            failure_code: "empty_result",
            audio_seconds_sent,
        });
    }
    Ok(Transcript {
        asr_text: Some(text.clone()),
        text,
        provider_cleaned_candidate: None,
        language: language_is_consistent.then_some(language).flatten(),
        confidence: None,
        segments,
        words,
        tokens: Vec::new(),
        limits: RateLimits::default(),
    })
}

fn parse_server_message(
    text: &str,
    audio_seconds_sent: f64,
) -> Result<ServerMessage, AdapterFailure> {
    if text.len() > MAX_MESSAGE_BYTES {
        return Err(AdapterFailure::other(
            "message_limit",
            "Qwen Message response exceeds the supported size",
            audio_seconds_sent,
        ));
    }
    let value: Value = serde_json::from_str(text).map_err(|_| {
        AdapterFailure::other(
            "invalid_provider_message",
            "Qwen Message returned an invalid response",
            audio_seconds_sent,
        )
    })?;
    let header = value.get("header").ok_or_else(|| {
        AdapterFailure::other(
            "invalid_provider_message",
            "Qwen Message returned an invalid response",
            audio_seconds_sent,
        )
    })?;
    let event = header
        .get("event")
        .and_then(Value::as_str)
        .filter(|event| event.len() <= 64)
        .ok_or_else(|| {
            AdapterFailure::other(
                "invalid_provider_event",
                "Qwen Message returned an invalid task event",
                audio_seconds_sent,
            )
        })?;
    let task_id = header
        .get("task_id")
        .and_then(Value::as_str)
        .filter(|task_id| task_id.len() <= 64)
        .ok_or_else(|| {
            AdapterFailure::other(
                "missing_provider_task_id",
                "Qwen Message returned an unbound task event",
                audio_seconds_sent,
            )
        })?;
    Ok(ServerMessage {
        event: event.to_owned(),
        task_id: task_id.to_owned(),
        payload: value.get("payload").cloned().unwrap_or(Value::Null),
    })
}

fn check_task_binding(
    message: &ServerMessage,
    task_id: &str,
    audio_seconds_sent: f64,
) -> Result<(), AdapterFailure> {
    if message.task_id != task_id {
        return Err(AdapterFailure::other(
            "task_id_mismatch",
            "Qwen Message response did not match the active task",
            audio_seconds_sent,
        ));
    }
    Ok(())
}

fn record_final_usage(diagnostics: &AsrRequestDiagnostics, usage: &Value) {
    for (field, unit) in [
        ("duration", AsrUsageUnit::Seconds),
        ("input_tokens", AsrUsageUnit::InputTokens),
        ("output_tokens", AsrUsageUnit::OutputTokens),
        ("total_tokens", AsrUsageUnit::TotalTokens),
    ] {
        if let Some(amount) = usage_number(usage, field) {
            diagnostics.record_usage(unit, amount);
        }
    }
}

fn usage_number(usage: &Value, field: &str) -> Option<f64> {
    let value = usage.get(field)?.as_f64()?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn provider_failure(audio_seconds_sent: f64) -> AdapterFailure {
    AdapterFailure::other(
        "provider_task_failed",
        "Qwen Message task failed",
        audio_seconds_sent,
    )
}

fn handshake_failure(error: WebSocketError) -> AdapterFailure {
    match error {
        WebSocketError::Http(response)
            if response.status()
                == tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED
                || response.status()
                    == tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN =>
        {
            AdapterFailure {
                error: AsrError::Unauthorized("Qwen Message authorization failed".to_owned()),
                failure_code: "unauthorized",
                audio_seconds_sent: 0.0,
            }
        }
        WebSocketError::Http(response)
            if response.status()
                == tokio_tungstenite::tungstenite::http::StatusCode::TOO_MANY_REQUESTS =>
        {
            AdapterFailure {
                error: AsrError::RateLimited(" by Qwen Message".to_owned()),
                failure_code: "rate_limited",
                audio_seconds_sent: 0.0,
            }
        }
        _ => AdapterFailure::network("connect_failed", 0.0),
    }
}

fn new_task_id() -> Result<String, AdapterFailure> {
    let mut bytes = [0u8; 16];
    let mut random = OsRng;
    random.try_fill_bytes(&mut bytes).map_err(|_| {
        AdapterFailure::other(
            "task_id_failed",
            "Could not create a Qwen Message task id",
            0.0,
        )
    })?;
    // Use the RFC 4122 UUID v4/version and variant bits required by DashScope.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    ))
}
