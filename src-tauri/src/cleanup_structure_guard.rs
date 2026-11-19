//! Strict acceptance of punctuation-only cleanup candidates for structured
//! source. Run this on the parsed provider reply before lossy normalization.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SafePunctuation {
    Period,
    Comma,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SafeView {
    /// Exact Unicode content and whitespace, with only safe prose punctuation
    /// removed. Byte slots keep allowed punctuation anchored to source content.
    skeleton: String,
    punctuation: Vec<PunctuationEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PunctuationEvent {
    slot: usize,
    kind: SafePunctuation,
    line_terminal: bool,
    question_like_line: bool,
}

struct PunctuationExclusions<'a> {
    literals: &'a [Range<usize>],
    code_regions: &'a [Range<usize>],
    protected: &'a [crate::protected_span::ProtectedSpan],
    list_prefixes: &'a [Range<usize>],
}

/// Whether a candidate preserves structured cleanup source except for a
/// narrow set of punctuation-only changes in prose.
///
/// The caller must pass the parsed provider reply before reply normalization,
/// layout restoration, or other lossy cleanup. Existing candidate guards still
/// run around this predicate at the provider and application boundaries.
pub(crate) fn preserves_structure_safe(source: &str, candidate: &str) -> bool {
    if source == candidate {
        return true;
    }
    let inline_filler_waivers = inline_filler_waiver_ranges(source, candidate).unwrap_or_default();
    if !crate::spoken_layout::preserves_required_layout(source, candidate)
        || !crate::protected_span::preserves_with_source_term_waivers(
            source,
            candidate,
            Some(crate::llm::CleanupOperation::Cleanup),
            &inline_filler_waivers,
        )
        || !exact_protected_span_values(source, candidate, &inline_filler_waivers)
        || !exact_literal_ranges(source, candidate)
        || !exact_code_regions(source, candidate)
        || !exact_list_prefixes(source, candidate)
    {
        return false;
    }

    let Some(source_view) = safe_view(source) else {
        return false;
    };
    let Some(candidate_view) = safe_view(candidate) else {
        return false;
    };

    if source_view.skeleton == candidate_view.skeleton
        && punctuation_changes_are_one_sided(&source_view.punctuation, &candidate_view.punctuation)
    {
        return true;
    }

    !inline_filler_waivers.is_empty()
}

/// Prove model-selected paragraph breaks against a flat prepared source.
/// Paragraph bodies must consume the source continuously and in order; the
/// only removed source bytes may be whitespace at an existing sentence break,
/// with a narrow zero-width exception after CJK sentence punctuation.
pub(crate) fn preserves_model_selected_paragraphs(source: &str, candidate: &str) -> bool {
    const MAX_PARAGRAPHS: usize = 64;
    if source.trim() != source
        || source.contains(['\n', '\r'])
        || crate::spoken_layout::has_structural_layout(source)
        || candidate.trim() != candidate
        || candidate.contains('\r')
    {
        return false;
    }

    let paragraphs = candidate.split("\n\n").collect::<Vec<_>>();
    if paragraphs.len() < 2
        || paragraphs.len() > MAX_PARAGRAPHS
        || paragraphs.iter().any(|paragraph| {
            paragraph.is_empty()
                || paragraph.contains('\n')
                || paragraph.trim() != *paragraph
                || has_forbidden_paragraph_prefix(paragraph)
        })
        || paragraphs[..paragraphs.len() - 1]
            .iter()
            .any(|paragraph| !ends_with_sentence_punctuation(paragraph))
    {
        return false;
    }

    let literal_ranges = crate::spoken_revision::literal_ranges(source);
    let protected =
        crate::protected_span::protected_spans(source, Some(crate::llm::CleanupOperation::Cleanup));
    let mut source_start = 0;
    for (paragraph_index, paragraph) in paragraphs[..paragraphs.len() - 1].iter().enumerate() {
        let mut matched_next_start = None;
        for (relative, punctuation) in source[source_start..].char_indices() {
            if !matches!(punctuation, '.' | '!' | '?' | '。' | '！' | '？') {
                continue;
            }
            let punctuation_start = source_start + relative;
            let punctuation_end = punctuation_start + punctuation.len_utf8();
            let mut boundary_end = punctuation_end;
            while let Some((closing, width)) = source[boundary_end..]
                .chars()
                .next()
                .map(|ch| (ch, ch.len_utf8()))
            {
                if !is_closing_quote_or_bracket(closing) {
                    break;
                }
                boundary_end += width;
            }
            let next_start = skip_whitespace(source, boundary_end);
            let has_source_whitespace = next_start > boundary_end;
            let cjk_zero_width_break = !has_source_whitespace
                && matches!(punctuation, '。' | '！' | '？')
                && source[boundary_end..]
                    .chars()
                    .next()
                    .is_some_and(is_cjk_character);
            if (!has_source_whitespace && !cjk_zero_width_break)
                || next_start >= source.len()
                || unsafe_sentence_break(
                    source,
                    punctuation,
                    punctuation_start,
                    boundary_end,
                    next_start,
                )
                || boundary_inside(&literal_ranges, boundary_end)
                || separator_overlaps(&literal_ranges, boundary_end..next_start)
                || protected_boundary_inside(&protected, boundary_end)
                || separator_overlaps_protected(&protected, boundary_end..next_start)
            {
                continue;
            }

            let source_body = &source[source_start..boundary_end];
            // Every allowed punctuation change is one-sided and the only
            // source mark omitted from a segment key can be its terminal
            // Unicode period. Later boundaries cannot match after this bound.
            if source_body.len() > paragraph.len().saturating_add(3) {
                break;
            }
            if preserves_structure_safe(source_body, paragraph)
                && begins_like_a_sentence(paragraphs[paragraph_index + 1])
            {
                matched_next_start = Some(next_start);
                break;
            }
        }
        let Some(next_start) = matched_next_start else {
            return false;
        };
        source_start = next_start;
    }

    source.get(source_start..).is_some_and(|remaining| {
        preserves_structure_safe(remaining, paragraphs[paragraphs.len() - 1])
    })
}

