//! Deterministic restatement, hallucination, and repeat cleanup.
//!
//! Runs after spoken punctuation/layout and promoted lexicon preparation, then
//! before provider cleanup. Terminal and form-filling stay LocalOnly; prose
//! still needs these markers when the LLM is skipped or fails.

const HALLUCINATIONS: &[&str] = &[
    "thanks for watching",
    "thank you for watching",
    "thanks for listening",
    "thank you for listening",
    "please subscribe",
    "thanks for watching!",
    "sous-titres par",
    "字幕志愿者",
    "请不吝点赞",
    "感谢收看",
    "谢谢观看",
];

const MARKERS: &[&str] = &[
    "scratch that",
    "i mean",
    "no wait",
    "哦,不对",
    "哦，不对",
    "哦不对",
    "我说不对",
    "我说错了",
    "我是说",
    "不对",
];

const REPLACEMENT_REQUIRED_MARKERS: &[&str] = &["i meant", "删掉", "算了", "sorry", "no"];

const PROTECTED_CONTENT: &[&str] = &["看它对不对", "你说不对的时候", "我说不对的时候", "对不对"];

#[cfg(test)]
pub fn apply(text: &str) -> String {
    apply_with_authorizations(text).text
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedCorrection {
    /// Byte range of the superseded entity in the prepared source transcript.
    pub source_start_byte: usize,
    pub source_end_byte: usize,
    /// Byte range of the replacement entity in the resolved transcript.
    pub output_start_byte: usize,
    pub output_end_byte: usize,
    pub source_value: String,
    pub replacement_value: String,
    pub kind: crate::protected_span::ProtectedSpanKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionOutput {
    pub text: String,
    pub authorizations: Vec<AuthorizedCorrection>,
}

pub fn apply_with_authorizations(text: &str) -> RevisionOutput {
    resolve_all_sentences(text)
}

pub(crate) fn authorizations_match_source_and_output(
    source: &str,
    output: &str,
    authorizations: &[AuthorizedCorrection],
) -> bool {
    let source_spans =
        crate::protected_span::protected_spans(source, Some(crate::llm::CleanupOperation::Cleanup));
    let output_spans =
        crate::protected_span::protected_spans(output, Some(crate::llm::CleanupOperation::Cleanup));
    let mut previous_source_end = 0;
    let mut previous_output_end = 0;
    for authorization in authorizations {
        if authorization.source_start_byte < previous_source_end
            || authorization.output_start_byte < previous_output_end
            || authorization.source_start_byte >= authorization.source_end_byte
            || authorization.output_start_byte >= authorization.output_end_byte
            || !source.is_char_boundary(authorization.source_start_byte)
            || !source.is_char_boundary(authorization.source_end_byte)
            || !output.is_char_boundary(authorization.output_start_byte)
            || !output.is_char_boundary(authorization.output_end_byte)
            || source.get(authorization.source_start_byte..authorization.source_end_byte)
                != Some(authorization.source_value.as_str())
            || output.get(authorization.output_start_byte..authorization.output_end_byte)
                != Some(authorization.replacement_value.as_str())
            || !source_spans.iter().any(|span| {
                span.start_byte == authorization.source_start_byte
                    && span.end_byte == authorization.source_end_byte
                    && span.kind == authorization.kind
            })
            || !output_spans.iter().any(|span| {
                span.start_byte == authorization.output_start_byte
                    && span.end_byte == authorization.output_end_byte
                    && span.kind == authorization.kind
            })
        {
            return false;
        }
        previous_source_end = authorization.source_end_byte;
        previous_output_end = authorization.output_end_byte;
    }
    true
}

pub fn is_hallucination_text(text: &str) -> bool {
    let folded = fold_compare(text);
    if folded.is_empty() {
        return false;
    }
    HALLUCINATIONS.iter().any(|phrase| {
        let needle = fold_compare(phrase);
        folded == needle || folded.starts_with(&needle) || folded.ends_with(&needle)
    })
}

fn fold_compare(text: &str) -> String {
    text.chars()
        .filter(|ch| {
            !ch.is_whitespace() && !matches!(*ch, '.' | '!' | '?' | '。' | '！' | '？' | ',' | '，')
        })
        .flat_map(char::to_lowercase)
        .collect()
}

#[derive(Debug, Clone)]
struct ResolvedSegment {
    source_start_byte: usize,
    text: String,
    authorizations: Vec<AuthorizedCorrection>,
}

fn resolve_all_sentences(text: &str) -> RevisionOutput {
    let literal_ranges = literal_ranges(text);
    let mut kept: Vec<ResolvedSegment> = Vec::new();
    let mut source_offset = 0;
    for sentence in split_sentences(text) {
        let sentence_start = source_offset;
        source_offset += sentence.len();
        if starts_with_correction(&sentence, text, sentence_start, &literal_ranges) {
            if let Some(previous) = kept.last_mut() {
                if let Some(corrected) = resolve_cross_sentence_revision(
                    previous,
                    &sentence,
                    sentence_start,
                    text,
                    &literal_ranges,
                ) {
                    *previous = corrected;
                    continue;
                }
            }
            // Without a matching, locally identifiable replacement, the
            // correction may refer to any earlier phrase. Preserve both.
            kept.push(ResolvedSegment {
                source_start_byte: sentence_start,
                text: sentence,
                authorizations: Vec::new(),
            });
            continue;
        }
        let (resolved, authorization) =
            resolve_one_sentence(&sentence, sentence_start, text, &literal_ranges);
        kept.push(ResolvedSegment {
            source_start_byte: sentence_start,
            text: resolved,
            authorizations: authorization.into_iter().collect(),
        });
    }
    let mut resolved = String::new();
    let mut authorizations = Vec::new();
    for segment in kept {
        let output_offset = resolved.len();
        resolved.push_str(&segment.text);
        for mut authorization in segment.authorizations {
            authorization.output_start_byte += output_offset;
            authorization.output_end_byte += output_offset;
            authorizations.push(authorization);
        }
    }
    RevisionOutput {
        text: resolved,
        authorizations,
    }
}

fn resolve_cross_sentence_revision(
    previous: &ResolvedSegment,
    correction: &str,
    correction_source_start: usize,
    full_source: &str,
    literal_ranges: &[std::ops::Range<usize>],
) -> Option<ResolvedSegment> {
    if !previous.authorizations.is_empty() {
        return None;
    }
    let (previous_body, _) = peel_closer(&previous.text);
    let (correction_body, correction_closer) = peel_closer(correction);
    let (marker_start, marker_end) = split_on_last_marker_run(correction_body)?;
    if marker_start != skip_junk(correction_body, 0)
        || overlaps_literal(
            literal_ranges,
            correction_source_start + marker_start,
            correction_source_start + marker_end,
        )
        || intersects_protected_content(correction_body, marker_start, marker_end)
    {
        return None;
    }
    let replacement = strip_replacement_introducer(correction_body[marker_end..].trim());
    let old_entity = trailing_revision_entity(previous_body)?;
    let new_entity = leading_revision_entity(&replacement)?;
    if old_entity.kind != new_entity.kind {
        return None;
    }
    let source_start = previous.source_start_byte + old_entity.start_byte;
    let source_end = previous.source_start_byte + old_entity.end_byte;
    if overlaps_literal(literal_ranges, source_start, source_end)
        || !full_source.is_char_boundary(source_start)
        || !full_source.is_char_boundary(source_end)
    {
        return None;
    }
    let prefix = previous_body[..old_entity.start_byte].trim_end();
    let separator = revision_separator(prefix, &replacement);
    let output_start = prefix.len() + separator.len() + new_entity.start_byte;
    let text = attach_closer(
        &join_revision_fragments(prefix, &replacement),
        correction_closer,
    );
    let source_value = previous_body[old_entity.start_byte..old_entity.end_byte].to_owned();
    let replacement_value = replacement[new_entity.start_byte..new_entity.end_byte].to_owned();
    Some(ResolvedSegment {
        source_start_byte: previous.source_start_byte,
        text,
        authorizations: vec![AuthorizedCorrection {
            source_start_byte: source_start,
            source_end_byte: source_end,
            output_start_byte: output_start,
            output_end_byte: output_start + replacement_value.len(),
            source_value,
            replacement_value,
            kind: old_entity.kind,
        }],
    })
}

fn starts_with_correction(
    sentence: &str,
    full_source: &str,
    source_start: usize,
    literal_ranges: &[std::ops::Range<usize>],
) -> bool {
    let (body, _) = peel_closer(sentence);
    let lower = body.to_ascii_lowercase();
    let start = skip_junk(body, 0);
    match_one_marker(body, &lower, start).is_some_and(|(_, end)| {
        !overlaps_literal(literal_ranges, source_start + start, source_start + end)
            && !intersects_protected_content(body, start, end)
            && full_source.is_char_boundary(source_start + start)
    })
}

fn resolve_one_sentence(
    sentence: &str,
    source_start_byte: usize,
    full_source: &str,
    literal_ranges: &[std::ops::Range<usize>],
) -> (String, Option<AuthorizedCorrection>) {
    let (body, closer) = peel_closer(sentence);
    if let Some((marker_start, marker_end)) = split_on_last_marker_run(body) {
        if overlaps_literal(
            literal_ranges,
            source_start_byte + marker_start,
            source_start_byte + marker_end,
        ) {
            return (sentence.to_owned(), None);
        }
        let suffix = &body[marker_end..];
        if suffix.trim().is_empty() {
            if marker_start == 0 {
                return (String::new(), None);
            }
            return (sentence.to_owned(), None);
        }
        if intersects_protected_content(body, marker_start, marker_end)
            || marker_start > 0 && !has_explicit_edit_boundary(body, marker_start)
        {
            return (sentence.to_owned(), None);
        }
        let replacement = strip_replacement_introducer(suffix.trim());
        if replacement.is_empty() {
            return (sentence.to_owned(), None);
        }
        let Some(old_entity) = trailing_revision_entity(&body[..marker_start]) else {
            return (sentence.to_owned(), None);
        };
        let Some(new_entity) = leading_revision_entity(&replacement) else {
            return (sentence.to_owned(), None);
        };
        if old_entity.kind != new_entity.kind {
            return (sentence.to_owned(), None);
        }
        let source_start = source_start_byte + old_entity.start_byte;
        let source_end = source_start_byte + old_entity.end_byte;
        if overlaps_literal(literal_ranges, source_start, source_end)
            || !full_source.is_char_boundary(source_start)
            || !full_source.is_char_boundary(source_end)
        {
            return (sentence.to_owned(), None);
        }
        let prefix = body[..old_entity.start_byte].trim_end();
        let separator = revision_separator(prefix, &replacement);
        let output_start = prefix.len() + separator.len() + new_entity.start_byte;
        let text = attach_closer(&join_revision_fragments(prefix, &replacement), closer);
        let source_value = body[old_entity.start_byte..old_entity.end_byte].to_owned();
        let replacement_value = replacement[new_entity.start_byte..new_entity.end_byte].to_owned();
        return (
            text,
            Some(AuthorizedCorrection {
                source_start_byte: source_start,
                source_end_byte: source_end,
                output_start_byte: output_start,
                output_end_byte: output_start + replacement_value.len(),
                source_value,
                replacement_value,
                kind: old_entity.kind,
            }),
        );
    }
    if let Some((replaced, authorization)) = apply_contrast_not_a_but_b(body, source_start_byte) {
        if !overlaps_literal(
            literal_ranges,
            authorization.source_start_byte,
            authorization.source_end_byte,
        ) {
            return (attach_closer(&replaced, closer), Some(authorization));
        }
    }
    (sentence.to_owned(), None)
}

fn has_explicit_edit_boundary(text: &str, marker_start: usize) -> bool {
    text[..marker_start]
        .trim_end()
        .chars()
        .next_back()
        .is_some_and(|ch| matches!(ch, ',' | '，' | ';' | '；' | ':' | '：' | '—'))
}

#[derive(Clone, Copy)]
struct RevisionEntity {
    start_byte: usize,
    end_byte: usize,
    kind: crate::protected_span::ProtectedSpanKind,
}

fn trailing_revision_entity(text: &str) -> Option<RevisionEntity> {
    let trimmed = text.trim_end_matches(|ch: char| {
        ch.is_whitespace() || matches!(ch, ',' | '，' | ';' | '；' | ':' | '：')
    });
    crate::protected_span::protected_spans(trimmed, Some(crate::llm::CleanupOperation::Cleanup))
        .into_iter()
        .rfind(|span| span.end_byte == trimmed.len() && revision_entity_kind(span.kind))
        .map(|span| RevisionEntity {
            start_byte: span.start_byte,
            end_byte: span.end_byte,
            kind: span.kind,
        })
}

fn leading_revision_entity(text: &str) -> Option<RevisionEntity> {
    let leading = text.trim_start_matches(|ch: char| {
        ch.is_whitespace() || matches!(ch, ',' | '，' | ';' | '；' | ':' | '：')
    });
    let offset = text.len() - leading.len();
    crate::protected_span::protected_spans(leading, Some(crate::llm::CleanupOperation::Cleanup))
        .into_iter()
        .find(|span| span.start_byte == 0 && revision_entity_kind(span.kind))
        .map(|span| RevisionEntity {
            start_byte: offset + span.start_byte,
            end_byte: offset + span.end_byte,
            kind: span.kind,
        })
}

fn revision_entity_kind(kind: crate::protected_span::ProtectedSpanKind) -> bool {
    matches!(
        kind,
        crate::protected_span::ProtectedSpanKind::Date
            | crate::protected_span::ProtectedSpanKind::DateWord
            | crate::protected_span::ProtectedSpanKind::Amount
            | crate::protected_span::ProtectedSpanKind::Version
            | crate::protected_span::ProtectedSpanKind::Term
    )
}

fn join_revision_fragments(prefix: &str, replacement: &str) -> String {
    let prefix = prefix.trim_end();
    if prefix.is_empty() {
        return replacement.to_owned();
    }
    let Some(first) = replacement.chars().next() else {
        return prefix.to_owned();
    };
    let last = prefix.chars().next_back();
    let separator = match last {
        None => "",
        Some(ch)
            if ch.is_whitespace()
                || matches!(ch, ',' | '，' | ';' | '；' | ':' | '：')
                || matches!(
                    first,
                    ',' | '，' | ';' | '；' | ':' | '：' | '.' | '。' | '!' | '！' | '?' | '？'
                )
                || is_cjk(ch) && is_cjk(first) =>
        {
            ""
        }
        Some(_) => " ",
    };
    format!("{prefix}{separator}{replacement}")
}

fn revision_separator(prefix: &str, replacement: &str) -> &'static str {
    let prefix = prefix.trim_end();
    let Some(first) = replacement.chars().next() else {
        return "";
    };
    match prefix.chars().next_back() {
        None => "",
        Some(ch)
            if ch.is_whitespace()
                || matches!(ch, ',' | '，' | ';' | '；' | ':' | '：')
                || matches!(
                    first,
                    ',' | '，' | ';' | '；' | ':' | '：' | '.' | '。' | '!' | '！' | '?' | '？'
                )
                || is_cjk(ch) && is_cjk(first) =>
        {
            ""
        }
        Some(_) => " ",
    }
}

pub(crate) fn literal_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let code_ranges = crate::spoken_layout::code_literal_ranges(text);
    let mut ranges = code_ranges.clone();
    let mut closers = Vec::new();
    let mut quote_start = None;
    for (byte_index, ch) in text.char_indices() {
        if code_ranges
            .iter()
            .any(|range| range.start <= byte_index && byte_index < range.end)
        {
            continue;
        }
        let next = text[byte_index + ch.len_utf8()..].chars().next();
        let was_unquoted = closers.is_empty();
        match ch {
            '“' => closers.push('”'),
            '「' => closers.push('」'),
            '『' => closers.push('』'),
            '‘' => closers.push('’'),
            '"' => {
                if closers.last() == Some(&'"') {
                    closers.pop();
                } else {
                    closers.push('"');
                }
            }
            '\'' if !(text[..byte_index]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric)
                && next.is_some_and(char::is_alphanumeric)) =>
            {
                if closers.last() == Some(&'\'') {
                    closers.pop();
                } else {
                    closers.push('\'');
                }
            }
            closer if closers.last() == Some(&closer) => {
                closers.pop();
            }
            _ => {}
        }
        if was_unquoted && !closers.is_empty() {
            quote_start = Some(byte_index);
        } else if !was_unquoted && closers.is_empty() {
            if let Some(start) = quote_start.take() {
                ranges.push(start..byte_index + ch.len_utf8());
            }
        }
    }
    if !closers.is_empty() {
        if let Some(start) = quote_start {
            ranges.push(start..text.len());
        }
    }
    ranges.sort_by_key(|range| range.start);
    ranges
}

