#[allow(unused_imports)]
use super::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const DICTIONARY_SOURCES_VERSION: u8 = 1;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum DictionarySource {
    Manual,
    LegacyUnknown,
    Rule(String),
    GlobalRule(String),
    Screen(String),
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DictionarySources {
    pub(super) version: u8,
    pub(super) owners:
        std::collections::BTreeMap<String, std::collections::BTreeSet<DictionarySource>>,
}

impl Default for DictionarySources {
    fn default() -> Self {
        Self {
            version: DICTIONARY_SOURCES_VERSION,
            owners: std::collections::BTreeMap::new(),
        }
    }
}

pub fn legacy_dictionary_sources() -> DictionarySources {
    DictionarySources {
        version: 0,
        owners: std::collections::BTreeMap::new(),
    }
}

pub fn dictionary_words_equal(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

pub fn dictionary_contains_word(dictionary: &[String], word: &str) -> bool {
    dictionary
        .iter()
        .any(|candidate| dictionary_words_equal(candidate, word))
}

pub fn truncate_dictionary_word(word: &str) -> String {
    let mut end = word.len().min(256);
    while !word.is_char_boundary(end) {
        end -= 1;
    }
    word[..end].to_owned()
}

pub fn normalized_dictionary_sources(
    dictionary: &mut Vec<String>,
    sources: &mut DictionarySources,
) {
    let migrate_legacy = sources.version < DICTIONARY_SOURCES_VERSION;
    let old_dictionary = std::mem::take(dictionary);
    let old_owners = std::mem::take(&mut sources.owners);
    let mut canonical_words = Vec::with_capacity(256);
    for word in old_dictionary
        .iter()
        .map(|word| truncate_dictionary_word(word))
        .chain(old_owners.keys().map(|word| truncate_dictionary_word(word)))
    {
        if canonical_words.len() == 256 {
            break;
        }
        if !canonical_words
            .iter()
            .any(|known: &String| dictionary_words_equal(known, &word))
        {
            canonical_words.push(word);
        }
    }

    let mut owners =
        std::collections::BTreeMap::<String, std::collections::BTreeSet<DictionarySource>>::new();
    for (word, word_sources) in old_owners {
        let truncated = truncate_dictionary_word(&word);
        let Some(canonical) = canonical_words
            .iter()
            .find(|known| dictionary_words_equal(known, &truncated))
        else {
            continue;
        };
        owners
            .entry(canonical.clone())
            .or_default()
            .extend(word_sources);
    }
    for word in old_dictionary {
        let truncated = truncate_dictionary_word(&word);
        let Some(canonical) = canonical_words
            .iter()
            .find(|known| dictionary_words_equal(known, &truncated))
        else {
            continue;
        };
        let has_owner = owners
            .get(canonical)
            .is_some_and(|word_sources| !word_sources.is_empty());
        if migrate_legacy || !has_owner {
            owners
                .entry(canonical.clone())
                .or_default()
                .insert(DictionarySource::LegacyUnknown);
        }
    }
    owners.retain(|_, word_sources| !word_sources.is_empty());
    for word in &canonical_words {
        if owners.contains_key(word) {
            dictionary.push(word.clone());
        }
    }
    sources.owners = owners;
    sources.version = DICTIONARY_SOURCES_VERSION;
}

pub fn dictionary_sources_need_normalization(settings: &Settings) -> bool {
    let mut dictionary = settings.dictionary.clone();
    let mut sources = settings.dictionary_sources.clone();
    normalized_dictionary_sources(&mut dictionary, &mut sources);
    dictionary != settings.dictionary || sources != settings.dictionary_sources
}

pub fn current_settings_schema_version() -> u32 {
    SETTINGS_SCHEMA_VERSION
}

/// Wire value and model ID of the cleanup route retired in schema 27.
pub const RETIRED_LMSTUDIO_CLEANUP_PROVIDER: &str = "lmstudio";
pub const RETIRED_LMSTUDIO_CLEANUP_MODEL: &str = "qwen/qwen3.8-27b@6bit";

/// A saved LM Studio selection reads as the bundled MLX route, which runs the
/// same Qwen 3.8 27B MLX 6-bit model locally; `normalize` repairs its model ID.
pub fn deserialize_cleanup_provider<'de, D>(
    deserializer: D,
) -> Result<crate::engine::EngineProvider, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value == RETIRED_LMSTUDIO_CLEANUP_PROVIDER {
        return Ok(crate::engine::EngineProvider::OnDeviceMlx);
    }
    crate::engine::EngineProvider::deserialize(
        serde::de::value::StringDeserializer::<D::Error>::new(value),
    )
}

pub fn default_writing_modes() -> Vec<crate::context::WritingMode> {
    crate::context::builtin_writing_modes()
}

pub fn default_theme() -> String {
    "system".into()
}

pub fn default_ui_language() -> String {
    "system".into()
}