fn has_forbidden_paragraph_prefix(paragraph: &str) -> bool {
    let trimmed = paragraph.trim_start();
    if trimmed.starts_with(['#', '>', '|'])
        || ["- ", "* ", "+ ", "• ", "‣ ", "– ", "— "]
            .iter()
            .any(|prefix| trimmed.starts_with(prefix))
    {
        return true;
    }
    if let Some(marker_end) = trimmed.find(['.', ')']) {
        if marker_end > 0
            && trimmed[..marker_end]
                .bytes()
                .all(|byte| byte.is_ascii_digit())
            && trimmed[marker_end + 1..].starts_with(' ')
        {
            return true;
        }
    }
    let normalized = trimmed
        .trim_end_matches(['.', ':', '：'])
        .to_ascii_lowercase();
    [
        "summary",
        "overview",
        "conclusion",
        "next steps",
        "action items",
        "update",
        "request",
        "context",
        "notes",
    ]
    .iter()
    .any(|heading| normalized == *heading)
}

fn ends_with_sentence_punctuation(paragraph: &str) -> bool {
    let core = paragraph.trim_end_matches(is_closing_quote_or_bracket);
    core.chars()
        .next_back()
        .is_some_and(|ch| matches!(ch, '.' | '!' | '?' | '。' | '！' | '？'))
}

fn begins_like_a_sentence(paragraph: &str) -> bool {
    paragraph
        .trim_start_matches(is_opening_quote_or_bracket)
        .chars()
        .next()
        .is_some_and(|ch| !ch.is_ascii_lowercase())
}

fn unsafe_sentence_break(
    source: &str,
    punctuation: char,
    punctuation_start: usize,
    boundary_end: usize,
    next_start: usize,
) -> bool {
    if punctuation != '.' {
        return false;
    }
    let left = source[..punctuation_start].trim_end();
    let token = left
        .rsplit(|character: char| !character.is_ascii_alphabetic())
        .next()
        .unwrap_or_default();
    let token_lower = token.to_ascii_lowercase();
    const ABBREVIATIONS: &[&str] = &[
        "a.m", "p.m", "adm", "approx", "ave", "dept", "dr", "e.g", "etc", "fig", "i.e", "inc",
        "jr", "mr", "mrs", "ms", "no", "prof", "rev", "sr", "st", "vs",
    ];
    let previous = left.chars().next_back();
    let next = source[next_start..].chars().next();
    token.len() == 1
        || ABBREVIATIONS.contains(&token_lower.as_str())
        || previous.is_some_and(|ch| ch.is_ascii_digit())
            && next.is_some_and(|ch| ch.is_ascii_digit())
        || boundary_end < next_start && next.is_some_and(|ch| ch.is_ascii_digit())
}

fn is_opening_quote_or_bracket(ch: char) -> bool {
    matches!(
        ch,
        '"' | '\'' | '“' | '‘' | '「' | '『' | '（' | '(' | '[' | '{'
    )
}

fn is_closing_quote_or_bracket(ch: char) -> bool {
    matches!(
        ch,
        '"' | '\'' | '”' | '’' | '」' | '』' | '）' | ')' | ']' | '}'
    )
}

fn is_cjk_character(ch: char) -> bool {
    matches!(ch as u32, 0x2E80..=0x2FFF | 0x3000..=0x30FF | 0x31F0..=0x31FF | 0x3400..=0x9FFF | 0xF900..=0xFAFF)
}

fn skip_whitespace(source: &str, mut cursor: usize) -> usize {
    for (relative, character) in source[cursor..].char_indices() {
        if !character.is_whitespace() {
            cursor += relative;
            return cursor;
        }
    }
    source.len()
}

fn boundary_inside(ranges: &[Range<usize>], boundary: usize) -> bool {
    ranges
        .iter()
        .any(|range| range.start < boundary && boundary < range.end)
}

fn protected_boundary_inside(
    protected: &[crate::protected_span::ProtectedSpan],
    boundary: usize,
) -> bool {
    protected
        .iter()
        .any(|span| span.start_byte < boundary && boundary < span.end_byte)
}

fn separator_overlaps(ranges: &[Range<usize>], separator: Range<usize>) -> bool {
    separator.start < separator.end && overlaps(ranges, separator)
}

fn separator_overlaps_protected(
    protected: &[crate::protected_span::ProtectedSpan],
    separator: Range<usize>,
) -> bool {
    separator.start < separator.end && overlaps_protected(protected, separator)
}

fn safe_view(text: &str) -> Option<SafeView> {
    let literals = crate::spoken_revision::literal_ranges(text);
    let code_regions = crate::protected_span::validated_code_region_pairs(text, text)?
        .into_iter()
        .map(|(source, _)| source)
        .collect::<Vec<_>>();
    let protected =
        crate::protected_span::protected_spans(text, Some(crate::llm::CleanupOperation::Cleanup));
    let list_prefixes = list_prefix_ranges(text);
    let exclusions = PunctuationExclusions {
        literals: &literals,
        code_regions: &code_regions,
        protected: &protected,
        list_prefixes: &list_prefixes,
    };

    let mut skeleton = String::with_capacity(text.len());
    let mut punctuation = Vec::new();
    for (offset, ch) in text.char_indices() {
        let range = offset..offset + ch.len_utf8();
        if let Some(event) = eligible_punctuation(text, offset, ch, skeleton.len(), &exclusions) {
            punctuation.push(event);
        } else {
            skeleton.push(ch);
        }
        debug_assert!(text.is_char_boundary(range.end));
    }

    Some(SafeView {
        skeleton,
        punctuation,
    })
}

