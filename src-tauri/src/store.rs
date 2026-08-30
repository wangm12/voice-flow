use crate::queue::QuotaView;
use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
    XChaCha20Poly1305, XNonce,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub const SETTINGS_SCHEMA_VERSION: u32 = 17;
const HISTORY_SCHEMA_VERSION: i32 = 8;
const LEARN_PAIRS_PENDING_CAP: i64 = 256;

fn ensure_private_dir(path: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn restrict_file_mode(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn restrict_history_sidecars(dir: &Path) -> anyhow::Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let path = dir.join(format!("history.sqlite{suffix}"));
        if path.exists() {
            restrict_file_mode(&path)?;
        }
    }
    Ok(())
}

fn current_settings_schema_version() -> u32 {
    SETTINGS_SCHEMA_VERSION
}

fn default_writing_modes() -> Vec<crate::context::WritingMode> {
    crate::context::builtin_writing_modes()
}

fn default_theme() -> String {
    "system".into()
}

fn default_ui_language() -> String {
    "system".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    #[serde(default = "current_settings_schema_version")]
    pub schema_version: u32,
    pub api_key: String,
    #[serde(default)]
    pub asr_api_key: String,
    #[serde(default)]
    pub asr_base_url: String,
    #[serde(default)]
    pub asr_provider: crate::engine::EngineProvider,
    #[serde(default = "default_asr_model")]
    pub asr_model: String,
    pub language: String,
    #[serde(default = "default_ui_language")]
    pub ui_language: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    pub dictionary: Vec<String>,
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    pub activation_mode: String,
    pub chunk_threshold_secs: u64,
    pub chunk_length_secs: usize,
    #[serde(default = "default_long_output_mode")]
    pub long_output_mode: String,
    #[serde(default = "default_delivery_policy")]
    pub delivery_policy: String,
    pub keep_audio_days: u64,
    pub keep_history_days: u64,
    pub onboarded: bool,
    pub cleanup_enabled: bool,
    #[serde(default = "default_cleanup_model")]
    pub cleanup_model: String,
    #[serde(default)]
    pub cleanup_provider: crate::engine::EngineProvider,
    #[serde(default)]
    pub cleanup_base_url: String,
    #[serde(default)]
    pub cleanup_api_key: String,
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
    #[serde(default)]
    pub provider_api_keys: std::collections::BTreeMap<String, String>,
    #[serde(default = "default_show_tray_icon")]
    pub show_tray_icon: bool,
    pub context_enabled: bool,
    pub browser_access_enabled: bool,
    pub context_mappings: Vec<crate::context::AppMapping>,
    #[serde(default = "default_writing_modes")]
    pub writing_modes: Vec<crate::context::WritingMode>,
    #[serde(default)]
    pub snippets: Vec<crate::snippets::Snippet>,
    #[serde(default = "default_output_mode")]
    pub output_mode: String,
    #[serde(default = "default_translation_target_language")]
    pub translation_target_language: String,
    #[serde(default = "default_selected_action_hotkey")]
    pub selected_action_hotkey: String,
    #[serde(default = "default_selected_actions_enabled")]
    pub selected_actions_enabled: bool,
    #[serde(default = "default_dictionary_learn_enabled")]
    pub dictionary_learn_enabled: bool,
    #[serde(default)]
    pub input_device: String,
    #[serde(default = "default_input_gain")]
    pub input_gain: f32,
}

fn default_cleanup_model() -> String {
    crate::llm::MODEL.to_owned()
}

fn default_show_tray_icon() -> bool {
    true
}

fn default_output_mode() -> String {
    "auto".into()
}

fn default_translation_target_language() -> String {
    "en".into()
}

fn default_long_output_mode() -> String {
    "paste".into()
}

fn default_delivery_policy() -> String {
    crate::delivery::DeliveryPolicy::default_value()
        .as_str()
        .into()
}

fn default_hotkey() -> String {
    "CmdOrControl+Alt+Space".into()
}

fn default_selected_action_hotkey() -> String {
    "CmdOrControl+Alt+Slash".into()
}

fn default_selected_actions_enabled() -> bool {
    true
}

fn default_dictionary_learn_enabled() -> bool {
    true
}

fn default_input_gain() -> f32 {
    1.0
}

fn default_asr_model() -> String {
    crate::asr::MODEL.to_owned()
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            api_key: String::new(),
            asr_api_key: String::new(),
            asr_base_url: String::new(),
            asr_provider: crate::engine::EngineProvider::Groq,
            asr_model: default_asr_model(),
            language: "auto".into(),
            ui_language: default_ui_language(),
            theme: default_theme(),
            dictionary: Vec::new(),
            hotkey: default_hotkey(),
            activation_mode: "tap".into(),
            chunk_threshold_secs: 25,
            chunk_length_secs: 35,
            long_output_mode: default_long_output_mode(),
            delivery_policy: default_delivery_policy(),
            keep_audio_days: 7,
            keep_history_days: 365,
            onboarded: false,
            cleanup_enabled: true,
            cleanup_model: default_cleanup_model(),
            cleanup_provider: crate::engine::EngineProvider::Groq,
            cleanup_base_url: String::new(),
            cleanup_api_key: String::new(),
            custom_base_url: String::new(),
            custom_asr: true,
            custom_llm: true,
            ollama_base_url: crate::providers::EngineProvider::Ollama
                .default_base_url()
                .to_owned(),
            local_whisper_base_url: crate::providers::EngineProvider::LocalWhisper
                .default_base_url()
                .to_owned(),
            provider_api_keys: std::collections::BTreeMap::new(),
            show_tray_icon: true,
            context_enabled: true,
            browser_access_enabled: false,
            context_mappings: Vec::new(),
            writing_modes: default_writing_modes(),
            snippets: Vec::new(),
            output_mode: default_output_mode(),
            translation_target_language: default_translation_target_language(),
            selected_action_hotkey: default_selected_action_hotkey(),
            selected_actions_enabled: default_selected_actions_enabled(),
            dictionary_learn_enabled: default_dictionary_learn_enabled(),
            input_device: String::new(),
            input_gain: default_input_gain(),
        }
    }
}

impl Settings {
    pub fn normalize(&mut self) {
        if self.schema_version < SETTINGS_SCHEMA_VERSION {
            // Before schema 7, a new install was silently stored as Chinese.
            // Preserve an explicit choice for onboarded users, but let an
            // unfinished setup follow the operating system language.
            if !self.onboarded && self.ui_language == "zh" {
                self.ui_language = default_ui_language();
            }
            if self.schema_version < 9 && self.selected_action_hotkey.trim().is_empty() {
                self.selected_action_hotkey = default_selected_action_hotkey();
                self.selected_actions_enabled = true;
            }
            if self.schema_version < 11 && self.output_mode == "plain" {
                self.output_mode = default_output_mode();
            }
            // Schema 11 used clipboard as the long-recording default. Treat
            // that legacy default as a migration to the new default, while
            // preserving explicit choices made in the current schema.
            if self.schema_version < 12 && self.long_output_mode == "clipboard" {
                self.long_output_mode = default_long_output_mode();
            }
            if self.schema_version < 13 {
                let migrated_policy = match self.long_output_mode.as_str() {
                    "clipboard" => "clipboard_only",
                    "history" => "history_only",
                    _ => "auto",
                };
                self.delivery_policy = migrated_policy.into();
            }
            if self.schema_version < 14 {
                if self.hotkey == "CmdOrControl+Shift+Space" {
                    self.hotkey = default_hotkey();
                }
                if self.selected_action_hotkey == "CmdOrControl+Shift+Slash" {
                    self.selected_action_hotkey = default_selected_action_hotkey();
                }
            }
            if self.schema_version < 15
                && self.selected_action_hotkey == "CmdOrControl+Alt+Backslash"
            {
                self.selected_action_hotkey = default_selected_action_hotkey();
            }
            if self.schema_version < 16 {
                self.asr_provider = if self.asr_base_url.trim().is_empty()
                    || crate::asr::groq_key_fallback_allowed(&self.asr_base_url)
                {
                    crate::engine::EngineProvider::Groq
                } else {
                    crate::engine::EngineProvider::Custom
                };
                if self.asr_provider.is_groq() {
                    self.asr_base_url.clear();
                }
                self.cleanup_provider = crate::engine::EngineProvider::Groq;
                self.cleanup_base_url.clear();
                self.cleanup_api_key.clear();
            }
            if self.schema_version < 17 {
                self.migrate_provider_pool();
            }
            self.schema_version = SETTINGS_SCHEMA_VERSION;
        }
        self.chunk_threshold_secs = self.chunk_threshold_secs.clamp(5, 3_600);
        self.chunk_length_secs = self.chunk_length_secs.clamp(15, 60);
        self.keep_audio_days = self.keep_audio_days.min(365);
        self.keep_history_days = self.keep_history_days.min(3650);
        if !matches!(self.language.as_str(), "auto" | "zh" | "en") {
            self.language = "auto".into();
        }
        if !matches!(self.ui_language.as_str(), "system" | "zh" | "en") {
            self.ui_language = default_ui_language();
        }
        if !matches!(self.theme.as_str(), "system" | "light" | "dark") {
            self.theme = default_theme();
        }
        if !matches!(
            self.long_output_mode.as_str(),
            "clipboard" | "paste" | "history"
        ) {
            self.long_output_mode = default_long_output_mode();
        }
        if !crate::delivery::DeliveryPolicy::is_valid(&self.delivery_policy) {
            self.delivery_policy = default_delivery_policy();
        }
        if self.cleanup_provider.is_groq() && !crate::llm::is_supported_model(&self.cleanup_model) {
            self.cleanup_model = default_cleanup_model();
        }
        if self.hotkey.is_empty() || self.hotkey.len() > 128 {
            self.hotkey = Self::default().hotkey;
        }
        self.hotkey = crate::hotkey::canonicalize_hotkey(&self.hotkey);
        self.selected_action_hotkey =
            crate::hotkey::canonicalize_hotkey(&self.selected_action_hotkey);
        if self.activation_mode == "hold" {
            self.activation_mode = if crate::modifier_hotkey::is_modifier_only(&self.hotkey) {
                "double_tap"
            } else {
                "hybrid"
            }
            .into();
        }
        if !matches!(
            self.activation_mode.as_str(),
            "tap" | "double_tap" | "hybrid"
        ) {
            self.activation_mode = "tap".into();
        }
        // Modifier-only shortcuts are implemented by the macOS event tap,
        // whose only supported gesture is a double tap. A legacy or manually
        // edited settings file must not leave the app with a shortcut that is
        // registered successfully but can never emit a toggle event.
        if crate::modifier_hotkey::is_modifier_only(&self.hotkey) {
            self.activation_mode = "double_tap".into();
        }
        self.dictionary.truncate(256);
        self.dictionary
            .iter_mut()
            .for_each(|word| word.truncate(256));
        self.context_mappings
            .retain(|mapping| mapping.validate().is_ok());
        for mapping in &mut self.context_mappings {
            if let Some(host) = &mapping.browser_host {
                mapping.browser_host = crate::context::normalize_host(host);
            }
            mapping.style_example_input = mapping.style_example_input.take().and_then(|value| {
                let trimmed: String = value.trim().chars().take(2_000).collect();
                (!trimmed.is_empty()).then_some(trimmed)
            });
            mapping.style_example_output = mapping.style_example_output.take().and_then(|value| {
                let trimmed: String = value.trim().chars().take(2_000).collect();
                (!trimmed.is_empty()).then_some(trimmed)
            });
        }
        crate::context::normalize_writing_modes(&mut self.writing_modes);
        crate::snippets::normalize_snippets(&mut self.snippets);
        if !matches!(
            self.output_mode.as_str(),
            "auto" | "email" | "bullets" | "meeting_notes" | "code" | "translation"
        ) {
            self.output_mode = default_output_mode();
        }
        if !matches!(
            self.translation_target_language.as_str(),
            "en" | "zh" | "ja" | "ko" | "es" | "fr" | "de"
        ) {
            self.translation_target_language = default_translation_target_language();
        }
        if self.selected_action_hotkey.len() > 128 {
            self.selected_action_hotkey.clear();
        }
        if crate::modifier_hotkey::is_modifier_only(&self.selected_action_hotkey) {
            self.selected_action_hotkey.clear();
        }
        self.input_device = self.input_device.trim().chars().take(512).collect();
        self.input_gain = if self.input_gain.is_finite() {
            self.input_gain.clamp(0.5, 4.0)
        } else {
            default_input_gain()
        };
        self.asr_base_url = self.asr_base_url.trim().chars().take(2_048).collect();
        self.cleanup_base_url = self.cleanup_base_url.trim().chars().take(2_048).collect();
        self.asr_model = crate::asr::resolve_asr_model(&self.asr_model).to_owned();
        if self.asr_model.len() > 256 {
            self.asr_model.truncate(256);
        }
        if self.cleanup_model.len() > 256 {
            self.cleanup_model.truncate(256);
        }
        if self.asr_api_key.len() > 512 {
            self.asr_api_key.truncate(512);
        }
        if self.cleanup_api_key.len() > 512 {
            self.cleanup_api_key.truncate(512);
        }
        // Only the URL can be trusted here. Keys live in the keychain and are
        // blank on disk, so treating an empty key as "incomplete custom" would
        // wipe a working custom URL during load_settings, before credentials
        // are rebound. Empty-key repair happens in
        // `repair_incomplete_engine_sides` after keys are in memory.
        if self.asr_provider.is_custom()
            && self.custom_base_url.trim().is_empty()
            && self.asr_base_url.trim().is_empty()
        {
            self.asr_provider = crate::engine::EngineProvider::Groq;
        }
        if self.cleanup_provider.is_custom()
            && self.custom_base_url.trim().is_empty()
            && self.cleanup_base_url.trim().is_empty()
        {
            self.cleanup_provider = crate::engine::EngineProvider::Groq;
        }
        if self.asr_provider.is_groq() {
            self.asr_base_url.clear();
            if !crate::asr::is_groq_asr_model(&self.asr_model) {
                self.asr_model = default_asr_model();
            }
        } else if self.asr_model.trim().is_empty() {
            self.asr_model = self.asr_provider.default_asr_model().to_owned();
        }
        if self.cleanup_provider.is_groq() {
            self.cleanup_base_url.clear();
            if !crate::llm::is_supported_model(&self.cleanup_model) {
                self.cleanup_model = default_cleanup_model();
            }
        } else if self.cleanup_model.trim().is_empty() {
            self.cleanup_model = self.cleanup_provider.default_llm_model().to_owned();
        }
        if self.ollama_base_url.trim().is_empty() {
            self.ollama_base_url = crate::providers::EngineProvider::Ollama
                .default_base_url()
                .to_owned();
        }
        if self.local_whisper_base_url.trim().is_empty() {
            self.local_whisper_base_url = crate::providers::EngineProvider::LocalWhisper
                .default_base_url()
                .to_owned();
        }
    }

    fn migrate_provider_pool(&mut self) {
        if self.asr_provider.is_custom() {
            if let Some(named) = crate::providers::infer_provider_from_host(&self.asr_base_url) {
                self.asr_provider = named;
                if !self.asr_api_key.trim().is_empty() {
                    self.provider_api_keys
                        .insert(named.as_str().to_owned(), self.asr_api_key.clone());
                }
                if named == crate::providers::EngineProvider::Ollama {
                    self.ollama_base_url = self.asr_base_url.clone();
                }
                if named == crate::providers::EngineProvider::LocalWhisper {
                    self.local_whisper_base_url = self.asr_base_url.clone();
                }
                if !named.is_custom() {
                    self.asr_base_url.clear();
                }
            } else if !self.asr_base_url.trim().is_empty() {
                self.custom_base_url = self.asr_base_url.clone();
                self.custom_asr = true;
                if !self.asr_api_key.trim().is_empty() {
                    self.provider_api_keys
                        .insert("custom".into(), self.asr_api_key.clone());
                }
            }
        }
        if self.cleanup_provider.is_custom() {
            if let Some(named) = crate::providers::infer_provider_from_host(&self.cleanup_base_url) {
                self.cleanup_provider = named;
                if !self.cleanup_api_key.trim().is_empty() {
                    self.provider_api_keys
                        .insert(named.as_str().to_owned(), self.cleanup_api_key.clone());
                }
                if named == crate::providers::EngineProvider::Ollama {
                    self.ollama_base_url = self.cleanup_base_url.clone();
                }
                if !named.is_custom() {
                    self.cleanup_base_url.clear();
                }
            } else if !self.cleanup_base_url.trim().is_empty() {
                if self.custom_base_url.trim().is_empty() {
                    self.custom_base_url = self.cleanup_base_url.clone();
                }
                self.custom_llm = true;
                if !self.cleanup_api_key.trim().is_empty() {
                    self.provider_api_keys
                        .entry("custom".into())
                        .or_insert_with(|| self.cleanup_api_key.clone());
                }
            }
        }
        if !self.api_key.trim().is_empty() {
            self.provider_api_keys
                .entry("groq".into())
                .or_insert_with(|| self.api_key.clone());
        }
    }

    /// Drop custom sides that still cannot run now that credentials are loaded.
    /// Call this after keychain bind, never from `normalize`.
    pub fn repair_incomplete_engine_sides(&mut self) -> bool {
        let asr = self.repair_incomplete_asr();
        let cleanup = self.repair_incomplete_cleanup();
        asr || cleanup
    }

    fn repair_incomplete_asr(&mut self) -> bool {
        if !self.asr_provider.is_custom() {
            return false;
        }
        let url = if self.custom_base_url.trim().is_empty() {
            self.asr_base_url.trim()
        } else {
            self.custom_base_url.trim()
        };
        let key = self.provider_secret(self.asr_provider);
        if !url.is_empty() && (!key.is_empty() || crate::providers::is_loopback_url(url)) {
            return false;
        }
        self.asr_provider = crate::engine::EngineProvider::Groq;
        self.asr_base_url.clear();
        if !crate::asr::is_groq_asr_model(&self.asr_model) {
            self.asr_model = default_asr_model();
        }
        true
    }

