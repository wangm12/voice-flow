//! Active-app context detection and policy resolution.
//!
//! The resolver is platform independent and deliberately accepts a small,
//! private signal object. Raw process/window/browser values never cross the
//! Tauri IPC boundary or enter an LLM request.

use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const BROWSER_QUERY_TIMEOUT_MS: u64 = 800;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContextFamily {
    Email,
    BrowserSearch,
    WorkChat,
    PersonalChat,
    Document,
    ProjectManagement,
    CalendarTask,
    DeveloperCollaboration,
    PromptOrCode,
    Terminal,
    FormFilling,
    NotesJournaling,
    SocialMedia,
    CustomerSupport,
    #[default]
    General,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSource {
    UserMapping,
    BrowserDomain,
    NativeProcess,
    WindowTitle,
    FocusedInput,
    ManualOverride,
    Fallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAccessStatus {
    #[default]
    NotApplicable,
    Disabled,
    NeedsPermission,
    Granted,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextPolicy {
    pub artifact_kind: String,
    pub formality: String,
    pub density: String,
    pub markup: String,
    pub list_behavior: String,
    pub sentence_completeness: String,
    pub preserve_technical_tokens: bool,
    pub forbidden_additions: Vec<String>,
    /// A user-managed writing mode prompt. The immutable system safety prompt
    /// remains separate and is always sent to the provider.
    #[serde(default)]
    pub writing_prompt: Option<String>,
    /// Per-recording output formatting selected in Settings. `None` keeps the
    /// existing app-aware cleanup behavior.
    #[serde(default)]
    pub output_mode: Option<String>,
    #[serde(default)]
    pub translation_target_language: Option<String>,
    #[serde(default)]
    pub style_example_input: Option<String>,
    #[serde(default)]
    pub style_example_output: Option<String>,
}

impl ContextPolicy {
    pub fn for_family(family: ContextFamily) -> Self {
        let mut policy = Self {
            artifact_kind: "general_text".into(),
            formality: "neutral".into(),
            density: "natural".into(),
            markup: "plain_text".into(),
            list_behavior: "only_when_explicit".into(),
            sentence_completeness: "light_cleanup".into(),
            preserve_technical_tokens: false,
            forbidden_additions: vec![
                "facts".into(),
                "claims".into(),
                "subject lines".into(),
                "greetings not spoken by the user".into(),
            ],
            writing_prompt: None,
            output_mode: None,
            translation_target_language: None,
            style_example_input: None,
            style_example_output: None,
        };
        match family {
            ContextFamily::Email => {
                policy.artifact_kind = "email_body".into();
                policy.formality = "professional".into();
                policy.density = "complete_paragraphs".into();
                policy.sentence_completeness = "complete_sentences".into();
            }
            ContextFamily::BrowserSearch => {
                policy.artifact_kind = "search_query_or_web_input".into();
                policy.density = "concise".into();
                policy.sentence_completeness = "preserve_fragments_when_intentional".into();
                policy
                    .forbidden_additions
                    .push("search context not spoken by the user".into());
            }
            ContextFamily::WorkChat => {
                policy.artifact_kind = "chat_message".into();
                policy.formality = "neutral".into();
                policy.density = "concise".into();
                policy.sentence_completeness = "natural_sentences".into();
            }
            ContextFamily::PersonalChat => {
                policy.artifact_kind = "chat_message".into();
                policy.formality = "casual".into();
                policy.density = "concise".into();
                policy.sentence_completeness = "preserve_fragments_when_intentional".into();
                policy.forbidden_additions.extend([
                    "greetings not spoken".into(),
                    "swear-word sanitization".into(),
                ]);
            }
            ContextFamily::Document => {
                policy.artifact_kind = "document_text".into();
                policy.density = "clear_paragraphs".into();
                policy.sentence_completeness = "complete_sentences".into();
            }
            ContextFamily::ProjectManagement => {
                policy.artifact_kind = "task_update".into();
                policy.density = "concise".into();
                policy.list_behavior = "only_when_explicit".into();
            }
            ContextFamily::CalendarTask => {
                policy.artifact_kind = "calendar_or_task_entry".into();
                policy.density = "concise".into();
                policy.sentence_completeness = "preserve_original_structure".into();
                policy.forbidden_additions.extend([
                    "events not spoken by the user".into(),
                    "tasks not spoken by the user".into(),
                    "attendees not spoken by the user".into(),
                    "locations not spoken by the user".into(),
                ]);
            }
            ContextFamily::DeveloperCollaboration | ContextFamily::PromptOrCode => {
                policy.artifact_kind = "developer_prompt_or_text".into();
                policy.density = "compact".into();
                policy.markup = "preserve_spoken_markup".into();
                policy.preserve_technical_tokens = true;
                policy.sentence_completeness = "preserve_fragments_when_intentional".into();
            }
            ContextFamily::Terminal => {
                policy.artifact_kind = "command_or_terminal_input".into();
                policy.density = "exact".into();
                policy.markup = "preserve_command_syntax".into();
                policy.preserve_technical_tokens = true;
                policy.sentence_completeness = "preserve_fragments_when_intentional".into();
            }
            ContextFamily::FormFilling => {
                policy.artifact_kind = "form_field_value".into();
                policy.density = "concise".into();
                policy.sentence_completeness = "preserve_original_structure".into();
            }
            ContextFamily::NotesJournaling => {
                policy.artifact_kind = "note_or_journal_entry".into();
                policy.density = "natural".into();
                policy.sentence_completeness = "light_cleanup".into();
            }
            ContextFamily::SocialMedia => {
                policy.artifact_kind = "social_post_or_reply".into();
                policy.density = "natural".into();
                policy.sentence_completeness = "natural_sentences".into();
            }
            ContextFamily::CustomerSupport => {
                policy.artifact_kind = "customer_support_message".into();
                policy.formality = "helpful_professional".into();
                policy.density = "complete_paragraphs".into();
                policy.sentence_completeness = "complete_sentences".into();
            }
            ContextFamily::General => {}
        }
        policy
    }
}

impl Default for ContextPolicy {
    fn default() -> Self {
        Self::for_family(ContextFamily::General)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextProfile {
    pub id: String,
    pub family: ContextFamily,
    #[serde(default)]
    pub writing_mode_id: Option<String>,
    pub app_label: String,
    pub icon_key: String,
    pub source: ContextSource,
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetAppGuard {
    pub pid: i32,
    pub bundle_id: Option<String>,
    pub browser_host: Option<String>,
    /// Local-only fingerprint of the active browser target. It includes the
    /// URL and, when the browser exposes them, the active tab index/title. The
    /// raw URL/title never crosses the detector boundary or gets serialized.
    pub browser_target_token: Option<u64>,
    /// Local-only fingerprints of the active native window and focused
    /// editable element. They never cross the detector boundary. They are
    /// best-effort because macOS can omit them for some apps or during a
    /// transient Accessibility query failure.
    pub window_token: Option<u64>,
    /// Stable macOS CoreGraphics identity for the active native window. When
    /// present, this disambiguates two windows that share the same title and
    /// geometry.
    pub window_id: Option<u64>,
    pub input_token: Option<u64>,
    /// Secure/password fields are never eligible for automatic injection.
    pub secure_input: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContextSnapshot {
    pub profile: ContextProfile,
    pub policy: ContextPolicy,
    pub captured_at_ms: u64,
    pub browser_access_status: BrowserAccessStatus,
    #[serde(skip)]
    pub target_guard: TargetAppGuard,
}

pub struct ContextState {
    pub enabled: bool,
    pub browser_access_enabled: bool,
    pub mappings: Vec<AppMapping>,
    pub writing_modes: Vec<WritingMode>,
    /// A temporary user-selected writing policy. It is intentionally kept in
    /// memory and never persisted, while the detected target guard remains
    /// tied to the real frontmost app/window/input.
    pub manual_override: Option<ContextFamily>,
    pub snapshot: ContextSnapshot,
    /// Monotonically identifies the latest in-flight detection request. A
    /// slow detector must not overwrite a snapshot produced after settings
    /// changed or a newer refresh started.
    pub detection_generation: u64,
}

impl ContextState {
    #[allow(dead_code)]
    pub fn new(enabled: bool, browser_access_enabled: bool, mappings: Vec<AppMapping>) -> Self {
        Self::new_with_modes(
            enabled,
            browser_access_enabled,
            mappings,
            builtin_writing_modes(),
        )
    }

    pub fn new_with_modes(
        enabled: bool,
        browser_access_enabled: bool,
        mappings: Vec<AppMapping>,
        writing_modes: Vec<WritingMode>,
    ) -> Self {
        // Do not run System Events / AppleScript during Tauri setup. The first
        // background refresh and every recording start perform a fresh probe;
        // startup should be able to render settings immediately.
        let snapshot = ContextSnapshot::general();
        Self {
            enabled,
            browser_access_enabled,
            mappings,
            writing_modes,
            manual_override: None,
            snapshot,
            detection_generation: 0,
        }
    }
}

impl ContextSnapshot {
    pub fn general() -> Self {
        snapshot_for_signal_with_modes(&AppSignal::default(), &[], false, &builtin_writing_modes())
    }
}

const UNKNOWN_APP_LABEL: &str = "未知应用";

pub fn display_style_label(family: ContextFamily) -> &'static str {
    match family {
        ContextFamily::PromptOrCode | ContextFamily::DeveloperCollaboration => "代码",
        ContextFamily::Email => "邮件",
        ContextFamily::BrowserSearch => "搜索",
        ContextFamily::WorkChat => "工作短讯",
        ContextFamily::PersonalChat => "口语",
        ContextFamily::Document => "文档",
        ContextFamily::Terminal => "命令",
        ContextFamily::FormFilling => "表单",
        ContextFamily::NotesJournaling => "笔记",
        ContextFamily::SocialMedia => "社交",
        ContextFamily::CustomerSupport => "客服",
        ContextFamily::ProjectManagement => "待办",
        ContextFamily::CalendarTask => "日程",
        ContextFamily::General => "通用",
    }
}

pub fn display_app_name(snapshot: &ContextSnapshot) -> Option<&str> {
    let label = snapshot.profile.app_label.trim();
    if !is_placeholder_app_label(label) && short_browser_name_for_label(label).is_none() {
        return Some(label);
    }
    if let Some(name) =
        short_browser_display_name(snapshot.target_guard.bundle_id.as_deref(), label)
    {
        return Some(name);
    }
    if is_placeholder_app_label(label) {
        return None;
    }
    Some(label)
}

fn is_placeholder_app_label(label: &str) -> bool {
    label.is_empty()
        || label.eq_ignore_ascii_case("General")
        || label.eq_ignore_ascii_case("Unknown App")
        || label.eq_ignore_ascii_case("Browser")
        || label == "未知 App"
        || label == UNKNOWN_APP_LABEL
}

fn short_browser_display_name(bundle_id: Option<&str>, label: &str) -> Option<&'static str> {
    short_browser_name_for_bundle(bundle_id).or_else(|| short_browser_name_for_label(label))
}

fn short_browser_name_for_bundle(bundle_id: Option<&str>) -> Option<&'static str> {
    match bundle_id {
        Some("com.google.Chrome") => Some("Chrome"),
        Some("com.google.Chrome.canary") => Some("Chrome Canary"),
        Some("com.apple.Safari") => Some("Safari"),
        Some("company.thebrowser.Browser") => Some("Arc"),
        Some("com.brave.Browser") => Some("Brave"),
        Some("com.microsoft.edgemac") => Some("Edge"),
        Some("org.mozilla.firefox") => Some("Firefox"),
        Some("com.vivaldi.Vivaldi") => Some("Vivaldi"),
        Some("com.operasoftware.Opera") => Some("Opera"),
        Some("com.kagi.kagimacOS") => Some("Orion"),
        _ => None,
    }
}

fn short_browser_name_for_label(label: &str) -> Option<&'static str> {
    match label {
        "Google Chrome" => Some("Chrome"),
        "Google Chrome Canary" => Some("Chrome Canary"),
        "Brave Browser" => Some("Brave"),
        "Microsoft Edge" => Some("Edge"),
        _ => None,
    }
}

pub fn hud_style_id(snapshot: &ContextSnapshot) -> &'static str {
    if snapshot.profile.confidence < 0.75 {
        "general"
    } else {
        family_id(snapshot.profile.family)
    }
}

pub fn display_label(snapshot: &ContextSnapshot) -> String {
    let style = if snapshot.profile.confidence < 0.75 {
        "通用"
    } else {
        display_style_label(snapshot.profile.family)
    };
    match display_app_name(snapshot) {
        Some(app) => format!("{app} · {style}"),
        None => format!("{UNKNOWN_APP_LABEL} · {style}"),
    }
}

pub fn family_id(family: ContextFamily) -> &'static str {
    match family {
        ContextFamily::Email => "email",
        ContextFamily::BrowserSearch => "browser_search",
        ContextFamily::WorkChat => "work_chat",
        ContextFamily::PersonalChat => "personal_chat",
        ContextFamily::Document => "document",
        ContextFamily::ProjectManagement => "project_management",
        ContextFamily::CalendarTask => "calendar_task",
        ContextFamily::DeveloperCollaboration => "developer_collaboration",
        ContextFamily::PromptOrCode => "prompt_or_code",
        ContextFamily::Terminal => "terminal",
        ContextFamily::FormFilling => "form_filling",
        ContextFamily::NotesJournaling => "notes_journaling",
        ContextFamily::SocialMedia => "social_media",
        ContextFamily::CustomerSupport => "customer_support",
        ContextFamily::General => "general",
    }
}

pub const MAX_WRITING_MODE_LABEL_CHARS: usize = 64;
pub const MAX_WRITING_MODE_PROMPT_CHARS: usize = 8_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WritingMode {
    pub id: String,
    pub label: String,
    pub family: ContextFamily,
    pub prompt: String,
    #[serde(default)]
    pub builtin: bool,
}

pub fn family_label(family: ContextFamily) -> &'static str {
    match family {
        ContextFamily::Email => "邮件",
        ContextFamily::BrowserSearch => "浏览器搜索 / 研究",
        ContextFamily::WorkChat => "工作聊天",
        ContextFamily::PersonalChat => "个人聊天",
        ContextFamily::Document => "文档",
        ContextFamily::ProjectManagement => "项目管理",
        ContextFamily::CalendarTask => "日历 / 任务",
        ContextFamily::DeveloperCollaboration => "开发协作",
        ContextFamily::PromptOrCode => "Prompt / 代码",
        ContextFamily::Terminal => "终端 / 命令行",
        ContextFamily::FormFilling => "填写表单",
        ContextFamily::NotesJournaling => "笔记 / 日记",
        ContextFamily::SocialMedia => "社交媒体",
        ContextFamily::CustomerSupport => "客户支持",
        ContextFamily::General => "通用",
    }
}

