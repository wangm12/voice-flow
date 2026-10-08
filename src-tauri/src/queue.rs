//! Bounded request queue, retry policy, and API quota tracking.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

const UNKNOWN_QUOTA_RESET_MS: i64 = 60_000;
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// Opaque identity for quota state belonging to one provider destination,
/// model bucket, and account. The endpoint and credential are hashed before
/// being retained, and neither digest is ever included in events or logs.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RequestScope {
    provider: String,
    destination_digest: [u8; 32],
    model_bucket: String,
    account_digest: [u8; 32],
}

impl RequestScope {
    pub fn new(
        provider: &str,
        destination: Option<&str>,
        model_bucket: &str,
        credential: &str,
    ) -> Self {
        Self {
            provider: provider.to_owned(),
            destination_digest: scope_digest(b"destination", provider, destination.unwrap_or("")),
            model_bucket: model_bucket.to_owned(),
            account_digest: scope_digest(b"account", provider, credential),
        }
    }
}

impl Default for RequestScope {
    fn default() -> Self {
        Self::new("legacy", None, "", "")
    }
}

impl fmt::Debug for RequestScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RequestScope")
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

fn scope_digest(domain: &[u8], provider: &str, value: &str) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"voiceflow-quota-scope-v1\0");
    digest.update(domain);
    digest.update([0]);
    digest.update(provider.as_bytes());
    digest.update([0]);
    digest.update(value.as_bytes());
    digest.finalize().into()
}

fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    Asr,
    Llm,
    HistoryLlm,
    WritingPreview,
}
impl RequestKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Asr => "asr",
            Self::Llm => "llm",
            Self::HistoryLlm => "history_llm",
            Self::WritingPreview => "writing_preview",
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
    #[cfg(test)]
    tokens_limit: i64,
}
#[derive(Debug, Clone, Serialize)]
#[cfg(test)]
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
    Server { retry_after: Option<Duration> },
    Unauthorized,
    Other,
}

/// Parse and bound an HTTP `Retry-After` value before it reaches the request
/// queue. Provider headers are untrusted input and must not stall dictation
/// indefinitely. Invalid or absent values use the queue's normal backoff.
pub fn bounded_retry_after(value: Option<&str>) -> Option<Duration> {
    let value = value?.trim();
    if let Ok(seconds) = value.parse::<f64>() {
        if !seconds.is_finite() || seconds < 0.0 {
            return None;
        }
        return Some(Duration::from_secs_f64(
            seconds.min(MAX_RETRY_AFTER.as_secs_f64()),
        ));
    }
    let at = httpdate::parse_http_date(value).ok()?;
    let delay = at.duration_since(SystemTime::now()).unwrap_or_default();
    Some(delay.min(MAX_RETRY_AFTER))
}

/// Rate-limit callers need a small fallback delay when the server omits or
/// malforms the header; valid numeric and HTTP-date values use the bounded
/// provider delay.
pub fn retry_after_seconds(value: Option<&str>) -> f64 {
    bounded_retry_after(value)
        .unwrap_or(Duration::from_secs(1))
        .as_secs_f64()
}

