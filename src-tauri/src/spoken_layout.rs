//! Spoken line, list, and email-block layout applied after punctuation and
//! before lexicon / LLM cleanup.

use crate::context::ContextFamily;

const LINE_COMMANDS: &[(&str, &str)] = &[
    ("start a new paragraph", "\n\n"),
    ("new paragraph", "\n\n"),
    ("skip a line", "\n"),
    ("line break", "\n"),
    ("next line", "\n"),
    ("new line", "\n"),
    ("new-line", "\n"),
    ("newline", "\n"),
    ("另起一段", "\n\n"),
    ("新段落", "\n\n"),
    ("另起一行", "\n"),
    ("下一行", "\n"),
    ("换行", "\n"),
];

const MAX_COUNT_ONLY_RETAINED_ITEMS: usize = 10;
const MAX_COUNT_ONLY_SOURCE_ITEMS: usize = 20;

/// Apply spoken layout to already-punctuated ASR text.
#[cfg(test)]
pub fn apply(text: &str, family: ContextFamily, confidence: f32) -> String {
    apply_layout_stages(text, family, confidence, false).text
}

pub(crate) struct AppliedLayout {
    pub(crate) text: String,
    pub(crate) source_preserved: bool,
    pub(crate) list_revision_proof: Option<ListLayoutProof>,
    /// Equivalent local preparation with only family-dependent email grouping
    /// suppressed. This remains separate from the canonical text/proof.
    pub(crate) projection_text: String,
    pub(crate) projection_source_preserved: bool,
    pub(crate) projection_list_revision_proof: Option<ListLayoutProof>,
    pub(crate) scene_email_layout_applied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListLayoutProof {
    source: String,
    layout_output: String,
    consumed_syntax_ranges: Vec<std::ops::Range<usize>>,
    revisions: Vec<ListItemRevision>,
    retention: Option<ListItemRetention>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ListItemRevision {
    ordinal: usize,
    rendered_ordinal: usize,
    target_marker_range: std::ops::Range<usize>,
    placeholder_marker_range: Option<std::ops::Range<usize>>,
    reference_marker_range: std::ops::Range<usize>,
    instruction_range: std::ops::Range<usize>,
    cue_range: std::ops::Range<usize>,
    cue_value: String,
    copula_range: std::ops::Range<usize>,
    copula_value: String,
    source_item_range: std::ops::Range<usize>,
    source_value: String,
    replacement_source_range: std::ops::Range<usize>,
    replacement_prefix: String,
    replacement_value: String,
    output_value_range: std::ops::Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ListRetentionReference {
    ordinal: usize,
    marker_range: std::ops::Range<usize>,
    marker_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RetainedListItem {
    ordinal: usize,
    rendered_ordinal: usize,
    marker_range: std::ops::Range<usize>,
    source_value_range: std::ops::Range<usize>,
    source_value: String,
    output_value_range: std::ops::Range<usize>,
    output_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemovedListItem {
    ordinal: usize,
    marker_range: std::ops::Range<usize>,
    source_value_range: std::ops::Range<usize>,
    source_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RetainedOrdinalLinker {
    ordinal: usize,
    source_range: std::ops::Range<usize>,
    output_range: std::ops::Range<usize>,
    canonicalized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ListItemRetention {
    form: ListRetentionForm,
    directive_range: std::ops::Range<usize>,
    directive_value: String,
    directive_suffix_range: Option<std::ops::Range<usize>>,
    cue_range: std::ops::Range<usize>,
    cue_value: String,
    action_range: std::ops::Range<usize>,
    action_value: String,
    count_range: std::ops::Range<usize>,
    count_value: String,
    retained_count: usize,
    directive_ordinal: Option<usize>,
    references: Vec<ListRetentionReference>,
    retained_items: Vec<RetainedListItem>,
    removed_items: Vec<RemovedListItem>,
    leading_restart: Option<EmptyOrdinalRestart>,
    ordinal_linker: Option<RetainedOrdinalLinker>,
}

/// A source-bound, empty ordinal that was abandoned before a clear restart at
/// item one. This can be ignored only when the downstream retention proof
/// also validates the restarted list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EmptyOrdinalRestart {
    source_range: std::ops::Range<usize>,
    marker_range: std::ops::Range<usize>,
    marker_value: String,
    filler_range: std::ops::Range<usize>,
    filler_value: String,
    restarted_marker_range: std::ops::Range<usize>,
    restarted_marker_value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListRetentionForm {
    ExplicitReferences,
    TerminalDeicticCount,
    CountOnly,
    RemoveOrdinal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ListRetentionDirective {
    form: ListRetentionForm,
    range: std::ops::Range<usize>,
    cue_range: std::ops::Range<usize>,
    action_range: std::ops::Range<usize>,
    count_range: std::ops::Range<usize>,
    keep_count: usize,
    references: Vec<ListRetentionReference>,
}

impl ListLayoutProof {
    pub(crate) fn is_valid(&self) -> bool {
        (!self.revisions.is_empty() || self.retention.is_some())
            && preserves_source_protected_content_after_layout_with_revisions(
                &self.source,
                &self.layout_output,
                &self.consumed_syntax_ranges,
                &self.revisions,
                self.retention.as_ref(),
            )
    }

    /// Remove only the proved adjacent-correction linker from the rendered
    /// transcript, then update every retained-item and correction output span
    /// before accepting the changed proof.
    pub(crate) fn canonicalize_ordinal_linker(&mut self) -> Option<String> {
        if !self.is_valid() {
            return None;
        }
        let mut updated = self.clone();
        let retention = updated.retention.as_mut()?;
        let linker = retention.ordinal_linker.as_mut()?;
        if linker.canonicalized {
            return None;
        }
        let range = linker.output_range.clone();
        if updated.layout_output.get(range.clone()) != Some("才是") {
            return None;
        }
        let item = retention
            .retained_items
            .iter_mut()
            .find(|item| item.ordinal == linker.ordinal)?;
        if item.output_value_range.start != range.start
            || item.output_value_range.end < range.end
            || item.output_value.get(.."才是".len()) != Some("才是")
        {
            return None;
        }
        let mut output = updated.layout_output.clone();
        output.replace_range(range.clone(), "");
        item.output_value.replace_range(.."才是".len(), "");
        item.output_value_range.end -= "才是".len();
        for retained in &mut retention.retained_items {
            if retained.ordinal == linker.ordinal {
                continue;
            }
            if retained.output_value_range.start >= range.end {
                retained.output_value_range.start -= "才是".len();
                retained.output_value_range.end -= "才是".len();
            } else if retained.output_value_range.end > range.start {
                return None;
            }
        }
        for revision in &mut updated.revisions {
            if revision.output_value_range.start >= range.end {
                revision.output_value_range.start -= "才是".len();
                revision.output_value_range.end -= "才是".len();
            } else if revision.output_value_range.end > range.start {
                return None;
            }
        }
        linker.output_range = range.start..range.start;
        linker.canonicalized = true;
        updated.layout_output = output;
        if !updated.is_valid() {
            return None;
        }
        *self = updated;
        Some(self.layout_output.clone())
    }
}

struct LayoutStage {
    text: String,
    consumed_syntax_ranges: Vec<std::ops::Range<usize>>,
}

struct LayoutPipeline {
    text: String,
    source_preserved: bool,
    list_revision_proof: Option<ListLayoutProof>,
    projection_text: String,
    projection_source_preserved: bool,
    projection_list_revision_proof: Option<ListLayoutProof>,
    scene_email_layout_applied: bool,
}

impl LayoutStage {
    fn unchanged(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            consumed_syntax_ranges: Vec::new(),
        }
    }
}

fn apply_layout_stages(
    source: &str,
    family: ContextFamily,
    confidence: f32,
    guard_source: bool,
) -> LayoutPipeline {
    let mut current = source.to_owned();
    let mut source_preserved = true;
    for transform in [
        apply_heading_body_stage as fn(&str) -> LayoutStage,
        apply_labeled_fields_stage,
        apply_following_three_points_stage,
        apply_paragraph_markers_stage,
        apply_line_commands_stage,
    ] {
        let stage = transform(&current);
        apply_layout_stage(&mut current, stage, guard_source, &mut source_preserved);
    }
    let projection_source = current.clone();
    let projection_source_preserved = source_preserved;
    let mut scene_email_layout_applied = false;
    if family == ContextFamily::Email && confidence >= 0.75 {
        let before = current.clone();
        let stage = apply_email_layout_stage(&current);
        apply_layout_stage(&mut current, stage, guard_source, &mut source_preserved);
        scene_email_layout_applied = current != before;
    }
    let canonical =
        apply_list_and_backtrack_stages(current, family, guard_source, source_preserved);
    let projection = if scene_email_layout_applied {
        apply_list_and_backtrack_stages(
            projection_source,
            family,
            guard_source,
            projection_source_preserved,
        )
    } else {
        LayoutTail {
            text: canonical.text.clone(),
            source_preserved: canonical.source_preserved,
            list_revision_proof: canonical.list_revision_proof.clone(),
        }
    };
    LayoutPipeline {
        text: canonical.text,
        source_preserved: canonical.source_preserved,
        list_revision_proof: canonical.list_revision_proof,
        projection_text: projection.text,
        projection_source_preserved: projection.source_preserved,
        projection_list_revision_proof: projection.list_revision_proof,
        scene_email_layout_applied,
    }
}

struct LayoutTail {
    text: String,
    source_preserved: bool,
    list_revision_proof: Option<ListLayoutProof>,
}

fn apply_list_and_backtrack_stages(
    mut current: String,
    family: ContextFamily,
    guard_source: bool,
    mut source_preserved: bool,
) -> LayoutTail {
    let mut list_revision_proof = None;
    let (lists, revisions, retention) = apply_lists_stage(&current, family);
    let list_source = current.clone();
    let list_output = lists.text.clone();
    let lists_syntax_ranges = lists.consumed_syntax_ranges.clone();
    let accepted = apply_layout_stage_with_revisions(
        &mut current,
        lists,
        guard_source,
        &mut source_preserved,
        &revisions,
        retention.as_ref(),
    );
    if accepted && (!revisions.is_empty() || retention.is_some()) {
        list_revision_proof = Some(ListLayoutProof {
            source: list_source,
            layout_output: list_output,
            consumed_syntax_ranges: lists_syntax_ranges,
            revisions,
            retention,
        });
    }
    let backtracks = apply_backtracks_stage(&current);
    apply_layout_stage(
        &mut current,
        backtracks,
        guard_source,
        &mut source_preserved,
    );
    LayoutTail {
        text: current,
        source_preserved,
        list_revision_proof,
    }
}

fn apply_layout_stage(
    current: &mut String,
    stage: LayoutStage,
    guard_source: bool,
    source_preserved: &mut bool,
) {
    let _ = apply_layout_stage_with_revisions(
        current,
        stage,
        guard_source,
        source_preserved,
        &[],
        None,
    );
}

fn apply_layout_stage_with_revisions(
    current: &mut String,
    stage: LayoutStage,
    guard_source: bool,
    source_preserved: &mut bool,
    list_revisions: &[ListItemRevision],
    retention: Option<&ListItemRetention>,
) -> bool {
    if guard_source
        && !preserves_source_protected_content_after_layout_with_revisions(
            current,
            &stage.text,
            &stage.consumed_syntax_ranges,
            list_revisions,
            retention,
        )
    {
        *source_preserved = false;
        return false;
    }
    *current = stage.text;
    true
}

fn apply_heading_body_stage(text: &str) -> LayoutStage {
    let Some(parsed) = parse_heading_body(text) else {
        return LayoutStage::unchanged(text);
    };
    LayoutStage {
        text: format!("{}\n\n{}", parsed.title, parsed.body),
        consumed_syntax_ranges: parsed.syntax_ranges.to_vec(),
    }
}

struct ParsedHeadingBody<'a> {
    title: &'a str,
    body: &'a str,
    syntax_ranges: Vec<std::ops::Range<usize>>,
}

fn parse_heading_body(text: &str) -> Option<ParsedHeadingBody<'_>> {
    let trimmed_start = text.len() - text.trim_start().len();
    if is_inside_explicit_quote(text, trimmed_start) || is_inside_code(text, trimmed_start) {
        return None;
    }
    let trimmed = text.trim();
    let (title_start, body_markers) = if trimmed.starts_with("标题是") {
        ("标题是".len(), &["正文是", "正文"][..])
    } else if trimmed.starts_with("标题叫") {
        ("标题叫".len(), &["正文是", "正文"][..])
    } else if starts_with_ignore_ascii_case(trimmed, "heading ") {
        ("heading ".len(), &["body is", "body"][..])
    } else if starts_with_ignore_ascii_case(trimmed, "title ") {
        let title_len = if starts_with_ignore_ascii_case(trimmed, "title is ") {
            "title is ".len()
        } else {
            "title ".len()
        };
        (title_len, &["body is", "body"][..])
    } else if starts_with_ignore_ascii_case(trimmed, "title: ") {
        ("title: ".len(), &["body:", "body is", "body"][..])
    } else {
        return None;
    };
    let remainder = &trimmed[title_start..];
    let (body_at, body_marker) = body_markers
        .iter()
        .filter_map(|marker| {
            find_unquoted_marker_with_scope(remainder, trimmed_start + title_start, text, marker)
                .map(|index| (index, *marker))
        })
        .min_by_key(|(index, _)| *index)?;
    let title_offset = trimmed_start + title_start;
    let (title, mut syntax_ranges) =
        trim_layout_label_with_ranges(&remainder[..body_at], title_offset);
    let body_offset = title_offset + body_at + body_marker.len();
    let (body, body_ranges) =
        trim_layout_label_with_ranges(&remainder[body_at + body_marker.len()..], body_offset);
    if title.is_empty() || body.is_empty() {
        return None;
    }
    syntax_ranges.extend(body_ranges);
    syntax_ranges.push(trimmed_start..trimmed_start + title_start);
    syntax_ranges.push(title_offset + body_at..title_offset + body_at + body_marker.len());
    Some(ParsedHeadingBody {
        title,
        body,
        syntax_ranges,
    })
}

fn apply_paragraph_markers_stage(text: &str) -> LayoutStage {
    let Some(parsed) = parse_paragraph_markers(text) else {
        return LayoutStage::unchanged(text);
    };
    LayoutStage {
        text: format!("{}\n\n{}", parsed.first, parsed.second),
        consumed_syntax_ranges: parsed.syntax_ranges.to_vec(),
    }
}

struct ParsedParagraphMarkers<'a> {
    first: &'a str,
    second: &'a str,
    syntax_ranges: Vec<std::ops::Range<usize>>,
}

fn parse_paragraph_markers(text: &str) -> Option<ParsedParagraphMarkers<'_>> {
    let trimmed_start = text.len() - text.trim_start().len();
    if is_inside_explicit_quote(text, trimmed_start) || is_inside_code(text, trimmed_start) {
        return None;
    }
    let trimmed = text.trim_start();
    if !starts_with_ignore_ascii_case(trimmed, "first paragraph") {
        return None;
    }
    let first_len = "first paragraph".len();
    let (second_at, second_len) = find_any_unquoted_marker_with_scope(
        &trimmed[first_len..],
        trimmed_start + first_len,
        text,
        &["second paragraph"],
    )?;
    let split_at = first_len + second_at;
    let first_offset = trimmed_start + first_len;
    let (first, mut syntax_ranges) =
        trim_layout_label_with_ranges(&trimmed[first_len..split_at], first_offset);
    let second_offset = trimmed_start + split_at + second_len;
    let (second, second_ranges) =
        trim_layout_label_with_ranges(&trimmed[split_at + second_len..], second_offset);
    if first.is_empty() || second.is_empty() {
        return None;
    }
    syntax_ranges.extend(second_ranges);
    syntax_ranges.push(trimmed_start..trimmed_start + first_len);
    syntax_ranges.push(trimmed_start + split_at..trimmed_start + split_at + second_len);
    Some(ParsedParagraphMarkers {
        first,
        second,
        syntax_ranges,
    })
}

fn apply_labeled_fields_stage(text: &str) -> LayoutStage {
    const ENGLISH_FIELDS: &[(&str, &str)] = &[
        ("owner", "Owner"),
        ("deadline", "Deadline"),
        ("risk", "Risk"),
        ("status", "Status"),
        ("blocker", "Blocker"),
        ("next step", "Next step"),
    ];
    const CHINESE_FIELDS: &[(&str, &str)] = &[
        ("负责人", "负责人"),
        ("截止日期", "截止日期"),
        ("风险", "风险"),
        ("状态", "状态"),
        ("阻塞项", "阻塞项"),
        ("下一步", "下一步"),
    ];
    let needs = find_unquoted_marker(text, "needs");
    let cjk_needs = find_unquoted_marker(text, "需要");
    let (fields, needs_at, needs_len) = if let Some(index) = needs {
        (ENGLISH_FIELDS, index, "needs".len())
    } else if let Some(index) = cjk_needs {
        (CHINESE_FIELDS, index, "需要".len())
    } else {
        return LayoutStage::unchanged(text);
    };
    let tail_offset = needs_at + needs_len;
    let tail = &text[tail_offset..];
    let mut found = fields
        .iter()
        .filter_map(|(label, display)| {
            find_unquoted_marker_with_scope(tail, tail_offset, text, label).map(|relative| {
                (
                    needs_at + needs_len + relative,
                    label.len(),
                    *label,
                    *display,
                )
            })
        })
        .collect::<Vec<_>>();
    found.sort_by_key(|item| item.0);
    if found.len() < 2 {
        return LayoutStage::unchanged(text);
    }
    let first_label_start = found[0].0;
    let before_label = text[needs_at + needs_len..first_label_start]
        .trim_matches(|ch: char| ch.is_whitespace() || matches!(ch, ':' | '：' | ',' | '，'));
    if !before_label.is_empty() {
        return LayoutStage::unchanged(text);
    }
    let preamble = text[..needs_at + needs_len].trim_end();
    let mut items = Vec::new();
    let mut consumed_syntax_ranges = found
        .iter()
        .map(|(start, label_len, _, _)| *start..*start + *label_len)
        .collect::<Vec<_>>();
    for (index, (start, label_len, _label, display)) in found.iter().enumerate() {
        let value_start = start + label_len;
        let value_end = found
            .get(index + 1)
            .map(|next| next.0)
            .unwrap_or(text.len());
        let (value, removed_syntax) = trim_field_value_with_ranges(
            &text[value_start..value_end],
            fields.as_ptr() == ENGLISH_FIELDS.as_ptr(),
        );
        if value.is_empty() {
            return LayoutStage::unchanged(text);
        }
        consumed_syntax_ranges.extend(
            removed_syntax
                .into_iter()
                .map(|range| value_start + range.start..value_start + range.end),
        );
        items.push(format!("- {display}: {value}"));
    }
    LayoutStage {
        text: format!("{preamble}:\n{}", items.join("\n")),
        consumed_syntax_ranges,
    }
}

fn trim_field_value_with_ranges(
    value: &str,
    english_fields: bool,
) -> (String, Vec<std::ops::Range<usize>>) {
    let mut value_offset = value.len() - value.trim_start_matches(is_layout_label_char).len();
    let (mut value, mut ranges) = trim_layout_label_with_ranges(value, 0);
    let leading = value.len()
        - value
            .trim_start_matches(|ch: char| {
                ch.is_whitespace() || matches!(ch, ',' | '，' | ';' | '；')
            })
            .len();
    if leading > 0 {
        ranges.push(value_offset..value_offset + leading);
        value = &value[leading..];
        value_offset += leading;
    }
    if english_fields {
        if let Some(rest) = strip_ascii_prefix_ignore_case(value, "and ") {
            ranges.push(value_offset..value_offset + "and ".len());
            value_offset += "and ".len();
            value = rest;
        }
        if strip_ascii_suffix_ignore_case(value, " and").is_some() {
            let start = value.len() - " and".len();
            ranges.push(value_offset + start..value_offset + value.len());
            value = &value[..start];
        }
    } else {
        if let Some(rest) = value.strip_prefix("以及") {
            ranges.push(value_offset..value_offset + "以及".len());
            value_offset += "以及".len();
            value = rest;
        }
        if value.ends_with("以及") {
            let start = value.len() - "以及".len();
            ranges.push(value_offset + start..value_offset + value.len());
            value = &value[..start];
        }
    }
    let (value, trim_ranges) = trim_layout_label_with_ranges(value, value_offset);
    ranges.extend(trim_ranges);
    (value.to_owned(), ranges)
}

fn strip_ascii_prefix_ignore_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))
        .map(|_| &value[prefix.len()..])
}

fn strip_ascii_suffix_ignore_case<'a>(value: &'a str, suffix: &str) -> Option<&'a str> {
    value
        .get(value.len().saturating_sub(suffix.len())..)
        .filter(|tail| tail.eq_ignore_ascii_case(suffix))
        .map(|_| &value[..value.len() - suffix.len()])
}

fn apply_following_three_points_stage(text: &str) -> LayoutStage {
    let markers = ["以下三点", "以下三个点", "the following three points"];
    let Some((marker_at, marker_len)) = markers
        .iter()
        .find_map(|marker| find_unquoted_marker(text, marker).map(|at| (at, marker.len())))
    else {
        return LayoutStage::unchanged(text);
    };
    let remainder_start = marker_at + marker_len;
    let remainder = &text[remainder_start..];
    let mut items = None;
    let mut consumed_delimiter_ranges = Vec::new();
    let mut consumed_item_trim_ranges = Vec::new();
    for delimiter in ["；", ";", "、"] {
        let (segments, delimiter_ranges) =
            split_unquoted_segments(remainder, remainder_start, text, delimiter);
        let mut pieces = Vec::new();
        let mut trim_ranges = Vec::new();
        for (segment, offset) in &segments {
            let (piece, ranges) = trim_layout_label_with_ranges(segment, *offset);
            if !piece.is_empty() {
                pieces.push(piece);
            }
            trim_ranges.extend(ranges);
        }
        if pieces.len() == 3 {
            items = Some(pieces);
            consumed_delimiter_ranges = delimiter_ranges;
            consumed_item_trim_ranges = trim_ranges;
            break;
        }
    }
    let Some(items) = items else {
        return LayoutStage::unchanged(text);
    };
    let (prefix, prefix_trim_ranges) = trim_layout_label_with_ranges(&text[..marker_at], 0);
    let list = items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n");
    let result = if prefix.is_empty() {
        list
    } else {
        format!("{prefix}\n{list}")
    };
    LayoutStage {
        text: result,
        consumed_syntax_ranges: std::iter::once(marker_at..marker_at + marker_len)
            .chain(prefix_trim_ranges)
            .chain(consumed_delimiter_ranges)
            .chain(consumed_item_trim_ranges)
            .collect(),
    }
}

fn find_marker(haystack: &str, marker: &str) -> Option<usize> {
    if marker.is_ascii() {
        let lower = haystack.to_ascii_lowercase();
        lower.find(&marker.to_ascii_lowercase()).filter(|start| {
            is_ascii_word_left(haystack, *start)
                && is_ascii_word_right(haystack, start + marker.len())
        })
    } else {
        haystack.find(marker)
    }
}

fn find_unquoted_marker(haystack: &str, marker: &str) -> Option<usize> {
    find_unquoted_marker_with_scope(haystack, 0, haystack, marker)
}

fn find_any_unquoted_marker_with_scope(
    haystack: &str,
    source_offset: usize,
    full_source: &str,
    markers: &[&str],
) -> Option<(usize, usize)> {
    markers
        .iter()
        .filter_map(|marker| {
            find_unquoted_marker_with_scope(haystack, source_offset, full_source, marker)
                .map(|at| (at, marker.len()))
        })
        .min_by_key(|(at, _)| *at)
}

fn find_unquoted_marker_with_scope(
    haystack: &str,
    source_offset: usize,
    full_source: &str,
    marker: &str,
) -> Option<usize> {
    let code_ranges = code_literal_ranges(full_source);
    let mut search_from = 0;
    while search_from < haystack.len() {
        let relative = find_marker(&haystack[search_from..], marker)?;
        let start = search_from + relative;
        let source_start = source_offset + start;
        if !is_inside_explicit_quote(full_source, source_start)
            && !code_ranges
                .iter()
                .any(|range| range.start <= source_start && source_start < range.end)
        {
            return Some(start);
        }
        let ch = haystack[start..].chars().next()?;
        search_from = start + ch.len_utf8();
    }
    None
}

fn trim_layout_label_with_ranges(
    text: &str,
    source_offset: usize,
) -> (&str, Vec<std::ops::Range<usize>>) {
    let start = text
        .char_indices()
        .find(|(_, ch)| !is_layout_label_char(*ch))
        .map_or(text.len(), |(index, _)| index);
    let end = text
        .char_indices()
        .rev()
        .find(|(_, ch)| !is_layout_label_char(*ch))
        .map_or(start, |(index, ch)| index + ch.len_utf8());
    let mut ranges = Vec::new();
    if start > 0 {
        ranges.push(source_offset..source_offset + start);
    }
    if end < text.len() {
        ranges.push(source_offset + end..source_offset + text.len());
    }
    (&text[start..end], ranges)
}

fn is_layout_label_char(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, ':' | '：' | ',' | '，' | ';' | '；')
}