const OLD_SHARED_CHAT_PROMPT: &str = "Keep the message natural, short, and conversational. Do not turn it into an email or add greetings/sign-offs.";

pub fn default_writing_prompt(family: ContextFamily) -> &'static str {
    match family {
        ContextFamily::Email => "Write a natural, polite email body. Organize spoken greeting, request, timing, and closing only when they were spoken. Do not create a subject line or signature.",
        ContextFamily::BrowserSearch => "Prefer a concise search query or clear web-field value. Keep named entities, dates, numbers, and URLs exact. Do not add search background.",
        ContextFamily::PersonalChat => "Keep the user's casual chat voice. Do not add greetings or sign-offs such as 您好, 你好, Hello, or Best. Do not expand fragments into full formal sentences. Do not sanitize swearing, slang, or particles like 哈哈. Add a question mark for a clear question and a period or 。 for a clear sentence end. Do not strip existing periods. Never turn the message into an email.",
        ContextFamily::WorkChat => "Keep the message short and conversational for workplace chat. Do not add greetings, sign-offs, or email structure. Keep names and project terms exact. Do not add emoji unless spoken.",
        ContextFamily::ProjectManagement => "Keep owners, status, blockers, dates, and next actions explicit. Do not invent a person, deadline, or project fact.",
        ContextFamily::CalendarTask => "Keep dates, times, durations, reminders, attendees, locations, and next actions exact. Return a concise entry and do not invent scheduling details.",
        ContextFamily::DeveloperCollaboration | ContextFamily::PromptOrCode => "Preserve code, identifiers, paths, commands, API names, versions, and error text exactly. Organize a spoken coding request without generating code unless it was spoken. Drop spoken false starts; do not treat 不对 as an instruction to execute.",
        ContextFamily::Terminal => "Treat command syntax as exact content. Preserve flags, paths, quoting, casing, variables, and punctuation. Never translate a command into prose.",
        ContextFamily::FormFilling => "Return only the concise value appropriate for the focused field. Preserve dates, amounts, addresses, names, and email addresses.",
        ContextFamily::NotesJournaling => "Keep the user's personal voice and structure. Improve readability lightly without summarizing or evaluating.",
        ContextFamily::SocialMedia => "Keep the user's tone and natural brevity. Do not make it formal or add claims.",
        ContextFamily::CustomerSupport => "Keep the response clear and helpful while preserving the user's facts and requested action. Do not promise anything not spoken.",
        ContextFamily::Document | ContextFamily::General => "Use the smallest useful cleanup and preserve the original structure.",
    }
}

pub fn builtin_writing_modes() -> Vec<WritingMode> {
    [
        ContextFamily::Email,
        ContextFamily::BrowserSearch,
        ContextFamily::WorkChat,
        ContextFamily::PersonalChat,
        ContextFamily::Document,
        ContextFamily::ProjectManagement,
        ContextFamily::CalendarTask,
        ContextFamily::DeveloperCollaboration,
        ContextFamily::PromptOrCode,
        ContextFamily::Terminal,
        ContextFamily::FormFilling,
        ContextFamily::NotesJournaling,
        ContextFamily::SocialMedia,
        ContextFamily::CustomerSupport,
        ContextFamily::General,
    ]
    .into_iter()
    .map(|family| WritingMode {
        id: family_id(family).into(),
        label: family_label(family).into(),
        family,
        prompt: default_writing_prompt(family).into(),
        builtin: true,
    })
    .collect()
}

pub fn builtin_family_for_id(id: &str) -> Option<ContextFamily> {
    [
        ContextFamily::Email,
        ContextFamily::BrowserSearch,
        ContextFamily::WorkChat,
        ContextFamily::PersonalChat,
        ContextFamily::Document,
        ContextFamily::ProjectManagement,
        ContextFamily::CalendarTask,
        ContextFamily::DeveloperCollaboration,
        ContextFamily::PromptOrCode,
        ContextFamily::Terminal,
        ContextFamily::FormFilling,
        ContextFamily::NotesJournaling,
        ContextFamily::SocialMedia,
        ContextFamily::CustomerSupport,
        ContextFamily::General,
    ]
    .into_iter()
    .find(|family| family_id(*family) == id)
}

pub fn normalize_writing_modes(modes: &mut Vec<WritingMode>) {
    let stored = std::mem::take(modes);
    let defaults = builtin_writing_modes();
    let mut normalized = defaults.clone();
    for mode in &mut normalized {
        if let Some(saved) = stored.iter().find(|candidate| candidate.id == mode.id) {
            let migrate_old_shared_chat = matches!(
                mode.family,
                ContextFamily::WorkChat | ContextFamily::PersonalChat
            ) && saved.prompt == OLD_SHARED_CHAT_PROMPT;
            if !saved.prompt.trim().is_empty() && !migrate_old_shared_chat {
                mode.prompt = saved
                    .prompt
                    .chars()
                    .take(MAX_WRITING_MODE_PROMPT_CHARS)
                    .collect();
            }
        }
    }
    for mut mode in stored {
        if builtin_family_for_id(&mode.id).is_some() {
            continue;
        }
        mode.label = mode
            .label
            .trim()
            .chars()
            .take(MAX_WRITING_MODE_LABEL_CHARS)
            .collect();
        mode.prompt = mode
            .prompt
            .trim()
            .chars()
            .take(MAX_WRITING_MODE_PROMPT_CHARS)
            .collect();
        mode.builtin = false;
        mode.family = ContextFamily::General;
        if mode.id.starts_with("custom.")
            && !mode.label.is_empty()
            && !mode.prompt.is_empty()
            && mode.id.len() <= 128
            && normalized.iter().all(|existing| existing.id != mode.id)
        {
            normalized.push(mode);
        }
    }
    *modes = normalized;
}

pub fn validate_writing_modes(modes: &[WritingMode]) -> Result<(), String> {
    if modes.len() > 64 {
        return Err("writing mode list cannot contain more than 64 modes".into());
    }
    let mut ids = std::collections::HashSet::new();
    for mode in modes {
        if mode.id.is_empty() || mode.id.len() > 128 || !ids.insert(&mode.id) {
            return Err(
                "writing mode ids must be unique and contain between 1 and 128 characters".into(),
            );
        }
        if mode.label.trim().is_empty() || mode.label.chars().count() > MAX_WRITING_MODE_LABEL_CHARS
        {
            return Err("writing mode labels must contain between 1 and 64 characters".into());
        }
        if mode.prompt.trim().is_empty()
            || mode.prompt.chars().count() > MAX_WRITING_MODE_PROMPT_CHARS
        {
            return Err("writing mode prompts must contain between 1 and 8000 characters".into());
        }
        match builtin_family_for_id(&mode.id) {
            Some(family) if mode.builtin && mode.family == family => {}
            None if !mode.builtin && mode.id.starts_with("custom.") => {}
            _ => return Err("writing mode builtin metadata is invalid".into()),
        }
    }
    Ok(())
}

/// Apply a temporary writing-policy override without changing the real target
/// identity used by the fail-closed delivery guard.
#[allow(dead_code)]
pub fn apply_manual_override(snapshot: &mut ContextSnapshot, family: Option<ContextFamily>) {
    apply_manual_override_with_modes(snapshot, family, &builtin_writing_modes());
}

pub fn apply_manual_override_with_modes(
    snapshot: &mut ContextSnapshot,
    family: Option<ContextFamily>,
    writing_modes: &[WritingMode],
) {
    let Some(family) = family else {
        return;
    };
    snapshot.profile = ContextProfile {
        id: format!("manual.{}", family_id(family)),
        family,
        writing_mode_id: Some(family_id(family).into()),
        app_label: "Manual override".into(),
        icon_key: "manual".into(),
        source: ContextSource::ManualOverride,
        confidence: 1.0,
    };
    snapshot.policy = policy_for_mode(family, Some(family_id(family)), writing_modes);
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppMapping {
    pub id: String,
    pub label: String,
    pub family: ContextFamily,
    #[serde(default)]
    pub mode_id: Option<String>,
    #[serde(default)]
    pub bundle_id: Option<String>,
    #[serde(default)]
    pub executable: Option<String>,
    #[serde(default)]
    pub browser_host: Option<String>,
    #[serde(default)]
    pub style_example_input: Option<String>,
    #[serde(default)]
    pub style_example_output: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub cleanup_effort: Option<crate::llm::CleanupEffort>,
    #[serde(default = "default_true")]
    pub cleanup_enabled: bool,
    #[serde(default = "default_true")]
    pub dictionary_learn_enabled: bool,
}

/// A user-facing application choice. Native code generates the selector so the
/// settings form never asks the user to enter bundle IDs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApplicationOption {
    pub bundle_id: String,
    pub label: String,
}

fn default_true() -> bool {
    true
}

