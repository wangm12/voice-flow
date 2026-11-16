//! Conservative History-owned weak evidence classification for writing
//! preferences. External AX observations never provide author or IME proof;
//! this classifier only accepts explicit edits saved through VoiceFlow's own
//! History editor and never activates a preference.

use crate::context::{
    FocusKind, HistoryPreferenceOutputKind, WritingPreferenceDimension, WritingPreferenceValue,
    WritingTaskId,
};
use crate::llm::IntentSource;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub(crate) const WINDOW_MS: u64 = 14 * 24 * 60 * 60 * 1_000;
pub(crate) const MIN_DISTINCT_ORIGINS: usize = 3;
pub(crate) const MIN_NATURAL_DAYS: usize = 2;
pub(crate) const MIN_SESSIONS: usize = 2;
pub(crate) const MAX_HISTORY_PREFERENCE_TEXT_BYTES: usize = 8_192;
const MAX_HISTORY_PREFERENCE_LIST_ITEMS: usize = 12;
const MAX_HISTORY_PREFERENCE_ITEM_BYTES: usize = 512;
const MAX_EDITOR_INPUT_KINDS: usize = 12;
pub(crate) const MAX_WEAK_HISTORY_EVIDENCE_ROWS: usize = 256;

/// Input classes emitted by the VoiceFlow-owned History textarea. The UI
/// records only these bounded categories, never the typed content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HistoryEditorInputKind {
    TrustedText,
    TrustedDelete,
    TrustedLineBreak,
    TrustedCompositionText,
    CompositionCommitted,
    Clipboard,
    Drop,
    Replacement,
    Untrusted,
    Unknown,
}

