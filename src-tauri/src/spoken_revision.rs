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

pub(crate) const MARKERS: &[&str] = &[
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
pub enum AuthorizedCorrection {
    ProtectedEntity {
        source_start_byte: usize,
        source_end_byte: usize,
        output_start_byte: usize,
        output_end_byte: usize,
        source_value: String,
        replacement_value: String,
        kind: crate::protected_span::ProtectedSpanKind,
    },
    ImmediateDecisionRestatement {
        affirmative_range: std::ops::Range<usize>,
        cue_range: std::ops::Range<usize>,
        corrective_range: std::ops::Range<usize>,
        output_corrective_range: std::ops::Range<usize>,
        actor: String,
        action: String,
        superseded_destination: String,
        final_destination: String,
    },
    SpokenRestartRemoval {
        source_segment_range: std::ops::Range<usize>,
        source_range: std::ops::Range<usize>,
        filler_range: std::ops::Range<usize>,
        abandoned_range: std::ops::Range<usize>,
        cue_range: std::ops::Range<usize>,
        repeated_anchor_range: std::ops::Range<usize>,
        output_segment_range: std::ops::Range<usize>,
    },
    StandalonePrefaceFillerRemoval {
        source_segment_range: std::ops::Range<usize>,
        filler_range: std::ops::Range<usize>,
        separator_range: std::ops::Range<usize>,
        output_segment_range: std::ops::Range<usize>,
    },
}

impl AuthorizedCorrection {
    fn shift_output(&mut self, offset: usize) {
        match self {
            Self::ProtectedEntity {
                output_start_byte,
                output_end_byte,
                ..
            } => {
                *output_start_byte += offset;
                *output_end_byte += offset;
            }
            Self::ImmediateDecisionRestatement {
                output_corrective_range,
                ..
            } => {
                output_corrective_range.start += offset;
                output_corrective_range.end += offset;
            }
            Self::SpokenRestartRemoval {
                output_segment_range,
                ..
            } => {
                output_segment_range.start += offset;
                output_segment_range.end += offset;
            }
            Self::StandalonePrefaceFillerRemoval {
                output_segment_range,
                ..
            } => {
                output_segment_range.start += offset;
                output_segment_range.end += offset;
            }
        }
    }

    fn source_bounds(&self) -> (usize, usize) {
        match self {
            Self::ProtectedEntity {
                source_start_byte,
                source_end_byte,
                ..
            } => (*source_start_byte, *source_end_byte),
            Self::ImmediateDecisionRestatement {
                affirmative_range,
                corrective_range,
                ..
            } => (affirmative_range.start, corrective_range.end),
            Self::SpokenRestartRemoval { source_range, .. } => {
                (source_range.start, source_range.end)
            }
            Self::StandalonePrefaceFillerRemoval {
                filler_range,
                separator_range,
                ..
            } => (filler_range.start, separator_range.end),
        }
    }

    fn output_bounds(&self) -> (usize, usize) {
        match self {
            Self::ProtectedEntity {
                output_start_byte,
                output_end_byte,
                ..
            } => (*output_start_byte, *output_end_byte),
            Self::ImmediateDecisionRestatement {
                output_corrective_range,
                ..
            } => (output_corrective_range.start, output_corrective_range.end),
            Self::SpokenRestartRemoval {
                source_range,
                source_segment_range,
                output_segment_range,
                ..
            } => {
                let offset = source_range
                    .start
                    .saturating_sub(source_segment_range.start);
                let position = output_segment_range.start.saturating_add(offset);
                (position, position)
            }
            Self::StandalonePrefaceFillerRemoval {
                filler_range,
                source_segment_range,
                output_segment_range,
                ..
            } => {
                let offset = filler_range
                    .start
                    .saturating_sub(source_segment_range.start);
                let position = output_segment_range.start.saturating_add(offset);
                (position, position)
            }
        }
    }
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
    if source == output && authorizations.is_empty() {
        return true;
    }
    let canonical = resolve_all_sentences(source);
    let canonical_has_source_proved_removal =
        canonical.authorizations.iter().any(|authorization| {
            matches!(
                authorization,
                AuthorizedCorrection::SpokenRestartRemoval { .. }
                    | AuthorizedCorrection::StandalonePrefaceFillerRemoval { .. }
            )
        });
    let supplied_source_proved_removal = authorizations.iter().any(|authorization| {
        matches!(
            authorization,
            AuthorizedCorrection::SpokenRestartRemoval { .. }
                | AuthorizedCorrection::StandalonePrefaceFillerRemoval { .. }
        )
    });
    if (canonical_has_source_proved_removal || supplied_source_proved_removal)
        && (canonical.text != output || canonical.authorizations != authorizations)
    {
        return false;
    }
    let source_spans =
        crate::protected_span::protected_spans(source, Some(crate::llm::CleanupOperation::Cleanup));
    let output_spans =
        crate::protected_span::protected_spans(output, Some(crate::llm::CleanupOperation::Cleanup));
    let source_literals = literal_ranges(source);
    let mut previous_source_end = 0;
    let mut previous_output_end = 0;
    for authorization in authorizations {
        let (source_start, source_end) = authorization.source_bounds();
        let (output_start, output_end) = authorization.output_bounds();
        if source_start < previous_source_end
            || output_start < previous_output_end
            || source_start >= source_end
            || (output_start >= output_end
                && !matches!(
                    authorization,
                    AuthorizedCorrection::SpokenRestartRemoval { .. }
                        | AuthorizedCorrection::StandalonePrefaceFillerRemoval { .. }
                ))
            || !source.is_char_boundary(source_start)
            || !source.is_char_boundary(source_end)
            || !output.is_char_boundary(output_start)
            || !output.is_char_boundary(output_end)
        {
            return false;
        }
        let valid = match authorization {
            AuthorizedCorrection::ProtectedEntity {
                source_start_byte,
                source_end_byte,
                output_start_byte,
                output_end_byte,
                source_value,
                replacement_value,
                kind,
            } => {
                source.get(*source_start_byte..*source_end_byte) == Some(source_value.as_str())
                    && output.get(*output_start_byte..*output_end_byte)
                        == Some(replacement_value.as_str())
                    && source_spans.iter().any(|span| {
                        span.start_byte == *source_start_byte
                            && span.end_byte == *source_end_byte
                            && span.kind == *kind
                    })
                    && output_spans.iter().any(|span| {
                        span.start_byte == *output_start_byte
                            && span.end_byte == *output_end_byte
                            && span.kind == *kind
                    })
            }
            AuthorizedCorrection::ImmediateDecisionRestatement {
                affirmative_range,
                cue_range,
                corrective_range,
                output_corrective_range,
                actor,
                action,
                superseded_destination,
                final_destination,
            } => {
                let valid_source_range = |range: &std::ops::Range<usize>| {
                    range.start < range.end
                        && range.end <= source.len()
                        && source.is_char_boundary(range.start)
                        && source.is_char_boundary(range.end)
                };
                valid_source_range(affirmative_range)
                    && valid_source_range(cue_range)
                    && valid_source_range(corrective_range)
                    && affirmative_range.end <= cue_range.start
                    && cue_range.end <= corrective_range.start
                    && source
                        .get(affirmative_range.end..cue_range.start)
                        .is_some_and(comma_only_gap)
                    && source
                        .get(cue_range.end..corrective_range.start)
                        .is_some_and(comma_only_gap)
                    && !source_literals.iter().any(|literal| {
                        [affirmative_range, cue_range, corrective_range]
                            .iter()
                            .any(|range| range.start < literal.end && literal.start < range.end)
                    })
                    && source.get(cue_range.clone()) == Some("不")
                    && parse_immediate_decision_parts(
                        source.get(affirmative_range.clone()).unwrap_or_default(),
                        source.get(cue_range.clone()).unwrap_or_default(),
                        source.get(corrective_range.clone()).unwrap_or_default(),
                    )
                    .is_some_and(|facts| {
                        facts.actor == *actor
                            && facts.action == *action
                            && facts.superseded_destination == *superseded_destination
                            && facts.final_destination == *final_destination
                    })
                    && output.get(output_corrective_range.clone())
                        == source.get(corrective_range.clone())
            }
            AuthorizedCorrection::SpokenRestartRemoval {
                source_segment_range,
                source_range,
                filler_range,
                abandoned_range,
                cue_range,
                repeated_anchor_range,
                output_segment_range,
            } => spoken_restart_authorization_matches(
                source,
                output,
                source_segment_range,
                source_range,
                filler_range,
                abandoned_range,
                cue_range,
                repeated_anchor_range,
                output_segment_range,
            ),
            AuthorizedCorrection::StandalonePrefaceFillerRemoval {
                source_segment_range,
                filler_range,
                separator_range,
                output_segment_range,
            } => standalone_preface_filler_authorization_matches(
                source,
                output,
                source_segment_range,
                filler_range,
                separator_range,
                output_segment_range,
            ),
        };
        if !valid {
            return false;
        }
        previous_source_end = source_end;
        previous_output_end = output_end;
    }
    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SpokenRestartRanges {
    source_range: std::ops::Range<usize>,
    filler_range: std::ops::Range<usize>,
    abandoned_range: std::ops::Range<usize>,
    cue_range: std::ops::Range<usize>,
    repeated_anchor_range: std::ops::Range<usize>,
}

#[expect(
    clippy::too_many_arguments,
    reason = "source and output byte ranges remain distinct authorization facts"
)]
fn spoken_restart_authorization_matches(
    source: &str,
    output: &str,
    source_segment_range: &std::ops::Range<usize>,
    source_range: &std::ops::Range<usize>,
    filler_range: &std::ops::Range<usize>,
    abandoned_range: &std::ops::Range<usize>,
    cue_range: &std::ops::Range<usize>,
    repeated_anchor_range: &std::ops::Range<usize>,
    output_segment_range: &std::ops::Range<usize>,
) -> bool {
    let valid_range = |text: &str, range: &std::ops::Range<usize>| {
        range.start < range.end
            && range.end <= text.len()
            && text.is_char_boundary(range.start)
            && text.is_char_boundary(range.end)
    };
    if !valid_range(source, source_segment_range)
        || !valid_range(source, source_range)
        || !valid_range(source, filler_range)
        || !valid_range(source, abandoned_range)
        || !valid_range(source, cue_range)
        || !valid_range(source, repeated_anchor_range)
        || !valid_range(output, output_segment_range)
        || source_range.start < source_segment_range.start
        || source_range.end > source_segment_range.end
        || filler_range.start != source_range.start
        || filler_range.end > abandoned_range.start
        || abandoned_range.end > cue_range.start
        || cue_range.end > source_range.end
        || repeated_anchor_range.start < source_range.end
        || repeated_anchor_range.end > source_segment_range.end
    {
        return false;
    }
    let Some(source_segment) = source.get(source_segment_range.clone()) else {
        return false;
    };
    let Some(output_segment) = output.get(output_segment_range.clone()) else {
        return false;
    };
    let Some(parsed) = parse_spoken_restart(source_segment) else {
        return false;
    };
    let Some(separator_gap) =
        source_segment.get(parsed.abandoned_range.end..parsed.cue_range.start)
    else {
        return false;
    };
    if skip_restart_separators(separator_gap, 0) != separator_gap.len() {
        return false;
    }
    let localize = |range: &std::ops::Range<usize>| {
        range.start - source_segment_range.start..range.end - source_segment_range.start
    };
    if localize(source_range) != parsed.source_range
        || localize(filler_range) != parsed.filler_range
        || localize(abandoned_range) != parsed.abandoned_range
        || localize(cue_range) != parsed.cue_range
        || localize(repeated_anchor_range) != parsed.repeated_anchor_range
    {
        return false;
    }
    let Some(prefix) = source_segment.get(..parsed.source_range.start) else {
        return false;
    };
    let Some(suffix) = source_segment.get(parsed.source_range.end..) else {
        return false;
    };
    prefix.to_owned() + suffix == output_segment
}

