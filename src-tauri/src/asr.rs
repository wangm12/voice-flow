use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::time::Duration;
use thiserror::Error;

pub const MODEL: &str = "whisper-large-v3-turbo";

#[derive(Debug, Clone, Default)]
pub struct RateLimits {
    pub requests: Option<String>,
    pub tokens: Option<String>,
    pub reset_requests: Option<String>,
    pub reset_tokens: Option<String>,
    pub retry_after: Option<String>,
}
#[derive(Debug, Error, Clone)]
pub enum AsrError {
    #[error("network error: {0}")]
    Network(String),
    #[error("request timed out")]
    Timeout,
    #[error("Groq authorization failed")]
    Unauthorized,
    #[error("rate limited{0}")]
    RateLimited(String),
    #[error("server error: {0}")]
    Server(String),
    #[error("empty speech result")]
    EmptyResult,
    #[error("Groq error: {0}")]
    Other(String),
}

#[derive(Debug, Clone, Default)]
pub struct AsrOptions {
    pub api_key: String,
    pub language: Option<String>,
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub struct AsrCapabilities {
    pub batch_transcription: bool,
    pub background_prefetch: bool,
    pub realtime_streaming: bool,
    pub cancellation: bool,
    pub word_timestamps: bool,
}

pub type AsrFuture = Pin<Box<dyn Future<Output = Result<Transcript, AsrError>> + Send>>;

/// Internal seam for ASR providers. Groq currently accepts completed audio
/// uploads, so prefetching is silent background work rather than a partial
/// transcript stream.
pub trait AsrProvider: Send + Sync {
    fn transcribe_batch(&self, audio: Vec<u8>, options: AsrOptions) -> AsrFuture;

    fn prefetch_chunk(&self, audio: Vec<u8>, options: AsrOptions) -> AsrFuture {
        self.transcribe_batch(audio, options)
    }

    #[allow(dead_code)]
    fn capabilities(&self) -> AsrCapabilities;
}
#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct Segment {
    pub text: String,
    pub avg_logprob: Option<f32>,
    pub no_speech_prob: Option<f32>,
}
#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct Word {
    pub word: String,
    pub start: Option<f32>,
    pub end: Option<f32>,
}
#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct Transcript {
    pub text: String,
    #[serde(default)]
    pub segments: Vec<Segment>,
    #[serde(default)]
    pub words: Vec<Word>,
    #[serde(skip)]
    pub limits: RateLimits,
}

#[derive(Debug, Clone)]
pub struct GroqAsrProvider {
    endpoint: String,
}

impl Default for GroqAsrProvider {
    fn default() -> Self {
        Self {
            endpoint: "https://api.groq.com/openai/v1/audio/transcriptions".into(),
        }
    }
}

impl GroqAsrProvider {
    #[cfg(test)]
    fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
        }
    }
}

impl AsrProvider for GroqAsrProvider {
    fn transcribe_batch(&self, audio: Vec<u8>, options: AsrOptions) -> AsrFuture {
        let endpoint = self.endpoint.clone();
        Box::pin(async move {
            transcribe_at(
                &endpoint,
                audio,
                &options.api_key,
                options.language.as_deref(),
                options.prompt.as_deref(),
            )
            .await
        })
    }

    fn capabilities(&self) -> AsrCapabilities {
        AsrCapabilities {
            batch_transcription: true,
            background_prefetch: true,
            realtime_streaming: false,
            cancellation: true,
            word_timestamps: true,
        }
    }
}

