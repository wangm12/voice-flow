//! Versioned, explicit local learning transfer between VoiceFlow installs.
//!
//! The portable file intentionally contains only globally eligible dictionary
//! words, live approved word rules, and active structured writing preferences.
//! It never contains History, audio, candidates, audit events, or permissions.

use crate::context::{
    AppMapping, ContextFamily, FocusKind, MappingTransferResolution, MappingTransferSelectorV1,
    WritingPreference, WritingPreferenceDimension, WritingPreferenceSource,
    WritingPreferenceStatus, WritingPreferenceValue, WritingTaskId,
};
use crate::store::{self, LearnPairRecord, LearnPairScope, Settings};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const MAX_TRANSFER_FILE_BYTES: usize = 1024 * 1024;
const MAX_GLOBAL_WORDS: usize = 256;
const MAX_WORD_PAIRS: usize = 1024;
const MAX_WRITING_PREFERENCES: usize = 960;
const PREVIEW_TTL_MS: u64 = 5 * 60 * 1000;
const TRANSFER_FORMAT: &str = "voiceflow-learning-transfer";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LearningTransferFileV1 {
    pub(crate) format: String,
    pub(crate) schema_version: u16,
    #[serde(default)]
    pub(crate) global_words: Vec<String>,
    #[serde(default)]
    pub(crate) word_pairs: Vec<TransferWordPairV1>,
    #[serde(default)]
    pub(crate) writing_preferences: Vec<TransferWritingPreferenceV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TransferTargetV1 {
    pub(crate) selector: MappingTransferSelectorV1,
    /// The selected writing task is a local mapping setting. It travels
    /// separately from selector identity so a destination with another task
    /// cannot be silently switched or duplicated.
    pub(crate) selected_task: Option<WritingTaskId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TransferPairScopeV1 {
    /// Concrete observed destination dimensions from the approved pair. These
    /// preserve a narrow browser host or native bundle condition; mapping IDs
    /// are intentionally never exported.
    pub(crate) family: Option<ContextFamily>,
    pub(crate) browser_host: Option<String>,
    pub(crate) native_bundle: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TransferWordPairV1 {
    pub(crate) target: Option<TransferTargetV1>,
    /// `None` is reserved for an unscoped global rule. Scoped rules carry a
    /// target selector and a concrete runtime host/bundle pair scope.
    pub(crate) pair_scope: Option<TransferPairScopeV1>,
    pub(crate) before: String,
    pub(crate) after: String,
    pub(crate) match_policy: store::LearnPairMatchPolicy,
    /// Manual App-scoped rules remain visibly local-only after transfer.
    #[serde(default)]
    pub(crate) manual_scoped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TransferWritingPreferenceV1 {
    pub(crate) target: TransferTargetV1,
    /// Active preferences are normalized to explicit task IDs by the source
    /// mapping before export.
    pub(crate) task_id: WritingTaskId,
    pub(crate) dimension: WritingPreferenceDimension,
    pub(crate) value: WritingPreferenceValue,
    pub(crate) version: u16,
    pub(crate) source: WritingPreferenceSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransferItemKind {
    GlobalWord,
    WordPair,
    WritingPreference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransferItemStatus {
    Ready,
    Conflict,
    AlreadyPresent,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransferAction {
    Import,
    KeepLocal,
    Replace,
    ImportGlobal,
    None,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LearningTransferItem {
    pub(crate) item_id: String,
    pub(crate) kind: TransferItemKind,
    pub(crate) summary: String,
    pub(crate) scope_summary: String,
    pub(crate) will_create_mapping: bool,
    pub(crate) status: TransferItemStatus,
    pub(crate) default_action: TransferAction,
    pub(crate) choice_options: Vec<TransferAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) details: Option<TransferItemDetails>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TransferItemDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) word: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) task_id: Option<WritingTaskId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) dimension: Option<WritingPreferenceDimension>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) value: Option<WritingPreferenceValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) existing_value: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LearningTransferSourceSummary {
    pub(crate) global_word_count: usize,
    pub(crate) pair_count: usize,
    pub(crate) preference_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LearningTransferPreview {
    pub(crate) status: &'static str,
    pub(crate) preview_id: String,
    pub(crate) expires_at_ms: u64,
    pub(crate) source_summary: LearningTransferSourceSummary,
    pub(crate) items: Vec<LearningTransferItem>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LearningTransferChoice {
    pub(crate) item_id: String,
    pub(crate) action: TransferAction,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransferItemResultStatus {
    Imported,
    KeptLocal,
    AlreadyPresent,
    Blocked,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LearningTransferItemResult {
    pub(crate) item_id: String,
    pub(crate) status: TransferItemResultStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LearningTransferApplyResult {
    pub(crate) status: &'static str,
    pub(crate) results: Vec<LearningTransferItemResult>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LearningTransferExportResult {
    pub(crate) status: &'static str,
    pub(crate) global_word_count: usize,
    pub(crate) pair_count: usize,
    pub(crate) preference_count: usize,
    pub(crate) skipped_count: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LearningTransferApplyInput {
    pub(crate) preview_id: String,
    #[serde(default)]
    pub(crate) choices: Vec<LearningTransferChoice>,
}

#[derive(Debug, Clone)]
pub(crate) struct LoadedTransferFile {
    pub(crate) file: LearningTransferFileV1,
    pub(crate) fingerprint: String,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum PlannedTransferValue {
    Word(String),
    Pair {
        source: TransferWordPairV1,
        destination_pair_key: String,
        destination_scope: LearnPairScope,
        expected_rows: Vec<store::LearnPairTransferExpectedRow>,
        replacement_allowed: bool,
    },
    Preference {
        source: TransferWritingPreferenceV1,
        destination_mapping_id: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct PlannedTransferItem {
    pub(crate) view: LearningTransferItem,
    pub(crate) value: PlannedTransferValue,
}

#[derive(Debug, Clone)]
pub(crate) struct PreviewSession {
    pub(crate) path: PathBuf,
    pub(crate) file_fingerprint: String,
    pub(crate) local_fingerprint: String,
    pub(crate) file: LearningTransferFileV1,
    pub(crate) items: Vec<PlannedTransferItem>,
    pub(crate) new_mappings: Vec<AppMapping>,
    pub(crate) expires_at_ms: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedTransferApply {
    pub(crate) next_settings: Settings,
    pub(crate) settings_changed: bool,
    pub(crate) pair_intents: Vec<store::LearnPairTransferIntent>,
    pub(crate) pair_item_ids: Vec<String>,
    pub(crate) pair_mapping_item_ids: Vec<String>,
    pub(crate) preliminary_results: Vec<LearningTransferItemResult>,
}

static PREVIEWS: OnceLock<Mutex<HashMap<String, PreviewSession>>> = OnceLock::new();
static NEXT_PREVIEW_ID: AtomicU64 = AtomicU64::new(1);

fn previews() -> &'static Mutex<HashMap<String, PreviewSession>> {
    PREVIEWS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn next_preview_id() -> String {
    let sequence = NEXT_PREVIEW_ID.fetch_add(1, Ordering::Relaxed);
    let stamp = now_ms();
    let digest = Sha256::digest(format!("{stamp}:{sequence}").as_bytes());
    hex::encode(&digest[..16])
}

pub(crate) fn make_preview_id() -> String {
    next_preview_id()
}

pub(crate) fn store_preview(id: String, session: PreviewSession) {
    let mut guard = previews()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = now_ms();
    guard.retain(|_, existing| existing.expires_at_ms > now);
    // Settings exposes a single transfer panel; retain only its latest
    // preview so imported values and file paths do not accumulate in memory.
    guard.clear();
    guard.insert(id, session);
}

pub(crate) fn take_preview(id: &str) -> Option<PreviewSession> {
    let mut guard = previews()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let session = guard.remove(id)?;
    (session.expires_at_ms > now_ms()).then_some(session)
}

pub(crate) fn preview_response(id: String, session: &PreviewSession) -> LearningTransferPreview {
    LearningTransferPreview {
        status: "preview",
        preview_id: id,
        expires_at_ms: session.expires_at_ms,
        source_summary: LearningTransferSourceSummary {
            global_word_count: session.file.global_words.len(),
            pair_count: session.file.word_pairs.len(),
            preference_count: session.file.writing_preferences.len(),
        },
        items: session.items.iter().map(|item| item.view.clone()).collect(),
    }
}

pub(crate) fn cancelled_preview_response() -> LearningTransferPreviewCancelled {
    LearningTransferPreviewCancelled {
        status: "cancelled",
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LearningTransferPreviewCancelled {
    pub(crate) status: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub(crate) enum LearningTransferPreviewResponse {
    Cancelled(LearningTransferPreviewCancelled),
    Preview(LearningTransferPreview),
}

impl From<LearningTransferPreview> for LearningTransferPreviewResponse {
    fn from(value: LearningTransferPreview) -> Self {
        Self::Preview(value)
    }
}

impl From<LearningTransferPreviewCancelled> for LearningTransferPreviewResponse {
    fn from(value: LearningTransferPreviewCancelled) -> Self {
        Self::Cancelled(value)
    }
}

pub(crate) fn parse_transfer_file(bytes: &[u8]) -> Result<LearningTransferFileV1, String> {
    if bytes.len() > MAX_TRANSFER_FILE_BYTES {
        return Err(format!(
            "learning_transfer_invalid_file: file exceeds {MAX_TRANSFER_FILE_BYTES} bytes"
        ));
    }
    let mut file: LearningTransferFileV1 = serde_json::from_slice(bytes)
        .map_err(|error| format!("learning_transfer_invalid_file: {error}"))?;
    if file.schema_version != 1 {
        return Err(format!(
            "learning_transfer_unsupported_version: {}",
            file.schema_version
        ));
    }
    if file.format != TRANSFER_FORMAT {
        return Err("learning_transfer_invalid_file: unrecognized format".into());
    }
    validate_transfer_file(&mut file)?;
    Ok(file)
}

pub(crate) fn validate_transfer_file(file: &mut LearningTransferFileV1) -> Result<(), String> {
    if file.global_words.len() > MAX_GLOBAL_WORDS {
        return Err("learning_transfer_invalid_file: too many global words".into());
    }
    if file.word_pairs.len() > MAX_WORD_PAIRS {
        return Err("learning_transfer_invalid_file: too many word pairs".into());
    }
    if file.writing_preferences.len() > MAX_WRITING_PREFERENCES {
        return Err("learning_transfer_invalid_file: too many writing preferences".into());
    }

    let mut words = BTreeSet::new();
    for word in &file.global_words {
        validate_word(word, "global word")?;
        let mut identity = word.clone();
        identity.make_ascii_lowercase();
        if !words.insert(identity) {
            return Err("learning_transfer_invalid_file: duplicate global word".into());
        }
    }

    let mut pair_keys = BTreeSet::new();
    for pair in &file.word_pairs {
        let scope = validate_transfer_pair_item(pair)?;
        let policy = pair.match_policy;
        let destination_key =
            store::scoped_word_rule_key(&pair.before, &pair.after, &scope, policy).into_string();
        let portable_owner = pair
            .target
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| format!("learning_transfer_invalid_file: {error}"))?
            .unwrap_or_else(|| "direct_scope".into());
        if !pair_keys.insert(format!("{portable_owner}:{destination_key}")) {
            return Err("learning_transfer_invalid_file: duplicate word pair".into());
        }
    }

    for index in 0..file.word_pairs.len() {
        for other in file.word_pairs.iter().skip(index + 1) {
            let left = &file.word_pairs[index];
            if pair_scopes_overlap(left, other)
                && alias_domains_intersect(left, other)
                && left.after != other.after
            {
                return Err(
                    "learning_transfer_invalid_file: overlapping aliases have different destinations"
                        .into(),
                );
            }
        }
    }

    let mut preference_keys = BTreeSet::new();
    let mut selector_tasks = Vec::<(MappingTransferSelectorV1, Option<WritingTaskId>)>::new();
    for preference in &file.writing_preferences {
        validate_target(&preference.target)?;
        if preference.version != 1 {
            return Err("learning_transfer_invalid_file: unsupported preference version".into());
        }
        if !preference_supported(preference.dimension, preference.value) {
            return Err("learning_transfer_invalid_file: unsupported preference value".into());
        }
        if let Some((_, selected_task)) = selector_tasks
            .iter()
            .find(|(selector, _)| selector == &preference.target.selector)
        {
            if *selected_task != preference.target.selected_task {
                return Err(
                    "learning_transfer_invalid_file: contradictory selected tasks for one selector"
                        .into(),
                );
            }
        } else {
            selector_tasks.push((
                preference.target.selector.clone(),
                preference.target.selected_task,
            ));
        }
        let serialized_target = serde_json::to_string(&preference.target)
            .map_err(|error| format!("learning_transfer_invalid_file: {error}"))?;
        let key = format!(
            "{serialized_target}:{}:{}",
            preference.task_id as u8, preference.dimension as u8
        );
        if !preference_keys.insert(key) {
            return Err("learning_transfer_invalid_file: duplicate writing preference".into());
        }
    }
    Ok(())
}

fn validate_transfer_pair_item(pair: &TransferWordPairV1) -> Result<LearnPairScope, String> {
    validate_word(&pair.before, "pair source")?;
    validate_word(&pair.after, "pair destination")?;
    if pair.before == pair.after {
        return Err("learning_transfer_invalid_file: word pair must change its surface".into());
    }
    if pair
        .target
        .as_ref()
        .is_some_and(|target| target.selected_task.is_some())
    {
        return Err(
            "learning_transfer_invalid_file: word pair cannot depend on selected writing task"
                .into(),
        );
    }
    if pair.manual_scoped
        && !matches!(
            (&pair.target, &pair.pair_scope),
            (None, Some(scope))
                if scope.native_bundle.is_some() && scope.browser_host.is_none()
        )
    {
        return Err(
            "learning_transfer_invalid_file: manual-scoped pairs must keep a direct native App scope"
                .into(),
        );
    }
    transfer_pair_scope(pair)
}

fn validate_word(word: &str, label: &str) -> Result<(), String> {
    if word.trim().is_empty() || word.trim() != word || word.len() > 256 {
        return Err(format!(
            "learning_transfer_invalid_file: {label} must contain 1 to 256 UTF-8 bytes without surrounding whitespace"
        ));
    }
    if word.chars().any(char::is_control) {
        return Err(format!(
            "learning_transfer_invalid_file: {label} contains a control character"
        ));
    }
    Ok(())
}

fn validate_target(target: &TransferTargetV1) -> Result<(), String> {
    target
        .selector
        .validate()
        .map_err(|error| format!("learning_transfer_invalid_file: {error}"))?;
    if target
        .selector
        .bundle_id
        .as_deref()
        .into_iter()
        .chain(target.selector.executable.as_deref())
        .any(|value| value.trim() != value)
    {
        return Err(
            "learning_transfer_invalid_file: mapping selector has surrounding whitespace".into(),
        );
    }
    if target.selected_task.is_some()
        && target.selector.focused_field.is_none()
        && (target.selector.browser_host.is_none() || target.selector.browser_path_prefix.is_none())
    {
        return Err(
            "learning_transfer_invalid_file: selected task requires a focused field or browser path selector"
                .into(),
        );
    }
    if target.selected_task.is_some()
        && target.selector.focused_field.is_some_and(|field| {
            matches!(
                field,
                FocusKind::Secure | FocusKind::Search | FocusKind::Unknown
            )
        })
    {
        return Err(
            "learning_transfer_invalid_file: selected task cannot use secure or search focus"
                .into(),
        );
    }
    Ok(())
}

fn transfer_pair_scope(pair: &TransferWordPairV1) -> Result<LearnPairScope, String> {
    match (&pair.target, &pair.pair_scope) {
        (None, None) => Ok(LearnPairScope::default()),
        (Some(target), Some(scope)) => {
            validate_target(target)?;
            if scope.family != Some(target.selector.family) {
                return Err(
                    "learning_transfer_invalid_file: pair family differs from target".into(),
                );
            }
            validate_scope_string(scope.browser_host.as_deref(), "pair browser host")?;
            validate_scope_string(scope.native_bundle.as_deref(), "pair native bundle")?;
            validate_pair_host_scope(scope)?;
            if let Some(selector_host) = target.selector.browser_host.as_deref() {
                let Some(observed_host) = scope.browser_host.as_deref() else {
                    return Err(
                        "learning_transfer_invalid_file: browser mapping pair is missing its observed host"
                            .into(),
                    );
                };
                if !host_selector_matches(selector_host, observed_host) {
                    return Err(
                        "learning_transfer_invalid_file: pair host is outside its mapping selector"
                            .into(),
                    );
                }
            }
            if let Some(selector_bundle) = target.selector.bundle_id.as_deref() {
                if let Some(observed_bundle) = scope.native_bundle.as_deref() {
                    if !selector_bundle.eq_ignore_ascii_case(observed_bundle) {
                        return Err(
                            "learning_transfer_invalid_file: pair bundle differs from its mapping selector"
                                .into(),
                        );
                    }
                } else if scope.browser_host.is_none() {
                    return Err(
                        "learning_transfer_invalid_file: native mapping pair is missing its observed bundle"
                            .into(),
                    );
                }
            }
            Ok(LearnPairScope {
                family: scope
                    .family
                    .map(|family| crate::context::family_id(family).to_owned()),
                mapping_id: None,
                browser_host: scope.browser_host.clone(),
                native_bundle: scope.native_bundle.clone(),
            })
        }
        (None, Some(scope)) => {
            validate_scope_string(scope.browser_host.as_deref(), "pair browser host")?;
            validate_scope_string(scope.native_bundle.as_deref(), "pair native bundle")?;
            validate_pair_host_scope(scope)?;
            if scope.browser_host.is_none() && scope.native_bundle.is_none() {
                return Err(
                    "learning_transfer_invalid_file: direct pair scope requires an observed host or native App"
                        .into(),
                );
            }
            Ok(LearnPairScope {
                family: scope
                    .family
                    .map(|family| crate::context::family_id(family).to_owned()),
                mapping_id: None,
                browser_host: scope.browser_host.clone(),
                native_bundle: scope.native_bundle.clone(),
            })
        }
        _ => Err("learning_transfer_invalid_file: incomplete scoped pair target".into()),
    }
}

fn validate_pair_host_scope(scope: &TransferPairScopeV1) -> Result<(), String> {
    if scope.browser_host.is_some() && scope.native_bundle.is_some() {
        return Err(
            "learning_transfer_invalid_file: pair scope cannot combine browser host and native App"
                .into(),
        );
    }
    if let Some(host) = &scope.browser_host {
        let normalized = crate::context::normalize_host(host);
        if normalized.as_deref() != Some(host.as_str()) {
            return Err(
                "learning_transfer_invalid_file: pair browser host must be a normalized host"
                    .into(),
            );
        }
    }
    Ok(())
}

fn validate_scope_string(value: Option<&str>, label: &str) -> Result<(), String> {
    if value.is_some_and(|value| {
        value.trim().is_empty()
            || value.trim() != value
            || value.len() > 256
            || value.chars().any(char::is_control)
    }) {
        return Err(format!("learning_transfer_invalid_file: invalid {label}"));
    }
    Ok(())
}

fn preference_supported(
    dimension: WritingPreferenceDimension,
    value: WritingPreferenceValue,
) -> bool {
    matches!(
        (dimension, value),
        (
            WritingPreferenceDimension::ListStructure,
            WritingPreferenceValue::Bullets
        ) | (
            WritingPreferenceDimension::GreetingTone,
            WritingPreferenceValue::ConversationalGreeting
                | WritingPreferenceValue::RespectfulGreeting
        ) | (
            WritingPreferenceDimension::TerminalPunctuation,
            WritingPreferenceValue::TerminalPeriod | WritingPreferenceValue::NoTerminalPeriod
        )
    )
}

pub(crate) fn selectors_overlap(
    left: &MappingTransferSelectorV1,
    right: &MappingTransferSelectorV1,
) -> bool {
    fn same_optional(left: Option<&str>, right: Option<&str>) -> bool {
        match (left, right) {
            (Some(left), Some(right)) => left.eq_ignore_ascii_case(right),
            _ => true,
        }
    }
    fn host_overlap(left: Option<&str>, right: Option<&str>) -> bool {
        match (left, right) {
            (Some(left), Some(right)) => {
                let left = left.to_ascii_lowercase();
                let right = right.to_ascii_lowercase();
                left == right
                    || left.strip_suffix(&format!(".{right}")).is_some()
                    || right.strip_suffix(&format!(".{left}")).is_some()
            }
            _ => true,
        }
    }
    fn path_overlap(left: Option<&str>, right: Option<&str>) -> bool {
        match (left, right) {
            (Some(left), Some(right)) => {
                path_prefix_contains(left, right) || path_prefix_contains(right, left)
            }
            _ => true,
        }
    }
    let focused_field_overlap = match (left.focused_field, right.focused_field) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    };
    same_optional(left.bundle_id.as_deref(), right.bundle_id.as_deref())
        && same_optional(left.executable.as_deref(), right.executable.as_deref())
        && host_overlap(left.browser_host.as_deref(), right.browser_host.as_deref())
        && path_overlap(
            left.browser_path_prefix.as_deref(),
            right.browser_path_prefix.as_deref(),
        )
        && focused_field_overlap
}

fn path_prefix_contains(parent: &str, child: &str) -> bool {
    parent == "/"
        || parent == child
        || child
            .strip_prefix(parent.trim_end_matches('/'))
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn pair_scopes_overlap(left: &TransferWordPairV1, right: &TransferWordPairV1) -> bool {
    let (left_scope, right_scope) = match (transfer_pair_scope(left), transfer_pair_scope(right)) {
        (Ok(left_scope), Ok(right_scope)) => (left_scope, right_scope),
        _ => return false,
    };
    if (left_scope.browser_host.is_some() && right_scope.native_bundle.is_some())
        || (right_scope.browser_host.is_some() && left_scope.native_bundle.is_some())
    {
        return false;
    }
    if let (Some(left), Some(right)) = (&left.target, &right.target) {
        if !selectors_overlap(&left.selector, &right.selector) {
            return false;
        }
        if crate::context::transfer_selector_specificity_cmp(&left.selector, &right.selector)
            != std::cmp::Ordering::Equal
        {
            return false;
        }
    }
    optional_scope_value_overlaps(left_scope.family.as_deref(), right_scope.family.as_deref())
        && optional_scope_value_overlaps(
            left_scope.browser_host.as_deref(),
            right_scope.browser_host.as_deref(),
        )
        && optional_scope_value_overlaps(
            left_scope.native_bundle.as_deref(),
            right_scope.native_bundle.as_deref(),
        )
}

fn optional_scope_value_overlaps(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.eq_ignore_ascii_case(right),
        _ => true,
    }
}

fn alias_domains_intersect(left: &TransferWordPairV1, right: &TransferWordPairV1) -> bool {
    match (left.match_policy, right.match_policy) {
        (store::LearnPairMatchPolicy::Exact, store::LearnPairMatchPolicy::Exact) => {
            left.before == right.before
        }
        (store::LearnPairMatchPolicy::IgnoreCase, store::LearnPairMatchPolicy::IgnoreCase) => {
            left.before.eq_ignore_ascii_case(&right.before)
        }
        (store::LearnPairMatchPolicy::Exact, store::LearnPairMatchPolicy::IgnoreCase)
        | (store::LearnPairMatchPolicy::IgnoreCase, store::LearnPairMatchPolicy::Exact) => {
            left.before.eq_ignore_ascii_case(&right.before)
        }
    }
}

pub(crate) fn load_transfer_file(path: &Path) -> Result<LoadedTransferFile, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|error| format!("learning_transfer_invalid_file: cannot open file: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("learning_transfer_invalid_file: cannot inspect file: {error}"))?;
    if !metadata.is_file() {
        return Err("learning_transfer_invalid_file: selected path is not a regular file".into());
    }
    if metadata.len() > MAX_TRANSFER_FILE_BYTES as u64 {
        return Err(format!(
            "learning_transfer_invalid_file: file exceeds {MAX_TRANSFER_FILE_BYTES} bytes"
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_TRANSFER_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("learning_transfer_invalid_file: cannot read file: {error}"))?;
    if bytes.len() > MAX_TRANSFER_FILE_BYTES {
        return Err(format!(
            "learning_transfer_invalid_file: file exceeds {MAX_TRANSFER_FILE_BYTES} bytes"
        ));
    }
    let fingerprint = hex::encode(Sha256::digest(&bytes));
    let parsed = parse_transfer_file(&bytes)?;
    Ok(LoadedTransferFile {
        file: parsed,
        fingerprint,
    })
}

pub(crate) fn local_fingerprint(
    settings: &Settings,
    pairs: &[LearnPairRecord],
    file: &LearningTransferFileV1,
) -> Result<String, String> {
    #[derive(Serialize)]
    struct MappingFingerprint {
        selector: MappingTransferSelectorV1,
        selected_task: Option<WritingTaskId>,
        preferences: Vec<WritingPreference>,
        deleted: bool,
        enabled: bool,
    }
    #[derive(Serialize)]
    struct ImportedWordFingerprint {
        word: String,
        local_word: Option<String>,
        eligible_global: bool,
        independent_global: bool,
    }
    let mappings = settings
        .context_mappings
        .iter()
        .map(|mapping| MappingFingerprint {
            selector: raw_mapping_selector(mapping),
            selected_task: mapping.writing_task_id,
            preferences: mapping.writing_preferences.clone(),
            deleted: mapping.deleted,
            enabled: mapping.enabled,
        })
        .collect::<Vec<_>>();
    let eligible = settings.asr_dictionary_words(pairs);
    let imported_words = file
        .global_words
        .iter()
        .map(|word| ImportedWordFingerprint {
            word: word.to_ascii_lowercase(),
            local_word: settings
                .dictionary
                .iter()
                .find(|local| store::dictionary_words_equal(local, word))
                .cloned(),
            eligible_global: eligible
                .iter()
                .any(|local| store::dictionary_words_equal(local, word)),
            independent_global: settings.has_independent_global_dictionary_owner(word),
        })
        .collect::<Vec<_>>();
    #[derive(Serialize)]
    struct Snapshot {
        mappings: Vec<MappingFingerprint>,
        imported_words: Vec<ImportedWordFingerprint>,
        dictionary_count: usize,
    }
    let encoded = serde_json::to_vec(&Snapshot {
        mappings,
        imported_words,
        dictionary_count: settings.dictionary.len(),
    })
    .map_err(|error| {
        format!("learning_transfer_stale_preview: cannot fingerprint local state: {error}")
    })?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

fn raw_mapping_selector(mapping: &AppMapping) -> MappingTransferSelectorV1 {
    MappingTransferSelectorV1 {
        version: 1,
        family: mapping.family,
        bundle_id: mapping.bundle_id.clone(),
        executable: mapping.executable.clone(),
        browser_host: mapping
            .browser_host
            .as_deref()
            .and_then(crate::context::normalize_host)
            .or_else(|| mapping.browser_host.clone()),
        browser_path_prefix: mapping
            .browser_path_prefix
            .as_deref()
            .and_then(crate::context::normalize_path_prefix)
            .or_else(|| mapping.browser_path_prefix.clone()),
        focused_field: mapping.focused_field,
    }
}

pub(crate) fn eligible_global_words(settings: &Settings, pairs: &[LearnPairRecord]) -> Vec<String> {
    let mut words = settings.asr_dictionary_words(pairs);
    words.sort_by_cached_key(|word| (word.to_ascii_lowercase(), word.clone()));
    words
}

pub(crate) fn build_export_file(
    settings: &Settings,
    mappings: &[AppMapping],
    pairs: &[LearnPairRecord],
) -> (LearningTransferFileV1, usize) {
    let mut skipped = 0usize;
    let mut global_words = eligible_global_words(settings, pairs);
    global_words.retain(|word| {
        if validate_word(word, "global word").is_ok() {
            true
        } else {
            skipped += 1;
            false
        }
    });
    let mut word_pairs = Vec::new();
    for row in pairs.iter().filter(|row| {
        row.is_live_promoted()
            && !row.before_surface.is_empty()
            && !row.pair_key.starts_with("learn:style:")
            && !row.pair_key.starts_with("learn:intensity:")
    }) {
        let safe_pair = transfer_pair_from_record(row, mappings)
            .and_then(|pair| validate_transfer_pair_item(&pair).map(|_| pair));
        match safe_pair {
            Ok(pair) => word_pairs.push(pair),
            Err(_) => skipped += 1,
        }
    }
    let (mut word_pairs, ambiguous_or_duplicate) = discard_ambiguous_export_pairs(word_pairs);
    skipped += ambiguous_or_duplicate;
    word_pairs.sort_by_cached_key(|pair| {
        (
            pair.before.to_ascii_lowercase(),
            pair.after.to_ascii_lowercase(),
            pair.before.clone(),
            pair.after.clone(),
            format!("{:?}", pair.target),
            format!("{:?}", pair.pair_scope),
            format!("{:?}", pair.match_policy),
            pair.manual_scoped,
        )
    });

    let mut writing_preferences = Vec::new();
    for mapping in mappings.iter().filter(|mapping| !mapping.deleted) {
        if !mapping.enabled {
            skipped += mapping
                .writing_preferences
                .iter()
                .filter(|preference| preference.status == WritingPreferenceStatus::Active)
                .count();
            continue;
        }
        let target = match mapping.transfer_selector() {
            Ok(selector) => TransferTargetV1 {
                selector,
                selected_task: mapping.writing_task_id,
            },
            Err(_) => {
                skipped += mapping
                    .writing_preferences
                    .iter()
                    .filter(|preference| preference.status == WritingPreferenceStatus::Active)
                    .count();
                continue;
            }
        };
        if mapping_has_active_specificity_tie(mapping, mappings) {
            skipped += mapping
                .writing_preferences
                .iter()
                .filter(|preference| preference.status == WritingPreferenceStatus::Active)
                .count();
            continue;
        }
        let preferences = match mapping.active_transfer_preferences() {
            Ok(preferences) => preferences,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if validate_target(&target).is_err() {
            skipped += preferences.len();
            continue;
        }
        for preference in preferences {
            let Some(task_id) = preference.task_id else {
                skipped += 1;
                continue;
            };
            writing_preferences.push(TransferWritingPreferenceV1 {
                target: target.clone(),
                task_id,
                dimension: preference.dimension,
                value: preference.value,
                version: preference.version,
                source: preference.source,
            });
        }
    }
    writing_preferences.sort_by(|left, right| {
        serde_json::to_string(left)
            .unwrap_or_default()
            .cmp(&serde_json::to_string(right).unwrap_or_default())
    });
    let file = LearningTransferFileV1 {
        format: TRANSFER_FORMAT.into(),
        schema_version: 1,
        global_words,
        word_pairs,
        writing_preferences,
    };
    (file, skipped)
}

fn discard_ambiguous_export_pairs(
    pairs: Vec<TransferWordPairV1>,
) -> (Vec<TransferWordPairV1>, usize) {
    let mut discard = vec![false; pairs.len()];
    let mut identities = HashMap::<String, usize>::new();
    for (index, pair) in pairs.iter().enumerate() {
        let Ok(scope) = transfer_pair_scope(pair) else {
            discard[index] = true;
            continue;
        };
        let owner = pair
            .target
            .as_ref()
            .and_then(|target| serde_json::to_string(target).ok())
            .unwrap_or_else(|| "direct_scope".into());
        let key = store::scoped_word_rule_key(&pair.before, &pair.after, &scope, pair.match_policy)
            .into_string();
        let identity = format!("{owner}:{key}");
        if let Some(previous) = identities.insert(identity, index) {
            // Keep one copy only when both portable rows are byte-for-byte
            // equivalent; otherwise the generated file would be ambiguous.
            if pairs[previous] == *pair {
                discard[index] = true;
            } else {
                discard[previous] = true;
                discard[index] = true;
            }
        }
    }
    for left_index in 0..pairs.len() {
        for right_index in (left_index + 1)..pairs.len() {
            let left = &pairs[left_index];
            let right = &pairs[right_index];
            if pair_scopes_overlap(left, right)
                && alias_domains_intersect(left, right)
                && left.after != right.after
            {
                discard[left_index] = true;
                discard[right_index] = true;
            }
        }
    }
    let removed = discard.iter().filter(|drop| **drop).count();
    (
        pairs
            .into_iter()
            .zip(discard)
            .filter_map(|(pair, drop)| (!drop).then_some(pair))
            .collect(),
        removed,
    )
}

pub(crate) fn build_preview(
    path: PathBuf,
    loaded: LoadedTransferFile,
    settings: &Settings,
    pairs: &[LearnPairRecord],
) -> Result<LearningTransferPreview, String> {
    let local_fingerprint = local_fingerprint(settings, pairs, &loaded.file)?;
    let mut all_mappings = settings.context_mappings.clone();
    let mut new_mappings = Vec::new();
    let mut items = Vec::with_capacity(
        loaded.file.global_words.len()
            + loaded.file.word_pairs.len()
            + loaded.file.writing_preferences.len(),
    );
    let eligible_global = settings.asr_dictionary_words(pairs);
    let mut planned_new_global_words = 0usize;

    for (index, word) in loaded.file.global_words.iter().enumerate() {
        let item_id = format!("word-{index:04}");
        let local_word = settings
            .dictionary
            .iter()
            .find(|local| store::dictionary_words_equal(local, word));
        let eligible = eligible_global
            .iter()
            .any(|local| store::dictionary_words_equal(local, word));
        let independent_global = settings.has_independent_global_dictionary_owner(word);
        let (status, default_action, choice_options, reason, existing_value) = if independent_global
        {
            (
                TransferItemStatus::AlreadyPresent,
                TransferAction::None,
                Vec::new(),
                Some("already_global".into()),
                None,
            )
        } else if eligible {
            (
                TransferItemStatus::Ready,
                TransferAction::Import,
                Vec::new(),
                Some("global_rule_owner_will_be_added".into()),
                Some("rule_owner".into()),
            )
        } else if local_word.is_some() {
            (
                TransferItemStatus::Conflict,
                TransferAction::KeepLocal,
                vec![TransferAction::KeepLocal, TransferAction::ImportGlobal],
                Some("global_scope_upgrade".into()),
                Some("local_only".into()),
            )
        } else if settings
            .dictionary
            .len()
            .saturating_add(planned_new_global_words)
            >= 256
        {
            (
                TransferItemStatus::Blocked,
                TransferAction::None,
                Vec::new(),
                Some("dictionary_capacity".into()),
                None,
            )
        } else {
            planned_new_global_words += 1;
            (
                TransferItemStatus::Ready,
                TransferAction::Import,
                Vec::new(),
                None,
                None,
            )
        };
        items.push(PlannedTransferItem {
            view: LearningTransferItem {
                item_id,
                kind: TransferItemKind::GlobalWord,
                summary: format!("Global word: {word}"),
                scope_summary: "Global dictionary".into(),
                will_create_mapping: false,
                status,
                default_action,
                choice_options,
                reason,
                details: Some(TransferItemDetails {
                    word: Some(word.clone()),
                    before: None,
                    after: None,
                    task_id: None,
                    dimension: None,
                    value: None,
                    existing_value,
                }),
            },
            value: PlannedTransferValue::Word(word.clone()),
        });
    }

    for (index, source) in loaded.file.word_pairs.iter().enumerate() {
        let item_id = format!("pair-{index:04}");
        let target_id = match source.target.as_ref() {
            Some(target) => {
                match resolve_target_mapping(target, &mut all_mappings, &mut new_mappings, false) {
                    Ok(mapping_id) => Some(mapping_id),
                    Err(reason) => {
                        items.push(blocked_pair_item(item_id, source, &reason));
                        continue;
                    }
                }
            }
            None => None,
        };
        let mut destination_scope = match transfer_pair_scope(source) {
            Ok(scope) => scope,
            Err(error) => {
                items.push(blocked_pair_item(item_id, source, &error));
                continue;
            }
        };
        destination_scope.mapping_id = target_id.clone();
        let destination_pair_key = store::scoped_word_rule_key(
            &source.before,
            &source.after,
            &destination_scope,
            source.match_policy,
        )
        .into_string();
        let expected_rows = match store::learn_pair_transfer_conflict_snapshot(
            pairs,
            &destination_pair_key,
            &source.before,
            &source.after,
            &destination_scope,
            source.match_policy,
        ) {
            Ok(rows) => rows,
            Err(error) => {
                return Err(format!("learning_transfer_preview_failed: {error}"));
            }
        };
        let (status, default_action, choice_options, reason, replacement_allowed, existing_value) =
            classify_pair_preview(
                &destination_pair_key,
                &destination_scope,
                source,
                &expected_rows,
            );
        let scope_summary = pair_scope_summary(source);
        items.push(PlannedTransferItem {
            view: LearningTransferItem {
                item_id,
                kind: TransferItemKind::WordPair,
                summary: format!("{} → {}", source.before, source.after),
                scope_summary,
                will_create_mapping: target_id
                    .as_ref()
                    .is_some_and(|id| new_mappings.iter().any(|mapping| &mapping.id == id)),
                status,
                default_action,
                choice_options,
                reason: reason.or_else(|| {
                    target_id
                        .as_ref()
                        .is_some_and(|id| new_mappings.iter().any(|mapping| &mapping.id == id))
                        .then(|| "new_local_mapping".into())
                }),
                details: Some(TransferItemDetails {
                    word: None,
                    before: Some(source.before.clone()),
                    after: Some(source.after.clone()),
                    task_id: source
                        .target
                        .as_ref()
                        .and_then(|target| target.selected_task),
                    dimension: None,
                    value: None,
                    existing_value,
                }),
            },
            value: PlannedTransferValue::Pair {
                source: source.clone(),
                destination_pair_key,
                destination_scope,
                expected_rows,
                replacement_allowed,
            },
        });
    }

    for (index, source) in loaded.file.writing_preferences.iter().enumerate() {
        let item_id = format!("preference-{index:04}");
        let mapping_id = match resolve_target_mapping(
            &source.target,
            &mut all_mappings,
            &mut new_mappings,
            true,
        ) {
            Ok(mapping_id) => mapping_id,
            Err(reason) => {
                items.push(blocked_preference_item(item_id, source, &reason));
                continue;
            }
        };
        let mapping = all_mappings
            .iter()
            .find(|mapping| mapping.id == mapping_id)
            .expect("resolved target mapping remains in the planning set");
        let existing = local_preference_for(mapping, source.task_id, source.dimension);
        let (status, default_action, choice_options, reason, existing_value) = match existing {
            Some(preference) if preference.status == WritingPreferenceStatus::Forgotten => (
                TransferItemStatus::Blocked,
                TransferAction::None,
                Vec::new(),
                Some("forgotten_preference".into()),
                Some(preference_value_summary(preference)),
            ),
            Some(preference)
                if preference.status == WritingPreferenceStatus::Active
                    && preference.value == source.value =>
            {
                (
                    TransferItemStatus::AlreadyPresent,
                    TransferAction::None,
                    Vec::new(),
                    Some("already_present".into()),
                    Some(preference_value_summary(preference)),
                )
            }
            Some(preference) => (
                TransferItemStatus::Conflict,
                TransferAction::KeepLocal,
                vec![TransferAction::KeepLocal, TransferAction::Replace],
                Some("preference_conflict".into()),
                Some(preference_value_summary(preference)),
            ),
            None => (
                TransferItemStatus::Ready,
                TransferAction::Import,
                Vec::new(),
                None,
                None,
            ),
        };
        items.push(PlannedTransferItem {
            view: LearningTransferItem {
                item_id,
                kind: TransferItemKind::WritingPreference,
                summary: format!("{:?}: {:?}", source.dimension, source.value),
                scope_summary: target_scope_summary(&source.target),
                will_create_mapping: new_mappings.iter().any(|mapping| mapping.id == mapping_id),
                status,
                default_action,
                choice_options,
                reason,
                details: Some(TransferItemDetails {
                    word: None,
                    before: None,
                    after: None,
                    task_id: Some(source.task_id),
                    dimension: Some(source.dimension),
                    value: Some(source.value),
                    existing_value,
                }),
            },
            value: PlannedTransferValue::Preference {
                source: source.clone(),
                destination_mapping_id: mapping_id,
            },
        });
    }

    mark_in_file_specificity_ties(&mut items, &new_mappings);

    let preview_id = make_preview_id();
    let session = PreviewSession {
        path,
        file_fingerprint: loaded.fingerprint,
        local_fingerprint,
        file: loaded.file,
        items,
        new_mappings,
        expires_at_ms: now_ms().saturating_add(PREVIEW_TTL_MS),
    };
    let response = preview_response(preview_id.clone(), &session);
    store_preview(preview_id, session);
    Ok(response)
}

fn planned_item_target(item: &PlannedTransferItem) -> Option<&TransferTargetV1> {
    match &item.value {
        PlannedTransferValue::Pair { source, .. } => source.target.as_ref(),
        PlannedTransferValue::Preference { source, .. } => Some(&source.target),
        PlannedTransferValue::Word(_) => None,
    }
}

fn item_can_import(item: &PlannedTransferItem) -> bool {
    match item.view.status {
        TransferItemStatus::Ready => item.view.default_action == TransferAction::Import,
        TransferItemStatus::Conflict => item
            .view
            .choice_options
            .iter()
            .any(|action| matches!(action, TransferAction::Import | TransferAction::Replace)),
        TransferItemStatus::AlreadyPresent | TransferItemStatus::Blocked => false,
    }
}

fn mark_in_file_specificity_ties(items: &mut [PlannedTransferItem], new_mappings: &[AppMapping]) {
    let mut viable_selectors = Vec::<MappingTransferSelectorV1>::new();
    for item in items.iter().filter(|item| item_can_import(item)) {
        let Some(target) = planned_item_target(item) else {
            continue;
        };
        let selector_is_new = new_mappings
            .iter()
            .any(|mapping| raw_mapping_selector(mapping) == target.selector);
        if selector_is_new && !viable_selectors.contains(&target.selector) {
            viable_selectors.push(target.selector.clone());
        }
    }

    let mut ambiguous = Vec::<MappingTransferSelectorV1>::new();
    for left_index in 0..viable_selectors.len() {
        for right in viable_selectors.iter().skip(left_index + 1) {
            let left = &viable_selectors[left_index];
            if selectors_overlap(left, right)
                && crate::context::transfer_selector_specificity_cmp(left, right)
                    == std::cmp::Ordering::Equal
            {
                for selector in [left, right] {
                    if !ambiguous.contains(selector) {
                        ambiguous.push(selector.clone());
                    }
                }
            }
        }
    }

    if ambiguous.is_empty() {
        return;
    }
    for item in items.iter_mut() {
        if !item_can_import(item) {
            continue;
        }
        let Some(selector) = planned_item_target(item).map(|target| target.selector.clone()) else {
            continue;
        };
        if ambiguous.contains(&selector) {
            item.view.status = TransferItemStatus::Blocked;
            item.view.default_action = TransferAction::None;
            item.view.choice_options.clear();
            item.view.reason = Some("mapping_selector_ambiguous".into());
            item.view.will_create_mapping = false;
        }
    }
}

pub(crate) fn prepare_apply(
    input: LearningTransferApplyInput,
    settings: &Settings,
    pairs: &[LearnPairRecord],
    _dir: &Path,
) -> Result<PreparedTransferApply, String> {
    let session = take_preview(&input.preview_id).ok_or_else(|| {
        "learning_transfer_stale_preview: preview expired or was already used".to_owned()
    })?;
    let loaded = load_transfer_file(&session.path).map_err(|_| {
        "learning_transfer_stale_preview: selected file changed or is no longer readable".to_owned()
    })?;
    if loaded.fingerprint != session.file_fingerprint {
        return Err("learning_transfer_stale_preview: selected file changed".into());
    }
    let current_local_fingerprint = local_fingerprint(settings, pairs, &session.file)?;
    if current_local_fingerprint != session.local_fingerprint {
        return Err("learning_transfer_stale_preview: local learning settings changed".into());
    }

    let pair_preflight_intents = session
        .items
        .iter()
        .filter(|item| item.view.status != TransferItemStatus::Blocked)
        .filter_map(|item| match &item.value {
            PlannedTransferValue::Pair {
                source,
                destination_pair_key,
                destination_scope,
                expected_rows,
                ..
            } => Some(store::LearnPairTransferIntent {
                pair_key: destination_pair_key.clone(),
                before_surface: source.before.clone(),
                after_surface: source.after.clone(),
                scope: destination_scope.clone(),
                match_policy: source.match_policy,
                manual_scoped: source.manual_scoped,
                expected_rows: expected_rows.clone(),
                replace_conflicts: false,
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    let pair_snapshots_match =
        store::learn_pair_transfer_intents_match_snapshot(pairs, &pair_preflight_intents).map_err(
            |error| {
                format!("learning_transfer_stale_preview: cannot validate local rules: {error}")
            },
        )?;
    if !pair_snapshots_match {
        return Err("learning_transfer_stale_preview: local word rules changed".into());
    }

    let mut choices = HashMap::new();
    for choice in &input.choices {
        if choices
            .insert(choice.item_id.as_str(), choice.action)
            .is_some()
        {
            return Err("learning_transfer_invalid_choice: duplicate item choice".into());
        }
    }
    for choice in &input.choices {
        let Some(item) = session
            .items
            .iter()
            .find(|item| item.view.item_id == choice.item_id)
        else {
            return Err("learning_transfer_invalid_choice: unknown item".into());
        };
        if item.view.status != TransferItemStatus::Conflict
            || !item.view.choice_options.contains(&choice.action)
        {
            return Err(
                "learning_transfer_invalid_choice: action is not available for this item".into(),
            );
        }
    }

    let mut next_settings = settings.clone();
    let starting_dictionary = settings.dictionary.clone();
    let starting_dictionary_sources = settings.dictionary_sources.clone();
    let starting_mappings = settings.context_mappings.clone();
    let manual_owner = store::DictionarySource::Manual;
    let mut preliminary_results = Vec::with_capacity(session.items.len());
    let mut pair_intents = Vec::new();
    let mut pair_item_ids = Vec::new();
    let mut pair_mapping_item_ids = Vec::new();

    for item in &session.items {
        let action = match item.view.status {
            TransferItemStatus::Ready => Some(TransferAction::Import),
            TransferItemStatus::Conflict => Some(
                choices
                    .get(item.view.item_id.as_str())
                    .copied()
                    .unwrap_or(TransferAction::KeepLocal),
            ),
            TransferItemStatus::AlreadyPresent => {
                preliminary_results.push(item_result(
                    &item.view.item_id,
                    TransferItemResultStatus::AlreadyPresent,
                    item.view.reason.clone(),
                ));
                None
            }
            TransferItemStatus::Blocked => {
                preliminary_results.push(item_result(
                    &item.view.item_id,
                    TransferItemResultStatus::Blocked,
                    item.view.reason.clone(),
                ));
                None
            }
        };
        let Some(action) = action else {
            continue;
        };
        if action == TransferAction::KeepLocal {
            preliminary_results.push(item_result(
                &item.view.item_id,
                TransferItemResultStatus::KeptLocal,
                None,
            ));
            continue;
        }
        match &item.value {
            PlannedTransferValue::Word(word) => {
                if action != TransferAction::Import && action != TransferAction::ImportGlobal {
                    return Err(
                        "learning_transfer_invalid_choice: global word action is invalid".into(),
                    );
                }
                if next_settings.has_dictionary_source(word, &manual_owner) {
                    preliminary_results.push(item_result(
                        &item.view.item_id,
                        TransferItemResultStatus::AlreadyPresent,
                        Some("already_global".into()),
                    ));
                } else if next_settings.add_dictionary_source(word, manual_owner.clone()) {
                    preliminary_results.push(item_result(
                        &item.view.item_id,
                        TransferItemResultStatus::Imported,
                        None,
                    ));
                } else {
                    preliminary_results.push(item_result(
                        &item.view.item_id,
                        TransferItemResultStatus::Failed,
                        Some("dictionary_capacity".into()),
                    ));
                }
            }
            PlannedTransferValue::Pair {
                source,
                destination_pair_key,
                destination_scope,
                expected_rows,
                replacement_allowed,
            } => {
                if action != TransferAction::Import && action != TransferAction::Replace {
                    return Err("learning_transfer_invalid_choice: pair action is invalid".into());
                }
                if action == TransferAction::Replace && !replacement_allowed {
                    return Err(
                        "learning_transfer_invalid_choice: pair replacement is unavailable".into(),
                    );
                }
                if let Some(mapping_id) = destination_scope.mapping_id.as_deref() {
                    ensure_planned_mapping(&mut next_settings, &session.new_mappings, mapping_id)?;
                }
                pair_intents.push(store::LearnPairTransferIntent {
                    pair_key: destination_pair_key.clone(),
                    before_surface: source.before.clone(),
                    after_surface: source.after.clone(),
                    scope: destination_scope.clone(),
                    match_policy: source.match_policy,
                    manual_scoped: source.manual_scoped,
                    expected_rows: expected_rows.clone(),
                    replace_conflicts: action == TransferAction::Replace,
                });
                pair_item_ids.push(item.view.item_id.clone());
                if item.view.will_create_mapping {
                    pair_mapping_item_ids.push(item.view.item_id.clone());
                }
                // The storage transaction supplies the final outcome.
                preliminary_results.push(item_result(
                    &item.view.item_id,
                    TransferItemResultStatus::Failed,
                    Some("pair_write_pending".into()),
                ));
            }
            PlannedTransferValue::Preference {
                source,
                destination_mapping_id,
            } => {
                if action != TransferAction::Import && action != TransferAction::Replace {
                    return Err(
                        "learning_transfer_invalid_choice: preference action is invalid".into(),
                    );
                }
                ensure_planned_mapping(
                    &mut next_settings,
                    &session.new_mappings,
                    destination_mapping_id,
                )?;
                apply_preference_import(
                    &mut next_settings,
                    destination_mapping_id,
                    source,
                    action == TransferAction::Replace,
                )?;
                preliminary_results.push(item_result(
                    &item.view.item_id,
                    TransferItemResultStatus::Imported,
                    None,
                ));
            }
        }
    }

    let settings_changed = next_settings.dictionary != starting_dictionary
        || next_settings.dictionary_sources != starting_dictionary_sources
        || next_settings.context_mappings != starting_mappings;
    Ok(PreparedTransferApply {
        next_settings,
        settings_changed,
        pair_intents,
        pair_item_ids,
        pair_mapping_item_ids,
        preliminary_results,
    })
}

fn item_result(
    item_id: &str,
    status: TransferItemResultStatus,
    reason: Option<String>,
) -> LearningTransferItemResult {
    LearningTransferItemResult {
        item_id: item_id.to_owned(),
        status,
        reason,
    }
}

fn ensure_planned_mapping(
    settings: &mut Settings,
    new_mappings: &[AppMapping],
    mapping_id: &str,
) -> Result<(), String> {
    if settings
        .context_mappings
        .iter()
        .any(|mapping| mapping.id == mapping_id && !mapping.deleted)
    {
        return Ok(());
    }
    let mapping = new_mappings
        .iter()
        .find(|mapping| mapping.id == mapping_id)
        .cloned()
        .ok_or_else(|| {
            "learning_transfer_stale_preview: planned mapping is unavailable".to_owned()
        })?;
    settings.context_mappings.push(mapping);
    Ok(())
}

fn apply_preference_import(
    settings: &mut Settings,
    mapping_id: &str,
    source: &TransferWritingPreferenceV1,
    replace: bool,
) -> Result<(), String> {
    let mapping = settings
        .context_mappings
        .iter_mut()
        .find(|mapping| mapping.id == mapping_id && !mapping.deleted)
        .ok_or_else(|| {
            "learning_transfer_stale_preview: preference mapping is unavailable".to_owned()
        })?;
    if mapping.writing_task_id != source.target.selected_task {
        return Err("learning_transfer_stale_preview: destination writing task changed".into());
    }
    let existing_index = mapping.writing_preferences.iter().position(|preference| {
        preference.task_id.or(mapping.writing_task_id) == Some(source.task_id)
            && preference.dimension == source.dimension
    });
    if let Some(index) = existing_index {
        let existing = &mapping.writing_preferences[index];
        if existing.status == WritingPreferenceStatus::Forgotten {
            return Err(
                "learning_transfer_stale_preview: forgotten preference cannot be restored".into(),
            );
        }
        if !replace {
            return Err(
                "learning_transfer_stale_preview: preference conflict requires explicit replace"
                    .into(),
            );
        }
        mapping.writing_preferences[index] = WritingPreference {
            task_id: Some(source.task_id),
            dimension: source.dimension,
            value: source.value,
            status: WritingPreferenceStatus::Active,
            version: source.version,
            source: source.source,
        };
    } else {
        if mapping.writing_preferences.len() >= 15 {
            return Err(
                "learning_transfer_settings_failed: mapping preference capacity reached".into(),
            );
        }
        mapping.writing_preferences.push(WritingPreference {
            task_id: Some(source.task_id),
            dimension: source.dimension,
            value: source.value,
            status: WritingPreferenceStatus::Active,
            version: source.version,
            source: source.source,
        });
    }
    if Some(source.task_id) == mapping.writing_task_id
        && matches!(
            source.dimension,
            WritingPreferenceDimension::GreetingTone
                | WritingPreferenceDimension::TerminalPunctuation
        )
    {
        // Imported cloud-supported guidance must be explicitly approved again
        // on this installation, even if the destination had a prior grant.
        mapping.preference_guidance_provider_approved = false;
    }
    mapping.preference_revision = mapping.preference_revision.wrapping_add(1).max(1);
    mapping.validate()?;
    Ok(())
}

pub(crate) fn finish_apply(
    prepared: PreparedTransferApply,
    outcomes: &[store::LearnPairTransferOutcome],
) -> Result<LearningTransferApplyResult, String> {
    if outcomes.len() != prepared.pair_item_ids.len() {
        return Err(
            "learning_transfer_apply_failed: pair result count did not match request".into(),
        );
    }
    let mut results = prepared.preliminary_results;
    let result_index = results
        .iter()
        .enumerate()
        .map(|(index, result)| (result.item_id.clone(), index))
        .collect::<HashMap<_, _>>();
    for (item_id, outcome) in prepared.pair_item_ids.iter().zip(outcomes) {
        let result = match outcome {
            store::LearnPairTransferOutcome::Inserted => {
                item_result(item_id, TransferItemResultStatus::Imported, None)
            }
            store::LearnPairTransferOutcome::Replaced { .. } => item_result(
                item_id,
                TransferItemResultStatus::Imported,
                Some("pair_replaced_and_localized".into()),
            ),
            store::LearnPairTransferOutcome::AlreadyPresent => item_result(
                item_id,
                TransferItemResultStatus::AlreadyPresent,
                Some("already_present".into()),
            ),
            store::LearnPairTransferOutcome::Conflict => item_result(
                item_id,
                TransferItemResultStatus::KeptLocal,
                Some("pair_conflict_rechecked".into()),
            ),
            store::LearnPairTransferOutcome::Blocked => item_result(
                item_id,
                TransferItemResultStatus::Blocked,
                Some("pair_blocked_after_recheck".into()),
            ),
            store::LearnPairTransferOutcome::StalePreview => item_result(
                item_id,
                TransferItemResultStatus::Blocked,
                Some("stale_pair_snapshot".into()),
            ),
        };
        let index = result_index.get(item_id).copied().ok_or_else(|| {
            "learning_transfer_apply_failed: pair result item was not planned".to_owned()
        })?;
        results[index] = result;
    }
    let mut partial = false;
    for item_id in &prepared.pair_mapping_item_ids {
        let Some(result) = results.iter_mut().find(|result| result.item_id == *item_id) else {
            continue;
        };
        if matches!(
            result.status,
            TransferItemResultStatus::KeptLocal
                | TransferItemResultStatus::Blocked
                | TransferItemResultStatus::Failed
        ) {
            result.reason = Some("learning_transfer_mapping_created_pair_failed".into());
            partial = true;
        }
    }
    let status = if partial { "partial" } else { "applied" };
    Ok(LearningTransferApplyResult { status, results })
}

fn resolve_target_mapping(
    target: &TransferTargetV1,
    all_mappings: &mut Vec<AppMapping>,
    new_mappings: &mut Vec<AppMapping>,
    check_selected_task: bool,
) -> Result<String, String> {
    validate_target(target)?;
    match destination_mapping(target, all_mappings) {
        MappingTransferResolution::Unique(mapping_id) => {
            if has_destination_specificity_tie(target, &mapping_id, all_mappings, new_mappings) {
                return Err("mapping_selector_ambiguous".into());
            }
            let mapping = all_mappings
                .iter()
                .find(|mapping| mapping.id == mapping_id)
                .ok_or_else(|| "mapping_unavailable".to_owned())?;
            if !mapping.enabled {
                return Err("mapping_disabled".into());
            }
            if check_selected_task && mapping.writing_task_id != target.selected_task {
                if let Some(planned) = new_mappings
                    .iter_mut()
                    .find(|candidate| candidate.id == mapping_id)
                {
                    if planned.writing_task_id.is_none() {
                        planned.writing_task_id = target.selected_task;
                        if let Some(all_mapping) = all_mappings
                            .iter_mut()
                            .find(|candidate| candidate.id == mapping_id)
                        {
                            all_mapping.writing_task_id = target.selected_task;
                        }
                    } else {
                        return Err("mapping_task_conflict".into());
                    }
                } else {
                    return Err("mapping_task_conflict".into());
                }
            }
            Ok(mapping_id)
        }
        MappingTransferResolution::Ambiguous => Err("mapping_selector_ambiguous".into()),
        MappingTransferResolution::Missing => {
            let destination_mappings = all_mappings
                .iter()
                .filter(|mapping| !new_mappings.iter().any(|planned| planned.id == mapping.id))
                .cloned()
                .collect::<Vec<_>>();
            if mapping_overlap_exists(target, &destination_mappings) {
                return Err("mapping_selector_overlap".into());
            }
            // New targets from this file are evaluated as a set after all
            // items have been classified. In particular, a blocked item must
            // not reserve its selector or make the result order-dependent.
            let mapping =
                make_new_mapping(target, &[]).map_err(|_| "mapping_selector_overlap".to_owned())?;
            let id = mapping.id.clone();
            all_mappings.push(mapping.clone());
            new_mappings.push(mapping);
            Ok(id)
        }
    }
}

fn has_destination_specificity_tie(
    target: &TransferTargetV1,
    resolved_mapping_id: &str,
    mappings: &[AppMapping],
    new_mappings: &[AppMapping],
) -> bool {
    mappings
        .iter()
        .filter(|mapping| {
            !mapping.deleted
                && mapping.enabled
                && mapping.id != resolved_mapping_id
                && !new_mappings.iter().any(|planned| planned.id == mapping.id)
        })
        .any(|mapping| {
            let selector = raw_mapping_selector(mapping);
            selectors_overlap(&target.selector, &selector)
                && crate::context::transfer_selector_specificity_cmp(&target.selector, &selector)
                    == std::cmp::Ordering::Equal
        })
}

fn classify_pair_preview(
    pair_key: &str,
    scope: &LearnPairScope,
    source: &TransferWordPairV1,
    expected: &[store::LearnPairTransferExpectedRow],
) -> (
    TransferItemStatus,
    TransferAction,
    Vec<TransferAction>,
    Option<String>,
    bool,
    Option<String>,
) {
    if expected.iter().any(|row| {
        row.pair_key == pair_key
            && (!aliases_intersect(source, row)
                || row.after_surface != source.after
                || row.match_policy != source.match_policy
                || row.scope != *scope)
    }) {
        return (
            TransferItemStatus::Blocked,
            TransferAction::None,
            Vec::new(),
            Some("pair_key_identity_conflict".into()),
            false,
            expected.first().map(|row| row.after_surface.clone()),
        );
    }
    let has_equivalent_same_policy = expected.iter().any(|row| {
        row.scope == *scope
            && row.promoted
            && !row.ignored
            && !row.tombstoned
            && !row.binding_invalidated
            && row.after_surface == source.after
            && row.match_policy == source.match_policy
            && aliases_intersect(source, row)
    });
    let relevant = expected
        .iter()
        .filter(|row| {
            // Another live scope with the same alias, output, and policy does
            // not conflict with creating the narrower portable destination.
            !(row.pair_key != pair_key
                && row.scope != *scope
                && row.promoted
                && !row.ignored
                && !row.tombstoned
                && !row.binding_invalidated
                && row.after_surface == source.after
                && row.match_policy == source.match_policy
                && aliases_intersect(source, row))
                // Exact and IgnoreCase rules with the same output can coexist
                // at one scope. Once a matching policy is already present,
                // its alternate-policy sibling must not make re-preview
                // ambiguous. If the alternate is the only local rule, it
                // remains a real policy conflict.
                && !(has_equivalent_same_policy
                    && row.pair_key != pair_key
                    && row.scope == *scope
                    && row.promoted
                    && !row.ignored
                    && !row.tombstoned
                    && !row.binding_invalidated
                    && row.after_surface == source.after
                    && row.match_policy != source.match_policy
                    && aliases_intersect(source, row))
        })
        .collect::<Vec<_>>();
    if relevant.is_empty() {
        return (
            TransferItemStatus::Ready,
            TransferAction::Import,
            Vec::new(),
            None,
            false,
            None,
        );
    }
    if relevant
        .iter()
        .any(|row| row.ignored || row.tombstoned || row.binding_invalidated || !row.promoted)
    {
        return (
            TransferItemStatus::Blocked,
            TransferAction::None,
            Vec::new(),
            Some("pair_has_local_suppression_or_pending_state".into()),
            false,
            relevant.first().map(|row| row.after_surface.clone()),
        );
    }
    let equivalent = relevant
        .iter()
        .copied()
        .filter(|row| {
            row.scope == *scope
                && row.after_surface == source.after
                && aliases_intersect(source, row)
                && row.match_policy == source.match_policy
        })
        .collect::<Vec<_>>();
    if equivalent.len() == relevant.len() && !equivalent.is_empty() {
        let has_global_owner = equivalent.iter().any(|row| {
            !row.auto_scoped
                && !row.manual_scoped
                && row.scope.family.is_none()
                && row.scope.mapping_id.is_none()
                && row.scope.browser_host.is_none()
                && row.scope.native_bundle.is_none()
        });
        if !has_global_owner {
            return (
                TransferItemStatus::AlreadyPresent,
                TransferAction::None,
                Vec::new(),
                Some("already_present".into()),
                false,
                equivalent.first().map(|row| row.after_surface.clone()),
            );
        }
        return (
            TransferItemStatus::Conflict,
            TransferAction::KeepLocal,
            vec![TransferAction::KeepLocal, TransferAction::Replace],
            Some("import_requires_local_only_pair".into()),
            true,
            equivalent.first().map(|row| row.after_surface.clone()),
        );
    }
    if relevant.len() != 1 {
        return (
            TransferItemStatus::Blocked,
            TransferAction::None,
            Vec::new(),
            Some("pair_conflict_ambiguous".into()),
            false,
            relevant.first().map(|row| row.after_surface.clone()),
        );
    }
    let row = relevant[0];
    let same_scope = row.scope == *scope;
    let same_alias = aliases_intersect(source, row);
    if same_scope && row.promoted && same_alias {
        return (
            TransferItemStatus::Conflict,
            TransferAction::KeepLocal,
            vec![TransferAction::KeepLocal, TransferAction::Replace],
            Some("pair_conflict".into()),
            true,
            Some(row.after_surface.clone()),
        );
    }
    (
        TransferItemStatus::Blocked,
        TransferAction::None,
        Vec::new(),
        Some("pair_scope_overlap_or_ambiguous".into()),
        false,
        Some(row.after_surface.clone()),
    )
}

fn aliases_intersect(
    source: &TransferWordPairV1,
    row: &store::LearnPairTransferExpectedRow,
) -> bool {
    match (source.match_policy, row.match_policy) {
        (store::LearnPairMatchPolicy::Exact, store::LearnPairMatchPolicy::Exact) => {
            source.before == row.before_surface
        }
        _ => source.before.eq_ignore_ascii_case(&row.before_surface),
    }
}

fn pair_scope_summary(source: &TransferWordPairV1) -> String {
    let mut summary = source
        .target
        .as_ref()
        .map(target_scope_summary)
        .unwrap_or_else(|| "Direct local rule scope".into());
    if let Some(scope) = &source.pair_scope {
        if let Some(family) = scope.family {
            summary.push_str(&format!(" · family {:?}", family));
        }
        if let Some(host) = &scope.browser_host {
            summary.push_str(&format!(" · host {host}"));
        }
        if let Some(bundle) = &scope.native_bundle {
            summary.push_str(&format!(" · bundle {bundle}"));
        }
    }
    summary
}

fn target_scope_summary(target: &TransferTargetV1) -> String {
    let selector = &target.selector;
    let mut parts = vec![format!("{:?}", selector.family)];
    for (label, value) in [
        ("bundle", selector.bundle_id.as_deref()),
        ("executable", selector.executable.as_deref()),
        ("host", selector.browser_host.as_deref()),
        ("path", selector.browser_path_prefix.as_deref()),
    ] {
        if let Some(value) = value {
            parts.push(format!("{label} {value}"));
        }
    }
    if let Some(field) = selector.focused_field {
        parts.push(format!("field {}", focus_kind_id(field)));
    }
    if let Some(task) = target.selected_task {
        parts.push(format!("selected task {task:?}"));
    }
    parts.join(" · ")
}

fn focus_kind_id(kind: FocusKind) -> String {
    format!("{kind:?}")
}

fn preference_value_summary(preference: &WritingPreference) -> String {
    format!("{:?} ({:?})", preference.value, preference.status)
}

fn blocked_pair_item(
    item_id: String,
    source: &TransferWordPairV1,
    reason: &str,
) -> PlannedTransferItem {
    PlannedTransferItem {
        view: LearningTransferItem {
            item_id,
            kind: TransferItemKind::WordPair,
            summary: format!("{} → {}", source.before, source.after),
            scope_summary: pair_scope_summary(source),
            will_create_mapping: false,
            status: TransferItemStatus::Blocked,
            default_action: TransferAction::None,
            choice_options: Vec::new(),
            reason: Some(reason.to_owned()),
            details: Some(TransferItemDetails {
                word: None,
                before: Some(source.before.clone()),
                after: Some(source.after.clone()),
                task_id: source
                    .target
                    .as_ref()
                    .and_then(|target| target.selected_task),
                dimension: None,
                value: None,
                existing_value: None,
            }),
        },
        value: PlannedTransferValue::Pair {
            source: source.clone(),
            destination_pair_key: String::new(),
            destination_scope: LearnPairScope::default(),
            expected_rows: Vec::new(),
            replacement_allowed: false,
        },
    }
}

fn blocked_preference_item(
    item_id: String,
    source: &TransferWritingPreferenceV1,
    reason: &str,
) -> PlannedTransferItem {
    PlannedTransferItem {
        view: LearningTransferItem {
            item_id,
            kind: TransferItemKind::WritingPreference,
            summary: format!("{:?}: {:?}", source.dimension, source.value),
            scope_summary: target_scope_summary(&source.target),
            will_create_mapping: false,
            status: TransferItemStatus::Blocked,
            default_action: TransferAction::None,
            choice_options: Vec::new(),
            reason: Some(reason.to_owned()),
            details: Some(TransferItemDetails {
                word: None,
                before: None,
                after: None,
                task_id: Some(source.task_id),
                dimension: Some(source.dimension),
                value: Some(source.value),
                existing_value: None,
            }),
        },
        value: PlannedTransferValue::Preference {
            source: source.clone(),
            destination_mapping_id: String::new(),
        },
    }
}

/// Prove that a more-specific source mapping owns every possible signal in a
/// browser-host-scoped rule. Pair scopes do not carry page paths or focused
/// fields, so selectors that require either cannot be proven to shadow the
/// complete observed host scope.
fn mapping_selector_covers_pair_scope(
    candidate: &MappingTransferSelectorV1,
    source_owner: &MappingTransferSelectorV1,
    pair_scope: &TransferPairScopeV1,
) -> bool {
    if let Some(observed_host) = pair_scope.browser_host.as_deref() {
        return candidate
            .browser_host
            .as_deref()
            .is_some_and(|host| host_selector_matches(host, observed_host))
            && candidate.browser_path_prefix.is_none()
            && candidate.focused_field.is_none()
            && candidate.bundle_id.as_deref().is_none_or(|bundle| {
                source_owner
                    .bundle_id
                    .as_deref()
                    .is_some_and(|owner| owner.eq_ignore_ascii_case(bundle))
            })
            && candidate.executable.as_deref().is_none_or(|executable| {
                source_owner
                    .executable
                    .as_deref()
                    .is_some_and(|owner| owner.eq_ignore_ascii_case(executable))
            });
    }

    let known_native_bundle = pair_scope
        .native_bundle
        .as_deref()
        .or(source_owner.bundle_id.as_deref());
    known_native_bundle.is_some_and(|bundle| {
        candidate
            .bundle_id
            .as_deref()
            .is_some_and(|candidate_bundle| candidate_bundle.eq_ignore_ascii_case(bundle))
            && candidate.browser_host.is_none()
            && candidate.browser_path_prefix.is_none()
            && candidate.focused_field.is_none()
            && candidate.executable.as_deref().is_none_or(|executable| {
                source_owner
                    .executable
                    .as_deref()
                    .is_some_and(|owner| owner.eq_ignore_ascii_case(executable))
            })
    })
}

fn transfer_pair_from_record(
    row: &LearnPairRecord,
    mappings: &[AppMapping],
) -> Result<TransferWordPairV1, String> {
    let unscoped = !row.has_declared_scope();
    if unscoped {
        if row.family.is_some()
            || row.mapping_id.is_some()
            || row.browser_host.is_some()
            || row.native_bundle.is_some()
        {
            return Err("partial global pair scope".into());
        }
        return Ok(TransferWordPairV1 {
            target: None,
            pair_scope: None,
            before: row.before_surface.clone(),
            after: row.after_surface.clone(),
            match_policy: row.match_policy,
            manual_scoped: row.manual_scoped,
        });
    }
    let Some(mapping_id) = row.mapping_id.as_deref() else {
        return Ok(TransferWordPairV1 {
            target: None,
            pair_scope: Some(TransferPairScopeV1 {
                family: parse_row_family(row.family.as_deref())?,
                browser_host: row.browser_host.clone(),
                native_bundle: row.native_bundle.clone(),
            }),
            before: row.before_surface.clone(),
            after: row.after_surface.clone(),
            match_policy: row.match_policy,
            manual_scoped: row.manual_scoped,
        });
    };
    let mapping = mappings
        .iter()
        .filter(|mapping| !mapping.deleted && mapping.id == mapping_id)
        .collect::<Vec<_>>();
    if mapping.len() != 1 {
        return Err("pair mapping owner is missing or ambiguous".into());
    }
    let mapping = mapping[0];
    if row.family.as_deref() != Some(crate::context::family_id(mapping.family)) {
        return Err("pair family differs from its live mapping owner".into());
    }
    if !mapping.enabled {
        return Err("pair mapping owner is disabled".into());
    }
    let selector = mapping.transfer_selector()?;
    if let (Some(expected), Some(observed)) = (
        selector.browser_host.as_deref(),
        row.browser_host.as_deref(),
    ) {
        if !host_selector_matches(expected, observed) {
            return Err("pair host is outside its live mapping selector".into());
        }
    }
    if selector.browser_host.is_some() && row.browser_host.is_none() {
        return Err("scoped browser pair is missing its observed host".into());
    }
    if selector.bundle_id.as_deref().is_some_and(|expected| {
        row.native_bundle
            .as_deref()
            .is_some_and(|actual| !expected.eq_ignore_ascii_case(actual))
    }) {
        return Err("pair bundle differs from its live mapping selector".into());
    }
    if mapping.enabled && mapping_has_active_specificity_tie(mapping, mappings) {
        return Err("pair mapping owner has a runtime specificity tie".into());
    }
    let pair_scope = TransferPairScopeV1 {
        family: parse_row_family(row.family.as_deref())?,
        browser_host: row.browser_host.clone(),
        native_bundle: row.native_bundle.clone(),
    };
    if mapping.enabled
        && mappings
            .iter()
            .filter(|other| other.id != mapping.id && !other.deleted && other.enabled)
            .any(|other| {
                let Ok(other_selector) = other.transfer_selector() else {
                    return false;
                };
                selectors_overlap(&selector, &other_selector)
                    && crate::context::transfer_selector_specificity_cmp(&selector, &other_selector)
                        == std::cmp::Ordering::Less
                    && mapping_selector_covers_pair_scope(&other_selector, &selector, &pair_scope)
            })
    {
        return Err("pair owner is shadowed across its complete observed scope".into());
    }
    Ok(TransferWordPairV1 {
        target: Some(TransferTargetV1 {
            selector,
            // Pair portability follows its declarative scope, not which
            // writing task happens to be selected on the source Mac.
            selected_task: None,
        }),
        pair_scope: Some(pair_scope),
        before: row.before_surface.clone(),
        after: row.after_surface.clone(),
        match_policy: row.match_policy,
        manual_scoped: row.manual_scoped,
    })
}

fn mapping_has_active_specificity_tie(mapping: &AppMapping, mappings: &[AppMapping]) -> bool {
    let Ok(selector) = mapping.transfer_selector() else {
        return false;
    };
    mappings
        .iter()
        .filter(|other| other.id != mapping.id && !other.deleted && other.enabled)
        .any(|other| {
            let Ok(other_selector) = other.transfer_selector() else {
                return false;
            };
            selectors_overlap(&selector, &other_selector)
                && crate::context::transfer_selector_specificity_cmp(&selector, &other_selector)
                    == std::cmp::Ordering::Equal
        })
}

fn parse_family_id(value: &str) -> Option<ContextFamily> {
    serde_json::from_value(serde_json::Value::String(value.to_owned())).ok()
}

fn parse_row_family(value: Option<&str>) -> Result<Option<ContextFamily>, String> {
    value
        .map(|value| parse_family_id(value).ok_or_else(|| "pair has an unknown family".into()))
        .transpose()
}

fn host_selector_matches(selector: &str, observed: &str) -> bool {
    let selector = selector.to_ascii_lowercase();
    let observed = observed.to_ascii_lowercase();
    selector == observed || observed.strip_suffix(&format!(".{selector}")).is_some()
}

pub(crate) fn make_transfer_file_bytes(file: &LearningTransferFileV1) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec_pretty(file)
        .map_err(|error| format!("learning_transfer_export_failed: {error}"))?;
    if bytes.len() > MAX_TRANSFER_FILE_BYTES {
        return Err(format!(
            "learning_transfer_export_failed: transfer file exceeds {MAX_TRANSFER_FILE_BYTES} bytes"
        ));
    }
    Ok(bytes)
}

pub(crate) fn write_export_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_TRANSFER_FILE_BYTES {
        return Err("learning_transfer_export_failed: export file exceeds size limit".into());
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| "learning_transfer_export_failed: invalid destination path".to_owned())?;
    let parent_metadata = fs::metadata(parent).map_err(|error| {
        format!("learning_transfer_export_failed: cannot inspect folder: {error}")
    })?;
    if !parent_metadata.is_dir() {
        return Err(
            "learning_transfer_export_failed: destination folder is not a directory".into(),
        );
    }
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(
            "learning_transfer_export_failed: destination cannot be a symbolic link".into(),
        );
    }
    let name = path
        .file_name()
        .ok_or_else(|| "learning_transfer_export_failed: invalid destination filename".to_owned())?
        .to_string_lossy();
    let mut temp_path = None;
    let mut temp_file: Option<File> = None;
    for _ in 0..8 {
        let temp_name = format!(".{name}.tmp-{}-{}", std::process::id(), next_preview_id());
        let candidate = parent.join(temp_name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        match options.open(&candidate) {
            Ok(file) => {
                temp_path = Some(candidate);
                temp_file = Some(file);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "learning_transfer_export_failed: cannot create temp file: {error}"
                ));
            }
        }
    }
    let temp_path = temp_path.ok_or_else(|| {
        "learning_transfer_export_failed: cannot allocate a safe temporary file".to_owned()
    })?;
    let mut temp_file = temp_file.expect("temporary file path and handle are paired");
    if let Err(error) = temp_file
        .write_all(bytes)
        .and_then(|()| temp_file.sync_all())
    {
        let _ = fs::remove_file(&temp_path);
        return Err(format!(
            "learning_transfer_export_failed: cannot write export: {error}"
        ));
    }
    drop(temp_file);
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        let _ = fs::remove_file(&temp_path);
        return Err(
            "learning_transfer_export_failed: destination changed to a symbolic link".into(),
        );
    }
    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(format!(
            "learning_transfer_export_failed: cannot publish export: {error}"
        ));
    }
    #[cfg(unix)]
    {
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                format!("learning_transfer_export_failed: cannot sync folder: {error}")
            })?;
    }
    Ok(())
}

pub(crate) fn destination_mapping(
    target: &TransferTargetV1,
    mappings: &[AppMapping],
) -> MappingTransferResolution {
    target.selector.resolve(mappings)
}

pub(crate) fn mapping_overlap_exists(target: &TransferTargetV1, mappings: &[AppMapping]) -> bool {
    mappings
        .iter()
        .filter(|mapping| !mapping.deleted)
        .any(|mapping| selectors_overlap(&target.selector, &raw_mapping_selector(mapping)))
}

pub(crate) fn local_preference_for(
    mapping: &AppMapping,
    task_id: WritingTaskId,
    dimension: WritingPreferenceDimension,
) -> Option<&WritingPreference> {
    mapping.writing_preferences.iter().find(|preference| {
        preference.task_id.or(mapping.writing_task_id) == Some(task_id)
            && preference.dimension == dimension
    })
}

pub(crate) fn make_new_mapping(
    target: &TransferTargetV1,
    mappings: &[AppMapping],
) -> Result<AppMapping, String> {
    if mapping_overlap_exists(target, mappings) {
        return Err("learning_transfer_mapping_overlap".into());
    }
    let selector = &target.selector;
    let label = selector
        .browser_host
        .as_deref()
        .or(selector.bundle_id.as_deref())
        .or(selector.executable.as_deref())
        .unwrap_or("Transferred target");
    let mut id = String::new();
    for _ in 0..8 {
        let seed = format!(
            "{}:{}:{}",
            now_ms(),
            NEXT_PREVIEW_ID.fetch_add(1, Ordering::Relaxed),
            label
        );
        let digest = Sha256::digest(seed.as_bytes());
        id = format!("imported-{}", hex::encode(&digest[..12]));
        if mappings.iter().all(|mapping| mapping.id != id) {
            break;
        }
    }
    let mapping = AppMapping {
        id,
        label: label.chars().take(128).collect(),
        family: selector.family,
        writing_task_id: target.selected_task,
        writing_preferences: Vec::new(),
        preference_guidance_provider_approved: false,
        preference_revision: 0,
        deleted: false,
        mode_id: None,
        bundle_id: selector.bundle_id.clone(),
        executable: selector.executable.clone(),
        browser_host: selector.browser_host.clone(),
        browser_path_prefix: selector.browser_path_prefix.clone(),
        focused_field: selector.focused_field,
        source_permissions: Default::default(),
        style_example_input: None,
        style_example_output: None,
        style_example_pairs: Vec::new(),
        style_examples_approved: false,
        enabled: true,
        cleanup_effort: None,
        cleanup_intensity: None,
        cleanup_enabled: true,
        dictionary_learn_enabled: false,
    };
    mapping.validate()?;
    Ok(mapping)
}

#[cfg(test)]
mod tests {
    use super::*;

    static PREVIEW_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn selector() -> MappingTransferSelectorV1 {
        MappingTransferSelectorV1 {
            version: 1,
            family: ContextFamily::General,
            bundle_id: Some("com.example.editor".into()),
            executable: None,
            browser_host: None,
            browser_path_prefix: None,
            focused_field: None,
        }
    }

    fn app_mapping(id: &str, family: ContextFamily, bundle: &str) -> AppMapping {
        AppMapping {
            id: id.into(),
            label: "Example editor".into(),
            family,
            writing_task_id: None,
            writing_preferences: Vec::new(),
            preference_guidance_provider_approved: false,
            preference_revision: 0,
            deleted: false,
            mode_id: None,
            bundle_id: Some(bundle.into()),
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
            dictionary_learn_enabled: false,
        }
    }

    fn pair(
        before: &str,
        after: &str,
        target: Option<TransferTargetV1>,
        pair_scope: Option<TransferPairScopeV1>,
    ) -> TransferWordPairV1 {
        TransferWordPairV1 {
            target,
            pair_scope,
            before: before.into(),
            after: after.into(),
            match_policy: store::LearnPairMatchPolicy::Exact,
            manual_scoped: false,
        }
    }

    fn target_with_path(path: &str) -> TransferTargetV1 {
        TransferTargetV1 {
            selector: MappingTransferSelectorV1 {
                version: 1,
                family: ContextFamily::BrowserSearch,
                bundle_id: None,
                executable: None,
                browser_host: Some("example.com".into()),
                browser_path_prefix: Some(path.into()),
                focused_field: None,
            },
            selected_task: None,
        }
    }

    fn file_with_pair(pair: TransferWordPairV1) -> LearningTransferFileV1 {
        LearningTransferFileV1 {
            format: TRANSFER_FORMAT.into(),
            schema_version: 1,
            global_words: Vec::new(),
            word_pairs: vec![pair],
            writing_preferences: Vec::new(),
        }
    }

    fn file_with_preferences(
        preferences: Vec<TransferWritingPreferenceV1>,
    ) -> LearningTransferFileV1 {
        LearningTransferFileV1 {
            format: TRANSFER_FORMAT.into(),
            schema_version: 1,
            global_words: Vec::new(),
            word_pairs: Vec::new(),
            writing_preferences: preferences,
        }
    }

    fn new_transfer_test_root(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "voiceflow-learning-transfer-{label}-{}-{}",
            std::process::id(),
            next_preview_id()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn mapping_for_target(id: &str, target: &TransferTargetV1) -> AppMapping {
        let selector = &target.selector;
        let mut mapping = app_mapping(
            id,
            selector.family,
            selector
                .bundle_id
                .as_deref()
                .unwrap_or("com.example.placeholder"),
        );
        mapping.writing_task_id = target.selected_task;
        mapping.bundle_id = selector.bundle_id.clone();
        mapping.executable = selector.executable.clone();
        mapping.browser_host = selector.browser_host.clone();
        mapping.browser_path_prefix = selector.browser_path_prefix.clone();
        mapping.focused_field = selector.focused_field;
        mapping
    }

    fn add_promoted_pair(root: &Path, before: &str, after: &str, scope: &LearnPairScope) {
        let policy = store::word_rule_match_policy(before, after);
        let key = store::scoped_word_rule_key(before, after, scope, policy);
        store::upsert_learn_pair_with_scope(root, key.as_str(), before, after, Some(scope))
            .unwrap();
        store::ensure_learn_pair_promoted(root, key.as_str(), before, after, Some(scope)).unwrap();
    }

    fn apply_transfer_file(
        mut file: LearningTransferFileV1,
        destination_root: &Path,
        reverse_pairs: bool,
    ) -> (
        LearningTransferPreview,
        LearningTransferApplyResult,
        Settings,
        Vec<LearnPairRecord>,
    ) {
        if reverse_pairs {
            file.word_pairs.reverse();
        }
        let path = destination_root.join("learning-transfer.json");
        let bytes = make_transfer_file_bytes(&file).unwrap();
        write_export_file(&path, &bytes).unwrap();
        let loaded = load_transfer_file(&path).unwrap();
        let settings = Settings::default();
        let pairs = store::list_learn_pairs(destination_root).unwrap();
        let preview = build_preview(path, loaded, &settings, &pairs).unwrap();
        let prepared = prepare_apply(
            LearningTransferApplyInput {
                preview_id: preview.preview_id.clone(),
                choices: Vec::new(),
            },
            &settings,
            &pairs,
            destination_root,
        )
        .unwrap();
        if prepared.settings_changed {
            store::save_configuration(destination_root, &prepared.next_settings).unwrap();
        }
        let outcomes =
            store::apply_learn_pair_transfers(destination_root, &prepared.pair_intents).unwrap();
        let applied = finish_apply(prepared, &outcomes).unwrap();
        let (settings, _) = store::load_settings(destination_root);
        let pairs = store::list_learn_pairs(destination_root).unwrap();
        (preview, applied, settings, pairs)
    }

    #[test]
    fn pair_export_is_task_independent_and_keeps_observed_browser_host() {
        let mut mapping = app_mapping("local", ContextFamily::BrowserSearch, "com.example.browser");
        mapping.writing_task_id = Some(WritingTaskId::Email);
        mapping.focused_field = Some(FocusKind::Email);
        let row = LearnPairRecord {
            pair_key: "legacy-or-v2-key".into(),
            before_surface: "teh".into(),
            after_surface: "the".into(),
            hits: 1,
            promoted: true,
            last_at: "now".into(),
            family: Some(crate::context::family_id(ContextFamily::BrowserSearch).into()),
            mapping_id: Some("local".into()),
            browser_host: Some("docs.example.com".into()),
            native_bundle: None,
            last_used_at: None,
            pinned: false,
            tombstoned_at: None,
            ignored: false,
            promote_hits: 0,
            manual_scoped: false,
            auto_scoped: false,
            match_policy: store::LearnPairMatchPolicy::IgnoreCase,
            binding_invalidated_at: None,
            binding_invalidated_reason: None,
            replacement_runs: 0,
        };

        let exported = transfer_pair_from_record(&row, &[mapping.clone()]).unwrap();
        assert_eq!(exported.target.as_ref().unwrap().selected_task, None);
        assert_eq!(
            exported
                .pair_scope
                .as_ref()
                .unwrap()
                .browser_host
                .as_deref(),
            Some("docs.example.com")
        );
        assert!(!exported.manual_scoped);
        assert_eq!(
            exported.match_policy,
            store::LearnPairMatchPolicy::IgnoreCase
        );

        let direct_manual = LearnPairRecord {
            pair_key: "manual-direct".into(),
            before_surface: "teh".into(),
            after_surface: "the".into(),
            hits: 1,
            promoted: true,
            last_at: "now".into(),
            family: None,
            mapping_id: None,
            browser_host: None,
            native_bundle: Some("com.example.editor".into()),
            last_used_at: None,
            pinned: false,
            tombstoned_at: None,
            ignored: false,
            promote_hits: 0,
            manual_scoped: true,
            auto_scoped: false,
            match_policy: store::LearnPairMatchPolicy::Exact,
            binding_invalidated_at: None,
            binding_invalidated_reason: None,
            replacement_runs: 0,
        };
        let exported_direct = transfer_pair_from_record(&direct_manual, &[mapping]).unwrap();
        assert!(exported_direct.manual_scoped);
        assert!(validate_transfer_file(&mut file_with_pair(exported_direct)).is_ok());
    }

    #[test]
    fn disabled_source_mapping_learning_is_skipped_but_direct_scope_still_exports() {
        let mut mapping = app_mapping("disabled", ContextFamily::General, "com.example.editor");
        mapping.focused_field = Some(FocusKind::Email);
        mapping.writing_task_id = Some(WritingTaskId::Email);
        mapping.writing_preferences.push(WritingPreference {
            task_id: Some(WritingTaskId::Email),
            dimension: WritingPreferenceDimension::ListStructure,
            value: WritingPreferenceValue::Bullets,
            status: WritingPreferenceStatus::Active,
            version: 1,
            source: WritingPreferenceSource::Manual,
        });

        let disabled_scope = LearnPairScope {
            family: Some(crate::context::family_id(mapping.family).into()),
            mapping_id: Some(mapping.id.clone()),
            native_bundle: mapping.bundle_id.clone(),
            ..LearnPairScope::default()
        };
        let direct_scope = LearnPairScope {
            family: Some(crate::context::family_id(ContextFamily::General).into()),
            native_bundle: Some("com.example.direct-notes".into()),
            ..LearnPairScope::default()
        };
        let make_row = |pair_key: &str,
                        before: &str,
                        after: &str,
                        scope: &LearnPairScope|
         -> LearnPairRecord {
            LearnPairRecord {
                pair_key: pair_key.into(),
                before_surface: before.into(),
                after_surface: after.into(),
                hits: 1,
                promoted: true,
                last_at: "now".into(),
                family: scope.family.clone(),
                mapping_id: scope.mapping_id.clone(),
                browser_host: scope.browser_host.clone(),
                native_bundle: scope.native_bundle.clone(),
                last_used_at: None,
                pinned: false,
                tombstoned_at: None,
                ignored: false,
                promote_hits: 0,
                manual_scoped: false,
                auto_scoped: false,
                match_policy: store::word_rule_match_policy(before, after),
                binding_invalidated_at: None,
                binding_invalidated_reason: None,
                replacement_runs: 0,
            }
        };
        let rows = [
            make_row("disabled-owner", "teh", "the", &disabled_scope),
            make_row("direct-scope", "recieve", "receive", &direct_scope),
        ];
        let settings = Settings {
            context_mappings: vec![mapping.clone()],
            ..Settings::default()
        };

        let (enabled_file, enabled_skipped) =
            build_export_file(&settings, std::slice::from_ref(&mapping), &rows);
        assert_eq!(enabled_skipped, 0);
        assert_eq!(enabled_file.word_pairs.len(), 2);
        assert_eq!(enabled_file.writing_preferences.len(), 1);

        mapping.enabled = false;
        let disabled_settings = Settings {
            context_mappings: vec![mapping.clone()],
            ..Settings::default()
        };
        let (file, skipped) = build_export_file(&disabled_settings, &[mapping], &rows);

        assert_eq!(
            skipped, 2,
            "the disabled pair and active preference are skipped"
        );
        assert_eq!(file.word_pairs.len(), 1);
        assert_eq!(file.word_pairs[0].before, "recieve");
        assert!(file.word_pairs[0].target.is_none());
        assert!(file.writing_preferences.is_empty());
    }

    #[test]
    fn pair_file_rejects_selected_task_and_invalid_manual_scope() {
        let mut selected_task_pair = pair(
            "teh",
            "the",
            Some(TransferTargetV1 {
                selector: selector(),
                selected_task: Some(WritingTaskId::Email),
            }),
            Some(TransferPairScopeV1 {
                family: Some(ContextFamily::General),
                browser_host: None,
                native_bundle: Some("com.example.editor".into()),
            }),
        );
        assert!(validate_transfer_file(&mut file_with_pair(selected_task_pair.clone())).is_err());

        selected_task_pair.target.as_mut().unwrap().selected_task = None;
        selected_task_pair.manual_scoped = true;
        assert!(validate_transfer_file(&mut file_with_pair(selected_task_pair)).is_err());

        let manual_direct_pair = TransferWordPairV1 {
            target: None,
            pair_scope: Some(TransferPairScopeV1 {
                family: None,
                browser_host: None,
                native_bundle: Some("com.example.editor".into()),
            }),
            before: "teh".into(),
            after: "the".into(),
            match_policy: store::LearnPairMatchPolicy::Exact,
            manual_scoped: true,
        };
        assert!(validate_transfer_file(&mut file_with_pair(manual_direct_pair)).is_ok());
    }

    #[test]
    fn ambiguous_export_pairs_are_skipped_but_same_output_sibling_scopes_survive() {
        let target = TransferTargetV1 {
            selector: MappingTransferSelectorV1 {
                version: 1,
                family: ContextFamily::BrowserSearch,
                bundle_id: None,
                executable: None,
                browser_host: Some("example.com".into()),
                browser_path_prefix: None,
                focused_field: None,
            },
            selected_task: None,
        };
        let scope = Some(TransferPairScopeV1 {
            family: Some(ContextFamily::BrowserSearch),
            browser_host: Some("example.com".into()),
            native_bundle: None,
        });
        let conflicting = vec![
            pair("teh", "the", Some(target.clone()), scope.clone()),
            pair("teh", "they", Some(target), scope),
        ];
        let (safe, skipped) = discard_ambiguous_export_pairs(conflicting);
        assert!(safe.is_empty());
        assert_eq!(skipped, 2);

        let sibling_scope = Some(TransferPairScopeV1 {
            family: Some(ContextFamily::BrowserSearch),
            browser_host: Some("example.com".into()),
            native_bundle: None,
        });
        let siblings = vec![
            pair(
                "teh",
                "the",
                Some(target_with_path("/docs")),
                sibling_scope.clone(),
            ),
            pair(
                "teh",
                "the",
                Some(target_with_path("/docs/edit")),
                sibling_scope,
            ),
        ];
        let (safe, skipped) = discard_ambiguous_export_pairs(siblings);
        assert_eq!(safe.len(), 2);
        assert_eq!(skipped, 0);
    }

    #[test]
    fn composed_mapping_scopes_round_trip_in_either_file_order() {
        let _preview_guard = PREVIEW_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let source_root = new_transfer_test_root("composed-source");
        let mut source_settings = Settings::default();

        let target = |family,
                      bundle: Option<&str>,
                      executable: Option<&str>,
                      host: Option<&str>,
                      path: Option<&str>| TransferTargetV1 {
            selector: MappingTransferSelectorV1 {
                version: 1,
                family,
                bundle_id: bundle.map(str::to_owned),
                executable: executable.map(str::to_owned),
                browser_host: host.map(str::to_owned),
                browser_path_prefix: path.map(str::to_owned),
                focused_field: None,
            },
            selected_task: None,
        };

        let cases = vec![
            (
                "generic-chrome",
                target(
                    ContextFamily::General,
                    Some("com.google.Chrome"),
                    None,
                    None,
                    None,
                ),
                None,
                Some("com.google.Chrome"),
                "the",
            ),
            (
                "gmail",
                target(
                    ContextFamily::Email,
                    Some("com.google.Chrome"),
                    None,
                    Some("mail.google.com"),
                    None,
                ),
                Some("mail.google.com"),
                None,
                "they",
            ),
            (
                "docs",
                target(
                    ContextFamily::BrowserSearch,
                    None,
                    None,
                    Some("docs.example.com"),
                    Some("/docs"),
                ),
                Some("docs.example.com"),
                None,
                "the document",
            ),
            (
                "docs-internal",
                target(
                    ContextFamily::BrowserSearch,
                    None,
                    None,
                    Some("docs.example.com"),
                    Some("/docs/internal"),
                ),
                Some("docs.example.com"),
                None,
                "the internal document",
            ),
            (
                "native-notes",
                target(
                    ContextFamily::General,
                    Some("com.example.notes"),
                    None,
                    None,
                    None,
                ),
                None,
                Some("com.example.notes"),
                "the note",
            ),
            (
                "website",
                target(
                    ContextFamily::General,
                    None,
                    None,
                    Some("notes.example.net"),
                    None,
                ),
                Some("notes.example.net"),
                None,
                "the website",
            ),
        ];

        for (id, target, browser_host, native_bundle, after) in &cases {
            let mapping = mapping_for_target(id, target);
            let scope = LearnPairScope {
                family: Some(crate::context::family_id(mapping.family).into()),
                mapping_id: Some(mapping.id.clone()),
                browser_host: browser_host.map(str::to_owned),
                native_bundle: native_bundle.map(str::to_owned),
            };
            add_promoted_pair(&source_root, "teh", after, &scope);
            source_settings.context_mappings.push(mapping);
        }
        add_promoted_pair(
            &source_root,
            "directalias",
            "the native note",
            &LearnPairScope {
                family: Some(crate::context::family_id(ContextFamily::General).into()),
                mapping_id: None,
                browser_host: None,
                native_bundle: Some("com.example.direct-notes".into()),
            },
        );
        add_promoted_pair(
            &source_root,
            "directalias",
            "the direct website",
            &LearnPairScope {
                family: Some(crate::context::family_id(ContextFamily::General).into()),
                mapping_id: None,
                browser_host: Some("direct.example.net".into()),
                native_bundle: None,
            },
        );

        let source_pairs = store::list_learn_pairs(&source_root).unwrap();
        let (file, skipped) = build_export_file(
            &source_settings,
            &source_settings.context_mappings,
            &source_pairs,
        );
        assert_eq!(skipped, 0);
        assert_eq!(file.word_pairs.len(), cases.len() + 2);
        validate_transfer_file(&mut file.clone()).unwrap();

        let mut results = Vec::new();
        for reverse_pairs in [false, true] {
            let destination_root = new_transfer_test_root(if reverse_pairs {
                "composed-destination-reversed"
            } else {
                "composed-destination-forward"
            });
            let (preview, applied, settings, pairs) =
                apply_transfer_file(file.clone(), &destination_root, reverse_pairs);
            assert_eq!(preview.items.len(), cases.len() + 2);
            assert!(preview.items.iter().all(|item| {
                item.kind == TransferItemKind::WordPair && item.status == TransferItemStatus::Ready
            }));
            assert_eq!(
                preview
                    .items
                    .iter()
                    .filter(|item| item.will_create_mapping)
                    .count(),
                cases.len()
            );
            assert_eq!(applied.status, "applied");
            assert!(applied
                .results
                .iter()
                .all(|result| matches!(result.status, TransferItemResultStatus::Imported)));
            assert_eq!(settings.context_mappings.len(), cases.len());
            assert_eq!(pairs.len(), cases.len() + 2);

            for (_, target, browser_host, native_bundle, after) in &cases {
                let row = pairs
                    .iter()
                    .find(|row| row.before_surface == "teh" && row.after_surface == *after)
                    .unwrap();
                let mapping_id = row.mapping_id.as_deref().unwrap();
                let mapping = settings
                    .context_mappings
                    .iter()
                    .find(|mapping| mapping.id == mapping_id)
                    .unwrap();
                assert_eq!(mapping.transfer_selector().unwrap(), target.selector);
                assert_eq!(row.browser_host, browser_host.map(str::to_owned));
                assert_eq!(row.native_bundle, native_bundle.map(str::to_owned));
                assert_eq!(
                    row.family.as_deref(),
                    Some(crate::context::family_id(mapping.family))
                );
                assert!(!mapping.dictionary_learn_enabled);
                assert!(!mapping.source_permissions.ax_text);
                assert!(!mapping.source_permissions.local_ocr);
                assert!(!mapping.source_permissions.cloud_vision);
                assert!(!mapping.source_permissions.context_text_to_providers);
                let active_scope = LearnPairScope {
                    family: Some(crate::context::family_id(mapping.family).into()),
                    mapping_id: Some(mapping.id.clone()),
                    browser_host: browser_host.map(str::to_owned),
                    native_bundle: native_bundle.map(str::to_owned),
                };
                let applicable = active_scope.applicable_promoted_records(&pairs);
                assert_eq!(applicable.len(), 1);
                assert_eq!(applicable[0].pair_key, row.pair_key);
            }
            for (after, browser_host, native_bundle) in [
                ("the native note", None, Some("com.example.direct-notes")),
                ("the direct website", Some("direct.example.net"), None),
            ] {
                let row = pairs.iter().find(|row| row.after_surface == after).unwrap();
                assert!(row.mapping_id.is_none());
                let scope = LearnPairScope {
                    family: Some(crate::context::family_id(ContextFamily::General).into()),
                    mapping_id: None,
                    browser_host: browser_host.map(str::to_owned),
                    native_bundle: native_bundle.map(str::to_owned),
                };
                let applicable = scope.applicable_promoted_records(&pairs);
                assert_eq!(applicable.len(), 1);
                assert_eq!(applicable[0].pair_key, row.pair_key);
            }
            let mut owners = BTreeSet::new();
            for row in &pairs {
                let owner = if let Some(mapping_id) = row.mapping_id.as_deref() {
                    let mapping = settings
                        .context_mappings
                        .iter()
                        .find(|mapping| mapping.id == mapping_id)
                        .unwrap();
                    serde_json::to_string(&mapping.transfer_selector().unwrap()).unwrap()
                } else {
                    serde_json::to_string(&LearnPairScope {
                        family: row.family.clone(),
                        mapping_id: None,
                        browser_host: row.browser_host.clone(),
                        native_bundle: row.native_bundle.clone(),
                    })
                    .unwrap()
                };
                owners.insert((row.after_surface.clone(), owner));
            }
            results.push(owners);
            let _ = fs::remove_dir_all(destination_root);
        }
        assert_eq!(results[0], results[1]);
        let _ = fs::remove_dir_all(source_root);
    }

    #[test]
    fn active_source_and_destination_specificity_ties_are_blocked() {
        let _preview_guard = PREVIEW_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let source_root = new_transfer_test_root("source-tie");
        let mut owner = app_mapping("z-owner", ContextFamily::General, "com.example.editor");
        owner.writing_task_id = Some(WritingTaskId::Email);
        owner.focused_field = Some(FocusKind::Email);
        owner.writing_preferences.push(WritingPreference {
            task_id: Some(WritingTaskId::Email),
            dimension: WritingPreferenceDimension::ListStructure,
            value: WritingPreferenceValue::Bullets,
            status: WritingPreferenceStatus::Active,
            version: 1,
            source: WritingPreferenceSource::Manual,
        });
        let mut tied_owner =
            app_mapping("a-tied-owner", ContextFamily::Email, "com.example.editor");
        tied_owner.focused_field = Some(FocusKind::Email);
        let source_row = LearnPairRecord {
            pair_key: "source-tie-row".into(),
            before_surface: "teh".into(),
            after_surface: "the".into(),
            hits: 1,
            promoted: true,
            last_at: "now".into(),
            family: Some(crate::context::family_id(ContextFamily::General).into()),
            mapping_id: Some(owner.id.clone()),
            browser_host: None,
            native_bundle: Some("com.example.editor".into()),
            last_used_at: None,
            pinned: false,
            tombstoned_at: None,
            ignored: false,
            promote_hits: 0,
            manual_scoped: false,
            auto_scoped: false,
            match_policy: store::LearnPairMatchPolicy::IgnoreCase,
            binding_invalidated_at: None,
            binding_invalidated_reason: None,
            replacement_runs: 0,
        };
        let mappings = vec![owner.clone(), tied_owner.clone()];
        assert!(transfer_pair_from_record(&source_row, &mappings).is_err());
        let source_settings = Settings {
            context_mappings: mappings.clone(),
            ..Settings::default()
        };
        let (exported, skipped) = build_export_file(&source_settings, &mappings, &[source_row]);
        assert_eq!(skipped, 2, "one pair and one active preference are skipped");
        assert!(exported.word_pairs.is_empty());
        assert!(exported.writing_preferences.is_empty());

        let exact_target = TransferTargetV1 {
            selector: owner.transfer_selector().unwrap(),
            selected_task: owner.writing_task_id,
        };
        let file = file_with_pair(pair(
            "recieve",
            "receive",
            Some(TransferTargetV1 {
                selected_task: None,
                ..exact_target.clone()
            }),
            Some(TransferPairScopeV1 {
                family: Some(ContextFamily::General),
                browser_host: None,
                native_bundle: Some("com.example.editor".into()),
            }),
        ));
        let destination_root = new_transfer_test_root("destination-tie");
        let path = destination_root.join("tie.json");
        write_export_file(&path, &make_transfer_file_bytes(&file).unwrap()).unwrap();
        let loaded = load_transfer_file(&path).unwrap();
        let pairs = store::list_learn_pairs(&destination_root).unwrap();
        let preview = build_preview(
            path,
            loaded,
            &Settings {
                context_mappings: mappings,
                ..Settings::default()
            },
            &pairs,
        )
        .unwrap();
        assert_eq!(preview.items.len(), 1);
        assert_eq!(preview.items[0].status, TransferItemStatus::Blocked);
        assert_eq!(
            preview.items[0].reason.as_deref(),
            Some("mapping_selector_ambiguous")
        );
        let _ = fs::remove_dir_all(source_root);
        let _ = fs::remove_dir_all(destination_root);
    }

    #[test]
    fn export_keeps_valid_explicit_pair_policy_and_skips_invalid_items_individually() {
        let mapping = app_mapping("owner", ContextFamily::General, "com.example.editor");
        let mut settings_json = serde_json::to_value(Settings::default()).unwrap();
        settings_json["dictionary"] = serde_json::json!(["ValidTerm", " InvalidTerm "]);
        settings_json["dictionary_sources"]["owners"] = serde_json::json!({
            "ValidTerm": [{"kind": "manual"}],
            " InvalidTerm ": [{"kind": "manual"}]
        });
        let mut settings: Settings = serde_json::from_value(settings_json).unwrap();
        settings.context_mappings.push(mapping.clone());

        let make_row =
            |pair_key: &str, before: &str, after: &str, native_bundle: Option<&str>, policy| {
                LearnPairRecord {
                    pair_key: pair_key.into(),
                    before_surface: before.into(),
                    after_surface: after.into(),
                    hits: 1,
                    promoted: true,
                    last_at: "now".into(),
                    family: Some(crate::context::family_id(ContextFamily::General).into()),
                    mapping_id: Some(mapping.id.clone()),
                    browser_host: None,
                    native_bundle: native_bundle.map(str::to_owned),
                    last_used_at: None,
                    pinned: false,
                    tombstoned_at: None,
                    ignored: false,
                    promote_hits: 0,
                    manual_scoped: false,
                    auto_scoped: false,
                    match_policy: policy,
                    binding_invalidated_at: None,
                    binding_invalidated_reason: None,
                    replacement_runs: 0,
                }
            };
        let rows = vec![
            make_row(
                "valid-exact",
                "Teh",
                "the",
                Some("com.example.editor"),
                store::LearnPairMatchPolicy::Exact,
            ),
            make_row(
                "invalid-word",
                " bad ",
                "good",
                Some("com.example.editor"),
                store::LearnPairMatchPolicy::IgnoreCase,
            ),
            make_row(
                "missing-observed-bundle",
                "recieve",
                "receive",
                None,
                store::LearnPairMatchPolicy::IgnoreCase,
            ),
        ];

        let (file, skipped) = build_export_file(&settings, &[mapping], &rows);
        assert_eq!(
            skipped, 3,
            "one invalid global word and two invalid pairs are skipped"
        );
        assert_eq!(file.global_words, ["ValidTerm"]);
        assert_eq!(file.word_pairs.len(), 1);
        assert_eq!(file.word_pairs[0].before, "Teh");
        assert_eq!(
            file.word_pairs[0].match_policy,
            store::LearnPairMatchPolicy::Exact
        );
        validate_transfer_file(&mut file.clone()).unwrap();
    }

    #[test]
    fn transfer_wire_rejects_malformed_scopes_and_task_assignments() {
        let mapped_target = TransferTargetV1 {
            selector: selector(),
            selected_task: None,
        };
        let mapped_scope = Some(TransferPairScopeV1 {
            family: Some(ContextFamily::General),
            browser_host: None,
            native_bundle: Some("com.example.editor".into()),
        });
        let mut malformed_pairs = Vec::new();

        let mut row = pair(
            "teh",
            "the",
            Some(mapped_target.clone()),
            mapped_scope.clone(),
        );
        row.pair_scope.as_mut().unwrap().family = None;
        malformed_pairs.push(("mapping family is missing", row));

        let mut row = pair(
            "teh",
            "the",
            Some(mapped_target.clone()),
            mapped_scope.clone(),
        );
        row.pair_scope.as_mut().unwrap().family = Some(ContextFamily::Email);
        malformed_pairs.push(("mapping family differs", row));

        let mut row = pair(
            "teh",
            "the",
            Some(mapped_target.clone()),
            mapped_scope.clone(),
        );
        row.pair_scope.as_mut().unwrap().native_bundle = Some("com.example.other".into());
        malformed_pairs.push(("observed bundle differs", row));

        let mut row = pair("teh", "the", Some(mapped_target.clone()), None);
        row.pair_scope = Some(TransferPairScopeV1 {
            family: Some(ContextFamily::General),
            browser_host: None,
            native_bundle: None,
        });
        malformed_pairs.push(("guaranteed bundle is missing", row));

        let browser_target = TransferTargetV1 {
            selector: MappingTransferSelectorV1 {
                version: 1,
                family: ContextFamily::Email,
                bundle_id: None,
                executable: None,
                browser_host: Some("mail.google.com".into()),
                browser_path_prefix: None,
                focused_field: None,
            },
            selected_task: None,
        };
        let row = pair(
            "teh",
            "the",
            Some(browser_target.clone()),
            Some(TransferPairScopeV1 {
                family: Some(ContextFamily::Email),
                browser_host: Some("other.google.com".into()),
                native_bundle: None,
            }),
        );
        malformed_pairs.push(("observed host is outside selector", row));
        let mut row = pair("teh", "the", Some(browser_target.clone()), None);
        row.pair_scope = Some(TransferPairScopeV1 {
            family: Some(ContextFamily::Email),
            browser_host: None,
            native_bundle: None,
        });
        malformed_pairs.push(("guaranteed host is missing", row));

        malformed_pairs.push((
            "pair source has surrounding whitespace",
            pair(" teh", "the", None, None),
        ));
        let mut row = pair("teh", "the", None, None);
        row.pair_scope = Some(TransferPairScopeV1 {
            family: Some(ContextFamily::General),
            browser_host: None,
            native_bundle: None,
        });
        malformed_pairs.push(("direct pair only has family", row));
        let mut row = pair(
            "teh",
            "the",
            Some(mapped_target.clone()),
            mapped_scope.clone(),
        );
        row.pair_scope.as_mut().unwrap().native_bundle = Some(" com.example.editor ".into());
        malformed_pairs.push(("observed bundle has surrounding whitespace", row));
        let mut whitespace_target = mapped_target.clone();
        whitespace_target.selector.bundle_id = Some(" com.example.editor ".into());
        malformed_pairs.push((
            "selector has surrounding whitespace",
            pair("teh", "the", Some(whitespace_target), mapped_scope.clone()),
        ));

        for (case, malformed) in malformed_pairs {
            assert!(
                validate_transfer_file(&mut file_with_pair(malformed)).is_err(),
                "malformed transfer case should reject: {case}"
            );
        }

        let preference =
            |target: TransferTargetV1,
             task_id: WritingTaskId,
             dimension: WritingPreferenceDimension,
             value: WritingPreferenceValue| TransferWritingPreferenceV1 {
                target,
                task_id,
                dimension,
                value,
                version: 1,
                source: WritingPreferenceSource::Manual,
            };
        let no_disambiguator = preference(
            TransferTargetV1 {
                selected_task: Some(WritingTaskId::Email),
                ..mapped_target.clone()
            },
            WritingTaskId::Email,
            WritingPreferenceDimension::ListStructure,
            WritingPreferenceValue::Bullets,
        );
        assert!(
            validate_transfer_file(&mut file_with_preferences(vec![no_disambiguator])).is_err()
        );

        for field in [FocusKind::Secure, FocusKind::Search] {
            let mut invalid_task = TransferTargetV1 {
                selected_task: Some(WritingTaskId::Email),
                ..browser_target.clone()
            };
            invalid_task.selector.browser_path_prefix = Some("/mail".into());
            invalid_task.selector.focused_field = Some(field);
            let invalid_preference = preference(
                invalid_task,
                WritingTaskId::Email,
                WritingPreferenceDimension::ListStructure,
                WritingPreferenceValue::Bullets,
            );
            assert!(
                validate_transfer_file(&mut file_with_preferences(vec![invalid_preference]))
                    .is_err()
            );
        }

        let task_selector = MappingTransferSelectorV1 {
            version: 1,
            family: ContextFamily::Email,
            bundle_id: None,
            executable: None,
            browser_host: Some("mail.google.com".into()),
            browser_path_prefix: Some("/mail".into()),
            focused_field: None,
        };
        let null_task = preference(
            TransferTargetV1 {
                selector: task_selector.clone(),
                selected_task: None,
            },
            WritingTaskId::Email,
            WritingPreferenceDimension::ListStructure,
            WritingPreferenceValue::Bullets,
        );
        assert!(validate_transfer_file(&mut file_with_preferences(vec![null_task])).is_ok());

        let contradictory_tasks = vec![
            preference(
                TransferTargetV1 {
                    selector: task_selector.clone(),
                    selected_task: None,
                },
                WritingTaskId::Email,
                WritingPreferenceDimension::ListStructure,
                WritingPreferenceValue::Bullets,
            ),
            preference(
                TransferTargetV1 {
                    selector: task_selector,
                    selected_task: Some(WritingTaskId::Email),
                },
                WritingTaskId::ProjectUpdate,
                WritingPreferenceDimension::TerminalPunctuation,
                WritingPreferenceValue::NoTerminalPeriod,
            ),
        ];
        assert!(validate_transfer_file(&mut file_with_preferences(contradictory_tasks)).is_err());
    }

    #[test]
    fn ignore_case_same_key_pair_deduplicates_case_insensitive_surfaces() {
        let _preview_guard = PREVIEW_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = new_transfer_test_root("case-insensitive-dedup");
        add_promoted_pair(&root, "teh", "the", &LearnPairScope::default());
        let mut imported = pair("Teh", "the", None, None);
        imported.match_policy = store::LearnPairMatchPolicy::IgnoreCase;
        let path = root.join("transfer.json");
        write_export_file(
            &path,
            &make_transfer_file_bytes(&file_with_pair(imported)).unwrap(),
        )
        .unwrap();
        let loaded = load_transfer_file(&path).unwrap();
        let pairs = store::list_learn_pairs(&root).unwrap();
        let preview = build_preview(path, loaded, &Settings::default(), &pairs).unwrap();
        assert_eq!(preview.items.len(), 1);
        assert_eq!(preview.items[0].status, TransferItemStatus::Conflict);
        assert_eq!(preview.items[0].default_action, TransferAction::KeepLocal);
        assert_eq!(
            preview.items[0].reason.as_deref(),
            Some("import_requires_local_only_pair")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn source_pair_fully_shadowed_by_enabled_host_mapping_is_not_exported() {
        let owner = app_mapping(
            "generic-chrome",
            ContextFamily::General,
            "com.google.Chrome",
        );
        let mut gmail = app_mapping("gmail", ContextFamily::Email, "placeholder");
        gmail.bundle_id = None;
        gmail.browser_host = Some("mail.google.com".into());
        let row = LearnPairRecord {
            pair_key: "generic-chrome-gmail-scope".into(),
            before_surface: "teh".into(),
            after_surface: "the".into(),
            hits: 1,
            promoted: true,
            last_at: "now".into(),
            family: Some(crate::context::family_id(owner.family).into()),
            mapping_id: Some(owner.id.clone()),
            browser_host: Some("mail.google.com".into()),
            native_bundle: None,
            last_used_at: None,
            pinned: false,
            tombstoned_at: None,
            ignored: false,
            promote_hits: 0,
            manual_scoped: false,
            auto_scoped: false,
            match_policy: store::LearnPairMatchPolicy::IgnoreCase,
            binding_invalidated_at: None,
            binding_invalidated_reason: None,
            replacement_runs: 0,
        };
        assert!(transfer_pair_from_record(&row, &[owner.clone(), gmail]).is_err());

        let mut nested = app_mapping(
            "docs-internal",
            ContextFamily::BrowserSearch,
            "com.google.Chrome",
        );
        nested.browser_host = Some("docs.example.com".into());
        nested.browser_path_prefix = Some("/docs/internal".into());
        let mut partial_scope_row = row;
        partial_scope_row.pair_key = "generic-chrome-docs-scope".into();
        partial_scope_row.browser_host = Some("docs.example.com".into());
        assert!(transfer_pair_from_record(&partial_scope_row, &[owner, nested]).is_ok());

        let mut executable_owner =
            app_mapping("editor-executable", ContextFamily::General, "placeholder");
        executable_owner.bundle_id = None;
        executable_owner.executable = Some("Editor".into());
        let bundle_owner = app_mapping(
            "editor-bundle",
            ContextFamily::General,
            "com.example.editor",
        );
        let native_scoped_row = LearnPairRecord {
            pair_key: "editor-executable-native-scope".into(),
            before_surface: "teh".into(),
            after_surface: "the".into(),
            hits: 1,
            promoted: true,
            last_at: "now".into(),
            family: Some(crate::context::family_id(ContextFamily::General).into()),
            mapping_id: Some(executable_owner.id.clone()),
            browser_host: None,
            native_bundle: Some("com.example.editor".into()),
            last_used_at: None,
            pinned: false,
            tombstoned_at: None,
            ignored: false,
            promote_hits: 0,
            manual_scoped: false,
            auto_scoped: false,
            match_policy: store::LearnPairMatchPolicy::IgnoreCase,
            binding_invalidated_at: None,
            binding_invalidated_reason: None,
            replacement_runs: 0,
        };
        assert!(
            transfer_pair_from_record(&native_scoped_row, &[executable_owner, bundle_owner])
                .is_err()
        );
    }

    #[test]
    fn blocked_file_item_does_not_reserve_a_tied_selector() {
        let _preview_guard = PREVIEW_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let target_for = |family, bundle_id: Option<&str>| TransferTargetV1 {
            selector: MappingTransferSelectorV1 {
                version: 1,
                family,
                bundle_id: bundle_id.map(str::to_owned),
                executable: None,
                browser_host: None,
                browser_path_prefix: None,
                focused_field: None,
            },
            selected_task: None,
        };
        let bundle_target = target_for(ContextFamily::General, Some("com.google.Chrome"));
        let tied_target = target_for(ContextFamily::Email, Some("com.google.Chrome"));
        let pair_scope = Some(TransferPairScopeV1 {
            family: Some(ContextFamily::General),
            browser_host: None,
            native_bundle: Some("com.google.Chrome".into()),
        });
        let blocked = pair(
            "teh",
            "they",
            Some(bundle_target.clone()),
            pair_scope.clone(),
        );
        let viable = pair(
            "recieve",
            "receive",
            Some(tied_target),
            Some(TransferPairScopeV1 {
                family: Some(ContextFamily::Email),
                browser_host: None,
                native_bundle: Some("com.google.Chrome".into()),
            }),
        );
        let mut file = file_with_pair(blocked);
        file.word_pairs.push(viable);
        validate_transfer_file(&mut file).unwrap();

        let mut previews = Vec::new();
        for reverse_pairs in [false, true] {
            let destination_root = new_transfer_test_root(if reverse_pairs {
                "ghost-reversed"
            } else {
                "ghost-forward"
            });
            add_promoted_pair(&destination_root, "teh", "the", &LearnPairScope::default());
            let (preview, applied, settings, rows) =
                apply_transfer_file(file.clone(), &destination_root, reverse_pairs);
            let by_before = preview
                .items
                .iter()
                .map(|item| {
                    (
                        item.details.as_ref().unwrap().before.clone().unwrap(),
                        (item.status, item.reason.clone()),
                    )
                })
                .collect::<HashMap<_, _>>();
            assert_eq!(by_before["teh"].0, TransferItemStatus::Blocked);
            assert_eq!(by_before["recieve"].0, TransferItemStatus::Ready);
            assert_eq!(applied.status, "applied");
            assert_eq!(settings.context_mappings.len(), 1);
            assert_eq!(settings.context_mappings[0].family, ContextFamily::Email);
            assert_eq!(
                settings.context_mappings[0].bundle_id.as_deref(),
                Some("com.google.Chrome")
            );
            assert_eq!(
                rows.len(),
                2,
                "the local blocker and only viable imported rule remain"
            );
            previews.push(by_before);
            let _ = fs::remove_dir_all(destination_root);
        }
        assert_eq!(previews[0], previews[1]);
    }

    #[test]
    fn imported_mapping_has_no_observation_or_dictionary_learning_grants() {
        let mapping = make_new_mapping(
            &TransferTargetV1 {
                selector: selector(),
                selected_task: None,
            },
            &[],
        )
        .unwrap();
        assert!(!mapping.dictionary_learn_enabled);
        assert!(!mapping.preference_guidance_provider_approved);
        assert!(!mapping.style_examples_approved);
        assert!(!mapping.source_permissions.ax_text);
        assert!(!mapping.source_permissions.local_ocr);
        assert!(!mapping.source_permissions.cloud_vision);
    }

    #[test]
    fn preview_uses_captured_pair_snapshot_without_creating_missing_store() {
        let _preview_guard = PREVIEW_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = new_transfer_test_root("preview-missing-store");
        let path = root.join("learning-transfer.json");
        let file = file_with_pair(pair("teh", "the", None, None));
        write_export_file(&path, &make_transfer_file_bytes(&file).unwrap()).unwrap();

        let preview = build_preview(
            path,
            load_transfer_file(&root.join("learning-transfer.json")).unwrap(),
            &Settings::default(),
            &[],
        )
        .unwrap();

        assert_eq!(preview.items.len(), 1);
        assert_eq!(preview.items[0].status, TransferItemStatus::Ready);
        assert!(
            !root.join("history.sqlite").exists(),
            "preview must use its existing pair snapshot without opening the database"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn exact_disabled_destination_mapping_blocks_import_without_reenabling_it() {
        let _preview_guard = PREVIEW_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = new_transfer_test_root("preview-disabled-destination");
        let path = root.join("learning-transfer.json");
        let mut mapping = app_mapping(
            "disabled-local",
            ContextFamily::General,
            "com.example.editor",
        );
        mapping.enabled = false;
        let file = file_with_pair(pair(
            "teh",
            "the",
            Some(TransferTargetV1 {
                selector: mapping.transfer_selector().unwrap(),
                selected_task: None,
            }),
            Some(TransferPairScopeV1 {
                family: Some(mapping.family),
                browser_host: None,
                native_bundle: mapping.bundle_id.clone(),
            }),
        ));
        write_export_file(&path, &make_transfer_file_bytes(&file).unwrap()).unwrap();
        let settings = Settings {
            context_mappings: vec![mapping],
            ..Settings::default()
        };

        let preview = build_preview(
            path.clone(),
            load_transfer_file(&path).unwrap(),
            &settings,
            &[],
        )
        .unwrap();

        assert_eq!(preview.items.len(), 1);
        assert_eq!(preview.items[0].status, TransferItemStatus::Blocked);
        assert_eq!(preview.items[0].reason.as_deref(), Some("mapping_disabled"));
        assert!(!preview.items[0].will_create_mapping);
        assert!(!settings.context_mappings[0].enabled);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stale_already_present_pair_aborts_before_ready_settings_import() {
        let _preview_guard = PREVIEW_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = new_transfer_test_root("stale-already-present");
        let mapping = app_mapping("local", ContextFamily::General, "com.example.editor");
        let mapping_scope = LearnPairScope {
            family: Some(crate::context::family_id(mapping.family).into()),
            mapping_id: Some(mapping.id.clone()),
            native_bundle: mapping.bundle_id.clone(),
            ..LearnPairScope::default()
        };
        let before = "teh";
        let after = "the";
        let match_policy = store::word_rule_match_policy(before, after);
        let pair_key =
            store::scoped_word_rule_key(before, after, &mapping_scope, match_policy).into_string();
        add_promoted_pair(&root, before, after, &mapping_scope);

        let target = TransferTargetV1 {
            selector: mapping.transfer_selector().unwrap(),
            selected_task: None,
        };
        let mut imported_pair = pair(
            before,
            after,
            Some(target),
            Some(TransferPairScopeV1 {
                family: Some(mapping.family),
                browser_host: None,
                native_bundle: mapping.bundle_id.clone(),
            }),
        );
        imported_pair.match_policy = match_policy;
        let mut file = file_with_pair(imported_pair);
        file.global_words.push("VoiceFlowTransferTerm".into());
        let path = root.join("learning-transfer.json");
        write_export_file(&path, &make_transfer_file_bytes(&file).unwrap()).unwrap();
        let loaded = load_transfer_file(&path).unwrap();
        let settings = Settings {
            context_mappings: vec![mapping],
            ..Settings::default()
        };
        let pairs = store::list_learn_pairs(&root).unwrap();
        let preview = build_preview(path, loaded, &settings, &pairs).unwrap();
        assert!(preview.items.iter().any(|item| {
            item.kind == TransferItemKind::WordPair
                && item.status == TransferItemStatus::AlreadyPresent
        }));
        assert!(preview.items.iter().any(|item| {
            item.kind == TransferItemKind::GlobalWord && item.status == TransferItemStatus::Ready
        }));

        assert!(store::tombstone_learn_pair(&root, &pair_key).unwrap());
        let changed_pairs = store::list_learn_pairs(&root).unwrap();
        let error = prepare_apply(
            LearningTransferApplyInput {
                preview_id: preview.preview_id,
                choices: Vec::new(),
            },
            &settings,
            &changed_pairs,
            &root,
        )
        .unwrap_err();

        assert!(error.contains("learning_transfer_stale_preview: local word rules changed"));
        assert!(settings.dictionary.is_empty());
        assert!(!settings
            .has_dictionary_source("VoiceFlowTransferTerm", &store::DictionarySource::Manual));
        assert!(
            !root.join("settings.json").exists(),
            "a stale preflight must fail before the command can persist Settings"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn two_local_roots_export_preview_apply_and_reload_approved_learning() {
        let _preview_guard = PREVIEW_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        fn new_root(label: &str) -> PathBuf {
            let path = std::env::temp_dir().join(format!(
                "voiceflow-learning-transfer-{label}-{}-{}",
                std::process::id(),
                next_preview_id()
            ));
            fs::create_dir_all(&path).unwrap();
            path
        }

        let source_root = new_root("source");
        let destination_root = new_root("destination");
        let mut source_settings = Settings::default();
        source_settings.add_dictionary_source("VoiceFlowTerm", store::DictionarySource::Manual);
        let mut source_mapping =
            app_mapping("source-mapping", ContextFamily::Email, "com.example.mail");
        source_mapping.writing_task_id = Some(WritingTaskId::Email);
        source_mapping.focused_field = Some(FocusKind::Email);
        source_mapping.writing_preferences.push(WritingPreference {
            task_id: Some(WritingTaskId::Email),
            dimension: WritingPreferenceDimension::ListStructure,
            value: WritingPreferenceValue::Bullets,
            status: WritingPreferenceStatus::Active,
            version: 1,
            source: WritingPreferenceSource::Manual,
        });
        source_settings
            .context_mappings
            .push(source_mapping.clone());

        let source_scope = LearnPairScope {
            family: Some(crate::context::family_id(ContextFamily::Email).into()),
            mapping_id: Some(source_mapping.id.clone()),
            browser_host: None,
            native_bundle: Some("com.example.mail".into()),
        };
        let before = "teh";
        let after = "the";
        let policy = store::word_rule_match_policy(before, after);
        let source_key = store::scoped_word_rule_key(before, after, &source_scope, policy);
        let _ = store::upsert_learn_pair_with_scope(
            &source_root,
            source_key.as_str(),
            before,
            after,
            Some(&source_scope),
        )
        .unwrap();
        store::ensure_learn_pair_promoted(
            &source_root,
            source_key.as_str(),
            before,
            after,
            Some(&source_scope),
        )
        .unwrap();

        let source_pairs = store::list_learn_pairs(&source_root).unwrap();
        let (export_file, skipped) = build_export_file(
            &source_settings,
            &source_settings.context_mappings,
            &source_pairs,
        );
        assert_eq!(skipped, 0);
        assert_eq!(export_file.global_words, ["VoiceFlowTerm"]);
        assert_eq!(export_file.word_pairs.len(), 1);
        assert_eq!(export_file.writing_preferences.len(), 1);

        let source_path = source_root.join("learning-transfer.json");
        let destination_path = destination_root.join("learning-transfer.json");
        let bytes = make_transfer_file_bytes(&export_file).unwrap();
        write_export_file(&source_path, &bytes).unwrap();
        fs::copy(&source_path, &destination_path).unwrap();
        let loaded = load_transfer_file(&destination_path).unwrap();

        let destination_settings = Settings::default();
        let destination_pairs = store::list_learn_pairs(&destination_root).unwrap();
        let preview = build_preview(
            destination_path,
            loaded,
            &destination_settings,
            &destination_pairs,
        )
        .unwrap();
        assert_eq!(preview.status, "preview");
        assert!(preview
            .items
            .iter()
            .all(|item| item.status == TransferItemStatus::Ready));
        assert!(preview
            .items
            .iter()
            .all(|item| item.will_create_mapping || item.kind == TransferItemKind::GlobalWord));

        let prepared = prepare_apply(
            LearningTransferApplyInput {
                preview_id: preview.preview_id,
                choices: Vec::new(),
            },
            &destination_settings,
            &destination_pairs,
            &destination_root,
        )
        .unwrap();
        assert!(prepared.settings_changed);
        store::save_configuration(&destination_root, &prepared.next_settings).unwrap();
        let outcomes =
            store::apply_learn_pair_transfers(&destination_root, &prepared.pair_intents).unwrap();
        let applied = finish_apply(prepared, &outcomes).unwrap();
        assert_eq!(applied.status, "applied");
        assert!(applied
            .results
            .iter()
            .all(|result| matches!(result.status, TransferItemResultStatus::Imported)));

        let (reloaded_settings, _) = store::load_settings(&destination_root);
        assert!(reloaded_settings
            .has_dictionary_source("VoiceFlowTerm", &store::DictionarySource::Manual));
        let imported_mapping = reloaded_settings
            .context_mappings
            .iter()
            .find(|mapping| mapping.id.starts_with("imported-"))
            .unwrap();
        assert_eq!(imported_mapping.writing_task_id, Some(WritingTaskId::Email));
        assert!(!imported_mapping.dictionary_learn_enabled);
        assert!(!imported_mapping.preference_guidance_provider_approved);
        assert_eq!(
            local_preference_for(
                imported_mapping,
                WritingTaskId::Email,
                WritingPreferenceDimension::ListStructure
            )
            .unwrap()
            .value,
            WritingPreferenceValue::Bullets
        );
        let imported_pair = store::list_learn_pairs(&destination_root)
            .unwrap()
            .into_iter()
            .find(|row| row.before_surface == before && row.after_surface == after)
            .unwrap();
        assert!(!imported_pair.manual_scoped);
        assert!(imported_pair.auto_scoped);
        assert_eq!(
            imported_pair.mapping_id.as_deref(),
            Some(imported_mapping.id.as_str())
        );

        let _ = fs::remove_dir_all(source_root);
        let _ = fs::remove_dir_all(destination_root);
    }

    #[test]
    fn same_scope_policy_siblings_import_in_either_order_and_repreview_idempotently() {
        let _preview_guard = PREVIEW_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let pair_scope = TransferPairScopeV1 {
            family: Some(ContextFamily::General),
            browser_host: None,
            native_bundle: Some("com.example.notes".into()),
        };
        let pair = |before: &str, match_policy| TransferWordPairV1 {
            target: None,
            pair_scope: Some(pair_scope.clone()),
            before: before.into(),
            after: "the".into(),
            match_policy,
            manual_scoped: false,
        };

        for reverse in [false, true] {
            let destination_root = new_transfer_test_root(if reverse {
                "mixed-policy-reverse"
            } else {
                "mixed-policy-forward"
            });
            let path = destination_root.join("learning-transfer.json");
            let mut file = LearningTransferFileV1 {
                format: TRANSFER_FORMAT.into(),
                schema_version: 1,
                global_words: Vec::new(),
                word_pairs: vec![
                    pair("Teh", store::LearnPairMatchPolicy::Exact),
                    pair("teh", store::LearnPairMatchPolicy::IgnoreCase),
                ],
                writing_preferences: Vec::new(),
            };
            if reverse {
                file.word_pairs.reverse();
            }
            write_export_file(&path, &make_transfer_file_bytes(&file).unwrap()).unwrap();

            let settings = Settings::default();
            let pairs = store::list_learn_pairs(&destination_root).unwrap();
            let preview = build_preview(
                path.clone(),
                load_transfer_file(&path).unwrap(),
                &settings,
                &pairs,
            )
            .unwrap();
            assert!(preview
                .items
                .iter()
                .all(|item| item.status == TransferItemStatus::Ready));
            let prepared = prepare_apply(
                LearningTransferApplyInput {
                    preview_id: preview.preview_id,
                    choices: Vec::new(),
                },
                &settings,
                &pairs,
                &destination_root,
            )
            .unwrap();
            let outcomes =
                store::apply_learn_pair_transfers(&destination_root, &prepared.pair_intents)
                    .unwrap();
            assert_eq!(
                outcomes,
                [
                    store::LearnPairTransferOutcome::Inserted,
                    store::LearnPairTransferOutcome::Inserted,
                ]
            );
            let applied = finish_apply(prepared, &outcomes).unwrap();
            assert!(applied
                .results
                .iter()
                .all(|result| matches!(result.status, TransferItemResultStatus::Imported)));

            let imported_rows = store::list_learn_pairs(&destination_root).unwrap();
            assert_eq!(imported_rows.len(), 2);
            assert!(imported_rows.iter().any(|row| {
                row.before_surface == "Teh"
                    && row.match_policy == store::LearnPairMatchPolicy::Exact
            }));
            assert!(imported_rows.iter().any(|row| {
                row.before_surface == "teh"
                    && row.match_policy == store::LearnPairMatchPolicy::IgnoreCase
            }));

            let repeated = build_preview(
                path.clone(),
                load_transfer_file(&path).unwrap(),
                &settings,
                &imported_rows,
            )
            .unwrap();
            assert!(repeated
                .items
                .iter()
                .all(|item| item.status == TransferItemStatus::AlreadyPresent));
            let repeated_apply = prepare_apply(
                LearningTransferApplyInput {
                    preview_id: repeated.preview_id,
                    choices: Vec::new(),
                },
                &settings,
                &imported_rows,
                &destination_root,
            )
            .unwrap();
            assert!(repeated_apply.pair_intents.is_empty());
            let repeated_result = finish_apply(repeated_apply, &[]).unwrap();
            assert!(repeated_result
                .results
                .iter()
                .all(|result| matches!(result.status, TransferItemResultStatus::AlreadyPresent)));
            let _ = fs::remove_dir_all(destination_root);
        }

        let local_root = new_transfer_test_root("mixed-policy-existing-local");
        let local_scope = LearnPairScope {
            family: Some(crate::context::family_id(ContextFamily::General).into()),
            mapping_id: None,
            browser_host: None,
            native_bundle: Some("com.example.notes".into()),
        };
        let before = "teh";
        let after = "the";
        let local_key = store::scoped_word_rule_key(
            before,
            after,
            &local_scope,
            store::LearnPairMatchPolicy::IgnoreCase,
        )
        .into_string();
        let expected_rows = store::learn_pair_transfer_conflict_snapshot(
            &[],
            &local_key,
            before,
            after,
            &local_scope,
            store::LearnPairMatchPolicy::IgnoreCase,
        )
        .unwrap();
        let local_outcomes = store::apply_learn_pair_transfers(
            &local_root,
            &[store::LearnPairTransferIntent {
                pair_key: local_key,
                before_surface: before.into(),
                after_surface: after.into(),
                scope: local_scope,
                match_policy: store::LearnPairMatchPolicy::IgnoreCase,
                manual_scoped: false,
                expected_rows,
                replace_conflicts: false,
            }],
        )
        .unwrap();
        assert_eq!(local_outcomes, [store::LearnPairTransferOutcome::Inserted]);

        let conflict_file = file_with_pair(pair("Teh", store::LearnPairMatchPolicy::Exact));
        let conflict_path = local_root.join("learning-transfer.json");
        write_export_file(
            &conflict_path,
            &make_transfer_file_bytes(&conflict_file).unwrap(),
        )
        .unwrap();
        let local_settings = Settings::default();
        let local_rows = store::list_learn_pairs(&local_root).unwrap();
        let conflict_preview = build_preview(
            conflict_path,
            load_transfer_file(&local_root.join("learning-transfer.json")).unwrap(),
            &local_settings,
            &local_rows,
        )
        .unwrap();
        assert_eq!(conflict_preview.items.len(), 1);
        assert_eq!(
            conflict_preview.items[0].status,
            TransferItemStatus::Conflict
        );
        assert_eq!(
            conflict_preview.items[0].reason.as_deref(),
            Some("pair_conflict")
        );
        let kept_local = prepare_apply(
            LearningTransferApplyInput {
                preview_id: conflict_preview.preview_id,
                choices: Vec::new(),
            },
            &local_settings,
            &local_rows,
            &local_root,
        )
        .unwrap();
        let kept_result = finish_apply(kept_local, &[]).unwrap();
        assert!(matches!(
            kept_result.results[0].status,
            TransferItemResultStatus::KeptLocal
        ));
        assert_eq!(store::list_learn_pairs(&local_root).unwrap().len(), 1);
        let _ = fs::remove_dir_all(local_root);
    }
}