fn standalone_preface_filler_authorization_matches(
    source: &str,
    output: &str,
    source_segment_range: &std::ops::Range<usize>,
    filler_range: &std::ops::Range<usize>,
    separator_range: &std::ops::Range<usize>,
    output_segment_range: &std::ops::Range<usize>,
) -> bool {
    let valid_range = |text: &str, range: &std::ops::Range<usize>| {
        range.start < range.end
            && range.end <= text.len()
            && text.is_char_boundary(range.start)
            && text.is_char_boundary(range.end)
    };
    if source_segment_range.start != 0
        || !valid_range(source, source_segment_range)
        || !valid_range(source, filler_range)
        || !valid_range(source, separator_range)
        || !valid_range(output, output_segment_range)
    {
        return false;
    }
    let Some((canonical_filler, canonical_separator)) =
        parse_standalone_preface_filler(source, source_segment_range)
    else {
        return false;
    };
    if filler_range != &canonical_filler || separator_range != &canonical_separator {
        return false;
    }
    let Some(source_segment) = source.get(source_segment_range.clone()) else {
        return false;
    };
    let Some(output_segment) = output.get(output_segment_range.clone()) else {
        return false;
    };
    let Some(prefix) = source_segment.get(..filler_range.start) else {
        return false;
    };
    let relative_separator_end = separator_range.end - source_segment_range.start;
    let Some(suffix) = source_segment.get(relative_separator_end..) else {
        return false;
    };
    format!("{prefix}{suffix}") == output_segment
}

