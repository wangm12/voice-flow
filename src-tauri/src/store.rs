#[cfg(test)]
use crate::queue::QuotaView;
use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
    XChaCha20Poly1305, XNonce,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const SETTINGS_SCHEMA_VERSION: u32 = 25;
const HISTORY_SCHEMA_VERSION: i32 = 13;
// Migration rollback copies contain historical text, so they have a finite
// lifetime even when the user elects to keep the live history forever.
const HISTORY_MIGRATION_BACKUP_MAX_DAYS: u64 = 7;
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
    #[serde(default = "default_legacy_activation_mode")]
    pub activation_mode: String,
    pub chunk_threshold_secs: u64,
    pub chunk_length_secs: usize,
    #[serde(default = "default_long_output_mode")]
    pub long_output_mode: String,
    #[serde(default = "default_delivery_policy")]
    pub delivery_policy: String,
    pub keep_audio_days: u64,
    pub keep_history_days: u64,
    #[serde(default)]
    pub keep_success_audio: bool,
    pub onboarded: bool,
    pub cleanup_enabled: bool,
    #[serde(default = "default_cleanup_intensity")]
    pub cleanup_intensity: String,
    #[serde(default)]
    pub accurate_asr_provider: crate::engine::EngineProvider,
    #[serde(default)]
    pub accurate_asr_model: String,
    #[serde(default)]
    pub accurate_asr_base_url: String,
    #[serde(default = "default_cascade_timeout_ms")]
    pub cascade_timeout_ms: u64,
    #[serde(default = "default_cascade_proper_noun_threshold")]
    pub cascade_proper_noun_threshold: usize,
    #[serde(default)]
    pub window_ocr_enabled: bool,
    #[serde(default)]
    pub screen_action_hotkey: String,
    #[serde(default)]
    pub vision_provider: String,
    #[serde(default)]
    pub vision_model: String,
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
    pub strict_offline_enabled: bool,
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
    #[serde(default)]
    pub verbatim_hotkey: String,
    #[serde(default)]
    pub translation_hotkey: String,
    #[serde(default = "default_extra_recording_buffer_ms")]
    pub extra_recording_buffer_ms: u64,
    #[serde(default)]
    pub audio_feedback_enabled: bool,
    #[serde(default = "default_audio_feedback_volume")]
    pub audio_feedback_volume: f32,
    #[serde(default)]
    pub vad_enabled: bool,
    #[serde(default)]
    pub always_on_microphone: bool,
    #[serde(default)]
    pub clamshell_microphone: String,
    #[serde(default)]
    pub autostart_enabled: bool,
    #[serde(default = "default_whats_new_last_seen_version")]
    pub whats_new_last_seen_version: String,
    #[serde(default)]
    pub debug_mode: bool,
    #[serde(default)]
    pub fuzzy_dictionary_enabled: bool,
    /// Legacy credential values that could not yet be verified in the OS
    /// credential store. These stay in memory only to preserve compatibility;
    /// save_settings keeps their source data intact until migration succeeds.
    #[serde(skip)]
    pub(crate) unverified_credential_sources: std::collections::BTreeMap<String, String>,
    /// Secure values observed while loading, used to distinguish ordinary
    /// settings saves from an explicit key replacement.
    #[serde(skip)]
    pub(crate) credential_baselines: std::collections::BTreeMap<String, String>,
}

fn default_cleanup_intensity() -> String {
    "auto".into()
}

fn default_cascade_timeout_ms() -> u64 {
    5000
}

