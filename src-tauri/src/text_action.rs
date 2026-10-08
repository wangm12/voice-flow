//! Bounded planning and deterministic fidelity checks for selected-text actions.
//!
//! This module has no provider, target-discovery, preview, persistence, or delivery
//! responsibilities. It only turns an explicit instruction into a finite plan
//! and checks a generated candidate against the plan's immutable source evidence.

use crate::protected_span::{self, ProtectedSpanKind};

pub const MAX_ACTION_SOURCE_BYTES: usize = 16 * 1024;
const MAX_INSTRUCTION_BYTES: usize = 4 * 1024;
const MAX_REPLY_CONTEXT_CHARS: usize = crate::screen_text::MAX_CHARS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextActionOperation {
    Rewrite,
    Shorten,
    Translate,
    Organize,
    DraftReply,
    ModifyExact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextActionSourceKind {
    Selection,
    FieldText,
    EmptyComposer,
}

pub struct TextActionInput<'a> {
    pub instruction: &'a str,
    pub source_kind: TextActionSourceKind,
    pub source_text: &'a str,
    pub target_is_empty: bool,
    pub configured_translation_target: Option<&'a str>,
    pub reply_context: Option<&'a str>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct AuthorizedEntityChange {
    pub start_byte: usize,
    pub end_byte: usize,
    pub source_value: String,
    pub replacement_value: String,
}