fn split_unquoted_segments<'a>(
    text: &'a str,
    source_offset: usize,
    full_source: &str,
    delimiter: &str,
) -> (Vec<(&'a str, usize)>, Vec<std::ops::Range<usize>>) {
    let code_ranges = code_literal_ranges(full_source);
    let mut pieces = Vec::new();
    let mut delimiter_ranges = Vec::new();
    let mut piece_start = 0;
    let mut search_from = 0;
    while let Some(relative) = text[search_from..].find(delimiter) {
        let delimiter_start = search_from + relative;
        if !is_literal_position(full_source, source_offset + delimiter_start, &code_ranges) {
            pieces.push((
                &text[piece_start..delimiter_start],
                source_offset + piece_start,
            ));
            delimiter_ranges.push(
                source_offset + delimiter_start..source_offset + delimiter_start + delimiter.len(),
            );
            piece_start = delimiter_start + delimiter.len();
        }
        search_from = delimiter_start + delimiter.len();
    }
    pieces.push((&text[piece_start..], source_offset + piece_start));
    (pieces, delimiter_ranges)
}

pub(crate) fn code_literal_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut offset = 0;
    let mut fence: Option<(char, usize, usize)> = None;
    for line in text.split_inclusive('\n') {
        let line_without_newline = line.strip_suffix('\n').unwrap_or(line);
        let leading = line_without_newline.len() - line_without_newline.trim_start().len();
        let remainder = &line_without_newline[leading..];
        let marker = remainder.chars().next();
        let run = marker.filter(|ch| matches!(ch, '`' | '~')).map_or(0, |ch| {
            remainder
                .chars()
                .take_while(|candidate| *candidate == ch)
                .count()
        });
        if let Some((fence_char, fence_len, fence_start)) = fence {
            let closes = marker == Some(fence_char)
                && run >= fence_len
                && remainder[remainder
                    .char_indices()
                    .take(run)
                    .last()
                    .map_or(0, |(index, ch)| index + ch.len_utf8())..]
                    .trim()
                    .is_empty()
                && leading <= 3;
            if closes {
                ranges.push(fence_start..offset + line.len());
                fence = None;
            }
        } else if leading <= 3 && run >= 3 {
            fence = Some((marker.unwrap_or('`'), run, offset));
        }
        offset += line.len();
    }
    if let Some((_, _, start)) = fence {
        ranges.push(start..text.len());
    }

    let mut index = 0;
    while index < text.len() {
        if ranges
            .iter()
            .any(|range| range.start <= index && index < range.end)
        {
            index = ranges
                .iter()
                .filter(|range| range.start <= index && index < range.end)
                .map(|range| range.end)
                .max()
                .unwrap_or(index + 1);
            continue;
        }
        if !text[index..].starts_with('`') {
            index += text[index..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        let run = text[index..]
            .bytes()
            .take_while(|byte| *byte == b'`')
            .count();
        if run >= 3 {
            index += run;
            continue;
        }
        let content_start = index + run;
        let mut cursor = content_start;
        let mut close = None;
        while cursor < text.len() {
            let Some(relative) = text[cursor..].find('`') else {
                break;
            };
            let candidate = cursor + relative;
            let candidate_run = text[candidate..]
                .bytes()
                .take_while(|byte| *byte == b'`')
                .count();
            if candidate_run == run {
                close = Some(candidate + candidate_run);
                break;
            }
            cursor = candidate + candidate_run.max(1);
        }
        if let Some(end) = close {
            ranges.push(index..end);
            index = end;
        } else {
            ranges.push(index..text.len());
            break;
        }
    }
    ranges.sort_by_key(|range| range.start);
    ranges
}

fn lists_disabled(family: ContextFamily) -> bool {
    matches!(
        family,
        ContextFamily::Terminal | ContextFamily::BrowserSearch | ContextFamily::FormFilling
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MarkerKind {
    Numbered,
    Bullet,
}

#[derive(Clone, Copy)]
struct Marker {
    start: usize,
    end: usize,
    kind: MarkerKind,
    ordinal: Option<usize>,
    may_strip_leading_copula: bool,
}

struct RenderedListBlock {
    text: String,
    consumed_syntax_ranges: Vec<std::ops::Range<usize>>,
    revisions: Vec<ListItemRevision>,
    retention: Option<ListItemRetention>,
}

#[derive(Clone, PartialEq, Eq)]
enum RetentionDirectiveParse {
    Absent,
    Unresolved(RetentionDirectiveRole),
    Parsed(ListRetentionDirective),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RetentionDirectiveRole {
    Hypothetical,
    Quoted,
    Question,
    Unknown,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RetentionIntroRole {
    Direct,
    Hypothetical,
    Unknown,
}

const HYPOTHETICAL_INTRODUCERS: &[&str] = &[
    "假设", "假如", "如果", "要是", "比如", "例如", "设想", "想象",
];

fn apply_lists_stage(
    text: &str,
    family: ContextFamily,
) -> (
    LayoutStage,
    Vec<ListItemRevision>,
    Option<ListItemRetention>,
) {
    if lists_disabled(family) {
        return (LayoutStage::unchanged(text), Vec::new(), None);
    }
    let code_ranges = code_literal_ranges(text);
    let date_ranges = protected_date_ranges(text);
    let mut out = String::with_capacity(text.len());
    let mut consumed_syntax_ranges = Vec::new();
    let mut revisions = Vec::new();
    let mut retentions = Vec::new();
    let mut source_offset = 0;
    for (index, block) in text.split("\n\n").enumerate() {
        if index > 0 {
            out.push_str("\n\n");
            source_offset += 2;
        }
        let block_end = source_offset + block.len();
        let contains_code = code_ranges
            .iter()
            .any(|range| range.start < block_end && source_offset < range.end);
        if contains_code {
            out.push_str(block);
        } else if let Some(mut rendered) =
            format_list_block_with_revisions(block, source_offset, text, &code_ranges, &date_ranges)
        {
            let output_offset = out.len();
            out.push_str(&rendered.text);
            consumed_syntax_ranges.extend(rendered.consumed_syntax_ranges);
            revisions.extend(rendered.revisions.into_iter().map(|mut revision| {
                revision.output_value_range.start += output_offset;
                revision.output_value_range.end += output_offset;
                revision
            }));
            if let Some(mut retention) = rendered.retention.take() {
                offset_retention_output_ranges(&mut retention, output_offset);
                retentions.push(retention);
            }
        } else {
            out.push_str(block);
        }
        source_offset = block_end;
    }
    (
        LayoutStage {
            text: out,
            consumed_syntax_ranges,
        },
        revisions,
        match retentions.len() {
            0 => None,
            1 => retentions.pop(),
            _ => None,
        },
    )
}

fn offset_retention_output_ranges(retention: &mut ListItemRetention, offset: usize) {
    for item in &mut retention.retained_items {
        item.output_value_range.start += offset;
        item.output_value_range.end += offset;
    }
    if let Some(linker) = &mut retention.ordinal_linker {
        linker.output_range.start += offset;
        linker.output_range.end += offset;
    }
}

fn format_list_block_with_revisions(
    block: &str,
    source_offset: usize,
    full_source: &str,
    code_ranges: &[std::ops::Range<usize>],
    date_ranges: &[std::ops::Range<usize>],
) -> Option<RenderedListBlock> {
    let retention = match parse_list_retention_directive(block) {
        RetentionDirectiveParse::Absent => None,
        RetentionDirectiveParse::Unresolved(_) => return None,
        RetentionDirectiveParse::Parsed(directive) => Some(directive),
    };
    let mut markers = collect_markers(block, source_offset, full_source, code_ranges, date_ranges);
    if let Some(directive) = retention.as_ref() {
        markers.retain(|marker| {
            let explicit_reference = directive.references.iter().any(|reference| {
                marker.start == reference.marker_range.start
                    && marker.end == reference.marker_range.end
            });
            let remove_ordinal = directive.form == ListRetentionForm::RemoveOrdinal
                && marker.start == directive.count_range.start
                && marker.end == directive.count_range.end
                && marker.ordinal == Some(directive.keep_count);
            !explicit_reference && !remove_ordinal
        });
    }
    let leading_restart = retention
        .as_ref()
        .and_then(|directive| parse_empty_ordinal_restart(block, &markers, directive));
    if leading_restart.is_some() {
        markers.remove(0);
    }
    let numbered = markers
        .iter()
        .filter(|item| item.kind == MarkerKind::Numbered)
        .copied()
        .collect::<Vec<_>>();
    let bullets = markers
        .iter()
        .filter(|item| item.kind == MarkerKind::Bullet)
        .copied()
        .collect::<Vec<_>>();
    if retention.is_some() {
        return render_list(
            block,
            source_offset,
            &numbered,
            MarkerKind::Numbered,
            full_source,
            code_ranges,
            retention.as_ref(),
            leading_restart.as_ref(),
        );
    }
    if numbered.len() >= 2 {
        return render_list(
            block,
            source_offset,
            &numbered,
            MarkerKind::Numbered,
            full_source,
            code_ranges,
            None,
            None,
        );
    }
    if bullets.len() >= 2 {
        return render_list(
            block,
            source_offset,
            &bullets,
            MarkerKind::Bullet,
            full_source,
            code_ranges,
            None,
            None,
        );
    }
    None
}

fn parse_empty_ordinal_restart(
    block: &str,
    markers: &[Marker],
    directive: &ListRetentionDirective,
) -> Option<EmptyOrdinalRestart> {
    let expected_content_markers = directive.keep_count.checked_add(1)?;
    let expected_markers = expected_content_markers.checked_add(1)?;
    if directive.keep_count == 0 || markers.len() != expected_markers {
        return None;
    }
    let (abandoned, restarted) = markers.split_first()?;
    if abandoned.kind != MarkerKind::Numbered
        || !abandoned.ordinal.is_some_and(|ordinal| ordinal > 1)
        || restarted.len() != expected_content_markers
        || restarted.iter().enumerate().any(|(index, marker)| {
            marker.kind != MarkerKind::Numbered || marker.ordinal != Some(index + 1)
        })
        || has_explanatory_list_preamble(&block[..abandoned.start])
    {
        return None;
    }

    let first = restarted.first()?;
    let filler_range = abandoned.end..first.start;
    let filler_value = block.get(filler_range.clone())?;
    if !is_empty_ordinal_placeholder_filler(filler_value) {
        return None;
    }
    let source_range = abandoned.start..first.start;
    if range_overlaps_protected_or_literal_except_numbered_marker(
        block,
        &source_range,
        &(abandoned.start..abandoned.end),
    ) {
        return None;
    }

    Some(EmptyOrdinalRestart {
        source_range,
        marker_range: abandoned.start..abandoned.end,
        marker_value: block[abandoned.start..abandoned.end].to_owned(),
        filler_range,
        filler_value: filler_value.to_owned(),
        restarted_marker_range: first.start..first.end,
        restarted_marker_value: block[first.start..first.end].to_owned(),
    })
}

fn is_empty_ordinal_placeholder_filler(text: &str) -> bool {
    let compact = text
        .chars()
        .filter(|ch| !ch.is_whitespace() && !matches!(ch, '，' | ',' | '、'))
        .collect::<String>();
    matches!(compact.as_str(), "是啊" | "是呃" | "是嗯")
}

fn is_empty_or_punctuation_source_range(source: &str, range: &std::ops::Range<usize>) -> bool {
    let Some(value) = source.get(range.clone()) else {
        return false;
    };
    value.chars().all(is_punctuation_or_space)
        || (is_empty_ordinal_placeholder_filler(value)
            && !range_overlaps_protected_or_literal(source, range))
}

fn is_sentence_separator_or_whitespace_source_range(
    source: &str,
    range: &std::ops::Range<usize>,
) -> bool {
    source.get(range.clone()).is_some_and(|value| {
        !value.is_empty()
            && value.chars().all(|ch| {
                ch.is_whitespace()
                    || matches!(
                        ch,
                        ',' | ';'
                            | ':'
                            | '.'
                            | '!'
                            | '?'
                            | '，'
                            | '、'
                            | '；'
                            | '：'
                            | '。'
                            | '！'
                            | '？'
                            | '…'
                            | '⋯'
                    )
            })
    })
}

fn range_overlaps_protected_or_literal(source: &str, range: &std::ops::Range<usize>) -> bool {
    let protected =
        crate::protected_span::protected_spans(source, Some(crate::llm::CleanupOperation::Cleanup));
    let code = code_literal_ranges(source);
    let quotes = explicit_quote_ranges(source);
    protected
        .iter()
        .any(|span| range.start < span.end_byte && span.start_byte < range.end)
        || code
            .iter()
            .chain(quotes.iter())
            .any(|literal| range.start < literal.end && literal.start < range.end)
}

fn range_overlaps_protected_or_literal_except_numbered_marker(
    source: &str,
    range: &std::ops::Range<usize>,
    marker_range: &std::ops::Range<usize>,
) -> bool {
    let Some(marker) = source.get(marker_range.clone()) else {
        return true;
    };
    if parse_numbered_marker_ordinal(source, marker_range).is_none() {
        return true;
    }
    let protected =
        crate::protected_span::protected_spans(source, Some(crate::llm::CleanupOperation::Cleanup));
    let code = code_literal_ranges(source);
    let quotes = explicit_quote_ranges(source);
    protected.iter().any(|span| {
        range.start < span.end_byte
            && span.start_byte < range.end
            && !(matches!(
                span.kind,
                crate::protected_span::ProtectedSpanKind::Number
                    | crate::protected_span::ProtectedSpanKind::Date
            ) && marker_range.start <= span.start_byte
                && span.end_byte <= marker_range.end
                && marker.chars().any(is_cn_digit))
    }) || code
        .iter()
        .chain(quotes.iter())
        .any(|literal| range.start < literal.end && literal.start < range.end)
}

#[expect(
    clippy::too_many_arguments,
    reason = "list rendering keeps source spans, literal exclusions, and retention facts explicit"
)]
fn render_list(
    block: &str,
    source_offset: usize,
    markers: &[Marker],
    kind: MarkerKind,
    full_source: &str,
    code_ranges: &[std::ops::Range<usize>],
    retention_directive: Option<&ListRetentionDirective>,
    leading_restart: Option<&EmptyOrdinalRestart>,
) -> Option<RenderedListBlock> {
    if markers.len() < 2 {
        return None;
    }
    let preamble_end = leading_restart
        .map(|restart| restart.source_range.start)
        .unwrap_or(markers[0].start);
    let preamble = block[..preamble_end].trim();
    let rewrites = if kind == MarkerKind::Numbered {
        find_ordinal_revisions(block, source_offset, markers)
    } else {
        Vec::new()
    };
    if kind == MarkerKind::Numbered
        && has_unresolved_ordinal_revision(
            block,
            source_offset,
            markers,
            &rewrites,
            full_source,
            code_ranges,
        )
    {
        return None;
    }
    let mut replacements = std::collections::HashMap::new();
    let mut skipped = std::collections::HashSet::new();
    let literal_ranges = crate::spoken_revision::literal_ranges(block);
    for (revision_index, rewrite) in rewrites.iter().enumerate() {
        replacements.insert(
            rewrite.target_index,
            (
                revision_index,
                format!(
                    "{}{}",
                    rewrite.revision.replacement_prefix, rewrite.revision.replacement_value
                ),
            ),
        );
        if let Some(placeholder_index) = rewrite.placeholder_index {
            skipped.insert(placeholder_index);
        }
        skipped.insert(rewrite.reference_index);
    }

    let effective_markers = markers
        .iter()
        .enumerate()
        .filter(|(index, _)| !skipped.contains(index))
        .map(|(index, marker)| (index, *marker))
        .collect::<Vec<_>>();
    if let Some(directive) = retention_directive {
        if kind != MarkerKind::Numbered
            || has_explanatory_list_preamble(&block[..markers[0].start])
            || !retention_directive_matches_items(directive, &effective_markers, block)
        {
            return None;
        }
    }

    let mut items = Vec::new();
    let mut cleanup_ranges = Vec::new();
    let mut omitted_terminal_item_ranges = Vec::new();
    let mut removed_items = Vec::new();
    let mut effective_rank = 0usize;
    for (index, marker) in markers.iter().enumerate() {
        if skipped.contains(&index) {
            continue;
        }
        let mut end = markers
            .get(index + 1)
            .map(|next| next.start)
            .unwrap_or(block.len());
        if retention_directive.is_some_and(|directive| {
            directive.form == ListRetentionForm::RemoveOrdinal
                && marker.ordinal != Some(directive.keep_count)
                && index + 1 == markers.len()
        }) {
            end = retention_directive?.range.start;
        }
        let item_source_range = trimmed_range(block, marker.end, end);
        if retention_directive.is_none()
            && kind == MarkerKind::Numbered
            && items.len() >= 2
            && index + 1 == markers.len()
            && marker.ordinal == Some(index + 1)
            && marker.ordinal == Some(items.len() + 1)
            && item_source_range.start < item_source_range.end
            && is_sentence_separator_or_whitespace_source_range(block, &item_source_range)
            && !range_overlaps_protected_or_literal(block, &item_source_range)
        {
            // An unfinished final ordinal is list structure without an item.
            // Consume only its punctuation-only tail so the preceding complete
            // items remain a valid list and the source-preservation guard can
            // account for the deliberately omitted placeholder.
            omitted_terminal_item_ranges.push(
                source_offset + item_source_range.start..source_offset + item_source_range.end,
            );
            effective_rank += 1;
            continue;
        }
        if retention_directive.is_some_and(|directive| match directive.form {
            ListRetentionForm::RemoveOrdinal => marker.ordinal == Some(directive.keep_count),
            _ => effective_rank >= directive.keep_count,
        }) {
            removed_items.push(RemovedListItem {
                ordinal: marker.ordinal?,
                marker_range: marker.start..marker.end,
                source_value_range: item_source_range.clone(),
                source_value: block[item_source_range].to_owned(),
            });
            effective_rank += 1;
            continue;
        }
        let item = if let Some((revision_index, replacement)) = replacements.get(&index) {
            // Keep source proof bytes intact while applying the same narrow
            // terminal separator rendering used for ordinary items.
            let replacement_range = rewrites[*revision_index]
                .revision
                .replacement_source_range
                .clone();
            strip_terminal_list_separator(
                replacement.clone(),
                block,
                replacement_range,
                source_offset,
                &literal_ranges,
                code_ranges,
            )
        } else {
            let raw_item = &block[item_source_range.clone()];
            let may_strip_proved_resume = rewrites.iter().any(|rewrite| {
                rewrite.target_index < index
                    && rewrite.revision.ordinal.checked_add(1) == marker.ordinal
            });
            let (mut normalized, relative_ranges) = clean_list_item_body(
                raw_item,
                item_source_range.start,
                marker.may_strip_leading_copula && kind == MarkerKind::Numbered,
                may_strip_proved_resume,
                markers.get(index + 1).is_some_and(|next| {
                    next.ordinal.is_some()
                        && marker.ordinal.and_then(|ordinal| ordinal.checked_add(1)) == next.ordinal
                }),
                &literal_ranges,
            );
            // The comma separates spoken list items; it is not part of the
            // rendered item. Keep the ASR source value and its proof span
            // untouched, and only omit a terminal comma when that exact
            // character is outside code and quoted literals.
            normalized = strip_terminal_list_separator(
                normalized,
                block,
                source_offset + item_source_range.start..source_offset + item_source_range.end,
                source_offset,
                &literal_ranges,
                code_ranges,
            );
            let absolute_ranges = relative_ranges
                .into_iter()
                .map(|range| {
                    source_offset + item_source_range.start + range.start
                        ..source_offset + item_source_range.start + range.end
                })
                .collect::<Vec<_>>();
            cleanup_ranges.extend(absolute_ranges);
            normalized
        };
        if !item.is_empty() {
            items.push((
                index,
                marker.ordinal.unwrap_or(effective_rank + 1),
                item,
                item_source_range.clone(),
                block[item_source_range].to_owned(),
            ));
        }
        effective_rank += 1;
    }
    if items.len() < 2 {
        return None;
    }
    let mut out = String::new();
    if !preamble.is_empty() {
        out.push_str(preamble);
        out.push_str("\n\n");
    }
    let consumed_reference_indices = rewrites
        .iter()
        .map(|rewrite| rewrite.reference_index)
        .collect::<std::collections::HashSet<_>>();
    let mut revisions = rewrites
        .into_iter()
        .map(|rewrite| rewrite.revision)
        .collect::<Vec<_>>();
    let mut retained_items = Vec::new();
    for (index, (source_index, source_ordinal, item, source_range, source_value)) in
        items.iter().enumerate()
    {
        if index > 0 {
            out.push('\n');
        }
        let rendered_ordinal = index + 1;
        match kind {
            MarkerKind::Numbered => {
                out.push_str(&rendered_ordinal.to_string());
                out.push_str(". ");
            }
            MarkerKind::Bullet => out.push_str("- "),
        }
        let value_start = out.len();
        out.push_str(item);
        if retention_directive.is_some() {
            retained_items.push(RetainedListItem {
                ordinal: *source_ordinal,
                rendered_ordinal,
                marker_range: markers[*source_index].start..markers[*source_index].end,
                source_value_range: source_range.clone(),
                source_value: source_value.clone(),
                output_value_range: value_start..value_start + item.len(),
                output_value: item.clone(),
            });
        }
        if let Some((revision_index, _)) = replacements.get(source_index) {
            if let Some(revision) = revisions.get_mut(*revision_index) {
                revision.rendered_ordinal = rendered_ordinal;
                revision.output_value_range = value_start..value_start + item.len();
            }
        }
    }
    let mut consumed = markers
        .iter()
        .enumerate()
        .filter(|(index, _)| !consumed_reference_indices.contains(index))
        .map(|(_, marker)| source_offset + marker.start..source_offset + marker.end)
        .collect::<Vec<_>>();
    consumed.extend(
        revisions
            .iter()
            .map(|revision| revision.instruction_range.clone()),
    );
    consumed.extend(cleanup_ranges);
    consumed.extend(omitted_terminal_item_ranges);
    let mut retention = retention_directive.map(|directive| {
        let absolute =
            |range: std::ops::Range<usize>| source_offset + range.start..source_offset + range.end;
        ListItemRetention {
            form: directive.form,
            directive_range: absolute(directive.range.clone()),
            directive_value: block[directive.range.clone()].to_owned(),
            directive_suffix_range: (directive.form == ListRetentionForm::RemoveOrdinal
                && markers
                    .last()
                    .is_some_and(|marker| marker.ordinal != Some(directive.keep_count)))
            .then(|| absolute(directive.range.end..block.len())),
            cue_range: absolute(directive.cue_range.clone()),
            cue_value: block[directive.cue_range.clone()].to_owned(),
            action_range: absolute(directive.action_range.clone()),
            action_value: block[directive.action_range.clone()].to_owned(),
            count_range: absolute(directive.count_range.clone()),
            count_value: block[directive.count_range.clone()].to_owned(),
            retained_count: if directive.form == ListRetentionForm::RemoveOrdinal {
                items.len()
            } else {
                directive.keep_count
            },
            directive_ordinal: (directive.form == ListRetentionForm::RemoveOrdinal)
                .then_some(directive.keep_count),
            references: directive
                .references
                .iter()
                .cloned()
                .map(|reference| ListRetentionReference {
                    ordinal: reference.ordinal,
                    marker_range: absolute(reference.marker_range),
                    marker_value: reference.marker_value,
                })
                .collect(),
            retained_items: retained_items
                .into_iter()
                .map(|mut item| {
                    item.marker_range = absolute(item.marker_range);
                    item.source_value_range = absolute(item.source_value_range);
                    item
                })
                .collect(),
            removed_items: removed_items
                .into_iter()
                .map(|mut item| {
                    item.marker_range = absolute(item.marker_range);
                    item.source_value_range = absolute(item.source_value_range);
                    item
                })
                .collect(),
            ordinal_linker: None,
            leading_restart: leading_restart.map(|restart| {
                let absolute = |range: std::ops::Range<usize>| {
                    source_offset + range.start..source_offset + range.end
                };
                EmptyOrdinalRestart {
                    source_range: absolute(restart.source_range.clone()),
                    marker_range: absolute(restart.marker_range.clone()),
                    marker_value: restart.marker_value.clone(),
                    filler_range: absolute(restart.filler_range.clone()),
                    filler_value: restart.filler_value.clone(),
                    restarted_marker_range: absolute(restart.restarted_marker_range.clone()),
                    restarted_marker_value: restart.restarted_marker_value.clone(),
                }
            }),
        }
    });
    if let Some(retention) = &mut retention {
        retention.ordinal_linker =
            find_retained_ordinal_linker(full_source, retention, &revisions, false);
    }
    Some(RenderedListBlock {
        text: out,
        consumed_syntax_ranges: consumed,
        revisions,
        retention,
    })
}

/// Prove the narrow ordinal linker case where an adjacent correction moves a
/// value from the preceding list item into this item. The source item must
/// start with `才是`, and its remaining payload must exactly match the value
/// that the adjacent correction removed. This keeps emphatic `才是` phrases
/// and unrelated occurrences outside the authorization.
fn find_retained_ordinal_linker(
    source: &str,
    retention: &ListItemRetention,
    revisions: &[ListItemRevision],
    canonicalized: bool,
) -> Option<RetainedOrdinalLinker> {
    if retention.form != ListRetentionForm::CountOnly {
        return None;
    }
    let mut proved = retention
        .retained_items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let ordinal = item.ordinal;
            if ordinal < 2
                || !item.source_value.starts_with("才是")
                || canonicalized == item.output_value.starts_with("才是")
            {
                return None;
            }
            let old_value = item.source_value.strip_prefix("才是")?;
            let old_value = normalized_previous_ordinal_value(old_value);
            if old_value.is_empty() || old_value.starts_with("才是") {
                return None;
            }
            let previous = retention.retained_items.get(index.checked_sub(1)?)?;
            if previous.ordinal.checked_add(1) != Some(ordinal) {
                return None;
            }
            let revision_matches = revisions.iter().any(|revision| {
                revision.rendered_ordinal.checked_add(1) == Some(ordinal)
                    && revision.target_marker_range == previous.marker_range
                    && revision.output_value_range == previous.output_value_range
                    && normalized_previous_ordinal_value(&revision.source_value) == old_value
            });
            if !revision_matches {
                return None;
            }
            let source_start = item.source_value_range.start;
            let output_start = item.output_value_range.start;
            let source_end = source_start.checked_add("才是".len())?;
            let output_end = if canonicalized {
                output_start
            } else {
                output_start.checked_add("才是".len())?
            };
            let source_range = source_start..source_end;
            let output_range = output_start..output_end;
            if source.get(source_range.clone()) != Some("才是")
                || source.get(item.source_value_range.clone()) != Some(item.source_value.as_str())
                || range_overlaps_protected_or_literal(source, &source_range)
            {
                return None;
            }
            Some(RetainedOrdinalLinker {
                ordinal,
                source_range,
                output_range,
                canonicalized,
            })
        });
    let linker = proved.next()?;
    proved.next().is_none().then_some(linker)
}

fn normalized_previous_ordinal_value(value: &str) -> &str {
    let value = value.trim_matches(is_punctuation_or_space);
    let value = value.strip_prefix('是').unwrap_or(value);
    value.trim_matches(is_punctuation_or_space)
}

/// Normalize only source-backed disfluency syntax inside an already parsed
/// list item. These exact byte spans are handed to the layout proof; facts,
/// uncertain language, and emphasis remain ordinary item content.
fn clean_list_item_body(
    source: &str,
    block_item_start: usize,
    may_strip_leading_copula: bool,
    may_strip_proved_resume: bool,
    has_next_consecutive_ordinal: bool,
    literal_ranges: &[std::ops::Range<usize>],
) -> (String, Vec<std::ops::Range<usize>>) {
    let mut removed = Vec::new();
    let may_remove = |range: &std::ops::Range<usize>| {
        !literal_ranges.iter().any(|literal| {
            literal.start < block_item_start + range.end
                && block_item_start + range.start < literal.end
        })
    };
    if may_strip_proved_resume {
        if let Some(rest) = source.strip_prefix("才是") {
            // A proved back-reference can leave a restarted ordinal linker
            // immediately before a separate question payload.
            if rest.starts_with("是不是") {
                let range = 0.."才是".len();
                if may_remove(&range) {
                    removed.push(range);
                }
            }
        }
    }
    if may_strip_leading_copula {
        if let Some(rest) = source.strip_prefix('是') {
            // Consume a bare list-item copula only when it introduces a
            // predicate. Keep a bare affirmative 是啊, question forms, and
            // meaningful emphasis.
            let bare_affirmative = rest
                .strip_prefix('啊')
                .is_some_and(|tail| tail.chars().all(is_punctuation_or_space));
            let payload_is_question_or_meaningful = bare_affirmative
                || [
                    "不",
                    "否",
                    "是",
                    "什",
                    "怎",
                    "如",
                    "哪",
                    "谁",
                    "几",
                    "的",
                    "唯一",
                    "真正",
                    "正确答案",
                ]
                .iter()
                .any(|prefix| rest.starts_with(prefix));
            if !rest.is_empty() && !payload_is_question_or_meaningful {
                let range = 0..'是'.len_utf8();
                if may_remove(&range) {
                    removed.push(range);
                }
            }
        }
    }

    let first_content = removed.last().map(|range| range.end).unwrap_or(0);
    let rest = &source[first_content..];
    let leading = rest.len() - rest.trim_start_matches(is_punctuation_or_space).len();
    let content_start = first_content + leading;
    let follows_proved_syntax = removed
        .iter()
        .any(|range| range.start == 0 && range.end <= content_start);
    if let Some(token_range) =
        standalone_filler_token_range(source, content_start, follows_proved_syntax)
    {
        let end = token_range.end;
        let has_payload = source[end..].chars().any(|ch| !is_punctuation_or_space(ch));
        let trailing = &source[end..];
        let payload_start =
            trailing.len() - trailing.trim_start_matches(is_punctuation_or_space).len();
        if has_payload {
            let range = content_start..end + payload_start;
            if may_remove(&range) {
                removed.push(range);
            }
        }
    }

    // A single pronoun stutter at the item head is safe to collapse only
    // before a clear clause continuation. Intentional emphasis is untouched.
    let without_proved_linker = if removed.first().is_some_and(|range| range.start == 0) {
        &source[removed[0].end..]
    } else {
        source
    };
    let duplicate = [
        ("你你", "呢"),
        ("我我", "觉得"),
        ("我我", "认为"),
        ("我我", "想"),
    ]
    .iter()
    .find(|(prefix, continuation)| {
        without_proved_linker.starts_with(prefix)
            && without_proved_linker[prefix.len()..].starts_with(continuation)
    });
    if let Some((prefix, _)) = duplicate {
        let source_start = source.len() - without_proved_linker.len();
        let first_character_bytes = prefix.chars().next().map(char::len_utf8).unwrap_or(0);
        if first_character_bytes > 0 {
            let range = source_start..source_start + first_character_bytes;
            if may_remove(&range) {
                removed.push(range);
            }
        }
    }

    if has_next_consecutive_ordinal {
        let end = source.trim_end().len();
        if source[..end].ends_with("然后") {
            let range = end - "然后".len()..end;
            if may_remove(&range) {
                removed.push(range);
            }
        }
    }

    removed.sort_by_key(|range| range.start);
    removed.dedup();
    let mut normalized = source.to_owned();
    for range in removed.iter().rev() {
        normalized.replace_range(range.clone(), "");
    }
    (normalized, removed)
}

struct OrdinalRewrite {
    target_index: usize,
    placeholder_index: Option<usize>,
    reference_index: usize,
    revision: ListItemRevision,
}

fn find_ordinal_revisions(
    block: &str,
    source_offset: usize,
    markers: &[Marker],
) -> Vec<OrdinalRewrite> {
    if markers
        .first()
        .is_some_and(|first| has_explanatory_list_preamble(&block[..first.start]))
    {
        return Vec::new();
    }
    let literals = crate::spoken_revision::literal_ranges(block);
    let mut revisions = Vec::new();
    let mut used_targets = std::collections::HashSet::new();
    let mut used_placeholders = std::collections::HashSet::new();
    for reference_index in 1..markers.len() {
        let reference = markers[reference_index];
        let Some(ordinal) = reference.ordinal else {
            continue;
        };
        if let Some(rewrite) = find_inline_item_correction(
            block,
            source_offset,
            markers,
            reference_index,
            &literals,
            &used_targets,
        ) {
            used_targets.insert(rewrite.target_index);
            revisions.push(rewrite);
            continue;
        }
        let placeholder_index = reference_index - 1;
        let placeholder = markers[placeholder_index];
        let gap_start = placeholder.end;
        let gap_end = reference.start;
        let parsed_cue = parse_revision_cue(&block[gap_start..gap_end]);
        if parsed_cue.is_none() {
            if let Some(rewrite) = find_adjacent_completed_item_revision(
                block,
                source_offset,
                markers,
                reference_index,
                &literals,
                &used_targets,
                &used_placeholders,
            ) {
                used_targets.insert(rewrite.target_index);
                revisions.push(rewrite);
            }
            continue;
        }
        let Some((cue_range, cue_value)) = parsed_cue else {
            continue;
        };
        let cue_range = gap_start + cue_range.start..gap_start + cue_range.end;
        if literals.iter().any(|range| {
            range.start < gap_end && gap_start < range.end
                || range.start < reference.end && reference.start < range.end
        }) {
            continue;
        }
        let targets = markers[..placeholder_index]
            .iter()
            .enumerate()
            .filter(|(_, marker)| {
                marker.kind == MarkerKind::Numbered && marker.ordinal == Some(ordinal)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let [target_index] = targets.as_slice() else {
            continue;
        };
        if used_targets.contains(target_index) || used_placeholders.contains(&placeholder_index) {
            continue;
        }
        let target = markers[*target_index];
        let source_item_end = markers
            .get(target_index + 1)
            .map(|marker| marker.start)
            .unwrap_or(placeholder.start);
        let source_item_range = trimmed_range(block, target.end, source_item_end);
        if source_item_range.is_empty()
            || literals.iter().any(|range| {
                range.start < source_item_range.end && source_item_range.start < range.end
            })
        {
            continue;
        }
        let replacement_end = markers
            .get(reference_index + 1)
            .map(|next| next.start)
            .unwrap_or(block.len());
        let Some((copula_range, replacement_start, replacement_prefix)) = strip_revision_copula(
            block,
            reference.end,
            replacement_end,
            &block[source_item_range.clone()],
        ) else {
            continue;
        };
        let replacement_source_range = trimmed_range(block, replacement_start, replacement_end);
        if replacement_source_range.is_empty()
            || literals.iter().any(|range| {
                range.start < replacement_source_range.end
                    && replacement_source_range.start < range.end
            })
        {
            continue;
        }
        let absolute =
            |range: std::ops::Range<usize>| source_offset + range.start..source_offset + range.end;
        let instruction_range = absolute(gap_start..replacement_source_range.start);
        if !instruction_is_only_authorized_syntax(
            block,
            gap_start,
            replacement_source_range.start,
            &cue_range,
            &cue_value,
            &(reference.start..reference.end),
            &block[copula_range.clone()],
        ) {
            continue;
        }
        let rendered_source_value = block[source_item_range.clone()].to_owned();
        let replacement_value = block[replacement_source_range.clone()].to_owned();
        let copula_value = block[copula_range.clone()].to_owned();
        revisions.push(OrdinalRewrite {
            target_index: *target_index,
            placeholder_index: Some(placeholder_index),
            reference_index,
            revision: ListItemRevision {
                ordinal,
                rendered_ordinal: 0,
                target_marker_range: absolute(target.start..target.end),
                placeholder_marker_range: Some(absolute(placeholder.start..placeholder.end)),
                reference_marker_range: absolute(reference.start..reference.end),
                instruction_range,
                cue_range: absolute(cue_range),
                cue_value,
                copula_range: absolute(copula_range),
                copula_value,
                source_item_range: absolute(source_item_range),
                source_value: rendered_source_value,
                replacement_source_range: absolute(replacement_source_range),
                replacement_prefix,
                replacement_value,
                output_value_range: 0..0,
            },
        });
        used_targets.insert(*target_index);
        used_placeholders.insert(placeholder_index);
    }
    revisions
}

/// A same-utterance correction can restate an ordinal immediately after a
/// complete revision cue, without first speaking an empty numbered item. The
/// old value, correction cue, reference, copula, and replacement remain bound
/// to exact source spans in the same ListItemRevision proof as other edits.
fn find_inline_item_correction(
    block: &str,
    source_offset: usize,
    markers: &[Marker],
    reference_index: usize,
    literals: &[std::ops::Range<usize>],
    used_targets: &std::collections::HashSet<usize>,
) -> Option<OrdinalRewrite> {
    let target_index = reference_index.checked_sub(1)?;
    let target = *markers.get(target_index)?;
    let reference = *markers.get(reference_index)?;
    let ordinal = reference.ordinal?;
    if target.kind != MarkerKind::Numbered
        || target.ordinal != Some(ordinal)
        || markers[..reference_index]
            .iter()
            .filter(|marker| marker.kind == MarkerKind::Numbered && marker.ordinal == Some(ordinal))
            .count()
            != 1
        || used_targets.contains(&target_index)
    {
        return None;
    }

    let gap = &block[target.end..reference.start];
    let (cue_relative, _) = trailing_revision_cue(gap)?;
    let cue_start = target.end + cue_relative.start;
    let source_item_range = trimmed_range(block, target.end, cue_start);
    if source_item_range.is_empty()
        || literals.iter().any(|literal| {
            literal.start < source_item_range.end && source_item_range.start < literal.end
                || literal.start < target.end + cue_relative.end
                    && target.end + cue_relative.start < literal.end
                || literal.start < reference.end && reference.start < literal.end
        })
    {
        return None;
    }
    let replacement_end = markers
        .get(reference_index + 1)
        .map(|next| next.start)
        .unwrap_or(block.len());
    let (copula_range, replacement_start, replacement_prefix) = strip_revision_copula(
        block,
        reference.end,
        replacement_end,
        &block[source_item_range.clone()],
    )?;
    let replacement_source_range = trimmed_range(block, replacement_start, replacement_end);
    if replacement_source_range.is_empty()
        || literals.iter().any(|literal| {
            literal.start < replacement_source_range.end
                && replacement_source_range.start < literal.end
        })
    {
        return None;
    }
    let instruction_range = source_item_range.end..replacement_source_range.start;
    let cue_range = target.end + cue_relative.start..target.end + cue_relative.end;
    let (cue_range, cue_value) = block
        .get(cue_range.clone())
        .map(|value| (cue_range, value.to_owned()))?;
    if !instruction_is_only_authorized_syntax(
        block,
        instruction_range.start,
        instruction_range.end,
        &cue_range,
        &cue_value,
        &(reference.start..reference.end),
        &block[copula_range.clone()],
    ) {
        return None;
    }
    let absolute =
        |range: std::ops::Range<usize>| source_offset + range.start..source_offset + range.end;
    Some(OrdinalRewrite {
        target_index,
        placeholder_index: None,
        reference_index,
        revision: ListItemRevision {
            ordinal,
            rendered_ordinal: 0,
            target_marker_range: absolute(target.start..target.end),
            placeholder_marker_range: None,
            reference_marker_range: absolute(reference.start..reference.end),
            instruction_range: absolute(instruction_range),
            cue_range: absolute(cue_range),
            cue_value,
            copula_range: absolute(copula_range.clone()),
            copula_value: block[copula_range].to_owned(),
            source_item_range: absolute(source_item_range.clone()),
            source_value: block[source_item_range].to_owned(),
            replacement_source_range: absolute(replacement_source_range.clone()),
            replacement_prefix,
            replacement_value: block[replacement_source_range].to_owned(),
            output_value_range: 0..0,
        },
    })
}

fn find_adjacent_completed_item_revision(
    block: &str,
    source_offset: usize,
    markers: &[Marker],
    reference_index: usize,
    literals: &[std::ops::Range<usize>],
    used_targets: &std::collections::HashSet<usize>,
    used_placeholders: &std::collections::HashSet<usize>,
) -> Option<OrdinalRewrite> {
    if reference_index == 0 {
        return None;
    }
    let target_index = reference_index - 1;
    let target = markers[target_index];
    let reference = markers[reference_index];
    let ordinal = reference.ordinal?;
    let targets = markers[..reference_index]
        .iter()
        .enumerate()
        .filter(|(_, marker)| {
            marker.kind == MarkerKind::Numbered && marker.ordinal == Some(ordinal)
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if targets.len() != 1
        || targets[0] != target_index
        || target.kind != MarkerKind::Numbered
        || target.ordinal != Some(ordinal)
        || used_targets.contains(&target_index)
        || used_placeholders.contains(&reference_index)
    {
        return None;
    }
    let cue_gap = &block[target.end..reference.start];
    let (cue_relative_range, cue_value) = trailing_revision_cue(cue_gap)?;
    if !matches!(
        cue_value.as_str(),
        "我说错了" | "哦不对" | "噢不对" | "不对"
    ) {
        return None;
    }
    let cue_start_in_block = target.end + cue_relative_range.start;
    let old_item_range = trimmed_range(block, target.end, cue_start_in_block);
    if old_item_range.is_empty()
        || !block[old_item_range.clone()]
            .chars()
            .next_back()
            .is_some_and(|ch| matches!(ch, '。' | '！' | '？' | '.' | '!' | '?' | '；' | ';'))
        || literals.iter().any(|range| {
            range.start < reference.end && reference.start < range.end
                || range.start < old_item_range.end && old_item_range.start < range.end
                || range.start < cue_start_in_block + cue_relative_range.end
                    && cue_start_in_block < range.end
        })
    {
        return None;
    }
    let replacement_end = markers
        .get(reference_index + 1)
        .map(|next| next.start)
        .unwrap_or(block.len());
    let (copula_range, replacement_start, replacement_prefix) = strip_revision_copula(
        block,
        reference.end,
        replacement_end,
        &block[old_item_range.clone()],
    )?;
    let replacement_source_range = trimmed_range(block, replacement_start, replacement_end);
    if replacement_source_range.is_empty()
        || literals.iter().any(|range| {
            range.start < replacement_source_range.end && replacement_source_range.start < range.end
        })
    {
        return None;
    }
    let instruction_range = old_item_range.end..replacement_source_range.start;
    let cue_range = cue_start_in_block..cue_start_in_block + cue_relative_range.len();
    let absolute =
        |range: std::ops::Range<usize>| source_offset + range.start..source_offset + range.end;
    if !instruction_is_only_authorized_syntax(
        block,
        instruction_range.start,
        instruction_range.end,
        &cue_range,
        &cue_value,
        &(reference.start..reference.end),
        &block[copula_range.clone()],
    ) {
        return None;
    }
    let source_value = block[old_item_range.clone()].to_owned();
    let replacement_value = block[replacement_source_range.clone()].to_owned();
    let copula_value = block[copula_range.clone()].to_owned();
    Some(OrdinalRewrite {
        target_index,
        placeholder_index: None,
        reference_index,
        revision: ListItemRevision {
            ordinal,
            rendered_ordinal: 0,
            target_marker_range: absolute(target.start..target.end),
            placeholder_marker_range: None,
            reference_marker_range: absolute(reference.start..reference.end),
            instruction_range: absolute(instruction_range),
            cue_range: absolute(cue_range),
            cue_value,
            copula_range: absolute(copula_range),
            copula_value,
            source_item_range: absolute(old_item_range),
            source_value,
            replacement_source_range: absolute(replacement_source_range),
            replacement_prefix,
            replacement_value,
            output_value_range: 0..0,
        },
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OrdinalCueRole {
    None,
    ResolvedBackReference,
    LegacyForwardBacktrack,
    UnresolvedBackReference,
}

fn ordinal_cue_role(
    block: &str,
    source_offset: usize,
    markers: &[Marker],
    rewrites: &[OrdinalRewrite],
    reference_index: usize,
    full_source: &str,
    code_ranges: &[std::ops::Range<usize>],
) -> OrdinalCueRole {
    let Some(reference) = markers.get(reference_index) else {
        return OrdinalCueRole::None;
    };
    if reference.ordinal.is_none() || reference_index == 0 {
        return OrdinalCueRole::None;
    }
    let placeholder = markers[reference_index - 1];
    let gap = &block[placeholder.end..reference.start];
    if rewrites
        .iter()
        .any(|rewrite| rewrite.reference_index == reference_index)
    {
        return OrdinalCueRole::ResolvedBackReference;
    }
    let resolved_revision_markers = rewrites
        .iter()
        .flat_map(|rewrite| {
            std::iter::once(rewrite.reference_index).chain(rewrite.placeholder_index)
        })
        .collect::<std::collections::HashSet<_>>();
    let has_earlier_same_ordinal =
        markers[..reference_index]
            .iter()
            .enumerate()
            .any(|(index, marker)| {
                !resolved_revision_markers.contains(&index)
                    && marker.kind == MarkerKind::Numbered
                    && marker.ordinal == reference.ordinal
            });
    if trailing_revision_cue(gap).is_none() {
        return if has_earlier_same_ordinal {
            OrdinalCueRole::UnresolvedBackReference
        } else {
            OrdinalCueRole::None
        };
    }
    let explanatory_preamble = markers
        .first()
        .is_some_and(|first| has_explanatory_list_preamble(&block[..first.start]));

    // The older backtrack owner handles a clearly forward false start such as
    // item 1, “哦,不对”, item 2. Keep that narrow path when the marker sequence
    // itself proves a consecutive next item and the existing parser confirms a
    // trailing backtrack cue. A back reference, duplicate, gap, or neutral cue
    // remains unresolved and must be kept intact.
    if has_earlier_same_ordinal {
        return OrdinalCueRole::UnresolvedBackReference;
    }
    if !has_earlier_same_ordinal
        && !explanatory_preamble
        && is_legacy_forward_backtrack(
            gap,
            source_offset + placeholder.end,
            placeholder,
            *reference,
            full_source,
            code_ranges,
        )
    {
        return OrdinalCueRole::LegacyForwardBacktrack;
    }
    OrdinalCueRole::UnresolvedBackReference
}

fn is_legacy_forward_backtrack(
    gap: &str,
    gap_source_offset: usize,
    placeholder: Marker,
    reference: Marker,
    full_source: &str,
    code_ranges: &[std::ops::Range<usize>],
) -> bool {
    let (Some(current_ordinal), Some(next_ordinal)) = (placeholder.ordinal, reference.ordinal)
    else {
        return false;
    };
    if current_ordinal.checked_add(1) != Some(next_ordinal) {
        return false;
    }
    let (_, consumed) = strip_trailing_backtrack(gap, gap_source_offset, full_source, code_ranges);
    !consumed.is_empty()
}

fn has_unresolved_ordinal_revision(
    block: &str,
    source_offset: usize,
    markers: &[Marker],
    rewrites: &[OrdinalRewrite],
    full_source: &str,
    code_ranges: &[std::ops::Range<usize>],
) -> bool {
    (1..markers.len()).any(|reference_index| {
        ordinal_cue_role(
            block,
            source_offset,
            markers,
            rewrites,
            reference_index,
            full_source,
            code_ranges,
        ) == OrdinalCueRole::UnresolvedBackReference
    })
}

fn trailing_revision_cue(gap: &str) -> Option<(std::ops::Range<usize>, String)> {
    const EXTRA_CUES: &[&str] = &[
        "我刚才说错了",
        "我说错了",
        "我刚才说的",
        "我刚说的",
        "啊不对",
        "噢不对",
    ];
    let trimmed_end = gap
        .trim_end_matches(|ch: char| ch.is_whitespace() || is_punctuation(ch))
        .len();
    for cue in crate::spoken_revision::MARKERS
        .iter()
        .chain(EXTRA_CUES.iter())
    {
        let Some(start) = gap[..trimmed_end].rfind(cue) else {
            continue;
        };
        if cue.is_ascii()
            && !crate::spoken_revision::ascii_boundaries(gap, start, start + cue.len())
        {
            continue;
        }
        if *cue == "不对"
            && start > 0
            && !gap[..start]
                .chars()
                .next_back()
                .is_some_and(is_punctuation_or_space)
        {
            continue;
        }
        let after = gap[start + cue.len()..]
            .trim_matches(|ch: char| ch.is_whitespace() || is_punctuation(ch));
        if after.is_empty() {
            return Some((start..start + cue.len(), (*cue).to_owned()));
        }
    }
    None
}

fn parse_revision_cue(gap: &str) -> Option<(std::ops::Range<usize>, String)> {
    let (range, value) = trailing_revision_cue(gap)?;
    if !matches!(
        value.as_str(),
        "我刚才说错了" | "我说错了" | "啊不对" | "哦不对" | "噢不对" | "不对"
    ) {
        return None;
    }
    let before =
        gap[..range.start].trim_matches(|ch: char| ch.is_whitespace() || is_punctuation(ch));
    let allowed_before = before.is_empty()
        || ["是", "为", "is", "was"]
            .iter()
            .any(|allowed| before.eq_ignore_ascii_case(allowed));
    allowed_before.then_some((range, value))
}

fn has_explanatory_list_preamble(preamble: &str) -> bool {
    let clause = preamble
        .rsplit(['。', '！', '？', '!', '?', '\n', ';', '；'])
        .next()
        .unwrap_or(preamble)
        .trim();
    const INTRODUCTIONS: &[(&str, bool)] = &[
        ("假设我说", false),
        ("假如我说", false),
        ("如果我说", false),
        ("比如我说", false),
        ("例如我说", false),
        ("设想我说", false),
        ("我希望系统在", true),
        ("我想要系统在", true),
        ("我想让系统在", true),
        ("suppose i say", false),
        ("if i say", false),
        ("imagine i say", false),
        ("i want the system to", true),
    ];
    INTRODUCTIONS
        .iter()
        .any(|(introduction, explicit_request)| {
            let Some(prefix) = clause.get(..introduction.len()) else {
                return false;
            };
            if !prefix.eq_ignore_ascii_case(introduction) {
                return false;
            }
            if *explicit_request {
                return true;
            }
            let rest = clause[introduction.len()..].trim_start();
            rest.is_empty()
                || rest
                    .chars()
                    .next()
                    .is_some_and(|ch| matches!(ch, ':' | '：' | ',' | '，' | '第'))
                || starts_with_ignore_ascii_case(rest, "first")
        })
}

fn strip_revision_copula(
    block: &str,
    start: usize,
    end: usize,
    target_item: &str,
) -> Option<(std::ops::Range<usize>, usize, String)> {
    const COPULAS: &[&str] = &["应该是", "应该为", "是", "为", "should be", "is"];
    let mut cursor = start;
    while cursor < end {
        let ch = block[cursor..].chars().next()?;
        if !ch.is_whitespace() && !is_punctuation(ch) {
            break;
        }
        cursor += ch.len_utf8();
    }
    let remainder = &block[cursor..end];
    let copula = COPULAS.iter().find(|copula| {
        block[cursor..end]
            .get(..copula.len())
            .is_some_and(|value| value.eq_ignore_ascii_case(copula))
    });
    if let Some(copula) = copula {
        let copula_range = cursor..cursor + copula.len();
        return Some((copula_range.clone(), copula_range.end, String::new()));
    }
    if let Some((head, named_copula)) = leading_named_head_copula(remainder) {
        let prefix = format!("{head}{named_copula}");
        if target_item.starts_with(&prefix) {
            let range = cursor..cursor + prefix.len();
            return Some((range.clone(), range.end, prefix));
        }
    }
    None
}

fn leading_named_head_copula(text: &str) -> Option<(&str, &str)> {
    for (index, _) in text.char_indices().skip(1) {
        let head = &text[..index];
        if head.chars().any(|ch| {
            ch.is_whitespace()
                || is_punctuation(ch)
                || !(is_cjk_letter(ch) || ch.is_ascii_alphanumeric())
        }) {
            return None;
        }
        if head.chars().count() < 2 {
            continue;
        }
        for copula in ["是", "为"] {
            if text[index..].starts_with(copula) {
                return Some((head, copula));
            }
        }
    }
    None
}

fn instruction_is_only_authorized_syntax(
    block: &str,
    start: usize,
    end: usize,
    cue_range: &std::ops::Range<usize>,
    cue_value: &str,
    reference_range: &std::ops::Range<usize>,
    copula_value: &str,
) -> bool {
    let mut parts = vec![
        (cue_range.clone(), cue_value),
        (reference_range.clone(), &block[reference_range.clone()]),
    ];
    if !copula_value.is_empty() {
        if let Some(relative) = block[reference_range.end..end].find(copula_value) {
            let copula_start = reference_range.end + relative;
            parts.push((
                copula_start..copula_start + copula_value.len(),
                copula_value,
            ));
        }
    }
    parts.sort_by_key(|(range, _)| range.start);
    let mut cursor = start;
    for (range, expected) in parts {
        if range.start < cursor || range.end > end || block.get(range.clone()) != Some(expected) {
            return false;
        }
        let separator = block[cursor..range.start].trim_matches(is_punctuation_or_space);
        let allowed_filler = cursor == start
            && (separator.is_empty()
                || ["是", "为", "is", "was"]
                    .iter()
                    .any(|filler| separator.eq_ignore_ascii_case(filler)));
        if !allowed_filler
            && !block[cursor..range.start]
                .chars()
                .all(is_punctuation_or_space)
        {
            return false;
        }
        cursor = range.end;
    }
    block[cursor..end].chars().all(is_punctuation_or_space)
}

fn trimmed_range(text: &str, start: usize, end: usize) -> std::ops::Range<usize> {
    let slice = &text[start..end];
    let leading = slice.len() - slice.trim_start().len();
    let trailing_end = slice.trim_end().len();
    start + leading..start + trailing_end
}

fn is_punctuation_or_space(ch: char) -> bool {
    ch.is_whitespace() || is_punctuation(ch)
}

fn is_punctuation(ch: char) -> bool {
    ch.is_ascii_punctuation()
        || matches!(ch as u32, 0x2000..=0x206F | 0x3000..=0x303F | 0xFE10..=0xFE1F | 0xFF01..=0xFF65)
}

fn apply_backtracks_stage(text: &str) -> LayoutStage {
    let code_ranges = code_literal_ranges(text);
    let quote_ranges = explicit_quote_ranges(text);
    let date_ranges = protected_date_ranges(text);
    let mut out = String::with_capacity(text.len());
    let mut consumed_syntax_ranges = Vec::new();
    let mut source_offset = 0;
    for (index, block) in text.split("\n\n").enumerate() {
        if index > 0 {
            out.push_str("\n\n");
            source_offset += 2;
        }
        let block_end = source_offset + block.len();
        let contains_literal = code_ranges
            .iter()
            .chain(quote_ranges.iter())
            .any(|range| range.start < block_end && source_offset < range.end);
        let markers = if contains_literal {
            Vec::new()
        } else {
            collect_markers(block, source_offset, text, &code_ranges, &date_ranges)
        };
        let retention_pending =
            parse_list_retention_directive(block) != RetentionDirectiveParse::Absent;
        let unresolved_ordinal_revision = !contains_literal
            && has_unresolved_ordinal_revision(
                block,
                source_offset,
                &markers,
                &find_ordinal_revisions(block, source_offset, &markers),
                text,
                &code_ranges,
            );
        if contains_literal || unresolved_ordinal_revision || retention_pending {
            out.push_str(block);
        } else {
            let stage = rewrite_list_backtrack_block(block, source_offset, text, &code_ranges);
            out.push_str(&stage.text);
            consumed_syntax_ranges.extend(stage.consumed_syntax_ranges);
        }
        source_offset = block_end;
    }
    LayoutStage {
        text: out,
        consumed_syntax_ranges,
    }
}

struct ListItem {
    body: String,
    discarded: bool,
}

fn rewrite_list_backtrack_block(
    block: &str,
    source_offset: usize,
    full_source: &str,
    code_ranges: &[std::ops::Range<usize>],
) -> LayoutStage {
    let mut preamble = Vec::new();
    let mut items: Vec<ListItem> = Vec::new();
    let mut numbered_source = false;
    let mut bullet_source = false;
    let mut consumed_syntax_ranges = Vec::new();
    let mut line_offset = 0;
    for segment in block.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        let line = line.strip_suffix('\r').unwrap_or(line);
        let (stripped, backtrack_ranges) =
            strip_trailing_backtrack(line, source_offset + line_offset, full_source, code_ranges);
        let discarded_here = !backtrack_ranges.is_empty();
        consumed_syntax_ranges.extend(backtrack_ranges);
        if is_list_line(&stripped) {
            if let Some(prefix) = list_prefix_range(&stripped) {
                consumed_syntax_ranges.push(
                    source_offset + line_offset + prefix.start
                        ..source_offset + line_offset + prefix.end,
                );
            }
            if stripped.trim_start().starts_with("- ") {
                bullet_source = true;
            } else {
                numbered_source = true;
            }
            items.push(ListItem {
                body: strip_list_prefix(&stripped),
                discarded: discarded_here,
            });
        } else if !items.is_empty() {
            if let Some(last) = items.last_mut() {
                last.body.push('\n');
                last.body.push_str(&stripped);
                last.discarded |= discarded_here;
            }
        } else if !stripped.is_empty() {
            preamble.push(stripped);
        }
        line_offset += segment.len();
    }
    if items.is_empty() {
        let rewritten = preamble.join("\n");
        if rewritten == block {
            consumed_syntax_ranges.clear();
        }
        return LayoutStage {
            text: rewritten,
            consumed_syntax_ranges,
        };
    }
    let mut candidates = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let body = item.body.trim();
        if body.is_empty() {
            continue;
        }
        let has_replacement = index + 1 < items.len();
        if item.discarded && has_replacement {
            continue;
        }
        candidates.push(body.to_string());
    }
    let mut kept = Vec::new();
    for (index, body) in candidates.iter().enumerate() {
        let superseded = candidates.get(index + 1).is_some_and(|next| {
            let a = strip_trailing_punct(body);
            !a.is_empty() && next.starts_with(&a) && next.len() > a.len()
        });
        if !superseded {
            kept.push(body.clone());
        }
    }
    if kept.is_empty() {
        let rewritten = preamble.join("\n");
        return LayoutStage {
            text: rewritten,
            consumed_syntax_ranges,
        };
    }
    let mut out = preamble;
    for (index, item) in kept.iter().enumerate() {
        if numbered_source {
            out.push(format!("{}. {item}", index + 1));
        } else if bullet_source {
            out.push(format!("- {item}"));
        } else {
            out.push(item.clone());
        }
    }
    let rewritten = out.join("\n");
    if rewritten == block {
        consumed_syntax_ranges.clear();
    }
    LayoutStage {
        text: rewritten,
        consumed_syntax_ranges,
    }
}

fn list_prefix_range(line: &str) -> Option<std::ops::Range<usize>> {
    let trimmed = line.trim_start();
    let start = line.len() - trimmed.len();
    if trimmed.starts_with("- ") {
        return Some(start..start + 2);
    }
    let digits = trimmed.chars().take_while(|ch| ch.is_ascii_digit()).count();
    (digits > 0
        && trimmed
            .get(digits..)
            .is_some_and(|rest| rest.starts_with(". ")))
    .then_some(start..start + digits + 2)
}

fn strip_trailing_punct(text: &str) -> String {
    text.trim_end_matches(|ch: char| {
        matches!(
            ch,
            ',' | '，' | '.' | '。' | '!' | '！' | ';' | '；' | ' ' | '\t'
        )
    })
    .to_string()
}

fn strip_trailing_backtrack(
    text: &str,
    source_offset: usize,
    full_source: &str,
    code_ranges: &[std::ops::Range<usize>],
) -> (String, Vec<std::ops::Range<usize>>) {
    let mut end = text.trim_end().len();
    let mut consumed_ranges = Vec::new();
    loop {
        let current = &text[..end];
        let Some((next_end, marker_start)) =
            strip_one_trailing_backtrack(current, source_offset, full_source, code_ranges)
        else {
            break;
        };
        if next_end >= end {
            break;
        }
        consumed_ranges.push(source_offset + marker_start..source_offset + end);
        end = next_end;
    }
    (text[..end].trim_end().to_owned(), consumed_ranges)
}

fn is_trailing_marker_punct(ch: char) -> bool {
    matches!(ch, ',' | '，' | '.' | '。' | '!' | '！' | ' ' | '\t')
}

fn strip_one_trailing_backtrack(
    text: &str,
    source_offset: usize,
    full_source: &str,
    code_ranges: &[std::ops::Range<usize>],
) -> Option<(usize, usize)> {
    const MARKER: &str = "不对";
    let trimmed = text.trim_end_matches(is_trailing_marker_punct);
    let Some(index) = trimmed.rfind(MARKER) else {
        return strip_trailing_english_backtrack(text, source_offset, full_source, code_ranges);
    };
    let after = &trimmed[index + MARKER.len()..];
    if !after.chars().all(is_trailing_marker_punct)
        || is_literal_position(full_source, source_offset + index, code_ranges)
    {
        return strip_trailing_english_backtrack(text, source_offset, full_source, code_ranges);
    }
    let before = trimmed[..index].trim_end_matches(is_trailing_marker_punct);
    let Some(particle) = before.chars().next_back() else {
        return strip_trailing_english_backtrack(text, source_offset, full_source, code_ranges);
    };
    if !matches!(particle, '哦' | '啊' | '喔' | '唔') {
        return strip_trailing_english_backtrack(text, source_offset, full_source, code_ranges);
    }
    let cut = before.len() - particle.len_utf8();
    Some((before[..cut].trim_end().len(), cut))
}

fn strip_trailing_english_backtrack(
    text: &str,
    source_offset: usize,
    full_source: &str,
    code_ranges: &[std::ops::Range<usize>],
) -> Option<(usize, usize)> {
    let lower = text.to_ascii_lowercase();
    for marker in ["scratch that", "oh, wait", "oh wait", "oh, no", "oh no"] {
        let mut search_end = lower.len();
        while let Some(index) = lower[..search_end].rfind(marker) {
            let after = lower[index + marker.len()..].trim_end_matches(is_trailing_marker_punct);
            if after.is_empty()
                && !is_literal_position(full_source, source_offset + index, code_ranges)
            {
                return Some((text[..index].trim_end().len(), index));
            }
            search_end = index;
        }
    }
    None
}

fn is_literal_position(text: &str, index: usize, code_ranges: &[std::ops::Range<usize>]) -> bool {
    is_inside_explicit_quote(text, index)
        || code_ranges
            .iter()
            .any(|range| range.start <= index && index < range.end)
}

fn collect_markers(
    text: &str,
    source_offset: usize,
    full_source: &str,
    code_ranges: &[std::ops::Range<usize>],
    date_ranges: &[std::ops::Range<usize>],
) -> Vec<Marker> {
    let mut markers = Vec::new();
    let mut index = 0;
    while index < text.len() {
        let source_index = source_offset + index;
        if is_inside_explicit_quote(full_source, source_index)
            || code_ranges
                .iter()
                .any(|range| range.start <= source_index && source_index < range.end)
        {
            index += text[index..]
                .chars()
                .next()
                .expect("rest is non-empty")
                .len_utf8();
            continue;
        }
        if let Some(marker) = match_marker_at(text, index) {
            index = marker.end;
            let absolute_start = source_offset + marker.start;
            let absolute_end = source_offset + marker.end;
            let overlaps_date = date_ranges.iter().any(|range| {
                range.start < absolute_end
                    && absolute_start < range.end
                    && !(marker.kind == MarkerKind::Numbered
                        && absolute_start <= range.start
                        && range.end <= absolute_end)
            });
            if !overlaps_date
                && !is_inline_focus_ordinal_reference(text, marker)
                && !is_same_ordinal_copula_body_reference(text, marker, &markers)
            {
                markers.push(marker);
            }
            continue;
        }
        let ch = text[index..].chars().next().expect("rest is non-empty");
        index += ch.len_utf8();
    }
    markers
}

/// An ordinal in the body of “第 N 个是第 N 个 …” belongs to the item text
/// when the parsed ordinal matches the preceding item and the source connects
/// them directly with the copula.
fn is_same_ordinal_copula_body_reference(text: &str, marker: Marker, markers: &[Marker]) -> bool {
    let Some(ordinal) = marker.ordinal else {
        return false;
    };
    let Some(previous) = markers.last() else {
        return false;
    };
    previous.kind == MarkerKind::Numbered
        && previous.ordinal == Some(ordinal)
        && text[previous.end..marker.start].trim() == "是"
}

/// Collect marker identities from a slice while returning ranges in its full source coordinate space.
/// Literal and code exclusions still use `source_offset` against `full_source` before ranges are normalized.
fn collect_markers_with_absolute_ranges(
    text: &str,
    source_offset: usize,
    full_source: &str,
    code_ranges: &[std::ops::Range<usize>],
    date_ranges: &[std::ops::Range<usize>],
) -> Vec<Marker> {
    collect_markers(text, source_offset, full_source, code_ranges, date_ranges)
        .into_iter()
        .map(|mut marker| {
            marker.start += source_offset;
            marker.end += source_offset;
            marker
        })
        .collect()
}

/// Date identities come from the shared protected-span parser. Marker scans
/// consume its absolute UTF-8 ranges in the same full-source coordinate space.
fn protected_date_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    crate::protected_span::protected_spans(text, Some(crate::llm::CleanupOperation::Cleanup))
        .into_iter()
        .filter(|span| span.kind == crate::protected_span::ProtectedSpanKind::Date)
        .map(|span| span.start_byte..span.end_byte)
        .collect()
}

/// An ordinal inside the narrow “只有/only [ordinal] 才 …” focus construction
/// is a body reference, not a new list item. Classify it before constructing
/// the shared marker sequence so rendering, backtracking, and revision proofs
/// see the same source roles.
fn is_inline_focus_ordinal_reference(text: &str, marker: Marker) -> bool {
    if marker.kind != MarkerKind::Numbered || marker.ordinal.is_none() {
        return false;
    }
    let before = text[..marker.start].trim_end();
    let after = text[marker.end..].trim_start();
    let follows_chinese_focus = before.ends_with("只有");
    let follows_english_focus = before
        .len()
        .checked_sub("only".len())
        .and_then(|start| before.get(start..).map(|suffix| (start, suffix)))
        .is_some_and(|(start, suffix)| {
            suffix.eq_ignore_ascii_case("only") && is_ascii_word_left(before, start)
        });
    (follows_chinese_focus || follows_english_focus) && after.starts_with('才')
}

fn match_marker_at(text: &str, start: usize) -> Option<Marker> {
    if let Some(end) = match_chinese_ordinal(text, start) {
        return Some(Marker {
            start,
            end,
            kind: MarkerKind::Numbered,
            ordinal: parse_chinese_ordinal(&text[start..end]),
            may_strip_leading_copula: true,
        });
    }
    if let Some(end) = match_digit_list_marker(text, start) {
        return Some(Marker {
            start,
            end,
            kind: MarkerKind::Numbered,
            ordinal: text[start..end]
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>()
                .parse::<usize>()
                .ok(),
            may_strip_leading_copula: false,
        });
    }
    if let Some(end) = match_yi_shi_marker(text, start) {
        return Some(Marker {
            start,
            end,
            kind: MarkerKind::Numbered,
            ordinal: parse_chinese_ordinal(&text[start..end]),
            may_strip_leading_copula: false,
        });
    }
    if let Some(end) = match_english_ordinal(text, start) {
        return Some(Marker {
            start,
            end,
            kind: MarkerKind::Numbered,
            ordinal: parse_english_ordinal(&text[start..end]),
            may_strip_leading_copula: true,
        });
    }
    if let Some(end) = match_bullet(text, start) {
        return Some(Marker {
            start,
            end,
            kind: MarkerKind::Bullet,
            ordinal: None,
            may_strip_leading_copula: false,
        });
    }
    None
}

fn parse_chinese_ordinal(marker: &str) -> Option<usize> {
    let token = marker
        .strip_prefix('第')
        .or_else(|| marker.strip_suffix('是'))
        .unwrap_or(marker);
    let ascii_digits = token
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    if !ascii_digits.is_empty() {
        if let Ok(value) = ascii_digits.parse::<usize>() {
            return Some(value);
        }
    }
    if let Ok(value) = token.parse::<usize>() {
        return Some(value);
    }
    let mut total = 0usize;
    let mut current = 0usize;
    for ch in token.chars() {
        match ch {
            '零' | '〇' => {}
            '一' => current = 1,
            '二' | '两' => current = 2,
            '三' => current = 3,
            '四' => current = 4,
            '五' => current = 5,
            '六' => current = 6,
            '七' => current = 7,
            '八' => current = 8,
            '九' => current = 9,
            '十' => {
                total += if current == 0 { 1 } else { current } * 10;
                current = 0;
            }
            '百' => {
                total += if current == 0 { 1 } else { current } * 100;
                current = 0;
            }
            '个' | '点' | '项' | '条' | '步' => {}
            _ => return None,
        }
    }
    (total + current > 0).then_some(total + current)
}

fn parse_english_ordinal(marker: &str) -> Option<usize> {
    let marker = marker.to_ascii_lowercase();
    let marker = marker.strip_prefix("number ").unwrap_or(&marker);
    match marker {
        "first" | "one" => Some(1),
        "second" | "two" => Some(2),
        "third" | "three" => Some(3),
        "fourth" | "four" => Some(4),
        "fifth" | "five" => Some(5),
        "sixth" | "six" => Some(6),
        "seventh" | "seven" => Some(7),
        "eighth" | "eight" => Some(8),
        "ninth" | "nine" => Some(9),
        "tenth" | "ten" => Some(10),
        _ => None,
    }
}

fn is_cn_digit(value: char) -> bool {
    matches!(
        value,
        '零' | '一' | '二' | '三' | '四' | '五' | '六' | '七' | '八' | '九' | '十' | '百'
    )
}

fn match_chinese_ordinal(text: &str, start: usize) -> Option<usize> {
    if !text.is_char_boundary(start) || !text[start..].starts_with('第') {
        return None;
    }
    let mut end = start + '第'.len_utf8();
    let mut digits = 0usize;
    for ch in text[end..].chars() {
        if is_cn_digit(ch) || ch.is_ascii_digit() {
            end += ch.len_utf8();
            digits += 1;
        } else {
            break;
        }
    }
    if digits == 0 {
        return None;
    }
    let after = &text[end..];
    if let Some(suffix) = after
        .chars()
        .next()
        .filter(|ch| matches!(*ch, '点' | '条' | '项' | '个' | '步'))
    {
        return Some(end + suffix.len_utf8());
    }
    const DENY: &[&str] = &[
        "时间", "部分", "次", "天", "段", "名", "周", "轮", "年", "页", "章", "节", "批", "遍",
        "回", "季", "月", "位", "种", "类", "场",
    ];
    if DENY.iter().any(|suffix| after.starts_with(suffix)) {
        return None;
    }
    Some(end)
}

fn match_digit_list_marker(text: &str, start: usize) -> Option<usize> {
    if !text.is_char_boundary(start) {
        return None;
    }
    if text[..start]
        .chars()
        .next_back()
        .is_some_and(|ch| ch.is_ascii_digit())
    {
        return None;
    }
    let rest = &text[start..];
    let digits = rest.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let after_digits = rest.get(digits..)?;
    let sep = after_digits.chars().next()?;
    if sep != '.' && sep != '、' {
        return None;
    }
    let end = start + digits + sep.len_utf8();
    match text[end..].chars().next() {
        Some(ch) if ch.is_ascii_alphanumeric() => None,
        _ => Some(end),
    }
}

fn match_yi_shi_marker(text: &str, start: usize) -> Option<usize> {
    const MARKERS: &[&str] = &[
        "一是", "二是", "三是", "四是", "五是", "六是", "七是", "八是", "九是", "十是",
    ];
    if !text.is_char_boundary(start) {
        return None;
    }
    let rest = &text[start..];
    for &from in MARKERS {
        if !rest.starts_with(from) {
            continue;
        }
        let end = start + from.len();
        if is_cjk_left_boundary(text, start) && is_cjk_right_boundary(text, end) {
            return Some(end);
        }
    }
    None
}

const ENGLISH_ORDINALS: &[&str] = &[
    "number ten",
    "number nine",
    "number eight",
    "number seven",
    "number six",
    "number five",
    "number four",
    "number three",
    "number two",
    "number one",
    "first",
    "second",
    "third",
    "fourth",
    "fifth",
    "sixth",
    "seventh",
    "eighth",
    "ninth",
    "tenth",
];

const ORDINAL_FOLLOW_DENY: &[&str] = &[
    "of",
    "all",
    "people",
    "person",
    "time",
    "times",
    "more",
    "day",
    "days",
    "week",
    "weeks",
    "month",
    "months",
    "year",
    "years",
    "thing",
    "things",
    "place",
    "half",
    "option",
    "options",
    "choice",
    "choices",
    "is",
    "are",
    "was",
    "were",
    "seems",
    "seem",
    "feels",
    "feel",
    "looks",
    "look",
    "sounds",
    "sound",
    "approach",
    "approaches",
    "phase",
    "phases",
    "stage",
    "stages",
    "step",
    "steps",
    "part",
    "parts",
    "section",
    "sections",
    "chapter",
    "chapters",
    "item",
    "items",
    "attempt",
    "attempts",
    "draft",
    "drafts",
    "project",
    "projects",
    "idea",
    "ideas",
    "thought",
    "thoughts",
    "version",
    "versions",
    "page",
    "pages",
    "paragraph",
    "paragraphs",
    "round",
    "rounds",
    "costs",
    "cost",
];

const ORDINAL_PRECEDING_DENY: &[&str] = &[
    "the", "a", "an", "this", "that", "each", "every", "my", "our", "your", "his", "her", "its",
    "another", "and", "but", "in", "on", "at", "during", "before", "after", "from", "until",
];

fn match_english_ordinal(text: &str, start: usize) -> Option<usize> {
    if !is_ascii_word_left(text, start) {
        return None;
    }
    let rest = &text[start..];
    for &from in ENGLISH_ORDINALS {
        if !starts_with_ignore_ascii_case(rest, from) {
            continue;
        }
        let end = start + from.len();
        if !is_ascii_word_right(text, end) {
            continue;
        }
        if previous_ascii_word(text, start).is_some_and(|word| {
            ORDINAL_PRECEDING_DENY
                .iter()
                .any(|item| word.eq_ignore_ascii_case(item))
        }) {
            continue;
        }
        if following_word_denied(text, end) {
            continue;
        }
        return Some(end);
    }
    None
}

fn previous_ascii_word(text: &str, start: usize) -> Option<&str> {
    let before = text[..start].trim_end();
    let word_start = before
        .char_indices()
        .rev()
        .find(|(_, ch)| !ch.is_ascii_alphabetic())
        .map(|(index, ch)| index + ch.len_utf8())
        .unwrap_or(0);
    let word = before.get(word_start..)?;
    (!word.is_empty() && word.chars().all(|ch| ch.is_ascii_alphabetic())).then_some(word)
}

fn following_word_denied(text: &str, end: usize) -> bool {
    next_ascii_word(text, end).is_some_and(|word| {
        ORDINAL_FOLLOW_DENY
            .iter()
            .any(|item| word.eq_ignore_ascii_case(item))
    })
}

fn following_word_denied_for_bullet(text: &str, end: usize) -> bool {
    const DENY: &[&str] = &["train", "point", "points", "list", "hole", "journal"];
    next_ascii_word(text, end)
        .is_some_and(|word| DENY.iter().any(|item| word.eq_ignore_ascii_case(item)))
}

fn next_ascii_word(text: &str, start: usize) -> Option<&str> {
    let rest = text[start..].trim_start();
    let end = rest
        .find(|ch: char| !ch.is_ascii_alphabetic())
        .unwrap_or(rest.len());
    let word = rest.get(..end)?;
    (!word.is_empty()).then_some(word)
}

fn match_bullet(text: &str, start: usize) -> Option<usize> {
    const BULLETS: &[&str] = &["项目符号", "要点", "bullets", "bullet"];
    if !text.is_char_boundary(start) {
        return None;
    }
    let rest = &text[start..];
    for &from in BULLETS {
        if !starts_with_ignore_ascii_case(rest, from) {
            continue;
        }
        let end = start + from.len();
        if from == "要点" && (text[end..].starts_with('是') || text[end..].starts_with("说明"))
        {
            continue;
        }
        if from.eq_ignore_ascii_case("bullet") && following_word_denied_for_bullet(text, end) {
            continue;
        }
        let has_boundaries = if is_cjk_command(from) {
            is_cjk_left_boundary(text, start) && is_cjk_right_boundary(text, end)
        } else {
            is_ascii_word_left(text, start) && is_ascii_word_right(text, end)
        };
        if has_boundaries {
            return Some(end);
        }
    }
    None
}

/// Punctuate and apply spoken layout through the legacy test helper.
#[cfg(test)]
pub fn apply_after_punctuation(text: &str, family: ContextFamily, confidence: f32) -> String {
    let applied = apply_after_punctuation_with_provenance(text, family, confidence);
    if applied.source_preserved {
        applied.text
    } else {
        text.to_owned()
    }
}

pub(crate) fn apply_after_punctuation_with_provenance(
    text: &str,
    family: ContextFamily,
    confidence: f32,
) -> AppliedLayout {
    let punctuation = crate::spoken_punctuation::apply_with_provenance(text);
    let punctuation_preserved = preserves_source_protected_content_after_layout(
        text,
        &punctuation.text,
        &punctuation.consumed_syntax_ranges,
    );
    let layout_input = if punctuation_preserved {
        punctuation.text.clone()
    } else {
        text.to_owned()
    };
    let layout = apply_layout_stages(&layout_input, family, confidence, true);
    AppliedLayout {
        text: layout.text,
        source_preserved: punctuation_preserved && layout.source_preserved,
        list_revision_proof: if punctuation_preserved && layout.source_preserved {
            layout.list_revision_proof
        } else {
            None
        },
        projection_text: layout.projection_text,
        projection_source_preserved: punctuation_preserved && layout.projection_source_preserved,
        projection_list_revision_proof: if punctuation_preserved
            && layout.projection_source_preserved
        {
            layout.projection_list_revision_proof
        } else {
            None
        },
        scene_email_layout_applied: punctuation_preserved && layout.scene_email_layout_applied,
    }
}

pub fn has_structural_layout(text: &str) -> bool {
    text.contains('\n') || list_prefix_count(text) > 0
}

/// Preserve transcript words that were explicitly arranged into headings,
/// paragraphs, or list items. Punctuation, case, and line prefixes may change;
/// removing or reordering dictated content rejects the candidate.
pub fn preserves_explicit_layout_content(source: &str, candidate: &str) -> bool {
    if !has_structural_layout(source) {
        return true;
    }
    let source = required_layout_content_chars(source);
    let candidate = layout_content_chars(candidate);
    let mut candidate_index = 0;
    for source_char in source {
        let Some(relative) = candidate[candidate_index..]
            .iter()
            .position(|candidate_char| *candidate_char == source_char)
        else {
            return false;
        };
        candidate_index += relative + 1;
    }
    true
}

/// Require layout and indentation to survive cleanup after best-effort repair.
pub fn preserves_required_layout(source: &str, candidate: &str) -> bool {
    let (mut source_view, mut candidate_view) =
        match crate::protected_span::validated_layout_views(source, candidate) {
            Some(views) => views,
            // Keep the pre-projection layout decision when literal binding
            // fails. This lets the established protected-token stage report
            // inline body mutations instead of preempting it as Layout.
            None => return preserves_layout_views(source, candidate),
        };

    // The same source-backed clause-leading hesitation owner used by cleanup
    // preparation may authorize removing these exact prose token bytes. It
    // excludes quoted/code ranges and returns offsets into the original text;
    // the edit below removes only those token bytes and never a newline.
    let hesitation_ranges = clause_leading_hesitation_ranges(source);
    if !hesitation_ranges.is_empty() {
        let Some(source_without_hesitations) =
            remove_original_hesitation_ranges(source, &hesitation_ranges)
        else {
            return false;
        };
        let Some((projected_source, projected_candidate)) =
            crate::protected_span::validated_layout_views(&source_without_hesitations, candidate)
        else {
            return false;
        };
        source_view = projected_source;
        candidate_view = projected_candidate;
    }

    preserves_layout_views(&source_view, &candidate_view)
}

fn preserves_layout_views(source_view: &str, candidate_view: &str) -> bool {
    if !has_structural_layout(source_view) {
        return !has_structural_layout(candidate_view);
    }
    if !preserves_explicit_layout_content(source_view, candidate_view)
        || newline_count(source_view) != newline_count(candidate_view)
        || list_prefix_count(source_view) != list_prefix_count(candidate_view)
    {
        return false;
    }
    let source_lines = source_view.lines().collect::<Vec<_>>();
    let candidate_lines = candidate_view.lines().collect::<Vec<_>>();
    if source_lines.len() != candidate_lines.len() {
        return false;
    }
    let preserve_indentation = source_lines.iter().any(|line| {
        let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
        indent > 0
    });
    if preserve_indentation {
        return source_view == candidate_view;
    }
    source_lines
        .iter()
        .zip(candidate_lines)
        .all(|(before, after)| {
            if before.trim().is_empty() {
                return after.trim().is_empty();
            }
            if is_list_line(before) && !is_list_line(after) {
                return false;
            }
            preserves_line_content(before, after)
        })
}

fn remove_original_hesitation_ranges(
    source: &str,
    ranges: &[std::ops::Range<usize>],
) -> Option<String> {
    let mut result = String::with_capacity(source.len());
    let mut cursor = 0;
    for range in ranges {
        if range.start < cursor
            || range.start >= range.end
            || range.end > source.len()
            || !source.is_char_boundary(range.start)
            || !source.is_char_boundary(range.end)
        {
            return None;
        }
        result.push_str(source.get(cursor..range.start)?);
        cursor = range.end;
    }
    result.push_str(source.get(cursor..)?);
    Some(result)
}

pub(crate) fn preserves_source_protected_content(source: &str, candidate: &str) -> bool {
    crate::protected_span::preserves(
        source,
        candidate,
        Some(crate::llm::CleanupOperation::Cleanup),
    )
}

pub(crate) fn preserves_source_protected_content_after_layout(
    source: &str,
    candidate: &str,
    consumed_syntax_ranges: &[std::ops::Range<usize>],
) -> bool {
    preserves_source_protected_content_after_layout_with_revisions(
        source,
        candidate,
        consumed_syntax_ranges,
        &[],
        None,
    )
}

fn preserves_source_protected_content_after_layout_with_revisions(
    source: &str,
    candidate: &str,
    consumed_syntax_ranges: &[std::ops::Range<usize>],
    revisions: &[ListItemRevision],
    retention: Option<&ListItemRetention>,
) -> bool {
    if !list_item_revisions_match_source_and_output(source, candidate, revisions)
        || retention.is_some_and(|fact| {
            !list_item_retention_matches_source_and_output(source, candidate, fact, revisions)
        })
    {
        return false;
    }
    let mut syntax_ranges = consumed_syntax_ranges.to_vec();
    syntax_ranges.sort_by_key(|range| range.start);
    let code_ranges = code_literal_ranges(source);
    let quote_ranges = explicit_quote_ranges(source);
    let protected_spans =
        crate::protected_span::protected_spans(source, Some(crate::llm::CleanupOperation::Cleanup));
    let mut previous_end = 0;
    for (index, range) in syntax_ranges.iter().enumerate() {
        if range.start >= range.end
            || range.end > source.len()
            || !source.is_char_boundary(range.start)
            || !source.is_char_boundary(range.end)
            || (index > 0 && range.start < previous_end)
            || code_ranges
                .iter()
                .chain(quote_ranges.iter())
                .any(|literal| range.start < literal.end && literal.start < range.end)
        {
            return false;
        }
        previous_end = range.end;
        if protected_spans.iter().any(|span| {
            range.start < span.end_byte
                && span.start_byte < range.end
                && !(range.start <= span.start_byte && span.end_byte <= range.end)
        }) {
            return false;
        }
    }
    let mut source_without_syntax_terms = source.to_owned();
    let mut authorized_ranges = revisions
        .iter()
        .flat_map(|revision| {
            [
                revision.source_item_range.clone(),
                revision.replacement_source_range.clone(),
            ]
        })
        .collect::<Vec<_>>();
    if let Some(retention) = retention {
        authorized_ranges.extend(
            retention
                .removed_items
                .iter()
                .map(|item| item.source_value_range.clone()),
        );
        if let Some(suffix) = &retention.directive_suffix_range {
            authorized_ranges.push(retention.directive_range.start..suffix.end);
        }
        if let Some(linker) = &retention.ordinal_linker {
            if linker.canonicalized {
                authorized_ranges.push(linker.source_range.clone());
            }
        }
        if let Some(restart) = &retention.leading_restart {
            authorized_ranges.push(restart.source_range.clone());
        }
    }
    authorized_ranges.sort_by_key(|range| range.start);
    let mut previous_authorized_end = 0;
    for range in &authorized_ranges {
        if range.start < previous_authorized_end
            || range.start >= range.end
            || range.end > source.len()
            || !source.is_char_boundary(range.start)
            || !source.is_char_boundary(range.end)
            || syntax_ranges
                .iter()
                .any(|syntax| range.start < syntax.end && syntax.start < range.end)
        {
            return false;
        }
        previous_authorized_end = range.end;
    }
    syntax_ranges.extend(authorized_ranges);
    syntax_ranges.sort_by_key(|range| range.start);
    for range in syntax_ranges.iter().rev() {
        let replacement = " ".repeat(source[range.start..range.end].chars().count());
        source_without_syntax_terms.replace_range(range.clone(), &replacement);
    }
    let mut candidate_without_authorized_revisions = candidate.to_owned();
    let mut output_ranges = revisions
        .iter()
        .map(|revision| revision.output_value_range.clone())
        .collect::<Vec<_>>();
    output_ranges.sort_by_key(|range| range.start);
    for range in output_ranges.iter().rev() {
        let replacement = " ".repeat(candidate[range.clone()].chars().count());
        candidate_without_authorized_revisions.replace_range(range.clone(), &replacement);
    }
    preserves_source_protected_content(
        &source_without_syntax_terms,
        &candidate_without_authorized_revisions,
    )
}

fn list_item_retention_matches_source_and_output(
    source: &str,
    candidate: &str,
    retention: &ListItemRetention,
    revisions: &[ListItemRevision],
) -> bool {
    let valid_range = |text: &str, range: &std::ops::Range<usize>| {
        range.start < range.end
            && range.end <= text.len()
            && text.is_char_boundary(range.start)
            && text.is_char_boundary(range.end)
    };
    let action_matches = match retention.form {
        ListRetentionForm::ExplicitReferences
        | ListRetentionForm::CountOnly
        | ListRetentionForm::RemoveOrdinal => {
            valid_range(source, &retention.action_range)
                && source.get(retention.action_range.clone())
                    == Some(retention.action_value.as_str())
        }
        ListRetentionForm::TerminalDeicticCount => {
            retention.action_range.is_empty()
                && source.is_char_boundary(retention.action_range.start)
                && retention.action_value.is_empty()
        }
    };
    let parsed_source_directive = retention_directive_in_source(source, &retention.directive_range);
    let directive_matches_source = parsed_source_directive.as_ref().is_some_and(|directive| {
        directive.form == retention.form
            && directive.range == retention.directive_range
            && directive.cue_range == retention.cue_range
            && directive.action_range == retention.action_range
            && directive.count_range == retention.count_range
            && (if directive.form == ListRetentionForm::RemoveOrdinal {
                retention.directive_ordinal == Some(directive.keep_count)
            } else {
                directive.keep_count == retention.retained_count
            })
            && directive.references.len() == retention.references.len()
            && directive
                .references
                .iter()
                .zip(&retention.references)
                .all(|(parsed, stored)| {
                    parsed.ordinal == stored.ordinal
                        && parsed.marker_value == stored.marker_value
                        && parsed.marker_range == stored.marker_range
                })
    });
    let form_is_supported = match retention.form {
        ListRetentionForm::ExplicitReferences => {
            retention.retained_count == 2
                && retention.references.len() == retention.retained_count
                && retention.removed_items.len() == 1
        }
        ListRetentionForm::TerminalDeicticCount => {
            retention.retained_count == 2
                && retention.references.is_empty()
                && retention.removed_items.len() == 1
        }
        ListRetentionForm::CountOnly => {
            (2..=MAX_COUNT_ONLY_RETAINED_ITEMS).contains(&retention.retained_count)
                && retention.references.is_empty()
        }
        ListRetentionForm::RemoveOrdinal => {
            retention.retained_count >= 1
                && retention
                    .directive_ordinal
                    .is_some_and(|ordinal| (1..=MAX_COUNT_ONLY_SOURCE_ITEMS).contains(&ordinal))
                && retention.references.is_empty()
                && retention.removed_items.len() == 1
        }
    };
    let ordinal_linker_matches = match retention.ordinal_linker.as_ref() {
        None => true,
        Some(stored) => {
            let output_range_valid = if stored.canonicalized {
                stored.output_range.is_empty()
                    && stored.output_range.end <= candidate.len()
                    && candidate.is_char_boundary(stored.output_range.start)
            } else {
                valid_range(candidate, &stored.output_range)
            };
            find_retained_ordinal_linker(source, retention, revisions, stored.canonicalized)
                .is_some_and(|derived| {
                    stored == &derived
                        && valid_range(source, &stored.source_range)
                        && output_range_valid
                        && source.get(stored.source_range.clone()) == Some("才是")
                        && candidate.get(stored.output_range.clone())
                            == Some(if stored.canonicalized { "" } else { "才是" })
                })
        }
    };
    if !form_is_supported
        || retention.retained_items.len() != retention.retained_count
        || retention.removed_items.is_empty()
        || retention.retained_count + retention.removed_items.len() > MAX_COUNT_ONLY_SOURCE_ITEMS
        || !valid_range(source, &retention.directive_range)
        || !valid_range(source, &retention.cue_range)
        || retention
            .directive_suffix_range
            .as_ref()
            .is_some_and(|range| {
                !valid_range(source, range)
                    || range.start != retention.directive_range.end
                    || !source[range.clone()].chars().all(is_punctuation_or_space)
            })
        || !action_matches
        || !valid_range(source, &retention.count_range)
        || source.get(retention.directive_range.clone()) != Some(retention.directive_value.as_str())
        || source.get(retention.cue_range.clone()) != Some(retention.cue_value.as_str())
        || source.get(retention.count_range.clone()) != Some(retention.count_value.as_str())
        || !directive_matches_source
        || !ordinal_linker_matches
    {
        return false;
    }

    let Some(removed) = retention.removed_items.first() else {
        return false;
    };
    let Some(last_removed) = retention.removed_items.last() else {
        return false;
    };
    let Some(last_retained) = retention.retained_items.last() else {
        return false;
    };
    let leading_restart_matches = retention.leading_restart.as_ref().is_none_or(|restart| {
        let valid_range = |range: &std::ops::Range<usize>| {
            range.start < range.end
                && range.end <= source.len()
                && source.is_char_boundary(range.start)
                && source.is_char_boundary(range.end)
        };
        valid_range(&restart.source_range)
            && valid_range(&restart.marker_range)
            && valid_range(&restart.filler_range)
            && valid_range(&restart.restarted_marker_range)
            && restart.source_range.start == restart.marker_range.start
            && restart.marker_range.end == restart.filler_range.start
            && restart.filler_range.end == restart.restarted_marker_range.start
            && restart.source_range.end == restart.restarted_marker_range.start
            && source.get(restart.marker_range.clone()) == Some(restart.marker_value.as_str())
            && source.get(restart.filler_range.clone()) == Some(restart.filler_value.as_str())
            && source.get(restart.restarted_marker_range.clone())
                == Some(restart.restarted_marker_value.as_str())
            && parse_numbered_marker_ordinal(source, &restart.marker_range)
                .is_some_and(|ordinal| ordinal > 1)
            && parse_numbered_marker_ordinal(source, &restart.restarted_marker_range) == Some(1)
            && is_empty_ordinal_placeholder_filler(&restart.filler_value)
            && !range_overlaps_protected_or_literal_except_numbered_marker(
                source,
                &restart.source_range,
                &restart.marker_range,
            )
            && !has_explanatory_list_preamble(&source[..restart.source_range.start])
            && retention
                .retained_items
                .first()
                .is_some_and(|first| first.marker_range == restart.restarted_marker_range)
    });
    let rendered_list_lines = candidate
        .lines()
        .filter(|line| is_numbered_list_line(line))
        .count();
    if !leading_restart_matches || rendered_list_lines != retention.retained_count {
        return false;
    }
    if (retention.form != ListRetentionForm::RemoveOrdinal
        && (removed.ordinal != retention.retained_count + 1
            || last_retained.ordinal != retention.retained_count))
        || retention
            .removed_items
            .iter()
            .enumerate()
            .any(|(index, item)| {
                (retention.form != ListRetentionForm::RemoveOrdinal
                    && item.ordinal != retention.retained_count + index + 1)
                    || item.source_value_range.start < item.marker_range.end
                    || !valid_range(source, &item.marker_range)
                    || !valid_range(source, &item.source_value_range)
                    || source.get(item.source_value_range.clone())
                        != Some(item.source_value.as_str())
                    || parse_numbered_marker_ordinal(source, &item.marker_range)
                        != Some(item.ordinal)
            })
        || retention.directive_range.start < last_removed.source_value_range.start
            && retention.form != ListRetentionForm::RemoveOrdinal
        || retention.directive_range.end > source.len()
    {
        return false;
    }
    let retained_first = &retention.retained_items[0];
    let block_end = source[retention.directive_range.end..]
        .find("\n\n")
        .map_or(source.len(), |relative| {
            retention.directive_range.end + relative
        });
    let marker_source_start = if retention.form == ListRetentionForm::RemoveOrdinal {
        let Some(first_source_item) = retention
            .retained_items
            .iter()
            .map(|item| (item.ordinal, &item.marker_range))
            .chain(
                retention
                    .removed_items
                    .iter()
                    .map(|item| (item.ordinal, &item.marker_range)),
            )
            .find(|(ordinal, _)| *ordinal == 1)
        else {
            return false;
        };
        first_source_item.1.start
    } else {
        retained_first.marker_range.start
    };
    let marker_source = &source[marker_source_start..block_end];
    // An explicit retention directive repeats ordinal references after the
    // completed list. They are authorization facts, not additional content
    // markers. The parsed directive above is source-bound before these exact
    // marker identities are excluded from the content sequence.
    let mut authorized_non_content_marker_ranges = match retention.form {
        ListRetentionForm::ExplicitReferences => retention
            .references
            .iter()
            .filter(|reference| {
                reference.marker_range.start >= retention.directive_range.start
                    && reference.marker_range.end <= retention.directive_range.end
                    && source.get(reference.marker_range.clone())
                        == Some(reference.marker_value.as_str())
                    && parse_numbered_marker_ordinal(source, &reference.marker_range)
                        == Some(reference.ordinal)
            })
            .map(|reference| reference.marker_range.clone())
            .collect::<Vec<_>>(),
        ListRetentionForm::TerminalDeicticCount | ListRetentionForm::CountOnly => Vec::new(),
        ListRetentionForm::RemoveOrdinal => {
            let marker_range = retention.count_range.clone();
            if source.get(marker_range.clone()).is_some_and(|value| {
                value.ends_with('个')
                    && parse_numbered_marker_ordinal(source, &marker_range)
                        == retention.directive_ordinal
            }) {
                vec![marker_range]
            } else {
                return false;
            }
        }
    };
    // A validated ordinal revision records the repeated ordinal and, for a
    // spoken empty placeholder form, its placeholder marker as non-content
    // source roles. They are not list items in the same parsed marker facts.
    for revision in revisions {
        authorized_non_content_marker_ranges.push(revision.reference_marker_range.clone());
        if let Some(placeholder) = &revision.placeholder_marker_range {
            authorized_non_content_marker_ranges.push(placeholder.clone());
        }
    }
    let source_markers = collect_markers_with_absolute_ranges(
        marker_source,
        marker_source_start,
        source,
        &code_literal_ranges(source),
        &protected_date_ranges(source),
    )
    .into_iter()
    .filter(|marker| marker.kind == MarkerKind::Numbered)
    .filter(|marker| {
        !authorized_non_content_marker_ranges
            .iter()
            .any(|range| range.start == marker.start && range.end == marker.end)
    })
    .collect::<Vec<_>>();
    let source_final_item_contains_directive = source_markers.last().is_some_and(|marker| {
        let final_value = trimmed_range(source, marker.end, block_end);
        final_value.start <= retention.directive_range.start
            && retention.directive_range.end <= final_value.end
    });
    if !source_final_item_contains_directive {
        return false;
    }
    let mut source_item_facts = retention
        .retained_items
        .iter()
        .map(|item| {
            (
                item.ordinal,
                &item.marker_range,
                &item.source_value_range,
                true,
            )
        })
        .chain(retention.removed_items.iter().map(|item| {
            (
                item.ordinal,
                &item.marker_range,
                &item.source_value_range,
                false,
            )
        }))
        .collect::<Vec<_>>();
    source_item_facts.sort_by_key(|(ordinal, _, _, _)| *ordinal);
    // Only directive-owned reference markers and validated correction roles
    // are omitted from the raw source marker scan. Every actual list item must
    // remain in one continuous, correctly numbered sequence.
    if source_markers.len() != retention.retained_items.len() + retention.removed_items.len()
        || source_markers.len() != source_item_facts.len()
        || source_markers
            .iter()
            .zip(&source_item_facts)
            .enumerate()
            .any(
                |(index, (marker, (ordinal, marker_range, source_value_range, retained)))| {
                    let mut item_end = source_markers
                        .get(index + 1)
                        .map_or(block_end, |next| next.start);
                    if retention.form == ListRetentionForm::RemoveOrdinal
                        && index + 1 == source_markers.len()
                        && retention.directive_suffix_range.is_some()
                    {
                        item_end = retention.directive_range.start;
                    }
                    let expected_value_range = trimmed_range(source, marker.end, item_end);
                    let item_revisions = revisions
                        .iter()
                        .filter(|revision| {
                            revision.target_marker_range.start == marker.start
                                && revision.target_marker_range.end == marker.end
                        })
                        .collect::<Vec<_>>();
                    let value_range_matches = if *retained && item_revisions.len() == 1 {
                        let revision = item_revisions[0];
                        source_value_range.start == revision.source_item_range.start
                            && source_value_range.end >= revision.source_item_range.end
                            && revision.source_item_range.start >= expected_value_range.start
                            && revision.source_item_range.end <= item_end
                            && revision.instruction_range.start >= expected_value_range.start
                            && revision.instruction_range.end <= item_end
                            && revision.replacement_source_range.start >= expected_value_range.start
                            && revision.replacement_source_range.end <= item_end
                    } else {
                        item_revisions.is_empty()
                            && source_value_range.start == expected_value_range.start
                            && source_value_range.end == expected_value_range.end
                    };
                    marker.start != marker_range.start
                        || marker.end != marker_range.end
                        || marker.ordinal != Some(*ordinal)
                        || !value_range_matches
                },
            )
        || source_markers
            .iter()
            .enumerate()
            .any(|(index, marker)| marker.ordinal != Some(index + 1))
    {
        return false;
    }
    if retention.form == ListRetentionForm::TerminalDeicticCount
        && (retention.cue_range.start != retention.directive_range.start
            || !is_empty_or_punctuation_source_range(
                source,
                &(removed.marker_range.end..retention.cue_range.start),
            ))
    {
        return false;
    }

    if retention.form == ListRetentionForm::RemoveOrdinal
        && (retention.removed_items.len() != 1
            || retention.directive_ordinal != Some(removed.ordinal)
            || !retention.references.is_empty())
    {
        return false;
    }

    for (index, retained) in retention.retained_items.iter().enumerate() {
        let reference_matches = match retention.form {
            ListRetentionForm::ExplicitReferences => {
                retention.references.get(index).is_some_and(|reference| {
                    reference.ordinal == index + 1
                        && valid_range(source, &reference.marker_range)
                        && reference.marker_range.start >= retention.directive_range.start
                        && reference.marker_range.end <= retention.directive_range.end
                        && parse_numbered_marker_ordinal(source, &reference.marker_range)
                            == Some(reference.ordinal)
                        && source.get(reference.marker_range.clone())
                            == Some(reference.marker_value.as_str())
                })
            }
            ListRetentionForm::TerminalDeicticCount
            | ListRetentionForm::CountOnly
            | ListRetentionForm::RemoveOrdinal => retention.references.is_empty(),
        };
        if !reference_matches
            || (retention.form == ListRetentionForm::RemoveOrdinal
                && index > 0
                && retention.retained_items[index - 1].ordinal >= retained.ordinal)
            || (retention.form != ListRetentionForm::RemoveOrdinal && retained.ordinal != index + 1)
            || retained.rendered_ordinal != index + 1
            || !valid_range(source, &retained.marker_range)
            || !valid_range(source, &retained.source_value_range)
            || !valid_range(candidate, &retained.output_value_range)
            || parse_numbered_marker_ordinal(source, &retained.marker_range)
                != Some(retained.ordinal)
            || retained.source_value_range.start < retained.marker_range.end
            || source.get(retained.source_value_range.clone())
                != Some(retained.source_value.as_str())
            || candidate.get(retained.output_value_range.clone())
                != Some(retained.output_value.as_str())
        {
            return false;
        }
        let line_start = candidate[..retained.output_value_range.start]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        if candidate[line_start..retained.output_value_range.start]
            != format!("{}. ", retained.rendered_ordinal)
        {
            return false;
        }
    }

    let mut source_ranges = retention
        .retained_items
        .iter()
        .flat_map(|item| [item.marker_range.clone(), item.source_value_range.clone()])
        .chain(
            retention
                .removed_items
                .iter()
                .flat_map(|item| [item.marker_range.clone(), item.source_value_range.clone()]),
        )
        .collect::<Vec<_>>();
    if let Some(restart) = &retention.leading_restart {
        source_ranges.push(restart.source_range.clone());
    }
    source_ranges.sort_by_key(|range| range.start);
    source_ranges
        .windows(2)
        .all(|window| window[0].end <= window[1].start)
        && (retention.form == ListRetentionForm::RemoveOrdinal
            || (retention.references.iter().all(|reference| {
                reference.marker_range.start >= retention.directive_range.start
                    && reference.marker_range.end <= retention.directive_range.end
                    && reference.marker_range.start >= last_removed.source_value_range.start
                    && reference.marker_range.end <= last_removed.source_value_range.end
            }) && retention.directive_range.start >= last_removed.source_value_range.start
                && retention.directive_range.end <= last_removed.source_value_range.end))
}

fn retention_directive_in_source(
    source: &str,
    directive_range: &std::ops::Range<usize>,
) -> Option<ListRetentionDirective> {
    if directive_range.start >= directive_range.end
        || directive_range.end > source.len()
        || !source.is_char_boundary(directive_range.start)
        || !source.is_char_boundary(directive_range.end)
    {
        return None;
    }
    let block_start = source[..directive_range.start]
        .rfind("\n\n")
        .map_or(0, |separator| separator + 2);
    let block_end = source[directive_range.end..]
        .find("\n\n")
        .map_or(source.len(), |relative| directive_range.end + relative);
    let mut directive = match parse_list_retention_directive(&source[block_start..block_end]) {
        RetentionDirectiveParse::Parsed(directive) => directive,
        RetentionDirectiveParse::Absent | RetentionDirectiveParse::Unresolved(_) => return None,
    };
    directive.range.start += block_start;
    directive.range.end += block_start;
    directive.cue_range.start += block_start;
    directive.cue_range.end += block_start;
    directive.action_range.start += block_start;
    directive.action_range.end += block_start;
    directive.count_range.start += block_start;
    directive.count_range.end += block_start;
    for reference in &mut directive.references {
        reference.marker_range.start += block_start;
        reference.marker_range.end += block_start;
    }
    Some(directive)
}

fn parse_list_retention_directive(block: &str) -> RetentionDirectiveParse {
    let Some((cue_start, cue_end)) = retention_cue_range(block) else {
        return RetentionDirectiveParse::Absent;
    };
    let suffix = block[cue_start..].trim_end_matches(is_punctuation_or_space);
    let command_offset = cue_end.saturating_sub(cue_start);
    if !retention_command_candidate(&suffix[command_offset..]) {
        return RetentionDirectiveParse::Absent;
    }
    let range = cue_start..cue_start + suffix.len();
    let literal_ranges = crate::spoken_revision::literal_ranges(block);
    if literal_ranges
        .iter()
        .any(|literal| literal.start < range.end && range.start < literal.end)
        || explicit_quote_ranges(block)
            .iter()
            .any(|literal| literal.start < range.end && range.start < literal.end)
    {
        return RetentionDirectiveParse::Unresolved(RetentionDirectiveRole::Quoted);
    }
    let Some(mut directive) = parse_list_retention_directive_text(suffix) else {
        return RetentionDirectiveParse::Unresolved(RetentionDirectiveRole::Unknown);
    };
    directive.range = range.clone();
    for span in [
        &mut directive.cue_range,
        &mut directive.action_range,
        &mut directive.count_range,
    ] {
        span.start += cue_start;
        span.end += cue_start;
    }
    for reference in &mut directive.references {
        reference.marker_range.start += cue_start;
        reference.marker_range.end += cue_start;
    }
    if block[range.end..]
        .trim_end()
        .chars()
        .rev()
        .take_while(|ch| is_punctuation_or_space(*ch))
        .any(|ch| matches!(ch, '?' | '？'))
    {
        return RetentionDirectiveParse::Unresolved(RetentionDirectiveRole::Question);
    }
    match retention_cue_intro_role(block, cue_start) {
        RetentionIntroRole::Direct => {}
        RetentionIntroRole::Hypothetical => {
            return RetentionDirectiveParse::Unresolved(RetentionDirectiveRole::Hypothetical);
        }
        RetentionIntroRole::Unknown => {
            return RetentionDirectiveParse::Unresolved(RetentionDirectiveRole::Unknown);
        }
    }
    RetentionDirectiveParse::Parsed(directive)
}

fn retention_cue_range(block: &str) -> Option<(usize, usize)> {
    let mut start = block.rfind("算了")?;
    let end = start + "算了".len();
    while start >= "算了".len() && block[..start].ends_with("算了") {
        start -= "算了".len();
    }
    if start >= '啊'.len_utf8() && block[..start].ends_with('啊') {
        start -= '啊'.len_utf8();
    }
    Some((start, end))
}

fn retention_cue_intro_role(block: &str, cue_start: usize) -> RetentionIntroRole {
    let prefix = block[..cue_start].trim_end_matches(is_punctuation_or_space);
    let clause_start = prefix
        .char_indices()
        .filter(|(_, ch)| {
            matches!(
                *ch,
                '。' | '！' | '？' | '!' | '?' | '\n' | ';' | '；' | '，' | ','
            )
        })
        .map(|(index, ch)| index + ch.len_utf8())
        .next_back()
        .unwrap_or(0);
    let intro = prefix[clause_start..].trim();
    const REPORTING_FRAMES: &[&str] = &["我说", "我讲"];

    for atom in HYPOTHETICAL_INTRODUCERS {
        let Some(remainder) = intro.strip_prefix(atom) else {
            continue;
        };
        let remainder = remainder.trim();
        if remainder.is_empty() || REPORTING_FRAMES.contains(&remainder) {
            return RetentionIntroRole::Hypothetical;
        }
        // Once a hypothetical introducer is recognized, unsupported trailing
        // material remains unresolved even after a comma inside an item. Item
        // position alone does not prove that the material is ordinary payload.
        return RetentionIntroRole::Unknown;
    }
    RetentionIntroRole::Direct
}

pub(crate) fn has_hypothetical_introducer(text: &str) -> bool {
    HYPOTHETICAL_INTRODUCERS
        .iter()
        .any(|atom| text.contains(atom))
}

fn retention_command_candidate(text: &str) -> bool {
    let mut cursor = 0;
    consume_retention_atom(text, &mut cursor, "吧");
    skip_retention_separators(text, &mut cursor);
    let has_jiu = consume_retention_atom(text, &mut cursor, "就");
    if has_jiu {
        skip_retention_separators(text, &mut cursor);
        if text
            .get(cursor..)
            .is_some_and(|suffix| suffix.starts_with('这'))
        {
            return true;
        }
    }
    if consume_retention_atom(text, &mut cursor, "只") {
        skip_retention_separators(text, &mut cursor);
    }
    text.get(cursor..).is_some_and(|suffix| {
        if suffix.starts_with("不要") {
            return suffix.starts_with("不要第");
        }
        suffix.starts_with("保留")
            || suffix.starts_with("保保留")
            || suffix.starts_with('要')
            || suffix.starts_with('留')
            || suffix.starts_with("前面两个")
            || suffix.starts_with('第')
            || suffix.starts_with("不要")
            || suffix.starts_with("删掉")
    })
}

fn skip_retention_separators(text: &str, cursor: &mut usize) {
    while text
        .get(*cursor..)
        .and_then(|rest| rest.chars().next())
        .is_some_and(|ch| ch.is_whitespace() || matches!(ch, '，' | ',' | '、'))
    {
        let ch = text[*cursor..].chars().next().unwrap();
        *cursor += ch.len_utf8();
    }
}

fn consume_retention_atom(text: &str, cursor: &mut usize, atom: &str) -> bool {
    if text
        .get(*cursor..)
        .is_some_and(|rest| rest.starts_with(atom))
    {
        *cursor += atom.len();
        true
    } else {
        false
    }
}

fn parse_list_retention_directive_text(text: &str) -> Option<ListRetentionDirective> {
    let mut cursor = 0;
    let cue_start = cursor;
    consume_retention_atom(text, &mut cursor, "啊");
    if !consume_retention_atom(text, &mut cursor, "算了") {
        return None;
    }
    consume_retention_atom(text, &mut cursor, "吧");
    skip_retention_separators(text, &mut cursor);
    if text
        .get(cursor..)
        .is_some_and(|rest| rest.starts_with("算了"))
    {
        consume_retention_atom(text, &mut cursor, "算了");
    }
    let cue_range = cue_start..cursor;
    skip_retention_separators(text, &mut cursor);
    let has_jiu = consume_retention_atom(text, &mut cursor, "就");
    skip_retention_separators(text, &mut cursor);
    if has_jiu
        && text
            .get(cursor..)
            .is_some_and(|rest| rest.starts_with('这'))
    {
        let count_start = cursor;
        if !consume_retention_atom(text, &mut cursor, "这两个") || cursor != text.len() {
            return None;
        }
        return Some(ListRetentionDirective {
            form: ListRetentionForm::TerminalDeicticCount,
            range: 0..text.len(),
            cue_range,
            action_range: count_start..count_start,
            count_range: count_start..cursor,
            keep_count: 2,
            references: Vec::new(),
        });
    }
    consume_retention_atom(text, &mut cursor, "只");
    skip_retention_separators(text, &mut cursor);
    let action_start = cursor;
    if consume_retention_atom(text, &mut cursor, "不要")
        || consume_retention_atom(text, &mut cursor, "删掉")
    {
        let action_range = action_start..cursor;
        skip_retention_separators(text, &mut cursor);
        if !consume_retention_atom(text, &mut cursor, "第") {
            return None;
        }
        let count_start = cursor - "第".len();
        let count_end = match_chinese_ordinal(text, count_start)?;
        let marker = &text[count_start..count_end];
        if !marker.ends_with('个') {
            return None;
        }
        let ordinal = parse_chinese_ordinal(marker)?;
        cursor = count_end;
        if cursor != text.len() || !(1..=MAX_COUNT_ONLY_SOURCE_ITEMS).contains(&ordinal) {
            return None;
        }
        return Some(ListRetentionDirective {
            form: ListRetentionForm::RemoveOrdinal,
            range: 0..text.len(),
            cue_range,
            action_range,
            count_range: count_start..cursor,
            keep_count: ordinal,
            references: Vec::new(),
        });
    }
    if consume_retention_atom(text, &mut cursor, "要") {
        // “只要前两个” is a bounded count-only retention instruction.
    } else if text
        .get(cursor..)
        .is_some_and(|rest| rest.starts_with("保保留"))
    {
        consume_retention_atom(text, &mut cursor, "保保留");
    } else if !consume_retention_atom(text, &mut cursor, "保留")
        && !consume_retention_atom(text, &mut cursor, "留")
    {
        return None;
    }
    let action_range = action_start..cursor;
    skip_retention_separators(text, &mut cursor);
    let count_start = cursor;
    let (keep_count, count_end) = parse_retention_count(text, cursor)?;
    cursor = count_end;
    let count_range = count_start..cursor;
    skip_retention_separators(text, &mut cursor);

    if cursor == text.len() {
        if !(2..=MAX_COUNT_ONLY_RETAINED_ITEMS).contains(&keep_count) {
            return None;
        }
        return Some(ListRetentionDirective {
            form: ListRetentionForm::CountOnly,
            range: 0..text.len(),
            cue_range,
            action_range,
            count_range,
            keep_count,
            references: Vec::new(),
        });
    }
    if keep_count != 2 {
        return None;
    }

    let mut references = Vec::with_capacity(2);
    for expected in 1..=2 {
        let start = cursor;
        let end = match_chinese_ordinal(text, cursor)?;
        if parse_chinese_ordinal(&text[start..end]) != Some(expected) {
            return None;
        }
        references.push(ListRetentionReference {
            ordinal: expected,
            marker_range: start..end,
            marker_value: text[start..end].to_owned(),
        });
        cursor = end;
        if expected == 1 {
            skip_retention_separators(text, &mut cursor);
            if !consume_retention_atom(text, &mut cursor, "和") {
                return None;
            }
            skip_retention_separators(text, &mut cursor);
        }
    }
    if cursor != text.len() {
        return None;
    }
    Some(ListRetentionDirective {
        form: ListRetentionForm::ExplicitReferences,
        range: 0..text.len(),
        cue_range,
        action_range,
        count_range,
        keep_count,
        references,
    })
}

fn parse_retention_count(text: &str, start: usize) -> Option<(usize, usize)> {
    if !text.is_char_boundary(start) {
        return None;
    }
    let mut cursor = start;
    if consume_retention_atom(text, &mut cursor, "前面") {
        // Keep the count adjacent to the direction phrase so unrelated nouns
        // or entities cannot become part of a deletion command.
    } else if !consume_retention_atom(text, &mut cursor, "前") {
        return None;
    }

    let number_start = cursor;
    while text
        .get(cursor..)
        .and_then(|rest| rest.chars().next())
        .is_some_and(is_retention_count_numeral)
    {
        let ch = text[cursor..].chars().next()?;
        cursor += ch.len_utf8();
    }
    if cursor == number_start {
        return None;
    }
    let number = &text[number_start..cursor];
    let keep_count = match number {
        "二" | "两" | "兩" => 2,
        "三" => 3,
        "四" => 4,
        "五" => 5,
        "六" => 6,
        "七" => 7,
        "八" => 8,
        "九" => 9,
        "十" => 10,
        _ if number.chars().all(|ch| ch.is_ascii_digit()) => number.parse().ok()?,
        _ => return None,
    };
    if !(2..=MAX_COUNT_ONLY_RETAINED_ITEMS).contains(&keep_count) {
        return None;
    }
    let unit = text.get(cursor..)?.chars().next()?;
    if !matches!(
        unit,
        '个' | '個' | '项' | '項' | '条' | '條' | '点' | '點' | '步'
    ) {
        return None;
    }
    cursor += unit.len_utf8();
    Some((keep_count, cursor))
}

fn is_retention_count_numeral(ch: char) -> bool {
    ch.is_ascii_digit()
        || matches!(
            ch,
            '二' | '两' | '兩' | '三' | '四' | '五' | '六' | '七' | '八' | '九' | '十'
        )
}

fn retention_directive_matches_items(
    directive: &ListRetentionDirective,
    markers: &[(usize, Marker)],
    block: &str,
) -> bool {
    let count_and_source_bounds_match = match directive.form {
        ListRetentionForm::ExplicitReferences | ListRetentionForm::TerminalDeicticCount => {
            directive.keep_count == 2 && markers.len() == 3
        }
        ListRetentionForm::CountOnly => {
            (2..=MAX_COUNT_ONLY_RETAINED_ITEMS).contains(&directive.keep_count)
                && markers.len() > directive.keep_count
                && markers.len() <= MAX_COUNT_ONLY_SOURCE_ITEMS
        }
        ListRetentionForm::RemoveOrdinal => {
            (2..=MAX_COUNT_ONLY_SOURCE_ITEMS).contains(&markers.len())
                && (1..=markers.len()).contains(&directive.keep_count)
        }
    };
    if !count_and_source_bounds_match {
        return false;
    }
    let ordered = markers.iter().enumerate().all(|(index, (_, marker))| {
        marker.ordinal == Some(index + 1)
            && (directive.form == ListRetentionForm::RemoveOrdinal
                || index < directive.keep_count
                || marker.start < directive.range.start)
    });
    ordered
        && match directive.form {
            ListRetentionForm::ExplicitReferences => {
                directive.references.len() == 2
                    && directive
                        .references
                        .iter()
                        .enumerate()
                        .all(|(index, reference)| {
                            reference.ordinal == index + 1
                                && block.get(reference.marker_range.clone())
                                    == Some(reference.marker_value.as_str())
                        })
            }
            ListRetentionForm::TerminalDeicticCount => {
                directive.references.is_empty()
                    && markers.last().is_some_and(|(_, marker)| {
                        is_empty_or_punctuation_source_range(
                            block,
                            &(marker.end..directive.range.start),
                        )
                    })
            }
            ListRetentionForm::CountOnly | ListRetentionForm::RemoveOrdinal => {
                directive.references.is_empty()
                    && (directive.form != ListRetentionForm::RemoveOrdinal
                        || markers
                            .iter()
                            .any(|(_, marker)| marker.ordinal == Some(directive.keep_count)))
            }
        }
}

fn list_item_revisions_match_source_and_output(
    source: &str,
    candidate: &str,
    revisions: &[ListItemRevision],
) -> bool {
    let mut source_ranges = Vec::new();
    let mut output_ranges = Vec::new();
    for revision in revisions {
        let valid_range = |text: &str, range: &std::ops::Range<usize>| {
            range.start < range.end
                && range.end <= text.len()
                && text.is_char_boundary(range.start)
                && text.is_char_boundary(range.end)
        };
        if !valid_range(source, &revision.target_marker_range)
            || revision
                .placeholder_marker_range
                .as_ref()
                .is_some_and(|range| !valid_range(source, range))
            || !valid_range(source, &revision.reference_marker_range)
            || !valid_range(source, &revision.instruction_range)
            || !valid_range(source, &revision.cue_range)
            || !valid_range(source, &revision.copula_range)
            || !valid_range(source, &revision.source_item_range)
            || !valid_range(source, &revision.replacement_source_range)
            || !valid_range(candidate, &revision.output_value_range)
            || source.get(revision.source_item_range.clone())
                != Some(revision.source_value.as_str())
            || source.get(revision.replacement_source_range.clone())
                != Some(revision.replacement_value.as_str())
            || !candidate
                .get(revision.output_value_range.clone())
                .is_some_and(|output| {
                    output
                        .strip_prefix(&revision.replacement_prefix)
                        .is_some_and(|value| {
                            value == revision.replacement_value
                                || strip_terminal_list_separator(
                                    revision.replacement_value.clone(),
                                    source,
                                    revision.replacement_source_range.clone(),
                                    0,
                                    &crate::spoken_revision::literal_ranges(source),
                                    &code_literal_ranges(source),
                                ) == value
                        })
                })
            || (!revision.replacement_prefix.is_empty()
                && (revision.copula_value != revision.replacement_prefix
                    || !revision
                        .source_value
                        .starts_with(&revision.replacement_prefix)))
            || source.get(revision.cue_range.clone()) != Some(revision.cue_value.as_str())
            || source.get(revision.copula_range.clone()) != Some(revision.copula_value.as_str())
            || parse_numbered_marker_ordinal(source, &revision.target_marker_range)
                != Some(revision.ordinal)
            || parse_numbered_marker_ordinal(source, &revision.reference_marker_range)
                != Some(revision.ordinal)
            || revision.source_item_range.start < revision.target_marker_range.end
            || match revision.placeholder_marker_range.as_ref() {
                Some(placeholder_range) => {
                    revision.source_item_range.end > placeholder_range.start
                        || revision.instruction_range.start != placeholder_range.end
                }
                None => revision.instruction_range.start != revision.source_item_range.end,
            }
            || revision.instruction_range.end != revision.replacement_source_range.start
            || revision.cue_range.start < revision.instruction_range.start
            || revision.cue_range.end > revision.instruction_range.end
            || revision.reference_marker_range.start < revision.instruction_range.start
            || revision.reference_marker_range.end > revision.instruction_range.end
            || revision.copula_range.start < revision.reference_marker_range.end
            || revision.copula_range.end > revision.instruction_range.end
            || revision.replacement_source_range.start < revision.copula_range.end
            || !instruction_is_only_authorized_syntax(
                source,
                revision.instruction_range.start,
                revision.instruction_range.end,
                &revision.cue_range,
                &revision.cue_value,
                &revision.reference_marker_range,
                &revision.copula_value,
            )
        {
            return false;
        }
        let line_start = candidate[..revision.output_value_range.start]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        if candidate[line_start..revision.output_value_range.start]
            != format!("{}. ", revision.rendered_ordinal)
        {
            return false;
        }
        source_ranges.push(revision.target_marker_range.clone());
        if let Some(placeholder_range) = revision.placeholder_marker_range.as_ref() {
            source_ranges.push(placeholder_range.clone());
        }
        source_ranges.extend([
            revision.instruction_range.clone(),
            revision.source_item_range.clone(),
            revision.replacement_source_range.clone(),
        ]);
        output_ranges.push(revision.output_value_range.clone());
    }
    source_ranges.sort_by_key(|range| range.start);
    output_ranges.sort_by_key(|range| range.start);
    source_ranges
        .windows(2)
        .all(|window| window[0].end <= window[1].start)
        && output_ranges
            .windows(2)
            .all(|window| window[0].end <= window[1].start)
}

fn strip_terminal_list_separator(
    mut rendered: String,
    source: &str,
    absolute_source_range: std::ops::Range<usize>,
    source_offset: usize,
    literal_ranges: &[std::ops::Range<usize>],
    code_ranges: &[std::ops::Range<usize>],
) -> String {
    let Some(local_start) = absolute_source_range.start.checked_sub(source_offset) else {
        return rendered;
    };
    let Some(local_end) = absolute_source_range.end.checked_sub(source_offset) else {
        return rendered;
    };
    let Some(source_value) = source.get(local_start..local_end) else {
        return rendered;
    };
    if let Some((comma_offset, comma)) = source_value.char_indices().last() {
        if matches!(comma, '，' | ',') && rendered.ends_with(comma) {
            let local_comma = local_start + comma_offset;
            let absolute_comma = source_offset + local_comma;
            let comma_range = absolute_comma..absolute_comma + comma.len_utf8();
            let in_literal = literal_ranges.iter().any(|range| {
                range.start < local_comma + comma.len_utf8() && local_comma < range.end
            });
            let in_code = code_ranges
                .iter()
                .any(|range| range.start < comma_range.end && comma_range.start < range.end);
            if !in_literal && !in_code {
                rendered.pop();
            }
        }
    }
    rendered
}

fn parse_numbered_marker_ordinal(source: &str, range: &std::ops::Range<usize>) -> Option<usize> {
    let marker = source.get(range.clone())?;
    if let Some(value) = marker.strip_prefix('第') {
        return parse_chinese_ordinal(value);
    }
    if marker.ends_with('是') {
        return parse_chinese_ordinal(marker);
    }
    let digits = marker
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    if !digits.is_empty() {
        return digits.parse().ok();
    }
    parse_english_ordinal(marker)
}

fn preserves_line_content(source: &str, candidate: &str) -> bool {
    let source_body = strip_list_prefix(source);
    let source = if is_numbered_list_line(source) {
        clear_list_disfluency_content_chars(&source_body)
    } else {
        layout_content_chars(&source_body)
    };
    let candidate = layout_content_chars(&strip_list_prefix(candidate));
    let mut candidate_index = 0;
    for source_char in source {
        let Some(relative) = candidate[candidate_index..]
            .iter()
            .position(|candidate_char| *candidate_char == source_char)
        else {
            return false;
        };
        candidate_index += relative + 1;
    }
    true
}

fn required_layout_content_chars(text: &str) -> Vec<char> {
    let mut content = Vec::new();
    for line in text.lines() {
        if is_numbered_list_line(line) {
            content.extend(clear_list_disfluency_content_chars(&strip_list_prefix(
                line,
            )));
        } else {
            content.extend(layout_content_chars(line));
        }
    }
    content
}

/// Return required content while excluding only source-backed, standalone
/// disfluencies from numbered list items. Quoted/code text and ordinary words
/// such as “umbrella” remain required content.
fn clear_list_disfluency_content_chars(text: &str) -> Vec<char> {
    let literals = crate::spoken_revision::literal_ranges(text);
    let mut removable = Vec::<std::ops::Range<usize>>::new();
    for (start, _) in text.char_indices() {
        if let Some(range) = standalone_filler_token_range(text, start, false) {
            if !literals
                .iter()
                .any(|literal| range.start < literal.end && literal.start < range.end)
            {
                removable.push(range);
            }
        }
    }
    for (prefix, continuation) in [
        ("你你", "呢"),
        ("我我", "觉得"),
        ("我我", "认为"),
        ("我我", "想"),
    ] {
        let mut search_start = 0;
        while let Some(relative) = text[search_start..].find(prefix) {
            let start = search_start + relative;
            let end = start + prefix.len();
            let first_char_end = start + prefix.chars().next().map(char::len_utf8).unwrap_or(0);
            if text[end..].starts_with(continuation)
                && !literals
                    .iter()
                    .any(|literal| start < literal.end && literal.start < end)
            {
                removable.push(start..first_char_end);
            }
            search_start = end;
        }
    }
    let chars = text
        .char_indices()
        .filter(|(index, _)| {
            !removable
                .iter()
                .any(|range| range.start <= *index && *index < range.end)
        })
        .map(|(_, ch)| ch)
        .filter(|ch| is_content_char(*ch))
        .flat_map(char::to_lowercase)
        .collect();
    chars
}

/// Recognize only a source-backed, standalone filler token. The same token
/// owner feeds list normalization and the layout guard's required-content
/// projection so that meaningful compounds cannot be hidden from either.
pub(crate) fn standalone_filler_token_range(
    text: &str,
    start: usize,
    follows_proved_syntax: bool,
) -> Option<std::ops::Range<usize>> {
    if start > text.len() || !text.is_char_boundary(start) {
        return None;
    }
    let before = text[..start].chars().next_back();
    if !follows_proved_syntax && before.is_some_and(|ch| !is_disfluency_boundary(ch)) {
        return None;
    }
    let tail = &text[start..];
    let token = ["erm", "uh", "um", "er", "呃", "嗯", "啊"]
        .into_iter()
        .find(|token| starts_with_ignore_ascii_case(tail, token))?;
    let end = start + token.len();
    let after = text[end..].chars().next();
    if after.is_some_and(|ch| !is_disfluency_boundary(ch)) {
        // Require an actual token boundary in both scripts. This preserves
        // meaningful compounds such as “嗯哼”, “啊哈”, and English words
        // beginning with “um”, rather than guessing from a syllable prefix.
        return None;
    }
    Some(start..end)
}

/// Source-backed English hesitation tokens that begin a clause rather than a
/// name or identifier. This is intentionally narrower than the general list
/// filler recognizer: a preceding sentence/clause boundary and the same real
/// token boundaries are required, and quoted/code literals are never classified.
pub(crate) fn clause_leading_hesitation_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let literals = crate::spoken_revision::literal_ranges(text);
    let mut ranges = Vec::new();
    for (start, ch) in text.char_indices() {
        if !ch.eq_ignore_ascii_case(&'u') {
            continue;
        }
        let Some(token) = text.get(start..start.saturating_add(2)) else {
            continue;
        };
        if !matches!(token, "Um" | "Uh" | "um" | "uh") {
            continue;
        }
        let end = start + token.len();
        let before_is_boundary = has_sentence_clause_boundary(text, start);
        let remainder = &text[end..];
        let after_is_boundary = remainder
            .chars()
            .next()
            .is_some_and(|next| is_disfluency_boundary(next) && is_clause_separator(next));
        let finite_clause = simple_finite_clause_continuation_range(remainder);
        let is_literal = literals.iter().any(|literal| {
            let continuation_end = finite_clause
                .as_ref()
                .map_or(end, |range| end.saturating_add(range.end));
            literal.start < continuation_end && start < literal.end
        });
        if before_is_boundary && after_is_boundary && finite_clause.is_some() && !is_literal {
            ranges.push(start..end);
        }
    }
    ranges
}

fn has_sentence_clause_boundary(text: &str, offset: usize) -> bool {
    let Some(prefix) = text.get(..offset) else {
        return false;
    };
    let prefix = prefix.trim_end();
    prefix.is_empty()
        || prefix.chars().next_back().is_some_and(|previous| {
            matches!(previous, '.' | '?' | '!' | '。' | '？' | '！' | ';' | '；')
        })
        || crate::protected_span::has_closed_fenced_block_boundary_before(text, offset)
}

/// This is deliberately a small role recognizer, not an English parser. The
/// filler waiver is useful only when the token introduces a bounded subject
/// clause or imperative; labels, names, and isolated hesitation sounds remain
/// protected source terms.
fn simple_finite_clause_continuation_range(remainder: &str) -> Option<std::ops::Range<usize>> {
    let separator_start = skip_horizontal_space(remainder, 0);
    let (separator_width, separator) = remainder[separator_start..].char_indices().next()?;
    if !is_clause_separator(separator) {
        return None;
    }
    let content_start = skip_horizontal_space(
        remainder,
        separator_start + separator_width + separator.len_utf8(),
    );
    let words = initial_english_word_spans(remainder, content_start, 8);
    if !words
        .first()
        .and_then(|first| remainder.get(content_start..first.start))
        .is_some_and(|gap| gap.chars().all(is_horizontal_space))
    {
        return None;
    }
    let first = remainder.get(words.first()?.clone())?;
    let second = words.get(1).and_then(|range| remainder.get(range.clone()));
    let third = words.get(2).and_then(|range| remainder.get(range.clone()));
    let gap_is_space = |left: usize, right: usize| -> bool {
        words
            .get(left)
            .zip(words.get(right))
            .and_then(|(left, right)| remainder.get(left.end..right.start))
            .is_some_and(|gap| gap.chars().all(is_horizontal_space))
    };
    let range_end = if matches!(
        first.to_ascii_lowercase().as_str(),
        "i" | "we" | "you" | "they" | "he" | "she" | "it" | "this"
    ) && second.is_some_and(is_finite_clause_verb)
        && gap_is_space(0, 1)
    {
        words[1].end
    } else if first.eq_ignore_ascii_case("my")
        && ((second.is_some_and(|word| word.eq_ignore_ascii_case("i"))
            && third.is_some_and(is_finite_clause_verb)
            && gap_is_my_i_restart(remainder.get(words[0].end..words[1].start)?)
            && gap_is_space(1, 2))
            || (second.is_some_and(|word| word.eq_ignore_ascii_case("major"))
                && third.is_some_and(|word| {
                    matches!(word.to_ascii_lowercase().as_str(), "is" | "was" | "were")
                })
                && gap_is_space(0, 1)
                && gap_is_space(1, 2)))
    {
        words[2].end
    } else if let Some(predicate_end) =
        article_subject_finite_predicate_end(remainder, first, &words, &gap_is_space)
    {
        predicate_end
    } else if is_clear_imperative(first) && has_immediate_imperative_object(remainder, words[0].end)
    {
        words[0].end
    } else {
        return None;
    };
    Some(separator_start..range_end)
}

fn has_immediate_imperative_object(text: &str, action_end: usize) -> bool {
    let object_start = skip_horizontal_space(text, action_end);
    let Some(tail) = text.get(object_start..) else {
        return false;
    };
    let Some(first) = tail.chars().next() else {
        return false;
    };
    let closing_quote = match first {
        '"' => Some('"'),
        '\'' => Some('\''),
        '`' => Some('`'),
        '“' => Some('”'),
        '‘' => Some('’'),
        '「' => Some('」'),
        '『' => Some('』'),
        _ => None,
    };
    if let Some(closing_quote) = closing_quote {
        let Some(literal) = crate::spoken_revision::literal_ranges(text)
            .into_iter()
            .find(|range| {
                range.start == object_start && range.end > object_start + first.len_utf8()
            })
        else {
            return false;
        };
        return text
            .get(literal.clone())
            .and_then(|value| value.chars().next_back())
            == Some(closing_quote);
    }
    if !first.is_alphanumeric() {
        return false;
    }
    let Some(word) = initial_english_word_spans(tail, 0, 1)
        .first()
        .and_then(|range| tail.get(range.clone()))
    else {
        return false;
    };
    !matches!(
        word.to_ascii_lowercase().as_str(),
        "and" | "or" | "but" | "then"
    )
}

fn article_subject_finite_predicate_end(
    text: &str,
    determiner: &str,
    words: &[std::ops::Range<usize>],
    gap_is_space: &impl Fn(usize, usize) -> bool,
) -> Option<usize> {
    if !matches!(determiner.to_ascii_lowercase().as_str(), "a" | "an" | "the") {
        return None;
    }
    let predicate_index = words
        .iter()
        .enumerate()
        .skip(2)
        .find_map(|(index, range)| {
            text.get(range.clone())
                .is_some_and(is_finite_auxiliary)
                .then_some(index)
        })?;
    if !(2..=6).contains(&predicate_index) {
        return None;
    }
    let subject = &words[1..predicate_index];
    let is_conjunction = |range: &std::ops::Range<usize>| {
        text.get(range.clone())
            .is_some_and(|word| matches!(word.to_ascii_lowercase().as_str(), "and" | "or"))
    };
    if subject.is_empty()
        || is_conjunction(&subject[0])
        || is_conjunction(subject.last()?)
        || !gap_is_space(0, 1)
        || subject
            .windows(2)
            .any(|pair| is_conjunction(&pair[0]) && is_conjunction(&pair[1]))
        || !subject
            .windows(2)
            .enumerate()
            .all(|(index, _)| gap_is_space(index + 1, index + 2))
        || !gap_is_space(predicate_index - 1, predicate_index)
    {
        return None;
    }
    Some(words[predicate_index].end)
}

fn is_finite_auxiliary(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "am" | "are"
            | "is"
            | "was"
            | "were"
            | "have"
            | "has"
            | "had"
            | "do"
            | "does"
            | "did"
            | "can"
            | "could"
            | "will"
            | "would"
            | "should"
            | "might"
            | "must"
    )
}

fn is_clear_imperative(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "add"
            | "check"
            | "close"
            | "confirm"
            | "keep"
            | "leave"
            | "open"
            | "read"
            | "remove"
            | "review"
            | "save"
            | "send"
            | "use"
            | "wait"
            | "write"
    )
}

fn initial_english_word_spans(
    text: &str,
    start: usize,
    limit: usize,
) -> Vec<std::ops::Range<usize>> {
    let mut words = Vec::new();
    let mut cursor = start;
    while cursor < text.len() && words.len() < limit {
        while cursor < text.len() {
            let Some(ch) = text[cursor..].chars().next() else {
                break;
            };
            if is_horizontal_space(ch) || matches!(ch, '-' | '–' | '—') {
                cursor += ch.len_utf8();
            } else {
                break;
            }
        }
        let token_start = cursor;
        while cursor < text.len() {
            let byte = text.as_bytes()[cursor];
            if byte.is_ascii_alphabetic() {
                cursor += 1;
            } else {
                break;
            }
        }
        if token_start == cursor {
            break;
        }
        words.push(token_start..cursor);
    }
    words
}

fn skip_horizontal_space(text: &str, mut cursor: usize) -> usize {
    while cursor < text.len() {
        let Some(ch) = text[cursor..].chars().next() else {
            break;
        };
        if is_horizontal_space(ch) {
            cursor += ch.len_utf8();
        } else {
            break;
        }
    }
    cursor
}

fn is_horizontal_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t')
}

fn gap_is_my_i_restart(gap: &str) -> bool {
    !gap.is_empty()
        && gap
            .chars()
            .all(|ch| is_horizontal_space(ch) || matches!(ch, '-' | '–' | '—'))
}

fn is_finite_clause_verb(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "am" | "are"
            | "is"
            | "was"
            | "were"
            | "have"
            | "has"
            | "had"
            | "do"
            | "does"
            | "did"
            | "can"
            | "could"
            | "will"
            | "would"
            | "should"
            | "might"
            | "must"
            | "need"
            | "want"
            | "think"
            | "major"
            | "work"
            | "study"
    )
}

pub(crate) fn is_disfluency_boundary(ch: char) -> bool {
    !ch.is_ascii_alphanumeric() && ch != '_' && !is_cjk_letter(ch)
}

fn is_clause_separator(ch: char) -> bool {
    ch.is_ascii_punctuation()
        || matches!(
            ch,
            '，' | '。' | '！' | '？' | '；' | '：' | '、' | '…' | '—' | '–'
        )
}

fn layout_content_chars(text: &str) -> Vec<char> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = strip_list_prefix(line);
        out.extend(
            line.chars()
                .filter(|ch| is_content_char(*ch))
                .flat_map(char::to_lowercase),
        );
    }
    out
}

