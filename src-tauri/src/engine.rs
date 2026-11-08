use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

use crate::asr::{self, AsrError};
use crate::llm::{self, CleanupEffort, LlmError};
use crate::ondevice_asr;
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
    pub custom_cleanup_parameters: crate::custom_cleanup_parameters::Parameters,
    #[serde(default)]
    pub probe_stage: ProbeScope,
    #[serde(default)]
    pub ollama_base_url: String,
    #[serde(default)]
    pub local_whisper_base_url: String,
    #[serde(default = "default_cleanup_enabled")]
    pub cleanup_enabled: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeScope {
    #[default]
    Both,
    Asr,
    Cleanup,
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
            custom_cleanup_parameters: Default::default(),
            probe_stage: ProbeScope::Both,
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
    OnDeviceModelMissing,
    OnDeviceInferenceUnavailable,
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
    #[cfg(test)]
    pub fn succeeded(&self) -> bool {
        self.asr.ok && self.cleanup.ok
    }
}

pub fn classify_status_body(status: u16, body: &str) -> ProbeErrorKind {
    let lower = body.to_ascii_lowercase();
    if status == 401 || status == 403 {
        return ProbeErrorKind::Key;
    }
    if lower.contains("model")
        && (lower.contains("not") || lower.contains("invalid") || lower.contains("unknown"))
    {
        return ProbeErrorKind::Model;
    }
    if status == 404 {
        return ProbeErrorKind::Path;
    }
    if status == 400 && (lower.contains("url") || lower.contains("path") || lower.contains("route"))
    {
        return ProbeErrorKind::Path;
    }
    ProbeErrorKind::Provider
}

pub fn classify_asr_error(error: &AsrError) -> ProbeErrorKind {
    match error {
        AsrError::Network(_) | AsrError::Timeout => ProbeErrorKind::Address,
        AsrError::Unauthorized(_) => ProbeErrorKind::Key,
        AsrError::OnDeviceModelMissing(_) => ProbeErrorKind::OnDeviceModelMissing,
        AsrError::OnDeviceInferenceUnavailable(_) => ProbeErrorKind::OnDeviceInferenceUnavailable,
        AsrError::Other(message) => classify_status_body(status_from_message(message), message),
        AsrError::Server(_)
        | AsrError::RetryableServer { .. }
        | AsrError::RateLimited(_)
        | AsrError::EmptyResult
        | AsrError::ContextAuthorizationChanged => ProbeErrorKind::Provider,
    }
}

pub fn classify_llm_error(error: &LlmError) -> ProbeErrorKind {
    match error {
        LlmError::Network(_) | LlmError::Timeout => ProbeErrorKind::Address,
        LlmError::Unauthorized => ProbeErrorKind::Key,
        LlmError::Other(message) => classify_status_body(status_from_message(message), message),
        LlmError::NativeLocal(failure) => match failure.category() {
            llm::NativeFailureCategory::Address => ProbeErrorKind::Address,
            llm::NativeFailureCategory::Path => ProbeErrorKind::Path,
            llm::NativeFailureCategory::Model => ProbeErrorKind::Model,
            llm::NativeFailureCategory::Provider => ProbeErrorKind::Provider,
        },
        LlmError::Server(_)
        | LlmError::RateLimited(_)
        | LlmError::Guard(_)
        | LlmError::CleanupRejected(_)
        | LlmError::ContextAuthorizationChanged => ProbeErrorKind::Provider,
    }
}

fn status_from_message(message: &str) -> u16 {
    message
        .split_whitespace()
        .find_map(|token| {
            token
                .parse::<u16>()
                .ok()
                .filter(|code| (100..600).contains(code))
        })
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
    custom_purpose: Option<crate::store::CustomCredentialPurpose>,
    fallback_typed: &str,
    stored_legacy: &str,
    destination_url: &str,
) -> Result<String, ProbeStageResult> {
    let typed = typed_provider_key(draft, provider);
    if !typed.is_empty() {
        return Ok(typed);
    }
    if provider.is_groq() && !draft.api_key.trim().is_empty() {
        return Ok(draft.api_key.trim().to_owned());
    }
    if provider == EngineProvider::Custom && !fallback_typed.trim().is_empty() {
        return Ok(fallback_typed.trim().to_owned());
    }
    let custom_host_changed = provider == EngineProvider::Custom
        && crate::llm::custom_connection_origin_changed(
            &stored.resolved_provider_base(EngineProvider::Custom),
            destination_url,
        );
    if custom_host_changed {
        if provider.allows_empty_key() && providers::is_loopback_url(destination_url) {
            return Ok(String::new());
        }
        return Err(missing_key());
    }
    let rejected_custom_session = provider.is_custom() && stored.session_custom_credential_rejected;
    let stored_key = if provider.is_custom() && stored.session_only_custom_credential {
        custom_purpose
            .map(|purpose| stored.session_custom_credential_for(purpose, destination_url))
            .unwrap_or("")
    } else if rejected_custom_session {
        ""
    } else {
        stored.provider_secret(provider)
    };
    if !stored_key.is_empty() {
        return Ok(stored_key.to_owned());
    }
    if provider.is_custom()
        && (stored.session_only_custom_credential || stored.session_custom_credential_rejected)
    {
        // A recovered value is scoped to the active ASR/cleanup purpose and
        // origin. Do not fall through to an unrelated saved stage alias.
        return Err(missing_key());
    }
    if provider == EngineProvider::Custom && !stored_legacy.trim().is_empty() {
        return Ok(stored_legacy.trim().to_owned());
    }
    if matches!(
        provider,
        EngineProvider::OnDevice | EngineProvider::OnDeviceMlx
    ) {
        return Ok(String::new());
    }
    if provider.allows_empty_key() && providers::is_loopback_url(destination_url) {
        return Ok(String::new());
    }
    Err(missing_key())
}