fn overlaps_literal(ranges: &[std::ops::Range<usize>], start: usize, end: usize) -> bool {
    ranges
        .iter()
        .any(|range| start < range.end && range.start < end)
}

fn intersects_protected_content(text: &str, start: usize, end: usize) -> bool {
    PROTECTED_CONTENT.iter().any(|needle| {
        text.match_indices(needle).any(|(phrase_start, phrase)| {
            let phrase_end = phrase_start + phrase.len();
            start < phrase_end && phrase_start < end
        })
    })
}

fn apply_contrast_not_a_but_b(
    text: &str,
    source_start_byte: usize,
) -> Option<(String, AuthorizedCorrection)> {
    let not_start = text.find("不是")?;
    if !text[..not_start].trim().is_empty() {
        return None;
    }
    let old_start = not_start + "不是".len();
    let shi = old_start + find_positive_shi(&text[old_start..])?;
    let old_segment = &text[old_start..shi];
    let old_text = old_segment
        .trim_matches(|ch: char| ch.is_whitespace() || matches!(ch, ',' | '，' | ';' | '；'));
    let replacement = text[shi + "是".len()..].trim();
    if replacement.is_empty() || replacement.starts_with(['的', '否', '不']) {
        return None;
    }
    let old_entity = trailing_revision_entity(old_text)?;
    let new_entity = leading_revision_entity(replacement)?;
    if old_entity.kind != new_entity.kind {
        return None;
    }
    let old_text_offset = old_start + old_segment.find(old_text)?;
    let source_value = old_text[old_entity.start_byte..old_entity.end_byte].to_owned();
    let replacement_value = replacement[new_entity.start_byte..new_entity.end_byte].to_owned();
    Some((
        replacement.to_owned(),
        AuthorizedCorrection {
            source_start_byte: source_start_byte + old_text_offset + old_entity.start_byte,
            source_end_byte: source_start_byte + old_text_offset + old_entity.end_byte,
            output_start_byte: new_entity.start_byte,
            output_end_byte: new_entity.start_byte + replacement_value.len(),
            source_value,
            replacement_value,
            kind: old_entity.kind,
        },
    ))
}

