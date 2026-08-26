use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::time::Duration;
use thiserror::Error;

pub const MODEL: &str = "whisper-large-v3-turbo";
pub const DEFAULT_ASR_BASE_URL: &str = "https://api.groq.com/openai/v1";
pub const GROQ_ASR_MODELS: &[&str] = &[
    "whisper-large-v3-turbo",
    "whisper-large-v3",
    "distil-whisper-large-v3-en",
];

pub fn is_groq_asr_model(model: &str) -> bool {
    GROQ_ASR_MODELS.contains(&model.trim())
}

/// Resolve an OpenAI-compatible path from a user-supplied base.
/// Empty values use `default_base`. A value that already contains `marker`
/// is used as-is (trailing slash stripped); a `/v1` base appends `/{suffix}`;
/// otherwise `/{v1}/{suffix}` is appended.
pub fn resolve_compat_url(base: &str, default_base: &str, marker: &str, suffix: &str) -> String {
    let trimmed = base.trim().trim_end_matches('/');
    let value = if trimmed.is_empty() {
        default_base
    } else {
        trimmed
    };
    if value.contains(marker) {
        value.to_owned()
    } else if value.ends_with("/v1") {
        format!("{value}/{suffix}")
    } else {
        format!("{value}/v1/{suffix}")
    }
}

/// Resolve an OpenAI-compatible transcription URL from a user-supplied base.
/// Empty values use Groq. A value that already contains `audio/transcriptions`
/// is used as-is; a `/v1` base appends `/audio/transcriptions`; otherwise
/// `/v1/audio/transcriptions` is appended.
pub fn resolve_transcription_url(base: &str) -> String {
    resolve_compat_url(
        base,
        DEFAULT_ASR_BASE_URL,
        "audio/transcriptions",
        "audio/transcriptions",
    )
}

/// Reuse the Groq chat key only for the Groq default or `api.groq.com`.
pub fn groq_key_fallback_allowed(base_url: &str) -> bool {
    if base_url.trim().is_empty() {
        return true;
    }
    transcription_host(base_url).as_deref() == Some("api.groq.com")
}

pub fn transcription_host(base_url: &str) -> Option<String> {
    host_from_url(&resolve_transcription_url(base_url))
}

pub fn asr_host_changed(previous: &str, next: &str) -> bool {
    transcription_host(previous) != transcription_host(next)
}

const ASR_URL_SCHEME_ERROR: &str = "ASR 地址必须是 http:// 或 https:// 开头的完整 URL。";
const ASR_URL_HTTPS_ERROR: &str = "非本机地址必须使用 https://。";

pub fn validate_asr_base_url(base_url: &str) -> Result<(), &'static str> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return Err(ASR_URL_SCHEME_ERROR);
    };
    let scheme = scheme.to_ascii_lowercase();
    if (scheme != "http" && scheme != "https") || rest.is_empty() || rest.starts_with('/') {
        return Err(ASR_URL_SCHEME_ERROR);
    }
    let Some(host) = host_from_url(trimmed) else {
        return Err(ASR_URL_SCHEME_ERROR);
    };
    if scheme == "http" && !is_loopback_host(&host) {
        return Err(ASR_URL_HTTPS_ERROR);
    }
    Ok(())
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "::1" | "localhost")
}

pub(crate) fn host_from_url(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let hostport = authority.rsplit_once('@').map(|(_, host)| host).unwrap_or(authority);
    let host = if let Some(end) = hostport.strip_prefix('[') {
        end.split_once(']')?.0
    } else {
        match hostport.rsplit_once(':') {
            Some((candidate, port)) if port.chars().all(|ch| ch.is_ascii_digit()) => candidate,
            _ => hostport,
        }
    };
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

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
    #[error("ASR authorization failed ({0})")]
    Unauthorized(String),
    #[error("rate limited{0}")]
    RateLimited(String),
    #[error("server error: {0}")]
    Server(String),
    #[error("empty speech result")]
    EmptyResult,
    #[error("Groq error: {0}")]
    Other(String),
}

#[derive(Debug, Clone)]
pub struct AsrOptions {
    pub api_key: String,
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub model: String,
}

impl Default for AsrOptions {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            language: None,
            prompt: None,
            model: MODEL.to_owned(),
        }
    }
}

pub fn resolve_asr_model(model: &str) -> &str {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        MODEL
    } else {
        trimmed
    }
}

