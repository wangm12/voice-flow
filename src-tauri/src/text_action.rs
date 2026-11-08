//! Bounded planning and deterministic fidelity checks for selected-text actions.
//!
//! This module has no provider, target-discovery, preview, persistence, or delivery
//! responsibilities. It only turns an explicit instruction into a finite plan
//! and checks a generated candidate against the plan's immutable source evidence.

use crate::protected_span::{self, ProtectedSpanKind};
use std::ops::Range;

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

/// Closed Organize variants. Recipe identity and formatting semantics are
/// owned here so callers can carry an identifier without duplicating policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrganizeRecipe {
    DeveloperTask,
    WorkUpdate,
    TodoList,
}

impl OrganizeRecipe {
    pub const fn id(self) -> &'static str {
        match self {
            Self::DeveloperTask => "developer_task",
            Self::WorkUpdate => "work_update",
            Self::TodoList => "todo_list",
        }
    }

    pub const fn version(self) -> u16 {
        1
    }

    pub const fn titles(self) -> &'static [&'static str] {
        match self {
            Self::DeveloperTask => &["开发任务", "Developer task"],
            Self::WorkUpdate => &["工作进展", "Work update"],
            Self::TodoList => &["待办清单", "To-do list"],
        }
    }

    const fn semantic_guardrail(self) -> &'static str {
        match self {
            Self::DeveloperTask => {
                "Do not add a technology stack, implementation, metric, schedule, or acceptance criterion that the source does not state."
            }
            Self::WorkUpdate => {
                "Do not turn a plan into completed work or create a commitment for the user."
            }
            Self::TodoList => {
                "Do not infer an owner or deadline, and do not create a calendar item."
            }
        }
    }
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
    pub recipe: Option<OrganizeRecipe>,
    validation_evidence: ActionValidationEvidence,
}

impl TextActionPlan {
    /// Returns the Rust-owned recipe instruction for the production prompt.
    /// The formatting rule is bound to the exact source snapshot used to plan.
    pub fn recipe_prompt_instruction(&self) -> Option<String> {
        let recipe = self.recipe?;
        let source_has_code =
            protected_span::has_code_literals(&self.validation_evidence.source_snapshot);
        let body_rule = if source_has_code {
            "Copy the body byte-for-byte from the source, including whitespace, punctuation, line endings, and code indentation. Do not add list markers."
        } else {
            "Preserve every source character, punctuation mark, whitespace character, and its order. Add exactly one '- ' prefix before each non-empty paragraph; a paragraph boundary is an existing blank line. Do not split or merge paragraphs."
        };
        Some(format!(
            "Organize recipe: {} version {}. {} Start with exactly one unformatted title line, choosing one exact title from [{}] (no '#', bold, or other wrapper). Then add exactly one LF and immediately continue with the source-preserving body; do not add a blank line. {} Preserve source leading/trailing whitespace and line endings. Do not paraphrase, split, merge, regroup, or reorder source content. Do not add section labels, facts, or missing-field guesses. Return only the title and body.",
            recipe.id(),
            recipe.version(),
            recipe.semantic_guardrail(),
            recipe.titles().join(" | "),
            body_rule,
        ))
    }
}

impl std::fmt::Debug for TextActionPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TextActionPlan")
            .field("operation", &self.operation)
            .field("recipe", &self.recipe.map(OrganizeRecipe::id))
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
    SourceTooLong,
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
    recipe_snapshot: Option<OrganizeRecipe>,
    recipe_layout_snapshot: Option<RecipeLayoutWitness>,
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
    let (operation, modification, recipe) =
        detect_operation(&unquoted_instruction, input.instruction)?;
    let recipe_layout_snapshot = match recipe {
        Some(recipe) => {
            let layout = recipe_layout(input.source_text);
            if maximum_recipe_candidate_bytes(recipe, &layout) > MAX_ACTION_SOURCE_BYTES {
                return Err(TextActionPlanError::SourceTooLong);
            }
            Some(layout)
        }
        None => None,
    };

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
        recipe_snapshot: recipe,
        recipe_layout_snapshot,
    };

    Ok(TextActionPlan {
        operation,
        target_language,
        authorized_changes,
        recipe,
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
        || plan.recipe != evidence.recipe_snapshot
        || (plan.recipe.is_some() && plan.operation != TextActionOperation::Organize)
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
            if let Some(recipe) = plan.recipe {
                let Some(body) = recipe_body_from_candidate(recipe, candidate) else {
                    return Err(TextActionGuardError::ProtectedFactChanged);
                };
                let layout = evidence
                    .recipe_layout_snapshot
                    .as_ref()
                    .ok_or(TextActionGuardError::UnauthorizedEntityChange)?;
                if body != layout.body {
                    return Err(TextActionGuardError::ProtectedFactChanged);
                }
                let projected = project_recipe_layout(body, layout)
                    .ok_or(TextActionGuardError::ProtectedFactChanged)?;
                if projected != source {
                    return Err(TextActionGuardError::ProtectedFactChanged);
                }
                validate_preserved_transform(plan.operation, source, &projected)
            } else {
                validate_preserved_transform(plan.operation, source, candidate)
            }
        }
        TextActionOperation::ModifyExact => validate_exact_modification(plan, source, candidate),
        TextActionOperation::Translate => validate_translation(plan, source, candidate),
        TextActionOperation::DraftReply => validate_reply(plan, candidate),
    }
}