#[derive(Clone)]
pub struct RequestGate {
    semaphore: Arc<Semaphore>,
    quotas: Arc<Mutex<HashMap<RequestScope, (Quota, Quota)>>>,
    app: Option<AppHandle>,
    session_generation: Arc<AtomicU64>,
    #[cfg(test)]
    emitted_events: Arc<Mutex<Vec<String>>>,
}
impl RequestGate {
    pub fn new(app: Option<AppHandle>) -> Self {
        let mut quotas = HashMap::new();
        quotas.insert(RequestScope::default(), (default_asr(), default_llm()));
        Self {
            semaphore: Arc::new(Semaphore::new(2)),
            quotas: Arc::new(Mutex::new(quotas)),
            app,
            session_generation: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            emitted_events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn set_session_generation(&self, generation: u64) {
        self.session_generation.store(generation, Ordering::Release);
    }
    #[cfg(test)]
    pub fn snapshots(&self) -> QuotaView {
        self.snapshots_for_scope(&RequestScope::default())
    }
    #[cfg(test)]
    pub fn snapshots_for_scope(&self, scope: &RequestScope) -> QuotaView {
        let q = lock_recover(&self.quotas);
        let Some((asr, llm)) = q.get(scope) else {
            let asr = default_asr();
            let llm = default_llm();
            return quota_view(&asr, &llm);
        };
        quota_view(asr, llm)
    }
    pub fn update_asr_for(&self, scope: &RequestScope, limits: &crate::asr::RateLimits) {
        self.update(
            scope,
            RequestKind::Asr,
            limits.requests.as_deref(),
            limits.tokens.as_deref(),
            limits.reset_requests.as_deref(),
            limits.reset_tokens.as_deref(),
        );
    }
    pub fn update_llm_for(&self, scope: &RequestScope, limits: &crate::llm::RateLimits) {
        self.update(
            scope,
            RequestKind::Llm,
            limits.requests.as_deref(),
            limits.tokens.as_deref(),
            limits.reset_requests.as_deref(),
            limits.reset_tokens.as_deref(),
        );
    }
    fn update(
        &self,
        scope: &RequestScope,
        kind: RequestKind,
        requests: Option<&str>,
        tokens: Option<&str>,
        reset_requests: Option<&str>,
        reset_tokens: Option<&str>,
    ) {
        let mut scopes = lock_recover(&self.quotas);
        let q = scopes
            .entry(scope.clone())
            .or_insert_with(|| (default_asr(), default_llm()));
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
    #[cfg(test)]
    pub fn mark_rate_limited(&self, kind: RequestKind, retry_after_secs: f64) {
        self.mark_rate_limited_for(&RequestScope::default(), kind, retry_after_secs);
    }
    fn mark_rate_limited_for(
        &self,
        scope: &RequestScope,
        kind: RequestKind,
        retry_after_secs: f64,
    ) {
        let mut scopes = lock_recover(&self.quotas);
        let q = scopes
            .entry(scope.clone())
            .or_insert_with(|| (default_asr(), default_llm()));
        let quota = if kind == RequestKind::Asr {
            &mut q.0
        } else {
            &mut q.1
        };
        quota.snapshot.remaining_requests_rpd = 0;
        let wait_ms = (retry_after_secs.max(0.0) * 1000.0).ceil() as i64;
        quota.snapshot.reset_requests_at = Some(now_ms().saturating_add(wait_ms));
    }
    async fn wait_for_quota(
        &self,
        scope: &RequestScope,
        kind: RequestKind,
        cancellation: &CancellationToken,
    ) -> bool {
        loop {
            let until = {
                let q = lock_recover(&self.quotas);
                let quota = q.get(scope).map(|pair| {
                    if kind == RequestKind::Asr {
                        &pair.0
                    } else {
                        &pair.1
                    }
                });
                if let Some(quota) = quota.filter(|q| q.snapshot.remaining_requests_rpd < 2) {
                    quota.snapshot.reset_requests_at
                } else {
                    None
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
    fn quota_is_available(&self, scope: &RequestScope, kind: RequestKind) -> bool {
        let scopes = lock_recover(&self.quotas);
        scopes.get(scope).is_none_or(|pair| {
            let quota = if kind == RequestKind::Asr {
                &pair.0
            } else {
                &pair.1
            };
            quota.snapshot.remaining_requests_rpd >= 2
                || quota
                    .snapshot
                    .reset_requests_at
                    .is_none_or(|reset| reset <= now_ms())
        })
    }
    fn emit(&self, event: &str, payload: serde_json::Value) {
        #[cfg(test)]
        lock_recover(&self.emitted_events).push(event.into());
        if let Some(app) = &self.app {
            let _ = app.emit(event, payload);
        }
    }

    fn current_session_generation(&self) -> u64 {
        self.session_generation.load(Ordering::Acquire)
    }
}

#[cfg(test)]
fn quota_view(asr: &Quota, llm: &Quota) -> QuotaView {
    QuotaView {
        asr: asr.snapshot.clone(),
        llm: llm.snapshot.clone(),
        asr_rpm_limit: 20,
        asr_requests_limit: asr.requests_limit,
        asr_audio_hours_limit: 7200,
        llm_rpm_limit: 30,
        llm_requests_limit: llm.requests_limit,
        llm_tokens_limit: llm.tokens_limit,
    }
}

#[derive(Debug)]
pub enum ExecuteError<E> {
    Operation(E),
    Cancelled,
}

impl<E: std::fmt::Display> std::fmt::Display for ExecuteError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Operation(error) => error.fmt(formatter),
            Self::Cancelled => formatter.write_str("request cancelled"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for ExecuteError<E> {}

#[cfg(test)]
pub async fn execute_with_retry<F, Fut, T, E>(
    gate: &RequestGate,
    kind: RequestKind,
    operation: F,
) -> Result<T, ExecuteError<E>>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: RetryError,
{
    execute_with_retry_cancelled(gate, kind, operation, CancellationToken::new()).await
}

#[cfg(test)]
pub async fn execute_with_retry_cancelled<F, Fut, T, E>(
    gate: &RequestGate,
    kind: RequestKind,
    operation: F,
    cancellation: CancellationToken,
) -> Result<T, ExecuteError<E>>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: RetryError,
{
    execute_with_retry_scoped_cancelled(
        gate,
        kind,
        &RequestScope::default(),
        operation,
        cancellation,
    )
    .await
}

pub async fn execute_with_retry_scoped_cancelled<F, Fut, T, E>(
    gate: &RequestGate,
    kind: RequestKind,
    scope: &RequestScope,
    operation: F,
    cancellation: CancellationToken,
) -> Result<T, ExecuteError<E>>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: RetryError,
{
    execute_with_retry_scoped_cancelled_checked(
        gate,
        kind,
        scope,
        operation,
        || std::future::ready(true),
        cancellation,
    )
    .await
    .map_err(|error| match error {
        CheckedExecuteError::Operation(error) => ExecuteError::Operation(error),
        CheckedExecuteError::Cancelled => ExecuteError::Cancelled,
        CheckedExecuteError::AuthorizationChanged => {
            unreachable!("unconditionally authorized requests cannot be rejected")
        }
    })
}

#[derive(Debug)]
pub enum CheckedExecuteError<E> {
    Operation(E),
    Cancelled,
    AuthorizationChanged,
}

impl<E: std::fmt::Display> std::fmt::Display for CheckedExecuteError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Operation(error) => error.fmt(formatter),
            Self::Cancelled => formatter.write_str("request cancelled"),
            Self::AuthorizationChanged => formatter.write_str("request authorization changed"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for CheckedExecuteError<E> {}

pub async fn execute_with_retry_scoped_cancelled_checked<F, Fut, Check, CheckFut, T, E>(
    gate: &RequestGate,
    kind: RequestKind,
    scope: &RequestScope,
    mut operation: F,
    mut authorized: Check,
    cancellation: CancellationToken,
) -> Result<T, CheckedExecuteError<E>>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    Check: FnMut() -> CheckFut,
    CheckFut: Future<Output = bool>,
    E: RetryError,
{
    let mut rate_retries = 0;
    let mut transient_retries = 0;
    loop {
        if !gate.wait_for_quota(scope, kind, &cancellation).await {
            return Err(CheckedExecuteError::Cancelled);
        }
        if cancellation.is_cancelled() {
            return Err(CheckedExecuteError::Cancelled);
        }
        let _permit = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(CheckedExecuteError::Cancelled),
            permit = gate.semaphore.acquire() => permit.expect("request semaphore is alive")
        };
        if cancellation.is_cancelled() {
            return Err(CheckedExecuteError::Cancelled);
        }
        if !gate.quota_is_available(scope, kind) {
            drop(_permit);
            continue;
        }
        let is_authorized = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(CheckedExecuteError::Cancelled),
            authorized = authorized() => authorized,
        };
        if !is_authorized {
            return Err(CheckedExecuteError::AuthorizationChanged);
        }
        if cancellation.is_cancelled() {
            return Err(CheckedExecuteError::Cancelled);
        }
        match tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(CheckedExecuteError::Cancelled),
            result = operation() => result
        } {
            Ok(value) => return Ok(value),
            Err(error) => match error.retry_kind() {
                RetryClass::Unauthorized | RetryClass::Other => {
                    return Err(CheckedExecuteError::Operation(error))
                }
                RetryClass::RateLimited(seconds) if rate_retries < 2 => {
                    // A retry wait must not occupy one of the shared provider
                    // slots; unrelated destinations still need to progress.
                    drop(_permit);
                    rate_retries += 1;
                    let seconds = seconds.clamp(0.0, MAX_RETRY_AFTER.as_secs_f64());
                    // Sync the quota view so `wait_for_quota` and the UI reflect
                    // that we've hit the rate limit.
                    gate.mark_rate_limited_for(scope, kind, seconds);
                    let (rate_event, retry_event) = match kind {
                        RequestKind::HistoryLlm => ("history://rate_limited", "history://retrying"),
                        RequestKind::WritingPreview => (
                            "writing-preview://rate_limited",
                            "writing-preview://retrying",
                        ),
                        RequestKind::Asr | RequestKind::Llm => {
                            ("quota://rate_limited", "quota://retrying")
                        }
                    };
                    gate.emit(
                        rate_event,
                        serde_json::json!({
                            "retry_after_secs": seconds,
                            "session_generation": gate.current_session_generation(),
                        }),
                    );
                    if matches!(kind, RequestKind::Asr | RequestKind::Llm) {
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
                    gate.emit(retry_event, serde_json::json!({"attempt": rate_retries, "wait_ms": wait_ms, "kind": kind.name(), "session_generation": gate.current_session_generation()}));
                    tokio::select! { _ = cancellation.cancelled() => return Err(CheckedExecuteError::Cancelled), _ = tokio::time::sleep(Duration::from_millis(wait_ms)) => {} }
                }
                RetryClass::Network | RetryClass::Server { .. } if transient_retries < 3 => {
                    drop(_permit);
                    transient_retries += 1;
                    let retry_after = match error.retry_kind() {
                        RetryClass::Server { retry_after } => retry_after,
                        _ => None,
                    };
                    let wait = retry_after
                        .map(|delay| delay.min(MAX_RETRY_AFTER).as_millis() as u64)
                        .unwrap_or_else(|| {
                            let base = 1u64 << (transient_retries - 1);
                            jittered_ms(base * 1000)
                        });
                    let retry_event = if kind == RequestKind::WritingPreview {
                        "writing-preview://retrying"
                    } else {
                        "quota://retrying"
                    };
                    gate.emit(retry_event, serde_json::json!({"attempt": transient_retries, "wait_ms": wait, "kind": kind.name(), "session_generation": gate.current_session_generation()}));
                    tokio::select! { _ = cancellation.cancelled() => return Err(CheckedExecuteError::Cancelled), _ = tokio::time::sleep(Duration::from_millis(wait)) => {} }
                }
                _ => return Err(CheckedExecuteError::Operation(error)),
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
        #[cfg(test)]
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
        #[cfg(test)]
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
    use std::sync::atomic::AtomicUsize;

    #[derive(Debug)]
    struct TestError;

    impl RetryError for TestError {
        fn retry_kind(&self) -> RetryClass {
            RetryClass::Other
        }
    }

    #[derive(Debug, Clone)]
    struct RetryableError(RetryClass);

    impl RetryError for RetryableError {
        fn retry_kind(&self) -> RetryClass {
            self.0.clone()
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
    fn retry_after_accepts_seconds_and_http_dates_and_is_bounded() {
        assert_eq!(
            bounded_retry_after(Some("2.5")),
            Some(Duration::from_millis(2500))
        );
        assert_eq!(
            bounded_retry_after(Some("500")),
            Some(Duration::from_secs(60))
        );
        assert_eq!(bounded_retry_after(Some("-1")), None);
        assert_eq!(retry_after_seconds(None), 1.0);

        let future = SystemTime::now() + Duration::from_secs(25);
        let header = httpdate::fmt_http_date(future);
        let delay = bounded_retry_after(Some(&header)).expect("HTTP-date delay");
        assert!(delay <= Duration::from_secs(25));
        assert!(delay >= Duration::from_secs(23));
    }
    #[test]
    fn quota_defaults() {
        let q = RequestGate::new(None).snapshots();
        assert_eq!(q.asr_requests_limit, 2000);
        assert_eq!(q.llm_requests_limit, 1000);
    }

    #[tokio::test]
    async fn writing_trial_retries_share_llm_quota_without_changing_the_live_hud() {
        let gate = RequestGate::new(None);
        let scope = RequestScope::new(
            "test",
            Some("https://test.invalid/v1"),
            "model",
            "synthetic",
        );
        gate.mark_rate_limited_for(&scope, RequestKind::WritingPreview, 60.0);
        assert!(!gate.quota_is_available(&scope, RequestKind::Llm));
        assert!(gate.quota_is_available(&scope, RequestKind::Asr));
        gate.mark_rate_limited_for(&scope, RequestKind::WritingPreview, 0.0);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let result = execute_with_retry_scoped_cancelled(
            &gate,
            RequestKind::WritingPreview,
            &scope,
            || async {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(crate::llm::LlmError::RateLimited("0".into()))
                } else {
                    Ok("trial result")
                }
            },
            CancellationToken::new(),
        )
        .await;
        assert_eq!(result.unwrap(), "trial result");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let events = lock_recover(&gate.emitted_events);
        assert!(events
            .iter()
            .any(|event| event == "writing-preview://rate_limited"));
        assert!(events
            .iter()
            .any(|event| event == "writing-preview://retrying"));
        assert!(!events.iter().any(|event| event.starts_with("dictation://")
            || event.starts_with("quota://")
            || event.starts_with("history://")));
    }

    #[tokio::test]
    async fn quota_waits_are_isolated_by_provider_destination_and_account() {
        let gate = RequestGate::new(None);
        let provider_a = RequestScope::new(
            "provider-a",
            Some("https://a.invalid/v1"),
            "model-a",
            "key-a",
        );
        let provider_b = RequestScope::new(
            "provider-b",
            Some("https://b.invalid/v1"),
            "model-a",
            "key-b",
        );
        let provider_a_other_endpoint = RequestScope::new(
            "provider-a",
            Some("https://a2.invalid/v1"),
            "model-a",
            "key-a",
        );
        let provider_a_other_key = RequestScope::new(
            "provider-a",
            Some("https://a.invalid/v1"),
            "model-a",
            "key-a2",
        );
        // Prompt/context/options are deliberately not inputs to the quota key.
        let provider_a_other_context = RequestScope::new(
            "provider-a",
            Some("https://a.invalid/v1"),
            "model-a",
            "key-a",
        );
        let provider_a_other_model = RequestScope::new(
            "provider-a",
            Some("https://a.invalid/v1"),
            "model-b",
            "key-a",
        );
        gate.mark_rate_limited_for(&provider_a, RequestKind::Asr, 60.0);

        assert_ne!(provider_a, provider_b);
        assert_ne!(provider_a, provider_a_other_endpoint);
        assert_ne!(provider_a, provider_a_other_key);
        assert_ne!(provider_a, provider_a_other_model);
        assert_eq!(provider_a, provider_a_other_context);
        let debug = format!("{provider_a:?}");
        assert!(!debug.contains("https://a.invalid"));
        assert!(!debug.contains("key-a"));

        let b_result = tokio::time::timeout(
            Duration::from_millis(100),
            execute_with_retry_scoped_cancelled(
                &gate,
                RequestKind::Asr,
                &provider_b,
                || async { Ok::<_, TestError>("backup") },
                CancellationToken::new(),
            ),
        )
        .await
        .expect("unrelated provider is not held by A's quota");
        assert!(matches!(b_result, Ok("backup")));

        let endpoint_result = execute_with_retry_scoped_cancelled(
            &gate,
            RequestKind::Asr,
            &provider_a_other_endpoint,
            || async { Ok::<_, TestError>("endpoint") },
            CancellationToken::new(),
        )
        .await;
        let key_result = execute_with_retry_scoped_cancelled(
            &gate,
            RequestKind::Asr,
            &provider_a_other_key,
            || async { Ok::<_, TestError>("account") },
            CancellationToken::new(),
        )
        .await;
        assert!(matches!(endpoint_result, Ok("endpoint")));
        assert!(matches!(key_result, Ok("account")));

        let other_model_result = execute_with_retry_scoped_cancelled(
            &gate,
            RequestKind::Asr,
            &provider_a_other_model,
            || async { Ok::<_, TestError>("other-model") },
            CancellationToken::new(),
        )
        .await;
        assert!(matches!(other_model_result, Ok("other-model")));

        let cancellation = CancellationToken::new();
        let cancellation_for_request = cancellation.clone();
        let gate_for_request = gate.clone();
        let scope_for_request = provider_a.clone();
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_in_request = Arc::clone(&called);
        let waiting = tokio::spawn(async move {
            execute_with_retry_scoped_cancelled(
                &gate_for_request,
                RequestKind::Asr,
                &scope_for_request,
                move || {
                    let called = Arc::clone(&called_in_request);
                    async move {
                        called.store(true, Ordering::Release);
                        Ok::<_, TestError>(())
                    }
                },
                cancellation_for_request,
            )
            .await
        });
        tokio::task::yield_now().await;
        cancellation.cancel();
        assert!(matches!(
            waiting.await.unwrap(),
            Err(ExecuteError::Cancelled)
        ));
        assert!(!called.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn retry_backoff_releases_shared_request_permits_for_other_scopes() {
        for (index, retry_class) in [
            RetryClass::RateLimited(0.2),
            RetryClass::Server {
                retry_after: Some(Duration::from_millis(200)),
            },
        ]
        .into_iter()
        .enumerate()
        {
            let gate = RequestGate::new(None);
            let provider_a = RequestScope::new("provider-a", None, "model-a", "key-a");
            let provider_b = RequestScope::new("provider-b", None, "model-b", "key-b");
            let both_a_requests = Arc::new(tokio::sync::Barrier::new(2));
            let a_calls = Arc::new(AtomicUsize::new(0));
            let cancel_a = CancellationToken::new();
            let cancel_b = CancellationToken::new();
            let mut tasks = Vec::new();

            for cancellation in [cancel_a.clone(), cancel_b.clone()] {
                let gate = gate.clone();
                let scope = provider_a.clone();
                let barrier = Arc::clone(&both_a_requests);
                let calls = Arc::clone(&a_calls);
                let retry_class = retry_class.clone();
                let first_attempt = Arc::new(std::sync::atomic::AtomicBool::new(true));
                tasks.push(tokio::spawn(async move {
                    execute_with_retry_scoped_cancelled(
                        &gate,
                        RequestKind::Asr,
                        &scope,
                        move || {
                            let barrier = Arc::clone(&barrier);
                            let calls = Arc::clone(&calls);
                            let retry_class = retry_class.clone();
                            let first_attempt = Arc::clone(&first_attempt);
                            async move {
                                calls.fetch_add(1, Ordering::AcqRel);
                                if first_attempt.swap(false, Ordering::AcqRel) {
                                    barrier.wait().await;
                                    Err(RetryableError(retry_class))
                                } else {
                                    Ok(())
                                }
                            }
                        },
                        cancellation,
                    )
                    .await
                }));
            }

            tokio::time::timeout(Duration::from_millis(100), async {
                while a_calls.load(Ordering::Acquire) < 2 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("both same-scope provider requests entered before returning retryable errors");
            tokio::time::sleep(Duration::from_millis(5)).await;

            let provider_b_result = tokio::time::timeout(
                Duration::from_millis(75),
                execute_with_retry_scoped_cancelled(
                    &gate,
                    RequestKind::Asr,
                    &provider_b,
                    || async { Ok::<_, TestError>("unrelated provider completed") },
                    CancellationToken::new(),
                ),
            )
            .await
            .expect("A's retry backoff does not hold both global request slots");
            assert!(matches!(
                provider_b_result,
                Ok("unrelated provider completed")
            ));

            cancel_a.cancel();
            cancel_b.cancel();
            for task in tasks {
                assert!(matches!(task.await.unwrap(), Err(ExecuteError::Cancelled)));
            }
            assert_eq!(a_calls.load(Ordering::Acquire), 2, "scenario {index}");
        }
    }

    #[tokio::test]
    async fn checked_request_rejects_after_initial_quota_wait_without_sending() {
        let gate = RequestGate::new(None);
        let scope = RequestScope::new("provider", Some("https://provider.invalid"), "model", "key");
        gate.mark_rate_limited_for(&scope, RequestKind::Llm, 0.05);
        let authorized = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let authorized_for_check = Arc::clone(&authorized);
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in_operation = Arc::clone(&calls);
        let request_gate = gate.clone();
        let request_scope = scope.clone();
        let request = tokio::spawn(async move {
            execute_with_retry_scoped_cancelled_checked(
                &request_gate,
                RequestKind::Llm,
                &request_scope,
                move || {
                    let calls = Arc::clone(&calls_in_operation);
                    async move {
                        calls.fetch_add(1, Ordering::AcqRel);
                        Ok::<_, TestError>(())
                    }
                },
                move || {
                    let authorized = Arc::clone(&authorized_for_check);
                    async move { authorized.load(Ordering::Acquire) }
                },
                CancellationToken::new(),
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(2)).await;
        // The request is now held by its known quota reset. Revoke before the
        // wait completes; authorization must be checked after it wakes.
        authorized.store(false, Ordering::Release);
        let result = request.await.unwrap();
        assert!(matches!(
            result,
            Err(CheckedExecuteError::AuthorizationChanged)
        ));
        assert_eq!(calls.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn checked_request_does_not_retry_after_revocation_and_regrant() {
        let gate = RequestGate::new(None);
        let scope = RequestScope::new("provider", Some("https://provider.invalid"), "model", "key");
        let captured_revision = 7_u64;
        let current_revision = Arc::new(AtomicU64::new(captured_revision));
        let request_count = Arc::new(AtomicUsize::new(0));
        let request_count_in_operation = Arc::clone(&request_count);
        let revision_in_operation = Arc::clone(&current_revision);
        let revision_in_check = Arc::clone(&current_revision);
        let result = execute_with_retry_scoped_cancelled_checked(
            &gate,
            RequestKind::Llm,
            &scope,
            move || {
                let count = Arc::clone(&request_count_in_operation);
                let revision = Arc::clone(&revision_in_operation);
                async move {
                    if count.fetch_add(1, Ordering::AcqRel) == 0 {
                        // Revocation and a later re-grant both advance the
                        // monotonic policy revision. Grants may be true again,
                        // but this request's captured revision stays stale.
                        revision.store(captured_revision + 2, Ordering::Release);
                        Err(RetryableError(RetryClass::RateLimited(0.01)))
                    } else {
                        Ok("must not be sent")
                    }
                }
            },
            move || {
                let revision = Arc::clone(&revision_in_check);
                async move { revision.load(Ordering::Acquire) == captured_revision }
            },
            CancellationToken::new(),
        )
        .await;
        assert!(matches!(
            result,
            Err(CheckedExecuteError::AuthorizationChanged)
        ));
        assert_eq!(request_count.load(Ordering::Acquire), 1);
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
        gate.update(
            &RequestScope::default(),
            RequestKind::Asr,
            Some("0"),
            None,
            None,
            None,
        );
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
        gate.update(
            &RequestScope::default(),
            RequestKind::Asr,
            Some("0"),
            None,
            None,
            None,
        );
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

    #[tokio::test]
    async fn non_cancelled_retry_api_returns_a_typed_operation_error() {
        let gate = RequestGate::new(None);
        let result = execute_with_retry(&gate, RequestKind::Asr, || async {
            Err::<(), _>(TestError)
        })
        .await;
        assert!(matches!(result, Err(ExecuteError::Operation(TestError))));
    }

    #[test]
    fn quota_reads_recover_after_mutex_poisoning() {
        let gate = RequestGate::new(None);
        let quotas = gate.quotas.clone();
        let _ = std::thread::spawn(move || {
            let _guard = quotas.lock().expect("initial quota lock");
            panic!("poison quota mutex");
        })
        .join();

        assert_eq!(gate.snapshots().asr_requests_limit, 2000);
    }
}