const ENGLISH_ONLY_ASR_MODEL: &str = "distil-whisper-large-v3-en";

/// Automatic / Chinese recognition must not use the English-only distil model.
pub fn resolve_recognition_model<'a>(model: &'a str, language: Option<&str>) -> &'a str {
    let resolved = resolve_asr_model(model);
    if resolved == ENGLISH_ONLY_ASR_MODEL && normalize_language(language) != Some("en") {
        MODEL
    } else {
        resolved
    }
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
/// uploads, so prefetching is silent batch work, not streaming ASR, and never
/// exposes partial transcripts.
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
        Self::from_base_url("")
    }
}

impl GroqAsrProvider {
    pub fn from_base_url(base_url: impl AsRef<str>) -> Self {
        Self {
            endpoint: resolve_transcription_url(base_url.as_ref()),
        }
    }

    pub fn from_resolved_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
        }
    }

    #[cfg(test)]
    fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
        }
    }

    #[cfg(test)]
    fn endpoint(&self) -> &str {
        &self.endpoint
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
                resolve_asr_model(&options.model),
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
                model: MODEL.to_owned(),
            },
        )
        .await
}

pub(crate) async fn probe_transcription(
    endpoint: &str,
    wav: Vec<u8>,
    key: &str,
    model: &str,
) -> Result<(), AsrError> {
    match transcribe_at(endpoint, wav, key, None, None, model).await {
        Ok(_) | Err(AsrError::EmptyResult) => Ok(()),
        Err(error) => Err(error),
    }
}