impl AppMapping {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() || self.id.len() > 128 {
            return Err("mapping id must contain between 1 and 128 characters".into());
        }
        if self.label.trim().is_empty() || self.label.len() > 128 {
            return Err("mapping label must contain between 1 and 128 characters".into());
        }
        let selectors = [
            self.bundle_id.as_deref(),
            self.executable.as_deref(),
            self.browser_host.as_deref(),
        ];
        if selectors.iter().all(|selector| selector.is_none()) {
            return Err("mapping needs a bundle id, executable, or browser host".into());
        }
        if self
            .mode_id
            .as_deref()
            .is_some_and(|mode| mode.is_empty() || mode.len() > 128)
        {
            return Err("mapping mode id must contain between 1 and 128 characters".into());
        }
        if let Some(host) = &self.browser_host {
            if normalize_host(host).is_none() {
                return Err("browser host must be a hostname without a path".into());
            }
        }
        if self
            .style_example_input
            .as_deref()
            .is_some_and(|value| value.chars().count() > 2_000)
            || self
                .style_example_output
                .as_deref()
                .is_some_and(|value| value.chars().count() > 2_000)
        {
            return Err("style examples cannot exceed 2000 characters".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
struct AppSignal {
    pid: i32,
    bundle_id: Option<String>,
    process_name: String,
    window_title: String,
    focus_kind: FocusKind,
    browser_host: Option<String>,
    browser_target_token: Option<u64>,
    window_token: Option<u64>,
    window_id: Option<u64>,
    input_token: Option<u64>,
    browser_access_status: BrowserAccessStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum FocusKind {
    Secure,
    Search,
    Code,
    Terminal,
    Email,
    Chat,
    Document,
    Form,
    Editable,
    #[default]
    Unknown,
}

impl FocusKind {
    fn is_editable(self) -> bool {
        !matches!(self, Self::Unknown | Self::Secure)
    }

    fn is_secure(self) -> bool {
        matches!(self, Self::Secure)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[allow(dead_code)]
fn snapshot_for_signal(
    signal: &AppSignal,
    mappings: &[AppMapping],
    browser_access_enabled: bool,
) -> ContextSnapshot {
    snapshot_for_signal_with_modes(
        signal,
        mappings,
        browser_access_enabled,
        &builtin_writing_modes(),
    )
}

fn snapshot_for_signal_with_modes(
    signal: &AppSignal,
    mappings: &[AppMapping],
    browser_access_enabled: bool,
    writing_modes: &[WritingMode],
) -> ContextSnapshot {
    let profile = resolve_profile(signal, mappings);
    let mut policy = policy_for_mode(
        profile.family,
        profile.writing_mode_id.as_deref(),
        writing_modes,
    );
    if let Some(mapping) = mappings
        .iter()
        .find(|mapping| profile.id == format!("user.{}", mapping.id))
    {
        policy.style_example_input = mapping.style_example_input.clone();
        policy.style_example_output = mapping.style_example_output.clone();
    }
    let browser_status = if signal.is_browser() {
        if !browser_access_enabled {
            BrowserAccessStatus::Disabled
        } else {
            signal.browser_access_status
        }
    } else {
        BrowserAccessStatus::NotApplicable
    };
    ContextSnapshot {
        profile,
        policy,
        captured_at_ms: now_ms(),
        browser_access_status: browser_status,
        target_guard: TargetAppGuard {
            pid: signal.pid,
            bundle_id: signal.bundle_id.clone(),
            browser_host: signal.browser_host.clone(),
            browser_target_token: signal.browser_target_token,
            window_token: signal.window_token,
            window_id: signal.window_id,
            input_token: signal.input_token,
            secure_input: signal.focus_kind.is_secure(),
        },
    }
}

fn resolve_profile(signal: &AppSignal, mappings: &[AppMapping]) -> ContextProfile {
    if let Some(mapping) = mappings.iter().find(|mapping| {
        mapping.enabled
            && (selector_matches(
                mapping.browser_host.as_deref(),
                signal.browser_host.as_deref(),
            ) || selector_matches(mapping.bundle_id.as_deref(), signal.bundle_id.as_deref())
                || mapping.executable.as_deref().is_some_and(|executable| {
                    !signal.process_name.is_empty()
                        && executable.eq_ignore_ascii_case(&signal.process_name)
                }))
    }) {
        return profile_from_mapping(mapping);
    }

    let has_unknown_browser_host = if let Some(host) = signal.browser_host.as_deref() {
        if let Some((id, family, label)) = browser_profile(host) {
            return profile(
                id,
                family,
                label,
                "browser",
                ContextSource::BrowserDomain,
                0.98,
            );
        }
        true
    } else {
        false
    };

    if let Some(bundle_id) = signal.bundle_id.as_deref() {
        if let Some((id, family, label, icon)) = native_profile(bundle_id) {
            return profile(id, family, label, icon, ContextSource::NativeProcess, 0.96);
        }
    }

    let process = signal.process_name.to_ascii_lowercase();
    if process.contains("cursor")
        || process.contains("visual studio code")
        || process.contains("xcode")
        || process.contains("zed")
    {
        return profile(
            "code.cursor",
            ContextFamily::PromptOrCode,
            "Cursor",
            "cursor",
            ContextSource::NativeProcess,
            0.88,
        );
    }
    if process.contains("terminal") || process.contains("iterm") || process.contains("warp") {
        return profile(
            "terminal.native",
            ContextFamily::Terminal,
            &signal.process_name,
            "terminal",
            ContextSource::NativeProcess,
            0.82,
        );
    }
    if process.contains("wechat") || process.contains("messages") || process.contains("discord") {
        return profile(
            "chat.personal",
            ContextFamily::PersonalChat,
            &signal.process_name,
            "chat",
            ContextSource::NativeProcess,
            0.82,
        );
    }
    if process.contains("slack") || process.contains("teams") {
        return profile(
            "chat.team",
            ContextFamily::WorkChat,
            &signal.process_name,
            "chat",
            ContextSource::NativeProcess,
            0.82,
        );
    }
    if signal.focus_kind == FocusKind::Search {
        return profile(
            "browser.search",
            ContextFamily::BrowserSearch,
            &signal.process_name,
            "search",
            ContextSource::WindowTitle,
            0.68,
        );
    }
    if signal.focus_kind == FocusKind::Terminal {
        return profile(
            "terminal.focused",
            ContextFamily::Terminal,
            &signal.process_name,
            "terminal",
            ContextSource::WindowTitle,
            0.68,
        );
    }
    if signal.focus_kind == FocusKind::Code {
        return profile(
            "code.focused",
            ContextFamily::PromptOrCode,
            if signal.process_name.is_empty() {
                "Code editor"
            } else {
                &signal.process_name
            },
            "code",
            ContextSource::FocusedInput,
            0.72,
        );
    }
    if signal.focus_kind == FocusKind::Email {
        return profile(
            "email.focused",
            ContextFamily::Email,
            &signal.process_name,
            "mail",
            ContextSource::FocusedInput,
            0.72,
        );
    }
    if signal.focus_kind == FocusKind::Chat {
        return profile(
            "chat.focused",
            ContextFamily::WorkChat,
            &signal.process_name,
            "chat",
            ContextSource::FocusedInput,
            0.68,
        );
    }
    if signal.focus_kind == FocusKind::Document {
        return profile(
            "document.focused",
            ContextFamily::Document,
            &signal.process_name,
            "document",
            ContextSource::FocusedInput,
            0.68,
        );
    }
    if signal.focus_kind == FocusKind::Form {
        return profile(
            "form.focused",
            ContextFamily::FormFilling,
            &signal.process_name,
            "form",
            ContextSource::FocusedInput,
            0.66,
        );
    }
    if let Some(profile) = profile_from_window_title(signal) {
        return profile;
    }
    if has_unknown_browser_host {
        return profile(
            "browser.general",
            ContextFamily::General,
            if signal.process_name.is_empty() {
                "Browser"
            } else {
                &signal.process_name
            },
            "browser",
            ContextSource::Fallback,
            0.65,
        );
    }
    profile(
        "native.general",
        ContextFamily::General,
        if signal.process_name.is_empty() {
            "General"
        } else {
            &signal.process_name
        },
        "app",
        ContextSource::Fallback,
        0.4,
    )
}

fn selector_matches(mapping: Option<&str>, signal: Option<&str>) -> bool {
    match (mapping, signal) {
        (Some(expected), Some(actual)) => expected.eq_ignore_ascii_case(actual),
        _ => false,
    }
}

fn profile_from_window_title(signal: &AppSignal) -> Option<ContextProfile> {
    let title = signal.window_title.to_ascii_lowercase();
    if title.is_empty() {
        return None;
    }
    if title.contains("terminal") || title.contains("shell") || title.contains("command line") {
        return Some(profile(
            "terminal.window",
            ContextFamily::Terminal,
            &signal.process_name,
            "terminal",
            ContextSource::WindowTitle,
            0.62,
        ));
    }
    if title.contains("search") || title.contains("google search") || title.contains("bing") {
        return Some(profile(
            "browser.search",
            ContextFamily::BrowserSearch,
            &signal.process_name,
            "search",
            ContextSource::WindowTitle,
            0.6,
        ));
    }
    if title.contains("cursor")
        || title.contains("visual studio code")
        || title.contains("xcode")
        || title.contains("zed")
        || title.contains("pull request")
        || title.contains("source code")
    {
        return Some(profile(
            "code.window",
            ContextFamily::PromptOrCode,
            &signal.process_name,
            "code",
            ContextSource::WindowTitle,
            0.62,
        ));
    }
    if title.contains("gmail")
        || title.contains("outlook")
        || title.contains("mail")
        || title.contains("inbox")
        || title.contains("compose")
    {
        return Some(profile(
            "email.window",
            ContextFamily::Email,
            &signal.process_name,
            "mail",
            ContextSource::WindowTitle,
            0.62,
        ));
    }
    if title.contains("slack") || title.contains("teams") {
        return Some(profile(
            "chat.team.window",
            ContextFamily::WorkChat,
            &signal.process_name,
            "chat",
            ContextSource::WindowTitle,
            0.6,
        ));
    }
    if title.contains("wechat")
        || title.contains("whatsapp")
        || title.contains("messenger")
        || title.contains("telegram")
        || title.contains("discord")
    {
        return Some(profile(
            "chat.personal.window",
            ContextFamily::PersonalChat,
            &signal.process_name,
            "chat",
            ContextSource::WindowTitle,
            0.6,
        ));
    }
    if title.contains("calendar")
        || title.contains("todoist")
        || title.contains("reminders")
        || title.contains("reminder")
    {
        return Some(profile(
            "calendar.window",
            ContextFamily::CalendarTask,
            &signal.process_name,
            "calendar",
            ContextSource::WindowTitle,
            0.6,
        ));
    }
    if title.contains("notion")
        || title.contains("google docs")
        || title.contains("word")
        || title.contains("pages")
        || title.contains("document")
        || title.contains("journal")
        || title.contains("notes")
    {
        return Some(profile(
            "document.window",
            ContextFamily::Document,
            &signal.process_name,
            "document",
            ContextSource::WindowTitle,
            0.6,
        ));
    }
    if title.contains("linear")
        || title.contains("asana")
        || title.contains("trello")
        || title.contains("jira")
    {
        return Some(profile(
            "project.window",
            ContextFamily::ProjectManagement,
            &signal.process_name,
            "project",
            ContextSource::WindowTitle,
            0.58,
        ));
    }
    if title.contains("zendesk") || title.contains("intercom") || title.contains("support") {
        return Some(profile(
            "support.window",
            ContextFamily::CustomerSupport,
            &signal.process_name,
            "support",
            ContextSource::WindowTitle,
            0.58,
        ));
    }
    if title.contains("reddit")
        || title.contains("twitter")
        || title.contains("x.com")
        || title.contains("instagram")
        || title.contains("facebook")
    {
        return Some(profile(
            "social.window",
            ContextFamily::SocialMedia,
            &signal.process_name,
            "social",
            ContextSource::WindowTitle,
            0.58,
        ));
    }
    if title.contains("checkout")
        || title.contains("sign in")
        || title.contains("signup")
        || title.contains("registration")
        || title.contains("form")
    {
        return Some(profile(
            "form.window",
            ContextFamily::FormFilling,
            &signal.process_name,
            "form",
            ContextSource::WindowTitle,
            0.56,
        ));
    }
    None
}

fn profile_from_mapping(mapping: &AppMapping) -> ContextProfile {
    let id = format!("user.{}", mapping.id);
    let mut profile = profile(
        &id,
        mapping.family,
        &mapping.label,
        mapping.browser_host.as_deref().unwrap_or("app"),
        ContextSource::UserMapping,
        1.0,
    );
    profile.writing_mode_id = Some(
        mapping
            .mode_id
            .clone()
            .unwrap_or_else(|| family_id(mapping.family).into()),
    );
    profile
}

fn policy_for_mode(
    family: ContextFamily,
    mode_id: Option<&str>,
    writing_modes: &[WritingMode],
) -> ContextPolicy {
    let mut policy = ContextPolicy::for_family(family);
    let resolved_id = mode_id.unwrap_or_else(|| family_id(family));
    if let Some(mode) = writing_modes.iter().find(|mode| mode.id == resolved_id) {
        policy.writing_prompt = Some(mode.prompt.clone());
    }
    policy
}

fn profile(
    id: &str,
    family: ContextFamily,
    label: &str,
    icon_key: &str,
    source: ContextSource,
    confidence: f32,
) -> ContextProfile {
    ContextProfile {
        id: id.into(),
        family,
        writing_mode_id: None,
        app_label: label.into(),
        icon_key: icon_key.into(),
        source,
        confidence,
    }
}

fn browser_profile(host: &str) -> Option<(&'static str, ContextFamily, &'static str)> {
    match host {
        "gmail.com" | "mail.google.com" => Some(("email.gmail", ContextFamily::Email, "Gmail")),
        "outlook.office.com" | "outlook.live.com" => {
            Some(("email.outlook", ContextFamily::Email, "Outlook"))
        }
        "slack.com" | "app.slack.com" => Some(("chat.slack", ContextFamily::WorkChat, "Slack")),
        "teams.microsoft.com" => Some(("chat.teams", ContextFamily::WorkChat, "Teams")),
        "notion.so" | "www.notion.so" => {
            Some(("document.notion", ContextFamily::Document, "Notion"))
        }
        "docs.google.com" => Some((
            "document.google_docs",
            ContextFamily::Document,
            "Google Docs",
        )),
        "drive.google.com" => Some((
            "document.google_drive",
            ContextFamily::Document,
            "Google Drive",
        )),
        "google.com" | "www.google.com" | "bing.com" | "www.bing.com" | "duckduckgo.com"
        | "www.duckduckgo.com" => Some(("browser.search", ContextFamily::BrowserSearch, "Search")),
        "github.com" | "gitlab.com" => Some((
            "developer.web",
            ContextFamily::DeveloperCollaboration,
            "Developer",
        )),
        "linear.app" | "asana.com" | "trello.com" => {
            Some(("project.web", ContextFamily::ProjectManagement, "Project"))
        }
        "calendar.google.com" => Some((
            "calendar.google",
            ContextFamily::CalendarTask,
            "Google Calendar",
        )),
        "todoist.com" | "app.todoist.com" => {
            Some(("task.todoist", ContextFamily::CalendarTask, "Todoist"))
        }
        "x.com" | "twitter.com" | "www.reddit.com" => {
            Some(("social.web", ContextFamily::SocialMedia, "Social"))
        }
        _ => None,
    }
}

fn native_profile(
    bundle_id: &str,
) -> Option<(&'static str, ContextFamily, &'static str, &'static str)> {
    match bundle_id {
        "com.todesktop.230313mzl4w4u92" => Some((
            "code.cursor",
            ContextFamily::PromptOrCode,
            "Cursor",
            "cursor",
        )),
        "com.microsoft.VSCode" => Some((
            "code.vscode",
            ContextFamily::PromptOrCode,
            "VS Code",
            "vscode",
        )),
        "com.apple.Terminal" | "com.googlecode.iterm2" | "dev.warp.Warp-Stable" => Some((
            "terminal.native",
            ContextFamily::Terminal,
            "Terminal",
            "terminal",
        )),
        "com.apple.mail" => Some(("email.native", ContextFamily::Email, "Mail", "mail")),
        "com.microsoft.Outlook" => Some(("email.native", ContextFamily::Email, "Outlook", "mail")),
        "com.tinyspeck.slackmacgap" => {
            Some(("chat.native", ContextFamily::WorkChat, "Slack", "chat"))
        }
        "com.microsoft.teams2" => Some(("chat.native", ContextFamily::WorkChat, "Teams", "chat")),
        "com.tencent.xinWeChat" => Some((
            "chat.personal",
            ContextFamily::PersonalChat,
            "WeChat",
            "chat",
        )),
        "com.apple.MobileSMS" => Some((
            "chat.personal",
            ContextFamily::PersonalChat,
            "Messages",
            "chat",
        )),
        "com.hnc.Discord" => Some((
            "chat.personal",
            ContextFamily::PersonalChat,
            "Discord",
            "chat",
        )),
        "notion.id" => Some((
            "document.native",
            ContextFamily::Document,
            "Notion",
            "document",
        )),
        "com.microsoft.Word" => Some((
            "document.native",
            ContextFamily::Document,
            "Word",
            "document",
        )),
        "com.apple.iCal" => Some((
            "calendar.native",
            ContextFamily::CalendarTask,
            "Calendar",
            "calendar",
        )),
        "com.apple.reminders" => Some((
            "reminders.native",
            ContextFamily::CalendarTask,
            "Reminders",
            "reminders",
        )),
        _ => None,
    }
}

impl AppSignal {
    fn is_browser(&self) -> bool {
        browser_application(self.bundle_id.as_deref(), &self.process_name).is_some()
    }
}

/// Return a fixed AppleScript application name for a known browser adapter.
/// The adapter list only controls how an authorized browser exposes its active
/// URL; profile resolution and unknown-app fallback remain independent of it.
fn browser_application(bundle_id: Option<&str>, process_name: &str) -> Option<&'static str> {
    let by_bundle = match bundle_id {
        Some("com.google.Chrome") => Some("Google Chrome"),
        Some("com.google.Chrome.canary") => Some("Google Chrome Canary"),
        Some("com.apple.Safari") => Some("Safari"),
        Some("company.thebrowser.Browser") => Some("Arc"),
        Some("com.brave.Browser") => Some("Brave Browser"),
        Some("com.microsoft.edgemac") => Some("Microsoft Edge"),
        Some("org.mozilla.firefox") => Some("Firefox"),
        Some("com.vivaldi.Vivaldi") => Some("Vivaldi"),
        Some("com.operasoftware.Opera") => Some("Opera"),
        Some("com.kagi.kagimacOS") => Some("Orion"),
        _ => None,
    };
    by_bundle.or_else(|| {
        [
            "Google Chrome",
            "Google Chrome Canary",
            "Safari",
            "Arc",
            "Brave Browser",
            "Microsoft Edge",
            "Firefox",
            "Vivaldi",
            "Opera",
            "Orion",
        ]
        .into_iter()
        .find(|name| name.eq_ignore_ascii_case(process_name))
    })
}

fn is_browser_bundle_id(bundle_id: Option<&str>) -> bool {
    browser_application(bundle_id, "").is_some()
}

/// Tell the idle preview loop whether an unchanged frontmost application is a
/// browser whose active tab may have changed. This exposes only the bundle
/// classification; no process, title, URL, or target identity leaves this
/// module.
pub fn is_browser_application(bundle_id: Option<&str>) -> bool {
    is_browser_bundle_id(bundle_id)
}

#[cfg(test)]
pub fn target_matches(guard: &TargetAppGuard, current: &TargetAppGuard) -> bool {
    target_mismatch_reason(guard, current).is_none()
}

/// Same-field observation after paste only needs app/window/input identity.
/// Skip the browser URL probe — that is for writing policy, not learning.
pub fn focus_mismatch_reason(
    guard: &TargetAppGuard,
    current: &TargetAppGuard,
) -> Option<&'static str> {
    target_mismatch_without_browser(guard, current)
}

fn target_mismatch_without_browser(
    guard: &TargetAppGuard,
    current: &TargetAppGuard,
) -> Option<&'static str> {
    if guard.pid <= 0
        || current.pid <= 0
        || guard.bundle_id.is_none()
        || current.bundle_id.is_none()
    {
        return Some("target_unavailable");
    }
    if guard.pid != current.pid || guard.bundle_id != current.bundle_id {
        return Some("target_changed");
    }
    if guard.secure_input || current.secure_input {
        return Some("secure_input");
    }
    if guard.window_id.is_some()
        && current.window_id.is_some()
        && guard.window_id != current.window_id
    {
        return Some("target_changed");
    }
    None
}

/// Frontmost app/window/input tokens without the browser AppleScript query.
pub fn probe_focus_guard() -> TargetAppGuard {
    let signal = frontmost_signal(false);
    TargetAppGuard {
        pid: signal.pid,
        bundle_id: signal.bundle_id,
        browser_host: None,
        browser_target_token: None,
        window_token: signal.window_token,
        window_id: signal.window_id,
        input_token: signal.input_token,
        secure_input: signal.focus_kind.is_secure(),
    }
}

/// Return a safe, non-sensitive reason for rejecting delivery. Raw window
/// titles, URLs, PIDs, and accessibility data never leave this module.
pub fn target_mismatch_reason(
    guard: &TargetAppGuard,
    current: &TargetAppGuard,
) -> Option<&'static str> {
    if let Some(reason) = target_mismatch_without_browser(guard, current) {
        return Some(reason);
    }
    // Browser URL/tab metadata is only needed for context-aware writing
    // policy. It is optional for delivery: without the separate browser
    // automation permission, the same browser app/window can still receive
    // the user's paste. If both probes return a concrete value, reject an
    // actual host/tab change.
    if is_browser_bundle_id(guard.bundle_id.as_deref()) {
        if guard.browser_host.is_some() && current.browser_host.is_none() {
            return Some("target_unavailable");
        }
        if guard.browser_target_token.is_some() && current.browser_target_token.is_none() {
            return Some("target_unavailable");
        }
        if let (Some(expected), Some(actual)) = (&guard.browser_host, &current.browser_host) {
            if expected != actual {
                return Some("target_changed");
            }
        }
        if let (Some(expected), Some(actual)) =
            (&guard.browser_target_token, &current.browser_target_token)
        {
            if expected != actual {
                return Some("target_changed");
            }
        }
    }
    None
}

pub fn normalize_host(value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches('.').to_ascii_lowercase();
    if value.is_empty() {
        return None;
    }
    let value = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .unwrap_or(&value);
    if value.contains('/') || value.contains('?') || value.contains('#') {
        return None;
    }
    let host = value.split(':').next()?.trim_end_matches('.');
    if host.is_empty() || host.chars().any(|ch| ch.is_whitespace()) {
        return None;
    }
    Some(host.to_owned())
}

fn extract_host(raw_url: &str) -> Option<String> {
    let raw_url = raw_url.trim();
    if !(raw_url.starts_with("https://") || raw_url.starts_with("http://")) {
        return None;
    }
    let authority = raw_url.split_once("://")?.1;
    let authority = authority.split(['/', '?', '#']).next()?;
    normalize_host(&format!("https://{authority}"))
}

fn query_browser_url(application: &str) -> Result<(String, u64), BrowserAccessStatus> {
    let script = match application {
        "Google Chrome" => {
            "tell application \"Google Chrome\"\n  set activeTab to active tab of front window\n  return (URL of activeTab) & (ASCII character 9) & (index of activeTab as text) & (ASCII character 9) & (title of activeTab)\nend tell"
        }
        "Google Chrome Canary" => {
            "tell application \"Google Chrome Canary\"\n  set activeTab to active tab of front window\n  return (URL of activeTab) & (ASCII character 9) & (index of activeTab as text) & (ASCII character 9) & (title of activeTab)\nend tell"
        }
        "Safari" => {
            "tell application \"Safari\"\n  set activeTab to current tab of front window\n  return (URL of activeTab) & (ASCII character 9) & (index of activeTab as text) & (ASCII character 9) & (name of activeTab)\nend tell"
        }
        "Arc" => {
            "tell application \"Arc\"\n  set activeTab to active tab of front window\n  return (URL of activeTab) & (ASCII character 9) & (index of activeTab as text) & (ASCII character 9) & (title of activeTab)\nend tell"
        }
        "Brave Browser" => {
            "tell application \"Brave Browser\"\n  set activeTab to active tab of front window\n  return (URL of activeTab) & (ASCII character 9) & (index of activeTab as text) & (ASCII character 9) & (title of activeTab)\nend tell"
        }
        "Microsoft Edge" => {
            "tell application \"Microsoft Edge\"\n  set activeTab to active tab of front window\n  return (URL of activeTab) & (ASCII character 9) & (index of activeTab as text) & (ASCII character 9) & (title of activeTab)\nend tell"
        }
        "Firefox" => {
            "tell application \"Firefox\"\n  set activeTab to active tab of front window\n  return (URL of activeTab) & (ASCII character 9) & (index of activeTab as text) & (ASCII character 9) & (title of activeTab)\nend tell"
        }
        "Vivaldi" => {
            "tell application \"Vivaldi\"\n  set activeTab to active tab of front window\n  return (URL of activeTab) & (ASCII character 9) & (index of activeTab as text) & (ASCII character 9) & (title of activeTab)\nend tell"
        }
        "Opera" => {
            "tell application \"Opera\"\n  set activeTab to active tab of front window\n  return (URL of activeTab) & (ASCII character 9) & (index of activeTab as text) & (ASCII character 9) & (title of activeTab)\nend tell"
        }
        "Orion" => {
            "tell application \"Orion\"\n  set activeTab to active tab of front window\n  return (URL of activeTab) & (ASCII character 9) & (index of activeTab as text) & (ASCII character 9) & (title of activeTab)\nend tell"
        }
        _ => return Err(BrowserAccessStatus::NotApplicable),
    };
    let mut child = Command::new("osascript")
        .args(["-e", script])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| BrowserAccessStatus::Unavailable)?;
    let deadline = std::time::Instant::now() + Duration::from_millis(BROWSER_QUERY_TIMEOUT_MS);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let output = child
                    .wait_with_output()
                    .map_err(|_| BrowserAccessStatus::Unavailable)?;
                let identity = String::from_utf8(output.stdout)
                    .map_err(|_| BrowserAccessStatus::NeedsPermission)?;
                let mut fields = identity.trim().split('\t');
                let raw_url = fields.next().unwrap_or_default();
                let host = extract_host(raw_url).ok_or(BrowserAccessStatus::NeedsPermission)?;
                // Keep the full tab identity local and hash it immediately.
                // If an adapter only returns a URL, the URL remains a safe
                // conservative fallback; adapters with tab metadata also
                // reject same-URL tab switches.
                let tab_metadata = fields.collect::<Vec<_>>().join("\t");
                let fingerprint_input = if tab_metadata.is_empty() {
                    raw_url.to_owned()
                } else {
                    format!("{raw_url}\t{tab_metadata}")
                };
                return Ok((host, browser_target_token(&fingerprint_input)));
            }
            Ok(Some(_)) => return Err(BrowserAccessStatus::NeedsPermission),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(BrowserAccessStatus::Unavailable);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(BrowserAccessStatus::Unavailable);
            }
        }
    }
}