fn parse_standalone_preface_filler(
    source: &str,
    source_segment_range: &std::ops::Range<usize>,
) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    if source_segment_range.start != 0 || source_segment_range.end > source.len() {
        return None;
    }
    let source_segment = source.get(source_segment_range.clone())?;
    if split_sentences(source).first().map(String::as_str) != Some(source_segment) {
        return None;
    }
    let filler_start = source_segment.len() - source_segment.trim_start().len();
    let filler_range =
        crate::spoken_layout::standalone_filler_token_range(source, filler_start, false)?;
    // This source-bound preface rule recognizes only these exact spoken
    // hesitations; it does not waive other filler tokens globally.
    if !matches!(source.get(filler_range.clone()), Some("嗯" | "呃")) {
        return None;
    }
    let separator_end =
        filler_range.end + source.get(filler_range.end..)?.chars().next()?.len_utf8();
    let separator_range = filler_range.end..separator_end;
    if !matches!(source.get(separator_range.clone()), Some("," | "，")) {
        return None;
    }
    let literals = literal_ranges(source);
    if literals.iter().any(|range| {
        range.start < source_segment_range.end && source_segment_range.start < range.end
    }) {
        return None;
    }
    let protected = crate::protected_span::protected_spans(
        source_segment,
        Some(crate::llm::CleanupOperation::Cleanup),
    );
    let removed_range = filler_range.start..separator_range.end;
    if protected
        .iter()
        .any(|span| span.start_byte < removed_range.end && removed_range.start < span.end_byte)
        || crate::protected_span::contains_negation(source_segment)
        || crate::spoken_layout::has_hypothetical_introducer(source_segment)
        || source_segment
            .chars()
            .any(|ch| matches!(ch, '?' | '？' | '!' | '！'))
        || !has_meaningful_preface_tail(&source[separator_range.end..source_segment_range.end])
    {
        return None;
    }
    Some((filler_range, separator_range))
}