fn eligible_punctuation(
    text: &str,
    offset: usize,
    ch: char,
    skeleton_slot: usize,
    exclusions: &PunctuationExclusions<'_>,
) -> Option<PunctuationEvent> {
    let range = offset..offset + ch.len_utf8();
    if overlaps(exclusions.literals, range.clone())
        || overlaps(exclusions.code_regions, range.clone())
        || overlaps_protected(exclusions.protected, range.clone())
        || overlaps(exclusions.list_prefixes, range)
    {
        return None;
    }

    let previous = text[..offset].chars().next_back();
    let punctuation_end = offset + ch.len_utf8();
    let next = text[punctuation_end..].chars().next();
    match ch {
        '.' | '。'
            if previous.is_some_and(is_word_character)
                && (is_line_terminal(text, punctuation_end)
                    || previous.is_some_and(is_cjk) && next.is_some_and(is_cjk)) =>
        {
            let line_terminal = is_line_terminal(text, punctuation_end);
            Some(PunctuationEvent {
                slot: skeleton_slot,
                kind: SafePunctuation::Period,
                line_terminal,
                question_like_line: line_terminal
                    && line_is_question_like(text, offset, exclusions.list_prefixes),
            })
        }
        ',' | '，'
            if previous.is_some_and(is_word_character)
                && next.is_some_and(|next| next.is_whitespace() || is_cjk(next))
                && !(previous.is_some_and(|previous| previous.is_ascii_digit())
                    && next.is_some_and(|next| next.is_ascii_digit())) =>
        {
            Some(PunctuationEvent {
                slot: skeleton_slot,
                kind: SafePunctuation::Comma,
                line_terminal: false,
                question_like_line: false,
            })
        }
        _ => None,
    }
}

fn punctuation_changes_are_one_sided_inner(
    source: &[PunctuationEvent],
    candidate: &[PunctuationEvent],
    allow_added_inline_commas: bool,
) -> bool {
    let source_inline = source
        .iter()
        .filter(|event| !event.line_terminal)
        .copied()
        .collect::<Vec<_>>();
    let candidate_inline = candidate
        .iter()
        .filter(|event| !event.line_terminal)
        .copied()
        .collect::<Vec<_>>();
    if source_inline != candidate_inline
        && (!allow_added_inline_commas
            || source_inline.len() >= candidate_inline.len()
            || !one_sided_subset(&source_inline, &candidate_inline))
    {
        return false;
    }

    let source_terminal = source
        .iter()
        .filter(|event| event.line_terminal && event.kind == SafePunctuation::Period)
        .collect::<Vec<_>>();
    let candidate_terminal = candidate
        .iter()
        .filter(|event| event.line_terminal && event.kind == SafePunctuation::Period)
        .collect::<Vec<_>>();
    if source_terminal
        .iter()
        .chain(candidate_terminal.iter())
        .any(|event| event.question_like_line)
    {
        return source_terminal == candidate_terminal;
    }
    one_sided_subset(&source_terminal, &candidate_terminal)
}

fn punctuation_changes_are_one_sided(
    source: &[PunctuationEvent],
    candidate: &[PunctuationEvent],
) -> bool {
    punctuation_changes_are_one_sided_inner(source, candidate, false)
}

fn one_sided_subset<T: PartialEq>(source: &[T], candidate: &[T]) -> bool {
    if source == candidate {
        return true;
    }
    let (subset, superset) = if source.len() < candidate.len() {
        (source, candidate)
    } else if candidate.len() < source.len() {
        (candidate, source)
    } else {
        // Same-count punctuation moves/replacements can change sentence sense.
        return false;
    };
    subset.iter().all(|event| superset.contains(event))
}

fn is_line_terminal(text: &str, offset: usize) -> bool {
    let line_end = text[offset..]
        .find(['\r', '\n'])
        .map_or(text.len(), |relative| offset + relative);
    text[offset..line_end].chars().all(char::is_whitespace)
}

fn line_is_question_like(text: &str, offset: usize, list_prefixes: &[Range<usize>]) -> bool {
    let line_start = text[..offset]
        .char_indices()
        .rev()
        .find(|(_, ch)| matches!(ch, '\r' | '\n'))
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    let line = &text[line_start..offset];
    let content_start = list_prefixes
        .iter()
        .find(|range| range.start >= line_start && range.end <= offset)
        .map_or(0, |range| range.end - line_start);
    let Some(content) = line.get(content_start..) else {
        return false;
    };
    let content = content.trim();
    if content.is_empty() {
        return false;
    }
    let prepared =
        crate::spoken_punctuation::ensure_terminal(content, crate::context::ContextFamily::General);
    prepared.ends_with('?') || prepared.ends_with('？')
}

fn exact_list_prefixes(source: &str, candidate: &str) -> bool {
    let source_prefixes = list_prefix_ranges(source)
        .into_iter()
        .filter_map(|range| source.get(range))
        .collect::<Vec<_>>();
    let candidate_prefixes = list_prefix_ranges(candidate)
        .into_iter()
        .filter_map(|range| candidate.get(range))
        .collect::<Vec<_>>();
    source_prefixes == candidate_prefixes
}

fn list_prefix_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let without_newline = line.strip_suffix('\n').unwrap_or(line);
        let content = without_newline
            .strip_suffix('\r')
            .unwrap_or(without_newline);
        let indentation = content.len() - content.trim_start().len();
        let rest = &content[indentation..];
        let prefix_len = if rest.starts_with("- ") {
            Some(2)
        } else {
            let digits = rest.chars().take_while(|ch| ch.is_ascii_digit()).count();
            (digits > 0
                && rest
                    .get(digits..)
                    .is_some_and(|suffix| suffix.starts_with(". ")))
            .then_some(digits + 2)
        };
        if let Some(prefix_len) = prefix_len {
            let start = line_start + indentation;
            ranges.push(start..start + prefix_len);
        }
        line_start += line.len();
    }
    ranges
}