fn browser_target_token(raw_url: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    raw_url.trim().hash(&mut hasher);
    hasher.finish()
}

#[cfg(target_os = "macos")]
#[derive(Debug, Default)]
struct WindowIdentity {
    title: String,
    focus_kind: FocusKind,
    window_token: Option<u64>,
    window_id: Option<u64>,
    input_token: Option<u64>,
}

#[cfg(target_os = "macos")]
fn identity_token(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    Some(hasher.finish())
}

#[cfg(target_os = "macos")]
type WindowDictionary =
    core_foundation::dictionary::CFDictionary<*const core::ffi::c_void, *const core::ffi::c_void>;

#[cfg(target_os = "macos")]
fn cf_number(
    dictionary: &WindowDictionary,
    key: &core_foundation::string::CFString,
) -> Option<i64> {
    use core_foundation::base::TCFType;
    use core_foundation::number::CFNumber;

    dictionary
        .find(key.as_CFTypeRef())
        .map(|value| unsafe { CFNumber::wrap_under_get_rule(*value as _) }.to_i64())?
}

#[cfg(target_os = "macos")]
fn cf_string(
    dictionary: &WindowDictionary,
    key: &core_foundation::string::CFString,
) -> Option<String> {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;

    dictionary
        .find(key.as_CFTypeRef())
        .map(|value| unsafe { CFString::wrap_under_get_rule(*value as _) }.to_string())
}

