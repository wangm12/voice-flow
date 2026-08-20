use crate::queue::QuotaView;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const SETTINGS_SCHEMA_VERSION: u32 = 13;
const HISTORY_SCHEMA_VERSION: i32 = 5;

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
    pub language: String,
    #[serde(default = "default_ui_language")]
    pub ui_language: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    pub dictionary: Vec<String>,
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
    #[serde(default)]
    pub input_device: String,
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

fn default_selected_action_hotkey() -> String {
    "CmdOrControl+Shift+Slash".into()
}

fn default_selected_actions_enabled() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            api_key: String::new(),
            language: "auto".into(),
            ui_language: default_ui_language(),
            theme: default_theme(),
            dictionary: Vec::new(),
            hotkey: "CmdOrControl+Shift+Space".into(),
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
            input_device: String::new(),
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
        if !crate::llm::is_supported_model(&self.cleanup_model) {
            self.cleanup_model = default_cleanup_model();
        }
        if self.hotkey.is_empty() || self.hotkey.len() > 128 {
            self.hotkey = Self::default().hotkey;
        }
        if self.hotkey.contains("Dead") {
            self.hotkey = self.hotkey.replace("Dead", "Space");
        }
        if self.activation_mode == "hold" {
            self.activation_mode = if crate::modifier_hotkey::is_modifier_only(&self.hotkey) {
                "double_tap"
            } else {
                "tap"
            }
            .into();
        }
        if !matches!(self.activation_mode.as_str(), "tap" | "double_tap") {
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
        if !crate::llm::is_supported_model(&self.cleanup_model) {
            anyhow::bail!("unsupported cleanup model");
        }
        if self.hotkey.is_empty() || self.hotkey.len() > 128 {
            anyhow::bail!("hotkey must contain between 1 and 128 characters");
        }
        if !matches!(self.activation_mode.as_str(), "tap" | "double_tap") {
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
        Ok(())
    }
}

/// Settings exposed to the webview. The backend keeps the real credential in
/// memory/keychain, while the UI only receives whether one is configured and a
/// non-sensitive hint for display.
#[derive(Debug, Clone, Serialize)]
pub struct SettingsView {
    pub schema_version: u32,
    pub api_key_configured: bool,
    pub api_key_hint: Option<String>,
    pub asr_model: String,
    pub cleanup_model: String,
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
    pub input_device: String,
}