/// If the LLM flattened spoken line breaks or list prefixes, restore the
/// breaks while keeping in-line polish. Invented breaks on unstructured
/// transcripts are collapsed.
pub fn restore_if_flattened(before: &str, after: &str) -> String {
    if crate::protected_span::has_fenced_code_literals(before) {
        // Fenced code is validated as an exact source-bound projection by the
        // later layout/literal owners. Do not let raw-line repair alter a
        // valid wrapper-drop candidate or reconstruct an invalid body from
        // the source before those owners can inspect it.
        let _validated_projection = crate::protected_span::validated_layout_views(before, after);
        return after.to_string();
    }
    if crate::protected_span::has_code_literals(before) {
        return restore_inline_code_layout(before, after);
    }
    restore_layout_without_code(before, after)
}

fn restore_inline_code_layout(before: &str, after: &str) -> String {
    let Some(regions) = crate::protected_span::validated_code_region_pairs(before, after) else {
        // A raw candidate without a complete source-bound body must reach the
        // existing literal guard unchanged. Layout repair must never recreate
        // source code and thereby manufacture its own proof.
        return after.to_string();
    };
    if regions.is_empty() {
        return restore_layout_without_code(before, after);
    }

    let mut markers = Vec::with_capacity(regions.len());
    for index in 0..regions.len() {
        let Some(marker) = fresh_layout_code_marker(index, before, after, &markers) else {
            return after.to_string();
        };
        markers.push(marker);
    }

    let source_regions = regions
        .iter()
        .map(|(source, _)| source.clone())
        .collect::<Vec<_>>();
    let candidate_regions = regions
        .iter()
        .map(|(_, candidate)| candidate.clone())
        .collect::<Vec<_>>();
    let Some(source_view) = layout_view_with_code_markers(before, &source_regions, &markers) else {
        return after.to_string();
    };
    let Some(candidate_view) = layout_view_with_code_markers(after, &candidate_regions, &markers)
    else {
        return after.to_string();
    };

    let restored_view = restore_layout_without_code(&source_view, &candidate_view);
    reinsert_candidate_code_regions(&restored_view, after, &candidate_regions, &markers)
        .unwrap_or_else(|| after.to_string())
}