fn find_positive_shi(after: &str) -> Option<usize> {
    let mut index = 0;
    while index < after.len() {
        if !after.is_char_boundary(index) {
            index += 1;
            continue;
        }
        if after[index..].starts_with("不是") || after[index..].starts_with("是否") {
            index += "不是".len();
            continue;
        }
        if after[index..].starts_with("是") {
            return Some(index);
        }
        index += after[index..]
            .chars()
            .next()
            .map(|ch| ch.len_utf8())
            .unwrap_or(1);
    }
    None
}

fn strip_replacement_introducer(suffix: &str) -> String {
    let rest = suffix.trim_start_matches([' ', ',', '，']);
    for intro in ["应该是", "我是说", "是"] {
        if let Some(after) = rest.strip_prefix(intro) {
            let after = after.trim_start();
            if !after.is_empty() && !after.starts_with(['的', '否', '不']) {
                return after.to_owned();
            }
        }
    }
    suffix.trim().to_owned()
}

fn peel_closer(sentence: &str) -> (&str, &str) {
    let trimmed = sentence.trim_end();
    for closer in ["。", "！", "？", ".", "!", "?", "\n"] {
        if let Some(body) = trimmed.strip_suffix(closer) {
            return (body, &sentence[body.len()..]);
        }
    }
    (sentence, "")
}