impl From<&Settings> for SettingsView {
    fn from(settings: &Settings) -> Self {
        let api_key_hint = if settings.api_key.is_empty() {
            None
        } else {
            let tail: String = settings.api_key.chars().rev().take(4).collect();
            Some(format!("••••{}", tail.chars().rev().collect::<String>()))
        };
        Self {
            schema_version: settings.schema_version,
            api_key_configured: !settings.api_key.is_empty(),
            api_key_hint,
            asr_model: crate::asr::MODEL.to_owned(),
            cleanup_model: settings.cleanup_model.clone(),
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
            input_device: settings.input_device.clone(),
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

/// Persist a retry/recovery audio file without exposing paths outside the
/// app-owned spool directory. The quota is checked before writing and the
/// final filename is installed with an atomic rename.
pub fn write_spool_file(dir: &Path, relative: &Path, bytes: &[u8]) -> anyhow::Result<PathBuf> {
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
    let current = spool_size(&root)?;
    let existing = fs::symlink_metadata(&path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if current
        .saturating_sub(existing)
        .saturating_add(bytes.len() as u64)
        > MAX_SPOOL_BYTES
    {
        anyhow::bail!("audio spool quota exceeded");
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid spool path"))?;
    fs::create_dir_all(parent)?;
    write_atomic_bytes(&path, bytes)?;
    Ok(path)
}

fn write_atomic_bytes(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid atomic path"))?;
    fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("invalid atomic filename"))?
        .to_string_lossy();
    let tmp = parent.join(format!(".{name}.tmp-{}", std::process::id()));
    let write_result = (|| -> anyhow::Result<()> {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
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
    fs::create_dir_all(&session_dir)?;
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
        let bytes = fs::read(path)?;
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

fn rebuild_recovery_wav(session_dir: &Path) -> anyhow::Result<RecoveredSpool> {
    let samples = read_recovery_samples(session_dir)?;
    let wav = crate::chunker::encode_wav(&samples).map_err(|error| anyhow::anyhow!(error))?;
    let audio_path = session_dir.join("recovery.wav");
    write_atomic_bytes(&audio_path, &wav)?;
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
    let needs_backup = schema_needs_persist || !settings.api_key.is_empty();
    if needs_backup {
        if let Some(raw) = raw.as_deref() {
            if let Err(error) = backup_legacy_settings(&path, raw) {
                log::warn!("failed to back up legacy settings before migration: {error}");
            }
        }
    }
    settings.normalize();

    // Reconcile the API key with the keychain. If we find a plaintext key in
    // the file, migrate it into the keychain and flag that the file should be
    // re-saved with the key removed.
    let plaintext_key = settings.api_key.clone();
    let had_plaintext = !plaintext_key.is_empty();
    let key_state = crate::keychain::resolve_api_key(&plaintext_key);
    let mut needs_persist = schema_needs_persist;
    match key_state {
        crate::keychain::ApiKeyState::Configured(key) => {
            settings.api_key = key;
            needs_persist = had_plaintext;
        }
        crate::keychain::ApiKeyState::Missing => {
            settings.api_key.clear();
            if settings.onboarded {
                needs_persist = true;
            }
            settings.onboarded = false;
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
    (settings, needs_persist)
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
    let temp = path.with_file_name(format!(
        ".settings.json.pre-migration.tmp-{}",
        std::process::id()
    ));
    let bytes = serde_json::to_vec_pretty(&value)?;
    fs::write(&temp, bytes)?;
    fs::rename(temp, backup)?;
    Ok(())
}
/// Persist settings to `settings.json`. The API key is stored in the OS
/// credential store, never in the file, so the on-disk JSON always has the
/// key field blanked out.
pub fn save_settings(dir: &Path, settings: &Settings) -> anyhow::Result<()> {
    settings.validate()?;
    let lock = SETTINGS_WRITE_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    std::fs::create_dir_all(dir)?;
    // Write a new key into the keychain first; only persist the file (without
    // the key) once the secret is safely stored. An empty in-memory key can
    // also mean that a non-interactive keychain read timed out during startup,
    // so ordinary settings writes must never interpret it as a destructive
    // credential deletion. Credential removal needs an explicit operation.
    if !settings.api_key.trim().is_empty() {
        crate::keychain::set_api_key(&settings.api_key).map_err(|e| {
            anyhow::anyhow!("credential_storage: failed to store API key securely: {e}")
        })?;
    }
    let mut on_disk = settings.clone();
    on_disk.api_key = String::new();
    let bytes = serde_json::to_vec_pretty(&on_disk)?;
    let path = dir.join("settings.json");
    let tmp_path = dir.join(format!(".settings.json.tmp-{}", std::process::id()));
    let mut file = File::create(&tmp_path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(&tmp_path, &path)?;
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
    ensure_column(c, "cleanup_status", "TEXT")?;
    c.pragma_update(None, "user_version", HISTORY_SCHEMA_VERSION)?;
    Ok(())
}

fn open_history(dir: &Path) -> anyhow::Result<Connection> {
    fs::create_dir_all(dir)?;
    let path = dir.join("history.sqlite");
    let existed = path.exists();
    let connection = Connection::open(&path)?;
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
            fs::copy(&path, backup)?;
        }
    }
    schema(&connection)?;
    Ok(connection)
}

static HISTORY_SCHEMA_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn ensure_column(c: &Connection, name: &str, definition: &str) -> anyhow::Result<()> {
    let mut statement = c.prepare("PRAGMA table_info(dictations)")?;
    let exists = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|column| column == name);
    if !exists {
        c.execute(
            &format!("ALTER TABLE dictations ADD COLUMN {name} {definition}"),
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
    c.execute("INSERT INTO dictations (created_at,duration_secs,raw_text,final_text,cleanup_status,engine,degraded,degraded_reason,status,raw_audio_path,delivery_method,fallback_reason,context_profile_id,context_policy_json) VALUES (datetime('now'),?,?,?,?,?,?,?,?,?,?,?,?,?)", params![duration, raw, final_text, cleanup_status, "groq", degraded as i32, degraded_reason, status, spool.map(|p| p.to_string_lossy().to_string()), delivery_method, fallback_reason, context_profile_id, context_policy_json])?;
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
) -> anyhow::Result<HistoryPage> {
    let c = open_history(dir)?;
    let spool_root = dir.join("spool");
    let limit = limit.clamp(1, 100);
    let fetch_limit = limit + 1;
    let sql = if before_id.is_some() {
        "SELECT id,created_at,COALESCE(raw_text,''),COALESCE((SELECT final_text FROM dictation_revisions WHERE dictation_id=dictations.id ORDER BY revision_id DESC LIMIT 1),final_text,''),COALESCE(duration_secs,0),COALESCE(degraded,0),degraded_reason,COALESCE(status,'ok'),delivery_method,fallback_reason,context_profile_id,raw_audio_path,COALESCE(cleanup_status,'unknown'),(SELECT COUNT(*) FROM dictation_revisions WHERE dictation_id=dictations.id) FROM dictations WHERE id < ? ORDER BY id DESC LIMIT ?"
    } else {
        "SELECT id,created_at,COALESCE(raw_text,''),COALESCE((SELECT final_text FROM dictation_revisions WHERE dictation_id=dictations.id ORDER BY revision_id DESC LIMIT 1),final_text,''),COALESCE(duration_secs,0),COALESCE(degraded,0),degraded_reason,COALESCE(status,'ok'),delivery_method,fallback_reason,context_profile_id,raw_audio_path,COALESCE(cleanup_status,'unknown'),(SELECT COUNT(*) FROM dictation_revisions WHERE dictation_id=dictations.id) FROM dictations ORDER BY id DESC LIMIT ?"
    };
    let mut s = c.prepare(sql)?;
    let rows = if let Some(before_id) = before_id {
        s.query_map(params![before_id, fetch_limit], history_row(&spool_root))?
    } else {
        s.query_map([fetch_limit], history_row(&spool_root))?
    };
    let mut items = rows.collect::<Result<Vec<_>, _>>()?;
    let has_more = items.len() > limit as usize;
    items.truncate(limit as usize);
    Ok(HistoryPage { items, has_more })
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
    Ok(get_history_page(dir, limit, None)?.items)
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

pub fn clear_all_data(dir: &Path) -> anyhow::Result<()> {
    let c = open_history(dir)?;
    c.execute_batch("DELETE FROM dictations; DELETE FROM usage;")?;
    drop(c);
    let spool = dir.join("spool");
    if spool.exists() {
        fs::remove_dir_all(&spool)?;
    }
    fs::create_dir_all(spool)?;
    Ok(())
}

pub fn export_history_json(dir: &Path) -> anyhow::Result<String> {
    let mut items = Vec::new();
    let mut before_id = None;
    loop {
        let page = get_history_page(dir, 100, before_id)?;
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
pub fn history_context(
    dir: &Path,
    id: i64,
) -> anyhow::Result<Option<crate::context::ContextPolicy>> {
    let c = open_history(dir)?;
    let json: Option<String> = c
        .query_row(
            "SELECT context_policy_json FROM dictations WHERE id=?",
            [id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    json.map(|value| serde_json::from_str(&value).map_err(anyhow::Error::from))
        .transpose()
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
        assert_eq!(settings.selected_action_hotkey, "CmdOrControl+Shift+Slash");
        assert!(settings.selected_actions_enabled);
        assert!(settings.input_device.is_empty());
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
    fn settings_view_redacts_api_key() {
        let settings = Settings {
            api_key: "test_key_1234".into(),
            ..Settings::default()
        };
        let view = SettingsView::from(&settings);
        let json = serde_json::to_string(&view).unwrap();
        assert!(view.api_key_configured);
        assert_eq!(view.api_key_hint.as_deref(), Some("••••1234"));
        assert_eq!(view.asr_model, crate::asr::MODEL);
        assert_eq!(view.cleanup_model, crate::llm::MODEL);
        assert!(!json.contains("test_key_1234"));
        assert!(!json.contains("secret"));
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
    fn history_page_uses_a_cursor_and_reports_more_rows() {
        let dir = temp_dir("history-page");
        insert_history(&dir, "one", "one", 1.0, false).unwrap();
        insert_history(&dir, "two", "two", 1.0, false).unwrap();
        insert_history(&dir, "three", "three", 1.0, false).unwrap();

        let first = get_history_page(&dir, 2, None).unwrap();
        assert_eq!(first.items.len(), 2);
        assert!(first.has_more);
        let before_id = first.items.last().unwrap().id;

        let second = get_history_page(&dir, 2, Some(before_id)).unwrap();
        assert_eq!(second.items.len(), 1);
        assert!(!second.has_more);
        assert!(second.items[0].id < before_id);
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
        let page = get_history_page(&dir, 10, None).unwrap();
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
        let page = get_history_page(&dir, 10, None).unwrap();
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

        assert!(get_history_page(&dir, 10, None).unwrap().items.is_empty());
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