fn exact_protected_span_values(
    source: &str,
    candidate: &str,
    inline_filler_waivers: &[Range<usize>],
) -> bool {
    let inline_filler_cleanup = !inline_filler_waivers.is_empty();
    let source_waivers = crate::spoken_layout::clause_leading_hesitation_ranges(source);
    let source_spans =
        crate::protected_span::protected_spans(source, Some(crate::llm::CleanupOperation::Cleanup))
            .into_iter()
            .filter(|span| {
                span.kind != crate::protected_span::ProtectedSpanKind::Term
                    || !inline_filler_waivers
                        .iter()
                        .any(|range| range.start == span.start_byte && range.end == span.end_byte)
            })
            .collect::<Vec<_>>();
    let candidate_spans = crate::protected_span::protected_spans(
        candidate,
        Some(crate::llm::CleanupOperation::Cleanup),
    );
    let source_lower = if inline_filler_cleanup {
        Some(source.to_ascii_lowercase())
    } else {
        None
    };

    let mut s = 0usize;
    let mut c = 0usize;
    while s < source_spans.len() || c < candidate_spans.len() {
        if s < source_spans.len() && c < candidate_spans.len() {
            let source_span = source_spans[s];
            let candidate_span = candidate_spans[c];
            if source_span.kind == candidate_span.kind
                && source.get(source_span.start_byte..source_span.end_byte)
                    == candidate.get(candidate_span.start_byte..candidate_span.end_byte)
            {
                s += 1;
                c += 1;
                continue;
            }
        }
        if inline_filler_cleanup
            && s < source_spans.len()
            && source_spans[s].kind == crate::protected_span::ProtectedSpanKind::Term
            && (source_waivers.iter().any(|range| {
                range.start == source_spans[s].start_byte && range.end == source_spans[s].end_byte
            }) || crate::protected_span::is_waivable_sentence_initial_source_term(
                source,
                source_spans[s],
            ))
        {
            s += 1;
            continue;
        }
        if inline_filler_cleanup
            && c < candidate_spans.len()
            && candidate_spans[c].kind == crate::protected_span::ProtectedSpanKind::Term
        {
            let cand_span = candidate_spans[c];
            let cand_token = &candidate[cand_span.start_byte..cand_span.end_byte];
            let promoted_from_lowercase =
                crate::protected_span::is_sentence_or_clause_start(candidate, cand_span.start_byte)
                    && source_lower
                        .as_deref()
                        .is_some_and(|lower| lower.contains(&cand_token.to_ascii_lowercase()));
            if promoted_from_lowercase {
                c += 1;
                continue;
            }
        }
        return false;
    }
    true
}

pub(crate) fn preserves_line_structure_with_inline_filler_cleanup(
    source: &str,
    candidate: &str,
) -> bool {
    inline_filler_waiver_ranges(source, candidate).is_some()
}

fn inline_filler_waiver_ranges(source: &str, candidate: &str) -> Option<Vec<Range<usize>>> {
    let src_lines: Vec<&str> = source.split_inclusive('\n').collect();
    let cand_lines: Vec<&str> = candidate.split_inclusive('\n').collect();
    if src_lines.len() != cand_lines.len() || src_lines.is_empty() {
        return None;
    }
    let src_trailing_nl = source.ends_with('\n');
    let cand_trailing_nl = candidate.ends_with('\n');
    if src_trailing_nl != cand_trailing_nl {
        return None;
    }
    let mut waivers = Vec::new();
    let mut deleted_any_filler = false;
    let mut line_start = 0usize;
    for (src_line, cand_line) in src_lines.iter().zip(cand_lines.iter()) {
        let src_nl = line_ending_slice(src_line);
        let cand_nl = line_ending_slice(cand_line);
        if src_nl != cand_nl {
            return None;
        }
        let src_content = &src_line[..src_line.len() - src_nl.len()];
        let cand_content = &cand_line[..cand_line.len() - cand_nl.len()];
        let src_prefix_len = line_structural_prefix_len(src_content);
        let cand_prefix_len = line_structural_prefix_len(cand_content);
        if src_content[..src_prefix_len] != cand_content[..cand_prefix_len] {
            return None;
        }
        let src_body = &src_content[src_prefix_len..];
        let cand_body = &cand_content[cand_prefix_len..];
        if src_body == cand_body {
            line_start += src_line.len();
            continue;
        }
        if let (Some(sv), Some(cv)) = (safe_view(src_body), safe_view(cand_body)) {
            if sv.skeleton == cv.skeleton
                && punctuation_changes_are_one_sided(&sv.punctuation, &cv.punctuation)
            {
                line_start += src_line.len();
                continue;
            }
        }
        let line_waivers = collect_line_spoken_filler_deletions(src_body, cand_body)?;
        let body_offset = line_start + src_prefix_len;
        waivers.extend(
            line_waivers
                .into_iter()
                .map(|r| body_offset + r.start..body_offset + r.end),
        );
        deleted_any_filler = true;
        line_start += src_line.len();
    }
    deleted_any_filler.then_some(waivers)
}

fn line_ending_slice(line: &str) -> &str {
    if line.ends_with("\r\n") {
        "\r\n"
    } else if line.ends_with('\n') {
        "\n"
    } else {
        ""
    }
}

fn line_structural_prefix_len(content: &str) -> usize {
    let indentation = content.len() - content.trim_start().len();
    let rest = &content[indentation..];
    if rest.starts_with("- ") || rest.starts_with("* ") {
        return indentation + 2;
    }
    if rest.starts_with("• ") {
        return indentation + "• ".len();
    }
    let digits = rest.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digits > 0
        && rest
            .get(digits..)
            .is_some_and(|suffix| suffix.starts_with(". ") || suffix.starts_with(") "))
    {
        return indentation + digits + 2;
    }
    indentation
}

