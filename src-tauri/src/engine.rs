use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::asr::{self, AsrError};
use crate::llm::{self, CleanupEffort, LlmError};
use crate::providers;
use crate::store::Settings;

pub use crate::providers::EngineProvider;

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
    #[serde(default)]
    pub provider_keys: BTreeMap<String, String>,
    #[serde(default)]
    pub custom_base_url: String,
    #[serde(default = "default_true")]
    pub custom_asr: bool,
    #[serde(default = "default_true")]
    pub custom_llm: bool,
    #[serde(default)]
    pub ollama_base_url: String,
    #[serde(default)]
    pub local_whisper_base_url: String,
    #[serde(default = "default_cleanup_enabled")]
    pub cleanup_enabled: bool,
}

fn default_cleanup_enabled() -> bool {
    true
}

fn default_true() -> bool {
    true
}

impl Default for EngineDraft {
    fn default() -> Self {
        Self {
            asr_provider: EngineProvider::Groq,
            cleanup_provider: EngineProvider::Groq,
            asr_base_url: String::new(),
            cleanup_base_url: String::new(),
            asr_model: asr::MODEL.to_owned(),
            cleanup_model: llm::MODEL.to_owned(),
            api_key: String::new(),
            asr_api_key: String::new(),
            cleanup_api_key: String::new(),
            provider_keys: BTreeMap::new(),
            custom_base_url: String::new(),
            custom_asr: true,
            custom_llm: true,
            ollama_base_url: EngineProvider::Ollama.default_base_url().to_owned(),
            local_whisper_base_url: EngineProvider::LocalWhisper.default_base_url().to_owned(),
            cleanup_enabled: true,
        }
    }
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

fn typed_provider_key(draft: &EngineDraft, provider: EngineProvider) -> String {
    draft
        .provider_keys
        .get(provider.as_str())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
}

fn resolve_side_key(
    draft: &EngineDraft,
    stored: &Settings,
    provider: EngineProvider,
    fallback_typed: &str,
    stored_legacy: &str,
) -> Result<String, ProbeStageResult> {
    let typed = typed_provider_key(draft, provider);
    if !typed.is_empty() {
        return Ok(typed);
    }
    if provider.is_groq() && !draft.api_key.trim().is_empty() {
        return Ok(draft.api_key.trim().to_owned());
    }
    if !fallback_typed.trim().is_empty() {
        return Ok(fallback_typed.trim().to_owned());
    }
    let stored_key = stored.provider_secret(provider);
    if !stored_key.is_empty() {
        return Ok(stored_key.to_owned());
    }
    if !stored_legacy.trim().is_empty() {
        return Ok(stored_legacy.trim().to_owned());
    }
    if provider.allows_empty_key() && providers::is_loopback_url(&draft_provider_base(draft, provider))
    {
        return Ok(String::new());
    }
    Err(missing_key())
}

fn resolve_asr_key(draft: &EngineDraft, stored: &Settings) -> Result<String, ProbeStageResult> {
    resolve_side_key(
        draft,
        stored,
        draft.asr_provider,
        &draft.asr_api_key,
        stored.asr_api_key.as_str(),
    )
}

fn resolve_cleanup_key(draft: &EngineDraft, stored: &Settings) -> Result<String, ProbeStageResult> {
    resolve_side_key(
        draft,
        stored,
        draft.cleanup_provider,
        &draft.cleanup_api_key,
        stored.cleanup_api_key.as_str(),
    )
}

fn draft_provider_base(draft: &EngineDraft, provider: EngineProvider) -> String {
    match provider {
        EngineProvider::Ollama => nonempty(
            &draft.ollama_base_url,
            provider.default_base_url(),
        ),
        EngineProvider::LocalWhisper => nonempty(
            &draft.local_whisper_base_url,
            provider.default_base_url(),
        ),
        EngineProvider::Custom => nonempty(
            &draft.custom_base_url,
            nonempty(&draft.asr_base_url, &draft.cleanup_base_url),
        ),
        EngineProvider::Groq => String::new(),
        other => other.default_base_url().to_owned(),
    }
}

fn nonempty(value: &str, fallback: impl Into<String>) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.into()
    } else {
        trimmed.to_owned()
    }
}

