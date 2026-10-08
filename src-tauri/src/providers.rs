use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum EngineProvider {
    #[default]
    #[serde(rename = "groq")]
    Groq,
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "deepgram")]
    Deepgram,
    #[serde(rename = "siliconflow")]
    SiliconFlow,
    #[serde(rename = "fireworks")]
    Fireworks,
    #[serde(rename = "mistral")]
    Mistral,
    #[serde(rename = "soniox")]
    Soniox,
    #[serde(rename = "assemblyai")]
    AssemblyAi,
    #[serde(rename = "dashscope")]
    DashScope,
    #[serde(rename = "deepseek")]
    DeepSeek,
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "ollama")]
    Ollama,
    #[serde(rename = "local_whisper")]
    LocalWhisper,
    #[serde(rename = "on_device")]
    OnDevice,
    #[serde(rename = "custom")]
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderProtocol {
    OpenAiCompat,
    Deepgram,
    SonioxWebSocket,
    AssemblyAi,
    QwenMessageWebSocket,
    Anthropic,
    OnDeviceLocal,
}

impl EngineProvider {
    pub const ALL: [Self; 15] = [
        Self::Groq,
        Self::OpenAi,
        Self::Deepgram,
        Self::SiliconFlow,
        Self::Fireworks,
        Self::Mistral,
        Self::Soniox,
        Self::AssemblyAi,
        Self::DashScope,
        Self::DeepSeek,
        Self::Anthropic,
        Self::Ollama,
        Self::LocalWhisper,
        Self::OnDevice,
        Self::Custom,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Groq => "groq",
            Self::OpenAi => "openai",
            Self::Deepgram => "deepgram",
            Self::SiliconFlow => "siliconflow",
            Self::Fireworks => "fireworks",
            Self::Mistral => "mistral",
            Self::Soniox => "soniox",
            Self::AssemblyAi => "assemblyai",
            Self::DashScope => "dashscope",
            Self::DeepSeek => "deepseek",
            Self::Anthropic => "anthropic",
            Self::Ollama => "ollama",
            Self::LocalWhisper => "local_whisper",
            Self::OnDevice => "on_device",
            Self::Custom => "custom",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|provider| provider.as_str() == value.trim())
    }

    pub fn is_groq(self) -> bool {
        matches!(self, Self::Groq)
    }

    pub fn is_custom(self) -> bool {
        matches!(self, Self::Custom)
    }

    pub fn has_asr(self) -> bool {
        matches!(
            self,
            Self::Groq
                | Self::OpenAi
                | Self::Deepgram
                | Self::SiliconFlow
                | Self::Fireworks
                | Self::Mistral
                | Self::Soniox
                | Self::AssemblyAi
                | Self::DashScope
                | Self::LocalWhisper
                | Self::OnDevice
                | Self::Custom
        )
    }

    pub fn has_llm(self) -> bool {
        matches!(
            self,
            Self::Groq
                | Self::OpenAi
                | Self::SiliconFlow
                | Self::DeepSeek
                | Self::Anthropic
                | Self::Ollama
                | Self::Custom
        )
    }

    pub fn has_http_asr(self) -> bool {
        self.has_asr() && !matches!(self, Self::OnDevice | Self::Soniox | Self::DashScope)
    }

    pub fn allows_empty_key(self) -> bool {
        matches!(
            self,
            Self::Ollama | Self::LocalWhisper | Self::OnDevice | Self::Custom
        )
    }

    pub fn protocol(self) -> ProviderProtocol {
        match self {
            Self::Deepgram => ProviderProtocol::Deepgram,
            Self::Soniox => ProviderProtocol::SonioxWebSocket,
            Self::AssemblyAi => ProviderProtocol::AssemblyAi,
            Self::DashScope => ProviderProtocol::QwenMessageWebSocket,
            Self::Anthropic => ProviderProtocol::Anthropic,
            Self::OnDevice => ProviderProtocol::OnDeviceLocal,
            _ => ProviderProtocol::OpenAiCompat,
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::Groq => "https://api.groq.com/openai/v1",
            Self::OpenAi => "https://api.openai.com/v1",
            Self::Deepgram => "https://api.deepgram.com",
            Self::SiliconFlow => "https://api.siliconflow.cn/v1",
            Self::Fireworks => "https://audio-turbo.api.fireworks.ai",
            Self::Mistral => "https://api.mistral.ai",
            Self::Soniox => crate::asr::SONIOX_WEBSOCKET_ENDPOINT,
            Self::AssemblyAi => "https://dictation.assemblyai.com",
            Self::DashScope => "https://dashscope.aliyuncs.com",
            Self::DeepSeek => "https://api.deepseek.com",
            Self::Anthropic => "https://api.anthropic.com",
            Self::Ollama => "http://127.0.0.1:11434/v1",
            Self::LocalWhisper => "http://127.0.0.1:9000/v1",
            Self::OnDevice => "",
            Self::Custom => "",
        }
    }

    pub fn default_asr_model(self) -> &'static str {
        match self {
            Self::Groq => "whisper-large-v3-turbo",
            Self::OpenAi => "gpt-transcribe",
            Self::Deepgram => "nova-3",
            Self::SiliconFlow => "FunAudioLLM/SenseVoiceSmall",
            Self::Fireworks => "whisper-v3-turbo",
            Self::Mistral => "voxtral-mini-2602",
            Self::Soniox => crate::asr::SONIOX_MODEL,
            Self::AssemblyAi => crate::asr::ASSEMBLYAI_SYNC_MODEL,
            Self::DashScope => crate::asr::QWEN_AUDIO_MESSAGE_MODEL,
            Self::LocalWhisper => "whisper-large-v3-turbo",
            Self::OnDevice => crate::ondevice_models::DEFAULT_LOCAL_MODEL_ID,
            Self::Custom => "whisper-1",
            _ => "",
        }
    }

    pub fn default_llm_model(self) -> &'static str {
        match self {
            Self::Groq => crate::llm::MODEL,
            Self::OpenAi => "gpt-4o-mini",
            Self::SiliconFlow => "deepseek-ai/DeepSeek-V3",
            Self::DeepSeek => "deepseek-chat",
            Self::Anthropic => "claude-sonnet-4-5",
            Self::Ollama => "qwen3.5:4b",
            Self::Custom => "",
            _ => "",
        }
    }

    pub fn keychain_account(self) -> &'static str {
        match self {
            Self::Groq => "groq_api_key",
            Self::OpenAi => "provider_openai",
            Self::Deepgram => "provider_deepgram",
            Self::SiliconFlow => "provider_siliconflow",
            Self::Fireworks => "provider_fireworks",
            Self::Mistral => "provider_mistral",
            Self::Soniox => "provider_soniox",
            Self::AssemblyAi => "provider_assemblyai",
            Self::DashScope => "provider_dashscope",
            Self::DeepSeek => "provider_deepseek",
            Self::Anthropic => "provider_anthropic",
            Self::Ollama => "provider_ollama",
            Self::LocalWhisper => "provider_local_whisper",
            Self::OnDevice => "provider_on_device",
            Self::Custom => "provider_custom",
        }
    }
}

