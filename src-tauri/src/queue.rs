//! Bounded request queue, retry policy, and API quota tracking.

use serde::Serialize;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

const UNKNOWN_QUOTA_RESET_MS: i64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    Asr,
    Llm,
    HistoryLlm,
}
impl RequestKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Asr => "asr",
            Self::Llm => "llm",
            Self::HistoryLlm => "history_llm",
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct QuotaSnapshot {
    pub remaining_requests_rpd: i64,
    pub remaining_tokens_tpm: i64,
    pub reset_requests_at: Option<i64>,
    pub reset_tokens_at: Option<i64>,
}
#[derive(Debug, Clone)]
struct Quota {
    snapshot: QuotaSnapshot,
    requests_limit: i64,
    tokens_limit: i64,
}
#[derive(Debug, Clone, Serialize)]
pub struct QuotaView {
    pub asr: QuotaSnapshot,
    pub llm: QuotaSnapshot,
    pub asr_rpm_limit: i64,
    pub asr_requests_limit: i64,
    pub asr_audio_hours_limit: i64,
    pub llm_rpm_limit: i64,
    pub llm_requests_limit: i64,
    pub llm_tokens_limit: i64,
}

pub trait RetryError {
    fn retry_kind(&self) -> RetryClass;
}
#[derive(Debug, Clone, PartialEq)]
pub enum RetryClass {
    RateLimited(f64),
    Network,
    Server,
    Unauthorized,
    Other,
}