fn draft_asr_url(draft: &EngineDraft) -> String {
    let override_base = draft.asr_base_url.trim();
    let base = if !override_base.is_empty() {
        override_base.to_owned()
    } else {
        draft_provider_base(draft, draft.asr_provider)
    };
    providers::resolve_asr_endpoint(draft.asr_provider, &base)
}

fn draft_cleanup_url(draft: &EngineDraft) -> String {
    let override_base = draft.cleanup_base_url.trim();
    let base = if !override_base.is_empty() {
        override_base.to_owned()
    } else {
        draft_provider_base(draft, draft.cleanup_provider)
    };
    providers::resolve_llm_endpoint(draft.cleanup_provider, &base)
}

pub fn apply_engine_draft(settings: &mut Settings, draft: &EngineDraft) {
    settings.asr_provider = draft.asr_provider;
    settings.asr_model = draft_asr_model(draft);
    settings.cleanup_provider = draft.cleanup_provider;
    settings.cleanup_model = draft_cleanup_model(draft);
    settings.custom_asr = draft.custom_asr;
    settings.custom_llm = draft.custom_llm;
    settings.cleanup_enabled = draft.cleanup_enabled;
    let custom = nonempty(
        &draft.custom_base_url,
        nonempty(&draft.asr_base_url, &draft.cleanup_base_url),
    );
    if !custom.is_empty() {
        settings.custom_base_url = custom;
    }
    if !draft.ollama_base_url.trim().is_empty() {
        settings.ollama_base_url = draft.ollama_base_url.trim().to_owned();
    }
    if !draft.local_whisper_base_url.trim().is_empty() {
        settings.local_whisper_base_url = draft.local_whisper_base_url.trim().to_owned();
    }
    if draft.asr_provider.is_custom() {
        settings.asr_base_url = settings.custom_base_url.clone();
    } else {
        settings.asr_base_url.clear();
    }
    if draft.cleanup_provider.is_custom() {
        settings.cleanup_base_url = settings.custom_base_url.clone();
    } else {
        settings.cleanup_base_url.clear();
    }
    for (id, key) in &draft.provider_keys {
        if key.trim().is_empty() {
            continue;
        }
        settings
            .provider_api_keys
            .insert(id.clone(), key.trim().to_owned());
        if id == "groq" {
            settings.api_key = key.trim().to_owned();
        }
        if id == "custom" {
            settings.asr_api_key = key.trim().to_owned();
            settings.cleanup_api_key = key.trim().to_owned();
        }
    }
    if draft.asr_provider.is_groq() && !draft.api_key.trim().is_empty() {
        settings.api_key = draft.api_key.trim().to_owned();
        settings
            .provider_api_keys
            .insert("groq".into(), settings.api_key.clone());
    }
    if !draft.asr_api_key.trim().is_empty() && draft.asr_provider.is_custom() {
        settings.asr_api_key = draft.asr_api_key.trim().to_owned();
        settings
            .provider_api_keys
            .insert("custom".into(), settings.asr_api_key.clone());
    }
    if !draft.cleanup_api_key.trim().is_empty() && draft.cleanup_provider.is_custom() {
        settings.cleanup_api_key = draft.cleanup_api_key.trim().to_owned();
        settings
            .provider_api_keys
            .entry("custom".into())
            .or_insert_with(|| settings.cleanup_api_key.clone());
    }
}

fn draft_asr_model(draft: &EngineDraft) -> String {
    let model = draft.asr_model.trim();
    if draft.asr_provider.is_groq() {
        if asr::is_groq_asr_model(model) {
            model.to_owned()
        } else {
            asr::MODEL.to_owned()
        }
    } else if model.is_empty() {
        draft.asr_provider.default_asr_model().to_owned()
    } else {
        model.to_owned()
    }
}