fn resolve_asr_key(draft: &EngineDraft, stored: &Settings) -> Result<String, ProbeStageResult> {
    let destination_url =
        draft_asr_url(draft).unwrap_or_else(|| draft_provider_base(draft, draft.asr_provider));
    resolve_side_key(
        draft,
        stored,
        draft.asr_provider,
        Some(crate::store::CustomCredentialPurpose::Asr),
        &draft.asr_api_key,
        stored.asr_api_key.as_str(),
        &destination_url,
    )
}

fn resolve_cleanup_key(draft: &EngineDraft, stored: &Settings) -> Result<String, ProbeStageResult> {
    let destination_url = draft_cleanup_url(draft);
    resolve_side_key(
        draft,
        stored,
        draft.cleanup_provider,
        Some(crate::store::CustomCredentialPurpose::Cleanup),
        &draft.cleanup_api_key,
        stored.cleanup_api_key.as_str(),
        &destination_url,
    )
}

fn draft_provider_base(draft: &EngineDraft, provider: EngineProvider) -> String {
    match provider {
        EngineProvider::Ollama => nonempty(&draft.ollama_base_url, provider.default_base_url()),
        EngineProvider::LocalWhisper => {
            nonempty(&draft.local_whisper_base_url, provider.default_base_url())
        }
        EngineProvider::Custom => draft.custom_base_url.trim().to_owned(),
        EngineProvider::DashScope => nonempty(&draft.asr_base_url, provider.default_base_url()),
        EngineProvider::AssemblyAi => provider.default_base_url().to_owned(),
        EngineProvider::Groq | EngineProvider::OnDevice => String::new(),
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

fn draft_asr_url(draft: &EngineDraft) -> Option<String> {
    if !draft.asr_provider.has_http_asr()
        && !matches!(
            draft.asr_provider,
            EngineProvider::Soniox | EngineProvider::DashScope
        )
    {
        return None;
    }
    let base = if draft.asr_provider == EngineProvider::Custom {
        draft_custom_asr_base_url(draft).to_owned()
    } else {
        let override_base = draft.asr_base_url.trim();
        if !override_base.is_empty() {
            override_base.to_owned()
        } else {
            draft_provider_base(draft, draft.asr_provider)
        }
    };
    if draft.asr_provider == EngineProvider::Custom && base.is_empty() {
        return None;
    }
    providers::resolve_asr_endpoint(draft.asr_provider, &base)
}

fn draft_cleanup_url(draft: &EngineDraft) -> String {
    if draft.cleanup_provider == EngineProvider::Ollama {
        let base = if draft.ollama_base_url.trim().is_empty() {
            EngineProvider::Ollama.default_base_url()
        } else {
            draft.ollama_base_url.trim()
        };
        return crate::ollama_local::chat_endpoint(base);
    }
    if draft.cleanup_provider == EngineProvider::LmStudio {
        return crate::lmstudio_local::chat_endpoint(EngineProvider::LmStudio.default_base_url());
    }
    let base = if draft.cleanup_provider == EngineProvider::Custom {
        draft_custom_cleanup_base_url(draft).to_owned()
    } else {
        let override_base = draft.cleanup_base_url.trim();
        if !override_base.is_empty() {
            override_base.to_owned()
        } else {
            draft_provider_base(draft, draft.cleanup_provider)
        }
    };
    if draft.cleanup_provider == EngineProvider::Custom && base.is_empty() {
        return String::new();
    }
    providers::resolve_llm_endpoint(draft.cleanup_provider, &base)
}

fn draft_custom_asr_base_url(draft: &EngineDraft) -> &str {
    let stage_base = draft.asr_base_url.trim();
    if stage_base.is_empty() {
        draft.custom_base_url.trim()
    } else {
        stage_base
    }
}

fn draft_custom_cleanup_base_url(draft: &EngineDraft) -> &str {
    let stage_base = draft.cleanup_base_url.trim();
    if stage_base.is_empty() {
        draft.custom_base_url.trim()
    } else {
        stage_base
    }
}

fn validate_custom_probe_base_url(base: &str) -> Result<(), &'static str> {
    if base.trim().is_empty() {
        return Err("a custom service endpoint is required");
    }
    crate::asr::validate_asr_base_url(base)
}

#[cfg(test)]
pub fn apply_engine_draft(settings: &mut Settings, draft: &EngineDraft) {
    settings.asr_provider = draft.asr_provider;
    settings.asr_model = draft_asr_model(draft);
    settings.cleanup_provider = draft.cleanup_provider;
    settings.cleanup_model = draft_cleanup_model(draft);
    settings.custom_asr = draft.custom_asr;
    settings.custom_llm = draft.custom_llm;
    settings.custom_cleanup_parameters = draft.custom_cleanup_parameters.clone();
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
    } else if draft.asr_provider == EngineProvider::DashScope {
        settings.asr_base_url = nonempty(
            &draft.asr_base_url,
            EngineProvider::DashScope.default_base_url(),
        );
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
    if matches!(
        draft.asr_provider,
        EngineProvider::AssemblyAi | EngineProvider::DashScope
    ) {
        return draft.asr_provider.default_asr_model().to_owned();
    }
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

#[cfg(test)]
pub async fn probe_engine_draft(
    draft: &EngineDraft,
    stored: &Settings,
    models_root: &Path,
) -> ProbeResult {
    probe_engine_draft_with_cleanup_demand(draft, stored, models_root, || ()).await
}

pub(crate) async fn probe_engine_draft_with_cleanup_demand<F, G>(
    draft: &EngineDraft,
    stored: &Settings,
    models_root: &Path,
    mut begin_cleanup_demand: F,
) -> ProbeResult
where
    F: FnMut() -> G + Send,
    G: Send,
{
    let includes_asr = draft.probe_stage != ProbeScope::Cleanup;
    let includes_cleanup = draft.probe_stage != ProbeScope::Asr;
    if includes_asr
        && crate::network_policy::is_strict_offline()
        && draft.asr_provider != EngineProvider::OnDevice
    {
        return ProbeResult {
            asr: ProbeStageResult::failure(
                ProbeErrorKind::Provider,
                crate::network_policy::STRICT_OFFLINE_MESSAGE,
            ),
            cleanup: ProbeStageResult::skipped(),
        };
    }
    let asr = if !includes_asr {
        ProbeStageResult::skipped()
    } else if draft.asr_provider == EngineProvider::OnDevice {
        let model = draft_asr_model(draft);
        if !crate::ondevice_models::is_mlx_model(&model) {
            ProbeStageResult::failure(
                ProbeErrorKind::OnDeviceInferenceUnavailable,
                "the selected local model is not supported by the MLX inference runtime",
            )
        } else if ondevice_asr::model_files_are_ready(models_root, &model) {
            let runtime = crate::ondevice_runtime::shared_runtime(models_root)
                .status()
                .await;
            if matches!(runtime.runtime_status.as_str(), "runtime_ready" | "loaded") {
                ProbeStageResult::success()
            } else {
                ProbeStageResult::failure(
                    ProbeErrorKind::OnDeviceInferenceUnavailable,
                    runtime.error.unwrap_or_else(|| {
                        "local MLX runtime handshake is unavailable on this device".into()
                    }),
                )
            }
        } else {
            ProbeStageResult::failure(
                ProbeErrorKind::OnDeviceModelMissing,
                "on-device model is not downloaded",
            )
        }
    } else if draft.asr_provider == EngineProvider::Custom
        && validate_custom_probe_base_url(draft_custom_asr_base_url(draft)).is_err()
    {
        ProbeStageResult::failure(
            ProbeErrorKind::Path,
            "Custom transcription requires an explicit valid HTTP(S) endpoint",
        )
    } else {
        match resolve_asr_key(draft, stored) {
            Ok(key) => match draft_asr_url(draft) {
                Some(url) => {
                    match asr::probe_transcription(&url, probe_wav(), &key, &draft_asr_model(draft))
                        .await
                    {
                        Ok(()) => ProbeStageResult::success(),
                        Err(error) => {
                            ProbeStageResult::failure(classify_asr_error(&error), error.to_string())
                        }
                    }
                }
                None => ProbeStageResult::failure(
                    ProbeErrorKind::Provider,
                    "selected ASR provider has no HTTP endpoint",
                ),
            },
            Err(error) => error,
        }
    };
    if includes_asr && !asr.ok {
        return ProbeResult {
            asr,
            cleanup: ProbeStageResult::skipped(),
        };
    }
    if !includes_cleanup || (!draft.cleanup_enabled && draft.probe_stage != ProbeScope::Cleanup) {
        return ProbeResult {
            asr,
            cleanup: ProbeStageResult::skipped(),
        };
    }
    if crate::network_policy::is_strict_offline()
        && !matches!(
            draft.cleanup_provider,
            EngineProvider::Ollama | EngineProvider::LmStudio | EngineProvider::OnDeviceMlx
        )
    {
        return ProbeResult {
            asr,
            cleanup: ProbeStageResult::failure(
                ProbeErrorKind::Provider,
                crate::network_policy::STRICT_OFFLINE_MESSAGE,
            ),
        };
    }
    if !draft.cleanup_provider.has_llm() {
        return ProbeResult {
            asr,
            cleanup: ProbeStageResult::failure(
                ProbeErrorKind::Provider,
                "OnDevice cannot be used for cleanup",
            ),
        };
    }
    let cleanup = if draft.cleanup_provider == EngineProvider::Custom
        && validate_custom_probe_base_url(draft_custom_cleanup_base_url(draft)).is_err()
    {
        ProbeStageResult::failure(
            ProbeErrorKind::Path,
            "Custom cleanup requires an explicit valid HTTP(S) endpoint",
        )
    } else if draft.cleanup_provider == EngineProvider::Custom {
        match crate::custom_cleanup_parameters::validate(&draft.custom_cleanup_parameters) {
            Ok(()) => match resolve_cleanup_key(draft, stored) {
                Ok(key) => {
                    let runtime = crate::mlx_cleanup_runtime::shared_runtime(
                        models_root,
                        crate::modelstore::shared_manager(),
                    );
                    match llm::cleanup_with_provider_and_native_runtime_and_intent_and_source(
                        draft.cleanup_provider,
                        &runtime,
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
                        None,
                        None,
                        Some(&draft.custom_cleanup_parameters),
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
            },
            Err(message) => ProbeStageResult::failure(ProbeErrorKind::Provider, message),
        }
    } else {
        match resolve_cleanup_key(draft, stored) {
            Ok(key) => {
                let _cleanup_demand = begin_cleanup_demand();
                let runtime = crate::mlx_cleanup_runtime::shared_runtime(
                    models_root,
                    crate::modelstore::shared_manager(),
                );
                match llm::cleanup_with_provider_and_native_runtime_and_intent_and_source(
                    draft.cleanup_provider,
                    &runtime,
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
                    None,
                    None,
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
        }
    };
    ProbeResult { asr, cleanup }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unused_models_root() -> std::path::PathBuf {
        std::env::temp_dir().join("voiceflow-probe-unused-models")
    }

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
        assert_eq!(classify_status_body(404, "not found"), ProbeErrorKind::Path);
        assert_eq!(
            classify_status_body(
                404,
                r#"{"error":{"message":"The model no-such-model does not exist"}}"#,
            ),
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

    #[test]
    fn classify_native_llm_errors_from_owner_categories() {
        use crate::lmstudio_local::LocalError;

        for (error, expected) in [
            (LocalError::Unreachable, ProbeErrorKind::Address),
            (LocalError::CliTimeout, ProbeErrorKind::Address),
            (LocalError::InvalidEndpoint, ProbeErrorKind::Path),
            (LocalError::ModelMismatch, ProbeErrorKind::Model),
            (LocalError::InstanceNotReady, ProbeErrorKind::Model),
            (
                LocalError::InputTooLarge {
                    input_bytes: 4097,
                    max_bytes: 4096,
                },
                ProbeErrorKind::Provider,
            ),
            (LocalError::InstanceSourceUnknown, ProbeErrorKind::Provider),
            (LocalError::Http, ProbeErrorKind::Provider),
        ] {
            let mapped = llm::map_lmstudio_error(error);
            assert_eq!(classify_llm_error(&mapped), expected);
        }
    }

    #[tokio::test]
    async fn probe_maps_connect_failure_to_address() {
        let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
        // Keep an owned, bound, non-listening loopback port alive throughout
        // the real connection attempt instead of relying on an unowned port.
        let unavailable = tokio::net::TcpSocket::new_v4().expect("create IPv4 socket");
        unavailable
            .bind("127.0.0.1:0".parse().expect("loopback socket address"))
            .expect("bind an ephemeral loopback port");
        let endpoint = format!(
            "http://{}",
            unavailable
                .local_addr()
                .expect("read bound loopback address")
        );
        let result = probe_engine_draft(
            &custom_asr_draft(&endpoint, "whisper-1"),
            &Settings::default(),
            &unused_models_root(),
        )
        .await;
        assert!(!result.asr.ok);
        assert_eq!(result.asr.error_kind, Some(ProbeErrorKind::Address));
        assert!(result.cleanup.skipped);
        assert!(!result.succeeded());
    }

    #[tokio::test]
    async fn probe_maps_unauthorized_to_key() {
        let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
        let endpoint = crate::test_http::spawn_response(401, "application/json", b"{}", &[]).await;
        let result = probe_engine_draft(
            &custom_asr_draft(&endpoint, "whisper-1"),
            &Settings::default(),
            &unused_models_root(),
        )
        .await;
        assert_eq!(result.asr.error_kind, Some(ProbeErrorKind::Key));
    }

    #[tokio::test]
    async fn probe_maps_generic_not_found_to_path() {
        let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
        let endpoint = crate::test_http::spawn_response(
            404,
            "application/json",
            br#"{"error":{"message":"resource not found"}}"#,
            &[],
        )
        .await;
        let result = probe_engine_draft(
            &custom_asr_draft(&endpoint, "no-such-model"),
            &Settings::default(),
            &unused_models_root(),
        )
        .await;
        assert_eq!(result.asr.error_kind, Some(ProbeErrorKind::Path));
    }

    #[tokio::test]
    async fn probe_accepts_empty_asr_text_and_skips_disabled_cleanup() {
        let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
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
            &unused_models_root(),
        )
        .await;
        assert!(result.asr.ok);
        assert!(result.cleanup.skipped);
        assert!(result.succeeded());
    }

    #[tokio::test]
    async fn probe_runs_cleanup_against_the_draft_chat_url() {
        let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
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
            "data: {\"choices\":[{\"delta\":{\"content\":\"嗯 那个 你好\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
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
        let result = probe_engine_draft(&draft, &Settings::default(), &unused_models_root()).await;
        assert!(result.asr.ok, "{}", result.asr.message);
        assert!(result.cleanup.ok, "{}", result.cleanup.message);
        assert!(result.succeeded());
    }

    #[tokio::test]
    async fn custom_both_stage_probe_uses_shared_key_and_router_path_prefix() {
        let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
        // This captures two independent stage probes on one Custom connection;
        // the cleanup probe fixture does not consume the ASR probe transcript.
        let (base, requests) = crate::test_http::spawn_responses_with_full_request_capture(
            vec![
                (
                    200,
                    "application/json",
                    br#"{"text":"probe transcript","segments":[],"words":[]}"#.to_vec(),
                ),
                (
                    200,
                    "text/event-stream",
                    "data: {\"choices\":[{\"delta\":{\"content\":\"嗯 那个 你好\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"
                        .as_bytes()
                        .to_vec(),
                ),
            ],
        )
        .await;
        let draft = EngineDraft {
            asr_provider: EngineProvider::Custom,
            cleanup_provider: EngineProvider::Custom,
            custom_base_url: format!("{base}/llm/v1"),
            asr_model: "groq/whisper-large-v3-turbo".into(),
            cleanup_model: "groq/openai/gpt-oss-20b".into(),
            provider_keys: BTreeMap::from([("custom".into(), "synthetic-router-key".into())]),
            custom_cleanup_parameters: BTreeMap::from([(
                "temperature".into(),
                serde_json::json!(0.2),
            )]),
            cleanup_enabled: true,
            ..EngineDraft::default()
        };

        let result = probe_engine_draft(&draft, &Settings::default(), &unused_models_root()).await;
        assert!(result.asr.ok, "{}", result.asr.message);
        assert!(result.cleanup.ok, "{}", result.cleanup.message);
        let requests = requests.await.expect("both stage requests captured");
        assert_eq!(requests.len(), 2);

        let asr_request = String::from_utf8_lossy(&requests[0]);
        let cleanup_request = String::from_utf8_lossy(&requests[1]);
        assert!(asr_request.starts_with("POST /llm/v1/audio/transcriptions HTTP/1.1\r\n"));
        assert!(cleanup_request.starts_with("POST /llm/v1/chat/completions HTTP/1.1\r\n"));
        for request in [&asr_request, &cleanup_request] {
            assert!(request
                .to_ascii_lowercase()
                .contains("authorization: bearer synthetic-router-key"));
        }
        let asr_multipart = asr_request.split_once("\r\n\r\n").unwrap().1;
        assert!(asr_multipart.contains("groq/whisper-large-v3-turbo"));
        assert!(asr_multipart.contains("verbose_json"));
        assert!(asr_multipart.contains("timestamp_granularities[]"));
        assert!(!asr_multipart.contains("name=\"temperature\""));

        let cleanup_body = cleanup_request.split_once("\r\n\r\n").unwrap().1;
        let cleanup_json: serde_json::Value =
            serde_json::from_str(cleanup_body).expect("cleanup request JSON");
        assert_eq!(cleanup_json["model"], "groq/openai/gpt-oss-20b");
        assert_eq!(cleanup_json["temperature"], serde_json::json!(0.2));
    }

    #[tokio::test]
    async fn custom_stage_probe_scope_sends_only_the_selected_stage_request() {
        let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
        for (scope, content_type, response_body, expected_path) in [
            (
                ProbeScope::Asr,
                "application/json",
                br#"{"text":"probe transcript","segments":[],"words":[]}"#.to_vec(),
                "/router/v1/audio/transcriptions",
            ),
            (
                ProbeScope::Cleanup,
                "text/event-stream",
                "data: {\"choices\":[{\"delta\":{\"content\":\"嗯 那个 你好\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"
                    .as_bytes()
                    .to_vec(),
                "/router/v1/chat/completions",
            ),
        ] {
            let (host, request) = crate::test_http::spawn_response_with_full_request_capture(
                200,
                content_type,
                response_body,
                &[],
            )
            .await;
            let draft = EngineDraft {
                asr_provider: EngineProvider::Custom,
                cleanup_provider: EngineProvider::Custom,
                custom_base_url: format!("{host}/router/v1"),
                asr_model: "groq/whisper-large-v3-turbo".into(),
                cleanup_model: "groq/openai/gpt-oss-20b".into(),
                provider_keys: BTreeMap::from([("custom".into(), "synthetic-router-key".into())]),
                cleanup_enabled: true,
                probe_stage: scope,
                ..EngineDraft::default()
            };

            let result =
                probe_engine_draft(&draft, &Settings::default(), &unused_models_root()).await;
            match scope {
                ProbeScope::Asr => {
                    assert!(result.asr.ok, "{}", result.asr.message);
                    assert!(result.cleanup.skipped);
                }
                ProbeScope::Cleanup => {
                    assert!(result.asr.skipped);
                    assert!(result.cleanup.ok, "{}", result.cleanup.message);
                }
                ProbeScope::Both => unreachable!("test only covers explicit stage scopes"),
            }
            let raw_request = request.await.expect("selected stage captured");
            let request = String::from_utf8_lossy(&raw_request);
            assert!(request.starts_with(&format!("POST {expected_path} HTTP/1.1\r\n")));
        }
    }

    #[tokio::test]
    async fn probe_accepts_deepgram_listen_json() {
        let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
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
        let result = probe_engine_draft(&draft, &Settings::default(), &unused_models_root()).await;
        assert!(result.asr.ok, "{}", result.asr.message);
        assert!(result.succeeded());
    }

    #[tokio::test]
    async fn probe_accepts_anthropic_messages_json() {
        let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
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
            r#"{"content":[{"type":"text","text":"嗯 那个 你好"}],"stop_reason":"end_turn"}"#
                .as_bytes(),
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
        let result = probe_engine_draft(&draft, &Settings::default(), &unused_models_root()).await;
        assert!(result.asr.ok, "{}", result.asr.message);
        assert!(result.cleanup.ok, "{}", result.cleanup.message);
        assert!(result.succeeded());
    }

    #[tokio::test]
    async fn probe_on_device_missing_model_is_not_missing_key_or_http() {
        let draft = EngineDraft {
            asr_provider: EngineProvider::OnDevice,
            asr_model: "qwen3-asr-0.6b".into(),
            cleanup_enabled: false,
            ..EngineDraft::default()
        };
        let missing_root = std::env::temp_dir().join(format!(
            "voiceflow-on-device-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&missing_root);
        std::fs::create_dir_all(&missing_root).unwrap();
        let result = probe_engine_draft(&draft, &Settings::default(), &missing_root).await;
        assert!(!result.asr.ok);
        assert_eq!(
            result.asr.error_kind,
            Some(ProbeErrorKind::OnDeviceModelMissing)
        );
        assert_ne!(result.asr.error_kind, Some(ProbeErrorKind::MissingKey));
        assert_ne!(result.asr.error_kind, Some(ProbeErrorKind::Provider));
        assert_ne!(result.asr.error_kind, Some(ProbeErrorKind::Model));
        let _ = std::fs::remove_dir_all(missing_root);
    }

    #[tokio::test]
    async fn probe_on_device_files_ready_reports_missing_inference_runtime() {
        let draft = EngineDraft {
            asr_provider: EngineProvider::OnDevice,
            asr_model: "sensevoice-small".into(),
            cleanup_enabled: false,
            ..EngineDraft::default()
        };
        let models_root = std::env::temp_dir().join(format!(
            "voiceflow-on-device-ready-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let dir = models_root.join("sensevoice-small");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("model.int8.onnx"), b"fixture-onnx").unwrap();
        std::fs::write(dir.join("tokens.txt"), b"fixture-tokens").unwrap();
        std::fs::write(
            crate::ondevice_asr::archive_sha_path(&models_root, "sensevoice-small"),
            crate::ondevice_models::SENSEVOICE_ARCHIVE_SHA256,
        )
        .unwrap();
        let result = probe_engine_draft(&draft, &Settings::default(), &models_root).await;
        assert!(!result.asr.ok, "downloaded files are not inference-ready");
        assert_eq!(
            result.asr.error_kind,
            Some(ProbeErrorKind::OnDeviceInferenceUnavailable)
        );
        assert!(!result.succeeded());
        let _ = std::fs::remove_dir_all(models_root);
    }

    #[tokio::test]
    async fn probe_llm_only_cleanup_without_key_is_missing_key() {
        let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
        for cleanup_provider in [EngineProvider::DeepSeek, EngineProvider::Anthropic] {
            let asr_endpoint = crate::test_http::spawn_response(
                200,
                "application/json",
                br#"{"text":"hello","segments":[],"words":[]}"#,
                &[],
            )
            .await;
            let draft = EngineDraft {
                asr_provider: EngineProvider::Custom,
                cleanup_provider,
                asr_base_url: asr_endpoint.clone(),
                cleanup_base_url: "http://127.0.0.1:1".into(),
                custom_base_url: asr_endpoint,
                asr_model: "whisper-1".into(),
                cleanup_model: cleanup_provider.default_llm_model().into(),
                asr_api_key: "asr".into(),
                cleanup_enabled: true,
                ..EngineDraft::default()
            };
            let result =
                probe_engine_draft(&draft, &Settings::default(), &unused_models_root()).await;
            assert!(result.asr.ok, "{}", result.asr.message);
            assert_eq!(
                result.cleanup.error_kind,
                Some(ProbeErrorKind::MissingKey),
                "{cleanup_provider:?} cleanup must require a key, not treat !has_http_asr as OnDevice"
            );
        }
    }

    #[test]
    fn on_device_mlx_cleanup_resolves_without_an_endpoint_or_secret() {
        let draft = EngineDraft {
            cleanup_provider: EngineProvider::OnDeviceMlx,
            cleanup_model: EngineProvider::OnDeviceMlx.default_llm_model().into(),
            ..EngineDraft::default()
        };
        assert_eq!(
            resolve_cleanup_key(&draft, &Settings::default()).unwrap(),
            ""
        );
        assert_eq!(draft_cleanup_url(&draft), "");
    }

    #[test]
    fn generic_not_found_probe_error_is_a_path_error_unless_it_names_a_model() {
        assert_eq!(classify_status_body(404, "not found"), ProbeErrorKind::Path);
        assert_eq!(
            classify_status_body(404, "model not found"),
            ProbeErrorKind::Model
        );
    }

    #[test]
    fn custom_credentials_do_not_follow_a_changed_host_without_a_typed_replacement() {
        let stored = Settings {
            custom_base_url: "https://old.example/v1".into(),
            provider_api_keys: std::collections::BTreeMap::from([(
                "custom".into(),
                "old-key".into(),
            )]),
            asr_api_key: "old-key".into(),
            ..Settings::default()
        };
        let mut draft = EngineDraft {
            cleanup_provider: EngineProvider::Custom,
            custom_base_url: "https://new.example/v1".into(),
            cleanup_model: "provider/model".into(),
            ..EngineDraft::default()
        };
        assert_eq!(
            resolve_cleanup_key(&draft, &stored).unwrap_err().error_kind,
            Some(ProbeErrorKind::MissingKey)
        );
        draft
            .provider_keys
            .insert("custom".into(), "new-key".into());
        assert_eq!(resolve_cleanup_key(&draft, &stored).unwrap(), "new-key");
    }

    #[test]
    fn custom_credentials_do_not_follow_scheme_or_port_changes() {
        let stored = Settings {
            custom_base_url: "https://router.example/v1".into(),
            provider_api_keys: std::collections::BTreeMap::from([(
                "custom".into(),
                "old-key".into(),
            )]),
            ..Settings::default()
        };

        for custom_base_url in ["https://router.example:8443/v1", "http://router.example/v1"] {
            let draft = EngineDraft {
                cleanup_provider: EngineProvider::Custom,
                custom_base_url: custom_base_url.into(),
                cleanup_model: "provider/model".into(),
                ..EngineDraft::default()
            };
            assert_eq!(
                resolve_cleanup_key(&draft, &stored).unwrap_err().error_kind,
                Some(ProbeErrorKind::MissingKey)
            );
        }

        let same_origin = EngineDraft {
            cleanup_provider: EngineProvider::Custom,
            custom_base_url: "https://router.example:443/chat/completions".into(),
            cleanup_model: "provider/model".into(),
            ..EngineDraft::default()
        };
        assert_eq!(
            resolve_cleanup_key(&same_origin, &stored).unwrap(),
            "old-key"
        );
    }

    #[test]
    fn custom_stage_url_resolution_does_not_fall_back_to_the_other_stage() {
        let draft = EngineDraft {
            asr_provider: EngineProvider::Custom,
            cleanup_provider: EngineProvider::Custom,
            asr_base_url: String::new(),
            cleanup_base_url: "http://127.0.0.1:1/v1".into(),
            ..EngineDraft::default()
        };
        assert_eq!(draft_custom_asr_base_url(&draft), "");
        assert_eq!(draft_asr_url(&draft), None);

        let draft = EngineDraft {
            asr_provider: EngineProvider::Custom,
            cleanup_provider: EngineProvider::Custom,
            asr_base_url: "http://127.0.0.1:1/v1".into(),
            cleanup_base_url: String::new(),
            ..EngineDraft::default()
        };
        assert_eq!(draft_custom_cleanup_base_url(&draft), "");
        assert_eq!(draft_cleanup_url(&draft), "");
        assert!(validate_custom_probe_base_url("").is_err());
        assert!(validate_custom_probe_base_url("ftp://router.example/v1").is_err());
        assert!(validate_custom_probe_base_url("https://router.example/v1").is_ok());
    }

    #[test]
    fn custom_cleanup_probe_admission_does_not_borrow_the_groq_asr_url() {
        let draft = EngineDraft {
            asr_provider: EngineProvider::Groq,
            asr_base_url: "https://api.groq.com/openai/v1".into(),
            cleanup_provider: EngineProvider::Custom,
            custom_base_url: String::new(),
            cleanup_base_url: String::new(),
            cleanup_api_key: "synthetic-custom-key".into(),
            cleanup_enabled: true,
            probe_stage: ProbeScope::Cleanup,
            ..EngineDraft::default()
        };

        assert_eq!(draft_custom_cleanup_base_url(&draft), "");
        assert_eq!(draft_cleanup_url(&draft), "");
        assert!(validate_custom_probe_base_url(draft_custom_cleanup_base_url(&draft)).is_err());
        assert_eq!(
            draft_asr_url(&draft).as_deref(),
            Some("https://api.groq.com/openai/v1/audio/transcriptions")
        );
    }

    #[test]
    fn custom_credentials_bind_to_each_stage_resolved_destination() {
        let stored = Settings {
            custom_base_url: "https://stored.example/v1".into(),
            provider_api_keys: std::collections::BTreeMap::from([(
                "custom".into(),
                "stored-key".into(),
            )]),
            ..Settings::default()
        };
        let draft = EngineDraft {
            asr_provider: EngineProvider::Custom,
            cleanup_provider: EngineProvider::Custom,
            custom_base_url: "https://stored.example/v1".into(),
            asr_base_url: "https://asr-override.example/v1".into(),
            cleanup_base_url: "https://cleanup-override.example/v1".into(),
            asr_model: "whisper-1".into(),
            cleanup_model: "provider/model".into(),
            ..EngineDraft::default()
        };

        assert_eq!(
            resolve_asr_key(&draft, &stored).unwrap_err().error_kind,
            Some(ProbeErrorKind::MissingKey),
            "the stored Custom key must not follow the ASR destination override"
        );
        assert_eq!(
            resolve_cleanup_key(&draft, &stored).unwrap_err().error_kind,
            Some(ProbeErrorKind::MissingKey),
            "the stored Custom key must not follow the cleanup destination override"
        );
    }

    #[test]
    fn recovered_custom_key_is_limited_to_its_active_stage_and_origin() {
        let mut stored = Settings {
            asr_provider: EngineProvider::Custom,
            asr_base_url: "https://router-a.example/v1/asr".into(),
            cleanup_enabled: false,
            cleanup_api_key: "synthetic-cleanup-legacy-key".into(),
            ..Settings::default()
        };
        let scope = crate::store::custom_credential_recovery_scope(
            &stored,
            crate::store::CustomCredentialPurpose::Asr,
        )
        .unwrap();
        crate::store::adopt_interactive_custom_credential(
            &mut stored,
            "synthetic-session-asr-key".into(),
            scope,
        );

        let asr_same_origin = EngineDraft {
            asr_provider: EngineProvider::Custom,
            asr_base_url: "https://router-a.example/v1/audio/transcriptions".into(),
            custom_base_url: "https://router-a.example/v1".into(),
            ..EngineDraft::default()
        };
        assert_eq!(
            resolve_asr_key(&asr_same_origin, &stored).unwrap(),
            "synthetic-session-asr-key"
        );

        let cleanup_same_origin = EngineDraft {
            cleanup_provider: EngineProvider::Custom,
            cleanup_base_url: "https://router-a.example/v1/chat/completions".into(),
            custom_base_url: "https://router-a.example/v1".into(),
            ..EngineDraft::default()
        };
        assert_eq!(
            resolve_cleanup_key(&cleanup_same_origin, &stored)
                .unwrap_err()
                .error_kind,
            Some(ProbeErrorKind::MissingKey),
            "an ASR-only recovery cannot flow to cleanup or its legacy alias"
        );

        let asr_other_origin = EngineDraft {
            asr_provider: EngineProvider::Custom,
            asr_base_url: "https://router-b.example/v1/audio/transcriptions".into(),
            custom_base_url: "https://router-b.example/v1".into(),
            ..EngineDraft::default()
        };
        assert_eq!(
            resolve_asr_key(&asr_other_origin, &stored)
                .unwrap_err()
                .error_kind,
            Some(ProbeErrorKind::MissingKey)
        );
    }

    #[test]
    fn revoked_custom_recovery_cannot_reappear_through_stage_legacy_fallback() {
        let mut stored = Settings {
            asr_provider: EngineProvider::Custom,
            asr_base_url: "https://router-a.example/v1/asr".into(),
            cleanup_enabled: false,
            cleanup_provider: EngineProvider::Groq,
            custom_base_url: "https://router-a.example/v1".into(),
            asr_api_key: "synthetic-old-asr-alias".into(),
            cleanup_api_key: "synthetic-cleanup-key-from-router-b".into(),
            ..Settings::default()
        };
        let scope = crate::store::custom_credential_recovery_scope(
            &stored,
            crate::store::CustomCredentialPurpose::Asr,
        )
        .unwrap();
        crate::store::adopt_interactive_custom_credential(
            &mut stored,
            "synthetic-session-key-from-router-a".into(),
            scope,
        );

        // This is the production revocation path: active Custom-purpose set
        // changes, so the process-only value is removed and rejection becomes sticky.
        stored.asr_provider = EngineProvider::Groq;
        stored.cleanup_enabled = true;
        stored.cleanup_provider = EngineProvider::Custom;
        stored.cleanup_base_url = "https://router-b.example/v1/cleanup".into();
        assert!(stored.clear_stale_session_custom_credential());
        assert!(!stored.session_only_custom_credential);
        assert!(stored.session_custom_credential_rejected);

        // The shared Custom origin remains router A, but the retained cleanup
        // alias came from router B. A later Custom probe must not revive it.
        let cleanup_probe = EngineDraft {
            cleanup_provider: EngineProvider::Custom,
            custom_base_url: "https://router-a.example/v1".into(),
            cleanup_base_url: String::new(),
            cleanup_model: "provider/model".into(),
            ..EngineDraft::default()
        };
        assert_eq!(
            resolve_cleanup_key(&cleanup_probe, &stored)
                .unwrap_err()
                .error_kind,
            Some(ProbeErrorKind::MissingKey)
        );

        let asr_probe = EngineDraft {
            asr_provider: EngineProvider::Custom,
            custom_base_url: "https://router-a.example/v1".into(),
            asr_base_url: String::new(),
            ..EngineDraft::default()
        };
        assert_eq!(
            resolve_asr_key(&asr_probe, &stored).unwrap_err().error_kind,
            Some(ProbeErrorKind::MissingKey)
        );
    }

    #[test]
    fn custom_stage_key_fields_do_not_fall_through_to_other_providers() {
        let mut stored = Settings::default();
        stored
            .provider_api_keys
            .insert("groq".into(), "stored-groq-key".into());
        let mut draft = EngineDraft::default();
        draft
            .provider_keys
            .insert("custom".into(), "typed-custom-key".into());
        draft.asr_api_key = "typed-custom-key".into();
        draft.cleanup_api_key = "typed-custom-key".into();

        assert_eq!(resolve_asr_key(&draft, &stored).unwrap(), "stored-groq-key");
        assert_eq!(
            resolve_cleanup_key(&draft, &stored).unwrap(),
            "stored-groq-key"
        );
    }

    #[test]
    fn custom_stages_do_not_reuse_a_saved_groq_credential() {
        let stored = Settings {
            api_key: "synthetic-groq-key".into(),
            provider_api_keys: BTreeMap::from([("groq".into(), "synthetic-groq-key".into())]),
            ..Settings::default()
        };
        let draft = EngineDraft {
            asr_provider: EngineProvider::Custom,
            cleanup_provider: EngineProvider::Custom,
            custom_base_url: "https://router.example/llm/v1".into(),
            ..EngineDraft::default()
        };

        assert_eq!(
            resolve_asr_key(&draft, &stored).unwrap_err().error_kind,
            Some(ProbeErrorKind::MissingKey)
        );
        assert_eq!(
            resolve_cleanup_key(&draft, &stored).unwrap_err().error_kind,
            Some(ProbeErrorKind::MissingKey)
        );
    }

    #[tokio::test]
    async fn non_custom_cleanup_probe_ignores_stashed_custom_parameters() {
        let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
        let (endpoint, request) = crate::test_http::spawn_response_with_request_capture(
            200,
            "text/event-stream",
            "data: {\"choices\":[{\"delta\":{\"content\":\"嗯 那个 你好\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"
                .as_bytes()
                .to_vec(),
            &[],
        )
        .await;
        let draft = EngineDraft {
            asr_provider: EngineProvider::OnDevice,
            asr_model: "not-an-mlx-model".into(),
            cleanup_provider: EngineProvider::DeepSeek,
            cleanup_base_url: endpoint,
            cleanup_model: "provider/model".into(),
            cleanup_enabled: true,
            probe_stage: ProbeScope::Cleanup,
            provider_keys: [("deepseek".into(), "synthetic-key".into())]
                .into_iter()
                .collect(),
            custom_cleanup_parameters: [
                ("temperature".into(), serde_json::json!(0.8)),
                (
                    "vendor_extension".into(),
                    serde_json::json!({"enabled": false}),
                ),
            ]
            .into_iter()
            .collect(),
            ..EngineDraft::default()
        };

        let result = probe_engine_draft(&draft, &Settings::default(), &unused_models_root()).await;
        assert!(result.asr.skipped);
        let body: serde_json::Value =
            serde_json::from_slice(&request.await.expect("cleanup request captured"))
                .expect("valid JSON request");
        assert_eq!(body["temperature"], serde_json::json!(0.0));
        assert_eq!(body["max_completion_tokens"], serde_json::json!(4096));
        assert!(!body
            .as_object()
            .is_some_and(|fields| fields.contains_key("vendor_extension")));
    }

    #[tokio::test]
    async fn cleanup_only_probe_skips_invalid_asr_draft_and_runs_only_its_validation() {
        let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
        let draft = EngineDraft {
            asr_provider: EngineProvider::OnDevice,
            asr_model: "not-an-mlx-model".into(),
            cleanup_provider: EngineProvider::Custom,
            cleanup_model: "provider/model".into(),
            custom_base_url: "https://provider.example/v1".into(),
            cleanup_enabled: true,
            probe_stage: ProbeScope::Cleanup,
            custom_cleanup_parameters: [(
                "model".into(),
                serde_json::Value::String("must be rejected".into()),
            )]
            .into_iter()
            .collect(),
            ..EngineDraft::default()
        };
        let result = probe_engine_draft(&draft, &Settings::default(), &unused_models_root()).await;
        assert!(result.asr.skipped);
        assert!(!result.cleanup.skipped);
        assert_eq!(result.cleanup.error_kind, Some(ProbeErrorKind::Provider));
        assert!(result.cleanup.message.contains("protocol or output"));
    }

    #[tokio::test]
    async fn asr_only_probe_skips_invalid_cleanup_parameters() {
        let draft = EngineDraft {
            asr_provider: EngineProvider::OnDevice,
            asr_model: "not-an-mlx-model".into(),
            cleanup_provider: EngineProvider::Custom,
            custom_cleanup_parameters: [("stream".into(), serde_json::Value::Bool(false))]
                .into_iter()
                .collect(),
            probe_stage: ProbeScope::Asr,
            cleanup_enabled: true,
            ..EngineDraft::default()
        };
        let result = probe_engine_draft(&draft, &Settings::default(), &unused_models_root()).await;
        assert!(!result.asr.skipped);
        assert_eq!(
            result.asr.error_kind,
            Some(ProbeErrorKind::OnDeviceInferenceUnavailable)
        );
        assert!(result.cleanup.skipped);
    }

    #[tokio::test]
    async fn probe_llm_only_asr_is_missing_key_not_on_device() {
        let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
        for asr_provider in [EngineProvider::DeepSeek, EngineProvider::Anthropic] {
            let draft = EngineDraft {
                asr_provider,
                cleanup_enabled: false,
                ..EngineDraft::default()
            };
            let result =
                probe_engine_draft(&draft, &Settings::default(), &unused_models_root()).await;
            assert!(!result.asr.ok);
            assert_eq!(
                result.asr.error_kind,
                Some(ProbeErrorKind::MissingKey),
                "{asr_provider:?} must not take the OnDevice empty-key path"
            );
            assert_ne!(
                result.asr.error_kind,
                Some(ProbeErrorKind::OnDeviceModelMissing)
            );
        }
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
