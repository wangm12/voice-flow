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
    if !crate::spoken_layout::preserves_required_layout(source, candidate)
        || !crate::protected_span::preserves(
            source,
            candidate,
            Some(crate::llm::CleanupOperation::Cleanup),
        )
        || !exact_protected_span_values(source, candidate)
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

    source_view.skeleton == candidate_view.skeleton
        && punctuation_changes_are_one_sided(&source_view.punctuation, &candidate_view.punctuation)
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

    let mut skeleton = String::with_capacity(text.len());
    let mut punctuation = Vec::new();
    for (offset, ch) in text.char_indices() {
        let range = offset..offset + ch.len_utf8();
        if let Some(event) = eligible_punctuation(
            text,
            offset,
            ch,
            skeleton.len(),
            &literals,
            &code_regions,
            &protected,
            &list_prefixes,
        ) {
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
    literals: &[Range<usize>],
    code_regions: &[Range<usize>],
    protected: &[crate::protected_span::ProtectedSpan],
    list_prefixes: &[Range<usize>],
) -> Option<PunctuationEvent> {
    let range = offset..offset + ch.len_utf8();
    if overlaps(literals, range.clone())
        || overlaps(code_regions, range.clone())
        || overlaps_protected(protected, range.clone())
        || overlaps(list_prefixes, range)
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
                    && line_is_question_like(text, offset, list_prefixes),
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

fn punctuation_changes_are_one_sided(
    source: &[PunctuationEvent],
    candidate: &[PunctuationEvent],
) -> bool {
    let source_inline = source
        .iter()
        .filter(|event| !event.line_terminal)
        .collect::<Vec<_>>();
    let candidate_inline = candidate
        .iter()
        .filter(|event| !event.line_terminal)
        .collect::<Vec<_>>();
    if source_inline != candidate_inline {
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

fn exact_protected_span_values(source: &str, candidate: &str) -> bool {
    let source_spans =
        crate::protected_span::protected_spans(source, Some(crate::llm::CleanupOperation::Cleanup));
    let candidate_spans = crate::protected_span::protected_spans(
        candidate,
        Some(crate::llm::CleanupOperation::Cleanup),
    );
    source_spans.len() == candidate_spans.len()
        && source_spans
            .iter()
            .zip(candidate_spans)
            .all(|(source_span, candidate_span)| {
                source_span.kind == candidate_span.kind
                    && source.get(source_span.start_byte..source_span.end_byte)
                        == candidate.get(candidate_span.start_byte..candidate_span.end_byte)
            })
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

#[cfg(test)]
mod tests {
    use super::preserves_structure_safe;

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
}