pub fn host_of(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let rest = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let hostport = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    let host = if let Some(end) = hostport.strip_prefix('[') {
        end.split_once(']').map(|(host, _)| host).unwrap_or(end)
    } else {
        match hostport.rsplit_once(':') {
            Some((candidate, port)) if port.chars().all(|ch| ch.is_ascii_digit()) => candidate,
            _ => hostport,
        }
    };
    host.to_ascii_lowercase()
}

pub fn is_loopback_url(url: &str) -> bool {
    matches!(host_of(url).as_str(), "127.0.0.1" | "localhost" | "::1")
}

pub fn resolve_asr_endpoint(provider: EngineProvider, base: &str) -> Option<String> {
    if provider == EngineProvider::Soniox {
        return Some(crate::asr::SONIOX_WEBSOCKET_ENDPOINT.to_owned());
    }
    if provider == EngineProvider::AssemblyAi {
        return Some(crate::asr::ASSEMBLYAI_SYNC_ENDPOINT.to_owned());
    }
    if provider == EngineProvider::DashScope {
        let origin = base.trim().trim_end_matches('/');
        let host = host_of(origin);
        let allowed = matches!(
            (origin, host.as_str()),
            ("https://dashscope.aliyuncs.com", "dashscope.aliyuncs.com")
                | (
                    "https://dashscope-intl.aliyuncs.com",
                    "dashscope-intl.aliyuncs.com"
                )
        );
        return allowed.then(|| {
            format!(
                "wss://{host}/api-ws/v1/inference",
                host = if host == "dashscope-intl.aliyuncs.com" {
                    host.as_str()
                } else {
                    "dashscope.aliyuncs.com"
                }
            )
        });
    }
    if !provider.has_http_asr() {
        return None;
    }
    let fallback = provider.default_base_url();
    let base = if base.trim().is_empty() {
        fallback
    } else {
        base.trim()
    };
    Some(match provider.protocol() {
        ProviderProtocol::Deepgram => {
            if base.contains("/listen") {
                base.trim_end_matches('/').to_owned()
            } else {
                format!("{}/v1/listen", base.trim_end_matches('/'))
            }
        }
        ProviderProtocol::OnDeviceLocal => return None,
        ProviderProtocol::AssemblyAi | ProviderProtocol::QwenMessageWebSocket => {
            unreachable!("handled above")
        }
        _ if provider.is_groq() => crate::asr::resolve_transcription_url(""),
        _ => crate::asr::resolve_transcription_url(base),
    })
}