fn fresh_layout_code_marker(
    index: usize,
    source: &str,
    candidate: &str,
    existing: &[String],
) -> Option<String> {
    (0..256).find_map(|nonce| {
        let marker = format!("VFOPAQUECODE{index}TOKEN{nonce}END");
        (!source.contains(&marker)
            && !candidate.contains(&marker)
            && existing.iter().all(|prior| prior != &marker))
        .then_some(marker)
    })
}

fn layout_view_with_code_markers(
    text: &str,
    regions: &[std::ops::Range<usize>],
    markers: &[String],
) -> Option<String> {
    if regions.len() != markers.len() {
        return None;
    }
    let mut view = String::with_capacity(text.len());
    let mut cursor = 0;
    for (region, marker) in regions.iter().zip(markers) {
        if region.start < cursor
            || region.start >= region.end
            || region.end > text.len()
            || !text.is_char_boundary(region.start)
            || !text.is_char_boundary(region.end)
            || text.get(region.clone()).is_none()
        {
            return None;
        }
        view.push_str(text.get(cursor..region.start)?);
        view.push_str(marker);
        cursor = region.end;
    }
    view.push_str(text.get(cursor..)?);
    Some(view)
}

fn reinsert_candidate_code_regions(
    restored_view: &str,
    candidate: &str,
    regions: &[std::ops::Range<usize>],
    markers: &[String],
) -> Option<String> {
    if regions.len() != markers.len() {
        return None;
    }
    let mut output = String::with_capacity(restored_view.len());
    let mut cursor = 0;
    for (region, marker) in regions.iter().zip(markers) {
        if restored_view.match_indices(marker).count() != 1 {
            return None;
        }
        let relative_start = restored_view.get(cursor..)?.find(marker)?;
        let marker_start = cursor + relative_start;
        let marker_end = marker_start + marker.len();
        output.push_str(restored_view.get(cursor..marker_start)?);
        output.push_str(candidate.get(region.clone())?);
        cursor = marker_end;
    }
    output.push_str(restored_view.get(cursor..)?);
    Some(output)
}

