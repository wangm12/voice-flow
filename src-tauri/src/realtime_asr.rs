//! Silent background ASR prefetching.
//!
//! Groq's transcription API accepts completed audio files rather than a
//! realtime stream. This module submits the same bounded chunks used by the
//! long-recording path while capture is still active. It never emits partial
//! text and it is only an optimization: callers can ignore incomplete
//! results and use the normal full-ASR path.

use crate::{asr, chunker::AudioChunk, metrics, queue};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
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

/// Bounded prefetch inbox shared by the audio capture thread and the ASR worker.
/// `try_send` never blocks the audio thread; drops are counted so callers can
/// ignore incomplete prefetch results.
#[derive(Clone)]
pub struct PrefetchInbox {
    sender: mpsc::Sender<RealtimeMessage>,
    drops: Arc<AtomicU32>,
}

impl PrefetchInbox {
    pub fn try_send(&self, message: RealtimeMessage) -> bool {
        match self.sender.try_send(message) {
            Ok(()) => true,
            Err(_) => {
                let count = self.drops.fetch_add(1, Ordering::Relaxed) + 1;
                log::warn!(
                    "realtime ASR prefetch dropped a chunk (drop #{count}); final ASR will cover it"
                );
                false
            }
        }
    }
}

pub struct RealtimeAsrSession {
    sender: mpsc::Sender<RealtimeMessage>,
    cancellation: CancellationToken,
    results: Arc<Mutex<RealtimeAsrResult>>,
    drops: Arc<AtomicU32>,
}

impl RealtimeAsrSession {
    pub fn channel() -> (PrefetchInbox, mpsc::Receiver<RealtimeMessage>) {
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
        receiver: mpsc::Receiver<RealtimeMessage>,
        inbox: PrefetchInbox,
        gate: Arc<queue::RequestGate>,
        provider: Arc<dyn asr::AsrProvider>,
        options: asr::AsrOptions,
        metrics: metrics::Metrics,
        cancellation: CancellationToken,
    ) -> Self {
        let sender = inbox.sender.clone();
        let drops = Arc::clone(&inbox.drops);
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
            drops,
        }
    }

    pub async fn finish(self, timeout: Duration) -> RealtimeAsrResult {
        let dropped = self.drops.load(Ordering::Relaxed);
        let (reply_tx, reply_rx) = oneshot::channel();
        let result = tokio::time::timeout(timeout, async {
            self.sender
                .send(RealtimeMessage::Finish { reply: reply_tx })
                .await
                .map_err(|_| ())?;
            reply_rx.await.map_err(|_| ())
        })
        .await;
        if dropped > 0 {
            log::warn!(
                "ignoring prefetch results because {dropped} chunk(s) were dropped; final ASR will cover them"
            );
            self.cancellation.cancel();
            return RealtimeAsrResult::default();
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
        AudioChunk {
            index,
            samples: vec![0.0; 16],
            start_secs: index as f32,
            end_secs: index as f32 + 1.0,
        }
    }

    #[test]
    fn prefetch_inbox_counts_dropped_chunks_without_blocking() {
        let (inbox, _receiver) = RealtimeAsrSession::channel();
        for index in 0..CHANNEL_CAPACITY {
            assert!(inbox.try_send(RealtimeMessage::Chunk(sample_chunk(index))));
        }
        assert_eq!(inbox.drops.load(Ordering::Relaxed), 0);
        assert!(!inbox.try_send(RealtimeMessage::Chunk(sample_chunk(CHANNEL_CAPACITY))));
        assert!(inbox.drops.load(Ordering::Relaxed) > 0);
    }
}