pub fn resolve_llm_endpoint(provider: EngineProvider, base: &str) -> String {
    if !provider.has_llm() {
        return String::new();
    }
    let fallback = provider.default_base_url();
    let base = if base.trim().is_empty() {
        fallback
    } else {
        base.trim()
    };
    match provider.protocol() {
        ProviderProtocol::Anthropic => {
            if base.contains("/messages") {
                base.trim_end_matches('/').to_owned()
            } else {
                format!("{}/v1/messages", base.trim_end_matches('/'))
            }
        }
        _ if provider.is_groq() => crate::llm::resolve_chat_url(""),
        _ => crate::llm::resolve_chat_url(base),
    }
}

pub fn infer_provider_from_host(url: &str) -> Option<EngineProvider> {
    let host = host_of(url);
    if host.is_empty() {
        return None;
    }
    let named = [
        ("api.groq.com", EngineProvider::Groq),
        ("api.openai.com", EngineProvider::OpenAi),
        ("api.deepgram.com", EngineProvider::Deepgram),
        ("api.siliconflow.cn", EngineProvider::SiliconFlow),
        ("api.deepseek.com", EngineProvider::DeepSeek),
        ("api.anthropic.com", EngineProvider::Anthropic),
        ("dictation.assemblyai.com", EngineProvider::AssemblyAi),
        ("sync.assemblyai.com", EngineProvider::AssemblyAi),
        ("dashscope.aliyuncs.com", EngineProvider::DashScope),
        ("dashscope-intl.aliyuncs.com", EngineProvider::DashScope),
    ]
    .into_iter()
    .find(|(candidate, _)| host == *candidate || host.ends_with(&format!(".{candidate}")));
    if let Some((_, provider)) = named {
        return Some(provider);
    }
    if is_loopback_url(url) {
        if url.contains("11434") {
            return Some(EngineProvider::Ollama);
        }
        if url.contains("9000") {
            return Some(EngineProvider::LocalWhisper);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_filters_match_the_catalog() {
        let asr: Vec<_> = EngineProvider::ALL
            .into_iter()
            .filter(|provider| provider.has_asr())
            .collect();
        let llm: Vec<_> = EngineProvider::ALL
            .into_iter()
            .filter(|provider| provider.has_llm())
            .collect();
        assert_eq!(
            asr,
            [
                EngineProvider::Groq,
                EngineProvider::OpenAi,
                EngineProvider::Deepgram,
                EngineProvider::SiliconFlow,
                EngineProvider::Fireworks,
                EngineProvider::Mistral,
                EngineProvider::Soniox,
                EngineProvider::AssemblyAi,
                EngineProvider::DashScope,
                EngineProvider::LocalWhisper,
                EngineProvider::OnDevice,
                EngineProvider::Custom
            ]
        );
        assert!(!llm.contains(&EngineProvider::Deepgram));
        assert!(!asr.contains(&EngineProvider::DeepSeek));
        assert!(!asr.contains(&EngineProvider::Anthropic));
    }

    #[test]
    fn openai_and_local_whisper_defaults_match_the_catalog() {
        assert_eq!(EngineProvider::OpenAi.default_asr_model(), "gpt-transcribe");
        assert_eq!(
            EngineProvider::LocalWhisper.default_asr_model(),
            "whisper-large-v3-turbo"
        );
    }

    #[test]
    fn infers_named_hosts_from_legacy_custom_urls() {
        assert_eq!(
            infer_provider_from_host("https://api.openai.com/v1"),
            Some(EngineProvider::OpenAi)
        );
        assert_eq!(
            infer_provider_from_host("https://api.deepseek.com"),
            Some(EngineProvider::DeepSeek)
        );
        assert_eq!(
            infer_provider_from_host("http://127.0.0.1:11434/v1"),
            Some(EngineProvider::Ollama)
        );
        assert_eq!(
            infer_provider_from_host("https://relay.example.com/v1"),
            None
        );
        assert_ne!(
            infer_provider_from_host("http://127.0.0.1:8080/v1"),
            Some(EngineProvider::OnDevice)
        );
        assert_ne!(
            infer_provider_from_host("http://localhost/v1"),
            Some(EngineProvider::OnDevice)
        );
    }

    #[test]
    fn on_device_is_asr_only_and_allows_empty_key() {
        let provider = EngineProvider::OnDevice;
        assert!(provider.has_asr());
        assert!(!provider.has_llm());
        assert!(provider.allows_empty_key());
        assert!(!provider.has_http_asr());
        assert_eq!(provider.protocol(), ProviderProtocol::OnDeviceLocal);
        assert_eq!(provider.default_asr_model(), "qwen3-asr-0.6b");
        assert_eq!(provider.as_str(), "on_device");
        assert_eq!(EngineProvider::parse("on_device"), Some(provider));
        assert_eq!(EngineProvider::ALL.len(), 15);
        assert!(EngineProvider::Groq.has_http_asr());
        assert!(EngineProvider::LocalWhisper.has_http_asr());
        assert!(EngineProvider::Custom.has_http_asr());
    }

    #[test]
    fn asr_endpoint_on_device_is_not_groq() {
        let settings = crate::store::Settings {
            asr_provider: EngineProvider::OnDevice,
            asr_model: "sensevoice-small".into(),
            asr_base_url: String::new(),
            custom_base_url: String::new(),
            ..crate::store::Settings::default()
        };
        let endpoint = settings.asr_endpoint();
        assert!(
            endpoint.is_none(),
            "OnDevice must not resolve an HTTP endpoint; got {endpoint:?}"
        );
        assert_ne!(endpoint.as_deref(), Some(crate::asr::DEFAULT_ASR_BASE_URL));
        assert_ne!(
            endpoint.as_deref(),
            Some("https://api.groq.com/openai/v1/audio/transcriptions")
        );
        assert_eq!(resolve_asr_endpoint(EngineProvider::OnDevice, ""), None);
    }
}