fn draft_cleanup_model(draft: &EngineDraft) -> String {
    let model = draft.cleanup_model.trim();
    if draft.cleanup_provider.is_groq() {
        if llm::is_supported_model(model) {
            model.to_owned()
        } else {
            llm::MODEL.to_owned()
        }
    } else if model.is_empty() {
        draft.cleanup_provider.default_llm_model().to_owned()
    } else {
        model.to_owned()
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
                None,
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
            asr_base_url: url.to_owned(),
            custom_base_url: url.to_owned(),
            asr_model: model.to_owned(),
            asr_api_key: "probe-key".into(),
            cleanup_enabled: false,
            ..EngineDraft::default()
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
            asr_base_url: asr_endpoint.clone(),
            cleanup_base_url: chat_endpoint,
            custom_base_url: asr_endpoint,
            asr_model: "whisper-1".into(),
            cleanup_model: "gpt-4o-mini".into(),
            asr_api_key: "asr".into(),
            cleanup_api_key: "sk-openai".into(),
            cleanup_enabled: true,
            ..EngineDraft::default()
        };
        let result = probe_engine_draft(&draft, &Settings::default()).await;
        assert!(result.asr.ok, "{}", result.asr.message);
        assert!(result.cleanup.ok, "{}", result.cleanup.message);
        assert!(result.succeeded());
    }

    #[tokio::test]
    async fn probe_accepts_deepgram_listen_json() {
        let endpoint = crate::test_http::spawn_response(
            200,
            "application/json",
            br#"{"results":{"channels":[{"alternatives":[{"transcript":"hello"}]}]}}"#,
            &[],
        )
        .await;
        let mut keys = BTreeMap::new();
        keys.insert("deepgram".into(), "dg-token".into());
        let draft = EngineDraft {
            asr_provider: EngineProvider::Deepgram,
            asr_base_url: endpoint,
            asr_model: "nova-3".into(),
            provider_keys: keys,
            cleanup_enabled: false,
            ..EngineDraft::default()
        };
        let result = probe_engine_draft(&draft, &Settings::default()).await;
        assert!(result.asr.ok, "{}", result.asr.message);
        assert!(result.succeeded());
    }

    #[tokio::test]
    async fn probe_accepts_anthropic_messages_json() {
        let asr_endpoint = crate::test_http::spawn_response(
            200,
            "application/json",
            br#"{"text":"hello","segments":[],"words":[]}"#,
            &[],
        )
        .await;
        let chat_endpoint = crate::test_http::spawn_response(
            200,
            "application/json",
            r#"{"content":[{"type":"text","text":"嗯 那个 你好"}]}"#.as_bytes(),
            &[],
        )
        .await;
        let mut keys = BTreeMap::new();
        keys.insert("custom".into(), "asr".into());
        keys.insert("anthropic".into(), "sk-ant".into());
        let draft = EngineDraft {
            asr_provider: EngineProvider::Custom,
            cleanup_provider: EngineProvider::Anthropic,
            asr_base_url: asr_endpoint.clone(),
            cleanup_base_url: chat_endpoint,
            custom_base_url: asr_endpoint,
            asr_model: "whisper-1".into(),
            cleanup_model: "claude-sonnet-4-5".into(),
            provider_keys: keys,
            cleanup_enabled: true,
            ..EngineDraft::default()
        };
        let result = probe_engine_draft(&draft, &Settings::default()).await;
        assert!(result.asr.ok, "{}", result.asr.message);
        assert!(result.cleanup.ok, "{}", result.cleanup.message);
        assert!(result.succeeded());
    }

    #[test]
    fn apply_engine_draft_keeps_unused_pool_keys() {
        let mut settings = Settings {
            api_key: "gsk_keep".into(),
            ..Settings::default()
        };
        settings
            .provider_api_keys
            .insert("groq".into(), "gsk_keep".into());
        let mut keys = BTreeMap::new();
        keys.insert("openai".into(), "sk-new".into());
        let draft = EngineDraft {
            asr_provider: EngineProvider::Groq,
            cleanup_provider: EngineProvider::OpenAi,
            cleanup_model: "gpt-4o-mini".into(),
            provider_keys: keys,
            ..EngineDraft::default()
        };
        apply_engine_draft(&mut settings, &draft);
        assert_eq!(settings.cleanup_provider, EngineProvider::OpenAi);
        assert_eq!(
            settings.provider_api_keys.get("groq").map(String::as_str),
            Some("gsk_keep")
        );
        assert_eq!(
            settings.provider_api_keys.get("openai").map(String::as_str),
            Some("sk-new")
        );
    }
}
