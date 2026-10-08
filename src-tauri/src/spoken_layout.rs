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

/// Apply spoken layout to already-punctuated ASR text.
#[cfg(test)]
pub fn apply(text: &str, family: ContextFamily, confidence: f32) -> String {
    apply_layout_stages(text, family, confidence, false).text
}

pub(crate) struct AppliedLayout {
    pub(crate) text: String,
    pub(crate) source_preserved: bool,
}

struct LayoutStage {
    text: String,
    consumed_syntax_ranges: Vec<std::ops::Range<usize>>,
}

struct LayoutPipeline {
    text: String,
    source_preserved: bool,
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
    if family == ContextFamily::Email && confidence >= 0.75 {
        let stage = apply_email_layout_stage(&current);
        apply_layout_stage(&mut current, stage, guard_source, &mut source_preserved);
    }
    let lists = apply_lists_stage(&current, family);
    apply_layout_stage(&mut current, lists, guard_source, &mut source_preserved);
    let backtracks = apply_backtracks_stage(&current);
    apply_layout_stage(
        &mut current,
        backtracks,
        guard_source,
        &mut source_preserved,
    );
    LayoutPipeline {
        text: current,
        source_preserved,
    }
}

fn apply_layout_stage(
    current: &mut String,
    stage: LayoutStage,
    guard_source: bool,
    source_preserved: &mut bool,
) {
    if guard_source
        && !preserves_source_protected_content_after_layout(
            current,
            &stage.text,
            &stage.consumed_syntax_ranges,
        )
    {
        *source_preserved = false;
        return;
    }
    *current = stage.text;
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
}

fn apply_lists_stage(text: &str, family: ContextFamily) -> LayoutStage {
    if lists_disabled(family) {
        return LayoutStage::unchanged(text);
    }
    let code_ranges = code_literal_ranges(text);
    let mut out = String::with_capacity(text.len());
    let mut consumed_syntax_ranges = Vec::new();
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
        } else if let Some((formatted, ranges)) =
            format_list_block(block, source_offset, text, &code_ranges)
        {
            out.push_str(&formatted);
            consumed_syntax_ranges.extend(ranges);
        } else {
            out.push_str(block);
        }
        source_offset = block_end;
    }
    LayoutStage {
        text: out,
        consumed_syntax_ranges,
    }
}

fn format_list_block(
    block: &str,
    source_offset: usize,
    full_source: &str,
    code_ranges: &[std::ops::Range<usize>],
) -> Option<(String, Vec<std::ops::Range<usize>>)> {
    let markers = collect_markers(block, source_offset, full_source, code_ranges);
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
    if numbered.len() >= 2 {
        return render_list(block, source_offset, &numbered, MarkerKind::Numbered);
    }
    if bullets.len() >= 2 {
        return render_list(block, source_offset, &bullets, MarkerKind::Bullet);
    }
    None
}

fn render_list(
    block: &str,
    source_offset: usize,
    markers: &[Marker],
    kind: MarkerKind,
) -> Option<(String, Vec<std::ops::Range<usize>>)> {
    let preamble = block[..markers[0].start].trim();
    let mut items = Vec::new();
    for (index, marker) in markers.iter().enumerate() {
        let end = markers
            .get(index + 1)
            .map(|next| next.start)
            .unwrap_or(block.len());
        let item = block[marker.end..end].trim();
        if !item.is_empty() {
            items.push(item);
        }
    }
    if items.len() < 2 {
        return None;
    }
    let mut out = String::new();
    if !preamble.is_empty() {
        out.push_str(preamble);
        out.push('\n');
    }
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        match kind {
            MarkerKind::Numbered => {
                out.push_str(&(index + 1).to_string());
                out.push_str(". ");
            }
            MarkerKind::Bullet => out.push_str("- "),
        }
        out.push_str(item);
    }
    Some((
        out,
        markers
            .iter()
            .map(|marker| source_offset + marker.start..source_offset + marker.end)
            .collect(),
    ))
}

fn apply_backtracks_stage(text: &str) -> LayoutStage {
    let code_ranges = code_literal_ranges(text);
    let quote_ranges = explicit_quote_ranges(text);
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
        if contains_literal {
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
            markers.push(marker);
            continue;
        }
        let ch = text[index..].chars().next().expect("rest is non-empty");
        index += ch.len_utf8();
    }
    markers
}