impl std::fmt::Debug for AuthorizedEntityChange {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthorizedEntityChange")
            .field("start_byte", &self.start_byte)
            .field("end_byte", &self.end_byte)
            .field("source_value_bytes", &self.source_value.len())
            .field("replacement_value_bytes", &self.replacement_value.len())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct TextActionPlan {
    pub operation: TextActionOperation,
    pub target_language: Option<String>,
    pub authorized_changes: Vec<AuthorizedEntityChange>,
    validation_evidence: ActionValidationEvidence,
}

impl std::fmt::Debug for TextActionPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TextActionPlan")
            .field("operation", &self.operation)
            .field("authorized_change_count", &self.authorized_changes.len())
            .field("has_target_language", &self.target_language.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextActionPlanError {
    UnsupportedInstruction,
    AmbiguousInstruction,
    NoSource,
    ReplyTargetMustBeEmpty,
    ReplyContextUnavailable,
    UnauthorizedSourceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextActionGuardError {
    EmptyResult,
    ProtectedFactChanged,
    UnauthorizedEntityChange,
    NegationChanged,
    TranslationEntityUnverifiable,
    UnverifiableFacts,
}

#[derive(Clone, PartialEq, Eq)]
struct ActionValidationEvidence {
    source_snapshot: String,
    instruction_snapshot: String,
    source_kind: TextActionSourceKind,
    reply_context: Option<String>,
    authorized_changes_snapshot: Vec<AuthorizedEntityChange>,
    authorized_change_kinds: Vec<ProtectedSpanKind>,
    operation_snapshot: TextActionOperation,
    target_language_snapshot: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SpanSignature {
    kind: ProtectedSpanKind,
    value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TranslationEntity {
    Exact(ProtectedSpanKind, String),
    Weekday(u8),
    Money {
        currency: &'static str,
        amount: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranslationEntityGroup {
    Weekday,
    Money,
    Exact(ProtectedSpanKind),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocatedTranslationEntity {
    start: usize,
    end: usize,
    entity: TranslationEntity,
}

pub fn plan_text_action(input: TextActionInput<'_>) -> Result<TextActionPlan, TextActionPlanError> {
    if input.instruction.trim().is_empty()
        || input.instruction.len() > MAX_INSTRUCTION_BYTES
        || input.source_text.len() > MAX_ACTION_SOURCE_BYTES
    {
        return Err(TextActionPlanError::UnsupportedInstruction);
    }

    let unquoted_instruction = without_quoted_content(input.instruction);
    let (operation, modification) = detect_operation(&unquoted_instruction, input.instruction)?;

    let mut target_language = None;
    let mut authorized_changes = Vec::new();
    let mut authorized_change_kinds = Vec::new();

    match operation {
        TextActionOperation::DraftReply => {
            if input.source_kind != TextActionSourceKind::EmptyComposer {
                return Err(TextActionPlanError::UnauthorizedSourceKind);
            }
            if !input.target_is_empty || !input.source_text.is_empty() {
                return Err(TextActionPlanError::ReplyTargetMustBeEmpty);
            }
            let reply_context = input
                .reply_context
                .filter(|context| !context.trim().is_empty())
                .ok_or(TextActionPlanError::ReplyContextUnavailable)?;
            if reply_context.chars().count() > MAX_REPLY_CONTEXT_CHARS {
                return Err(TextActionPlanError::ReplyContextUnavailable);
            }
        }
        TextActionOperation::Translate => {
            require_transform_source(&input)?;
            target_language = resolve_target_language(
                &unquoted_instruction,
                input.configured_translation_target,
            )?;
        }
        TextActionOperation::ModifyExact => {
            require_transform_source(&input)?;
            let Some(parsed) = modification else {
                return Err(TextActionPlanError::AmbiguousInstruction);
            };
            let authorization = authorize_modification(input.source_text, parsed)?;
            authorized_changes.push(authorization.change);
            authorized_change_kinds.push(authorization.kind);
        }
        TextActionOperation::Rewrite
        | TextActionOperation::Shorten
        | TextActionOperation::Organize => {
            require_transform_source(&input)?;
        }
    }

    let reply_context = if operation == TextActionOperation::DraftReply {
        input.reply_context.map(str::to_owned)
    } else {
        None
    };
    let validation_evidence = ActionValidationEvidence {
        source_snapshot: input.source_text.to_owned(),
        instruction_snapshot: input.instruction.to_owned(),
        source_kind: input.source_kind,
        reply_context,
        authorized_changes_snapshot: authorized_changes.clone(),
        authorized_change_kinds,
        operation_snapshot: operation,
        target_language_snapshot: target_language.clone(),
    };

    Ok(TextActionPlan {
        operation,
        target_language,
        authorized_changes,
        validation_evidence,
    })
}

pub fn validate_generated_result(
    plan: &TextActionPlan,
    source: &str,
    candidate: &str,
) -> Result<(), TextActionGuardError> {
    let evidence = &plan.validation_evidence;
    if plan.operation != evidence.operation_snapshot
        || plan.target_language != evidence.target_language_snapshot
        || plan.authorized_changes != evidence.authorized_changes_snapshot
        || source != evidence.source_snapshot
    {
        return Err(TextActionGuardError::UnauthorizedEntityChange);
    }
    if candidate.trim().is_empty() {
        return Err(TextActionGuardError::EmptyResult);
    }
    if candidate.len() > MAX_ACTION_SOURCE_BYTES {
        return Err(TextActionGuardError::UnverifiableFacts);
    }

    match plan.operation {
        TextActionOperation::Rewrite
        | TextActionOperation::Shorten
        | TextActionOperation::Organize => {
            validate_preserved_transform(plan.operation, source, candidate)
        }
        TextActionOperation::ModifyExact => validate_exact_modification(plan, source, candidate),
        TextActionOperation::Translate => validate_translation(plan, source, candidate),
        TextActionOperation::DraftReply => validate_reply(plan, candidate),
    }
}

fn require_transform_source(input: &TextActionInput<'_>) -> Result<(), TextActionPlanError> {
    match input.source_kind {
        TextActionSourceKind::Selection | TextActionSourceKind::FieldText => {}
        TextActionSourceKind::EmptyComposer => {
            return Err(TextActionPlanError::UnauthorizedSourceKind)
        }
    }
    if input.source_text.trim().is_empty() {
        return Err(TextActionPlanError::NoSource);
    }
    Ok(())
}

fn detect_operation(
    unquoted_instruction: &str,
    original_instruction: &str,
) -> Result<(TextActionOperation, Option<ParsedModification>), TextActionPlanError> {
    let rewrite = has_any_cue(
        unquoted_instruction,
        &[
            "rewrite",
            "rephrase",
            "polish",
            "改写",
            "重写",
            "润色",
            "改措辞",
        ],
    );
    let shorten = has_any_cue(
        unquoted_instruction,
        &[
            "shorten",
            "condense",
            "make it shorter",
            "make this shorter",
            "make it concise",
            "make this concise",
            "more concise",
            "be concise",
            "缩短",
            "精简",
            "简洁一点",
            "更简短",
        ],
    );
    let translate = has_any_cue(unquoted_instruction, &["translate", "translation", "翻译"]);
    let organize = has_any_cue(
        unquoted_instruction,
        &[
            "organize",
            "structure",
            "format",
            "outline",
            "bullet points",
            "into bullets",
            "整理",
            "结构化",
            "列成要点",
            "列出要点",
            "整理成列表",
        ],
    );
    let draft_reply = has_any_cue(
        unquoted_instruction,
        &[
            "draft a reply",
            "write a reply",
            "draft reply",
            "draft a response",
            "write a response",
            "reply to this",
            "起草回复",
            "草拟回复",
            "帮我回复",
            "回复对方",
        ],
    );

    let (modification, has_modify_cue) = parse_modification(original_instruction);
    let has_operation_cue = rewrite || shorten || translate || organize || draft_reply;
    if modification.is_some() {
        if has_operation_cue {
            return Err(TextActionPlanError::AmbiguousInstruction);
        }
        return Ok((TextActionOperation::ModifyExact, modification));
    }
    if has_modify_cue {
        return Err(TextActionPlanError::AmbiguousInstruction);
    }

    let recognized = [
        (rewrite, TextActionOperation::Rewrite),
        (shorten, TextActionOperation::Shorten),
        (translate, TextActionOperation::Translate),
        (organize, TextActionOperation::Organize),
        (draft_reply, TextActionOperation::DraftReply),
    ]
    .into_iter()
    .filter_map(|(present, operation)| present.then_some(operation))
    .collect::<Vec<_>>();
    match recognized.as_slice() {
        [operation] => Ok((*operation, None)),
        [] => Err(TextActionPlanError::UnsupportedInstruction),
        _ => Err(TextActionPlanError::AmbiguousInstruction),
    }
}

fn has_any_cue(text: &str, cues: &[&str]) -> bool {
    cues.iter().any(|cue| {
        if cue.is_ascii() {
            contains_ascii_phrase(text, cue)
        } else {
            text.contains(cue)
        }
    })
}

fn contains_ascii_phrase(text: &str, phrase: &str) -> bool {
    count_ascii_phrase(text, phrase) > 0
}

fn count_ascii_phrase(text: &str, phrase: &str) -> usize {
    let folded = text.to_ascii_lowercase();
    let phrase = phrase.to_ascii_lowercase();
    folded
        .match_indices(&phrase)
        .filter(|(start, _)| {
            let end = start + phrase.len();
            let before_ok = *start == 0
                || !folded[..*start]
                    .chars()
                    .next_back()
                    .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_');
            let after_ok = end == folded.len()
                || !folded[end..]
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_');
            before_ok && after_ok
        })
        .count()
}

#[derive(Debug)]
struct ParsedModification {
    old_value: String,
    new_value: String,
}

fn parse_modification(instruction: &str) -> (Option<ParsedModification>, bool) {
    let directive = without_quoted_content(instruction);
    let english_replace = contains_ascii_phrase(&directive, "replace");
    let english_change =
        contains_ascii_phrase(&directive, "change") || contains_ascii_phrase(&directive, "update");
    let chinese_edit = ["改成", "改为", "改為", "替换为", "替換為", "换成", "換成"]
        .iter()
        .any(|cue| directive.contains(cue));
    let has_modify_cue = english_replace || english_change || chinese_edit;
    if !has_modify_cue {
        return (None, false);
    }
    // Exact entity changes require an affirmative instruction. A negated edit
    // cue is ambiguous authorization, even if its old/new values parse cleanly.
    // Quoted text was removed above, so a quoted example does not revoke an
    // otherwise affirmative instruction or authorize a change by itself.
    if !negation_signature(&directive).is_empty() {
        return (None, true);
    }
    if (english_replace && english_change)
        || (chinese_edit && (english_replace || english_change))
        || ["replace", "change", "update"]
            .iter()
            .map(|cue| count_ascii_phrase(&directive, cue))
            .sum::<usize>()
            > 1
    {
        return (None, true);
    }

    let parsed =
        parse_english_modification(instruction).or_else(|| parse_chinese_modification(instruction));
    (parsed, true)
}

fn parse_english_modification(instruction: &str) -> Option<ParsedModification> {
    let lower = instruction.to_ascii_lowercase();
    let replace_start = find_ascii_word_unquoted(instruction, &lower, "replace")
        .map(|index| index + "replace".len());
    let change_start = find_ascii_word_unquoted(instruction, &lower, "change")
        .map(|index| index + "change".len())
        .or_else(|| {
            find_ascii_word_unquoted(instruction, &lower, "update")
                .map(|index| index + "update".len())
        });

    if let Some(start) = replace_start {
        let remainder = &instruction[start..];
        let lower_remainder = remainder.to_ascii_lowercase();
        let delimiters = [" with ", " by "];
        let matches = delimiters
            .iter()
            .filter_map(|delimiter| {
                lower_remainder
                    .find(delimiter)
                    .map(|index| (index, delimiter.len()))
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return None;
        }
        let (split, delimiter_len) = matches[0];
        let old_value = sole_edit_entity(&remainder[..split])?;
        let new_value = sole_edit_entity(&remainder[split + delimiter_len..])?;
        return Some(ParsedModification {
            old_value,
            new_value,
        });
    }

    if let Some(start) = change_start {
        let remainder = &instruction[start..];
        let lower_remainder = remainder.to_ascii_lowercase();
        let delimiter = " to ";
        let mut splits = lower_remainder.match_indices(delimiter);
        let (split, _) = splits.next()?;
        if splits.next().is_some() {
            return None;
        }
        let left = remainder[..split]
            .rsplit_once(" from ")
            .map(|(_, tail)| tail)
            .unwrap_or(&remainder[..split]);
        let old_value = sole_edit_entity(left)?;
        let new_value = sole_edit_entity(&remainder[split + delimiter.len()..])?;
        return Some(ParsedModification {
            old_value,
            new_value,
        });
    }

    None
}

fn parse_chinese_modification(instruction: &str) -> Option<ParsedModification> {
    let mut matches = Vec::new();
    for delimiter in ["改成", "改为", "改為", "替换为", "替換為", "换成", "換成"] {
        matches.extend(
            instruction
                .match_indices(delimiter)
                .filter(|(index, _)| !is_inside_quote(instruction, *index))
                .map(|(index, _)| (index, delimiter.len())),
        );
    }
    if matches.len() != 1 {
        return None;
    }
    let (split, delimiter_len) = matches[0];
    let left = &instruction[..split];
    let right = &instruction[split + delimiter_len..];
    Some(ParsedModification {
        old_value: sole_edit_entity(left)?,
        new_value: sole_edit_entity(right)?,
    })
}

fn find_ascii_word_unquoted(original: &str, folded: &str, needle: &str) -> Option<usize> {
    let mut search_from = 0;
    while let Some(relative) = folded[search_from..].find(needle) {
        let start = search_from + relative;
        let end = start + needle.len();
        let before_ok = start == 0
            || !folded[..start]
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_');
        let after_ok = end == folded.len()
            || !folded[end..]
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_');
        if before_ok && after_ok && !is_inside_quote(original, start) {
            return Some(start);
        }
        search_from = end;
        if search_from >= folded.len() {
            break;
        }
    }
    None
}

fn is_inside_quote(text: &str, target_byte: usize) -> bool {
    let chars = text.char_indices().collect::<Vec<_>>();
    let mut quote = None;
    for (index, (byte, ch)) in chars.iter().enumerate() {
        if *byte >= target_byte {
            return quote.is_some();
        }
        if let Some(open) = quote {
            if *ch == matching_quote(open) {
                quote = None;
            }
        } else if is_quote_open(text, &chars, index, *ch) {
            quote = Some(*ch);
        }
    }
    false
}

fn sole_edit_entity(fragment: &str) -> Option<String> {
    let spans = protected_span::protected_spans(fragment, None)
        .into_iter()
        .filter(|span| {
            matches!(
                span.kind,
                ProtectedSpanKind::Amount
                    | ProtectedSpanKind::Number
                    | ProtectedSpanKind::Version
                    | ProtectedSpanKind::Term
            )
        })
        .collect::<Vec<_>>();
    if spans.len() != 1 {
        return None;
    }
    Some(fragment[spans[0].start_byte..spans[0].end_byte].to_owned())
}

struct AuthorizedModification {
    change: AuthorizedEntityChange,
    kind: ProtectedSpanKind,
}

fn authorize_modification(
    source: &str,
    parsed: ParsedModification,
) -> Result<AuthorizedModification, TextActionPlanError> {
    if parsed.old_value.is_empty()
        || parsed.new_value.is_empty()
        || parsed.old_value == parsed.new_value
    {
        return Err(TextActionPlanError::AmbiguousInstruction);
    }
    let old_matches = protected_span::protected_spans(source, None)
        .into_iter()
        .filter(|span| {
            matches!(
                span.kind,
                ProtectedSpanKind::Amount
                    | ProtectedSpanKind::Number
                    | ProtectedSpanKind::Version
                    | ProtectedSpanKind::Term
            ) && source[span.start_byte..span.end_byte] == parsed.old_value
        })
        .collect::<Vec<_>>();
    if old_matches.len() != 1 {
        return Err(TextActionPlanError::AmbiguousInstruction);
    }
    let old_span = old_matches[0];
    let replacement_spans = protected_span::protected_spans(&parsed.new_value, None)
        .into_iter()
        .filter(|span| {
            span.start_byte == 0
                && span.end_byte == parsed.new_value.len()
                && span.kind == old_span.kind
        })
        .collect::<Vec<_>>();
    if replacement_spans.len() != 1 {
        return Err(TextActionPlanError::AmbiguousInstruction);
    }
    let existing_replacements = protected_span::protected_spans(source, None)
        .into_iter()
        .filter(|span| {
            span.kind == old_span.kind && source[span.start_byte..span.end_byte] == parsed.new_value
        })
        .count();
    if existing_replacements > 0 {
        return Err(TextActionPlanError::AmbiguousInstruction);
    }
    Ok(AuthorizedModification {
        change: AuthorizedEntityChange {
            start_byte: old_span.start_byte,
            end_byte: old_span.end_byte,
            source_value: parsed.old_value,
            replacement_value: parsed.new_value,
        },
        kind: old_span.kind,
    })
}

fn resolve_target_language(
    instruction: &str,
    configured_target: Option<&str>,
) -> Result<Option<String>, TextActionPlanError> {
    let explicit = explicit_target_language(instruction)?;
    let configured = if explicit.is_none() {
        configured_target
            .map(normalize_language)
            .transpose()?
            .flatten()
    } else {
        None
    };
    match (explicit, configured) {
        (Some(explicit), Some(configured)) if explicit != configured => Ok(Some(explicit)),
        (Some(explicit), _) => Ok(Some(explicit)),
        (None, Some(configured)) => Ok(Some(configured)),
        (None, None) => Err(TextActionPlanError::AmbiguousInstruction),
    }
}

fn explicit_target_language(instruction: &str) -> Result<Option<String>, TextActionPlanError> {
    const TARGET_CUES: &[&str] = &[" into ", " to ", "成", "为", "到"];
    // Cues are ASCII/Chinese and byte offsets from this folded copy index the
    // original instruction, so use length-preserving ASCII folding.
    let folded = instruction.to_ascii_lowercase();
    let aliases = language_aliases();
    let mut found = Vec::<String>::new();
    for cue in TARGET_CUES {
        for (cue_start, _) in folded.match_indices(cue) {
            let value_start = cue_start + cue.len();
            let suffix = &instruction[value_start..];
            if let Some((_, canonical)) = aliases.iter().find(|(alias, _)| {
                let alias_chars = alias.chars().count();
                let alias_end = suffix
                    .char_indices()
                    .nth(alias_chars)
                    .map_or(suffix.len(), |(byte, _)| byte);
                suffix[..alias_end].to_lowercase() == **alias
                    && suffix[alias_end..]
                        .chars()
                        .next()
                        .is_none_or(|ch| !ch.is_ascii_alphanumeric() && ch != '-')
            }) {
                if !found.iter().any(|existing| existing == canonical) {
                    found.push((*canonical).to_owned());
                }
            }
        }
    }
    if found.len() > 1 {
        return Err(TextActionPlanError::AmbiguousInstruction);
    }
    if let Some(language) = found.pop() {
        return Ok(Some(language));
    }
    if TARGET_CUES.iter().any(|cue| folded.contains(cue))
        || ["成", "为", "到"].iter().any(|cue| folded.contains(cue))
    {
        return Err(TextActionPlanError::AmbiguousInstruction);
    }
    Ok(None)
}

fn normalize_language(value: &str) -> Result<Option<String>, TextActionPlanError> {
    let normalized = value
        .trim()
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`' | '“' | '”' | '‘' | '’'))
        .to_lowercase();
    let aliases = language_aliases();
    aliases
        .iter()
        .find(|(alias, _)| *alias == normalized)
        .map(|(_, canonical)| Some((*canonical).to_owned()))
        .ok_or(TextActionPlanError::AmbiguousInstruction)
}

fn language_aliases() -> Vec<(&'static str, &'static str)> {
    vec![
        ("simplified chinese", "Chinese"),
        ("traditional chinese", "Chinese"),
        ("zh-hans", "Chinese"),
        ("zh-hant", "Chinese"),
        ("zh-cn", "Chinese"),
        ("zh-tw", "Chinese"),
        ("mandarin", "Chinese"),
        ("zh", "Chinese"),
        ("chinese", "Chinese"),
        ("中文", "Chinese"),
        ("简体中文", "Chinese"),
        ("繁體中文", "Chinese"),
        ("繁体中文", "Chinese"),
        ("普通话", "Chinese"),
        ("english", "English"),
        ("en-us", "English"),
        ("en-gb", "English"),
        ("en", "English"),
        ("英语", "English"),
        ("英文", "English"),
        ("spanish", "Spanish"),
        ("es", "Spanish"),
        ("español", "Spanish"),
        ("espanol", "Spanish"),
        ("西班牙语", "Spanish"),
        ("french", "French"),
        ("fr-fr", "French"),
        ("fr", "French"),
        ("français", "French"),
        ("francais", "French"),
        ("法语", "French"),
        ("german", "German"),
        ("de-de", "German"),
        ("de", "German"),
        ("deutsch", "German"),
        ("德语", "German"),
        ("japanese", "Japanese"),
        ("ja-jp", "Japanese"),
        ("ja", "Japanese"),
        ("日本語", "Japanese"),
        ("日语", "Japanese"),
        ("korean", "Korean"),
        ("ko-kr", "Korean"),
        ("ko", "Korean"),
        ("한국어", "Korean"),
        ("韩语", "Korean"),
    ]
}

fn without_quoted_content(text: &str) -> String {
    let chars = text.char_indices().collect::<Vec<_>>();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    let mut quote: Option<char> = None;
    while index < chars.len() {
        let (byte, ch) = chars[index];
        if let Some(open) = quote {
            let close = matching_quote(open);
            if ch == close {
                quote = None;
                output.push(' ');
            }
            index += 1;
            continue;
        }
        if is_quote_open(text, &chars, index, ch) {
            quote = Some(ch);
            output.push(' ');
            index += 1;
            continue;
        }
        let end = chars.get(index + 1).map_or(text.len(), |(next, _)| *next);
        output.push_str(&text[byte..end]);
        index += 1;
    }
    output
}

fn is_quote_open(text: &str, chars: &[(usize, char)], index: usize, ch: char) -> bool {
    if matches!(ch, '"' | '`' | '“' | '‘' | '「' | '『') {
        return true;
    }
    if ch != '\'' {
        return false;
    }
    let previous = index
        .checked_sub(1)
        .and_then(|i| chars.get(i))
        .map(|(_, c)| *c);
    let next = chars.get(index + 1).map(|(_, c)| *c);
    let Some(next) = next else {
        return false;
    };
    if previous.is_some_and(char::is_alphanumeric) && next.is_alphanumeric() {
        return false;
    }
    let byte = chars[index].0 + ch.len_utf8();
    // A lone quote pair around an edit phrase is instruction data, while a
    // contraction such as don't has alphanumeric characters on both sides.
    text[byte..]
        .char_indices()
        .any(|(_, candidate)| candidate == '\'')
}

fn matching_quote(open: char) -> char {
    match open {
        '“' => '”',
        '‘' => '’',
        '「' => '」',
        '『' => '』',
        other => other,
    }
}

fn validate_preserved_transform(
    operation: TextActionOperation,
    source: &str,
    candidate: &str,
) -> Result<(), TextActionGuardError> {
    if negation_signature(source) != negation_signature(candidate) {
        return Err(TextActionGuardError::NegationChanged);
    }
    if transform_span_signatures(source, operation)
        != transform_span_signatures(candidate, operation)
    {
        return Err(TextActionGuardError::ProtectedFactChanged);
    }
    Ok(())
}

/// Sentence-initial words can be ordinary action language even though the
/// shared protected-span detector conservatively tags capitalized tokens as
/// possible names. Ignore only a small verb/modal set in rewrite, shorten, and
/// organize actions, and only when the following words support that grammar.
/// Names and technical terms elsewhere remain exact protected facts.
fn transform_span_signatures(text: &str, operation: TextActionOperation) -> Vec<SpanSignature> {
    protected_span::protected_spans(text, None)
        .into_iter()
        .filter(|span| !is_ordinary_action_lead(text, *span, operation))
        .map(|span| SpanSignature {
            kind: span.kind,
            value: text[span.start_byte..span.end_byte].to_owned(),
        })
        .collect()
}

fn is_ordinary_action_lead(
    text: &str,
    span: protected_span::ProtectedSpan,
    operation: TextActionOperation,
) -> bool {
    if span.kind != ProtectedSpanKind::Term
        || !matches!(
            operation,
            TextActionOperation::Rewrite
                | TextActionOperation::Shorten
                | TextActionOperation::Organize
        )
    {
        return false;
    }

    let token = &text[span.start_byte..span.end_byte];
    let folded = token.to_ascii_lowercase();
    let next_words = text[span.end_byte..]
        .split(|ch: char| !ch.is_ascii_alphabetic())
        .filter(|word| !word.is_empty())
        .take(2)
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    let starts_clause = action_clause_prefix(text, span.start_byte, operation);

    if !starts_clause {
        return false;
    }

    match folded.as_str() {
        "can" | "could" | "would" | "will" | "should" => next_words
            .first()
            .is_some_and(|word| matches!(word.as_str(), "you" | "we" | "i" | "they")),
        "please" => next_words.first().is_some_and(|word| {
            matches!(
                word.as_str(),
                "check"
                    | "send"
                    | "share"
                    | "review"
                    | "confirm"
                    | "tell"
                    | "forward"
                    | "provide"
                    | "attach"
                    | "include"
                    | "draft"
            )
        }),
        "check" | "share" | "forward" | "provide" | "attach" | "include" => {
            next_words.first().is_some_and(|word| {
                matches!(
                    word.as_str(),
                    "the"
                        | "a"
                        | "an"
                        | "this"
                        | "these"
                        | "my"
                        | "our"
                        | "your"
                        | "his"
                        | "her"
                        | "their"
                        | "me"
                        | "us"
                        | "them"
                        | "it"
                )
            })
        }
        "send" | "tell" => next_words.first().is_some_and(|word| {
            matches!(
                word.as_str(),
                "me" | "us"
                    | "him"
                    | "her"
                    | "them"
                    | "it"
                    | "the"
                    | "a"
                    | "an"
                    | "this"
                    | "these"
                    | "my"
                    | "our"
                    | "your"
            )
        }),
        "review" | "confirm" => next_words.first().is_some_and(|word| {
            matches!(
                word.as_str(),
                "the"
                    | "a"
                    | "an"
                    | "this"
                    | "these"
                    | "my"
                    | "our"
                    | "your"
                    | "his"
                    | "her"
                    | "their"
            )
        }),
        _ => false,
    }
}

fn action_clause_prefix(text: &str, token_start: usize, operation: TextActionOperation) -> bool {
    let prefix = &text[..token_start];
    let boundary = prefix
        .char_indices()
        .rev()
        .find(|(_, ch)| {
            matches!(ch, '.' | '!' | '?' | '。' | '！' | '？' | '\n' | '\r')
                || (operation == TextActionOperation::Organize && matches!(ch, ';' | '；'))
        })
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    let mut clause_prefix = prefix[boundary..].trim_start();
    if operation == TextActionOperation::Organize {
        if let Some(rest) = clause_prefix.strip_prefix(['-', '*', '•']) {
            clause_prefix = rest.trim_start();
        }
    }
    if clause_prefix
        .get(.."please".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("please"))
    {
        clause_prefix = clause_prefix["please".len()..].trim_start();
    }
    clause_prefix.is_empty()
}

fn validate_exact_modification(
    plan: &TextActionPlan,
    source: &str,
    candidate: &str,
) -> Result<(), TextActionGuardError> {
    let evidence = &plan.validation_evidence;
    let [change] = plan.authorized_changes.as_slice() else {
        return Err(TextActionGuardError::UnauthorizedEntityChange);
    };
    let [kind] = evidence.authorized_change_kinds.as_slice() else {
        return Err(TextActionGuardError::UnauthorizedEntityChange);
    };
    if change.start_byte >= change.end_byte
        || !source.is_char_boundary(change.start_byte)
        || !source.is_char_boundary(change.end_byte)
        || source.get(change.start_byte..change.end_byte) != Some(change.source_value.as_str())
    {
        return Err(TextActionGuardError::UnauthorizedEntityChange);
    }
    let source_spans = span_signatures(source);
    let Some(source_index) = source_spans
        .iter()
        .position(|span| span.kind == *kind && span.value == change.source_value)
    else {
        return Err(TextActionGuardError::UnauthorizedEntityChange);
    };
    if source_spans
        .iter()
        .filter(|span| span.kind == *kind && span.value == change.source_value)
        .count()
        != 1
    {
        return Err(TextActionGuardError::UnauthorizedEntityChange);
    }
    let replacement = protected_span::protected_spans(&change.replacement_value, None)
        .into_iter()
        .filter(|span| {
            span.start_byte == 0
                && span.end_byte == change.replacement_value.len()
                && span.kind == *kind
        })
        .count();
    if replacement != 1 {
        return Err(TextActionGuardError::UnauthorizedEntityChange);
    }

    if negation_signature(source) != negation_signature(candidate) {
        return Err(TextActionGuardError::NegationChanged);
    }
    let mut expected = source_spans;
    expected[source_index].value = change.replacement_value.clone();
    if expected != span_signatures(candidate) {
        return Err(TextActionGuardError::UnauthorizedEntityChange);
    }
    Ok(())
}

fn validate_translation(
    plan: &TextActionPlan,
    source: &str,
    candidate: &str,
) -> Result<(), TextActionGuardError> {
    if plan.target_language.as_deref().is_none_or(str::is_empty) {
        return Err(TextActionGuardError::TranslationEntityUnverifiable);
    }
    if source.trim() == candidate.trim() {
        return Err(TextActionGuardError::TranslationEntityUnverifiable);
    }
    let target_language = plan
        .target_language
        .as_deref()
        .ok_or(TextActionGuardError::TranslationEntityUnverifiable)?;
    if negation_signature(source) != negation_signature(candidate) {
        return Err(TextActionGuardError::NegationChanged);
    }
    let source_entities = translation_entities(source, None, None)?;
    let candidate_entities = translation_entities(candidate, Some(target_language), Some(source))?;
    // Translation changes byte offsets and can change the order of different
    // fact classes for grammar. Compare semantic facts by typed class, keeping
    // order and multiplicity within each class so same-kind facts cannot swap.
    if !translation_entities_match(&source_entities, &candidate_entities) {
        return Err(TextActionGuardError::ProtectedFactChanged);
    }
    Ok(())
}

fn translation_entities_match(
    source: &[LocatedTranslationEntity],
    candidate: &[LocatedTranslationEntity],
) -> bool {
    const GROUPS: &[TranslationEntityGroup] = &[
        TranslationEntityGroup::Weekday,
        TranslationEntityGroup::Money,
        TranslationEntityGroup::Exact(ProtectedSpanKind::Url),
        TranslationEntityGroup::Exact(ProtectedSpanKind::Email),
        TranslationEntityGroup::Exact(ProtectedSpanKind::Path),
        TranslationEntityGroup::Exact(ProtectedSpanKind::Command),
        TranslationEntityGroup::Exact(ProtectedSpanKind::Flag),
        TranslationEntityGroup::Exact(ProtectedSpanKind::Date),
        TranslationEntityGroup::Exact(ProtectedSpanKind::DateWord),
        TranslationEntityGroup::Exact(ProtectedSpanKind::Amount),
        TranslationEntityGroup::Exact(ProtectedSpanKind::Version),
        TranslationEntityGroup::Exact(ProtectedSpanKind::Number),
        TranslationEntityGroup::Exact(ProtectedSpanKind::Term),
        TranslationEntityGroup::Exact(ProtectedSpanKind::DeicticReference),
    ];
    GROUPS.iter().all(|group| {
        source
            .iter()
            .filter(|located| translation_entity_group(&located.entity) == *group)
            .map(|located| &located.entity)
            .eq(candidate
                .iter()
                .filter(|located| translation_entity_group(&located.entity) == *group)
                .map(|located| &located.entity))
    })
}

fn translation_entity_group(entity: &TranslationEntity) -> TranslationEntityGroup {
    match entity {
        TranslationEntity::Weekday(_) => TranslationEntityGroup::Weekday,
        TranslationEntity::Money { .. } => TranslationEntityGroup::Money,
        TranslationEntity::Exact(kind, _) => TranslationEntityGroup::Exact(*kind),
    }
}

fn span_signatures(text: &str) -> Vec<SpanSignature> {
    protected_span::protected_spans(text, None)
        .into_iter()
        .map(|span| SpanSignature {
            kind: span.kind,
            value: text[span.start_byte..span.end_byte].to_owned(),
        })
        .collect()
}

fn validate_reply(plan: &TextActionPlan, candidate: &str) -> Result<(), TextActionGuardError> {
    let evidence = &plan.validation_evidence;
    if evidence.source_kind != TextActionSourceKind::EmptyComposer
        || !evidence.source_snapshot.is_empty()
    {
        return Err(TextActionGuardError::UnauthorizedEntityChange);
    }
    let Some(context) = evidence.reply_context.as_deref() else {
        return Err(TextActionGuardError::UnverifiableFacts);
    };
    let mut allowed = span_signatures(context);
    let assertion = explicit_reply_assertion(&evidence.instruction_snapshot);
    if let Some(assertion) = assertion.as_deref() {
        allowed.extend(span_signatures(assertion));
    }
    let candidate_spans = reply_fact_signatures(candidate);
    if !is_multiset_subset(&candidate_spans, &allowed) {
        return Err(TextActionGuardError::ProtectedFactChanged);
    }
    if assertion
        .as_deref()
        .is_some_and(|assertion| !reply_assertion_facts_are_bound(assertion, &candidate_spans))
    {
        return Err(TextActionGuardError::ProtectedFactChanged);
    }
    let allowed_negations = negation_signature(context).len()
        + assertion
            .as_deref()
            .map(negation_signature)
            .unwrap_or_default()
            .len();
    if negation_signature(candidate).len() > allowed_negations {
        return Err(TextActionGuardError::UnverifiableFacts);
    }
    if !is_bounded_reply_shell(candidate, assertion.as_deref()) {
        return Err(TextActionGuardError::UnverifiableFacts);
    }
    Ok(())
}

fn reply_fact_signatures(text: &str) -> Vec<SpanSignature> {
    protected_span::protected_spans(text, None)
        .into_iter()
        .filter(|span| !is_supported_reply_opening_entity(text, *span))
        .map(|span| SpanSignature {
            kind: span.kind,
            value: text[span.start_byte..span.end_byte].to_owned(),
        })
        .collect()
}

/// Facts explicitly asserted by the user must remain bound to that assertion.
/// Other context can add supported facts, but it cannot substitute a different
/// value of the same typed kind (for example, Monday for an asserted Friday).
fn reply_assertion_facts_are_bound(assertion: &str, candidate: &[SpanSignature]) -> bool {
    let expected = reply_fact_signatures(assertion);
    let mut kinds = Vec::new();
    for fact in &expected {
        if !kinds.contains(&fact.kind) {
            kinds.push(fact.kind);
        }
    }
    kinds.into_iter().all(|kind| {
        expected
            .iter()
            .filter(|fact| fact.kind == kind)
            .eq(candidate.iter().filter(|fact| fact.kind == kind))
    })
}

fn is_supported_reply_opening_entity(text: &str, span: protected_span::ProtectedSpan) -> bool {
    if span.kind != ProtectedSpanKind::Term {
        return false;
    }

    const OPENERS: &[&str] = &[
        "thanks",
        "thank you",
        "sure",
        "got it",
        "understood",
        "sounds good",
        "happy to help",
    ];
    let prefix = &text[..span.start_byte];
    let sentence_start = prefix
        .char_indices()
        .rev()
        .find(|(_, ch)| matches!(ch, '.' | '!' | '?' | '。' | '！' | '？' | '\n' | '\r'))
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    let sentence = &text[sentence_start..];
    let trimmed_sentence = sentence.trim_start();
    let leading_space_bytes = sentence.len() - trimmed_sentence.len();
    let Some(opener_start) = sentence_start.checked_add(leading_space_bytes) else {
        return false;
    };
    if span.start_byte != opener_start {
        return false;
    }
    OPENERS.iter().any(|opener| {
        trimmed_sentence
            .get(..opener.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(opener))
            && trimmed_sentence
                .get(opener.len()..)
                .and_then(|suffix| suffix.chars().next())
                .is_none_or(|next| next.is_whitespace() || matches!(next, ',' | '.' | '!' | '?'))
            && span.end_byte.checked_sub(span.start_byte) == Some(opener.len())
    })
}

fn explicit_reply_assertion(instruction: &str) -> Option<String> {
    let unquoted = without_quoted_content(instruction);
    let lower = unquoted.to_ascii_lowercase();
    let markers = [
        "tell them that ",
        "say that ",
        "reply that ",
        "let them know that ",
        "mention that ",
        "告诉对方说",
        "回复对方说",
        "回复说",
    ];
    for marker in markers {
        let found = if marker.is_ascii() {
            lower.find(marker).map(|index| index + marker.len())
        } else {
            unquoted.find(marker).map(|index| index + marker.len())
        };
        if let Some(start) = found {
            let tail = unquoted[start..].trim();
            if !tail.is_empty() && negation_signature(tail).is_empty() {
                return Some(tail.to_owned());
            }
        }
    }
    None
}

fn is_bounded_reply_shell(candidate: &str, assertion: Option<&str>) -> bool {
    const ALLOWED: &[&str] = &[
        "thanks",
        "thank you",
        "thanks for sharing",
        "thank you for sharing",
        "got it",
        "understood",
        "sounds good",
        "thanks i'll review and get back to you",
        "thank you i'll review and get back to you",
        "thanks i will review and get back to you",
        "thank you i will review and get back to you",
        "i'll review and get back to you",
        "i will review and get back to you",
        "i'll take a look and follow up",
        "i will take a look and follow up",
        "i'll check and follow up",
        "i will check and follow up",
        "let me check and get back to you",
        "happy to help",
        "谢谢",
        "感谢",
        "收到",
        "明白",
        "好的",
        "没问题",
        "我会查看并跟进",
        "我会确认后回复",
        "我再跟进",
        "请告诉我",
        "gracias",
        "entendido",
        "lo revisaré y te respondo",
        "merci",
        "bien reçu",
        "je vais vérifier et revenir vers vous",
    ];
    let assertion_shell =
        assertion.map(|value| normalize_reply_shell(&remove_protected_spans(value)));
    let pieces = candidate
        .split(['.', '!', '?', '。', '！', '？', '\n', '\r'])
        .map(|piece| (piece, normalize_reply_shell(&remove_protected_spans(piece))))
        .filter(|(_, normalized)| !normalized.is_empty())
        .collect::<Vec<_>>();
    !pieces.is_empty()
        && pieces.iter().all(|(original, piece)| {
            ALLOWED
                .iter()
                .any(|allowed| piece == &normalize_reply_shell(allowed))
                || assertion_shell.as_ref().is_some_and(|assertion| {
                    !assertion.is_empty()
                        && reply_assertion_shell_matches(original, piece, assertion)
                })
        })
}

fn reply_assertion_shell_matches(
    original_piece: &str,
    normalized_piece: &str,
    assertion_shell: &str,
) -> bool {
    let shell = normalized_piece
        .strip_prefix("sure ")
        .unwrap_or(normalized_piece);
    shell == assertion_shell
        || (shell.strip_suffix(" on") == Some(assertion_shell)
            && has_trailing_weekday_entity(original_piece))
}

fn has_trailing_weekday_entity(text: &str) -> bool {
    protected_span::protected_spans(text, None)
        .into_iter()
        .any(|span| {
            span.kind == ProtectedSpanKind::DateWord
                && text[span.end_byte..]
                    .trim()
                    .trim_matches(|ch| matches!(ch, '.' | '!' | '?' | '。' | '！' | '？'))
                    .is_empty()
        })
}

fn remove_protected_spans(text: &str) -> String {
    let spans = protected_span::protected_spans(text, None);
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    for span in spans {
        output.push_str(&text[cursor..span.start_byte]);
        output.push(' ');
        cursor = span.end_byte;
    }
    output.push_str(&text[cursor..]);
    output
}

fn normalize_reply_shell(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|ch| ch.is_alphanumeric() || ch.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_multiset_subset<T: Eq>(subset: &[T], superset: &[T]) -> bool {
    let mut matched = vec![false; superset.len()];
    subset.iter().all(|value| {
        if let Some(index) = superset
            .iter()
            .enumerate()
            .position(|(index, candidate)| !matched[index] && candidate == value)
        {
            matched[index] = true;
            true
        } else {
            false
        }
    })
}

fn translation_entities(
    text: &str,
    target_language: Option<&str>,
    source_text: Option<&str>,
) -> Result<Vec<LocatedTranslationEntity>, TextActionGuardError> {
    let spans = protected_span::protected_spans(text, None);
    let money_ranges = money_ranges(text, &spans);
    let weekday_ranges = weekday_ranges(text);
    let source_terms = source_text.map_or_else(Vec::new, |source| {
        protected_span::protected_spans(source, None)
            .into_iter()
            .filter(|span| span.kind == ProtectedSpanKind::Term)
            .map(|span| source[span.start_byte..span.end_byte].to_owned())
            .collect::<Vec<_>>()
    });
    let mut entities = Vec::new();
    let mut represented_weekdays = Vec::new();
    for span in spans {
        if money_ranges
            .iter()
            .any(|(start, end)| span.start_byte < *end && *start < span.end_byte)
        {
            continue;
        }
        let value = &text[span.start_byte..span.end_byte];
        if let Some(day) = weekday_day(value) {
            represented_weekdays.push((span.start_byte, span.end_byte));
            entities.push(LocatedTranslationEntity {
                start: span.start_byte,
                end: span.end_byte,
                entity: TranslationEntity::Weekday(day),
            });
            continue;
        }
        if span.kind == ProtectedSpanKind::Term && is_sentence_initial(text, span.start_byte) {
            let source_function_word =
                target_language.is_none() && is_source_initial_function_word(value);
            let target_function_word = target_language
                .is_some_and(|language| is_translation_function_word(value, language))
                && !source_terms.iter().any(|term| term == value);
            if source_function_word || target_function_word {
                continue;
            }
        }
        entities.push(LocatedTranslationEntity {
            start: span.start_byte,
            end: span.end_byte,
            entity: TranslationEntity::Exact(span.kind, value.to_owned()),
        });
    }
    for (start, end) in &money_ranges {
        let (currency, amount) = money_signature(&text[*start..*end])
            .ok_or(TextActionGuardError::TranslationEntityUnverifiable)?;
        entities.push(LocatedTranslationEntity {
            start: *start,
            end: *end,
            entity: TranslationEntity::Money { currency, amount },
        });
    }
    for (start, end, day) in weekday_ranges {
        if represented_weekdays
            .iter()
            .any(|(known_start, known_end)| *known_start == start && *known_end == end)
        {
            continue;
        }
        let covered_by_typed_entity = protected_span::protected_spans(text, None)
            .iter()
            .any(|span| start < span.end_byte && span.start_byte < end);
        if covered_by_typed_entity {
            continue;
        }
        entities.push(LocatedTranslationEntity {
            start,
            end,
            entity: TranslationEntity::Weekday(day),
        });
    }
    entities.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then_with(|| left.end.cmp(&right.end))
    });
    Ok(entities)
}

fn is_translation_function_word(value: &str, language: &str) -> bool {
    let value = value.to_lowercase();
    match language {
        "English" => [
            "a", "an", "the", "and", "but", "or", "so", "if", "in", "on", "at", "by", "for",
            "from", "to", "of", "with", "as", "is", "are", "was", "were", "be", "been", "being",
            "have", "has", "had", "i", "you", "he", "she", "it", "we", "they", "this", "that",
            "these", "those", "do", "does", "did", "can", "could", "will", "would", "should",
            "must", "use", "run", "release", "budget", "version", "ship", "meet", "review",
            "thanks", "hello", "no", "not", "update", "replace", "change",
        ]
        .contains(&value.as_str()),
        "Spanish" => [
            "el", "la", "los", "las", "un", "una", "unos", "unas", "y", "pero", "no", "al", "del",
            "en", "de", "se", "su", "sus", "con", "por", "para", "que", "es",
        ]
        .contains(&value.as_str()),
        "French" => [
            "le", "la", "les", "un", "une", "des", "du", "et", "mais", "je", "il", "elle", "nous",
            "vous", "ils", "elles", "ne", "pas", "non", "au", "aux", "en", "dans", "de", "pour",
            "que", "qui", "ce", "cette",
        ]
        .contains(&value.as_str()),
        "German" => [
            "der", "die", "das", "ein", "eine", "einer", "eines", "und", "aber", "ich", "wir",
            "sie", "er", "es", "nicht", "kein", "keine", "keinen", "keinem", "keiner", "im", "am",
            "auf", "mit", "für", "zu", "den", "dem", "von", "vom", "ist",
        ]
        .contains(&value.as_str()),
        _ => false,
    }
}

fn is_source_initial_function_word(value: &str) -> bool {
    matches!(value.to_ascii_lowercase().as_str(), "do" | "does" | "did")
}

fn is_sentence_initial(text: &str, start: usize) -> bool {
    let prefix = text[..start].trim_end();
    prefix.is_empty()
        || prefix
            .chars()
            .next_back()
            .is_some_and(|ch| matches!(ch, '.' | '!' | '?' | '。' | '！' | '？' | ';' | '；' | ':'))
}

fn money_ranges(text: &str, spans: &[protected_span::ProtectedSpan]) -> Vec<(usize, usize)> {
    let mut ranges = spans
        .iter()
        .filter(|span| span.kind == ProtectedSpanKind::Amount)
        .map(|span| (span.start_byte, span.end_byte))
        .collect::<Vec<_>>();
    for range in &mut ranges {
        if text[range.0..].starts_with('$') {
            let prefix_start = text[..range.0]
                .char_indices()
                .rev()
                .take_while(|(_, ch)| ch.is_ascii_alphabetic())
                .map(|(index, _)| index)
                .last();
            if let Some(prefix_start) = prefix_start {
                *range = (prefix_start, range.1);
            }
        }
    }
    const PREFIX_CODES: &[&str] = &["USD", "EUR", "GBP", "CNY", "HKD", "RMB"];
    // The matched currency codes are ASCII. ASCII folding keeps each match
    // byte offset aligned with the original UTF-8 text, unlike Unicode
    // lowercase expansion (for example U+0130 LATIN CAPITAL I WITH DOT).
    let folded = text.to_ascii_lowercase();
    for code in PREFIX_CODES {
        let code_lower = code.to_ascii_lowercase();
        for (start, _) in folded.match_indices(&code_lower) {
            let code_end = start + code.len();
            let before = folded[..start].chars().next_back();
            let after = folded[code_end..].chars().next();
            if before.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                || after.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            {
                continue;
            }
            let number_start = code_end
                + text[code_end..]
                    .chars()
                    .take_while(|ch| ch.is_whitespace())
                    .map(char::len_utf8)
                    .sum::<usize>();
            let Some(number) = spans.iter().find(|span| {
                span.start_byte == number_start && span.kind == ProtectedSpanKind::Number
            }) else {
                continue;
            };
            ranges.push((start, number.end_byte));
        }
    }
    ranges.sort_unstable();
    ranges.dedup();
    ranges
}

fn weekday_ranges(text: &str) -> Vec<(usize, usize, u8)> {
    const WEEKDAYS: &[(&str, u8)] = &[
        ("wednesday", 3),
        ("thursday", 4),
        ("tuesday", 2),
        ("saturday", 6),
        ("monday", 1),
        ("sunday", 7),
        ("friday", 5),
        ("mercredi", 3),
        ("vendredi", 5),
        ("dimanche", 7),
        ("samedi", 6),
        ("mardi", 2),
        ("jueves", 4),
        ("viernes", 5),
        ("martes", 2),
        ("sábado", 6),
        ("sabado", 6),
        ("domingo", 7),
        ("lunes", 1),
        ("montag", 1),
        ("dienstag", 2),
        ("mittwoch", 3),
        ("donnerstag", 4),
        ("freitag", 5),
        ("samstag", 6),
        ("sonntag", 7),
        ("星期一", 1),
        ("星期二", 2),
        ("星期三", 3),
        ("星期四", 4),
        ("星期五", 5),
        ("星期六", 6),
        ("星期日", 7),
        ("星期天", 7),
        ("礼拜一", 1),
        ("礼拜二", 2),
        ("礼拜三", 3),
        ("礼拜四", 4),
        ("礼拜五", 5),
        ("礼拜六", 6),
        ("礼拜日", 7),
        ("礼拜天", 7),
        ("周一", 1),
        ("周二", 2),
        ("周三", 3),
        ("周四", 4),
        ("周五", 5),
        ("周六", 6),
        ("周日", 7),
        ("周天", 7),
        ("月曜日", 1),
        ("火曜日", 2),
        ("水曜日", 3),
        ("木曜日", 4),
        ("金曜日", 5),
        ("土曜日", 6),
        ("日曜日", 7),
        ("월요일", 1),
        ("화요일", 2),
        ("수요일", 3),
        ("목요일", 4),
        ("금요일", 5),
        ("토요일", 6),
        ("일요일", 7),
    ];
    let folded = text.to_ascii_lowercase();
    let mut matches = Vec::new();
    for (alias, day) in WEEKDAYS {
        for (start, _) in folded.match_indices(alias) {
            let end = start + alias.len();
            let ascii = alias.is_ascii();
            let before = folded[..start].chars().next_back();
            let after = folded[end..].chars().next();
            if ascii
                && (before.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                    || after.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_'))
            {
                continue;
            }
            matches.push((start, end, *day));
        }
    }
    matches.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| (right.1 - right.0).cmp(&(left.1 - left.0)))
    });
    let mut selected: Vec<(usize, usize, u8)> = Vec::new();
    for found in matches {
        if selected.last().is_some_and(|previous| found.0 < previous.1) {
            continue;
        }
        selected.push(found);
    }
    selected
}

fn weekday_day(value: &str) -> Option<u8> {
    weekday_ranges(value)
        .into_iter()
        .find(|(start, end, _)| *start == 0 && *end == value.len())
        .map(|(_, _, day)| day)
}

fn money_signature(value: &str) -> Option<(&'static str, String)> {
    let folded = value.to_lowercase();
    let currency = if folded.contains("hk$") || folded.contains("hkd") || value.contains("港币") {
        "HKD"
    } else if folded.contains("us$") || folded.contains("usd") || value.contains("美元") {
        "USD"
    } else if folded.contains("dollar") || value.contains('$') {
        if value.contains('$') {
            let symbol_index = value.find('$')?;
            let prefix = &value[..symbol_index];
            if !prefix.is_empty() && !matches!(prefix.to_ascii_lowercase().as_str(), "us" | "hk") {
                return None;
            }
        }
        "USD"
    } else if value.contains("欧元")
        || folded.contains("eur")
        || folded.contains("euro")
        || value.contains('€')
    {
        "EUR"
    } else if value.contains("英镑")
        || folded.contains("gbp")
        || folded.contains("pound")
        || value.contains('£')
    {
        "GBP"
    } else if folded.contains("cny")
        || folded.contains("rmb")
        || folded.contains("yuan")
        || value.contains("人民币")
        || value.contains("元")
    {
        "CNY"
    } else {
        return None;
    };

    let numeric = value
        .chars()
        .filter(|ch| ch.is_ascii_digit() || matches!(ch, ',' | '.' | '+' | '-'))
        .collect::<String>();
    Some((currency, canonical_decimal(&numeric)?))
}

fn canonical_decimal(value: &str) -> Option<String> {
    let unsigned_number = value.trim_start_matches(['+', '-']);
    let (integer_part, fractional_part) = unsigned_number
        .split_once('.')
        .unwrap_or((unsigned_number, ""));
    if fractional_part.contains(',') {
        return None;
    }
    if integer_part.contains(',') {
        let groups = integer_part.split(',').collect::<Vec<_>>();
        if groups
            .first()
            .is_none_or(|first| first.is_empty() || first.len() > 3)
            || groups.iter().skip(1).any(|group| group.len() != 3)
        {
            return None;
        }
    }
    let normalized = value.replace(',', "");
    let (negative, unsigned) = if let Some(rest) = normalized.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = normalized.strip_prefix('+') {
        (false, rest)
    } else {
        (false, normalized.as_str())
    };
    if unsigned.is_empty() || unsigned.contains(['+', '-']) || unsigned.matches('.').count() > 1 {
        return None;
    }
    let (integer, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let integer = integer.trim_start_matches('0');
    let integer = if integer.is_empty() { "0" } else { integer };
    let fraction = fraction.trim_end_matches('0');
    let is_zero = integer == "0" && fraction.is_empty();
    let mut result = String::new();
    if negative && !is_zero {
        result.push('-');
    }
    result.push_str(integer);
    if !fraction.is_empty() {
        result.push('.');
        result.push_str(fraction);
    }
    Some(result)
}

fn negation_signature(text: &str) -> Vec<&'static str> {
    const NEGATIONS: &[&str] = &[
        "shouldn't",
        "wouldn't",
        "couldn't",
        "doesn't",
        "didn't",
        "isn't",
        "aren't",
        "wasn't",
        "weren't",
        "mustn't",
        "cannot",
        "can't",
        "won't",
        "don't",
        "without",
        "neither",
        "nobody",
        "nothing",
        "never",
        "none",
        "not",
        "no",
        "hardly",
        "scarcely",
        "rarely",
        "jamais",
        "ninguno",
        "ninguna",
        "ningún",
        "ningun",
        "without",
        "nicht",
        "keiner",
        "kein",
        "nie",
        "ohne",
        "pas",
        "ne",
        "nunca",
        "sin",
        "禁止",
        "勿",
        "并非",
        "不要",
        "不能",
        "不会",
        "没有",
        "不是",
        "未曾",
        "未",
        "别",
        "没",
        "不",
        "ません",
        "ないで",
        "なく",
        "ない",
        "않습니다",
        "않다",
        "않",
        "없습니다",
        "없다",
        "없",
        "아니요",
        "아니",
    ];
    let normalized = text.to_lowercase().replace(['’', '‘', 'ʼ'], "'");
    let has_french_pas = normalized
        .split(|ch: char| !ch.is_ascii_alphabetic())
        .any(|word| word == "pas");
    let mut found: Vec<(usize, usize)> = Vec::new();
    for marker in NEGATIONS {
        if *marker == "ne" && has_french_pas {
            continue;
        }
        for (start, _) in normalized.match_indices(marker) {
            let end = start + marker.len();
            if marker.is_ascii() {
                let before = normalized[..start].chars().next_back();
                let after = normalized[end..].chars().next();
                if before.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                    || after.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                {
                    continue;
                }
            }
            if found
                .iter()
                .any(|(known_start, known_end)| start < *known_end && *known_start < end)
            {
                continue;
            }
            found.push((start, end));
        }
    }
    found.sort_by_key(|(start, _)| *start);
    vec!["negative"; found.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transform_input<'a>(instruction: &'a str, source: &'a str) -> TextActionInput<'a> {
        TextActionInput {
            instruction,
            source_kind: TextActionSourceKind::Selection,
            source_text: source,
            target_is_empty: false,
            configured_translation_target: None,
            reply_context: None,
        }
    }

    fn make_plan(instruction: &str, source: &str) -> Result<TextActionPlan, TextActionPlanError> {
        plan_text_action(transform_input(instruction, source))
    }

    fn reply_input<'a>(instruction: &'a str, context: &'a str) -> TextActionInput<'a> {
        TextActionInput {
            instruction,
            source_kind: TextActionSourceKind::EmptyComposer,
            source_text: "",
            target_is_empty: true,
            configured_translation_target: None,
            reply_context: Some(context),
        }
    }

    #[test]
    fn planner_accepts_only_explicit_bounded_operations() {
        let cases = [
            (
                "Rewrite the selected paragraph clearly",
                TextActionOperation::Rewrite,
            ),
            ("Please shorten this text", TextActionOperation::Shorten),
            (
                "Organize this as bullet points",
                TextActionOperation::Organize,
            ),
            ("翻译成中文", TextActionOperation::Translate),
            ("把预算1250改成1500", TextActionOperation::ModifyExact),
        ];
        for (instruction, operation) in cases {
            let source = if operation == TextActionOperation::ModifyExact {
                "预算1250"
            } else {
                "The selected paragraph is ready."
            };
            let plan = make_plan(instruction, source).expect("supported explicit instruction");
            assert_eq!(plan.operation, operation, "{instruction}");
        }
        let plan = plan_text_action(reply_input(
            "Draft a reply",
            "Leon asked for the Friday review.",
        ))
        .expect("reply uses empty composer plus authorized context");
        assert_eq!(plan.operation, TextActionOperation::DraftReply);
    }

    #[test]
    fn negated_exact_change_requests_are_not_authorization() {
        for instruction in [
            "Do not replace $900 with $1200.",
            "Don't change $900 to $1200.",
            "不要把 $900 改成 $1200。",
        ] {
            assert_eq!(
                make_plan(instruction, "The budget is $900.").unwrap_err(),
                TextActionPlanError::AmbiguousInstruction,
                "negated exact-change request must fail closed: {instruction}"
            );
        }

        for instruction in ["Replace $900 with $1200.", "把 $900 改成 $1200。"] {
            let plan = make_plan(instruction, "The budget is $900.").unwrap();
            assert_eq!(plan.operation, TextActionOperation::ModifyExact);
            assert_eq!(
                validate_generated_result(&plan, "The budget is $900.", "The budget is $1200."),
                Ok(())
            );
        }

        let quoted_values =
            make_plan("Replace '$900' with '$1200'.", "The budget is $900.").unwrap();
        assert_eq!(quoted_values.operation, TextActionOperation::ModifyExact);
    }

    #[test]
    fn unsupported_ambiguous_and_wrong_source_instructions_are_rejected() {
        assert_eq!(
            make_plan("Make it better", "The text is ready.").unwrap_err(),
            TextActionPlanError::UnsupportedInstruction
        );
        assert_eq!(
            make_plan("Shorten and translate this to Spanish", "Ready.").unwrap_err(),
            TextActionPlanError::AmbiguousInstruction
        );
        assert_eq!(
            plan_text_action(TextActionInput {
                instruction: "Rewrite this",
                source_kind: TextActionSourceKind::EmptyComposer,
                source_text: "",
                target_is_empty: true,
                configured_translation_target: None,
                reply_context: None,
            })
            .unwrap_err(),
            TextActionPlanError::UnauthorizedSourceKind
        );
        assert_eq!(
            make_plan("Rewrite this", "  ").unwrap_err(),
            TextActionPlanError::NoSource
        );
        assert_eq!(
            plan_text_action(TextActionInput {
                instruction: "Draft a reply",
                source_kind: TextActionSourceKind::EmptyComposer,
                source_text: "",
                target_is_empty: true,
                configured_translation_target: None,
                reply_context: Some(" "),
            })
            .unwrap_err(),
            TextActionPlanError::ReplyContextUnavailable
        );
    }

    #[test]
    fn oversized_sources_are_rejected_without_partial_authorization() {
        let source = "a".repeat(MAX_ACTION_SOURCE_BYTES + 1);
        assert_eq!(
            make_plan("Rewrite this", &source).unwrap_err(),
            TextActionPlanError::UnsupportedInstruction
        );
    }

    #[test]
    fn exact_version_change_authorizes_only_its_source_occurrence() {
        let source = "Version 1.2.3 is used by React at /tmp/config.json.";
        let plan = make_plan("Change version 1.2.3 to 1.2.4", source).unwrap();
        assert_eq!(plan.operation, TextActionOperation::ModifyExact);
        assert_eq!(plan.authorized_changes.len(), 1);
        let change = &plan.authorized_changes[0];
        assert_eq!(&source[change.start_byte..change.end_byte], "1.2.3");
        assert_eq!(change.replacement_value, "1.2.4");

        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "Version 1.2.4 is used by React at /tmp/config.json."
            ),
            Ok(())
        );
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "Version 1.2.4 is used by Preact at /tmp/config.json."
            ),
            Err(TextActionGuardError::UnauthorizedEntityChange)
        );
    }

    #[test]
    fn exact_term_and_chinese_amount_edits_keep_other_entities() {
        let source = "We ship React beside Notion under v2.4.1.";
        let plan = make_plan("Replace React with Preact", source).unwrap();
        assert_eq!(
            validate_generated_result(&plan, source, "We ship Preact beside Notion under v2.4.1."),
            Ok(())
        );
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "We ship Preact beside Airtable under v2.4.1."
            ),
            Err(TextActionGuardError::UnauthorizedEntityChange)
        );

        let source = "预算1250美元，周五发送。";
        let plan = make_plan("把预算1250美元改成1500美元", source).unwrap();
        assert_eq!(
            validate_generated_result(&plan, source, "预算1500美元，周五发送。"),
            Ok(())
        );
        assert_eq!(
            validate_generated_result(&plan, source, "预算1500美元，周一发送。"),
            Err(TextActionGuardError::UnauthorizedEntityChange)
        );
    }