    fn repair_incomplete_cleanup(&mut self) -> bool {
        if !self.cleanup_provider.is_custom() {
            return false;
        }
        let url = if self.custom_base_url.trim().is_empty() {
            self.cleanup_base_url.trim()
        } else {
            self.custom_base_url.trim()
        };
        let key = self.provider_secret(self.cleanup_provider);
        if !url.is_empty() && (!key.is_empty() || crate::providers::is_loopback_url(url)) {
            return false;
        }
        self.cleanup_provider = crate::engine::EngineProvider::Groq;
        self.cleanup_base_url.clear();
        if !crate::llm::is_supported_model(&self.cleanup_model) {
            self.cleanup_model = default_cleanup_model();
        }
        true
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.onboarded && self.api_key.trim().is_empty() {
            anyhow::bail!("a valid API key is required before onboarding can be completed");
        }
        if !matches!(self.language.as_str(), "auto" | "zh" | "en") {
            anyhow::bail!("unsupported recognition language");
        }
        if !matches!(self.ui_language.as_str(), "system" | "zh" | "en") {
            anyhow::bail!("unsupported interface language");
        }
        if !matches!(self.theme.as_str(), "system" | "light" | "dark") {
            anyhow::bail!("unsupported theme");
        }
        if !(5..=3_600).contains(&self.chunk_threshold_secs) {
            anyhow::bail!("chunk threshold must be between 5 and 3600 seconds");
        }
        if !(15..=60).contains(&self.chunk_length_secs) {
            anyhow::bail!("chunk length must be between 15 and 60 seconds");
        }
        if self.keep_audio_days > 365 {
            anyhow::bail!("audio retention must be 365 days or less");
        }
        if self.keep_history_days > 3_650 {
            anyhow::bail!("history retention must be 3650 days or less");
        }
        if !matches!(
            self.long_output_mode.as_str(),
            "clipboard" | "paste" | "history"
        ) {
            anyhow::bail!("unsupported long output mode");
        }
        if !matches!(
            self.delivery_policy.as_str(),
            "auto" | "paste_shortcut" | "clipboard_only" | "history_only"
        ) {
            anyhow::bail!("unsupported delivery policy");
        }
        if self.cleanup_provider.is_groq() {
            if !crate::llm::is_supported_model(&self.cleanup_model) {
                anyhow::bail!("unsupported cleanup model");
            }
        } else if self.cleanup_model.trim().is_empty() {
            anyhow::bail!("自定义整理需要填写模型名。");
        }
        if self.hotkey.is_empty() || self.hotkey.len() > 128 {
            anyhow::bail!("hotkey must contain between 1 and 128 characters");
        }
        if !matches!(
            self.activation_mode.as_str(),
            "tap" | "double_tap" | "hybrid"
        ) {
            anyhow::bail!("unsupported activation mode");
        }
        if crate::modifier_hotkey::is_modifier_only(&self.hotkey)
            && self.activation_mode != "double_tap"
        {
            anyhow::bail!("modifier-only hotkeys require double_tap activation");
        }
        if !crate::modifier_hotkey::is_modifier_only(&self.hotkey)
            && self.activation_mode == "double_tap"
        {
            anyhow::bail!("double_tap activation requires a modifier-only hotkey");
        }
        if self.dictionary.len() > 256 || self.dictionary.iter().any(|word| word.len() > 256) {
            anyhow::bail!("dictionary is too large");
        }
        if self.api_key.len() > 512 {
            anyhow::bail!("API key is too long");
        }
        if self.asr_api_key.len() > 512 {
            anyhow::bail!("ASR API key is too long");
        }
        if self.cleanup_api_key.len() > 512 {
            anyhow::bail!("cleanup API key is too long");
        }
        if self.asr_base_url.len() > 2_048 {
            anyhow::bail!("ASR base URL is too long");
        }
        if self.cleanup_base_url.len() > 2_048 {
            anyhow::bail!("cleanup base URL is too long");
        }
        if self.asr_model.len() > 256 {
            anyhow::bail!("ASR model name is too long");
        }
        crate::asr::validate_asr_base_url(&self.asr_base_url)
            .map_err(|error| anyhow::anyhow!(error))?;
        crate::asr::validate_asr_base_url(&self.cleanup_base_url)
            .map_err(|error| anyhow::anyhow!(error))?;
        crate::asr::validate_asr_base_url(&self.custom_base_url)
            .map_err(|error| anyhow::anyhow!(error))?;
        crate::asr::validate_asr_base_url(&self.ollama_base_url)
            .map_err(|error| anyhow::anyhow!(error))?;
        crate::asr::validate_asr_base_url(&self.local_whisper_base_url)
            .map_err(|error| anyhow::anyhow!(error))?;
        self.validate_provider_side(self.asr_provider, true)?;
        if self.cleanup_enabled {
            self.validate_provider_side(self.cleanup_provider, false)?;
        }
        for mapping in &self.context_mappings {
            mapping.validate().map_err(|error| anyhow::anyhow!(error))?;
        }
        crate::context::validate_writing_modes(&self.writing_modes)
            .map_err(|error| anyhow::anyhow!(error))?;
        crate::snippets::validate_snippets(&self.snippets)
            .map_err(|error| anyhow::anyhow!(error))?;
        if !matches!(
            self.output_mode.as_str(),
            "auto" | "email" | "bullets" | "meeting_notes" | "code" | "translation"
        ) {
            anyhow::bail!("unsupported output mode");
        }
        if !matches!(
            self.translation_target_language.as_str(),
            "en" | "zh" | "ja" | "ko" | "es" | "fr" | "de"
        ) {
            anyhow::bail!("unsupported translation target language");
        }
        if self.selected_action_hotkey.len() > 128 {
            anyhow::bail!("selected action hotkey is too long");
        }
        if crate::modifier_hotkey::is_modifier_only(&self.selected_action_hotkey) {
            anyhow::bail!("selected action hotkeys require a key combination");
        }
        if self.input_device.len() > 512 {
            anyhow::bail!("input device name is too long");
        }
        if !self.input_gain.is_finite() || !(0.5..=4.0).contains(&self.input_gain) {
            anyhow::bail!("input gain must be between 0.5 and 4.0");
        }
        Ok(())
    }

    fn validate_provider_side(
        &self,
        provider: crate::engine::EngineProvider,
        asr: bool,
    ) -> anyhow::Result<()> {
        let url = self.resolved_provider_base(provider);
        if provider.is_custom() && url.trim().is_empty() {
            anyhow::bail!(if asr {
                "自定义 ASR 需要填写兼容地址。"
            } else {
                "自定义整理需要填写兼容地址。"
            });
        }
        let key = self.provider_secret(provider);
        let empty_ok = provider.allows_empty_key() && crate::providers::is_loopback_url(&url);
        if key.is_empty() && !empty_ok {
            if provider.is_groq() && !self.onboarded {
                return Ok(());
            }
            if provider.is_custom() {
                anyhow::bail!(if asr {
                    "自定义 ASR 地址需要填写 ASR 密钥。"
                } else {
                    "自定义整理地址需要填写整理密钥。"
                });
            }
            anyhow::bail!("缺少所选服务商的密钥。");
        }
        Ok(())
    }

    pub fn provider_secret(&self, provider: crate::engine::EngineProvider) -> &str {
        if let Some(key) = self
            .provider_api_keys
            .get(provider.as_str())
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return key;
        }
        match provider {
            crate::engine::EngineProvider::Groq => self.api_key.trim(),
            crate::engine::EngineProvider::Custom => {
                let asr = self.asr_api_key.trim();
                if !asr.is_empty() {
                    asr
                } else {
                    self.cleanup_api_key.trim()
                }
            }
            _ => "",
        }
    }

    pub fn resolved_provider_base(&self, provider: crate::engine::EngineProvider) -> String {
        match provider {
            crate::engine::EngineProvider::Groq => String::new(),
            crate::engine::EngineProvider::Ollama => {
                let url = self.ollama_base_url.trim();
                if url.is_empty() {
                    provider.default_base_url().to_owned()
                } else {
                    url.to_owned()
                }
            }
            crate::engine::EngineProvider::LocalWhisper => {
                let url = self.local_whisper_base_url.trim();
                if url.is_empty() {
                    provider.default_base_url().to_owned()
                } else {
                    url.to_owned()
                }
            }
            crate::engine::EngineProvider::Custom => {
                let custom = self.custom_base_url.trim();
                if !custom.is_empty() {
                    custom.to_owned()
                } else if !self.asr_base_url.trim().is_empty() {
                    self.asr_base_url.trim().to_owned()
                } else {
                    self.cleanup_base_url.trim().to_owned()
                }
            }
            other => other.default_base_url().to_owned(),
        }
    }

    pub fn asr_endpoint(&self) -> String {
        crate::providers::resolve_asr_endpoint(self.asr_provider, &self.resolved_provider_base(self.asr_provider))
    }

    /// Prefer a dedicated ASR key when set. Reuse the Groq key only for the
    /// Groq default or `api.groq.com`; custom hosts must supply `asr_api_key`.
    pub fn asr_credential(&self) -> &str {
        self.provider_secret(self.asr_provider)
    }

    pub fn cleanup_credential(&self) -> &str {
        self.provider_secret(self.cleanup_provider)
    }

    pub fn cleanup_endpoint(&self) -> String {
        crate::providers::resolve_llm_endpoint(
            self.cleanup_provider,
            &self.resolved_provider_base(self.cleanup_provider),
        )
    }

    pub fn cleanup_request_model(&self) -> String {
        if self.cleanup_provider.is_groq() {
            if crate::llm::is_supported_model(&self.cleanup_model) {
                self.cleanup_model.clone()
            } else {
                crate::llm::MODEL.to_owned()
            }
        } else {
            let model = self.cleanup_model.trim();
            if model.is_empty() {
                self.cleanup_provider.default_llm_model().to_owned()
            } else {
                model.to_owned()
            }
        }
    }
}

pub(crate) fn bind_asr_key_to_host(
    previous_url: &str,
    next_url: &str,
    incoming_key: &str,
    previous_key: &str,
) -> String {
    if !incoming_key.trim().is_empty() {
        return incoming_key.to_string();
    }
    if crate::asr::asr_host_changed(previous_url, next_url) {
        String::new()
    } else {
        previous_key.to_string()
    }
}

pub(crate) fn bind_cleanup_key_to_host(
    previous_url: &str,
    next_url: &str,
    incoming_key: &str,
    previous_key: &str,
) -> String {
    if !incoming_key.trim().is_empty() {
        return incoming_key.to_string();
    }
    if crate::llm::chat_host_changed(previous_url, next_url) {
        String::new()
    } else {
        previous_key.to_string()
    }
}

/// Settings exposed to the webview. The backend keeps the real credential in
/// memory/keychain, while the UI only receives whether one is configured and a
/// non-sensitive hint for display.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderKeyView {
    pub configured: bool,
    pub hint: Option<String>,
}

fn credential_hint(key: &str) -> Option<String> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    let tail: String = key.chars().rev().take(5).collect();
    Some(format!("••••{}", tail.chars().rev().collect::<String>()))
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingsView {
    pub schema_version: u32,
    pub api_key_configured: bool,
    pub api_key_hint: Option<String>,
    pub asr_api_key_configured: bool,
    pub asr_api_key_hint: Option<String>,
    pub asr_base_url: String,
    pub asr_provider: crate::engine::EngineProvider,
    pub asr_model: String,
    pub cleanup_model: String,
    pub cleanup_provider: crate::engine::EngineProvider,
    pub cleanup_base_url: String,
    pub cleanup_api_key_configured: bool,
    pub cleanup_api_key_hint: Option<String>,
    pub custom_base_url: String,
    pub custom_asr: bool,
    pub custom_llm: bool,
    pub ollama_base_url: String,
    pub local_whisper_base_url: String,
    pub provider_keys: std::collections::BTreeMap<String, ProviderKeyView>,
    pub language: String,
    pub ui_language: String,
    pub theme: String,
    pub dictionary: Vec<String>,
    pub hotkey: String,
    pub activation_mode: String,
    pub hotkey_error: Option<String>,
    pub chunk_threshold_secs: u64,
    pub chunk_length_secs: usize,
    pub long_output_mode: String,
    pub delivery_policy: String,
    pub keep_audio_days: u64,
    pub keep_history_days: u64,
    pub onboarded: bool,
    pub cleanup_enabled: bool,
    pub show_tray_icon: bool,
    pub context_enabled: bool,
    pub browser_access_enabled: bool,
    pub context_mappings: Vec<crate::context::AppMapping>,
    pub writing_modes: Vec<crate::context::WritingMode>,
    pub snippets: Vec<crate::snippets::Snippet>,
    pub output_mode: String,
    pub translation_target_language: String,
    pub selected_action_hotkey: String,
    pub selected_actions_enabled: bool,
    pub dictionary_learn_enabled: bool,
    pub input_device: String,
    pub input_gain: f32,
}

fn provider_key_views(settings: &Settings) -> std::collections::BTreeMap<String, ProviderKeyView> {
    crate::providers::EngineProvider::ALL
        .into_iter()
        .map(|provider| {
            let secret = settings.provider_secret(provider);
            (
                provider.as_str().to_owned(),
                ProviderKeyView {
                    configured: !secret.is_empty(),
                    hint: credential_hint(secret),
                },
            )
        })
        .collect()
}