fn attach_closer(body: &str, closer: &str) -> String {
    let mut out = body.trim().to_owned();
    out.push_str(closer);
    out
}

fn split_on_last_marker_run(text: &str) -> Option<(usize, usize)> {
    let lower = text.to_ascii_lowercase();
    let mut best: Option<(usize, usize)> = None;
    let mut index = 0;
    while index < text.len() {
        if !text.is_char_boundary(index) {
            index += 1;
            continue;
        }
        if let Some((start, end)) = match_marker_run(text, &lower, index) {
            best = Some((start, end));
            index = end;
            continue;
        }
        index += text[index..]
            .chars()
            .next()
            .map(|ch| ch.len_utf8())
            .unwrap_or(1);
    }
    best
}

fn match_marker_run(text: &str, lower: &str, start: usize) -> Option<(usize, usize)> {
    let mut cursor = skip_junk(text, start);
    let (_, end) = match_one_marker(text, lower, cursor)?;
    let run_start = cursor;
    cursor = end;
    loop {
        cursor = skip_junk(text, cursor);
        let Some((_, end)) = match_one_marker(text, lower, cursor) else {
            break;
        };
        cursor = end;
    }
    Some((run_start, cursor))
}

fn match_one_marker(text: &str, lower: &str, start: usize) -> Option<(usize, usize)> {
    if start >= text.len() || !text.is_char_boundary(start) {
        return None;
    }
    for marker in REPLACEMENT_REQUIRED_MARKERS {
        if matches!(*marker, "sorry" | "no")
            && (start == 0 || !has_explicit_edit_boundary(text, start))
        {
            continue;
        }
        if let Some(end) = match_plain_marker(text, lower, start, marker) {
            if has_replacement_after(text, end) {
                return Some((start, end));
            }
        }
    }
    for marker in MARKERS {
        if let Some(end) = match_plain_marker(text, lower, start, marker) {
            if *marker == "不对" && !cjk_bu_dui_right_ok(text, end) {
                continue;
            }
            return Some((start, end));
        }
    }
    if let Some(end) = match_no_capital_replacement(text, start) {
        return Some((start, end));
    }
    if let Some(end) = match_actually_replacement(text, start) {
        return Some((start, end));
    }
    None
}

