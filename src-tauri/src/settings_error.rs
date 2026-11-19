use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsErrorCode {
    InvalidPatch,
    ShutdownInProgress,
    LanguageUnsupported,
    PreferredLanguagesInvalid,
    RetiredOnDeviceAsrModel,
    InputMonitoringRequired,
    ModifierEventTapUnavailable,
    HotkeyConflict,
    TranslationHotkeyConflict,
    HotkeyInvalid,
    TranslationHotkeyInvalid,
    CredentialStorageFailed,
    ApiKeyInvalid,
    RateLimited,
    NetworkUnavailable,
    InvalidEndpointUrl,
    DictionaryPartialSync,
    DictionaryCapacityOrInvalidWord,
    DictionaryFileInvalid,
    LocalModelActive,
    LocalModelUnavailable,
    MappingValidationFailed,
    OperationBusy,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsCommandError {
    pub code: SettingsErrorCode,
    pub message: String,
}

impl SettingsCommandError {
    #[cfg(test)]
    pub fn new(code: SettingsErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn classify(message: impl Into<String>) -> Self {
        let message = message.into();
        let code = classify_message(&message);
        Self { code, message }
    }

    pub fn from_string(message: String) -> Self {
        Self::classify(message)
    }
}

fn classify_message(message: &str) -> SettingsErrorCode {
    let trimmed = message.trim();
    let lower = trimmed.to_ascii_lowercase();

    if lower.starts_with("learning_dictionary_partial:") {
        return SettingsErrorCode::DictionaryPartialSync;
    }
    if trimmed == "dictionary_capacity_or_invalid_word" {
        return SettingsErrorCode::DictionaryCapacityOrInvalidWord;
    }
    if lower == "retired_on_device_asr_model" {
        return SettingsErrorCode::RetiredOnDeviceAsrModel;
    }
    if lower == "input_monitoring_required" {
        return SettingsErrorCode::InputMonitoringRequired;
    }
    if lower == "modifier_event_tap_unavailable" {
        return SettingsErrorCode::ModifierEventTapUnavailable;
    }
    if trimmed == "local_cleanup_model_is_active" {
        return SettingsErrorCode::LocalModelActive;
    }
    if lower == "voiceflow is shutting down." || lower == "voiceflow is shutting down" {
        return SettingsErrorCode::ShutdownInProgress;
    }
    if trimmed.contains("CSV")
        || trimmed.contains("词典文件")
        || trimmed.contains("词典路径")
        || trimmed.contains("无法读取词典")
    {
        return SettingsErrorCode::DictionaryFileInvalid;
    }
    if lower.starts_with("settings patch must be an object")
        || lower.starts_with("unsupported settings field:")
        || lower.starts_with("context mappings must be changed")
    {
        return SettingsErrorCode::InvalidPatch;
    }
    if (lower.contains("recognition language")
        && lower.contains("is not supported by the selected asr model"))
        || lower.starts_with("unsupported recognition language")
    {
        return SettingsErrorCode::LanguageUnsupported;
    }
    if lower.contains("preferred_languages") || lower.contains("preferred language") {
        return SettingsErrorCode::PreferredLanguagesInvalid;
    }
    if lower.contains("hotkey") || lower.contains("accelerator") || lower.contains("shortcut") {
        let is_translation = lower.contains("translation");
        if lower.contains("conflicts") {
            return if is_translation {
                SettingsErrorCode::TranslationHotkeyConflict
            } else {
                SettingsErrorCode::HotkeyConflict
            };
        }
        return if is_translation {
            SettingsErrorCode::TranslationHotkeyInvalid
        } else {
            SettingsErrorCode::HotkeyInvalid
        };
    }
    if lower.contains("credential_storage")
        || lower.contains("failed to store api key securely")
        || lower.contains("failed to remove provider key securely")
        || lower.contains("failed to remove custom api key securely")
        || lower.contains("credential write")
        || lower.contains("keychain")
    {
        return SettingsErrorCode::CredentialStorageFailed;
    }
    if lower.contains("api key validation failed")
        || lower.contains("invalid api key")
        || lower.contains("invalid_api_key")
        || lower.contains("incorrect api key")
        || lower.contains("unauthorized")
        || lower.contains("http 401")
    {
        return SettingsErrorCode::ApiKeyInvalid;
    }
    if lower.contains("rate_limited") || trimmed.contains("请求过频") {
        return SettingsErrorCode::RateLimited;
    }
    if (lower.contains("network") || lower.contains("timeout"))
        && !lower.contains("spool")
        && !lower.contains("sqlite")
        && !lower.contains("history")
        && !lower.contains("disk")
    {
        return SettingsErrorCode::NetworkUnavailable;
    }
    if lower.contains("invalid url")
        || lower.contains("invalid base_url")
        || lower.contains("invalid endpoint")
        || lower.contains("custom_base_url")
        || lower.contains("asr_base_url")
        || lower.contains("cleanup_base_url")
        || lower.contains("accurate_asr_base_url")
        || lower.contains("ollama_base_url")
        || lower.contains("missing host")
        || lower.contains("relative url")
        || lower.contains("unsupported url scheme")
    {
        return SettingsErrorCode::InvalidEndpointUrl;
    }
    if lower.contains("insufficient_memory")
        || lower.contains("memory_unknown")
        || lower.contains("memory_unavailable")
        || lower.contains("unsupported local")
        || lower.contains("not available for new")
        || lower.contains("missing or not verified")
        || lower.contains("requires apple silicon")
        || lower.contains("model folder")
        || lower.contains("cannot open selected model folder")
    {
        return SettingsErrorCode::LocalModelUnavailable;
    }
    if lower.contains("mapping")
        || lower.contains("task identity")
        || lower.contains("browser_host")
        || lower.contains("browser_path_prefix")
        || lower.contains("manual_pair_")
        || lower.contains("manual_scoped_pair_")
        || lower.contains("enter two different terms")
        || lower.contains("valid website domain")
        || lower.contains("native application")
    {
        return SettingsErrorCode::MappingValidationFailed;
    }
    if lower.contains("busy")
        || lower.contains("still stopping")
        || lower.contains("already in progress")
        || lower.contains("another dictation operation is active")
    {
        return SettingsErrorCode::OperationBusy;
    }
    SettingsErrorCode::Internal
}

impl fmt::Display for SettingsCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SettingsCommandError {}

impl From<String> for SettingsCommandError {
    fn from(message: String) -> Self {
        Self::classify(message)
    }
}

impl From<&str> for SettingsCommandError {
    fn from(message: &str) -> Self {
        Self::classify(message)
    }
}

impl PartialEq<str> for SettingsCommandError {
    fn eq(&self, other: &str) -> bool {
        self.message == other
    }
}

impl PartialEq<&str> for SettingsCommandError {
    fn eq(&self, other: &&str) -> bool {
        self.message == *other
    }
}

#[cfg(test)]
mod tests {
    use super::{SettingsCommandError, SettingsErrorCode};

    #[test]
    fn classifies_known_settings_errors_into_typed_codes() {
        assert_eq!(
            SettingsCommandError::classify("learning_dictionary_partial:settings_save_failed").code,
            SettingsErrorCode::DictionaryPartialSync
        );
        assert_eq!(
            SettingsCommandError::classify("dictionary_capacity_or_invalid_word").code,
            SettingsErrorCode::DictionaryCapacityOrInvalidWord
        );
        assert_eq!(
            SettingsCommandError::classify("retired_on_device_asr_model").code,
            SettingsErrorCode::RetiredOnDeviceAsrModel
        );
        assert_eq!(
            SettingsCommandError::classify("input_monitoring_required").code,
            SettingsErrorCode::InputMonitoringRequired
        );
        assert_eq!(
            SettingsCommandError::classify("modifier_event_tap_unavailable").code,
            SettingsErrorCode::ModifierEventTapUnavailable
        );
        assert_eq!(
            SettingsCommandError::classify("local_cleanup_model_is_active").code,
            SettingsErrorCode::LocalModelActive
        );
        assert_eq!(
            SettingsCommandError::classify("translation hotkey conflicts with dictation").code,
            SettingsErrorCode::TranslationHotkeyConflict
        );
        assert_eq!(
            SettingsCommandError::classify("invalid hotkey combination").code,
            SettingsErrorCode::HotkeyInvalid
        );
        assert_eq!(
            SettingsCommandError::classify(
                "recognition language 'ar' is not supported by the selected ASR model"
            )
            .code,
            SettingsErrorCode::LanguageUnsupported
        );
        assert_eq!(
            SettingsCommandError::classify("preferred_languages contains invalid entries").code,
            SettingsErrorCode::PreferredLanguagesInvalid
        );
        assert_eq!(
            SettingsCommandError::classify("unsupported preferred language: xx").code,
            SettingsErrorCode::PreferredLanguagesInvalid
        );
        assert_eq!(
            SettingsCommandError::classify("duplicate preferred language: zh").code,
            SettingsErrorCode::PreferredLanguagesInvalid
        );
        assert_eq!(
            SettingsCommandError::classify("invalid custom_base_url: missing host").code,
            SettingsErrorCode::InvalidEndpointUrl
        );
    }

    #[test]
    fn serializes_code_and_message_as_structured_envelope() {
        let err = SettingsCommandError::new(
            SettingsErrorCode::LocalModelActive,
            "local_cleanup_model_is_active",
        );
        let value = serde_json::to_value(&err).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "code": "local_model_active",
                "message": "local_cleanup_model_is_active",
            })
        );
        assert_eq!(err, "local_cleanup_model_is_active");
    }
}