impl From<&Settings> for SettingsView {
    fn from(settings: &Settings) -> Self {
        Self {
            schema_version: settings.schema_version,
            api_key_configured: !settings.api_key.is_empty(),
            api_key_hint: credential_hint(&settings.api_key),
            asr_api_key_configured: !settings.asr_api_key.trim().is_empty(),
            asr_api_key_hint: credential_hint(&settings.asr_api_key),
            asr_base_url: settings.asr_base_url.clone(),
            asr_provider: settings.asr_provider,
            asr_model: crate::asr::resolve_asr_model(&settings.asr_model).to_owned(),
            cleanup_model: settings.cleanup_model.clone(),
            cleanup_provider: settings.cleanup_provider,
            cleanup_base_url: settings.cleanup_base_url.clone(),
            cleanup_api_key_configured: !settings.cleanup_api_key.trim().is_empty(),
            cleanup_api_key_hint: credential_hint(&settings.cleanup_api_key),
            custom_base_url: settings.custom_base_url.clone(),
            custom_asr: settings.custom_asr,
            custom_llm: settings.custom_llm,
            ollama_base_url: settings.ollama_base_url.clone(),
            local_whisper_base_url: settings.local_whisper_base_url.clone(),
            provider_keys: provider_key_views(settings),
            language: settings.language.clone(),
            ui_language: settings.ui_language.clone(),
            theme: settings.theme.clone(),
            dictionary: settings.dictionary.clone(),
            hotkey: settings.hotkey.clone(),
            activation_mode: settings.activation_mode.clone(),
            hotkey_error: crate::hotkey::registration_error(),
            chunk_threshold_secs: settings.chunk_threshold_secs,
            chunk_length_secs: settings.chunk_length_secs,
            long_output_mode: settings.long_output_mode.clone(),
            delivery_policy: settings.delivery_policy.clone(),
            keep_audio_days: settings.keep_audio_days,
            keep_history_days: settings.keep_history_days,
            onboarded: settings.onboarded,
            cleanup_enabled: settings.cleanup_enabled,
            show_tray_icon: settings.show_tray_icon,
            context_enabled: settings.context_enabled,
            browser_access_enabled: settings.browser_access_enabled,
            context_mappings: settings.context_mappings.clone(),
            writing_modes: settings.writing_modes.clone(),
            snippets: settings.snippets.clone(),
            output_mode: settings.output_mode.clone(),
            translation_target_language: settings.translation_target_language.clone(),
            selected_action_hotkey: settings.selected_action_hotkey.clone(),
            selected_actions_enabled: settings.selected_actions_enabled,
            dictionary_learn_enabled: settings.dictionary_learn_enabled,
            input_device: settings.input_device.clone(),
            input_gain: settings.input_gain,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct HistoryItem {
    pub id: i64,
    pub created_at: String,
    pub raw_text: String,
    pub final_text: String,
    pub cleanup_status: String,
    pub duration: f64,
    pub degraded: bool,
    pub degraded_reason: Option<String>,
    pub status: String,
    pub delivery_method: Option<String>,
    pub fallback_reason: Option<String>,
    pub context_profile_id: Option<String>,
    pub retryable: bool,
    pub revision_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryRevision {
    pub revision_id: i64,
    pub dictation_id: i64,
    pub created_at: String,
    pub final_text: String,
    pub cleanup_status: Option<String>,
    pub intent: Option<serde_json::Value>,
    pub model: Option<String>,
    pub context_policy: Option<serde_json::Value>,
    pub revision_reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryPage {
    pub items: Vec<HistoryItem>,
    pub has_more: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct Usage {
    pub asr_requests: i64,
    pub llm_requests: i64,
    pub audio_seconds: f64,
    pub quota: QuotaView,
}
#[allow(dead_code)]
pub const ASR_DAILY_LIMIT: i64 = 2000;
#[allow(dead_code)]
pub const LLM_DAILY_LIMIT: i64 = 1000;
pub const MAX_SPOOL_BYTES: u64 = 256 * 1024 * 1024;
const SPOOL_MANIFEST_VERSION: u32 = 1;
const AUDIO_SAMPLE_RATE: usize = 16_000;
const SPOOL_ENVELOPE_MAGIC: &[u8; 8] = b"VFSPOOL1";
const SPOOL_ENVELOPE_VERSION: u8 = 1;
const SPOOL_ENVELOPE_HEADER_LEN: usize = SPOOL_ENVELOPE_MAGIC.len() + 1 + 24;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SpoolChunkManifest {
    index: usize,
    start_secs: f32,
    end_secs: f32,
    status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SpoolManifest {
    schema_version: u32,
    session_id: String,
    created_at_ms: u64,
    status: String,
    chunks: Vec<SpoolChunkManifest>,
}

#[derive(Debug, Clone)]
pub struct RecoveredSpool {
    pub audio_path: PathBuf,
    pub duration_secs: f64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn is_safe_spool_path(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

fn spool_size(path: &Path) -> anyhow::Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    if !metadata.is_dir() {
        return Ok(0);
    }
    let mut total: u64 = 0;
    for entry in fs::read_dir(path)? {
        total = total.saturating_add(spool_size(&entry?.path())?);
    }
    Ok(total)
}

fn spool_encryption_enabled() -> bool {
    cfg!(feature = "encrypted-spool")
        && std::env::var("VOICEFLOW_ENCRYPT_SPOOL")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
}

fn history_encryption_key_for_write() -> anyhow::Result<[u8; 32]> {
    if let Some(key) = crate::keychain::get_history_key().map_err(anyhow::Error::msg)? {
        return key
            .try_into()
            .map_err(|_| anyhow::anyhow!("stored history key must be exactly 32 bytes"));
    }

    // Generate the key only after encryption has been explicitly enabled.
    // If the credential store cannot persist it, the verification read below
    // fails and no plaintext fallback is allowed.
    let generated = XChaCha20Poly1305::generate_key(&mut OsRng);
    crate::keychain::set_history_key(Some(generated.as_slice())).map_err(anyhow::Error::msg)?;
    crate::keychain::get_history_key()
        .map_err(anyhow::Error::msg)?
        .ok_or_else(|| anyhow::anyhow!("history encryption key is unavailable"))?
        .try_into()
        .map_err(|_| anyhow::anyhow!("stored history key must be exactly 32 bytes"))
}

fn cipher_for_key(key: &[u8]) -> anyhow::Result<XChaCha20Poly1305> {
    XChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| anyhow::anyhow!("history encryption key must be exactly 32 bytes"))
}

fn encrypt_spool_bytes(bytes: &[u8], key: &[u8]) -> anyhow::Result<Vec<u8>> {
    let cipher = cipher_for_key(key)?;
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let mut header = Vec::with_capacity(SPOOL_ENVELOPE_HEADER_LEN);
    header.extend_from_slice(SPOOL_ENVELOPE_MAGIC);
    header.push(SPOOL_ENVELOPE_VERSION);
    header.extend_from_slice(nonce.as_slice());
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: bytes,
                aad: &header,
            },
        )
        .map_err(|_| anyhow::anyhow!("failed to encrypt audio spool"))?;
    header.extend_from_slice(&ciphertext);
    Ok(header)
}

fn decrypt_spool_bytes(bytes: &[u8], key: &[u8]) -> anyhow::Result<Vec<u8>> {
    if bytes.len() < SPOOL_ENVELOPE_HEADER_LEN
        || &bytes[..SPOOL_ENVELOPE_MAGIC.len()] != SPOOL_ENVELOPE_MAGIC
        || bytes[SPOOL_ENVELOPE_MAGIC.len()] != SPOOL_ENVELOPE_VERSION
    {
        anyhow::bail!("unsupported audio spool envelope");
    }
    let header = &bytes[..SPOOL_ENVELOPE_HEADER_LEN];
    let nonce = XNonce::from_slice(
        &header[SPOOL_ENVELOPE_MAGIC.len() + 1..SPOOL_ENVELOPE_HEADER_LEN],
    );
    cipher_for_key(key)?
        .decrypt(
            nonce,
            Payload {
                msg: &bytes[SPOOL_ENVELOPE_HEADER_LEN..],
                aad: header,
            },
        )
        .map_err(|_| anyhow::anyhow!("failed to decrypt audio spool"))
}

/// Read a recovery artifact. Plaintext artifacts from older versions remain
/// readable; encrypted artifacts require the history key and are never
/// returned as ciphertext.
pub fn read_spool_file(path: &Path) -> anyhow::Result<Vec<u8>> {
    let bytes = fs::read(path)?;
    if bytes.starts_with(SPOOL_ENVELOPE_MAGIC) {
        let key = crate::keychain::get_history_key()
            .map_err(anyhow::Error::msg)?
            .ok_or_else(|| anyhow::anyhow!("history encryption key is unavailable"))?;
        return decrypt_spool_bytes(&bytes, &key);
    }
    Ok(bytes)
}

/// Persist a retry/recovery audio file without exposing paths outside the
/// app-owned spool directory. The quota is checked before writing and the
/// final filename is installed with an atomic rename.
pub fn write_spool_file(dir: &Path, relative: &Path, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let encryption_enabled = spool_encryption_enabled();
    let encryption_key = if encryption_enabled {
        Some(history_encryption_key_for_write()?)
    } else {
        None
    };
    write_spool_file_internal(
        dir,
        relative,
        bytes,
        encryption_enabled,
        encryption_key.as_ref().map(|key| key.as_slice()),
    )
}

fn write_spool_file_internal(
    dir: &Path,
    relative: &Path,
    bytes: &[u8],
    encryption_enabled: bool,
    encryption_key: Option<&[u8]>,
) -> anyhow::Result<PathBuf> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        anyhow::bail!("invalid spool path");
    }
    let root = dir.join("spool");
    let path = root.join(relative);
    let stored_bytes = if encryption_enabled {
        let key = encryption_key
            .ok_or_else(|| anyhow::anyhow!("history encryption key is unavailable"))?;
        encrypt_spool_bytes(bytes, key)?
    } else {
        bytes.to_vec()
    };
    let current = spool_size(&root)?;
    let existing = fs::symlink_metadata(&path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if current
        .saturating_sub(existing)
        .saturating_add(stored_bytes.len() as u64)
        > MAX_SPOOL_BYTES
    {
        anyhow::bail!("audio spool quota exceeded");
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid spool path"))?;
    ensure_private_dir(parent)?;
    write_atomic_bytes(&path, &stored_bytes)?;
    Ok(path)
}

fn write_atomic_bytes(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid atomic path"))?;
    ensure_private_dir(parent)?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("invalid atomic filename"))?
        .to_string_lossy();
    let tmp = parent.join(format!(".{name}.tmp-{}", std::process::id()));
    let write_result = (|| -> anyhow::Result<()> {
        let mut file = File::create(&tmp)?;
        restrict_file_mode(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        restrict_file_mode(path)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    write_result
}

fn manifest_path(session_dir: &Path) -> PathBuf {
    session_dir.join("manifest.json")
}

fn load_manifest(session_dir: &Path) -> anyhow::Result<SpoolManifest> {
    Ok(serde_json::from_slice(&fs::read(manifest_path(
        session_dir,
    ))?)?)
}

fn save_manifest(session_dir: &Path, manifest: &SpoolManifest) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(manifest)?;
    write_atomic_bytes(&manifest_path(session_dir), &bytes)
}

pub fn begin_spool_session(root: &Path, session_id: &str) -> anyhow::Result<PathBuf> {
    if session_id.trim().is_empty()
        || Path::new(session_id)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        anyhow::bail!("invalid spool session id");
    }
    let session_dir = root.join("spool").join(session_id);
    ensure_private_dir(&session_dir)?;
    save_manifest(
        &session_dir,
        &SpoolManifest {
            schema_version: SPOOL_MANIFEST_VERSION,
            session_id: session_id.to_owned(),
            created_at_ms: now_ms(),
            status: "active".into(),
            chunks: Vec::new(),
        },
    )?;
    Ok(session_dir)
}

pub fn record_spool_chunk(
    session_dir: &Path,
    index: usize,
    start_secs: f32,
    end_secs: f32,
    status: &str,
) -> anyhow::Result<()> {
    let mut manifest = load_manifest(session_dir)?;
    if let Some(chunk) = manifest
        .chunks
        .iter_mut()
        .find(|chunk| chunk.index == index)
    {
        chunk.start_secs = start_secs;
        chunk.end_secs = end_secs;
        chunk.status = status.to_owned();
    } else {
        manifest.chunks.push(SpoolChunkManifest {
            index,
            start_secs,
            end_secs,
            status: status.to_owned(),
        });
    }
    save_manifest(session_dir, &manifest)
}

pub fn mark_spool_status(session_dir: &Path, status: &str) -> anyhow::Result<()> {
    let mut manifest = load_manifest(session_dir)?;
    manifest.status = status.to_owned();
    save_manifest(session_dir, &manifest)
}

fn read_recovery_samples(session_dir: &Path) -> anyhow::Result<Vec<f32>> {
    let chunks_dir = session_dir.join("chunks");
    if !chunks_dir.is_dir() {
        anyhow::bail!("recovery chunks are missing");
    }
    let mut chunks = fs::read_dir(&chunks_dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("f32") {
                return None;
            }
            let index = path
                .file_stem()
                .and_then(|value| value.to_str())
                .and_then(|value| value.parse::<usize>().ok())?;
            Some((index, path))
        })
        .collect::<Vec<_>>();
    chunks.sort_by_key(|(index, _)| *index);
    if chunks.is_empty() {
        anyhow::bail!("recovery contains no complete audio chunks");
    }

    let mut samples = Vec::new();
    for (_, path) in chunks {
        let bytes = read_spool_file(&path)?;
        if bytes.len() % std::mem::size_of::<f32>() != 0 {
            anyhow::bail!("recovery audio chunk is truncated");
        }
        samples.extend(
            bytes
                .chunks_exact(4)
                .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
        );
    }
    if samples.is_empty() {
        anyhow::bail!("recovery contains no audio samples");
    }
    Ok(samples)
}

fn write_spool_session_file(
    session_dir: &Path,
    file_name: &str,
    bytes: &[u8],
) -> anyhow::Result<PathBuf> {
    let session_id = session_dir
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("invalid spool session path"))?;
    let root = session_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow::anyhow!("invalid spool session path"))?;
    write_spool_file(root, &PathBuf::from(session_id).join(file_name), bytes)
}

fn rebuild_recovery_wav(session_dir: &Path) -> anyhow::Result<RecoveredSpool> {
    let samples = read_recovery_samples(session_dir)?;
    let wav = crate::chunker::encode_wav(&samples).map_err(|error| anyhow::anyhow!(error))?;
    let audio_path = write_spool_session_file(session_dir, "recovery.wav", &wav)?;
    Ok(RecoveredSpool {
        audio_path,
        duration_secs: samples.len() as f64 / AUDIO_SAMPLE_RATE as f64,
    })
}

/// Rebuild a retryable WAV from the complete audio chunks kept for a long
/// recording. This is also used when processing finishes in a degraded state
/// so the user can retry the whole recording instead of losing failed chunks.
pub fn rebuild_spool_recovery(session_dir: &Path) -> anyhow::Result<RecoveredSpool> {
    rebuild_recovery_wav(session_dir)
}

/// Recover interrupted sessions into retryable audio artifacts. Active
/// manifests are never treated as successful dictations: only complete,
/// atomically-written chunks are rebuilt into a WAV for manual retry.
pub fn recover_spool(dir: &Path, keep_audio_days: u64) -> anyhow::Result<Vec<RecoveredSpool>> {
    let root = dir.join("spool");
    if !root.exists() {
        return Ok(Vec::new());
    }
    let max_age = keep_audio_days.saturating_mul(86_400);
    let mut recovered = Vec::new();
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        let path = entry.path();
        let age = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(max_age.saturating_add(1));
        if path.is_dir() {
            let manifest = match load_manifest(&path) {
                Ok(manifest) if manifest.schema_version == SPOOL_MANIFEST_VERSION => Some(manifest),
                Ok(manifest) => {
                    log::warn!(
                        "discarding unsupported spool manifest version {} at {}",
                        manifest.schema_version,
                        path.display()
                    );
                    None
                }
                Err(error) => {
                    log::warn!(
                        "discarding unreadable spool session at {}: {error}",
                        path.display()
                    );
                    None
                }
            };
            let recoverable = manifest.as_ref().is_some_and(|manifest| {
                matches!(
                    manifest.status.as_str(),
                    "active" | "recoverable" | "degraded"
                )
            });
            if recoverable && age <= max_age {
                match rebuild_recovery_wav(&path) {
                    Ok(recovery) => {
                        let _ = mark_spool_status(&path, "recoverable");
                        recovered.push(recovery);
                    }
                    Err(_) => {
                        let _ = mark_spool_status(&path, "abandoned");
                        fs::remove_dir_all(path)?;
                    }
                }
            } else if age > max_age {
                fs::remove_dir_all(path)?;
            } else if manifest.is_none() {
                // There is no safe way to reconstruct ownership/status from a
                // malformed manifest. Remove it now instead of letting an
                // orphan consume the spool quota indefinitely.
                fs::remove_dir_all(path)?;
            }
        } else if age > max_age {
            fs::remove_file(path)?;
        }
    }
    Ok(recovered)
}

fn secret_sidecar_slot(slot: &str) -> Option<&'static str> {
    match slot {
        "api_key" => Some("api_key"),
        "asr_api_key" => Some("asr_api_key"),
        "cleanup_api_key" => Some("cleanup_api_key"),
        _ => None,
    }
}

fn secret_sidecar_path(dir: &Path, slot: &str) -> PathBuf {
    let slot = secret_sidecar_slot(slot).unwrap_or("api_key");
    dir.join("secrets").join(slot)
}

fn write_secret_sidecar(dir: &Path, slot: &str, key: &str) -> anyhow::Result<()> {
    let slot = secret_sidecar_slot(slot)
        .ok_or_else(|| anyhow::anyhow!("unsupported secret sidecar slot"))?;
    write_atomic_bytes(&secret_sidecar_path(dir, slot), key.as_bytes())
}

fn read_secret_sidecar(dir: &Path, slot: &str) -> Option<String> {
    let bytes = fs::read(secret_sidecar_path(dir, slot)).ok()?;
    let key = String::from_utf8(bytes).ok()?;
    let key = key.trim();
    (!key.is_empty()).then(|| key.to_string())
}

fn clear_secret_sidecar(dir: &Path, slot: &str) {
    let _ = fs::remove_file(secret_sidecar_path(dir, slot));
}

fn fill_empty_secret_from_sidecar(dir: &Path, slot: &str, current: &mut String) {
    if !current.trim().is_empty() {
        return;
    }
    if let Some(key) = read_secret_sidecar(dir, slot) {
        *current = key;
    }
}

fn persist_secret_to_keychain_or_sidecar(
    dir: &Path,
    slot: &str,
    key: &str,
    store: impl FnOnce(&str) -> Result<(), String>,
) -> anyhow::Result<()> {
    match store(key) {
        Ok(()) => {
            clear_secret_sidecar(dir, slot);
            Ok(())
        }
        Err(error) => {
            log::warn!("keychain write for {slot} failed ({error}); storing in app data");
            write_secret_sidecar(dir, slot, key).map_err(|sidecar_error| {
                anyhow::anyhow!(
                    "credential_storage: failed to store {slot} securely: {error}; sidecar: {sidecar_error}"
                )
            })
        }
    }
}

/// Load settings from `settings.json`, then reconcile the API key with the
/// OS credential store. Returns the settings plus a flag indicating whether
/// the caller should persist the normalized result.
pub fn load_settings(dir: &Path) -> (Settings, bool) {
    let path = dir.join("settings.json");
    let raw = std::fs::read(&path).ok();
    let malformed = raw
        .as_deref()
        .is_some_and(|bytes| serde_json::from_slice::<Settings>(bytes).is_err());
    if malformed {
        log::error!("settings.json is malformed; keeping it untouched and starting with defaults");
    }
    let mut settings: Settings = raw
        .as_deref()
        .and_then(|bytes| serde_json::from_slice(bytes).ok())
        .unwrap_or_default();
    let schema_needs_persist = settings.schema_version < SETTINGS_SCHEMA_VERSION;
    let needs_backup = schema_needs_persist
        || !settings.api_key.is_empty()
        || !settings.asr_api_key.is_empty()
        || !settings.cleanup_api_key.is_empty();
    if needs_backup {
        if let Some(raw) = raw.as_deref() {
            if let Err(error) = backup_legacy_settings(&path, raw) {
                log::warn!("failed to back up legacy settings before migration: {error}");
            }
        }
    }
    let hotkey_before = settings.hotkey.clone();
    let selected_action_hotkey_before = settings.selected_action_hotkey.clone();
    settings.normalize();
    let hotkeys_rewritten = settings.hotkey != hotkey_before
        || settings.selected_action_hotkey != selected_action_hotkey_before;

    // Reconcile the API key with the keychain. If we find a plaintext key in
    // the file, migrate it into the keychain and flag that the file should be
    // re-saved with the key removed.
    let plaintext_key = settings.api_key.clone();
    let had_plaintext = !plaintext_key.is_empty();
    let key_state = crate::keychain::resolve_api_key(&plaintext_key);
    let plaintext_asr_key = settings.asr_api_key.clone();
    let had_plaintext_asr = !plaintext_asr_key.is_empty();
    let asr_key_state = crate::keychain::resolve_asr_api_key(&plaintext_asr_key);
    let mut needs_persist = schema_needs_persist || hotkeys_rewritten;
    let mut api_key_missing = false;
    match key_state {
        crate::keychain::ApiKeyState::Configured(key) => {
            settings.api_key = key;
            if had_plaintext {
                needs_persist = true;
            }
        }
        crate::keychain::ApiKeyState::Missing => {
            api_key_missing = true;
            settings.api_key.clear();
        }
        crate::keychain::ApiKeyState::Unavailable(error) => {
            // Do not rewrite settings or revoke onboarding when the OS store
            // is temporarily unavailable. The next launch can retry, and the
            // settings UI can still offer an explicit replacement/removal.
            log::warn!("API key state unavailable during startup: {error}");
            if had_plaintext {
                settings.api_key = plaintext_key;
            }
        }
    }
    let mut asr_key_missing = false;
    match asr_key_state {
        crate::keychain::ApiKeyState::Configured(key) => {
            settings.asr_api_key = key;
            if had_plaintext_asr {
                needs_persist = true;
            }
        }
        crate::keychain::ApiKeyState::Missing => {
            asr_key_missing = true;
            settings.asr_api_key.clear();
            if had_plaintext_asr {
                needs_persist = true;
            }
        }
        crate::keychain::ApiKeyState::Unavailable(error) => {
            log::warn!("ASR API key state unavailable during startup: {error}");
            if had_plaintext_asr {
                settings.asr_api_key = plaintext_asr_key;
            }
        }
    }
    let plaintext_cleanup_key = settings.cleanup_api_key.clone();
    let had_plaintext_cleanup = !plaintext_cleanup_key.is_empty();
    let cleanup_key_state = crate::keychain::resolve_cleanup_api_key(&plaintext_cleanup_key);
    let mut cleanup_key_missing = false;
    match cleanup_key_state {
        crate::keychain::ApiKeyState::Configured(key) => {
            settings.cleanup_api_key = key;
            if had_plaintext_cleanup {
                needs_persist = true;
            }
        }
        crate::keychain::ApiKeyState::Missing => {
            cleanup_key_missing = true;
            settings.cleanup_api_key.clear();
            if had_plaintext_cleanup {
                needs_persist = true;
            }
        }
        crate::keychain::ApiKeyState::Unavailable(error) => {
            log::warn!("cleanup API key state unavailable during startup: {error}");
            if had_plaintext_cleanup {
                settings.cleanup_api_key = plaintext_cleanup_key;
            }
        }
    }
    fill_empty_secret_from_sidecar(dir, "api_key", &mut settings.api_key);
    fill_empty_secret_from_sidecar(dir, "asr_api_key", &mut settings.asr_api_key);
    fill_empty_secret_from_sidecar(dir, "cleanup_api_key", &mut settings.cleanup_api_key);
    for provider in crate::providers::EngineProvider::ALL {
        match crate::keychain::get_provider_api_key_state(provider) {
            crate::keychain::ApiKeyState::Configured(key) => {
                settings
                    .provider_api_keys
                    .entry(provider.as_str().to_owned())
                    .or_insert(key);
            }
            _ => {}
        }
    }
    bind_legacy_keys_into_pool(&mut settings);
    if api_key_missing && settings.api_key.trim().is_empty() {
        if settings.onboarded {
            needs_persist = true;
        }
        settings.onboarded = false;
    }
    // Only repair when the store confirmed the key is absent. A timeout must
    // not wipe a custom URL that still has a credential in the keychain.
    if asr_key_missing
        && settings.asr_api_key.trim().is_empty()
        && settings.repair_incomplete_asr()
    {
        needs_persist = true;
    }
    if cleanup_key_missing
        && settings.cleanup_api_key.trim().is_empty()
        && settings.repair_incomplete_cleanup()
    {
        needs_persist = true;
    }
    (settings, needs_persist)
}