/// Teaching acknowledgement is not feature readiness or a permission grant.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OnDemandGuideId {
    DictionaryLearning,
    ContextScreen,
    SelectedTranslation,
    ScenePreferences,
}
impl OnDemandGuideId {
    const ALL: [Self; 4] = [
        Self::DictionaryLearning,
        Self::ContextScreen,
        Self::SelectedTranslation,
        Self::ScenePreferences,
    ];
    fn current_version(self) -> u32 {
        match self {
            Self::DictionaryLearning => 1,
            Self::ContextScreen => 1,
            Self::SelectedTranslation => 1,
            Self::ScenePreferences => 1,
        }
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GuideAcknowledgements {
    pub(super) dictionary_learning: Option<u32>,
    pub(super) context_screen: Option<u32>,
    pub(super) selected_translation: Option<u32>,
    pub(super) scene_preferences: Option<u32>,
}
#[derive(Debug, Serialize)]
pub struct GuideAcknowledgementView {
    pub(super) guide_id: OnDemandGuideId,
    pub(super) current_version: u32,
    pub(super) acknowledged_version: Option<u32>,
}
impl GuideAcknowledgements {
    fn slot(&mut self, id: OnDemandGuideId) -> &mut Option<u32> {
        match id {
            OnDemandGuideId::DictionaryLearning => &mut self.dictionary_learning,
            OnDemandGuideId::ContextScreen => &mut self.context_screen,
            OnDemandGuideId::SelectedTranslation => &mut self.selected_translation,
            OnDemandGuideId::ScenePreferences => &mut self.scene_preferences,
        }
    }
    pub(crate) fn acknowledge(
        &mut self,
        id: OnDemandGuideId,
        version: u32,
    ) -> Result<bool, &'static str> {
        if version != id.current_version() {
            return Err("guide_version_changed");
        }
        let slot = self.slot(id);
        let changed = *slot != Some(version);
        *slot = Some(version);
        Ok(changed)
    }
    pub(crate) fn view(&self) -> Vec<GuideAcknowledgementView> {
        let mut values = self.clone();
        OnDemandGuideId::ALL
            .into_iter()
            .map(|id| GuideAcknowledgementView {
                guide_id: id,
                current_version: id.current_version(),
                acknowledged_version: *values.slot(id),
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
/// Non-secret outcome of the last credential-store resolution. This is not a
/// provider connectivity or credential-validity result.
pub enum CredentialReadState {
    Configured,
    Missing,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomCredentialPurpose {
    Asr,
    Cleanup,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Written only by the explicit acknowledge_guide command.
    pub guide_acknowledgements: GuideAcknowledgements,
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
    #[serde(default)]
    pub preferred_languages: Vec<String>,
    #[serde(default = "default_ui_language")]
    pub ui_language: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_hud_appearance")]
    pub hud_appearance: String,
    #[serde(default = "default_hud_color_variant")]
    pub hud_color_variant: String,
    #[serde(default = "default_hud_color_variant")]
    pub hud_processing_color_variant: String,
    #[serde(default = "default_hud_voice_style")]
    pub hud_voice_style: String,
    #[serde(default = "default_hud_processing_style")]
    pub hud_processing_style: String,
    #[serde(default = "default_hud_voice_intensity")]
    pub hud_voice_intensity: f32,
    pub dictionary: Vec<String>,
    #[serde(default = "legacy_dictionary_sources")]
    pub(crate) dictionary_sources: DictionarySources,
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
    #[serde(default, deserialize_with = "deserialize_cleanup_provider")]
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
    pub custom_cleanup_parameters: crate::custom_cleanup_parameters::Parameters,
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
    /// Separate local-only opt-in for watching edits in the same verified
    /// field immediately after VoiceFlow inserts text. This never grants
    /// screen/context capture permissions.
    #[serde(default)]
    pub local_edit_learning_enabled: bool,
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
    /// Non-secret result of the last secure-store resolution for each fixed
    /// account. This remains process-only and is intentionally not persisted.
    #[serde(skip)]
    pub(crate) credential_read_states: std::collections::BTreeMap<String, CredentialReadState>,
    /// A foreground reread is session-only. Keep it out of later implicit
    /// credential fan-out/persistence until the user explicitly saves a key.
    #[serde(skip)]
    pub(crate) session_only_custom_credential: bool,
    /// The active Custom origin and purposes for which a foreground-only
    /// credential was recovered. This is process-only routing metadata.
    #[serde(skip)]
    pub(crate) session_custom_credential_scope: Option<CustomCredentialRecoveryScope>,
    /// Prevents a revoked foreground-only value from falling through to an
    /// unrelated legacy alias at a later Custom route.
    #[serde(skip)]
    pub(crate) session_custom_credential_rejected: bool,
}

pub fn default_cleanup_intensity() -> String {
    "auto".into()
}

pub fn default_cascade_timeout_ms() -> u64 {
    5000
}

pub fn default_cascade_proper_noun_threshold() -> usize {
    3
}

pub fn default_cleanup_model() -> String {
    crate::llm::MODEL.to_owned()
}

pub fn default_show_tray_icon() -> bool {
    true
}

pub fn default_output_mode() -> String {
    "auto".into()
}

pub fn default_translation_target_language() -> String {
    "en".into()
}

pub fn default_long_output_mode() -> String {
    "paste".into()
}

pub fn default_delivery_policy() -> String {
    crate::delivery::DeliveryPolicy::default_value()
        .as_str()
        .into()
}

pub fn default_hotkey() -> String {
    "CmdOrControl+Alt+Space".into()
}

pub fn default_selected_action_hotkey() -> String {
    "CmdOrControl+Alt+Slash".into()
}

pub fn default_selected_actions_enabled() -> bool {
    true
}

pub fn default_dictionary_learn_enabled() -> bool {
    true
}

pub fn default_legacy_activation_mode() -> String {
    "tap".into()
}

pub fn default_extra_recording_buffer_ms() -> u64 {
    200
}

pub fn default_audio_feedback_volume() -> f32 {
    0.6
}

pub fn default_whats_new_last_seen_version() -> String {
    env!("CARGO_PKG_VERSION").into()
}

pub fn default_input_gain() -> f32 {
    1.0
}

pub fn default_asr_model() -> String {
    crate::asr::MODEL.to_owned()
}

pub fn default_true() -> bool {
    true
}

pub fn default_hud_appearance() -> String {
    "classic".into()
}

pub fn default_hud_color_variant() -> String {
    "colorful".into()
}

pub fn default_hud_voice_style() -> String {
    "ripple".into()
}

pub fn default_hud_processing_style() -> String {
    "flow_line".into()
}

pub fn default_hud_voice_intensity() -> f32 {
    1.0
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            guide_acknowledgements: GuideAcknowledgements::default(),
            schema_version: SETTINGS_SCHEMA_VERSION,
            api_key: String::new(),
            asr_api_key: String::new(),
            asr_base_url: String::new(),
            asr_provider: crate::engine::EngineProvider::Groq,
            asr_model: default_asr_model(),
            language: "auto".into(),
            preferred_languages: Vec::new(),
            ui_language: default_ui_language(),
            theme: default_theme(),
            hud_appearance: default_hud_appearance(),
            hud_color_variant: default_hud_color_variant(),
            hud_processing_color_variant: default_hud_color_variant(),
            hud_voice_style: default_hud_voice_style(),
            hud_processing_style: default_hud_processing_style(),
            hud_voice_intensity: default_hud_voice_intensity(),
            dictionary: Vec::new(),
            dictionary_sources: DictionarySources::default(),
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
            custom_cleanup_parameters: Default::default(),
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
            local_edit_learning_enabled: false,
            input_device: String::new(),
            input_gain: default_input_gain(),
            verbatim_hotkey: String::new(),
            translation_hotkey: String::new(),
            extra_recording_buffer_ms: 200,
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
            credential_read_states: std::collections::BTreeMap::new(),
            session_only_custom_credential: false,
            session_custom_credential_scope: None,
            session_custom_credential_rejected: false,
        }
    }
}

impl Settings {
    pub(crate) fn resolved_asr_model(&self) -> String {
        let resolved = crate::asr::resolve_asr_model(&self.asr_model);
        if self.asr_provider.is_groq() {
            if crate::asr::is_groq_asr_model(resolved) {
                resolved.to_owned()
            } else {
                default_asr_model()
            }
        } else if self.asr_provider == crate::engine::EngineProvider::Soniox {
            crate::asr::SONIOX_MODEL.to_owned()
        } else if matches!(
            self.asr_provider,
            crate::engine::EngineProvider::AssemblyAi | crate::engine::EngineProvider::DashScope
        ) || resolved.trim().is_empty()
        {
            self.asr_provider.default_asr_model().to_owned()
        } else {
            resolved.to_owned()
        }
    }

    pub(super) fn normalize_dictionary_sources(&mut self) {
        normalized_dictionary_sources(&mut self.dictionary, &mut self.dictionary_sources);
    }

    pub(crate) fn can_add_dictionary_source(&self, word: &str) -> bool {
        let word = word.trim();
        if word.is_empty() || word.len() > 256 {
            return false;
        }
        if dictionary_contains_word(&self.dictionary, word)
            || self
                .dictionary_sources
                .owners
                .keys()
                .any(|existing| dictionary_words_equal(existing, word))
        {
            return true;
        }
        let mut canonical_terms: Vec<&str> = Vec::new();
        for existing in self
            .dictionary
            .iter()
            .map(String::as_str)
            .chain(self.dictionary_sources.owners.keys().map(String::as_str))
        {
            if !canonical_terms
                .iter()
                .any(|known| dictionary_words_equal(known, existing))
            {
                canonical_terms.push(existing);
            }
        }
        canonical_terms.len() < 256
    }

    pub(crate) fn add_dictionary_source(&mut self, word: &str, source: DictionarySource) -> bool {
        let word = word.trim().to_owned();
        if word.is_empty() || word.len() > 256 {
            return false;
        }
        self.normalize_dictionary_sources();
        if !self.can_add_dictionary_source(&word) {
            return false;
        }
        let word = self
            .dictionary
            .iter()
            .find(|existing| dictionary_words_equal(existing, &word))
            .cloned()
            .or_else(|| {
                self.dictionary_sources
                    .owners
                    .keys()
                    .find(|existing| dictionary_words_equal(existing, &word))
                    .cloned()
            })
            .unwrap_or(word);
        let inserted = self
            .dictionary_sources
            .owners
            .entry(word.to_owned())
            .or_default()
            .insert(source);
        if inserted && !dictionary_contains_word(&self.dictionary, &word) {
            self.dictionary.push(word);
        }
        inserted
    }

    pub(crate) fn has_dictionary_source(&self, word: &str, source: &DictionarySource) -> bool {
        let word = word.trim();
        self.dictionary_sources
            .owners
            .iter()
            .find(|(existing, _)| dictionary_words_equal(existing, word))
            .map(|(_, owners)| owners)
            .is_some_and(|owners| owners.contains(source))
    }

    /// Whether this word has a global owner that survives removal of any one
    /// promoted word-rule owner. Transfer imports use this to distinguish a
    /// genuinely present global word from a projection that a pair replacement
    /// may remove later in the same transaction.
    pub(crate) fn has_independent_global_dictionary_owner(&self, word: &str) -> bool {
        self.dictionary_sources
            .owners
            .iter()
            .filter(|(existing, _)| dictionary_words_equal(existing, word))
            .flat_map(|(_, owners)| owners.iter())
            .any(|owner| {
                matches!(
                    owner,
                    DictionarySource::Manual
                        | DictionarySource::Screen(_)
                        | DictionarySource::Other(_)
                )
            })
    }

    /// Return dictionary terms whose current ownership permits ASR hint
    /// egress. A scoped or missing learn-pair owner cannot make its canonical
    /// word a global hint; an independent Manual/Screen/Other owner still can.
    /// LegacyUnknown is never sufficient because its original owner cannot be
    /// proved, even when its old pair row has been cleared or cannot be read.
    pub(crate) fn asr_dictionary_words(&self, pairs: &[LearnPairRecord]) -> Vec<String> {
        self.dictionary
            .iter()
            .filter(|word| {
                self.dictionary_sources
                    .owners
                    .iter()
                    .filter(|(existing, _)| dictionary_words_equal(existing, word))
                    .flat_map(|(_, owners)| owners.iter())
                    .any(|owner| match owner {
                        DictionarySource::Manual
                        | DictionarySource::Screen(_)
                        | DictionarySource::Other(_) => true,
                        DictionarySource::Rule(pair_key)
                        | DictionarySource::GlobalRule(pair_key) => pairs.iter().any(|row| {
                            row.pair_key == *pair_key
                                && row.is_live_promoted()
                                && row.is_provider_egress_eligible_dictionary_owner()
                                && dictionary_words_equal(&row.after_surface, word)
                        }),
                        DictionarySource::LegacyUnknown => false,
                    })
            })
            .cloned()
            .collect()
    }

    pub(crate) fn remove_all_rule_dictionary_sources(&mut self) -> bool {
        self.normalize_dictionary_sources();
        let mut changed = false;
        let words: Vec<String> = self.dictionary_sources.owners.keys().cloned().collect();
        for word in words {
            if let Some(owners) = self.dictionary_sources.owners.get_mut(&word) {
                let before = owners.len();
                owners.retain(|owner| {
                    !matches!(
                        owner,
                        DictionarySource::Rule(_) | DictionarySource::GlobalRule(_)
                    )
                });
                if owners.len() != before {
                    changed = true;
                }
                if owners.is_empty() {
                    self.dictionary_sources.owners.remove(&word);
                    self.dictionary
                        .retain(|entry| !dictionary_words_equal(entry, &word));
                }
            }
        }
        changed
    }

    pub(crate) fn remove_dictionary_source(
        &mut self,
        word: &str,
        source: &DictionarySource,
    ) -> bool {
        self.normalize_dictionary_sources();
        let Some(canonical_word) = self
            .dictionary_sources
            .owners
            .keys()
            .find(|existing| dictionary_words_equal(existing, word))
            .cloned()
        else {
            return false;
        };
        let Some(owners) = self.dictionary_sources.owners.get_mut(&canonical_word) else {
            return false;
        };
        let removed = owners.remove(source);
        if owners.is_empty() {
            self.dictionary_sources.owners.remove(&canonical_word);
            self.dictionary.retain(|entry| entry != &canonical_word);
        }
        removed
    }

    pub(crate) fn remove_dictionary_word(&mut self, word: &str) -> bool {
        self.normalize_dictionary_sources();
        let canonical_word = self
            .dictionary_sources
            .owners
            .keys()
            .find(|existing| dictionary_words_equal(existing, word))
            .cloned();
        let removed = canonical_word
            .as_ref()
            .and_then(|canonical| self.dictionary_sources.owners.remove(canonical))
            .is_some();
        let before = self.dictionary.len();
        self.dictionary
            .retain(|entry| !dictionary_words_equal(entry, word));
        removed || self.dictionary.len() != before
    }

    pub(crate) fn reconcile_dictionary_patch(&mut self, requested: Vec<String>) -> Vec<String> {
        let old_terms = self
            .dictionary_sources
            .owners
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut deduplicated = Vec::with_capacity(requested.len().min(256));
        for word in requested {
            let word = truncate_dictionary_word(word.trim());
            if word.is_empty() {
                continue;
            }
            let canonical = old_terms
                .iter()
                .chain(deduplicated.iter())
                .find(|existing| dictionary_words_equal(existing, &word))
                .cloned()
                .unwrap_or(word);
            if !deduplicated
                .iter()
                .any(|existing| dictionary_words_equal(existing, &canonical))
            {
                if deduplicated.len() == 256 {
                    break;
                }
                deduplicated.push(canonical);
            }
        }
        let removed = old_terms
            .iter()
            .filter(|word| {
                !deduplicated
                    .iter()
                    .any(|requested| dictionary_words_equal(requested, word))
            })
            .cloned()
            .collect::<Vec<_>>();
        for word in &removed {
            self.dictionary_sources.owners.remove(word);
        }
        for word in &deduplicated {
            if !old_terms
                .iter()
                .any(|existing| dictionary_words_equal(existing, word))
            {
                self.dictionary_sources
                    .owners
                    .entry(word.clone())
                    .or_default()
                    .insert(DictionarySource::Manual);
            }
        }
        self.dictionary = deduplicated;
        self.normalize_dictionary_sources();
        removed
    }

    pub fn normalize(&mut self) {
        self.normalize_dictionary_sources();
        if self.schema_version < SETTINGS_SCHEMA_VERSION {
            // Before schema 7, a new install was silently stored as Chinese.
            // Preserve an explicit choice for onboarded users, but let an
            // unfinished setup follow the operating system language.
            if self.schema_version < 7 && !self.onboarded && self.ui_language == "zh" {
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
            if self.schema_version < 27
                && self.cleanup_provider == crate::engine::EngineProvider::OnDeviceMlx
                && self.cleanup_model.trim() == RETIRED_LMSTUDIO_CLEANUP_MODEL
            {
                // Retired LM Studio selections, and older MLX selections that
                // stored the LM Studio variant, keep the same model locally.
                self.cleanup_model = crate::modelstore::MODEL_ID.to_owned();
            }
            self.schema_version = SETTINGS_SCHEMA_VERSION;
        }
        self.chunk_threshold_secs = self.chunk_threshold_secs.clamp(5, 3_600);
        self.chunk_length_secs = self.chunk_length_secs.clamp(15, 60);
        self.keep_audio_days = self.keep_audio_days.min(365);
        self.keep_history_days = self.keep_history_days.min(3650);
        self.language = crate::languages::normalize_selected_language(&self.language);
        self.preferred_languages =
            crate::languages::normalize_preferred_languages(&self.preferred_languages);
        if !matches!(self.ui_language.as_str(), "system" | "zh" | "en") {
            self.ui_language = default_ui_language();
        }
        if !matches!(self.theme.as_str(), "system" | "light" | "dark") {
            self.theme = default_theme();
        }
        if !matches!(self.hud_appearance.as_str(), "classic" | "liquid_glass") {
            self.hud_appearance = default_hud_appearance();
        }
        if !matches!(
            self.hud_color_variant.as_str(),
            "colorful" | "ocean" | "sunset" | "forest" | "candy" | "ice" | "gold" | "mono"
        ) {
            self.hud_color_variant = default_hud_color_variant();
        }
        if !matches!(
            self.hud_processing_color_variant.as_str(),
            "colorful" | "ocean" | "sunset" | "forest" | "candy" | "ice" | "gold" | "mono"
        ) {
            self.hud_processing_color_variant = default_hud_color_variant();
        }
        if !matches!(
            self.hud_voice_style.as_str(),
            "ripple" | "aurora" | "prism" | "whisper"
        ) {
            self.hud_voice_style = default_hud_voice_style();
        }
        if !matches!(
            self.hud_processing_style.as_str(),
            "flow_line" | "brilliant_sweep" | "quiet_trace"
        ) {
            self.hud_processing_style = default_hud_processing_style();
        }
        self.hud_voice_intensity = if self.hud_voice_intensity.is_finite() {
            self.hud_voice_intensity.clamp(0.5, 1.5)
        } else {
            default_hud_voice_intensity()
        };
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
        if !matches!(
            self.activation_mode.as_str(),
            "tap" | "hold_to_talk" | "hybrid_hold_lock"
        ) {
            self.activation_mode = "tap".into();
        }

        self.dictionary.truncate(256);
        self.dictionary
            .iter_mut()
            .for_each(|word| word.truncate(256));
        self.context_mappings
            .retain(|mapping| mapping.validate().is_ok());
        for mapping in &mut self.context_mappings {
            if mapping.deleted {
                mapping.scrub_as_deleted_tombstone();
                continue;
            }
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
        // the load-side repair helpers only after keys are confirmed absent.
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
        }
        self.asr_model = self.resolved_asr_model();
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

    fn repair_incomplete_asr(&mut self) -> bool {
        if !self.asr_provider.is_custom() {
            return false;
        }
        let url = if self.custom_base_url.trim().is_empty() {
            self.asr_base_url.trim()
        } else {
            self.custom_base_url.trim()
        };
        let key = self.asr_credential();
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
        let key = self.cleanup_credential();
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
        crate::custom_cleanup_parameters::validate(&self.custom_cleanup_parameters)
            .map_err(|error| anyhow::anyhow!(error))?;
        if crate::languages::canonical_language_code(&self.language) != Some(self.language.as_str())
        {
            anyhow::bail!("unsupported recognition language");
        }
        crate::languages::validate_preferred_languages(&self.preferred_languages)?;
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
        } else if self.cleanup_provider == crate::engine::EngineProvider::OnDeviceMlx {
            if self.cleanup_model != crate::modelstore::MODEL_ID
                && !crate::modelstore::is_supported_mlx_model(&self.cleanup_model)
            {
                anyhow::bail!("unsupported local MLX cleanup model");
            }
        } else if self.cleanup_provider == crate::engine::EngineProvider::OnDeviceLlamaCpp {
            if !crate::gguf_cleanup_store::is_supported_model(&self.cleanup_model) {
                anyhow::bail!("unsupported local cleanup model");
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
        if !matches!(
            self.activation_mode.as_str(),
            "tap" | "hold_to_talk" | "hybrid_hold_lock"
        ) {
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

    pub(super) fn validate_provider_side(
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
        let key = if provider.is_custom() && self.session_only_custom_credential {
            let purpose = if asr {
                CustomCredentialPurpose::Asr
            } else {
                CustomCredentialPurpose::Cleanup
            };
            self.session_custom_credential_for(purpose, &url)
        } else {
            self.provider_secret(provider)
        };
        let empty_ok = matches!(
            provider,
            crate::engine::EngineProvider::OnDeviceMlx
                | crate::engine::EngineProvider::OnDeviceLlamaCpp
        ) || provider.allows_empty_key() && crate::providers::is_loopback_url(&url);
        if key.is_empty() && !empty_ok {
            if !check_readiness || !self.onboarded {
                return Ok(());
            }
            let read_state = if provider.is_custom() {
                self.custom_credential_read_state()
            } else {
                self.credential_read_state_for_slot(provider.keychain_account(), key)
            };
            if read_state == CredentialReadState::Unavailable {
                anyhow::bail!("钥匙串暂时不可读取；当前设置未更改，请在前台设置中重试读取。")
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
        if provider.is_custom()
            && (self.session_custom_credential_rejected || self.session_only_custom_credential)
        {
            return "";
        }
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

    fn credential_read_state_for_slot(
        &self,
        slot: &str,
        runtime_value: &str,
    ) -> CredentialReadState {
        self.credential_read_states
            .get(slot)
            .copied()
            .unwrap_or_else(|| {
                if runtime_value.trim().is_empty() {
                    CredentialReadState::Missing
                } else {
                    CredentialReadState::Configured
                }
            })
    }

    pub fn custom_credential_read_state(&self) -> CredentialReadState {
        let scope = custom_credential_recovery_scope(self, CustomCredentialPurpose::Asr)
            .or_else(|| custom_credential_recovery_scope(self, CustomCredentialPurpose::Cleanup));
        let Some(scope) = scope else {
            return self.credential_read_state_for_slot(
                "provider_custom",
                self.provider_secret(crate::engine::EngineProvider::Custom),
            );
        };

        let mut slots = vec!["provider_custom"];
        if scope.include_asr_legacy {
            slots.push("asr_api_key");
        }
        if scope.include_cleanup_legacy {
            slots.push("cleanup_api_key");
        }
        for slot in slots {
            let runtime_value = match slot {
                "provider_custom" => self
                    .provider_api_keys
                    .get("custom")
                    .map(String::as_str)
                    .unwrap_or_default(),
                "asr_api_key" => self.asr_api_key.as_str(),
                "cleanup_api_key" => self.cleanup_api_key.as_str(),
                _ => "",
            };
            match self.credential_read_state_for_slot(slot, runtime_value) {
                CredentialReadState::Configured => return CredentialReadState::Configured,
                CredentialReadState::Unavailable => return CredentialReadState::Unavailable,
                CredentialReadState::Missing => {}
            }
        }
        CredentialReadState::Missing
    }

    pub fn asr_credential_read_state(&self) -> CredentialReadState {
        if self.asr_provider.is_custom() {
            self.custom_credential_read_state()
        } else {
            self.credential_read_state_for_slot(
                self.asr_provider.keychain_account(),
                self.asr_credential(),
            )
        }
    }

    /// Mark non-empty runtime credentials configured after a successful
    /// settings commit, while preserving unavailable legacy values until a
    /// verified replacement has removed their unverified-source marker.
    pub(crate) fn refresh_verified_credential_read_states(&mut self) {
        let mut values = vec![
            ("groq_api_key", self.api_key.as_str()),
            ("asr_api_key", self.asr_api_key.as_str()),
            ("cleanup_api_key", self.cleanup_api_key.as_str()),
        ];
        for (provider_id, key) in &self.provider_api_keys {
            if let Some(provider) = crate::providers::EngineProvider::parse(provider_id) {
                values.push((provider.keychain_account(), key.as_str()));
            }
        }
        for (slot, value) in values {
            if value.trim().is_empty() || has_unverified_source(self, slot, value) {
                continue;
            }
            self.credential_read_states
                .insert(slot.to_owned(), CredentialReadState::Configured);
        }
    }

    pub(crate) fn preserve_credential_runtime_metadata_from(&mut self, previous: &Settings) {
        self.unverified_credential_sources = previous.unverified_credential_sources.clone();
        self.credential_baselines = previous.credential_baselines.clone();
        self.credential_read_states = previous.credential_read_states.clone();
        self.session_only_custom_credential = previous.session_only_custom_credential;
        self.session_custom_credential_scope = previous.session_custom_credential_scope.clone();
        self.session_custom_credential_rejected = previous.session_custom_credential_rejected;
    }

    fn session_custom_credential_scope_matches(&self) -> bool {
        let Some(captured) = self.session_custom_credential_scope.as_ref() else {
            return false;
        };
        self.provider_api_keys
            .get("custom")
            .is_some_and(|key| !key.trim().is_empty())
            && custom_credential_recovery_scope(self, captured.purpose).as_ref() == Some(captured)
    }

    fn session_custom_credential_matches_base(
        &self,
        purpose: CustomCredentialPurpose,
        base_url: &str,
    ) -> bool {
        let Some(captured) = self.session_custom_credential_scope.as_ref() else {
            return false;
        };
        let purpose_is_active = match purpose {
            CustomCredentialPurpose::Asr => captured.asr_active,
            CustomCredentialPurpose::Cleanup => captured.cleanup_active,
        };
        if !self.session_only_custom_credential || !purpose_is_active {
            return false;
        }
        let Some(current) = custom_credential_recovery_scope(self, purpose) else {
            return false;
        };
        let mut expected = captured.clone();
        expected.purpose = purpose;
        current == expected
            && canonical_http_origin(base_url).as_deref() == Some(captured.origin.as_str())
    }

    /// Return a foreground-only Custom value only to a currently active
    /// ASR/cleanup purpose whose destination still has the captured origin.
    /// Generic Custom consumers must use `provider_secret`, which deliberately
    /// hides session-only values.
    pub(crate) fn session_custom_credential_for(
        &self,
        purpose: CustomCredentialPurpose,
        base_url: &str,
    ) -> &str {
        if !self.session_custom_credential_matches_base(purpose, base_url) {
            return "";
        }
        self.provider_api_keys
            .get("custom")
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("")
    }

    /// Drop a foreground-only Custom value when the active purpose set or
    /// effective service origin no longer matches the read that adopted it.
    /// This only clears process memory and never touches Keychain.
    pub(crate) fn clear_stale_session_custom_credential(&mut self) -> bool {
        if !self.session_only_custom_credential {
            return false;
        }
        if self.session_custom_credential_scope_matches() {
            return false;
        }
        self.clear_session_custom_credential();
        true
    }

    pub(crate) fn clear_session_custom_credential(&mut self) {
        if self.session_only_custom_credential {
            self.provider_api_keys.remove("custom");
            self.credential_baselines.remove("provider_custom");
            self.session_custom_credential_rejected = true;
        }
        self.session_only_custom_credential = false;
        self.session_custom_credential_scope = None;
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
        if self.asr_provider.is_custom() && self.session_only_custom_credential {
            return self.session_custom_credential_for(
                CustomCredentialPurpose::Asr,
                &self.resolved_provider_base(crate::engine::EngineProvider::Custom),
            );
        }
        self.provider_secret(self.asr_provider)
    }

    pub fn accurate_asr_configured(&self) -> bool {
        self.accurate_asr_provider.has_http_asr()
            && !self.accurate_asr_model.trim().is_empty()
            && !self.accurate_asr_credential().trim().is_empty()
    }

    pub fn accurate_asr_credential(&self) -> &str {
        let base = if !self.accurate_asr_base_url.trim().is_empty() {
            self.accurate_asr_base_url.trim().to_owned()
        } else {
            self.resolved_provider_base(self.accurate_asr_provider)
        };
        if self.accurate_asr_provider.is_custom() && self.session_only_custom_credential {
            return self.session_custom_credential_for(CustomCredentialPurpose::Asr, &base);
        }
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
        if !provider.has_vision() {
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
        if !provider.has_vision() {
            return None;
        }
        Some(crate::llm::resolve_chat_url(
            &self.resolved_provider_base(provider),
        ))
    }

    pub fn cleanup_credential(&self) -> &str {
        if self.cleanup_provider.is_custom() && self.session_only_custom_credential {
            return self.session_custom_credential_for(
                CustomCredentialPurpose::Cleanup,
                &self.resolved_provider_base(crate::engine::EngineProvider::Custom),
            );
        }
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

pub fn bind_asr_key_to_host(
    provider: crate::engine::EngineProvider,
    previous_url: &str,
    next_url: &str,
    incoming_key: &str,
    previous_key: &str,
) -> String {
    if !incoming_key.trim().is_empty() {
        return incoming_key.to_string();
    }
    let host_changed = if provider == crate::engine::EngineProvider::Custom {
        crate::llm::custom_connection_origin_changed(previous_url, next_url)
    } else {
        crate::asr::asr_host_changed(previous_url, next_url)
    };
    if host_changed {
        String::new()
    } else {
        previous_key.to_string()
    }
}

pub fn bind_cleanup_key_to_host(
    provider: crate::engine::EngineProvider,
    previous_url: &str,
    next_url: &str,
    incoming_key: &str,
    previous_key: &str,
) -> String {
    if !incoming_key.trim().is_empty() {
        return incoming_key.to_string();
    }
    let host_changed = if provider == crate::engine::EngineProvider::Custom {
        crate::llm::custom_connection_origin_changed(previous_url, next_url)
    } else {
        crate::llm::chat_host_changed(previous_url, next_url)
    };
    if host_changed {
        String::new()
    } else {
        previous_key.to_string()
    }
}

/// Settings exposed to the webview. The backend keeps the real credential in
/// memory/keychain, while the UI only receives whether one is configured and a
/// non-sensitive hint for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomCredentialRecoveryScope {
    pub origin: String,
    pub purpose: CustomCredentialPurpose,
    pub asr_active: bool,
    pub cleanup_active: bool,
    pub include_asr_legacy: bool,
    pub include_cleanup_legacy: bool,
}

pub fn canonical_http_origin(base_url: &str) -> Option<String> {
    let base_url = base_url.trim();
    if base_url.is_empty() {
        return None;
    }
    let candidate = if base_url.contains("://") {
        base_url.to_owned()
    } else {
        format!("https://{base_url}")
    };
    let url = reqwest::Url::parse(&candidate).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

/// Build the only safe legacy-alias set for a foreground Custom reread.
/// The primary Custom account remains first. Legacy ASR/cleanup accounts are
/// eligible only while their own route currently uses Custom, and both active
/// Custom purposes must resolve to the same valid origin.
pub fn custom_credential_recovery_scope(
    settings: &Settings,
    purpose: CustomCredentialPurpose,
) -> Option<CustomCredentialRecoveryScope> {
    let asr_custom_active = settings.asr_provider.is_custom();
    let cleanup_custom_active = settings.cleanup_enabled && settings.cleanup_provider.is_custom();
    let requested_purpose_is_active = match purpose {
        CustomCredentialPurpose::Asr => asr_custom_active,
        CustomCredentialPurpose::Cleanup => cleanup_custom_active,
    };
    if !requested_purpose_is_active {
        return None;
    }

    let origin = canonical_http_origin(
        &settings.resolved_provider_base(crate::engine::EngineProvider::Custom),
    )?;
    // Legacy account names do not identify an origin. Only consult each one
    // when that purpose's saved endpoint independently proves the same origin.
    let include_asr_legacy = asr_custom_active
        && canonical_http_origin(&settings.asr_base_url).as_deref() == Some(origin.as_str());
    let include_cleanup_legacy = cleanup_custom_active
        && canonical_http_origin(&settings.cleanup_base_url).as_deref() == Some(origin.as_str());
    Some(CustomCredentialRecoveryScope {
        origin,
        purpose,
        asr_active: asr_custom_active,
        cleanup_active: cleanup_custom_active,
        include_asr_legacy,
        include_cleanup_legacy,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderKeyView {
    /// A credential string is currently available in this process.
    pub configured: bool,
    pub hint: Option<String>,
    /// Last Keychain resolution, kept separate from current session memory.
    pub keychain_state: CredentialReadState,
}

pub fn credential_hint(key: &str) -> Option<String> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    if key.chars().count() <= 5 {
        return Some("••••".into());
    }
    let tail: String = key.chars().rev().take(5).collect();
    Some(format!("••••{}", tail.chars().rev().collect::<String>()))
}

#[cfg(test)]
pub fn add_manual_dictionary_words(settings: &mut Settings, words: &[String]) -> usize {
    words
        .iter()
        .filter(|word| settings.add_dictionary_source(word, DictionarySource::Manual))
        .count()
}

pub fn reconcile_dictionary_patch(settings: &mut Settings, requested: Vec<String>) -> Vec<String> {
    settings.reconcile_dictionary_patch(requested)
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingsView {
    pub schema_version: u32,
    pub api_key_configured: bool,
    pub api_key_hint: Option<String>,
    pub api_key_keychain_state: CredentialReadState,
    pub asr_api_key_configured: bool,
    pub asr_api_key_hint: Option<String>,
    pub asr_api_key_keychain_state: CredentialReadState,
    pub asr_base_url: String,
    pub asr_provider: crate::engine::EngineProvider,
    pub asr_model: String,
    pub cleanup_model: String,
    pub cleanup_provider: crate::engine::EngineProvider,
    pub cleanup_base_url: String,
    pub cleanup_api_key_configured: bool,
    pub cleanup_api_key_hint: Option<String>,
    pub cleanup_api_key_keychain_state: CredentialReadState,
    pub custom_base_url: String,
    pub custom_asr: bool,
    pub custom_llm: bool,
    pub custom_cleanup_parameters: crate::custom_cleanup_parameters::Parameters,
    pub ollama_base_url: String,
    pub strict_offline_enabled: bool,
    pub local_whisper_base_url: String,
    pub provider_keys: std::collections::BTreeMap<String, ProviderKeyView>,
    pub language: String,
    pub preferred_languages: Vec<String>,
    pub ui_language: String,
    pub theme: String,
    pub hud_appearance: String,
    pub hud_color_variant: String,
    pub hud_processing_color_variant: String,
    pub hud_voice_style: String,
    pub hud_processing_style: String,
    pub hud_voice_intensity: f32,
    pub dictionary: Vec<String>,
    pub hotkey: String,
    pub activation_mode: String,
    pub hotkey_error: Option<String>,
    pub hotkey_notice: Option<String>,
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
    pub local_edit_learning_enabled: bool,
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

pub fn provider_key_views(
    settings: &Settings,
) -> std::collections::BTreeMap<String, ProviderKeyView> {
    crate::providers::EngineProvider::ALL
        .into_iter()
        .map(|provider| {
            let secret = if provider.is_custom() && settings.session_only_custom_credential {
                let asr = settings.asr_credential();
                if !asr.is_empty() {
                    asr
                } else {
                    settings.cleanup_credential()
                }
            } else {
                settings.provider_secret(provider)
            };
            (
                provider.as_str().to_owned(),
                ProviderKeyView {
                    configured: !secret.is_empty(),
                    hint: credential_hint(secret),
                    keychain_state: if provider.is_custom() {
                        settings.custom_credential_read_state()
                    } else {
                        settings.credential_read_state_for_slot(provider.keychain_account(), secret)
                    },
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
            api_key_keychain_state: settings
                .credential_read_state_for_slot("groq_api_key", &settings.api_key),
            asr_api_key_configured: !settings.asr_api_key.trim().is_empty(),
            asr_api_key_hint: credential_hint(&settings.asr_api_key),
            asr_api_key_keychain_state: settings
                .credential_read_state_for_slot("asr_api_key", &settings.asr_api_key),
            asr_base_url: settings.asr_base_url.clone(),
            asr_provider: settings.asr_provider,
            asr_model: crate::asr::resolve_asr_model(&settings.asr_model).to_owned(),
            cleanup_model: settings.cleanup_model.clone(),
            cleanup_provider: settings.cleanup_provider,
            cleanup_base_url: settings.cleanup_base_url.clone(),
            cleanup_api_key_configured: !settings.cleanup_api_key.trim().is_empty(),
            cleanup_api_key_hint: credential_hint(&settings.cleanup_api_key),
            cleanup_api_key_keychain_state: settings
                .credential_read_state_for_slot("cleanup_api_key", &settings.cleanup_api_key),
            custom_base_url: settings.custom_base_url.clone(),
            custom_asr: settings.custom_asr,
            custom_llm: settings.custom_llm,
            custom_cleanup_parameters: settings.custom_cleanup_parameters.clone(),
            ollama_base_url: settings.ollama_base_url.clone(),
            strict_offline_enabled: settings.strict_offline_enabled,
            local_whisper_base_url: settings.local_whisper_base_url.clone(),
            provider_keys: provider_key_views(settings),
            language: settings.language.clone(),
            preferred_languages: settings.preferred_languages.clone(),
            ui_language: settings.ui_language.clone(),
            theme: settings.theme.clone(),
            hud_appearance: settings.hud_appearance.clone(),
            hud_color_variant: settings.hud_color_variant.clone(),
            hud_processing_color_variant: settings.hud_processing_color_variant.clone(),
            hud_voice_style: settings.hud_voice_style.clone(),
            hud_processing_style: settings.hud_processing_style.clone(),
            hud_voice_intensity: settings.hud_voice_intensity,
            dictionary: settings.dictionary.clone(),
            hotkey: settings.hotkey.clone(),
            activation_mode: settings.activation_mode.clone(),
            hotkey_error: crate::hotkey::registration_error(),
            hotkey_notice: crate::hotkey::registration_notice(),
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
            context_mappings: settings
                .context_mappings
                .iter()
                .filter(|mapping| !mapping.deleted)
                .cloned()
                .collect(),
            writing_modes: settings.writing_modes.clone(),
            snippets: settings.snippets.clone(),
            output_mode: settings.output_mode.clone(),
            translation_target_language: settings.translation_target_language.clone(),
            selected_action_hotkey: settings.selected_action_hotkey.clone(),
            selected_actions_enabled: settings.selected_actions_enabled,
            dictionary_learn_enabled: settings.dictionary_learn_enabled,
            local_edit_learning_enabled: settings.local_edit_learning_enabled,
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

pub fn is_safe_secret_sidecar_slot(slot: &str) -> bool {
    !slot.is_empty()
        && slot.len() <= 64
        && slot
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

pub fn secret_sidecar_slot(slot: &str) -> Option<&str> {
    match slot {
        "api_key" | "groq_api_key" => Some("api_key"),
        "asr_api_key" => Some("asr_api_key"),
        "cleanup_api_key" => Some("cleanup_api_key"),
        other if is_safe_secret_sidecar_slot(other) => Some(other),
        _ => None,
    }
}

pub fn secret_sidecar_path(dir: &Path, slot: &str) -> PathBuf {
    let slot = secret_sidecar_slot(slot).unwrap_or("api_key");
    dir.join("secrets").join(slot)
}

#[cfg(test)]
pub fn write_secret_sidecar(dir: &Path, slot: &str, key: &str) -> anyhow::Result<()> {
    let slot = secret_sidecar_slot(slot)
        .ok_or_else(|| anyhow::anyhow!("unsupported secret sidecar slot"))?;
    write_atomic_bytes(&secret_sidecar_path(dir, slot), key.as_bytes())
}

pub fn read_secret_sidecar(dir: &Path, slot: &str) -> Option<String> {
    let bytes = fs::read(secret_sidecar_path(dir, slot)).ok()?;
    let key = String::from_utf8(bytes).ok()?;
    (!key.trim().is_empty()).then_some(key)
}

pub fn clear_secret_sidecar(dir: &Path, slot: &str) {
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

pub const CUSTOM_HOST_CHANGE_CREDENTIAL_SLOTS: [&str; 3] =
    ["provider_custom", "asr_api_key", "cleanup_api_key"];

/// Remove the Custom credential and its legacy aliases before a keyless host
/// change can be persisted. The Custom read API falls back to both legacy
/// slots, so verify it only after every slot has been cleared.
pub fn clear_custom_credentials_before_host_change(dir: &Path) -> anyhow::Result<()> {
    clear_custom_credentials_before_host_change_with_backend(dir, &OsCredentialBackend)
}

pub fn clear_custom_credentials_before_host_change_with_backend(
    dir: &Path,
    backend: &dyn CredentialBackend,
) -> anyhow::Result<()> {
    let mut failure = None;
    for slot in CUSTOM_HOST_CHANGE_CREDENTIAL_SLOTS {
        if let Err(error) = backend.write(slot, "") {
            failure.get_or_insert_with(|| {
                anyhow::anyhow!("credential_storage: failed to clear custom {slot}: {error}")
            });
        }
    }

    for slot in CUSTOM_HOST_CHANGE_CREDENTIAL_SLOTS {
        match backend.read(slot) {
            crate::keychain::ApiKeyState::Missing => {}
            crate::keychain::ApiKeyState::Configured(_) => {
                failure.get_or_insert_with(|| {
                    anyhow::anyhow!("credential_storage: custom {slot} remains after clearing")
                });
            }
            crate::keychain::ApiKeyState::Unavailable(error) => {
                failure.get_or_insert_with(|| {
                    anyhow::anyhow!(
                        "credential_storage: cannot verify cleared custom {slot}: {error}"
                    )
                });
            }
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }

    let sidecar_slots = ["provider_custom", "asr_api_key", "cleanup_api_key"];
    for slot in sidecar_slots {
        remove_secret_sidecar_for_host_change(dir, slot)?;
    }
    for slot in sidecar_slots {
        verify_secret_sidecar_absent(dir, slot)?;
    }
    Ok(())
}

pub fn remove_secret_sidecar_for_host_change(dir: &Path, slot: &str) -> anyhow::Result<()> {
    let path = secret_sidecar_path(dir, slot);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(anyhow::anyhow!(
            "credential_storage: failed to remove custom {slot} sidecar: {error}"
        )),
    }
}

pub fn verify_secret_sidecar_absent(dir: &Path, slot: &str) -> anyhow::Result<()> {
    let path = secret_sidecar_path(dir, slot);
    match fs::symlink_metadata(&path) {
        Ok(_) => Err(anyhow::anyhow!(
            "credential_storage: custom {slot} sidecar remains after clearing"
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(anyhow::anyhow!(
            "credential_storage: cannot verify cleared custom {slot} sidecar: {error}"
        )),
    }
}

pub fn clear_asr_key_sidecar(dir: &Path) {
    clear_secret_sidecar(dir, "asr_api_key");
}

pub fn clear_cleanup_key_sidecar(dir: &Path) {
    clear_secret_sidecar(dir, "cleanup_api_key");
}

pub trait CredentialBackend {
    fn read(&self, slot: &str) -> crate::keychain::ApiKeyState;
    fn write(&self, slot: &str, key: &str) -> Result<(), String>;
}

pub struct OsCredentialBackend;

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

pub fn clear_secret_sidecar_if_matches(dir: &Path, slot: &str, key: &str) {
    if read_secret_sidecar(dir, slot).as_deref() == Some(key) {
        clear_secret_sidecar(dir, slot);
    }
}

/// Persist to the OS credential store and confirm the exact value can be read
/// back before treating the operation as durable. `preserve_existing` is used
/// for legacy migration: it prevents a stale source from replacing a secure
/// credential that already exists or could not be read.
pub fn persist_secret_to_secure_store(
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
        crate::keychain::ApiKeyState::Unavailable(error)
            if preserve_existing || !clear_replaced_sidecar =>
        {
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

/// Adopt a value read directly from the active Groq Keychain namespace into
/// this process only. The caller never persists this Settings snapshot.
pub fn adopt_interactive_groq_credential(settings: &mut Settings, key: String) {
    settings.api_key = key.clone();
    settings
        .provider_api_keys
        .insert("groq".into(), key.clone());
    settings
        .credential_baselines
        .insert("groq_api_key".into(), key);
    settings
        .unverified_credential_sources
        .remove("settings:api_key");
    settings
        .unverified_credential_sources
        .remove("sidecar:groq_api_key");
    settings
        .credential_read_states
        .insert("groq_api_key".into(), CredentialReadState::Configured);
}

/// Adopt a required-key provider credential into memory after a foreground
/// Keychain read. The API returns only a SettingsView; this value is never
/// exposed to the webview or written to settings.json by this operation.
pub fn adopt_interactive_provider_credential(
    settings: &mut Settings,
    provider: crate::providers::EngineProvider,
    key: String,
) {
    let provider_id = provider.as_str();
    let account = provider.keychain_account();
    settings
        .provider_api_keys
        .insert(provider_id.to_owned(), key.clone());
    settings
        .credential_baselines
        .insert(account.to_owned(), key);
    settings
        .unverified_credential_sources
        .remove(&format!("settings:provider:{provider_id}"));
    settings
        .unverified_credential_sources
        .remove(&format!("sidecar:{account}"));
    settings
        .credential_read_states
        .insert(account.to_owned(), CredentialReadState::Configured);
}

/// Adopt a verified Custom credential into this process only. The caller
/// restricts reads to currently active Custom purposes sharing the captured
/// origin; this function deliberately does not write settings or Keychain.
pub fn adopt_interactive_custom_credential(
    settings: &mut Settings,
    key: String,
    scope: CustomCredentialRecoveryScope,
) {
    settings
        .provider_api_keys
        .insert("custom".into(), key.clone());
    settings
        .credential_baselines
        .insert("provider_custom".into(), key);
    settings
        .unverified_credential_sources
        .remove("settings:provider:custom");
    settings
        .unverified_credential_sources
        .remove("sidecar:provider_custom");
    settings
        .credential_read_states
        .insert("provider_custom".into(), CredentialReadState::Configured);
    settings.session_only_custom_credential = true;
    settings.session_custom_credential_scope = Some(scope);
    settings.session_custom_credential_rejected = false;
}

pub fn credential_source_slot(source: &str) -> Option<String> {
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

pub fn has_unverified_source(settings: &Settings, slot: &str, key: &str) -> bool {
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

pub fn is_explicit_credential_replacement(settings: &Settings, slot: &str, key: &str) -> bool {
    if has_unverified_source(settings, slot, key) {
        return false;
    }
    settings
        .credential_baselines
        .get(slot)
        .is_none_or(|previous| previous != key)
}

pub struct CredentialResolution {
    pub(super) value: String,
    pub(super) state: crate::keychain::ApiKeyState,
    pub(super) settings_source_verified: bool,
}

pub fn credential_read_state_from_resolution(
    state: &crate::keychain::ApiKeyState,
) -> CredentialReadState {
    match state {
        crate::keychain::ApiKeyState::Configured(_) => CredentialReadState::Configured,
        crate::keychain::ApiKeyState::Missing => CredentialReadState::Missing,
        crate::keychain::ApiKeyState::Unavailable(_) => CredentialReadState::Unavailable,
    }
}

pub fn resolve_legacy_credential(
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

pub fn load_settings_with_backend(dir: &Path, backend: &dyn CredentialBackend) -> (Settings, bool) {
    let path = dir.join("settings.json");
    let raw = std::fs::read(&path).ok();
    let malformed = raw
        .as_deref()
        .is_some_and(|bytes| serde_json::from_slice::<Settings>(bytes).is_err());
    if malformed {
        log::error!("settings.json is malformed; keeping it untouched and starting with defaults");
    }
    let raw_json_value = raw
        .as_deref()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).ok());
    let mut settings: Settings = raw
        .as_deref()
        .and_then(|bytes| serde_json::from_slice(bytes).ok())
        .unwrap_or_default();
    // Preserve previous zh+en auto-detection behavior for already-onboarded users
    // upgrading from builds before `preferred_languages` existed; fresh installs
    // start with an empty list (unbiased multilingual auto-detect).
    let legacy_preferred_migrated = settings.onboarded
        && raw_json_value.as_ref().is_some_and(|value| {
            value
                .as_object()
                .is_some_and(|obj| !obj.contains_key("preferred_languages"))
        });
    if legacy_preferred_migrated {
        settings.preferred_languages = vec!["zh".into(), "en".into()];
    }
    let schema_needs_persist = settings.schema_version < SETTINGS_SCHEMA_VERSION;
    let dictionary_sources_need_persist = dictionary_sources_need_normalization(&settings);
    let needs_backup = schema_needs_persist
        || dictionary_sources_need_persist
        || legacy_preferred_migrated
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

    let mut needs_persist = schema_needs_persist
        || dictionary_sources_need_persist
        || hotkeys_rewritten
        || legacy_preferred_migrated;
    let legacy_api_key = settings.api_key.clone();
    let api_resolution = resolve_legacy_credential(
        dir,
        "groq_api_key",
        &legacy_api_key,
        (!legacy_api_key.trim().is_empty()).then_some("settings:api_key"),
        &mut settings,
        backend,
    );
    settings.credential_read_states.insert(
        "groq_api_key".into(),
        credential_read_state_from_resolution(&api_resolution.state),
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
    settings.credential_read_states.insert(
        "asr_api_key".into(),
        credential_read_state_from_resolution(&asr_resolution.state),
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
    settings.credential_read_states.insert(
        "cleanup_api_key".into(),
        credential_read_state_from_resolution(&cleanup_resolution.state),
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
        settings.credential_read_states.insert(
            provider.keychain_account().to_owned(),
            credential_read_state_from_resolution(&resolution.state),
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

pub fn bind_legacy_keys_into_pool(settings: &mut Settings) {
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
pub fn backup_legacy_settings(path: &Path, raw: &[u8]) -> anyhow::Result<()> {
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

pub fn backup_malformed_settings_before_write(path: &Path) -> anyhow::Result<()> {
    let raw = match fs::read(path) {
        Ok(raw) if serde_json::from_slice::<Settings>(&raw).is_err() => raw,
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let backup = path.with_file_name("settings.json.malformed.bak");
    if backup.exists() {
        let metadata = fs::symlink_metadata(&backup)?;
        if !metadata.file_type().is_file() || fs::read(&backup)? != raw {
            anyhow::bail!("malformed settings backup already exists with different contents");
        }
        #[cfg(unix)]
        if metadata.permissions().mode() & 0o777 != 0o600 {
            anyhow::bail!("malformed settings backup has unexpected permissions");
        }
        return Ok(());
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&backup)?;
    file.write_all(&raw)?;
    file.sync_all()?;
    sync_directory(
        backup
            .parent()
            .ok_or_else(|| anyhow::anyhow!("invalid settings path"))?,
    )?;
    Ok(())
}
/// Persist settings to `settings.json`. API keys are written only to the OS
/// credential store and verified there before the settings file is updated.
pub fn save_settings(dir: &Path, settings: &Settings) -> anyhow::Result<()> {
    save_settings_with_backend(dir, settings, &OsCredentialBackend)
}

pub fn save_settings_with_backend(
    dir: &Path,
    settings: &Settings,
    backend: &dyn CredentialBackend,
) -> anyhow::Result<()> {
    save_settings_with_validation(dir, settings, backend, true)
}

/// Persist a structurally valid non-service settings change without accessing
/// credential stores. Verified credentials remain in their existing secure
/// store; deferred legacy settings values and sidecars remain their source of
/// truth until an explicit credential or service-route save can migrate them.
pub fn save_configuration(dir: &Path, settings: &Settings) -> anyhow::Result<()> {
    settings.validate_configuration()?;
    persist_settings_without_credential_changes(dir, settings)
}

pub fn persist_settings_without_credential_changes(
    dir: &Path,
    settings: &Settings,
) -> anyhow::Result<()> {
    let lock = SETTINGS_WRITE_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_private_dir(dir)?;
    backup_malformed_settings_before_write(&dir.join("settings.json"))?;

    let mut on_disk = settings.clone();
    on_disk.normalize_dictionary_sources();
    on_disk.api_key.clear();
    on_disk.asr_api_key.clear();
    on_disk.cleanup_api_key.clear();
    on_disk.provider_api_keys.clear();
    for (source, value) in &settings.unverified_credential_sources {
        let Some(field) = source.strip_prefix("settings:") else {
            // Sidecar sources remain in place; a preference save neither
            // reads nor removes their files.
            continue;
        };
        match field {
            "api_key" => on_disk.api_key.clone_from(value),
            "asr_api_key" => on_disk.asr_api_key.clone_from(value),
            "cleanup_api_key" => on_disk.cleanup_api_key.clone_from(value),
            provider_field if provider_field.starts_with("provider:") => {
                let provider = &provider_field["provider:".len()..];
                if crate::providers::EngineProvider::parse(provider).is_some() {
                    on_disk
                        .provider_api_keys
                        .insert(provider.to_owned(), value.clone());
                }
            }
            _ => {}
        }
    }

    let bytes = serde_json::to_vec_pretty(&on_disk)?;
    write_atomic_bytes(&dir.join("settings.json"), &bytes)?;
    Ok(())
}

pub fn save_settings_with_validation(
    dir: &Path,
    settings: &Settings,
    backend: &dyn CredentialBackend,
    check_readiness: bool,
) -> anyhow::Result<()> {
    settings.validate_internal(Some(&dir.join("models")), check_readiness)?;
    persist_settings_with_backend(dir, settings, backend)
}

/// Only the typed teaching fact can change through this entry. Do not validate
/// model availability/credentials as a prerequisite to acknowledging a guide.
pub fn acknowledge_guide(
    dir: &Path,
    current: &Settings,
    id: OnDemandGuideId,
    version: u32,
) -> anyhow::Result<Settings> {
    let mut next = current.clone();
    if next
        .guide_acknowledgements
        .acknowledge(id, version)
        .map_err(anyhow::Error::msg)?
    {
        persist_settings_without_credential_changes(dir, &next)?;
    }
    Ok(next)
}

pub fn persist_settings_with_backend(
    dir: &Path,
    settings: &Settings,
    backend: &dyn CredentialBackend,
) -> anyhow::Result<()> {
    let lock = SETTINGS_WRITE_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_private_dir(dir)?;
    backup_malformed_settings_before_write(&dir.join("settings.json"))?;
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
        if provider.is_custom() && settings.session_only_custom_credential {
            continue;
        }
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
    on_disk.normalize_dictionary_sources();
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