fn collect_line_spoken_filler_deletions(
    src_body: &str,
    cand_body: &str,
) -> Option<Vec<Range<usize>>> {
    // Strip curated spoken filler tokens/phrases from src_body step-by-step only
    // when they match deleted spans needed to produce cand_body, then validate
    // the stripped source line against cand_body via safe_view so line-terminal
    // questions and unsafe punctuation remain rejected while safe punctuation
    // additions (e.g. "。" or inline "，") remain allowed.
    if cand_body.is_empty() || src_body == cand_body {
        return None;
    }
    const INLINE_FILLERS: &[&str] = &[
        "basically",
        "actually",
        "honestly",
        "literally",
        "you know",
        "i mean",
        "um",
        "uh",
        "er",
        "ah",
        "hmm",
        "erm",
        "那个",
        "这个",
        "就是说",
        "其实",
        "然后",
        "那么",
        "嗯",
        "啊",
        "呃",
        "额",
        "bueno",
        "pues",
        "entonces",
        "alors",
        "euh",
        "ben",
        "ähm",
        "äh",
        "also",
        "halt",
        "allora",
        "cioè",
        "tipo",
        "então",
        "nou",
        "dus",
        "えっと",
        "あの",
        "まあ",
        "なんか",
        "음",
        "어",
    ];
    fn requires_pause_delimiter_for_inline_filler(filler: &str) -> bool {
        matches!(
            filler,
            "also"
                | "halt"
                | "er"
                | "ben"
                | "bueno"
                | "pues"
                | "entonces"
                | "alors"
                | "allora"
                | "tipo"
                | "então"
                | "nou"
                | "dus"
                | "那么"
                | "然后"
                | "其实"
        )
    }
    const DELETION_BACKTRACK_LIMIT: u8 = 4;
    let literals = crate::spoken_revision::literal_ranges(src_body);
    let find_filler = |f_start: usize| -> Option<(Range<usize>, usize)> {
        let rest_lower = src_body[f_start..].to_lowercase();
        for filler in INLINE_FILLERS {
            let Some(after_filler) = src_body.get(f_start + filler.len()..) else {
                continue;
            };
            let matched = if is_latin_word_filler(filler) {
                let lower_filler = filler.to_lowercase();
                rest_lower.starts_with(&lower_filler)
                    && after_filler
                        .chars()
                        .next()
                        .is_none_or(|ch| !ch.is_alphanumeric() && ch != '_')
                    && src_body[..f_start]
                        .chars()
                        .next_back()
                        .is_none_or(|ch| !ch.is_alphanumeric() && ch != '_')
            } else {
                src_body[f_start..].starts_with(filler)
            };
            if !matched {
                continue;
            }
            let f_end = f_start + filler.len();
            if literals.iter().any(|r| r.start < f_end && f_start < r.end) {
                continue;
            }
            if is_kana_or_hangul_filler(filler)
                && (src_body[..f_start]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_alphanumeric)
                    || after_filler
                        .chars()
                        .next()
                        .is_some_and(char::is_alphanumeric))
            {
                continue;
            }
            if !filler.is_ascii()
                && (src_body[..f_start].chars().next_back().is_some_and(is_cjk)
                    || (*filler == "额" && after_filler.chars().next().is_some_and(is_cjk))
                    || (*filler == "那么"
                        && matches!(
                            after_filler.chars().next(),
                            Some(
                                '多' | '大'
                                    | '小'
                                    | '少'
                                    | '高'
                                    | '低'
                                    | '长'
                                    | '短'
                                    | '快'
                                    | '慢'
                                    | '好'
                                    | '坏'
                                    | '贵'
                                    | '远'
                                    | '近'
                                    | '早'
                                    | '晚'
                                    | '难'
                                    | '久'
                                    | '深'
                                    | '重'
                                    | '新'
                                    | '旧'
                                    | '些'
                                    | '点'
                                    | '样'
                                    | '美'
                                    | '强'
                                    | '弱'
                                    | '巧'
                                    | '准'
                                    | '稳'
                            )
                        )))
            {
                continue;
            }
            let capitalized_line_initial = src_body[..f_start].trim().is_empty()
                && src_body[f_start..f_end]
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_uppercase());
            if capitalized_line_initial
                && !matches!(
                    src_body[f_end..].chars().next(),
                    Some(',' | '，' | '、' | ';' | '；' | ':' | '：')
                )
            {
                continue;
            }
            if requires_pause_delimiter_for_inline_filler(filler)
                && !matches!(
                    after_filler.trim_start().chars().next(),
                    Some(',' | '，' | '、' | ';' | '；' | ':' | '：')
                )
            {
                continue;
            }
            let trail_glue = src_body[f_end..]
                .chars()
                .take_while(|ch| ch.is_whitespace() || matches!(ch, ',' | '，' | '、'))
                .map(char::len_utf8)
                .sum::<usize>();
            return Some((f_start..f_end, f_end + trail_glue));
        }
        None
    };

    struct FillerWalkEnv<'a> {
        src_body: &'a str,
        cand_body: &'a str,
        find_filler: &'a dyn Fn(usize) -> Option<(Range<usize>, usize)>,
    }

    fn apply_filler_step(
        env: &FillerWalkEnv<'_>,
        s: &mut usize,
        c: &mut usize,
        stripped_src: &mut String,
        deleted_ranges: &mut Vec<Range<usize>>,
        filler_range: Range<usize>,
        next_s: usize,
    ) {
        if env.cand_body[*c..].starts_with(' ')
            && env.cand_body[..*c]
                .chars()
                .next_back()
                .is_some_and(|ch| !ch.is_whitespace())
            && env.src_body[*s..next_s].chars().any(char::is_whitespace)
            && env.cand_body[*c + 1..]
                .chars()
                .next()
                .is_some_and(|ch| !ch.is_whitespace())
        {
            *c += 1;
            stripped_src.push(' ');
        }
        deleted_ranges.push(filler_range);
        *s = next_s;
    }

    fn walk(
        env: &FillerWalkEnv<'_>,
        mut s: usize,
        mut c: usize,
        backtracks_remaining: u8,
        mut deleted_ranges: Vec<Range<usize>>,
        mut stripped_src: String,
    ) -> Option<Vec<Range<usize>>> {
        let FillerWalkEnv {
            src_body,
            cand_body,
            find_filler,
        } = *env;
        while s < src_body.len() || c < cand_body.len() {
            if let (Some(sc), Some(cc)) =
                (src_body[s..].chars().next(), cand_body[c..].chars().next())
            {
                if sc == cc {
                    // If a delimited filler shares its first character with the
                    // kept continuation (e.g. "额，额外注意安全" -> "额外注意安全"),
                    // try stripping the filler first before greedily consuming `sc`.
                    if backtracks_remaining > 0 {
                        if let Some((filler_range, next_s)) = find_filler(s) {
                            if next_s > filler_range.end
                                && !cand_body[c..].starts_with(&src_body[s..next_s])
                            {
                                let mut bs = s;
                                let mut bc = c;
                                let mut b_stripped = stripped_src.clone();
                                let mut b_deleted = deleted_ranges.clone();
                                apply_filler_step(
                                    env,
                                    &mut bs,
                                    &mut bc,
                                    &mut b_stripped,
                                    &mut b_deleted,
                                    filler_range,
                                    next_s,
                                );
                                if let Some(found) = walk(
                                    env,
                                    bs,
                                    bc,
                                    backtracks_remaining - 1,
                                    b_deleted,
                                    b_stripped,
                                ) {
                                    return Some(found);
                                }
                            }
                        }
                    }
                    s += sc.len_utf8();
                    c += cc.len_utf8();
                    stripped_src.push(sc);
                    continue;
                }
            }
            // Optional ASCII sentence-initial capitalization fold right at a filler boundary
            if !deleted_ranges.is_empty() {
                if let (Some(sc), Some(cc)) = (
                    src_body[s..]
                        .chars()
                        .next()
                        .filter(|ch| ch.is_ascii_lowercase()),
                    cand_body[c..]
                        .chars()
                        .next()
                        .filter(|ch| ch.is_ascii_uppercase()),
                ) {
                    if sc.eq_ignore_ascii_case(&cc) {
                        s += sc.len_utf8();
                        c += cc.len_utf8();
                        stripped_src.push(cc);
                        continue;
                    }
                }
            }
            // Check if src_body[s..] starts with a curated inline filler + optional comma/space glue
            let skip_ws = src_body[s..]
                .chars()
                .take_while(|ch| ch.is_whitespace() || matches!(ch, ',' | '，' | '、'))
                .map(char::len_utf8)
                .sum::<usize>();
            if let Some((filler_range, next_s)) = find_filler(s + skip_ws) {
                apply_filler_step(
                    env,
                    &mut s,
                    &mut c,
                    &mut stripped_src,
                    &mut deleted_ranges,
                    filler_range,
                    next_s,
                );
                continue;
            }
            // Allow candidate safe comma/period additions or trailing source period removal
            // to be adjudicated by safe_view + punctuation_changes_are_one_sided_inner.
            if !deleted_ranges.is_empty() {
                if let Some(cc) = cand_body[c..].chars().next() {
                    if matches!(cc, ',' | '，')
                        && !crate::protected_span::contains_negation(
                            cand_body[..c]
                                .split_whitespace()
                                .last()
                                .unwrap_or(&cand_body[..c]),
                        )
                    {
                        c += cc.len_utf8();
                        continue;
                    }
                    if matches!(cc, '.' | '。')
                        && cand_body[c + cc.len_utf8()..].is_empty()
                        && s == src_body.len()
                    {
                        c += cc.len_utf8();
                        continue;
                    }
                }
                if let Some(sc) = src_body[s..].chars().next() {
                    if matches!(sc, '.' | '。')
                        && src_body[s + sc.len_utf8()..].is_empty()
                        && c == cand_body.len()
                    {
                        s += sc.len_utf8();
                        stripped_src.push(sc);
                        continue;
                    }
                }
            }
            return None;
        }
        if deleted_ranges.is_empty() {
            return None;
        }
        let sv = safe_view(&stripped_src)?;
        let cv = safe_view(cand_body)?;
        (sv.skeleton == cv.skeleton
            && punctuation_changes_are_one_sided_inner(&sv.punctuation, &cv.punctuation, true))
        .then_some(deleted_ranges)
    }

    let env = FillerWalkEnv {
        src_body,
        cand_body,
        find_filler: &find_filler,
    };
    walk(
        &env,
        0,
        0,
        DELETION_BACKTRACK_LIMIT,
        Vec::new(),
        String::with_capacity(src_body.len()),
    )
}