/// A recipe header is the only authorized title occurrence. The source is
/// compared without projection, so a matching title inside the source remains
/// part of the required body.
fn recipe_body_from_candidate(recipe: OrganizeRecipe, candidate: &str) -> Option<&str> {
    let (title, body) = candidate.split_once('\n')?;
    let title = title.strip_suffix('\r').unwrap_or(title);
    recipe.titles().contains(&title).then_some(body)
}

/// Source-bound layout projection. Every marker range is recorded at the
/// moment this recipe owner inserts it; callers may project only after the
/// candidate body matches `body` byte-for-byte.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecipeLayoutWitness {
    body: String,
    inserted_markers: Vec<Range<usize>>,
}

fn recipe_layout(source: &str) -> RecipeLayoutWitness {
    if protected_span::has_code_literals(source) {
        return RecipeLayoutWitness {
            body: source.to_owned(),
            inserted_markers: Vec::new(),
        };
    }

    let mut body = String::with_capacity(source.len().saturating_add(2));
    let mut inserted_markers = Vec::new();
    let mut paragraph_start = true;
    for line in source.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let content = content.strip_suffix('\r').unwrap_or(content);
        let blank_line = content.chars().all(char::is_whitespace);
        if !blank_line && paragraph_start {
            let start = body.len();
            body.push_str("- ");
            inserted_markers.push(start..body.len());
            paragraph_start = false;
        }
        body.push_str(line);
        if blank_line {
            paragraph_start = true;
        }
    }
    RecipeLayoutWitness {
        body,
        inserted_markers,
    }
}

// Test helper delegated to the same layout owner.
#[cfg(test)]
fn expected_recipe_body(source: &str) -> String {
    recipe_layout(source).body
}

fn maximum_recipe_candidate_bytes(recipe: OrganizeRecipe, layout: &RecipeLayoutWitness) -> usize {
    // The parser accepts both LF and CRLF on the title boundary. Budget the
    // largest allowed title and that full separator so every accepted layout
    // remains inside the existing output cap before a request is dispatched.
    let title_bytes = recipe
        .titles()
        .iter()
        .map(|title| title.len())
        .max()
        .unwrap_or(0);
    layout
        .body
        .len()
        .saturating_add(title_bytes)
        .saturating_add(2)
}

fn project_recipe_layout(body: &str, layout: &RecipeLayoutWitness) -> Option<String> {
    if body != layout.body {
        return None;
    }
    let removed_bytes = layout
        .inserted_markers
        .iter()
        .map(|range| range.end.checked_sub(range.start))
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .sum::<usize>();
    let mut projected = String::with_capacity(body.len().saturating_sub(removed_bytes));
    let mut cursor = 0;
    for marker in &layout.inserted_markers {
        if marker.start < cursor || marker.end > body.len() || marker.start > marker.end {
            return None;
        }
        projected.push_str(&body[cursor..marker.start]);
        cursor = marker.end;
    }
    projected.push_str(&body[cursor..]);
    Some(projected)
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
) -> Result<
    (
        TextActionOperation,
        Option<ParsedModification>,
        Option<OrganizeRecipe>,
    ),
    TextActionPlanError,