    #[test]
    fn repeated_or_untyped_exact_change_references_are_ambiguous() {
        assert_eq!(
            make_plan(
                "Change version 1.2.3 to 1.2.4",
                "Version 1.2.3 and backup 1.2.3 are both present."
            )
            .unwrap_err(),
            TextActionPlanError::AmbiguousInstruction
        );
        assert_eq!(
            make_plan("Replace React with Preact", "We use Presto.").unwrap_err(),
            TextActionPlanError::AmbiguousInstruction
        );
        assert_eq!(
            make_plan("Replace React and Notion with Preact", "We use React.").unwrap_err(),
            TextActionPlanError::AmbiguousInstruction
        );
        let source = "预算1250";
        let plan = make_plan("请按这句改写：‘把预算1250改成1500’", source).unwrap();
        assert_eq!(plan.operation, TextActionOperation::Rewrite);
        assert!(plan.authorized_changes.is_empty());
        assert_eq!(
            validate_generated_result(&plan, source, "预算1500"),
            Err(TextActionGuardError::ProtectedFactChanged)
        );
    }

    #[test]
    fn quoted_change_sentence_does_not_grant_a_translation_edit() {
        let source = "The release uses version 1.2.3.";
        let plan = make_plan(
            "Translate to Spanish the sentence ‘change version 1.2.3 to 1.2.4’.",
            source,
        )
        .unwrap();
        assert_eq!(plan.operation, TextActionOperation::Translate);
        assert!(plan.authorized_changes.is_empty());
        assert_eq!(
            validate_generated_result(&plan, source, "La versión usa 1.2.4."),
            Err(TextActionGuardError::ProtectedFactChanged)
        );

        let source = "The release uses version 1.2.3.";
        let plan = make_plan("Rewrite this: 'change version 1.2.3 to 1.2.4'", source).unwrap();
        assert_eq!(plan.operation, TextActionOperation::Rewrite);
        assert!(plan.authorized_changes.is_empty());
        assert_eq!(
            validate_generated_result(&plan, source, "The release uses version 1.2.4."),
            Err(TextActionGuardError::ProtectedFactChanged)
        );
    }