fn exact_literal_ranges(source: &str, candidate: &str) -> bool {
    let source_literals = crate::spoken_revision::literal_ranges(source);
    let candidate_literals = crate::spoken_revision::literal_ranges(candidate);
    source_literals.len() == candidate_literals.len()
        && source_literals
            .iter()
            .zip(candidate_literals)
            .all(|(source_range, candidate_range)| {
                source.get(source_range.clone()) == candidate.get(candidate_range)
            })
}

fn exact_code_regions(source: &str, candidate: &str) -> bool {
    let Some(regions) = crate::protected_span::validated_code_region_pairs(source, candidate)
    else {
        return false;
    };
    regions.into_iter().all(|(source_range, candidate_range)| {
        source.get(source_range) == candidate.get(candidate_range)
    })
}

fn overlaps(ranges: &[Range<usize>], range: Range<usize>) -> bool {
    ranges
        .iter()
        .any(|other| range.start < other.end && other.start < range.end)
}

fn overlaps_protected(
    protected: &[crate::protected_span::ProtectedSpan],
    range: Range<usize>,
) -> bool {
    protected
        .iter()
        .any(|span| range.start < span.end_byte && span.start_byte < range.end)
}

fn is_word_character(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn is_cjk(ch: char) -> bool {
    matches!(ch as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

fn is_latin_word_filler(filler: &str) -> bool {
    filler.chars().any(char::is_alphabetic)
        && filler.chars().all(|ch| {
            (ch.is_alphanumeric() || ch.is_whitespace())
                && !is_cjk_character(ch)
                && !is_hangul_character(ch)
        })
}

fn is_kana_or_hangul_filler(filler: &str) -> bool {
    filler
        .chars()
        .all(|ch| is_kana_character(ch) || is_hangul_character(ch))
}

fn is_kana_character(ch: char) -> bool {
    matches!(ch as u32, 0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9D)
}

fn is_hangul_character(ch: char) -> bool {
    matches!(ch as u32, 0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF)
}

#[cfg(test)]
mod tests {
    use super::{
        preserves_line_structure_with_inline_filler_cleanup, preserves_model_selected_paragraphs,
        preserves_structure_safe,
    };

    #[test]
    fn model_selected_paragraphs_require_contiguous_source_sentence_segments() {
        let source = "The release is ready. We should review the docs tomorrow.";
        assert!(preserves_model_selected_paragraphs(
            source,
            "The release is ready.\n\nWe should review the docs tomorrow."
        ));
        assert!(!preserves_model_selected_paragraphs(
            source,
            "The release is ready.\n\nWe should review the API tomorrow."
        ));
        assert!(!preserves_model_selected_paragraphs(
            source,
            "We should review the docs tomorrow.\n\nThe release is ready."
        ));
        assert!(!preserves_model_selected_paragraphs(
            "We should review the docs tomorrow because the release is ready.",
            "We should review the docs tomorrow.\n\nBecause the release is ready."
        ));
    }

    #[test]
    fn model_selected_paragraphs_reject_abbreviations_initials_and_added_labels() {
        for (source, candidate) in [
            (
                "Please ask Dr. Smith tomorrow about the release.",
                "Please ask Dr.\n\nSmith tomorrow about the release.",
            ),
            (
                "J. Smith approved the release. The review starts tomorrow.",
                "J.\n\nSmith approved the release. The review starts tomorrow.",
            ),
            (
                "The date is 2026. 11/14 is the planned launch. Please review tomorrow.",
                "The date is 2026.\n\n11/14 is the planned launch. Please review tomorrow.",
            ),
            (
                "Use `v1.2` for the API. Then call me.",
                "Use `v1.\n\n2` for the API. Then call me.",
            ),
            (
                "The release is ready. The review starts tomorrow.",
                "The release is ready.\n\n- The review starts tomorrow.",
            ),
            (
                "The release is ready. The review starts tomorrow.",
                "The release is ready.\n\nSummary. The review starts tomorrow.",
            ),
        ] {
            assert!(
                !preserves_model_selected_paragraphs(source, candidate),
                "source={source:?} candidate={candidate:?}"
            );
        }
    }

    #[test]
    fn model_selected_paragraphs_allow_unambiguous_cjk_zero_width_sentence_breaks() {
        assert!(preserves_model_selected_paragraphs(
            "第一主题。第二主题。",
            "第一主题。\n\n第二主题。"
        ));
        assert!(preserves_model_selected_paragraphs(
            "第一主题。第二主题还没结束",
            "第一主题。\n\n第二主题还没结束"
        ));
        assert!(preserves_model_selected_paragraphs(
            "She said “Done.” We should review the docs",
            "She said “Done.”\n\nWe should review the docs"
        ));
        assert!(!preserves_model_selected_paragraphs(
            "我们讨论第一主题第二主题。",
            "我们讨论第一主题。\n\n第二主题。"
        ));
    }

    #[test]
    fn accepts_unchanged_crlf_blank_lines_and_terminal_newline() {
        for source in [
            "\r\n- first\r\n- second\r\n\r\n",
            "\n1. first\n2. second\n",
            "first\r\n\r\nsecond\r\n",
        ] {
            assert!(preserves_structure_safe(source, source));
        }
    }

    #[test]
    fn accepts_only_safe_prose_punctuation_changes() {
        assert!(preserves_structure_safe("Done.", "Done。"));
        assert!(preserves_structure_safe(
            "We can ship this today",
            "We can ship this today."
        ));
        assert!(preserves_structure_safe(
            "We can ship this today.",
            "We can ship this today"
        ));
        assert!(preserves_structure_safe("你好。明天继续", "你好.明天继续"));
    }

    #[test]
    fn rejects_inline_punctuation_edits_that_can_change_meaning() {
        assert!(!preserves_structure_safe(
            "No refunds.\nKeep records.",
            "No, refunds.\nKeep records."
        ));
        assert!(!preserves_structure_safe(
            "Use the same rule.",
            "Use the same. rule."
        ));
        assert!(!preserves_structure_safe(
            "First item. Second item.",
            "First item Second item."
        ));
        assert!(!preserves_structure_safe("No refunds.", "No refunds,"));
    }

    #[test]
    fn does_not_add_or_remove_a_period_from_question_like_lines() {
        assert!(!preserves_structure_safe("Can we ship", "Can we ship."));
        assert!(!preserves_structure_safe("Can we ship.", "Can we ship"));
        assert!(!preserves_structure_safe("你好吗", "你好吗。"));
        assert!(!preserves_structure_safe("你好吗。", "你好吗"));
    }

    #[test]
    fn rejects_whitespace_line_ending_and_boundary_changes() {
        assert!(!preserves_structure_safe(
            "first\r\nsecond",
            "first\nsecond"
        ));
        assert!(!preserves_structure_safe(
            "first\n\nsecond",
            "first\nsecond"
        ));
        assert!(!preserves_structure_safe(
            "- first\n- second",
            "- first - second"
        ));
        assert!(!preserves_structure_safe(" text", "text"));
        assert!(!preserves_structure_safe("\ntext", "text"));
        assert!(!preserves_structure_safe("text\n", "text"));
    }

    #[test]
    fn preserves_question_meaning_and_emphatic_punctuation() {
        assert!(preserves_structure_safe("Can we ship?", "Can we ship?"));
        assert!(!preserves_structure_safe("Can we ship?", "Can we ship."));
        assert!(!preserves_structure_safe("Can we ship?", "Can we ship"));
        assert!(!preserves_structure_safe(
            "Can we ship? Then we wait.",
            "Can we ship. Then we wait?"
        ));
        assert!(!preserves_structure_safe(
            "That is great!",
            "That is great."
        ));
        assert!(!preserves_structure_safe("Wait…", "Wait."));
    }

    #[test]
    fn preserves_words_case_and_meaningful_punctuation() {
        assert!(!preserves_structure_safe(
            "Ship the patch.",
            "Ship a patch."
        ));
        assert!(!preserves_structure_safe(
            "Ship the patch.",
            "ship the patch."
        ));
        assert!(!preserves_structure_safe("Keep A-B.", "Keep A B."));
        assert!(!preserves_structure_safe(
            "Keep key: value.",
            "Keep key value."
        ));
        assert!(!preserves_structure_safe(
            "First; second.",
            "First, second."
        ));
    }

    #[test]
    fn keeps_numbered_and_bullet_prefixes_exact() {
        assert!(!preserves_structure_safe(
            "1. first\n2. second",
            "1 first\n2 second"
        ));
        assert!(!preserves_structure_safe(
            "- first\n- second",
            "* first\n* second"
        ));
    }

    #[test]
    fn keeps_protected_numbers_code_and_quotes_exact() {
        assert!(!preserves_structure_safe(
            "Use 1,000 items.",
            "Use 1000 items."
        ));
        assert!(!preserves_structure_safe(
            "Use `count=30`.",
            "Use `count=31`."
        ));
        assert!(!preserves_structure_safe(
            "Say “go?” now.",
            "Say “go.” now."
        ));
        assert!(!preserves_structure_safe(
            "```text\nvalue = 1;\n```\nDone.",
            "```text\nvalue = 1,\n```\nDone."
        ));
    }

    #[test]
    fn allows_inline_filler_cleanup_inside_structured_lists_and_letters() {
        assert!(preserves_structure_safe(
            "- 那个，今天上线\n- 嗯，明天复盘",
            "- 今天上线\n- 明天复盘"
        ));
        assert!(preserves_structure_safe(
            "Hi Alice,\nUm, basically we ship.",
            "Hi Alice,\nWe ship."
        ));
        assert!(preserves_structure_safe(
            "I know, actually, it works.",
            "I know it works."
        ));
        // Non-filler edits stay rejected
        assert!(!preserves_structure_safe(
            "- Ship the patch\n- Wait",
            "- Ship a patch\n- Wait"
        ));
        // Equal-byte period normalization, comma punctuation normalization, and indented list bullets
        assert!(preserves_structure_safe("- 嗯今天上线", "- 今天上线。"));
        assert!(!preserves_structure_safe(
            "- 嗯今天上线吗",
            "- 今天上线吗。"
        ));
        assert!(preserves_structure_safe(
            "- 那个今天上线明天复盘",
            "- 今天上线，明天复盘。"
        ));
        assert!(preserves_structure_safe("  - um ship it", "  - Ship it."));
        assert!(preserves_structure_safe("• um ship it", "• Ship it."));
        assert!(preserves_structure_safe("  • um ship it", "  • Ship it."));
        assert!(preserves_structure_safe("- Äh, ship it", "- Ship it."));
        assert!(preserves_structure_safe("- 음, 배포해", "- 배포해."));
        assert!(preserves_structure_safe("- あの、今日出発", "- 今日出発。"));
        assert!(preserves_line_structure_with_inline_filler_cleanup(
            "• um ship it",
            "• Ship it."
        ));
        // `就是` is lexical content in this phrase, not a detachable filler.
        assert!(!preserves_structure_safe(
            "- 这就是问题根源",
            "- 这问题根源。"
        ));
        assert!(!preserves_structure_safe(
            "- 那个问题要复盘",
            "- 问题要复盘。"
        ));
        assert!(!preserves_structure_safe(
            "- 음식이 식어요",
            "- 식이 식어요."
        ));
        assert!(!preserves_structure_safe(
            "- ähnlich wie morgen",
            "- nlich wie morgen."
        ));
        assert!(!preserves_structure_safe("- 어제 배포해", "- 제 배포해."));
        assert!(!preserves_structure_safe("- あの人が来る", "- 人が来る。"));
        assert!(preserves_structure_safe("- 嗯今天上线", "- 今天上线。"));
        assert!(preserves_structure_safe(
            "- 那个今天上线明天复盘",
            "- 今天上线明天复盘。"
        ));
        // Lexical Chinese words starting with filler syllables stay protected.
        assert!(!preserves_structure_safe("- 额外注意安全", "- 外注意安全"));
        assert!(!preserves_structure_safe("- 那么多问题", "- 问题"));
        assert!(preserves_structure_safe(
            "- 额，额外注意安全",
            "- 额外注意安全"
        ));
        // Punctuation-only normalization must keep sentence-initial terms intact.
        assert!(preserves_structure_safe(
            "- Actually, we ship",
            "- Actually, we ship."
        ));
        assert!(preserves_structure_safe("- First step", "- First step."));
        assert!(preserves_structure_safe("- Um, we ship", "- Um, we ship."));
        // Content hedges and unpunctuated ambiguous cross-lingual connectors stay protected.
        assert!(!preserves_structure_safe(
            "- We also shipped v2",
            "- We shipped v2."
        ));
        assert!(!preserves_structure_safe(
            "- We must halt deployment",
            "- We must deployment."
        ));
        assert!(!preserves_structure_safe(
            "- It is sort of broken",
            "- It is broken."
        ));
        assert!(!preserves_structure_safe("- 那么做不对", "- 做不对。"));
        assert!(!preserves_structure_safe(
            "- 先测试，然后上线",
            "- 先测试，上线。"
        ));
    }
}