fn restore_layout_without_code(before: &str, after: &str) -> String {
    if !has_structural_layout(before) {
        if newline_count(after) > 0 || list_prefix_count(after) > 0 {
            return collapse_invented_structure(after);
        }
        return after.to_string();
    }
    if newline_count(after) == newline_count(before)
        && list_prefix_count(after) >= list_prefix_count(before)
    {
        return after.to_string();
    }
    reinsert_breaks(before, after).unwrap_or_else(|| after.to_string())
}

fn collapse_extra_breaks(piece: &str) -> String {
    let mut lines = piece.lines();
    let Some(first) = lines.next() else {
        return String::new();
    };
    let rest = lines
        .map(strip_list_prefix)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if rest.is_empty() {
        first.trim().to_string()
    } else {
        format!("{} {}", first.trim(), rest.join(" "))
    }
}

fn collapse_invented_structure(after: &str) -> String {
    after
        .lines()
        .map(strip_list_prefix)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_list_prefix(line: &str) -> String {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix("- ") {
        return rest.trim().to_string();
    }
    let digits = trimmed.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digits > 0 {
        if let Some(rest) = trimmed.get(digits..) {
            if let Some(rest) = rest.strip_prefix(". ") {
                return rest.trim().to_string();
            }
        }
    }
    trimmed.to_string()
}

fn is_content_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || is_cjk_letter(ch)
}