    #[test]
    fn source_binding_and_public_plan_mutation_fail_closed() {
        let source = "Use React version 1.2.3.";
        let mut plan = make_plan("Rewrite this clearly", source).unwrap();
        assert_eq!(
            validate_generated_result(
                &plan,
                "Use React version 1.2.4.",
                "Use React version 1.2.4."
            ),
            Err(TextActionGuardError::UnauthorizedEntityChange)
        );
        plan.operation = TextActionOperation::Shorten;
        assert_eq!(
            validate_generated_result(&plan, source, source),
            Err(TextActionGuardError::UnauthorizedEntityChange)
        );
    }

    #[test]
    fn debug_format_omits_source_instruction_and_reply_context_values() {
        let plan = plan_text_action(reply_input(
            "Draft a reply to the private note",
            "Leon asked for the Friday review.",
        ))
        .unwrap();
        let debug = format!("{plan:?}");
        for private_value in ["private note", "Leon", "Friday", "asked for"] {
            assert!(
                !debug.contains(private_value),
                "debug leaked {private_value}"
            );
        }
    }

    #[test]
    fn rewrite_shorten_and_organize_preserve_entities_paths_and_polarity() {
        let source = "Do not run `git status` in /tmp/voice-flow; release Friday, not Monday.";
        for instruction in [
            "Rewrite this sentence",
            "Shorten this sentence",
            "Organize this as a short list",
        ] {
            let plan = make_plan(instruction, source).unwrap();
            assert_eq!(
                validate_generated_result(&plan, source, source),
                Ok(()),
                "identity should preserve every typed source fact"
            );
            assert_eq!(
                validate_generated_result(
                    &plan,
                    source,
                    "Run `git status` in /tmp/voice-flow; release Friday, not Monday."
                ),
                Err(TextActionGuardError::NegationChanged),
                "deleting one polarity marker must fail"
            );
            assert_eq!(
                validate_generated_result(
                    &plan,
                    source,
                    "Do not run `git status` in /tmp/voice-flow; release Monday, not Friday."
                ),
                Err(TextActionGuardError::ProtectedFactChanged),
                "weekdays remain in their original order"
            );
        }
    }