#[cfg(target_os = "macos")]
fn window_bounds(
    dictionary: &WindowDictionary,
    bounds_key: &core_foundation::string::CFString,
    x_key: &core_foundation::string::CFString,
    y_key: &core_foundation::string::CFString,
    width_key: &core_foundation::string::CFString,
    height_key: &core_foundation::string::CFString,
) -> Option<(f64, f64, f64, f64)> {
    use core_foundation::base::TCFType;

    let bounds = dictionary
        .find(bounds_key.as_CFTypeRef())
        .map(|value| unsafe { WindowDictionary::wrap_under_get_rule(*value as _) })?;
    Some((
        cf_number(&bounds, x_key)? as f64,
        cf_number(&bounds, y_key)? as f64,
        cf_number(&bounds, width_key)? as f64,
        cf_number(&bounds, height_key)? as f64,
    ))
}

#[cfg(target_os = "macos")]
fn parse_pair(value: &str) -> Option<(f64, f64)> {
    let values = value
        .split(|character: char| {
            !character.is_ascii_digit() && character != '.' && character != '-'
        })
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<f64>().ok())
        .collect::<Vec<_>>();
    (values.len() == 2).then(|| (values[0], values[1]))
}

#[cfg(target_os = "macos")]
fn bounds_match(
    candidate: (f64, f64, f64, f64),
    position: Option<(f64, f64)>,
    size: Option<(f64, f64)>,
) -> bool {
    let Some((x, y)) = position else { return false };
    let Some((width, height)) = size else {
        return false;
    };
    (candidate.0 - x).abs() <= 2.0
        && (candidate.1 - y).abs() <= 2.0
        && (candidate.2 - width).abs() <= 2.0
        && (candidate.3 - height).abs() <= 2.0
}

/// Resolve the Accessibility front window to a stable CoreGraphics window
/// number. If the list cannot identify exactly one candidate, return `None` so
/// delivery falls back to the clipboard rather than guessing.
#[cfg(target_os = "macos")]
fn query_window_id(pid: i32, title: &str, position: &str, size: &str) -> Option<u64> {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;
    use core_graphics::window::{
        copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer,
        kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowName,
        kCGWindowNumber, kCGWindowOwnerPID,
    };

    let dictionaries = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    )?;
    let owner_pid_key = unsafe { CFString::wrap_under_get_rule(kCGWindowOwnerPID) };
    let window_number_key = unsafe { CFString::wrap_under_get_rule(kCGWindowNumber) };
    let layer_key = unsafe { CFString::wrap_under_get_rule(kCGWindowLayer) };
    let name_key = unsafe { CFString::wrap_under_get_rule(kCGWindowName) };
    let bounds_key = unsafe { CFString::wrap_under_get_rule(kCGWindowBounds) };
    let x_key = CFString::from("X");
    let y_key = CFString::from("Y");
    let width_key = CFString::from("Width");
    let height_key = CFString::from("Height");
    let position = parse_pair(position);
    let size = parse_pair(size);

    struct Candidate {
        id: u64,
        name: Option<String>,
        bounds: Option<(f64, f64, f64, f64)>,
    }

    let mut candidates = Vec::new();
    for raw in dictionaries.get_all_values() {
        if raw.is_null() {
            continue;
        }
        let dictionary = unsafe { WindowDictionary::wrap_under_get_rule(raw as _) };
        if cf_number(&dictionary, &owner_pid_key) != Some(pid as i64)
            || cf_number(&dictionary, &layer_key).unwrap_or(0) != 0
        {
            continue;
        }
        let Some(id) = cf_number(&dictionary, &window_number_key)
            .filter(|id| *id > 0)
            .map(|id| id as u64)
        else {
            continue;
        };
        candidates.push(Candidate {
            id,
            name: cf_string(&dictionary, &name_key).filter(|name| !name.trim().is_empty()),
            bounds: window_bounds(
                &dictionary,
                &bounds_key,
                &x_key,
                &y_key,
                &width_key,
                &height_key,
            ),
        });
    }

    let exact_title = candidates
        .iter()
        .filter(|candidate| candidate.name.as_deref() == Some(title))
        .collect::<Vec<_>>();
    if exact_title.len() == 1 {
        return Some(exact_title[0].id);
    }
    if !exact_title.is_empty() {
        let matching_bounds = exact_title
            .iter()
            .filter(|candidate| {
                candidate
                    .bounds
                    .is_some_and(|bounds| bounds_match(bounds, position, size))
            })
            .collect::<Vec<_>>();
        return (matching_bounds.len() == 1).then(|| matching_bounds[0].id);
    }

    let matching_bounds = candidates
        .iter()
        .filter(|candidate| {
            candidate
                .bounds
                .is_some_and(|bounds| bounds_match(bounds, position, size))
        })
        .collect::<Vec<_>>();
    if matching_bounds.len() == 1 {
        return Some(matching_bounds[0].id);
    }
    (candidates.len() == 1).then(|| candidates[0].id)
}

/// Look up the on-screen bounds for a recorded CoreGraphics window number so
/// delivery can AXRaise that window instead of a different window of the same app.
#[cfg(target_os = "macos")]
pub fn window_bounds_for_id(pid: i32, window_id: u64) -> Option<(f64, f64, f64, f64)> {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;
    use core_graphics::window::{
        copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer,
        kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowNumber,
        kCGWindowOwnerPID,
    };

    if pid <= 0 || window_id == 0 {
        return None;
    }
    let dictionaries = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    )?;
    let owner_pid_key = unsafe { CFString::wrap_under_get_rule(kCGWindowOwnerPID) };
    let window_number_key = unsafe { CFString::wrap_under_get_rule(kCGWindowNumber) };
    let layer_key = unsafe { CFString::wrap_under_get_rule(kCGWindowLayer) };
    let bounds_key = unsafe { CFString::wrap_under_get_rule(kCGWindowBounds) };
    let x_key = CFString::from("X");
    let y_key = CFString::from("Y");
    let width_key = CFString::from("Width");
    let height_key = CFString::from("Height");
    for raw in dictionaries.get_all_values() {
        if raw.is_null() {
            continue;
        }
        let dictionary = unsafe { WindowDictionary::wrap_under_get_rule(raw as _) };
        if cf_number(&dictionary, &owner_pid_key) != Some(pid as i64)
            || cf_number(&dictionary, &layer_key).unwrap_or(0) != 0
        {
            continue;
        }
        if cf_number(&dictionary, &window_number_key).map(|id| id as u64) != Some(window_id) {
            continue;
        }
        return window_bounds(
            &dictionary,
            &bounds_key,
            &x_key,
            &y_key,
            &width_key,
            &height_key,
        );
    }
    None
}

#[cfg(not(target_os = "macos"))]
pub fn window_bounds_for_id(_pid: i32, _window_id: u64) -> Option<(f64, f64, f64, f64)> {
    None
}

fn focus_marker(value: &str, marker: &str) -> bool {
    if marker.contains(' ') {
        value.contains(marker)
    } else {
        value
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|part| part == marker)
    }
}

fn classify_focus(
    role: &str,
    subrole: &str,
    window_title: &str,
    focused_description: &str,
    focused_title: &str,
) -> FocusKind {
    let role_value = format!("{role} {subrole}").to_ascii_lowercase();
    if role_value.contains("securetextfield") || role_value.contains("secure text field") {
        return FocusKind::Secure;
    }
    let editable = role_value.contains("textfield")
        || role_value.contains("textarea")
        || role_value.contains("combobox");
    if !editable {
        return FocusKind::Unknown;
    }
    let value = format!("{role_value} {window_title} {focused_description} {focused_title}")
        .to_ascii_lowercase();
    if ["search", "query", "find", "address bar"]
        .iter()
        .any(|marker| focus_marker(&value, marker))
    {
        return FocusKind::Search;
    }
    if value.contains("terminal") || value.contains("shell") || value.contains("command line") {
        return FocusKind::Terminal;
    }
    if [
        "email",
        "e-mail",
        "subject",
        "recipient",
        "compose",
        "reply",
        "forward",
        "cc",
        "bcc",
    ]
    .iter()
    .any(|marker| focus_marker(&value, marker))
    {
        return FocusKind::Email;
    }
    if ["message", "chat", "comment", "slack", "teams", "discord"]
        .iter()
        .any(|marker| focus_marker(&value, marker))
    {
        return FocusKind::Chat;
    }
    if [
        "document",
        "notion",
        "notes",
        "journal",
        "paragraph",
        "rich text",
    ]
    .iter()
    .any(|marker| focus_marker(&value, marker))
    {
        return FocusKind::Document;
    }
    if [
        "first name",
        "last name",
        "phone",
        "address",
        "postal",
        "zip code",
        "amount",
        "date",
        "website",
    ]
    .iter()
    .any(|marker| focus_marker(&value, marker))
    {
        return FocusKind::Form;
    }
    if value.contains("code") || value.contains("editor") {
        return FocusKind::Code;
    }
    FocusKind::Editable
}

/// Read only a local identity for target safety and conservative scene hints.
/// The raw title and focused-element metadata are hashed before they can enter
/// a snapshot, history record, IPC payload, or provider request.
#[cfg(target_os = "macos")]
fn query_frontmost_window(pid: i32) -> WindowIdentity {
    const TIMEOUT_MS: u64 = 350;
    let script = r#"
tell application "System Events"
  set p to first application process whose frontmost is true
  set wTitle to ""
  set wPosition to ""
  set wSize to ""
  try
    set frontWindow to front window of p
    set wTitle to name of frontWindow
    try
      set wPosition to (position of frontWindow) as text
      set wSize to (size of frontWindow) as text
    end try
  end try
  set focusedRole to ""
  set focusedSubrole to ""
  set focusedDescription to ""
  set focusedTitle to ""
  set focusedPosition to ""
  set focusedSize to ""
    try
    -- System Events does not expose a `focused UI element` property on the
    -- process object. Read the standard Accessibility attribute instead.
    set focusedElement to value of attribute "AXFocusedUIElement" of p
    set focusedRole to role of focusedElement
    try
      set focusedSubrole to subrole of focusedElement
    end try
    try
      set focusedDescription to description of focusedElement
    end try
    try
      set focusedTitle to title of focusedElement
    end try
    try
      set focusedPosition to (position of focusedElement) as text
      set focusedSize to (size of focusedElement) as text
    end try
  end try
  set separator to (ASCII character 9)
  return wTitle & separator & wPosition & separator & wSize & separator & focusedRole & separator & focusedSubrole & separator & focusedDescription & separator & focusedTitle & separator & focusedPosition & separator & focusedSize
end tell
"#;
    let mut child = match Command::new("osascript")
        .args(["-e", script])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return WindowIdentity::default(),
    };
    let deadline = std::time::Instant::now() + Duration::from_millis(TIMEOUT_MS);
    let output = loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => match child.wait_with_output() {
                Ok(output) => break output,
                Err(_) => return WindowIdentity::default(),
            },
            Ok(Some(_)) => return WindowIdentity::default(),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(15));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return WindowIdentity::default();
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return WindowIdentity::default();
            }
        }
    };
    let raw = String::from_utf8_lossy(&output.stdout);
    let mut fields = raw.trim().split('\t');
    let title = fields.next().unwrap_or_default().trim().to_owned();
    let window_position = fields.next().unwrap_or_default().trim();
    let window_size = fields.next().unwrap_or_default().trim();
    let role = fields
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let subrole = fields
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let description = fields.next().unwrap_or_default().trim();
    let focused_title = fields.next().unwrap_or_default().trim();
    let focused_position = fields.next().unwrap_or_default().trim();
    let focused_size = fields.next().unwrap_or_default().trim();
    let focus_kind = classify_focus(&role, &subrole, &title, description, focused_title);
    let input_token = focus_kind
        .is_editable()
        .then(|| {
            identity_token(&format!(
                "{role}\t{subrole}\t{description}\t{focused_title}\t{focused_position}\t{focused_size}"
            ))
        })
        .flatten();
    WindowIdentity {
        window_token: identity_token(&format!("{title}\t{window_position}\t{window_size}")),
        window_id: query_window_id(pid, &title, window_position, window_size),
        input_token,
        focus_kind,
        title,
    }
}