async fn transcribe_at(
    endpoint: &str,
    wav: Vec<u8>,
    key: &str,
    language: Option<&str>,
    prompt: Option<&str>,
    model: &str,
) -> Result<Transcript, AsrError> {
    if endpoint.contains("api.deepgram.com") || endpoint.contains("/listen") {
        return transcribe_deepgram(endpoint, wav, key, language, model).await;
    }
    let client = http_client()?;
    let mut form = Form::new()
        .part("file", Part::bytes(wav).file_name("audio.wav"))
        .text("model", resolve_asr_model(model).to_owned())
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
        let host = host_from_url(endpoint).unwrap_or_else(|| "unknown".into());
        return Err(AsrError::Unauthorized(host));
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

async fn transcribe_deepgram(
    endpoint: &str,
    wav: Vec<u8>,
    key: &str,
    language: Option<&str>,
    model: &str,
) -> Result<Transcript, AsrError> {
    let mut url = endpoint.to_owned();
    if !url.contains('?') {
        url.push_str("?smart_format=true");
    }
    if !url.contains("model=") {
        url.push_str(&format!(
            "{}model={}",
            if url.contains('?') { "&" } else { "?" },
            resolve_asr_model(model)
        ));
    }
    if let Some(language) = normalize_language(language) {
        url.push_str("&language=");
        url.push_str(language);
    }
    let client = http_client()?;
    let response = client
        .post(&url)
        .header("Authorization", format!("Token {key}"))
        .header("Content-Type", "audio/wav")
        .body(wav)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                AsrError::Timeout
            } else {
                AsrError::Network(error.to_string())
            }
        })?;
    let limits = parse_rate_limits(response.headers());
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(AsrError::Unauthorized(
            host_from_url(endpoint).unwrap_or_else(|| "deepgram".into()),
        ));
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(AsrError::RateLimited(
            limits.retry_after.unwrap_or_default(),
        ));
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(AsrError::Other(format!("HTTP status {status} {body}")));
    }
    let parsed: serde_json::Value = response
        .json()
        .await
        .map_err(|error| AsrError::Other(error.to_string()))?;
    let text = parsed
        .pointer("/results/channels/0/alternatives/0/transcript")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();
    if text.trim().is_empty() {
        return Err(AsrError::EmptyResult);
    }
    Ok(Transcript {
        text,
        segments: Vec::new(),
        words: Vec::new(),
        limits,
    })
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
            Self::Unauthorized(_) => crate::queue::RetryClass::Unauthorized,
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
    fn resolve_asr_model_falls_back_to_whisper_turbo() {
        assert_eq!(resolve_asr_model(""), MODEL);
        assert_eq!(resolve_asr_model("   "), MODEL);
        assert_eq!(resolve_asr_model("whisper-1"), "whisper-1");
    }

    #[test]
    fn english_only_model_is_kept_only_when_recognition_is_pinned_english() {
        assert_eq!(
            resolve_recognition_model("distil-whisper-large-v3-en", Some("en")),
            "distil-whisper-large-v3-en"
        );
        assert_eq!(
            resolve_recognition_model("distil-whisper-large-v3-en", Some("auto")),
            MODEL
        );
        assert_eq!(
            resolve_recognition_model("distil-whisper-large-v3-en", Some("zh")),
            MODEL
        );
        assert_eq!(
            resolve_recognition_model("whisper-large-v3-turbo", Some("auto")),
            "whisper-large-v3-turbo"
        );
    }

    #[test]
    fn transcription_url_never_uses_the_english_translation_endpoint() {
        assert!(!resolve_transcription_url("").contains("translations"));
        assert!(!resolve_transcription_url("https://api.groq.com/openai/v1").contains("translations"));
        assert!(resolve_transcription_url("").contains("transcriptions"));
    }

    #[test]
    fn groq_asr_allowlist_excludes_openai_whisper_one() {
        assert!(is_groq_asr_model("whisper-large-v3-turbo"));
        assert!(is_groq_asr_model(" whisper-large-v3 "));
        assert!(is_groq_asr_model("distil-whisper-large-v3-en"));
        assert!(!is_groq_asr_model("whisper-1"));
        assert!(!is_groq_asr_model("my-local-whisper"));
        assert!(!is_groq_asr_model(""));
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
        let result = transcribe_at(&endpoint, b"wav".to_vec(), "test-key", Some("auto"), None, MODEL)
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
            transcribe_at(&empty, b"wav".to_vec(), "test-key", None, None, MODEL).await,
            Err(AsrError::EmptyResult)
        ));
    }

    #[tokio::test]
    async fn provider_auth_rate_limit_and_server_errors_are_classified() {
        let unauthorized =
            crate::test_http::spawn_response(401, "application/json", b"{}", &[]).await;
        let unauthorized_error =
            transcribe_at(&unauthorized, b"wav".to_vec(), "bad-key", None, None, MODEL)
                .await
                .expect_err("401 must be unauthorized");
        let unauthorized_message = unauthorized_error.to_string();
        assert!(
            unauthorized_message.starts_with("ASR authorization failed"),
            "{unauthorized_message}"
        );
        assert!(
            !unauthorized_message.contains("Groq"),
            "{unauthorized_message}"
        );
        assert!(matches!(
            unauthorized_error,
            AsrError::Unauthorized(host) if host == "127.0.0.1"
        ));

        let limited = crate::test_http::spawn_response(
            429,
            "application/json",
            b"{}",
            &[("retry-after", "2")],
        )
        .await;
        assert!(matches!(
            transcribe_at(&limited, b"wav".to_vec(), "test-key", None, None, MODEL).await,
            Err(AsrError::RateLimited(value)) if value == "2"
        ));

        let server = crate::test_http::spawn_response(500, "application/json", b"{}", &[]).await;
        assert!(matches!(
            transcribe_at(&server, b"wav".to_vec(), "test-key", None, None, MODEL).await,
            Err(AsrError::Server(_))
        ));

        let invalid =
            crate::test_http::spawn_response(200, "application/json", b"not-json".to_vec(), &[])
                .await;
        assert!(matches!(
            transcribe_at(&invalid, b"wav".to_vec(), "test-key", None, None, MODEL).await,
            Err(AsrError::Other(_))
        ));

        assert!(matches!(
            transcribe_at(
                "http://127.0.0.1:1/unreachable",
                b"wav".to_vec(),
                "test-key",
                None,
                None,
                MODEL,
            )
            .await,
            Err(AsrError::Network(_))
        ));
    }

    #[test]
    fn empty_base_url_resolves_to_groq_transcriptions() {
        assert_eq!(
            resolve_transcription_url(""),
            "https://api.groq.com/openai/v1/audio/transcriptions"
        );
        assert_eq!(
            resolve_transcription_url("   "),
            "https://api.groq.com/openai/v1/audio/transcriptions"
        );
        assert_eq!(
            GroqAsrProvider::default().endpoint(),
            "https://api.groq.com/openai/v1/audio/transcriptions"
        );
    }

    #[test]
    fn resolves_host_v1_and_full_transcriptions_urls() {
        assert_eq!(
            resolve_transcription_url("http://127.0.0.1:8000"),
            "http://127.0.0.1:8000/v1/audio/transcriptions"
        );
        assert_eq!(
            resolve_transcription_url("http://127.0.0.1:8000/"),
            "http://127.0.0.1:8000/v1/audio/transcriptions"
        );
        assert_eq!(
            resolve_transcription_url("http://127.0.0.1:8000/v1"),
            "http://127.0.0.1:8000/v1/audio/transcriptions"
        );
        assert_eq!(
            resolve_transcription_url("http://127.0.0.1:8000/v1/"),
            "http://127.0.0.1:8000/v1/audio/transcriptions"
        );
        assert_eq!(
            resolve_transcription_url("https://api.groq.com/openai/v1"),
            "https://api.groq.com/openai/v1/audio/transcriptions"
        );
        assert_eq!(
            resolve_transcription_url("http://127.0.0.1:8000/audio/transcriptions"),
            "http://127.0.0.1:8000/audio/transcriptions"
        );
        assert_eq!(
            resolve_transcription_url(
                "http://127.0.0.1:8000/v1/audio/transcriptions/"
            ),
            "http://127.0.0.1:8000/v1/audio/transcriptions"
        );
    }

    #[test]
    fn groq_key_fallback_is_limited_to_groq_hosts() {
        assert!(groq_key_fallback_allowed(""));
        assert!(groq_key_fallback_allowed("   "));
        assert!(groq_key_fallback_allowed("https://api.groq.com/openai/v1"));
        assert!(groq_key_fallback_allowed("https://API.GROQ.COM/openai/v1"));
        assert!(!groq_key_fallback_allowed("http://127.0.0.1:8000/v1"));
        assert!(!groq_key_fallback_allowed("https://asr.example.com/v1"));
        assert_eq!(
            transcription_host("https://api.groq.com/openai/v1").as_deref(),
            Some("api.groq.com")
        );
        assert_eq!(
            transcription_host("http://127.0.0.1:8000/v1").as_deref(),
            Some("127.0.0.1")
        );
        assert!(!asr_host_changed(
            "",
            "https://api.groq.com/openai/v1"
        ));
        assert!(asr_host_changed(
            "https://api.groq.com/openai/v1",
            "https://asr.example.com/v1"
        ));
        assert!(!asr_host_changed(
            "http://127.0.0.1:8000/v1",
            "http://127.0.0.1:9000/v1"
        ));
    }

    #[test]
    fn asr_base_url_requires_absolute_http_or_https_and_https_off_loopback() {
        assert!(validate_asr_base_url("").is_ok());
        assert!(validate_asr_base_url("   ").is_ok());
        assert!(validate_asr_base_url("https://asr.example.com/v1").is_ok());
        assert!(validate_asr_base_url("http://127.0.0.1:8000/v1").is_ok());
        assert!(validate_asr_base_url("http://localhost:8000/v1").is_ok());
        assert!(validate_asr_base_url("http://[::1]:8000/v1").is_ok());
        assert_eq!(
            validate_asr_base_url("asr.example.com/v1"),
            Err("ASR 地址必须是 http:// 或 https:// 开头的完整 URL。")
        );
        assert_eq!(
            validate_asr_base_url("ftp://asr.example.com/v1"),
            Err("ASR 地址必须是 http:// 或 https:// 开头的完整 URL。")
        );
        assert_eq!(
            validate_asr_base_url("http://asr.example.com/v1"),
            Err("非本机地址必须使用 https://。")
        );
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
    async fn provider_from_base_url_posts_to_resolved_mock_endpoint() {
        let host = crate::test_http::spawn_response(
            200,
            "application/json",
            br#"{"text":"compatible asr","segments":[],"words":[]}"#.to_vec(),
            &[],
        )
        .await;
        let provider = GroqAsrProvider::from_base_url(&host);
        assert_eq!(
            provider.endpoint(),
            format!("{host}/v1/audio/transcriptions")
        );
        let result = provider
            .transcribe_batch(
                b"wav".to_vec(),
                AsrOptions {
                    api_key: "compat-key".into(),
                    ..AsrOptions::default()
                },
            )
            .await
            .expect("compatible ASR request should succeed");
        assert_eq!(result.text, "compatible asr");
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