fn bind_legacy_keys_into_pool(settings: &mut Settings) {
    if !settings.api_key.trim().is_empty() {
        settings
            .provider_api_keys
            .entry("groq".into())
            .or_insert_with(|| settings.api_key.clone());
    }
    if !settings.asr_provider.is_groq() && !settings.asr_api_key.trim().is_empty() {
        settings
            .provider_api_keys
            .entry(settings.asr_provider.as_str().to_owned())
            .or_insert_with(|| settings.asr_api_key.clone());
    }
    if !settings.cleanup_provider.is_groq() && !settings.cleanup_api_key.trim().is_empty() {
        settings
            .provider_api_keys
            .entry(settings.cleanup_provider.as_str().to_owned())
            .or_insert_with(|| settings.cleanup_api_key.clone());
    }
}

/// Keep a rollback point for settings migrations without copying a legacy API
/// key back onto disk. The backup is intentionally created only once so later
/// ordinary settings writes do not overwrite the original migration point.
fn backup_legacy_settings(path: &Path, raw: &[u8]) -> anyhow::Result<()> {
    let backup = path.with_file_name("settings.json.pre-migration.bak");
    if backup.exists() {
        return Ok(());
    }
    let mut value: serde_json::Value = serde_json::from_slice(raw)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("legacy settings must be a JSON object"))?;
    object.insert("api_key".into(), serde_json::Value::String(String::new()));
    object.insert(
        "asr_api_key".into(),
        serde_json::Value::String(String::new()),
    );
    object.insert(
        "cleanup_api_key".into(),
        serde_json::Value::String(String::new()),
    );
    let bytes = serde_json::to_vec_pretty(&value)?;
    write_atomic_bytes(&backup, &bytes)?;
    Ok(())
}
/// Persist settings to `settings.json`. The API key is stored in the OS
/// credential store, never in the file, so the on-disk JSON always has the
/// key field blanked out.
pub fn save_settings(dir: &Path, settings: &Settings) -> anyhow::Result<()> {
    settings.validate()?;
    let lock = SETTINGS_WRITE_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_private_dir(dir)?;
    // Write a new key into the keychain first; only persist the file (without
    // the key) once the secret is safely stored. An empty in-memory key can
    // also mean that a non-interactive keychain read timed out during startup,
    // so ordinary settings writes must never interpret it as a destructive
    // credential deletion. Credential removal needs an explicit operation.
    if !settings.api_key.trim().is_empty() {
        persist_secret_to_keychain_or_sidecar(dir, "api_key", &settings.api_key, crate::keychain::set_api_key)?;
    }
    if !settings.asr_api_key.trim().is_empty() {
        persist_secret_to_keychain_or_sidecar(
            dir,
            "asr_api_key",
            &settings.asr_api_key,
            crate::keychain::set_asr_api_key,
        )?;
    }
    if !settings.cleanup_api_key.trim().is_empty() {
        persist_secret_to_keychain_or_sidecar(
            dir,
            "cleanup_api_key",
            &settings.cleanup_api_key,
            crate::keychain::set_cleanup_api_key,
        )?;
    }
    for (id, key) in &settings.provider_api_keys {
        let Some(provider) = crate::providers::EngineProvider::parse(id) else {
            continue;
        };
        if key.trim().is_empty() {
            continue;
        }
        persist_secret_to_keychain_or_sidecar(
            dir,
            provider.keychain_account(),
            key,
            |value| crate::keychain::set_provider_api_key(provider, value),
        )?;
    }
    let mut on_disk = settings.clone();
    on_disk.api_key = String::new();
    on_disk.asr_api_key = String::new();
    on_disk.cleanup_api_key = String::new();
    on_disk.provider_api_keys.clear();
    let bytes = serde_json::to_vec_pretty(&on_disk)?;
    let path = dir.join("settings.json");
    write_atomic_bytes(&path, &bytes)?;
    Ok(())
}
static SETTINGS_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn schema(c: &Connection) -> anyhow::Result<()> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS dictations (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, duration_secs REAL, raw_text TEXT, final_text TEXT, cleanup_status TEXT, engine TEXT, degraded INTEGER, degraded_reason TEXT, status TEXT, raw_audio_path TEXT, delivery_method TEXT, fallback_reason TEXT, context_profile_id TEXT, context_policy_json TEXT); CREATE TABLE IF NOT EXISTS dictation_revisions (revision_id INTEGER PRIMARY KEY, dictation_id INTEGER NOT NULL, created_at TEXT NOT NULL, final_text TEXT NOT NULL, cleanup_status TEXT, intent_json TEXT, model TEXT, context_policy_json TEXT, revision_reason TEXT NOT NULL, FOREIGN KEY(dictation_id) REFERENCES dictations(id) ON DELETE CASCADE); CREATE INDEX IF NOT EXISTS idx_dictation_revisions_dictation ON dictation_revisions(dictation_id, revision_id DESC); CREATE TABLE IF NOT EXISTS usage (day TEXT PRIMARY KEY, asr_requests INTEGER NOT NULL DEFAULT 0, llm_requests INTEGER NOT NULL DEFAULT 0, audio_seconds REAL NOT NULL DEFAULT 0);")?;
    ensure_column(c, "raw_audio_path", "TEXT")?;
    ensure_column(c, "degraded_reason", "TEXT")?;
    ensure_column(c, "delivery_method", "TEXT")?;
    ensure_column(c, "fallback_reason", "TEXT")?;
    ensure_column(c, "context_profile_id", "TEXT")?;
    ensure_column(c, "context_policy_json", "TEXT")?;
    ensure_column(c, "context_family", "TEXT")?;
    ensure_column(c, "context_browser_host", "TEXT")?;
    ensure_column(c, "context_native_bundle", "TEXT")?;
    ensure_column(c, "cleanup_status", "TEXT")?;
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS learn_pairs (
            pair_key TEXT PRIMARY KEY,
            before_surface TEXT NOT NULL,
            after_surface TEXT NOT NULL,
            hits INTEGER NOT NULL,
            promoted INTEGER NOT NULL DEFAULT 0,
            last_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS style_drafts (
            draft_key TEXT PRIMARY KEY,
            mapping_id TEXT NOT NULL,
            style_key TEXT NOT NULL,
            excerpt TEXT NOT NULL,
            before_excerpt TEXT NOT NULL,
            after_excerpt TEXT NOT NULL,
            created_at TEXT NOT NULL
        );",
    )?;
    ensure_table_column(c, "learn_pairs", "family", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "mapping_id", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "browser_host", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "native_bundle", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "last_used_at", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "pinned", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_table_column(c, "learn_pairs", "tombstoned_at", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "ignored", "INTEGER NOT NULL DEFAULT 0")?;
    c.pragma_update(None, "user_version", HISTORY_SCHEMA_VERSION)?;
    Ok(())
}

fn open_history(dir: &Path) -> anyhow::Result<Connection> {
    ensure_private_dir(dir)?;
    let path = dir.join("history.sqlite");
    let existed = path.exists();
    let connection = Connection::open(&path)?;
    restrict_file_mode(&path)?;
    connection.execute_batch("PRAGMA foreign_keys = ON;")?;
    // History writes can overlap with a completion, recovery scan, or the
    // settings window loading its list. Let SQLite briefly wait for the
    // active writer instead of turning a transient lock into lost history.
    connection.busy_timeout(Duration::from_secs(2))?;

    let schema_lock = HISTORY_SCHEMA_LOCK.get_or_init(|| Mutex::new(()));
    let _schema_guard = schema_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_version: i32 =
        connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if existed && previous_version < HISTORY_SCHEMA_VERSION {
        let backup = dir.join(format!("history.sqlite.v{previous_version}.bak"));
        if !backup.exists() {
            fs::copy(&path, &backup)?;
            restrict_file_mode(&backup)?;
        }
    }
    schema(&connection)?;
    restrict_history_sidecars(dir)?;
    Ok(connection)
}

static HISTORY_SCHEMA_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn ensure_column(c: &Connection, name: &str, definition: &str) -> anyhow::Result<()> {
    ensure_table_column(c, "dictations", name, definition)
}

fn ensure_table_column(
    c: &Connection,
    table: &str,
    name: &str,
    definition: &str,
) -> anyhow::Result<()> {
    let mut statement = c.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|column| column == name);
    if !exists {
        c.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {name} {definition}"),
            [],
        )?;
    }
    Ok(())
}
pub struct InsertHistory<'a> {
    pub dir: &'a Path,
    pub raw: &'a str,
    pub final_text: &'a str,
    pub cleanup_status: Option<&'a str>,
    pub duration: f64,
    pub degraded: bool,
    pub degraded_reason: Option<&'a str>,
    pub status: String,
    pub delivery_method: Option<&'a str>,
    pub fallback_reason: Option<&'a str>,
    pub spool: Option<&'a Path>,
    pub count_asr: bool,
    pub count_llm: bool,
    pub context: Option<&'a crate::context::ContextSnapshot>,
}

#[cfg(test)]
pub fn insert_history(
    dir: &Path,
    raw: &str,
    final_text: &str,
    duration: f64,
    degraded: bool,
) -> anyhow::Result<()> {
    insert_history_with_status(InsertHistory {
        dir,
        raw,
        final_text,
        cleanup_status: None,
        duration,
        degraded,
        degraded_reason: None,
        status: if degraded { "degraded" } else { "ok" }.to_owned(),
        delivery_method: Some("history"),
        fallback_reason: None,
        spool: None,
        count_asr: true,
        count_llm: true,
        context: None,
    })
}