fn default_cascade_proper_noun_threshold() -> usize {
    3
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

fn default_legacy_activation_mode() -> String {
    "tap".into()
}

fn default_extra_recording_buffer_ms() -> u64 {
    250
}

fn default_audio_feedback_volume() -> f32 {
    0.6
}

fn default_whats_new_last_seen_version() -> String {
    env!("CARGO_PKG_VERSION").into()
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
            keep_success_audio: false,
            onboarded: false,
            cleanup_enabled: true,
            cleanup_intensity: default_cleanup_intensity(),
            accurate_asr_provider: crate::engine::EngineProvider::Groq,
            accurate_asr_model: String::new(),
            accurate_asr_base_url: String::new(),
            cascade_timeout_ms: default_cascade_timeout_ms(),
            cascade_proper_noun_threshold: default_cascade_proper_noun_threshold(),
            window_ocr_enabled: false,
            screen_action_hotkey: String::new(),
            vision_provider: String::new(),
            vision_model: String::new(),
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
            strict_offline_enabled: false,
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
            verbatim_hotkey: String::new(),
            translation_hotkey: String::new(),
            extra_recording_buffer_ms: 250,
            audio_feedback_enabled: false,
            audio_feedback_volume: 0.6,
            vad_enabled: false,
            always_on_microphone: false,
            clamshell_microphone: String::new(),
            autostart_enabled: false,
            whats_new_last_seen_version: default_whats_new_last_seen_version(),
            debug_mode: false,
            fuzzy_dictionary_enabled: false,
            unverified_credential_sources: std::collections::BTreeMap::new(),
            credential_baselines: std::collections::BTreeMap::new(),
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
            if self.schema_version < 21 && self.window_ocr_enabled {
                // Before per-App OCR grants existed, enabling the global OCR
                // switch together with an App mapping was the user's explicit
                // local OCR opt-in. Preserve that choice for those mappings;
                // no AX text, provider text, or cloud vision grant is inferred.
                for mapping in &mut self.context_mappings {
                    mapping.source_permissions.local_ocr = true;
                }
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
        self.screen_action_hotkey = crate::hotkey::canonicalize_hotkey(&self.screen_action_hotkey);
        // Legacy hold meant the hybrid gesture, not pure hold-to-talk. All
        // retired gestures migrate to tap; an explicit v25 hold choice survives.
        if !matches!(self.activation_mode.as_str(), "tap" | "hold_to_talk") {
            self.activation_mode = "tap".into();
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
            if let Some(path) = &mapping.browser_path_prefix {
                mapping.browser_path_prefix = crate::context::normalize_path_prefix(path);
            }
            mapping.style_example_input = mapping.style_example_input.take().and_then(|value| {
                let trimmed: String = value.trim().chars().take(2_000).collect();
                (!trimmed.is_empty()).then_some(trimmed)
            });
            mapping.style_example_output = mapping.style_example_output.take().and_then(|value| {
                let trimmed: String = value.trim().chars().take(2_000).collect();
                (!trimmed.is_empty()).then_some(trimmed)
            });
            if mapping.style_example_pairs.is_empty() {
                if let (Some(input), Some(output)) = (
                    mapping.style_example_input.clone(),
                    mapping.style_example_output.clone(),
                ) {
                    mapping
                        .style_example_pairs
                        .push(crate::context::StyleExamplePair { input, output });
                }
            } else {
                mapping.style_example_pairs.truncate(3);
                for pair in &mut mapping.style_example_pairs {
                    pair.input = pair.input.trim().chars().take(2_000).collect();
                    pair.output = pair.output.trim().chars().take(2_000).collect();
                }
                mapping
                    .style_example_pairs
                    .retain(|pair| !pair.input.is_empty() && !pair.output.is_empty());
                if let Some(first) = mapping.style_example_pairs.first() {
                    mapping.style_example_input = Some(first.input.clone());
                    mapping.style_example_output = Some(first.output.clone());
                }
            }
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
        if self.screen_action_hotkey.len() > 128 {
            self.screen_action_hotkey.clear();
        }
        self.vision_provider = self.vision_provider.trim().chars().take(64).collect();
        if !self.vision_provider.is_empty()
            && crate::engine::EngineProvider::parse(&self.vision_provider).is_none()
        {
            self.vision_provider.clear();
        }
        self.vision_model = self.vision_model.trim().chars().take(256).collect();
        self.input_device = self.input_device.trim().chars().take(512).collect();
        self.input_gain = if self.input_gain.is_finite() {
            self.input_gain.clamp(0.5, 4.0)
        } else {
            default_input_gain()
        };
        if self.verbatim_hotkey.len() > 128 {
            self.verbatim_hotkey.clear();
        }
        self.verbatim_hotkey = crate::hotkey::canonicalize_hotkey(&self.verbatim_hotkey);
        if self.translation_hotkey.len() > 128 {
            self.translation_hotkey.clear();
        }
        self.translation_hotkey = crate::hotkey::canonicalize_hotkey(&self.translation_hotkey);
        self.extra_recording_buffer_ms = self.extra_recording_buffer_ms.clamp(0, 2000);
        self.audio_feedback_volume = if self.audio_feedback_volume.is_finite() {
            self.audio_feedback_volume.clamp(0.0, 1.0)
        } else {
            default_audio_feedback_volume()
        };
        self.clamshell_microphone = self.clamshell_microphone.trim().chars().take(512).collect();
        self.whats_new_last_seen_version = self
            .whats_new_last_seen_version
            .trim()
            .chars()
            .take(64)
            .collect();
        if self.whats_new_last_seen_version.is_empty() {
            self.whats_new_last_seen_version = env!("CARGO_PKG_VERSION").into();
        }
        if crate::llm::CleanupIntensity::parse(&self.cleanup_intensity).is_none() {
            self.cleanup_intensity = default_cleanup_intensity();
        }
        if self.cascade_timeout_ms == 0 {
            self.cascade_timeout_ms = default_cascade_timeout_ms();
        }
        if self.cascade_proper_noun_threshold == 0 {
            self.cascade_proper_noun_threshold = default_cascade_proper_noun_threshold();
        }
        self.accurate_asr_model = self.accurate_asr_model.trim().chars().take(256).collect();
        self.accurate_asr_base_url = self
            .accurate_asr_base_url
            .trim()
            .chars()
            .take(2_048)
            .collect();
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
        } else if self.asr_provider == crate::engine::EngineProvider::Soniox {
            self.asr_model = crate::asr::SONIOX_MODEL.to_owned();
        } else if matches!(
            self.asr_provider,
            crate::engine::EngineProvider::AssemblyAi | crate::engine::EngineProvider::DashScope
        ) || self.asr_model.trim().is_empty()
        {
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
            if let Some(named) = crate::providers::infer_provider_from_host(&self.cleanup_base_url)
            {
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

    #[cfg(test)]
    pub fn validate(&self) -> anyhow::Result<()> {
        self.validate_with_models_root(None)
    }

    pub fn validate_with_models_root(&self, models_root: Option<&Path>) -> anyhow::Result<()> {
        self.validate_internal(models_root, true)
    }

    /// Structural validation for a binding transaction or schema migration.
    /// Service edits, onboarding completion and recording still check readiness.
    pub fn validate_configuration(&self) -> anyhow::Result<()> {
        self.validate_internal(None, false)
    }

    fn validate_internal(
        &self,
        models_root: Option<&Path>,
        check_readiness: bool,
    ) -> anyhow::Result<()> {
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
        if crate::llm::CleanupIntensity::parse(&self.cleanup_intensity).is_none() {
            anyhow::bail!("unsupported cleanup intensity");
        }
        if self.cleanup_provider.is_groq() {
            if !crate::llm::is_supported_model(&self.cleanup_model) {
                anyhow::bail!("unsupported cleanup model");
            }
        } else if self.cleanup_model.trim().is_empty() {
            anyhow::bail!("自定义整理需要填写模型名。");
        }
        self.validate_bindings()?;
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
        self.validate_provider_side(self.asr_provider, true, models_root, check_readiness)?;
        if self.cleanup_enabled && self.asr_provider != crate::engine::EngineProvider::AssemblyAi {
            self.validate_provider_side(
                self.cleanup_provider,
                false,
                models_root,
                check_readiness,
            )?;
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

        if self.screen_action_hotkey.len() > 128 {
            anyhow::bail!("look-at-screen hotkey is too long");
        }

        if self.verbatim_hotkey.len() > 128 {
            anyhow::bail!("verbatim hotkey is too long");
        }
        if !self.vision_provider.is_empty()
            && crate::engine::EngineProvider::parse(&self.vision_provider).is_none()
        {
            anyhow::bail!("unsupported vision provider");
        }
        if self.vision_model.len() > 256 {
            anyhow::bail!("vision model name is too long");
        }
        if self.input_device.len() > 512 {
            anyhow::bail!("input device name is too long");
        }
        if !self.input_gain.is_finite() || !(0.5..=4.0).contains(&self.input_gain) {
            anyhow::bail!("input gain must be between 0.5 and 4.0");
        }
        Ok(())
    }

    pub fn validate_bindings(&self) -> anyhow::Result<()> {
        if !matches!(self.activation_mode.as_str(), "tap" | "hold_to_talk") {
            anyhow::bail!("unsupported recording mode");
        }
        let bindings = [
            &self.hotkey,
            &self.selected_action_hotkey,
            &self.screen_action_hotkey,
            &self.verbatim_hotkey,
            &self.translation_hotkey,
        ];
        if self.hotkey.trim().is_empty() {
            anyhow::bail!("hotkey is required");
        }
        for (index, binding) in bindings.iter().enumerate() {
            if binding.len() > 128 {
                anyhow::bail!("hotkey must contain at most 128 characters");
            }
            if binding.trim().is_empty() {
                continue;
            }
            // Preserve deprecated single modifiers so users can see and replace
            // their old bindings. New bindings are checked at the transaction boundary.
            if !crate::modifier_hotkey::is_modifier_only(binding) {
                binding
                    .parse::<tauri_plugin_global_shortcut::Shortcut>()
                    .map_err(|_| anyhow::anyhow!("invalid hotkey: {binding}"))?;
            }
            for other in bindings
                .iter()
                .skip(index + 1)
                .filter(|key| !key.trim().is_empty())
            {
                if crate::hotkey::bindings_equal(binding, other) {
                    anyhow::bail!("hotkey conflicts with another shortcut");
                }
            }
        }
        Ok(())
    }

    pub fn validate_binding_changes(&self, previous: &Self) -> anyhow::Result<()> {
        self.validate_bindings()?;
        for (new, old, recording) in [
            (&self.hotkey, &previous.hotkey, true),
            (&self.verbatim_hotkey, &previous.verbatim_hotkey, true),
            (&self.translation_hotkey, &previous.translation_hotkey, true),
            (
                &self.selected_action_hotkey,
                &previous.selected_action_hotkey,
                false,
            ),
            (
                &self.screen_action_hotkey,
                &previous.screen_action_hotkey,
                false,
            ),
        ] {
            if new != old
                && crate::modifier_hotkey::is_modifier_only(new)
                && !(recording && crate::modifier_hotkey::is_fn_only(new))
            {
                anyhow::bail!(if recording {
                    "请再按一个键组成快捷键；单键录音可选择 Fn。"
                } else {
                    "请再按一个键组成快捷键。"
                });
            }
            if new != old
                && !new.trim().is_empty()
                && !crate::modifier_hotkey::is_modifier_only(new)
            {
                use tauri_plugin_global_shortcut::{Modifiers, Shortcut};
                let shortcut: Shortcut = new
                    .parse()
                    .map_err(|_| anyhow::anyhow!("invalid hotkey: {new}"))?;
                let key = shortcut.key.to_string();
                let function_key = key
                    .strip_prefix('F')
                    .and_then(|number| number.parse::<u8>().ok())
                    .is_some_and(|number| (1..=24).contains(&number));
                if !shortcut
                    .mods
                    .intersects(Modifiers::SUPER | Modifiers::CONTROL | Modifiers::ALT)
                    && !function_key
                {
                    anyhow::bail!(if recording {
                        "请使用 ⌘、⌥ 或 ⌃ 组成快捷键，或选择 Fn。"
                    } else {
                        "请使用 ⌘、⌥ 或 ⌃ 加上另一个键。"
                    });
                }
            }
        }
        Ok(())
    }

    pub fn on_device_asr_ready(&self, models_root: Option<&Path>) -> bool {
        self.asr_provider == crate::engine::EngineProvider::OnDevice
            && models_root.is_some_and(|root| {
                crate::ondevice_asr::model_setup_is_ready(root, &self.asr_model)
            })
    }

    fn validate_provider_side(
        &self,
        provider: crate::engine::EngineProvider,
        asr: bool,
        models_root: Option<&Path>,
        check_readiness: bool,
    ) -> anyhow::Result<()> {
        if asr && !provider.has_asr() {
            anyhow::bail!("所选服务商不支持转写。");
        }
        if !asr && !provider.has_llm() {
            anyhow::bail!("OnDevice 不能用于文字整理。");
        }
        if provider == crate::engine::EngineProvider::OnDevice && asr {
            if check_readiness && self.onboarded && !self.on_device_asr_ready(models_root) {
                anyhow::bail!("本机语音识别尚未就绪，不能完成设置。");
            }
            return Ok(());
        }
        if !provider.has_http_asr()
            && asr
            && !matches!(
                provider,
                crate::engine::EngineProvider::Soniox | crate::engine::EngineProvider::DashScope
            )
        {
            return Ok(());
        }
        let url = self.resolved_provider_base(provider);
        if provider == crate::engine::EngineProvider::DashScope
            && crate::providers::resolve_asr_endpoint(provider, &url).is_none()
        {
            anyhow::bail!("请选择有效的 DashScope 区域端点。");
        }
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
            if !check_readiness || !self.onboarded {
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
            crate::engine::EngineProvider::AssemblyAi => crate::engine::EngineProvider::AssemblyAi
                .default_base_url()
                .to_owned(),
            crate::engine::EngineProvider::DashScope => {
                let url = self.asr_base_url.trim();
                if url.is_empty() {
                    provider.default_base_url().to_owned()
                } else {
                    url.to_owned()
                }
            }
            crate::engine::EngineProvider::OnDevice => String::new(),
            other => other.default_base_url().to_owned(),
        }
    }

    pub fn asr_endpoint(&self) -> Option<String> {
        crate::providers::resolve_asr_endpoint(
            self.asr_provider,
            &self.resolved_provider_base(self.asr_provider),
        )
    }

    /// Prefer a dedicated ASR key when set. Reuse the Groq key only for the
    /// Groq default or `api.groq.com`; custom hosts must supply `asr_api_key`.
    pub fn asr_credential(&self) -> &str {
        self.provider_secret(self.asr_provider)
    }

    pub fn accurate_asr_configured(&self) -> bool {
        self.accurate_asr_provider.has_http_asr()
            && !self.accurate_asr_model.trim().is_empty()
            && !self.accurate_asr_credential().trim().is_empty()
    }

    pub fn accurate_asr_credential(&self) -> &str {
        self.provider_secret(self.accurate_asr_provider)
    }

    pub fn accurate_asr_endpoint(&self) -> Option<String> {
        if !self.accurate_asr_provider.has_http_asr() {
            return None;
        }
        let base = if !self.accurate_asr_base_url.trim().is_empty() {
            self.accurate_asr_base_url.trim().to_owned()
        } else {
            self.resolved_provider_base(self.accurate_asr_provider)
        };
        crate::providers::resolve_asr_endpoint(self.accurate_asr_provider, &base)
    }

    pub fn vision_configured(&self) -> bool {
        if !crate::screen_action::vision_settings_ready(&self.vision_provider, &self.vision_model) {
            return false;
        }
        let Some(provider) = crate::engine::EngineProvider::parse(&self.vision_provider) else {
            return false;
        };
        if !provider.has_llm() {
            return false;
        }
        let key = self.provider_secret(provider);
        !key.is_empty() || provider.allows_empty_key()
    }

    pub fn vision_credential(&self) -> &str {
        match crate::engine::EngineProvider::parse(&self.vision_provider) {
            Some(provider) => self.provider_secret(provider),
            None => "",
        }
    }

    pub fn vision_endpoint(&self) -> Option<String> {
        let provider = crate::engine::EngineProvider::parse(&self.vision_provider)?;
        if !provider.has_llm() {
            return None;
        }
        Some(crate::llm::resolve_chat_url(
            &self.resolved_provider_base(provider),
        ))
    }

    pub fn cleanup_credential(&self) -> &str {
        self.provider_secret(self.cleanup_provider)
    }

    pub fn cleanup_endpoint(&self) -> String {
        if self.cleanup_provider == crate::engine::EngineProvider::Ollama {
            return crate::ollama_local::chat_endpoint(&self.ollama_base_url);
        }
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
    pub strict_offline_enabled: bool,
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
    pub keep_success_audio: bool,
    pub onboarded: bool,
    pub cleanup_enabled: bool,
    pub cleanup_intensity: String,
    pub accurate_asr_provider: crate::engine::EngineProvider,
    pub accurate_asr_model: String,
    pub accurate_asr_base_url: String,
    pub cascade_timeout_ms: u64,
    pub cascade_proper_noun_threshold: usize,
    pub window_ocr_enabled: bool,
    pub screen_action_hotkey: String,
    pub vision_provider: String,
    pub vision_model: String,
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
    pub verbatim_hotkey: String,
    pub translation_hotkey: String,
    pub extra_recording_buffer_ms: u64,
    pub audio_feedback_enabled: bool,
    pub audio_feedback_volume: f32,
    pub vad_enabled: bool,
    pub always_on_microphone: bool,
    pub clamshell_microphone: String,
    pub autostart_enabled: bool,
    pub whats_new_last_seen_version: String,
    pub debug_mode: bool,
    pub fuzzy_dictionary_enabled: bool,
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
            strict_offline_enabled: settings.strict_offline_enabled,
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
            keep_success_audio: settings.keep_success_audio,
            onboarded: settings.onboarded,
            cleanup_enabled: settings.cleanup_enabled,
            cleanup_intensity: settings.cleanup_intensity.clone(),
            accurate_asr_provider: settings.accurate_asr_provider,
            accurate_asr_model: settings.accurate_asr_model.clone(),
            accurate_asr_base_url: settings.accurate_asr_base_url.clone(),
            cascade_timeout_ms: settings.cascade_timeout_ms,
            cascade_proper_noun_threshold: settings.cascade_proper_noun_threshold,
            window_ocr_enabled: settings.window_ocr_enabled,
            screen_action_hotkey: settings.screen_action_hotkey.clone(),
            vision_provider: settings.vision_provider.clone(),
            vision_model: settings.vision_model.clone(),
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
            verbatim_hotkey: settings.verbatim_hotkey.clone(),
            translation_hotkey: settings.translation_hotkey.clone(),
            extra_recording_buffer_ms: settings.extra_recording_buffer_ms,
            audio_feedback_enabled: settings.audio_feedback_enabled,
            audio_feedback_volume: settings.audio_feedback_volume,
            vad_enabled: settings.vad_enabled,
            always_on_microphone: settings.always_on_microphone,
            clamshell_microphone: settings.clamshell_microphone.clone(),
            autostart_enabled: settings.autostart_enabled,
            whats_new_last_seen_version: settings.whats_new_last_seen_version.clone(),
            debug_mode: settings.debug_mode,
            fuzzy_dictionary_enabled: settings.fuzzy_dictionary_enabled,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct HistoryItem {
    pub id: i64,
    pub created_at: String,
    pub raw_text: String,
    /// Provider-returned transcript before local filtering, revisions, or
    /// dictionary replacement. Null on rows written before this field existed.
    pub asr_text: Option<String>,
    /// Provider-supplied cleanup candidate kept distinct from the ASR text and
    /// VoiceFlow's final guarded output. Null when the provider supplied none.
    pub provider_cleaned_candidate: Option<String>,
    /// ASR provider/model that produced the selected transcript, when known.
    pub engine: Option<String>,
    pub final_text: String,
    pub cleanup_status: String,
    pub duration: f64,
    pub degraded: bool,
    pub degraded_reason: Option<String>,
    pub status: String,
    pub delivery_method: Option<String>,
    pub fallback_reason: Option<String>,
    /// Stable reason code for a paste delivery failure or unverified result.
    pub delivery_error_code: Option<String>,
    /// Safe, transcript-free next step shown in History.
    pub delivery_user_reason: Option<String>,
    pub context_profile_id: Option<String>,
    pub retryable: bool,
    pub revision_count: usize,
    pub has_audio: bool,
    pub verbatim_text: Option<String>,
    pub verbatim_reviewed: bool,
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
#[cfg(test)]
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
pub const MAX_GOLD_BYTES: u64 = 4 * 1024 * 1024 * 1024;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_start_sample: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sample_count: Option<u64>,
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

fn spool_manifest_age_secs(manifest: &SpoolManifest, now_ms: u64) -> Option<u64> {
    if manifest.created_at_ms == 0 || manifest.created_at_ms > now_ms {
        return None;
    }
    Some((now_ms - manifest.created_at_ms) / 1_000)
}

fn is_safe_spool_path(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

fn gold_root(dir: &Path) -> PathBuf {
    dir.join("gold")
}

fn is_safe_managed_audio_path(dir: &Path, path: &Path) -> bool {
    is_safe_spool_path(&dir.join("spool"), path) || is_safe_spool_path(&gold_root(dir), path)
}

fn managed_audio_exists(dir: &Path, path: &Path) -> bool {
    is_safe_managed_audio_path(dir, path) && path.is_file()
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
    let nonce =
        XNonce::from_slice(&header[SPOOL_ENVELOPE_MAGIC.len() + 1..SPOOL_ENVELOPE_HEADER_LEN]);
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

/// Persist a successful-dictation WAV under `gold/`. Quota is independent of
/// the recovery spool. Full quota returns an error; callers must not fail the
/// dictation itself.
pub fn write_gold_file(dir: &Path, file_name: &str, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let relative = Path::new(file_name);
    if file_name.is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        anyhow::bail!("invalid gold audio path");
    }
    let encryption_enabled = spool_encryption_enabled();
    let encryption_key = if encryption_enabled {
        Some(history_encryption_key_for_write()?)
    } else {
        None
    };
    let root = gold_root(dir);
    let path = root.join(relative);
    let stored_bytes = if encryption_enabled {
        let key = encryption_key
            .as_ref()
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
        > MAX_GOLD_BYTES
    {
        anyhow::bail!("gold audio quota exceeded");
    }
    ensure_private_dir(&root)?;
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

/// Persist one complete WAV as a recoverable spool session. This is used when
/// History is the only delivery recovery path and a failed History write must
/// leave audio that startup recovery can promote into a retryable History item.
pub fn persist_recovery_wav_session(
    root: &Path,
    session_id: &str,
    wav: &[u8],
) -> anyhow::Result<RecoveredSpool> {
    let duration_secs = crate::asr::wav_duration_seconds(wav)
        .filter(|duration| duration.is_finite() && *duration > 0.0)
        .ok_or_else(|| anyhow::anyhow!("recovery WAV duration could not be read"))?;
    let session_dir = begin_spool_session(root, session_id)?;
    let result = (|| {
        let audio_path = write_spool_session_file(&session_dir, "recovery.wav", wav)?;
        mark_spool_status(&session_dir, "recoverable")?;
        Ok(RecoveredSpool {
            audio_path,
            duration_secs,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&session_dir);
    }
    result
}

pub fn record_spool_chunk(
    session_dir: &Path,
    index: usize,
    start_secs: f32,
    end_secs: f32,
    status: &str,
) -> anyhow::Result<()> {
    record_spool_chunk_inner(session_dir, index, start_secs, end_secs, status, None)
}

/// Record a written chunk with exact source-timeline sample coverage.
/// Later status-only updates through `record_spool_chunk` preserve these fields.
pub fn record_spool_chunk_with_samples(
    session_dir: &Path,
    index: usize,
    start_secs: f32,
    end_secs: f32,
    status: &str,
    source_start_sample: u64,
    sample_count: u64,
) -> anyhow::Result<()> {
    if sample_count == 0 || source_start_sample.checked_add(sample_count).is_none() {
        anyhow::bail!("invalid recovery chunk sample interval");
    }
    record_spool_chunk_inner(
        session_dir,
        index,
        start_secs,
        end_secs,
        status,
        Some((source_start_sample, sample_count)),
    )
}

fn record_spool_chunk_inner(
    session_dir: &Path,
    index: usize,
    start_secs: f32,
    end_secs: f32,
    status: &str,
    source_interval: Option<(u64, u64)>,
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
        if let Some((source_start_sample, sample_count)) = source_interval {
            chunk.source_start_sample = Some(source_start_sample);
            chunk.sample_count = Some(sample_count);
        }
    } else {
        manifest.chunks.push(SpoolChunkManifest {
            index,
            start_secs,
            end_secs,
            status: status.to_owned(),
            source_start_sample: source_interval.map(|(start, _)| start),
            sample_count: source_interval.map(|(_, count)| count),
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

    let manifest = if manifest_path(session_dir).is_file() {
        Some(load_manifest(session_dir)?)
    } else {
        None
    };
    let mut manifest_chunks = std::collections::BTreeMap::new();
    if let Some(manifest) = &manifest {
        for chunk in &manifest.chunks {
            if manifest_chunks.insert(chunk.index, chunk).is_some() {
                anyhow::bail!("recovery manifest contains duplicate chunk indexes");
            }
            if chunk.source_start_sample.is_some() != chunk.sample_count.is_some() {
                anyhow::bail!("recovery manifest contains an incomplete sample interval");
            }
        }
    }

    let manifest_has_exact_intervals = manifest_chunks
        .values()
        .any(|chunk| chunk.source_start_sample.is_some());
    let mut decoded_chunks = Vec::with_capacity(chunks.len());
    for (index, path) in chunks {
        let bytes = read_spool_file(&path)?;
        if bytes.len() % std::mem::size_of::<f32>() != 0 {
            anyhow::bail!("recovery audio chunk is truncated");
        }
        let samples = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect::<Vec<_>>();
        let interval = manifest_chunks
            .get(&index)
            .and_then(|chunk| chunk.source_start_sample.zip(chunk.sample_count));
        decoded_chunks.push((index, samples, interval));
    }

    let exact_interval_count = decoded_chunks
        .iter()
        .filter(|(_, _, interval)| interval.is_some())
        .count();
    if exact_interval_count == 0 {
        if manifest_has_exact_intervals {
            anyhow::bail!("recovery sample intervals are missing for one or more chunks");
        }
        let mut legacy_timing = decoded_chunks
            .iter()
            .filter_map(|(index, _, _)| manifest_chunks.get(index).copied())
            .collect::<Vec<_>>();
        legacy_timing.sort_by_key(|chunk| chunk.index);
        if legacy_timing
            .windows(2)
            .any(|pair| pair[1].start_secs < pair[0].end_secs)
        {
            anyhow::bail!("legacy recovery chunks overlap but lack exact source sample intervals");
        }
        let mut samples: Vec<f32> = Vec::new();
        for (_, chunk_samples, _) in decoded_chunks {
            samples.extend(chunk_samples);
        }
        if samples.is_empty() {
            anyhow::bail!("recovery contains no audio samples");
        }
        return Ok(samples);
    }

    if exact_interval_count != decoded_chunks.len()
        || manifest_chunks.values().any(|chunk| {
            chunk.source_start_sample.is_some()
                && !decoded_chunks.iter().any(|(i, _, _)| i == &chunk.index)
        })
    {
        anyhow::bail!("recovery sample intervals are missing for one or more chunks");
    }

    let mut positioned = Vec::with_capacity(decoded_chunks.len());
    for (index, samples, interval) in decoded_chunks {
        let (source_start, sample_count) =
            interval.ok_or_else(|| anyhow::anyhow!("recovery sample intervals are incomplete"))?;
        if sample_count == 0 || u64::try_from(samples.len()).ok() != Some(sample_count) {
            anyhow::bail!("recovery chunk sample count does not match its manifest");
        }
        let source_end = source_start
            .checked_add(sample_count)
            .ok_or_else(|| anyhow::anyhow!("recovery chunk sample interval overflows"))?;
        positioned.push((source_start, source_end, index, samples));
    }

    positioned.sort_by_key(|(start, _, index, _)| (*start, *index));
    let mut samples: Vec<f32> = Vec::new();
    let mut covered_until = 0_u64;
    for (source_start, source_end, _, chunk_samples) in positioned {
        if source_start > covered_until {
            anyhow::bail!("recovery audio contains a gap in source samples");
        }
        let overlap_count = usize::try_from(covered_until.saturating_sub(source_start))
            .unwrap_or(usize::MAX)
            .min(chunk_samples.len());
        let overlap_start = usize::try_from(source_start)
            .map_err(|_| anyhow::anyhow!("recovery sample interval is too large"))?;
        for (offset, chunk_sample) in chunk_samples.iter().take(overlap_count).enumerate() {
            let existing = samples
                .get(overlap_start + offset)
                .ok_or_else(|| anyhow::anyhow!("recovery sample coverage is inconsistent"))?;
            if existing.to_bits() != chunk_sample.to_bits() {
                anyhow::bail!("overlapping recovery chunks contain conflicting samples");
            }
        }
        if source_end > covered_until {
            samples.extend_from_slice(&chunk_samples[overlap_count..]);
            covered_until = source_end;
        }
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

fn read_existing_recovery_wav(session_dir: &Path) -> anyhow::Result<RecoveredSpool> {
    let audio_path = session_dir.join("recovery.wav");
    let wav = read_spool_file(&audio_path)?;
    let duration_secs = crate::asr::wav_duration_seconds(&wav)
        .filter(|duration| duration.is_finite() && *duration > 0.0)
        .ok_or_else(|| anyhow::anyhow!("stored recovery WAV duration could not be read"))?;
    Ok(RecoveredSpool {
        audio_path,
        duration_secs,
    })
}

/// Rebuild a retryable WAV from the complete audio chunks kept for a long
/// recording. This is also used when processing finishes in a degraded state
/// so the user can retry the whole recording instead of losing failed chunks.
pub fn rebuild_spool_recovery(session_dir: &Path) -> anyhow::Result<RecoveredSpool> {
    rebuild_recovery_wav(session_dir)
}

/// Recover interrupted sessions into retryable audio artifacts. Active
/// manifests are never treated as successful dictations: complete,
/// atomically-written chunks are rebuilt, while explicitly persisted single
/// WAV recovery sessions are retained for manual retry.
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
            // Session-directory mtime changes whenever recovery output or the
            // manifest is rewritten. Retention must remain anchored to the
            // manifest's immutable creation time, otherwise every successful
            // startup could extend the lifetime indefinitely. Invalid or
            // future timestamps cannot establish a trustworthy age, so expire
            // those sessions fail-closed.
            let age = manifest
                .as_ref()
                .and_then(|manifest| spool_manifest_age_secs(manifest, now_ms()));
            let expired = age.is_none_or(|age| age > max_age);
            let recoverable = manifest.as_ref().is_some_and(|manifest| {
                matches!(
                    manifest.status.as_str(),
                    "active" | "recoverable" | "degraded"
                )
            });
            if recoverable && !expired {
                let recovery = rebuild_recovery_wav(&path)
                    .or_else(|chunk_error| {
                        read_existing_recovery_wav(&path).map_err(|wav_error| {
                            anyhow::anyhow!(
                                "chunks could not be rebuilt ({chunk_error}); stored WAV could not be read ({wav_error})"
                            )
                        })
                    });
                match recovery {
                    Ok(recovery) => {
                        let _ = mark_spool_status(&path, "recoverable");
                        recovered.push(recovery);
                    }
                    Err(error) => {
                        log::warn!(
                            "preserving spool session at {} without a rebuilt recovery WAV; the source chunks could not be reconstructed: {error}",
                            path.display()
                        );
                        let _ = mark_spool_status(&path, "abandoned");
                    }
                }
            } else if expired {
                fs::remove_dir_all(path)?;
            }
        } else {
            let age = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .map(|elapsed| elapsed.as_secs());
            if age.is_none_or(|age| age > max_age) {
                fs::remove_file(path)?;
            }
        }
    }
    Ok(recovered)
}

fn is_safe_secret_sidecar_slot(slot: &str) -> bool {
    !slot.is_empty()
        && slot.len() <= 64
        && slot
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

fn secret_sidecar_slot(slot: &str) -> Option<&str> {
    match slot {
        "api_key" | "groq_api_key" => Some("api_key"),
        "asr_api_key" => Some("asr_api_key"),
        "cleanup_api_key" => Some("cleanup_api_key"),
        other if is_safe_secret_sidecar_slot(other) => Some(other),
        _ => None,
    }
}

fn secret_sidecar_path(dir: &Path, slot: &str) -> PathBuf {
    let slot = secret_sidecar_slot(slot).unwrap_or("api_key");
    dir.join("secrets").join(slot)
}

#[cfg(test)]
fn write_secret_sidecar(dir: &Path, slot: &str, key: &str) -> anyhow::Result<()> {
    let slot = secret_sidecar_slot(slot)
        .ok_or_else(|| anyhow::anyhow!("unsupported secret sidecar slot"))?;
    write_atomic_bytes(&secret_sidecar_path(dir, slot), key.as_bytes())
}

fn read_secret_sidecar(dir: &Path, slot: &str) -> Option<String> {
    let bytes = fs::read(secret_sidecar_path(dir, slot)).ok()?;
    let key = String::from_utf8(bytes).ok()?;
    (!key.trim().is_empty()).then_some(key)
}

fn clear_secret_sidecar(dir: &Path, slot: &str) {
    let _ = fs::remove_file(secret_sidecar_path(dir, slot));
}

pub fn clear_provider_key_sidecars(dir: &Path, provider: crate::engine::EngineProvider) {
    clear_secret_sidecar(dir, provider.keychain_account());
    if provider == crate::engine::EngineProvider::Groq {
        clear_secret_sidecar(dir, "api_key");
    }
    if provider == crate::engine::EngineProvider::Custom {
        clear_secret_sidecar(dir, "asr_api_key");
        clear_secret_sidecar(dir, "cleanup_api_key");
    }
}

pub fn clear_asr_key_sidecar(dir: &Path) {
    clear_secret_sidecar(dir, "asr_api_key");
}

pub fn clear_cleanup_key_sidecar(dir: &Path) {
    clear_secret_sidecar(dir, "cleanup_api_key");
}

trait CredentialBackend {
    fn read(&self, slot: &str) -> crate::keychain::ApiKeyState;
    fn write(&self, slot: &str, key: &str) -> Result<(), String>;
}

struct OsCredentialBackend;

impl CredentialBackend for OsCredentialBackend {
    fn read(&self, slot: &str) -> crate::keychain::ApiKeyState {
        use crate::engine::EngineProvider;
        match slot {
            "api_key" | "groq_api_key" => crate::keychain::get_api_key_state(),
            "asr_api_key" => crate::keychain::get_asr_api_key_state(),
            "cleanup_api_key" => crate::keychain::get_cleanup_api_key_state(),
            account => EngineProvider::ALL
                .into_iter()
                .find(|provider| provider.keychain_account() == account)
                .map(crate::keychain::get_provider_api_key_state)
                .unwrap_or_else(|| {
                    crate::keychain::ApiKeyState::Unavailable(format!(
                        "unsupported credential account {account}"
                    ))
                }),
        }
    }

    fn write(&self, slot: &str, key: &str) -> Result<(), String> {
        use crate::engine::EngineProvider;
        match slot {
            "api_key" | "groq_api_key" => crate::keychain::set_api_key(key),
            "asr_api_key" => crate::keychain::set_asr_api_key(key),
            "cleanup_api_key" => crate::keychain::set_cleanup_api_key(key),
            account => EngineProvider::ALL
                .into_iter()
                .find(|provider| provider.keychain_account() == account)
                .ok_or_else(|| format!("unsupported credential account {account}"))
                .and_then(|provider| crate::keychain::set_provider_api_key(provider, key)),
        }
    }
}

fn clear_secret_sidecar_if_matches(dir: &Path, slot: &str, key: &str) {
    if read_secret_sidecar(dir, slot).as_deref() == Some(key) {
        clear_secret_sidecar(dir, slot);
    }
}

/// Persist to the OS credential store and confirm the exact value can be read
/// back before treating the operation as durable. `preserve_existing` is used
/// for legacy migration: it prevents a stale source from replacing a secure
/// credential that already exists or could not be read.
fn persist_secret_to_secure_store(
    dir: &Path,
    slot: &str,
    key: &str,
    backend: &dyn CredentialBackend,
    preserve_existing: bool,
    clear_replaced_sidecar: bool,
) -> anyhow::Result<()> {
    let before = backend.read(slot);
    match &before {
        crate::keychain::ApiKeyState::Configured(stored) if stored == key => {
            if clear_replaced_sidecar {
                clear_secret_sidecar(dir, slot);
            } else {
                clear_secret_sidecar_if_matches(dir, slot, key);
            }
            return Ok(());
        }
        crate::keychain::ApiKeyState::Configured(_) if preserve_existing => {
            anyhow::bail!("credential_storage: secure {slot} credential changed during migration")
        }
        crate::keychain::ApiKeyState::Unavailable(error) if preserve_existing => {
            anyhow::bail!("credential_storage: cannot verify existing {slot} credential: {error}")
        }
        crate::keychain::ApiKeyState::Missing
        | crate::keychain::ApiKeyState::Configured(_)
        | crate::keychain::ApiKeyState::Unavailable(_) => {}
    }

    backend.write(slot, key).map_err(|error| {
        anyhow::anyhow!("credential_storage: failed to store {slot} securely: {error}")
    })?;
    match backend.read(slot) {
        crate::keychain::ApiKeyState::Configured(stored) if stored == key => {
            if clear_replaced_sidecar {
                clear_secret_sidecar(dir, slot);
            } else {
                clear_secret_sidecar_if_matches(dir, slot, key);
            }
            Ok(())
        }
        crate::keychain::ApiKeyState::Configured(_) => {
            anyhow::bail!("credential_storage: secure {slot} write verification failed")
        }
        crate::keychain::ApiKeyState::Missing => {
            anyhow::bail!("credential_storage: secure {slot} write was not readable")
        }
        crate::keychain::ApiKeyState::Unavailable(error) => {
            anyhow::bail!("credential_storage: cannot verify secure {slot} write: {error}")
        }
    }
}

fn credential_source_slot(source: &str) -> Option<String> {
    if let Some(field) = source.strip_prefix("settings:") {
        return match field {
            "api_key" => Some("groq_api_key".into()),
            "asr_api_key" => Some("asr_api_key".into()),
            "cleanup_api_key" => Some("cleanup_api_key".into()),
            provider_field if provider_field.starts_with("provider:") => {
                let id = provider_field.strip_prefix("provider:")?;
                let provider = crate::providers::EngineProvider::parse(id)?;
                Some(provider.keychain_account().into())
            }
            _ => None,
        };
    }
    let sidecar = source.strip_prefix("sidecar:")?;
    let normalized = secret_sidecar_slot(sidecar)?;
    if normalized == "api_key" {
        Some("groq_api_key".into())
    } else {
        Some(normalized.to_owned())
    }
}

fn has_unverified_source(settings: &Settings, slot: &str, key: &str) -> bool {
    settings
        .unverified_credential_sources
        .iter()
        .any(|(source, value)| {
            if value != key {
                return false;
            }
            let Some(source_slot) = credential_source_slot(source) else {
                return false;
            };
            source_slot == slot
                || (slot == crate::engine::EngineProvider::Custom.keychain_account()
                    && matches!(source_slot.as_str(), "asr_api_key" | "cleanup_api_key"))
        })
}

fn is_explicit_credential_replacement(settings: &Settings, slot: &str, key: &str) -> bool {
    if has_unverified_source(settings, slot, key) {
        return false;
    }
    settings
        .credential_baselines
        .get(slot)
        .is_none_or(|previous| previous != key)
}

struct CredentialResolution {
    value: String,
    state: crate::keychain::ApiKeyState,
    settings_source_verified: bool,
}

fn resolve_legacy_credential(
    dir: &Path,
    slot: &str,
    legacy_value: &str,
    settings_source: Option<&str>,
    settings: &mut Settings,
    backend: &dyn CredentialBackend,
) -> CredentialResolution {
    let has_legacy_value = !legacy_value.trim().is_empty();
    let sidecar_value = read_secret_sidecar(dir, slot);
    let (candidate, source) = if has_legacy_value {
        (
            Some(legacy_value.to_owned()),
            settings_source.map(str::to_owned),
        )
    } else if let Some(value) = sidecar_value {
        (Some(value), Some(format!("sidecar:{slot}")))
    } else {
        (None, None)
    };

    match backend.read(slot) {
        crate::keychain::ApiKeyState::Configured(stored) => {
            settings
                .credential_baselines
                .insert(slot.to_owned(), stored.clone());
            if let Some(value) = candidate.as_deref() {
                if value == stored {
                    clear_secret_sidecar_if_matches(dir, slot, value);
                }
            }
            CredentialResolution {
                value: stored.clone(),
                state: crate::keychain::ApiKeyState::Configured(stored),
                settings_source_verified: settings_source.is_some() && has_legacy_value,
            }
        }
        crate::keychain::ApiKeyState::Unavailable(error) => {
            if let (Some(value), Some(source)) = (candidate, source) {
                settings
                    .unverified_credential_sources
                    .insert(source, value.clone());
                CredentialResolution {
                    value,
                    state: crate::keychain::ApiKeyState::Unavailable(error),
                    settings_source_verified: false,
                }
            } else {
                CredentialResolution {
                    value: String::new(),
                    state: crate::keychain::ApiKeyState::Unavailable(error),
                    settings_source_verified: false,
                }
            }
        }
        crate::keychain::ApiKeyState::Missing => {
            let Some(value) = candidate else {
                return CredentialResolution {
                    value: String::new(),
                    state: crate::keychain::ApiKeyState::Missing,
                    settings_source_verified: false,
                };
            };
            match persist_secret_to_secure_store(dir, slot, &value, backend, true, false) {
                Ok(()) => {
                    settings
                        .credential_baselines
                        .insert(slot.to_owned(), value.clone());
                    CredentialResolution {
                        value: value.clone(),
                        state: crate::keychain::ApiKeyState::Configured(value),
                        settings_source_verified: settings_source.is_some() && has_legacy_value,
                    }
                }
                Err(error) => {
                    let error = error.to_string();
                    if let Some(source) = source {
                        settings
                            .unverified_credential_sources
                            .insert(source, value.clone());
                    }
                    log::warn!("legacy credential migration for {slot} was not verified: {error}");
                    CredentialResolution {
                        value,
                        state: crate::keychain::ApiKeyState::Unavailable(error),
                        settings_source_verified: false,
                    }
                }
            }
        }
    }
}

/// Load settings from `settings.json`, then reconcile the API key with the
/// OS credential store. Returns the settings plus a flag indicating whether
/// the caller should persist the normalized result.
pub fn load_settings(dir: &Path) -> (Settings, bool) {
    load_settings_with_backend(dir, &OsCredentialBackend)
}

fn load_settings_with_backend(dir: &Path, backend: &dyn CredentialBackend) -> (Settings, bool) {
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
        || !settings.cleanup_api_key.is_empty()
        || !settings.provider_api_keys.is_empty();
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

    let mut needs_persist = schema_needs_persist || hotkeys_rewritten;
    let legacy_api_key = settings.api_key.clone();
    let api_resolution = resolve_legacy_credential(
        dir,
        "groq_api_key",
        &legacy_api_key,
        (!legacy_api_key.trim().is_empty()).then_some("settings:api_key"),
        &mut settings,
        backend,
    );
    settings.api_key = api_resolution.value;
    if api_resolution.settings_source_verified {
        needs_persist = true;
    }
    let api_key_missing = matches!(api_resolution.state, crate::keychain::ApiKeyState::Missing);

    let legacy_asr_key = settings.asr_api_key.clone();
    let asr_resolution = resolve_legacy_credential(
        dir,
        "asr_api_key",
        &legacy_asr_key,
        (!legacy_asr_key.trim().is_empty()).then_some("settings:asr_api_key"),
        &mut settings,
        backend,
    );
    settings.asr_api_key = asr_resolution.value;
    if asr_resolution.settings_source_verified {
        needs_persist = true;
    }
    let asr_key_missing = matches!(asr_resolution.state, crate::keychain::ApiKeyState::Missing);

    let legacy_cleanup_key = settings.cleanup_api_key.clone();
    let cleanup_resolution = resolve_legacy_credential(
        dir,
        "cleanup_api_key",
        &legacy_cleanup_key,
        (!legacy_cleanup_key.trim().is_empty()).then_some("settings:cleanup_api_key"),
        &mut settings,
        backend,
    );
    settings.cleanup_api_key = cleanup_resolution.value;
    if cleanup_resolution.settings_source_verified {
        needs_persist = true;
    }
    let cleanup_key_missing = matches!(
        cleanup_resolution.state,
        crate::keychain::ApiKeyState::Missing
    );

    let mut provider_states = std::collections::HashMap::new();
    for provider in crate::providers::EngineProvider::ALL {
        let legacy_key = settings
            .provider_api_keys
            .get(provider.as_str())
            .cloned()
            .unwrap_or_default();
        let source = (!legacy_key.trim().is_empty())
            .then(|| format!("settings:provider:{}", provider.as_str()));
        let resolution = resolve_legacy_credential(
            dir,
            provider.keychain_account(),
            &legacy_key,
            source.as_deref(),
            &mut settings,
            backend,
        );
        if resolution.settings_source_verified {
            needs_persist = true;
        }
        if !resolution.value.trim().is_empty() {
            settings
                .provider_api_keys
                .insert(provider.as_str().to_owned(), resolution.value);
        } else {
            settings.provider_api_keys.remove(provider.as_str());
        }
        provider_states.insert(provider, resolution.state);
    }
    bind_legacy_keys_into_pool(&mut settings);
    let selected_asr = settings.asr_provider;
    let selected_asr_key_missing = if selected_asr.is_groq() {
        api_key_missing
    } else {
        matches!(
            provider_states.get(&selected_asr),
            Some(crate::keychain::ApiKeyState::Missing)
        )
    };
    let selected_asr_url = settings.resolved_provider_base(selected_asr);
    let selected_asr_empty_ok = selected_asr == crate::engine::EngineProvider::OnDevice
        && settings.on_device_asr_ready(Some(&dir.join("models")))
        || selected_asr.allows_empty_key() && crate::providers::is_loopback_url(&selected_asr_url);
    if settings.onboarded
        && (selected_asr == crate::engine::EngineProvider::OnDevice
            && !settings.on_device_asr_ready(Some(&dir.join("models")))
            || selected_asr_key_missing
                && settings.asr_credential().trim().is_empty()
                && !selected_asr_empty_ok)
    {
        needs_persist = true;
        settings.onboarded = false;
    }
    // Only repair when the store confirmed the key is absent. A timeout must
    // not wipe a custom URL that still has a credential in the keychain.
    if asr_key_missing && settings.asr_api_key.trim().is_empty() && settings.repair_incomplete_asr()
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
    object.insert(
        "provider_api_keys".into(),
        serde_json::Value::Object(serde_json::Map::new()),
    );
    let bytes = serde_json::to_vec_pretty(&value)?;
    write_atomic_bytes(&backup, &bytes)?;
    Ok(())
}
/// Persist settings to `settings.json`. API keys are written only to the OS
/// credential store and verified there before the settings file is updated.
pub fn save_settings(dir: &Path, settings: &Settings) -> anyhow::Result<()> {
    save_settings_with_backend(dir, settings, &OsCredentialBackend)
}

fn save_settings_with_backend(
    dir: &Path,
    settings: &Settings,
    backend: &dyn CredentialBackend,
) -> anyhow::Result<()> {
    save_settings_with_validation(dir, settings, backend, true)
}

pub fn save_configuration(dir: &Path, settings: &Settings) -> anyhow::Result<()> {
    save_settings_with_validation(dir, settings, &OsCredentialBackend, false)
}

fn save_settings_with_validation(
    dir: &Path,
    settings: &Settings,
    backend: &dyn CredentialBackend,
    check_readiness: bool,
) -> anyhow::Result<()> {
    settings.validate_internal(Some(&dir.join("models")), check_readiness)?;
    let lock = SETTINGS_WRITE_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_private_dir(dir)?;
    // Persist every non-empty credential before writing settings.json. A
    // legacy value may remain in memory while its migration is pending; its
    // marker prevents a transient read failure from replacing a newer secure
    // value. On failure this function returns before modifying settings.json.
    if !settings.api_key.trim().is_empty() {
        persist_secret_to_secure_store(
            dir,
            "groq_api_key",
            &settings.api_key,
            backend,
            has_unverified_source(settings, "groq_api_key", &settings.api_key),
            is_explicit_credential_replacement(settings, "groq_api_key", &settings.api_key),
        )?;
    }
    if !settings.asr_api_key.trim().is_empty() {
        persist_secret_to_secure_store(
            dir,
            "asr_api_key",
            &settings.asr_api_key,
            backend,
            has_unverified_source(settings, "asr_api_key", &settings.asr_api_key),
            is_explicit_credential_replacement(settings, "asr_api_key", &settings.asr_api_key),
        )?;
    }
    if !settings.cleanup_api_key.trim().is_empty() {
        persist_secret_to_secure_store(
            dir,
            "cleanup_api_key",
            &settings.cleanup_api_key,
            backend,
            has_unverified_source(settings, "cleanup_api_key", &settings.cleanup_api_key),
            is_explicit_credential_replacement(
                settings,
                "cleanup_api_key",
                &settings.cleanup_api_key,
            ),
        )?;
    }
    for (id, key) in &settings.provider_api_keys {
        let Some(provider) = crate::providers::EngineProvider::parse(id) else {
            continue;
        };
        if key.trim().is_empty() {
            continue;
        }
        let slot = provider.keychain_account();
        persist_secret_to_secure_store(
            dir,
            slot,
            key,
            backend,
            has_unverified_source(settings, slot, key),
            is_explicit_credential_replacement(settings, slot, key),
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
    ensure_column(c, "delivery_error_code", "TEXT")?;
    ensure_column(c, "delivery_user_reason", "TEXT")?;
    ensure_column(c, "context_profile_id", "TEXT")?;
    ensure_column(c, "context_policy_json", "TEXT")?;
    ensure_column(c, "context_family", "TEXT")?;
    ensure_column(c, "context_browser_host", "TEXT")?;
    ensure_column(c, "context_native_bundle", "TEXT")?;
    ensure_column(c, "cleanup_status", "TEXT")?;
    ensure_column(c, "verbatim_text", "TEXT")?;
    ensure_column(c, "verbatim_reviewed", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column(c, "asr_text", "TEXT")?;
    ensure_column(c, "provider_cleaned_candidate", "TEXT")?;
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS learn_pairs (
            pair_key TEXT PRIMARY KEY,
            before_surface TEXT NOT NULL,
            after_surface TEXT NOT NULL,
            hits INTEGER NOT NULL,
            promoted INTEGER NOT NULL DEFAULT 0,
            last_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS learned_term_usage (
            word TEXT PRIMARY KEY,
            replacement_runs INTEGER NOT NULL,
            last_replaced_at TEXT NOT NULL
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
        match fs::symlink_metadata(&backup) {
            Ok(metadata) if metadata.file_type().is_file() => {}
            Ok(_) => anyhow::bail!("unsafe history migration backup path"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut options = fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                // create_new refuses existing links, including a link placed
                // after the metadata check. Never overwrite another file.
                let mut destination = options.open(&backup)?;
                let mut source = File::open(&path)?;
                if let Err(error) = std::io::copy(&mut source, &mut destination) {
                    drop(destination);
                    let _ = fs::remove_file(&backup);
                    return Err(error.into());
                }
                restrict_file_mode(&backup)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    schema(&connection)?;
    restrict_history_sidecars(dir)?;
    prune_history_migration_backups(
        dir,
        Some(HISTORY_MIGRATION_BACKUP_MAX_DAYS),
        SystemTime::now(),
    )?;
    Ok(connection)
}

/// Only names generated by open_history are owned rollback copies. Do not
/// recurse or follow links; user exports, settings and model files are outside
/// this cleanup boundary. None means the user explicitly cleared all data.
fn prune_history_migration_backups(
    dir: &Path,
    keep_days: Option<u64>,
    now: SystemTime,
) -> anyhow::Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(version) = name
            .to_str()
            .and_then(|name| name.strip_prefix("history.sqlite.v"))
            .and_then(|name| name.strip_suffix(".bak"))
        else {
            continue;
        };
        if !version
            .parse::<u32>()
            .is_ok_and(|value| value.to_string() == version)
        {
            continue;
        }
        let metadata = match fs::symlink_metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if !metadata.file_type().is_file() {
            continue;
        }
        let expired = match keep_days {
            None => true,
            Some(days) => now
                .duration_since(metadata.modified()?)
                .is_ok_and(|age| age >= Duration::from_secs(days.saturating_mul(86_400))),
        };
        if expired {
            match fs::remove_file(entry.path()) {
                Ok(()) => removed += 1,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(removed)
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
    pub asr_text: Option<&'a str>,
    pub engine: Option<&'a str>,
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
        asr_text: None,
        engine: None,
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
        asr_text: None,
        engine: None,
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
pub fn insert_history_with_asr_and_delivery_and_spool_and_cleanup(
    dir: &Path,
    raw: &str,
    asr_text: Option<&str>,
    final_text: &str,
    duration: f64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    context: &crate::context::ContextSnapshot,
    spool: Option<&Path>,
    engine: &str,
    cleanup_status: &str,
) -> anyhow::Result<()> {
    insert_history_with_asr_candidate_and_delivery_and_spool_and_cleanup(
        dir,
        raw,
        asr_text,
        None,
        final_text,
        duration,
        degraded,
        degraded_reason,
        status,
        delivery_method,
        fallback_reason,
        context,
        spool,
        engine,
        cleanup_status,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn insert_history_with_asr_candidate_and_delivery_and_spool_and_cleanup(
    dir: &Path,
    raw: &str,
    asr_text: Option<&str>,
    provider_cleaned_candidate: Option<&str>,
    final_text: &str,
    duration: f64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    context: &crate::context::ContextSnapshot,
    spool: Option<&Path>,
    engine: &str,
    cleanup_status: &str,
) -> anyhow::Result<()> {
    insert_history_with_asr_candidate_and_delivery_and_spool_and_cleanup_diagnostic(
        dir,
        raw,
        asr_text,
        provider_cleaned_candidate,
        final_text,
        duration,
        degraded,
        degraded_reason,
        status,
        delivery_method,
        fallback_reason,
        context,
        spool,
        engine,
        cleanup_status,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn insert_history_with_asr_candidate_and_delivery_and_spool_and_cleanup_diagnostic(
    dir: &Path,
    raw: &str,
    asr_text: Option<&str>,
    provider_cleaned_candidate: Option<&str>,
    final_text: &str,
    duration: f64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    context: &crate::context::ContextSnapshot,
    spool: Option<&Path>,
    engine: &str,
    cleanup_status: &str,
    diagnostic: Option<&crate::delivery_diagnostics::DeliveryDiagnostic>,
) -> anyhow::Result<()> {
    insert_history_with_status_and_candidate(
        InsertHistory {
            dir,
            raw,
            asr_text,
            engine: Some(engine),
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
        },
        provider_cleaned_candidate,
        diagnostic,
    )
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
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
    insert_history_with_asr_and_delivery_and_spool_and_cleanup(
        dir,
        raw,
        None,
        final_text,
        duration,
        degraded,
        degraded_reason,
        status,
        delivery_method,
        fallback_reason,
        context,
        spool,
        "unknown",
        cleanup_status,
    )
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
        asr_text: None,
        engine: None,
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
    engine: &str,
) -> anyhow::Result<()> {
    insert_history_with_status(InsertHistory {
        dir,
        raw,
        asr_text: None,
        engine: Some(engine),
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
    insert_history_with_status_and_candidate(input, None, None)
}

fn insert_history_with_status_and_candidate(
    input: InsertHistory,
    provider_cleaned_candidate: Option<&str>,
    diagnostic: Option<&crate::delivery_diagnostics::DeliveryDiagnostic>,
) -> anyhow::Result<()> {
    let InsertHistory {
        dir,
        raw,
        asr_text,
        engine,
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
        .map(|snapshot| serde_json::to_string(&snapshot.policy.history_metadata()))
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
    c.execute("INSERT INTO dictations (created_at,duration_secs,raw_text,final_text,cleanup_status,engine,degraded,degraded_reason,status,raw_audio_path,delivery_method,fallback_reason,context_profile_id,context_policy_json,context_family,context_browser_host,context_native_bundle,asr_text,provider_cleaned_candidate,delivery_error_code,delivery_user_reason) VALUES (datetime('now'),?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)", params![duration, raw, final_text, cleanup_status, engine.unwrap_or("unknown"), degraded as i32, degraded_reason, status, spool.map(|p| p.to_string_lossy().to_string()), delivery_method, fallback_reason, context_profile_id, context_policy_json, context_family, context_browser_host, context_native_bundle, asr_text, provider_cleaned_candidate, diagnostic.map(|item| item.code), diagnostic.map(|item| item.user_reason)])?;
    if count_asr || count_llm {
        c.execute("INSERT INTO usage(day,asr_requests,llm_requests,audio_seconds) VALUES (date('now'),?,?,?) ON CONFLICT(day) DO UPDATE SET asr_requests=asr_requests+excluded.asr_requests,llm_requests=llm_requests+excluded.llm_requests,audio_seconds=audio_seconds+excluded.audio_seconds", params![count_asr as i64, count_llm as i64, duration])?;
    }
    Ok(())
}
#[cfg(test)]
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
    let limit = limit.clamp(1, 100);
    let fetch_limit = limit + 1;
    let search_pattern = query
        .filter(|value| !value.is_empty())
        .map(|value| format!("%{}%", escape_history_search(value)));
    let mut s = c.prepare("SELECT id,created_at,COALESCE(raw_text,''),COALESCE((SELECT final_text FROM dictation_revisions WHERE dictation_id=dictations.id ORDER BY revision_id DESC LIMIT 1),final_text,''),COALESCE(duration_secs,0),COALESCE(degraded,0),degraded_reason,COALESCE(status,'ok'),delivery_method,fallback_reason,context_profile_id,raw_audio_path,COALESCE(cleanup_status,'unknown'),(SELECT COUNT(*) FROM dictation_revisions WHERE dictation_id=dictations.id),verbatim_text,COALESCE(verbatim_reviewed,0),asr_text,engine,provider_cleaned_candidate,delivery_error_code,delivery_user_reason FROM dictations WHERE (?1 IS NULL OR id < ?1) AND (?2 IS NULL OR (COALESCE(raw_text,'') LIKE ?2 ESCAPE '\\' OR COALESCE(asr_text,'') LIKE ?2 ESCAPE '\\' OR COALESCE(provider_cleaned_candidate,'') LIKE ?2 ESCAPE '\\' OR COALESCE((SELECT final_text FROM dictation_revisions WHERE dictation_id=dictations.id ORDER BY revision_id DESC LIMIT 1),final_text,'') LIKE ?2 ESCAPE '\\')) ORDER BY id DESC LIMIT ?3")?;
    let mut items = s
        .query_map(
            params![before_id, search_pattern.as_deref(), fetch_limit],
            history_row(dir),
        )?
        .collect::<Result<Vec<_>, _>>()?;
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
    dir: &'a Path,
) -> impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<HistoryItem> + 'a {
    move |r| {
        let raw_audio_path: Option<String> = r.get(11)?;
        let status: String = r.get(7)?;
        let has_audio = raw_audio_path
            .as_deref()
            .is_some_and(|path| managed_audio_exists(dir, Path::new(path)));
        let retryable = has_audio && matches!(status.as_str(), "failed" | "degraded");
        let verbatim_text: Option<String> = r.get::<_, Option<String>>(14)?.and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(value)
            }
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
            status,
            delivery_method: r.get(8)?,
            fallback_reason: r.get(9)?,
            context_profile_id: r.get(10)?,
            retryable,
            revision_count: r.get::<_, i64>(13)? as usize,
            has_audio,
            verbatim_text,
            verbatim_reviewed: r.get::<_, i32>(15)? != 0,
            asr_text: r.get(16)?,
            engine: r.get(17)?,
            provider_cleaned_candidate: r.get(18)?,
            delivery_error_code: r.get(19)?,
            delivery_user_reason: r.get(20)?,
        })
    }
}

#[cfg(test)]
pub fn get_history(dir: &Path, limit: i64) -> anyhow::Result<Vec<HistoryItem>> {
    Ok(get_history_page(dir, limit, None, None)?.items)
}

pub fn purge_history(dir: &Path, keep_history_days: u64) -> anyhow::Result<usize> {
    let backup_days = if keep_history_days == 0 {
        HISTORY_MIGRATION_BACKUP_MAX_DAYS
    } else {
        keep_history_days.min(HISTORY_MIGRATION_BACKUP_MAX_DAYS)
    };
    prune_history_migration_backups(dir, Some(backup_days), SystemTime::now())?;
    // Zero is the explicit "keep forever" setting for live history text.
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
    #[serde(default)]
    pub promote_hits: u32,
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
        promote_hits: 0,
    })
}

#[cfg(test)]
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

pub fn set_learn_pair_hits(dir: &Path, pair_key: &str, hits: u32) -> anyhow::Result<bool> {
    let c = open_history(dir)?;
    c.execute(
        "UPDATE learn_pairs SET hits = ?2, promoted = 0, last_at = datetime('now') WHERE pair_key = ?1 AND ignored = 0 AND tombstoned_at IS NULL",
        params![pair_key, hits as i64],
    )?;
    Ok(c.changes() > 0)
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

pub fn ensure_pinned_dictionary_term(
    dir: &Path,
    after_surface: &str,
    pinned: bool,
) -> anyhow::Result<()> {
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
    let mut statement =
        c.prepare("UPDATE learn_pairs SET last_used_at = datetime('now') WHERE pair_key = ?1")?;
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

#[derive(Clone, Serialize)]
pub struct LearnedTermUsage {
    pub word: String,
    pub replacement_runs: u64,
    pub last_replaced_at: String,
}

/// One observation per distinct corrected term in one local processing pass.
pub fn record_learned_term_usage(dir: &Path, words: &[String]) -> anyhow::Result<()> {
    if words.is_empty() {
        return Ok(());
    }
    let mut c = open_history(dir)?;
    let transaction = c.transaction()?;
    for word in words.iter().collect::<std::collections::BTreeSet<_>>() {
        transaction.execute("INSERT INTO learned_term_usage (word, replacement_runs, last_replaced_at) VALUES (?1, 1, datetime('now')) ON CONFLICT(word) DO UPDATE SET replacement_runs = replacement_runs + 1, last_replaced_at = datetime('now')", [word])?;
    }
    transaction.commit()?;
    Ok(())
}

pub fn list_learned_term_usage(dir: &Path) -> anyhow::Result<Vec<LearnedTermUsage>> {
    let c = open_history(dir)?;
    let mut statement = c.prepare("SELECT word, replacement_runs, last_replaced_at FROM learned_term_usage WHERE EXISTS (SELECT 1 FROM learn_pairs WHERE after_surface = word AND promoted = 1 AND ignored = 0 AND tombstoned_at IS NULL) ORDER BY last_replaced_at DESC, word ASC")?;
    let rows = statement.query_map([], |row| {
        Ok(LearnedTermUsage {
            word: row.get(0)?,
            replacement_runs: row.get(1)?,
            last_replaced_at: row.get(2)?,
        })
    })?;
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
    c.execute_batch("DELETE FROM dictations; DELETE FROM usage; DELETE FROM learn_pairs; DELETE FROM style_drafts; DELETE FROM learned_term_usage;")?;
    drop(c);
    // Run after open_history: clearing a legacy database can itself create a
    // rollback copy, which must not retain the very text the user cleared.
    prune_history_migration_backups(dir, None, SystemTime::now())?;
    let spool = dir.join("spool");
    if spool.exists() {
        fs::remove_dir_all(&spool)?;
    }
    ensure_private_dir(&spool)?;
    let gold = gold_root(dir);
    if gold.exists() {
        fs::remove_dir_all(&gold)?;
    }
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

#[cfg(test)]
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
    let context_policy_json = context_policy
        .map(crate::context::ContextPolicy::history_metadata)
        .map(|policy| serde_json::to_string(&policy))
        .transpose()?;
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
    mark_retried_with_texts(
        dir,
        id,
        None,
        None,
        None,
        final_text,
        degraded,
        degraded_reason,
        None,
    )
}

// Keep the persisted transcript, provider-original transcript, and delivery
// status explicit at this history boundary.
#[allow(clippy::too_many_arguments)]
pub fn mark_retried_with_texts(
    dir: &Path,
    id: i64,
    raw_text: Option<&str>,
    asr_text: Option<&str>,
    engine: Option<&str>,
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
        "UPDATE dictations SET status=?, degraded=?, degraded_reason=?, raw_text=COALESCE(?,raw_text), asr_text=COALESCE(?,asr_text), engine=COALESCE(?,engine), cleanup_status=COALESCE(?,cleanup_status), raw_audio_path=NULL, delivery_method='clipboard', fallback_reason='retry_clipboard_only', delivery_error_code=NULL, delivery_user_reason=NULL WHERE id=?",
        params![
            if degraded { "degraded" } else { "copied" },
            degraded as i32,
            degraded_reason,
            raw_text,
            asr_text,
            engine,
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
        asr_text: None,
        engine: None,
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
    let spool_root = dir.join("spool");
    if is_safe_spool_path(&spool_root, path) {
        if path.is_dir() {
            let _ = fs::remove_dir_all(path);
            return;
        }
        let _ = fs::remove_file(path);
        if let Some(parent) = path.parent() {
            if parent != spool_root && parent.join("manifest.json").is_file() {
                let _ = fs::remove_dir_all(parent);
            }
        }
        return;
    }
    if is_safe_spool_path(&gold_root(dir), path) && path.is_file() {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
pub fn history_audio_bytes(dir: &Path, id: i64) -> anyhow::Result<Vec<u8>> {
    let c = open_history(dir)?;
    let path: Option<String> = c
        .query_row(
            "SELECT raw_audio_path FROM dictations WHERE id=?",
            [id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let path = path.ok_or_else(|| anyhow::anyhow!("history audio is no longer available"))?;
    let path = Path::new(&path);
    if !managed_audio_exists(dir, path) {
        anyhow::bail!("history audio is no longer available");
    }
    read_spool_file(path)
}

#[cfg(test)]
pub fn save_verbatim(dir: &Path, id: i64, text: &str, reviewed: bool) -> anyhow::Result<()> {
    let trimmed = text.trim();
    if reviewed && trimmed.is_empty() {
        anyhow::bail!("reviewed verbatim text cannot be empty");
    }
    let stored = if reviewed || !trimmed.is_empty() {
        Some(trimmed)
    } else {
        None
    };
    let c = open_history(dir)?;
    c.execute(
        "UPDATE dictations SET verbatim_text=?, verbatim_reviewed=? WHERE id=?",
        params![stored, reviewed as i32, id],
    )?;
    if c.changes() == 0 {
        anyhow::bail!("history record was not found");
    }
    Ok(())
}

pub fn qwen_gold_language_tag(language: &str) -> &'static str {
    match language {
        "zh" => "Chinese",
        "en" => "English",
        _ => "None",
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GoldExport {
    pub directory: String,
    pub jsonl_path: String,
    pub count: usize,
}

pub fn export_gold_corpus(
    dir: &Path,
    downloads: &Path,
    language: &str,
) -> anyhow::Result<GoldExport> {
    if downloads.as_os_str().is_empty() || !downloads.is_absolute() {
        anyhow::bail!("invalid export path");
    }
    let c = open_history(dir)?;
    let mut statement = c.prepare(
        "SELECT id, COALESCE(raw_text,''), raw_audio_path FROM dictations WHERE raw_audio_path IS NOT NULL ORDER BY id ASC",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    drop(c);

    let language_tag = qwen_gold_language_tag(language);
    let export_dir = downloads.join(format!("voiceflow-gold-{}", now_ms()));
    ensure_private_dir(&export_dir)?;
    let jsonl_path = export_dir.join("corpus.jsonl");
    let mut jsonl = String::new();
    let mut count = 0usize;
    let gold = gold_root(dir);
    for (id, raw_text, audio_path) in rows {
        let text = raw_text.trim();
        if text.is_empty() {
            continue;
        }
        let Some(audio_path) = audio_path else {
            continue;
        };
        let source = Path::new(&audio_path);
        if !is_safe_spool_path(&gold, source) || !source.is_file() {
            continue;
        }
        let wav = match read_spool_file(source) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!("skipping gold export for history {id}: {error}");
                continue;
            }
        };
        count += 1;
        let file_name = format!("utt{count:04}.wav");
        let dest = export_dir.join(&file_name);
        write_atomic_bytes(&dest, &wav)?;
        let line = serde_json::json!({
            "audio": dest.to_string_lossy(),
            "text": format!("language {language_tag}<asr_text>{text}"),
        });
        jsonl.push_str(&line.to_string());
        jsonl.push('\n');
    }
    if count == 0 {
        let _ = fs::remove_dir_all(&export_dir);
        anyhow::bail!("没有可导出的训练音频。");
    }
    write_atomic_bytes(&jsonl_path, jsonl.as_bytes())?;
    Ok(GoldExport {
        directory: export_dir.to_string_lossy().into_owned(),
        jsonl_path: jsonl_path.to_string_lossy().into_owned(),
        count,
    })
}

pub fn purge_gold_audio(dir: &Path, keep_audio_days: u64) -> anyhow::Result<usize> {
    let root = gold_root(dir);
    if !root.exists() {
        return Ok(0);
    }
    let max_age = keep_audio_days.saturating_mul(86_400);
    let mut deleted = 0usize;
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let age = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(max_age.saturating_add(1));
        if (keep_audio_days == 0 || age > max_age) && fs::remove_file(&path).is_ok() {
            deleted += 1;
        }
    }
    let c = open_history(dir)?;
    let mut statement =
        c.prepare("SELECT id, raw_audio_path FROM dictations WHERE raw_audio_path IS NOT NULL")?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for (id, path) in rows.into_iter() {
        let Some(path) = path else {
            continue;
        };
        let path = Path::new(&path);
        if is_safe_spool_path(&root, path) && !path.is_file() {
            c.execute("UPDATE dictations SET raw_audio_path=NULL WHERE id=?", [id])?;
        }
    }
    Ok(deleted)
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
        "UPDATE dictations SET status=?,degraded=?,degraded_reason=?,cleanup_status=COALESCE(?,cleanup_status),delivery_method='clipboard',fallback_reason=NULL,delivery_error_code=NULL,delivery_user_reason=NULL WHERE id=?",
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
        remove_spool_artifact(dir, Path::new(&spool));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeCredentialBackend {
        values: RefCell<std::collections::BTreeMap<String, String>>,
        read_scripts: RefCell<
            std::collections::BTreeMap<
                String,
                std::collections::VecDeque<crate::keychain::ApiKeyState>,
            >,
        >,
        write_errors: RefCell<std::collections::BTreeMap<String, String>>,
        writes: RefCell<Vec<(String, String)>>,
    }

    impl FakeCredentialBackend {
        fn seed(&self, slot: &str, key: &str) {
            self.values
                .borrow_mut()
                .insert(slot.to_owned(), key.to_owned());
        }

        fn fail_write(&self, slot: &str, error: &str) {
            self.write_errors
                .borrow_mut()
                .insert(slot.to_owned(), error.to_owned());
        }

        fn script_reads(
            &self,
            slot: &str,
            states: impl IntoIterator<Item = crate::keychain::ApiKeyState>,
        ) {
            self.read_scripts
                .borrow_mut()
                .insert(slot.to_owned(), states.into_iter().collect());
        }
    }

    impl CredentialBackend for FakeCredentialBackend {
        fn read(&self, slot: &str) -> crate::keychain::ApiKeyState {
            if let Some(state) = self
                .read_scripts
                .borrow_mut()
                .get_mut(slot)
                .and_then(std::collections::VecDeque::pop_front)
            {
                return state;
            }
            self.values
                .borrow()
                .get(slot)
                .cloned()
                .map(crate::keychain::ApiKeyState::Configured)
                .unwrap_or(crate::keychain::ApiKeyState::Missing)
        }

        fn write(&self, slot: &str, key: &str) -> Result<(), String> {
            self.writes
                .borrow_mut()
                .push((slot.to_owned(), key.to_owned()));
            if let Some(error) = self.write_errors.borrow().get(slot) {
                return Err(error.clone());
            }
            self.values
                .borrow_mut()
                .insert(slot.to_owned(), key.to_owned());
            Ok(())
        }
    }

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

    fn write_spool_f32_chunk(root: &Path, session_id: &str, index: usize, samples: &[f32]) {
        let mut bytes = Vec::with_capacity(std::mem::size_of_val(samples));
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let relative = PathBuf::from(session_id)
            .join("chunks")
            .join(format!("{index:08}.f32"));
        write_spool_file(root, &relative, &bytes).unwrap();
    }

    fn decode_wav_samples(path: &Path) -> Vec<i16> {
        let wav = read_spool_file(path).unwrap();
        hound::WavReader::new(std::io::Cursor::new(wav))
            .unwrap()
            .into_samples::<i16>()
            .map(Result::unwrap)
            .collect()
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
    fn schema_20_defaults_cleanup_intensity_auto() {
        let settings = Settings::default();
        assert_eq!(settings.cleanup_intensity, "auto");
        assert_eq!(settings.cascade_timeout_ms, 5000);
        assert_eq!(settings.cascade_proper_noun_threshold, 3);
        assert!(settings.accurate_asr_model.is_empty());
        assert_eq!(
            settings.accurate_asr_provider,
            crate::engine::EngineProvider::Groq
        );
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        let view = SettingsView::from(&settings);
        assert_eq!(view.cleanup_intensity, "auto");
        assert_eq!(view.cascade_timeout_ms, 5000);
        assert_eq!(view.cascade_proper_noun_threshold, 3);
        assert!(view.accurate_asr_model.is_empty());
        assert!(!settings.accurate_asr_configured());
        assert!(!settings.window_ocr_enabled);
        assert!(!view.window_ocr_enabled);
        assert!(settings.screen_action_hotkey.is_empty());
        assert!(settings.vision_provider.is_empty());
        assert!(settings.vision_model.is_empty());
        assert!(!settings.vision_configured());
        assert!(view.screen_action_hotkey.is_empty());
        assert!(view.vision_model.is_empty());
    }
    #[test]
    fn migration_keeps_an_explicit_saved_cleanup_intensity() {
        let mut settings = Settings {
            schema_version: 19,
            cleanup_intensity: "heavy".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.cleanup_intensity, "heavy");
    }

    #[test]
    fn accurate_asr_helpers_use_model_and_provider_keychain() {
        let mut settings = Settings::default();
        assert!(!settings.accurate_asr_configured());
        assert_eq!(settings.accurate_asr_credential(), "");
        settings.accurate_asr_model = "whisper-large-v3".into();
        settings.accurate_asr_provider = crate::engine::EngineProvider::OpenAi;
        settings
            .provider_api_keys
            .insert("openai".into(), "sk-accurate".into());
        assert!(settings.accurate_asr_configured());
        assert_eq!(settings.accurate_asr_credential(), "sk-accurate");
        assert!(settings
            .accurate_asr_endpoint()
            .expect("HTTP accurate endpoint")
            .contains("transcriptions"));
    }

    #[test]
    fn accurate_asr_configured_requires_nonempty_credential() {
        let mut settings = Settings {
            api_key: "gsk-groq".into(),
            accurate_asr_provider: crate::engine::EngineProvider::Custom,
            accurate_asr_model: "qwen3-asr-flash".into(),
            ..Settings::default()
        };
        assert_eq!(settings.accurate_asr_credential().trim(), "");
        assert!(!settings.accurate_asr_configured());

        settings
            .provider_api_keys
            .insert("custom".into(), "   ".into());
        assert_eq!(settings.accurate_asr_credential().trim(), "");
        assert!(!settings.accurate_asr_configured());

        settings
            .provider_api_keys
            .insert("custom".into(), "sk-bailian".into());
        assert_eq!(settings.accurate_asr_credential(), "sk-bailian");
        assert!(settings.accurate_asr_configured());

        settings.accurate_asr_provider = crate::engine::EngineProvider::Groq;
        settings.accurate_asr_model = "whisper-large-v3".into();
        settings.provider_api_keys.remove("custom");
        assert!(settings.accurate_asr_configured());
        assert_eq!(settings.accurate_asr_credential(), "gsk-groq");
    }

    #[test]
    fn vision_configured_needs_provider_model_and_key() {
        let mut settings = Settings {
            vision_model: "gpt-4o".into(),
            ..Settings::default()
        };
        assert!(!settings.vision_configured());
        settings.vision_provider = "openai".into();
        assert!(!settings.vision_configured());
        settings
            .provider_api_keys
            .insert("openai".into(), "sk-vision".into());
        assert!(settings.vision_configured());
        settings.vision_provider = "ollama".into();
        settings.vision_model = "llava".into();
        assert!(settings.vision_configured());
    }

    fn write_sensevoice_fixture(models_root: &Path) {
        let dir = models_root.join("sensevoice-small");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("model.int8.onnx"), b"fixture-onnx").unwrap();
        std::fs::write(dir.join("tokens.txt"), b"fixture-tokens").unwrap();
        std::fs::write(
            crate::ondevice_asr::archive_sha_path(models_root, "sensevoice-small"),
            crate::ondevice_models::SENSEVOICE_ARCHIVE_SHA256,
        )
        .unwrap();
    }

    fn on_device_settings(onboarded: bool) -> Settings {
        Settings {
            asr_provider: crate::engine::EngineProvider::OnDevice,
            asr_model: "sensevoice-small".into(),
            api_key: String::new(),
            onboarded,
            cleanup_enabled: false,
            ..Settings::default()
        }
    }

    #[test]
    fn on_device_files_ready_do_not_allow_onboarding_without_inference() {
        let dir = temp_dir("on-device-ready-validate");
        let models_root = dir.join("models");
        write_sensevoice_fixture(&models_root);
        let settings = on_device_settings(true);
        assert!(settings
            .validate_with_models_root(Some(&models_root))
            .is_err());
        assert!(!settings.on_device_asr_ready(Some(&models_root)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn onboarded_on_device_empty_groq_key_rejected_when_model_missing() {
        let dir = temp_dir("on-device-missing-validate");
        let models_root = dir.join("models");
        std::fs::create_dir_all(&models_root).unwrap();
        let settings = on_device_settings(true);
        assert!(settings
            .validate_with_models_root(Some(&models_root))
            .is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn validate_without_models_root_rejects_on_device_empty_key() {
        let dir = temp_dir("on-device-validate-no-root");
        write_sensevoice_fixture(&dir.join("models"));
        let settings = on_device_settings(true);
        assert!(
            settings.validate().is_err(),
            "validate() must not grant the empty-key exception without a Ready check"
        );
        assert!(settings.validate_with_models_root(None).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn load_settings_does_not_keep_onboarding_for_files_only_on_device() {
        let dir = temp_dir("on-device-load-ready");
        std::fs::create_dir_all(&dir).unwrap();
        write_sensevoice_fixture(&dir.join("models"));
        std::fs::write(
            dir.join("settings.json"),
            r#"{"schema_version":19,"onboarded":true,"asr_provider":"on_device","asr_model":"sensevoice-small","cleanup_enabled":false}"#,
        )
        .unwrap();
        let (settings, _) = load_settings(&dir);
        assert_eq!(
            settings.asr_provider,
            crate::engine::EngineProvider::OnDevice
        );
        assert!(!settings.onboarded);
        assert!(settings.api_key.trim().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn vision_and_cleanup_reject_on_device() {
        let mut settings = on_device_settings(false);
        settings.cleanup_enabled = true;
        settings.cleanup_provider = crate::engine::EngineProvider::OnDevice;
        settings.cleanup_model = "llama3.2".into();
        assert!(settings.validate().is_err());

        settings.cleanup_enabled = false;
        settings.vision_provider = "on_device".into();
        settings.vision_model = "sensevoice-small".into();
        assert!(!settings.vision_configured());
        assert!(settings.vision_endpoint().is_none());
    }

    #[test]
    fn accurate_asr_rejects_on_device_and_empty_key() {
        let settings = Settings {
            accurate_asr_provider: crate::engine::EngineProvider::OnDevice,
            accurate_asr_model: "sensevoice-small".into(),
            ..Settings::default()
        };
        assert!(!settings.accurate_asr_configured());
        assert!(settings.accurate_asr_endpoint().is_none());
    }

    #[test]
    fn schema_20_copies_legacy_style_example_into_pairs() {
        let mut settings = Settings {
            cleanup_intensity: String::new(),
            cascade_timeout_ms: 0,
            cascade_proper_noun_threshold: 0,
            context_mappings: vec![crate::context::AppMapping {
                id: "wechat".into(),
                label: "微信".into(),
                family: crate::context::ContextFamily::PersonalChat,
                mode_id: None,
                bundle_id: Some("com.tencent.xinWeChat".into()),
                executable: None,
                browser_host: None,
                browser_path_prefix: None,
                focused_field: None,
                source_permissions: Default::default(),
                style_example_input: Some("好的哈哈".into()),
                style_example_output: Some("好的哈哈。".into()),
                style_example_pairs: Vec::new(),
                style_examples_approved: false,
                enabled: true,
                cleanup_effort: None,
                cleanup_intensity: None,
                cleanup_enabled: true,
                dictionary_learn_enabled: true,
            }],
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.cleanup_intensity, "auto");
        assert_eq!(settings.cascade_timeout_ms, 5000);
        assert_eq!(settings.cascade_proper_noun_threshold, 3);
        assert_eq!(settings.context_mappings[0].style_example_pairs.len(), 1);
        assert_eq!(
            settings.context_mappings[0].style_example_pairs[0].input,
            "好的哈哈"
        );
        assert_eq!(
            settings.context_mappings[0].style_example_pairs[0].output,
            "好的哈哈。"
        );
    }

    #[test]
    fn schema_20_preserves_only_explicit_legacy_local_ocr_choice() {
        let mapping = crate::context::AppMapping {
            id: "notes".into(),
            label: "Notes".into(),
            family: crate::context::ContextFamily::Document,
            mode_id: None,
            bundle_id: Some("com.example.Notes".into()),
            executable: None,
            browser_host: None,
            browser_path_prefix: None,
            focused_field: None,
            source_permissions: Default::default(),
            style_example_input: None,
            style_example_output: None,
            style_example_pairs: Vec::new(),
            style_examples_approved: false,
            enabled: true,
            cleanup_effort: None,
            cleanup_intensity: None,
            cleanup_enabled: true,
            dictionary_learn_enabled: true,
        };
        let mut settings = Settings {
            schema_version: 20,
            window_ocr_enabled: true,
            context_mappings: vec![mapping],
            ..Settings::default()
        };
        settings.normalize();
        let grants = settings.context_mappings[0].source_permissions;
        assert!(grants.local_ocr);
        assert!(!grants.ax_text);
        assert!(!grants.cloud_vision);
        assert!(!grants.context_text_to_providers);

        let legacy: crate::context::AppMapping = serde_json::from_str(
            r#"{"id":"legacy","label":"Legacy","family":"document","bundle_id":"com.example.Editor","style_example_input":"private","style_example_output":"private."}"#,
        )
        .unwrap();
        assert_eq!(legacy.style_example_input.as_deref(), Some("private"));
        assert!(!legacy.style_examples_approved);
        assert_eq!(legacy.source_permissions, Default::default());
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
        assert_eq!(settings.cleanup_intensity, "auto");
        assert_eq!(settings.cascade_timeout_ms, 5000);
        assert_eq!(settings.cascade_proper_noun_threshold, 3);
        assert!(settings.accurate_asr_model.is_empty());
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
        assert!(!settings.keep_success_audio);
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
    fn schema_25_migrates_retired_gestures_to_tap_idempotently_without_rebinding() {
        for mode in ["double_tap", "hybrid", "hold", "tap"] {
            for hotkey in ["Fn", "CmdOrControl+Alt+K", "Shift"] {
                let mut settings = Settings {
                    schema_version: 24,
                    activation_mode: mode.into(),
                    hotkey: hotkey.into(),
                    ..Settings::default()
                };
                settings.normalize();
                assert_eq!(settings.schema_version, 25);
                assert_eq!(settings.activation_mode, "tap");
                assert_eq!(settings.hotkey, hotkey);
                settings.normalize();
                assert_eq!(settings.activation_mode, "tap");
                settings.validate().unwrap();
            }
        }
    }
    #[test]
    fn both_current_modes_support_fn_and_combinations() {
        assert_eq!(Settings::default().activation_mode, "tap");
        for mode in ["tap", "hold_to_talk"] {
            for hotkey in ["Fn", "CmdOrControl+Alt+Space"] {
                let mut settings = Settings {
                    activation_mode: mode.into(),
                    hotkey: hotkey.into(),
                    ..Settings::default()
                };
                settings.normalize();
                assert_eq!(settings.activation_mode, mode);
                settings.validate().unwrap();
            }
        }
    }
    #[test]
    fn changing_binding_preserves_mode_and_rejects_new_standalone_modifiers() {
        let previous = Settings {
            activation_mode: "hold_to_talk".into(),
            ..Settings::default()
        };
        let mut next = previous.clone();
        next.hotkey = "Fn".into();
        next.validate_binding_changes(&previous).unwrap();
        assert_eq!(next.activation_mode, "hold_to_talk");
        next.hotkey = "Shift".into();
        assert!(next.validate_binding_changes(&previous).is_err());
        let legacy = next.clone();
        next.validate_binding_changes(&legacy).unwrap();
    }
    #[test]
    fn new_global_bindings_do_not_take_over_typing_or_navigation() {
        let previous = Settings::default();
        for binding in ["A", "Enter", "Tab", "Space", "Shift+A", "Shift+Space"] {
            let mut next = previous.clone();
            next.hotkey = binding.into();
            next.validate_bindings().unwrap();
            assert!(
                next.validate_binding_changes(&previous).is_err(),
                "{binding}"
            );
            next.hotkey = previous.hotkey.clone();
            next.selected_action_hotkey = binding.into();
            assert!(
                next.validate_binding_changes(&previous).is_err(),
                "{binding}"
            );
        }
    }
    #[test]
    fn new_combinations_and_function_keys_remain_available() {
        let previous = Settings::default();
        for binding in [
            "Fn",
            "CmdOrControl+Shift+1",
            "Alt+A",
            "Control+K",
            "F13",
            "Shift+F13",
        ] {
            let mut next = previous.clone();
            next.hotkey = binding.into();
            next.validate_binding_changes(&previous).unwrap();
        }
    }
    #[test]
    fn mode_changes_preserve_existing_plain_key_bindings() {
        let previous = Settings {
            hotkey: "Shift+A".into(),
            ..Settings::default()
        };
        let mut next = previous.clone();
        next.activation_mode = "hold_to_talk".into();
        next.validate_binding_changes(&previous).unwrap();
        assert_eq!(next.hotkey, "Shift+A");
    }
    #[test]
    fn configuration_validation_allows_missing_unchanged_keys_but_readiness_does_not() {
        let settings = Settings {
            onboarded: true,
            activation_mode: "hold_to_talk".into(),
            ..Settings::default()
        };
        assert!(settings.validate_configuration().is_ok());
        assert!(settings.validate().is_err());
        let dir = temp_dir("binding-without-key");
        let backend = FakeCredentialBackend::default();
        save_settings_with_validation(&dir, &settings, &backend, false).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(saved["activation_mode"], "hold_to_talk");
        let _ = std::fs::remove_dir_all(dir);
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
    fn deprecated_single_modifier_bindings_remain_visible_after_migration() {
        let mut settings = Settings {
            selected_action_hotkey: "Shift".into(),
            verbatim_hotkey: "Alt".into(),
            translation_hotkey: "Control".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.selected_action_hotkey, "Shift");
        assert_eq!(settings.verbatim_hotkey, "Alt");
        assert_eq!(settings.translation_hotkey, "Control");
        settings.validate().unwrap();
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
    fn cleanup_default_moves_off_retired_llama_without_rewriting_saved_choices() {
        assert_eq!(Settings::default().cleanup_model, "openai/gpt-oss-20b");
        let dir = temp_dir("cleanup-model-retirement");
        std::fs::create_dir_all(&dir).unwrap();
        for model in [
            "llama-3.1-8b-instant",
            "llama-3.3-70b-versatile",
            "openai/gpt-oss-120b",
        ] {
            std::fs::write(
                dir.join("settings.json"),
                serde_json::to_vec(&serde_json::json!({
                    "schema_version": SETTINGS_SCHEMA_VERSION,
                    "onboarded": false,
                    "cleanup_enabled": false,
                    "cleanup_provider": "groq",
                    "cleanup_model": model,
                }))
                .unwrap(),
            )
            .unwrap();
            let (settings, _) = load_settings(&dir);
            assert_eq!(settings.cleanup_model, model);
        }

        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": SETTINGS_SCHEMA_VERSION,
                "onboarded": false,
                "cleanup_enabled": false,
                "cleanup_provider": "groq",
            }))
            .unwrap(),
        )
        .unwrap();
        let (settings, _) = load_settings(&dir);
        assert_eq!(settings.cleanup_model, "openai/gpt-oss-20b");
        let _ = std::fs::remove_dir_all(dir);
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
            onboarded: true,
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
        assert_eq!(
            settings.cleanup_provider,
            crate::engine::EngineProvider::Groq
        );
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
            onboarded: true,
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
                "CREATE TABLE dictations (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, duration_secs REAL, raw_text TEXT, final_text TEXT, engine TEXT, degraded INTEGER, status TEXT, raw_audio_path TEXT); INSERT INTO dictations (created_at,raw_text,final_text) VALUES (datetime('now'),'legacy raw','legacy final'); PRAGMA user_version=2;",
            )
            .unwrap();
        drop(connection);

        let history = get_history(&dir, 10).unwrap();

        assert!(dir.join("history.sqlite.v2.bak").is_file());
        assert_eq!(history[0].asr_text, None);
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn clear_all_data_removes_backups_created_while_migrating_legacy_history() {
        let dir = temp_dir("clear-legacy-migration-backups");
        fs::create_dir_all(&dir).unwrap();
        let connection = Connection::open(dir.join("history.sqlite")).unwrap();
        connection.execute_batch("CREATE TABLE dictations (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, duration_secs REAL, raw_text TEXT, final_text TEXT, engine TEXT, degraded INTEGER, status TEXT, raw_audio_path TEXT); INSERT INTO dictations (created_at,raw_text,final_text) VALUES (datetime('now'),'private legacy','private legacy'); PRAGMA user_version=2;").unwrap();
        drop(connection);
        fs::write(dir.join("history.sqlite.v1.bak"), "private previous").unwrap();
        fs::write(dir.join("history.sqlite.v2.bak.user"), "user export").unwrap();
        fs::write(dir.join("settings.json"), "user settings").unwrap();
        clear_all_data(&dir).unwrap();
        assert!(!dir.join("history.sqlite.v1.bak").exists());
        assert!(!dir.join("history.sqlite.v2.bak").exists());
        assert!(get_history(&dir, 10).unwrap().is_empty());
        assert_eq!(
            fs::read_to_string(dir.join("history.sqlite.v2.bak.user")).unwrap(),
            "user export"
        );
        assert_eq!(
            fs::read_to_string(dir.join("settings.json")).unwrap(),
            "user settings"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_backups_have_finite_retention_even_with_forever_live_history() {
        let dir = temp_dir("migration-backup-retention");
        fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now();
        for (version, days) in [(1, 8), (2, 3), (3, 0)] {
            let file = File::create(dir.join(format!("history.sqlite.v{version}.bak"))).unwrap();
            file.set_times(
                fs::FileTimes::new().set_modified(now - Duration::from_secs(days * 86_400)),
            )
            .unwrap();
        }
        assert_eq!(purge_history(&dir, 0).unwrap(), 0);
        assert!(!dir.join("history.sqlite.v1.bak").exists());
        assert!(dir.join("history.sqlite.v2.bak").exists());
        assert!(dir.join("history.sqlite.v3.bak").exists());
        purge_history(&dir, 1).unwrap();
        assert!(!dir.join("history.sqlite.v2.bak").exists());
        assert!(dir.join("history.sqlite.v3.bak").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_backup_cleanup_ignores_future_dates_and_nonowned_names() {
        let dir = temp_dir("migration-backup-boundary");
        fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now();
        for name in [
            "history.sqlite.v1.bak",
            "history.sqlite.v02.bak",
            "history.sqlite.v-1.bak",
            "settings.json.pre-migration.bak",
            "history.sqlite.v2.bak.user",
        ] {
            fs::write(dir.join(name), "private").unwrap();
        }
        File::open(dir.join("history.sqlite.v1.bak"))
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(now + Duration::from_secs(86_400)))
            .unwrap();
        fs::create_dir(dir.join("history.sqlite.v3.bak")).unwrap();
        assert_eq!(
            prune_history_migration_backups(&dir, Some(7), now).unwrap(),
            0
        );
        assert_eq!(prune_history_migration_backups(&dir, None, now).unwrap(), 1);
        assert!(dir.join("history.sqlite.v02.bak").exists());
        assert!(dir.join("history.sqlite.v-1.bak").exists());
        assert!(dir.join("settings.json.pre-migration.bak").exists());
        assert!(dir.join("history.sqlite.v2.bak.user").exists());
        assert!(dir.join("history.sqlite.v3.bak").is_dir());
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn migration_backup_links_are_neither_followed_nor_overwritten() {
        use std::os::unix::fs::symlink;
        let dir = temp_dir("migration-backup-links");
        fs::create_dir_all(&dir).unwrap();
        let external = dir.join("user-backup.txt");
        fs::write(&external, "user-owned private backup").unwrap();
        symlink(&external, dir.join("history.sqlite.v1.bak")).unwrap();
        let dangling_target = dir.join("must-not-create.txt");
        symlink(&dangling_target, dir.join("history.sqlite.v2.bak")).unwrap();
        assert_eq!(
            prune_history_migration_backups(&dir, None, SystemTime::now()).unwrap(),
            0
        );
        assert_eq!(
            fs::read_to_string(&external).unwrap(),
            "user-owned private backup"
        );
        let connection = Connection::open(dir.join("history.sqlite")).unwrap();
        connection.pragma_update(None, "user_version", 2).unwrap();
        drop(connection);
        assert!(open_history(&dir).is_err());
        assert!(!dangling_target.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn save_settings_never_writes_plaintext_key() {
        let dir = temp_dir("keyblank");
        let backend = FakeCredentialBackend::default();
        let settings = Settings {
            api_key: "test_key_should_not_be_on_disk".into(),
            provider_api_keys: [("openai".to_owned(), "synthetic-openai-key".to_owned())]
                .into_iter()
                .collect(),
            ..Settings::default()
        };
        save_settings_with_backend(&dir, &settings, &backend).unwrap();
        let on_disk = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            !on_disk.contains("test_key_should_not_be_on_disk"),
            "plaintext API key must never be written to settings.json"
        );
        let parsed: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(parsed["api_key"].as_str().unwrap_or(""), "");
        assert_eq!(
            backend
                .values
                .borrow()
                .get("groq_api_key")
                .map(String::as_str),
            Some("test_key_should_not_be_on_disk")
        );
        assert_eq!(
            backend
                .values
                .borrow()
                .get("provider_openai")
                .map(String::as_str),
            Some("synthetic-openai-key")
        );
        assert!(!on_disk.contains("synthetic-openai-key"));
        assert!(!secret_sidecar_path(&dir, "api_key").exists());
        assert!(!secret_sidecar_path(&dir, "provider_openai").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn secure_store_failure_preserves_sources_and_creates_no_plaintext() {
        let dir = temp_dir("credential-write-failure");
        std::fs::create_dir_all(dir.join("secrets")).unwrap();
        let previous_settings = br#"{"api_key":"legacy-original","onboarded":false}"#;
        std::fs::write(dir.join("settings.json"), previous_settings).unwrap();
        write_secret_sidecar(&dir, "api_key", "legacy-sidecar").unwrap();
        let previous_sidecar = std::fs::read(secret_sidecar_path(&dir, "api_key")).unwrap();
        let backend = FakeCredentialBackend::default();
        backend.fail_write("groq_api_key", "synthetic secure-store failure");
        let settings = Settings {
            api_key: "new-synthetic-key".into(),
            ..Settings::default()
        };

        let error = save_settings_with_backend(&dir, &settings, &backend).unwrap_err();

        assert!(error.to_string().contains("credential_storage"));
        assert_eq!(
            std::fs::read(dir.join("settings.json")).unwrap(),
            previous_settings
        );
        assert_eq!(
            std::fs::read(secret_sidecar_path(&dir, "api_key")).unwrap(),
            previous_sidecar
        );
        assert!(
            !std::fs::read_to_string(secret_sidecar_path(&dir, "api_key"))
                .unwrap()
                .contains("new-synthetic-key")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn verified_settings_migration_scrubs_settings_only_after_secure_readback() {
        let dir = temp_dir("settings-credential-migration");
        std::fs::create_dir_all(&dir).unwrap();
        let legacy_settings =
            br#"{"schema_version":20,"api_key":"synthetic-legacy-key","onboarded":false}"#;
        std::fs::write(dir.join("settings.json"), legacy_settings).unwrap();
        let backend = FakeCredentialBackend::default();

        let (settings, needs_persist) = load_settings_with_backend(&dir, &backend);

        assert_eq!(settings.api_key, "synthetic-legacy-key");
        assert!(needs_persist);
        assert_eq!(
            backend
                .values
                .borrow()
                .get("groq_api_key")
                .map(String::as_str),
            Some("synthetic-legacy-key")
        );
        save_settings_with_backend(&dir, &settings, &backend).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(saved["api_key"].as_str(), Some(""));
        assert!(!std::fs::read_to_string(dir.join("settings.json"))
            .unwrap()
            .contains("synthetic-legacy-key"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn verified_sidecar_migration_removes_only_matching_legacy_file() {
        let dir = temp_dir("sidecar-migration");
        write_secret_sidecar(&dir, "api_key", "synthetic-groq-legacy").unwrap();
        write_secret_sidecar(&dir, "provider_openai", "unrelated-openai-sidecar").unwrap();
        let backend = FakeCredentialBackend::default();
        let mut settings = Settings::default();

        let resolution =
            resolve_legacy_credential(&dir, "groq_api_key", "", None, &mut settings, &backend);

        assert_eq!(resolution.value, "synthetic-groq-legacy");
        assert!(matches!(
            resolution.state,
            crate::keychain::ApiKeyState::Configured(ref key) if key == "synthetic-groq-legacy"
        ));
        assert!(!secret_sidecar_path(&dir, "api_key").exists());
        assert_eq!(
            read_secret_sidecar(&dir, "provider_openai").as_deref(),
            Some("unrelated-openai-sidecar")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn provider_and_custom_alias_sidecars_migrate_to_their_secure_accounts() {
        let provider_dir = temp_dir("provider-sidecar-migration");
        write_secret_sidecar(&provider_dir, "provider_openai", "synthetic-provider-key").unwrap();
        let provider_backend = FakeCredentialBackend::default();
        let mut provider_settings = Settings::default();
        let provider_resolution = resolve_legacy_credential(
            &provider_dir,
            "provider_openai",
            "",
            None,
            &mut provider_settings,
            &provider_backend,
        );
        assert_eq!(provider_resolution.value, "synthetic-provider-key");
        assert_eq!(
            provider_backend
                .values
                .borrow()
                .get("provider_openai")
                .map(String::as_str),
            Some("synthetic-provider-key")
        );
        assert!(!secret_sidecar_path(&provider_dir, "provider_openai").exists());

        let custom_dir = temp_dir("custom-alias-sidecar-migration");
        write_secret_sidecar(&custom_dir, "asr_api_key", "synthetic-custom-asr").unwrap();
        write_secret_sidecar(&custom_dir, "cleanup_api_key", "synthetic-custom-cleanup").unwrap();
        let custom_backend = FakeCredentialBackend::default();
        let (custom_settings, _) = load_settings_with_backend(&custom_dir, &custom_backend);
        assert_eq!(custom_settings.asr_api_key, "synthetic-custom-asr");
        assert_eq!(custom_settings.cleanup_api_key, "synthetic-custom-cleanup");
        assert_eq!(
            custom_backend
                .values
                .borrow()
                .get("asr_api_key")
                .map(String::as_str),
            Some("synthetic-custom-asr")
        );
        assert_eq!(
            custom_backend
                .values
                .borrow()
                .get("cleanup_api_key")
                .map(String::as_str),
            Some("synthetic-custom-cleanup")
        );
        assert!(!secret_sidecar_path(&custom_dir, "asr_api_key").exists());
        assert!(!secret_sidecar_path(&custom_dir, "cleanup_api_key").exists());
        let _ = std::fs::remove_dir_all(provider_dir);
        let _ = std::fs::remove_dir_all(custom_dir);
    }

    #[test]
    fn failed_or_unverified_legacy_migration_retains_settings_and_sidecar_sources() {
        let dir = temp_dir("failed-credential-migration");
        std::fs::create_dir_all(&dir).unwrap();
        let raw =
            br#"{"schema_version":22,"api_key":"synthetic-settings-source","onboarded":false}"#;
        std::fs::write(dir.join("settings.json"), raw).unwrap();
        write_secret_sidecar(&dir, "api_key", "synthetic-sidecar-source").unwrap();
        let backend = FakeCredentialBackend::default();
        backend.script_reads(
            "groq_api_key",
            [
                crate::keychain::ApiKeyState::Missing,
                crate::keychain::ApiKeyState::Missing,
                crate::keychain::ApiKeyState::Missing,
            ],
        );
        backend.fail_write("groq_api_key", "synthetic write failure");

        let (settings, needs_persist) = load_settings_with_backend(&dir, &backend);

        assert_eq!(settings.api_key, "synthetic-settings-source");
        // Schema 23 requests a save, but a failed credential migration must
        // still prevent overwriting the original source configuration.
        assert!(needs_persist);
        assert!(save_settings_with_backend(&dir, &settings, &backend).is_err());
        assert_eq!(std::fs::read(dir.join("settings.json")).unwrap(), raw);
        assert_eq!(
            read_secret_sidecar(&dir, "api_key").as_deref(),
            Some("synthetic-sidecar-source")
        );
        assert!(settings
            .unverified_credential_sources
            .contains_key("settings:api_key"));

        let unverifiable_dir = temp_dir("unverified-credential-migration");
        write_secret_sidecar(&unverifiable_dir, "api_key", "synthetic-unverified-sidecar").unwrap();
        let unverifiable_backend = FakeCredentialBackend::default();
        unverifiable_backend.script_reads(
            "groq_api_key",
            [
                crate::keychain::ApiKeyState::Missing,
                crate::keychain::ApiKeyState::Missing,
                crate::keychain::ApiKeyState::Missing,
            ],
        );
        let mut unresolved = Settings::default();
        let result = resolve_legacy_credential(
            &unverifiable_dir,
            "groq_api_key",
            "",
            None,
            &mut unresolved,
            &unverifiable_backend,
        );
        assert_eq!(result.value, "synthetic-unverified-sidecar");
        assert!(matches!(
            result.state,
            crate::keychain::ApiKeyState::Unavailable(_)
        ));
        assert_eq!(
            read_secret_sidecar(&unverifiable_dir, "api_key").as_deref(),
            Some("synthetic-unverified-sidecar")
        );
        assert!(unresolved
            .unverified_credential_sources
            .contains_key("sidecar:groq_api_key"));
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(unverifiable_dir);
    }

    #[test]
    fn existing_secure_credential_wins_over_stale_sidecar_without_removing_it() {
        let dir = temp_dir("secure-precedence");
        write_secret_sidecar(&dir, "api_key", "stale-sidecar-value").unwrap();
        let backend = FakeCredentialBackend::default();
        backend.seed("groq_api_key", "newer-secure-value");
        let mut settings = Settings::default();

        let resolution =
            resolve_legacy_credential(&dir, "groq_api_key", "", None, &mut settings, &backend);

        assert_eq!(resolution.value, "newer-secure-value");
        assert!(backend.writes.borrow().is_empty());
        assert_eq!(
            read_secret_sidecar(&dir, "api_key").as_deref(),
            Some("stale-sidecar-value")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn explicit_key_replacement_updates_secure_value_and_clears_legacy_alias() {
        let dir = temp_dir("explicit-credential-replacement");
        write_secret_sidecar(&dir, "groq_api_key", "synthetic-old-sidecar").unwrap();
        let backend = FakeCredentialBackend::default();
        backend.seed("groq_api_key", "synthetic-old-secure");
        let mut settings = Settings {
            api_key: "synthetic-new-key".into(),
            ..Settings::default()
        };
        settings
            .credential_baselines
            .insert("groq_api_key".into(), "synthetic-old-secure".into());

        save_settings_with_backend(&dir, &settings, &backend).unwrap();

        assert_eq!(
            backend
                .values
                .borrow()
                .get("groq_api_key")
                .map(String::as_str),
            Some("synthetic-new-key")
        );
        assert!(!secret_sidecar_path(&dir, "api_key").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn explicit_provider_deletion_clears_groq_and_custom_legacy_aliases() {
        let dir = temp_dir("credential-alias-delete");
        for slot in [
            "api_key",
            "provider_custom",
            "asr_api_key",
            "cleanup_api_key",
        ] {
            write_secret_sidecar(&dir, slot, &format!("synthetic-{slot}")).unwrap();
        }

        clear_provider_key_sidecars(&dir, crate::engine::EngineProvider::Groq);
        assert!(!secret_sidecar_path(&dir, "groq_api_key").exists());
        clear_provider_key_sidecars(&dir, crate::engine::EngineProvider::Custom);
        for slot in ["provider_custom", "asr_api_key", "cleanup_api_key"] {
            assert!(!secret_sidecar_path(&dir, slot).exists());
        }
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
    fn onboarding_requires_the_selected_provider_credential() {
        let settings = Settings {
            onboarded: true,
            api_key: String::new(),
            cleanup_enabled: false,
            ..Settings::default()
        };
        assert!(settings.validate().is_err());
    }

    #[test]
    fn onboarding_accepts_selected_provider_key_without_a_groq_key() {
        let mut settings = Settings {
            onboarded: true,
            api_key: String::new(),
            asr_provider: crate::engine::EngineProvider::SiliconFlow,
            asr_model: "FunAudioLLM/SenseVoiceSmall".into(),
            cleanup_provider: crate::engine::EngineProvider::SiliconFlow,
            cleanup_model: "Qwen/Qwen2.5-7B-Instruct".into(),
            ..Settings::default()
        };
        settings
            .provider_api_keys
            .insert("siliconflow".into(), "configured-in-memory".into());
        settings
            .validate()
            .expect("selected ASR/cleanup provider keys validate");
    }

    #[test]
    fn onboarding_accepts_keyless_loopback_asr_without_a_groq_key() {
        let settings = Settings {
            onboarded: true,
            api_key: String::new(),
            asr_provider: crate::engine::EngineProvider::LocalWhisper,
            cleanup_enabled: false,
            ..Settings::default()
        };
        settings
            .validate()
            .expect("loopback ASR has a valid keyless route");
    }

    #[test]
    fn onboarding_rejects_remote_keyless_cleanup_provider() {
        let mut settings = Settings {
            onboarded: true,
            cleanup_provider: crate::engine::EngineProvider::Ollama,
            ollama_base_url: "https://ollama.example/v1".into(),
            cleanup_model: "llama3.2".into(),
            ..Settings::default()
        };
        settings.api_key = "configured-groq-key".into();
        assert!(settings.validate().is_err());
    }

    #[test]
    fn load_settings_keeps_onboarding_for_selected_provider_sidecar() {
        let dir = temp_dir("selected-provider-onboarding");
        std::fs::create_dir_all(dir.join("secrets")).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": SETTINGS_SCHEMA_VERSION,
                "onboarded": true,
                "asr_provider": "openai",
                "asr_model": "whisper-1",
                "cleanup_enabled": false,
                "api_key": ""
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            secret_sidecar_path(&dir, "provider_openai"),
            "selected-provider-secret",
        )
        .unwrap();

        let (settings, _) = load_settings(&dir);
        assert!(settings.onboarded);
        assert_eq!(settings.asr_provider, crate::engine::EngineProvider::OpenAi);
        assert_eq!(settings.asr_credential(), "selected-provider-secret");
        let _ = std::fs::remove_dir_all(dir);
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
        assert!(!read_spool_file(&recovered[0].audio_path)
            .unwrap()
            .is_empty());
        let manifest: SpoolManifest =
            serde_json::from_slice(&std::fs::read(session.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest.status, "recoverable");
        let _ = std::fs::remove_dir_all(dir);
    }

    fn set_spool_created_at_for_test(session: &Path, created_at_ms: u64) {
        let mut manifest = load_manifest(session).unwrap();
        manifest.created_at_ms = created_at_ms;
        save_manifest(session, &manifest).unwrap();
    }

    fn assert_spool_mtime_is_recent(session: &Path) {
        let elapsed = std::fs::metadata(session)
            .unwrap()
            .modified()
            .unwrap()
            .elapsed()
            .unwrap();
        assert!(
            elapsed.as_secs() < 60,
            "test must model a recent directory mtime"
        );
    }

    #[test]
    fn recovery_expiry_uses_session_creation_time_after_success_or_failure() {
        const KEEP_DAYS: u64 = 7;
        let dir = temp_dir("spool-origin-retention");

        let successful = begin_spool_session(&dir, "successful-old-session").unwrap();
        write_spool_f32_chunk(&dir, "successful-old-session", 0, &[0.1, 0.2, 0.3]);
        record_spool_chunk(&successful, 0, 0.0, 0.0001875, "written").unwrap();
        assert_eq!(recover_spool(&dir, KEEP_DAYS).unwrap().len(), 1);
        set_spool_created_at_for_test(
            &successful,
            now_ms().saturating_sub((KEEP_DAYS + 1) * 86_400_000),
        );
        assert_spool_mtime_is_recent(&successful);

        // A successful startup rewrites recovery.wav and manifest.json. Those
        // filesystem mtimes must not renew the immutable session retention age.
        assert!(recover_spool(&dir, KEEP_DAYS).unwrap().is_empty());
        assert!(
            !successful.exists(),
            "expired source and recovery files are purged"
        );

        let failed = begin_spool_session(&dir, "failed-old-session").unwrap();
        write_spool_f32_chunk(&dir, "failed-old-session", 0, &[0.1, 0.2]);
        write_spool_f32_chunk(&dir, "failed-old-session", 1, &[0.9, 0.3]);
        record_spool_chunk(&failed, 0, 0.0, 1.0, "written").unwrap();
        record_spool_chunk(&failed, 1, 0.5, 1.5, "written").unwrap();
        assert!(recover_spool(&dir, KEEP_DAYS).unwrap().is_empty());
        assert_eq!(load_manifest(&failed).unwrap().status, "abandoned");
        set_spool_created_at_for_test(
            &failed,
            now_ms().saturating_sub((KEEP_DAYS + 1) * 86_400_000),
        );
        assert_spool_mtime_is_recent(&failed);

        // Marking a failed rebuild abandoned changes the directory mtime too,
        // but it does not start a new retention period.
        assert!(recover_spool(&dir, KEEP_DAYS).unwrap().is_empty());
        assert!(
            !failed.exists(),
            "expired unreconstructable chunks are purged"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn recent_recovery_keeps_immutable_creation_time_and_invalid_dates_expire() {
        const KEEP_DAYS: u64 = 7;
        let dir = temp_dir("spool-valid-retention-dates");

        let in_window = begin_spool_session(&dir, "in-window").unwrap();
        let created_at_ms = load_manifest(&in_window).unwrap().created_at_ms;
        write_spool_f32_chunk(&dir, "in-window", 0, &[0.1, 0.2]);
        record_spool_chunk(&in_window, 0, 0.0, 0.000125, "written").unwrap();
        set_spool_created_at_for_test(
            &in_window,
            now_ms().saturating_sub((KEEP_DAYS - 1) * 86_400_000),
        );
        let retained_created_at_ms = load_manifest(&in_window).unwrap().created_at_ms;
        assert_ne!(retained_created_at_ms, created_at_ms);
        assert_eq!(recover_spool(&dir, KEEP_DAYS).unwrap().len(), 1);
        assert_eq!(
            load_manifest(&in_window).unwrap().created_at_ms,
            retained_created_at_ms,
            "successful recovery must preserve its original creation timestamp"
        );
        assert!(in_window.exists());

        for (session_id, created_at_ms) in [
            ("zero-timestamp", 0),
            ("future-timestamp", now_ms().saturating_add(86_400_000)),
        ] {
            let invalid_dir = temp_dir(session_id);
            let invalid = begin_spool_session(&invalid_dir, session_id).unwrap();
            write_spool_f32_chunk(&invalid_dir, session_id, 0, &[0.1, 0.2]);
            record_spool_chunk(&invalid, 0, 0.0, 0.000125, "written").unwrap();
            set_spool_created_at_for_test(&invalid, created_at_ms);
            assert_spool_mtime_is_recent(&invalid);
            assert!(recover_spool(&invalid_dir, u64::MAX).unwrap().is_empty());
            assert!(
                !invalid.exists(),
                "untrustworthy creation timestamps expire fail-closed: {session_id}"
            );
            let _ = std::fs::remove_dir_all(invalid_dir);
        }

        // A schema-one manifest from a build predating the required immutable
        // creation timestamp cannot establish an age, so preserve the existing
        // cleanup behavior for timestamp-less legacy manifests.
        let legacy_dir = temp_dir("legacy-without-creation-time");
        let legacy = legacy_dir.join("spool/legacy-without-creation-time");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join("manifest.json"),
            r#"{"schema_version":1,"session_id":"legacy-without-creation-time","status":"active","chunks":[]}"#,
        )
        .unwrap();
        assert!(recover_spool(&legacy_dir, KEEP_DAYS).unwrap().is_empty());
        assert!(!legacy.exists());

        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(legacy_dir);
    }

    #[test]
    fn chunker_spool_recovery_reconstructs_exact_overlapping_sample_coverage() {
        const SAMPLE_RATE: usize = 16_000;
        const SOURCE_LEN: usize = 50 * SAMPLE_RATE;
        let dir = temp_dir("exact-chunk-recovery");
        let session_id = "exact-chunks";
        let session = begin_spool_session(&dir, session_id).unwrap();

        let mut source = (0..SOURCE_LEN)
            .map(|index| ((index % 997) as f32 / 997.0 - 0.5) * 0.008)
            .collect::<Vec<_>>();
        let config = crate::chunker::ChunkerConfig {
            chunk_length_secs: 15,
        };
        let mut probe = crate::chunker::Chunker::new(config);
        let mut planned = probe.push(&source);
        planned.extend(probe.finish());
        assert!(planned.len() >= 2);
        let boundary_sample = planned[1].source_start_sample + 100;
        source[boundary_sample] = 0.008;

        let mut chunker = crate::chunker::Chunker::new(config);
        let mut chunks = chunker.push(&source);
        chunks.extend(chunker.finish());
        assert!(chunks.len() >= 2);
        assert_eq!(
            chunks[1].source_start_sample,
            planned[1].source_start_sample
        );
        assert!(chunks[1].source_start_sample <= boundary_sample);
        assert!(boundary_sample < chunks[0].source_start_sample + chunks[0].samples.len());

        for chunk in &chunks {
            write_spool_f32_chunk(&dir, session_id, chunk.index, &chunk.samples);
            record_spool_chunk_with_samples(
                &session,
                chunk.index,
                chunk.start_secs,
                chunk.end_secs,
                "written",
                chunk.source_start_sample as u64,
                chunk.samples.len() as u64,
            )
            .unwrap();
        }
        // The older status API is also used as chunks progress; it must not
        // erase the exact interval required for recovery.
        record_spool_chunk(
            &session,
            chunks[0].index,
            chunks[0].start_secs,
            chunks[0].end_secs,
            "transcribed",
        )
        .unwrap();
        let manifest: SpoolManifest =
            serde_json::from_slice(&std::fs::read(session.join("manifest.json")).unwrap()).unwrap();
        let first = manifest
            .chunks
            .iter()
            .find(|chunk| chunk.index == chunks[0].index)
            .unwrap();
        assert_eq!(first.source_start_sample, Some(0));
        assert_eq!(first.sample_count, Some(chunks[0].samples.len() as u64));

        let recovered = rebuild_spool_recovery(&session).unwrap();
        assert_eq!(
            recovered.duration_secs,
            SOURCE_LEN as f64 / SAMPLE_RATE as f64
        );
        let expected = source
            .iter()
            .map(|sample| (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .collect::<Vec<_>>();
        let recovered_samples = decode_wav_samples(&recovered.audio_path);
        assert_eq!(recovered_samples.len(), SOURCE_LEN);
        assert_eq!(recovered_samples, expected);
        let marker = expected[boundary_sample];
        assert_eq!(
            recovered_samples
                .iter()
                .filter(|sample| **sample == marker)
                .count(),
            1
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn legacy_nonoverlapping_spool_chunks_still_concatenate_in_index_order() {
        let dir = temp_dir("legacy-nonoverlap-recovery");
        let session = begin_spool_session(&dir, "legacy-chunks").unwrap();
        let first = [0.1_f32, 0.2];
        let second = [-0.3_f32, 0.4];
        write_spool_f32_chunk(&dir, "legacy-chunks", 0, &first);
        write_spool_f32_chunk(&dir, "legacy-chunks", 1, &second);
        record_spool_chunk(&session, 0, 0.0, 0.000125, "written").unwrap();
        record_spool_chunk(&session, 1, 0.000125, 0.00025, "written").unwrap();

        let recovered = recover_spool(&dir, 7).unwrap();
        assert_eq!(recovered.len(), 1);
        let expected = first
            .iter()
            .chain(&second)
            .map(|sample| (*sample * i16::MAX as f32) as i16)
            .collect::<Vec<_>>();
        assert_eq!(decode_wav_samples(&recovered[0].audio_path), expected);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn exact_spool_intervals_reject_missing_gaps_and_conflicting_samples() {
        let dir = temp_dir("invalid-exact-recovery");

        let missing = begin_spool_session(&dir, "missing").unwrap();
        write_spool_f32_chunk(&dir, "missing", 0, &[0.1, 0.2]);
        write_spool_f32_chunk(&dir, "missing", 1, &[0.3, 0.4]);
        record_spool_chunk_with_samples(&missing, 0, 0.0, 0.000125, "written", 0, 2).unwrap();
        record_spool_chunk(&missing, 1, 0.000125, 0.00025, "written").unwrap();
        let error = rebuild_spool_recovery(&missing).unwrap_err().to_string();
        assert!(error.contains("intervals are missing"), "{error}");

        let unmatched = begin_spool_session(&dir, "unmatched-exact").unwrap();
        record_spool_chunk_with_samples(&unmatched, 0, 0.0, 0.000125, "written", 0, 2).unwrap();
        write_spool_f32_chunk(&dir, "unmatched-exact", 1, &[0.3, 0.4]);
        let error = rebuild_spool_recovery(&unmatched).unwrap_err().to_string();
        assert!(error.contains("intervals are missing"), "{error}");

        let gap = begin_spool_session(&dir, "gap").unwrap();
        write_spool_f32_chunk(&dir, "gap", 0, &[0.1, 0.2]);
        write_spool_f32_chunk(&dir, "gap", 1, &[0.3, 0.4]);
        record_spool_chunk_with_samples(&gap, 0, 0.0, 0.000125, "written", 0, 2).unwrap();
        record_spool_chunk_with_samples(&gap, 1, 0.0001875, 0.0003125, "written", 3, 2).unwrap();
        let error = rebuild_spool_recovery(&gap).unwrap_err().to_string();
        assert!(error.contains("gap in source samples"), "{error}");

        let conflict = begin_spool_session(&dir, "conflict").unwrap();
        write_spool_f32_chunk(&dir, "conflict", 0, &[0.1, 0.2, 0.3]);
        write_spool_f32_chunk(&dir, "conflict", 1, &[0.9, 0.4]);
        record_spool_chunk_with_samples(&conflict, 0, 0.0, 0.0001875, "written", 0, 3).unwrap();
        record_spool_chunk_with_samples(&conflict, 1, 0.000125, 0.00025, "written", 2, 2).unwrap();
        let error = rebuild_spool_recovery(&conflict).unwrap_err().to_string();
        assert!(error.contains("conflicting samples"), "{error}");

        assert!(record_spool_chunk_with_samples(&conflict, 2, 0.0, 0.0, "written", 0, 0).is_err());
        assert!(
            record_spool_chunk_with_samples(&conflict, 2, 0.0, 0.0, "written", u64::MAX, 1,)
                .is_err()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn legacy_overlapping_chunks_without_sample_intervals_fail_truthfully() {
        let dir = temp_dir("legacy-overlap-recovery");
        let session = begin_spool_session(&dir, "legacy-overlap").unwrap();
        write_spool_f32_chunk(&dir, "legacy-overlap", 0, &[0.1, 0.2]);
        write_spool_f32_chunk(&dir, "legacy-overlap", 1, &[0.2, 0.3]);
        record_spool_chunk(&session, 0, 0.0, 1.0, "written").unwrap();
        record_spool_chunk(&session, 1, 0.5, 1.5, "written").unwrap();

        let error = rebuild_spool_recovery(&session).unwrap_err().to_string();
        assert!(error.contains("overlap but lack exact source sample intervals"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unreconstructable_recovery_keeps_original_chunks_until_retention() {
        let dir = temp_dir("unreconstructable-recovery-retained");

        let legacy = begin_spool_session(&dir, "legacy-overlap-retained").unwrap();
        let legacy_first = [0.1_f32, 0.2];
        let legacy_second = [0.2_f32, 0.3];
        write_spool_f32_chunk(&dir, "legacy-overlap-retained", 0, &legacy_first);
        write_spool_f32_chunk(&dir, "legacy-overlap-retained", 1, &legacy_second);
        record_spool_chunk(&legacy, 0, 0.0, 1.0, "written").unwrap();
        record_spool_chunk(&legacy, 1, 0.5, 1.5, "written").unwrap();
        let legacy_first_bytes = read_spool_file(&legacy.join("chunks/00000000.f32")).unwrap();
        let legacy_second_bytes = read_spool_file(&legacy.join("chunks/00000001.f32")).unwrap();
        let legacy_manifest_before: SpoolManifest =
            serde_json::from_slice(&std::fs::read(legacy.join("manifest.json")).unwrap()).unwrap();

        let conflict = begin_spool_session(&dir, "conflict-retained").unwrap();
        let conflict_first = [0.1_f32, 0.2, 0.3];
        let conflict_second = [0.9_f32, 0.4];
        write_spool_f32_chunk(&dir, "conflict-retained", 0, &conflict_first);
        write_spool_f32_chunk(&dir, "conflict-retained", 1, &conflict_second);
        record_spool_chunk_with_samples(&conflict, 0, 0.0, 0.0001875, "written", 0, 3).unwrap();
        record_spool_chunk_with_samples(&conflict, 1, 0.000125, 0.00025, "written", 2, 2).unwrap();
        let conflict_first_bytes = read_spool_file(&conflict.join("chunks/00000000.f32")).unwrap();
        let conflict_second_bytes = read_spool_file(&conflict.join("chunks/00000001.f32")).unwrap();
        let conflict_manifest_before: SpoolManifest =
            serde_json::from_slice(&std::fs::read(conflict.join("manifest.json")).unwrap())
                .unwrap();

        let recovered = recover_spool(&dir, 7).unwrap();
        assert!(
            recovered.is_empty(),
            "unreconstructable audio must not be reported recovered"
        );

        for (session, expected, second_expected, manifest_before) in [
            (
                legacy,
                legacy_first_bytes,
                legacy_second_bytes,
                legacy_manifest_before,
            ),
            (
                conflict,
                conflict_first_bytes,
                conflict_second_bytes,
                conflict_manifest_before,
            ),
        ] {
            assert!(session.is_dir());
            assert!(!session.join("recovery.wav").exists());
            assert_eq!(
                read_spool_file(&session.join("chunks/00000000.f32")).unwrap(),
                expected
            );
            assert_eq!(
                read_spool_file(&session.join("chunks/00000001.f32")).unwrap(),
                second_expected
            );
            let manifest_after: SpoolManifest =
                serde_json::from_slice(&std::fs::read(session.join("manifest.json")).unwrap())
                    .unwrap();
            assert_eq!(manifest_after.status, "abandoned");
            assert_eq!(manifest_after.chunks.len(), manifest_before.chunks.len());
            for (before, after) in manifest_before.chunks.iter().zip(&manifest_after.chunks) {
                assert_eq!(before.index, after.index);
                assert_eq!(before.start_secs, after.start_secs);
                assert_eq!(before.end_secs, after.end_secs);
                assert_eq!(before.source_start_sample, after.source_start_sample);
                assert_eq!(before.sample_count, after.sample_count);
                assert_eq!(before.status, after.status);
            }
        }
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
    fn history_exports_original_asr_separately_and_preserves_null_for_legacy_rows() {
        let dir = temp_dir("history-asr-text");
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_asr_and_delivery_and_spool_and_cleanup(
            &dir,
            "prepared and dictionary-replaced",
            Some("uh, raw provider output"),
            "cleaned final",
            1.0,
            false,
            None,
            "ok",
            "history",
            None,
            &context,
            None,
            "openai:whisper-1",
            "ai_success",
        )
        .unwrap();

        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.raw_text, "prepared and dictionary-replaced");
        assert_eq!(item.asr_text.as_deref(), Some("uh, raw provider output"));
        assert_eq!(item.engine.as_deref(), Some("openai:whisper-1"));
        assert_eq!(
            get_history_page(&dir, 10, None, Some("raw provider"))
                .unwrap()
                .items
                .len(),
            1
        );
        let export: serde_json::Value =
            serde_json::from_str(&export_history_json(&dir).unwrap()).unwrap();
        assert_eq!(export[0]["asr_text"], "uh, raw provider output");
        assert_eq!(export[0]["engine"], "openai:whisper-1");

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
            Some("retry ASR before local filters"),
            Some("deepgram:nova-3"),
            "final",
            false,
            None,
            Some("ai_success"),
        )
        .unwrap();
        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.status, "copied");
        assert_eq!(item.raw_text, "retried raw");
        assert_eq!(
            item.asr_text.as_deref(),
            Some("retry ASR before local filters")
        );
        assert_eq!(item.final_text, "final");
        assert_eq!(item.cleanup_status, "ai_success");
        assert_eq!(item.engine.as_deref(), Some("deepgram:nova-3"));
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
            onboarded: true,
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
        assert_eq!(
            settings.provider_secret(crate::engine::EngineProvider::Groq),
            "gsk_keep"
        );
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
            bind_asr_key_to_host("", "https://api.groq.com/openai/v1", "", "old-key"),
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
        let backend = FakeCredentialBackend::default();
        let settings = Settings {
            asr_api_key: "asr_secret_should_not_be_on_disk".into(),
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            ..Settings::default()
        };
        save_settings_with_backend(&dir, &settings, &backend).unwrap();
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
        let backend = FakeCredentialBackend::default();
        let settings = Settings {
            cleanup_provider: crate::engine::EngineProvider::Custom,
            cleanup_base_url: "https://api.openai.com/v1".into(),
            cleanup_model: "gpt-4o-mini".into(),
            cleanup_api_key: "cleanup_secret_should_not_be_on_disk".into(),
            ..Settings::default()
        };
        save_settings_with_backend(&dir, &settings, &backend).unwrap();
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

        let second = get_history_page(&dir, 1, Some(first.items[0].id), Some("VoiceFlow")).unwrap();
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
        write_gold_file(&dir, "kept.wav", b"gold").unwrap();

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
        assert!(!dir.join("gold").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clear_all_data_does_not_delete_on_device_models() {
        let dir = temp_dir("clear-keeps-models");
        insert_history(&dir, "raw", "final", 1.0, false).unwrap();
        let models = dir.join("models").join("sensevoice-small");
        std::fs::create_dir_all(&models).unwrap();
        std::fs::write(models.join("model.int8.onnx"), b"onnx").unwrap();
        std::fs::write(models.join("tokens.txt"), b"tokens").unwrap();
        let sidecar = dir.join("models").join("sensevoice-small.archive.sha256");
        std::fs::write(&sidecar, crate::ondevice_models::SENSEVOICE_ARCHIVE_SHA256).unwrap();

        clear_all_data(&dir).unwrap();

        assert_eq!(
            std::fs::read(models.join("model.int8.onnx")).unwrap(),
            b"onnx"
        );
        assert_eq!(std::fs::read(models.join("tokens.txt")).unwrap(), b"tokens");
        assert_eq!(
            std::fs::read_to_string(&sidecar).unwrap(),
            crate::ondevice_models::SENSEVOICE_ARCHIVE_SHA256
        );
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
        assert_eq!(
            upsert_learn_pair(&dir, &key, "知呼", "知乎")
                .unwrap()
                .unwrap()
                .hits,
            1
        );
        upsert_learn_pair(&dir, &key, "知呼", "知乎").unwrap();
        let third = upsert_learn_pair(&dir, &key, "知呼", "知乎")
            .unwrap()
            .unwrap();
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
    fn learned_usage_deduplicates_processing_passes_hides_forgotten_terms_and_clears() {
        let dir = temp_dir("learned-usage");
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        ensure_learn_pair_promoted(&dir, &key, "知呼", "知乎", None).unwrap();
        assert!(list_learned_term_usage(&dir).unwrap().is_empty());
        record_learned_term_usage(&dir, &["知乎".into(), "知乎".into()]).unwrap();
        record_learned_term_usage(&dir, &["知乎".into()]).unwrap();
        let usage = list_learned_term_usage(&dir).unwrap();
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].word, "知乎");
        assert_eq!(usage[0].replacement_runs, 2);
        assert!(!usage[0].last_replaced_at.is_empty());
        tombstone_learn_pair(&dir, &key).unwrap();
        assert!(list_learned_term_usage(&dir).unwrap().is_empty());
        clear_all_data(&dir).unwrap();
        let count: u64 = open_history(&dir)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM learned_term_usage", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_v12_migration_preserves_learning_without_inventing_usage() {
        let dir = temp_dir("learned-usage-migration");
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        ensure_learn_pair_promoted(&dir, &key, "知呼", "知乎", None).unwrap();
        let connection = open_history(&dir).unwrap();
        connection
            .execute_batch("DROP TABLE learned_term_usage; PRAGMA user_version = 12;")
            .unwrap();
        drop(connection);
        assert!(list_learned_term_usage(&dir).unwrap().is_empty());
        assert!(list_learn_pairs(&dir).unwrap()[0].is_live_promoted());
        assert!(dir.join("history.sqlite.v12.bak").is_file());
        clear_all_data(&dir).unwrap();
        assert!(!dir.join("history.sqlite.v12.bak").exists());
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
        assert_eq!(get_learn_pair(&dir, &key).unwrap().unwrap().hits, 8);
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
        assert!(upsert_learn_pair(&dir, &key, "知呼", "知乎")
            .unwrap()
            .is_none());
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
        assert_eq!(
            scene.native_bundle.as_deref(),
            Some("com.tencent.xinWeChat")
        );
        assert!(history_context(&dir, id).unwrap().is_some());
        let key = crate::dictionary_learn::pair_key("派森", "Python");
        ensure_learn_pair_promoted(&dir, &key, "派森", "Python", Some(&scene.learn_scope()))
            .unwrap();
        let row = get_learn_pair(&dir, &key).unwrap().unwrap();
        assert!(row.is_live_promoted());
        assert_eq!(row.family.as_deref(), Some("personal_chat"));
        assert_eq!(row.native_bundle.as_deref(), Some("com.tencent.xinWeChat"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_and_revision_policies_keep_only_safe_metadata() {
        let dir = temp_dir("history-context-source-safety");
        let mut snapshot = crate::context::ContextSnapshot::general();
        snapshot.policy.style_examples_approved = true;
        snapshot.policy.style_example_input = Some("private style input".into());
        snapshot.policy.style_example_output = Some("private style output".into());
        snapshot.policy.style_example_pairs = vec![crate::context::StyleExamplePair {
            input: "private pair input".into(),
            output: "private pair output".into(),
        }];
        snapshot
            .evidence
            .items
            .push(crate::screen_text::ContextEvidenceItem {
                source: crate::screen_text::ContextEvidenceSource::Ax,
                kind: crate::screen_text::ContextEvidenceKind::NearbyText,
                value: "private live window excerpt".into(),
                confidence_milli: Some(900),
                truncated: false,
            });

        insert_history_with_context(&dir, "spoken", "final", 1.0, false, &snapshot).unwrap();
        let stored = history_scene(&dir, 1).unwrap().policy.unwrap();
        assert!(!stored.style_examples_approved);
        assert!(stored.style_example_input.is_none());
        assert!(stored.style_example_output.is_none());
        assert!(stored.style_example_pairs.is_empty());

        save_history_revision(
            &dir,
            1,
            "revised",
            None,
            None,
            None,
            Some(&snapshot.policy),
            "manual_edit",
        )
        .unwrap();
        let revision = get_history_revisions(&dir, 1).unwrap().remove(0);
        let serialized = revision.context_policy.unwrap().to_string();
        assert!(!serialized.contains("private style"));
        assert!(!serialized.contains("private pair"));
        assert!(!serialized.contains("private live window"));
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

    #[test]
    fn success_gold_audio_is_playable_but_not_retryable() {
        let dir = temp_dir("gold-success");
        let path = write_gold_file(&dir, "12.wav", b"RIFF").unwrap();
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw spoken",
            "Cleaned text.",
            1.0,
            false,
            None,
            "ok",
            "paste",
            None,
            &context,
            Some(&path),
        )
        .unwrap();
        let item = get_history(&dir, 1).unwrap().remove(0);
        assert!(item.has_audio);
        assert!(!item.retryable);
        assert!(!item.verbatim_reviewed);
        assert_eq!(item.verbatim_text, None);
        assert!(failed_spool(&dir, item.id).unwrap().is_none());
        assert_eq!(history_audio_bytes(&dir, item.id).unwrap(), b"RIFF");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn write_gold_file_rejects_parent_dir_escape() {
        let dir = temp_dir("gold-escape");
        assert!(write_gold_file(&dir, "../escape.wav", b"nope").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_verbatim_marks_reviewed_and_can_unreview() {
        let dir = temp_dir("verbatim-review");
        insert_history(&dir, "draft words", "Cleaned.", 1.0, false).unwrap();
        let id = get_history(&dir, 1).unwrap()[0].id;
        save_verbatim(&dir, id, "  mouth words  ", true).unwrap();
        let item = get_history(&dir, 1).unwrap().remove(0);
        assert_eq!(item.verbatim_text.as_deref(), Some("mouth words"));
        assert!(item.verbatim_reviewed);
        save_verbatim(&dir, id, "", false).unwrap();
        let item = get_history(&dir, 1).unwrap().remove(0);
        assert_eq!(item.verbatim_text, None);
        assert!(!item.verbatim_reviewed);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn purge_and_delete_remove_gold_files() {
        let dir = temp_dir("gold-purge");
        let path = write_gold_file(&dir, "kept.wav", b"gold").unwrap();
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw",
            "final",
            1.0,
            false,
            None,
            "ok",
            "paste",
            None,
            &context,
            Some(&path),
        )
        .unwrap();
        let id = get_history(&dir, 1).unwrap()[0].id;
        assert_eq!(purge_gold_audio(&dir, 0).unwrap(), 1);
        assert!(!path.exists());
        assert!(!get_history(&dir, 1).unwrap()[0].has_audio);

        let path = write_gold_file(&dir, "again.wav", b"gold").unwrap();
        let c = open_history(&dir).unwrap();
        c.execute(
            "UPDATE dictations SET raw_audio_path=? WHERE id=?",
            params![path.to_string_lossy(), id],
        )
        .unwrap();
        drop(c);
        delete_history(&dir, id).unwrap();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn export_gold_corpus_exports_gold_audio_with_raw_text() {
        let dir = temp_dir("gold-export");
        let downloads = dir.join("downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        let gold = write_gold_file(&dir, "kept.wav", b"RIFFGOLD").unwrap();
        let spool = write_spool_file(&dir, Path::new("failed-short.wav"), b"SPOOL").unwrap();
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "spoken words",
            "Cleaned words.",
            1.0,
            false,
            None,
            "ok",
            "paste",
            None,
            &context,
            Some(&gold),
        )
        .unwrap();
        insert_history_with_delivery_and_spool(
            &dir,
            "failed raw",
            "",
            1.0,
            false,
            Some("asr_failed"),
            "failed",
            "none",
            None,
            &context,
            Some(&spool),
        )
        .unwrap();

        let exported = export_gold_corpus(&dir, &downloads, "zh").unwrap();
        assert_eq!(exported.count, 1);
        let wav = std::fs::read(Path::new(&exported.directory).join("utt0001.wav")).unwrap();
        assert_eq!(wav, b"RIFFGOLD");
        let jsonl = std::fs::read_to_string(&exported.jsonl_path).unwrap();
        assert!(jsonl.contains("language Chinese<asr_text>spoken words"));
        assert!(!jsonl.contains("Cleaned words"));
        assert!(!jsonl.contains("failed raw"));
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod handy_settings_tests {
    use super::*;
    #[test]
    fn translation_shortcut_defaults_off_and_round_trips_in_public_view() {
        let mut settings: Settings = serde_json::from_str("{}").unwrap();
        assert!(settings.translation_hotkey.is_empty());
        settings.translation_hotkey = "Command+Shift+T".into();
        settings.translation_target_language = "ja".into();
        settings.normalize();
        let restored: Settings =
            serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
        assert_eq!(restored.translation_hotkey, "Command+Shift+T");
        let view = serde_json::to_value(SettingsView::from(&restored)).unwrap();
        assert_eq!(view["translation_hotkey"], "Command+Shift+T");
        assert_eq!(view["translation_target_language"], "ja");
    }

    #[test]
    fn translation_shortcut_rejects_alias_conflicts_and_accepts_fn() {
        for field in ["hotkey", "selected", "screen", "verbatim"] {
            let mut settings = Settings {
                translation_hotkey: "Cmd+Shift+T".into(),
                ..Settings::default()
            };
            match field {
                "hotkey" => settings.hotkey = "Command+Shift+T".into(),
                "selected" => settings.selected_action_hotkey = "Command+Shift+T".into(),
                "screen" => settings.screen_action_hotkey = "Command+Shift+T".into(),
                _ => settings.verbatim_hotkey = "Command+Shift+T".into(),
            }
            assert!(settings
                .validate()
                .unwrap_err()
                .to_string()
                .contains("hotkey conflicts"));
        }
        let settings = Settings {
            translation_hotkey: "Fn".into(),
            ..Settings::default()
        };
        settings.validate().unwrap();
        let mut fn_conflict = settings.clone();
        fn_conflict.hotkey = "Globe".into();
        assert!(fn_conflict.validate().is_err());
    }
    #[test]
    fn missing_legacy_mode_deserializes_as_tap() {
        let legacy: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.activation_mode, "tap");
    }
    #[test]
    fn conservative_settings_round_trip_and_clamp() {
        let mut settings = Settings {
            extra_recording_buffer_ms: 9000,
            audio_feedback_volume: f32::NAN,
            verbatim_hotkey: "Cmd+Shift+V".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.extra_recording_buffer_ms, 2000);
        assert_eq!(settings.audio_feedback_volume, 0.6);
        assert!(
            !settings.vad_enabled
                && !settings.audio_feedback_enabled
                && !settings.fuzzy_dictionary_enabled
                && !settings.always_on_microphone
                && !settings.autostart_enabled
                && !settings.debug_mode
        );
        let encoded = serde_json::to_value(&settings).unwrap();
        let restored: Settings = serde_json::from_value(encoded).unwrap();
        assert_eq!(restored.verbatim_hotkey, settings.verbatim_hotkey);
        let view = serde_json::to_value(SettingsView::from(&restored)).unwrap();
        for key in [
            "verbatim_hotkey",
            "extra_recording_buffer_ms",
            "audio_feedback_enabled",
            "audio_feedback_volume",
            "vad_enabled",
            "fuzzy_dictionary_enabled",
            "always_on_microphone",
            "clamshell_microphone",
            "autostart_enabled",
            "whats_new_last_seen_version",
            "debug_mode",
        ] {
            assert!(view.get(key).is_some(), "{key}");
        }
    }
}