    #[test]
    fn negation_markers_are_case_and_script_aware() {
        let source = "No release today.";
        let plan = make_plan("Rewrite this", source).unwrap();
        assert_eq!(
            validate_generated_result(&plan, source, "Release today."),
            Err(TextActionGuardError::NegationChanged)
        );
        let source = "不要上线。";
        let plan = make_plan("改写这句话", source).unwrap();
        assert_eq!(
            validate_generated_result(&plan, source, "上线。"),
            Err(TextActionGuardError::NegationChanged)
        );
    }

    #[test]
    fn captured_polite_rewrite_and_organized_bullets_preserve_real_facts() {
        let source = "Send me the review notes when you have time.";
        let plan = make_plan("Rewrite this sentence more politely", source).unwrap();
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "Could you please send me the review notes at your convenience?"
            ),
            Ok(()),
            "sentence-initial Send and Could are grammatical words, not names"
        );

        let source = "Send me the review notes from React for $1,250 when you have time.";
        let plan = make_plan("Rewrite this sentence more politely", source).unwrap();
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "Could you please send me the review notes from React for $1,250 at your convenience?"
            ),
            Ok(())
        );
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "Could you please send me the review notes from Preact for $1,250 at your convenience?"
            ),
            Err(TextActionGuardError::ProtectedFactChanged),
            "an actual product-name change stays blocked"
        );
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "Could you please send me the review notes from React for $1,500 at your convenience?"
            ),
            Err(TextActionGuardError::ProtectedFactChanged),
            "an actual amount change stays blocked"
        );

        let source = "Review the draft; confirm the agenda; send the notes.";
        let plan = make_plan("Organize this as a short list", source).unwrap();
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "- Review the draft  \n- Confirm the agenda  \n- Send the notes"
            ),
            Ok(()),
            "capitalization and list layout do not turn ordinary verbs into names"
        );

        let source = "Review the draft for React; confirm the agenda; send the notes.";
        let plan = make_plan("Organize this as a short list", source).unwrap();
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "- Review the draft for Preact  \n- Confirm the agenda  \n- Send the notes"
            ),
            Err(TextActionGuardError::ProtectedFactChanged),
            "organization still protects a real name-like term"
        );

        let held_out = [
            (
                "Can you send me the notes?",
                "Could you please send me the notes?",
            ),
            ("Would you share the draft?", "Please share the draft."),
            (
                "Check the report and send it to Maya.",
                "Please check the report and send it to Maya.",
            ),
        ];
        for (source, candidate) in held_out {
            let plan = make_plan("Rewrite politely", source).unwrap();
            assert_eq!(
                validate_generated_result(&plan, source, candidate),
                Ok(()),
                "bounded grammatical leads should allow this held-out rewrite"
            );
        }
        let source = "Check the report and send it to Maya.";
        let plan = make_plan("Rewrite politely", source).unwrap();
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "Please check the report and send it to Morgan."
            ),
            Err(TextActionGuardError::ProtectedFactChanged),
            "the grammatical lead exception must not erase a changed recipient"
        );
    }

    #[test]
    fn captured_chinese_prohibition_translation_preserves_negation_and_entities() {
        let source = "Do not deploy v1.2.3 at /tmp/voice-flow on Friday; keep React.";
        let plan = make_plan("Translate this into Chinese", source).unwrap();
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "请勿在星期五将 v1.2.3 部署到 /tmp/voice-flow；保持 React。"
            ),
            Ok(())
        );
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "在星期五将 v1.2.3 部署到 /tmp/voice-flow；保持 React。"
            ),
            Err(TextActionGuardError::NegationChanged),
            "dropping the translated prohibition remains blocked"
        );
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "请勿在星期一将 v1.2.3 部署到 /tmp/voice-flow；保持 React。"
            ),
            Err(TextActionGuardError::ProtectedFactChanged),
            "the weekday remains protected"
        );
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "请勿在星期五将 v1.2.4 部署到 /tmp/voice-flow；保持 React。"
            ),
            Err(TextActionGuardError::ProtectedFactChanged),
            "the version remains protected"
        );
    }

    #[test]
    fn captured_reply_opener_accepts_only_the_grounded_assertion_and_unicode_is_safe() {
        let plan = plan_text_action(reply_input(
            "Draft a reply. Tell them that I can meet Friday.",
            "Leon asked for the Friday review.",
        ))
        .unwrap();
        assert_eq!(
            validate_generated_result(&plan, "", "Sure, I can meet on Friday."),
            Ok(()),
            "the conventional opener and preposition do not add a reply fact"
        );
        assert_eq!(
            validate_generated_result(&plan, "", "Sure, I can meet on Monday."),
            Err(TextActionGuardError::ProtectedFactChanged),
            "a changed weekday stays blocked"
        );
        assert_eq!(
            validate_generated_result(&plan, "", "Sure, Morgan can meet on Friday."),
            Err(TextActionGuardError::ProtectedFactChanged),
            "an unauthorized person and changed assertion stay blocked"
        );
        assert_eq!(
            validate_generated_result(&plan, "", "Sure, I can meet on Friday and approve $900."),
            Err(TextActionGuardError::ProtectedFactChanged),
            "an unsupported amount stays blocked"
        );

        let unicode_plan =
            plan_text_action(reply_input("起草回复", "Mike 将在周五审阅报告。")).unwrap();
        for punctuation in ['。', '！', '？'] {
            let candidate = format!("好的{punctuation}Mike 将在周五审阅报告。");
            let result = std::panic::catch_unwind(|| {
                validate_generated_result(&unicode_plan, "", &candidate)
            });
            assert!(result.is_ok(), "unicode punctuation must not panic");
        }
    }

    #[test]
    fn translation_allows_only_finite_weekday_and_money_equivalences() {
        let source = "预算是$1,250，周五发送给React。";
        let plan = make_plan("Translate this to Spanish", source).unwrap();
        assert_eq!(plan.target_language.as_deref(), Some("Spanish"));
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "El presupuesto es USD 1,250 y se envía el viernes a React."
            ),
            Ok(())
        );
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "El presupuesto es USD 1,250 y se envía el lunes a React."
            ),
            Err(TextActionGuardError::ProtectedFactChanged)
        );
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "El presupuesto es EUR 1,250 y se envía el viernes a React."
            ),
            Err(TextActionGuardError::ProtectedFactChanged)
        );
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "El presupuesto es USD 1,500 y se envía el viernes a React."
            ),
            Err(TextActionGuardError::ProtectedFactChanged)
        );
        assert_eq!(
            validate_generated_result(&plan, source, "El presupuesto es USD 1,250 para React."),
            Err(TextActionGuardError::ProtectedFactChanged)
        );

        let ordered_source = "Friday, then Monday.";
        let ordered_plan = make_plan("Translate this to Spanish", ordered_source).unwrap();
        assert_eq!(
            validate_generated_result(
                &ordered_plan,
                ordered_source,
                "El lunes y luego el viernes."
            ),
            Err(TextActionGuardError::ProtectedFactChanged),
            "the finite weekday equivalence keeps source order and multiplicity"
        );
    }

    #[test]
    fn translation_uses_explicit_instruction_target_before_configured_target() {
        let mut input = transform_input("Translate to Chinese", "Friday is release day.");
        input.configured_translation_target = Some("French");
        let plan = plan_text_action(input).unwrap();
        assert_eq!(plan.target_language.as_deref(), Some("Chinese"));

        let mut input = transform_input("Translate this", "Friday is release day.");
        input.configured_translation_target = Some("fr-FR");
        let plan = plan_text_action(input).unwrap();
        assert_eq!(plan.target_language.as_deref(), Some("French"));

        let mut input = transform_input("İ, translate this to French", "This is ready.");
        input.configured_translation_target = None;
        let plan = plan_text_action(input).unwrap();
        assert_eq!(plan.target_language.as_deref(), Some("French"));
    }

    #[test]
    fn money_scanning_keeps_utf8_offsets_after_unicode_case_expansion_candidates() {
        for (source, candidate) in [
            ("İ USD金额", "İ USD amount"),
            ("İ USD 1,250金额", "İ USD 1,250 amount"),
        ] {
            let plan = make_plan("Translate into English", source).unwrap();
            let result =
                std::panic::catch_unwind(|| validate_generated_result(&plan, source, candidate));
            assert!(
                result.is_ok(),
                "money scanning must not panic for {source:?}"
            );
            assert_eq!(result.unwrap(), Ok(()), "unicode-safe scan for {source:?}");
        }
    }

    #[test]
    fn translation_requires_a_target_and_rejects_unverifiable_money() {
        assert_eq!(
            make_plan("Translate this", "This text is ready.").unwrap_err(),
            TextActionPlanError::AmbiguousInstruction
        );
        let source = "Budget: ¥1,250.";
        let plan = make_plan("Translate to English", source).unwrap();
        assert_eq!(
            validate_generated_result(&plan, source, "Budget: 1250 yen."),
            Err(TextActionGuardError::TranslationEntityUnverifiable)
        );
        let source = "Version 1.2.3 ships Friday.";
        let plan = make_plan("Translate to Chinese", source).unwrap();
        assert_eq!(
            validate_generated_result(&plan, source, source),
            Err(TextActionGuardError::TranslationEntityUnverifiable)
        );
    }

    #[test]
    fn translation_keeps_version_path_name_and_negation() {
        let source = "Do not deploy v1.2.3 at /tmp/voice-flow on Friday; keep React.";
        let plan = make_plan("Translate this into Chinese", source).unwrap();
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "不要部署 v1.2.3 到 /tmp/voice-flow 周五；保留 React。"
            ),
            Ok(())
        );
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "部署 v1.2.3 到 /tmp/voice-flow 周五；保留 React。"
            ),
            Err(TextActionGuardError::NegationChanged)
        );

        for (language, candidate) in [
            ("Spanish", "No publicar v1.2.3 el viernes."),
            ("French", "Ne pas publier v1.2.3 vendredi."),
            ("German", "Nicht am Freitag v1.2.3 veröffentlichen."),
        ] {
            let source = "Do not publish v1.2.3 on Friday.";
            let mut input = transform_input("Translate this", source);
            input.configured_translation_target = Some(language);
            let plan = plan_text_action(input).unwrap();
            assert_eq!(
                validate_generated_result(&plan, source, candidate),
                Ok(()),
                "sentence-initial target-language function words are not names ({language})"
            );
        }
    }

    #[test]
    fn reply_draft_is_grounded_by_private_context_and_supported_instruction_facts() {
        let instruction = "Draft a reply. Tell them that I can meet Friday.";
        let context = "Leon asked for the Friday review.";
        let plan = plan_text_action(reply_input(instruction, context)).unwrap();
        assert_eq!(
            validate_generated_result(
                &plan,
                "",
                "Thanks, Leon. I'll review and get back to you Friday."
            ),
            Ok(())
        );
        assert_eq!(
            validate_generated_result(&plan, "", "I can meet Friday."),
            Ok(())
        );
        assert_eq!(
            validate_generated_result(
                &plan,
                "",
                "Thanks, Morgan. I'll review and follow up Friday."
            ),
            Err(TextActionGuardError::ProtectedFactChanged)
        );
        assert_eq!(
            validate_generated_result(&plan, "", "Thanks, Leon. I approved the $900 payment."),
            Err(TextActionGuardError::ProtectedFactChanged)
        );
        assert_eq!(
            validate_generated_result(&plan, "", "Thanks, Leon. I have already approved it."),
            Err(TextActionGuardError::ProtectedFactChanged),
            "omitting the explicitly asserted Friday fact fails before reply-shell classification"
        );
    }

    #[test]
    fn reply_context_alternatives_cannot_replace_explicit_assertion_facts() {
        let friday_instruction = "Draft a reply. Tell them that I can meet Friday.";
        let weekday_context = "Leon asked whether Monday or Friday works.";
        let friday_plan =
            plan_text_action(reply_input(friday_instruction, weekday_context)).unwrap();
        assert_eq!(
            validate_generated_result(&friday_plan, "", "Sure, I can meet on Friday."),
            Ok(())
        );
        assert_eq!(
            validate_generated_result(&friday_plan, "", "Sure, I can meet on Monday."),
            Err(TextActionGuardError::ProtectedFactChanged)
        );

        let amount_instruction = "Draft a reply. Tell them that I can approve $900.";
        let amount_context = "Leon asked whether the budget is $900 or $1200.";
        let amount_plan =
            plan_text_action(reply_input(amount_instruction, amount_context)).unwrap();
        assert_eq!(
            validate_generated_result(&amount_plan, "", "I can approve $900."),
            Ok(())
        );
        assert_eq!(
            validate_generated_result(&amount_plan, "", "I can approve $1200."),
            Err(TextActionGuardError::ProtectedFactChanged)
        );
    }

    #[test]
    fn reply_instruction_quotes_and_negation_do_not_authorize_extra_facts() {
        for instruction in [
            "Draft a reply. Tell them that I cannot pay $1,250.",
            "Draft a reply quoting ‘Tell them that I paid $1,250.’",
        ] {
            let plan =
                plan_text_action(reply_input(instruction, "Thanks for the update.")).unwrap();
            assert_eq!(
                validate_generated_result(&plan, "", "Thanks. I paid $1,250."),
                Err(TextActionGuardError::ProtectedFactChanged)
            );
        }
    }

    #[test]
    fn reply_planning_requires_empty_composer_and_fresh_bounded_context() {
        let mut input = reply_input("Draft a reply", "The request is ready.");
        input.target_is_empty = false;
        assert_eq!(
            plan_text_action(input).unwrap_err(),
            TextActionPlanError::ReplyTargetMustBeEmpty
        );
        let mut input = reply_input("Draft a reply", "The request is ready.");
        input.source_text = "existing text";
        assert_eq!(
            plan_text_action(input).unwrap_err(),
            TextActionPlanError::ReplyTargetMustBeEmpty
        );
        let context = "x".repeat(MAX_REPLY_CONTEXT_CHARS + 1);
        assert_eq!(
            plan_text_action(reply_input("Draft a reply", &context)).unwrap_err(),
            TextActionPlanError::ReplyContextUnavailable
        );
    }
}