fn match_plain_marker(text: &str, lower: &str, start: usize, marker: &str) -> Option<usize> {
    if marker.is_ascii() {
        if lower[start..].starts_with(marker) && ascii_boundaries(text, start, start + marker.len())
        {
            return Some(start + marker.len());
        }
        return None;
    }
    if text[start..].starts_with(marker) {
        return Some(start + marker.len());
    }
    None
}

fn has_replacement_after(text: &str, end: usize) -> bool {
    let after = skip_junk(text, end);
    if after >= text.len() {
        return false;
    }
    !is_blank_or_markers(&text[after..])
}

fn match_no_capital_replacement(text: &str, start: usize) -> Option<usize> {
    if !has_explicit_edit_boundary(text, start) {
        return None;
    }
    let rest = text[start..].trim_start_matches([' ', ',', '，']);
    let offset = start + (text[start..].len() - rest.len());
    let lower = rest.to_ascii_lowercase();
    if !lower.starts_with("no ") {
        return None;
    }
    let after = rest.get(3..)?;
    let next = after.chars().next()?;
    if !next.is_ascii_uppercase() {
        return None;
    }
    Some(offset + 3)
}

fn match_actually_replacement(text: &str, start: usize) -> Option<usize> {
    if !has_explicit_edit_boundary(text, start) {
        return None;
    }
    let rest = text[start..].trim_start_matches([' ', ',', '，']);
    let offset = start + (text[start..].len() - rest.len());
    if !rest.to_ascii_lowercase().starts_with("actually") {
        return None;
    }
    let before = text[..start].trim_end();
    if before.to_ascii_lowercase().ends_with("i") {
        return None;
    }
    let after = rest.get("actually".len()..)?.trim_start();
    after.chars().next().filter(|ch| ch.is_alphanumeric())?;
    Some(offset + "actually".len())
}