/// Best-effort local read of the focused Accessibility value. The value is
/// used only in memory to compare the input before and after a Cmd+V; it is
/// never serialized, persisted, or sent to a provider. `None` means macOS
/// could not expose a readable value for this control.
#[cfg(target_os = "macos")]
pub fn focused_input_value() -> Option<String> {
    const TIMEOUT_MS: u64 = 350;
    const UNAVAILABLE: &str = "__VOICEFLOW_AX_UNAVAILABLE__";
    let script = r#"
tell application "System Events"
  try
    set p to first application process whose frontmost is true
    set focusedElement to value of attribute "AXFocusedUIElement" of p
    try
      return (value of focusedElement) as text
    on error
      return "__VOICEFLOW_AX_UNAVAILABLE__"
    end try
  on error
    return "__VOICEFLOW_AX_UNAVAILABLE__"
  end try
end tell
"#;
    let mut child = Command::new("osascript")
        .args(["-e", script])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + Duration::from_millis(TIMEOUT_MS);
    let output = loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break child.wait_with_output().ok()?,
            Ok(Some(_)) => return None,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(15));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Err(_) => return None,
        }
    };
    let value = String::from_utf8_lossy(&output.stdout)
        .trim_end_matches(['\r', '\n'])
        .to_owned();
    (value != UNAVAILABLE).then_some(value)
}

#[cfg(not(target_os = "macos"))]
pub fn focused_input_value() -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
fn frontmost_signal(browser_access_enabled: bool) -> AppSignal {
    use std::ffi::CStr;
    use std::os::raw::c_char;

    fn ns_string(value: *mut objc::runtime::Object) -> Option<String> {
        if value.is_null() {
            return None;
        }
        unsafe {
            use objc::{msg_send, sel, sel_impl};
            let bytes: *const c_char = msg_send![value, UTF8String];
            if bytes.is_null() {
                None
            } else {
                CStr::from_ptr(bytes).to_str().ok().map(str::to_owned)
            }
        }
    }

    unsafe {
        use objc::{class, msg_send, sel, sel_impl};
        let workspace: *mut objc::runtime::Object = msg_send![class!(NSWorkspace), sharedWorkspace];
        let application: *mut objc::runtime::Object = msg_send![workspace, frontmostApplication];
        if application.is_null() {
            return AppSignal::default();
        }
        let pid: i32 = msg_send![application, processIdentifier];
        let bundle_id = ns_string(msg_send![application, bundleIdentifier]);
        let process_name = ns_string(msg_send![application, localizedName]).unwrap_or_default();
        // NSWorkspace app identity is available without Accessibility. Do not
        // invoke System Events until the user has granted Accessibility: the
        // window/focused-element probe is both unnecessary for the native
        // profile preview and can trigger an avoidable macOS permission flow.
        let window = if crate::permissions::accessibility_is_trusted() {
            query_frontmost_window(pid)
        } else {
            WindowIdentity::default()
        };
        let browser_application = browser_application(bundle_id.as_deref(), &process_name);
        let (browser_host, browser_target_token, browser_access_status) =
            if let Some(browser_application) = browser_application {
                if browser_access_enabled {
                    match query_browser_url(browser_application) {
                        Ok((host, token)) => {
                            (Some(host), Some(token), BrowserAccessStatus::Granted)
                        }
                        Err(status) => (None, None, status),
                    }
                } else {
                    (None, None, BrowserAccessStatus::Disabled)
                }
            } else {
                (None, None, BrowserAccessStatus::NotApplicable)
            };
        AppSignal {
            pid,
            bundle_id,
            process_name,
            window_title: window.title,
            focus_kind: window.focus_kind,
            browser_host,
            browser_target_token,
            window_token: window.window_token,
            window_id: window.window_id,
            input_token: window.input_token,
            browser_access_status,
        }
    }
}

/// Read the cheap part of context detection used by the idle preview loop.
/// The expensive Accessibility/browser probe remains reserved for an actual
/// frontmost-app change or the fresh capture taken at recording start.
#[cfg(target_os = "macos")]
pub fn frontmost_application_key() -> (i32, Option<String>) {
    use std::ffi::CStr;
    use std::os::raw::c_char;

    unsafe {
        use objc::{class, msg_send, sel, sel_impl};

        let workspace: *mut objc::runtime::Object = msg_send![class!(NSWorkspace), sharedWorkspace];
        let application: *mut objc::runtime::Object = msg_send![workspace, frontmostApplication];
        if application.is_null() {
            return (0, None);
        }
        let pid: i32 = msg_send![application, processIdentifier];
        let bundle: *mut objc::runtime::Object = msg_send![application, bundleIdentifier];
        let bundle_id = if bundle.is_null() {
            None
        } else {
            let bytes: *const c_char = msg_send![bundle, UTF8String];
            (!bytes.is_null()).then(|| CStr::from_ptr(bytes).to_string_lossy().into_owned())
        };
        (pid, bundle_id)
    }
}

#[cfg(not(target_os = "macos"))]
fn frontmost_signal(_browser_access_enabled: bool) -> AppSignal {
    AppSignal::default()
}

#[cfg(not(target_os = "macos"))]
pub fn frontmost_application_key() -> (i32, Option<String>) {
    (0, None)
}

pub fn detect_snapshot(mappings: &[AppMapping], browser_access_enabled: bool) -> ContextSnapshot {
    detect_snapshot_with_modes(mappings, browser_access_enabled, &builtin_writing_modes())
}

pub fn detect_snapshot_with_modes(
    mappings: &[AppMapping],
    browser_access_enabled: bool,
    writing_modes: &[WritingMode],
) -> ContextSnapshot {
    snapshot_for_signal_with_modes(
        &frontmost_signal(browser_access_enabled),
        mappings,
        browser_access_enabled,
        writing_modes,
    )
}

#[cfg(target_os = "macos")]
pub fn available_applications() -> Vec<ApplicationOption> {
    use std::ffi::CStr;
    use std::os::raw::c_char;

    fn ns_string(value: *mut objc::runtime::Object) -> Option<String> {
        if value.is_null() {
            return None;
        }
        unsafe {
            use objc::{msg_send, sel, sel_impl};
            let bytes: *const c_char = msg_send![value, UTF8String];
            if bytes.is_null() {
                None
            } else {
                CStr::from_ptr(bytes).to_str().ok().map(str::to_owned)
            }
        }
    }

    let mut applications = unsafe {
        use objc::{class, msg_send, sel, sel_impl};
        let workspace: *mut objc::runtime::Object = msg_send![class!(NSWorkspace), sharedWorkspace];
        let running: *mut objc::runtime::Object = msg_send![workspace, runningApplications];
        let count: usize = msg_send![running, count];
        let mut result = Vec::with_capacity(count);
        for index in 0..count {
            let application: *mut objc::runtime::Object = msg_send![running, objectAtIndex: index];
            let activation_policy: isize = msg_send![application, activationPolicy];
            let Some(bundle_id) = ns_string(msg_send![application, bundleIdentifier]) else {
                continue;
            };
            if activation_policy != 0 || bundle_id == "com.voiceflow.desktop" {
                continue;
            }
            let Some(label) = ns_string(msg_send![application, localizedName]) else {
                continue;
            };
            result.push(ApplicationOption { bundle_id, label });
        }
        result
    };

    applications.sort_by_cached_key(|application| application.label.to_ascii_lowercase());
    applications.dedup_by(|left, right| left.bundle_id == right.bundle_id);
    applications
}

#[cfg(not(target_os = "macos"))]
pub fn available_applications() -> Vec<ApplicationOption> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn read_application_plist_value(plist: &std::path::Path, key: &str) -> Result<String, String> {
    let plist_path = plist
        .to_str()
        .ok_or_else(|| "应用路径无法读取".to_owned())?;
    let output = Command::new("/usr/bin/plutil")
        .args(["-extract", key, "raw", "-o", "-", plist_path])
        .output()
        .map_err(|_| "无法读取应用信息".to_owned())?;
    if !output.status.success() {
        return Err("应用信息中缺少必要字段".to_owned());
    }
    let value = String::from_utf8(output.stdout).map_err(|_| "应用信息格式无效".to_owned())?;
    let value = value.trim();
    if value.is_empty() {
        return Err("应用信息中缺少必要字段".to_owned());
    }
    Ok(value.to_owned())
}

#[cfg(target_os = "macos")]
pub fn application_from_path(path: &str) -> Result<ApplicationOption, String> {
    let app_path = std::path::Path::new(path);
    let is_app_bundle = app_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"));
    if !is_app_bundle || !app_path.is_dir() {
        return Err("请选择一个 .app 应用".to_owned());
    }

    let plist = app_path.join("Contents").join("Info.plist");
    if !plist.is_file() {
        return Err("这个应用缺少有效的应用信息".to_owned());
    }
    let bundle_id = read_application_plist_value(&plist, "CFBundleIdentifier")?;
    if bundle_id == "com.voiceflow.desktop" {
        return Err("不能把 VoiceFlow 添加到自己的应用映射中".to_owned());
    }
    let label = read_application_plist_value(&plist, "CFBundleDisplayName")
        .or_else(|_| read_application_plist_value(&plist, "CFBundleName"))
        .or_else(|_| {
            app_path
                .file_stem()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| "无法读取应用名称".to_owned())
        })?;
    Ok(ApplicationOption { bundle_id, label })
}

#[cfg(not(target_os = "macos"))]
pub fn application_from_path(_path: &str) -> Result<ApplicationOption, String> {
    Err("从应用程序文件夹选择仅支持 macOS".to_owned())
}

#[allow(dead_code)]
pub fn detect_snapshot_for_state(
    enabled: bool,
    mappings: &[AppMapping],
    browser_access_enabled: bool,
) -> ContextSnapshot {
    detect_snapshot_for_state_with_modes(
        enabled,
        mappings,
        browser_access_enabled,
        &builtin_writing_modes(),
    )
}

pub fn detect_snapshot_for_state_with_modes(
    enabled: bool,
    mappings: &[AppMapping],
    browser_access_enabled: bool,
    writing_modes: &[WritingMode],
) -> ContextSnapshot {
    snapshot_for_enabled_state(
        detect_snapshot_with_modes(mappings, browser_access_enabled, writing_modes),
        enabled,
        writing_modes,
    )
}

