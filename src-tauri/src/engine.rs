use serde::{Deserialize, Serialize};

use crate::asr::{self, AsrError};
use crate::llm::{self, CleanupEffort, LlmError};
use crate::store::Settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EngineProvider {
    #[default]
    Groq,
    Custom,
}

impl EngineProvider {
    pub fn is_custom(self) -> bool {
        matches!(self, Self::Custom)
    }

    pub fn is_groq(self) -> bool {
        matches!(self, Self::Groq)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineDraft {
    pub asr_provider: EngineProvider,
    pub cleanup_provider: EngineProvider,
    pub asr_base_url: String,
    pub cleanup_base_url: String,
    pub asr_model: String,
    pub cleanup_model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub asr_api_key: String,
    #[serde(default)]
    pub cleanup_api_key: String,
    #[serde(default = "default_cleanup_enabled")]
    pub cleanup_enabled: bool,
}

fn default_cleanup_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeErrorKind {
    Address,
    Key,
    Model,
    Path,
    Provider,
    MissingKey,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProbeStageResult {
    pub ok: bool,
    pub skipped: bool,
    pub error_kind: Option<ProbeErrorKind>,
    pub message: String,
}

impl ProbeStageResult {
    fn success() -> Self {
        Self {
            ok: true,
            skipped: false,
            error_kind: None,
            message: String::new(),
        }
    }

    fn skipped() -> Self {
        Self {
            ok: true,
            skipped: true,
            error_kind: None,
            message: String::new(),
        }
    }

    fn failure(kind: ProbeErrorKind, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            skipped: false,
            error_kind: Some(kind),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProbeResult {
    pub asr: ProbeStageResult,
    pub cleanup: ProbeStageResult,
}

impl ProbeResult {
    pub fn succeeded(&self) -> bool {
        self.asr.ok && self.cleanup.ok
    }
}

pub fn classify_status_body(status: u16, body: &str) -> ProbeErrorKind {
    let lower = body.to_ascii_lowercase();
    if status == 401 || status == 403 {
        return ProbeErrorKind::Key;
    }
    if status == 404 || lower.contains("model") && (lower.contains("not") || lower.contains("invalid") || lower.contains("unknown")) {
        return ProbeErrorKind::Model;
    }
    if status == 400 && (lower.contains("url") || lower.contains("path") || lower.contains("route")) {
        return ProbeErrorKind::Path;
    }
    ProbeErrorKind::Provider
}

pub fn classify_asr_error(error: &AsrError) -> ProbeErrorKind {
    match error {
        AsrError::Network(_) | AsrError::Timeout => ProbeErrorKind::Address,
        AsrError::Unauthorized(_) => ProbeErrorKind::Key,
        AsrError::Other(message) => classify_status_body(status_from_message(message), message),
        AsrError::Server(_) | AsrError::RateLimited(_) | AsrError::EmptyResult => {
            ProbeErrorKind::Provider
        }
    }
}

pub fn classify_llm_error(error: &LlmError) -> ProbeErrorKind {
    match error {
        LlmError::Network(_) | LlmError::Timeout => ProbeErrorKind::Address,
        LlmError::Unauthorized => ProbeErrorKind::Key,
        LlmError::Other(message) => classify_status_body(status_from_message(message), message),
        LlmError::Server(_) | LlmError::RateLimited(_) => ProbeErrorKind::Provider,
    }
}

fn status_from_message(message: &str) -> u16 {
    message
        .split_whitespace()
        .find_map(|token| token.parse::<u16>().ok().filter(|code| (100..600).contains(code)))
        .unwrap_or(0)
}

fn missing_key() -> ProbeStageResult {
    ProbeStageResult::failure(ProbeErrorKind::MissingKey, "missing key")
}

fn resolve_asr_key(draft: &EngineDraft, stored: &Settings) -> Result<String, ProbeStageResult> {
    if draft.asr_provider.is_groq() {
        if !draft.api_key.trim().is_empty() {
            return Ok(draft.api_key.trim().to_owned());
        }
        if !stored.api_key.trim().is_empty() {
            return Ok(stored.api_key.clone());
        }
        return Err(missing_key());
    }
    if !draft.asr_api_key.trim().is_empty() {
        return Ok(draft.asr_api_key.trim().to_owned());
    }
    if !asr::asr_host_changed(&stored.asr_base_url, &draft.asr_base_url)
        && !stored.asr_api_key.trim().is_empty()
    {
        return Ok(stored.asr_api_key.clone());
    }
    Err(missing_key())
}

fn resolve_cleanup_key(draft: &EngineDraft, stored: &Settings) -> Result<String, ProbeStageResult> {
    if draft.cleanup_provider.is_groq() {
        if !draft.api_key.trim().is_empty() {
            return Ok(draft.api_key.trim().to_owned());
        }
        if !stored.api_key.trim().is_empty() {
            return Ok(stored.api_key.clone());
        }
        return Err(missing_key());
    }
    if !draft.cleanup_api_key.trim().is_empty() {
        return Ok(draft.cleanup_api_key.trim().to_owned());
    }
    if !llm::chat_host_changed(&stored.cleanup_base_url, &draft.cleanup_base_url)
        && !stored.cleanup_api_key.trim().is_empty()
    {
        return Ok(stored.cleanup_api_key.clone());
    }
    Err(missing_key())
}

fn draft_asr_url(draft: &EngineDraft) -> String {
    if draft.asr_provider.is_groq() {
        asr::resolve_transcription_url("")
    } else {
        asr::resolve_transcription_url(&draft.asr_base_url)
    }
}

fn draft_cleanup_url(draft: &EngineDraft) -> String {
    if draft.cleanup_provider.is_groq() {
        llm::resolve_chat_url("")
    } else {
        llm::resolve_chat_url(&draft.cleanup_base_url)
    }
}

fn draft_asr_model(draft: &EngineDraft) -> String {
    if draft.asr_provider.is_groq() {
        if asr::is_groq_asr_model(&draft.asr_model) {
            draft.asr_model.trim().to_owned()
        } else {
            asr::MODEL.to_owned()
        }
    } else {
        let model = draft.asr_model.trim();
        if model.is_empty() {
            asr::MODEL.to_owned()
        } else {
            model.to_owned()
        }
    }
}

fn draft_cleanup_model(draft: &EngineDraft) -> String {
    if draft.cleanup_provider.is_groq() {
        if llm::is_supported_model(&draft.cleanup_model) {
            draft.cleanup_model.clone()
        } else {
            llm::MODEL.to_owned()
        }
    } else {
        let model = draft.cleanup_model.trim();
        if model.is_empty() {
            llm::MODEL.to_owned()
        } else {
            model.to_owned()
        }
    }
}

fn probe_wav() -> Vec<u8> {
    const SAMPLE_RATE: u32 = 16_000;
    const DURATION_MS: u32 = 250;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = std::io::Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut cursor, spec).expect("probe wav header");
    for _ in 0..(SAMPLE_RATE * DURATION_MS / 1000) {
        writer.write_sample(0i16).expect("probe wav sample");
    }
    writer.finalize().expect("probe wav finalize");
    cursor.into_inner()
}

pub async fn probe_engine_draft(draft: &EngineDraft, stored: &Settings) -> ProbeResult {
    let asr = match resolve_asr_key(draft, stored) {
        Ok(key) => match asr::probe_transcription(
            &draft_asr_url(draft),
            probe_wav(),
            &key,
            &draft_asr_model(draft),
        )
        .await
        {
            Ok(()) => ProbeStageResult::success(),
            Err(error) => ProbeStageResult::failure(classify_asr_error(&error), error.to_string()),
        },
        Err(error) => error,
    };
    if !asr.ok {
        return ProbeResult {
            asr,
            cleanup: ProbeStageResult::skipped(),
        };
    }
    if !draft.cleanup_enabled {
        return ProbeResult {
            asr,
            cleanup: ProbeStageResult::skipped(),
        };
    }
    let cleanup = match resolve_cleanup_key(draft, stored) {
        Ok(key) => {
            match llm::cleanup_with_model_and_limits_and_language_and_profile_and_intent(
                &draft_cleanup_url(draft),
                &draft_cleanup_model(draft),
                "嗯 那个 你好",
                &key,
                &[],
                None,
                None,
                None,
                None,
                None,
                None,
                CleanupEffort::Light,
            )
            .await
            {
                Ok(_) => ProbeStageResult::success(),
                Err(error) => {
                    ProbeStageResult::failure(classify_llm_error(&error), error.to_string())
                }
            }
        }
        Err(error) => error,
    };
    ProbeResult { asr, cleanup }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom_asr_draft(url: &str, model: &str) -> EngineDraft {
        EngineDraft {
            asr_provider: EngineProvider::Custom,
            cleanup_provider: EngineProvider::Groq,
            asr_base_url: url.to_owned(),
            cleanup_base_url: String::new(),
            asr_model: model.to_owned(),
            cleanup_model: llm::MODEL.to_owned(),
            api_key: String::new(),
            asr_api_key: "probe-key".into(),
            cleanup_api_key: String::new(),
            cleanup_enabled: false,
        }
    }

    #[test]
    fn probe_wav_meets_groq_minimum_duration() {
        let wav = probe_wav();
        let reader = hound::WavReader::new(std::io::Cursor::new(&wav)).expect("valid wav");
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 16_000);
        assert_eq!(spec.bits_per_sample, 16);
        let samples = reader
            .into_samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .expect("pcm samples");
        let seconds = samples.len() as f64 / f64::from(spec.sample_rate);
        assert!(
            seconds >= 0.01,
            "Groq rejects files shorter than 0.01s; got {seconds}s ({} samples)",
            samples.len()
        );
    }

    #[test]
    fn classify_status_body_splits_key_model_and_path() {
        assert_eq!(classify_status_body(401, ""), ProbeErrorKind::Key);
        assert_eq!(
            classify_status_body(404, "not found"),
            ProbeErrorKind::Model
        );
        assert_eq!(
            classify_status_body(400, "unknown model"),
            ProbeErrorKind::Model
        );
        assert_eq!(
            classify_status_body(400, "invalid path"),
            ProbeErrorKind::Path
        );
        assert_eq!(classify_status_body(500, "oops"), ProbeErrorKind::Provider);
    }

    #[tokio::test]
    async fn probe_maps_connect_failure_to_address() {
        let result = probe_engine_draft(
            &custom_asr_draft("http://127.0.0.1:1", "whisper-1"),
            &Settings::default(),
        )
        .await;
        assert!(!result.asr.ok);
        assert_eq!(result.asr.error_kind, Some(ProbeErrorKind::Address));
        assert!(result.cleanup.skipped);
        assert!(!result.succeeded());
    }

    #[tokio::test]
    async fn probe_maps_unauthorized_to_key() {
        let endpoint = crate::test_http::spawn_response(401, "application/json", b"{}", &[]).await;
        let result = probe_engine_draft(
            &custom_asr_draft(&endpoint, "whisper-1"),
            &Settings::default(),
        )
        .await;
        assert_eq!(result.asr.error_kind, Some(ProbeErrorKind::Key));
    }

    #[tokio::test]
    async fn probe_maps_unknown_model_to_model() {
        let endpoint = crate::test_http::spawn_response(
            404,
            "application/json",
            br#"{"error":{"message":"model_not_found"}}"#,
            &[],
        )
        .await;
        let result = probe_engine_draft(
            &custom_asr_draft(&endpoint, "no-such-model"),
            &Settings::default(),
        )
        .await;
        assert_eq!(result.asr.error_kind, Some(ProbeErrorKind::Model));
    }

    #[tokio::test]
    async fn probe_accepts_empty_asr_text_and_skips_disabled_cleanup() {
        let endpoint = crate::test_http::spawn_response(
            200,
            "application/json",
            br#"{"text":"  ","segments":[],"words":[]}"#,
            &[],
        )
        .await;
        let result = probe_engine_draft(
            &custom_asr_draft(&endpoint, "whisper-1"),
            &Settings::default(),
        )
        .await;
        assert!(result.asr.ok);
        assert!(result.cleanup.skipped);
        assert!(result.succeeded());
    }

    #[tokio::test]
    async fn probe_runs_cleanup_against_the_draft_chat_url() {
        let asr_endpoint = crate::test_http::spawn_response(
            200,
            "application/json",
            br#"{"text":"hello","segments":[],"words":[]}"#,
            &[],
        )
        .await;
        let chat_endpoint = crate::test_http::spawn_response(
            200,
            "text/event-stream",
            "data: {\"choices\":[{\"delta\":{\"content\":\"嗯 那个 你好\"}}]}\n\ndata: [DONE]\n\n",
            &[],
        )
        .await;
        let draft = EngineDraft {
            asr_provider: EngineProvider::Custom,
            cleanup_provider: EngineProvider::Custom,
            asr_base_url: asr_endpoint,
            cleanup_base_url: chat_endpoint,
            asr_model: "whisper-1".into(),
            cleanup_model: "gpt-4o-mini".into(),
            api_key: String::new(),
            asr_api_key: "asr".into(),
            cleanup_api_key: "sk-openai".into(),
            cleanup_enabled: true,
        };
        let result = probe_engine_draft(&draft, &Settings::default()).await;
        assert!(result.asr.ok, "{}", result.asr.message);
        assert!(result.cleanup.ok, "{}", result.cleanup.message);
        assert!(result.succeeded());
    }
}
