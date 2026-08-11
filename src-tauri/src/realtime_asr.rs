//! Silent background ASR prefetching.
//!
//! Groq's transcription API accepts completed audio files rather than a
//! realtime stream. This module submits the same bounded chunks used by the
//! long-recording path while capture is still active. It never emits partial
//! text and it is only an optimization: callers can ignore incomplete
//! results and use the normal full-ASR path.

use crate::{asr, chunker::AudioChunk, metrics, queue};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

const CHANNEL_CAPACITY: usize = 4;
pub const WARMUP_CHUNK_SECS: usize = 10;
pub const WARMUP_OVERLAP_SECS: usize = 2;

pub enum RealtimeMessage {
    Warmup(AudioChunk),
    Chunk(AudioChunk),
    Finish {
        reply: oneshot::Sender<RealtimeAsrResult>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RealtimeAsrResult {
    pub warmup: Option<String>,
    pub transcripts: HashMap<usize, String>,
}

pub struct RealtimeAsrSession {
    sender: mpsc::Sender<RealtimeMessage>,
    cancellation: CancellationToken,
    results: Arc<Mutex<RealtimeAsrResult>>,
}

impl RealtimeAsrSession {
    pub fn channel() -> (
        mpsc::Sender<RealtimeMessage>,
        mpsc::Receiver<RealtimeMessage>,
    ) {
        mpsc::channel(CHANNEL_CAPACITY)
    }

    pub fn spawn(
        receiver: mpsc::Receiver<RealtimeMessage>,
        sender: mpsc::Sender<RealtimeMessage>,
        gate: Arc<queue::RequestGate>,
        provider: Arc<dyn asr::AsrProvider>,
        options: asr::AsrOptions,
        metrics: metrics::Metrics,
        cancellation: CancellationToken,
    ) -> Self {
        let results = Arc::new(Mutex::new(RealtimeAsrResult::default()));
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
                    RealtimeMessage::Warmup(chunk) => (chunk, true),
                    RealtimeMessage::Chunk(chunk) => (chunk, false),
                    RealtimeMessage::Finish { reply } => {
                        let result = shared_results
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .clone();
                        let _ = reply.send(result);
                        return;
                    }
                };
                if failed {
                    // A failed prefetch session drains queued chunks until
                    // Finish so the stop path can complete cleanly. The
                    // normal ASR path will process every missing chunk.
                    continue;
                }
                {
                    let wav = match crate::chunker::encode_wav(&chunk.samples) {
                        Ok(wav) => wav,
                        Err(error) => {
                            log::warn!(
                                "realtime ASR chunk {} encoding failed: {error}",
                                chunk.index
                            );
                            failed = true;
                            continue;
                        }
                    };
                    let chunk_index = chunk.index;
                    let provider = provider.clone();
                    let options = options.clone();
                    let _latency = metrics.timer(metrics::MetricKind::PrefetchAsr);
                    let result = queue::execute_with_retry_cancelled(
                        &gate,
                        queue::RequestKind::Asr,
                        || provider.prefetch_chunk(wav.clone(), options.clone()),
                        worker_cancellation.clone(),
                    )
                    .await;
                    match result {
                        Ok(transcript) => {
                            gate.update_asr(&transcript.limits);
                            if !transcript.text.trim().is_empty() {
                                let mut results = shared_results
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                if is_warmup {
                                    results.warmup = Some(transcript.text);
                                } else {
                                    results.transcripts.insert(chunk_index, transcript.text);
                                }
                            }
                        }
                        Err(queue::ExecuteError::Cancelled) => return,
                        Err(queue::ExecuteError::Operation(error)) => {
                            log::warn!(
                                    "realtime ASR chunk {chunk_index} failed; final ASR will retry it: {error}"
                                );
                            failed = true;
                        }
                    }
                }
            }
        });
        Self {
            sender,
            cancellation,
            results,
        }
    }

    pub async fn finish(self, timeout: Duration) -> RealtimeAsrResult {
        let (reply_tx, reply_rx) = oneshot::channel();
        let result = tokio::time::timeout(timeout, async {
            self.sender
                .send(RealtimeMessage::Finish { reply: reply_tx })
                .await
                .map_err(|_| ())?;
            reply_rx.await.map_err(|_| ())
        })
        .await;
        match result {
            Ok(Ok(result)) => result,
            _ => {
                self.cancellation.cancel();
                let result = self
                    .results
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                result
            }
        }
    }

    pub fn cancel(self) {
        self.cancellation.cancel();
    }
}