fn match_marker_at(text: &str, start: usize) -> Option<Marker> {
    if let Some(end) = match_chinese_ordinal(text, start) {
        return Some(Marker {
            start,
            end,
            kind: MarkerKind::Numbered,
        });
    }
    if let Some(end) = match_digit_list_marker(text, start) {
        return Some(Marker {
            start,
            end,
            kind: MarkerKind::Numbered,
        });
    }
    if let Some(end) = match_yi_shi_marker(text, start) {
        return Some(Marker {
            start,
            end,
            kind: MarkerKind::Numbered,
        });
    }
    if let Some(end) = match_english_ordinal(text, start) {
        return Some(Marker {
            start,
            end,
            kind: MarkerKind::Numbered,
        });
    }
    if let Some(end) = match_bullet(text, start) {
        return Some(Marker {
            start,
            end,
            kind: MarkerKind::Bullet,
        });
    }
    None
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
    let source = layout_content_chars(source);
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
    if !has_structural_layout(source) {
        return !has_structural_layout(candidate);
    }
    if !preserves_explicit_layout_content(source, candidate)
        || newline_count(source) != newline_count(candidate)
        || list_prefix_count(source) != list_prefix_count(candidate)
    {
        return false;
    }
    let source_lines = source.lines().collect::<Vec<_>>();
    let candidate_lines = candidate.lines().collect::<Vec<_>>();
    if source_lines.len() != candidate_lines.len() {
        return false;
    }
    let preserve_indentation = source.contains("```")
        || source_lines.iter().any(|line| {
            let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
            indent > 0
        });
    if preserve_indentation {
        return source == candidate;
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
    for range in syntax_ranges.iter().rev() {
        let replacement = " ".repeat(source[range.start..range.end].chars().count());
        source_without_syntax_terms.replace_range(range.clone(), &replacement);
    }
    preserves_source_protected_content(&source_without_syntax_terms, candidate)
}

fn preserves_line_content(source: &str, candidate: &str) -> bool {
    let source = layout_content_chars(&strip_list_prefix(source));
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
        out.push(if last_content {
            collapse_extra_breaks(&piece)
        } else {
            piece.trim().to_string()
        });
    }
    Some(out.join("\n"))
}

fn newline_count(text: &str) -> usize {
    text.chars().filter(|&ch| ch == '\n').count()
}

fn list_prefix_count(text: &str) -> usize {
    text.lines().filter(|line| is_list_line(line)).count()
}

fn is_list_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("- ") {
        return true;
    }
    let digits = trimmed.chars().take_while(|ch| ch.is_ascii_digit()).count();
    digits > 0
        && trimmed
            .get(digits..)
            .is_some_and(|rest| rest.starts_with(". "))
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
    let start = skip_leading_whitespace(text, 0);
    if is_inside_explicit_quote(text, start) || is_inside_code(text, start) {
        return (String::new(), text.to_owned());
    }
    let rest = &text[start..];
    for &from in EMAIL_GREETINGS {
        if !starts_with_ignore_ascii_case(rest, from) {
            continue;
        }
        let mut end = start + from.len();
        if is_cjk_command(from) {
            if !is_cjk_right_boundary(text, end) {
                continue;
            }
            if cjk_greeting_is_question(text, end) {
                continue;
            }
            end = consume_cjk_name(text, end);
        } else {
            if !is_ascii_word_right(text, end) {
                continue;
            }
            end = consume_english_name(text, end);
        }
        let greeting = text[start..end].trim().to_string();
        let body = text[end..].trim().to_string();
        if greeting.is_empty() {
            break;
        }
        return (greeting, body);
    }
    (String::new(), text.to_string())
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
    use super::{apply, restore_if_flattened};
    use crate::context::ContextFamily;

    fn layout(text: &str) -> String {
        apply(text, ContextFamily::General, 1.0)
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
            "bring\n- computer\n- charger"
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
            "我们来测试一下,看一下具体的 ASR 流程。\n1. 是 system prompt,看一下具体的流程怎么样。\n2. 是看一下 style,和它的逻辑是怎么样。\n3. 是看一下它整个的识别率怎么样。"
        );
        assert_eq!(
            layout(
                "我们来测试一下,看一下具体的 ASR 流程。\n1. 是 prompt。看一下 prompt 到底怎么样。哦,不对,\n2. 是 system。哦,不对,\n3. 是 system prompt,看一下具体的流程怎么样。\n4. 是看一下 style,和它的\n逻辑是怎么样。\n5. 是看一下它整个的识别率怎么样。"
            ),
            "我们来测试一下,看一下具体的 ASR 流程。\n1. 是 system prompt,看一下具体的流程怎么样。\n2. 是看一下 style,和它的\n逻辑是怎么样。\n3. 是看一下它整个的识别率怎么样。"
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
            layout("1. 是 prompt，不对 2. 是 system"),
            "1. 是 prompt，不对\n2. 是 system"
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
}
