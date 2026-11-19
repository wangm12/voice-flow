use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewSourceKind {
    SelectedText,
    CurrentField,
    EmptyComposer,
    Screenshot,
    Dictation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewContextSource {
    Ax,
    Ocr,
    CloudVision,
    AxNearbyText,
    Screenshot,
}

/// Bounded positive-only source facts captured from successful provider
/// requests. This contains source identifiers only, never projected text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewContextFacts {
    pub context_source: Vec<PreviewContextSource>,
    /// `None` means the request did not prove whether a screenshot was used.
    pub screen_captured: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewProvenanceReceipt {
    pub source_kind: PreviewSourceKind,
    pub char_count: Option<usize>,
    pub word_count: Option<usize>,
    /// `None` means the saved row did not retain this fact.
    pub screen_captured: Option<bool>,
    /// `None` means the source was not retained; an empty list means the
    /// current transaction confirms that no extra context was included.
    pub context_source: Option<Vec<PreviewContextSource>>,
    /// Provider and model only. This never contains a base URL or credential.
    pub provider_label: Option<String>,
}

impl PreviewProvenanceReceipt {
    pub fn measured_text_counts(text: &str) -> (usize, usize) {
        (text.chars().count(), text.split_whitespace().count())
    }

    pub fn provider_label(provider: crate::providers::EngineProvider) -> &'static str {
        match provider {
            crate::providers::EngineProvider::Groq => "Groq",
            crate::providers::EngineProvider::OpenAi => "OpenAI",
            crate::providers::EngineProvider::Deepgram => "Deepgram",
            crate::providers::EngineProvider::SiliconFlow => "SiliconFlow",
            crate::providers::EngineProvider::Fireworks => "Fireworks",
            crate::providers::EngineProvider::Mistral => "Mistral",
            crate::providers::EngineProvider::Soniox => "Soniox",
            crate::providers::EngineProvider::AssemblyAi => "AssemblyAI",
            crate::providers::EngineProvider::DashScope => "DashScope",
            crate::providers::EngineProvider::DeepSeek => "DeepSeek",
            crate::providers::EngineProvider::Anthropic => "Anthropic",
            crate::providers::EngineProvider::Ollama => "Ollama",
            crate::providers::EngineProvider::LocalWhisper => "Whisper",
            crate::providers::EngineProvider::OnDevice => "On Device",
            crate::providers::EngineProvider::OnDeviceMlx => "VoiceFlow MLX",
            crate::providers::EngineProvider::OnDeviceLlamaCpp => "VoiceFlow llama.cpp",
            crate::providers::EngineProvider::Custom => "Custom",
        }
    }

    pub fn for_dictation_history(engine: Option<&str>, raw_text: &str) -> Option<Self> {
        let provider = engine
            .map(str::trim)
            .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("unknown"))
            .and_then(|value| value.split(':').next())
            .and_then(crate::providers::EngineProvider::parse);
        let (char_count, word_count) = Self::measured_text_counts(raw_text);
        Some(Self {
            source_kind: PreviewSourceKind::Dictation,
            char_count: Some(char_count),
            word_count: Some(word_count),
            // Existing History rows do not prove whether optional screen text
            // evidence was used during the dictation.
            screen_captured: None,
            // History does not retain enough information to assert which
            // context source was included in this request.
            context_source: None,
            provider_label: provider.map(|provider| Self::provider_label(provider).to_owned()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{PreviewContextSource, PreviewProvenanceReceipt, PreviewSourceKind};

    #[test]
    fn counts_unicode_scalars_and_whitespace_delimited_words_without_text() {
        assert_eq!(
            PreviewProvenanceReceipt::measured_text_counts("你好 hello  world"),
            (15, 3)
        );
    }

    #[test]
    fn history_receipt_only_uses_recorded_dictation_facts_and_marks_context_unknown() {
        let receipt = PreviewProvenanceReceipt::for_dictation_history(
            Some("groq:whisper-large-v3-turbo"),
            "hello world",
        )
        .unwrap();

        assert_eq!(receipt.source_kind, PreviewSourceKind::Dictation);
        assert_eq!(receipt.char_count, Some(11));
        assert_eq!(receipt.word_count, Some(2));
        assert_eq!(receipt.screen_captured, None);
        assert_eq!(receipt.context_source, None);
        assert_eq!(receipt.provider_label.as_deref(), Some("Groq"));
        assert_eq!(
            PreviewProvenanceReceipt::for_dictation_history(Some("unknown"), "hello")
                .unwrap()
                .provider_label,
            None
        );
        let custom = PreviewProvenanceReceipt::for_dictation_history(
            Some("custom:https://example.test"),
            "hello",
        )
        .unwrap();
        assert_eq!(custom.provider_label.as_deref(), Some("Custom"));
        assert!(!serde_json::to_string(&custom)
            .unwrap()
            .contains("example.test"));
    }

    #[test]
    fn serialized_context_source_values_are_narrow_identifiers() {
        assert_eq!(
            serde_json::to_value(PreviewContextSource::AxNearbyText).unwrap(),
            serde_json::json!("ax_nearby_text")
        );
    }

    #[test]
    fn positive_context_facts_serialize_only_source_identifiers_and_actual_screen_evidence() {
        let facts = super::PreviewContextFacts {
            context_source: vec![PreviewContextSource::Ax],
            screen_captured: None,
        };
        let value = serde_json::to_value(facts).unwrap();
        assert_eq!(value["context_source"], serde_json::json!(["ax"]));
        assert!(value["screen_captured"].is_null());
        assert!(serde_json::from_value::<super::PreviewContextFacts>(value).is_ok());
    }
}