fn snapshot_for_enabled_state(
    detected: ContextSnapshot,
    enabled: bool,
    writing_modes: &[WritingMode],
) -> ContextSnapshot {
    if enabled {
        return detected;
    }
    let mut snapshot = detected;
    snapshot.policy = policy_for_mode(ContextFamily::General, Some("general"), writing_modes);
    snapshot.profile.family = ContextFamily::General;
    snapshot.profile.writing_mode_id = Some("general".into());
    snapshot.profile.id = "general".into();
    snapshot.profile.confidence = 0.4;
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal(bundle_id: &str, process_name: &str, host: Option<&str>) -> AppSignal {
        AppSignal {
            pid: 42,
            bundle_id: Some(bundle_id.into()),
            process_name: process_name.into(),
            window_title: String::new(),
            focus_kind: FocusKind::Unknown,
            browser_host: host.map(str::to_owned),
            browser_target_token: None,
            window_token: None,
            window_id: None,
            input_token: None,
            browser_access_status: BrowserAccessStatus::Granted,
        }
    }

    #[test]
    fn native_cursor_resolves_to_code_policy() {
        let snapshot = snapshot_for_signal(
            &signal("com.todesktop.230313mzl4w4u92", "Cursor", None),
            &[],
            false,
        );
        assert_eq!(snapshot.profile.id, "code.cursor");
        assert!(snapshot.policy.preserve_technical_tokens);
        assert_eq!(display_label(&snapshot), "Cursor · 代码");
        assert_eq!(hud_style_id(&snapshot), "prompt_or_code");
        assert_eq!(display_app_name(&snapshot), Some("Cursor"));
    }

    #[test]
    fn display_label_keeps_a_known_app_when_confidence_is_low() {
        let snapshot = snapshot_for_signal(
            &signal("com.apple.Safari", "Safari", None),
            &[],
            false,
        );
        assert!(snapshot.profile.confidence < 0.75);
        assert_eq!(display_label(&snapshot), "Safari · 通用");
        assert_eq!(hud_style_id(&snapshot), "general");
        assert_eq!(display_app_name(&snapshot), Some("Safari"));
    }

    #[test]
    fn chrome_without_a_mapped_site_shows_chrome_not_unknown_app() {
        let chrome = snapshot_for_signal(
            &signal("com.google.Chrome", "Google Chrome", None),
            &[],
            false,
        );
        assert!(chrome.profile.confidence < 0.75);
        assert_eq!(display_label(&chrome), "Chrome · 通用");
        assert_eq!(display_app_name(&chrome), Some("Chrome"));

        let canary = snapshot_for_signal(
            &signal("com.google.Chrome.canary", "Google Chrome Canary", None),
            &[],
            false,
        );
        assert_eq!(display_label(&canary), "Chrome Canary · 通用");
        assert_eq!(display_app_name(&canary), Some("Chrome Canary"));

        let unknown_site = snapshot_for_signal(
            &signal("com.google.Chrome", "Google Chrome", Some("example.com")),
            &[],
            true,
        );
        assert_eq!(unknown_site.profile.id, "browser.general");
        assert_eq!(display_label(&unknown_site), "Chrome · 通用");

        let mut placeholder = ContextSnapshot::general();
        placeholder.target_guard.bundle_id = Some("com.google.Chrome".into());
        assert_eq!(display_label(&placeholder), "Chrome · 通用");
    }

    #[test]
    fn chrome_on_gmail_keeps_the_site_label() {
        let gmail = snapshot_for_signal(
            &signal("com.google.Chrome", "Google Chrome", Some("gmail.com")),
            &[],
            true,
        );
        assert_eq!(display_label(&gmail), "Gmail · 邮件");
        assert_eq!(display_app_name(&gmail), Some("Gmail"));
    }

    #[test]
    fn display_label_hides_placeholder_general_app_names() {
        assert_eq!(display_label(&ContextSnapshot::general()), "未知应用 · 通用");
        assert_eq!(display_app_name(&ContextSnapshot::general()), None);
    }

    #[test]
    fn disabled_context_keeps_detected_app_with_general_style() {
        let detected = snapshot_for_signal(
            &signal("com.todesktop.230313mzl4w4u92", "Cursor", None),
            &[],
            false,
        );
        let snapshot = snapshot_for_enabled_state(detected, false, &builtin_writing_modes());
        assert_eq!(snapshot.profile.app_label, "Cursor");
        assert_eq!(snapshot.profile.family, ContextFamily::General);
        assert!(snapshot.profile.confidence < 0.75);
        assert!(!snapshot.policy.preserve_technical_tokens);
        assert_eq!(display_label(&snapshot), "Cursor · 通用");
        assert_eq!(hud_style_id(&snapshot), "general");
    }

    #[test]
    fn native_profiles_keep_the_active_app_label() {
        let outlook = snapshot_for_signal(
            &signal("com.microsoft.Outlook", "Microsoft Outlook", None),
            &[],
            false,
        );
        assert_eq!(display_label(&outlook), "Outlook · 邮件");

        let slack = snapshot_for_signal(
            &signal("com.tinyspeck.slackmacgap", "Slack", None),
            &[],
            false,
        );
        assert_eq!(display_label(&slack), "Slack · 工作短讯");

        let wechat = snapshot_for_signal(
            &signal("com.tencent.xinWeChat", "WeChat", None),
            &[],
            false,
        );
        assert_eq!(wechat.profile.app_label, "WeChat");
        assert_eq!(display_label(&wechat), "WeChat · 口语");
    }

    #[test]
    fn personal_and_work_chat_writing_prompts_differ() {
        let personal = default_writing_prompt(ContextFamily::PersonalChat);
        let work = default_writing_prompt(ContextFamily::WorkChat);
        assert_ne!(personal, work);
        assert_eq!(
            personal,
            "Keep the user's casual chat voice. Do not add greetings or sign-offs such as 您好, 你好, Hello, or Best. Do not expand fragments into full formal sentences. Do not sanitize swearing, slang, or particles like 哈哈. Add a question mark for a clear question and a period or 。 for a clear sentence end. Do not strip existing periods. Never turn the message into an email."
        );
        assert_eq!(
            work,
            "Keep the message short and conversational for workplace chat. Do not add greetings, sign-offs, or email structure. Keep names and project terms exact. Do not add emoji unless spoken."
        );
        assert_eq!(
            default_writing_prompt(ContextFamily::Email),
            "Write a natural, polite email body. Organize spoken greeting, request, timing, and closing only when they were spoken. Do not create a subject line or signature."
        );
        assert_eq!(
            default_writing_prompt(ContextFamily::PromptOrCode),
            "Preserve code, identifiers, paths, commands, API names, versions, and error text exactly. Organize a spoken coding request without generating code unless it was spoken. Drop spoken false starts; do not treat 不对 as an instruction to execute."
        );
    }

    #[test]
    fn personal_and_work_chat_policies_split_tone() {
        let personal = ContextPolicy::for_family(ContextFamily::PersonalChat);
        let work = ContextPolicy::for_family(ContextFamily::WorkChat);

        assert_eq!(personal.formality, "casual");
        assert_eq!(
            personal.sentence_completeness,
            "preserve_fragments_when_intentional"
        );
        assert!(personal
            .forbidden_additions
            .iter()
            .any(|item| item == "greetings not spoken"));
        assert!(personal
            .forbidden_additions
            .iter()
            .any(|item| item == "swear-word sanitization"));

        assert_eq!(work.formality, "neutral");
        assert_eq!(work.density, "concise");
        assert!(work
            .forbidden_additions
            .iter()
            .any(|item| item.contains("greetings")));

        assert_eq!(
            ContextPolicy::for_family(ContextFamily::ProjectManagement).list_behavior,
            "only_when_explicit"
        );
        assert_eq!(
            ContextPolicy::for_family(ContextFamily::Document).list_behavior,
            "only_when_explicit"
        );
    }

    #[test]
    fn normalize_migrates_the_old_shared_chat_prompt() {
        let mut modes = builtin_writing_modes();
        for mode in &mut modes {
            if mode.id == "work_chat" || mode.id == "personal_chat" {
                mode.prompt = OLD_SHARED_CHAT_PROMPT.into();
            }
        }
        modes.push(WritingMode {
            id: "custom.keep".into(),
            label: "Keep".into(),
            family: ContextFamily::General,
            prompt: OLD_SHARED_CHAT_PROMPT.into(),
            builtin: false,
        });

        normalize_writing_modes(&mut modes);

        let personal = modes
            .iter()
            .find(|mode| mode.id == "personal_chat")
            .unwrap();
        let work = modes.iter().find(|mode| mode.id == "work_chat").unwrap();
        let custom = modes.iter().find(|mode| mode.id == "custom.keep").unwrap();
        assert_eq!(
            personal.prompt,
            default_writing_prompt(ContextFamily::PersonalChat)
        );
        assert_eq!(work.prompt, default_writing_prompt(ContextFamily::WorkChat));
        assert_ne!(personal.prompt, work.prompt);
        assert_eq!(custom.prompt, OLD_SHARED_CHAT_PROMPT);
    }

    #[test]
    fn normalize_keeps_customized_builtin_chat_prompts() {
        let custom_work = "Keep Slack messages punchy and never add emoji.";
        let mut modes = builtin_writing_modes();
        modes
            .iter_mut()
            .find(|mode| mode.id == "work_chat")
            .unwrap()
            .prompt = custom_work.into();

        normalize_writing_modes(&mut modes);

        assert_eq!(
            modes
                .iter()
                .find(|mode| mode.id == "work_chat")
                .unwrap()
                .prompt,
            custom_work
        );
    }

    #[test]
    fn browser_host_resolves_to_email_without_storing_url() {
        let snapshot = snapshot_for_signal(
            &signal("com.google.Chrome", "Google Chrome", Some("gmail.com")),
            &[],
            true,
        );
        assert_eq!(snapshot.profile.family, ContextFamily::Email);
        assert_eq!(snapshot.profile.id, "email.gmail");
        assert!(!serde_json::to_string(&snapshot)
            .unwrap()
            .contains("https://"));
    }

    #[test]
    fn calendar_and_task_hosts_get_a_conservative_planning_policy() {
        let calendar = snapshot_for_signal(
            &signal(
                "com.google.Chrome",
                "Google Chrome",
                Some("calendar.google.com"),
            ),
            &[],
            true,
        );
        assert_eq!(calendar.profile.id, "calendar.google");
        assert_eq!(calendar.profile.family, ContextFamily::CalendarTask);
        assert_eq!(calendar.policy.artifact_kind, "calendar_or_task_entry");
        assert!(calendar
            .policy
            .forbidden_additions
            .iter()
            .any(|item| item.contains("attendees")));

        let todoist = snapshot_for_signal(
            &signal(
                "com.google.Chrome",
                "Google Chrome",
                Some("app.todoist.com"),
            ),
            &[],
            true,
        );
        assert_eq!(todoist.profile.id, "task.todoist");
        assert_eq!(display_label(&todoist), "Todoist · 日程");
    }

    #[test]
    fn native_calendar_apps_get_the_same_planning_family() {
        let calendar = snapshot_for_signal(&signal("com.apple.iCal", "Calendar", None), &[], false);
        assert_eq!(calendar.profile.id, "calendar.native");
        assert_eq!(calendar.profile.family, ContextFamily::CalendarTask);

        let reminders = snapshot_for_signal(
            &signal("com.apple.reminders", "Reminders", None),
            &[],
            false,
        );
        assert_eq!(reminders.profile.id, "reminders.native");
        assert_eq!(reminders.profile.family, ContextFamily::CalendarTask);
    }

    #[test]
    fn mapping_precedes_builtin_profile() {
        let mapping = AppMapping {
            id: "cursor-formal".into(),
            label: "Research writing".into(),
            family: ContextFamily::Document,
            mode_id: None,
            bundle_id: Some("com.todesktop.230313mzl4w4u92".into()),
            executable: None,
            browser_host: None,
            style_example_input: None,
            style_example_output: None,
            enabled: true,
            cleanup_effort: None,
            cleanup_enabled: true,
            dictionary_learn_enabled: true,
        };
        let snapshot = snapshot_for_signal(
            &signal("com.todesktop.230313mzl4w4u92", "Cursor", None),
            &[mapping],
            false,
        );
        assert_eq!(snapshot.profile.id, "user.cursor-formal");
        assert_eq!(snapshot.profile.family, ContextFamily::Document);
    }

    #[test]
    fn custom_mapping_resolves_to_the_user_prompt() {
        let custom = WritingMode {
            id: "custom.concise".into(),
            label: "我的简洁模式".into(),
            family: ContextFamily::General,
            prompt: "保留我的语气，只删除明显重复，不要总结。".into(),
            builtin: false,
        };
        let mapping = AppMapping {
            id: "cursor-custom".into(),
            label: "Cursor · 我的简洁模式".into(),
            family: ContextFamily::General,
            mode_id: Some(custom.id.clone()),
            bundle_id: Some("com.todesktop.230313mzl4w4u92".into()),
            executable: None,
            browser_host: None,
            style_example_input: None,
            style_example_output: None,
            enabled: true,
            cleanup_effort: None,
            cleanup_enabled: true,
            dictionary_learn_enabled: true,
        };
        let snapshot = snapshot_for_signal_with_modes(
            &signal("com.todesktop.230313mzl4w4u92", "Cursor", None),
            &[mapping],
            false,
            &[custom],
        );

        assert_eq!(
            snapshot.profile.writing_mode_id.as_deref(),
            Some("custom.concise")
        );
        assert_eq!(
            snapshot.policy.writing_prompt.as_deref(),
            Some("保留我的语气，只删除明显重复，不要总结。")
        );
    }

    #[test]
    fn normalize_preserves_builtin_prompt_and_keeps_valid_custom_modes() {
        let mut modes = vec![
            WritingMode {
                id: "email".into(),
                label: "邮件".into(),
                family: ContextFamily::Email,
                prompt: "  只保留邮件正文。  ".into(),
                builtin: true,
            },
            WritingMode {
                id: "custom.notes".into(),
                label: "  我的笔记  ".into(),
                family: ContextFamily::Email,
                prompt: "  保持第一人称。  ".into(),
                builtin: true,
            },
        ];

        normalize_writing_modes(&mut modes);

        assert_eq!(
            modes.iter().find(|mode| mode.id == "email").unwrap().prompt,
            "  只保留邮件正文。  "
        );
        let custom = modes.iter().find(|mode| mode.id == "custom.notes").unwrap();
        assert_eq!(custom.label, "我的笔记");
        assert_eq!(custom.prompt, "保持第一人称。");
        assert!(!custom.builtin);
        assert_eq!(custom.family, ContextFamily::General);
    }

    #[test]
    fn mapping_with_one_selector_does_not_match_missing_signals() {
        let mapping = AppMapping {
            id: "cursor-only".into(),
            label: "Cursor writing".into(),
            family: ContextFamily::PromptOrCode,
            mode_id: None,
            bundle_id: Some("com.todesktop.230313mzl4w4u92".into()),
            executable: None,
            browser_host: None,
            style_example_input: None,
            style_example_output: None,
            enabled: true,
            cleanup_effort: None,
            cleanup_enabled: true,
            dictionary_learn_enabled: true,
        };
        let snapshot = snapshot_for_signal(
            &signal("com.google.Chrome", "Google Chrome", Some("gmail.com")),
            &[mapping],
            true,
        );
        assert_eq!(snapshot.profile.family, ContextFamily::Email);
    }

    #[test]
    fn focus_identity_requires_an_editable_role() {
        assert_eq!(
            classify_focus("AXWindow", "", "Code", "", ""),
            FocusKind::Unknown
        );
        assert_eq!(
            classify_focus("AXWebArea", "AXWebArea", "Code", "", ""),
            FocusKind::Unknown
        );
        assert_eq!(
            classify_focus("AXTextArea", "", "Code", "", ""),
            FocusKind::Code
        );
        assert_eq!(
            classify_focus("AXTextField", "", "Gmail", "", "Subject"),
            FocusKind::Email
        );
        assert_eq!(
            classify_focus("AXTextArea", "", "Workspace", "", "Message"),
            FocusKind::Chat
        );
        assert_eq!(
            classify_focus("AXTextArea", "", "Notes", "", "Entry"),
            FocusKind::Document
        );
        assert_eq!(
            classify_focus("AXTextField", "", "Checkout", "", "Postal code"),
            FocusKind::Form
        );
    }

    #[test]
    fn focused_code_editor_resolves_to_code_policy_for_unknown_app() {
        let mut signal = signal("com.example.editor", "Example Editor", None);
        signal.focus_kind = FocusKind::Code;
        let snapshot = snapshot_for_signal(&signal, &[], false);
        assert_eq!(snapshot.profile.id, "code.focused");
        assert_eq!(snapshot.profile.source, ContextSource::FocusedInput);
        assert!(snapshot.policy.preserve_technical_tokens);
    }

    #[test]
    fn manual_override_changes_policy_but_preserves_target_guard() {
        let signal = signal("com.example.editor", "Example Editor", None);
        let mut snapshot = snapshot_for_signal(&signal, &[], false);
        let target = snapshot.target_guard.clone();

        apply_manual_override(&mut snapshot, Some(ContextFamily::Email));

        assert_eq!(snapshot.profile.id, "manual.email");
        assert_eq!(snapshot.profile.source, ContextSource::ManualOverride);
        assert_eq!(snapshot.profile.family, ContextFamily::Email);
        assert_eq!(snapshot.policy.artifact_kind, "email_body");
        assert_eq!(snapshot.target_guard, target);
    }

    #[test]
    fn unknown_browser_uses_focused_scene_before_general_fallback() {
        let mut signal = signal("com.google.Chrome", "Google Chrome", Some("example.com"));
        signal.focus_kind = FocusKind::Email;
        let email = snapshot_for_signal(&signal, &[], true);
        assert_eq!(email.profile.family, ContextFamily::Email);
        assert_eq!(email.profile.source, ContextSource::FocusedInput);

        signal.focus_kind = FocusKind::Form;
        let form = snapshot_for_signal(&signal, &[], true);
        assert_eq!(form.profile.family, ContextFamily::FormFilling);
        assert_eq!(form.profile.source, ContextSource::FocusedInput);

        signal.focus_kind = FocusKind::Unknown;
        let general = snapshot_for_signal(&signal, &[], true);
        assert_eq!(general.profile.id, "browser.general");
        assert_eq!(general.profile.family, ContextFamily::General);
    }

    #[test]
    fn window_title_adds_conservative_scene_hints_for_unknown_apps() {
        let mut signal = signal("com.example.browser", "Example Browser", None);
        signal.window_title = "Google Docs — Project notes".into();
        let document = snapshot_for_signal(&signal, &[], false);
        assert_eq!(document.profile.family, ContextFamily::Document);

        signal.window_title = "Calendar — Tuesday planning".into();
        let calendar = snapshot_for_signal(&signal, &[], false);
        assert_eq!(calendar.profile.family, ContextFamily::CalendarTask);

        signal.window_title = "Checkout — Shipping address".into();
        let form = snapshot_for_signal(&signal, &[], false);
        assert_eq!(form.profile.family, ContextFamily::FormFilling);
    }

    #[test]
    fn target_guard_rejects_app_or_host_changes() {
        let original = TargetAppGuard {
            pid: 1,
            bundle_id: Some("com.google.Chrome".into()),
            browser_host: Some("gmail.com".into()),
            browser_target_token: Some(1),
            window_token: Some(10),
            window_id: Some(100),
            input_token: Some(20),
            secure_input: false,
        };
        let changed_host = TargetAppGuard {
            browser_host: Some("github.com".into()),
            ..original.clone()
        };
        assert!(!target_matches(&original, &changed_host));
        assert!(!target_matches(
            &original,
            &TargetAppGuard {
                browser_target_token: Some(2),
                ..original.clone()
            }
        ));
        assert!(target_matches(
            &TargetAppGuard {
                browser_host: None,
                browser_target_token: None,
                window_token: None,
                window_id: None,
                input_token: None,
                ..original.clone()
            },
            &TargetAppGuard {
                browser_host: None,
                browser_target_token: None,
                window_token: None,
                input_token: None,
                ..original.clone()
            }
        ));
        assert!(!target_matches(
            &TargetAppGuard {
                pid: 0,
                ..original.clone()
            },
            &original
        ));
        assert!(!target_matches(
            &original,
            &TargetAppGuard {
                pid: 2,
                ..original.clone()
            }
        ));
        assert!(target_matches(
            &original,
            &TargetAppGuard {
                window_token: Some(11),
                input_token: Some(21),
                ..original.clone()
            }
        ));
        assert!(!target_matches(
            &original,
            &TargetAppGuard {
                window_id: Some(101),
                ..original.clone()
            }
        ));
        assert!(target_matches(
            &TargetAppGuard {
                window_id: None,
                ..original.clone()
            },
            &original
        ));
        assert_eq!(
            target_mismatch_reason(
                &original,
                &TargetAppGuard {
                    browser_target_token: None,
                    ..original.clone()
                }
            ),
            Some("target_unavailable")
        );
        assert_eq!(
            target_mismatch_reason(
                &original,
                &TargetAppGuard {
                    browser_host: None,
                    ..original.clone()
                }
            ),
            Some("target_unavailable")
        );
        assert_eq!(
            target_mismatch_reason(
                &original,
                &TargetAppGuard {
                    secure_input: true,
                    ..original.clone()
                }
            ),
            Some("secure_input")
        );
    }

    #[test]
    fn delivery_guard_ignores_title_and_input_tokens() {
        let original = TargetAppGuard {
            pid: 1,
            bundle_id: Some("com.todesktop.230313mzl4w4u92".into()),
            browser_host: None,
            browser_target_token: None,
            window_token: Some(10),
            window_id: Some(100),
            input_token: Some(20),
            secure_input: false,
        };
        assert!(target_matches(
            &original,
            &TargetAppGuard {
                window_token: Some(11),
                input_token: Some(21),
                ..original.clone()
            }
        ));
        assert!(!target_matches(
            &original,
            &TargetAppGuard {
                window_id: Some(101),
                ..original.clone()
            }
        ));
        assert!(!target_matches(
            &original,
            &TargetAppGuard {
                pid: 2,
                ..original.clone()
            }
        ));
    }

    #[test]
    fn target_guard_rejects_same_fingerprint_from_another_window() {
        let original = TargetAppGuard {
            pid: 42,
            bundle_id: Some("com.example.editor".into()),
            browser_host: None,
            browser_target_token: None,
            window_token: Some(7),
            window_id: Some(1001),
            input_token: Some(9),
            secure_input: false,
        };
        let same_title_and_geometry = TargetAppGuard {
            window_id: Some(1002),
            ..original.clone()
        };
        assert!(!target_matches(&original, &same_title_and_geometry));
    }

    #[test]
    fn secure_text_fields_are_not_editable_delivery_targets() {
        assert_eq!(
            classify_focus("AXTextField", "AXSecureTextField", "", "", ""),
            FocusKind::Secure
        );
        assert!(!FocusKind::Secure.is_editable());
        let mut signal = signal("com.example.login", "Login", None);
        signal.focus_kind = FocusKind::Secure;
        assert!(
            snapshot_for_signal(&signal, &[], false)
                .target_guard
                .secure_input
        );
    }

    #[test]
    fn host_normalization_rejects_paths_and_accepts_host_only_values() {
        assert_eq!(
            normalize_host("HTTPS://GMAIL.COM"),
            Some("gmail.com".into())
        );
        assert_eq!(normalize_host("https://gmail.com/path"), None);
        assert_eq!(
            extract_host("https://mail.google.com/u/0/#inbox"),
            Some("mail.google.com".into())
        );
    }

    #[test]
    fn browser_target_fingerprint_distinguishes_same_url_tabs() {
        let first_tab = browser_target_token("https://example.com\t1\tExample");
        let second_tab = browser_target_token("https://example.com\t2\tExample");
        let renamed_tab = browser_target_token("https://example.com\t1\tRenamed");

        assert_ne!(first_tab, second_tab);
        assert_ne!(first_tab, renamed_tab);
    }

    #[test]
    fn identifies_browser_adapters_without_changing_unknown_app_fallback() {
        assert_eq!(
            browser_application(Some("org.mozilla.firefox"), "Firefox"),
            Some("Firefox")
        );
        assert_eq!(
            browser_application(Some("com.google.Chrome.canary"), "Google Chrome Canary"),
            Some("Google Chrome Canary")
        );
        assert_eq!(
            browser_application(Some("com.example.editor"), "Editor"),
            None
        );
        assert!(!is_browser_bundle_id(Some("com.example.editor")));
    }

    #[test]
    fn target_mismatch_reason_allows_browser_delivery_without_tab_access() {
        let browser = TargetAppGuard {
            pid: 42,
            bundle_id: Some("com.google.Chrome.canary".into()),
            browser_host: None,
            browser_target_token: None,
            window_token: Some(10),
            window_id: Some(100),
            input_token: Some(20),
            secure_input: false,
        };
        assert_eq!(target_mismatch_reason(&browser, &browser), None);

        let input_changed = TargetAppGuard {
            input_token: Some(21),
            ..browser.clone()
        };
        assert_eq!(target_mismatch_reason(&browser, &input_changed), None);
    }

    #[test]
    fn missing_optional_window_or_input_metadata_does_not_block_same_app_delivery() {
        let complete = TargetAppGuard {
            pid: 42,
            bundle_id: Some("com.example.editor".into()),
            browser_host: None,
            browser_target_token: None,
            window_token: Some(10),
            window_id: Some(100),
            input_token: Some(20),
            secure_input: false,
        };
        let incomplete = TargetAppGuard {
            window_token: None,
            window_id: None,
            input_token: None,
            ..complete.clone()
        };

        assert_eq!(target_mismatch_reason(&complete, &incomplete), None);
        assert_eq!(target_mismatch_reason(&incomplete, &complete), None);
    }
}