#[cfg(test)]
#[derive(Clone)]
pub struct MockAsrProvider {
    response: std::sync::Arc<std::sync::Mutex<Result<Transcript, AsrError>>>,
    delay: Duration,
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl MockAsrProvider {
    pub fn new(response: Result<Transcript, AsrError>, delay: Duration) -> Self {
        Self {
            response: std::sync::Arc::new(std::sync::Mutex::new(response)),
            delay,
            calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
impl AsrProvider for MockAsrProvider {
    fn transcribe_batch(&self, _audio: Vec<u8>, _options: AsrOptions) -> AsrFuture {
        let response = self
            .response
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let delay = self.delay;
        let calls = self.calls.clone();
        Box::pin(async move {
            calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            response
        })
    }

    fn capabilities(&self) -> AsrCapabilities {
        AsrCapabilities {
            batch_transcription: true,
            background_prefetch: true,
            realtime_streaming: false,
            cancellation: true,
            word_timestamps: false,
        }
    }
}

pub fn parse_rate_limits(h: &reqwest::header::HeaderMap) -> RateLimits {
    let get = |n| h.get(n).and_then(|v| v.to_str().ok()).map(str::to_owned);
    RateLimits {
        requests: get("x-ratelimit-remaining-requests"),
        tokens: get("x-ratelimit-remaining-tokens"),
        reset_requests: get("x-ratelimit-reset-requests"),
        reset_tokens: get("x-ratelimit-reset-tokens"),
        retry_after: get("retry-after"),
    }
}
#[allow(dead_code)]
pub async fn transcribe(
    wav: Vec<u8>,
    key: &str,
    language: Option<&str>,
    prompt: Option<&str>,
) -> Result<Transcript, AsrError> {
    GroqAsrProvider::default()
        .transcribe_batch(
            wav,
            AsrOptions {
                api_key: key.to_owned(),
                language: language.map(str::to_owned),
                prompt: prompt.map(str::to_owned),
            },
        )
        .await
}

async fn transcribe_at(
    endpoint: &str,
    wav: Vec<u8>,
    key: &str,
    language: Option<&str>,
    prompt: Option<&str>,
) -> Result<Transcript, AsrError> {
    let client = http_client()?;
    let mut form = Form::new()
        .part("file", Part::bytes(wav).file_name("audio.wav"))
        .text("model", MODEL)
        .text("response_format", "verbose_json")
        .text("timestamp_granularities[]", "word")
        .text("timestamp_granularities[]", "segment");
    if let Some(v) = normalize_language(language) {
        form = form.text("language", v.to_owned());
    }
    if let Some(v) = prompt.filter(|v| !v.is_empty()) {
        form = form.text("prompt", v.to_owned());
    }
    let response = client
        .post(endpoint)
        .bearer_auth(key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                AsrError::Timeout
            } else {
                AsrError::Network(e.to_string())
            }
        })?;
    let limits = parse_rate_limits(response.headers());
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(AsrError::Unauthorized);
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(AsrError::RateLimited(
            limits.retry_after.unwrap_or_default(),
        ));
    }
    if status.is_server_error() {
        return Err(AsrError::Server(format!("HTTP status {status}")));
    }
    if !status.is_success() {
        return Err(AsrError::Other(format!("HTTP status {status}")));
    }
    let mut result: Transcript = response
        .json()
        .await
        .map_err(|e| AsrError::Other(e.to_string()))?;
    result.limits = limits;
    if result.text.trim().is_empty() || is_silence(&result.segments) {
        return Err(AsrError::EmptyResult);
    }
    Ok(result)
}

fn http_client() -> Result<&'static reqwest::Client, AsrError> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    match CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| error.to_string())
    }) {
        Ok(client) => Ok(client),
        Err(error) => Err(AsrError::Network(error.clone())),
    }
}

/// Provider APIs interpret an omitted language as automatic detection. The UI
/// uses the explicit `auto` value, which must never be sent as a language code.
pub fn normalize_language(language: Option<&str>) -> Option<&str> {
    language
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("auto"))
}

/// Treat the transcript as silence only when the overwhelming majority of
/// segments are flagged as non-speech. A single quiet segment in an otherwise
/// normal dictation must not fail the whole recording.
fn is_silence(segments: &[Segment]) -> bool {
    if segments.is_empty() {
        return false;
    }
    let non_speech = segments
        .iter()
        .filter(|s| s.no_speech_prob.unwrap_or(0.0) > 0.9)
        .count();
    // Silence only if every segment is non-speech, or >80% are (long dictation
    // with a few pauses still succeeds).
    non_speech == segments.len() || (non_speech as f32 / segments.len() as f32) > 0.8
}
impl crate::queue::RetryError for AsrError {
    fn retry_kind(&self) -> crate::queue::RetryClass {
        match self {
            Self::RateLimited(v) => crate::queue::RetryClass::RateLimited(parse_retry_after(v)),
            Self::Network(_) | Self::Timeout => crate::queue::RetryClass::Network,
            Self::Server(_) => crate::queue::RetryClass::Server,
            Self::Unauthorized => crate::queue::RetryClass::Unauthorized,
            Self::Other(_) | Self::EmptyResult => crate::queue::RetryClass::Other,
        }
    }
}

