//! Conservative, source-offset-only layout projection for cleanup input.
//!
//! Callers decide route, field eligibility, and scene/manual precedence. This
//! module only renders explicit source-backed separators and list prefixes;
//! it does not change words, infer content, or persist a projection.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputLayoutMode {
    None,
    Paragraph,
    List,
    Email,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectedStructure {
    Unchanged,
    Paragraph,
    List,
    Email,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceEdit {
    /// Byte range in the supplied source. Empty ranges represent insertions.
    pub(crate) source_range: Range<usize>,
    pub(crate) replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CleanupLayoutProjection {
    pub(crate) text: String,
    pub(crate) structure: ProjectedStructure,
    pub(crate) edits: Vec<SourceEdit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectionError {
    InvalidSourceEdit,
}

/// Project a cleanup input after the caller has established that the target
/// is a multiline-capable prose field and selected a layout mode.
///
/// `source` must be the canonical prepared text with spoken/user-supplied
/// structure preserved and scene-only inferred structure suppressed. Existing
/// line/list structure is always left intact. The returned edits are ephemeral
/// source-offset evidence and do not authorize edits to the canonical source.
pub(crate) fn project_cleanup_layout(
    source: &str,
    requested: OutputLayoutMode,
) -> Result<CleanupLayoutProjection, ProjectionError> {
    if requested == OutputLayoutMode::None
        || source.trim().is_empty()
        || source.contains('`')
        || crate::spoken_layout::has_structural_layout(source)
    {
        return Ok(unchanged(source));
    }

    let literal_ranges = crate::spoken_revision::literal_ranges(source);
    let protected =
        crate::protected_span::protected_spans(source, Some(crate::llm::CleanupOperation::Cleanup));
    let edits = match requested {
        OutputLayoutMode::None => Vec::new(),
        OutputLayoutMode::Paragraph => paragraph_edits(source, &literal_ranges, &protected),
        OutputLayoutMode::List => list_edits(source, &literal_ranges, &protected),
        OutputLayoutMode::Email => email_edits(source, &literal_ranges, &protected),
    };
    if edits.is_empty() {
        return Ok(unchanged(source));
    }

    let text = apply_source_edits(source, &edits)?;
    Ok(CleanupLayoutProjection {
        text,
        structure: match requested {
            OutputLayoutMode::Paragraph => ProjectedStructure::Paragraph,
            OutputLayoutMode::List => ProjectedStructure::List,
            OutputLayoutMode::Email => ProjectedStructure::Email,
            OutputLayoutMode::None => ProjectedStructure::Unchanged,
        },
        edits,
    })
}

fn unchanged(source: &str) -> CleanupLayoutProjection {
    CleanupLayoutProjection {
        text: source.to_owned(),
        structure: ProjectedStructure::Unchanged,
        edits: Vec::new(),
    }
}

fn paragraph_edits(
    source: &str,
    literals: &[Range<usize>],
    protected: &[crate::protected_span::ProtectedSpan],
) -> Vec<SourceEdit> {
    // These source phrases explicitly announce a purpose/topic transition.
    // Both sides must end as complete sentences; no heading or summary is made.
    const TRANSITIONS: &[&str] = &[
        "Separately,",
        "On another topic,",
        "On a separate topic,",
        "另外，",
        "另外,",
        "另一方面，",
        "至于",
    ];

    let ascii_lower = source.to_ascii_lowercase();
    let mut edits = Vec::new();
    for transition in TRANSITIONS {
        let needle = transition.to_ascii_lowercase();
        let mut search_from = 0;
        while let Some(relative) = ascii_lower[search_from..].find(&needle) {
            let start = search_from + relative;
            let end = start + transition.len();
            search_from = end;
            if overlaps_literal(literals, start..end) {
                continue;
            }

            let left = source[..start].trim_end();
            let gap_start = left.len();
            if !source[gap_start..start].chars().all(char::is_whitespace)
                || !left.chars().next_back().is_some_and(is_sentence_end)
            {
                continue;
            }
            let right = source[end..].trim();
            if right.is_empty()
                || !right.chars().next_back().is_some_and(is_sentence_end)
                || overlaps_literal(literals, gap_start..end)
                || overlaps_protected(protected, gap_start..start)
            {
                continue;
            }
            edits.push(SourceEdit {
                source_range: gap_start..start,
                replacement: "\n\n".to_owned(),
            });
        }
    }
    edits.sort_by_key(|edit| edit.source_range.start);
    non_overlapping(edits)
}

fn list_edits(
    source: &str,
    literals: &[Range<usize>],
    protected: &[crate::protected_span::ProtectedSpan],
) -> Vec<SourceEdit> {
    let lead_in = shared_list_lead_in(source, literals);
    let (body_start, exact_count) = lead_in.unwrap_or((0, None));
    let body = &source[body_start..];
    if body.contains('?') || body.contains('？') || contains_disjunction(body) {
        return Vec::new();
    }
    if body.trim_end().ends_with(';') || body.trim_end().ends_with('；') {
        return Vec::new();
    }

    let separators = list_separators(source, body_start, literals, protected);
    if separators.is_empty() {
        return Vec::new();
    }
    let mut item_ranges = Vec::with_capacity(separators.len() + 1);
    let mut cursor = body_start;
    for separator in &separators {
        item_ranges.push(trimmed_range(source, cursor..separator.start));
        cursor = separator.end;
    }
    item_ranges.push(trimmed_range(source, cursor..source.len()));
    if item_ranges.iter().any(Range::is_empty)
        || exact_count.is_some_and(|count| count != item_ranges.len())
        || (lead_in.is_none() && contains_unparsed_count_cue(source))
        || !has_same_independent_item_shape(source, &item_ranges)
    {
        return Vec::new();
    }
    let Some(last_item) = item_ranges
        .last()
        .and_then(|range| source.get(range.clone()))
    else {
        return Vec::new();
    };
    if has_trailing_clause_or_shared_predicate(last_item) {
        return Vec::new();
    }

    // Without a shared lead-in, require each semicolon-delimited item to begin
    // with a known imperative verb. Delimiters plus parallel imperative syntax
    // are required; repeated wording alone is never enough.
    if lead_in.is_none()
        && !item_ranges
            .iter()
            .filter_map(|range| source.get(range.clone()))
            .all(starts_with_supported_action)
    {
        return Vec::new();
    }

    let mut edits = Vec::with_capacity(item_ranges.len());
    for (index, item) in item_ranges.iter().enumerate() {
        let range = if index == 0 {
            body_start..item.start
        } else {
            separators[index - 1].start..item.start
        };
        if overlaps_literal(literals, range.clone()) || overlaps_protected(protected, range.clone())
        {
            return Vec::new();
        }
        edits.push(SourceEdit {
            source_range: range,
            replacement: if index == 0 && lead_in.is_none() {
                "- ".to_owned()
            } else {
                "\n- ".to_owned()
            },
        });
    }
    edits
}

fn email_edits(
    source: &str,
    literals: &[Range<usize>],
    protected: &[crate::protected_span::ProtectedSpan],
) -> Vec<SourceEdit> {
    let ranges = crate::spoken_layout::email_layout_separator_ranges(source);
    if ranges.is_empty() {
        return Vec::new();
    }
    let edits = ranges
        .into_iter()
        .map(|source_range| SourceEdit {
            source_range,
            replacement: "\n".to_owned(),
        })
        .collect::<Vec<_>>();
    if edits.iter().any(|edit| {
        edit.source_range.start > edit.source_range.end
            || edit.source_range.end > source.len()
            || !source.is_char_boundary(edit.source_range.start)
            || !source.is_char_boundary(edit.source_range.end)
            || !source[edit.source_range.clone()]
                .chars()
                .all(char::is_whitespace)
            || overlaps_literal(literals, edit.source_range.clone())
            || overlaps_protected(protected, edit.source_range.clone())
    }) {
        return Vec::new();
    }
    non_overlapping(edits)
}

/// Recognize a small set of colon-terminated shared lead-ins. Any count in the
/// lead-in is a hard exact-count constraint; unsupported count forms are not
/// allowed to fall through to the parallel-action recognizer.
fn shared_list_lead_in(source: &str, literals: &[Range<usize>]) -> Option<(usize, Option<usize>)> {
    const LEAD_INS: &[(&str, Option<usize>)] = &[
        ("the two tasks are:", Some(2)),
        ("the three tasks are:", Some(3)),
        ("the four tasks are:", Some(4)),
        ("there are two tasks:", Some(2)),
        ("there are three tasks:", Some(3)),
        ("there are four tasks:", Some(4)),
        ("the two steps are:", Some(2)),
        ("the three steps are:", Some(3)),
        ("the four steps are:", Some(4)),
        ("the two actions are:", Some(2)),
        ("the three actions are:", Some(3)),
        ("the four actions are:", Some(4)),
        ("the tasks are:", None),
        ("the steps are:", None),
        ("the actions are:", None),
        ("the items are:", None),
        ("here are the tasks:", None),
        ("here are the steps:", None),
        ("以下两项：", Some(2)),
        ("以下三项：", Some(3)),
        ("以下四项：", Some(4)),
        ("这两项是：", Some(2)),
        ("这三项是：", Some(3)),
        ("这四项是：", Some(4)),
        ("以下任务：", None),
        ("下面的步骤：", None),
    ];
    let ascii_lower = source.to_ascii_lowercase();
    LEAD_INS
        .iter()
        .filter_map(|(lead_in, exact_count)| {
            let start = ascii_lower.find(lead_in)?;
            let end = start + lead_in.len();
            if overlaps_literal(literals, start..end)
                || !is_sentence_start(source, start)
                || !source[end..]
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_whitespace() || lead_in.ends_with('：'))
            {
                return None;
            }
            Some((end, *exact_count))
        })
        .min_by_key(|(end, _)| *end)
}

fn list_separators(
    source: &str,
    start: usize,
    literals: &[Range<usize>],
    protected: &[crate::protected_span::ProtectedSpan],
) -> Vec<Range<usize>> {
    source[start..]
        .char_indices()
        .filter_map(|(relative, ch)| {
            if !matches!(ch, ';' | '；') {
                return None;
            }
            let at = start + relative;
            let end = at + ch.len_utf8();
            if overlaps_literal(literals, at..end) || overlaps_protected(protected, at..end) {
                return None;
            }
            let next = skip_whitespace(source, end);
            (next < source.len()).then_some(at..next)
        })
        .collect()
}

fn is_sentence_start(source: &str, start: usize) -> bool {
    let prefix = source[..start].trim_end();
    prefix.is_empty() || prefix.chars().next_back().is_some_and(is_sentence_end)
}

fn is_sentence_end(ch: char) -> bool {
    matches!(ch, '.' | '!' | '?' | '。' | '！' | '？')
}

fn contains_disjunction(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|word| matches!(word, "or" | "either"))
        || ["或者", "要么", "还是"]
            .iter()
            .any(|word| text.contains(word))
}

fn contains_unparsed_count_cue(text: &str) -> bool {
    const ENGLISH_COUNTS: &[&str] = &[
        "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "2", "3", "4", "5",
        "6", "7", "8", "9", "10",
    ];
    const NOUNS: &[&str] = &[
        "task", "tasks", "step", "steps", "action", "actions", "item", "items", "point", "points",
        "thing", "things",
    ];
    let lower = text.to_ascii_lowercase();
    if ENGLISH_COUNTS.iter().any(|count| {
        NOUNS
            .iter()
            .any(|noun| lower.contains(&format!("{count} {noun}")))
    }) {
        return true;
    }
    [
        "两项",
        "三项",
        "四项",
        "五项",
        "两个任务",
        "三个任务",
        "四个任务",
        "五个任务",
        "两步",
        "三步",
        "四步",
        "五步",
        "两件事",
        "三件事",
        "四件事",
        "五件事",
    ]
    .iter()
    .any(|cue| text.contains(cue))
}

fn has_same_independent_item_shape(source: &str, ranges: &[Range<usize>]) -> bool {
    #[derive(PartialEq, Eq)]
    enum Shape {
        Imperative,
        Fragment(usize),
    }

    let mut expected_shape = None;
    for range in ranges {
        let Some(item) = source.get(range.clone()) else {
            return false;
        };
        if starts_with_serial_cue(item)
            || contains_clause_predicate(item)
            || has_intermediate_sentence_end(item)
        {
            return false;
        }
        let shape = if begins_with_supported_imperative(item) {
            Shape::Imperative
        } else {
            Shape::Fragment(item.split_whitespace().count())
        };
        if expected_shape
            .as_ref()
            .is_some_and(|expected| expected != &shape)
        {
            return false;
        }
        expected_shape = Some(shape);
    }
    expected_shape.is_some()
}

fn starts_with_serial_cue(item: &str) -> bool {
    let lower = item.to_ascii_lowercase();
    [
        "then ",
        "and then ",
        "next ",
        "after that ",
        "subsequently ",
    ]
    .iter()
    .any(|cue| lower.starts_with(cue))
        || ["然后", "接着", "随后", "下一步", "再"]
            .iter()
            .any(|cue| item.starts_with(cue))
}

fn contains_clause_predicate(item: &str) -> bool {
    const FINITE_PREDICATES: &[&str] = &[
        "is", "are", "was", "were", "has", "have", "does", "do", "will", "would", "should", "must",
        "can", "could", "may", "might", "need", "needs", "required",
    ];
    let lower = item.to_ascii_lowercase();
    if lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|word| FINITE_PREDICATES.contains(&word))
    {
        return true;
    }
    [
        "是", "为", "需要", "必须", "应该", "可以", "能够", "都要", "都需",
    ]
    .iter()
    .any(|cue| item.contains(cue))
}