fn cjk_bu_dui_right_ok(text: &str, end: usize) -> bool {
    match text[end..].chars().next() {
        None => true,
        Some(ch) if ch.is_whitespace() => true,
        Some(',' | '，' | '。' | '.' | '、' | '!' | '?' | '！' | '？' | '\n') => true,
        Some('是') => true,
        Some('啊' | '呀' | '呢' | '吧' | '嘛') => true,
        Some(ch) if is_cjk(ch) => false,
        _ => true,
    }
}

fn skip_junk(text: &str, start: usize) -> usize {
    let mut index = start;
    for ch in text[start..].chars() {
        if ch.is_whitespace()
            || matches!(
                ch,
                ',' | '，' | '。' | '.' | '、' | '哦' | '啊' | '呀' | '呢' | '吧' | '嘛'
            )
        {
            index += ch.len_utf8();
        } else {
            break;
        }
    }
    index
}

fn ascii_boundaries(text: &str, start: usize, end: usize) -> bool {
    let left_ok = text[..start]
        .chars()
        .next_back()
        .is_none_or(|ch| !ch.is_ascii_alphabetic());
    let right_ok = text[end..]
        .chars()
        .next()
        .is_none_or(|ch| !ch.is_ascii_alphabetic());
    left_ok && right_ok
}