/// Parse the `Retry-After` header value. Accepts whole or fractional seconds
/// (e.g. "3", "3.5"); falls back to 1s on anything unexpected.
pub fn parse_retry_after(value: &str) -> f64 {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return 1.0;
    }
    trimmed
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(1.0)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_headers() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("retry-after", "3".parse().unwrap());
        h.insert("x-ratelimit-reset-tokens", "1s".parse().unwrap());
        assert_eq!(parse_rate_limits(&h).retry_after.as_deref(), Some("3"));
        assert_eq!(parse_rate_limits(&h).reset_tokens.as_deref(), Some("1s"));
    }
    #[test]
    fn parses_retry_after_fractional_and_fallback() {
        assert_eq!(parse_retry_after("3"), 3.0);
        assert_eq!(parse_retry_after("3.5"), 3.5);
        assert_eq!(parse_retry_after(""), 1.0);
        assert_eq!(parse_retry_after("not-a-number"), 1.0);
        assert_eq!(parse_retry_after("-2"), 1.0);
    }

    #[test]
    fn auto_language_is_omitted() {
        assert_eq!(normalize_language(Some("auto")), None);
        assert_eq!(normalize_language(Some(" AUTO ")), None);
        assert_eq!(normalize_language(Some("zh")), Some("zh"));
        assert_eq!(normalize_language(None), None);
    }

    #[tokio::test]
    async fn provider_success_and_empty_response_are_safe() {
        let endpoint = crate::test_http::spawn_response(
            200,
            "application/json",
            br#"{"text":"hello world","segments":[],"words":[]}"#.to_vec(),
            &[],
        )
        .await;
        let result = transcribe_at(&endpoint, b"wav".to_vec(), "test-key", Some("auto"), None)
            .await
            .expect("ASR success");
        assert_eq!(result.text, "hello world");

        let empty = crate::test_http::spawn_response(
            200,
            "application/json",
            br#"{"text":"  ","segments":[],"words":[]}"#.to_vec(),
            &[],
        )
        .await;
        assert!(matches!(
            transcribe_at(&empty, b"wav".to_vec(), "test-key", None, None).await,
            Err(AsrError::EmptyResult)
        ));
    }

    #[tokio::test]
    async fn provider_auth_rate_limit_and_server_errors_are_classified() {
        let unauthorized =
            crate::test_http::spawn_response(401, "application/json", b"{}", &[]).await;
        assert!(matches!(
            transcribe_at(&unauthorized, b"wav".to_vec(), "bad-key", None, None).await,
            Err(AsrError::Unauthorized)
        ));

        let limited = crate::test_http::spawn_response(
            429,
            "application/json",
            b"{}",
            &[("retry-after", "2")],
        )
        .await;
        assert!(matches!(
            transcribe_at(&limited, b"wav".to_vec(), "test-key", None, None).await,
            Err(AsrError::RateLimited(value)) if value == "2"
        ));

        let server = crate::test_http::spawn_response(500, "application/json", b"{}", &[]).await;
        assert!(matches!(
            transcribe_at(&server, b"wav".to_vec(), "test-key", None, None).await,
            Err(AsrError::Server(_))
        ));

        let invalid =
            crate::test_http::spawn_response(200, "application/json", b"not-json".to_vec(), &[])
                .await;
        assert!(matches!(
            transcribe_at(&invalid, b"wav".to_vec(), "test-key", None, None).await,
            Err(AsrError::Other(_))
        ));

        assert!(matches!(
            transcribe_at(
                "http://127.0.0.1:1/unreachable",
                b"wav".to_vec(),
                "test-key",
                None,
                None,
            )
            .await,
            Err(AsrError::Network(_))
        ));
    }

    #[tokio::test]
    async fn groq_provider_exposes_batch_and_prefetch_capabilities() {
        let endpoint = crate::test_http::spawn_response(
            200,
            "application/json",
            br#"{"text":"provider text","segments":[],"words":[]}"#.to_vec(),
            &[],
        )
        .await;
        let provider = GroqAsrProvider::with_endpoint(endpoint);
        let result = provider
            .transcribe_batch(
                b"wav".to_vec(),
                AsrOptions {
                    api_key: "test-key".into(),
                    ..AsrOptions::default()
                },
            )
            .await
            .expect("provider request should succeed");
        assert_eq!(result.text, "provider text");
        assert!(provider.capabilities().batch_transcription);
        assert!(provider.capabilities().background_prefetch);
        assert!(!provider.capabilities().realtime_streaming);
    }

    #[tokio::test]
    async fn mock_provider_can_model_empty_and_rate_limited_results() {
        let empty = MockAsrProvider::new(Err(AsrError::EmptyResult), Duration::ZERO);
        assert!(matches!(
            empty
                .transcribe_batch(b"wav".to_vec(), AsrOptions::default())
                .await,
            Err(AsrError::EmptyResult)
        ));
        assert_eq!(empty.calls(), 1);

        let limited = MockAsrProvider::new(
            Err(AsrError::RateLimited("2".into())),
            Duration::from_millis(1),
        );
        assert!(matches!(
            limited
                .prefetch_chunk(b"wav".to_vec(), AsrOptions::default())
                .await,
            Err(AsrError::RateLimited(value)) if value == "2"
        ));
    }
}
