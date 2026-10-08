//! Silent batch ASR prefetching.
//!
//! Batch providers accept completed audio files, so this is not streaming
//! ASR. This module submits the same bounded chunks used by the long-recording
//! path while capture is still active. Completed transcripts stay in the
//! background for the final-ASR path; they are never shown on the HUD,
//! clipboard, History, or paste. Callers retain completed results and use the
//! normal final-ASR path for missing chunks. True realtime streaming providers
//! bypass this module and send the captured PCM directly.

use crate::{
    asr,
    chunker::{AudioChunk, AudioChunkIdentity},
    context::TargetAppGuard,
    metrics, queue,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

const CHANNEL_CAPACITY: usize = 4;
pub const WARMUP_CHUNK_SECS: usize = 10;
pub const WARMUP_OVERLAP_SECS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PrefetchRequestIdentity([u8; 32]);

impl PrefetchRequestIdentity {
    pub fn new(provider: &str, endpoint: Option<&str>, options: &asr::AsrOptions) -> Self {
        let mut hasher = Sha256::new();
        hash_field(&mut hasher, provider);
        hash_optional_field(&mut hasher, endpoint);
        // Credentials affect routing and are included only in this ephemeral
        // digest; the credential itself is never stored in the result.
        hash_field(&mut hasher, &options.api_key);
        hash_optional_field(&mut hasher, options.language.as_deref());
        hash_optional_field(&mut hasher, options.prompt.as_deref());
        hash_field(&mut hasher, &options.model);
        hasher.update((options.keywords.len() as u64).to_le_bytes());
        for keyword in &options.keywords {
            hash_field(&mut hasher, keyword);
        }
        Self(hasher.finalize().into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrefetchSessionBinding {
    pub recording_generation: u64,
    pub configuration_generation: u64,
    target_identity: [u8; 32],
}

impl PrefetchSessionBinding {
    pub fn new(
        recording_generation: u64,
        configuration_generation: u64,
        target: &TargetAppGuard,
    ) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"voiceflow-target-identity-v1");
        hasher.update(target.pid.to_le_bytes());
        hash_optional_field(&mut hasher, target.bundle_id.as_deref());
        hash_optional_field(&mut hasher, target.browser_host.as_deref());
        hash_option_u64(&mut hasher, target.browser_target_token);
        hash_option_u64(&mut hasher, target.window_token);
        hash_option_u64(&mut hasher, target.window_id);
        hash_option_u64(&mut hasher, target.input_token);
        hasher.update([u8::from(target.secure_input)]);
        Self {
            recording_generation,
            configuration_generation,
            target_identity: hasher.finalize().into(),
        }
    }

    #[cfg(test)]
    pub fn test_default() -> Self {
        Self {
            recording_generation: 0,
            configuration_generation: 0,
            target_identity: [0; 32],
        }
    }

    pub fn matches_target(&self, target: &TargetAppGuard) -> bool {
        self.target_identity
            == Self::new(
                self.recording_generation,
                self.configuration_generation,
                target,
            )
            .target_identity
    }
}

fn hash_field(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

fn hash_optional_field(hasher: &mut Sha256, value: Option<&str>) {
    if let Some(value) = value {
        hasher.update([1]);
        hash_field(hasher, value);
    } else {
        hasher.update([0]);
    }
}

fn hash_option_u64(hasher: &mut Sha256, value: Option<u64>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hasher.update(value.to_le_bytes());
        }
        None => hasher.update([0]),
    }
}

pub enum PrefetchMessage {
    Warmup(AudioChunk),
    Chunk(AudioChunk),
    Finish {
        reply: oneshot::Sender<PrefetchAsrResult>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PrefetchedTranscript {
    pub request_identity: PrefetchRequestIdentity,
    pub chunk_index: usize,
    /// Keep the complete adapter result so metadata, confidence, limits, and
    /// provider-original text survive silent prefetch reuse.
    pub transcript: asr::Transcript,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PrefetchedWarmup {
    pub request_identity: PrefetchRequestIdentity,
    pub sample_identity: AudioChunkIdentity,
    pub transcript: asr::Transcript,
}

const HUD_PARTIAL_MAX_CHARS: usize = 280;

/// Concatenate completed non-warmup chunk transcripts for HUD display.
/// Index order, space-separated, trimmed, capped at 280 chars with ellipsis.
pub fn hud_partial_text(transcripts: &HashMap<AudioChunkIdentity, PrefetchedTranscript>) -> String {
    let mut completed: Vec<&PrefetchedTranscript> = transcripts.values().collect();
    completed.sort_by_key(|entry| entry.chunk_index);
    let joined = completed
        .into_iter()
        .map(|entry| entry.transcript.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    cap_hud_partial(&joined)
}

fn cap_hud_partial(text: &str) -> String {
    let char_count = text.chars().count();
    if char_count <= HUD_PARTIAL_MAX_CHARS {
        return text.to_owned();
    }
    const ELLIPSIS: &str = "...";
    let keep = HUD_PARTIAL_MAX_CHARS.saturating_sub(ELLIPSIS.chars().count());
    let mut truncated: String = text.chars().take(keep).collect();
    truncated.push_str(ELLIPSIS);
    truncated
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrefetchAsrResult {
    /// Session-scoped provenance for every result in this container.
    pub session_binding: Option<PrefetchSessionBinding>,
    /// Silent warmup output used only by final short-recording assembly.
    pub warmup: Option<PrefetchedWarmup>,
    /// Results are keyed by exact samples, never by a positional chunk index.
    pub transcripts: HashMap<AudioChunkIdentity, PrefetchedTranscript>,
}

impl PrefetchAsrResult {
    pub fn belongs_to(
        &self,
        recording_generation: u64,
        configuration_generation: u64,
        target: &TargetAppGuard,
    ) -> bool {
        self.session_binding.is_some_and(|binding| {
            binding.recording_generation == recording_generation
                && binding.configuration_generation == configuration_generation
                && binding.matches_target(target)
        })
    }

    pub fn transcripts_for_request(
        &self,
        request_identity: PrefetchRequestIdentity,
    ) -> HashMap<AudioChunkIdentity, asr::Transcript> {
        self.transcripts
            .iter()
            .filter(|(_, transcript)| transcript.request_identity == request_identity)
            .map(|(sample_identity, transcript)| (*sample_identity, transcript.transcript.clone()))
            .collect()
    }

    pub fn warmup_for_request(
        &self,
        request_identity: PrefetchRequestIdentity,
    ) -> Option<&PrefetchedWarmup> {
        self.warmup
            .as_ref()
            .filter(|warmup| warmup.request_identity == request_identity)
    }
}

/// Bounded batch-prefetch inbox shared by capture and the ASR worker.
/// `try_send` never blocks the audio thread; drops are counted so final ASR
/// can process the missing chunk indexes.
#[derive(Clone)]
pub struct PrefetchInbox {
    sender: mpsc::Sender<PrefetchMessage>,
    drops: Arc<AtomicU32>,
}

impl PrefetchInbox {
    pub fn try_send(&self, message: PrefetchMessage) -> bool {
        match self.sender.try_send(message) {
            Ok(()) => true,
            Err(_) => {
                let count = self.drops.fetch_add(1, Ordering::Relaxed) + 1;
                log::warn!(
                    "batch ASR prefetch dropped a chunk (drop #{count}); final ASR will cover it"
                );
                false
            }
        }
    }
}

pub type HudPartialEmit = Arc<dyn Fn(u64, String) + Send + Sync>;

pub struct PrefetchSessionConfig {
    pub metrics: metrics::Metrics,
    pub metric_group: metrics::MetricGroup,
    pub cancellation: CancellationToken,
    pub hud_partial: Option<HudPartialEmit>,
    pub session_generation: u64,
    pub session_binding: PrefetchSessionBinding,
    pub request_identity: PrefetchRequestIdentity,
    pub quota_scope: queue::RequestScope,
}

pub struct PrefetchAsrSession {
    sender: mpsc::Sender<PrefetchMessage>,
    cancellation: CancellationToken,
    results: Arc<Mutex<PrefetchAsrResult>>,
    drops: Arc<AtomicU32>,
}

impl PrefetchAsrSession {
    pub fn channel() -> (PrefetchInbox, mpsc::Receiver<PrefetchMessage>) {
        let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
        (
            PrefetchInbox {
                sender,
                drops: Arc::new(AtomicU32::new(0)),
            },
            receiver,
        )
    }

    pub fn spawn(
        receiver: mpsc::Receiver<PrefetchMessage>,
        inbox: PrefetchInbox,
        gate: Arc<queue::RequestGate>,
        provider: Arc<dyn asr::AsrProvider>,
        options: asr::AsrOptions,
        config: PrefetchSessionConfig,
    ) -> Self {
        let PrefetchSessionConfig {
            metrics,
            metric_group,
            cancellation,
            hud_partial,
            session_generation,
            session_binding,
            request_identity,
            quota_scope,
        } = config;
        let sender = inbox.sender.clone();
        let drops = Arc::clone(&inbox.drops);
        let results = Arc::new(Mutex::new(PrefetchAsrResult {
            session_binding: Some(session_binding),
            ..PrefetchAsrResult::default()
        }));
        let shared_results = Arc::clone(&results);
        let worker_cancellation = cancellation.clone();
        tokio::spawn(async move {
            let mut failed = false;
            let mut receiver = receiver;
            loop {
                let message = tokio::select! {
                    biased;
                    _ = worker_cancellation.cancelled() => return,
                    message = receiver.recv() => message,
                };
                let Some(message) = message else {
                    return;
                };
                let (chunk, is_warmup) = match message {
                    PrefetchMessage::Warmup(chunk) => (chunk, true),
                    PrefetchMessage::Chunk(chunk) => (chunk, false),
                    PrefetchMessage::Finish { reply } => {
                        let result = shared_results
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .clone();
                        let _ = reply.send(result);
                        return;
                    }
                };
                if failed {
                    // A failed batch-prefetch session drains queued chunks
                    // until Finish so the stop path can complete cleanly. The
                    // normal final-ASR path processes every missing chunk.
                    continue;
                }
                let wav = match crate::chunker::encode_wav(&chunk.samples) {
                    Ok(wav) => wav,
                    Err(error) => {
                        log::warn!(
                            "batch ASR prefetch chunk {} encoding failed: {error}",
                            chunk.index
                        );
                        failed = true;
                        continue;
                    }
                };
                let chunk_index = chunk.index;
                let sample_identity = chunk.identity;
                let provider = provider.clone();
                let options = options.clone();
                let _latency =
                    metrics.timer_for(metrics::MetricKind::PrefetchAsr, metric_group.clone());
                let result = queue::execute_with_retry_scoped_cancelled(
                    &gate,
                    queue::RequestKind::Asr,
                    &quota_scope,
                    || provider.prefetch_chunk(wav.clone(), options.clone()),
                    worker_cancellation.clone(),
                )
                .await;
                match result {
                    Ok(transcript) => {
                        gate.update_asr_for(&quota_scope, &transcript.limits);
                        if !transcript.text.trim().is_empty() {
                            let mut results = shared_results
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            if is_warmup {
                                // Warmup remains silent-only: it is never
                                // inserted into the indexed long results.
                                results.warmup = Some(PrefetchedWarmup {
                                    request_identity,
                                    sample_identity,
                                    transcript,
                                });
                            } else {
                                results.transcripts.insert(
                                    sample_identity,
                                    PrefetchedTranscript {
                                        request_identity,
                                        chunk_index,
                                        transcript,
                                    },
                                );
                                let text = hud_partial_text(&results.transcripts);
                                drop(results);
                                if let Some(emit) = &hud_partial {
                                    if !text.is_empty() {
                                        emit(session_generation, text);
                                    }
                                }
                            }
                        }
                    }
                    Err(queue::ExecuteError::Cancelled) => return,
                    Err(queue::ExecuteError::Operation(error)) => {
                        log::warn!(
                            "batch ASR prefetch chunk {chunk_index} failed; final ASR will retry it: {error}"
                        );
                        metrics.record_error(&metric_group, "prefetch_asr_failed");
                        failed = true;
                    }
                }
            }
        });
        Self {
            sender,
            cancellation,
            results,
            drops,
        }
    }

    pub async fn finish(self, timeout: Duration) -> PrefetchAsrResult {
        let dropped = self.drops.load(Ordering::Relaxed);
        let (reply_tx, reply_rx) = oneshot::channel();
        let result = tokio::time::timeout(timeout, async {
            self.sender
                .send(PrefetchMessage::Finish { reply: reply_tx })
                .await
                .map_err(|_| ())?;
            reply_rx.await.map_err(|_| ())
        })
        .await;
        if dropped > 0 {
            log::warn!(
                "batch prefetch inbox dropped {dropped} chunk(s); retaining completed results and final ASR will cover only missing chunks"
            );
        }
        match result {
            Ok(Ok(result)) => result,
            _ => {
                self.cancellation.cancel();
                self.results
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone()
            }
        }
    }

    pub fn cancel(self) {
        self.cancellation.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_chunk(index: usize) -> AudioChunk {
        let samples = vec![0.0; 16];
        let source_start_sample = index * 16;
        AudioChunk {
            index,
            identity: AudioChunkIdentity::from_samples_at(source_start_sample, &samples),
            samples,
            source_start_sample,
            start_secs: index as f32,
            end_secs: index as f32 + 1.0,
        }
    }

    fn test_request_identity() -> PrefetchRequestIdentity {
        PrefetchRequestIdentity::new(
            "groq",
            Some("https://api.groq.com/v1"),
            &asr::AsrOptions::default(),
        )
    }

    fn transcript(text: &str) -> asr::Transcript {
        asr::Transcript {
            text: text.to_owned(),
            asr_text: Some(text.to_owned()),
            provider_cleaned_candidate: None,
            segments: Vec::new(),
            words: Vec::new(),
            tokens: Vec::new(),
            limits: asr::RateLimits::default(),
            language: None,
            confidence: None,
        }
    }

    fn prefetched(index: usize, text: &str) -> (AudioChunkIdentity, PrefetchedTranscript) {
        let identity = sample_chunk(index).identity;
        (
            identity,
            PrefetchedTranscript {
                request_identity: test_request_identity(),
                chunk_index: index,
                transcript: transcript(text),
            },
        )
    }

    #[test]
    fn prefetch_inbox_counts_dropped_chunks_without_blocking() {
        let (inbox, _receiver) = PrefetchAsrSession::channel();
        for index in 0..CHANNEL_CAPACITY {
            assert!(inbox.try_send(PrefetchMessage::Chunk(sample_chunk(index))));
        }
        assert_eq!(inbox.drops.load(Ordering::Relaxed), 0);
        assert!(!inbox.try_send(PrefetchMessage::Chunk(sample_chunk(CHANNEL_CAPACITY))));
        assert!(inbox.drops.load(Ordering::Relaxed) > 0);
    }

    #[tokio::test]
    async fn finish_retains_completed_chunks_after_an_inbox_drop() {
        let (sender, _receiver) = mpsc::channel(CHANNEL_CAPACITY);
        let completed = PrefetchAsrResult {
            session_binding: None,
            warmup: Some(PrefetchedWarmup {
                request_identity: test_request_identity(),
                sample_identity: sample_chunk(0).identity,
                transcript: transcript("warmup only"),
            }),
            transcripts: HashMap::from([prefetched(2, "already complete")]),
        };
        let session = PrefetchAsrSession {
            sender,
            cancellation: CancellationToken::new(),
            results: Arc::new(Mutex::new(completed.clone())),
            drops: Arc::new(AtomicU32::new(1)),
        };

        let result = session.finish(Duration::ZERO).await;

        assert_eq!(result, completed);
    }

    #[test]
    fn hud_partial_text_joins_completed_chunks_in_index_order() {
        let transcripts = HashMap::from([
            prefetched(2, "  two  "),
            prefetched(0, "one"),
            prefetched(5, ""),
            prefetched(1, "  "),
        ]);

        assert_eq!(hud_partial_text(&transcripts), "one two");
    }

    #[test]
    fn hud_partial_text_caps_at_280_chars_with_ellipsis() {
        let long = "x".repeat(300);
        let transcripts = HashMap::from([prefetched(0, &long)]);
        let text = hud_partial_text(&transcripts);

        assert_eq!(text.chars().count(), 280);
        assert!(text.ends_with("..."));
        assert_eq!(text.chars().take(277).collect::<String>(), "x".repeat(277));
    }

    #[test]
    fn hud_partial_text_does_not_include_warmup_and_is_display_only() {
        let result = PrefetchAsrResult {
            session_binding: None,
            warmup: Some(PrefetchedWarmup {
                request_identity: test_request_identity(),
                sample_identity: sample_chunk(0).identity,
                transcript: transcript("warmup must stay silent"),
            }),
            transcripts: HashMap::from([prefetched(1, "visible")]),
        };

        assert_eq!(hud_partial_text(&result.transcripts), "visible");
        assert_eq!(
            result
                .warmup
                .as_ref()
                .map(|warmup| warmup.transcript.text.as_str()),
            Some("warmup must stay silent")
        );
    }

    type HudEvents = Arc<Mutex<Vec<(u64, String)>>>;

    struct ScriptedAsr {
        replies: Mutex<std::collections::VecDeque<Result<String, asr::AsrError>>>,
    }

    impl ScriptedAsr {
        fn ok(texts: &[&str]) -> Arc<Self> {
            Arc::new(Self {
                replies: Mutex::new(texts.iter().map(|text| Ok((*text).to_owned())).collect()),
            })
        }

        fn err() -> Arc<Self> {
            Arc::new(Self {
                replies: Mutex::new(std::collections::VecDeque::from([Err(
                    asr::AsrError::Other("prefetch failed".into()),
                )])),
            })
        }
    }

    impl asr::AsrProvider for ScriptedAsr {
        fn transcribe_batch(&self, _audio: Vec<u8>, _options: asr::AsrOptions) -> asr::AsrFuture {
            let reply = self
                .replies
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop_front()
                .unwrap_or_else(|| Ok(String::new()));
            Box::pin(async move {
                reply.map(|text| asr::Transcript {
                    asr_text: Some(text.clone()),
                    provider_cleaned_candidate: None,
                    text,
                    language: None,
                    confidence: None,
                    segments: Vec::new(),
                    words: Vec::new(),
                    tokens: Vec::new(),
                    limits: asr::RateLimits::default(),
                })
            })
        }

        fn capabilities(&self) -> asr::AsrCapabilities {
            asr::asr_capabilities_for(
                "https://api.groq.com/openai/v1/audio/transcriptions",
                "whisper-large-v3",
            )
        }
    }

    fn spawn_with_hud(
        provider: Arc<dyn asr::AsrProvider>,
        hud: HudEvents,
        generation: u64,
    ) -> (PrefetchInbox, PrefetchAsrSession) {
        let (inbox, receiver) = PrefetchAsrSession::channel();
        let session = PrefetchAsrSession::spawn(
            receiver,
            inbox.clone(),
            Arc::new(queue::RequestGate::new(None)),
            provider,
            asr::AsrOptions::default(),
            PrefetchSessionConfig {
                metrics: metrics::Metrics::default(),
                metric_group: metrics::MetricGroup::unknown("prefetch"),
                session_binding: PrefetchSessionBinding::test_default(),
                cancellation: CancellationToken::new(),
                hud_partial: Some(Arc::new(move |generation, text| {
                    hud.lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push((generation, text));
                })),
                session_generation: generation,
                request_identity: test_request_identity(),
                quota_scope: queue::RequestScope::new("mock", None, "mock-model", ""),
            },
        );
        (inbox, session)
    }

    fn spawn_silent(provider: Arc<dyn asr::AsrProvider>) -> (PrefetchInbox, PrefetchAsrSession) {
        let (inbox, receiver) = PrefetchAsrSession::channel();
        let session = PrefetchAsrSession::spawn(
            receiver,
            inbox.clone(),
            Arc::new(queue::RequestGate::new(None)),
            provider,
            asr::AsrOptions::default(),
            PrefetchSessionConfig {
                metrics: metrics::Metrics::default(),
                metric_group: metrics::MetricGroup::unknown("prefetch"),
                session_binding: PrefetchSessionBinding::test_default(),
                cancellation: CancellationToken::new(),
                hud_partial: None,
                session_generation: 1,
                request_identity: test_request_identity(),
                quota_scope: queue::RequestScope::new("mock", None, "mock-model", ""),
            },
        );
        (inbox, session)
    }

    #[tokio::test]
    async fn successful_chunks_stay_in_the_background_without_a_hud_callback() {
        let (inbox, session) = spawn_silent(ScriptedAsr::ok(&[" later", "hello "]));

        assert!(inbox.try_send(PrefetchMessage::Chunk(sample_chunk(1))));
        assert!(inbox.try_send(PrefetchMessage::Chunk(sample_chunk(0))));
        let result = session.finish(Duration::from_secs(2)).await;

        assert_eq!(
            result
                .transcripts
                .values()
                .map(|entry| (entry.chunk_index, entry.transcript.text.as_str()))
                .collect::<HashMap<_, _>>(),
            HashMap::from([(0, "hello "), (1, " later")])
        );
    }

    #[tokio::test]
    async fn successful_non_warmup_chunks_emit_concatenated_hud_partials() {
        let hud: HudEvents = Arc::new(Mutex::new(Vec::new()));
        let (inbox, session) =
            spawn_with_hud(ScriptedAsr::ok(&[" later", "hello "]), Arc::clone(&hud), 7);

        assert!(inbox.try_send(PrefetchMessage::Chunk(sample_chunk(1))));
        assert!(inbox.try_send(PrefetchMessage::Chunk(sample_chunk(0))));
        let result = session.finish(Duration::from_secs(2)).await;

        assert_eq!(
            result
                .transcripts
                .values()
                .map(|entry| (entry.chunk_index, entry.transcript.text.as_str()))
                .collect::<HashMap<_, _>>(),
            HashMap::from([(0, "hello "), (1, " later")])
        );
        assert_eq!(
            hud.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[(7, "later".to_owned()), (7, "hello later".to_owned())]
        );
    }

    #[tokio::test]
    async fn warmup_and_prefetch_failure_stay_silent_on_the_hud() {
        let warmup_hud: HudEvents = Arc::new(Mutex::new(Vec::new()));
        let (warmup_inbox, warmup_session) = spawn_with_hud(
            ScriptedAsr::ok(&["warmup only"]),
            Arc::clone(&warmup_hud),
            3,
        );
        assert!(warmup_inbox.try_send(PrefetchMessage::Warmup(sample_chunk(0))));
        let warmup_result = warmup_session.finish(Duration::from_secs(2)).await;
        assert_eq!(
            warmup_result
                .warmup
                .as_ref()
                .map(|warmup| warmup.transcript.text.as_str()),
            Some("warmup only")
        );
        assert!(warmup_hud
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());

        let fail_hud: HudEvents = Arc::new(Mutex::new(Vec::new()));
        let (fail_inbox, fail_session) =
            spawn_with_hud(ScriptedAsr::err(), Arc::clone(&fail_hud), 4);
        assert!(fail_inbox.try_send(PrefetchMessage::Chunk(sample_chunk(0))));
        let fail_result = fail_session.finish(Duration::from_secs(2)).await;
        assert!(fail_result.transcripts.is_empty());
        assert!(fail_hud
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
    }
}