fn is_blank_or_markers(text: &str) -> bool {
    let stripped = text
        .trim()
        .trim_matches(|ch: char| {
            ch.is_whitespace()
                || matches!(ch, ',' | '，' | '。' | '.' | '、' | '!' | '?' | '！' | '？')
        })
        .to_ascii_lowercase();
    stripped.is_empty() || MARKERS.iter().any(|marker| stripped == *marker)
}

fn split_sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut current = String::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        current.push(ch);
        let next = chars.get(index + 1).copied();
        let prev = if index > 0 {
            Some(chars[index - 1])
        } else {
            None
        };
        let is_version_dot = ch == '.'
            && prev.is_some_and(|item| item.is_ascii_digit())
            && next.is_some_and(|item| item.is_ascii_digit());
        let is_list_dot = ch == '.'
            && prev.is_some_and(|item| item.is_ascii_digit())
            && next.is_some_and(|item| item.is_whitespace());
        let english_end = matches!(ch, '.' | '!' | '?')
            && !is_version_dot
            && !is_list_dot
            && next.is_none_or(|item| item.is_whitespace() || is_cjk(item));
        if matches!(ch, '。' | '！' | '？' | '\n') || english_end {
            out.push(std::mem::take(&mut current));
        }
        index += 1;
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn is_cjk(ch: char) -> bool {
    crate::dictionary_learn::is_cjk(ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_level_chinese_correction() {
        assert_eq!(apply("周四，不对，周五"), "周五");
        assert_eq!(
            apply("嗯，我周四，不对，周五下午开会"),
            "嗯，我周五下午开会"
        );
        assert_eq!(
            apply("请通知小王周四，不对，周五开会"),
            "请通知小王周五开会"
        );
    }

    #[test]
    fn preserves_ambiguous_false_starts_and_repeated_content() {
        let raw = "现在做一个完整的这种 cloud 的，不对，不对，不对。我现在在做一种完整的这个 cursor 的这个测试，看一下它这个具体的 cleanup。看一下它具体的 cleanup。";
        let out = apply(raw);
        assert!(out.contains("cloud"), "{out}");
        assert!(out.contains("不对"), "{out}");
        assert!(out.contains("cursor"), "{out}");
        assert_eq!(out.matches("cleanup").count(), 2, "{out}");
    }

    #[test]
    fn repeated_referents_and_quoted_or_code_lines_survive_revision_pass() {
        let referents = "这个项目需要 review。那个项目需要 review。";
        assert_eq!(apply(referents), referents);

        let quoted = "他说：‘运行测试。’他说：‘运行测试。’";
        assert_eq!(apply(quoted), quoted);

        let code = "```sh\ncargo test\ncargo test\n```";
        assert_eq!(apply(code), code);
    }

    #[test]
    fn keeps_content_bu_dui() {
        assert_eq!(apply("看它对不对"), "看它对不对");
        assert_eq!(apply("你说不对的时候再改"), "你说不对的时候再改");
        assert_eq!(apply("这件事不对。"), "这件事不对。");
        assert_eq!(apply("我觉得这样不对。"), "我觉得这样不对。");
        assert_eq!(apply("这件事不对劲。"), "这件事不对劲。");
        assert_eq!(apply("你说的不对"), "你说的不对");
        assert!(apply("预算是 1250，不是 1500").contains("1250"));
        assert!(apply("预算是 1250，不是 1500").contains("1500"));
    }

    #[test]
    fn english_markers_require_matching_weekday_edits() {
        assert_eq!(apply("Tuesday, no Wednesday"), "Wednesday");
        assert_eq!(
            apply("let's meet Tuesday, actually Wednesday"),
            "let's meet Wednesday"
        );
        assert_eq!(
            apply("I actually enjoyed the movie"),
            "I actually enjoyed the movie"
        );
        assert_eq!(
            apply("write a cloud test, scratch that, write a cursor test"),
            "write a cloud test, scratch that, write a cursor test"
        );
        assert_eq!(
            apply("write a cloud test, I meant write a cursor test"),
            "write a cloud test, I meant write a cursor test"
        );
        assert_eq!(
            apply("Please notify John, I mean tomorrow"),
            "Please notify John, I mean tomorrow"
        );
        assert_eq!(
            apply("I have no Java experience"),
            "I have no Java experience"
        );
        assert_eq!(apply("We actually need fix"), "We actually need fix");
    }

    #[test]
    fn revision_markers_need_a_proven_replacement() {
        assert_eq!(
            apply("写 cloud 测试，删掉，写 cursor 测试"),
            "写 cloud 测试，删掉，写 cursor 测试"
        );
        assert_eq!(
            apply("写 cloud 测试，算了，写 cursor 测试"),
            "写 cloud 测试，算了，写 cursor 测试"
        );
        assert_eq!(apply("这件事删掉"), "这件事删掉");
        assert_eq!(apply("算了"), "算了");
        assert!(apply("I actually enjoyed the movie").contains("actually"));
    }

    #[test]
    fn text_alone_never_deletes_possible_hallucinations() {
        assert_eq!(apply("请在片尾写上谢谢观看"), "请在片尾写上谢谢观看");
        assert_eq!(
            apply("hello world. Thanks for watching!"),
            "hello world. Thanks for watching!"
        );
        assert_eq!(
            apply("hello world. Thanks for watching the show."),
            "hello world. Thanks for watching the show."
        );
        assert_eq!(apply("感谢收看本期节目"), "感谢收看本期节目");
        // Repetition alone is not evidence of a false start: both sentences
        // may have been spoken intentionally, so preserve the source wording.
        assert_eq!(
            apply("看一下具体的 cleanup。看一下具体的 cleanup。"),
            "看一下具体的 cleanup。看一下具体的 cleanup。"
        );
        assert!(is_hallucination_text("Thanks for watching the show."));
        assert!(is_hallucination_text("感谢收看本期节目"));
        assert!(!is_hallucination_text("hello world"));
    }

    #[test]
    fn does_not_split_numbered_list_markers() {
        let out = apply("1. 是 prompt\n2. 是 system");
        assert!(out.contains("1. "), "{out}");
        assert!(out.contains("2. "), "{out}");
        assert!(out.contains("是 prompt"), "{out}");
        assert!(out.contains("是 system"), "{out}");
        assert!(!out.contains("1.2."), "{out}");
    }

    #[test]
    fn sentence_initial_revisions_require_a_matching_local_date() {
        assert_eq!(
            apply("请通知小王周四。不对，周五开会"),
            "请通知小王周五开会"
        );
        assert_eq!(
            apply("做一个 cloud 测试。不对，是 cursor 测试。"),
            "做一个 cloud 测试。不对，是 cursor 测试。"
        );
        assert_eq!(
            apply("做一个 cloud 测试。我说不对，是 cursor 测试。"),
            "做一个 cloud 测试。我说不对，是 cursor 测试。"
        );
        assert_eq!(
            apply("做一个 cloud 测试。不对是 cursor 测试。"),
            "做一个 cloud 测试。不对是 cursor 测试。"
        );
        assert_eq!(apply("周四，不对，是周五"), "周五");
        assert_eq!(
            apply("做一个 cloud 测试。不对。是 cursor 测试。"),
            "做一个 cloud 测试。不对。是 cursor 测试。"
        );
        assert_eq!(apply("周四，我是说周五"), "周五");
        assert_eq!(
            apply("做一个 cloud 测试，不是 cloud，是 cursor"),
            "做一个 cloud 测试，不是 cloud，是 cursor"
        );
        assert_eq!(apply("Tuesday, I mean Wednesday"), "Wednesday");
    }

    #[test]
    fn uncertain_sentence_initial_markers_preserve_the_previous_content() {
        let out = apply("现在做一个 cloud 的。不对，不对，不对。我现在在做 cursor 的测试。");
        assert!(out.contains("cloud"), "{out}");
        assert!(out.contains("不对"), "{out}");
        assert!(out.contains("cursor"), "{out}");
    }
}