fn has_intermediate_sentence_end(item: &str) -> bool {
    item.char_indices()
        .any(|(offset, ch)| is_sentence_end(ch) && offset + ch.len_utf8() < item.len())
}

/// Small allowlist for truly parallel semicolon-delimited imperative items.
/// A repeated token without these action shapes does not authorize layout.
fn starts_with_supported_action(item: &str) -> bool {
    const ENGLISH_ACTIONS: &[&str] = &[
        "add", "build", "call", "check", "close", "create", "delete", "draft", "email", "fix",
        "open", "preserve", "remove", "review", "run", "send", "ship", "update",
    ];
    const CJK_ACTIONS: &[&str] = &[
        "添加", "创建", "删除", "发送", "打开", "关闭", "检查", "修复", "更新", "构建", "测试",
        "联系", "审查", "核对",
    ];
    ENGLISH_ACTIONS.iter().any(|verb| {
        let Some(prefix) = item.get(..verb.len()) else {
            return false;
        };
        if !prefix.eq_ignore_ascii_case(verb)
            || !item[verb.len()..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
        {
            return false;
        }
        let object = item[verb.len()..].trim_start();
        let first_word = object
            .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
            .find(|word| !word.is_empty())
            .unwrap_or("")
            .to_ascii_lowercase();
        [
            "a", "an", "the", "my", "our", "your", "this", "that", "these", "those", "each",
            "every", "all",
        ]
        .contains(&first_word.as_str())
    }) || CJK_ACTIONS.iter().any(|verb| item.starts_with(verb))
}

/// Classify an action-shaped item when a shared lead-in already establishes
/// list intent. The no-lead-in path uses the stricter determiner check above.
fn begins_with_supported_imperative(item: &str) -> bool {
    const ENGLISH_ACTIONS: &[&str] = &[
        "add", "build", "call", "check", "close", "create", "delete", "draft", "email", "fix",
        "open", "preserve", "remove", "review", "run", "send", "ship", "update",
    ];
    const CJK_ACTIONS: &[&str] = &[
        "添加", "创建", "删除", "发送", "打开", "关闭", "检查", "修复", "更新", "构建", "测试",
        "联系", "审查", "核对",
    ];
    ENGLISH_ACTIONS.iter().any(|verb| {
        item.get(..verb.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(verb))
            && item[verb.len()..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
    }) || CJK_ACTIONS.iter().any(|verb| item.starts_with(verb))
}

/// Conservative guard for a trailing predicate or clause that applies to the
/// full list. False positives only suppress layout.
fn has_trailing_clause_or_shared_predicate(last_item: &str) -> bool {
    let lower = last_item.to_ascii_lowercase();
    [
        ", and ",
        ", which ",
        ", that ",
        ", who ",
        ", because ",
        ", while ",
        ", unless ",
        ", but ",
        "?",
        "？",
        "，并且",
        "，而且",
        "，所以",
        "，因为",
        "，它们",
        "，这些",
        "，我们",
    ]
    .iter()
    .any(|cue| lower.contains(&cue.to_ascii_lowercase()))
}

fn trimmed_range(source: &str, range: Range<usize>) -> Range<usize> {
    let slice = &source[range.clone()];
    let start = slice.len() - slice.trim_start().len();
    let end = slice.trim_end().len();
    if start >= end {
        return range.end..range.end;
    }
    range.start + start..range.start + end
}

fn skip_whitespace(source: &str, mut offset: usize) -> usize {
    while offset < source.len() {
        let ch = source[offset..].chars().next().expect("offset is in range");
        if !ch.is_whitespace() {
            break;
        }
        offset += ch.len_utf8();
    }
    offset
}

fn overlaps_literal(literals: &[Range<usize>], range: Range<usize>) -> bool {
    literals
        .iter()
        .any(|literal| range.start < literal.end && literal.start < range.end)
}

fn overlaps_protected(
    protected: &[crate::protected_span::ProtectedSpan],
    range: Range<usize>,
) -> bool {
    protected
        .iter()
        .any(|span| range.start < span.end_byte && span.start_byte < range.end)
}

fn non_overlapping(edits: Vec<SourceEdit>) -> Vec<SourceEdit> {
    let mut accepted: Vec<SourceEdit> = Vec::with_capacity(edits.len());
    for edit in edits {
        if accepted
            .last()
            .is_some_and(|previous| edit.source_range.start < previous.source_range.end)
        {
            return Vec::new();
        }
        accepted.push(edit);
    }
    accepted
}

fn apply_source_edits(source: &str, edits: &[SourceEdit]) -> Result<String, ProjectionError> {
    let mut output = String::with_capacity(source.len() + edits.len() * 4);
    let mut cursor = 0;
    for edit in edits {
        let range = &edit.source_range;
        if range.start < cursor
            || range.start > range.end
            || range.end > source.len()
            || !source.is_char_boundary(range.start)
            || !source.is_char_boundary(range.end)
        {
            return Err(ProjectionError::InvalidSourceEdit);
        }
        output.push_str(&source[cursor..range.start]);
        output.push_str(&edit.replacement);
        cursor = range.end;
    }
    output.push_str(&source[cursor..]);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{project_cleanup_layout, OutputLayoutMode, ProjectedStructure};

    #[test]
    fn shared_lead_in_renders_exact_count_as_source_preserving_bullets() {
        let source = "The three tasks are: update API; fix tests; review docs.";
        let projected = project_cleanup_layout(source, OutputLayoutMode::List).unwrap();

        assert_eq!(
            projected.text,
            "The three tasks are:\n- update API\n- fix tests\n- review docs."
        );
        assert_eq!(projected.structure, ProjectedStructure::List);
        assert_eq!(projected.edits.len(), 3);
    }

    #[test]
    fn count_mismatch_and_unsupported_exact_count_remain_prose() {
        for source in [
            "The three tasks are: update API; review docs.",
            "The five tasks are: update API; fix tests; review docs.",
        ] {
            let projected = project_cleanup_layout(source, OutputLayoutMode::List).unwrap();
            assert_eq!(projected.text, source);
            assert_eq!(projected.structure, ProjectedStructure::Unchanged);
        }
    }

    #[test]
    fn unmistakable_parallel_imperatives_can_render_without_a_lead_in() {
        let source = "Update the API; fix the tests; review the docs.";
        let projected = project_cleanup_layout(source, OutputLayoutMode::List).unwrap();
        assert_eq!(
            projected.text,
            "- Update the API\n- fix the tests\n- review the docs."
        );
    }

    #[test]
    fn narrative_with_repeated_first_person_verbs_stays_prose() {
        for source in [
            "I update the API; I fix the tests; I review the docs.",
            "Test results improved; test coverage fell.",
        ] {
            let projected = project_cleanup_layout(source, OutputLayoutMode::List).unwrap();
            assert_eq!(projected.text, source);
        }
    }

    #[test]
    fn list_guard_rejects_disjunctions_questions_and_trailing_shared_predicates() {
        for source in [
            "The two tasks are: update API or keep the old plan; review tests.",
            "The two tasks are: update API; review tests?",
            "The two tasks are: update API; review tests, which must ship tomorrow.",
            "The tasks are: check logs; then restart the service.",
            "The two tasks are: review the patch; deploy the build. Do neither before approval.",
            "The items are: staging logs; production logs are both required before release.",
        ] {
            let projected = project_cleanup_layout(source, OutputLayoutMode::List).unwrap();
            assert_eq!(projected.text, source);
        }
    }

    #[test]
    fn quoted_separators_and_code_literals_are_not_split_or_rewritten() {
        let quoted = "The two tasks are: preserve “A; B”; run tests.";
        let projected = project_cleanup_layout(quoted, OutputLayoutMode::List).unwrap();
        assert_eq!(
            projected.text,
            "The two tasks are:\n- preserve “A; B”\n- run tests."
        );

        let inline_code = "The two tasks are: preserve the literal `A; B`; run the tests.";
        let projected = project_cleanup_layout(inline_code, OutputLayoutMode::List).unwrap();
        assert_eq!(projected.text, inline_code);
    }

    #[test]
    fn chinese_shared_lead_in_accepts_a_tight_fullwidth_colon() {
        let source = "以下两项：检查日志；核对账单。";
        let projected = project_cleanup_layout(source, OutputLayoutMode::List).unwrap();
        assert_eq!(projected.text, "以下两项：\n- 检查日志\n- 核对账单。");
    }

    #[test]
    fn email_mode_splits_only_proven_spoken_greeting_body_and_closing() {
        let source = "Hi John looking forward to chatting tomorrow. Best Allan";
        let projected = project_cleanup_layout(source, OutputLayoutMode::Email).unwrap();
        assert_eq!(
            projected.text,
            "Hi John\nlooking forward to chatting tomorrow.\nBest\nAllan"
        );
        assert_eq!(projected.structure, ProjectedStructure::Email);

        let cjk = "您好明天三点开会谢谢";
        let projected = project_cleanup_layout(cjk, OutputLayoutMode::Email).unwrap();
        assert_eq!(projected.text, "您好\n明天三点开会\n谢谢");
    }

    #[test]
    fn email_mode_does_not_invent_a_greeting_or_closing() {
        for source in [
            "Looking forward to chatting tomorrow.",
            "We did our best today.",
            "我想谢谢你",
        ] {
            let projected = project_cleanup_layout(source, OutputLayoutMode::Email).unwrap();
            assert_eq!(projected.text, source);
            assert_eq!(projected.structure, ProjectedStructure::Unchanged);
        }
    }

    #[test]
    fn preformatted_source_and_none_mode_are_untouched() {
        for (source, mode) in [
            (
                "The tasks are:\n- update API\n- fix tests",
                OutputLayoutMode::List,
            ),
            (
                "We ship the patch. Separately, review the docs.",
                OutputLayoutMode::None,
            ),
        ] {
            let projected = project_cleanup_layout(source, mode).unwrap();
            assert_eq!(projected.text, source);
            assert_eq!(projected.structure, ProjectedStructure::Unchanged);
            assert!(projected.edits.is_empty());
        }
    }

    #[test]
    fn explicit_topic_transition_adds_only_a_paragraph_separator() {
        let source = "We will ship the patch. Separately, we should review the docs.";
        let projected = project_cleanup_layout(source, OutputLayoutMode::Paragraph).unwrap();
        assert_eq!(
            projected.text,
            "We will ship the patch.\n\nSeparately, we should review the docs."
        );
        assert_eq!(projected.structure, ProjectedStructure::Paragraph);
        assert_eq!(projected.edits.len(), 1);
    }

    #[test]
    fn paragraph_projection_rejects_narrative_only_or_incomplete_transitions() {
        for source in [
            "We will ship the patch. We should review the docs.",
            "We will ship the patch. Separately, review",
            "He wrote “Separately, we should review the docs.” after the meeting.",
        ] {
            let projected = project_cleanup_layout(source, OutputLayoutMode::Paragraph).unwrap();
            assert_eq!(projected.text, source);
        }
    }

    #[test]
    fn paragraph_layout_keeps_question_punctuation_and_adds_no_header() {
        let source = "Is release blocked? Separately, the team needs review.";
        let projected = project_cleanup_layout(source, OutputLayoutMode::Paragraph).unwrap();
        assert_eq!(
            projected.text,
            "Is release blocked?\n\nSeparately, the team needs review."
        );
        assert!(!projected.text.contains("Meeting notes"));
    }
}