> {
    let recipe = detect_organize_recipe(unquoted_instruction)?;
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
    ) || recipe.is_some();
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
        return Ok((TextActionOperation::ModifyExact, modification, None));
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
        [operation] => Ok((*operation, None, recipe)),
        [] => Err(TextActionPlanError::UnsupportedInstruction),
        _ => Err(TextActionPlanError::AmbiguousInstruction),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecipeMention {
    start: usize,
    end: usize,
    recipe: OrganizeRecipe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecipeLabelSpan {
    start: usize,
    end: usize,
    recipe: OrganizeRecipe,
}

fn detect_organize_recipe(
    instruction: &str,
) -> Result<Option<OrganizeRecipe>, TextActionPlanError> {
    let mentions = recipe_mentions(instruction);
    if mentions.is_empty() {
        return Ok(None);
    }

    let mut recipes = Vec::new();
    for mention in &mentions {
        if instruction_cue_is_negated(instruction, mention.start)
            || !recipe_command_prefix_is_allowed(instruction, mention.start)
        {
            return Err(TextActionPlanError::AmbiguousInstruction);
        }
        if !recipes.contains(&mention.recipe) {
            recipes.push(mention.recipe);
        }
    }
    if recipes.len() == 1 && has_coordinated_recipe_choice(instruction, &mentions) {
        return Err(TextActionPlanError::AmbiguousInstruction);
    }
    if mentions
        .iter()
        .any(|mention| !recipe_tail_is_allowed(&instruction[mention.end..]))
    {
        return Err(TextActionPlanError::AmbiguousInstruction);
    }
    match recipes.as_slice() {
        [] => Ok(None),
        [recipe] => Ok(Some(*recipe)),
        _ => Err(TextActionPlanError::AmbiguousInstruction),
    }
}

fn recipe_mentions(instruction: &str) -> Vec<RecipeMention> {
    const CUES: &[(&str, OrganizeRecipe)] = &[
        ("整理成开发任务", OrganizeRecipe::DeveloperTask),
        ("整理为开发任务", OrganizeRecipe::DeveloperTask),
        ("组织成开发任务", OrganizeRecipe::DeveloperTask),
        ("组织为开发任务", OrganizeRecipe::DeveloperTask),
        (
            "organize as a developer task",
            OrganizeRecipe::DeveloperTask,
        ),
        (
            "organize as a development task",
            OrganizeRecipe::DeveloperTask,
        ),
        ("format as a developer task", OrganizeRecipe::DeveloperTask),
        (
            "format as a development task",
            OrganizeRecipe::DeveloperTask,
        ),
        ("整理成工作进展", OrganizeRecipe::WorkUpdate),
        ("整理为工作进展", OrganizeRecipe::WorkUpdate),
        ("组织成工作进展", OrganizeRecipe::WorkUpdate),
        ("组织为工作进展", OrganizeRecipe::WorkUpdate),
        ("organize as a progress report", OrganizeRecipe::WorkUpdate),
        ("format as a progress report", OrganizeRecipe::WorkUpdate),
        ("整理成待办", OrganizeRecipe::TodoList),
        ("整理为待办", OrganizeRecipe::TodoList),
        ("整理成待办清单", OrganizeRecipe::TodoList),
        ("整理为待办清单", OrganizeRecipe::TodoList),
        ("列成待办", OrganizeRecipe::TodoList),
        ("列成待办清单", OrganizeRecipe::TodoList),
        ("organize as a to-do list", OrganizeRecipe::TodoList),
        ("organize as a todo list", OrganizeRecipe::TodoList),
        ("format as a to-do list", OrganizeRecipe::TodoList),
        ("format as a todo list", OrganizeRecipe::TodoList),
    ];

    let mut mentions: Vec<RecipeMention> = Vec::new();
    for (cue, recipe) in CUES {
        for start in phrase_occurrences(instruction, cue) {
            let end = start + cue.len();
            if let Some(existing) = mentions
                .iter_mut()
                .find(|mention| mention.start == start && mention.recipe == *recipe)
            {
                existing.end = existing.end.max(end);
            } else {
                mentions.push(RecipeMention {
                    start,
                    end,
                    recipe: *recipe,
                });
            }
        }
    }
    mentions.sort_by_key(|mention| (mention.start, mention.end));
    mentions
}

fn recipe_tail_is_allowed(tail: &str) -> bool {
    let normalized = normalize_recipe_text(tail);
    if normalized.is_empty() {
        return true;
    }
    let mut constraint = normalized.as_str();
    for prefix in ["and also ", "and ", "also ", "while ", "but "] {
        if let Some(rest) = constraint.strip_prefix(prefix) {
            constraint = rest.trim();
            break;
        }
    }
    for prefix in ["并且", "而且", "同时", "且", "并"] {
        if let Some(rest) = constraint.strip_prefix(prefix) {
            constraint = rest.trim();
            break;
        }
    }

    if constraint.is_empty() {
        return false;
    }
    if constraint == "please" {
        return true;
    }

    const ENGLISH_CONSTRAINTS: &[&str] = &[
        "without adding facts",
        "without adding any facts",
        "without inventing facts",
        "without adding content",
        "without adding any content",
        "without adding new content",
        "without inventing content",
        "do not add facts",
        "do not add any facts",
        "don't add facts",
        "don't add any facts",
        "do not invent facts",
        "don't invent facts",
        "do not add content",
        "do not add any content",
        "don't add content",
        "don't add any content",
        "do not invent content",
        "don't invent content",
        "preserve the original wording",
        "keep the original wording",
        "preserve the original title",
        "keep the original title",
        "preserve the source title",
        "keep the source title",
        "preserve the body title",
        "keep the body title",
        "preserve the title",
        "keep the title",
        "preserve all source content",
    ];
    const CHINESE_CONSTRAINTS: &[&str] = &[
        "不要增加内容",
        "不要增加任何内容",
        "不要添加内容",
        "不要添加任何内容",
        "不要补充内容",
        "不要补充任何内容",
        "不要增加事实",
        "不要增加任何事实",
        "不要添加事实",
        "不要补充事实",
        "不要编造事实",
        "不添加新内容",
        "不要新增内容",
        "保留原文",
        "保留原意",
        "保持原文",
        "保持原意",
        "保留标题",
        "保持标题",
        "保留正文标题",
        "保持正文标题",
        "保留正文里的",
        "保留正文中的",
    ];
    ENGLISH_CONSTRAINTS.contains(&constraint) || CHINESE_CONSTRAINTS.contains(&constraint)
}

fn normalize_recipe_text(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut previous_was_space = true;
    for ch in text.chars() {
        if ch.is_whitespace()
            || matches!(
                ch,
                ',' | '，' | ';' | '；' | '.' | '。' | '!' | '！' | '?' | '？'
            )
        {
            if !previous_was_space {
                normalized.push(' ');
            }
            previous_was_space = true;
        } else {
            normalized.push(if ch.is_ascii() {
                ch.to_ascii_lowercase()
            } else {
                ch
            });
            previous_was_space = false;
        }
    }
    normalized.trim().to_owned()
}

fn has_coordinated_recipe_choice(instruction: &str, mentions: &[RecipeMention]) -> bool {
    const LABELS: &[(&str, OrganizeRecipe)] = &[
        ("开发任务", OrganizeRecipe::DeveloperTask),
        ("工作进展", OrganizeRecipe::WorkUpdate),
        ("待办清单", OrganizeRecipe::TodoList),
        ("待办", OrganizeRecipe::TodoList),
        ("developer task", OrganizeRecipe::DeveloperTask),
        ("development task", OrganizeRecipe::DeveloperTask),
        ("progress report", OrganizeRecipe::WorkUpdate),
        ("to-do list", OrganizeRecipe::TodoList),
        ("todo list", OrganizeRecipe::TodoList),
    ];

    let mut spans = Vec::new();
    for (label, recipe) in LABELS {
        for start in phrase_occurrences(instruction, label) {
            spans.push(RecipeLabelSpan {
                start,
                end: start + label.len(),
                recipe: *recipe,
            });
        }
    }
    spans.sort_by_key(|span| (span.start, span.end));

    spans.windows(2).any(|pair| {
        let [left, right] = pair else {
            return false;
        };
        if left.recipe == right.recipe || !has_recipe_cue_overlap(left, right, mentions) {
            return false;
        }
        is_recipe_coordinator_gap(&instruction[left.end..right.start])
    })
}

fn has_recipe_cue_overlap(
    left: &RecipeLabelSpan,
    right: &RecipeLabelSpan,
    mentions: &[RecipeMention],
) -> bool {
    mentions.iter().any(|mention| {
        let overlaps_left = left.start < mention.end && mention.start < left.end;
        let overlaps_right = right.start < mention.end && mention.start < right.end;
        (overlaps_left && mention.recipe == left.recipe)
            || (overlaps_right && mention.recipe == right.recipe)
    })
}

fn is_recipe_coordinator_gap(gap: &str) -> bool {
    let normalized = normalize_recipe_text(gap);
    matches!(
        normalized.as_str(),
        "and"
            | "and a"
            | "and the"
            | "or"
            | "or a"
            | "or the"
            | "and or"
            | "and or a"
            | "或者"
            | "或"
            | "和"
            | "与"
            | "及"
            | "以及"
            | "并且"
    )
}

fn phrase_occurrences(text: &str, phrase: &str) -> Vec<usize> {
    if !phrase.is_ascii() {
        return text.match_indices(phrase).map(|(start, _)| start).collect();
    }

    let folded = text.to_ascii_lowercase();
    let phrase = phrase.to_ascii_lowercase();
    folded
        .match_indices(&phrase)
        .filter_map(|(start, _)| {
            let end = start + phrase.len();
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
            (before_ok && after_ok).then_some(start)
        })
        .collect()
}

fn instruction_cue_is_negated(text: &str, cue_start: usize) -> bool {
    let prefix = &text[..cue_start];
    if action_negative_prefix(prefix) {
        return true;
    }

    // A prior comma or sentence mark cannot erase a direct negative command.
    // Suffix checks still match only a closed negative phrase, never any
    // arbitrary negation elsewhere in the instruction.
    prefix
        .char_indices()
        .filter(|(_, ch)| {
            matches!(
                ch,
                ',' | '，' | ';' | '；' | '.' | '!' | '?' | '。' | '！' | '？' | '\n' | '\r'
            )
        })
        .any(|(index, ch)| action_negative_prefix(&prefix[index + ch.len_utf8()..]))
}

fn recipe_command_prefix_is_allowed(text: &str, cue_start: usize) -> bool {
    let prefix = normalize_action_prefix(&text[..cue_start]);
    const ENGLISH_PREFIXES: &[&str] = &[
        "",
        "please",
        "kindly",
        "could you",
        "could you please",
        "can you",
        "can you please",
        "would you",
        "would you please",
        "please help me",
        "please help me to",
        "help me",
        "help me to",
        "i want to",
        "i want you to",
        "i need to",
        "i need you to",
        "i would like you to",
        "i'd like you to",
        "could you kindly",
        "would you kindly",
        "without adding facts",
        "without adding any facts",
        "without adding content",
        "without adding any content",
        "do not add facts",
        "don't add facts",
        "do not add content",
        "don't add content",
    ];
    const CHINESE_PREFIXES: &[&str] = &[
        "请",
        "请你",
        "请帮我",
        "请你帮我",
        "帮我",
        "麻烦",
        "麻烦你",
        "麻烦帮我",
        "麻烦你帮我",
        "可以帮我",
        "能否帮我",
        "我想",
        "我想把这段",
        "我想把这段文字",
        "请帮我把这段话",
        "请帮我把这段文字",
        "请帮我把这段内容",
        "帮我把这段话",
        "帮我把这段文字",
        "帮我把这段内容",
        "请帮我将这段话",
        "请帮我将这段文字",
        "帮我将这段话",
        "帮我将这段文字",
        "请把这段话",
        "把这段话",
        "请将这段话",
        "将这段话",
        "请将内容",
        "请把内容",
        "将内容",
        "把内容",
        "请把这段",
        "请把这段文字",
        "请将这段",
        "请将这段文字",
        "把这段",
        "把这段文字",
        "将这段",
        "将这段文字",
        "请把这份内容",
        "请将这份内容",
        "把这份内容",
        "将这份内容",
        "请把它",
        "请将它",
        "把它",
        "将它",
        "不要增加内容",
        "不要增加事实",
        "不要添加内容",
        "不要添加事实",
        "不要补充内容",
        "不要补充事实",
    ];
    ENGLISH_PREFIXES.contains(&prefix.as_str()) || CHINESE_PREFIXES.contains(&prefix.as_str())
}

fn action_negative_prefix(prefix: &str) -> bool {
    let mut normalized = normalize_action_prefix(prefix);
    for polite in [
        "could you please ",
        "can you please ",
        "would you please ",
        "please help me to ",
        "please help me ",
        "i would like you to ",
        "i'd like you to ",
        "i want you to ",
        "could you ",
        "can you ",
        "would you ",
        "please ",
        "kindly ",
        "help me to ",
        "help me ",
    ] {
        if let Some(rest) = normalized.strip_prefix(polite) {
            normalized = rest.trim().to_owned();
            break;
        }
    }
    for polite in [
        "麻烦你帮我",
        "麻烦帮我",
        "请你帮我",
        "可以帮我",
        "能否帮我",
        "请你",
        "请帮我",
        "麻烦你",
        "麻烦",
        "请",
    ] {
        if let Some(rest) = normalized.strip_prefix(polite) {
            normalized = rest.trim().to_owned();
            break;
        }
    }

    const ENGLISH_NEGATIVE_PREFIXES: &[&str] = &[
        "do not",
        "do not under any circumstances",
        "do not under no circumstances",
        "do not in any circumstances",
        "do not in any case",
        "do not under any condition",
        "do not by any means",
        "do not at any time",
        "do not ever",
        "don't",
        "don't under any circumstances",
        "don't in any case",
        "don't under any condition",
        "don't by any means",
        "don't under no circumstances",
        "don't ever",
        "does not",
        "doesn't",
        "will not",
        "will not under any circumstances",
        "won't",
        "won't under any circumstances",
        "would not",
        "wouldn't",
        "should not",
        "shouldn't",
        "must not",
        "mustn't",
        "cannot",
        "can't",
        "never",
        "never under any circumstances",
        "avoid",
        "try not to",
        "not",
        "i do not",
        "i don't",
        "i will not",
        "i won't",
        "i would not",
        "i wouldn't",
        "i should not",
        "i shouldn't",
        "you do not",
        "you don't",
        "you will not",
        "you won't",
    ];
    if ENGLISH_NEGATIVE_PREFIXES.contains(&normalized.as_str()) {
        return true;
    }

    let compact_chinese = normalized
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    const CHINESE_NEGATIVE_PREFIXES: &[&str] = &[
        "不要",
        "不要在任何情况下",
        "别",
        "别在任何情况下",
        "勿",
        "请勿",
        "切勿",
        "禁止",
        "千万不要",
        "绝不要",
        "不",
    ];
    CHINESE_NEGATIVE_PREFIXES.contains(&compact_chinese.as_str())
}

fn normalize_action_prefix(prefix: &str) -> String {
    let mut normalized = String::with_capacity(prefix.len());
    let mut previous_was_space = true;
    for ch in prefix.chars() {
        let separator = ch.is_whitespace()
            || matches!(
                ch,
                ',' | '，'
                    | ';'
                    | '；'
                    | ':'
                    | '：'
                    | '.'
                    | '!'
                    | '?'
                    | '。'
                    | '！'
                    | '？'
                    | '('
                    | ')'
                    | '（'
                    | '）'
            );
        if separator {
            if !previous_was_space {
                normalized.push(' ');
            }
            previous_was_space = true;
        } else {
            normalized.push(if ch.is_ascii() {
                ch.to_ascii_lowercase()
            } else {
                ch
            });
            previous_was_space = false;
        }
    }
    normalized.trim().to_owned()
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
    fn organize_recipe_requires_an_explicit_unquoted_unambiguous_command() {
        let source = "One source paragraph.";
        let cases = [
            ("整理成开发任务", OrganizeRecipe::DeveloperTask),
            (
                "Organize as a development task",
                OrganizeRecipe::DeveloperTask,
            ),
            ("整理成工作进展", OrganizeRecipe::WorkUpdate),
            ("Organize as a progress report", OrganizeRecipe::WorkUpdate),
            ("整理成待办清单", OrganizeRecipe::TodoList),
            ("Format as a to-do list", OrganizeRecipe::TodoList),
        ];
        for (instruction, expected) in cases {
            let plan = make_plan(instruction, source).expect("explicit recipe command");
            assert_eq!(
                plan.operation,
                TextActionOperation::Organize,
                "{instruction}"
            );
            assert_eq!(plan.recipe, Some(expected), "{instruction}");
        }

        let generic = make_plan("Organize as bullet points", "整理成待办").unwrap();
        assert_eq!(generic.operation, TextActionOperation::Organize);
        assert_eq!(generic.recipe, None, "source words do not select a recipe");
        let generic_quoted = make_plan("Format the text as ‘to-do list’", source).unwrap();
        assert_eq!(
            generic_quoted.recipe, None,
            "quoted words do not select a recipe"
        );
        assert_eq!(
            make_plan("Please say ‘整理成待办’", source).unwrap_err(),
            TextActionPlanError::UnsupportedInstruction,
            "a quoted-only recipe phrase is not an action"
        );

        for instruction in [
            "不要整理成待办",
            "Do not under any circumstances organize as a developer task",
            "Do not, under any circumstances, organize as a developer task",
            "I won't organize as a developer task",
            "Please do not organize as a developer task",
            "Organize as a developer task is not what I want",
            "整理成开发任务不是我要的",
            "整理成开发任务我不需要",
            "整理成开发任务这个不行",
            "整理成开发任务不要",
        ] {
            assert_eq!(
                make_plan(instruction, source).unwrap_err(),
                TextActionPlanError::AmbiguousInstruction,
                "negative recipe instruction must fail closed: {instruction}"
            );
        }

        for (instruction, expected) in [
            ("整理成待办并且不要增加内容", OrganizeRecipe::TodoList),
            ("整理成待办，且不要补充事实。", OrganizeRecipe::TodoList),
            ("请帮我整理成待办并且不要增加内容", OrganizeRecipe::TodoList),
            (
                "Please organize as a to-do list without adding facts",
                OrganizeRecipe::TodoList,
            ),
            (
                "Could you please organize as a to-do list without adding facts",
                OrganizeRecipe::TodoList,
            ),
            (
                "Organize as a to-do list, and do not add any content.",
                OrganizeRecipe::TodoList,
            ),
            (
                "整理成开发任务，保留正文里的‘工作进展和待办清单’",
                OrganizeRecipe::DeveloperTask,
            ),
            (
                "Organize as a developer task and preserve the title ‘work update and to-do list’",
                OrganizeRecipe::DeveloperTask,
            ),
        ] {
            assert_eq!(
                make_plan(instruction, "Review the draft.").unwrap().recipe,
                Some(expected),
                "a bounded fidelity constraint does not negate the recipe: {instruction}"
            );
        }
        for instruction in [
            "Organize as a to-do list and add a deadline",
            "整理成待办然后补充负责人",
            "整理成待办和",
            "整理成待办并且",
            "整理成待办且",
            "整理成待办并",
            "整理成待办或者日历事项",
            "Organize as a developer task and",
            "Organize as a developer task or",
            "Organize as a developer task or a calendar item",
            "Organize as a to-do list without adding facts, but do not do that",
        ] {
            assert_eq!(
                make_plan(instruction, source).unwrap_err(),
                TextActionPlanError::AmbiguousInstruction,
                "an unrecognized recipe tail fails closed: {instruction}"
            );
        }

        let quoted_other_recipe = make_plan(
            "‘developer task and work update’; please organize as a to-do list",
            "One source paragraph.",
        )
        .unwrap();
        assert_eq!(quoted_other_recipe.recipe, Some(OrganizeRecipe::TodoList));
        let quoted_title_reference = make_plan(
            "Organize as a to-do list and preserve the original title ‘developer task and work update’",
            "One source paragraph.",
        )
        .unwrap();
        assert_eq!(
            quoted_title_reference.recipe,
            Some(OrganizeRecipe::TodoList)
        );
        let source_mentions_other_recipes = make_plan(
            "整理成待办",
            "Developer task and work update are source words.",
        )
        .unwrap();
        assert_eq!(
            source_mentions_other_recipes.recipe,
            Some(OrganizeRecipe::TodoList)
        );

        for instruction in [
            "整理成开发任务和工作进展",
            "整理成待办与开发任务",
            "整理成开发任务及待办清单",
            "Organize as a developer task and a progress report",
            "Format as a to-do list and a development task",
        ] {
            assert_eq!(
                make_plan(instruction, source).unwrap_err(),
                TextActionPlanError::AmbiguousInstruction,
                "shared-verb recipe coordination cannot silently select one: {instruction}"
            );
        }

        for instruction in [
            "整理成开发任务或工作进展",
            "Organize as a developer task or a progress report",
        ] {
            assert_eq!(
                make_plan(instruction, source).unwrap_err(),
                TextActionPlanError::AmbiguousInstruction,
                "a coordinated recipe choice must not silently choose one: {instruction}"
            );
        }

        for instruction in [
            "Rewrite and organize as a to-do list",
            "Shorten and organize as a to-do list",
            "Translate this and organize as a to-do list",
            "Draft a reply and organize as a to-do list",
            "Replace React with Preact and organize as a to-do list",
        ] {
            assert_eq!(
                make_plan(instruction, "React is ready.").unwrap_err(),
                TextActionPlanError::AmbiguousInstruction,
                "a recipe cannot accompany another operation: {instruction}"
            );
        }

        let legacy = make_plan("Don't organize this", source).unwrap();
        assert_eq!(legacy.operation, TextActionOperation::Organize);
        assert_eq!(
            legacy.recipe, None,
            "generic Organize detection stays unchanged"
        );
        assert_eq!(
            make_plan("Organize as a work update", source).unwrap_err(),
            TextActionPlanError::AmbiguousInstruction,
            "the standalone update verb remains protected by exact-change detection"
        );
    }

    #[test]
    fn recipe_plan_binds_its_closed_identity_to_result_validation() {
        let source = "API rollout is pending.";
        let mut plan = make_plan("整理成开发任务", source).unwrap();
        assert_eq!(
            validate_generated_result(&plan, source, "开发任务\n- API rollout is pending."),
            Ok(())
        );
        plan.recipe = Some(OrganizeRecipe::TodoList);
        assert_eq!(
            validate_generated_result(&plan, source, "待办清单\n- API rollout is pending."),
            Err(TextActionGuardError::UnauthorizedEntityChange),
            "changing recipe identity invalidates old source/instruction evidence"
        );
    }

    #[test]
    fn recipe_layout_proof_allows_only_fixed_title_and_source_order_paragraph_markers() {
        let source = "Goal: API v1 is not ready.\n\nNext: keep React.";
        let plan = make_plan("整理成开发任务", source).unwrap();
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "开发任务\n- Goal: API v1 is not ready.\n\n- Next: keep React."
            ),
            Ok(())
        );

        for candidate in [
            "Developer task: API v1\n- Goal: API v1 is not ready.\n\n- Next: keep React.",
            "未知标题\n- Goal: API v1 is not ready.\n\n- Next: keep React.",
            "开发任务\n- Goal: API v1 is ready.\n\n- Next: keep React.",
            "开发任务\n- Next: keep React.\n\n- Goal: API v1 is not ready.",
            "开发任务\n- Goal: API v1 is not ready.\n\n- Next: keep React.\n- Ship Friday.",
            "开发任务\n- Goal: API v1 is not ready.",
        ] {
            assert_eq!(
                validate_generated_result(&plan, source, candidate),
                Err(TextActionGuardError::ProtectedFactChanged),
                "recipe body must preserve source exactly: {candidate}"
            );
        }

        let source = "- Send the report.";
        let plan = make_plan("整理成待办", source).unwrap();
        let candidate = "待办清单\n- - Send the report.";
        assert_eq!(
            recipe_body_from_candidate(OrganizeRecipe::TodoList, candidate),
            Some(expected_recipe_body(source).as_str()),
            "the title and exact source layout proof already pass"
        );
        assert_eq!(
            validate_generated_result(&plan, source, candidate),
            Ok(()),
            "the exact added outer marker must not change source-initial action-lead classification"
        );
        for candidate in [
            "待办清单\n* - Send the report.",
            "待办清单\n-  - Send the report.",
            "待办清单\n- - Send the report.\n- extra",
        ] {
            assert_eq!(
                validate_generated_result(&plan, source, candidate),
                Err(TextActionGuardError::ProtectedFactChanged),
                "layout projection is authorized only for exact witnessed markers: {candidate:?}"
            );
        }

        let unicode_source = "预算1500元。\n\n下一步：Review API";
        let unicode_plan = make_plan("整理成待办", unicode_source).unwrap();
        assert_eq!(
            validate_generated_result(
                &unicode_plan,
                unicode_source,
                "待办清单\n- 预算1500元。\n\n- 下一步：Review API"
            ),
            Ok(()),
            "witness byte ranges preserve Unicode source exactly"
        );
        assert_eq!(
            validate_generated_result(
                &unicode_plan,
                unicode_source,
                "待办清单\n- 预算1500元。\n\n- 下一步：Review Api"
            ),
            Err(TextActionGuardError::ProtectedFactChanged),
            "Unicode body mutation remains outside the layout projection"
        );

        let duplicate_source = "One item.\n\nOne item.";
        let duplicate_plan = make_plan("整理成待办", duplicate_source).unwrap();
        assert_eq!(
            validate_generated_result(&duplicate_plan, duplicate_source, "待办清单\n- One item."),
            Err(TextActionGuardError::ProtectedFactChanged),
            "repeated source paragraphs cannot be deleted"
        );
        let source = "can not run.";
        let plan = make_plan("整理成待办", source).unwrap();
        assert_eq!(
            validate_generated_result(&plan, source, "待办清单\n- cannot run."),
            Err(TextActionGuardError::ProtectedFactChanged),
            "whitespace inside source words is not normalized away"
        );

        for (instruction, source, candidate) in [
            (
                "Organize as a developer task",
                "Ship API v2 on Friday.",
                "Developer task\n- Ship API v2 on Friday.",
            ),
            (
                "Organize as a progress report",
                "API rollout is pending.",
                "Work update\n- API rollout is pending.",
            ),
            (
                "Format as a to-do list",
                "Review the migration.",
                "To-do list\n- Review the migration.",
            ),
        ] {
            let plan = make_plan(instruction, source).unwrap();
            assert_eq!(
                validate_generated_result(&plan, source, candidate),
                Ok(()),
                "the fixed English recipe title is valid: {candidate}"
            );
        }

        let source = "下一步甲；完成乙";
        let plan = make_plan("整理成工作进展", source).unwrap();
        assert_eq!(
            validate_generated_result(&plan, source, "工作进展\n- 完成甲；下一步乙"),
            Err(TextActionGuardError::ProtectedFactChanged),
            "recipe proof rejects changed attribution/order without protected terms"
        );
        let source = "明天上线";
        let plan = make_plan("整理成开发任务", source).unwrap();
        assert_eq!(
            validate_generated_result(&plan, source, "开发任务\n- 已经上线"),
            Err(TextActionGuardError::ProtectedFactChanged),
            "recipe proof rejects changed facts without protected terms"
        );
    }

    #[test]
    fn recipe_layout_proof_preserves_source_whitespace_titles_and_code() {
        let source = "\tFirst  \r\n\r\n  Second\r\n";
        let plan = make_plan("整理成待办", source).unwrap();
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "待办清单\r\n- \tFirst  \r\n\r\n-   Second\r\n"
            ),
            Ok(())
        );

        let source = "# Work update\n- API v1 is pending.";
        let plan = make_plan("整理成工作进展", source).unwrap();
        assert_eq!(
            validate_generated_result(
                &plan,
                source,
                "工作进展\n- # Work update\n- API v1 is pending."
            ),
            Ok(()),
            "source-owned title text remains in the body after title projection"
        );
        assert_eq!(
            validate_generated_result(&plan, source, "工作进展\n- API v1 is pending."),
            Err(TextActionGuardError::ProtectedFactChanged),
            "the recipe title cannot erase a source-owned title"
        );

        let source = "Run `cargo test`.\n  cargo fmt";
        assert!(protected_span::has_code_literals(source));
        let plan = make_plan("整理成开发任务", source).unwrap();
        assert_eq!(
            validate_generated_result(&plan, source, &format!("开发任务\n{source}")),
            Ok(()),
            "code source stays byte-for-byte, without a list prefix"
        );
        assert_eq!(
            validate_generated_result(&plan, source, &format!("开发任务\n- {source}")),
            Err(TextActionGuardError::ProtectedFactChanged),
            "recipe formatting cannot change code indentation or wrapper layout"
        );
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
    fn recipe_output_budget_is_checked_before_dispatch_for_every_allowed_title() {
        let cases = [
            (
                "整理成开发任务",
                MAX_ACTION_SOURCE_BYTES - 14 - 2 - 2,
                "Developer task",
                "开发任务",
            ),
            (
                "整理成工作进展",
                MAX_ACTION_SOURCE_BYTES - 12 - 2 - 2,
                "工作进展",
                "Work update",
            ),
            (
                "整理成待办",
                MAX_ACTION_SOURCE_BYTES - 12 - 2 - 2,
                "待办清单",
                "To-do list",
            ),
        ];
        for (instruction, exact_source_len, longest_title, second_title) in cases {
            let exact_source = "a".repeat(exact_source_len);
            let plan = make_plan(instruction, &exact_source)
                .expect("the largest accepted title plus CRLF and layout fits exactly");
            for title in [longest_title, second_title] {
                for separator in ["\n", "\r\n"] {
                    let candidate = format!("{title}{separator}- {exact_source}");
                    assert!(candidate.len() <= MAX_ACTION_SOURCE_BYTES);
                    if title == longest_title && separator == "\r\n" {
                        assert_eq!(candidate.len(), MAX_ACTION_SOURCE_BYTES);
                    }
                    assert_eq!(
                        validate_generated_result(&plan, &exact_source, &candidate),
                        Ok(()),
                        "every fixed title and accepted header separator must fit"
                    );
                }
            }

            let one_byte_too_long = "a".repeat(exact_source_len + 1);
            assert_eq!(
                make_plan(instruction, &one_byte_too_long).unwrap_err(),
                TextActionPlanError::SourceTooLong,
                "the longest accepted title plus CRLF and body exceeds the cap by one byte"
            );
        }

        let many_paragraphs = format!("{}a", "a\n\n".repeat(5_000));
        assert!(many_paragraphs.len() < MAX_ACTION_SOURCE_BYTES);
        assert_eq!(
            make_plan("整理成待办", &many_paragraphs).unwrap_err(),
            TextActionPlanError::SourceTooLong,
            "all inserted paragraph markers are included in the dispatch budget"
        );

        let advertised_source_limit = "a".repeat(MAX_ACTION_SOURCE_BYTES);
        assert_eq!(
            make_plan("整理成待办", &advertised_source_limit).unwrap_err(),
            TextActionPlanError::SourceTooLong,
            "a source accepted by the capture limit but unable to fit any complete recipe output is rejected before dispatch"
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