pub struct RequestGate {
    semaphore: Arc<Semaphore>,
    quotas: Arc<Mutex<(Quota, Quota)>>,
    app: Option<AppHandle>,
    session_generation: Arc<AtomicU64>,
}
impl RequestGate {
    pub fn new(app: Option<AppHandle>) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(2)),
            quotas: Arc::new(Mutex::new((default_asr(), default_llm()))),
            app,
            session_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn set_session_generation(&self, generation: u64) {
        self.session_generation.store(generation, Ordering::Release);
    }
    pub fn snapshots(&self) -> QuotaView {
        let q = self.quotas.lock().unwrap();
        QuotaView {
            asr: q.0.snapshot.clone(),
            llm: q.1.snapshot.clone(),
            asr_rpm_limit: 20,
            asr_requests_limit: q.0.requests_limit,
            asr_audio_hours_limit: 7200,
            llm_rpm_limit: 30,
            llm_requests_limit: q.1.requests_limit,
            llm_tokens_limit: q.1.tokens_limit,
        }
    }
    pub fn update_asr(&self, limits: &crate::asr::RateLimits) {
        self.update(
            RequestKind::Asr,
            limits.requests.as_deref(),
            limits.tokens.as_deref(),
            limits.reset_requests.as_deref(),
            limits.reset_tokens.as_deref(),
        );
    }
    pub fn update_llm(&self, limits: &crate::llm::RateLimits) {
        self.update(
            RequestKind::Llm,
            limits.requests.as_deref(),
            limits.tokens.as_deref(),
            limits.reset_requests.as_deref(),
            limits.reset_tokens.as_deref(),
        );
    }
    fn update(
        &self,
        kind: RequestKind,
        requests: Option<&str>,
        tokens: Option<&str>,
        reset_requests: Option<&str>,
        reset_tokens: Option<&str>,
    ) {
        let mut q = self.quotas.lock().unwrap();
        let quota = if kind == RequestKind::Asr {
            &mut q.0
        } else {
            &mut q.1
        };
        let parsed_requests = requests.and_then(|x| x.parse::<i64>().ok());
        if let Some(v) = parsed_requests {
            quota.snapshot.remaining_requests_rpd = v;
        }
        if let Some(v) = tokens.and_then(|x| x.parse().ok()) {
            quota.snapshot.remaining_tokens_tpm = v;
        }
        if let Some(reset) = reset_requests.and_then(parse_reset_at) {
            quota.snapshot.reset_requests_at = Some(reset);
        } else if parsed_requests.is_some_and(|remaining| remaining < 2) {
            // Some provider responses expose the remaining count without a
            // reset header. Do not immediately send another request when the
            // gate already considers the quota exhausted; retain any existing
            // reset or use a short conservative backoff until the next header.
            quota
                .snapshot
                .reset_requests_at
                .get_or_insert_with(|| now_ms().saturating_add(UNKNOWN_QUOTA_RESET_MS));
        } else if parsed_requests.is_some_and(|remaining| remaining >= 2) {
            quota.snapshot.reset_requests_at = None;
        }
        if let Some(reset) = reset_tokens.and_then(parse_reset_at) {
            quota.snapshot.reset_tokens_at = Some(reset);
        }
        if quota.snapshot.remaining_requests_rpd * 10 < quota.requests_limit {
            self.emit("quota://low", serde_json::json!({"kind": kind.name(), "remaining": quota.snapshot.remaining_requests_rpd}));
        }
    }
    /// Mark a quota as exhausted (remaining = 0) so `wait_for_quota` blocks
    /// until the reset time, and the UI shows the true remaining count. Called
    /// when the server returns 429 before any successful response updates the
    /// remaining-* headers.
    pub fn mark_rate_limited(&self, kind: RequestKind, retry_after_secs: f64) {
        let mut q = self.quotas.lock().unwrap();
        let quota = if kind == RequestKind::Asr {
            &mut q.0
        } else {
            &mut q.1
        };
        quota.snapshot.remaining_requests_rpd = 0;
        let wait_ms = (retry_after_secs.max(0.0) * 1000.0).ceil() as i64;
        quota.snapshot.reset_requests_at = Some(now_ms().saturating_add(wait_ms));
    }
    async fn wait_for_quota(&self, kind: RequestKind, cancellation: &CancellationToken) -> bool {
        loop {
            let until = {
                let q = self.quotas.lock().unwrap();
                let quota = if kind == RequestKind::Asr { &q.0 } else { &q.1 };
                if quota.snapshot.remaining_requests_rpd >= 2 {
                    None
                } else {
                    quota.snapshot.reset_requests_at
                }
            };
            if let Some(at) = until {
                let now = now_ms();
                if at > now {
                    tokio::select! { _ = cancellation.cancelled() => return false, _ = tokio::time::sleep(Duration::from_millis((at - now) as u64)) => {} }
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        true
    }
    fn emit(&self, event: &str, payload: serde_json::Value) {
        if let Some(app) = &self.app {
            let _ = app.emit(event, payload);
        }
    }

    fn current_session_generation(&self) -> u64 {
        self.session_generation.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
pub enum ExecuteError<E> {
    Operation(E),
    Cancelled,
}

pub async fn execute_with_retry<F, Fut, T, E>(
    gate: &RequestGate,
    kind: RequestKind,
    operation: F,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: RetryError,
{
    execute_with_retry_cancelled(gate, kind, operation, CancellationToken::new())
        .await
        .map_err(|error| match error {
            ExecuteError::Operation(error) => error,
            ExecuteError::Cancelled => unreachable!(),
        })
}

pub async fn execute_with_retry_cancelled<F, Fut, T, E>(
    gate: &RequestGate,
    kind: RequestKind,
    mut operation: F,
    cancellation: CancellationToken,
) -> Result<T, ExecuteError<E>>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: RetryError,
{
    let mut rate_retries = 0;
    let mut transient_retries = 0;
    loop {
        if !gate.wait_for_quota(kind, &cancellation).await {
            return Err(ExecuteError::Cancelled);
        }
        if cancellation.is_cancelled() {
            return Err(ExecuteError::Cancelled);
        }
        let _permit = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(ExecuteError::Cancelled),
            permit = gate.semaphore.acquire() => permit.expect("request semaphore is alive")
        };
        if cancellation.is_cancelled() {
            return Err(ExecuteError::Cancelled);
        }
        match tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(ExecuteError::Cancelled),
            result = operation() => result
        } {
            Ok(value) => return Ok(value),
            Err(error) => match error.retry_kind() {
                RetryClass::Unauthorized | RetryClass::Other => {
                    return Err(ExecuteError::Operation(error))
                }
                RetryClass::RateLimited(seconds) if rate_retries < 2 => {
                    rate_retries += 1;
                    // Sync the quota view so `wait_for_quota` and the UI reflect
                    // that we've hit the rate limit.
                    gate.mark_rate_limited(kind, seconds);
                    let rate_event = if kind == RequestKind::HistoryLlm {
                        "history://rate_limited"
                    } else {
                        "quota://rate_limited"
                    };
                    gate.emit(
                        rate_event,
                        serde_json::json!({
                            "retry_after_secs": seconds,
                            "session_generation": gate.current_session_generation(),
                        }),
                    );
                    if kind != RequestKind::HistoryLlm {
                        gate.emit(
                            "dictation://state",
                            serde_json::json!({
                                "state": "processing",
                                "phase": "waiting_retry",
                                "retry_after_secs": seconds.ceil() as u64,
                                "session_generation": gate.current_session_generation(),
                            }),
                        );
                    }
                    let wait_ms = (seconds * 1000.0).ceil() as u64;
                    let retry_event = if kind == RequestKind::HistoryLlm {
                        "history://retrying"
                    } else {
                        "quota://retrying"
                    };
                    gate.emit(retry_event, serde_json::json!({"attempt": rate_retries, "wait_ms": wait_ms, "kind": kind.name(), "session_generation": gate.current_session_generation()}));
                    tokio::select! { _ = cancellation.cancelled() => return Err(ExecuteError::Cancelled), _ = tokio::time::sleep(Duration::from_millis(wait_ms)) => {} }
                }
                RetryClass::Network | RetryClass::Server if transient_retries < 3 => {
                    transient_retries += 1;
                    let base = 1u64 << (transient_retries - 1);
                    let wait = jittered_ms(base * 1000);
                    gate.emit("quota://retrying", serde_json::json!({"attempt": transient_retries, "wait_ms": wait, "kind": kind.name()}));
                    tokio::select! { _ = cancellation.cancelled() => return Err(ExecuteError::Cancelled), _ = tokio::time::sleep(Duration::from_millis(wait)) => {} }
                }
                _ => return Err(ExecuteError::Operation(error)),
            },
        }
    }
}
fn default_asr() -> Quota {
    Quota {
        snapshot: QuotaSnapshot {
            remaining_requests_rpd: 2000,
            remaining_tokens_tpm: 7200,
            reset_requests_at: None,
            reset_tokens_at: None,
        },
        requests_limit: 2000,
        tokens_limit: 7200,
    }
}
fn default_llm() -> Quota {
    Quota {
        snapshot: QuotaSnapshot {
            remaining_requests_rpd: 1000,
            remaining_tokens_tpm: 8000,
            reset_requests_at: None,
            reset_tokens_at: None,
        },
        requests_limit: 1000,
        tokens_limit: 8000,
    }
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
pub fn parse_reset_at(value: &str) -> Option<i64> {
    let seconds = value
        .strip_suffix('s')
        .and_then(|v| v.parse::<u64>().ok())
        .or_else(|| {
            value
                .strip_suffix('m')
                .and_then(|v| v.parse::<u64>().ok().map(|n| n * 60))
        })?;
    Some(now_ms() + seconds as i64 * 1000)
}
#[cfg(test)]
pub fn backoff_ms(attempt: u32) -> u64 {
    1000 * 2u64.saturating_pow(attempt.saturating_sub(1))
}
fn jittered_ms(base: u64) -> u64 {
    let n = now_ms().unsigned_abs() % 401;
    base * (800 + n) / 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct TestError;

    impl RetryError for TestError {
        fn retry_kind(&self) -> RetryClass {
            RetryClass::Other
        }
    }

    #[test]
    fn backoff_sequence() {
        assert_eq!(
            [backoff_ms(1), backoff_ms(2), backoff_ms(3)],
            [1000, 2000, 4000]
        );
    }
    #[test]
    fn parses_reset() {
        assert!(parse_reset_at("2s").unwrap() > now_ms());
    }
    #[test]
    fn quota_defaults() {
        let q = RequestGate::new(None).snapshots();
        assert_eq!(q.asr_requests_limit, 2000);
        assert_eq!(q.llm_requests_limit, 1000);
    }

    #[test]
    fn rate_limit_marks_a_future_quota_reset() {
        let gate = RequestGate::new(None);
        gate.mark_rate_limited(RequestKind::Asr, 2.5);
        let quota = gate.snapshots().asr;
        assert_eq!(quota.remaining_requests_rpd, 0);
        assert!(quota
            .reset_requests_at
            .is_some_and(|reset| { reset >= now_ms() + 2_000 && reset <= now_ms() + 3_000 }));
    }

    #[test]
    fn missing_request_reset_header_keeps_low_quota_blocked() {
        let gate = RequestGate::new(None);
        gate.update(RequestKind::Asr, Some("0"), None, None, None);
        let quota = gate.snapshots().asr;
        assert_eq!(quota.remaining_requests_rpd, 0);
        assert!(quota
            .reset_requests_at
            .is_some_and(|reset| reset >= now_ms() + 59_000));
    }

    #[test]
    fn missing_reset_header_does_not_clear_an_existing_future_reset() {
        let gate = RequestGate::new(None);
        gate.mark_rate_limited(RequestKind::Asr, 120.0);
        let before = gate
            .snapshots()
            .asr
            .reset_requests_at
            .expect("rate limit reset");
        gate.update(RequestKind::Asr, Some("0"), None, None, None);
        let after = gate
            .snapshots()
            .asr
            .reset_requests_at
            .expect("reset is retained");
        assert!(after >= before);
    }

    #[tokio::test]
    async fn cancelled_request_never_starts_provider_operation() {
        let gate = RequestGate::new(None);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_in_operation = called.clone();
        let result = execute_with_retry_cancelled(
            &gate,
            RequestKind::Asr,
            move || {
                let called = called_in_operation.clone();
                async move {
                    called.store(true, std::sync::atomic::Ordering::Release);
                    Ok::<_, TestError>(())
                }
            },
            cancellation,
        )
        .await;
        assert!(matches!(result, Err(ExecuteError::Cancelled)));
        assert!(!called.load(std::sync::atomic::Ordering::Acquire));
    }
}