fn content_chars(text: &str) -> Vec<char> {
    text.chars()
        .filter(|ch| is_content_char(*ch))
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn reinsert_breaks(before: &str, after: &str) -> Option<String> {
    let lines: Vec<&str> = before.split('\n').collect();
    let after_chars: Vec<char> = after.chars().collect();
    let mut index = 0usize;
    let mut out = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        if line.is_empty() {
            out.push(String::new());
            continue;
        }
        while index < after_chars.len() && after_chars[index].is_whitespace() {
            index += 1;
        }
        let needed = content_chars(line);
        if needed.is_empty() {
            out.push((*line).to_string());
            continue;
        }
        let start = index;
        let mut matched = 0usize;
        while index < after_chars.len() && matched < needed.len() {
            let ch = after_chars[index];
            if is_content_char(ch) {
                let lower = ch.to_lowercase().next().unwrap_or(ch);
                if lower == needed[matched] {
                    matched += 1;
                    index += 1;
                } else {
                    break;
                }
            } else {
                index += 1;
            }
        }
        let last_content = lines[line_index + 1..]
            .iter()
            .all(|item| item.is_empty() || content_chars(item).is_empty());
        if matched == 0 {
            index = start;
            continue;
        }
        if matched < needed.len() {
            if last_content {
                index = after_chars.len();
            }
        } else {
            while index < after_chars.len()
                && !is_content_char(after_chars[index])
                && !after_chars[index].is_whitespace()
            {
                index += 1;
            }
            if last_content {
                index = after_chars.len();
            }
        }
        let piece: String = after_chars[start..index].iter().collect();
        // Any breaks inside an aligned piece were introduced by the model;
        // only breaks from the source line sequence are reinserted below.
        out.push(collapse_extra_breaks(&piece));
    }
    Some(out.join("\n"))
}

fn newline_count(text: &str) -> usize {
    text.chars().filter(|&ch| ch == '\n').count()
}

fn list_prefix_count(text: &str) -> usize {
    text.lines().filter(|line| is_list_line(line)).count()
}

fn is_numbered_list_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let digits = trimmed.chars().take_while(|ch| ch.is_ascii_digit()).count();
    digits > 0
        && trimmed
            .get(digits..)
            .is_some_and(|rest| rest.starts_with(". "))
}

fn is_list_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("- ") {
        return true;
    }
    is_numbered_list_line(line)
}

fn join_layout_parts(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn apply_email_layout_stage(text: &str) -> LayoutStage {
    let (greeting, rest) = peel_greeting(text);
    let (body, closing) = peel_closing(&rest);
    LayoutStage {
        text: join_layout_parts(&[greeting, body, closing]),
        consumed_syntax_ranges: Vec::new(),
    }
}

const EMAIL_GREETINGS: &[&str] = &["hello", "hey", "dear", "hi", "您好", "你好"];
const EMAIL_CLOSINGS: &[&str] = &[
    "best regards",
    "kind regards",
    "thank you",
    "regards",
    "thanks",
    "best",
    "此致",
    "谢谢",
];
const GREETING_BODY_STARTERS: &[&str] = &[
    "looking", "please", "can", "could", "would", "will", "let", "just", "the", "this", "that",
    "i", "we", "i'm", "im", "my", "thanks", "thank", "um", "uh", "a",
];
const CJK_NAME_STOP: &[char] = &[
    '我', '请', '帮', '今', '明', '那', '这', '有', '会', '已', '先', '谢', '赶', '把', '给', '能',
    '要', '想', '看', '开',
];

fn peel_greeting(text: &str) -> (String, String) {
    let Some((start, end)) = find_greeting_range(text) else {
        return (String::new(), text.to_owned());
    };
    (
        text[start..end].trim().to_string(),
        text[end..].trim().to_string(),
    )
}

fn find_greeting_range(text: &str) -> Option<(usize, usize)> {
    let start = skip_leading_whitespace(text, 0);
    if is_inside_explicit_quote(text, start) || is_inside_code(text, start) {
        return None;
    }
    let rest = &text[start..];
    for &from in EMAIL_GREETINGS {
        if !starts_with_ignore_ascii_case(rest, from) {
            continue;
        }
        let mut end = start + from.len();
        if is_cjk_command(from) {
            if !is_cjk_right_boundary(text, end) || cjk_greeting_is_question(text, end) {
                continue;
            }
            end = consume_cjk_name(text, end);
        } else {
            if !is_ascii_word_right(text, end) {
                continue;
            }
            end = consume_english_name(text, end);
        }
        if !text[start..end].trim().is_empty() {
            return Some((start, end));
        }
    }
    None
}

fn consume_english_name(text: &str, start: usize) -> usize {
    let mut end = start;
    for _ in 0..2 {
        let next_start = skip_leading_whitespace(text, end);
        if next_start >= text.len() {
            break;
        }
        let rest = &text[next_start..];
        let word_len = rest
            .find(|ch: char| !ch.is_ascii_alphabetic() && ch != '\'')
            .unwrap_or(rest.len());
        if word_len == 0 {
            break;
        }
        let word = &rest[..word_len];
        if is_body_starter(word) || is_signature_function_word(word) {
            break;
        }
        end = next_start + word_len;
        if rest[word_len..].starts_with(',') {
            end += 1;
        }
    }
    end
}

fn cjk_greeting_is_question(text: &str, end: usize) -> bool {
    text[end..]
        .chars()
        .next()
        .is_some_and(|ch| matches!(ch, '吗' | '啊' | '呀' | '呢' | '吧' | '嘛'))
}

fn is_body_starter(word: &str) -> bool {
    GREETING_BODY_STARTERS
        .iter()
        .any(|item| word.eq_ignore_ascii_case(item))
}

fn is_signature_function_word(word: &str) -> bool {
    const DENY: &[&str] = &[
        "option", "way", "for", "to", "of", "if", "when", "that", "this", "please", "us", "me",
        "you", "it", "one", "the", "an", "and", "or", "but", "with", "about", "at", "on", "in",
        "by", "from", "as", "so", "not",
    ];
    DENY.iter().any(|item| word.eq_ignore_ascii_case(item))
}

fn consume_cjk_name(text: &str, start: usize) -> usize {
    let mut end = start;
    for (count, ch) in text[start..].chars().enumerate() {
        if !is_cjk_letter(ch) || CJK_NAME_STOP.contains(&ch) || count >= 4 {
            break;
        }
        end += ch.len_utf8();
    }
    end
}

fn peel_closing(text: &str) -> (String, String) {
    let Some((start, token_end)) = find_last_closing(text) else {
        return (text.to_string(), String::new());
    };
    let signature = text[token_end..].trim();
    if !is_short_signature(signature) {
        return (text.to_string(), String::new());
    }
    let body = text[..start].trim().to_string();
    let closing_word = text[start..token_end].trim().to_string();
    let closing = if signature.is_empty() {
        closing_word
    } else {
        format!("{closing_word}\n{signature}")
    };
    (body, closing)
}

fn find_last_closing(text: &str) -> Option<(usize, usize)> {
    let mut found = None;
    let code_ranges = code_literal_ranges(text);
    let mut index = 0;
    while index < text.len() {
        if is_inside_explicit_quote(text, index)
            || code_ranges
                .iter()
                .any(|range| range.start <= index && index < range.end)
        {
            index += text[index..]
                .chars()
                .next()
                .expect("rest is non-empty")
                .len_utf8();
            continue;
        }
        if let Some(end) = match_token_from(text, index, EMAIL_CLOSINGS) {
            let cjk = is_cjk_command(&text[index..end]);
            let ok = if cjk {
                is_cjk_left_boundary(text, index) && is_cjk_right_boundary(text, end)
            } else {
                is_ascii_word_left(text, index) && is_ascii_word_right(text, end)
            };
            if ok {
                found = Some((index, end));
                index = end;
                continue;
            }
        }
        let ch = text[index..].chars().next().expect("rest is non-empty");
        index += ch.len_utf8();
    }
    found
}

/// Return only whitespace separators that the existing email recognizers can
/// prove between a greeting/body, body/closing, or closing/signature. No
/// content bytes are consumed, so callers can render the same grouping with
/// source-offset edits while keeping the legacy canonical output intact.
pub(crate) fn email_layout_separator_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::with_capacity(3);
    if let Some((greeting_start, greeting_end)) = find_greeting_range(text) {
        let body_start = skip_leading_whitespace(text, greeting_end);
        if body_start > greeting_end && body_start < text.len() {
            ranges.push(greeting_end..body_start);
        } else if body_start == greeting_end
            && body_start < text.len()
            && text[greeting_start..greeting_end]
                .chars()
                .any(is_cjk_letter)
        {
            // Compact CJK greeting/body boundaries can be source-proven by
            // the recognized greeting token even without an intervening gap.
            ranges.push(greeting_end..greeting_end);
        }
    }

    if let Some((closing_start, closing_end)) = find_last_closing(text) {
        let signature_start = skip_leading_whitespace(text, closing_end);
        let signature = &text[signature_start..];
        let body_end = text[..closing_start].trim_end().len();
        let body = text[..body_end].trim();
        let closing = &text[closing_start..closing_end];
        let compact_cjk_signoff = matches!(closing, "谢谢" | "此致") && signature.trim().is_empty();
        let proper_name_signature = !signature.trim().is_empty()
            && output_email_signature_is_safe(signature)
            && closing.chars().next().is_some_and(char::is_uppercase);
        let preceding_is_sentence_end = text[..body_end]
            .chars()
            .next_back()
            .is_some_and(|ch| matches!(ch, '.' | '!' | '?' | '。' | '！' | '？'));
        if !body.is_empty()
            && (compact_cjk_signoff || preceding_is_sentence_end || proper_name_signature)
            && output_email_signature_is_safe(signature)
        {
            if body_end < closing_start {
                ranges.push(body_end..closing_start);
            } else if compact_cjk_signoff {
                ranges.push(closing_start..closing_start);
            }
            if !signature.trim().is_empty() && signature_start > closing_end {
                ranges.push(closing_end..signature_start);
            }
        }
    }

    ranges.sort_by_key(|range| range.start);
    ranges.dedup();
    if ranges.iter().any(|range| {
        range.start > range.end
            || range.end > text.len()
            || !text
                .get(range.clone())
                .is_some_and(|value| value.chars().all(char::is_whitespace))
    }) || ranges.windows(2).any(|pair| pair[0].end > pair[1].start)
    {
        return Vec::new();
    }
    ranges
}

fn output_email_signature_is_safe(text: &str) -> bool {
    let signature = text.trim().trim_start_matches(',').trim();
    if signature.is_empty() {
        return text.trim().is_empty();
    }
    if signature.chars().all(is_cjk_letter) {
        return cjk_len(signature) <= 8;
    }
    let words = signature.split_whitespace().collect::<Vec<_>>();
    (1..=3).contains(&words.len())
        && words.iter().all(|word| {
            let mut chars = word.chars();
            chars.next().is_some_and(|first| first.is_ascii_uppercase())
                && chars.all(|ch| ch.is_ascii_alphabetic() || ch == '\'')
        })
}

fn is_short_signature(text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    if cjk_len(text) > 8 {
        return false;
    }
    let words = text.split_whitespace().collect::<Vec<_>>();
    if words.len() > 3 {
        return false;
    }
    words.iter().all(|word| {
        let first = word.chars().next();
        first.is_some_and(|ch| ch.is_ascii_alphabetic() || is_cjk_letter(ch))
            && !is_body_starter(word)
            && !is_signature_function_word(word)
    })
}

fn match_token_from(text: &str, start: usize, tokens: &[&str]) -> Option<usize> {
    let rest = &text[start..];
    for &from in tokens {
        if starts_with_ignore_ascii_case(rest, from) {
            return Some(start + from.len());
        }
    }
    None
}

fn cjk_len(text: &str) -> usize {
    text.chars().filter(|ch| is_cjk_letter(*ch)).count()
}

fn apply_line_commands_stage(text: &str) -> LayoutStage {
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    let mut consumed_syntax_ranges = Vec::new();
    while index < text.len() {
        if let Some((end, replacement)) = match_line_command_at(text, index) {
            while out.ends_with(char::is_whitespace) {
                out.pop();
            }
            out.push_str(replacement);
            consumed_syntax_ranges.push(index..end);
            index = skip_leading_whitespace(text, end);
            continue;
        }
        let ch = text[index..].chars().next().expect("rest is non-empty");
        out.push(ch);
        index += ch.len_utf8();
    }
    LayoutStage {
        text: out,
        consumed_syntax_ranges,
    }
}

fn match_line_command_at(text: &str, start: usize) -> Option<(usize, &'static str)> {
    if !text.is_char_boundary(start) {
        return None;
    }
    if is_inside_explicit_quote(text, start) || is_inside_code(text, start) {
        return None;
    }
    let rest = &text[start..];
    for &(from, to) in LINE_COMMANDS {
        if !starts_with_ignore_ascii_case(rest, from) {
            continue;
        }
        let end = start + from.len();
        if is_cjk_command(from) {
            if !is_cjk_left_boundary(text, start) || !is_cjk_right_boundary(text, end) {
                continue;
            }
            if negated_cjk_break(text, start) {
                continue;
            }
            let after = text[end..].trim_start();
            if ["符", "功能", "按钮", "选项", "命令", "指令", "这个功能"]
                .iter()
                .any(|word| after.starts_with(word))
            {
                continue;
            }
        } else {
            if !is_ascii_word_left(text, start) || !is_ascii_word_right(text, end) {
                continue;
            }
            if followed_by_character_word(text, end) {
                continue;
            }
            if followed_by_layout_noun(text, end) {
                continue;
            }
            if !from.eq_ignore_ascii_case("start a new paragraph")
                && previous_ascii_word(text, start).is_some_and(is_plain_layout_reference)
            {
                continue;
            }
            if negated_english_break(text, start) {
                continue;
            }
        }
        return Some((end, to));
    }
    None
}

fn starts_with_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    haystack.len() >= needle.len()
        && haystack.is_char_boundary(needle.len())
        && haystack[..needle.len()].eq_ignore_ascii_case(needle)
}

fn is_cjk_command(from: &str) -> bool {
    from.chars().any(is_cjk_letter)
}

fn negated_cjk_break(text: &str, start: usize) -> bool {
    text[..start].ends_with("不要") || text[..start].ends_with('不')
}

fn negated_english_break(text: &str, start: usize) -> bool {
    let before = text[..start].trim_end().to_ascii_lowercase();
    before.ends_with("do not")
        || before.ends_with("don't")
        || before.ends_with("dont")
        || before.ends_with(" not")
}

fn followed_by_character_word(text: &str, end: usize) -> bool {
    let rest = text[end..].trim_start();
    starts_with_ignore_ascii_case(rest, "character") && is_ascii_word_right(rest, "character".len())
}

fn followed_by_layout_noun(text: &str, end: usize) -> bool {
    const DENIED: &[&str] = &[
        "feature", "function", "button", "option", "command", "commands", "item", "items",
        "setting", "settings", "of", "for", "in", "with", "is", "was", "are", "means", "would",
        "could", "will", "can", "to", "contains", "reads", "output", "this", "that",
    ];
    let rest = text[end..].trim_start();
    DENIED.iter().any(|word| {
        starts_with_ignore_ascii_case(rest, word) && is_ascii_word_right(rest, word.len())
    })
}

fn is_plain_layout_reference(word: &str) -> bool {
    const DENIED: &[&str] = &[
        "a", "an", "the", "this", "that", "these", "those", "our", "their", "another", "each",
        "every", "first", "last", "next",
    ];
    DENIED
        .iter()
        .any(|candidate| word.eq_ignore_ascii_case(candidate))
}

pub(crate) fn is_inside_explicit_quote(text: &str, start: usize) -> bool {
    explicit_quote_ranges(text)
        .iter()
        .any(|range| range.start < start && start < range.end)
}

fn explicit_quote_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let code_ranges = code_literal_ranges(text);
    crate::spoken_revision::literal_ranges(text)
        .into_iter()
        // Keep a quote range that encloses a code fence; filter only code's own range.
        .filter(|range| {
            !code_ranges
                .iter()
                .any(|code| code.start == range.start && code.end == range.end)
        })
        .collect()
}

fn is_inside_code(text: &str, index: usize) -> bool {
    code_literal_ranges(text)
        .iter()
        .any(|range| range.start <= index && index < range.end)
}

fn skip_leading_whitespace(text: &str, start: usize) -> usize {
    let mut index = start;
    for ch in text[start..].chars() {
        if !ch.is_whitespace() {
            break;
        }
        index += ch.len_utf8();
    }
    index
}

fn is_cjk_letter(value: char) -> bool {
    matches!(
        value,
        '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}'
    )
}

fn is_boundary_punct(value: char) -> bool {
    value.is_whitespace()
        || value.is_ascii_punctuation()
        || matches!(
            value,
            '\u{3000}'..='\u{303F}'
                | '\u{FF01}'..='\u{FF0F}'
                | '\u{FF1A}'..='\u{FF20}'
                | '\u{FF3B}'..='\u{FF40}'
                | '\u{FF5B}'..='\u{FF65}'
        )
}

fn is_cjk_left_boundary(text: &str, start: usize) -> bool {
    text[..start]
        .chars()
        .next_back()
        .map(|ch| is_boundary_punct(ch) || is_cjk_letter(ch))
        .unwrap_or(true)
}

fn is_cjk_right_boundary(text: &str, end: usize) -> bool {
    text[end..]
        .chars()
        .next()
        .map(|ch| is_boundary_punct(ch) || is_cjk_letter(ch))
        .unwrap_or(true)
}

fn is_ascii_word_left(text: &str, start: usize) -> bool {
    text[..start]
        .chars()
        .next_back()
        .map(|ch| !ch.is_ascii_alphanumeric())
        .unwrap_or(true)
}