/// Per-commit evidence from the History editor. A page-level UUID is reused
/// across rows and saves so it can represent an actual editing session.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) struct HistoryEditorCommitV1 {
    pub(crate) version: u8,
    pub(crate) editor_session_id: String,
    pub(crate) input_kinds: Vec<HistoryEditorInputKind>,
    pub(crate) composing_at_save: bool,
    pub(crate) save_action_trusted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryPreferenceDecision {
    WeakEvidence {
        dimension: WritingPreferenceDimension,
        value: WritingPreferenceValue,
    },
    Excluded(HistoryPreferenceExclusion),
    Unknown(HistoryPreferenceUnknown),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryPreferenceExclusion {
    ExplicitOutputMode,
    ExplicitOneShotIntent,
    SessionCleanupBypass,
    SnippetExpansion,
    SourceRestoration,
    UnsafeEditorInput,
    UnsupportedEdit,
    ProtectedInputKind,
    KnownDeliveryFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryPreferenceUnknown {
    MissingTransaction,
    MissingTask,
    UntrustedTaskOrMapping,
    MissingHistoryPolicy,
    MissingCapturedOutputMode,
    MissingSessionCleanupMode,
    UnsupportedInputKind,
    MissingOriginalSource,
    MissingSourceAlignment,
    MissingEditorCommit,
    InvalidEditorCommit,
    UnsupportedOutputPath,
    UnsupportedTaskValue,
    AmbiguousDiff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryPreferenceOwner {
    VoiceFlowHistory,
    #[cfg(test)]
    ExternalAccessibility,
}

/// Runtime facts for a single explicit History Save. `task_id` and policy
/// fields must come from the original History transaction; this classifier
/// never borrows task metadata from current app focus.
pub(crate) struct HistoryPreferenceInput<'a> {
    pub(crate) owner: HistoryPreferenceOwner,
    pub(crate) delivery_eligible: bool,
    pub(crate) input_kind: FocusKind,
    pub(crate) transaction_id: Option<&'a str>,
    pub(crate) task_id: Option<WritingTaskId>,
    pub(crate) task_identity_trusted: bool,
    pub(crate) mapping_identity_matches: bool,
    pub(crate) mapping_revision_matches: bool,
    pub(crate) history_policy_present: bool,
    pub(crate) captured_intent_source: Option<IntentSource>,
    pub(crate) output_kind: HistoryPreferenceOutputKind,
    pub(crate) snippet_expanded: bool,
    pub(crate) output_mode: Option<&'a str>,
    /// Captured recording-level bypass. A missing value cannot distinguish
    /// legacy records from an ordinary cleanup-disabled route.
    pub(crate) session_skip_llm_cleanup: Option<bool>,
    /// History's legacy `raw_text` column stores the canonical prepared
    /// transcript. Keep that source distinct from the original ASR text.
    pub(crate) original_prepared_text: Option<&'a str>,
    pub(crate) original_asr_text: Option<&'a str>,
    pub(crate) configured_translation_target: Option<&'a str>,
    pub(crate) before_text: &'a str,
    pub(crate) after_text: &'a str,
    /// Result of a compare-only DB query against the raw/ASR/prepared/cleanup
    /// source and previous revisions. No source body is copied into drafts.
    pub(crate) matches_existing_source: Option<bool>,
    pub(crate) editor_commit: Option<&'a HistoryEditorCommitV1>,
}

/// Classifies one VoiceFlow-owned explicit edit into a finite weak-evidence
/// value. It never emits independent long-term labels or activates settings.
pub(crate) fn classify_history_preference_edit(
    input: &HistoryPreferenceInput<'_>,
) -> HistoryPreferenceDecision {
    if input.owner != HistoryPreferenceOwner::VoiceFlowHistory {
        return HistoryPreferenceDecision::Unknown(
            HistoryPreferenceUnknown::UntrustedTaskOrMapping,
        );
    }
    if !input.delivery_eligible {
        return HistoryPreferenceDecision::Excluded(
            HistoryPreferenceExclusion::KnownDeliveryFailure,
        );
    }
    match input.input_kind {
        FocusKind::Email
        | FocusKind::Chat
        | FocusKind::Document
        | FocusKind::Form
        | FocusKind::Editable => {}
        FocusKind::Secure
        | FocusKind::Search
        | FocusKind::Code
        | FocusKind::Terminal
        | FocusKind::CodingPrompt => {
            return HistoryPreferenceDecision::Excluded(
                HistoryPreferenceExclusion::ProtectedInputKind,
            );
        }
        FocusKind::Unknown => {
            return HistoryPreferenceDecision::Unknown(
                HistoryPreferenceUnknown::UnsupportedInputKind,
            );
        }
    }
    if input
        .transaction_id
        .is_none_or(|transaction_id| transaction_id.trim().is_empty())
    {
        return HistoryPreferenceDecision::Unknown(HistoryPreferenceUnknown::MissingTransaction);
    }
    if input.task_id.is_none() {
        return HistoryPreferenceDecision::Unknown(HistoryPreferenceUnknown::MissingTask);
    }
    if !input.task_identity_trusted
        || !input.mapping_identity_matches
        || !input.mapping_revision_matches
    {
        return HistoryPreferenceDecision::Unknown(
            HistoryPreferenceUnknown::UntrustedTaskOrMapping,
        );
    }
    if !input.history_policy_present {
        return HistoryPreferenceDecision::Unknown(HistoryPreferenceUnknown::MissingHistoryPolicy);
    }
    match input.output_mode {
        Some("auto") => {}
        Some(_) => {
            return HistoryPreferenceDecision::Excluded(
                HistoryPreferenceExclusion::ExplicitOutputMode,
            );
        }
        None => {
            return HistoryPreferenceDecision::Unknown(
                HistoryPreferenceUnknown::MissingCapturedOutputMode,
            );
        }
    }
    match input.session_skip_llm_cleanup {
        Some(false) => {}
        Some(true) => {
            return HistoryPreferenceDecision::Excluded(
                HistoryPreferenceExclusion::SessionCleanupBypass,
            );
        }
        None => {
            return HistoryPreferenceDecision::Unknown(
                HistoryPreferenceUnknown::MissingSessionCleanupMode,
            );
        }
    }
    if input.snippet_expanded || input.output_kind == HistoryPreferenceOutputKind::SnippetExpansion
    {
        return HistoryPreferenceDecision::Excluded(HistoryPreferenceExclusion::SnippetExpansion);
    }
    if input.captured_intent_source != Some(IntentSource::Implicit) {
        return HistoryPreferenceDecision::Excluded(
            HistoryPreferenceExclusion::ExplicitOneShotIntent,
        );
    }
    if !matches!(
        input.output_kind,
        HistoryPreferenceOutputKind::PreparedTranscript
            | HistoryPreferenceOutputKind::AcceptedCleanup
    ) {
        return HistoryPreferenceDecision::Unknown(HistoryPreferenceUnknown::UnsupportedOutputPath);
    }
    if input.before_text.len() > MAX_HISTORY_PREFERENCE_TEXT_BYTES
        || input.after_text.len() > MAX_HISTORY_PREFERENCE_TEXT_BYTES
    {
        return HistoryPreferenceDecision::Unknown(HistoryPreferenceUnknown::AmbiguousDiff);
    }
    if input.before_text.trim().is_empty() || input.after_text.trim().is_empty() {
        return HistoryPreferenceDecision::Excluded(HistoryPreferenceExclusion::UnsupportedEdit);
    }
    if input.before_text == input.after_text {
        return HistoryPreferenceDecision::Excluded(HistoryPreferenceExclusion::UnsupportedEdit);
    }
    let (Some(prepared_text), Some(asr_text)) =
        (input.original_prepared_text, input.original_asr_text)
    else {
        return HistoryPreferenceDecision::Unknown(HistoryPreferenceUnknown::MissingOriginalSource);
    };
    if prepared_text.trim().is_empty()
        || asr_text.trim().is_empty()
        || prepared_text.len() > MAX_HISTORY_PREFERENCE_TEXT_BYTES
        || asr_text.len() > MAX_HISTORY_PREFERENCE_TEXT_BYTES
    {
        return HistoryPreferenceDecision::Unknown(HistoryPreferenceUnknown::MissingOriginalSource);
    }
    let Some(matches_existing_source) = input.matches_existing_source else {
        return HistoryPreferenceDecision::Unknown(
            HistoryPreferenceUnknown::MissingSourceAlignment,
        );
    };
    if matches_existing_source {
        return HistoryPreferenceDecision::Excluded(HistoryPreferenceExclusion::SourceRestoration);
    }
    let Some(editor_commit) = input.editor_commit else {
        return HistoryPreferenceDecision::Unknown(HistoryPreferenceUnknown::MissingEditorCommit);
    };
    match validate_history_editor_commit(editor_commit) {
        EditorCommitDecision::Eligible => {}
        EditorCommitDecision::Unsafe => {
            return HistoryPreferenceDecision::Excluded(
                HistoryPreferenceExclusion::UnsafeEditorInput,
            );
        }
        EditorCommitDecision::Unknown => {
            return HistoryPreferenceDecision::Unknown(
                HistoryPreferenceUnknown::InvalidEditorCommit,
            );
        }
    }
    if crate::llm::parse_cleanup_intent(asr_text, input.configured_translation_target).source
        != IntentSource::Implicit
        || crate::llm::parse_cleanup_intent(prepared_text, input.configured_translation_target)
            .source
            != IntentSource::Implicit
    {
        return HistoryPreferenceDecision::Excluded(
            HistoryPreferenceExclusion::ExplicitOneShotIntent,
        );
    }

    let mut supported = Vec::with_capacity(3);
    if let Some(value) = list_structure_change(input.before_text, input.after_text) {
        supported.push((WritingPreferenceDimension::ListStructure, value));
    }
    if let Some(value) = greeting_tone_change(input.before_text, input.after_text) {
        supported.push((WritingPreferenceDimension::GreetingTone, value));
    }
    if let Some(value) = terminal_punctuation_change(input.before_text, input.after_text) {
        supported.push((WritingPreferenceDimension::TerminalPunctuation, value));
    }
    match supported.as_slice() {
        [(dimension, value)] => {
            let Some(task_id) = input.task_id else {
                return HistoryPreferenceDecision::Unknown(HistoryPreferenceUnknown::MissingTask);
            };
            if !supported_preference_for_task(task_id, *dimension, *value) {
                return HistoryPreferenceDecision::Unknown(
                    HistoryPreferenceUnknown::UnsupportedTaskValue,
                );
            }
            match source_slot_alignment(input, *dimension, *value) {
                SourceSlotAlignment::AlreadyPresent => {
                    return HistoryPreferenceDecision::Excluded(
                        HistoryPreferenceExclusion::SourceRestoration,
                    );
                }
                SourceSlotAlignment::AlignedWithoutTarget => {}
                SourceSlotAlignment::Unaligned | SourceSlotAlignment::Ambiguous => {
                    return HistoryPreferenceDecision::Unknown(
                        HistoryPreferenceUnknown::MissingSourceAlignment,
                    );
                }
            }
            HistoryPreferenceDecision::WeakEvidence {
                dimension: *dimension,
                value: *value,
            }
        }
        [] => HistoryPreferenceDecision::Excluded(HistoryPreferenceExclusion::UnsupportedEdit),
        _ => HistoryPreferenceDecision::Unknown(HistoryPreferenceUnknown::AmbiguousDiff),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorCommitDecision {
    Eligible,
    Unsafe,
    Unknown,
}

fn validate_history_editor_commit(commit: &HistoryEditorCommitV1) -> EditorCommitDecision {
    if commit.version != 1
        || !valid_editor_session_id(&commit.editor_session_id)
        || commit.input_kinds.is_empty()
        || commit.input_kinds.len() > MAX_EDITOR_INPUT_KINDS
    {
        return EditorCommitDecision::Unknown;
    }
    if commit.composing_at_save {
        return EditorCommitDecision::Unknown;
    }
    if !commit.save_action_trusted {
        return EditorCommitDecision::Unsafe;
    }
    if commit.input_kinds.iter().any(|kind| {
        matches!(
            kind,
            HistoryEditorInputKind::Clipboard
                | HistoryEditorInputKind::Drop
                | HistoryEditorInputKind::Replacement
                | HistoryEditorInputKind::Untrusted
        )
    }) {
        return EditorCommitDecision::Unsafe;
    }
    if commit
        .input_kinds
        .contains(&HistoryEditorInputKind::Unknown)
    {
        return EditorCommitDecision::Unknown;
    }
    let has_composition_text = commit
        .input_kinds
        .contains(&HistoryEditorInputKind::TrustedCompositionText);
    if has_composition_text
        && !commit
            .input_kinds
            .contains(&HistoryEditorInputKind::CompositionCommitted)
    {
        return EditorCommitDecision::Unknown;
    }
    let has_edit = commit.input_kinds.iter().any(|kind| {
        matches!(
            kind,
            HistoryEditorInputKind::TrustedText
                | HistoryEditorInputKind::TrustedDelete
                | HistoryEditorInputKind::TrustedLineBreak
                | HistoryEditorInputKind::TrustedCompositionText
        )
    });
    if has_edit {
        EditorCommitDecision::Eligible
    } else {
        EditorCommitDecision::Unknown
    }
}

fn valid_editor_session_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn list_structure_change(before: &str, after: &str) -> Option<WritingPreferenceValue> {
    if let (Some(before_items), Some(after_items)) =
        (plain_line_items(before), bullet_line_items(after))
    {
        if before_items == after_items {
            return Some(WritingPreferenceValue::Bullets);
        }
    }

    let projection = crate::cleanup_output_layout::project_cleanup_layout(
        before,
        crate::cleanup_output_layout::OutputLayoutMode::List,
    )
    .ok()?;
    if projection.structure != crate::cleanup_output_layout::ProjectedStructure::List {
        return None;
    }
    let (projected_lead_in, projected_items) = projected_bullet_items(&projection.text)?;
    let (edited_lead_in, edited_items) = projected_bullet_items(after)?;
    (projected_lead_in == edited_lead_in && projected_items == edited_items)
        .then_some(WritingPreferenceValue::Bullets)
}

fn projected_bullet_items(text: &str) -> Option<(Vec<&str>, Vec<&str>)> {
    if text.contains('\r') || text.contains('\t') || unsafe_preference_surface(text) {
        return None;
    }
    let mut lead_in = Vec::new();
    let mut items = Vec::new();
    let mut saw_bullet = false;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.len() > MAX_HISTORY_PREFERENCE_ITEM_BYTES {
            return None;
        }
        if let Some(body) = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .or_else(|| line.strip_prefix("• "))
        {
            saw_bullet = true;
            if body.trim().is_empty() || items.len() == MAX_HISTORY_PREFERENCE_LIST_ITEMS {
                return None;
            }
            items.push(body.trim());
        } else if saw_bullet {
            return None;
        } else {
            lead_in.push(line);
        }
    }
    (items.len() >= 2).then_some((lead_in, items))
}

fn supported_preference_for_task(
    task_id: WritingTaskId,
    dimension: WritingPreferenceDimension,
    value: WritingPreferenceValue,
) -> bool {
    use WritingPreferenceDimension as Dimension;
    use WritingPreferenceValue as Value;
    matches!(
        (task_id, dimension, value),
        (_, Dimension::ListStructure, Value::Bullets)
            | (
                WritingTaskId::Email | WritingTaskId::WorkChat | WritingTaskId::PersonalChat,
                Dimension::GreetingTone,
                Value::ConversationalGreeting | Value::RespectfulGreeting,
            )
            | (
                WritingTaskId::Email
                    | WritingTaskId::ProjectUpdate
                    | WritingTaskId::DocumentEditing,
                Dimension::TerminalPunctuation,
                Value::TerminalPeriod,
            )
            | (
                WritingTaskId::WorkChat | WritingTaskId::PersonalChat,
                Dimension::TerminalPunctuation,
                Value::NoTerminalPeriod,
            )
    )
}

fn plain_line_items(text: &str) -> Option<Vec<&str>> {
    let lines = bounded_lines(text)?;
    if lines.iter().any(|line| starts_with_bullet_marker(line)) {
        return None;
    }
    Some(lines)
}

fn bullet_line_items(text: &str) -> Option<Vec<&str>> {
    let lines = bounded_lines(text)?;
    let mut items = Vec::with_capacity(lines.len());
    for line in lines {
        let body = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .or_else(|| line.strip_prefix("• "))?;
        if body.trim().is_empty() || unsafe_preference_surface(body) {
            return None;
        }
        items.push(body.trim());
    }
    Some(items)
}

fn bounded_lines(text: &str) -> Option<Vec<&str>> {
    if text.contains('\r') || text.contains('\t') || unsafe_preference_surface(text) {
        return None;
    }
    let lines = text.lines().map(str::trim).collect::<Vec<_>>();
    (lines.len() >= 2
        && lines.len() <= MAX_HISTORY_PREFERENCE_LIST_ITEMS
        && lines
            .iter()
            .all(|line| !line.is_empty() && line.len() <= MAX_HISTORY_PREFERENCE_ITEM_BYTES))
    .then_some(lines)
}

fn starts_with_bullet_marker(line: &str) -> bool {
    line.starts_with("- ") || line.starts_with("* ") || line.starts_with("• ")
}

fn greeting_tone_change(before: &str, after: &str) -> Option<WritingPreferenceValue> {
    for (old, new, value) in [
        ("你好", "您好", WritingPreferenceValue::RespectfulGreeting),
        (
            "您好",
            "你好",
            WritingPreferenceValue::ConversationalGreeting,
        ),
    ] {
        let Some(suffix) = before.strip_prefix(old) else {
            continue;
        };
        if !valid_greeting_boundary(suffix) || !after.starts_with(new) {
            continue;
        }
        if after.strip_prefix(new) == Some(suffix)
            && !unsafe_preference_surface(before)
            && !unsafe_preference_surface(after)
        {
            return Some(value);
        }
    }
    None
}

fn valid_greeting_boundary(suffix: &str) -> bool {
    suffix.is_empty()
        || suffix.chars().next().is_some_and(|character| {
            character.is_whitespace()
                || matches!(
                    character,
                    '，' | ',' | '、' | '：' | ':' | '。' | '.' | '！' | '!'
                )
        })
}

fn terminal_punctuation_change(before: &str, after: &str) -> Option<WritingPreferenceValue> {
    if let Some(body) = before.strip_suffix('。') {
        if after == body && has_safe_single_sentence_body(body) {
            return Some(WritingPreferenceValue::NoTerminalPeriod);
        }
    }
    if let Some(body) = after.strip_suffix('。') {
        if before == body && has_safe_single_sentence_body(body) {
            return Some(WritingPreferenceValue::TerminalPeriod);
        }
    }
    None
}

fn has_safe_single_sentence_body(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty()
        || text.len() > MAX_HISTORY_PREFERENCE_TEXT_BYTES
        || !text.chars().next_back().is_some_and(|character| {
            character.is_alphanumeric() || crate::dictionary_learn::is_cjk(character)
        })
        || text.contains([
            '\n', '\r', '?', '？', '!', '！', ';', '；', ':', '：', '.', '。', '．', '｡',
        ])
        || text.starts_with(['-', '*', '+', '•'])
        || unsafe_preference_surface(text)
    {
        return false;
    }
    text.chars()
        .any(|character| character.is_alphanumeric() || crate::dictionary_learn::is_cjk(character))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceSlotAlignment {
    /// The source already had the exact target feature in the aligned slot.
    AlreadyPresent,
    /// The source aligned to the edited body but did not already have the
    /// target feature, so this edit may represent a preference.
    AlignedWithoutTarget,
    /// The source does not align to the edited feature body.
    Unaligned,
    /// The source body aligns, but its feature cannot be interpreted safely.
    Ambiguous,
}

fn source_slot_alignment(
    input: &HistoryPreferenceInput<'_>,
    dimension: WritingPreferenceDimension,
    value: WritingPreferenceValue,
) -> SourceSlotAlignment {
    let mut aligned = false;
    let mut ambiguous = false;
    for source in [input.original_prepared_text, input.original_asr_text]
        .into_iter()
        .flatten()
    {
        match source_slot_match(source, input.after_text, dimension, value) {
            SourceSlotAlignment::AlreadyPresent => return SourceSlotAlignment::AlreadyPresent,
            SourceSlotAlignment::AlignedWithoutTarget => aligned = true,
            SourceSlotAlignment::Ambiguous => ambiguous = true,
            SourceSlotAlignment::Unaligned => {}
        }
    }
    if ambiguous {
        SourceSlotAlignment::Ambiguous
    } else if aligned {
        SourceSlotAlignment::AlignedWithoutTarget
    } else {
        SourceSlotAlignment::Unaligned
    }
}

/// Compare only the requested feature slot against a candidate edit. This is
/// also used by the compare-only History DB helper for earlier revisions; it
/// never returns or persists source text.
pub(crate) fn source_slot_match(
    source: &str,
    edited: &str,
    dimension: WritingPreferenceDimension,
    value: WritingPreferenceValue,
) -> SourceSlotAlignment {
    if source.len() > MAX_HISTORY_PREFERENCE_TEXT_BYTES
        || edited.len() > MAX_HISTORY_PREFERENCE_TEXT_BYTES
        || source.trim().is_empty()
        || edited.trim().is_empty()
    {
        return SourceSlotAlignment::Unaligned;
    }
    match dimension {
        WritingPreferenceDimension::GreetingTone => {
            let (Some((source_value, source_body)), Some((edited_value, edited_body))) =
                (greeting_slot(source), greeting_slot(edited))
            else {
                return SourceSlotAlignment::Unaligned;
            };
            if edited_value != Some(value)
                || normalize_alignment_body(source_body) != normalize_alignment_body(edited_body)
            {
                return SourceSlotAlignment::Unaligned;
            }
            if source_value == Some(value) {
                SourceSlotAlignment::AlreadyPresent
            } else {
                SourceSlotAlignment::AlignedWithoutTarget
            }
        }
        WritingPreferenceDimension::TerminalPunctuation => {
            let (Some((source_value, source_body)), Some((edited_value, edited_body))) =
                (terminal_slot(source), terminal_slot(edited))
            else {
                return SourceSlotAlignment::Unaligned;
            };
            if edited_value != Some(value)
                || normalize_alignment_body(source_body) != normalize_alignment_body(edited_body)
            {
                return SourceSlotAlignment::Unaligned;
            }
            if source_value.is_none() {
                SourceSlotAlignment::Ambiguous
            } else if source_value == Some(value) {
                SourceSlotAlignment::AlreadyPresent
            } else {
                SourceSlotAlignment::AlignedWithoutTarget
            }
        }
        WritingPreferenceDimension::ListStructure => {
            let Some((_, edited_items)) = list_source_items(edited) else {
                return SourceSlotAlignment::Unaligned;
            };
            if value != WritingPreferenceValue::Bullets {
                return SourceSlotAlignment::Unaligned;
            }
            let Some((source_is_bulleted, source_items)) = list_source_items(source) else {
                return SourceSlotAlignment::Unaligned;
            };
            if !same_normalized_items(&source_items, &edited_items) {
                SourceSlotAlignment::Unaligned
            } else if source_is_bulleted {
                SourceSlotAlignment::AlreadyPresent
            } else {
                SourceSlotAlignment::AlignedWithoutTarget
            }
        }
    }
}

fn greeting_slot(text: &str) -> Option<(Option<WritingPreferenceValue>, &str)> {
    for (greeting, value) in [
        ("你好", WritingPreferenceValue::ConversationalGreeting),
        ("您好", WritingPreferenceValue::RespectfulGreeting),
    ] {
        if let Some(suffix) = text.strip_prefix(greeting) {
            if valid_greeting_boundary(suffix) {
                return Some((Some(value), suffix));
            }
        }
    }
    Some((None, text))
}

fn terminal_slot(text: &str) -> Option<(Option<WritingPreferenceValue>, &str)> {
    let text = text.trim_end();
    if let Some(body) = text.strip_suffix('。') {
        Some((Some(WritingPreferenceValue::TerminalPeriod), body))
    } else if text.ends_with('.') {
        // ASCII periods may belong to abbreviations such as "Dr." or "Inc.".
        // Keep the body for alignment but mark the feature as ambiguous.
        Some((None, text.strip_suffix('.')?))
    } else if text.chars().next_back().is_some_and(|character| {
        character.is_alphanumeric() || crate::dictionary_learn::is_cjk(character)
    }) {
        Some((Some(WritingPreferenceValue::NoTerminalPeriod), text))
    } else {
        Some((None, text))
    }
}

fn list_source_items(text: &str) -> Option<(bool, Vec<String>)> {
    if let Some((_, items)) = projected_bullet_items(text) {
        return normalized_items(items).map(|items| (true, items));
    }
    if let Some(items) = plain_line_items(text) {
        return normalized_items(items).map(|items| (false, items));
    }
    let projection = crate::cleanup_output_layout::project_cleanup_layout(
        text,
        crate::cleanup_output_layout::OutputLayoutMode::List,
    )
    .ok()?;
    if projection.structure != crate::cleanup_output_layout::ProjectedStructure::List {
        return None;
    }
    let (_, items) = projected_bullet_items(&projection.text)?;
    normalized_items(items).map(|items| (false, items))
}

fn normalized_items(items: Vec<&str>) -> Option<Vec<String>> {
    let normalized = items
        .into_iter()
        .map(normalize_alignment_body)
        .collect::<Vec<_>>();
    normalized
        .iter()
        .all(|item| !item.is_empty())
        .then_some(normalized)
}

fn same_normalized_items(left: &[String], right: &[String]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| !left.is_empty() && left == right)
}

fn normalize_alignment_body(text: &str) -> String {
    let mut normalized = String::new();
    let mut pending_space = false;
    for character in text.chars() {
        if character.is_whitespace() || is_alignment_punctuation(character) {
            pending_space = !normalized.is_empty();
            continue;
        }
        if pending_space && !normalized.ends_with(' ') {
            normalized.push(' ');
        }
        pending_space = false;
        if character.is_ascii_alphabetic() {
            normalized.push(character.to_ascii_lowercase());
        } else {
            normalized.push(character);
        }
    }
    normalized.trim().to_owned()
}

fn is_alignment_punctuation(character: char) -> bool {
    matches!(
        character,
        ',' | '，' | '、' | ':' | '：' | ';' | '；' | '.' | '。' | '．' | '｡'
    )
}

fn unsafe_preference_surface(text: &str) -> bool {
    text.contains("https://")
        || text.contains("http://")
        || text.contains("www.")
        || text.contains([
            '`', '\\', '/', '"', '“', '”', '「', '」', '『', '』', '{', '}', '[', ']', '<', '>',
        ])
}

/// A local coarse shape fingerprint groups exact and near-template text
/// conservatively. It omits lexical content, ASCII word counts, and CJK run
/// lengths. Same-script phrases within a clause become one shape token;
/// punctuation, number slots, and paragraph boundaries retain broad structure.
pub(crate) fn template_family_fingerprint(text: &str) -> Option<String> {
    if text.trim().is_empty()
        || text.len() > MAX_HISTORY_PREFERENCE_TEXT_BYTES
        || text.contains('\t')
        || unsafe_preference_surface(text)
    {
        return None;
    }
    let mut shapes = Vec::new();
    let mut in_latin_phrase = false;
    let mut in_cjk_run = false;
    let mut in_digit_run = false;
    let mut alpha_characters = 0usize;
    let terminal_boundary = text
        .trim_end()
        .trim_end_matches(is_template_terminal_punctuation)
        .len();
    let mut characters = text.char_indices().peekable();
    while let Some((byte_index, character)) = characters.next() {
        if character.is_ascii_alphabetic() {
            in_cjk_run = false;
            if in_digit_run {
                in_digit_run = false;
            }
            if !in_latin_phrase {
                shapes.push("latin".to_owned());
                in_latin_phrase = true;
            }
            alpha_characters = alpha_characters.saturating_add(1);
        } else if character.is_ascii_digit() {
            in_latin_phrase = false;
            in_cjk_run = false;
            if !in_digit_run {
                shapes.push("number".to_owned());
                in_digit_run = true;
            }
        } else if crate::dictionary_learn::is_cjk(character) {
            in_latin_phrase = false;
            if in_digit_run {
                in_digit_run = false;
            }
            if !in_cjk_run {
                shapes.push("c".to_owned());
                in_cjk_run = true;
            }
            alpha_characters = alpha_characters.saturating_add(1);
        } else if character.is_whitespace() {
            // Whitespace between ASCII words does not create a new template
            // token. A new line remains a paragraph-shape boundary.
            in_digit_run = false;
            if character == '\r' {
                if characters
                    .peek()
                    .is_some_and(|(_, next_character)| *next_character == '\n')
                {
                    continue;
                }
                in_latin_phrase = false;
                in_cjk_run = false;
                shapes.push("line".to_owned());
            } else if character == '\n' {
                in_latin_phrase = false;
                in_cjk_run = false;
                shapes.push("line".to_owned());
            }
        } else if is_template_terminal_punctuation(character) && byte_index >= terminal_boundary {
            // A changed final full stop must not manufacture template
            // diversity for a terminal-punctuation candidate.
            in_latin_phrase = false;
            in_cjk_run = false;
            in_digit_run = false;
        } else if matches!(character, '-' | '‐' | '‑' | '\'' | '’') {
            // Keep common in-word joiners within one coarse Latin phrase.
        } else if matches!(
            character,
            ',' | '，' | '、' | ':' | '：' | ';' | '；' | '?' | '？' | '!' | '！'
        ) {
            in_latin_phrase = false;
            in_cjk_run = false;
            in_digit_run = false;
            shapes.push(match character {
                ',' | '，' | '、' => "comma".to_owned(),
                ':' | '：' => "colon".to_owned(),
                ';' | '；' => "semicolon".to_owned(),
                '?' | '？' => "question".to_owned(),
                _ => "exclamation".to_owned(),
            });
        } else {
            in_latin_phrase = false;
            in_cjk_run = false;
            in_digit_run = false;
        }
    }
    if alpha_characters < 3 || shapes.is_empty() {
        return None;
    }
    // Each clause's Latin wording is one token, so names in any clause
    // position cannot manufacture template diversity by changing word count.
    // This intentionally over-groups uncertain text, reducing readiness.
    let mut framed = b"voiceflow-template-family-v2\0".to_vec();
    for shape in shapes {
        framed.extend_from_slice(shape.as_bytes());
        framed.push(0);
    }
    let digest = Sha256::digest(&framed);
    Some(hex::encode(digest))
}

fn is_template_terminal_punctuation(character: char) -> bool {
    matches!(character, '.' | '。' | '！' | '!' | '？' | '?')
}

/// Content fingerprints are local, bounded identifiers. Whitespace-only
/// edits collapse, but source content is never returned or stored here.
pub(crate) fn content_fingerprint(text: &str) -> Option<String> {
    if text.trim().is_empty() || text.len() > MAX_HISTORY_PREFERENCE_TEXT_BYTES {
        return None;
    }
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let digest = Sha256::digest(normalized.as_bytes());
    Some(hex::encode(digest))
}

/// A persisted, source-bounded History edit. It carries no text or independent
/// human label; it only supplies weak runtime support for a later review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WeakHistoryPreferenceEvidence {
    pub(crate) task_id: WritingTaskId,
    pub(crate) dimension: WritingPreferenceDimension,
    pub(crate) value: WritingPreferenceValue,
    pub(crate) origin_id: String,
    pub(crate) content_fingerprint: String,
    pub(crate) occurred_at_ms: u64,
    pub(crate) natural_day: String,
    pub(crate) editor_session_id: String,
    pub(crate) template_family_fingerprint: Option<String>,
    pub(crate) eligible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HistoryCandidateDecision {
    CandidateReady {
        supporting_origins: usize,
        distinct_days: usize,
        distinct_sessions: usize,
        distinct_template_families: usize,
    },
    BelowThreshold {
        observed_origins: usize,
        supporting_origins: usize,
        distinct_days: usize,
        distinct_sessions: usize,
        distinct_template_families: usize,
    },
    Unknown,
}

/// Count only authentic, weak History commits. Independent holdout labels use
/// the separate `TrustedPreferenceEvidence` classifier below.
pub(crate) fn classify_weak_history_candidate(
    now_ms: u64,
    task_id: WritingTaskId,
    dimension: WritingPreferenceDimension,
    value: WritingPreferenceValue,
    evidence: &[WeakHistoryPreferenceEvidence],
) -> HistoryCandidateDecision {
    if evidence.len() > MAX_WEAK_HISTORY_EVIDENCE_ROWS {
        return HistoryCandidateDecision::Unknown;
    }
    let cutoff = now_ms.saturating_sub(WINDOW_MS);
    if evidence.iter().any(|item| {
        item.eligible
            && item.task_id == task_id
            && item.dimension == dimension
            && item.value != value
            && item.occurred_at_ms >= cutoff
            && item.occurred_at_ms <= now_ms
    }) {
        return HistoryCandidateDecision::Unknown;
    }

    let mut observed_origins = HashSet::new();
    let mut content_fingerprints = HashSet::new();
    let mut supporting_origins = HashSet::new();
    let mut days = HashSet::new();
    let mut sessions = HashSet::new();
    let mut template_families = HashSet::new();
    for item in evidence {
        if !item.eligible
            || item.task_id != task_id
            || item.dimension != dimension
            || item.value != value
            || item.occurred_at_ms < cutoff
            || item.occurred_at_ms > now_ms
            || item.origin_id.trim().is_empty()
            || !is_sha256_hex(&item.content_fingerprint)
            || item.natural_day.trim().is_empty()
            || !valid_editor_session_id(&item.editor_session_id)
        {
            continue;
        }
        if item
            .template_family_fingerprint
            .as_deref()
            .is_some_and(|fingerprint| !is_sha256_hex(fingerprint))
        {
            continue;
        }
        if !observed_origins.insert(item.origin_id.as_str())
            || !content_fingerprints.insert(item.content_fingerprint.as_str())
        {
            continue;
        }
        let Some(template_family) = item.template_family_fingerprint.as_deref() else {
            // This origin remains visible as weak evidence but cannot prove
            // template independence or contribute to candidate readiness.
            continue;
        };
        if !template_families.insert(template_family) {
            continue;
        }
        supporting_origins.insert(item.origin_id.as_str());
        days.insert(item.natural_day.as_str());
        sessions.insert(item.editor_session_id.as_str());
    }

    let observed_origins = observed_origins.len();
    let supporting_origins = supporting_origins.len();
    let distinct_days = days.len();
    let distinct_sessions = sessions.len();
    let distinct_template_families = template_families.len();
    if supporting_origins >= MIN_DISTINCT_ORIGINS
        && distinct_days >= MIN_NATURAL_DAYS
        && distinct_sessions >= MIN_SESSIONS
        && distinct_template_families >= MIN_DISTINCT_ORIGINS
    {
        HistoryCandidateDecision::CandidateReady {
            supporting_origins,
            distinct_days,
            distinct_sessions,
            distinct_template_families,
        }
    } else if observed_origins == 0 {
        HistoryCandidateDecision::Unknown
    } else {
        HistoryCandidateDecision::BelowThreshold {
            observed_origins,
            supporting_origins,
            distinct_days,
            distinct_sessions,
            distinct_template_families,
        }
    }
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Research labels and provenance proofs supplied by an independent shadow
/// harness. These fields are not currently persisted by VoiceFlow.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustedPreferenceEvidence {
    pub(crate) task_id: WritingTaskId,
    pub(crate) dimension: WritingPreferenceDimension,
    pub(crate) value: WritingPreferenceValue,
    pub(crate) origin_id: String,
    pub(crate) content_fingerprint: String,
    pub(crate) occurred_at_ms: u64,
    /// Stable local natural-day label assigned by the independent evaluator.
    pub(crate) natural_day: String,
    pub(crate) session_id: String,
    /// A known template family is required; unknown template diversity does
    /// not count as evidence that the examples are independent.
    pub(crate) template_family_id: Option<String>,
    pub(crate) task_identity_trusted: bool,
    pub(crate) independent_long_term_label: bool,
    pub(crate) source_alignment_verified: bool,
    pub(crate) author_commit_verified: bool,
    pub(crate) ime_commit_verified: bool,
    pub(crate) host_formatting_excluded: bool,
    pub(crate) temporary_intent: bool,
    pub(crate) counterevidence: bool,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShadowDecision {
    CandidateReady {
        supporting_origins: usize,
    },
    BelowThreshold {
        supporting_origins: usize,
        distinct_days: usize,
        distinct_sessions: usize,
    },
    Unknown,
}

/// Apply the proposed §7 threshold to explicitly trusted, independently
/// labeled evidence. This function never activates or persists a preference.
#[cfg(test)]
pub(crate) fn classify_shadow_candidate(
    now_ms: u64,
    task_id: WritingTaskId,
    dimension: WritingPreferenceDimension,
    value: WritingPreferenceValue,
    evidence: &[TrustedPreferenceEvidence],
) -> ShadowDecision {
    let cutoff = now_ms.saturating_sub(WINDOW_MS);
    if evidence.iter().any(|item| {
        item.task_id == task_id
            && item.dimension == dimension
            && item.counterevidence
            && item.occurred_at_ms >= cutoff
            && item.occurred_at_ms <= now_ms
    }) {
        return ShadowDecision::Unknown;
    }

    let mut origins = HashSet::new();
    let mut content = HashSet::new();
    let mut template_families = HashSet::new();
    let mut days = HashSet::new();
    let mut sessions = HashSet::new();

    for item in evidence {
        if item.task_id != task_id
            || item.dimension != dimension
            || item.value != value
            || item.occurred_at_ms < cutoff
            || item.occurred_at_ms > now_ms
            || item.temporary_intent
            || !item.task_identity_trusted
            || !item.independent_long_term_label
            || !item.source_alignment_verified
            || !item.author_commit_verified
            || !item.ime_commit_verified
            || !item.host_formatting_excluded
            || item.origin_id.trim().is_empty()
            || item.content_fingerprint.trim().is_empty()
            || item.natural_day.trim().is_empty()
            || item.session_id.trim().is_empty()
        {
            continue;
        }
        let Some(template_family) = item
            .template_family_id
            .as_deref()
            .map(str::trim)
            .filter(|family| !family.is_empty())
        else {
            continue;
        };
        if origins.contains(item.origin_id.as_str())
            || content.contains(item.content_fingerprint.as_str())
            || template_families.contains(template_family)
        {
            continue;
        }
        origins.insert(item.origin_id.as_str());
        content.insert(item.content_fingerprint.as_str());
        template_families.insert(template_family);
        days.insert(item.natural_day.as_str());
        sessions.insert(item.session_id.as_str());
    }

    let supporting_origins = origins.len();
    let distinct_days = days.len();
    let distinct_sessions = sessions.len();
    if supporting_origins >= MIN_DISTINCT_ORIGINS
        && distinct_days >= MIN_NATURAL_DAYS
        && distinct_sessions >= MIN_SESSIONS
    {
        ShadowDecision::CandidateReady { supporting_origins }
    } else if supporting_origins == 0 {
        ShadowDecision::Unknown
    } else {
        ShadowDecision::BelowThreshold {
            supporting_origins,
            distinct_days,
            distinct_sessions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW_MS: u64 = 20 * WINDOW_MS;

    fn evidence(index: usize) -> TrustedPreferenceEvidence {
        TrustedPreferenceEvidence {
            task_id: WritingTaskId::ProjectUpdate,
            dimension: WritingPreferenceDimension::ListStructure,
            value: WritingPreferenceValue::Bullets,
            origin_id: format!("origin-{index}"),
            content_fingerprint: format!("content-{index}"),
            occurred_at_ms: NOW_MS - index as u64 * 86_400_000,
            natural_day: format!("day-{}", index % 2),
            session_id: format!("session-{}", index % 2),
            template_family_id: Some(format!("template-{index}")),
            task_identity_trusted: true,
            independent_long_term_label: true,
            source_alignment_verified: true,
            author_commit_verified: true,
            ime_commit_verified: true,
            host_formatting_excluded: true,
            temporary_intent: false,
            counterevidence: false,
        }
    }

    fn history_commit(kinds: Vec<HistoryEditorInputKind>) -> HistoryEditorCommitV1 {
        HistoryEditorCommitV1 {
            version: 1,
            editor_session_id: "00000000-0000-4000-8000-000000000001".into(),
            input_kinds: kinds,
            composing_at_save: false,
            save_action_trusted: true,
        }
    }

    fn history_input<'a>(
        before_text: &'a str,
        after_text: &'a str,
        raw_text: &'a str,
        asr_text: &'a str,
        commit: &'a HistoryEditorCommitV1,
    ) -> HistoryPreferenceInput<'a> {
        HistoryPreferenceInput {
            owner: HistoryPreferenceOwner::VoiceFlowHistory,
            delivery_eligible: true,
            input_kind: FocusKind::Editable,
            transaction_id: Some("dictation-transaction"),
            task_id: Some(WritingTaskId::ProjectUpdate),
            task_identity_trusted: true,
            mapping_identity_matches: true,
            mapping_revision_matches: true,
            history_policy_present: true,
            captured_intent_source: Some(IntentSource::Implicit),
            output_kind: HistoryPreferenceOutputKind::AcceptedCleanup,
            snippet_expanded: false,
            output_mode: Some("auto"),
            session_skip_llm_cleanup: Some(false),
            original_prepared_text: Some(raw_text),
            original_asr_text: Some(asr_text),
            configured_translation_target: None,
            before_text,
            after_text,
            matches_existing_source: Some(false),
            editor_commit: Some(commit),
        }
    }

    fn weak_history_evidence(index: usize) -> WeakHistoryPreferenceEvidence {
        WeakHistoryPreferenceEvidence {
            task_id: WritingTaskId::ProjectUpdate,
            dimension: WritingPreferenceDimension::ListStructure,
            value: WritingPreferenceValue::Bullets,
            origin_id: format!("origin-{index}"),
            content_fingerprint: format!("{:064x}", index + 1),
            occurred_at_ms: NOW_MS - index as u64 * 86_400_000,
            natural_day: format!("day-{}", index % 2),
            editor_session_id: format!("00000000-0000-4000-8000-00000000000{}", index % 2 + 1),
            template_family_fingerprint: Some(format!("{:064x}", index + 101)),
            eligible: true,
        }
    }

    #[test]
    fn history_list_edit_accepts_only_source_backed_item_projection() {
        let source = "The three tasks are: update API; fix tests; review docs.";
        let projected = crate::cleanup_output_layout::project_cleanup_layout(
            source,
            crate::cleanup_output_layout::OutputLayoutMode::List,
        )
        .unwrap();
        let commit = history_commit(vec![HistoryEditorInputKind::TrustedLineBreak]);
        let input = history_input(source, &projected.text, source, source, &commit);
        assert_eq!(
            classify_history_preference_edit(&input),
            HistoryPreferenceDecision::WeakEvidence {
                dimension: WritingPreferenceDimension::ListStructure,
                value: WritingPreferenceValue::Bullets,
            }
        );

        let ambiguous_source = "We should update the API, then fix tests, and review docs.";
        let ambiguous_output = "- We should update the API\n- then fix tests\n- and review docs.";
        let ambiguous = history_input(
            ambiguous_source,
            ambiguous_output,
            ambiguous_source,
            ambiguous_source,
            &commit,
        );
        assert_eq!(
            classify_history_preference_edit(&ambiguous),
            HistoryPreferenceDecision::Excluded(HistoryPreferenceExclusion::UnsupportedEdit)
        );
    }

    #[test]
    fn history_classifier_accepts_supported_tone_punctuation_and_local_prepared_edits() {
        let greeting_before = "你好，Mingjie，周五见。";
        let greeting_after = "您好，Mingjie，周五见。";
        let greeting_commit = history_commit(vec![HistoryEditorInputKind::TrustedText]);
        let mut greeting = history_input(
            greeting_before,
            greeting_after,
            greeting_before,
            greeting_before,
            &greeting_commit,
        );
        greeting.task_id = Some(WritingTaskId::Email);
        assert_eq!(
            classify_history_preference_edit(&greeting),
            HistoryPreferenceDecision::WeakEvidence {
                dimension: WritingPreferenceDimension::GreetingTone,
                value: WritingPreferenceValue::RespectfulGreeting,
            }
        );

        let punctuation_before = "我们今天可以交付。";
        let punctuation_after = "我们今天可以交付";
        let punctuation_commit = history_commit(vec![HistoryEditorInputKind::TrustedDelete]);
        let mut punctuation = history_input(
            punctuation_before,
            punctuation_after,
            punctuation_before,
            punctuation_before,
            &punctuation_commit,
        );
        punctuation.task_id = Some(WritingTaskId::WorkChat);
        assert_eq!(
            classify_history_preference_edit(&punctuation),
            HistoryPreferenceDecision::WeakEvidence {
                dimension: WritingPreferenceDimension::TerminalPunctuation,
                value: WritingPreferenceValue::NoTerminalPeriod,
            }
        );

        let mut punctuation_restore = history_input(
            "我们今天可以交付。",
            "我们今天可以交付",
            "我们今天可以交付",
            "我们今天可以交付",
            &punctuation_commit,
        );
        punctuation_restore.task_id = Some(WritingTaskId::WorkChat);
        assert_eq!(
            classify_history_preference_edit(&punctuation_restore),
            HistoryPreferenceDecision::Excluded(HistoryPreferenceExclusion::SourceRestoration)
        );

        let source = "The three tasks are: update API; fix tests; review docs.";
        let projected = crate::cleanup_output_layout::project_cleanup_layout(
            source,
            crate::cleanup_output_layout::OutputLayoutMode::List,
        )
        .unwrap();
        let prepared_commit = history_commit(vec![HistoryEditorInputKind::TrustedLineBreak]);
        let mut prepared = history_input(source, &projected.text, source, source, &prepared_commit);
        prepared.output_kind = HistoryPreferenceOutputKind::PreparedTranscript;
        assert!(matches!(
            classify_history_preference_edit(&prepared),
            HistoryPreferenceDecision::WeakEvidence {
                dimension: WritingPreferenceDimension::ListStructure,
                value: WritingPreferenceValue::Bullets,
            }
        ));
    }

    #[test]
    fn session_verbatim_is_excluded_but_global_local_only_can_learn() {
        let source = "The three tasks are: update API; fix tests; review docs.";
        let projection = crate::cleanup_output_layout::project_cleanup_layout(
            source,
            crate::cleanup_output_layout::OutputLayoutMode::List,
        )
        .unwrap();
        let commit = history_commit(vec![HistoryEditorInputKind::TrustedLineBreak]);

        // Session Verbatim changes only the cloned session settings. Output
        // mode remains Auto, so the captured per-session bit is required to
        // distinguish it from an ordinary local-only route.
        let mut verbatim_settings = crate::store::Settings {
            output_mode: "auto".into(),
            ..crate::store::Settings::default()
        };
        crate::dictation::apply_session_mode(&mut verbatim_settings, true, None);
        assert!(!verbatim_settings.cleanup_enabled);
        assert_eq!(verbatim_settings.output_mode, "auto");
        let verbatim_provenance = crate::history_preference_provenance(
            &crate::context::ContextSnapshot::general(),
            IntentSource::Implicit,
            &verbatim_settings.output_mode,
            false,
            crate::CLEANUP_STATUS_LOCAL_ONLY,
            source,
            source,
            true,
        );
        assert_eq!(verbatim_provenance.session_skip_llm_cleanup, Some(true));
        let mut verbatim = history_input(source, &projection.text, source, source, &commit);
        verbatim.output_kind = verbatim_provenance.output_kind;
        verbatim.output_mode = verbatim_provenance.output_mode.as_deref();
        verbatim.session_skip_llm_cleanup = verbatim_provenance.session_skip_llm_cleanup;
        assert_eq!(
            classify_history_preference_edit(&verbatim),
            HistoryPreferenceDecision::Excluded(HistoryPreferenceExclusion::SessionCleanupBypass)
        );

        // A globally disabled cleanup setting has no session bypass bit and
        // retains the same PreparedTranscript path where source checks pass.
        let mut local_only_settings = crate::store::Settings {
            output_mode: "auto".into(),
            cleanup_enabled: false,
            ..crate::store::Settings::default()
        };
        crate::dictation::apply_session_mode(&mut local_only_settings, false, None);
        assert!(!local_only_settings.cleanup_enabled);
        let local_only_provenance = crate::history_preference_provenance(
            &crate::context::ContextSnapshot::general(),
            IntentSource::Implicit,
            &local_only_settings.output_mode,
            false,
            crate::CLEANUP_STATUS_LOCAL_ONLY,
            source,
            source,
            false,
        );
        assert_eq!(local_only_provenance.session_skip_llm_cleanup, Some(false));
        let mut legacy_policy_json = serde_json::to_value(&local_only_provenance).unwrap();
        legacy_policy_json
            .as_object_mut()
            .unwrap()
            .remove("session_skip_llm_cleanup");
        let legacy_provenance: crate::context::HistoryPreferenceProvenanceV1 =
            serde_json::from_value(legacy_policy_json).unwrap();
        assert_eq!(legacy_provenance.session_skip_llm_cleanup, None);
        let mut local_only = history_input(source, &projection.text, source, source, &commit);
        local_only.output_kind = local_only_provenance.output_kind;
        local_only.output_mode = local_only_provenance.output_mode.as_deref();
        local_only.session_skip_llm_cleanup = local_only_provenance.session_skip_llm_cleanup;
        assert!(matches!(
            classify_history_preference_edit(&local_only),
            HistoryPreferenceDecision::WeakEvidence {
                dimension: WritingPreferenceDimension::ListStructure,
                value: WritingPreferenceValue::Bullets,
            }
        ));

        let mut legacy = local_only;
        legacy.session_skip_llm_cleanup = legacy_provenance.session_skip_llm_cleanup;
        assert_eq!(
            classify_history_preference_edit(&legacy),
            HistoryPreferenceDecision::Unknown(HistoryPreferenceUnknown::MissingSessionCleanupMode)
        );
    }

    #[test]
    fn history_classifier_excludes_restoration_temporary_intent_and_unsafe_commits() {
        let before = "The three tasks are: update API; fix tests; review docs.";
        let projected = crate::cleanup_output_layout::project_cleanup_layout(
            before,
            crate::cleanup_output_layout::OutputLayoutMode::List,
        )
        .unwrap();
        let safe_commit = history_commit(vec![HistoryEditorInputKind::TrustedLineBreak]);

        let restored = history_input(before, before, before, before, &safe_commit);
        assert_eq!(
            classify_history_preference_edit(&restored),
            HistoryPreferenceDecision::Excluded(HistoryPreferenceExclusion::UnsupportedEdit)
        );

        let restored_source =
            history_input(before, "restored raw text", before, before, &safe_commit);
        let mut restored_source = restored_source;
        restored_source.matches_existing_source = Some(true);
        assert_eq!(
            classify_history_preference_edit(&restored_source),
            HistoryPreferenceDecision::Excluded(HistoryPreferenceExclusion::SourceRestoration)
        );

        let mut explicit_mode =
            history_input(before, &projected.text, before, before, &safe_commit);
        explicit_mode.output_mode = Some("bullets");
        assert_eq!(
            classify_history_preference_edit(&explicit_mode),
            HistoryPreferenceDecision::Excluded(HistoryPreferenceExclusion::ExplicitOutputMode)
        );

        let mut explicit_intent =
            history_input(before, &projected.text, before, before, &safe_commit);
        explicit_intent.original_asr_text = Some("Please rewrite this as bullets.");
        assert_eq!(
            classify_history_preference_edit(&explicit_intent),
            HistoryPreferenceDecision::Excluded(HistoryPreferenceExclusion::ExplicitOneShotIntent)
        );

        let paste_commit = history_commit(vec![HistoryEditorInputKind::Clipboard]);
        let paste = history_input(before, &projected.text, before, before, &paste_commit);
        assert_eq!(
            classify_history_preference_edit(&paste),
            HistoryPreferenceDecision::Excluded(HistoryPreferenceExclusion::UnsafeEditorInput)
        );

        let mut composing_commit =
            history_commit(vec![HistoryEditorInputKind::TrustedCompositionText]);
        composing_commit.composing_at_save = true;
        let composing = history_input(before, &projected.text, before, before, &composing_commit);
        assert_eq!(
            classify_history_preference_edit(&composing),
            HistoryPreferenceDecision::Unknown(HistoryPreferenceUnknown::InvalidEditorCommit)
        );

        let mut external = history_input(before, &projected.text, before, before, &safe_commit);
        external.owner = HistoryPreferenceOwner::ExternalAccessibility;
        assert!(matches!(
            classify_history_preference_edit(&external),
            HistoryPreferenceDecision::Unknown(_)
        ));
    }

    #[test]
    fn history_classifier_rejects_slot_restoration_and_unaligned_sources() {
        let commit = history_commit(vec![HistoryEditorInputKind::TrustedText]);
        let mut greeting_restore = history_input(
            "您好，请发给团队。",
            "你好，请发给团队。",
            "你好，请发给团队",
            "你好，请发给团队",
            &commit,
        );
        greeting_restore.task_id = Some(WritingTaskId::Email);
        assert_eq!(
            classify_history_preference_edit(&greeting_restore),
            HistoryPreferenceDecision::Excluded(HistoryPreferenceExclusion::SourceRestoration)
        );

        let sentence = "The three tasks are: update API; fix tests; review docs.";
        let projection = crate::cleanup_output_layout::project_cleanup_layout(
            sentence,
            crate::cleanup_output_layout::OutputLayoutMode::List,
        )
        .unwrap();
        let prior_list = "- update API\n- fix tests\n- review docs.";
        let list_commit = history_commit(vec![HistoryEditorInputKind::TrustedLineBreak]);
        let list_restore = history_input(
            sentence,
            &projection.text,
            prior_list,
            prior_list,
            &list_commit,
        );
        assert_eq!(
            classify_history_preference_edit(&list_restore),
            HistoryPreferenceDecision::Excluded(HistoryPreferenceExclusion::SourceRestoration)
        );

        let unrelated_source = history_input(
            sentence,
            &projection.text,
            "Unrelated original source text",
            "Unrelated original source text",
            &list_commit,
        );
        assert_eq!(
            classify_history_preference_edit(&unrelated_source),
            HistoryPreferenceDecision::Unknown(HistoryPreferenceUnknown::MissingSourceAlignment)
        );
    }

    #[test]
    fn template_family_groups_variable_word_lengths_without_retaining_text() {
        let al = template_family_fingerprint("你好，please send this to Al。").unwrap();
        let sam = template_family_fingerprint("你好，please send this to Sam Lee。").unwrap();
        let morgan =
            template_family_fingerprint("你好，please send this to Morgan Van Smith。").unwrap();
        assert_eq!(al, sam);
        assert_eq!(al, morgan);
        assert_eq!(al.len(), 64);
        assert!(al.bytes().all(|byte| byte.is_ascii_hexdigit()));
        let without_terminal = template_family_fingerprint("你好，Al will review this").unwrap();
        assert_eq!(al, without_terminal);

        let cjk_short = template_family_fingerprint("周五记得回复。").unwrap();
        let cjk_different_words = template_family_fingerprint("明天早上记得发报告。").unwrap();
        assert_eq!(cjk_short, cjk_different_words);
        let cjk_clause = template_family_fingerprint("明天早上，记得发报告。").unwrap();
        let cjk_multi_clause = template_family_fingerprint(
            "下周三之前：请完成关于项目延期情况的全面技术评估；并发送给相关负责人。",
        )
        .unwrap();
        assert_ne!(cjk_short, cjk_clause);
        assert_ne!(cjk_clause, cjk_multi_clause);
        assert_ne!(cjk_short, cjk_multi_clause);
        let cjk_number = template_family_fingerprint("明天3天后发送报告。").unwrap();
        let cjk_paragraphs = template_family_fingerprint("明天发送报告。\n周五跟进。").unwrap();
        assert_ne!(cjk_short, cjk_number);
        assert_ne!(cjk_short, cjk_paragraphs);
    }

    #[test]
    fn template_digit_marker_is_stable_across_digit_letter_spacing() {
        assert_eq!(
            template_family_fingerprint("5G 网络").unwrap(),
            template_family_fingerprint("5 G 网络").unwrap()
        );
    }

    #[test]
    fn variable_recipient_word_count_cannot_create_three_template_families() {
        let examples = [
            ("你好，Al will review this。", "您好，Al will review this。"),
            (
                "你好，Sam Lee will review this。",
                "您好，Sam Lee will review this。",
            ),
            (
                "你好，Morgan Van Smith will review this。",
                "您好，Morgan Van Smith will review this。",
            ),
        ];
        let commit = history_commit(vec![HistoryEditorInputKind::TrustedText]);
        let mut evidence = Vec::new();

        for (index, (source, edited)) in examples.iter().enumerate() {
            let mut input = history_input(source, edited, source, source, &commit);
            input.task_id = Some(WritingTaskId::Email);
            assert_eq!(
                classify_history_preference_edit(&input),
                HistoryPreferenceDecision::WeakEvidence {
                    dimension: WritingPreferenceDimension::GreetingTone,
                    value: WritingPreferenceValue::RespectfulGreeting,
                }
            );

            let mut row = weak_history_evidence(index);
            row.task_id = WritingTaskId::Email;
            row.dimension = WritingPreferenceDimension::GreetingTone;
            row.value = WritingPreferenceValue::RespectfulGreeting;
            row.content_fingerprint = content_fingerprint(source).unwrap();
            row.template_family_fingerprint = template_family_fingerprint(source);
            evidence.push(row);
        }

        assert_eq!(
            classify_weak_history_candidate(
                NOW_MS,
                WritingTaskId::Email,
                WritingPreferenceDimension::GreetingTone,
                WritingPreferenceValue::RespectfulGreeting,
                &evidence,
            ),
            HistoryCandidateDecision::BelowThreshold {
                observed_origins: 3,
                supporting_origins: 1,
                distinct_days: 1,
                distinct_sessions: 1,
                distinct_template_families: 1,
            }
        );
    }

    #[test]
    fn cjk_greeting_and_terminal_examples_can_reach_weak_threshold() {
        let sources = [
            "周五记得回复。",
            "明天早上，记得发报告。",
            "开会结束后，先整理资料，再发送给团队。",
        ];
        let terminal_commit = history_commit(vec![HistoryEditorInputKind::TrustedDelete]);
        let greeting_commit = history_commit(vec![HistoryEditorInputKind::TrustedText]);
        let mut terminal_evidence = Vec::new();
        let mut greeting_evidence = Vec::new();

        for (index, source) in sources.iter().enumerate() {
            let terminal_after = source.strip_suffix('。').unwrap();
            let mut terminal_input =
                history_input(source, terminal_after, source, source, &terminal_commit);
            terminal_input.task_id = Some(WritingTaskId::WorkChat);
            let terminal_decision = classify_history_preference_edit(&terminal_input);
            assert_eq!(
                terminal_decision,
                HistoryPreferenceDecision::WeakEvidence {
                    dimension: WritingPreferenceDimension::TerminalPunctuation,
                    value: WritingPreferenceValue::NoTerminalPeriod,
                }
            );
            let mut terminal_row = weak_history_evidence(index);
            terminal_row.task_id = WritingTaskId::WorkChat;
            terminal_row.dimension = WritingPreferenceDimension::TerminalPunctuation;
            terminal_row.value = WritingPreferenceValue::NoTerminalPeriod;
            terminal_row.content_fingerprint = content_fingerprint(source).unwrap();
            terminal_row.template_family_fingerprint = template_family_fingerprint(source);
            terminal_evidence.push(terminal_row);

            let before = format!("你好，{source}");
            let after = format!("您好，{source}");
            let mut greeting_input =
                history_input(&before, &after, &before, &before, &greeting_commit);
            greeting_input.task_id = Some(WritingTaskId::Email);
            let greeting_decision = classify_history_preference_edit(&greeting_input);
            assert_eq!(
                greeting_decision,
                HistoryPreferenceDecision::WeakEvidence {
                    dimension: WritingPreferenceDimension::GreetingTone,
                    value: WritingPreferenceValue::RespectfulGreeting,
                }
            );
            let mut greeting_row = weak_history_evidence(index);
            greeting_row.task_id = WritingTaskId::Email;
            greeting_row.dimension = WritingPreferenceDimension::GreetingTone;
            greeting_row.value = WritingPreferenceValue::RespectfulGreeting;
            greeting_row.content_fingerprint = content_fingerprint(&before).unwrap();
            greeting_row.template_family_fingerprint = template_family_fingerprint(&before);
            greeting_evidence.push(greeting_row);
        }

        assert!(matches!(
            classify_weak_history_candidate(
                NOW_MS,
                WritingTaskId::WorkChat,
                WritingPreferenceDimension::TerminalPunctuation,
                WritingPreferenceValue::NoTerminalPeriod,
                &terminal_evidence,
            ),
            HistoryCandidateDecision::CandidateReady { .. }
        ));
        assert!(matches!(
            classify_weak_history_candidate(
                NOW_MS,
                WritingTaskId::Email,
                WritingPreferenceDimension::GreetingTone,
                WritingPreferenceValue::RespectfulGreeting,
                &greeting_evidence,
            ),
            HistoryCandidateDecision::CandidateReady { .. }
        ));
    }

    #[test]
    fn ascii_period_abbreviations_do_not_produce_terminal_preference() {
        let commit = history_commit(vec![HistoryEditorInputKind::TrustedDelete]);
        for (before, after) in [
            ("Please ask Dr.", "Please ask Dr"),
            ("Acme Inc", "Acme Inc."),
        ] {
            let input = history_input(before, after, before, before, &commit);
            assert_eq!(
                classify_history_preference_edit(&input),
                HistoryPreferenceDecision::Excluded(HistoryPreferenceExclusion::UnsupportedEdit),
                "{before:?} -> {after:?} must not be treated as a terminal-period preference"
            );
        }
    }

    #[test]
    fn history_classifier_requires_captured_scope_and_supported_task_value_pairs() {
        let before = "你好，Mingjie。";
        let after = "您好，Mingjie。";
        let commit = history_commit(vec![HistoryEditorInputKind::TrustedText]);
        let mut old_history = history_input(before, after, before, before, &commit);
        old_history.history_policy_present = false;
        assert_eq!(
            classify_history_preference_edit(&old_history),
            HistoryPreferenceDecision::Unknown(HistoryPreferenceUnknown::MissingHistoryPolicy)
        );
        let mut missing_mode = history_input(before, after, before, before, &commit);
        missing_mode.output_mode = None;
        assert_eq!(
            classify_history_preference_edit(&missing_mode),
            HistoryPreferenceDecision::Unknown(HistoryPreferenceUnknown::MissingCapturedOutputMode)
        );
        let mut wrong_task = history_input(before, after, before, before, &commit);
        assert_eq!(
            classify_history_preference_edit(&wrong_task),
            HistoryPreferenceDecision::Unknown(HistoryPreferenceUnknown::UnsupportedTaskValue)
        );
        wrong_task.task_id = None;
        assert_eq!(
            classify_history_preference_edit(&wrong_task),
            HistoryPreferenceDecision::Unknown(HistoryPreferenceUnknown::MissingTask)
        );
    }

    #[test]
    fn history_classifier_rejects_protected_targets_and_unsuccessful_delivery() {
        let before = "The three tasks are: update API; fix tests; review docs.";
        let projected = crate::cleanup_output_layout::project_cleanup_layout(
            before,
            crate::cleanup_output_layout::OutputLayoutMode::List,
        )
        .unwrap();
        let commit = history_commit(vec![HistoryEditorInputKind::TrustedLineBreak]);

        let mut code = history_input(before, &projected.text, before, before, &commit);
        code.input_kind = FocusKind::Code;
        assert_eq!(
            classify_history_preference_edit(&code),
            HistoryPreferenceDecision::Excluded(HistoryPreferenceExclusion::ProtectedInputKind)
        );

        let mut unknown = history_input(before, &projected.text, before, before, &commit);
        unknown.input_kind = FocusKind::Unknown;
        assert_eq!(
            classify_history_preference_edit(&unknown),
            HistoryPreferenceDecision::Unknown(HistoryPreferenceUnknown::UnsupportedInputKind)
        );

        let mut failed_delivery = history_input(before, &projected.text, before, before, &commit);
        failed_delivery.delivery_eligible = false;
        assert_eq!(
            classify_history_preference_edit(&failed_delivery),
            HistoryPreferenceDecision::Excluded(HistoryPreferenceExclusion::KnownDeliveryFailure)
        );
    }

    #[test]
    fn weak_history_threshold_requires_distinct_origins_content_templates_days_and_sessions() {
        assert_eq!(
            classify_weak_history_candidate(
                NOW_MS,
                WritingTaskId::ProjectUpdate,
                WritingPreferenceDimension::ListStructure,
                WritingPreferenceValue::Bullets,
                &[
                    weak_history_evidence(0),
                    weak_history_evidence(1),
                    weak_history_evidence(2),
                ],
            ),
            HistoryCandidateDecision::CandidateReady {
                supporting_origins: 3,
                distinct_days: 2,
                distinct_sessions: 2,
                distinct_template_families: 3,
            }
        );

        let mut no_template = weak_history_evidence(2);
        no_template.template_family_fingerprint = None;
        assert_eq!(
            classify_weak_history_candidate(
                NOW_MS,
                WritingTaskId::ProjectUpdate,
                WritingPreferenceDimension::ListStructure,
                WritingPreferenceValue::Bullets,
                &[
                    weak_history_evidence(0),
                    weak_history_evidence(1),
                    no_template,
                ],
            ),
            HistoryCandidateDecision::BelowThreshold {
                observed_origins: 3,
                supporting_origins: 2,
                distinct_days: 2,
                distinct_sessions: 2,
                distinct_template_families: 2,
            }
        );
    }

    #[test]
    fn requires_three_distinct_origins_days_sessions_and_template_families() {
        assert_eq!(
            classify_shadow_candidate(
                NOW_MS,
                WritingTaskId::ProjectUpdate,
                WritingPreferenceDimension::ListStructure,
                WritingPreferenceValue::Bullets,
                &[evidence(0), evidence(1), evidence(2)],
            ),
            ShadowDecision::CandidateReady {
                supporting_origins: 3
            }
        );
    }

    #[test]
    fn duplicate_origins_content_or_template_families_do_not_reach_threshold() {
        let mut repeated_origin = evidence(1);
        repeated_origin.origin_id = evidence(0).origin_id;
        let mut repeated_content = evidence(2);
        repeated_content.content_fingerprint = evidence(0).content_fingerprint;
        let mut repeated_template = evidence(3);
        repeated_template.template_family_id = evidence(0).template_family_id;
        assert_eq!(
            classify_shadow_candidate(
                NOW_MS,
                WritingTaskId::ProjectUpdate,
                WritingPreferenceDimension::ListStructure,
                WritingPreferenceValue::Bullets,
                &[
                    evidence(0),
                    repeated_origin,
                    repeated_content,
                    repeated_template,
                ],
            ),
            ShadowDecision::BelowThreshold {
                supporting_origins: 1,
                distinct_days: 1,
                distinct_sessions: 1,
            }
        );
    }

    #[test]
    fn untrusted_or_ambiguous_live_evidence_abstains() {
        let mut no_author_proof = evidence(0);
        no_author_proof.author_commit_verified = false;
        let mut ime_uncertain = evidence(1);
        ime_uncertain.ime_commit_verified = false;
        let mut temporary = evidence(2);
        temporary.temporary_intent = true;
        assert_eq!(
            classify_shadow_candidate(
                NOW_MS,
                WritingTaskId::ProjectUpdate,
                WritingPreferenceDimension::ListStructure,
                WritingPreferenceValue::Bullets,
                &[no_author_proof, ime_uncertain, temporary],
            ),
            ShadowDecision::Unknown
        );
    }

    #[test]
    fn counterevidence_blocks_candidate_and_stale_evidence_is_outside_window() {
        let mut counterevidence = evidence(0);
        counterevidence.counterevidence = true;
        assert_eq!(
            classify_shadow_candidate(
                NOW_MS,
                WritingTaskId::ProjectUpdate,
                WritingPreferenceDimension::ListStructure,
                WritingPreferenceValue::Bullets,
                &[evidence(1), evidence(2), evidence(3), counterevidence],
            ),
            ShadowDecision::Unknown
        );

        let mut stale = evidence(1);
        stale.occurred_at_ms = NOW_MS - WINDOW_MS - 1;
        assert_eq!(
            classify_shadow_candidate(
                NOW_MS,
                WritingTaskId::ProjectUpdate,
                WritingPreferenceDimension::ListStructure,
                WritingPreferenceValue::Bullets,
                &[evidence(0), stale],
            ),
            ShadowDecision::BelowThreshold {
                supporting_origins: 1,
                distinct_days: 1,
                distinct_sessions: 1,
            }
        );
    }
}