#[allow(dead_code)]
pub fn insert_history_with_context(
    dir: &Path,
    raw: &str,
    final_text: &str,
    duration: f64,
    degraded: bool,
    context: &crate::context::ContextSnapshot,
) -> anyhow::Result<()> {
    insert_history_with_delivery(
        dir,
        raw,
        final_text,
        duration,
        degraded,
        None,
        if degraded { "degraded" } else { "ok" },
        "history",
        None,
        context,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn insert_history_with_delivery(
    dir: &Path,
    raw: &str,
    final_text: &str,
    duration: f64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    context: &crate::context::ContextSnapshot,
) -> anyhow::Result<()> {
    insert_history_with_delivery_and_spool(
        dir,
        raw,
        final_text,
        duration,
        degraded,
        degraded_reason,
        status,
        delivery_method,
        fallback_reason,
        context,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn insert_history_with_delivery_and_spool(
    dir: &Path,
    raw: &str,
    final_text: &str,
    duration: f64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    context: &crate::context::ContextSnapshot,
    spool: Option<&Path>,
) -> anyhow::Result<()> {
    insert_history_with_status(InsertHistory {
        dir,
        raw,
        final_text,
        cleanup_status: None,
        duration,
        degraded,
        degraded_reason,
        status: status.to_owned(),
        delivery_method: Some(delivery_method),
        fallback_reason,
        spool,
        count_asr: true,
        count_llm: true,
        context: Some(context),
    })
}

#[allow(clippy::too_many_arguments)]
pub fn insert_history_with_delivery_and_spool_and_cleanup(
    dir: &Path,
    raw: &str,
    final_text: &str,
    duration: f64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    context: &crate::context::ContextSnapshot,
    spool: Option<&Path>,
    cleanup_status: &str,
) -> anyhow::Result<()> {
    insert_history_with_status(InsertHistory {
        dir,
        raw,
        final_text,
        cleanup_status: Some(cleanup_status),
        duration,
        degraded,
        degraded_reason,
        status: status.to_owned(),
        delivery_method: Some(delivery_method),
        fallback_reason,
        spool,
        count_asr: true,
        count_llm: true,
        context: Some(context),
    })
}
#[cfg(test)]
#[allow(dead_code)]
pub fn insert_failed_history(
    dir: &Path,
    raw: &str,
    duration: f64,
    spool: Option<&Path>,
) -> anyhow::Result<()> {
    insert_history_with_status(InsertHistory {
        dir,
        raw,
        final_text: "",
        cleanup_status: None,
        duration,
        degraded: false,
        degraded_reason: Some("asr_failed"),
        status: "failed".to_owned(),
        delivery_method: Some("none"),
        fallback_reason: Some("asr_failed"),
        spool,
        count_asr: false,
        count_llm: false,
        context: None,
    })
}

pub fn insert_failed_history_with_context(
    dir: &Path,
    raw: &str,
    duration: f64,
    spool: Option<&Path>,
    context: &crate::context::ContextSnapshot,
) -> anyhow::Result<()> {
    insert_history_with_status(InsertHistory {
        dir,
        raw,
        final_text: "",
        cleanup_status: None,
        duration,
        degraded: false,
        degraded_reason: Some("asr_failed"),
        status: "failed".to_owned(),
        delivery_method: Some("none"),
        fallback_reason: Some("asr_failed"),
        spool,
        count_asr: false,
        count_llm: false,
        context: Some(context),
    })
}
fn insert_history_with_status(input: InsertHistory) -> anyhow::Result<()> {
    let InsertHistory {
        dir,
        raw,
        final_text,
        cleanup_status,
        duration,
        degraded,
        degraded_reason,
        status,
        delivery_method,
        fallback_reason,
        spool,
        count_asr,
        count_llm,
        context,
    } = input;
    let c = open_history(dir)?;
    let context_profile_id = context.map(|snapshot| snapshot.profile.id.as_str());
    let context_policy_json = context
        .map(|snapshot| serde_json::to_string(&snapshot.policy))
        .transpose()?;
    let context_family =
        context.map(|snapshot| crate::context::family_id(snapshot.profile.family).to_owned());
    let context_browser_host =
        context.and_then(|snapshot| snapshot.target_guard.browser_host.clone());
    let context_native_bundle = context.and_then(|snapshot| {
        if snapshot.target_guard.browser_host.is_some() {
            None
        } else {
            snapshot.target_guard.bundle_id.clone()
        }
    });
    c.execute("INSERT INTO dictations (created_at,duration_secs,raw_text,final_text,cleanup_status,engine,degraded,degraded_reason,status,raw_audio_path,delivery_method,fallback_reason,context_profile_id,context_policy_json,context_family,context_browser_host,context_native_bundle) VALUES (datetime('now'),?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)", params![duration, raw, final_text, cleanup_status, "groq", degraded as i32, degraded_reason, status, spool.map(|p| p.to_string_lossy().to_string()), delivery_method, fallback_reason, context_profile_id, context_policy_json, context_family, context_browser_host, context_native_bundle])?;
    if count_asr || count_llm {
        c.execute("INSERT INTO usage(day,asr_requests,llm_requests,audio_seconds) VALUES (date('now'),?,?,?) ON CONFLICT(day) DO UPDATE SET asr_requests=asr_requests+excluded.asr_requests,llm_requests=llm_requests+excluded.llm_requests,audio_seconds=audio_seconds+excluded.audio_seconds", params![count_asr as i64, count_llm as i64, duration])?;
    }
    Ok(())
}
pub fn get_usage(dir: &Path, quota: QuotaView) -> anyhow::Result<Usage> {
    let c = open_history(dir)?;
    let row = match c.query_row(
        "SELECT asr_requests,llm_requests,audio_seconds FROM usage WHERE day=date('now')",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ) {
        Ok(row) => row,
        Err(rusqlite::Error::QueryReturnedNoRows) => (0, 0, 0.0),
        Err(error) => return Err(error.into()),
    };
    Ok(Usage {
        asr_requests: row.0,
        llm_requests: row.1,
        audio_seconds: row.2,
        quota,
    })
}
pub fn get_history_page(
    dir: &Path,
    limit: i64,
    before_id: Option<i64>,
    query: Option<&str>,
) -> anyhow::Result<HistoryPage> {
    let c = open_history(dir)?;
    let spool_root = dir.join("spool");
    let limit = limit.clamp(1, 100);
    let fetch_limit = limit + 1;
    let search_pattern = query
        .filter(|value| !value.is_empty())
        .map(|value| format!("%{}%", escape_history_search(value)));
    let sql = match (before_id.is_some(), search_pattern.is_some()) {
        (true, true) => "SELECT id,created_at,COALESCE(raw_text,''),COALESCE((SELECT final_text FROM dictation_revisions WHERE dictation_id=dictations.id ORDER BY revision_id DESC LIMIT 1),final_text,''),COALESCE(duration_secs,0),COALESCE(degraded,0),degraded_reason,COALESCE(status,'ok'),delivery_method,fallback_reason,context_profile_id,raw_audio_path,COALESCE(cleanup_status,'unknown'),(SELECT COUNT(*) FROM dictation_revisions WHERE dictation_id=dictations.id) FROM dictations WHERE id < ? AND (COALESCE(raw_text,'') LIKE ? ESCAPE '\\' OR COALESCE((SELECT final_text FROM dictation_revisions WHERE dictation_id=dictations.id ORDER BY revision_id DESC LIMIT 1),final_text,'') LIKE ? ESCAPE '\\') ORDER BY id DESC LIMIT ?",
        (true, false) => "SELECT id,created_at,COALESCE(raw_text,''),COALESCE((SELECT final_text FROM dictation_revisions WHERE dictation_id=dictations.id ORDER BY revision_id DESC LIMIT 1),final_text,''),COALESCE(duration_secs,0),COALESCE(degraded,0),degraded_reason,COALESCE(status,'ok'),delivery_method,fallback_reason,context_profile_id,raw_audio_path,COALESCE(cleanup_status,'unknown'),(SELECT COUNT(*) FROM dictation_revisions WHERE dictation_id=dictations.id) FROM dictations WHERE id < ? ORDER BY id DESC LIMIT ?",
        (false, true) => "SELECT id,created_at,COALESCE(raw_text,''),COALESCE((SELECT final_text FROM dictation_revisions WHERE dictation_id=dictations.id ORDER BY revision_id DESC LIMIT 1),final_text,''),COALESCE(duration_secs,0),COALESCE(degraded,0),degraded_reason,COALESCE(status,'ok'),delivery_method,fallback_reason,context_profile_id,raw_audio_path,COALESCE(cleanup_status,'unknown'),(SELECT COUNT(*) FROM dictation_revisions WHERE dictation_id=dictations.id) FROM dictations WHERE (COALESCE(raw_text,'') LIKE ? ESCAPE '\\' OR COALESCE((SELECT final_text FROM dictation_revisions WHERE dictation_id=dictations.id ORDER BY revision_id DESC LIMIT 1),final_text,'') LIKE ? ESCAPE '\\') ORDER BY id DESC LIMIT ?",
        (false, false) => "SELECT id,created_at,COALESCE(raw_text,''),COALESCE((SELECT final_text FROM dictation_revisions WHERE dictation_id=dictations.id ORDER BY revision_id DESC LIMIT 1),final_text,''),COALESCE(duration_secs,0),COALESCE(degraded,0),degraded_reason,COALESCE(status,'ok'),delivery_method,fallback_reason,context_profile_id,raw_audio_path,COALESCE(cleanup_status,'unknown'),(SELECT COUNT(*) FROM dictation_revisions WHERE dictation_id=dictations.id) FROM dictations ORDER BY id DESC LIMIT ?",
    };
    let mut s = c.prepare(sql)?;
    let mut items = match (before_id, search_pattern.as_deref()) {
        (Some(before_id), Some(pattern)) => s
            .query_map(
                params![before_id, pattern, pattern, fetch_limit],
                history_row(&spool_root),
            )?
            .collect::<Result<Vec<_>, _>>()?,
        (Some(before_id), None) => s
            .query_map(params![before_id, fetch_limit], history_row(&spool_root))?
            .collect::<Result<Vec<_>, _>>()?,
        (None, Some(pattern)) => s
            .query_map(params![pattern, pattern, fetch_limit], history_row(&spool_root))?
            .collect::<Result<Vec<_>, _>>()?,
        (None, None) => s
            .query_map([fetch_limit], history_row(&spool_root))?
            .collect::<Result<Vec<_>, _>>()?,
    };
    let has_more = items.len() > limit as usize;
    items.truncate(limit as usize);
    Ok(HistoryPage { items, has_more })
}

fn escape_history_search(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn history_row<'a>(
    spool_root: &'a Path,
) -> impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<HistoryItem> + 'a {
    move |r| {
        let raw_audio_path: Option<String> = r.get(11)?;
        let retryable = raw_audio_path.as_deref().is_some_and(|path| {
            let path = Path::new(path);
            is_safe_spool_path(spool_root, path) && path.is_file()
        });
        Ok(HistoryItem {
            id: r.get(0)?,
            created_at: r.get(1)?,
            raw_text: r.get(2)?,
            final_text: r.get(3)?,
            cleanup_status: r.get(12)?,
            duration: r.get(4)?,
            degraded: r.get::<_, i32>(5)? != 0,
            degraded_reason: r.get(6)?,
            status: r.get(7)?,
            delivery_method: r.get(8)?,
            fallback_reason: r.get(9)?,
            context_profile_id: r.get(10)?,
            retryable,
            revision_count: r.get::<_, i64>(13)? as usize,
        })
    }
}

#[cfg(test)]
pub fn get_history(dir: &Path, limit: i64) -> anyhow::Result<Vec<HistoryItem>> {
    Ok(get_history_page(dir, limit, None, None)?.items)
}

pub fn purge_history(dir: &Path, keep_history_days: u64) -> anyhow::Result<usize> {
    // Zero is the explicit "keep forever" setting for history text.
    if keep_history_days == 0 {
        return Ok(0);
    }
    let c = open_history(dir)?;
    let cutoff = format!("-{keep_history_days} days");
    let mut statement =
        c.prepare("SELECT raw_audio_path FROM dictations WHERE created_at < datetime('now', ?)")?;
    let paths = statement
        .query_map([cutoff.as_str()], |row| row.get::<_, Option<String>>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let deleted = c.execute(
        "DELETE FROM dictations WHERE created_at < datetime('now', ?)",
        [cutoff.as_str()],
    )?;
    drop(statement);
    drop(c);
    for path in paths.into_iter().flatten() {
        remove_spool_artifact(dir, Path::new(&path));
    }
    Ok(deleted)
}

const LEARN_PAIR_COLUMNS: &str = "pair_key, before_surface, after_surface, hits, promoted, last_at, family, mapping_id, browser_host, native_bundle, last_used_at, pinned, tombstoned_at, ignored";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LearnPairScope {
    pub family: Option<String>,
    pub mapping_id: Option<String>,
    pub browser_host: Option<String>,
    pub native_bundle: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnPairRecord {
    pub pair_key: String,
    pub before_surface: String,
    pub after_surface: String,
    pub hits: u32,
    pub promoted: bool,
    pub last_at: String,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub mapping_id: Option<String>,
    #[serde(default)]
    pub browser_host: Option<String>,
    #[serde(default)]
    pub native_bundle: Option<String>,
    #[serde(default)]
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub tombstoned_at: Option<String>,
    #[serde(default)]
    pub ignored: bool,
}

impl LearnPairRecord {
    pub fn is_live_promoted(&self) -> bool {
        self.promoted && !self.ignored && self.tombstoned_at.is_none()
    }

    pub fn is_pending(&self) -> bool {
        !self.promoted && !self.ignored && self.tombstoned_at.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleDraftRecord {
    pub draft_key: String,
    pub mapping_id: String,
    pub style_key: String,
    pub excerpt: String,
    pub before_excerpt: String,
    pub after_excerpt: String,
    pub created_at: String,
}

fn learn_pair_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LearnPairRecord> {
    Ok(LearnPairRecord {
        pair_key: row.get(0)?,
        before_surface: row.get(1)?,
        after_surface: row.get(2)?,
        hits: row.get::<_, i64>(3)? as u32,
        promoted: row.get::<_, i64>(4)? != 0,
        last_at: row.get(5)?,
        family: row.get(6)?,
        mapping_id: row.get(7)?,
        browser_host: row.get(8)?,
        native_bundle: row.get(9)?,
        last_used_at: row.get(10)?,
        pinned: row.get::<_, Option<i64>>(11)?.unwrap_or(0) != 0,
        tombstoned_at: row.get(12)?,
        ignored: row.get::<_, Option<i64>>(13)?.unwrap_or(0) != 0,
    })
}

pub fn upsert_learn_pair(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
) -> anyhow::Result<Option<LearnPairRecord>> {
    upsert_learn_pair_with_scope(dir, pair_key, before_surface, after_surface, None)
}

pub fn upsert_learn_pair_with_scope(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: Option<&LearnPairScope>,
) -> anyhow::Result<Option<LearnPairRecord>> {
    let c = open_history(dir)?;
    if let Some(existing) = c
        .query_row(
            &format!("SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs WHERE pair_key = ?1"),
            [pair_key],
            learn_pair_from_row,
        )
        .optional()?
    {
        if existing.ignored || existing.tombstoned_at.is_some() {
            return Ok(None);
        }
    } else {
        let pending: i64 = c.query_row(
            "SELECT COUNT(*) FROM learn_pairs WHERE promoted = 0 AND ignored = 0 AND tombstoned_at IS NULL",
            [],
            |row| row.get(0),
        )?;
        if pending >= LEARN_PAIRS_PENDING_CAP {
            return Ok(None);
        }
    }
    let scope = scope.cloned().unwrap_or_default();
    let record = c.query_row(
        &format!(
            "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at, family, mapping_id, browser_host, native_bundle, last_used_at, pinned, tombstoned_at, ignored)
             VALUES (?1, ?2, ?3, 1, 0, datetime('now'), ?4, ?5, ?6, ?7, NULL, 0, NULL, 0)
             ON CONFLICT(pair_key) DO UPDATE SET
               hits = hits + 1,
               last_at = datetime('now'),
               family = excluded.family,
               mapping_id = excluded.mapping_id,
               browser_host = excluded.browser_host,
               native_bundle = excluded.native_bundle
             RETURNING {LEARN_PAIR_COLUMNS}"
        ),
        params![
            pair_key,
            before_surface,
            after_surface,
            scope.family,
            scope.mapping_id,
            scope.browser_host,
            scope.native_bundle
        ],
        learn_pair_from_row,
    )?;
    Ok(Some(record))
}

pub fn get_learn_pair(dir: &Path, pair_key: &str) -> anyhow::Result<Option<LearnPairRecord>> {
    let c = open_history(dir)?;
    Ok(c.query_row(
        &format!("SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs WHERE pair_key = ?1"),
        [pair_key],
        learn_pair_from_row,
    )
    .optional()?)
}

pub fn mark_learn_pair_promoted(dir: &Path, pair_key: &str) -> anyhow::Result<bool> {
    let c = open_history(dir)?;
    c.execute(
        "UPDATE learn_pairs SET promoted = 1, ignored = 0, tombstoned_at = NULL, last_at = datetime('now') WHERE pair_key = ?1",
        [pair_key],
    )?;
    Ok(c.changes() > 0)
}

pub fn ensure_learn_pair_promoted(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: Option<&LearnPairScope>,
) -> anyhow::Result<()> {
    let scope = scope.cloned().unwrap_or_default();
    let c = open_history(dir)?;
    c.execute(
        "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at, family, mapping_id, browser_host, native_bundle, pinned, ignored)
         VALUES (?1, ?2, ?3, 3, 1, datetime('now'), ?4, ?5, ?6, ?7, 0, 0)
         ON CONFLICT(pair_key) DO UPDATE SET
           promoted = 1,
           ignored = 0,
           tombstoned_at = NULL,
           last_at = datetime('now'),
           family = COALESCE(learn_pairs.family, excluded.family),
           mapping_id = COALESCE(learn_pairs.mapping_id, excluded.mapping_id),
           browser_host = COALESCE(learn_pairs.browser_host, excluded.browser_host),
           native_bundle = COALESCE(learn_pairs.native_bundle, excluded.native_bundle)",
        params![
            pair_key,
            before_surface,
            after_surface,
            scope.family,
            scope.mapping_id,
            scope.browser_host,
            scope.native_bundle
        ],
    )?;
    Ok(())
}

pub fn tombstone_learn_pair(dir: &Path, pair_key: &str) -> anyhow::Result<bool> {
    let c = open_history(dir)?;
    c.execute(
        "UPDATE learn_pairs SET ignored = 1, tombstoned_at = datetime('now'), last_at = datetime('now') WHERE pair_key = ?1",
        [pair_key],
    )?;
    Ok(c.changes() > 0)
}

#[allow(dead_code)]
pub fn set_learn_pair_pinned(dir: &Path, pair_key: &str, pinned: bool) -> anyhow::Result<bool> {
    let c = open_history(dir)?;
    c.execute(
        "UPDATE learn_pairs SET pinned = ?2, last_at = datetime('now') WHERE pair_key = ?1",
        params![pair_key, i64::from(pinned)],
    )?;
    Ok(c.changes() > 0)
}

pub fn ensure_pinned_dictionary_term(dir: &Path, after_surface: &str, pinned: bool) -> anyhow::Result<()> {
    let pair_key = crate::dictionary_learn::pair_key("", after_surface);
    let c = open_history(dir)?;
    c.execute(
        "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at, pinned, ignored)
         VALUES (?1, '', ?2, 0, 1, datetime('now'), ?3, 0)
         ON CONFLICT(pair_key) DO UPDATE SET
           pinned = excluded.pinned,
           last_at = datetime('now')",
        params![pair_key, after_surface, i64::from(pinned)],
    )?;
    Ok(())
}

pub fn bump_learn_pairs_used(dir: &Path, pair_keys: &[String]) -> anyhow::Result<()> {
    if pair_keys.is_empty() {
        return Ok(());
    }
    let c = open_history(dir)?;
    let mut statement = c.prepare(
        "UPDATE learn_pairs SET last_used_at = datetime('now') WHERE pair_key = ?1",
    )?;
    for key in pair_keys {
        statement.execute([key])?;
    }
    Ok(())
}

pub fn list_learn_pairs(dir: &Path) -> anyhow::Result<Vec<LearnPairRecord>> {
    let c = open_history(dir)?;
    let mut statement = c.prepare(&format!(
        "SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs ORDER BY last_at DESC, pair_key ASC"
    ))?;
    let rows = statement.query_map([], learn_pair_from_row)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn upsert_style_draft(
    dir: &Path,
    mapping_id: &str,
    style_key: &str,
    excerpt: &str,
    before_excerpt: &str,
    after_excerpt: &str,
) -> anyhow::Result<StyleDraftRecord> {
    let draft_key = format!("{mapping_id}\u{1e}{style_key}");
    let c = open_history(dir)?;
    c.execute(
        "INSERT INTO style_drafts (draft_key, mapping_id, style_key, excerpt, before_excerpt, after_excerpt, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))
         ON CONFLICT(draft_key) DO UPDATE SET
           excerpt = excluded.excerpt,
           before_excerpt = excluded.before_excerpt,
           after_excerpt = excluded.after_excerpt,
           created_at = datetime('now')",
        params![draft_key, mapping_id, style_key, excerpt, before_excerpt, after_excerpt],
    )?;
    Ok(StyleDraftRecord {
        draft_key,
        mapping_id: mapping_id.to_owned(),
        style_key: style_key.to_owned(),
        excerpt: excerpt.to_owned(),
        before_excerpt: before_excerpt.to_owned(),
        after_excerpt: after_excerpt.to_owned(),
        created_at: String::new(),
    })
}

pub fn list_style_drafts(dir: &Path) -> anyhow::Result<Vec<StyleDraftRecord>> {
    let c = open_history(dir)?;
    let mut statement = c.prepare(
        "SELECT draft_key, mapping_id, style_key, excerpt, before_excerpt, after_excerpt, created_at
         FROM style_drafts ORDER BY created_at DESC, draft_key ASC",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(StyleDraftRecord {
            draft_key: row.get(0)?,
            mapping_id: row.get(1)?,
            style_key: row.get(2)?,
            excerpt: row.get(3)?,
            before_excerpt: row.get(4)?,
            after_excerpt: row.get(5)?,
            created_at: row.get(6)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn get_style_draft(dir: &Path, draft_key: &str) -> anyhow::Result<Option<StyleDraftRecord>> {
    let c = open_history(dir)?;
    Ok(c.query_row(
        "SELECT draft_key, mapping_id, style_key, excerpt, before_excerpt, after_excerpt, created_at
         FROM style_drafts WHERE draft_key = ?1",
        [draft_key],
        |row| {
            Ok(StyleDraftRecord {
                draft_key: row.get(0)?,
                mapping_id: row.get(1)?,
                style_key: row.get(2)?,
                excerpt: row.get(3)?,
                before_excerpt: row.get(4)?,
                after_excerpt: row.get(5)?,
                created_at: row.get(6)?,
            })
        },
    )
    .optional()?)
}

pub fn delete_style_draft(dir: &Path, draft_key: &str) -> anyhow::Result<bool> {
    let c = open_history(dir)?;
    c.execute("DELETE FROM style_drafts WHERE draft_key = ?1", [draft_key])?;
    Ok(c.changes() > 0)
}

pub fn clear_all_data(dir: &Path) -> anyhow::Result<()> {
    let c = open_history(dir)?;
    c.execute_batch("DELETE FROM dictations; DELETE FROM usage; DELETE FROM learn_pairs; DELETE FROM style_drafts;")?;
    drop(c);
    let spool = dir.join("spool");
    if spool.exists() {
        fs::remove_dir_all(&spool)?;
    }
    ensure_private_dir(&spool)?;
    Ok(())
}

pub fn export_history_json(dir: &Path) -> anyhow::Result<String> {
    let mut items = Vec::new();
    let mut before_id = None;
    loop {
        let page = get_history_page(dir, 100, before_id, None)?;
        if let Some(last) = page.items.last() {
            before_id = Some(last.id);
        }
        let done = !page.has_more;
        items.extend(page.items);
        if done {
            break;
        }
    }
    Ok(serde_json::to_string_pretty(&items)?)
}

pub fn write_export_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    if path.as_os_str().is_empty() || !path.is_absolute() {
        anyhow::bail!("invalid export path");
    }
    write_atomic_bytes(path, contents.as_bytes())
}
pub fn failed_spool(dir: &Path, id: i64) -> anyhow::Result<Option<String>> {
    let c = open_history(dir)?;
    let path: Option<String> = c
        .query_row(
            "SELECT raw_audio_path FROM dictations WHERE id=? AND status IN ('failed','degraded')",
            [id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let root = dir.join("spool");
    Ok(path.filter(|value| {
        let path = Path::new(value);
        is_safe_spool_path(&root, path) && path.is_file()
    }))
}
#[derive(Debug, Clone, Default)]
pub struct HistoryScene {
    pub policy: Option<crate::context::ContextPolicy>,
    pub profile_id: Option<String>,
    pub family: Option<String>,
    pub browser_host: Option<String>,
    pub native_bundle: Option<String>,
}

impl HistoryScene {
    pub fn learn_scope(&self) -> LearnPairScope {
        LearnPairScope {
            family: self.family.clone(),
            mapping_id: self
                .profile_id
                .as_deref()
                .and_then(|id| id.strip_prefix("user."))
                .map(str::to_owned),
            browser_host: self.browser_host.clone(),
            native_bundle: self.native_bundle.clone(),
        }
    }
}

pub fn history_context(
    dir: &Path,
    id: i64,
) -> anyhow::Result<Option<crate::context::ContextPolicy>> {
    Ok(history_scene(dir, id)?.policy)
}

pub fn history_scene(dir: &Path, id: i64) -> anyhow::Result<HistoryScene> {
    let c = open_history(dir)?;
    let scene = c
        .query_row(
            "SELECT context_policy_json, context_profile_id, context_family, context_browser_host, context_native_bundle FROM dictations WHERE id=?",
            [id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((policy_json, profile_id, family, browser_host, native_bundle)) = scene else {
        return Ok(HistoryScene::default());
    };
    Ok(HistoryScene {
        policy: policy_json
            .map(|value| serde_json::from_str(&value))
            .transpose()?,
        profile_id,
        family,
        browser_host,
        native_bundle,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn save_history_revision(
    dir: &Path,
    dictation_id: i64,
    final_text: &str,
    cleanup_status: Option<&str>,
    intent: Option<&crate::llm::CleanupIntent>,
    model: Option<&str>,
    context_policy: Option<&crate::context::ContextPolicy>,
    revision_reason: &str,
) -> anyhow::Result<i64> {
    if final_text.trim().is_empty() {
        anyhow::bail!("history revision cannot be empty");
    }
    if revision_reason.trim().is_empty() || revision_reason.len() > 128 {
        anyhow::bail!("history revision reason is invalid");
    }
    let c = open_history(dir)?;
    let intent_json = intent.map(serde_json::to_string).transpose()?;
    let context_policy_json = context_policy.map(serde_json::to_string).transpose()?;
    c.execute(
        "INSERT INTO dictation_revisions (dictation_id,created_at,final_text,cleanup_status,intent_json,model,context_policy_json,revision_reason) SELECT ?,datetime('now'),?,?,?,?,?,? WHERE EXISTS (SELECT 1 FROM dictations WHERE id=?)",
        params![
            dictation_id,
            final_text,
            cleanup_status,
            intent_json,
            model,
            context_policy_json,
            revision_reason,
            dictation_id
        ],
    )?;
    if c.changes() == 0 {
        anyhow::bail!("history record was not found");
    }
    Ok(c.last_insert_rowid())
}

pub fn get_history_revisions(
    dir: &Path,
    dictation_id: i64,
) -> anyhow::Result<Vec<HistoryRevision>> {
    let c = open_history(dir)?;
    let mut statement = c.prepare("SELECT revision_id,dictation_id,created_at,final_text,cleanup_status,intent_json,model,context_policy_json,revision_reason FROM dictation_revisions WHERE dictation_id=? ORDER BY revision_id ASC")?;
    let rows = statement.query_map([dictation_id], |row| {
        let intent_json: Option<String> = row.get(5)?;
        let context_policy_json: Option<String> = row.get(7)?;
        Ok(HistoryRevision {
            revision_id: row.get(0)?,
            dictation_id: row.get(1)?,
            created_at: row.get(2)?,
            final_text: row.get(3)?,
            cleanup_status: row.get(4)?,
            intent: intent_json.and_then(|value| serde_json::from_str(&value).ok()),
            model: row.get(6)?,
            context_policy: context_policy_json.and_then(|value| serde_json::from_str(&value).ok()),
            revision_reason: row.get(8)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}
#[allow(dead_code)]
pub fn mark_retried(
    dir: &Path,
    id: i64,
    final_text: &str,
    degraded: bool,
    degraded_reason: Option<&str>,
) -> anyhow::Result<()> {
    mark_retried_with_texts(dir, id, None, final_text, degraded, degraded_reason, None)
}

pub fn mark_retried_with_texts(
    dir: &Path,
    id: i64,
    raw_text: Option<&str>,
    final_text: &str,
    degraded: bool,
    degraded_reason: Option<&str>,
    cleanup_status: Option<&str>,
) -> anyhow::Result<()> {
    let c = open_history(dir)?;
    c.execute(
        "INSERT INTO dictation_revisions (dictation_id,created_at,final_text,cleanup_status,revision_reason) SELECT ?,datetime('now'),?,?,? WHERE EXISTS (SELECT 1 FROM dictations WHERE id=?)",
        params![id, final_text, cleanup_status, "retry", id],
    )?;
    c.execute(
        "UPDATE dictations SET status=?, degraded=?, degraded_reason=?, raw_text=COALESCE(?,raw_text), cleanup_status=COALESCE(?,cleanup_status), raw_audio_path=NULL, delivery_method='clipboard', fallback_reason='retry_clipboard_only' WHERE id=?",
        params![
            if degraded { "degraded" } else { "copied" },
            degraded as i32,
            degraded_reason,
            raw_text,
            cleanup_status,
            id
        ],
    )?;
    Ok(())
}

pub fn record_recovered_history(dir: &Path, recovery: &RecoveredSpool) -> anyhow::Result<()> {
    let c = open_history(dir)?;
    let already_recorded: i64 = c.query_row(
        "SELECT COUNT(*) FROM dictations WHERE raw_audio_path=?",
        [recovery.audio_path.to_string_lossy().as_ref()],
        |row| row.get(0),
    )?;
    if already_recorded > 0 {
        return Ok(());
    }
    drop(c);
    let context = crate::context::ContextSnapshot::general();
    insert_history_with_status(InsertHistory {
        dir,
        raw: "",
        final_text: "",
        cleanup_status: None,
        duration: recovery.duration_secs,
        degraded: true,
        degraded_reason: Some("interrupted_recording"),
        status: "failed".into(),
        delivery_method: Some("none"),
        fallback_reason: Some("crash_recovery"),
        spool: Some(&recovery.audio_path),
        count_asr: false,
        count_llm: false,
        context: Some(&context),
    })
}

pub fn remove_spool_artifact(dir: &Path, path: &Path) {
    let root = dir.join("spool");
    if !is_safe_spool_path(&root, path) {
        return;
    }
    if path.is_dir() {
        let _ = fs::remove_dir_all(path);
        return;
    }
    let _ = fs::remove_file(path);
    if let Some(parent) = path.parent() {
        if parent != root && parent.join("manifest.json").is_file() {
            let _ = fs::remove_dir_all(parent);
        }
    }
}
pub fn history_text(dir: &Path, id: i64) -> anyhow::Result<String> {
    let c = open_history(dir)?;
    Ok(c.query_row(
        "SELECT COALESCE(NULLIF((SELECT final_text FROM dictation_revisions WHERE dictation_id=dictations.id ORDER BY revision_id DESC LIMIT 1),''),NULLIF(final_text,''),raw_text,'') FROM dictations WHERE id=?",
        [id],
        |r| r.get(0),
    )?)
}

pub fn history_raw_text(dir: &Path, id: i64) -> anyhow::Result<String> {
    let c = open_history(dir)?;
    Ok(c.query_row(
        "SELECT COALESCE(raw_text,'') FROM dictations WHERE id=?",
        [id],
        |r| r.get(0),
    )?)
}

pub fn update_history_revision_state(
    dir: &Path,
    id: i64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    cleanup_status: Option<&str>,
) -> anyhow::Result<()> {
    let c = open_history(dir)?;
    c.execute(
        "UPDATE dictations SET status=?,degraded=?,degraded_reason=?,cleanup_status=COALESCE(?,cleanup_status),delivery_method='clipboard',fallback_reason=NULL WHERE id=?",
        params![status, degraded as i32, degraded_reason, cleanup_status, id],
    )?;
    if c.changes() == 0 {
        anyhow::bail!("history record was not found");
    }
    Ok(())
}
pub fn delete_history(dir: &Path, id: i64) -> anyhow::Result<()> {
    let c = open_history(dir)?;
    let spool: Option<String> = c
        .query_row(
            "SELECT raw_audio_path FROM dictations WHERE id=?",
            [id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    c.execute("DELETE FROM dictations WHERE id=?", [id])?;
    if let Some(spool) = spool {
        let spool_root = dir.join("spool");
        let path = Path::new(&spool);
        if is_safe_spool_path(&spool_root, path) {
            remove_spool_artifact(dir, path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "voiceflow-store-{name}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }
    #[test]
    fn usage_upsert_by_day() {
        let dir = temp_dir("usage");
        insert_history(&dir, "a", "A", 2.0, false).unwrap();
        insert_history(&dir, "b", "B", 3.0, true).unwrap();
        let q = crate::queue::RequestGate::new(None).snapshots();
        let u = get_usage(&dir, q).unwrap();
        assert_eq!(u.asr_requests, 2);
        assert_eq!(u.llm_requests, 2);
        assert_eq!(u.audio_seconds, 5.0);
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn old_settings_get_new_defaults() {
        let dir = temp_dir("settings");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"schema_version":8,"api_key":"test","selected_action_hotkey":"","selected_actions_enabled":false,"output_mode":"plain"}"#,
        )
        .unwrap();
        let (settings, _) = load_settings(&dir);
        assert!(!settings.onboarded);
        assert!(settings.cleanup_enabled);
        assert!(settings.show_tray_icon);
        assert_eq!(settings.cleanup_model, crate::llm::MODEL);
        assert_eq!(settings.activation_mode, "tap");
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.ui_language, "system");
        assert_eq!(settings.theme, "system");
        assert_eq!(settings.output_mode, "auto");
        assert_eq!(settings.translation_target_language, "en");
        assert_eq!(settings.long_output_mode, "paste");
        assert_eq!(settings.hotkey, "CmdOrControl+Alt+Space");
        assert_eq!(settings.selected_action_hotkey, "CmdOrControl+Alt+Slash");
        assert!(settings.selected_actions_enabled);
        assert!(settings.dictionary_learn_enabled);
        assert!(settings.input_device.is_empty());
        assert_eq!(settings.input_gain, 1.0);
        assert!(settings.asr_base_url.is_empty());
        assert!(settings.asr_api_key.is_empty());
        assert!(settings
            .writing_modes
            .iter()
            .any(|mode| mode.id == "general" && mode.builtin));
        let backup: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.join("settings.json.pre-migration.bak")).unwrap(),
        )
        .unwrap();
        assert_eq!(backup["api_key"], "");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn schema_14_replaces_legacy_shift_slash_defaults_and_option_divide() {
        let mut settings = Settings {
            schema_version: 13,
            hotkey: "CmdOrControl+Shift+Space".into(),
            selected_action_hotkey: "CmdOrControl+Alt+÷".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.hotkey, "CmdOrControl+Alt+Space");
        assert_eq!(settings.selected_action_hotkey, "CmdOrControl+Alt+Slash");

        let mut legacy_slash = Settings {
            schema_version: 13,
            selected_action_hotkey: "CmdOrControl+Shift+Slash".into(),
            ..Settings::default()
        };
        legacy_slash.normalize();
        assert_eq!(
            legacy_slash.selected_action_hotkey,
            "CmdOrControl+Alt+Slash"
        );

        let mut previous_default = Settings {
            schema_version: 14,
            selected_action_hotkey: "CmdOrControl+Alt+Backslash".into(),
            ..Settings::default()
        };
        previous_default.normalize();
        assert_eq!(
            previous_default.selected_action_hotkey,
            "CmdOrControl+Alt+Slash"
        );

        let mut custom = Settings {
            schema_version: 13,
            hotkey: "CmdOrControl+Shift+K".into(),
            selected_action_hotkey: "CmdOrControl+Alt+K".into(),
            ..Settings::default()
        };
        custom.normalize();
        assert_eq!(custom.hotkey, "CmdOrControl+Shift+K");
        assert_eq!(custom.selected_action_hotkey, "CmdOrControl+Alt+K");
    }

    #[test]
    fn hold_combo_hotkey_migrates_to_hybrid() {
        let mut settings = Settings {
            activation_mode: "hold".into(),
            hotkey: "CmdOrControl+Shift+Space".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.activation_mode, "hybrid");
        settings.validate().unwrap();
    }

    #[test]
    fn hold_modifier_hotkey_still_migrates_to_double_tap() {
        let mut settings = Settings {
            activation_mode: "hold".into(),
            hotkey: "Fn".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.activation_mode, "double_tap");
    }

    #[test]
    fn hybrid_combo_survives_normalize_and_validate() {
        let mut settings = Settings {
            activation_mode: "hybrid".into(),
            hotkey: "CmdOrControl+Shift+Space".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.activation_mode, "hybrid");
        settings.validate().unwrap();
    }

    #[test]
    fn hybrid_modifier_only_is_forced_to_double_tap() {
        let mut settings = Settings {
            activation_mode: "hybrid".into(),
            hotkey: "Command".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.activation_mode, "double_tap");
    }

    #[test]
    fn selected_action_migration_preserves_custom_shortcuts() {
        let mut settings = Settings {
            schema_version: 8,
            selected_action_hotkey: "CmdOrControl+Alt+K".into(),
            selected_actions_enabled: false,
            ..Settings::default()
        };

        settings.normalize();

        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.selected_action_hotkey, "CmdOrControl+Alt+K");
        assert!(!settings.selected_actions_enabled);
    }

    #[test]
    fn input_device_preference_is_trimmed_and_keeps_default_empty() {
        let mut settings = Settings {
            input_device: "  EarPods Microphone  ".into(),
            ..Settings::default()
        };

        settings.normalize();

        assert_eq!(settings.input_device, "EarPods Microphone");
        assert!(Settings::default().input_device.is_empty());
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn long_recording_defaults_to_auto_paste() {
        assert_eq!(Settings::default().long_output_mode, "paste");
        assert_eq!(Settings::default().delivery_policy, "auto");

        let mut settings = Settings {
            long_output_mode: "unsupported".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.long_output_mode, "paste");
        assert_eq!(settings.delivery_policy, "auto");
    }

    #[test]
    fn legacy_long_recording_default_migrates_to_auto_paste() {
        let mut settings = Settings {
            schema_version: 11,
            long_output_mode: "clipboard".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.long_output_mode, "paste");
        assert_eq!(settings.delivery_policy, "auto");

        let mut current_choice = Settings {
            schema_version: SETTINGS_SCHEMA_VERSION,
            long_output_mode: "clipboard".into(),
            ..Settings::default()
        };
        current_choice.normalize();
        assert_eq!(current_choice.long_output_mode, "clipboard");

        let mut explicit_delivery = Settings {
            delivery_policy: "clipboard_only".into(),
            ..Settings::default()
        };
        explicit_delivery.normalize();
        assert_eq!(explicit_delivery.delivery_policy, "clipboard_only");
    }

    #[test]
    fn invalid_ui_language_and_theme_fall_back_to_safe_defaults() {
        let mut settings = Settings {
            ui_language: "fr".into(),
            theme: "sepia".into(),
            ..Settings::default()
        };

        settings.normalize();

        assert_eq!(settings.ui_language, "system");
        assert_eq!(settings.theme, "system");
    }

    #[test]
    fn invalid_selected_action_hotkey_is_cleared_without_disabling_feature() {
        let mut settings = Settings {
            selected_action_hotkey: "Shift".into(),
            selected_actions_enabled: true,
            ..Settings::default()
        };
        assert!(settings.validate().is_err());
        settings.normalize();
        assert!(settings.selected_action_hotkey.is_empty());
        assert!(settings.selected_actions_enabled);
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn selected_actions_can_be_enabled_before_a_hotkey_is_assigned() {
        let settings = Settings {
            selected_actions_enabled: true,
            ..Settings::default()
        };

        assert!(settings.validate().is_ok());
    }

    #[test]
    fn unsupported_cleanup_model_falls_back_to_default() {
        let mut settings = Settings {
            cleanup_model: "retired/model".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.cleanup_model, crate::llm::MODEL);
        assert!(Settings {
            cleanup_model: "retired/model".into(),
            ..Settings::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn normalize_keeps_custom_asr_url_when_the_key_is_empty() {
        let mut settings = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            asr_api_key: String::new(),
            asr_model: "my-local-whisper".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.asr_provider, crate::engine::EngineProvider::Custom);
        assert_eq!(settings.asr_base_url, "http://127.0.0.1:8000/v1");
        assert_eq!(settings.asr_model, "my-local-whisper");
    }

    #[test]
    fn repair_incomplete_custom_asr_so_a_groq_key_can_be_saved() {
        let mut settings = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://relay.example.com/v1".into(),
            custom_base_url: "https://relay.example.com/v1".into(),
            asr_api_key: String::new(),
            asr_model: "whisper-1".into(),
            api_key: "gsk_test".into(),
            onboarded: false,
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.asr_provider, crate::engine::EngineProvider::Custom);
        assert!(settings.validate().is_err());
        assert!(settings.repair_incomplete_engine_sides());
        assert_eq!(settings.asr_provider, crate::engine::EngineProvider::Groq);
        assert!(settings.asr_base_url.is_empty());
        assert_eq!(settings.asr_model, crate::asr::MODEL);
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn load_settings_repairs_incomplete_custom_asr_left_on_disk() {
        let dir = temp_dir("incomplete-custom-asr");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"schema_version":16,"asr_provider":"custom","asr_base_url":"https://relay.example.com/v1","asr_model":"whisper-1"}"#,
        )
        .unwrap();
        let (settings, needs_persist) = load_settings(&dir);
        assert_eq!(settings.asr_provider, crate::engine::EngineProvider::Groq);
        assert!(settings.asr_base_url.is_empty());
        assert_eq!(settings.asr_model, crate::asr::MODEL);
        assert!(settings.validate().is_ok());
        assert!(needs_persist);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn schema_16_keeps_groq_urls_on_the_groq_provider() {
        let mut settings = Settings {
            schema_version: 15,
            asr_base_url: "https://api.groq.com/openai/v1".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.asr_provider, crate::engine::EngineProvider::Groq);
        assert!(settings.asr_base_url.is_empty());
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn schema_16_infers_custom_asr_from_a_saved_url() {
        let mut settings = Settings {
            schema_version: 15,
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            asr_api_key: "local-key".into(),
            asr_model: "my-local-whisper".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.asr_provider, crate::engine::EngineProvider::Custom);
        assert_eq!(settings.asr_model, "my-local-whisper");
        assert_eq!(settings.cleanup_provider, crate::engine::EngineProvider::Groq);
        assert!(settings.cleanup_base_url.is_empty());
    }

    #[test]
    fn groq_provider_clears_custom_url_and_resets_unknown_asr_model() {
        let mut settings = Settings {
            asr_provider: crate::engine::EngineProvider::Groq,
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            asr_model: "whisper-1".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert!(settings.asr_base_url.is_empty());
        assert_eq!(settings.asr_model, crate::asr::MODEL);
    }

    #[test]
    fn custom_cleanup_keeps_unknown_models_and_does_not_reuse_the_groq_key() {
        let settings = Settings {
            cleanup_provider: crate::engine::EngineProvider::Custom,
            cleanup_base_url: "https://api.openai.com/v1".into(),
            cleanup_model: "gpt-4o-mini".into(),
            api_key: "gsk_fallback".into(),
            cleanup_api_key: String::new(),
            ..Settings::default()
        };
        assert_eq!(settings.cleanup_credential(), "");
        assert_eq!(
            settings.validate().unwrap_err().to_string(),
            "自定义整理地址需要填写整理密钥。"
        );

        let mut kept = settings.clone();
        kept.cleanup_api_key = "sk-openai".into();
        kept.normalize();
        assert_eq!(kept.cleanup_model, "gpt-4o-mini");
        assert_eq!(kept.cleanup_request_model(), "gpt-4o-mini");
        assert_eq!(
            kept.cleanup_endpoint(),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(kept.cleanup_credential(), "sk-openai");
        assert!(kept.validate().is_ok());
    }

    #[test]
    fn history_schema_migration_keeps_a_versioned_backup() {
        let dir = temp_dir("history-migration");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE dictations (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, duration_secs REAL, raw_text TEXT, final_text TEXT, engine TEXT, degraded INTEGER, status TEXT, raw_audio_path TEXT); PRAGMA user_version=2;",
            )
            .unwrap();
        drop(connection);

        let _ = get_history(&dir, 10).unwrap();

        assert!(dir.join("history.sqlite.v2.bak").is_file());
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn save_settings_never_writes_plaintext_key() {
        let dir = temp_dir("keyblank");
        let settings = Settings {
            api_key: "test_key_should_not_be_on_disk".into(),
            ..Settings::default()
        };
        save_settings(&dir, &settings).unwrap();
        let on_disk = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            !on_disk.contains("test_key_should_not_be_on_disk"),
            "plaintext API key must never be written to settings.json"
        );
        let parsed: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(parsed["api_key"].as_str().unwrap_or(""), "");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn persist_secret_uses_sidecar_when_keychain_write_fails() {
        let dir = temp_dir("sidecar-write");
        persist_secret_to_keychain_or_sidecar(&dir, "api_key", "gsk_secret", |_| {
            Err("keychain write timed out".into())
        })
        .unwrap();
        let path = secret_sidecar_path(&dir, "api_key");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "gsk_secret");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn persist_secret_clears_sidecar_after_keychain_succeeds() {
        let dir = temp_dir("sidecar-clear");
        write_secret_sidecar(&dir, "api_key", "old").unwrap();
        persist_secret_to_keychain_or_sidecar(&dir, "api_key", "new", |_| Ok(())).unwrap();
        assert!(!secret_sidecar_path(&dir, "api_key").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn load_settings_reads_api_key_from_sidecar_when_keychain_is_empty() {
        let dir = temp_dir("sidecar-load");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"onboarded":true,"api_key":""}"#,
        )
        .unwrap();
        write_secret_sidecar(&dir, "api_key", "gsk_from_sidecar").unwrap();
        let (settings, _) = load_settings(&dir);
        assert_eq!(settings.api_key, "gsk_from_sidecar");
        assert!(settings.onboarded);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn credential_hint_shows_last_five_characters() {
        assert_eq!(
            credential_hint("gsk_abcdefghij").as_deref(),
            Some("••••fghij")
        );
        assert_eq!(credential_hint("only").as_deref(), Some("••••only"));
        assert_eq!(credential_hint("asr_only").as_deref(), Some("••••_only"));
    }

    #[test]
    fn settings_view_redacts_api_key() {
        let settings = Settings {
            api_key: "test_key_1234".into(),
            ..Settings::default()
        };
        let view = SettingsView::from(&settings);
        let json = serde_json::to_string(&view).unwrap();
        assert!(view.api_key_configured);
        assert_eq!(view.api_key_hint.as_deref(), Some("••••_1234"));
        assert_eq!(view.asr_model, crate::asr::MODEL);
        assert_eq!(view.cleanup_model, crate::llm::MODEL);
        assert!(!json.contains("test_key_1234"));
        assert!(!json.contains("secret"));
    }

    #[test]
    fn settings_view_exposes_configured_asr_model() {
        let settings = Settings {
            asr_model: "whisper-1".into(),
            ..Settings::default()
        };
        assert_eq!(SettingsView::from(&settings).asr_model, "whisper-1");
    }

    #[test]
    fn onboarding_requires_an_api_key() {
        let settings = Settings {
            onboarded: true,
            api_key: String::new(),
            ..Settings::default()
        };
        assert!(settings.validate().is_err());
    }

    #[test]
    fn missing_key_clears_stale_onboarded_state_for_persistence() {
        let dir = temp_dir("stale-onboarding");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"onboarded":true,"api_key":""}"#,
        )
        .unwrap();
        let (settings, needs_persist) = load_settings(&dir);
        assert!(!settings.onboarded);
        assert!(needs_persist);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn spool_writes_are_atomic_and_path_bounded() {
        let dir = temp_dir("spool");
        let path = write_spool_file(&dir, Path::new("session/chunk.wav"), b"audio").unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"audio");
        assert!(write_spool_file(&dir, Path::new("../escape.wav"), b"nope").is_err());
        assert!(!dir.join("escape.wav").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("spool/session/chunk.wav"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
            let dir_mode = std::fs::metadata(dir.join("spool/session"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn encrypted_spool_round_trip_and_plaintext_compatibility() {
        let key = [7_u8; 32];
        let plaintext = b"audio bytes";
        let encrypted = encrypt_spool_bytes(plaintext, &key).unwrap();
        assert_ne!(encrypted, plaintext);
        assert_eq!(decrypt_spool_bytes(&encrypted, &key).unwrap(), plaintext);

        let dir = temp_dir("plaintext-compatibility");
        let path = dir.join("legacy.wav");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, plaintext).unwrap();
        assert_eq!(read_spool_file(&path).unwrap(), plaintext);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn encrypted_spool_requires_a_key_for_write_and_read() {
        let dir = temp_dir("encrypted-missing-key");
        let result = write_spool_file_internal(
            &dir,
            Path::new("missing-key.wav"),
            b"secret audio",
            true,
            None,
        );
        assert!(result.is_err());
        assert!(!dir.join("spool/missing-key.wav").exists());

        let path = dir.join("encrypted.wav");
        let encrypted = encrypt_spool_bytes(b"secret audio", &[9_u8; 32]).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, encrypted).unwrap();
        assert!(read_spool_file(&path).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn recovery_wav_uses_the_encrypted_spool_envelope_when_enabled() {
        let dir = temp_dir("encrypted-recovery");
        let wav = b"RIFF recovery";
        write_spool_file_internal(
            &dir,
            Path::new("session/recovery.wav"),
            wav,
            true,
            Some(&[3_u8; 32]),
        )
        .unwrap();
        let on_disk = std::fs::read(dir.join("spool/session/recovery.wav")).unwrap();
        assert!(on_disk.starts_with(SPOOL_ENVELOPE_MAGIC));
        assert_ne!(on_disk.as_slice(), wav);
        assert_eq!(decrypt_spool_bytes(&on_disk, &[3_u8; 32]).unwrap(), wav);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn spool_manifest_tracks_chunks_and_recovers_active_sessions() {
        let dir = temp_dir("manifest");
        let session = begin_spool_session(&dir, "session-test").unwrap();
        record_spool_chunk(&session, 0, 0.0, 15.0, "written").unwrap();
        std::fs::create_dir_all(session.join("chunks")).unwrap();
        std::fs::write(session.join("chunks/00000000.f32"), 0.25_f32.to_le_bytes()).unwrap();
        let manifest: SpoolManifest =
            serde_json::from_slice(&std::fs::read(session.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest.status, "active");
        assert_eq!(manifest.chunks[0].status, "written");
        let recovered = recover_spool(&dir, 7).unwrap();
        assert_eq!(recovered.len(), 1);
        assert!(recovered[0].audio_path.is_file());
        assert!(!read_spool_file(&recovered[0].audio_path).unwrap().is_empty());
        let manifest: SpoolManifest =
            serde_json::from_slice(&std::fs::read(session.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest.status, "recoverable");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn recovered_history_is_idempotent_and_deletes_session_artifacts() {
        let dir = temp_dir("recovered-history");
        let session = begin_spool_session(&dir, "session-recovered").unwrap();
        std::fs::create_dir_all(session.join("chunks")).unwrap();
        std::fs::write(session.join("chunks/00000000.f32"), 0.25_f32.to_le_bytes()).unwrap();
        let recovery = recover_spool(&dir, 7).unwrap().remove(0);
        record_recovered_history(&dir, &recovery).unwrap();
        record_recovered_history(&dir, &recovery).unwrap();
        assert_eq!(get_history(&dir, 10).unwrap().len(), 1);
        let id = get_history(&dir, 10).unwrap()[0].id;
        delete_history(&dir, id).unwrap();
        assert!(!session.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn degraded_history_exposes_recovery_audio_for_retry() {
        let dir = temp_dir("degraded-retry");
        let session = begin_spool_session(&dir, "session-degraded-retry").unwrap();
        std::fs::create_dir_all(session.join("chunks")).unwrap();
        std::fs::write(session.join("chunks/00000000.f32"), 0.25_f32.to_le_bytes()).unwrap();
        mark_spool_status(&session, "degraded").unwrap();
        let recovery = rebuild_spool_recovery(&session).unwrap();
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw",
            "partial",
            recovery.duration_secs,
            true,
            Some("partial_asr_failure"),
            "degraded",
            "history",
            None,
            &context,
            Some(&recovery.audio_path),
        )
        .unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        assert_eq!(
            failed_spool(&dir, id).unwrap(),
            Some(recovery.audio_path.to_string_lossy().into())
        );
        delete_history(&dir, id).unwrap();
        assert!(!session.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleting_history_removes_a_degraded_spool_directory() {
        let dir = temp_dir("delete-spool-dir");
        let session = begin_spool_session(&dir, "session-degraded").unwrap();
        std::fs::write(session.join("chunk_0.wav"), b"audio").unwrap();
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw",
            "final",
            1.0,
            true,
            Some("partial_asr_failure"),
            "degraded",
            "history",
            None,
            &context,
            Some(&session),
        )
        .unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        delete_history(&dir, id).unwrap();
        assert!(!session.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_can_be_deleted() {
        let dir = temp_dir("delete");
        insert_history(&dir, "raw", "final", 1.0, false).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        delete_history(&dir, id).unwrap();
        assert!(get_history(&dir, 10).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_copy_falls_back_to_raw_text_when_final_text_is_empty() {
        let dir = temp_dir("history-copy-fallback");
        insert_history(&dir, "raw transcript", "", 1.0, true).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        assert_eq!(history_text(&dir, id).unwrap(), "raw transcript");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_keeps_raw_final_and_cleanup_status_separately() {
        let dir = temp_dir("history-cleanup-texts");
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool_and_cleanup(
            &dir,
            "spoken before cleanup",
            "cleaned after cleanup",
            1.0,
            false,
            None,
            "ok",
            "paste",
            None,
            &context,
            None,
            "ai_success",
        )
        .unwrap();

        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.raw_text, "spoken before cleanup");
        assert_eq!(item.final_text, "cleaned after cleanup");
        assert_eq!(item.cleanup_status, "ai_success");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_retryable_only_when_recovery_audio_exists() {
        let dir = temp_dir("history-retryable");
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw",
            "partial",
            1.0,
            true,
            Some("llm_cleanup_failed"),
            "degraded",
            "paste",
            None,
            &context,
            None,
        )
        .unwrap();
        assert!(!get_history(&dir, 10).unwrap()[0].retryable);

        let path = write_spool_file(&dir, Path::new("failed-short.wav"), b"audio").unwrap();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw",
            "partial",
            1.0,
            true,
            Some("llm_cleanup_failed"),
            "degraded",
            "paste",
            None,
            &context,
            Some(&path),
        )
        .unwrap();
        assert!(get_history(&dir, 1).unwrap()[0].retryable);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retry_updates_history_text() {
        let dir = temp_dir("retry");
        insert_failed_history(&dir, "raw", 1.0, None).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        mark_retried_with_texts(
            &dir,
            id,
            Some("retried raw"),
            "final",
            false,
            None,
            Some("ai_success"),
        )
        .unwrap();
        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.status, "copied");
        assert_eq!(item.raw_text, "retried raw");
        assert_eq!(item.final_text, "final");
        assert_eq!(item.cleanup_status, "ai_success");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retry_cleanup_failure_stays_degraded() {
        let dir = temp_dir("retry-degraded");
        insert_failed_history(&dir, "raw", 1.0, None).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        mark_retried(&dir, id, "raw", true, Some("llm_cleanup_failed")).unwrap();
        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.status, "degraded");
        assert!(item.degraded);
        assert_eq!(item.degraded_reason.as_deref(), Some("llm_cleanup_failed"));
        assert_eq!(item.delivery_method.as_deref(), Some("clipboard"));
        assert!(failed_spool(&dir, id).unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_revisions_keep_the_parent_and_latest_text() {
        let dir = temp_dir("revisions");
        insert_history(&dir, "raw", "first", 1.0, false).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        let intent =
            crate::llm::CleanupIntent::selected_text(crate::llm::CleanupOperation::Shorten, "raw");
        save_history_revision(
            &dir,
            id,
            "second",
            Some("ai_success"),
            Some(&intent),
            Some(crate::llm::MODEL),
            Some(&crate::context::ContextPolicy::default()),
            "ai_reclean",
        )
        .unwrap();
        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.final_text, "second");
        assert_eq!(item.revision_count, 1);
        assert_eq!(history_text(&dir, id).unwrap(), "second");
        assert_eq!(get_history_revisions(&dir, id).unwrap().len(), 1);
        delete_history(&dir, id).unwrap();
        assert!(get_history_revisions(&dir, id).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_reclean_state_does_not_create_a_fake_fallback_error() {
        let dir = temp_dir("revision-state");
        insert_history(&dir, "raw", "first", 1.0, false).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        update_history_revision_state(&dir, id, false, None, "copied", Some("ai_success")).unwrap();
        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.status, "copied");
        assert_eq!(item.fallback_reason, None);
        assert_eq!(item.cleanup_status, "ai_success");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dictionary_round_trips() {
        let mut settings = Settings::default();
        settings.dictionary.push("VoiceFlow".into());
        settings.dictionary.retain(|word| word != "missing");
        settings.dictionary.retain(|word| word != "VoiceFlow");
        assert!(settings.dictionary.is_empty());
    }

    #[test]
    fn default_chunk_settings_match_recommended_values() {
        let settings = Settings::default();
        assert_eq!(settings.chunk_threshold_secs, 25);
        assert_eq!(settings.chunk_length_secs, 35);
    }

    #[test]
    fn input_gain_defaults_to_one_and_clamps() {
        assert_eq!(Settings::default().input_gain, 1.0);
        let parsed: Settings = serde_json::from_str(r#"{"api_key":"x"}"#).unwrap();
        assert_eq!(parsed.input_gain, 1.0);
        assert_eq!(SettingsView::from(&parsed).input_gain, 1.0);
        let mut quiet = Settings {
            input_gain: 0.1,
            ..Settings::default()
        };
        quiet.normalize();
        assert_eq!(quiet.input_gain, 0.5);
        let mut loud = Settings {
            input_gain: 9.0,
            ..Settings::default()
        };
        loud.normalize();
        assert_eq!(loud.input_gain, 4.0);
        let mut invalid = Settings {
            input_gain: f32::NAN,
            ..Settings::default()
        };
        invalid.normalize();
        assert_eq!(invalid.input_gain, 1.0);
    }

    #[test]
    fn dictionary_learn_enabled_defaults_true() {
        assert!(Settings::default().dictionary_learn_enabled);
        let parsed: Settings = serde_json::from_str(r#"{"api_key":"x"}"#).unwrap();
        assert!(parsed.dictionary_learn_enabled);
        let view = SettingsView::from(&parsed);
        assert!(view.dictionary_learn_enabled);
    }

    #[test]
    fn asr_base_url_defaults_empty_and_falls_back_to_groq_key() {
        assert!(Settings::default().asr_base_url.is_empty());
        let parsed: Settings = serde_json::from_str(r#"{"api_key":"gsk_fallback"}"#).unwrap();
        assert!(parsed.asr_base_url.is_empty());
        assert!(parsed.asr_api_key.is_empty());
        assert_eq!(parsed.asr_credential(), "gsk_fallback");
        let groq_url = Settings {
            api_key: "gsk_fallback".into(),
            asr_base_url: "https://api.groq.com/openai/v1".into(),
            ..Settings::default()
        };
        assert_eq!(groq_url.asr_credential(), "gsk_fallback");
        let custom_without_asr_key = Settings {
            api_key: "gsk_fallback".into(),
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            custom_base_url: "http://127.0.0.1:8000/v1".into(),
            ..Settings::default()
        };
        assert_eq!(custom_without_asr_key.asr_credential(), "");
        let with_asr_key = Settings {
            api_key: "gsk_fallback".into(),
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_api_key: "asr_only".into(),
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            custom_base_url: "http://127.0.0.1:8000/v1".into(),
            ..Settings::default()
        };
        assert_eq!(with_asr_key.asr_credential(), "asr_only");
        let view = SettingsView::from(&with_asr_key);
        assert_eq!(view.asr_base_url, "http://127.0.0.1:8000/v1");
        assert!(view.asr_api_key_configured);
        assert_eq!(view.asr_api_key_hint.as_deref(), Some("••••_only"));
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("asr_only"));
        assert!(!json.contains("gsk_fallback"));
    }

    #[test]
    fn asr_base_url_validate_rejects_bad_scheme_plaintext_and_keyless_custom() {
        assert!(Settings::default().validate().is_ok());
        let groq = Settings {
            asr_base_url: "https://api.groq.com/openai/v1".into(),
            ..Settings::default()
        };
        assert!(groq.validate().is_ok());
        let loopback_with_key = Settings {
            asr_api_key: "local-key".into(),
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            ..Settings::default()
        };
        assert!(loopback_with_key.validate().is_ok());
        let relative = Settings {
            asr_base_url: "asr.example.com/v1".into(),
            asr_api_key: "k".into(),
            ..Settings::default()
        };
        assert!(relative
            .validate()
            .unwrap_err()
            .to_string()
            .contains("http:// 或 https://"));
        let plaintext = Settings {
            asr_base_url: "http://asr.example.com/v1".into(),
            asr_api_key: "k".into(),
            ..Settings::default()
        };
        assert!(plaintext
            .validate()
            .unwrap_err()
            .to_string()
            .contains("https://"));
        let custom_without_key = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://asr.example.com/v1".into(),
            custom_base_url: "https://asr.example.com/v1".into(),
            ..Settings::default()
        };
        assert_eq!(
            custom_without_key.validate().unwrap_err().to_string(),
            "自定义 ASR 地址需要填写 ASR 密钥。"
        );
        let loopback_without_key = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            custom_base_url: "http://127.0.0.1:8000/v1".into(),
            ..Settings::default()
        };
        assert!(loopback_without_key.validate().is_ok());
    }

    #[test]
    fn schema_17_promotes_openai_cleanup_host_into_the_provider_pool() {
        let mut settings = Settings {
            schema_version: 16,
            api_key: "gsk_keep".into(),
            asr_provider: crate::engine::EngineProvider::Groq,
            cleanup_provider: crate::engine::EngineProvider::Custom,
            cleanup_base_url: "https://api.openai.com/v1".into(),
            cleanup_api_key: "sk-openai".into(),
            cleanup_model: "gpt-4o-mini".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(
            settings.cleanup_provider,
            crate::engine::EngineProvider::OpenAi
        );
        assert_eq!(
            settings.provider_api_keys.get("openai").map(String::as_str),
            Some("sk-openai")
        );
        assert_eq!(
            settings.provider_api_keys.get("groq").map(String::as_str),
            Some("gsk_keep")
        );
        assert!(settings.cleanup_base_url.is_empty());
        assert_eq!(settings.cleanup_model, "gpt-4o-mini");
        assert_eq!(settings.cleanup_credential(), "sk-openai");
        assert_eq!(settings.asr_credential(), "gsk_keep");
    }

    #[test]
    fn named_asr_does_not_reuse_the_groq_key() {
        let settings = Settings {
            api_key: "gsk_keep".into(),
            asr_provider: crate::engine::EngineProvider::OpenAi,
            ..Settings::default()
        };
        assert!(settings.asr_credential().is_empty());
        assert_eq!(settings.provider_secret(crate::engine::EngineProvider::Groq), "gsk_keep");
    }

    #[test]
    fn bind_cleanup_key_clears_on_host_change_unless_a_new_key_is_supplied() {
        assert_eq!(
            bind_cleanup_key_to_host(
                "https://api.groq.com/openai/v1",
                "https://api.openai.com/v1",
                "",
                "old-key"
            ),
            ""
        );
        assert_eq!(
            bind_cleanup_key_to_host(
                "https://api.openai.com/v1",
                "https://api.openai.com/v1",
                "",
                "old-key"
            ),
            "old-key"
        );
    }

    #[test]
    fn bind_asr_key_clears_on_host_change_unless_a_new_key_is_supplied() {
        assert_eq!(
            bind_asr_key_to_host(
                "https://api.groq.com/openai/v1",
                "https://asr.example.com/v1",
                "",
                "old-key"
            ),
            ""
        );
        assert_eq!(
            bind_asr_key_to_host(
                "https://api.groq.com/openai/v1",
                "https://asr.example.com/v1",
                "new-key",
                "old-key"
            ),
            "new-key"
        );
        assert_eq!(
            bind_asr_key_to_host(
                "",
                "https://api.groq.com/openai/v1",
                "",
                "old-key"
            ),
            "old-key"
        );
        assert_eq!(
            bind_asr_key_to_host(
                "https://asr.example.com/v1",
                "https://asr.example.com/v1",
                "",
                "old-key"
            ),
            "old-key"
        );
    }

    #[test]
    fn save_settings_never_writes_plaintext_asr_key() {
        let dir = temp_dir("asr-keyblank");
        let settings = Settings {
            asr_api_key: "asr_secret_should_not_be_on_disk".into(),
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            ..Settings::default()
        };
        save_settings(&dir, &settings).unwrap();
        let on_disk = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            !on_disk.contains("asr_secret_should_not_be_on_disk"),
            "plaintext ASR API key must never be written to settings.json"
        );
        let parsed: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(parsed["asr_api_key"].as_str().unwrap_or(""), "");
        assert_eq!(
            parsed["asr_base_url"].as_str().unwrap_or(""),
            "http://127.0.0.1:8000/v1"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_settings_never_writes_plaintext_cleanup_key() {
        let dir = temp_dir("cleanup-keyblank");
        let settings = Settings {
            cleanup_provider: crate::engine::EngineProvider::Custom,
            cleanup_base_url: "https://api.openai.com/v1".into(),
            cleanup_model: "gpt-4o-mini".into(),
            cleanup_api_key: "cleanup_secret_should_not_be_on_disk".into(),
            ..Settings::default()
        };
        save_settings(&dir, &settings).unwrap();
        let on_disk = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            !on_disk.contains("cleanup_secret_should_not_be_on_disk"),
            "plaintext cleanup API key must never be written to settings.json"
        );
        let parsed: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(parsed["cleanup_api_key"].as_str().unwrap_or(""), "");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_page_uses_a_cursor_and_reports_more_rows() {
        let dir = temp_dir("history-page");
        insert_history(&dir, "one", "one", 1.0, false).unwrap();
        insert_history(&dir, "two", "two", 1.0, false).unwrap();
        insert_history(&dir, "three", "three", 1.0, false).unwrap();

        let first = get_history_page(&dir, 2, None, None).unwrap();
        assert_eq!(first.items.len(), 2);
        assert!(first.has_more);
        let before_id = first.items.last().unwrap().id;

        let second = get_history_page(&dir, 2, Some(before_id), None).unwrap();
        assert_eq!(second.items.len(), 1);
        assert!(!second.has_more);
        assert!(second.items[0].id < before_id);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_page_filters_both_text_columns_before_cursor_pagination() {
        let dir = temp_dir("history-search");
        insert_history(&dir, "first raw", "VoiceFlow first", 1.0, false).unwrap();
        insert_history(&dir, "unrelated", "other", 1.0, false).unwrap();
        insert_history(&dir, "VoiceFlow second", "cleaned", 1.0, false).unwrap();

        let first = get_history_page(&dir, 1, None, Some("VoiceFlow")).unwrap();
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.items[0].raw_text, "VoiceFlow second");
        assert!(first.has_more);

        let second = get_history_page(
            &dir,
            1,
            Some(first.items[0].id),
            Some("VoiceFlow"),
        )
        .unwrap();
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.items[0].final_text, "VoiceFlow first");
        assert!(!second.has_more);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_search_escapes_like_wildcards() {
        let dir = temp_dir("history-search-escape");
        insert_history(&dir, "plain", "plain", 1.0, false).unwrap();
        insert_history(&dir, "100% complete", "done", 1.0, false).unwrap();
        insert_history(&dir, "under_score", "other", 1.0, false).unwrap();
        insert_history(&dir, r"path\to\file", "slash", 1.0, false).unwrap();

        let percent = get_history_page(&dir, 10, None, Some("%")).unwrap();
        assert_eq!(percent.items.len(), 1);
        assert_eq!(percent.items[0].raw_text, "100% complete");

        let underscore = get_history_page(&dir, 10, None, Some("_")).unwrap();
        assert_eq!(underscore.items.len(), 1);
        assert_eq!(underscore.items[0].raw_text, "under_score");

        let slash = get_history_page(&dir, 10, None, Some("\\")).unwrap();
        assert_eq!(slash.items.len(), 1);
        assert_eq!(slash.items[0].raw_text, r"path\to\file");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_retention_removes_old_text_and_audio_reference() {
        let dir = temp_dir("history-retention");
        insert_history(&dir, "old", "old", 1.0, false).unwrap();
        insert_history(&dir, "new", "new", 1.0, false).unwrap();
        let connection = open_history(&dir).unwrap();
        connection
            .execute(
                "UPDATE dictations SET created_at=datetime('now','-400 days') WHERE raw_text='old'",
                [],
            )
            .unwrap();
        drop(connection);

        assert_eq!(purge_history(&dir, 365).unwrap(), 1);
        let page = get_history_page(&dir, 10, None, None).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].raw_text, "new");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_retention_zero_keeps_history_forever() {
        let dir = temp_dir("history-retention-forever");
        insert_history(&dir, "old", "old", 1.0, false).unwrap();
        let connection = open_history(&dir).unwrap();
        connection
            .execute(
                "UPDATE dictations SET created_at=datetime('now','-4000 days')",
                [],
            )
            .unwrap();
        drop(connection);

        assert_eq!(purge_history(&dir, 0).unwrap(), 0);
        let page = get_history_page(&dir, 10, None, None).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].raw_text, "old");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clear_all_data_removes_history_usage_and_spool() {
        let dir = temp_dir("clear-all-data");
        insert_history(&dir, "raw", "final", 2.0, false).unwrap();
        write_spool_file(&dir, Path::new("recovery.wav"), b"audio").unwrap();

        clear_all_data(&dir).unwrap();

        assert!(get_history_page(&dir, 10, None, None)
            .unwrap()
            .items
            .is_empty());
        assert_eq!(
            get_usage(&dir, crate::queue::RequestGate::new(None).snapshots())
                .unwrap()
                .asr_requests,
            0
        );
        assert!(dir.join("spool").is_dir());
        assert!(std::fs::read_dir(dir.join("spool"))
            .unwrap()
            .next()
            .is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("spool"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_settings_are_kept_untouched() {
        let dir = temp_dir("malformed-settings");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let raw = b"{not valid json";
        std::fs::write(&path, raw).unwrap();

        let (settings, needs_persist) = load_settings(&dir);

        assert_eq!(std::fs::read(&path).unwrap(), raw);
        assert!(!needs_persist);
        assert!(!settings.onboarded);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learn_pair_upsert_counts_only_and_leaves_dictionary_empty() {
        let dir = temp_dir("learn-pairs-count");
        std::fs::create_dir_all(&dir).unwrap();
        save_settings(
            &dir,
            &Settings {
                dictionary: vec![],
                ..Settings::default()
            },
        )
        .unwrap();
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        assert_eq!(upsert_learn_pair(&dir, &key, "知呼", "知乎").unwrap().unwrap().hits, 1);
        upsert_learn_pair(&dir, &key, "知呼", "知乎").unwrap();
        let third = upsert_learn_pair(&dir, &key, "知呼", "知乎").unwrap().unwrap();
        assert_eq!(third.hits, 3);
        assert!(!third.promoted);
        assert!(load_settings(&dir).0.dictionary.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clear_all_data_deletes_learn_pairs() {
        let dir = temp_dir("learn-pairs-clear");
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        upsert_learn_pair(&dir, &key, "知呼", "知乎").unwrap();
        clear_all_data(&dir).unwrap();
        assert!(list_learn_pairs(&dir).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learn_pair_concurrent_increments_are_not_lost() {
        let dir = temp_dir("learn-pairs-race");
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let dir = &dir;
                let key = &key;
                scope.spawn(move || {
                    upsert_learn_pair(dir, key, "知呼", "知乎").unwrap();
                });
            }
        });
        assert_eq!(
            get_learn_pair(&dir, &key).unwrap().unwrap().hits,
            8
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learn_pair_pending_cap_rejects_new_pairs() {
        let dir = temp_dir("learn-pairs-cap");
        for index in 0..256 {
            let before = format!("b{index}");
            let after = format!("a{index}");
            let key = crate::dictionary_learn::pair_key(&before, &after);
            assert!(upsert_learn_pair(&dir, &key, &before, &after)
                .unwrap()
                .is_some());
        }
        let extra = crate::dictionary_learn::pair_key("知呼", "知乎");
        assert!(upsert_learn_pair(&dir, &extra, "知呼", "知乎")
            .unwrap()
            .is_none());
        let first = crate::dictionary_learn::pair_key("b0", "a0");
        assert_eq!(
            upsert_learn_pair(&dir, &first, "b0", "a0")
                .unwrap()
                .unwrap()
                .hits,
            2
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ignore_writes_tombstone_and_blocks_upsert() {
        let dir = temp_dir("learn-pairs-tombstone");
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        upsert_learn_pair(&dir, &key, "知呼", "知乎").unwrap();
        assert!(tombstone_learn_pair(&dir, &key).unwrap());
        let row = get_learn_pair(&dir, &key).unwrap().unwrap();
        assert!(row.ignored);
        assert!(row.tombstoned_at.is_some());
        assert!(!row.is_pending());
        assert!(upsert_learn_pair(&dir, &key, "知呼", "知乎").unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clear_all_data_deletes_style_drafts() {
        let dir = temp_dir("style-drafts-clear");
        upsert_style_draft(&dir, "wechat", "fewer_periods", "你好。", "你好。", "你好").unwrap();
        assert!(!list_style_drafts(&dir).unwrap().is_empty());
        clear_all_data(&dir).unwrap();
        assert!(list_style_drafts(&dir).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learn_pair_schema_adds_scope_columns_on_existing_db() {
        let dir = temp_dir("learn-pairs-alter");
        std::fs::create_dir_all(&dir).unwrap();
        let connection = rusqlite::Connection::open(dir.join("history.sqlite")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE learn_pairs (
                    pair_key TEXT PRIMARY KEY,
                    before_surface TEXT NOT NULL,
                    after_surface TEXT NOT NULL,
                    hits INTEGER NOT NULL,
                    promoted INTEGER NOT NULL DEFAULT 0,
                    last_at TEXT NOT NULL
                );
                PRAGMA user_version = 6;",
            )
            .unwrap();
        drop(connection);
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        let row = upsert_learn_pair_with_scope(
            &dir,
            &key,
            "知呼",
            "知乎",
            Some(&LearnPairScope {
                family: Some("personal_chat".into()),
                mapping_id: Some("wechat".into()),
                browser_host: None,
                native_bundle: Some("com.tencent.xinWeChat".into()),
            }),
        )
        .unwrap()
        .unwrap();
        assert_eq!(row.family.as_deref(), Some("personal_chat"));
        assert_eq!(row.mapping_id.as_deref(), Some("wechat"));
        assert!(!row.pinned);
        assert!(!row.ignored);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_scene_and_confirm_keep_wechat_snapshot_scope() {
        let dir = temp_dir("history-wechat-scope");
        let mut snapshot = crate::context::ContextSnapshot::general();
        snapshot.profile.id = "chat.personal".into();
        snapshot.profile.family = crate::context::ContextFamily::PersonalChat;
        snapshot.target_guard.bundle_id = Some("com.tencent.xinWeChat".into());
        snapshot.target_guard.browser_host = None;
        insert_history_with_context(&dir, "知呼", "知乎", 1.0, false, &snapshot).unwrap();
        let id = get_history_page(&dir, 1, None, None).unwrap().items[0].id;
        let scene = history_scene(&dir, id).unwrap();
        assert_eq!(scene.profile_id.as_deref(), Some("chat.personal"));
        assert_eq!(scene.family.as_deref(), Some("personal_chat"));
        assert_eq!(scene.native_bundle.as_deref(), Some("com.tencent.xinWeChat"));
        assert!(history_context(&dir, id).unwrap().is_some());
        let key = crate::dictionary_learn::pair_key("派森", "Python");
        ensure_learn_pair_promoted(
            &dir,
            &key,
            "派森",
            "Python",
            Some(&scene.learn_scope()),
        )
        .unwrap();
        let row = get_learn_pair(&dir, &key).unwrap().unwrap();
        assert!(row.is_live_promoted());
        assert_eq!(row.family.as_deref(), Some("personal_chat"));
        assert_eq!(row.native_bundle.as_deref(), Some("com.tencent.xinWeChat"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unsupported_spool_manifest_is_removed_before_it_consumes_quota() {
        let dir = temp_dir("unsupported-spool");
        let session = dir.join("spool").join("unsupported");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(
            session.join("manifest.json"),
            r#"{"schema_version":999,"session_id":"unsupported","created_at_ms":0,"status":"active","chunks":[]}"#,
        )
        .unwrap();

        assert!(recover_spool(&dir, 7).unwrap().is_empty());
        assert!(!session.exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