fn is_ascii_word_right(text: &str, end: usize) -> bool {
    text[end..]
        .chars()
        .next()
        .map(|ch| !ch.is_ascii_alphanumeric())
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::{
        apply, apply_after_punctuation_with_provenance, apply_layout_stages,
        parse_list_retention_directive, restore_if_flattened, ListLayoutProof, ListRetentionForm,
        RetentionDirectiveParse, RetentionDirectiveRole,
    };
    use crate::context::ContextFamily;

    fn layout(text: &str) -> String {
        apply(text, ContextFamily::General, 1.0)
    }

    #[test]
    fn date_suffixes_are_not_markers_and_absolute_ranges_keep_the_full_source_offset() {
        let source =
            "前置说明。\n\n日期2026-10-18、金额12.50美元；无效日期2026-99-18、仍保留；18、真实编号";
        let prefix_len = "前置说明。\n\n".len();
        let block = &source[prefix_len..];
        let code_ranges = super::code_literal_ranges(source);
        let date_ranges = super::protected_date_ranges(source);
        assert!(date_ranges
            .iter()
            .any(|range| { source.get(range.clone()) == Some("2026-10-18") }));
        assert!(date_ranges
            .iter()
            .any(|range| { source.get(range.clone()) == Some("2026-99-18") }));

        let markers = super::collect_markers(block, prefix_len, source, &code_ranges, &date_ranges);
        assert_eq!(markers.len(), 1);
        assert_eq!(&block[markers[0].start..markers[0].end], "18、");

        let absolute = super::collect_markers_with_absolute_ranges(
            block,
            prefix_len,
            source,
            &code_ranges,
            &date_ranges,
        );
        assert_eq!(absolute.len(), 1);
        assert_eq!(source.get(absolute[0].start..absolute[0].end), Some("18、"));
        assert_eq!(absolute[0].start, prefix_len + markers[0].start);
    }

    #[test]
    fn mid_sentence_english_new_line_inserts_a_break() {
        assert_eq!(
            layout("the reading club new line should be tomorrow"),
            "the reading club\nshould be tomorrow"
        );
    }

    #[test]
    fn accepts_wispr_line_command_aliases() {
        assert_eq!(layout("hello next line world"), "hello\nworld");
        assert_eq!(layout("hello line break world"), "hello\nworld");
        assert_eq!(layout("hello skip a line world"), "hello\nworld");
        assert_eq!(layout("hello newline world"), "hello\nworld");
        assert_eq!(layout("hello new-line world"), "hello\nworld");
    }

    #[test]
    fn mid_sentence_chinese_line_break_does_not_need_spaces() {
        assert_eq!(layout("请换行然后继续"), "请\n然后继续");
        assert_eq!(layout("先写完另起一行再改"), "先写完\n再改");
    }

    #[test]
    fn spoken_new_paragraph_inserts_a_blank_line() {
        assert_eq!(
            layout("第一段内容 新段落 第二段内容"),
            "第一段内容\n\n第二段内容"
        );
        assert_eq!(
            layout("first block new paragraph second block"),
            "first block\n\nsecond block"
        );
        assert_eq!(
            layout("first start a new paragraph second"),
            "first\n\nsecond"
        );
    }

    #[test]
    fn rejects_newline_character_and_negated_chinese_break() {
        assert_eq!(
            layout("use a newline character here"),
            "use a newline character here"
        );
        assert_eq!(layout("这里不要换行谢谢"), "这里不要换行谢谢");
        assert_eq!(layout("这里不换行"), "这里不换行");
        assert_eq!(layout("使用换行符即可"), "使用换行符即可");
        assert_eq!(
            layout("please do not new line here"),
            "please do not new line here"
        );
        assert_eq!(
            layout("please don't new line here"),
            "please don't new line here"
        );
    }

    #[test]
    fn calendar_and_section_classifiers_are_not_lists() {
        assert_eq!(
            layout("第一天做设计第二天写测试"),
            "第一天做设计第二天写测试"
        );
        assert_eq!(
            layout("第一段写背景第二段写结论"),
            "第一段写背景第二段写结论"
        );
    }

    #[test]
    fn ge_ordinals_list_but_first_thought_stays_prose() {
        assert_eq!(
            layout("第一个完成设计第二个写测试"),
            "1. 完成设计\n2. 写测试"
        );
        assert_eq!(
            layout("我第一个想到的是先验证数据"),
            "我第一个想到的是先验证数据"
        );
    }

    #[test]
    fn unfinished_terminal_ordinal_does_not_render_an_empty_list_item() {
        let source = "嗯，测试一下，看一下具体的这个流程是什么。然后看一下详细的它这个准确度是什么。第一个是看它这个稳定性，第二个是看它的这个准确度，第三个。";
        let applied = apply_after_punctuation_with_provenance(source, ContextFamily::General, 1.0);

        assert_eq!(
            applied.text,
            "嗯，测试一下，看一下具体的这个流程是什么。然后看一下详细的它这个准确度是什么。\n\n1. 看它这个稳定性\n2. 看它的这个准确度"
        );
        assert!(applied.source_preserved);
        assert!(!applied.text.contains("3. 。"));
        assert_eq!(
            crate::llm::local_cleanup(&applied.text),
            "测试一下，看一下具体的这个流程是什么。然后看一下详细的它这个准确度是什么。\n\n1. 看它这个稳定性\n2. 看它的这个准确度"
        );
    }

    #[test]
    fn completed_terminal_ordinal_remains_a_list_item() {
        let source = "第一个是看它这个稳定性，第二个是看它的这个准确度，第三个是看它的延迟。";
        let applied = apply_after_punctuation_with_provenance(source, ContextFamily::General, 1.0);

        assert_eq!(
            applied.text,
            "1. 看它这个稳定性\n2. 看它的这个准确度\n3. 看它的延迟。"
        );
        assert!(applied.source_preserved);
    }

    #[test]
    fn punctuation_only_quoted_ordinal_is_preserved_as_literal_content() {
        let source = "第一个是稳定性，第二个是准确度，第三个是“。”。";
        let applied = apply_after_punctuation_with_provenance(source, ContextFamily::General, 1.0);

        assert_eq!(applied.text, "1. 稳定性\n2. 准确度\n3. “。”。");
        assert!(applied.source_preserved);
    }

    #[test]
    fn terminal_ordinal_keeps_affirmations_and_nonseparator_symbols() {
        for (body, expected_item) in [
            ("是啊", "是啊"),
            ("是啊哈这样", "啊哈这样"),
            ("+", "+"),
            ("%", "%"),
            ("/", "/"),
            ("*", "*"),
        ] {
            let source = format!("第一个是甲，第二个是乙，第三个{body}");
            let applied =
                apply_after_punctuation_with_provenance(&source, ContextFamily::General, 1.0);

            assert_eq!(
                applied.text,
                format!("1. 甲\n2. 乙\n3. {expected_item}"),
                "terminal list item was not preserved: {body}"
            );
            assert!(applied.source_preserved, "source check failed for: {body}");
        }
    }

    #[test]
    fn explicit_ordinal_revision_replaces_only_the_named_same_block_item() {
        let source = "一下具体的流程是什么？第一个是，呃，看一下这个L O M的clean up是不是正常。第二个是，哦不对，第一个是看一下L O M的clean up是不是正确。第二个才是是不是正常。第三个是稳定性强不强？";
        let applied = apply_layout_stages(source, ContextFamily::General, 1.0, true);

        assert_eq!(
            applied.text,
            "一下具体的流程是什么？\n\n1. 看一下L O M的clean up是不是正确。\n2. 是不是正常。\n3. 稳定性强不强？"
        );
        assert!(applied.source_preserved);
        let proof = applied
            .list_revision_proof
            .as_ref()
            .expect("authorized list revision proof");
        assert!(proof.is_valid());
        assert_eq!(proof.revisions.len(), 1);
        assert_eq!(proof.revisions[0].ordinal, 1);
        assert_eq!(
            proof.revisions[0].source_value,
            "是，呃，看一下这个L O M的clean up是不是正常。"
        );
        assert_eq!(
            proof.revisions[0].replacement_value,
            "看一下L O M的clean up是不是正确。"
        );
    }

    #[test]
    fn same_utterance_correction_and_keep_prefix_retract_only_the_named_third_item() {
        let source = "但是它的这个流程是什么？具体，呃，测试不对不对，帮我看一下这个它这个软件具体测试什么。第一个是看一下它的这个稳定性，啊不对，第一个是准确性，第二个才是稳定性。然后第三个是帮我看一下它的这个UI设计，啊算了算了，就保保留前面两个，第一个和第二个。";
        let applied = apply_layout_stages(source, ContextFamily::General, 1.0, true);
        assert_eq!(
            applied.text,
            "但是它的这个流程是什么？具体，呃，测试不对不对，帮我看一下这个它这个软件具体测试什么。\n\n1. 准确性\n2. 才是稳定性。"
        );
        assert!(applied.source_preserved);
        let proof = applied
            .list_revision_proof
            .as_ref()
            .expect("same-utterance correction and explicit prefix retention proof");
        assert!(proof.is_valid());
        assert_eq!(proof.revisions.len(), 1);
        let retention = proof.retention.as_ref().expect("retention proof");
        assert_eq!(retention.retained_count, 2);
        assert_eq!(retention.cue_value, "啊算了算了");
        assert_eq!(retention.action_value, "保保留");
        assert_eq!(retention.count_value, "前面两个");
        assert_eq!(
            retention
                .removed_items
                .iter()
                .map(|item| item.ordinal)
                .collect::<Vec<_>>(),
            vec![3]
        );
        let repeated = apply_layout_stages(&applied.text, ContextFamily::General, 1.0, true);
        assert_eq!(repeated.text, applied.text);
        assert!(repeated.source_preserved);

        for unresolved in [
            source.replace("前面两个，第一个和第二个", "前面两个，第一个和第三个"),
            source.replace("前面两个，第一个和第二个", "前面三个，第一个和第二个"),
            source.replace(
                "啊算了算了，就保保留前面两个，第一个和第二个",
                "啊算了算了，就保保留前面两个，‘第一个和第二个’",
            ),
            source.replace(
                "但是它的这个流程",
                "假设我说：第一个是开始。但是它的这个流程",
            ),
        ] {
            let rejected = apply_layout_stages(&unresolved, ContextFamily::General, 1.0, true);
            assert_eq!(
                rejected.text, unresolved,
                "unresolved retention must remain source-bound"
            );
            assert!(rejected.source_preserved);
            assert!(rejected.list_revision_proof.is_none());
            assert_eq!(
                apply_layout_stages(&rejected.text, ContextFamily::General, 1.0, true).text,
                unresolved,
                "unresolved directive must be idempotently preserved"
            );
        }

        let hypothetical = "第一个是准确性。第二个是稳定性。第三个是UI设计。假设我说啊算了算了，就保留前面两个，第一个和第二个。";
        assert!(matches!(
            parse_list_retention_directive(hypothetical),
            RetentionDirectiveParse::Unresolved(RetentionDirectiveRole::Hypothetical)
        ));
        for incomplete_intro in [
            "第一个是准确性。第二个是稳定性。第三个是UI设计。假设啊算了，只保留前面两个，第一个和第二个。",
            "第一个是准确性。第二个是稳定性。第三个是UI设计。比如啊算了，只保留前面两个，第一个和第二个。",
        ] {
            assert!(matches!(
                parse_list_retention_directive(incomplete_intro),
                RetentionDirectiveParse::Unresolved(RetentionDirectiveRole::Hypothetical)
            ));
        }
        let unknown_intro = "第一个是准确性。第二个是稳定性。第三个是UI设计。假设我啊算了，只保留前面两个，第一个和第二个。";
        assert!(matches!(
            parse_list_retention_directive(unknown_intro),
            RetentionDirectiveParse::Unresolved(RetentionDirectiveRole::Unknown)
        ));
        let unknown_result = apply_layout_stages(unknown_intro, ContextFamily::General, 1.0, true);
        assert_eq!(unknown_result.text, unknown_intro);
        assert!(unknown_result.list_revision_proof.is_none());
        assert_eq!(
            apply_layout_stages(&unknown_result.text, ContextFamily::General, 1.0, true).text,
            unknown_intro
        );
        let question = "第一个是准确性。第二个是稳定性。第三个是UI设计，啊算了算了，就保留前面两个，第一个和第二个？！";
        assert!(matches!(
            parse_list_retention_directive(question),
            RetentionDirectiveParse::Unresolved(RetentionDirectiveRole::Question)
        ));
        let quoted = "第一个是准确性。第二个是稳定性。第三个是UI设计，‘啊算了算了，就保留前面两个，第一个和第二个’。";
        assert!(matches!(
            parse_list_retention_directive(quoted),
            RetentionDirectiveParse::Unresolved(RetentionDirectiveRole::Quoted)
        ));
    }

    #[test]
    fn short_retention_cue_requires_a_direct_complete_same_block_instruction() {
        let direct = "第一个是准确性。第二个是稳定性。第三个是UI设计，啊算了，只保留前面两个，第一个和第二个。";
        let applied = apply_layout_stages(direct, ContextFamily::General, 1.0, true);
        assert_eq!(applied.text, "1. 准确性。\n2. 稳定性。");
        assert!(applied.source_preserved);
        assert!(applied
            .list_revision_proof
            .as_ref()
            .is_some_and(ListLayoutProof::is_valid));

        let with_prefix = "先处理前置说明。\n\n第一个是准确性。第二个是稳定性。第三个是UI设计，啊算了，只保留前面两个，第一个和第二个。";
        let offset_applied = apply_layout_stages(with_prefix, ContextFamily::General, 1.0, true);
        assert_eq!(
            offset_applied.text,
            "先处理前置说明。\n\n1. 准确性。\n2. 稳定性。"
        );
        assert!(offset_applied.source_preserved);
        assert!(offset_applied
            .list_revision_proof
            .as_ref()
            .is_some_and(ListLayoutProof::is_valid));

        // A comma inside an item cannot establish that an unsupported
        // hypothetical introducer is ordinary payload rather than a directive.
        let payload_if = direct.replace("第三个是UI设计，啊算了", "第三个是UI设计，如果分支啊算了");
        let payload_if_applied =
            apply_layout_stages(&payload_if, ContextFamily::General, 1.0, true);
        assert_eq!(payload_if_applied.text, payload_if);
        assert!(payload_if_applied.source_preserved);
        assert!(payload_if_applied.list_revision_proof.is_none());
        assert_eq!(
            apply_layout_stages(&payload_if_applied.text, ContextFamily::General, 1.0, true).text,
            payload_if
        );

        for unresolved in [
            "第一个是准确性。第二个是稳定性。第三个是UI设计。假设我说啊算了算了，就保留前面两个，第一个和第二个。",
            "第一个是准确性。第二个是稳定性。第三个是UI设计。假设啊算了，只保留前面两个，第一个和第二个。",
            "第一个是准确性。第二个是稳定性。第三个是UI设计。比如啊算了，只保留前面两个，第一个和第二个。",
            "第一个是准确性。第二个是稳定性。第三个是UI设计。假设她说啊算了，只保留前面两个，第一个和第二个。",
            "第一个是准确性。第二个是稳定性。第三个是UI设计。比如小明说啊算了，只保留前面两个，第一个和第二个。",
            "第一个是准确性。第二个是稳定性。第三个是UI设计。假设如果分支，啊算了，只保留前面两个，第一个和第二个。",
            "第一个是准确性。第二个是稳定性。第三个是UI设计。比如我说错了啊算了，只保留前面两个，第一个和第二个。",
            "第一个是准确性。第二个是稳定性。第三个是UI设计，啊算了算了，就保留前面两个，第一个和第二个？",
            "第一个是准确性。第二个是稳定性。第三个是UI设计，啊算了算了，就保留前面两个，第一个和第二个？！",
            "第一个是准确性。第二个是稳定性。第三个是UI设计，啊算了算了，就保留前面两个，第一个和第二个?!",
            "第一个是准确性。第二个是稳定性。第三个是UI设计，‘啊算了算了，就保留前面两个，第一个和第二个’。",
        ] {
            let rejected = apply_layout_stages(unresolved, ContextFamily::General, 1.0, true);
            assert_eq!(rejected.text, unresolved);
            assert!(rejected.source_preserved);
            assert!(rejected.list_revision_proof.is_none());
            assert_eq!(
                apply_layout_stages(&rejected.text, ContextFamily::General, 1.0, true).text,
                unresolved
            );
        }
        for unsupported_intro in [
            "第一个是准确性。第二个是稳定性。第三个是UI设计。假设她说啊算了，只保留前面两个，第一个和第二个。",
            "第一个是准确性。第二个是稳定性。第三个是UI设计。比如小明说啊算了，只保留前面两个，第一个和第二个。",
            "第一个是准确性。第二个是稳定性。第三个是UI设计。假设如果分支，啊算了，只保留前面两个，第一个和第二个。",
            "第一个是准确性。第二个是稳定性。第三个是UI设计，如果她说啊算了，只保留前面两个，第一个和第二个。",
        ] {
            assert!(matches!(
                parse_list_retention_directive(unsupported_intro),
                RetentionDirectiveParse::Unresolved(RetentionDirectiveRole::Unknown)
            ));
        }
    }

    #[test]
    fn retention_supports_only_leave_and_source_bound_arbitrary_ordinal_deletion() {
        let only_leave = "第一个是准确性。第二个是稳定性。第三个是UI设计。啊算了，只留前两个。";
        let kept = apply_after_punctuation_with_provenance(only_leave, ContextFamily::General, 1.0);
        assert_eq!(kept.text, "1. 准确性。\n2. 稳定性。");
        assert!(kept.source_preserved);
        assert!(kept
            .list_revision_proof
            .as_ref()
            .is_some_and(ListLayoutProof::is_valid));

        for (directive, expected) in [
            (
                "不要第一个",
                "1. 第二项 value 2\n2. C++ value 3\n3. 第四项。",
            ),
            ("不要第二个", "1. 第一项\n2. C++ value 3\n3. 第四项。"),
            ("删掉第四个", "1. 第一项\n2. 第二项 value 2\n3. C++ value 3"),
        ] {
            let source = format!(
                "第一个是第一项，第二个是第二项 value 2，第三个是C++ value 3，第四个是第四项。啊算了，{directive}。"
            );
            let parsed = parse_list_retention_directive(&source);
            let RetentionDirectiveParse::Parsed(parsed) = parsed else {
                panic!("{directive} must parse as a retention directive");
            };
            assert_eq!(parsed.form, ListRetentionForm::RemoveOrdinal, "{directive}");
            let expected_count = match directive {
                "不要第一个" => "第一个",
                "不要第二个" => "第二个",
                "删掉第四个" => "第四个",
                _ => unreachable!(),
            };
            assert_eq!(source.get(parsed.count_range.clone()), Some(expected_count));
            assert_eq!(
                parsed.keep_count,
                match directive {
                    "不要第一个" => 1,
                    "不要第二个" => 2,
                    "删掉第四个" => 4,
                    _ => unreachable!(),
                }
            );
            assert!(parsed.references.is_empty(), "{directive}");
            let applied =
                apply_after_punctuation_with_provenance(&source, ContextFamily::General, 1.0);
            assert_eq!(applied.text, expected, "{directive}");
            assert!(applied.source_preserved, "{directive}");
            let proof = applied
                .list_revision_proof
                .as_ref()
                .expect("source-bound ordinal deletion proof");
            assert!(proof.is_valid(), "{directive}");
            assert!(
                !applied.text.contains('，'),
                "rendered separators were removed: {directive}"
            );
            let retention = proof.retention.as_ref().expect("retention proof");
            assert_eq!(retention.form, ListRetentionForm::RemoveOrdinal);
            assert_eq!(retention.removed_items.len(), 1);
            assert_eq!(
                retention.removed_items[0].ordinal,
                retention.directive_ordinal.unwrap()
            );
            if directive == "不要第二个" {
                let removed = &retention.removed_items[0];
                assert_eq!(removed.source_value, "是第二项 value 2，");
                assert_eq!(
                    proof.source.get(removed.source_value_range.clone()),
                    Some("是第二项 value 2，")
                );
            }
            for item in &retention.retained_items {
                assert!(proof.source.get(item.marker_range.clone()).is_some());
                assert_eq!(
                    proof.source.get(item.source_value_range.clone()),
                    Some(item.source_value.as_str())
                );
                assert_eq!(
                    super::parse_numbered_marker_ordinal(&proof.source, &item.marker_range),
                    Some(item.ordinal)
                );
            }
            assert!(retention
                .retained_items
                .iter()
                .any(|item| item.source_value.ends_with('，')));
            for item in &retention.removed_items {
                assert!(proof.source.get(item.marker_range.clone()).is_some());
                assert_eq!(
                    proof.source.get(item.source_value_range.clone()),
                    Some(item.source_value.as_str())
                );
                assert_eq!(
                    super::parse_numbered_marker_ordinal(&proof.source, &item.marker_range),
                    Some(item.ordinal)
                );
            }
            assert!(retention
                .retained_items
                .iter()
                .enumerate()
                .all(|(index, item)| item.rendered_ordinal == index + 1));

            let mut forged = proof.clone();
            let retention = forged.retention.as_mut().unwrap();
            retention.directive_ordinal = Some(if retention.directive_ordinal == Some(1) {
                2
            } else {
                1
            });
            assert!(!forged.is_valid(), "mismatched delete target must fail");
        }

        for source in [
            "第一个是甲。第二个是乙。第三个是丙。啊算了，不要第四个。",
            "第一个是甲。第二个是乙。第三个是丙。假设啊算了，不要第二个。",
            "第一个是甲。第二个是乙。第三个是丙。‘啊算了，不要第二个。’",
        ] {
            let applied =
                apply_after_punctuation_with_provenance(source, ContextFamily::General, 1.0);
            assert_eq!(applied.text, source);
            assert!(applied.list_revision_proof.is_none());
        }
    }

    #[test]
    fn count_only_retention_proves_corrected_items_and_limits_final_guard_authority() {
        let source = "第一个是稳定性，啊不对，第一个是准确性，第二个才是稳定性。第三个看 UI，算了，只保留前两个。";
        let applied = apply_after_punctuation_with_provenance(source, ContextFamily::General, 0.9);
        assert_eq!(applied.text, "1. 准确性\n2. 才是稳定性。");
        assert!(applied.source_preserved);

        let proof = applied
            .list_revision_proof
            .as_ref()
            .expect("count-only retention proof");
        assert!(proof.is_valid());
        assert_eq!(proof.revisions.len(), 1);
        let retention = proof.retention.as_ref().expect("retention facts");
        assert_eq!(retention.form, ListRetentionForm::CountOnly);
        assert_eq!(retention.retained_count, 2);
        assert_eq!(retention.count_value, "前两个");
        let linker = retention
            .ordinal_linker
            .as_ref()
            .expect("source-backed adjacent ordinal linker");
        assert_eq!(linker.ordinal, 2);
        assert_eq!(proof.source.get(linker.source_range.clone()), Some("才是"));
        assert_eq!(
            proof.layout_output.get(linker.output_range.clone()),
            Some("才是")
        );
        assert_eq!(
            retention
                .removed_items
                .iter()
                .map(|item| item.ordinal)
                .collect::<Vec<_>>(),
            vec![3]
        );
        assert!(retention.removed_items[0].source_value.contains("UI"));

        let revision = crate::spoken_revision::apply_with_authorizations(&applied.text);
        let cleanup_candidate = "1. 准确性。\n2. 稳定性。";
        let context = crate::FinalizationContext {
            scene: crate::ResolvedCleanupScene::observed(
                ContextFamily::General,
                crate::context::FocusKind::Editable,
            ),
            operation: crate::llm::CleanupOperation::Cleanup,
            revision_source: Some(&applied.text),
            prepared_transcript: Some(&revision.text),
            revision_authorizations: &revision.authorizations,
            promoted_pair_protections: &[],
            list_revision_proof: Some(proof),
            structure_safe_source: None,
            output_projection_list_revision_proof: None,
            layout_source: None,
            exact_fallback: false,
        };
        let waivers = super::clause_leading_hesitation_ranges(&revision.text);
        assert!(crate::protected_span::preserves_with_source_term_waivers(
            &revision.text,
            cleanup_candidate,
            Some(crate::llm::CleanupOperation::Cleanup),
            &waivers
        ));

        // A surface-retaining candidate remains accepted by the complete
        // final guard independently of the narrower linker omission proof.
        let layout_candidate = "1. 准确性。\n2. 才是稳定性。";
        let (accepted, reason, _) =
            crate::guard_final_output_with_reason(&revision.text, layout_candidate, context);
        assert_eq!(accepted, layout_candidate);
        assert!(reason.is_none());

        let mut canonical_proof = proof.clone();
        let canonicalized = canonical_proof
            .canonicalize_ordinal_linker()
            .expect("proved linker canonicalization");
        assert_eq!(canonicalized, "1. 准确性\n2. 稳定性。");
        assert!(canonical_proof.is_valid());
        assert!(super::preserves_required_layout(
            &canonicalized,
            cleanup_candidate
        ));
        assert!(!super::preserves_required_layout(
            &canonicalized,
            "1. 准确性。\n2. 稳定。"
        ));

        let (rejected, reason, _) =
            crate::guard_final_output_with_reason(&revision.text, "1. 准确性。", context);
        assert_eq!(rejected, revision.text);
        assert!(reason.is_some(), "the guard must retain the second item");

        let mut forged = proof.clone();
        let facts = forged.retention.as_mut().expect("retention facts");
        facts.removed_items[0].source_value_range =
            facts.retained_items[0].source_value_range.clone();
        assert!(
            !forged.is_valid(),
            "a different source span cannot be authorized"
        );

        let mut forged_linker = proof.clone();
        let linker = forged_linker
            .retention
            .as_mut()
            .and_then(|retention| retention.ordinal_linker.as_mut())
            .expect("linker proof");
        linker.source_range.start += "才是".len();
        linker.source_range.end += "才是".len();
        assert!(
            !forged_linker.is_valid(),
            "a different same-list span cannot be authorized as the linker"
        );
    }

    #[test]
    fn count_only_retention_supports_bounded_chinese_and_numeric_forms() {
        let cases = [
            (
                "第一项是安全性。第二项是准确性。第三项是 UI 设计。啊算了，只保留前两项。",
                "1. 安全性。\n2. 准确性。",
                2,
                vec![3],
                "前两项",
            ),
            (
                "第一个是安全性。第二个是准确性。第三个是 UI。第四个是延迟。算了，保留前2个。",
                "1. 安全性。\n2. 准确性。",
                2,
                vec![3, 4],
                "前2个",
            ),
            (
                "第一个是安全性。第二个是准确性。第三个是隐私。第四个是部署。第五个是支持。啊算了，只保留前三项。",
                "1. 安全性。\n2. 准确性。\n3. 隐私。",
                3,
                vec![4, 5],
                "前三项",
            ),
            (
                "第一个是预算1250美元。第二个是不要删除 API v2。第三个是 UI。算了，只保留前兩個。",
                "1. 预算1250美元。\n2. 是不要删除 API v2。",
                2,
                vec![3],
                "前兩個",
            ),
        ];

        for (source, expected, keep_count, removed_ordinals, count_value) in cases {
            let applied = apply_layout_stages(source, ContextFamily::General, 1.0, true);
            assert_eq!(applied.text, expected, "count form failed: {source}");
            assert!(applied.source_preserved, "source proof failed: {source}");
            let proof = applied
                .list_revision_proof
                .as_ref()
                .expect("bounded count-only retention proof");
            assert!(proof.is_valid(), "invalid count-only proof: {source}");
            let retention = proof.retention.as_ref().expect("retention facts");
            assert_eq!(retention.form, ListRetentionForm::CountOnly);
            assert_eq!(retention.retained_count, keep_count);
            assert_eq!(retention.count_value, count_value);
            assert_eq!(
                retention
                    .removed_items
                    .iter()
                    .map(|item| item.ordinal)
                    .collect::<Vec<_>>(),
                removed_ordinals
            );
        }

        let spoken = "第一个是安全性。第二个是准确性。第三个是 UI 设计，算了吧，只要前两个。";
        let applied = apply_after_punctuation_with_provenance(spoken, ContextFamily::General, 1.0);
        assert_eq!(applied.text, "1. 安全性。\n2. 准确性。");
        assert!(applied.source_preserved);
        let proof = applied
            .list_revision_proof
            .as_ref()
            .expect("source-backed count-only retention proof");
        assert!(proof.is_valid());
        let retention = proof.retention.as_ref().expect("retention facts");
        assert_eq!(retention.form, ListRetentionForm::CountOnly);
        assert_eq!(retention.retained_count, 2);
        assert_eq!(retention.count_value, "前两个");
        assert_eq!(
            proof.source.get(retention.directive_range.clone()),
            Some("算了吧，只要前两个")
        );
    }

    #[test]
    fn count_only_retention_preserves_quoted_hypothetical_mismatched_and_entity_cases() {
        let unresolved = [
            "第一个是准确性。第二个是稳定性。第三个是 UI，‘算了，只保留前两个’。",
            "第一个是准确性。第二个是稳定性。第三个是 UI，‘算了吧，只要前两个’。",
            "第一个是准确性。第二个是稳定性。第三个是 UI。假设我说算了，只保留前两个。",
            "第一个是准确性。第二个是稳定性。第三个是 UI。假设我说算了吧，只要前两个。",
            "第一个是准确性。第二个是稳定性。第三个是 UI，算了，只保留前两个？",
            "第一个是准确性。第三个是稳定性。第四个是 UI，算了，只保留前两个。",
            "第一个是金额1250美元。第二个是版本API v2。第三个是前两个月的费用，算了，只保留前两个月。",
            "第一个是金额1250美元。第二个是版本API v2。第三个是 UI，算了，只保留前两个版本。",
            "第一个是准确性。第二个是稳定性。第三个是 UI，算了，只保留前11个。",
        ];
        for source in unresolved {
            let applied = apply_layout_stages(source, ContextFamily::General, 1.0, true);
            assert_eq!(
                applied.text, source,
                "unresolved count directive changed: {source}"
            );
            assert!(applied.source_preserved);
            assert!(applied.list_revision_proof.is_none());
        }

        let code = "第一个是准确性。第二个是稳定性。第三个是 `算了，只保留前两个`。";
        let code_applied = apply_layout_stages(code, ContextFamily::General, 1.0, true);
        assert_eq!(code_applied.text, code);
        assert!(code_applied.list_revision_proof.is_none());

        let negated = "第一个是准确性。第二个是稳定性。第三个是 UI，算了，不要只保留前两个。";
        let negated_applied = apply_layout_stages(negated, ContextFamily::General, 1.0, true);
        assert!(negated_applied.text.contains("3. UI"));
        assert!(negated_applied.list_revision_proof.is_none());

        let cross_block = "第一个是准确性。第二个是稳定性。第三个是 UI。\n\n算了，只保留前两个。";
        let cross_block_applied =
            apply_layout_stages(cross_block, ContextFamily::General, 1.0, true);
        assert!(cross_block_applied.text.contains("3. UI"));
        assert!(cross_block_applied.list_revision_proof.is_none());
    }

    #[test]
    fn terminal_deictic_two_retracts_only_an_empty_pending_third_marker() {
        let source = "1. 稳定性。 2. 准确度。 3. ，算了就这两个。";
        let applied = apply_layout_stages(source, ContextFamily::General, 1.0, true);
        assert_eq!(applied.text, "1. 稳定性。\n2. 准确度。");
        assert!(applied.source_preserved);
        let proof = applied
            .list_revision_proof
            .as_ref()
            .expect("source-bound terminal deictic retention proof");
        assert!(proof.is_valid());
        let retention = proof.retention.as_ref().expect("retention facts");
        assert_eq!(retention.form, ListRetentionForm::TerminalDeicticCount);
        assert!(retention.references.is_empty());
        assert_eq!(retention.count_value, "这两个");
        assert_eq!(retention.removed_items[0].ordinal, 3);

        for protected in [
            source.replace("3. ，算了", "3. 还有设计项，算了"),
            source.replace("算了就这两个", "‘算了就这两个’"),
            source.replace("算了就这两个", "假设算了就这两个"),
            source.replace("这两个。", "这两个？"),
            source.replace("这两个", "这三个"),
        ] {
            let retained = apply_layout_stages(&protected, ContextFamily::General, 1.0, true);
            assert_eq!(retained.text, protected);
            assert!(retained.source_preserved);
            assert!(retained.list_revision_proof.is_none());
        }
    }

    #[test]
    fn terminal_deictic_two_validates_prefixed_spoken_and_numeric_markers() {
        let prefix = "会前目标已经记录。\n\n";
        let spoken = format!("{prefix}第一项是稳定性。第二项是准确度。第三个，算了就这两个。");
        for source in [
            spoken.clone(),
            format!("{prefix}1. 稳定性。 2. 准确度。 3. ，算了就这两个。"),
        ] {
            let applied =
                apply_after_punctuation_with_provenance(&source, ContextFamily::General, 1.0);
            assert!(applied.source_preserved);
            assert!(applied.text.starts_with("会前目标已经记录。\n\n"));
            assert!(applied.text.contains("1. 稳定性"));
            assert!(applied.text.contains("2. 准确度"));
            assert!(!applied.text.contains("第三个"));
            let proof = applied
                .list_revision_proof
                .as_ref()
                .expect("prefixed full-source retention proof");
            assert!(proof.is_valid());
            let retention = proof.retention.as_ref().expect("retention facts");
            assert_eq!(retention.form, ListRetentionForm::TerminalDeicticCount);
            assert!(retention.references.is_empty());
            assert_eq!(retention.removed_items[0].ordinal, 3);
        }

        for protected in [
            spoken.replace("第三个，算了", "第三个，还有设计项，算了"),
            spoken.replace("算了就这两个", "‘算了就这两个’"),
            spoken.replace("算了就这两个", "假设算了就这两个"),
            spoken.replace("这两个。", "这两个？"),
            spoken.replace("这两个", "这三个"),
        ] {
            let retained = apply_layout_stages(&protected, ContextFamily::General, 1.0, true);
            assert_eq!(retained.text, protected);
            assert!(retained.source_preserved);
            assert!(retained.list_revision_proof.is_none());
        }
    }

    #[test]
    fn empty_leading_ordinal_restart_formats_the_restarted_list_in_prompt_or_code() {
        let source = "呃，详细的情况帮我看一下。然后这个具体的UI为什么变了？第二个是啊，第一个是啊，看一下稳定性怎么样。第二个是看一下它的这个准确率怎么样。第三个是啊，算了就这两个。";
        let expected = "呃，详细的情况帮我看一下。然后这个具体的UI为什么变了？\n\n1. 看一下稳定性怎么样。\n2. 看一下它的这个准确率怎么样。";

        for family in [ContextFamily::General, ContextFamily::PromptOrCode] {
            let applied = apply_after_punctuation_with_provenance(source, family, 1.0);
            assert_eq!(applied.text, expected);
            assert!(applied.source_preserved);
            let proof = applied
                .list_revision_proof
                .as_ref()
                .expect("source-bound abandoned ordinal and retention proof");
            assert!(proof.is_valid());
            let restart = proof
                .retention
                .as_ref()
                .and_then(|retention| retention.leading_restart.as_ref())
                .expect("leading empty ordinal restart fact");
            assert_eq!(restart.marker_value, "第二个");
            assert_eq!(restart.filler_value, "是啊，");
            assert_eq!(restart.restarted_marker_value, "第一个");
        }

        // The same proof is based on the empty placeholder and continuous
        // restarted ordinals, not the particular abandoned ordinal in row 12.
        let restarted_from_three =
            "前言。第三个是呃，第一个是啊，稳定性。第二个是准确率。第三个是啊，算了就这两个。";
        let applied =
            apply_layout_stages(restarted_from_three, ContextFamily::PromptOrCode, 1.0, true);
        assert_eq!(applied.text, "前言。\n\n1. 稳定性。\n2. 准确率。");
        assert!(applied.source_preserved);
        assert!(applied
            .list_revision_proof
            .as_ref()
            .is_some_and(ListLayoutProof::is_valid));
    }

    #[test]
    fn empty_ordinal_restart_requires_unquoted_empty_source_and_a_direct_keep_cue() {
        for source in [
            "这是第一个客户。",
            "第二个是这次日期2026-10-18、金额3.14美元、版本v1.2.3、路径/2/3，第一个是稳定性。第二个是准确率。第三个是设计。算了就这两个。",
            "假设我说，第二个是啊，第一个是稳定性。第二个是准确率。第三个是设计。算了就这两个。",
            "‘第二个是啊，第一个是稳定性。第二个是准确率。第三个是啊，算了就这两个。’",
            "`第二个是啊，第一个是稳定性。第二个是准确率。第三个是啊，算了就这两个。`",
        ] {
            let applied = apply_layout_stages(source, ContextFamily::PromptOrCode, 1.0, true);
            assert_eq!(applied.text, source, "unproved restart changed: {source}");
            assert!(applied.source_preserved);
            assert!(applied.list_revision_proof.is_none());
        }
    }

    #[test]
    fn adjacent_completed_ordinal_item_can_be_replaced_with_a_typed_proof() {
        let source = "第一个是保留 API v2 的 3 份备份，不要删除。哦不对，第一个是保留 API v2 的 4 份备份，不要删除。第二个是联系 Alice 确认。";
        let applied = apply_layout_stages(source, ContextFamily::General, 1.0, true);

        assert_eq!(
            applied.text,
            "1. 保留 API v2 的 4 份备份，不要删除。\n2. 联系 Alice 确认。"
        );
        assert!(applied.source_preserved);
        let proof = applied
            .list_revision_proof
            .as_ref()
            .expect("adjacent completed item revision proof");
        assert!(proof.is_valid());
        assert_eq!(proof.revisions.len(), 1);
        assert_eq!(proof.revisions[0].ordinal, 1);
        assert!(proof.revisions[0].placeholder_marker_range.is_none());
        assert_eq!(
            proof.revisions[0].source_value,
            "是保留 API v2 的 3 份备份，不要删除。"
        );
        assert_eq!(
            proof.revisions[0].replacement_value,
            "保留 API v2 的 4 份备份，不要删除。"
        );
        assert_eq!(
            apply_layout_stages(&applied.text, ContextFamily::General, 1.0, true).text,
            applied.text,
            "prepared list revision must be idempotent"
        );
    }

    #[test]
    fn proved_list_cleanup_removes_only_linker_fillers_and_clear_pronoun_restart() {
        let source = "第一个是呃，确认这个流程。第二个是我我觉得可能可以开始。";
        let applied = apply_layout_stages(source, ContextFamily::General, 1.0, true);

        assert_eq!(applied.text, "1. 确认这个流程。\n2. 我觉得可能可以开始。");
        assert!(applied.source_preserved);

        for (emphatic, expected) in [
            (
                "第一个是非常非常重要。第二个是只有第二个才是正确答案。",
                "1. 非常非常重要。\n2. 只有第二个才是正确答案。",
            ),
            (
                "第一个是非常非常重要。第二个是只有第一个才是正确答案。",
                "1. 非常非常重要。\n2. 只有第一个才是正确答案。",
            ),
            (
                "第一个是非常非常重要。第二个是only第二个才是正确答案。",
                "1. 非常非常重要。\n2. only第二个才是正确答案。",
            ),
        ] {
            let applied = apply_layout_stages(emphatic, ContextFamily::General, 1.0, true);
            assert_eq!(applied.text, expected);
            assert!(applied.source_preserved);
            assert!(applied.list_revision_proof.is_none());
            let reprepared = apply_layout_stages(&applied.text, ContextFamily::General, 1.0, true);
            assert_eq!(reprepared.text, expected);
            assert!(reprepared.source_preserved);
        }

        let already_numbered = "1. 非常非常重要。\n2. 只有第二个才是正确答案。";
        let stable = apply_layout_stages(already_numbered, ContextFamily::General, 1.0, true);
        assert_eq!(stable.text, already_numbered);
        assert!(stable.source_preserved);
        assert!(stable.list_revision_proof.is_none());
        assert_eq!(
            apply_layout_stages(&stable.text, ContextFamily::General, 1.0, true).text,
            already_numbered
        );
    }

    #[test]
    fn numbered_layout_allows_only_clear_source_backed_disfluency_removal() {
        let source = "1. Um, send the report.\n2. 你你呢？\n3. 只有第二个才是正确答案。";
        let cleaned = "1. Send the report.\n2. 你呢？\n3. 只有第二个才是正确答案。";
        assert!(super::preserves_required_layout(source, cleaned));

        for (before, candidate) in [
            ("1. Send the report.", "1. Send report."),
            ("1. Umami is a food.", "1. Ami is a food."),
            ("1. `um` is a value.", "1. `` is a value."),
            ("1. 可能会延迟。", "1. 会延迟。"),
            ("1. 非常非常重要。", "1. 非常重要。"),
        ] {
            assert!(
                !super::preserves_required_layout(before, candidate),
                "unproved list deletion accepted: {before} -> {candidate}"
            );
        }
    }

    #[test]
    fn named_head_correction_requires_the_same_exact_target_head() {
        let source = "第一个预算是25美元。第二个是，哦不对，第一个预算是30美元。第三个是备注。";
        let applied = apply_layout_stages(source, ContextFamily::General, 1.0, true);
        assert_eq!(applied.text, "1. 预算是30美元。\n2. 备注。");
        let proof = applied
            .list_revision_proof
            .as_ref()
            .expect("unique matching named-head revision");
        assert!(proof.is_valid());
        assert_eq!(proof.revisions[0].replacement_prefix, "预算是");

        let mismatched = "第一个预算是25美元。第二个是，哦不对，第一个限额是30美元。第三个是备注。";
        let rejected = apply_layout_stages(mismatched, ContextFamily::General, 1.0, true);
        assert_eq!(rejected.text, mismatched);
        assert!(rejected.list_revision_proof.is_none());
    }

    #[test]
    fn extra_break_inside_a_quoted_nonfinal_piece_is_collapsed() {
        assert_eq!(
            super::restore_if_flattened(
                "“原样引用文字”\n第二行正文",
                "“原样引用\n文字”\n第二行正文"
            ),
            "“原样引用 文字”\n第二行正文"
        );
    }

    #[test]
    fn ordinal_revision_replaces_only_the_target_before_an_intervening_item() {
        let source = "第一个是旧内容。第二个是保留的中间项。第三个是，哦不对，第一个是新内容。第四个是最后一项。";
        let applied = apply_layout_stages(source, ContextFamily::General, 1.0, true);

        assert_eq!(
            applied.text,
            "1. 新内容。\n2. 保留的中间项。\n3. 最后一项。"
        );
        let proof = applied
            .list_revision_proof
            .as_ref()
            .expect("authorized non-adjacent list revision proof");
        assert!(proof.is_valid());
        assert_eq!(proof.revisions[0].source_value, "是旧内容。");
        assert_eq!(proof.revisions[0].replacement_value, "新内容。");
    }

    #[test]
    fn inline_cue_variants_resolve_same_slot_revision() {
        for cue in [
            "啊不对",
            "哦，不对",
            "哦,不对",
            "我刚才说错了",
            "我刚才说的",
            "我刚说的",
        ] {
            let source = format!("第一个是旧内容。{cue}，第一个是新内容。第二个是保留项。");
            let applied =
                apply_after_punctuation_with_provenance(&source, ContextFamily::General, 1.0);
            assert_eq!(applied.text, "1. 新内容。\n2. 保留项。");
            let proof = applied
                .list_revision_proof
                .as_ref()
                .expect("inline revision proof from source transcript");
            assert!(proof.is_valid());
            assert_eq!(proof.revisions.len(), 1);
            let revision = &proof.revisions[0];
            assert_eq!(proof.source.get(revision.cue_range.clone()), Some(cue));
            assert_eq!(revision.cue_value, cue);
            assert_eq!(
                proof.source.get(revision.source_item_range.clone()),
                Some("是旧内容。")
            );
            assert_eq!(
                proof.source.get(revision.replacement_source_range.clone()),
                Some("新内容。")
            );
        }

        let word_internal = "第一个是bonsai mean。第一个是新内容。第二个是保留项。";
        let rejected =
            apply_after_punctuation_with_provenance(word_internal, ContextFamily::General, 1.0);
        assert_eq!(rejected.text, word_internal);
        assert!(rejected.list_revision_proof.is_none());

        for protected in [
            "第一个是旧内容。‘哦，不对，第一个是新内容。’第二个是保留项。",
            "假设我说：第一个是旧内容。哦，不对，第一个是新内容。第二个是保留项。",
        ] {
            let applied =
                apply_after_punctuation_with_provenance(protected, ContextFamily::General, 1.0);
            assert!(applied.text.contains("旧内容"));
            assert!(applied.text.contains("哦，不对"));
            assert!(applied.text.contains("新内容"));
            assert!(applied.text.contains("保留项"));
            assert!(applied.list_revision_proof.is_none());
        }
    }

    #[test]
    fn reverse_target_chinese_revisions_keep_valid_output_ranges_after_masking() {
        let source = "第一个是甲。第二个是乙。第三个是丙。第四个是，哦不对，第二个是乙新。第五个是，哦不对，第一个是甲新。第六个是丁。";
        let applied = apply_layout_stages(source, ContextFamily::General, 1.0, true);

        assert_eq!(applied.text, "1. 甲新。\n2. 乙新。\n3. 丙。\n4. 丁。");
        assert!(applied.source_preserved);
        let proof = applied
            .list_revision_proof
            .as_ref()
            .expect("two authorized reverse-target list revisions");
        assert!(proof.is_valid());
        assert_eq!(
            proof
                .revisions
                .iter()
                .map(|revision| revision.ordinal)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
    }

    #[test]
    fn ordinal_revision_validator_rejects_tampered_and_overlapping_output_ranges() {
        let source = "第一个是旧甲。第二个是旧乙。第三个是旧丙。第四个是，哦不对，第二个是新项。第五个是，哦不对，第一个是新项。第六个是末项。";
        let applied = apply_layout_stages(source, ContextFamily::General, 1.0, true);
        let proof = applied
            .list_revision_proof
            .as_ref()
            .expect("two authorized list revisions");
        assert!(proof.is_valid());

        let mut tampered = proof.revisions.clone();
        tampered[0].output_value_range = 0..tampered[0].replacement_value.len();
        assert!(!super::list_item_revisions_match_source_and_output(
            &proof.source,
            &proof.layout_output,
            &tampered
        ));

        let mut overlapping = proof.revisions.clone();
        overlapping[1].output_value_range = overlapping[0].output_value_range.clone();
        overlapping[1].rendered_ordinal = overlapping[0].rendered_ordinal;
        assert!(!super::list_item_revisions_match_source_and_output(
            &proof.source,
            &proof.layout_output,
            &overlapping
        ));
    }

    #[test]
    fn direct_self_correction_is_not_disabled_by_an_earlier_example_clause() {
        let source =
            "比如先看一下流程。第一个是旧内容。第二个是，哦不对，第一个是新内容。第三个是稳定性。";
        let applied = apply_layout_stages(source, ContextFamily::General, 1.0, true);

        assert_eq!(
            applied.text,
            "比如先看一下流程。\n\n1. 新内容。\n2. 稳定性。"
        );
        assert!(applied
            .list_revision_proof
            .as_ref()
            .is_some_and(ListLayoutProof::is_valid));
    }

    #[test]
    fn list_cleanup_preserves_question_copulas_and_meaningful_repeated_fillers() {
        for (source, expected) in [
            ("第一个是否正常？第二个是备注。", "1. 是否正常？\n2. 备注。"),
            (
                "第一个是嗯嗯表示同意。第二个是备注。",
                "1. 嗯嗯表示同意。\n2. 备注。",
            ),
            (
                "第一个是嗯哼表示同意。第二个是啊哈这样。第三个是备注。",
                "1. 嗯哼表示同意。\n2. 啊哈这样。\n3. 备注。",
            ),
            (
                "第一个是umbrella。第二个是umami。",
                "1. umbrella。\n2. umami。",
            ),
            ("第一个是呃，确认。第二个是备注。", "1. 确认。\n2. 备注。"),
            (
                "第一个是不是正常？第二个是什么？第三个是的是肯定答复。",
                "1. 是不是正常？\n2. 是什么？\n3. 是的是肯定答复。",
            ),
        ] {
            let applied = apply_layout_stages(source, ContextFamily::General, 1.0, true);
            assert_eq!(applied.text, expected);
            assert!(applied.source_preserved);
        }
    }

    #[test]
    fn unresolved_ordinal_revision_keeps_its_cue_and_does_not_delete_items() {
        for source in [
            "第二项是，哦不对，第一个是新内容。",
            "第一个是旧内容。第二项是原内容，哦不对，第一个是新内容。",
            "第二项是原内容。第三项是，哦不对，第一个是新内容。第四项是其余内容。",
            "第一个是旧内容。第一个是另一个目标。第二个是，哦不对，第一个是新内容。第三个是其余内容。",
            "假设我说：第一个是旧内容。第二个是，哦不对，第一个是新内容。第三个是其余内容。",
            "假设我说：第一个是旧内容。第二个是旧占位，哦不对，第三个是新内容。",
            "第一个是旧内容。第二个是，我刚才说的，第一个是新内容。第三个是余下。",
            "第一个是旧内容。第二个是，我刚说的，第一个是新内容。第三个是余下。",
            "1. 是 prompt，不对 2. 是 system",
        ] {
            let applied = apply_layout_stages(source, ContextFamily::General, 1.0, true);
            assert_eq!(applied.text, source, "source changed: {source} -> {}", applied.text);
            assert!(applied.list_revision_proof.is_none());
            assert_eq!(
                apply(&applied.text, ContextFamily::General, 1.0),
                source,
                "unresolved list cue was not idempotently preserved: {source}"
            );
        }
    }

    #[test]
    fn earlier_duplicate_target_prevents_legacy_forward_backtrack() {
        let source = "第二项是乙。第一项是甲，哦不对，第二项是新乙。第三项是丙。";
        let applied = apply_layout_stages(source, ContextFamily::General, 1.0, true);

        assert_eq!(applied.text, source);
        assert!(applied.list_revision_proof.is_none());
        assert_eq!(apply(&applied.text, ContextFamily::General, 1.0), source);
    }

    #[test]
    fn bare_bu_dui_requires_a_clause_boundary_before_becoming_a_revision_cue() {
        let source = "1. 看它对不对 2. 再看 style";
        let applied = apply_layout_stages(source, ContextFamily::General, 1.0, true);

        assert_eq!(applied.text, "1. 看它对不对\n2. 再看 style");
        assert!(applied.list_revision_proof.is_none());
        assert_eq!(
            apply(&applied.text, ContextFamily::General, 1.0),
            applied.text
        );
    }

    #[test]
    fn example_wording_without_an_authorized_correction_never_proves_a_revision() {
        let source = "比如我说不对，第一个是系统应该怎样改。第二个是其余内容。";
        let applied = apply_layout_stages(source, ContextFamily::General, 1.0, true);
        assert!(applied.list_revision_proof.is_none());
        assert!(applied.text.contains("系统应该怎样改"));
        assert!(applied.text.contains("其余内容"));
    }

    #[test]
    fn quoted_revision_language_does_not_block_outer_list_formatting() {
        let source = "第一个是旧内容。第二个是“哦不对，第一个是新内容”。第三个是其余内容。";
        let applied = apply_layout_stages(source, ContextFamily::General, 1.0, true);
        assert!(applied.text.contains("“哦不对，第一个是新内容”"));
        assert!(applied.list_revision_proof.is_none());
        assert_eq!(
            apply(&applied.text, ContextFamily::General, 1.0),
            applied.text
        );
    }

    #[test]
    fn first_time_and_first_of_are_not_lists() {
        assert_eq!(
            layout("the first time I tried and the second time I failed"),
            "the first time I tried and the second time I failed"
        );
        assert_eq!(
            layout("first of all we should wait"),
            "first of all we should wait"
        );
    }

    #[test]
    fn comparative_first_and_second_options_stay_prose() {
        assert_eq!(
            layout("The second option feels clearer but the first is less expensive"),
            "The second option feels clearer but the first is less expensive"
        );
    }

    #[test]
    fn distinct_ordinal_noun_phrases_stay_prose() {
        assert_eq!(
            layout("The second phase took five days and the third phase took six days"),
            "The second phase took five days and the third phase took six days"
        );
        assert_eq!(
            layout("My first thought differs from her second idea"),
            "My first thought differs from her second idea"
        );
    }

    #[test]
    fn a_single_bullet_or_yaodian_stays_prose() {
        assert_eq!(layout("今天会议的要点是进度"), "今天会议的要点是进度");
        assert_eq!(
            layout("the bullet train is late"),
            "the bullet train is late"
        );
    }

    #[test]
    fn two_chinese_ordinals_become_a_numbered_list() {
        assert_eq!(
            layout("第一完成设计第二写测试第三发布"),
            "1. 完成设计\n2. 写测试\n3. 发布"
        );
        assert_eq!(
            layout("第一点我们需要先验证数据第二点再发布结果"),
            "1. 我们需要先验证数据\n2. 再发布结果"
        );
        let date_like = "第一天开会第二天交付";
        assert_eq!(layout(date_like), date_like);
    }

    #[test]
    fn english_ordinals_and_explicit_number_markers_become_lists() {
        assert_eq!(
            layout("first finish the design second write tests"),
            "1. finish the design\n2. write tests"
        );
        assert_eq!(
            layout("number one finish the report number two send it"),
            "1. finish the report\n2. send it"
        );
        assert_eq!(
            layout("number one buy milk number two buy eggs"),
            "1. buy milk\n2. buy eggs"
        );
    }

    #[test]
    fn spoken_digit_and_yi_shi_markers_become_lists() {
        assert_eq!(
            layout("1. 是 prompt 2. 是标点符号 3. 是逻辑"),
            "1. 是 prompt\n2. 是标点符号\n3. 是逻辑"
        );
        assert_eq!(layout("1、完成设计 2、写测试"), "1. 完成设计\n2. 写测试");
        assert_eq!(
            layout("一是 prompt 二是标点符号 三是逻辑"),
            "1. prompt\n2. 标点符号\n3. 逻辑"
        );
    }

    #[test]
    fn spoken_digit_list_false_positives_stay_prose() {
        assert_eq!(layout("预算是 1250"), "预算是 1250");
        assert_eq!(layout("看它对不对"), "看它对不对");
        assert_eq!(layout("一是先验证数据"), "一是先验证数据");
        assert_eq!(layout("version 1.2 and 1.3"), "version 1.2 and 1.3");
        assert_eq!(
            layout("The cold room was minus six point five degrees Celsius"),
            "The cold room was minus six point five degrees Celsius"
        );
        assert_eq!(
            layout("The report lists six incidents and five follow-up checks"),
            "The report lists six incidents and five follow-up checks"
        );
        assert_eq!(
            layout("this is one of the options"),
            "this is one of the options"
        );
    }

    #[test]
    fn production_preparation_preserves_decimal_measurements() {
        let input = "The cold room was minus six point five degrees Celsius";
        assert_eq!(
            crate::prepare_spoken_transcript(input, ContextFamily::General, 0.9),
            input
        );
    }

    #[test]
    fn spoken_bullets_become_dash_list() {
        assert_eq!(
            layout("bring bullet computer bullet charger"),
            "bring\n\n- computer\n- charger"
        );
    }

    #[test]
    fn list_false_positives_and_incomplete_ordinals_stay_prose() {
        assert_eq!(
            layout("我第一个想到的是先验证数据"),
            "我第一个想到的是先验证数据"
        );
        assert_eq!(layout("第一我们先验证数据"), "第一我们先验证数据");
        assert_eq!(
            layout("第一部分是数据库迁移。第二部分是回滚方案"),
            "第一部分是数据库迁移。第二部分是回滚方案"
        );
        assert_eq!(
            layout("this is one of the options"),
            "this is one of the options"
        );
        assert_eq!(layout("two people are waiting"), "two people are waiting");
    }

    #[test]
    fn wechat_still_lists_spoken_ordinals() {
        assert_eq!(
            apply("第一完成设计第二写测试", ContextFamily::PersonalChat, 0.9),
            "1. 完成设计\n2. 写测试"
        );
    }

    #[test]
    fn terminal_search_and_forms_do_not_auto_list() {
        for family in [
            ContextFamily::Terminal,
            ContextFamily::BrowserSearch,
            ContextFamily::FormFilling,
        ] {
            assert_eq!(
                apply("第一完成设计第二写测试", family, 0.9),
                "第一完成设计第二写测试"
            );
        }
        assert_eq!(
            apply("hello new line world", ContextFamily::Terminal, 0.9),
            "hello\nworld"
        );
    }

    #[test]
    fn email_splits_spoken_greeting_and_closing_only() {
        assert_eq!(
            apply(
                "Hi John looking forward to chatting tomorrow Best Allan",
                ContextFamily::Email,
                0.9
            ),
            "Hi John\nlooking forward to chatting tomorrow\nBest\nAllan"
        );
        assert_eq!(
            apply(
                "Hello looking forward to chatting tomorrow",
                ContextFamily::Email,
                0.9
            ),
            "Hello\nlooking forward to chatting tomorrow"
        );
        assert_eq!(
            apply(
                "looking forward to chatting tomorrow",
                ContextFamily::Email,
                0.9
            ),
            "looking forward to chatting tomorrow"
        );
        assert_eq!(
            apply("您好明天三点开会谢谢", ContextFamily::Email, 0.9),
            "您好\n明天三点开会\n谢谢"
        );
        let compact_ranges = super::email_layout_separator_ranges("您好明天三点开会谢谢");
        assert_eq!(compact_ranges, vec![6..6, 24..24]);
        assert!(super::email_layout_separator_ranges("We did our best today.").is_empty());
        assert_eq!(
            apply(
                "hi john looking forward to chatting tomorrow best allan",
                ContextFamily::Email,
                0.9
            ),
            "hi john\nlooking forward to chatting tomorrow\nbest\nallan"
        );
        assert_eq!(
            apply("你好吗我周五有空", ContextFamily::Email, 0.9),
            "你好吗我周五有空"
        );
    }

    #[test]
    fn chat_does_not_split_on_thanks() {
        assert_eq!(
            apply("can you ping Maya thanks", ContextFamily::WorkChat, 0.9),
            "can you ping Maya thanks"
        );
        assert_eq!(
            apply("谢谢你周五前发报告", ContextFamily::PersonalChat, 0.9),
            "谢谢你周五前发报告"
        );
        assert_eq!(
            apply(
                "Hi John looking forward Best Allan",
                ContextFamily::Email,
                0.5
            ),
            "Hi John looking forward Best Allan"
        );
    }

    #[test]
    fn restore_if_flattened_keeps_spoken_breaks_and_list_prefixes() {
        assert_eq!(
            restore_if_flattened(
                "the reading club\nshould be tomorrow",
                "The reading club should be tomorrow."
            ),
            "The reading club\nshould be tomorrow."
        );
        assert_eq!(
            restore_if_flattened("1. a\n2. b", "1. a 2. b"),
            "1. a\n2. b"
        );
        assert_eq!(
            restore_if_flattened("hello world", "Hello world."),
            "Hello world."
        );
        assert_eq!(
            restore_if_flattened("1. a\n2. b", "1. a\n2. b."),
            "1. a\n2. b."
        );
        assert_eq!(
            restore_if_flattened("hello world", "Hello.\nWorld."),
            "Hello. World."
        );
        assert_eq!(
            restore_if_flattened(
                "the reading club\nshould be tomorrow",
                "The reading club\n\nshould be tomorrow."
            ),
            "The reading club\nshould be tomorrow."
        );
        assert_eq!(
            restore_if_flattened("1. a\n2. b", "1. a\n2. b\n3. c"),
            "1. a\n2. b c"
        );
        assert_eq!(
            restore_if_flattened(
                "1. 是 system prompt\n2. 是看一下 style",
                "1. 是 system prompt. 2. 是看一下 style."
            ),
            "1. 是 system prompt.\n2. 是看一下 style."
        );
        assert_eq!(
            restore_if_flattened(
                "1. 是 system prompt\n2. 是看一下 style",
                "1. 是 system prompt\n2. 是看一下 style\n3. 是 prompt"
            ),
            "1. 是 system prompt\n2. 是看一下 style 是 prompt"
        );
    }

    #[test]
    fn oh_bu_dui_list_backtracks_drop_the_false_start() {
        assert_eq!(
            layout(
                "我们来测试一下,看一下具体的 ASR 流程。1. 是 prompt。看一下 prompt 到底怎么样。哦,不对, 2. 是 system。哦,不对, 3. 是 system prompt,看一下具体的流程怎么样。4. 是看一下 style,和它的逻辑是怎么样。5. 是看一下它整个的识别率怎么样。"
            ),
            "我们来测试一下,看一下具体的 ASR 流程。\n\n1. 是 system prompt,看一下具体的流程怎么样。\n2. 是看一下 style,和它的逻辑是怎么样。\n3. 是看一下它整个的识别率怎么样。"
        );
        assert_eq!(
            layout(
                "我们来测试一下,看一下具体的 ASR 流程。\n1. 是 prompt。看一下 prompt 到底怎么样。哦,不对,\n2. 是 system。哦,不对,\n3. 是 system prompt,看一下具体的流程怎么样。\n4. 是看一下 style,和它的\n逻辑是怎么样。\n5. 是看一下它整个的识别率怎么样。"
            ),
            "我们来测试一下,看一下具体的 ASR 流程。\n\n1. 是 system prompt,看一下具体的流程怎么样。\n2. 是看一下 style,和它的\n逻辑是怎么样。\n3. 是看一下它整个的识别率怎么样。"
        );
        assert_eq!(
            layout("1. write a cloud test oh wait 2. write a cursor test"),
            "1. write a cursor test"
        );
        assert_eq!(
            layout(
                "1. 是 system prompt，看一下具体的流程怎么样。2. 是 system prompt，看一下具体的流程怎么样。再确认一下。哦,不对 3. 是看一下 style"
            ),
            "1. 是 system prompt，看一下具体的流程怎么样。\n2. 是看一下 style"
        );
        assert_eq!(
            layout("1. 看它对不对 oh wait 2. 再看 style"),
            "1. 再看 style"
        );
        assert_eq!(
            layout("1. 是 prompt。哦,不对, 2. 是 system。哦,不对,"),
            "1. 是 system。"
        );
        assert_eq!(
            layout("1. 是 system prompt 2. 是看一下 style。哦,不对,"),
            "1. 是 system prompt\n2. 是看一下 style。"
        );
        assert_eq!(
            layout("1. write a cloud test oh wait"),
            "1. write a cloud test"
        );
        assert_eq!(
            layout("1. write a cloud test scratch that 2. write a cursor test"),
            "1. write a cursor test"
        );
        assert_eq!(
            layout("1. write a cloud test oh wait。 2. write a cursor test"),
            "1. write a cursor test"
        );
        assert_eq!(layout("看它对不对"), "看它对不对");
        assert_eq!(
            layout("周四，不对，周五下午开会"),
            "周四，不对，周五下午开会"
        );
        assert_eq!(
            layout("1. 看它对不对 2. 再看 style"),
            "1. 看它对不对\n2. 再看 style"
        );
    }

    #[test]
    fn fenced_code_layout_projection_preserves_bodies_and_checks_outside_structure() {
        let source = "```text\n  alpha\n```\nPlease, continue.";
        assert!(super::preserves_required_layout(
            source,
            "```text\n  alpha\n```\nPlease continue."
        ));
        assert!(super::preserves_required_layout(
            source,
            "  alpha\nPlease continue."
        ));
        assert!(!super::preserves_required_layout(
            source,
            "```text\n alpha\n```\nPlease continue."
        ));
        assert!(!super::preserves_required_layout(
            "```text\n  alpha\n\n```\nPlease continue.",
            "```text\n  alpha\n```\nPlease continue."
        ));
        assert!(!super::preserves_required_layout(
            source,
            "  alpha Please continue."
        ));
        let dropped_wrapper = "  alpha\nPlease continue.";
        assert_eq!(
            super::restore_if_flattened(source, dropped_wrapper),
            dropped_wrapper,
            "raw fence-line repair must not rewrite a source-bound wrapper drop"
        );

        let inline_source = "Use `API` first.\nKeep going.";
        let inline_flattened = "Use `API` first. Keep going.";
        assert_eq!(
            super::restore_if_flattened(inline_source, inline_flattened),
            inline_source,
            "inline-only sources retain the prior break-repair path"
        );

        let (source_view, candidate_view) = crate::protected_span::validated_layout_views(
            "```text\n  alpha\n```\nPlease, continue.",
            "  alpha\nPlease continue.",
        )
        .expect("exact body and ordered source binding");
        assert_eq!(source_view, "CODE\nPlease, continue.");
        assert_eq!(candidate_view, "CODE\nPlease continue.");
        assert!(crate::protected_span::validated_layout_views(
            "```text\n  alpha\n```\nPlease continue.",
            "```text\n alpha\n```\nPlease continue.",
        )
        .is_none());
    }

    #[test]
    fn inline_code_break_repair_requires_and_reuses_candidate_owned_regions() {
        let cases = [
            ("`!`\nKeep going.", ", Keep going.", "`!` Keep going."),
            ("`---`\nKeep going.", ", Keep going.", "`---` Keep going."),
            ("`🙂`\nKeep going.", ", Keep going.", "`🙂` Keep going."),
        ];

        for (source, missing_body, present_once) in cases {
            assert_eq!(
                super::restore_if_flattened(source, missing_body),
                missing_body,
                "a layout owner must not create a missing code body"
            );
            assert_eq!(
                super::restore_if_flattened(source, present_once),
                source,
                "a proven candidate code occurrence is moved exactly once"
            );
        }

        assert_eq!(
            super::restore_if_flattened(
                "Use `API` first.\nKeep going.",
                "Use API first. Keep going."
            ),
            "Use API first.\nKeep going.",
            "wrapper removal keeps the bound bare candidate occurrence"
        );
        assert_eq!(
            super::restore_if_flattened(
                "1. Use `API` first.\n2. Keep going.",
                "1. Use `API` first. 2. Keep going."
            ),
            "1. Use `API` first.\n2. Keep going.",
            "the inline list repair still restores prose layout"
        );
        let changed_body = "Use `Api` first. Keep going.";
        assert_eq!(
            super::restore_if_flattened("Use `API` first.\nKeep going.", changed_body),
            changed_body,
            "an invalid code body is left for the protected owner"
        );
    }

    #[test]
    fn layout_excludes_only_source_backed_clause_leading_hesitations() {
        let fenced = "```text\n  alpha\n```\nUm, I am ready.\nPlease continue.";
        let hesitation = super::clause_leading_hesitation_ranges(fenced);
        assert_eq!(hesitation.len(), 1);
        assert_eq!(&fenced[hesitation[0].clone()], "Um");
        assert!(super::preserves_required_layout(
            fenced,
            "```text\n  alpha\n```\nI am ready.\nPlease continue."
        ));

        let ordinary = "Um, I am ready.\nPlease continue.";
        assert!(super::preserves_required_layout(
            ordinary,
            "I am ready.\nPlease continue."
        ));
        assert!(!super::preserves_required_layout(
            "```text\n  alpha\n```\n“Um, I am ready.”\nPlease continue.",
            "```text\n  alpha\n```\n“I am ready.”\nPlease continue."
        ));
        assert!(!super::preserves_required_layout(
            "```text\n  alpha\n```\nUm is a person.\nPlease continue.",
            "```text\n  alpha\n```\nis a person.\nPlease continue."
        ));
        assert!(!super::preserves_required_layout(
            "```text\n  alpha\n```\nUmami is a person.\nPlease continue.",
            "```text\n  alpha\n```\nami is a person.\nPlease continue."
        ));
        assert!(!super::preserves_required_layout(
            "```text\n  alpha\n```\nUm, do not pay $12.\nPlease continue.",
            "```text\n  alpha\n```\ndo pay $13.\nPlease continue."
        ));
        assert!(!super::preserves_required_layout(
            "```text\n  alpha\n```\nUm, keep the complete report.\nPlease continue.",
            "```text\n  alpha\n```\nkeep the report.\nPlease continue."
        ));
        assert!(super::preserves_required_layout(
            "```text\n  alpha\n```\n1. Um, first item.\n2. second item.",
            "```text\n  alpha\n```\n1. first item.\n2. second item."
        ));
        assert!(super::preserves_required_layout(
            "```text\n  Um\n```\nPlease continue.",
            "```text\n  Um\n```\nPlease, continue."
        ));

        let numbered = "```text\n  alpha\n```\n1. Um, first item.\n2. Second item.";
        assert!(
            super::clause_leading_hesitation_ranges(numbered).is_empty(),
            "the unresolved numbered-row role remains outside the finite-clause waiver"
        );
    }
}