fn has_meaningful_preface_tail(mut tail: &str) -> bool {
    loop {
        tail = tail.trim_start_matches(|ch: char| {
            ch.is_whitespace()
                || matches!(
                    ch,
                    ',' | '，'
                        | '。'
                        | '.'
                        | '、'
                        | ';'
                        | '；'
                        | ':'
                        | '：'
                        | '!'
                        | '?'
                        | '！'
                        | '？'
                )
        });
        if tail.is_empty() {
            return false;
        }
        if let Some(filler) = crate::spoken_layout::standalone_filler_token_range(tail, 0, false) {
            tail = &tail[filler.end..];
            continue;
        }
        return tail
            .chars()
            .any(|ch| ch.is_ascii_alphanumeric() || is_cjk(ch));
    }
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
            authorization.shift_output(output_offset);
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
        authorizations: vec![AuthorizedCorrection::ProtectedEntity {
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
    if let Some((replaced, authorization)) =
        apply_immediate_decision_restatement(body, source_start_byte, literal_ranges)
    {
        return (attach_closer(&replaced, closer), Some(authorization));
    }
    if let Some((replaced, authorization)) =
        apply_source_proved_spoken_restart(sentence, source_start_byte, literal_ranges)
    {
        return (replaced, Some(authorization));
    }
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
            Some(AuthorizedCorrection::ProtectedEntity {
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
        let (authorization_start, authorization_end) = authorization.source_bounds();
        if !overlaps_literal(literal_ranges, authorization_start, authorization_end) {
            return (attach_closer(&replaced, closer), Some(authorization));
        }
    }
    if source_start_byte == 0 && full_source.get(..sentence.len()) == Some(sentence) {
        let source_segment_range = 0..sentence.len();
        if let Some((filler_range, separator_range)) =
            parse_standalone_preface_filler(full_source, &source_segment_range)
        {
            let prefix = &sentence[..filler_range.start];
            let suffix = &sentence[separator_range.end..];
            let text = format!("{prefix}{suffix}");
            let output_segment_range = 0..text.len();
            return (
                text,
                Some(AuthorizedCorrection::StandalonePrefaceFillerRemoval {
                    source_segment_range,
                    filler_range,
                    separator_range,
                    output_segment_range,
                }),
            );
        }
    }
    (sentence.to_owned(), None)
}

fn apply_source_proved_spoken_restart(
    sentence: &str,
    source_start_byte: usize,
    full_source_literals: &[std::ops::Range<usize>],
) -> Option<(String, AuthorizedCorrection)> {
    let ranges = parse_spoken_restart(sentence)?;
    let source_range =
        source_start_byte + ranges.source_range.start..source_start_byte + ranges.source_range.end;
    let filler_range =
        source_start_byte + ranges.filler_range.start..source_start_byte + ranges.filler_range.end;
    let abandoned_range = source_start_byte + ranges.abandoned_range.start
        ..source_start_byte + ranges.abandoned_range.end;
    let cue_range =
        source_start_byte + ranges.cue_range.start..source_start_byte + ranges.cue_range.end;
    let repeated_anchor_range = source_start_byte + ranges.repeated_anchor_range.start
        ..source_start_byte + ranges.repeated_anchor_range.end;
    if full_source_literals.iter().any(|literal| {
        [
            &source_range,
            &filler_range,
            &abandoned_range,
            &cue_range,
            &repeated_anchor_range,
        ]
        .into_iter()
        .any(|range| range.start < literal.end && literal.start < range.end)
    }) {
        return None;
    }
    let output = format!(
        "{}{}",
        &sentence[..ranges.source_range.start],
        &sentence[ranges.source_range.end..]
    );
    let output_segment_range = 0..output.len();
    Some((
        output,
        AuthorizedCorrection::SpokenRestartRemoval {
            source_segment_range: source_start_byte..source_start_byte + sentence.len(),
            source_range,
            filler_range,
            abandoned_range,
            cue_range,
            repeated_anchor_range,
            output_segment_range,
        },
    ))
}

fn parse_spoken_restart(text: &str) -> Option<SpokenRestartRanges> {
    let literals = literal_ranges(text);
    let protected =
        crate::protected_span::protected_spans(text, Some(crate::llm::CleanupOperation::Cleanup));
    let cues = text.match_indices("不对不对").collect::<Vec<_>>();
    if cues.len() != 1 {
        return None;
    }
    let (cue_start, cue_value) = cues[0];
    let cue_end = cue_start + cue_value.len();
    let mut restart = None;
    for (start, _) in text.char_indices() {
        let Some(filler_range) =
            crate::spoken_layout::standalone_filler_token_range(text, start, false)
        else {
            continue;
        };
        if text.get(filler_range.clone()) != Some("呃") || filler_range.end >= cue_start {
            continue;
        }
        let prefix = &text[..filler_range.start];
        if crate::spoken_layout::has_hypothetical_introducer(prefix)
            || prefix
                .chars()
                .any(|ch| matches!(ch, '?' | '？' | '!' | '！'))
        {
            continue;
        }
        let filler_separator_end = skip_restart_separators(text, filler_range.end);
        if filler_separator_end == filler_range.end || filler_separator_end >= cue_start {
            continue;
        }
        let fragment_end = trim_restart_separator_suffix(text, filler_separator_end, cue_start);
        if fragment_end <= filler_separator_end {
            continue;
        }
        let fragment_range = filler_separator_end..fragment_end;
        let fragment = text.get(fragment_range.clone())?;
        if crate::spoken_layout::has_hypothetical_introducer(fragment)
            || fragment.chars().any(|ch| {
                matches!(
                    ch,
                    ',' | '，' | ';' | '；' | ':' | '：' | '?' | '？' | '!' | '！'
                )
            })
            || fragment.contains("对不对")
            || crate::protected_span::contains_negation(fragment)
            || intersects_protected_content(text, fragment_range.start, fragment_range.end)
            || protected.iter().any(|span| {
                fragment_range.start < span.end_byte && span.start_byte < fragment_range.end
            })
            || literals.iter().any(|literal| {
                fragment_range.start < literal.end && literal.start < fragment_range.end
            })
        {
            continue;
        }
        let after_cue = skip_restart_separators(text, cue_end);
        if after_cue == cue_end || after_cue >= text.len() {
            continue;
        }
        let tail = &text[after_cue..];
        let anchor_matches = tail
            .match_indices(fragment)
            .map(|(relative, value)| {
                let start = after_cue + relative;
                start..start + value.len()
            })
            .filter(|range| {
                !literals
                    .iter()
                    .any(|literal| range.start < literal.end && literal.start < range.end)
                    && !protected
                        .iter()
                        .any(|span| range.start < span.end_byte && span.start_byte < range.end)
            })
            .collect::<Vec<_>>();
        if anchor_matches.len() != 1 {
            continue;
        }
        let repeated_anchor_range = anchor_matches[0].clone();
        let source_range = filler_range.start..after_cue;
        if literals
            .iter()
            .any(|literal| source_range.start < literal.end && literal.start < source_range.end)
        {
            continue;
        }
        let candidate = SpokenRestartRanges {
            source_range,
            filler_range: filler_range.clone(),
            abandoned_range: fragment_range,
            cue_range: cue_start..cue_end,
            repeated_anchor_range,
        };
        if restart.is_some() {
            return None;
        }
        restart = Some(candidate);
    }
    restart
}

fn skip_restart_separators(text: &str, mut offset: usize) -> usize {
    while let Some(ch) = text.get(offset..).and_then(|tail| tail.chars().next()) {
        if ch.is_whitespace() || matches!(ch, ',' | '，' | '、') {
            offset += ch.len_utf8();
        } else {
            break;
        }
    }
    offset
}

fn trim_restart_separator_suffix(text: &str, start: usize, end: usize) -> usize {
    let mut trimmed_end = end;
    while trimmed_end > start {
        let ch = text[..trimmed_end].chars().next_back().unwrap_or_default();
        if ch.is_whitespace() || matches!(ch, ',' | '，' | '、') {
            trimmed_end -= ch.len_utf8();
        } else {
            break;
        }
    }
    trimmed_end
}

fn has_explicit_edit_boundary(text: &str, marker_start: usize) -> bool {
    text[..marker_start]
        .trim_end()
        .chars()
        .next_back()
        .is_some_and(|ch| matches!(ch, ',' | '，' | ';' | '；' | ':' | '：' | '—'))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImmediateDecisionFacts {
    actor: String,
    action: String,
    superseded_destination: String,
    final_destination: String,
}

fn parse_immediate_decision_parts(
    affirmative: &str,
    cue: &str,
    corrective: &str,
) -> Option<ImmediateDecisionFacts> {
    if cue != "不" {
        return None;
    }
    let affirmative = affirmative.trim();
    let corrective = corrective.trim();
    let affirmative_action = ["我要去", "我准备去", "我打算去"]
        .into_iter()
        .find(|prefix| {
            affirmative
                .get(..prefix.len())
                .is_some_and(|value| value == *prefix)
        })?;
    let old_destination = affirmative[affirmative_action.len()..].trim();
    if old_destination.is_empty()
        || old_destination
            .chars()
            .any(|ch| matches!(ch, '，' | ',' | '。' | ';' | '；'))
    {
        return None;
    }
    let correction_prefix = "我决定不去";
    let correction = corrective.strip_prefix(correction_prefix)?;
    let split_at = correction
        .find("，去")
        .map(|index| (index, "，去".len()))
        .or_else(|| correction.find(",去").map(|index| (index, ",去".len())))?;
    let corrected_old_destination = correction[..split_at.0].trim();
    let final_destination = correction[split_at.0 + split_at.1..].trim();
    if corrected_old_destination.is_empty()
        || corrected_old_destination != old_destination
        || final_destination.is_empty()
        || final_destination
            .chars()
            .any(|ch| matches!(ch, '，' | ',' | '。' | ';' | '；'))
        || final_destination == old_destination
    {
        return None;
    }
    Some(ImmediateDecisionFacts {
        actor: "我".into(),
        action: "去".into(),
        superseded_destination: old_destination.to_owned(),
        final_destination: final_destination.to_owned(),
    })
}

fn comma_only_gap(gap: &str) -> bool {
    matches!(gap.trim_matches(char::is_whitespace), "," | "，")
}

fn trimmed_byte_range(text: &str, start: usize, end: usize) -> std::ops::Range<usize> {
    let slice = &text[start..end];
    let leading = slice.len() - slice.trim_start().len();
    let trailing = slice.trim_end().len();
    start + leading..start + trailing
}

fn apply_immediate_decision_restatement(
    body: &str,
    source_start_byte: usize,
    literal_ranges: &[std::ops::Range<usize>],
) -> Option<(String, AuthorizedCorrection)> {
    let candidates = ["，不，", ",不，", "，不,", ",不,"]
        .iter()
        .filter_map(|delimiter| body.find(delimiter).map(|start| (start, *delimiter)))
        .collect::<Vec<_>>();
    let [(cue_delimiter_start, delimiter)] = candidates.as_slice() else {
        return None;
    };
    let cue_offset = delimiter.find('不')?;
    let cue_start = cue_delimiter_start + cue_offset;
    let cue_end = cue_start + "不".len();
    let affirmative_range = trimmed_byte_range(body, 0, *cue_delimiter_start);
    let corrective_range =
        trimmed_byte_range(body, *cue_delimiter_start + delimiter.len(), body.len());
    if affirmative_range.is_empty() || corrective_range.is_empty() {
        return None;
    }
    let facts = parse_immediate_decision_parts(
        body.get(affirmative_range.clone())?,
        body.get(cue_start..cue_end)?,
        body.get(corrective_range.clone())?,
    )?;
    let absolute = |range: std::ops::Range<usize>| {
        source_start_byte + range.start..source_start_byte + range.end
    };
    let affirmative_range = absolute(affirmative_range);
    let cue_range = absolute(cue_start..cue_end);
    let corrective_range = absolute(corrective_range);
    if literal_ranges.iter().any(|literal| {
        [&affirmative_range, &cue_range, &corrective_range]
            .iter()
            .any(|range| range.start < literal.end && literal.start < range.end)
    }) {
        return None;
    }
    let local_output_range = body.get(
        corrective_range.start - source_start_byte..corrective_range.end - source_start_byte,
    )?;
    let output_corrective_range = 0..local_output_range.len();
    Some((
        local_output_range.to_owned(),
        AuthorizedCorrection::ImmediateDecisionRestatement {
            affirmative_range,
            cue_range,
            corrective_range,
            output_corrective_range,
            actor: facts.actor,
            action: facts.action,
            superseded_destination: facts.superseded_destination,
            final_destination: facts.final_destination,
        },
    ))
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
        AuthorizedCorrection::ProtectedEntity {
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

pub(crate) fn ascii_boundaries(text: &str, start: usize, end: usize) -> bool {
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

    #[test]
    fn immediate_negated_decision_restatement_is_source_bound_and_keeps_final_negation() {
        let source = "我要去 A，不，我决定不去 A，去 B。";
        let revision = apply_with_authorizations(source);
        assert_eq!(revision.text, "我决定不去 A，去 B。");
        assert_eq!(revision.authorizations.len(), 1);
        assert!(authorizations_match_source_and_output(
            source,
            &revision.text,
            &revision.authorizations
        ));
        let (final_text, reason, _) = crate::guard_final_output_with_reason(
            &revision.text,
            &revision.text,
            crate::FinalizationContext {
                scene: crate::ResolvedCleanupScene::observed(
                    crate::context::ContextFamily::General,
                    crate::context::FocusKind::Editable,
                ),
                operation: crate::llm::CleanupOperation::Cleanup,
                revision_source: Some(source),
                prepared_transcript: Some(&revision.text),
                revision_authorizations: &revision.authorizations,
                promoted_pair_protections: &[],
                list_revision_proof: None,
                structure_safe_source: None,
                output_projection_list_revision_proof: None,
                layout_source: None,
                exact_fallback: false,
            },
        );
        assert_eq!(reason, None);
        assert_eq!(final_text, "我决定不去 A，去 B。");

        let changed_final_destination = revision.text.replace("去 B", "去 C");
        assert!(!authorizations_match_source_and_output(
            source,
            &changed_final_destination,
            &revision.authorizations
        ));

        for raw in [
            "如果我要去 A，不，我决定不去 A，去 B。",
            "我要去 A，不，我决定不去 C，去 B。",
            "我要去 A，不，我决定去 A，去 B。",
            "我要去 A，不，我决定不去 A，去 A。",
            "我要去 A，讨论‘不’，我决定不去 A，去 B。",
        ] {
            let result = apply_with_authorizations(raw);
            assert_eq!(result.text, raw, "unproved decision rewrite: {raw}");
            assert!(result.authorizations.is_empty());
            assert!(authorizations_match_source_and_output(
                raw,
                &result.text,
                &[]
            ));
        }
    }

    #[test]
    fn source_proved_spoken_restart_is_normalized_before_shared_guards() {
        let source = "流程是什么？具体，呃，测试不对不对，帮我看一下软件具体测试什么。第一个是准确性。第二个是稳定性。";
        let revision = apply_with_authorizations(source);
        assert_eq!(
            revision.text,
            "流程是什么？具体，帮我看一下软件具体测试什么。第一个是准确性。第二个是稳定性。"
        );
        assert_eq!(revision.authorizations.len(), 1);
        assert!(authorizations_match_source_and_output(
            source,
            &revision.text,
            &revision.authorizations
        ));
        assert!(authorizations_match_source_and_output(source, source, &[]));
        assert!(!authorizations_match_source_and_output(
            source,
            &revision.text,
            &[]
        ));

        let (final_text, reason, _) = crate::guard_final_output_with_reason(
            &revision.text,
            &revision.text,
            crate::FinalizationContext {
                scene: crate::ResolvedCleanupScene::observed(
                    crate::context::ContextFamily::General,
                    crate::context::FocusKind::Editable,
                ),
                operation: crate::llm::CleanupOperation::Cleanup,
                revision_source: Some(source),
                prepared_transcript: Some(&revision.text),
                revision_authorizations: &revision.authorizations,
                promoted_pair_protections: &[],
                list_revision_proof: None,
                structure_safe_source: None,
                output_projection_list_revision_proof: None,
                layout_source: None,
                exact_fallback: false,
            },
        );
        assert_eq!(reason, None);
        assert_eq!(final_text, revision.text);

        let changed = revision.text.replace("准确性", "稳定性");
        assert!(!authorizations_match_source_and_output(
            source,
            &changed,
            &revision.authorizations
        ));

        let separated_source = "具体，呃，测试，不对不对，帮我看一下测试。";
        let separated = apply_with_authorizations(separated_source);
        assert_eq!(separated.text, "具体，帮我看一下测试。");
        assert!(authorizations_match_source_and_output(
            separated_source,
            &separated.text,
            &separated.authorizations
        ));

        let mut malformed = revision.authorizations.clone();
        if let AuthorizedCorrection::SpokenRestartRemoval { source_range, .. } = &mut malformed[0] {
            source_range.start += 1;
        }
        assert!(!authorizations_match_source_and_output(
            source,
            &revision.text,
            &malformed
        ));
        let mut overlapping = revision.authorizations.clone();
        overlapping.extend(revision.authorizations.clone());
        assert!(!authorizations_match_source_and_output(
            source,
            &revision.text,
            &overlapping
        ));
    }

    #[test]
    fn standalone_preface_filler_removal_is_canonical_and_source_bound() {
        let source = "嗯，先检查流程。第一个是稳定性。第二个是准确度。";
        let revision = apply_with_authorizations(source);
        assert_eq!(
            revision.text,
            "先检查流程。第一个是稳定性。第二个是准确度。"
        );
        assert_eq!(revision.authorizations.len(), 1);
        assert!(matches!(
            &revision.authorizations[0],
            AuthorizedCorrection::StandalonePrefaceFillerRemoval { .. }
        ));
        assert!(authorizations_match_source_and_output(
            source,
            &revision.text,
            &revision.authorizations
        ));
        assert!(authorizations_match_source_and_output(source, source, &[]));
        assert!(!authorizations_match_source_and_output(
            source,
            &revision.text,
            &[]
        ));

        let changed_output = revision.text.replace("稳定性", "延迟");
        assert!(!authorizations_match_source_and_output(
            source,
            &changed_output,
            &revision.authorizations
        ));

        let mut forged = revision.authorizations.clone();
        if let AuthorizedCorrection::StandalonePrefaceFillerRemoval {
            separator_range, ..
        } = &mut forged[0]
        {
            separator_range.end += 1;
        }
        assert!(!authorizations_match_source_and_output(
            source,
            &revision.text,
            &forged
        ));

        let prefixed_source = format!("引用：‘{source}’");
        let prefixed_output = format!("引用：‘{}’", revision.text);
        assert!(!authorizations_match_source_and_output(
            &prefixed_source,
            &prefixed_output,
            &revision.authorizations
        ));

        let er_source = "呃，先检查流程。第一个是稳定性。第二个是准确度。";
        let er_revision = apply_with_authorizations(er_source);
        assert_eq!(
            er_revision.text,
            "先检查流程。第一个是稳定性。第二个是准确度。"
        );
        assert_eq!(er_revision.authorizations.len(), 1);
        assert!(matches!(
            &er_revision.authorizations[0],
            AuthorizedCorrection::StandalonePrefaceFillerRemoval { .. }
        ));
        assert!(authorizations_match_source_and_output(
            er_source,
            &er_revision.text,
            &er_revision.authorizations
        ));
    }

    #[test]
    fn standalone_preface_filler_preserves_literals_questions_and_unknowns() {
        for source in [
            "引用：‘嗯，测试流程。’",
            "引用：‘呃，测试流程。’",
            "```text\n嗯，测试流程。\n```",
            "```text\n呃，测试流程。\n```",
            "嗯，",
            "呃，",
            "嗯？",
            "呃？",
            "嗯嗯，测试流程。",
            "嗯哼，测试流程。",
            "呃呃，测试流程。",
            "呃哈，测试流程。",
            "额，测试流程。",
            "Um, test the flow.",
            "嗯，嗯。",
            "呃，呃。",
            "嗯，如果只是举例，测试流程。",
            "呃，如果只是举例，测试流程。",
            "嗯，不要删除测试流程。",
            "呃，不要删除测试流程。",
            "普通前言嗯，测试流程。",
            "普通前言呃，测试流程。",
            "呃，`API v2`。",
        ] {
            let revision = apply_with_authorizations(source);
            assert_eq!(
                revision.text, source,
                "unproved preface filler changed: {source}"
            );
            assert!(revision.authorizations.is_empty());
            assert!(authorizations_match_source_and_output(
                source,
                &revision.text,
                &revision.authorizations
            ));
        }
    }

    #[test]
    fn standalone_preface_filler_keeps_protected_suffix_facts_exact() {
        let source = "嗯，测试一下这个流程，保留 API v2 和 /var/tmp/result.json。";
        let revision = apply_with_authorizations(source);
        assert_eq!(
            revision.text,
            "测试一下这个流程，保留 API v2 和 /var/tmp/result.json。"
        );
        assert_eq!(revision.authorizations.len(), 1);
        assert!(matches!(
            &revision.authorizations[0],
            AuthorizedCorrection::StandalonePrefaceFillerRemoval { .. }
        ));
        assert!(authorizations_match_source_and_output(
            source,
            &revision.text,
            &revision.authorizations
        ));
        for protected in ["这个流程", "API v2", "/var/tmp/result.json"] {
            assert!(revision.text.contains(protected), "lost {protected}");
        }

        let er_source = "呃，测试一下这个流程，保留 API v2 和 /var/tmp/result.json。";
        let er_revision = apply_with_authorizations(er_source);
        assert_eq!(
            er_revision.text,
            "测试一下这个流程，保留 API v2 和 /var/tmp/result.json。"
        );
        assert!(authorizations_match_source_and_output(
            er_source,
            &er_revision.text,
            &er_revision.authorizations
        ));
        for protected in ["这个流程", "API v2", "/var/tmp/result.json"] {
            assert!(er_revision.text.contains(protected), "lost {protected}");
        }
    }

    #[test]
    fn restart_authorization_cannot_be_forged_from_a_context_free_substring() {
        let inner = "具体，呃，测试不对不对，帮我看一下测试。";
        let inner_revision = apply_with_authorizations(inner);
        assert_eq!(inner_revision.authorizations.len(), 1);
        let AuthorizedCorrection::SpokenRestartRemoval {
            source_segment_range,
            source_range,
            filler_range,
            abandoned_range,
            cue_range,
            repeated_anchor_range,
            ..
        } = &inner_revision.authorizations[0]
        else {
            unreachable!("the narrow restart fixture must produce its typed proof")
        };

        for (prefix, suffix) in [("引用：‘", "’"), ("假设我只是在举例，", "")] {
            let full_source = format!("{prefix}{inner}{suffix}");
            let full_output = format!("{prefix}{}{suffix}", inner_revision.text);
            let source_offset = prefix.len();
            let output_offset = prefix.len();
            let forged = AuthorizedCorrection::SpokenRestartRemoval {
                source_segment_range: source_offset + source_segment_range.start
                    ..source_offset + source_segment_range.end,
                source_range: source_offset + source_range.start..source_offset + source_range.end,
                filler_range: source_offset + filler_range.start..source_offset + filler_range.end,
                abandoned_range: source_offset + abandoned_range.start
                    ..source_offset + abandoned_range.end,
                cue_range: source_offset + cue_range.start..source_offset + cue_range.end,
                repeated_anchor_range: source_offset + repeated_anchor_range.start
                    ..source_offset + repeated_anchor_range.end,
                output_segment_range: output_offset..output_offset + inner_revision.text.len(),
            };
            assert!(!authorizations_match_source_and_output(
                &full_source,
                &full_output,
                &[forged]
            ));
        }
    }

    #[test]
    fn spoken_restart_removal_preserves_literals_negation_and_unproved_cues() {
        for source in [
            "具体，呃，测试不对，帮我看一下测试。",
            "如果要讨论，呃，测试不对不对，帮我看一下测试。",
            "具体，呃，不要删除测试不对不对，帮我看一下测试。",
            "具体，呃，测试不对不对，帮我看一下流程。",
            "具体，呃哈，测试不对不对，帮我看一下测试。",
            "引用：‘具体，呃，测试不对不对，帮我看一下测试。’",
            "```text\n具体，呃，测试不对不对，帮我看一下测试。\n```",
            "具体，呃，API v2不对不对，帮我检查 API v2。",
        ] {
            let revision = apply_with_authorizations(source);
            assert_eq!(revision.text, source, "unproved restart changed: {source}");
            assert!(revision.authorizations.is_empty());
            assert!(authorizations_match_source_and_output(
                source,
                &revision.text,
                &revision.authorizations
            ));
        }
    }
}
