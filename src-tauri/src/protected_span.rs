//! Deterministic, offset-based preservation for factual, technical, and limited
//! deictic-reference spans.
//!
//! This guard checks exact source spans after cleanup; it does not establish
//! semantic equivalence or verify facts that were never present in the input.

use crate::llm::CleanupOperation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectedSpanKind {
    Url,
    Email,
    Path,
    Command,
    Flag,
    Date,
    DateWord,
    Amount,
    Version,
    Number,
    Term,
    DeicticReference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtectedSpan {
    pub start_byte: usize,
    pub end_byte: usize,
    pub kind: ProtectedSpanKind,
}

/// Finds nonoverlapping protected spans using earliest-start, then kind
/// priority, then longest-span resolution.
pub fn protected_spans(text: &str, operation: Option<CleanupOperation>) -> Vec<ProtectedSpan> {
    let mut candidates = Vec::new();
    collect_urls(text, &mut candidates);
    collect_emails(text, &mut candidates);
    collect_paths(text, &mut candidates);
    collect_flags(text, &mut candidates);
    collect_commands(text, &mut candidates);
    collect_dates(text, &mut candidates);
    collect_amounts(text, &mut candidates);
    collect_versions(text, &mut candidates);
    collect_numbers(text, &mut candidates);
    collect_terms(text, &mut candidates);
    if should_preserve_deictic_references(operation) {
        collect_deictic_references(text, &mut candidates);
    }
    if operation == Some(CleanupOperation::Translate) {
        candidates.retain(|span| {
            if span.kind == ProtectedSpanKind::DateWord {
                return false;
            }
            // Chinese weekday aliases end in a numeral character. That
            // character is part of the weekday, not an independent number;
            // weekday_counts below validates its day after localization.
            if span.kind == ProtectedSpanKind::Number && is_weekday_number_component(text, *span) {
                return false;
            }
            // A weekday can be translated as part of an authorized translation.
            // It is checked by canonical day below, while names such as Mike stay
            // exact protected terms.
            !(span.kind == ProtectedSpanKind::Term
                && is_weekday_alias(&text[span.start_byte..span.end_byte]))
        });
    }
    candidates.sort_by(|left, right| {
        left.start_byte
            .cmp(&right.start_byte)
            .then_with(|| priority(right.kind).cmp(&priority(left.kind)))
            .then_with(|| {
                (right.end_byte - right.start_byte).cmp(&(left.end_byte - left.start_byte))
            })
    });
    let mut selected: Vec<ProtectedSpan> = Vec::new();
    for span in candidates {
        if span.start_byte >= span.end_byte
            || !text.is_char_boundary(span.start_byte)
            || !text.is_char_boundary(span.end_byte)
            || selected
                .last()
                .is_some_and(|previous| span.start_byte < previous.end_byte)
        {
            continue;
        }
        selected.push(span);
    }
    selected
}

pub fn preserves(source: &str, candidate: &str, operation: Option<CleanupOperation>) -> bool {
    preserves_with_source_term_waivers(source, candidate, operation, &[])
}

/// Whether a source code literal lacks an exact candidate binding under the
/// same body, order, and attachment rules used by the protection guard.
pub(crate) fn code_literal_binding_failed(source: &str, candidate: &str) -> bool {
    !code_literal_payloads(source).is_empty() && bind_literals(source, candidate).is_err()
}

/// Check the normal preservation contract while waiving only source Term spans
/// that an upstream Cleanup owner has classified as spoken hesitation tokens.
/// Candidate terms, all non-Term source spans, and non-cleanup callers retain
/// the canonical protected-span inventory.
pub(crate) fn preserves_with_source_term_waivers(
    source: &str,
    candidate: &str,
    operation: Option<CleanupOperation>,
    source_term_waivers: &[std::ops::Range<usize>],
) -> bool {
    let source_term_waivers = if operation == Some(CleanupOperation::Cleanup) {
        source_term_waivers
    } else {
        &[]
    };
    let Ok(literal_bindings) = bind_literals(source, candidate) else {
        return false;
    };
    if should_preserve_deictic_references(operation)
        && deictic_reference_signature(source) != deictic_reference_signature(candidate)
    {
        return false;
    }
    let mut source_spans = protected_spans(source, operation);
    let mut candidate_spans = protected_spans(candidate, operation);
    // Add source-bound lexical alternatives before recognizing the same
    // inherited binary-sign syntax on either inventory. A bare candidate
    // such as `alpha+1` exposes its `1` only in this validated view.
    candidate_spans.extend(candidate_span_alternatives(&literal_bindings, operation));
    reclassify_inherited_binary_sign_numbers(
        source,
        candidate,
        &literal_bindings,
        &mut source_spans,
        &mut candidate_spans,
    );
    let source_spans = source_spans
        .into_iter()
        .filter(|span| {
            span.kind != ProtectedSpanKind::Term
                || !source_term_waivers
                    .iter()
                    .any(|range| range.start == span.start_byte && range.end == span.end_byte)
        })
        .collect::<Vec<_>>();
    candidate_spans.sort_by_key(|span| (span.start_byte, span.end_byte, span.kind as u8));
    candidate_spans.dedup();
    let mut candidate_end = 0;
    for source_span in source_spans {
        let source_token = &source[source_span.start_byte..source_span.end_byte];
        let found = candidate_spans.iter().find(|candidate_span| {
            if candidate_span.start_byte < candidate_end || candidate_span.kind != source_span.kind
            {
                return false;
            }
            let candidate_token = &candidate[candidate_span.start_byte..candidate_span.end_byte];
            if source_span.kind == ProtectedSpanKind::DeicticReference {
                source_token.eq_ignore_ascii_case(candidate_token)
            } else if matches_numeric_value(source_token, candidate_token, source_span.kind) {
                true
            } else {
                candidate_token == source_token
            }
        });
        let Some(candidate_span) = found else {
            return false;
        };
        candidate_end = candidate_span.end_byte;
    }
    preserves_negation(source, candidate, operation)
        && (operation != Some(CleanupOperation::Translate)
            || weekday_counts(source) == weekday_counts(candidate))
}

fn reclassify_inherited_binary_sign_numbers(
    source: &str,
    candidate: &str,
    plan: &VerifiedLiteralBindings<'_, '_>,
    source_spans: &mut [ProtectedSpan],
    candidate_spans: &mut [ProtectedSpan],
) {
    if plan.bindings.is_empty() {
        return;
    }
    let source_literals = code_literal_payloads(source);
    let candidate_literals = code_literal_payloads(candidate);

    for binding in &plan.bindings {
        if binding._boundary_witness != LocalCodeBoundaryWitness::SameInheritedEdge
            || source_literals
                .iter()
                .any(|literal| literal.outer_range == binding.source_outer && literal.block_fence)
            || binding.candidate_outer.as_ref().is_some_and(|outer| {
                candidate_literals
                    .iter()
                    .any(|literal| literal.outer_range == *outer && literal.block_fence)
            })
        {
            continue;
        }
        let candidate_edge = binding
            .candidate_outer
            .as_ref()
            .unwrap_or(&binding.candidate_body);
        let Some(source_suffix) =
            code_attachment_after(source, binding.source_outer.end, &source_literals)
        else {
            continue;
        };
        let Some(candidate_suffix) =
            code_attachment_after(candidate, candidate_edge.end, &candidate_literals)
        else {
            continue;
        };
        if source_suffix != candidate_suffix {
            continue;
        }

        reclassify_binary_number_spans(
            source,
            binding.source_outer.end,
            &source_suffix,
            source_spans,
        );
        reclassify_binary_number_spans(
            candidate,
            candidate_edge.end,
            &candidate_suffix,
            candidate_spans,
        );
    }
}

fn reclassify_binary_number_spans(
    text: &str,
    literal_edge: usize,
    suffix: &str,
    spans: &mut [ProtectedSpan],
) {
    for span in spans
        .iter_mut()
        .filter(|span| span.kind == ProtectedSpanKind::Number)
    {
        let Some(token) = text.get(span.start_byte..span.end_byte) else {
            continue;
        };
        let Some(sign) = token.chars().next().filter(|ch| matches!(*ch, '+' | '-')) else {
            continue;
        };
        let sign_len = sign.len_utf8();
        let Some(unsigned_number) = token.get(sign_len..) else {
            continue;
        };
        if !unsigned_number
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_digit())
            || span.start_byte < literal_edge
            || !text
                .get(literal_edge..span.start_byte)
                .is_some_and(|gap| gap.chars().all(char::is_whitespace))
        {
            continue;
        }
        let mut signed_number = String::with_capacity(unsigned_number.len() + sign_len);
        signed_number.push(sign);
        signed_number.push_str(unsigned_number);
        if suffix.starts_with(signed_number.as_str()) {
            // The sign remains a separately classified operator. This uses
            // the original contiguous digit range, never a synthesized span.
            span.start_byte += sign_len;
        }
    }
}

#[cfg(test)]
fn preserves_code_literals_exactly(source: &str, candidate: &str) -> bool {
    bind_literals(source, candidate).is_ok()
}

#[derive(Debug)]
struct VerifiedLiteralBindings<'source, 'candidate> {
    source: &'source str,
    candidate: &'candidate str,
    bindings: Vec<LiteralBinding>,
}

#[derive(Debug)]
struct LiteralBinding {
    source_outer: std::ops::Range<usize>,
    source_body: std::ops::Range<usize>,
    candidate_outer: Option<std::ops::Range<usize>>,
    candidate_body: std::ops::Range<usize>,
    _boundary_witness: LocalCodeBoundaryWitness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalCodeBoundaryWitness {
    SameInheritedEdge,
    NewCodeContinuation,
    AmbiguousCodeEdge,
}

#[derive(Debug, Clone)]
struct CodeLiteralPayload<'a> {
    text: &'a str,
    outer_range: std::ops::Range<usize>,
    body_range: std::ops::Range<usize>,
    block_fence: bool,
}

fn code_literal_payloads(text: &str) -> Vec<CodeLiteralPayload<'_>> {
    crate::spoken_layout::code_literal_ranges(text)
        .into_iter()
        .filter_map(|outer_range| {
            let range_text = text.get(outer_range.clone())?;
            let first_line_end =
                outer_range.start + range_text.find('\n').unwrap_or(range_text.len());
            let first_line = text.get(outer_range.start..first_line_end)?;
            let block_fence = markdown_fence(first_line).is_some();
            let body_range = code_literal_payload_range(text, &outer_range)?;
            Some(CodeLiteralPayload {
                text: text.get(body_range.clone())?,
                outer_range,
                body_range,
                block_fence,
            })
        })
        .collect()
}

fn bind_literals<'source, 'candidate>(
    source: &'source str,
    candidate: &'candidate str,
) -> Result<VerifiedLiteralBindings<'source, 'candidate>, ()> {
    if source == candidate {
        return Ok(VerifiedLiteralBindings {
            source,
            candidate,
            bindings: Vec::new(),
        });
    }
    let source_payloads = code_literal_payloads(source);
    let candidate_payloads = code_literal_payloads(candidate);
    if source_payloads.len() > 64 || candidate_payloads.len() > 64 {
        return Err(());
    }

    if source_payloads.is_empty() {
        return if candidate_payloads.is_empty() {
            Ok(VerifiedLiteralBindings {
                source,
                candidate,
                bindings: Vec::new(),
            })
        } else {
            Err(())
        };
    }

    // Only take the fast path when source literals are not ambiguous with
    // matching plain prose occurrences. Otherwise use monotone token alignment
    // so an earlier lookalike cannot stand in for a deleted code occurrence.
    let mut source_has_plain_lookalike = false;
    for (index, source_payload) in source_payloads.iter().enumerate() {
        if source_payloads[..index]
            .iter()
            .any(|previous| previous.text == source_payload.text)
        {
            continue;
        }
        let literal_count = source_payloads
            .iter()
            .filter(|other| other.text == source_payload.text)
            .count();
        if source_literal_occurrence_count(source, source_payload, &source_payloads)?
            > literal_count
        {
            source_has_plain_lookalike = true;
            break;
        }
    }

    // Retained, exact wrappers are distinct occurrence witnesses when no
    // identical plain occurrence in the source could be mistaken for one.
    // They must still pass the local edge check, including syntax outside ticks.
    if !source_has_plain_lookalike
        && source_payloads.len() == candidate_payloads.len()
        && source_payloads.iter().zip(&candidate_payloads).all(
            |(source_payload, candidate_payload)| {
                source_payload.text == candidate_payload.text
                    && literal_boundary_witness(
                        source,
                        &source_payload.outer_range,
                        &source_payloads,
                        candidate,
                        &candidate_payload.outer_range,
                        &candidate_payloads,
                    ) == LocalCodeBoundaryWitness::SameInheritedEdge
            },
        )
    {
        let bindings = source_payloads
            .iter()
            .zip(&candidate_payloads)
            .map(|(source_payload, candidate_payload)| LiteralBinding {
                source_outer: source_payload.outer_range.clone(),
                source_body: source_payload.body_range.clone(),
                candidate_outer: Some(candidate_payload.outer_range.clone()),
                candidate_body: candidate_payload.body_range.clone(),
                _boundary_witness: LocalCodeBoundaryWitness::SameInheritedEdge,
            })
            .collect();
        return Ok(VerifiedLiteralBindings {
            source,
            candidate,
            bindings,
        });
    }

    if candidate_payloads.iter().any(|candidate_payload| {
        !source_payloads
            .iter()
            .any(|source_payload| source_payload.text == candidate_payload.text)
    }) {
        return Err(());
    }

    let mut eligible_by_source = Vec::with_capacity(source_payloads.len());
    let mut needs_alignment = false;
    for (source_index, source_payload) in source_payloads.iter().enumerate() {
        let source_literal_count = source_payloads
            .iter()
            .filter(|other| other.text == source_payload.text)
            .count();
        let source_remaining_count = source_payloads[source_index..]
            .iter()
            .filter(|other| other.text == source_payload.text)
            .count();

        let source_occurrence_count =
            source_literal_occurrence_count(source, source_payload, &source_payloads)?;

        let mut eligible = Vec::new();
        for body_range in literal_occurrences(candidate, source_payload.text, &candidate_payloads)?
        {
            let wrapped_index = candidate_payloads
                .iter()
                .position(|payload| payload.body_range == body_range);
            let candidate_edge = wrapped_index
                .map(|index| candidate_payloads[index].outer_range.clone())
                .unwrap_or_else(|| body_range.clone());
            if literal_boundary_witness(
                source,
                &source_payload.outer_range,
                &source_payloads,
                candidate,
                &candidate_edge,
                &candidate_payloads,
            ) != LocalCodeBoundaryWitness::SameInheritedEdge
            {
                continue;
            }
            let replacement_range = wrapped_index
                .map(|index| candidate_payloads[index].outer_range.clone())
                .unwrap_or_else(|| body_range.clone());
            let occurrence = CandidateLiteralOccurrence {
                body_range,
                wrapped_index,
                replacement_range,
            };
            if !eligible
                .iter()
                .any(|existing: &CandidateLiteralOccurrence| {
                    existing.body_range == occurrence.body_range
                })
            {
                eligible.push(occurrence);
            }
        }

        if eligible.is_empty() {
            return Err(());
        }
        if source_occurrence_count > source_literal_count || eligible.len() > source_remaining_count
        {
            needs_alignment = true;
        }
        eligible_by_source.push(eligible);
    }

    let selected = if needs_alignment {
        align_ambiguous_literal_occurrences(
            source,
            candidate,
            &source_payloads,
            &eligible_by_source,
        )?
    } else {
        let mut selected = Vec::with_capacity(source_payloads.len());
        let mut candidate_cursor = 0;
        for eligible in &eligible_by_source {
            let Some(occurrence) = eligible
                .iter()
                .find(|occurrence| occurrence.body_range.start >= candidate_cursor)
            else {
                return Err(());
            };
            if occurrence.body_range.end <= occurrence.body_range.start {
                return Err(());
            }
            selected.push(occurrence.clone());
            candidate_cursor = occurrence.body_range.end;
        }
        selected
    };

    let mut bindings = Vec::with_capacity(source_payloads.len());
    let mut used_candidate_wrappers = vec![false; candidate_payloads.len()];
    let mut candidate_cursor = 0;
    for (source_payload, occurrence) in source_payloads.iter().zip(selected) {
        if occurrence.body_range.start < candidate_cursor
            || candidate.get(occurrence.body_range.clone()) != Some(source_payload.text)
        {
            return Err(());
        }
        let candidate_outer = occurrence
            .wrapped_index
            .map(|index| candidate_payloads[index].outer_range.clone());
        if let Some(index) = occurrence.wrapped_index {
            if used_candidate_wrappers[index] {
                return Err(());
            }
            used_candidate_wrappers[index] = true;
        }
        bindings.push(LiteralBinding {
            source_outer: source_payload.outer_range.clone(),
            source_body: source_payload.body_range.clone(),
            candidate_outer,
            candidate_body: occurrence.body_range.clone(),
            _boundary_witness: LocalCodeBoundaryWitness::SameInheritedEdge,
        });
        candidate_cursor = occurrence.body_range.end;
    }
    if used_candidate_wrappers.iter().any(|used| !used) {
        return Err(());
    }
    Ok(VerifiedLiteralBindings {
        source,
        candidate,
        bindings,
    })
}

/// Produce private layout views only after the same exact literal owner has
/// bound every code body to a distinct, ordered candidate occurrence. The
/// views hide wrapper metadata and code indentation from the prose layout
/// policy while preserving every body newline; exact body bytes remain proven
/// by `bind_literals` above.
pub(crate) fn validated_layout_views(source: &str, candidate: &str) -> Option<(String, String)> {
    let plan = bind_literals(source, candidate).ok()?;
    let source_view = project_layout_view(&plan, true)?;
    let candidate_view = project_layout_view(&plan, false)?;
    Some((source_view, candidate_view))
}

/// Return the exact source and candidate code regions only after the same
/// source-bound literal owner has matched every body to one ordered candidate
/// occurrence. The candidate range covers its retained wrapper when present,
/// or the bare body when wrappers were dropped. Layout repair uses these
/// private ranges to move only candidate-owned bytes; they are not guard
/// waivers or caller-supplied proof.
pub(crate) fn validated_code_region_pairs(
    source: &str,
    candidate: &str,
) -> Option<Vec<(std::ops::Range<usize>, std::ops::Range<usize>)>> {
    let source_literals = code_literal_payloads(source);
    if source_literals.is_empty() {
        return Some(Vec::new());
    }
    if source == candidate {
        return Some(
            source_literals
                .into_iter()
                .map(|literal| (literal.outer_range.clone(), literal.outer_range))
                .collect(),
        );
    }

    let plan = bind_literals(source, candidate).ok()?;
    if plan.bindings.len() != source_literals.len() {
        return None;
    }

    let mut pairs = Vec::with_capacity(plan.bindings.len());
    let mut source_end = 0;
    let mut candidate_end = 0;
    for binding in plan.bindings {
        let candidate_region = binding
            .candidate_outer
            .unwrap_or_else(|| binding.candidate_body.clone());
        if binding.source_outer.start < source_end
            || candidate_region.start < candidate_end
            || binding.source_outer.start >= binding.source_outer.end
            || candidate_region.start >= candidate_region.end
            || source.get(binding.source_outer.clone()).is_none()
            || candidate.get(candidate_region.clone()).is_none()
            || source.get(binding.source_body.clone())
                != candidate.get(binding.candidate_body.clone())
        {
            return None;
        }
        source_end = binding.source_outer.end;
        candidate_end = candidate_region.end;
        pairs.push((binding.source_outer, candidate_region));
    }
    Some(pairs)
}

/// Whether this source contains any parsed code literal.
pub(crate) fn has_code_literals(source: &str) -> bool {
    !code_literal_payloads(source).is_empty()
}

/// Whether this source contains a fenced block whose raw lines must never be
/// passed through the legacy prose break-reinsertion routine.
pub(crate) fn has_fenced_code_literals(source: &str) -> bool {
    code_literal_payloads(source)
        .iter()
        .any(|literal| literal.block_fence)
}

/// Recognize a clause boundary only when a parsed, closed source fence ends
/// before the token and the bytes between the fence and token are whitespace.
pub(crate) fn has_closed_fenced_block_boundary_before(source: &str, offset: usize) -> bool {
    if offset > source.len() || !source.is_char_boundary(offset) {
        return false;
    }
    code_literal_payloads(source).iter().any(|literal| {
        literal.block_fence
            && literal.outer_range.end <= offset
            && source
                .get(literal.outer_range.end..offset)
                .is_some_and(|gap| gap.chars().all(char::is_whitespace))
    })
}

fn project_layout_view(
    plan: &VerifiedLiteralBindings<'_, '_>,
    source_side: bool,
) -> Option<String> {
    let text = if source_side {
        plan.source
    } else {
        plan.candidate
    };
    let mut view = String::with_capacity(text.len());
    let mut cursor = 0;

    for binding in &plan.bindings {
        let (outer, body) = if source_side {
            (&binding.source_outer, &binding.source_body)
        } else {
            let wrapper = binding
                .candidate_outer
                .as_ref()
                .unwrap_or(&binding.candidate_body);
            (wrapper, &binding.candidate_body)
        };
        if outer.start < cursor
            || outer.start > body.start
            || body.end > outer.end
            || outer.end > text.len()
            || !text.is_char_boundary(outer.start)
            || !text.is_char_boundary(body.start)
            || !text.is_char_boundary(body.end)
            || !text.is_char_boundary(outer.end)
            || plan.source.get(binding.source_body.clone())
                != plan.candidate.get(binding.candidate_body.clone())
        {
            return None;
        }

        view.push_str(text.get(cursor..outer.start)?);
        view.push_str(&layout_code_placeholder(text.get(body.clone())?));
        cursor = outer.end;
    }
    view.push_str(text.get(cursor..)?);
    Some(view)
}

fn layout_code_placeholder(body: &str) -> String {
    let mut placeholder = String::with_capacity(body.len().max(4));
    if body.is_empty() {
        placeholder.push_str("CODE");
        return placeholder;
    }

    for line in body.split_inclusive('\n') {
        if let Some(line_without_lf) = line.strip_suffix('\n') {
            placeholder.push_str("CODE");
            if line_without_lf.ends_with('\r') {
                placeholder.push_str("\r\n");
            } else {
                placeholder.push('\n');
            }
        } else {
            placeholder.push_str("CODE");
        }
    }
    placeholder
}

fn source_literal_occurrence_count(
    source: &str,
    source_payload: &CodeLiteralPayload<'_>,
    source_payloads: &[CodeLiteralPayload<'_>],
) -> Result<usize, ()> {
    let mut count = 0;
    for range in literal_occurrences(source, source_payload.text, source_payloads)? {
        let occurrence_outer = source_payloads
            .iter()
            .find(|payload| payload.body_range == range)
            .map(|payload| payload.outer_range.clone())
            .unwrap_or_else(|| range.clone());
        if literal_boundary_witness(
            source,
            &source_payload.outer_range,
            source_payloads,
            source,
            &occurrence_outer,
            source_payloads,
        ) == LocalCodeBoundaryWitness::SameInheritedEdge
        {
            count += 1;
        }
    }
    Ok(count)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CandidateLiteralOccurrence {
    body_range: std::ops::Range<usize>,
    wrapped_index: Option<usize>,
    replacement_range: std::ops::Range<usize>,
}

#[derive(Debug, Clone)]
struct LiteralAlignmentToken {
    key: String,
    literal_body: Option<String>,
    source_literal_index: Option<usize>,
    candidate_literal: Option<CandidateLiteralOccurrence>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct LiteralAlignmentScore {
    matched_tokens: usize,
    bound_literals: usize,
}

const MAX_LITERAL_ALIGNMENT_TOKENS: usize = 256;

fn align_ambiguous_literal_occurrences(
    source: &str,
    candidate: &str,
    source_payloads: &[CodeLiteralPayload<'_>],
    eligible_by_source: &[Vec<CandidateLiteralOccurrence>],
) -> Result<Vec<CandidateLiteralOccurrence>, ()> {
    let source_tokens = source_literal_alignment_tokens(source, source_payloads)?;
    let candidate_tokens = candidate_literal_alignment_tokens(candidate, eligible_by_source)?;
    let source_count = source_tokens.len();
    let candidate_count = candidate_tokens.len();
    if source_count > MAX_LITERAL_ALIGNMENT_TOKENS || candidate_count > MAX_LITERAL_ALIGNMENT_TOKENS
    {
        return Err(());
    }

    let mut prefix =
        vec![vec![LiteralAlignmentScore::default(); candidate_count + 1]; source_count + 1];
    for source_index in 0..source_count {
        for candidate_index in 0..candidate_count {
            let mut best = prefix[source_index][candidate_index + 1]
                .max(prefix[source_index + 1][candidate_index]);
            if let Some(increment) = literal_alignment_match(
                &source_tokens[source_index],
                &candidate_tokens[candidate_index],
                eligible_by_source,
            ) {
                best = best.max(prefix[source_index][candidate_index].add(increment));
            }
            prefix[source_index + 1][candidate_index + 1] = best;
        }
    }

    let mut suffix =
        vec![vec![LiteralAlignmentScore::default(); candidate_count + 1]; source_count + 1];
    for source_index in (0..source_count).rev() {
        for candidate_index in (0..candidate_count).rev() {
            let mut best = suffix[source_index + 1][candidate_index]
                .max(suffix[source_index][candidate_index + 1]);
            if let Some(increment) = literal_alignment_match(
                &source_tokens[source_index],
                &candidate_tokens[candidate_index],
                eligible_by_source,
            ) {
                best = best.max(suffix[source_index + 1][candidate_index + 1].add(increment));
            }
            suffix[source_index][candidate_index] = best;
        }
    }

    let best = prefix[source_count][candidate_count];
    if best.bound_literals != source_payloads.len() {
        return Err(());
    }

    // Every optimal monotone alignment must consume each source literal. A
    // tie that can instead align a plain lookalike as prose is ambiguous and
    // must not let that lookalike stand in for the code occurrence.
    for source_index in 0..source_count {
        if source_tokens[source_index].source_literal_index.is_none() {
            continue;
        }
        let mut best_without_literal = LiteralAlignmentScore::default();
        for candidate_split in 0..=candidate_count {
            best_without_literal = best_without_literal.max(
                prefix[source_index][candidate_split]
                    .add(suffix[source_index + 1][candidate_split]),
            );
        }
        if best_without_literal.matched_tokens == best.matched_tokens {
            return Err(());
        }
    }

    let mut selected = Vec::with_capacity(source_payloads.len());
    for source_index in 0..source_count {
        let Some(literal_index) = source_tokens[source_index].source_literal_index else {
            continue;
        };
        let mut selected_range: Option<CandidateLiteralOccurrence> = None;
        for candidate_index in 0..candidate_count {
            let Some(increment) = literal_alignment_match(
                &source_tokens[source_index],
                &candidate_tokens[candidate_index],
                eligible_by_source,
            ) else {
                continue;
            };
            let total = prefix[source_index][candidate_index]
                .add(increment)
                .add(suffix[source_index + 1][candidate_index + 1]);
            if total != best {
                continue;
            }
            let Some(occurrence) = candidate_tokens[candidate_index].candidate_literal.clone()
            else {
                return Err(());
            };
            if let Some(previous) = &selected_range {
                if previous.body_range != occurrence.body_range {
                    return Err(());
                }
            } else {
                selected_range = Some(occurrence);
            }
        }
        let Some(occurrence) = selected_range else {
            return Err(());
        };
        if selected.len() != literal_index {
            return Err(());
        }
        selected.push(occurrence);
    }
    if selected.len() != source_payloads.len() {
        return Err(());
    }
    Ok(selected)
}

impl LiteralAlignmentScore {
    fn add(self, other: Self) -> Self {
        Self {
            matched_tokens: self.matched_tokens + other.matched_tokens,
            bound_literals: self.bound_literals + other.bound_literals,
        }
    }
}

fn literal_alignment_match(
    source: &LiteralAlignmentToken,
    candidate: &LiteralAlignmentToken,
    eligible_by_source: &[Vec<CandidateLiteralOccurrence>],
) -> Option<LiteralAlignmentScore> {
    if let Some(source_index) = source.source_literal_index {
        let body = source.literal_body.as_ref()?;
        let candidate_body = candidate.literal_body.as_ref()?;
        let occurrence = candidate.candidate_literal.as_ref()?;
        return (body == candidate_body
            && eligible_by_source
                .get(source_index)?
                .iter()
                .any(|eligible| eligible.body_range == occurrence.body_range))
        .then_some(LiteralAlignmentScore {
            matched_tokens: 1,
            bound_literals: 1,
        });
    }
    if candidate
        .candidate_literal
        .as_ref()
        .is_some_and(|occurrence| occurrence.wrapped_index.is_some())
    {
        // A prose word cannot borrow an explicitly retained code wrapper.
        return None;
    }
    (source.key == candidate.key).then_some(LiteralAlignmentScore {
        matched_tokens: 1,
        bound_literals: 0,
    })
}

fn source_literal_alignment_tokens(
    text: &str,
    payloads: &[CodeLiteralPayload<'_>],
) -> Result<Vec<LiteralAlignmentToken>, ()> {
    let mut tokens = Vec::new();
    let mut cursor = 0;
    for (literal_index, payload) in payloads.iter().enumerate() {
        push_alignment_words(text, cursor..payload.outer_range.start, &mut tokens)?;
        tokens.push(LiteralAlignmentToken {
            key: payload.text.to_lowercase(),
            literal_body: Some(payload.text.to_owned()),
            source_literal_index: Some(literal_index),
            candidate_literal: None,
        });
        cursor = payload.outer_range.end;
    }
    push_alignment_words(text, cursor..text.len(), &mut tokens)?;
    Ok(tokens)
}

fn candidate_literal_alignment_tokens(
    text: &str,
    eligible_by_source: &[Vec<CandidateLiteralOccurrence>],
) -> Result<Vec<LiteralAlignmentToken>, ()> {
    let mut occurrences = Vec::new();
    for occurrence in eligible_by_source.iter().flatten() {
        if !occurrences
            .iter()
            .any(|existing: &CandidateLiteralOccurrence| {
                existing.body_range == occurrence.body_range
            })
        {
            occurrences.push(occurrence.clone());
        }
    }
    occurrences.sort_by_key(|occurrence| occurrence.replacement_range.start);

    let mut tokens = Vec::new();
    let mut cursor = 0;
    for occurrence in occurrences {
        let replacement = occurrence.replacement_range.clone();
        if replacement.start < cursor || replacement.end <= replacement.start {
            return Err(());
        }
        push_alignment_words(text, cursor..replacement.start, &mut tokens)?;
        let body = text.get(occurrence.body_range.clone()).ok_or(())?;
        tokens.push(LiteralAlignmentToken {
            key: body.to_lowercase(),
            literal_body: Some(body.to_owned()),
            source_literal_index: None,
            candidate_literal: Some(occurrence.clone()),
        });
        cursor = replacement.end;
    }
    push_alignment_words(text, cursor..text.len(), &mut tokens)?;
    Ok(tokens)
}

fn push_alignment_words(
    text: &str,
    range: std::ops::Range<usize>,
    tokens: &mut Vec<LiteralAlignmentToken>,
) -> Result<(), ()> {
    let segment = text.get(range).ok_or(())?;
    let mut word_start = None;
    for (relative, ch) in segment.char_indices() {
        let is_word = is_code_identifier_continue(ch, true);
        if is_word {
            word_start.get_or_insert(relative);
        } else if let Some(start) = word_start.take() {
            let word = &segment[start..relative];
            tokens.push(LiteralAlignmentToken {
                key: word.to_lowercase(),
                literal_body: None,
                source_literal_index: None,
                candidate_literal: None,
            });
            if tokens.len() > MAX_LITERAL_ALIGNMENT_TOKENS {
                return Err(());
            }
        }
    }
    if let Some(start) = word_start {
        tokens.push(LiteralAlignmentToken {
            key: segment[start..].to_lowercase(),
            literal_body: None,
            source_literal_index: None,
            candidate_literal: None,
        });
        if tokens.len() > MAX_LITERAL_ALIGNMENT_TOKENS {
            return Err(());
        }
    }
    Ok(())
}

fn exact_occurrence_ranges(
    text: &str,
    needle: &str,
    limit: usize,
) -> Option<Vec<std::ops::Range<usize>>> {
    if needle.is_empty() {
        return Some(Vec::new());
    }
    let mut ranges = Vec::new();
    let mut cursor = 0;
    while cursor < text.len() {
        let Some(relative) = text[cursor..].find(needle) else {
            break;
        };
        let start = cursor + relative;
        let end = start + needle.len();
        ranges.push(start..end);
        if ranges.len() > limit {
            return None;
        }
        let Some(ch) = text[start..].chars().next() else {
            break;
        };
        cursor = start + ch.len_utf8();
    }
    Some(ranges)
}

fn literal_occurrences(
    text: &str,
    body: &str,
    payloads: &[CodeLiteralPayload<'_>],
) -> Result<Vec<std::ops::Range<usize>>, ()> {
    if body.is_empty() {
        return Ok(payloads
            .iter()
            .filter(|payload| payload.text.is_empty())
            .map(|payload| payload.body_range.clone())
            .collect());
    }
    exact_occurrence_ranges(text, body, 256).ok_or(())
}

// XID ranges copied verbatim from the lockfile-cached regex-syntax 0.8.11
// `src/unicode_tables/property_bool.rs` (SHA-256
// 66cf5bd2a1438bf9694152f077a285cf014fbd50b9dd63a97233b2ea61d64962).
// Generated by `ucd-generate property-bool ucd-16.0.0 --chars` (ucd-generate
// 0.3.1; Unicode 16.0.0). Range digests: XID_Start
// f969db8daf96036062465fb82a74a2967d33e8e6513f7571c26e3c2f8c9a12ca; XID_Continue
// 786635a8a2886ce6f953a377c57a7ecb0ee1a91360e1571e08bda28e08859f6a.
// Unicode 16.0.0 data: Copyright © 2024 Unicode, Inc.; Unicode License v3:
// https://www.unicode.org/license.txt. The full cached-crate notice is
// reproduced verbatim in the shipped `THIRD_PARTY_NOTICES.md` (original source
// SHA-256 74db5baf44a41b1000312c673544b3374e4198af5605c7f9080a402cec42cfa3).
const UNICODE_XID_START_RANGES: &[(char, char)] = &[
    ('A', 'Z'),
    ('a', 'z'),
    ('ª', 'ª'),
    ('µ', 'µ'),
    ('º', 'º'),
    ('À', 'Ö'),
    ('Ø', 'ö'),
    ('ø', 'ˁ'),
    ('ˆ', 'ˑ'),
    ('ˠ', 'ˤ'),
    ('ˬ', 'ˬ'),
    ('ˮ', 'ˮ'),
    ('Ͱ', 'ʹ'),
    ('Ͷ', 'ͷ'),
    ('ͻ', 'ͽ'),
    ('Ϳ', 'Ϳ'),
    ('Ά', 'Ά'),
    ('Έ', 'Ί'),
    ('Ό', 'Ό'),
    ('Ύ', 'Ρ'),
    ('Σ', 'ϵ'),
    ('Ϸ', 'ҁ'),
    ('Ҋ', 'ԯ'),
    ('Ա', 'Ֆ'),
    ('ՙ', 'ՙ'),
    ('ՠ', 'ֈ'),
    ('א', 'ת'),
    ('ׯ', 'ײ'),
    ('ؠ', 'ي'),
    ('ٮ', 'ٯ'),
    ('ٱ', 'ۓ'),
    ('ە', 'ە'),
    ('ۥ', 'ۦ'),
    ('ۮ', 'ۯ'),
    ('ۺ', 'ۼ'),
    ('ۿ', 'ۿ'),
    ('ܐ', 'ܐ'),
    ('ܒ', 'ܯ'),
    ('ݍ', 'ޥ'),
    ('ޱ', 'ޱ'),
    ('ߊ', 'ߪ'),
    ('ߴ', 'ߵ'),
    ('ߺ', 'ߺ'),
    ('ࠀ', 'ࠕ'),
    ('ࠚ', 'ࠚ'),
    ('ࠤ', 'ࠤ'),
    ('ࠨ', 'ࠨ'),
    ('ࡀ', 'ࡘ'),
    ('ࡠ', 'ࡪ'),
    ('ࡰ', 'ࢇ'),
    ('ࢉ', 'ࢎ'),
    ('ࢠ', 'ࣉ'),
    ('ऄ', 'ह'),
    ('ऽ', 'ऽ'),
    ('ॐ', 'ॐ'),
    ('क़', 'ॡ'),
    ('ॱ', 'ঀ'),
    ('অ', 'ঌ'),
    ('এ', 'ঐ'),
    ('ও', 'ন'),
    ('প', 'র'),
    ('ল', 'ল'),
    ('শ', 'হ'),
    ('ঽ', 'ঽ'),
    ('ৎ', 'ৎ'),
    ('ড়', 'ঢ়'),
    ('য়', 'ৡ'),
    ('ৰ', 'ৱ'),
    ('ৼ', 'ৼ'),
    ('ਅ', 'ਊ'),
    ('ਏ', 'ਐ'),
    ('ਓ', 'ਨ'),
    ('ਪ', 'ਰ'),
    ('ਲ', 'ਲ਼'),
    ('ਵ', 'ਸ਼'),
    ('ਸ', 'ਹ'),
    ('ਖ਼', 'ੜ'),
    ('ਫ਼', 'ਫ਼'),
    ('ੲ', 'ੴ'),
    ('અ', 'ઍ'),
    ('એ', 'ઑ'),
    ('ઓ', 'ન'),
    ('પ', 'ર'),
    ('લ', 'ળ'),
    ('વ', 'હ'),
    ('ઽ', 'ઽ'),
    ('ૐ', 'ૐ'),
    ('ૠ', 'ૡ'),
    ('ૹ', 'ૹ'),
    ('ଅ', 'ଌ'),
    ('ଏ', 'ଐ'),
    ('ଓ', 'ନ'),
    ('ପ', 'ର'),
    ('ଲ', 'ଳ'),
    ('ଵ', 'ହ'),
    ('ଽ', 'ଽ'),
    ('ଡ଼', 'ଢ଼'),
    ('ୟ', 'ୡ'),
    ('ୱ', 'ୱ'),
    ('ஃ', 'ஃ'),
    ('அ', 'ஊ'),
    ('எ', 'ஐ'),
    ('ஒ', 'க'),
    ('ங', 'ச'),
    ('ஜ', 'ஜ'),
    ('ஞ', 'ட'),
    ('ண', 'த'),
    ('ந', 'ப'),
    ('ம', 'ஹ'),
    ('ௐ', 'ௐ'),
    ('అ', 'ఌ'),
    ('ఎ', 'ఐ'),
    ('ఒ', 'న'),
    ('ప', 'హ'),
    ('ఽ', 'ఽ'),
    ('ౘ', 'ౚ'),
    ('ౝ', 'ౝ'),
    ('ౠ', 'ౡ'),
    ('ಀ', 'ಀ'),
    ('ಅ', 'ಌ'),
    ('ಎ', 'ಐ'),
    ('ಒ', 'ನ'),
    ('ಪ', 'ಳ'),
    ('ವ', 'ಹ'),
    ('ಽ', 'ಽ'),
    ('ೝ', 'ೞ'),
    ('ೠ', 'ೡ'),
    ('ೱ', 'ೲ'),
    ('ഄ', 'ഌ'),
    ('എ', 'ഐ'),
    ('ഒ', 'ഺ'),
    ('ഽ', 'ഽ'),
    ('ൎ', 'ൎ'),
    ('ൔ', 'ൖ'),
    ('ൟ', 'ൡ'),
    ('ൺ', 'ൿ'),
    ('අ', 'ඖ'),
    ('ක', 'න'),
    ('ඳ', 'ර'),
    ('ල', 'ල'),
    ('ව', 'ෆ'),
    ('ก', 'ะ'),
    ('า', 'า'),
    ('เ', 'ๆ'),
    ('ກ', 'ຂ'),
    ('ຄ', 'ຄ'),
    ('ຆ', 'ຊ'),
    ('ຌ', 'ຣ'),
    ('ລ', 'ລ'),
    ('ວ', 'ະ'),
    ('າ', 'າ'),
    ('ຽ', 'ຽ'),
    ('ເ', 'ໄ'),
    ('ໆ', 'ໆ'),
    ('ໜ', 'ໟ'),
    ('ༀ', 'ༀ'),
    ('ཀ', 'ཇ'),
    ('ཉ', 'ཬ'),
    ('ྈ', 'ྌ'),
    ('က', 'ဪ'),
    ('ဿ', 'ဿ'),
    ('ၐ', 'ၕ'),
    ('ၚ', 'ၝ'),
    ('ၡ', 'ၡ'),
    ('ၥ', 'ၦ'),
    ('ၮ', 'ၰ'),
    ('ၵ', 'ႁ'),
    ('ႎ', 'ႎ'),
    ('Ⴀ', 'Ⴥ'),
    ('Ⴧ', 'Ⴧ'),
    ('Ⴭ', 'Ⴭ'),
    ('ა', 'ჺ'),
    ('ჼ', 'ቈ'),
    ('ቊ', 'ቍ'),
    ('ቐ', 'ቖ'),
    ('ቘ', 'ቘ'),
    ('ቚ', 'ቝ'),
    ('በ', 'ኈ'),
    ('ኊ', 'ኍ'),
    ('ነ', 'ኰ'),
    ('ኲ', 'ኵ'),
    ('ኸ', 'ኾ'),
    ('ዀ', 'ዀ'),
    ('ዂ', 'ዅ'),
    ('ወ', 'ዖ'),
    ('ዘ', 'ጐ'),
    ('ጒ', 'ጕ'),
    ('ጘ', 'ፚ'),
    ('ᎀ', 'ᎏ'),
    ('Ꭰ', 'Ᏽ'),
    ('ᏸ', 'ᏽ'),
    ('ᐁ', 'ᙬ'),
    ('ᙯ', 'ᙿ'),
    ('ᚁ', 'ᚚ'),
    ('ᚠ', 'ᛪ'),
    ('ᛮ', 'ᛸ'),
    ('ᜀ', 'ᜑ'),
    ('ᜟ', 'ᜱ'),
    ('ᝀ', 'ᝑ'),
    ('ᝠ', 'ᝬ'),
    ('ᝮ', 'ᝰ'),
    ('ក', 'ឳ'),
    ('ៗ', 'ៗ'),
    ('ៜ', 'ៜ'),
    ('ᠠ', 'ᡸ'),
    ('ᢀ', 'ᢨ'),
    ('ᢪ', 'ᢪ'),
    ('ᢰ', 'ᣵ'),
    ('ᤀ', 'ᤞ'),
    ('ᥐ', 'ᥭ'),
    ('ᥰ', 'ᥴ'),
    ('ᦀ', 'ᦫ'),
    ('ᦰ', 'ᧉ'),
    ('ᨀ', 'ᨖ'),
    ('ᨠ', 'ᩔ'),
    ('ᪧ', 'ᪧ'),
    ('ᬅ', 'ᬳ'),
    ('ᭅ', 'ᭌ'),
    ('ᮃ', 'ᮠ'),
    ('ᮮ', 'ᮯ'),
    ('ᮺ', 'ᯥ'),
    ('ᰀ', 'ᰣ'),
    ('ᱍ', 'ᱏ'),
    ('ᱚ', 'ᱽ'),
    ('ᲀ', 'ᲊ'),
    ('Ა', 'Ჺ'),
    ('Ჽ', 'Ჿ'),
    ('ᳩ', 'ᳬ'),
    ('ᳮ', 'ᳳ'),
    ('ᳵ', 'ᳶ'),
    ('ᳺ', 'ᳺ'),
    ('ᴀ', 'ᶿ'),
    ('Ḁ', 'ἕ'),
    ('Ἐ', 'Ἕ'),
    ('ἠ', 'ὅ'),
    ('Ὀ', 'Ὅ'),
    ('ὐ', 'ὗ'),
    ('Ὑ', 'Ὑ'),
    ('Ὓ', 'Ὓ'),
    ('Ὕ', 'Ὕ'),
    ('Ὗ', 'ώ'),
    ('ᾀ', 'ᾴ'),
    ('ᾶ', 'ᾼ'),
    ('ι', 'ι'),
    ('ῂ', 'ῄ'),
    ('ῆ', 'ῌ'),
    ('ῐ', 'ΐ'),
    ('ῖ', 'Ί'),
    ('ῠ', 'Ῥ'),
    ('ῲ', 'ῴ'),
    ('ῶ', 'ῼ'),
    ('ⁱ', 'ⁱ'),
    ('ⁿ', 'ⁿ'),
    ('ₐ', 'ₜ'),
    ('ℂ', 'ℂ'),
    ('ℇ', 'ℇ'),
    ('ℊ', 'ℓ'),
    ('ℕ', 'ℕ'),
    ('℘', 'ℝ'),
    ('ℤ', 'ℤ'),
    ('Ω', 'Ω'),
    ('ℨ', 'ℨ'),
    ('K', 'ℹ'),
    ('ℼ', 'ℿ'),
    ('ⅅ', 'ⅉ'),
    ('ⅎ', 'ⅎ'),
    ('Ⅰ', 'ↈ'),
    ('Ⰰ', 'ⳤ'),
    ('Ⳬ', 'ⳮ'),
    ('Ⳳ', 'ⳳ'),
    ('ⴀ', 'ⴥ'),
    ('ⴧ', 'ⴧ'),
    ('ⴭ', 'ⴭ'),
    ('ⴰ', 'ⵧ'),
    ('ⵯ', 'ⵯ'),
    ('ⶀ', 'ⶖ'),
    ('ⶠ', 'ⶦ'),
    ('ⶨ', 'ⶮ'),
    ('ⶰ', 'ⶶ'),
    ('ⶸ', 'ⶾ'),
    ('ⷀ', 'ⷆ'),
    ('ⷈ', 'ⷎ'),
    ('ⷐ', 'ⷖ'),
    ('ⷘ', 'ⷞ'),
    ('々', '〇'),
    ('〡', '〩'),
    ('〱', '〵'),
    ('〸', '〼'),
    ('ぁ', 'ゖ'),
    ('ゝ', 'ゟ'),
    ('ァ', 'ヺ'),
    ('ー', 'ヿ'),
    ('ㄅ', 'ㄯ'),
    ('ㄱ', 'ㆎ'),
    ('ㆠ', 'ㆿ'),
    ('ㇰ', 'ㇿ'),
    ('㐀', '䶿'),
    ('一', 'ꒌ'),
    ('ꓐ', 'ꓽ'),
    ('ꔀ', 'ꘌ'),
    ('ꘐ', 'ꘟ'),
    ('ꘪ', 'ꘫ'),
    ('Ꙁ', 'ꙮ'),
    ('ꙿ', 'ꚝ'),
    ('ꚠ', 'ꛯ'),
    ('ꜗ', 'ꜟ'),
    ('Ꜣ', 'ꞈ'),
    ('Ꞌ', 'ꟍ'),
    ('Ꟑ', 'ꟑ'),
    ('ꟓ', 'ꟓ'),
    ('ꟕ', 'Ƛ'),
    ('ꟲ', 'ꠁ'),
    ('ꠃ', 'ꠅ'),
    ('ꠇ', 'ꠊ'),
    ('ꠌ', 'ꠢ'),
    ('ꡀ', 'ꡳ'),
    ('ꢂ', 'ꢳ'),
    ('ꣲ', 'ꣷ'),
    ('ꣻ', 'ꣻ'),
    ('ꣽ', 'ꣾ'),
    ('ꤊ', 'ꤥ'),
    ('ꤰ', 'ꥆ'),
    ('ꥠ', 'ꥼ'),
    ('ꦄ', 'ꦲ'),
    ('ꧏ', 'ꧏ'),
    ('ꧠ', 'ꧤ'),
    ('ꧦ', 'ꧯ'),
    ('ꧺ', 'ꧾ'),
    ('ꨀ', 'ꨨ'),
    ('ꩀ', 'ꩂ'),
    ('ꩄ', 'ꩋ'),
    ('ꩠ', 'ꩶ'),
    ('ꩺ', 'ꩺ'),
    ('ꩾ', 'ꪯ'),
    ('ꪱ', 'ꪱ'),
    ('ꪵ', 'ꪶ'),
    ('ꪹ', 'ꪽ'),
    ('ꫀ', 'ꫀ'),
    ('ꫂ', 'ꫂ'),
    ('ꫛ', 'ꫝ'),
    ('ꫠ', 'ꫪ'),
    ('ꫲ', 'ꫴ'),
    ('ꬁ', 'ꬆ'),
    ('ꬉ', 'ꬎ'),
    ('ꬑ', 'ꬖ'),
    ('ꬠ', 'ꬦ'),
    ('ꬨ', 'ꬮ'),
    ('ꬰ', 'ꭚ'),
    ('ꭜ', 'ꭩ'),
    ('ꭰ', 'ꯢ'),
    ('가', '힣'),
    ('ힰ', 'ퟆ'),
    ('ퟋ', 'ퟻ'),
    ('豈', '舘'),
    ('並', '龎'),
    ('ﬀ', 'ﬆ'),
    ('ﬓ', 'ﬗ'),
    ('יִ', 'יִ'),
    ('ײַ', 'ﬨ'),
    ('שׁ', 'זּ'),
    ('טּ', 'לּ'),
    ('מּ', 'מּ'),
    ('נּ', 'סּ'),
    ('ףּ', 'פּ'),
    ('צּ', 'ﮱ'),
    ('ﯓ', 'ﱝ'),
    ('ﱤ', 'ﴽ'),
    ('ﵐ', 'ﶏ'),
    ('ﶒ', 'ﷇ'),
    ('ﷰ', 'ﷹ'),
    ('ﹱ', 'ﹱ'),
    ('ﹳ', 'ﹳ'),
    ('ﹷ', 'ﹷ'),
    ('ﹹ', 'ﹹ'),
    ('ﹻ', 'ﹻ'),
    ('ﹽ', 'ﹽ'),
    ('ﹿ', 'ﻼ'),
    ('Ａ', 'Ｚ'),
    ('ａ', 'ｚ'),
    ('ｦ', 'ﾝ'),
    ('ﾠ', 'ﾾ'),
    ('ￂ', 'ￇ'),
    ('ￊ', 'ￏ'),
    ('ￒ', 'ￗ'),
    ('ￚ', 'ￜ'),
    ('𐀀', '𐀋'),
    ('𐀍', '𐀦'),
    ('𐀨', '𐀺'),
    ('𐀼', '𐀽'),
    ('𐀿', '𐁍'),
    ('𐁐', '𐁝'),
    ('𐂀', '𐃺'),
    ('𐅀', '𐅴'),
    ('𐊀', '𐊜'),
    ('𐊠', '𐋐'),
    ('𐌀', '𐌟'),
    ('𐌭', '𐍊'),
    ('𐍐', '𐍵'),
    ('𐎀', '𐎝'),
    ('𐎠', '𐏃'),
    ('𐏈', '𐏏'),
    ('𐏑', '𐏕'),
    ('𐐀', '𐒝'),
    ('𐒰', '𐓓'),
    ('𐓘', '𐓻'),
    ('𐔀', '𐔧'),
    ('𐔰', '𐕣'),
    ('𐕰', '𐕺'),
    ('𐕼', '𐖊'),
    ('𐖌', '𐖒'),
    ('𐖔', '𐖕'),
    ('𐖗', '𐖡'),
    ('𐖣', '𐖱'),
    ('𐖳', '𐖹'),
    ('𐖻', '𐖼'),
    ('𐗀', '𐗳'),
    ('𐘀', '𐜶'),
    ('𐝀', '𐝕'),
    ('𐝠', '𐝧'),
    ('𐞀', '𐞅'),
    ('𐞇', '𐞰'),
    ('𐞲', '𐞺'),
    ('𐠀', '𐠅'),
    ('𐠈', '𐠈'),
    ('𐠊', '𐠵'),
    ('𐠷', '𐠸'),
    ('𐠼', '𐠼'),
    ('𐠿', '𐡕'),
    ('𐡠', '𐡶'),
    ('𐢀', '𐢞'),
    ('𐣠', '𐣲'),
    ('𐣴', '𐣵'),
    ('𐤀', '𐤕'),
    ('𐤠', '𐤹'),
    ('𐦀', '𐦷'),
    ('𐦾', '𐦿'),
    ('𐨀', '𐨀'),
    ('𐨐', '𐨓'),
    ('𐨕', '𐨗'),
    ('𐨙', '𐨵'),
    ('𐩠', '𐩼'),
    ('𐪀', '𐪜'),
    ('𐫀', '𐫇'),
    ('𐫉', '𐫤'),
    ('𐬀', '𐬵'),
    ('𐭀', '𐭕'),
    ('𐭠', '𐭲'),
    ('𐮀', '𐮑'),
    ('𐰀', '𐱈'),
    ('𐲀', '𐲲'),
    ('𐳀', '𐳲'),
    ('𐴀', '𐴣'),
    ('𐵊', '𐵥'),
    ('𐵯', '𐶅'),
    ('𐺀', '𐺩'),
    ('𐺰', '𐺱'),
    ('𐻂', '𐻄'),
    ('𐼀', '𐼜'),
    ('𐼧', '𐼧'),
    ('𐼰', '𐽅'),
    ('𐽰', '𐾁'),
    ('𐾰', '𐿄'),
    ('𐿠', '𐿶'),
    ('𑀃', '𑀷'),
    ('𑁱', '𑁲'),
    ('𑁵', '𑁵'),
    ('𑂃', '𑂯'),
    ('𑃐', '𑃨'),
    ('𑄃', '𑄦'),
    ('𑅄', '𑅄'),
    ('𑅇', '𑅇'),
    ('𑅐', '𑅲'),
    ('𑅶', '𑅶'),
    ('𑆃', '𑆲'),
    ('𑇁', '𑇄'),
    ('𑇚', '𑇚'),
    ('𑇜', '𑇜'),
    ('𑈀', '𑈑'),
    ('𑈓', '𑈫'),
    ('𑈿', '𑉀'),
    ('𑊀', '𑊆'),
    ('𑊈', '𑊈'),
    ('𑊊', '𑊍'),
    ('𑊏', '𑊝'),
    ('𑊟', '𑊨'),
    ('𑊰', '𑋞'),
    ('𑌅', '𑌌'),
    ('𑌏', '𑌐'),
    ('𑌓', '𑌨'),
    ('𑌪', '𑌰'),
    ('𑌲', '𑌳'),
    ('𑌵', '𑌹'),
    ('𑌽', '𑌽'),
    ('𑍐', '𑍐'),
    ('𑍝', '𑍡'),
    ('𑎀', '𑎉'),
    ('𑎋', '𑎋'),
    ('𑎎', '𑎎'),
    ('𑎐', '𑎵'),
    ('𑎷', '𑎷'),
    ('𑏑', '𑏑'),
    ('𑏓', '𑏓'),
    ('𑐀', '𑐴'),
    ('𑑇', '𑑊'),
    ('𑑟', '𑑡'),
    ('𑒀', '𑒯'),
    ('𑓄', '𑓅'),
    ('𑓇', '𑓇'),
    ('𑖀', '𑖮'),
    ('𑗘', '𑗛'),
    ('𑘀', '𑘯'),
    ('𑙄', '𑙄'),
    ('𑚀', '𑚪'),
    ('𑚸', '𑚸'),
    ('𑜀', '𑜚'),
    ('𑝀', '𑝆'),
    ('𑠀', '𑠫'),
    ('𑢠', '𑣟'),
    ('𑣿', '𑤆'),
    ('𑤉', '𑤉'),
    ('𑤌', '𑤓'),
    ('𑤕', '𑤖'),
    ('𑤘', '𑤯'),
    ('𑤿', '𑤿'),
    ('𑥁', '𑥁'),
    ('𑦠', '𑦧'),
    ('𑦪', '𑧐'),
    ('𑧡', '𑧡'),
    ('𑧣', '𑧣'),
    ('𑨀', '𑨀'),
    ('𑨋', '𑨲'),
    ('𑨺', '𑨺'),
    ('𑩐', '𑩐'),
    ('𑩜', '𑪉'),
    ('𑪝', '𑪝'),
    ('𑪰', '𑫸'),
    ('𑯀', '𑯠'),
    ('𑰀', '𑰈'),
    ('𑰊', '𑰮'),
    ('𑱀', '𑱀'),
    ('𑱲', '𑲏'),
    ('𑴀', '𑴆'),
    ('𑴈', '𑴉'),
    ('𑴋', '𑴰'),
    ('𑵆', '𑵆'),
    ('𑵠', '𑵥'),
    ('𑵧', '𑵨'),
    ('𑵪', '𑶉'),
    ('𑶘', '𑶘'),
    ('𑻠', '𑻲'),
    ('𑼂', '𑼂'),
    ('𑼄', '𑼐'),
    ('𑼒', '𑼳'),
    ('𑾰', '𑾰'),
    ('𒀀', '𒎙'),
    ('𒐀', '𒑮'),
    ('𒒀', '𒕃'),
    ('𒾐', '𒿰'),
    ('𓀀', '𓐯'),
    ('𓑁', '𓑆'),
    ('𓑠', '𔏺'),
    ('𔐀', '𔙆'),
    ('𖄀', '𖄝'),
    ('𖠀', '𖨸'),
    ('𖩀', '𖩞'),
    ('𖩰', '𖪾'),
    ('𖫐', '𖫭'),
    ('𖬀', '𖬯'),
    ('𖭀', '𖭃'),
    ('𖭣', '𖭷'),
    ('𖭽', '𖮏'),
    ('𖵀', '𖵬'),
    ('𖹀', '𖹿'),
    ('𖼀', '𖽊'),
    ('𖽐', '𖽐'),
    ('𖾓', '𖾟'),
    ('𖿠', '𖿡'),
    ('𖿣', '𖿣'),
    ('𗀀', '𘟷'),
    ('𘠀', '𘳕'),
    ('𘳿', '𘴈'),
    ('𚿰', '𚿳'),
    ('𚿵', '𚿻'),
    ('𚿽', '𚿾'),
    ('𛀀', '𛄢'),
    ('𛄲', '𛄲'),
    ('𛅐', '𛅒'),
    ('𛅕', '𛅕'),
    ('𛅤', '𛅧'),
    ('𛅰', '𛋻'),
    ('𛰀', '𛱪'),
    ('𛱰', '𛱼'),
    ('𛲀', '𛲈'),
    ('𛲐', '𛲙'),
    ('𝐀', '𝑔'),
    ('𝑖', '𝒜'),
    ('𝒞', '𝒟'),
    ('𝒢', '𝒢'),
    ('𝒥', '𝒦'),
    ('𝒩', '𝒬'),
    ('𝒮', '𝒹'),
    ('𝒻', '𝒻'),
    ('𝒽', '𝓃'),
    ('𝓅', '𝔅'),
    ('𝔇', '𝔊'),
    ('𝔍', '𝔔'),
    ('𝔖', '𝔜'),
    ('𝔞', '𝔹'),
    ('𝔻', '𝔾'),
    ('𝕀', '𝕄'),
    ('𝕆', '𝕆'),
    ('𝕊', '𝕐'),
    ('𝕒', '𝚥'),
    ('𝚨', '𝛀'),
    ('𝛂', '𝛚'),
    ('𝛜', '𝛺'),
    ('𝛼', '𝜔'),
    ('𝜖', '𝜴'),
    ('𝜶', '𝝎'),
    ('𝝐', '𝝮'),
    ('𝝰', '𝞈'),
    ('𝞊', '𝞨'),
    ('𝞪', '𝟂'),
    ('𝟄', '𝟋'),
    ('𝼀', '𝼞'),
    ('𝼥', '𝼪'),
    ('𞀰', '𞁭'),
    ('𞄀', '𞄬'),
    ('𞄷', '𞄽'),
    ('𞅎', '𞅎'),
    ('𞊐', '𞊭'),
    ('𞋀', '𞋫'),
    ('𞓐', '𞓫'),
    ('𞗐', '𞗭'),
    ('𞗰', '𞗰'),
    ('𞟠', '𞟦'),
    ('𞟨', '𞟫'),
    ('𞟭', '𞟮'),
    ('𞟰', '𞟾'),
    ('𞠀', '𞣄'),
    ('𞤀', '𞥃'),
    ('𞥋', '𞥋'),
    ('𞸀', '𞸃'),
    ('𞸅', '𞸟'),
    ('𞸡', '𞸢'),
    ('𞸤', '𞸤'),
    ('𞸧', '𞸧'),
    ('𞸩', '𞸲'),
    ('𞸴', '𞸷'),
    ('𞸹', '𞸹'),
    ('𞸻', '𞸻'),
    ('𞹂', '𞹂'),
    ('𞹇', '𞹇'),
    ('𞹉', '𞹉'),
    ('𞹋', '𞹋'),
    ('𞹍', '𞹏'),
    ('𞹑', '𞹒'),
    ('𞹔', '𞹔'),
    ('𞹗', '𞹗'),
    ('𞹙', '𞹙'),
    ('𞹛', '𞹛'),
    ('𞹝', '𞹝'),
    ('𞹟', '𞹟'),
    ('𞹡', '𞹢'),
    ('𞹤', '𞹤'),
    ('𞹧', '𞹪'),
    ('𞹬', '𞹲'),
    ('𞹴', '𞹷'),
    ('𞹹', '𞹼'),
    ('𞹾', '𞹾'),
    ('𞺀', '𞺉'),
    ('𞺋', '𞺛'),
    ('𞺡', '𞺣'),
    ('𞺥', '𞺩'),
    ('𞺫', '𞺻'),
    ('𠀀', '𪛟'),
    ('𪜀', '𫜹'),
    ('𫝀', '𫠝'),
    ('𫠠', '𬺡'),
    ('𬺰', '𮯠'),
    ('𮯰', '𮹝'),
    ('丽', '𪘀'),
    ('𰀀', '𱍊'),
    ('𱍐', '𲎯'),
];

const UNICODE_XID_CONTINUE_RANGES: &[(char, char)] = &[
    ('0', '9'),
    ('A', 'Z'),
    ('_', '_'),
    ('a', 'z'),
    ('ª', 'ª'),
    ('µ', 'µ'),
    ('·', '·'),
    ('º', 'º'),
    ('À', 'Ö'),
    ('Ø', 'ö'),
    ('ø', 'ˁ'),
    ('ˆ', 'ˑ'),
    ('ˠ', 'ˤ'),
    ('ˬ', 'ˬ'),
    ('ˮ', 'ˮ'),
    ('\u{300}', 'ʹ'),
    ('Ͷ', 'ͷ'),
    ('ͻ', 'ͽ'),
    ('Ϳ', 'Ϳ'),
    ('Ά', 'Ί'),
    ('Ό', 'Ό'),
    ('Ύ', 'Ρ'),
    ('Σ', 'ϵ'),
    ('Ϸ', 'ҁ'),
    ('\u{483}', '\u{487}'),
    ('Ҋ', 'ԯ'),
    ('Ա', 'Ֆ'),
    ('ՙ', 'ՙ'),
    ('ՠ', 'ֈ'),
    ('\u{591}', '\u{5bd}'),
    ('\u{5bf}', '\u{5bf}'),
    ('\u{5c1}', '\u{5c2}'),
    ('\u{5c4}', '\u{5c5}'),
    ('\u{5c7}', '\u{5c7}'),
    ('א', 'ת'),
    ('ׯ', 'ײ'),
    ('\u{610}', '\u{61a}'),
    ('ؠ', '٩'),
    ('ٮ', 'ۓ'),
    ('ە', '\u{6dc}'),
    ('\u{6df}', '\u{6e8}'),
    ('\u{6ea}', 'ۼ'),
    ('ۿ', 'ۿ'),
    ('ܐ', '\u{74a}'),
    ('ݍ', 'ޱ'),
    ('߀', 'ߵ'),
    ('ߺ', 'ߺ'),
    ('\u{7fd}', '\u{7fd}'),
    ('ࠀ', '\u{82d}'),
    ('ࡀ', '\u{85b}'),
    ('ࡠ', 'ࡪ'),
    ('ࡰ', 'ࢇ'),
    ('ࢉ', 'ࢎ'),
    ('\u{897}', '\u{8e1}'),
    ('\u{8e3}', '\u{963}'),
    ('०', '९'),
    ('ॱ', 'ঃ'),
    ('অ', 'ঌ'),
    ('এ', 'ঐ'),
    ('ও', 'ন'),
    ('প', 'র'),
    ('ল', 'ল'),
    ('শ', 'হ'),
    ('\u{9bc}', '\u{9c4}'),
    ('ে', 'ৈ'),
    ('ো', 'ৎ'),
    ('\u{9d7}', '\u{9d7}'),
    ('ড়', 'ঢ়'),
    ('য়', '\u{9e3}'),
    ('০', 'ৱ'),
    ('ৼ', 'ৼ'),
    ('\u{9fe}', '\u{9fe}'),
    ('\u{a01}', 'ਃ'),
    ('ਅ', 'ਊ'),
    ('ਏ', 'ਐ'),
    ('ਓ', 'ਨ'),
    ('ਪ', 'ਰ'),
    ('ਲ', 'ਲ਼'),
    ('ਵ', 'ਸ਼'),
    ('ਸ', 'ਹ'),
    ('\u{a3c}', '\u{a3c}'),
    ('ਾ', '\u{a42}'),
    ('\u{a47}', '\u{a48}'),
    ('\u{a4b}', '\u{a4d}'),
    ('\u{a51}', '\u{a51}'),
    ('ਖ਼', 'ੜ'),
    ('ਫ਼', 'ਫ਼'),
    ('੦', '\u{a75}'),
    ('\u{a81}', 'ઃ'),
    ('અ', 'ઍ'),
    ('એ', 'ઑ'),
    ('ઓ', 'ન'),
    ('પ', 'ર'),
    ('લ', 'ળ'),
    ('વ', 'હ'),
    ('\u{abc}', '\u{ac5}'),
    ('\u{ac7}', 'ૉ'),
    ('ો', '\u{acd}'),
    ('ૐ', 'ૐ'),
    ('ૠ', '\u{ae3}'),
    ('૦', '૯'),
    ('ૹ', '\u{aff}'),
    ('\u{b01}', 'ଃ'),
    ('ଅ', 'ଌ'),
    ('ଏ', 'ଐ'),
    ('ଓ', 'ନ'),
    ('ପ', 'ର'),
    ('ଲ', 'ଳ'),
    ('ଵ', 'ହ'),
    ('\u{b3c}', '\u{b44}'),
    ('େ', 'ୈ'),
    ('ୋ', '\u{b4d}'),
    ('\u{b55}', '\u{b57}'),
    ('ଡ଼', 'ଢ଼'),
    ('ୟ', '\u{b63}'),
    ('୦', '୯'),
    ('ୱ', 'ୱ'),
    ('\u{b82}', 'ஃ'),
    ('அ', 'ஊ'),
    ('எ', 'ஐ'),
    ('ஒ', 'க'),
    ('ங', 'ச'),
    ('ஜ', 'ஜ'),
    ('ஞ', 'ட'),
    ('ண', 'த'),
    ('ந', 'ப'),
    ('ம', 'ஹ'),
    ('\u{bbe}', 'ூ'),
    ('ெ', 'ை'),
    ('ொ', '\u{bcd}'),
    ('ௐ', 'ௐ'),
    ('\u{bd7}', '\u{bd7}'),
    ('௦', '௯'),
    ('\u{c00}', 'ఌ'),
    ('ఎ', 'ఐ'),
    ('ఒ', 'న'),
    ('ప', 'హ'),
    ('\u{c3c}', 'ౄ'),
    ('\u{c46}', '\u{c48}'),
    ('\u{c4a}', '\u{c4d}'),
    ('\u{c55}', '\u{c56}'),
    ('ౘ', 'ౚ'),
    ('ౝ', 'ౝ'),
    ('ౠ', '\u{c63}'),
    ('౦', '౯'),
    ('ಀ', 'ಃ'),
    ('ಅ', 'ಌ'),
    ('ಎ', 'ಐ'),
    ('ಒ', 'ನ'),
    ('ಪ', 'ಳ'),
    ('ವ', 'ಹ'),
    ('\u{cbc}', 'ೄ'),
    ('\u{cc6}', '\u{cc8}'),
    ('\u{cca}', '\u{ccd}'),
    ('\u{cd5}', '\u{cd6}'),
    ('ೝ', 'ೞ'),
    ('ೠ', '\u{ce3}'),
    ('೦', '೯'),
    ('ೱ', 'ೳ'),
    ('\u{d00}', 'ഌ'),
    ('എ', 'ഐ'),
    ('ഒ', '\u{d44}'),
    ('െ', 'ൈ'),
    ('ൊ', 'ൎ'),
    ('ൔ', '\u{d57}'),
    ('ൟ', '\u{d63}'),
    ('൦', '൯'),
    ('ൺ', 'ൿ'),
    ('\u{d81}', 'ඃ'),
    ('අ', 'ඖ'),
    ('ක', 'න'),
    ('ඳ', 'ර'),
    ('ල', 'ල'),
    ('ව', 'ෆ'),
    ('\u{dca}', '\u{dca}'),
    ('\u{dcf}', '\u{dd4}'),
    ('\u{dd6}', '\u{dd6}'),
    ('ෘ', '\u{ddf}'),
    ('෦', '෯'),
    ('ෲ', 'ෳ'),
    ('ก', '\u{e3a}'),
    ('เ', '\u{e4e}'),
    ('๐', '๙'),
    ('ກ', 'ຂ'),
    ('ຄ', 'ຄ'),
    ('ຆ', 'ຊ'),
    ('ຌ', 'ຣ'),
    ('ລ', 'ລ'),
    ('ວ', 'ຽ'),
    ('ເ', 'ໄ'),
    ('ໆ', 'ໆ'),
    ('\u{ec8}', '\u{ece}'),
    ('໐', '໙'),
    ('ໜ', 'ໟ'),
    ('ༀ', 'ༀ'),
    ('\u{f18}', '\u{f19}'),
    ('༠', '༩'),
    ('\u{f35}', '\u{f35}'),
    ('\u{f37}', '\u{f37}'),
    ('\u{f39}', '\u{f39}'),
    ('༾', 'ཇ'),
    ('ཉ', 'ཬ'),
    ('\u{f71}', '\u{f84}'),
    ('\u{f86}', '\u{f97}'),
    ('\u{f99}', '\u{fbc}'),
    ('\u{fc6}', '\u{fc6}'),
    ('က', '၉'),
    ('ၐ', '\u{109d}'),
    ('Ⴀ', 'Ⴥ'),
    ('Ⴧ', 'Ⴧ'),
    ('Ⴭ', 'Ⴭ'),
    ('ა', 'ჺ'),
    ('ჼ', 'ቈ'),
    ('ቊ', 'ቍ'),
    ('ቐ', 'ቖ'),
    ('ቘ', 'ቘ'),
    ('ቚ', 'ቝ'),
    ('በ', 'ኈ'),
    ('ኊ', 'ኍ'),
    ('ነ', 'ኰ'),
    ('ኲ', 'ኵ'),
    ('ኸ', 'ኾ'),
    ('ዀ', 'ዀ'),
    ('ዂ', 'ዅ'),
    ('ወ', 'ዖ'),
    ('ዘ', 'ጐ'),
    ('ጒ', 'ጕ'),
    ('ጘ', 'ፚ'),
    ('\u{135d}', '\u{135f}'),
    ('፩', '፱'),
    ('ᎀ', 'ᎏ'),
    ('Ꭰ', 'Ᏽ'),
    ('ᏸ', 'ᏽ'),
    ('ᐁ', 'ᙬ'),
    ('ᙯ', 'ᙿ'),
    ('ᚁ', 'ᚚ'),
    ('ᚠ', 'ᛪ'),
    ('ᛮ', 'ᛸ'),
    ('ᜀ', '\u{1715}'),
    ('ᜟ', '\u{1734}'),
    ('ᝀ', '\u{1753}'),
    ('ᝠ', 'ᝬ'),
    ('ᝮ', 'ᝰ'),
    ('\u{1772}', '\u{1773}'),
    ('ក', '\u{17d3}'),
    ('ៗ', 'ៗ'),
    ('ៜ', '\u{17dd}'),
    ('០', '៩'),
    ('\u{180b}', '\u{180d}'),
    ('\u{180f}', '᠙'),
    ('ᠠ', 'ᡸ'),
    ('ᢀ', 'ᢪ'),
    ('ᢰ', 'ᣵ'),
    ('ᤀ', 'ᤞ'),
    ('\u{1920}', 'ᤫ'),
    ('ᤰ', '\u{193b}'),
    ('᥆', 'ᥭ'),
    ('ᥰ', 'ᥴ'),
    ('ᦀ', 'ᦫ'),
    ('ᦰ', 'ᧉ'),
    ('᧐', '᧚'),
    ('ᨀ', '\u{1a1b}'),
    ('ᨠ', '\u{1a5e}'),
    ('\u{1a60}', '\u{1a7c}'),
    ('\u{1a7f}', '᪉'),
    ('᪐', '᪙'),
    ('ᪧ', 'ᪧ'),
    ('\u{1ab0}', '\u{1abd}'),
    ('\u{1abf}', '\u{1ace}'),
    ('\u{1b00}', 'ᭌ'),
    ('᭐', '᭙'),
    ('\u{1b6b}', '\u{1b73}'),
    ('\u{1b80}', '\u{1bf3}'),
    ('ᰀ', '\u{1c37}'),
    ('᱀', '᱉'),
    ('ᱍ', 'ᱽ'),
    ('ᲀ', 'ᲊ'),
    ('Ა', 'Ჺ'),
    ('Ჽ', 'Ჿ'),
    ('\u{1cd0}', '\u{1cd2}'),
    ('\u{1cd4}', 'ᳺ'),
    ('ᴀ', 'ἕ'),
    ('Ἐ', 'Ἕ'),
    ('ἠ', 'ὅ'),
    ('Ὀ', 'Ὅ'),
    ('ὐ', 'ὗ'),
    ('Ὑ', 'Ὑ'),
    ('Ὓ', 'Ὓ'),
    ('Ὕ', 'Ὕ'),
    ('Ὗ', 'ώ'),
    ('ᾀ', 'ᾴ'),
    ('ᾶ', 'ᾼ'),
    ('ι', 'ι'),
    ('ῂ', 'ῄ'),
    ('ῆ', 'ῌ'),
    ('ῐ', 'ΐ'),
    ('ῖ', 'Ί'),
    ('ῠ', 'Ῥ'),
    ('ῲ', 'ῴ'),
    ('ῶ', 'ῼ'),
    ('\u{200c}', '\u{200d}'),
    ('‿', '⁀'),
    ('⁔', '⁔'),
    ('ⁱ', 'ⁱ'),
    ('ⁿ', 'ⁿ'),
    ('ₐ', 'ₜ'),
    ('\u{20d0}', '\u{20dc}'),
    ('\u{20e1}', '\u{20e1}'),
    ('\u{20e5}', '\u{20f0}'),
    ('ℂ', 'ℂ'),
    ('ℇ', 'ℇ'),
    ('ℊ', 'ℓ'),
    ('ℕ', 'ℕ'),
    ('℘', 'ℝ'),
    ('ℤ', 'ℤ'),
    ('Ω', 'Ω'),
    ('ℨ', 'ℨ'),
    ('K', 'ℹ'),
    ('ℼ', 'ℿ'),
    ('ⅅ', 'ⅉ'),
    ('ⅎ', 'ⅎ'),
    ('Ⅰ', 'ↈ'),
    ('Ⰰ', 'ⳤ'),
    ('Ⳬ', 'ⳳ'),
    ('ⴀ', 'ⴥ'),
    ('ⴧ', 'ⴧ'),
    ('ⴭ', 'ⴭ'),
    ('ⴰ', 'ⵧ'),
    ('ⵯ', 'ⵯ'),
    ('\u{2d7f}', 'ⶖ'),
    ('ⶠ', 'ⶦ'),
    ('ⶨ', 'ⶮ'),
    ('ⶰ', 'ⶶ'),
    ('ⶸ', 'ⶾ'),
    ('ⷀ', 'ⷆ'),
    ('ⷈ', 'ⷎ'),
    ('ⷐ', 'ⷖ'),
    ('ⷘ', 'ⷞ'),
    ('\u{2de0}', '\u{2dff}'),
    ('々', '〇'),
    ('〡', '\u{302f}'),
    ('〱', '〵'),
    ('〸', '〼'),
    ('ぁ', 'ゖ'),
    ('\u{3099}', '\u{309a}'),
    ('ゝ', 'ゟ'),
    ('ァ', 'ヿ'),
    ('ㄅ', 'ㄯ'),
    ('ㄱ', 'ㆎ'),
    ('ㆠ', 'ㆿ'),
    ('ㇰ', 'ㇿ'),
    ('㐀', '䶿'),
    ('一', 'ꒌ'),
    ('ꓐ', 'ꓽ'),
    ('ꔀ', 'ꘌ'),
    ('ꘐ', 'ꘫ'),
    ('Ꙁ', '\u{a66f}'),
    ('\u{a674}', '\u{a67d}'),
    ('ꙿ', '\u{a6f1}'),
    ('ꜗ', 'ꜟ'),
    ('Ꜣ', 'ꞈ'),
    ('Ꞌ', 'ꟍ'),
    ('Ꟑ', 'ꟑ'),
    ('ꟓ', 'ꟓ'),
    ('ꟕ', 'Ƛ'),
    ('ꟲ', 'ꠧ'),
    ('\u{a82c}', '\u{a82c}'),
    ('ꡀ', 'ꡳ'),
    ('ꢀ', '\u{a8c5}'),
    ('꣐', '꣙'),
    ('\u{a8e0}', 'ꣷ'),
    ('ꣻ', 'ꣻ'),
    ('ꣽ', '\u{a92d}'),
    ('ꤰ', '\u{a953}'),
    ('ꥠ', 'ꥼ'),
    ('\u{a980}', '\u{a9c0}'),
    ('ꧏ', '꧙'),
    ('ꧠ', 'ꧾ'),
    ('ꨀ', '\u{aa36}'),
    ('ꩀ', 'ꩍ'),
    ('꩐', '꩙'),
    ('ꩠ', 'ꩶ'),
    ('ꩺ', 'ꫂ'),
    ('ꫛ', 'ꫝ'),
    ('ꫠ', 'ꫯ'),
    ('ꫲ', '\u{aaf6}'),
    ('ꬁ', 'ꬆ'),
    ('ꬉ', 'ꬎ'),
    ('ꬑ', 'ꬖ'),
    ('ꬠ', 'ꬦ'),
    ('ꬨ', 'ꬮ'),
    ('ꬰ', 'ꭚ'),
    ('ꭜ', 'ꭩ'),
    ('ꭰ', 'ꯪ'),
    ('꯬', '\u{abed}'),
    ('꯰', '꯹'),
    ('가', '힣'),
    ('ힰ', 'ퟆ'),
    ('ퟋ', 'ퟻ'),
    ('豈', '舘'),
    ('並', '龎'),
    ('ﬀ', 'ﬆ'),
    ('ﬓ', 'ﬗ'),
    ('יִ', 'ﬨ'),
    ('שׁ', 'זּ'),
    ('טּ', 'לּ'),
    ('מּ', 'מּ'),
    ('נּ', 'סּ'),
    ('ףּ', 'פּ'),
    ('צּ', 'ﮱ'),
    ('ﯓ', 'ﱝ'),
    ('ﱤ', 'ﴽ'),
    ('ﵐ', 'ﶏ'),
    ('ﶒ', 'ﷇ'),
    ('ﷰ', 'ﷹ'),
    ('\u{fe00}', '\u{fe0f}'),
    ('\u{fe20}', '\u{fe2f}'),
    ('︳', '︴'),
    ('﹍', '﹏'),
    ('ﹱ', 'ﹱ'),
    ('ﹳ', 'ﹳ'),
    ('ﹷ', 'ﹷ'),
    ('ﹹ', 'ﹹ'),
    ('ﹻ', 'ﹻ'),
    ('ﹽ', 'ﹽ'),
    ('ﹿ', 'ﻼ'),
    ('０', '９'),
    ('Ａ', 'Ｚ'),
    ('＿', '＿'),
    ('ａ', 'ｚ'),
    ('･', 'ﾾ'),
    ('ￂ', 'ￇ'),
    ('ￊ', 'ￏ'),
    ('ￒ', 'ￗ'),
    ('ￚ', 'ￜ'),
    ('𐀀', '𐀋'),
    ('𐀍', '𐀦'),
    ('𐀨', '𐀺'),
    ('𐀼', '𐀽'),
    ('𐀿', '𐁍'),
    ('𐁐', '𐁝'),
    ('𐂀', '𐃺'),
    ('𐅀', '𐅴'),
    ('\u{101fd}', '\u{101fd}'),
    ('𐊀', '𐊜'),
    ('𐊠', '𐋐'),
    ('\u{102e0}', '\u{102e0}'),
    ('𐌀', '𐌟'),
    ('𐌭', '𐍊'),
    ('𐍐', '\u{1037a}'),
    ('𐎀', '𐎝'),
    ('𐎠', '𐏃'),
    ('𐏈', '𐏏'),
    ('𐏑', '𐏕'),
    ('𐐀', '𐒝'),
    ('𐒠', '𐒩'),
    ('𐒰', '𐓓'),
    ('𐓘', '𐓻'),
    ('𐔀', '𐔧'),
    ('𐔰', '𐕣'),
    ('𐕰', '𐕺'),
    ('𐕼', '𐖊'),
    ('𐖌', '𐖒'),
    ('𐖔', '𐖕'),
    ('𐖗', '𐖡'),
    ('𐖣', '𐖱'),
    ('𐖳', '𐖹'),
    ('𐖻', '𐖼'),
    ('𐗀', '𐗳'),
    ('𐘀', '𐜶'),
    ('𐝀', '𐝕'),
    ('𐝠', '𐝧'),
    ('𐞀', '𐞅'),
    ('𐞇', '𐞰'),
    ('𐞲', '𐞺'),
    ('𐠀', '𐠅'),
    ('𐠈', '𐠈'),
    ('𐠊', '𐠵'),
    ('𐠷', '𐠸'),
    ('𐠼', '𐠼'),
    ('𐠿', '𐡕'),
    ('𐡠', '𐡶'),
    ('𐢀', '𐢞'),
    ('𐣠', '𐣲'),
    ('𐣴', '𐣵'),
    ('𐤀', '𐤕'),
    ('𐤠', '𐤹'),
    ('𐦀', '𐦷'),
    ('𐦾', '𐦿'),
    ('𐨀', '\u{10a03}'),
    ('\u{10a05}', '\u{10a06}'),
    ('\u{10a0c}', '𐨓'),
    ('𐨕', '𐨗'),
    ('𐨙', '𐨵'),
    ('\u{10a38}', '\u{10a3a}'),
    ('\u{10a3f}', '\u{10a3f}'),
    ('𐩠', '𐩼'),
    ('𐪀', '𐪜'),
    ('𐫀', '𐫇'),
    ('𐫉', '\u{10ae6}'),
    ('𐬀', '𐬵'),
    ('𐭀', '𐭕'),
    ('𐭠', '𐭲'),
    ('𐮀', '𐮑'),
    ('𐰀', '𐱈'),
    ('𐲀', '𐲲'),
    ('𐳀', '𐳲'),
    ('𐴀', '\u{10d27}'),
    ('𐴰', '𐴹'),
    ('𐵀', '𐵥'),
    ('\u{10d69}', '\u{10d6d}'),
    ('𐵯', '𐶅'),
    ('𐺀', '𐺩'),
    ('\u{10eab}', '\u{10eac}'),
    ('𐺰', '𐺱'),
    ('𐻂', '𐻄'),
    ('\u{10efc}', '𐼜'),
    ('𐼧', '𐼧'),
    ('𐼰', '\u{10f50}'),
    ('𐽰', '\u{10f85}'),
    ('𐾰', '𐿄'),
    ('𐿠', '𐿶'),
    ('𑀀', '\u{11046}'),
    ('𑁦', '𑁵'),
    ('\u{1107f}', '\u{110ba}'),
    ('\u{110c2}', '\u{110c2}'),
    ('𑃐', '𑃨'),
    ('𑃰', '𑃹'),
    ('\u{11100}', '\u{11134}'),
    ('𑄶', '𑄿'),
    ('𑅄', '𑅇'),
    ('𑅐', '\u{11173}'),
    ('𑅶', '𑅶'),
    ('\u{11180}', '𑇄'),
    ('\u{111c9}', '\u{111cc}'),
    ('𑇎', '𑇚'),
    ('𑇜', '𑇜'),
    ('𑈀', '𑈑'),
    ('𑈓', '\u{11237}'),
    ('\u{1123e}', '\u{11241}'),
    ('𑊀', '𑊆'),
    ('𑊈', '𑊈'),
    ('𑊊', '𑊍'),
    ('𑊏', '𑊝'),
    ('𑊟', '𑊨'),
    ('𑊰', '\u{112ea}'),
    ('𑋰', '𑋹'),
    ('\u{11300}', '𑌃'),
    ('𑌅', '𑌌'),
    ('𑌏', '𑌐'),
    ('𑌓', '𑌨'),
    ('𑌪', '𑌰'),
    ('𑌲', '𑌳'),
    ('𑌵', '𑌹'),
    ('\u{1133b}', '𑍄'),
    ('𑍇', '𑍈'),
    ('𑍋', '\u{1134d}'),
    ('𑍐', '𑍐'),
    ('\u{11357}', '\u{11357}'),
    ('𑍝', '𑍣'),
    ('\u{11366}', '\u{1136c}'),
    ('\u{11370}', '\u{11374}'),
    ('𑎀', '𑎉'),
    ('𑎋', '𑎋'),
    ('𑎎', '𑎎'),
    ('𑎐', '𑎵'),
    ('𑎷', '\u{113c0}'),
    ('\u{113c2}', '\u{113c2}'),
    ('\u{113c5}', '\u{113c5}'),
    ('\u{113c7}', '𑏊'),
    ('𑏌', '𑏓'),
    ('\u{113e1}', '\u{113e2}'),
    ('𑐀', '𑑊'),
    ('𑑐', '𑑙'),
    ('\u{1145e}', '𑑡'),
    ('𑒀', '𑓅'),
    ('𑓇', '𑓇'),
    ('𑓐', '𑓙'),
    ('𑖀', '\u{115b5}'),
    ('𑖸', '\u{115c0}'),
    ('𑗘', '\u{115dd}'),
    ('𑘀', '\u{11640}'),
    ('𑙄', '𑙄'),
    ('𑙐', '𑙙'),
    ('𑚀', '𑚸'),
    ('𑛀', '𑛉'),
    ('𑛐', '𑛣'),
    ('𑜀', '𑜚'),
    ('\u{1171d}', '\u{1172b}'),
    ('𑜰', '𑜹'),
    ('𑝀', '𑝆'),
    ('𑠀', '\u{1183a}'),
    ('𑢠', '𑣩'),
    ('𑣿', '𑤆'),
    ('𑤉', '𑤉'),
    ('𑤌', '𑤓'),
    ('𑤕', '𑤖'),
    ('𑤘', '𑤵'),
    ('𑤷', '𑤸'),
    ('\u{1193b}', '\u{11943}'),
    ('𑥐', '𑥙'),
    ('𑦠', '𑦧'),
    ('𑦪', '\u{119d7}'),
    ('\u{119da}', '𑧡'),
    ('𑧣', '𑧤'),
    ('𑨀', '\u{11a3e}'),
    ('\u{11a47}', '\u{11a47}'),
    ('𑩐', '\u{11a99}'),
    ('𑪝', '𑪝'),
    ('𑪰', '𑫸'),
    ('𑯀', '𑯠'),
    ('𑯰', '𑯹'),
    ('𑰀', '𑰈'),
    ('𑰊', '\u{11c36}'),
    ('\u{11c38}', '𑱀'),
    ('𑱐', '𑱙'),
    ('𑱲', '𑲏'),
    ('\u{11c92}', '\u{11ca7}'),
    ('𑲩', '\u{11cb6}'),
    ('𑴀', '𑴆'),
    ('𑴈', '𑴉'),
    ('𑴋', '\u{11d36}'),
    ('\u{11d3a}', '\u{11d3a}'),
    ('\u{11d3c}', '\u{11d3d}'),
    ('\u{11d3f}', '\u{11d47}'),
    ('𑵐', '𑵙'),
    ('𑵠', '𑵥'),
    ('𑵧', '𑵨'),
    ('𑵪', '𑶎'),
    ('\u{11d90}', '\u{11d91}'),
    ('𑶓', '𑶘'),
    ('𑶠', '𑶩'),
    ('𑻠', '𑻶'),
    ('\u{11f00}', '𑼐'),
    ('𑼒', '\u{11f3a}'),
    ('𑼾', '\u{11f42}'),
    ('𑽐', '\u{11f5a}'),
    ('𑾰', '𑾰'),
    ('𒀀', '𒎙'),
    ('𒐀', '𒑮'),
    ('𒒀', '𒕃'),
    ('𒾐', '𒿰'),
    ('𓀀', '𓐯'),
    ('\u{13440}', '\u{13455}'),
    ('𓑠', '𔏺'),
    ('𔐀', '𔙆'),
    ('𖄀', '𖄹'),
    ('𖠀', '𖨸'),
    ('𖩀', '𖩞'),
    ('𖩠', '𖩩'),
    ('𖩰', '𖪾'),
    ('𖫀', '𖫉'),
    ('𖫐', '𖫭'),
    ('\u{16af0}', '\u{16af4}'),
    ('𖬀', '\u{16b36}'),
    ('𖭀', '𖭃'),
    ('𖭐', '𖭙'),
    ('𖭣', '𖭷'),
    ('𖭽', '𖮏'),
    ('𖵀', '𖵬'),
    ('𖵰', '𖵹'),
    ('𖹀', '𖹿'),
    ('𖼀', '𖽊'),
    ('\u{16f4f}', '𖾇'),
    ('\u{16f8f}', '𖾟'),
    ('𖿠', '𖿡'),
    ('𖿣', '\u{16fe4}'),
    ('\u{16ff0}', '\u{16ff1}'),
    ('𗀀', '𘟷'),
    ('𘠀', '𘳕'),
    ('𘳿', '𘴈'),
    ('𚿰', '𚿳'),
    ('𚿵', '𚿻'),
    ('𚿽', '𚿾'),
    ('𛀀', '𛄢'),
    ('𛄲', '𛄲'),
    ('𛅐', '𛅒'),
    ('𛅕', '𛅕'),
    ('𛅤', '𛅧'),
    ('𛅰', '𛋻'),
    ('𛰀', '𛱪'),
    ('𛱰', '𛱼'),
    ('𛲀', '𛲈'),
    ('𛲐', '𛲙'),
    ('\u{1bc9d}', '\u{1bc9e}'),
    ('𜳰', '𜳹'),
    ('\u{1cf00}', '\u{1cf2d}'),
    ('\u{1cf30}', '\u{1cf46}'),
    ('\u{1d165}', '\u{1d169}'),
    ('\u{1d16d}', '\u{1d172}'),
    ('\u{1d17b}', '\u{1d182}'),
    ('\u{1d185}', '\u{1d18b}'),
    ('\u{1d1aa}', '\u{1d1ad}'),
    ('\u{1d242}', '\u{1d244}'),
    ('𝐀', '𝑔'),
    ('𝑖', '𝒜'),
    ('𝒞', '𝒟'),
    ('𝒢', '𝒢'),
    ('𝒥', '𝒦'),
    ('𝒩', '𝒬'),
    ('𝒮', '𝒹'),
    ('𝒻', '𝒻'),
    ('𝒽', '𝓃'),
    ('𝓅', '𝔅'),
    ('𝔇', '𝔊'),
    ('𝔍', '𝔔'),
    ('𝔖', '𝔜'),
    ('𝔞', '𝔹'),
    ('𝔻', '𝔾'),
    ('𝕀', '𝕄'),
    ('𝕆', '𝕆'),
    ('𝕊', '𝕐'),
    ('𝕒', '𝚥'),
    ('𝚨', '𝛀'),
    ('𝛂', '𝛚'),
    ('𝛜', '𝛺'),
    ('𝛼', '𝜔'),
    ('𝜖', '𝜴'),
    ('𝜶', '𝝎'),
    ('𝝐', '𝝮'),
    ('𝝰', '𝞈'),
    ('𝞊', '𝞨'),
    ('𝞪', '𝟂'),
    ('𝟄', '𝟋'),
    ('𝟎', '𝟿'),
    ('\u{1da00}', '\u{1da36}'),
    ('\u{1da3b}', '\u{1da6c}'),
    ('\u{1da75}', '\u{1da75}'),
    ('\u{1da84}', '\u{1da84}'),
    ('\u{1da9b}', '\u{1da9f}'),
    ('\u{1daa1}', '\u{1daaf}'),
    ('𝼀', '𝼞'),
    ('𝼥', '𝼪'),
    ('\u{1e000}', '\u{1e006}'),
    ('\u{1e008}', '\u{1e018}'),
    ('\u{1e01b}', '\u{1e021}'),
    ('\u{1e023}', '\u{1e024}'),
    ('\u{1e026}', '\u{1e02a}'),
    ('𞀰', '𞁭'),
    ('\u{1e08f}', '\u{1e08f}'),
    ('𞄀', '𞄬'),
    ('\u{1e130}', '𞄽'),
    ('𞅀', '𞅉'),
    ('𞅎', '𞅎'),
    ('𞊐', '\u{1e2ae}'),
    ('𞋀', '𞋹'),
    ('𞓐', '𞓹'),
    ('𞗐', '𞗺'),
    ('𞟠', '𞟦'),
    ('𞟨', '𞟫'),
    ('𞟭', '𞟮'),
    ('𞟰', '𞟾'),
    ('𞠀', '𞣄'),
    ('\u{1e8d0}', '\u{1e8d6}'),
    ('𞤀', '𞥋'),
    ('𞥐', '𞥙'),
    ('𞸀', '𞸃'),
    ('𞸅', '𞸟'),
    ('𞸡', '𞸢'),
    ('𞸤', '𞸤'),
    ('𞸧', '𞸧'),
    ('𞸩', '𞸲'),
    ('𞸴', '𞸷'),
    ('𞸹', '𞸹'),
    ('𞸻', '𞸻'),
    ('𞹂', '𞹂'),
    ('𞹇', '𞹇'),
    ('𞹉', '𞹉'),
    ('𞹋', '𞹋'),
    ('𞹍', '𞹏'),
    ('𞹑', '𞹒'),
    ('𞹔', '𞹔'),
    ('𞹗', '𞹗'),
    ('𞹙', '𞹙'),
    ('𞹛', '𞹛'),
    ('𞹝', '𞹝'),
    ('𞹟', '𞹟'),
    ('𞹡', '𞹢'),
    ('𞹤', '𞹤'),
    ('𞹧', '𞹪'),
    ('𞹬', '𞹲'),
    ('𞹴', '𞹷'),
    ('𞹹', '𞹼'),
    ('𞹾', '𞹾'),
    ('𞺀', '𞺉'),
    ('𞺋', '𞺛'),
    ('𞺡', '𞺣'),
    ('𞺥', '𞺩'),
    ('𞺫', '𞺻'),
    ('🯰', '🯹'),
    ('𠀀', '𪛟'),
    ('𪜀', '𫜹'),
    ('𫝀', '𫠝'),
    ('𫠠', '𬺡'),
    ('𬺰', '𮯠'),
    ('𮯰', '𮹝'),
    ('丽', '𪘀'),
    ('𰀀', '𱍊'),
    ('𱍐', '𲎯'),
    ('\u{e0100}', '\u{e01ef}'),
];

fn literal_boundary_witness(
    source: &str,
    source_outer: &std::ops::Range<usize>,
    source_literals: &[CodeLiteralPayload<'_>],
    candidate: &str,
    candidate_edge: &std::ops::Range<usize>,
    candidate_literals: &[CodeLiteralPayload<'_>],
) -> LocalCodeBoundaryWitness {
    let block_boundary = source_literals
        .iter()
        .any(|literal| literal.block_fence && literal.outer_range == *source_outer)
        || candidate_literals
            .iter()
            .any(|literal| literal.block_fence && literal.outer_range == *candidate_edge);
    if block_boundary {
        // A closing fence line, including its newline, ends the code block.
        // Following prose is outside the literal's code unit even after fence
        // removal from the candidate.
        return LocalCodeBoundaryWitness::SameInheritedEdge;
    }

    let Some(source_left) =
        code_attachment_at_edge(source, source_outer.start, true, source_literals)
    else {
        return LocalCodeBoundaryWitness::AmbiguousCodeEdge;
    };
    let Some(source_right) =
        code_attachment_at_edge(source, source_outer.end, false, source_literals)
    else {
        return LocalCodeBoundaryWitness::AmbiguousCodeEdge;
    };
    let Some(candidate_left) =
        code_attachment_at_edge(candidate, candidate_edge.start, true, candidate_literals)
    else {
        return LocalCodeBoundaryWitness::AmbiguousCodeEdge;
    };
    let Some(candidate_right) =
        code_attachment_at_edge(candidate, candidate_edge.end, false, candidate_literals)
    else {
        return LocalCodeBoundaryWitness::AmbiguousCodeEdge;
    };
    if source_left != candidate_left || source_right != candidate_right {
        return LocalCodeBoundaryWitness::NewCodeContinuation;
    }

    for (source_anchor, candidate_anchor) in [
        (
            adjacent_cjk_character(source, source_outer.start, true),
            adjacent_cjk_character(candidate, candidate_edge.start, true),
        ),
        (
            adjacent_cjk_character(source, source_outer.end, false),
            adjacent_cjk_character(candidate, candidate_edge.end, false),
        ),
    ] {
        if candidate_anchor.is_some() && source_anchor != candidate_anchor {
            return LocalCodeBoundaryWitness::AmbiguousCodeEdge;
        }
    }
    LocalCodeBoundaryWitness::SameInheritedEdge
}

fn code_attachment_at_edge(
    text: &str,
    edge: usize,
    before: bool,
    literals: &[CodeLiteralPayload<'_>],
) -> Option<String> {
    if before {
        code_attachment_before(text, edge, literals)
    } else {
        code_attachment_after(text, edge, literals)
    }
}

fn code_attachment_before(
    text: &str,
    edge: usize,
    literals: &[CodeLiteralPayload<'_>],
) -> Option<String> {
    let mut cursor = edge;
    let Some((previous_start, previous)) = text[..edge].char_indices().next_back() else {
        return Some(String::new());
    };
    if is_code_word_character(previous) {
        let mut start = previous_start;
        let mut count = 1usize;
        while let Some((next_start, ch)) = text[..start].char_indices().next_back() {
            if !is_code_word_character(ch) {
                break;
            }
            count += 1;
            if count > MAX_LOCAL_CODE_EDGE_CHARS {
                return None;
            }
            start = next_start;
        }
        return Some(format!("word:{}", &text[start..edge]));
    }

    let mut whitespace_count = 0usize;
    while cursor > 0 {
        let Some((previous_start, ch)) = text[..cursor].char_indices().next_back() else {
            break;
        };
        if !ch.is_whitespace() {
            break;
        }
        whitespace_count += 1;
        if whitespace_count > MAX_LOCAL_CODE_EDGE_CHARS {
            return None;
        }
        cursor = previous_start;
    }
    let Some((token_start, token)) = text[..cursor].char_indices().next_back() else {
        return Some(String::new());
    };
    if token == '\x60' && is_markdown_wrapper_delimiter(literals, token_start) {
        return Some(String::new());
    }
    if matches!(token, '\'' | '"') {
        return Some(format!("quote:{token}"));
    }
    if matches!(token, '(' | '[' | '{') {
        let prefix = preceding_code_word(text, token_start)?;
        return Some(format!("call-prefix:{}{token}", prefix.unwrap_or("")));
    }
    if token == '.' {
        let prefix = preceding_code_word(text, token_start)?;
        return Some(prefix.map_or_else(
            || "prefix:.".to_owned(),
            |prefix| format!("member-prefix:{prefix}."),
        ));
    }
    if token == ':' {
        if token_start > 0 && text[..token_start].ends_with(':') {
            let prefix = preceding_code_word(text, token_start - 1)?;
            return Some(format!("operator:{}::", prefix.unwrap_or("")));
        }
        return Some("prefix::".to_owned());
    }
    if is_operator_character(token) {
        let mut operator_start = token_start;
        let mut operator_count = 1usize;
        while let Some((previous_start, ch)) = text[..operator_start].char_indices().next_back() {
            if !is_operator_character(ch) || operator_count >= MAX_OPERATOR_CHARS {
                break;
            }
            operator_count += 1;
            operator_start = previous_start;
        }
        let operator = &text[operator_start..cursor];
        if operator_count >= MAX_OPERATOR_CHARS
            && text[..operator_start]
                .chars()
                .next_back()
                .is_some_and(is_operator_character)
        {
            return None;
        }
        let prefix = preceding_code_word(text, operator_start)?;
        return Some(format!(
            "operator-prefix:{}{}",
            prefix.unwrap_or(""),
            operator
        ));
    }
    Some(String::new())
}

fn code_attachment_after(
    text: &str,
    edge: usize,
    literals: &[CodeLiteralPayload<'_>],
) -> Option<String> {
    let Some((immediate_start, immediate)) = text[edge..]
        .char_indices()
        .next()
        .map(|(relative, ch)| (edge + relative, ch))
    else {
        return Some(String::new());
    };
    if is_code_word_character(immediate) {
        let mut end = immediate_start + immediate.len_utf8();
        let mut count = 1usize;
        while let Some(ch) = text[end..].chars().next() {
            if !is_code_word_character(ch) {
                break;
            }
            count += 1;
            if count > MAX_LOCAL_CODE_EDGE_CHARS {
                return None;
            }
            end += ch.len_utf8();
        }
        return Some(format!("word:{}", &text[immediate_start..end]));
    }
    if immediate == '\x60' && is_markdown_wrapper_delimiter(literals, immediate_start) {
        return Some(String::new());
    }
    if is_cjk_context_character(immediate) {
        return Some(String::new());
    }
    if matches!(immediate, '\'' | '"') {
        return Some(format!("quote:{immediate}"));
    }

    let mut start = edge;
    let mut whitespace_count = 0usize;
    while let Some(ch) = text[start..].chars().next() {
        if !ch.is_whitespace() {
            break;
        }
        whitespace_count += 1;
        if whitespace_count > MAX_LOCAL_CODE_EDGE_CHARS {
            return None;
        }
        start += ch.len_utf8();
    }
    let Some(first) = text[start..].chars().next() else {
        return Some(String::new());
    };
    if first == '\x60' && is_markdown_wrapper_delimiter(literals, start) {
        return Some(String::new());
    }
    if matches!(first, '\'' | '"') {
        return Some(format!("quote:{first}"));
    }
    if is_cjk_context_character(first) {
        return Some(String::new());
    }
    if matches!(first, '(' | '[' | '{') || is_operator_character(first) {
        return normalized_code_suffix(text, edge);
    }
    Some(String::new())
}

const MAX_LOCAL_CODE_EDGE_CHARS: usize = 64;
const MAX_OPERATOR_CHARS: usize = 6;

fn normalized_code_suffix(text: &str, start: usize) -> Option<String> {
    let mut cursor = start;
    let mut result = String::new();
    let mut iterations = 0usize;
    loop {
        iterations += 1;
        if iterations > MAX_LOCAL_CODE_EDGE_CHARS {
            return None;
        }
        let whitespace_before_operator = cursor;
        skip_code_whitespace(text, start, &mut cursor)?;
        if !within_code_edge_bound(text, start, cursor) {
            return None;
        }
        let had_whitespace_before_operator = cursor > whitespace_before_operator;
        let Some(first) = text[cursor..].chars().next() else {
            break;
        };

        if matches!(first, '(' | '[' | '{') {
            let (group, end) = normalized_group_suffix(text, cursor, first)?;
            result.push_str(&group);
            cursor = end;
            continue;
        }
        if matches!(first, '\'' | '"' | '\x60') {
            let (quoted, end) = normalized_quoted_token(text, cursor, first)?;
            result.push_str(&quoted);
            cursor = end;
            continue;
        }

        let Some(operator) = operator_at(text, cursor) else {
            break;
        };
        let after_operator = cursor + operator.len();
        let mut operand_start = after_operator;
        skip_code_whitespace(text, start, &mut operand_start)?;
        if !within_code_edge_bound(text, start, operand_start) {
            return None;
        }
        if operator == "."
            && !had_whitespace_before_operator
            && operand_start > after_operator
            && text[operand_start..]
                .chars()
                .next()
                .is_some_and(char::is_uppercase)
        {
            // A sentence period followed by an uppercase prose word is a
            // natural-language boundary. A spaced lowercase member still
            // remains part of the inherited code edge.
            break;
        }
        let Some(operand_first) = text[operand_start..].chars().next() else {
            if is_terminal_edge_punctuation(operator) {
                break;
            }
            return None;
        };
        if is_cjk_context_character(operand_first) && operator == "." {
            if result.is_empty() {
                break;
            }
            return None;
        }

        result.push_str(operator);
        cursor = operand_start;
        if matches!(operand_first, '(' | '[' | '{') {
            let (group, end) = normalized_group_suffix(text, cursor, operand_first)?;
            result.push_str(&group);
            cursor = end;
        } else if matches!(operand_first, '\'' | '"' | '\x60') {
            let (quoted, end) = normalized_quoted_token(text, cursor, operand_first)?;
            result.push_str(&quoted);
            cursor = end;
        } else if is_code_identifier_start(operand_first, true) || operand_first.is_ascii_digit() {
            let start_operand = cursor;
            let mut count = 0usize;
            while let Some(ch) = text[cursor..].chars().next() {
                if !is_code_identifier_continue(ch, true) {
                    break;
                }
                count += 1;
                if count > MAX_LOCAL_CODE_EDGE_CHARS {
                    return None;
                }
                cursor += ch.len_utf8();
                if !within_code_edge_bound(text, start, cursor) {
                    return None;
                }
            }
            result.push_str(&text[start_operand..cursor]);
        } else {
            return None;
        }
    }
    Some(result)
}

fn normalized_group_suffix(text: &str, start: usize, opening: char) -> Option<(String, usize)> {
    let expected_close = match opening {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        _ => return None,
    };
    if text.get(start..)?.chars().next()? != opening {
        return None;
    }
    let mut stack = vec![expected_close];
    let mut cursor = start + opening.len_utf8();
    let mut result = String::from(opening);
    let mut quote = None;
    let mut escaped = false;
    while let Some(ch) = text[cursor..].chars().next() {
        cursor += ch.len_utf8();
        if !within_code_edge_bound(text, start, cursor) {
            return None;
        }

        if let Some(active_quote) = quote {
            result.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' | '\x60' => {
                quote = Some(ch);
                result.push(ch);
            }
            '(' => {
                stack.push(')');
                result.push(ch);
            }
            '[' => {
                stack.push(']');
                result.push(ch);
            }
            '{' => {
                stack.push('}');
                result.push(ch);
            }
            ')' | ']' | '}' => {
                if stack.pop() != Some(ch) {
                    return None;
                }
                result.push(ch);
                if stack.is_empty() {
                    return Some((result, cursor));
                }
            }
            _ if ch.is_whitespace() => {}
            _ => result.push(ch),
        }
    }
    None
}

fn normalized_quoted_token(text: &str, start: usize, quote: char) -> Option<(String, usize)> {
    let mut cursor = start + quote.len_utf8();
    let mut result = String::from(quote);
    let mut escaped = false;
    while let Some(ch) = text[cursor..].chars().next() {
        cursor += ch.len_utf8();
        if !within_code_edge_bound(text, start, cursor) {
            return None;
        }
        result.push(ch);
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote {
            return Some((result, cursor));
        }
    }
    None
}

fn skip_code_whitespace(text: &str, start: usize, cursor: &mut usize) -> Option<()> {
    while let Some(ch) = text[*cursor..].chars().next() {
        if !ch.is_whitespace() {
            break;
        }
        *cursor += ch.len_utf8();
        if !within_code_edge_bound(text, start, *cursor) {
            return None;
        }
    }
    Some(())
}

fn within_code_edge_bound(text: &str, start: usize, end: usize) -> bool {
    text.get(start..end)
        .is_some_and(|slice| slice.chars().count() <= MAX_LOCAL_CODE_EDGE_CHARS)
}

fn operator_at(text: &str, start: usize) -> Option<&'static str> {
    const OPERATORS: &[&str] = &[
        "!==", "===", ">>>", "<<=", ">>=", "**=", "&&=", "||=", "??=", "->", "=>", "==", "!=",
        "<=", ">=", "<<", ">>", "**", "&&", "||", "??", "?.", "::", "...", "..", ":=", "+=", "-=",
        "*=", "/=", "%=", "&=", "|=", "^=", "//", "+", "-", "*", "/", "\\", "=", "%", "&", "|",
        "^", "<", ">", ":", ".", "!", "?", "@", "~", "#", "≤", "≥", "≠", "→", "←", "⇒", "⇐", "×",
        "÷", "±", "∈", "∉", "⊂", "⊃", "⊆", "⊇", "∧", "∨", "⊕", "⊗", "∪", "∩",
    ];
    OPERATORS
        .iter()
        .copied()
        .find(|operator| text[start..].starts_with(operator))
}

fn is_terminal_edge_punctuation(operator: &str) -> bool {
    matches!(operator, "." | ":" | "!" | "?")
}

fn is_operator_character(ch: char) -> bool {
    matches!(
        ch,
        '!' | '#'
            | '$'
            | '%'
            | '&'
            | '*'
            | '+'
            | '-'
            | '.'
            | '/'
            | ':'
            | '<'
            | '='
            | '>'
            | '?'
            | '@'
            | '\\'
            | '^'
            | '|'
            | '~'
            | '≤'
            | '≥'
            | '≠'
            | '→'
            | '←'
            | '⇒'
            | '⇐'
            | '×'
            | '÷'
            | '±'
            | '∈'
            | '∉'
            | '⊂'
            | '⊃'
            | '⊆'
            | '⊇'
            | '∧'
            | '∨'
            | '⊕'
            | '⊗'
            | '∪'
            | '∩'
    )
}

// Unicode 16.0.0 General_Category=Mark ranges copied verbatim from the
// lockfile-cached regex-syntax 0.8.11 Unicode tables. Cached source:
// `regex-syntax-0.8.11/src/unicode_tables/general_category.rs`, generated by
// ucd-generate 0.3.1 with `general-category ucd-16.0.0 --chars --exclude surrogate`.
// The full Unicode notice is reproduced verbatim in the shipped
// `THIRD_PARTY_NOTICES.md`. Source SHA-256:
// 9488e3721f7c2ae20e1b77fcff9a59b4ed8f22954b8645ea6d8592eac1856423;
// MARK table SHA-256: 039174cf338e79d77bf18c91e9d021f811f3d0529cb9256064ca09e6fe28cf3a.
const UNICODE_GENERAL_CATEGORY_MARK_RANGES: &[(char, char)] = &[
    ('\u{300}', '\u{36f}'),
    ('\u{483}', '\u{489}'),
    ('\u{591}', '\u{5bd}'),
    ('\u{5bf}', '\u{5bf}'),
    ('\u{5c1}', '\u{5c2}'),
    ('\u{5c4}', '\u{5c5}'),
    ('\u{5c7}', '\u{5c7}'),
    ('\u{610}', '\u{61a}'),
    ('\u{64b}', '\u{65f}'),
    ('\u{670}', '\u{670}'),
    ('\u{6d6}', '\u{6dc}'),
    ('\u{6df}', '\u{6e4}'),
    ('\u{6e7}', '\u{6e8}'),
    ('\u{6ea}', '\u{6ed}'),
    ('\u{711}', '\u{711}'),
    ('\u{730}', '\u{74a}'),
    ('\u{7a6}', '\u{7b0}'),
    ('\u{7eb}', '\u{7f3}'),
    ('\u{7fd}', '\u{7fd}'),
    ('\u{816}', '\u{819}'),
    ('\u{81b}', '\u{823}'),
    ('\u{825}', '\u{827}'),
    ('\u{829}', '\u{82d}'),
    ('\u{859}', '\u{85b}'),
    ('\u{897}', '\u{89f}'),
    ('\u{8ca}', '\u{8e1}'),
    ('\u{8e3}', 'ः'),
    ('\u{93a}', '\u{93c}'),
    ('ा', 'ॏ'),
    ('\u{951}', '\u{957}'),
    ('\u{962}', '\u{963}'),
    ('\u{981}', 'ঃ'),
    ('\u{9bc}', '\u{9bc}'),
    ('\u{9be}', '\u{9c4}'),
    ('ে', 'ৈ'),
    ('ো', '\u{9cd}'),
    ('\u{9d7}', '\u{9d7}'),
    ('\u{9e2}', '\u{9e3}'),
    ('\u{9fe}', '\u{9fe}'),
    ('\u{a01}', 'ਃ'),
    ('\u{a3c}', '\u{a3c}'),
    ('ਾ', '\u{a42}'),
    ('\u{a47}', '\u{a48}'),
    ('\u{a4b}', '\u{a4d}'),
    ('\u{a51}', '\u{a51}'),
    ('\u{a70}', '\u{a71}'),
    ('\u{a75}', '\u{a75}'),
    ('\u{a81}', 'ઃ'),
    ('\u{abc}', '\u{abc}'),
    ('ા', '\u{ac5}'),
    ('\u{ac7}', 'ૉ'),
    ('ો', '\u{acd}'),
    ('\u{ae2}', '\u{ae3}'),
    ('\u{afa}', '\u{aff}'),
    ('\u{b01}', 'ଃ'),
    ('\u{b3c}', '\u{b3c}'),
    ('\u{b3e}', '\u{b44}'),
    ('େ', 'ୈ'),
    ('ୋ', '\u{b4d}'),
    ('\u{b55}', '\u{b57}'),
    ('\u{b62}', '\u{b63}'),
    ('\u{b82}', '\u{b82}'),
    ('\u{bbe}', 'ூ'),
    ('ெ', 'ை'),
    ('ொ', '\u{bcd}'),
    ('\u{bd7}', '\u{bd7}'),
    ('\u{c00}', '\u{c04}'),
    ('\u{c3c}', '\u{c3c}'),
    ('\u{c3e}', 'ౄ'),
    ('\u{c46}', '\u{c48}'),
    ('\u{c4a}', '\u{c4d}'),
    ('\u{c55}', '\u{c56}'),
    ('\u{c62}', '\u{c63}'),
    ('\u{c81}', 'ಃ'),
    ('\u{cbc}', '\u{cbc}'),
    ('ಾ', 'ೄ'),
    ('\u{cc6}', '\u{cc8}'),
    ('\u{cca}', '\u{ccd}'),
    ('\u{cd5}', '\u{cd6}'),
    ('\u{ce2}', '\u{ce3}'),
    ('ೳ', 'ೳ'),
    ('\u{d00}', 'ഃ'),
    ('\u{d3b}', '\u{d3c}'),
    ('\u{d3e}', '\u{d44}'),
    ('െ', 'ൈ'),
    ('ൊ', '\u{d4d}'),
    ('\u{d57}', '\u{d57}'),
    ('\u{d62}', '\u{d63}'),
    ('\u{d81}', 'ඃ'),
    ('\u{dca}', '\u{dca}'),
    ('\u{dcf}', '\u{dd4}'),
    ('\u{dd6}', '\u{dd6}'),
    ('ෘ', '\u{ddf}'),
    ('ෲ', 'ෳ'),
    ('\u{e31}', '\u{e31}'),
    ('\u{e34}', '\u{e3a}'),
    ('\u{e47}', '\u{e4e}'),
    ('\u{eb1}', '\u{eb1}'),
    ('\u{eb4}', '\u{ebc}'),
    ('\u{ec8}', '\u{ece}'),
    ('\u{f18}', '\u{f19}'),
    ('\u{f35}', '\u{f35}'),
    ('\u{f37}', '\u{f37}'),
    ('\u{f39}', '\u{f39}'),
    ('༾', '༿'),
    ('\u{f71}', '\u{f84}'),
    ('\u{f86}', '\u{f87}'),
    ('\u{f8d}', '\u{f97}'),
    ('\u{f99}', '\u{fbc}'),
    ('\u{fc6}', '\u{fc6}'),
    ('ါ', '\u{103e}'),
    ('ၖ', '\u{1059}'),
    ('\u{105e}', '\u{1060}'),
    ('ၢ', 'ၤ'),
    ('ၧ', 'ၭ'),
    ('\u{1071}', '\u{1074}'),
    ('\u{1082}', '\u{108d}'),
    ('ႏ', 'ႏ'),
    ('ႚ', '\u{109d}'),
    ('\u{135d}', '\u{135f}'),
    ('\u{1712}', '\u{1715}'),
    ('\u{1732}', '\u{1734}'),
    ('\u{1752}', '\u{1753}'),
    ('\u{1772}', '\u{1773}'),
    ('\u{17b4}', '\u{17d3}'),
    ('\u{17dd}', '\u{17dd}'),
    ('\u{180b}', '\u{180d}'),
    ('\u{180f}', '\u{180f}'),
    ('\u{1885}', '\u{1886}'),
    ('\u{18a9}', '\u{18a9}'),
    ('\u{1920}', 'ᤫ'),
    ('ᤰ', '\u{193b}'),
    ('\u{1a17}', '\u{1a1b}'),
    ('ᩕ', '\u{1a5e}'),
    ('\u{1a60}', '\u{1a7c}'),
    ('\u{1a7f}', '\u{1a7f}'),
    ('\u{1ab0}', '\u{1ace}'),
    ('\u{1b00}', 'ᬄ'),
    ('\u{1b34}', '\u{1b44}'),
    ('\u{1b6b}', '\u{1b73}'),
    ('\u{1b80}', 'ᮂ'),
    ('ᮡ', '\u{1bad}'),
    ('\u{1be6}', '\u{1bf3}'),
    ('ᰤ', '\u{1c37}'),
    ('\u{1cd0}', '\u{1cd2}'),
    ('\u{1cd4}', '\u{1ce8}'),
    ('\u{1ced}', '\u{1ced}'),
    ('\u{1cf4}', '\u{1cf4}'),
    ('᳷', '\u{1cf9}'),
    ('\u{1dc0}', '\u{1dff}'),
    ('\u{20d0}', '\u{20f0}'),
    ('\u{2cef}', '\u{2cf1}'),
    ('\u{2d7f}', '\u{2d7f}'),
    ('\u{2de0}', '\u{2dff}'),
    ('\u{302a}', '\u{302f}'),
    ('\u{3099}', '\u{309a}'),
    ('\u{a66f}', '\u{a672}'),
    ('\u{a674}', '\u{a67d}'),
    ('\u{a69e}', '\u{a69f}'),
    ('\u{a6f0}', '\u{a6f1}'),
    ('\u{a802}', '\u{a802}'),
    ('\u{a806}', '\u{a806}'),
    ('\u{a80b}', '\u{a80b}'),
    ('ꠣ', 'ꠧ'),
    ('\u{a82c}', '\u{a82c}'),
    ('ꢀ', 'ꢁ'),
    ('ꢴ', '\u{a8c5}'),
    ('\u{a8e0}', '\u{a8f1}'),
    ('\u{a8ff}', '\u{a8ff}'),
    ('\u{a926}', '\u{a92d}'),
    ('\u{a947}', '\u{a953}'),
    ('\u{a980}', 'ꦃ'),
    ('\u{a9b3}', '\u{a9c0}'),
    ('\u{a9e5}', '\u{a9e5}'),
    ('\u{aa29}', '\u{aa36}'),
    ('\u{aa43}', '\u{aa43}'),
    ('\u{aa4c}', 'ꩍ'),
    ('ꩻ', 'ꩽ'),
    ('\u{aab0}', '\u{aab0}'),
    ('\u{aab2}', '\u{aab4}'),
    ('\u{aab7}', '\u{aab8}'),
    ('\u{aabe}', '\u{aabf}'),
    ('\u{aac1}', '\u{aac1}'),
    ('ꫫ', 'ꫯ'),
    ('ꫵ', '\u{aaf6}'),
    ('ꯣ', 'ꯪ'),
    ('꯬', '\u{abed}'),
    ('\u{fb1e}', '\u{fb1e}'),
    ('\u{fe00}', '\u{fe0f}'),
    ('\u{fe20}', '\u{fe2f}'),
    ('\u{101fd}', '\u{101fd}'),
    ('\u{102e0}', '\u{102e0}'),
    ('\u{10376}', '\u{1037a}'),
    ('\u{10a01}', '\u{10a03}'),
    ('\u{10a05}', '\u{10a06}'),
    ('\u{10a0c}', '\u{10a0f}'),
    ('\u{10a38}', '\u{10a3a}'),
    ('\u{10a3f}', '\u{10a3f}'),
    ('\u{10ae5}', '\u{10ae6}'),
    ('\u{10d24}', '\u{10d27}'),
    ('\u{10d69}', '\u{10d6d}'),
    ('\u{10eab}', '\u{10eac}'),
    ('\u{10efc}', '\u{10eff}'),
    ('\u{10f46}', '\u{10f50}'),
    ('\u{10f82}', '\u{10f85}'),
    ('𑀀', '𑀂'),
    ('\u{11038}', '\u{11046}'),
    ('\u{11070}', '\u{11070}'),
    ('\u{11073}', '\u{11074}'),
    ('\u{1107f}', '𑂂'),
    ('𑂰', '\u{110ba}'),
    ('\u{110c2}', '\u{110c2}'),
    ('\u{11100}', '\u{11102}'),
    ('\u{11127}', '\u{11134}'),
    ('𑅅', '𑅆'),
    ('\u{11173}', '\u{11173}'),
    ('\u{11180}', '𑆂'),
    ('𑆳', '\u{111c0}'),
    ('\u{111c9}', '\u{111cc}'),
    ('𑇎', '\u{111cf}'),
    ('𑈬', '\u{11237}'),
    ('\u{1123e}', '\u{1123e}'),
    ('\u{11241}', '\u{11241}'),
    ('\u{112df}', '\u{112ea}'),
    ('\u{11300}', '𑌃'),
    ('\u{1133b}', '\u{1133c}'),
    ('\u{1133e}', '𑍄'),
    ('𑍇', '𑍈'),
    ('𑍋', '\u{1134d}'),
    ('\u{11357}', '\u{11357}'),
    ('𑍢', '𑍣'),
    ('\u{11366}', '\u{1136c}'),
    ('\u{11370}', '\u{11374}'),
    ('\u{113b8}', '\u{113c0}'),
    ('\u{113c2}', '\u{113c2}'),
    ('\u{113c5}', '\u{113c5}'),
    ('\u{113c7}', '𑏊'),
    ('𑏌', '\u{113d0}'),
    ('\u{113d2}', '\u{113d2}'),
    ('\u{113e1}', '\u{113e2}'),
    ('𑐵', '\u{11446}'),
    ('\u{1145e}', '\u{1145e}'),
    ('\u{114b0}', '\u{114c3}'),
    ('\u{115af}', '\u{115b5}'),
    ('𑖸', '\u{115c0}'),
    ('\u{115dc}', '\u{115dd}'),
    ('𑘰', '\u{11640}'),
    ('\u{116ab}', '\u{116b7}'),
    ('\u{1171d}', '\u{1172b}'),
    ('𑠬', '\u{1183a}'),
    ('\u{11930}', '𑤵'),
    ('𑤷', '𑤸'),
    ('\u{1193b}', '\u{1193e}'),
    ('𑥀', '𑥀'),
    ('𑥂', '\u{11943}'),
    ('𑧑', '\u{119d7}'),
    ('\u{119da}', '\u{119e0}'),
    ('𑧤', '𑧤'),
    ('\u{11a01}', '\u{11a0a}'),
    ('\u{11a33}', '𑨹'),
    ('\u{11a3b}', '\u{11a3e}'),
    ('\u{11a47}', '\u{11a47}'),
    ('\u{11a51}', '\u{11a5b}'),
    ('\u{11a8a}', '\u{11a99}'),
    ('𑰯', '\u{11c36}'),
    ('\u{11c38}', '\u{11c3f}'),
    ('\u{11c92}', '\u{11ca7}'),
    ('𑲩', '\u{11cb6}'),
    ('\u{11d31}', '\u{11d36}'),
    ('\u{11d3a}', '\u{11d3a}'),
    ('\u{11d3c}', '\u{11d3d}'),
    ('\u{11d3f}', '\u{11d45}'),
    ('\u{11d47}', '\u{11d47}'),
    ('𑶊', '𑶎'),
    ('\u{11d90}', '\u{11d91}'),
    ('𑶓', '\u{11d97}'),
    ('\u{11ef3}', '𑻶'),
    ('\u{11f00}', '\u{11f01}'),
    ('𑼃', '𑼃'),
    ('𑼴', '\u{11f3a}'),
    ('𑼾', '\u{11f42}'),
    ('\u{11f5a}', '\u{11f5a}'),
    ('\u{13440}', '\u{13440}'),
    ('\u{13447}', '\u{13455}'),
    ('\u{1611e}', '\u{1612f}'),
    ('\u{16af0}', '\u{16af4}'),
    ('\u{16b30}', '\u{16b36}'),
    ('\u{16f4f}', '\u{16f4f}'),
    ('𖽑', '𖾇'),
    ('\u{16f8f}', '\u{16f92}'),
    ('\u{16fe4}', '\u{16fe4}'),
    ('\u{16ff0}', '\u{16ff1}'),
    ('\u{1bc9d}', '\u{1bc9e}'),
    ('\u{1cf00}', '\u{1cf2d}'),
    ('\u{1cf30}', '\u{1cf46}'),
    ('\u{1d165}', '\u{1d169}'),
    ('\u{1d16d}', '\u{1d172}'),
    ('\u{1d17b}', '\u{1d182}'),
    ('\u{1d185}', '\u{1d18b}'),
    ('\u{1d1aa}', '\u{1d1ad}'),
    ('\u{1d242}', '\u{1d244}'),
    ('\u{1da00}', '\u{1da36}'),
    ('\u{1da3b}', '\u{1da6c}'),
    ('\u{1da75}', '\u{1da75}'),
    ('\u{1da84}', '\u{1da84}'),
    ('\u{1da9b}', '\u{1da9f}'),
    ('\u{1daa1}', '\u{1daaf}'),
    ('\u{1e000}', '\u{1e006}'),
    ('\u{1e008}', '\u{1e018}'),
    ('\u{1e01b}', '\u{1e021}'),
    ('\u{1e023}', '\u{1e024}'),
    ('\u{1e026}', '\u{1e02a}'),
    ('\u{1e08f}', '\u{1e08f}'),
    ('\u{1e130}', '\u{1e136}'),
    ('\u{1e2ae}', '\u{1e2ae}'),
    ('\u{1e2ec}', '\u{1e2ef}'),
    ('\u{1e4ec}', '\u{1e4ef}'),
    ('\u{1e5ee}', '\u{1e5ef}'),
    ('\u{1e8d0}', '\u{1e8d6}'),
    ('\u{1e944}', '\u{1e94a}'),
    ('\u{e0100}', '\u{e01ef}'),
];

fn is_code_word_character(ch: char) -> bool {
    is_code_identifier_continue(ch, false)
}

fn is_code_identifier_start(ch: char, allow_cjk: bool) -> bool {
    let is_start = is_unicode_xid_start(ch) || matches!(ch, '_' | '$');
    is_start && (allow_cjk || !is_cjk_context_character(ch))
}

fn is_code_identifier_continue(ch: char, allow_cjk: bool) -> bool {
    let is_continue = is_unicode_xid_continue(ch) || is_unicode_mark(ch) || ch == '$';
    is_continue && (allow_cjk || !is_cjk_context_character(ch))
}

fn is_unicode_xid_start(ch: char) -> bool {
    unicode_character_in_ranges(ch, UNICODE_XID_START_RANGES)
}

fn is_unicode_xid_continue(ch: char) -> bool {
    unicode_character_in_ranges(ch, UNICODE_XID_CONTINUE_RANGES)
}

fn is_unicode_mark(ch: char) -> bool {
    unicode_character_in_ranges(ch, UNICODE_GENERAL_CATEGORY_MARK_RANGES)
}

fn unicode_character_in_ranges(ch: char, ranges: &[(char, char)]) -> bool {
    ranges
        .binary_search_by(|(start, end)| {
            if ch < *start {
                std::cmp::Ordering::Greater
            } else if ch > *end {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

fn preceding_code_word(text: &str, before: usize) -> Option<Option<&str>> {
    let mut end = before;
    let mut whitespace_count = 0usize;
    while let Some(ch) = text[..end].chars().next_back() {
        if !ch.is_whitespace() {
            break;
        }
        whitespace_count += 1;
        if whitespace_count > MAX_LOCAL_CODE_EDGE_CHARS {
            return None;
        }
        end -= ch.len_utf8();
    }
    let mut start = end;
    let mut count = 0usize;
    while let Some((previous, ch)) = text[..start].char_indices().next_back() {
        if !is_code_word_character(ch) {
            break;
        }
        count += 1;
        if count > MAX_LOCAL_CODE_EDGE_CHARS {
            return None;
        }
        start = previous;
    }
    Some((start < end).then_some(&text[start..end]))
}

fn is_markdown_wrapper_delimiter(literals: &[CodeLiteralPayload<'_>], offset: usize) -> bool {
    literals.iter().any(|literal| {
        literal.outer_range.start <= offset
            && offset < literal.outer_range.end
            && (offset < literal.body_range.start || offset >= literal.body_range.end)
    })
}

fn adjacent_cjk_character(text: &str, edge: usize, before: bool) -> Option<char> {
    let ch = if before {
        text[..edge].chars().next_back()?
    } else {
        text[edge..].chars().next()?
    };
    is_cjk_context_character(ch).then_some(ch)
}

fn is_cjk_context_character(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2E80..=0x2EFF
            | 0x2F00..=0x2FDF
            | 0x3000..=0x30FF
            | 0x3100..=0x312F
            | 0x3130..=0x318F
            | 0x31A0..=0x31BF
            | 0x31F0..=0x31FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xAC00..=0xD7AF
            | 0xF900..=0xFAFF
            | 0x20000..=0x3134F
    )
}

fn candidate_span_alternatives(
    plan: &VerifiedLiteralBindings<'_, '_>,
    operation: Option<CleanupOperation>,
) -> Vec<ProtectedSpan> {
    if plan.bindings.is_empty() {
        return Vec::new();
    }
    if plan.bindings.iter().any(|binding| {
        binding.source_outer.start > binding.source_body.start
            || binding.source_body.end > binding.source_outer.end
            || binding.candidate_body.end > plan.candidate.len()
            || plan.source.get(binding.source_body.clone())
                != plan.candidate.get(binding.candidate_body.clone())
            || binding.candidate_outer.as_ref().is_some_and(|outer| {
                outer.start > binding.candidate_body.start || binding.candidate_body.end > outer.end
            })
    }) {
        return Vec::new();
    }
    let mut view = String::with_capacity(plan.candidate.len() + plan.bindings.len() * 2);
    let mut origins = Vec::with_capacity(view.capacity());
    let mut cursor = 0;
    for binding in &plan.bindings {
        let wrapper = binding
            .candidate_outer
            .as_ref()
            .unwrap_or(&binding.candidate_body);
        append_candidate_view_slice(
            &mut view,
            &mut origins,
            plan.candidate,
            cursor..wrapper.start,
        );
        view.push(' ');
        origins.push(None);
        append_candidate_view_slice(
            &mut view,
            &mut origins,
            plan.candidate,
            binding.candidate_body.clone(),
        );
        view.push(' ');
        origins.push(None);
        cursor = wrapper.end;
    }
    append_candidate_view_slice(
        &mut view,
        &mut origins,
        plan.candidate,
        cursor..plan.candidate.len(),
    );

    protected_spans(&view, operation)
        .into_iter()
        .filter_map(|span| {
            let span_origins = origins.get(span.start_byte..span.end_byte)?;
            let first = span_origins.first().copied().flatten()?;
            if span_origins
                .iter()
                .enumerate()
                .any(|(offset, origin)| *origin != Some(first + offset))
            {
                return None;
            }
            let end = first + span_origins.len();
            let candidate_token = plan.candidate.get(first..end)?;
            let view_token = view.get(span.start_byte..span.end_byte)?;
            (candidate_token == view_token).then_some(ProtectedSpan {
                start_byte: first,
                end_byte: end,
                kind: span.kind,
            })
        })
        .collect()
}

fn append_candidate_view_slice(
    view: &mut String,
    origins: &mut Vec<Option<usize>>,
    candidate: &str,
    range: std::ops::Range<usize>,
) {
    view.push_str(&candidate[range.clone()]);
    origins.extend(range.map(Some));
}

fn code_literal_payload_range(
    text: &str,
    range: &std::ops::Range<usize>,
) -> Option<std::ops::Range<usize>> {
    let range_text = text.get(range.clone())?;
    let first_line_end = range.start + range_text.find('\n').unwrap_or(range_text.len());
    let first_line = text.get(range.start..first_line_end)?;
    if let Some((fence_char, fence_len)) = markdown_fence(first_line) {
        let payload_start = if first_line_end < range.end {
            first_line_end + 1
        } else {
            range.end
        };
        let final_line_end = range.end - usize::from(text[..range.end].ends_with('\n'));
        let final_line_start = text[..final_line_end]
            .rfind('\n')
            .map_or(range.start, |newline| newline + 1);
        let final_line = text.get(final_line_start..final_line_end)?;
        let payload_end = if markdown_fence_close(final_line, fence_char, fence_len) {
            final_line_start
        } else {
            range.end
        };
        return (payload_start <= payload_end).then_some(payload_start..payload_end);
    }

    let opening_run = range_text.bytes().take_while(|byte| *byte == b'`').count();
    if opening_run == 0 || opening_run >= 3 {
        return None;
    }
    let payload_start = range.start + opening_run;
    let closing_run = range_text
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'`')
        .count();
    let payload_end = if closing_run == opening_run {
        range.end - closing_run
    } else {
        range.end
    };
    (payload_start <= payload_end).then_some(payload_start..payload_end)
}

fn markdown_fence(line: &str) -> Option<(char, usize)> {
    let leading = line.len() - line.trim_start().len();
    if leading > 3 {
        return None;
    }
    let remainder = &line[leading..];
    let marker = remainder.chars().next()?;
    if !matches!(marker, '`' | '~') {
        return None;
    }
    let run = remainder.chars().take_while(|ch| *ch == marker).count();
    (run >= 3).then_some((marker, run))
}

fn markdown_fence_close(line: &str, marker: char, minimum_run: usize) -> bool {
    let leading = line.len() - line.trim_start().len();
    if leading > 3 {
        return false;
    }
    let remainder = &line[leading..];
    let run = remainder.chars().take_while(|ch| *ch == marker).count();
    run >= minimum_run
        && remainder[remainder
            .char_indices()
            .take(run)
            .last()
            .map_or(0, |(index, ch)| index + ch.len_utf8())..]
            .trim()
            .is_empty()
}

fn should_preserve_deictic_references(operation: Option<CleanupOperation>) -> bool {
    matches!(operation, None | Some(CleanupOperation::Cleanup))
}

/// Collect only demonstratives used as attached noun phrases. This deliberately
/// leaves standalone fillers such as `那个，麻烦你…` and English complementizer
/// phrases such as `I think that it is ready` editable by normal cleanup.
fn collect_deictic_references(text: &str, out: &mut Vec<ProtectedSpan>) {
    const CHINESE_DETERMINERS: &[&str] = &[
        "这部分",
        "那部分",
        "这方面",
        "那方面",
        "这些",
        "那些",
        "这个",
        "那个",
        "这种",
        "那种",
        "这件",
        "那件",
        "这段",
        "那段",
        "这份",
        "那份",
        "这条",
        "那条",
        "这位",
        "那位",
        "这家",
        "那家",
        "这场",
        "那场",
        "这款",
        "那款",
        "这篇",
        "那篇",
        "这只",
        "那只",
        "这张",
        "那张",
        "这本",
        "那本",
        "这项",
        "那项",
        "这台",
        "那台",
        "这次",
        "那次",
        "这边",
        "那边",
    ];
    const CHINESE_FILLER_OR_REQUEST_PREFIXES: &[&str] = &[
        "麻烦",
        "请",
        "把",
        "给",
        "发",
        "说",
        "问",
        "帮",
        "先",
        "再",
        "看一下",
        "听一下",
        "想",
        "我",
        "我们",
        "你",
        "你们",
        "他",
        "她",
        "它",
        "嗯",
        "啊",
        "就",
        "很",
        "挺",
        "特别",
        "比较",
        "都",
        "也",
        "还",
        "吧",
        "呢",
        "嘛",
        "吗",
    ];
    for marker in CHINESE_DETERMINERS {
        for (start, _) in text.match_indices(marker) {
            let after_marker = start + marker.len();
            let remainder = text[after_marker..].trim_start();
            let Some(next) = remainder.chars().next() else {
                continue;
            };
            if CHINESE_FILLER_OR_REQUEST_PREFIXES
                .iter()
                .any(|prefix| remainder.starts_with(prefix))
            {
                continue;
            }
            if is_cjk_character(next) || next.is_ascii_alphanumeric() {
                push(
                    out,
                    start,
                    after_marker,
                    ProtectedSpanKind::DeicticReference,
                );
            }
        }
    }

    const ENGLISH_CLAUSE_CONTINUATIONS: &[&str] = &[
        "a",
        "an",
        "the",
        "it",
        "he",
        "she",
        "they",
        "we",
        "you",
        "i",
        "there",
        "this",
        "these",
        "those",
        "everything",
        "something",
        "someone",
        "nothing",
        "nobody",
        "who",
        "what",
        "when",
        "where",
        "whether",
        "if",
        "in",
        "on",
        "at",
        "for",
        "from",
        "to",
        "with",
        "without",
        "about",
        "over",
        "under",
        "after",
        "before",
        "because",
        "although",
        "while",
        "since",
        "do",
        "does",
        "did",
        "is",
        "are",
        "was",
        "were",
        "has",
        "have",
        "had",
        "can",
        "could",
        "will",
        "would",
        "should",
        "might",
        "must",
    ];
    let tokens = ascii_tokens(text);
    for (index, (start, token)) in tokens.iter().enumerate() {
        if !["this", "that", "these", "those"]
            .iter()
            .any(|marker| marker.eq_ignore_ascii_case(token))
        {
            continue;
        }
        let Some((next_start, next_token)) = tokens.get(index + 1) else {
            continue;
        };
        let end = start + token.len();
        if !text[end..*next_start].chars().all(char::is_whitespace) {
            continue;
        }
        // `that` is often a clause connector, not a referent. Require a noun-like
        // following token instead of locking phrases such as `that it is ready`.
        if token.eq_ignore_ascii_case("that")
            && ENGLISH_CLAUSE_CONTINUATIONS
                .iter()
                .any(|word| word.eq_ignore_ascii_case(next_token))
        {
            continue;
        }
        if next_token.chars().any(char::is_alphanumeric) {
            push(out, *start, end, ProtectedSpanKind::DeicticReference);
        }
    }
}

fn deictic_reference_signature(text: &str) -> Vec<String> {
    let mut spans = Vec::new();
    collect_deictic_references(text, &mut spans);
    spans.sort_by_key(|span| span.start_byte);
    spans
        .into_iter()
        .map(|span| text[span.start_byte..span.end_byte].to_ascii_lowercase())
        .collect()
}

fn is_weekday_alias(word: &str) -> bool {
    let folded = word.to_lowercase();
    WEEKDAYS
        .iter()
        .any(|(aliases, _)| aliases.contains(&folded.as_str()))
}

fn is_weekday_number_component(text: &str, span: ProtectedSpan) -> bool {
    let Some(number) = text.get(span.start_byte..span.end_byte) else {
        return false;
    };
    if !matches!(
        number,
        "一" | "二" | "三" | "四" | "五" | "六" | "日" | "天"
    ) {
        return false;
    }
    WEEKDAYS.iter().any(|(aliases, _)| {
        aliases.iter().any(|alias| {
            !alias.is_ascii()
                && alias.ends_with(number)
                && text.match_indices(alias).any(|(start, _)| {
                    start + alias.len() == span.end_byte
                        && span.start_byte == span.end_byte - number.len()
                })
        })
    })
}

/// Count weekday mentions by their calendar day so an authorized translation
/// can localize the word while still rejecting a changed day. This is a narrow
/// deterministic equivalence check, not general semantic verification.
const WEEKDAYS: &[(&[&str], u8)] = &[
    (
        &[
            "monday",
            "lundi",
            "lunes",
            "montag",
            "月曜日",
            "月曜",
            "월요일",
            "周一",
            "星期一",
            "礼拜一",
        ],
        1,
    ),
    (
        &[
            "tuesday",
            "mardi",
            "martes",
            "dienstag",
            "火曜日",
            "火曜",
            "화요일",
            "周二",
            "星期二",
            "礼拜二",
        ],
        2,
    ),
    (
        &[
            "wednesday",
            "mercredi",
            "miércoles",
            "mittwoch",
            "水曜日",
            "水曜",
            "수요일",
            "周三",
            "星期三",
            "礼拜三",
        ],
        3,
    ),
    (
        &[
            "thursday",
            "jeudi",
            "jueves",
            "donnerstag",
            "木曜日",
            "木曜",
            "목요일",
            "周四",
            "星期四",
            "礼拜四",
        ],
        4,
    ),
    (
        &[
            "friday",
            "vendredi",
            "viernes",
            "freitag",
            "金曜日",
            "金曜",
            "금요일",
            "周五",
            "星期五",
            "礼拜五",
        ],
        5,
    ),
    (
        &[
            "saturday",
            "samedi",
            "sábado",
            "samstag",
            "土曜日",
            "土曜",
            "토요일",
            "周六",
            "星期六",
            "礼拜六",
        ],
        6,
    ),
    (
        &[
            "sunday",
            "dimanche",
            "domingo",
            "sonntag",
            "日曜日",
            "日曜",
            "일요일",
            "周日",
            "周天",
            "星期日",
            "星期天",
            "礼拜日",
            "礼拜天",
        ],
        7,
    ),
];

fn weekday_counts(text: &str) -> std::collections::BTreeMap<u8, usize> {
    let folded = text.to_lowercase();
    let mut counts = std::collections::BTreeMap::new();
    for (aliases, day) in WEEKDAYS {
        let mut count = 0;
        for alias in *aliases {
            for (start, _) in folded.match_indices(alias) {
                let end = start + alias.len();
                let ascii_word = alias.is_ascii();
                let before = folded[..start].chars().next_back();
                let after = folded[end..].chars().next();
                if ascii_word
                    && (before.is_some_and(char::is_alphanumeric)
                        || after.is_some_and(char::is_alphanumeric))
                {
                    continue;
                }
                count += 1;
            }
        }
        if count > 0 {
            counts.insert(*day, count);
        }
    }
    counts
}

fn preserves_negation(source: &str, candidate: &str, operation: Option<CleanupOperation>) -> bool {
    if operation == Some(CleanupOperation::Translate) {
        return true;
    }
    let source_bindings = negation_bindings(source);
    let candidate_bindings = negation_bindings(candidate);
    if source_bindings.len() != candidate_bindings.len()
        || source_bindings.iter().zip(&candidate_bindings).any(
            |((source_marker, source_target), (candidate_marker, candidate_target))| {
                source_marker != candidate_marker
                    || source_target.is_some() != candidate_target.is_some()
            },
        )
    {
        return false;
    }
    if source_bindings.is_empty() {
        return true;
    }

    let (source_content, source_targets) = negation_content_and_targets(source, &source_bindings);
    let (candidate_content, candidate_targets) =
        negation_content_and_targets(candidate, &candidate_bindings);
    let mut expected = vec![None; source_content.len()];
    for ((_, source_target), (_, candidate_target)) in
        source_bindings.iter().zip(&candidate_bindings)
    {
        if let (Some(source_index), Some(candidate_index)) = (source_target, candidate_target) {
            if expected[*source_index].is_some_and(|existing| existing != *candidate_index) {
                return false;
            }
            expected[*source_index] = Some(*candidate_index);
        }
    }
    if source_content == candidate_content {
        return source_targets
            .iter()
            .zip(&candidate_targets)
            .all(|(left, right)| left == right)
            && expected.iter().enumerate().all(|(index, target)| {
                target.is_none_or(|candidate_index| candidate_index == index)
            });
    }
    negation_targets_have_unambiguous_lcs(&source_content, &candidate_content, &expected)
}

fn canonical_negation_marker(marker: &str) -> String {
    for (contracted, expanded) in AUXILIARY_NEGATION_PAIRS {
        if marker == *contracted || marker == *expanded {
            return (*contracted).to_owned();
        }
    }
    marker.to_owned()
}

// Finite equivalences retained from the legacy guard: each pair has the same
// auxiliary and maps one source marker to one expanded surface expression.
const AUXILIARY_NEGATION_PAIRS: &[(&str, &str)] = &[
    ("shouldn't", "should not"),
    ("wouldn't", "would not"),
    ("couldn't", "could not"),
    ("doesn't", "does not"),
    ("don't", "do not"),
    ("didn't", "did not"),
    ("isn't", "is not"),
    ("aren't", "are not"),
    ("wasn't", "was not"),
    ("weren't", "were not"),
    ("won't", "will not"),
    ("mustn't", "must not"),
    ("cannot", "can not"),
];

fn auxiliary_negation_expansion(
    text: &str,
    tokens: &[(usize, &str)],
    not_start: usize,
    cursor: &mut usize,
) -> Option<(usize, &'static str)> {
    while tokens
        .get(*cursor)
        .is_some_and(|(start, token)| *start + token.len() <= not_start)
    {
        *cursor += 1;
    }
    let (aux_start, aux) = *(*cursor).checked_sub(1).and_then(|i| tokens.get(i))?;
    // A marker inside a larger ASCII token (for example the second `not` in
    // `not-not`) cannot be adjacent to the preceding auxiliary. Reject it
    // before inspecting the intervening source, which may contain a long gap
    // already examined for the token's first `not`.
    if tokens
        .get(*cursor)
        .is_some_and(|(start, _)| *start < not_start)
    {
        return None;
    }
    if !text[aux_start + aux.len()..not_start]
        .chars()
        .all(char::is_whitespace)
    {
        return None;
    }
    let pair = AUXILIARY_NEGATION_PAIRS
        .iter()
        .find(|(_, expanded)| expanded.strip_suffix(" not") == Some(aux))?;
    Some((aux_start, pair.0))
}

pub(crate) fn contains_negation(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase().replace(['’', '‘', 'ʼ'], "'");
    !negation_marker_ranges(&normalized).is_empty()
}

// Recognized markers are a finite guard, not a semantic classifier.
fn negation_bindings(text: &str) -> Vec<(String, Option<usize>)> {
    // Fold English casing and common curly apostrophes before matching.
    let normalized = text.to_ascii_lowercase().replace(['’', '‘', 'ʼ'], "'");
    let text = normalized.as_str();
    let ranges = negation_marker_ranges(text);
    if ranges.is_empty() {
        return Vec::new();
    }
    let tokens = ascii_tokens(text);
    let mut token_cursor = 0;
    let mut selected: Vec<(usize, usize, &str)> = Vec::new();
    for range in ranges {
        if !selected.last().is_some_and(|previous| range.0 < previous.1) {
            if range.2 == "not" {
                if let Some((aux_start, canonical)) =
                    auxiliary_negation_expansion(text, &tokens, range.0, &mut token_cursor)
                {
                    selected.push((aux_start, range.1, canonical));
                    continue;
                }
            }
            selected.push(range);
        }
    }

    // Punctuation and whitespace do not define scope. ASCII words remain whole;
    // CJK characters are individual content tokens so repeated phrases in
    // separate clauses keep distinct local bindings.
    let mut content: Vec<(usize, String)> = Vec::new();
    let mut ascii_word: Option<usize> = None;
    let mut range_cursor = 0;
    for (index, ch) in text.char_indices() {
        while selected
            .get(range_cursor)
            .is_some_and(|(_, end, _)| *end <= index)
        {
            range_cursor += 1;
        }
        if selected
            .get(range_cursor)
            .is_some_and(|(start, end, _)| *start <= index && index < *end)
        {
            if let Some(start) = ascii_word.take() {
                content.push((start, text[start..index].to_owned()));
            }
            continue;
        }
        if ch.is_ascii_alphanumeric() || ch == '_' {
            ascii_word.get_or_insert(index);
        } else {
            if let Some(start) = ascii_word.take() {
                content.push((start, text[start..index].to_owned()));
            }
            if !ch.is_ascii() && ch.is_alphanumeric() {
                content.push((index, ch.to_string()));
            }
        }
    }
    if let Some(start) = ascii_word {
        content.push((start, text[start..].to_owned()));
    }

    let mut content_cursor = 0;
    selected
        .iter()
        .map(|(start, _, marker)| {
            while content
                .get(content_cursor)
                .is_some_and(|(content_start, _)| *content_start <= *start)
            {
                content_cursor += 1;
            }
            (
                canonical_negation_marker(marker),
                (content_cursor < content.len()).then_some(content_cursor),
            )
        })
        .collect()
}

fn negation_content_and_targets(
    text: &str,
    bindings: &[(String, Option<usize>)],
) -> (Vec<String>, Vec<Option<usize>>) {
    let normalized = text.to_ascii_lowercase().replace(['’', '‘', 'ʼ'], "'");
    let text = normalized.as_str();
    let ranges = negation_expression_ranges(text);
    let content = negation_content_tokens(text, &ranges);
    let targets = bindings.iter().map(|(_, target)| *target).collect();
    (content, targets)
}

fn negation_expression_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let tokens = ascii_tokens(text);
    let mut token_cursor = 0;
    for (start, end, marker) in negation_marker_ranges(text) {
        if marker == "not" {
            if let Some((aux_start, _)) =
                auxiliary_negation_expansion(text, &tokens, start, &mut token_cursor)
            {
                ranges.push((aux_start, end));
                continue;
            }
        }
        ranges.push((start, end));
    }
    ranges
}

fn negation_marker_ranges(text: &str) -> Vec<(usize, usize, &'static str)> {
    let mut ranges = Vec::new();
    for (start, ch) in text.char_indices() {
        let markers: &[&str] = match ch {
            'a' => &["aren't"],
            'c' => &["cannot", "can't", "couldn't"],
            'd' => &["doesn't", "don't", "didn't"],
            'h' => &["haven't", "hasn't", "hadn't"],
            'i' => &["isn't"],
            'm' => &["mustn't"],
            'n' => &[
                "needn't", "neither", "nor", "nobody", "nothing", "never", "none", "not", "no",
            ],
            's' => &["shouldn't"],
            'w' => &["wouldn't", "wasn't", "weren't", "won't", "without"],
            '禁' => &["禁止"],
            '并' => &["并非"],
            '不' => &["不要", "不能", "不会", "不是", "不"],
            '无' => &["无法"],
            '没' => &["没有", "没"],
            '未' => &["未曾", "未"],
            '别' => &["别"],
            '勿' => &["勿"],
            _ => &[],
        };
        for marker in markers {
            if !text[start..].starts_with(*marker) {
                continue;
            }
            let end = start + marker.len();
            let before = text[..start].chars().next_back();
            let after = text[end..].chars().next();
            if marker.is_ascii()
                && (before.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                    || after.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_'))
            {
                continue;
            }
            ranges.push((start, end, *marker));
        }
    }
    ranges.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
    let mut selected: Vec<(usize, usize, &'static str)> = Vec::new();
    for range in ranges {
        if !selected.last().is_some_and(|prior| range.0 < prior.1) {
            selected.push(range);
        }
    }
    selected
}

fn negation_content_tokens(text: &str, ranges: &[(usize, usize)]) -> Vec<String> {
    let mut content = Vec::new();
    let mut ascii_start = None;
    let mut range_cursor = 0;
    for (index, ch) in text.char_indices() {
        while ranges
            .get(range_cursor)
            .is_some_and(|(_, end)| *end <= index)
        {
            range_cursor += 1;
        }
        if ranges
            .get(range_cursor)
            .is_some_and(|(start, end)| *start <= index && index < *end)
        {
            if let Some(start) = ascii_start.take() {
                content.push(text[start..index].to_owned());
            }
        } else if ch.is_ascii_alphanumeric() || ch == '_' {
            ascii_start.get_or_insert(index);
        } else {
            if let Some(start) = ascii_start.take() {
                content.push(text[start..index].to_owned());
            }
            if !ch.is_ascii() && ch.is_alphanumeric() {
                content.push(ch.to_string());
            }
        }
    }
    if let Some(start) = ascii_start {
        content.push(text[start..].to_owned());
    }
    content
}

fn negation_targets_have_unambiguous_lcs(
    source: &[String],
    candidate: &[String],
    expected: &[Option<usize>],
) -> bool {
    #[derive(Clone, Copy)]
    struct State {
        len: usize,
        bad: bool,
    }
    fn choose(options: [Option<State>; 3]) -> State {
        let best = options
            .iter()
            .flatten()
            .map(|state| state.len)
            .max()
            .unwrap_or(0);
        State {
            len: best,
            bad: options
                .iter()
                .flatten()
                .any(|state| state.len == best && state.bad),
        }
    }
    let mut next = vec![State { len: 0, bad: false }; candidate.len() + 1];
    let mut row = vec![State { len: 0, bad: false }; candidate.len() + 1];
    for i in (0..source.len()).rev() {
        for j in (0..candidate.len()).rev() {
            let targeted = expected[i].is_some();
            let skip_source = State {
                len: next[j].len,
                bad: next[j].bad || targeted,
            };
            let skip_candidate = row[j + 1];
            let is_match = source[i] == candidate[j];
            let matched = is_match.then_some(State {
                len: next[j + 1].len + 1,
                bad: next[j + 1].bad || (targeted && expected[i] != Some(j)),
            });
            row[j] = choose([matched, Some(skip_source), Some(skip_candidate)]);
        }
        let targeted_at_edge = expected[i].is_some();
        row[candidate.len()] = State {
            len: next[candidate.len()].len,
            bad: next[candidate.len()].bad || targeted_at_edge,
        };
        std::mem::swap(&mut next, &mut row);
    }
    !next[0].bad
}

fn priority(kind: ProtectedSpanKind) -> u8 {
    match kind {
        ProtectedSpanKind::Url => 110,
        ProtectedSpanKind::Email => 105,
        ProtectedSpanKind::Path => 100,
        ProtectedSpanKind::Command => 95,
        ProtectedSpanKind::Flag => 90,
        ProtectedSpanKind::Date => 85,
        ProtectedSpanKind::Amount => 80,
        ProtectedSpanKind::Version => 75,
        ProtectedSpanKind::Number => 70,
        ProtectedSpanKind::DateWord => 60,
        ProtectedSpanKind::DeicticReference => 55,
        ProtectedSpanKind::Term => 50,
    }
}

fn push(out: &mut Vec<ProtectedSpan>, start_byte: usize, end_byte: usize, kind: ProtectedSpanKind) {
    if start_byte < end_byte {
        out.push(ProtectedSpan {
            start_byte,
            end_byte,
            kind,
        });
    }
}

fn collect_urls(text: &str, out: &mut Vec<ProtectedSpan>) {
    for scheme in ["https://", "http://", "www."] {
        for (start, _) in text.match_indices(scheme) {
            let mut end = text[start..]
                .find(|ch: char| {
                    ch.is_whitespace()
                        || matches!(
                            ch,
                            '，' | '。' | '！' | '？' | '；' | '：' | '<' | '>' | '"' | '\''
                        )
                })
                .map(|relative| start + relative)
                .unwrap_or(text.len());
            while end > start
                && text[..end].chars().next_back().is_some_and(|ch| {
                    matches!(ch, '.' | ',' | '!' | '?' | ';' | ':' | ')' | ']' | '}')
                })
            {
                end -= text[..end].chars().next_back().unwrap().len_utf8();
            }
            push(out, start, end, ProtectedSpanKind::Url);
        }
    }
    for (start, token) in ascii_tokens(text) {
        if looks_like_domain(token) && !token.contains('@') {
            push(out, start, start + token.len(), ProtectedSpanKind::Url);
        }
    }
}

fn collect_emails(text: &str, out: &mut Vec<ProtectedSpan>) {
    for (at, ch) in text.char_indices().filter(|(_, ch)| *ch == '@') {
        let local_start = text[..at]
            .char_indices()
            .rev()
            .take_while(|(_, ch)| {
                ch.is_ascii_alphanumeric() || ".!#$%&'*+/=?^_`{|}~-".contains(*ch)
            })
            .map(|(index, _)| index)
            .last()
            .unwrap_or(at);
        let local =
            text[local_start..at].trim_matches(|ch: char| ".!#$%&'*+/=?^_`{|}~-".contains(ch));
        let start = at - local.len();
        let domain_start = at + ch.len_utf8();
        let domain_len = text[domain_start..]
            .char_indices()
            .take_while(|(_, ch)| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-'))
            .map(|(_, ch)| ch.len_utf8())
            .sum::<usize>();
        let end = domain_start + domain_len;
        if !local.is_empty() && looks_like_domain(&text[domain_start..end]) {
            push(out, start, end, ProtectedSpanKind::Email);
        }
    }
}

fn collect_paths(text: &str, out: &mut Vec<ProtectedSpan>) {
    for (start, end) in ascii_path_tokens(text) {
        let token = &text[start..end];
        if token.starts_with('/')
            || token.starts_with("./")
            || token.starts_with("../")
            || token.starts_with("~/")
            || token.contains('\\')
            || (token.as_bytes().get(1) == Some(&b':') && token.contains('\\'))
            || (token.contains('/') && token.split('/').any(|part| part.contains('.')))
            || is_bare_dotfile(token)
        {
            push(out, start, end, ProtectedSpanKind::Path);
        }
    }
}

fn is_bare_dotfile(token: &str) -> bool {
    token.starts_with('.')
        && token
            .as_bytes()
            .get(1)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}

fn collect_flags(text: &str, out: &mut Vec<ProtectedSpan>) {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'-' || (index > 0 && bytes[index - 1] == b'-') {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        if bytes.get(index) == Some(&b'-') {
            index += 1;
        }
        let body_start = index;
        while index < bytes.len()
            && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'_' | b'-'))
        {
            index += 1;
        }
        if index > body_start
            && bytes[body_start].is_ascii_alphabetic()
            && !(index == body_start + 1 && bytes.get(body_start) == Some(&b'-'))
        {
            push(out, start, index, ProtectedSpanKind::Flag);
        }
    }
}

fn collect_commands(text: &str, out: &mut Vec<ProtectedSpan>) {
    const COMMANDS: &[&str] = &[
        "brew",
        "cargo",
        "cat",
        "cd",
        "cmake",
        "curl",
        "deno",
        "docker",
        "fd",
        "gh",
        "git",
        "go",
        "grep",
        "java",
        "javac",
        "kubectl",
        "ls",
        "make",
        "mkdir",
        "node",
        "npm",
        "npx",
        "osascript",
        "open",
        "pnpm",
        "python",
        "python3",
        "pytest",
        "rg",
        "rm",
        "ruff",
        "rustc",
        "sed",
        "security",
        "swift",
        "swiftc",
        "tsc",
        "uv",
        "yarn",
    ];
    for (start, token) in ascii_tokens(text) {
        if !COMMANDS
            .iter()
            .any(|command| command.eq_ignore_ascii_case(token))
        {
            continue;
        }
        let line_start = text[..start]
            .rfind(['\n', '\r'])
            .map(|index| index + 1)
            .unwrap_or(0);
        let line_end = text[start..]
            .find(|ch: char| {
                matches!(
                    ch,
                    '\n' | '\r' | '，' | '。' | '！' | '？' | '；' | ';' | ',' | '!' | '?'
                )
            })
            .map(|relative| start + relative)
            .unwrap_or(text.len());
        let prefix = text[line_start..start].trim_end();
        let explicit_command = [
            "run", "execute", "command", "type", "enter", "use", "运行", "执行", "敲", "输入",
            "调用",
        ]
        .iter()
        .any(|cue| {
            prefix
                .to_ascii_lowercase()
                .ends_with(&cue.to_ascii_lowercase())
        });
        let shell_prompt = prefix.is_empty() || prefix.ends_with('$') || prefix.ends_with('>');
        let remainder = text[start + token.len()..line_end].trim_start();
        let shell_shaped = remainder.starts_with('-')
            || remainder.starts_with('/')
            || remainder.starts_with("./")
            || remainder.starts_with("../")
            || remainder.starts_with("~/")
            || remainder.starts_with('`')
            || known_subcommand(token, remainder);
        // Ordinary prose can mention command-shaped words (especially `open`
        // and `go`). Require an explicit cue or a shell-like line/subcommand.
        if !explicit_command && !(shell_prompt && shell_shaped) {
            continue;
        }
        let mut end = line_end;
        for connector in ["然后", "之后", "再", " and then ", " then ", " and "] {
            if let Some(relative) = text[start + token.len()..end].find(connector) {
                end = end.min(start + token.len() + relative);
            }
        }
        while end > start
            && text[..end]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
        {
            end -= text[..end].chars().next_back().unwrap().len_utf8();
        }
        if end > start + token.len() || explicit_command {
            push(out, start, end, ProtectedSpanKind::Command);
        }
    }
}

fn known_subcommand(command: &str, remainder: &str) -> bool {
    let first = remainder
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(|ch: char| matches!(ch, '\'' | '"' | '`'));
    let command = command.to_ascii_lowercase();
    match command.as_str() {
        "git" => matches!(
            first,
            "add"
                | "branch"
                | "checkout"
                | "clone"
                | "commit"
                | "diff"
                | "fetch"
                | "log"
                | "merge"
                | "pull"
                | "push"
                | "rebase"
                | "reset"
                | "show"
                | "status"
                | "switch"
                | "tag"
        ),
        "cargo" => matches!(
            first,
            "build" | "check" | "clippy" | "fmt" | "run" | "test" | "update"
        ),
        "npm" | "pnpm" | "yarn" => matches!(first, "add" | "build" | "install" | "run" | "test"),
        "docker" => matches!(
            first,
            "build" | "compose" | "exec" | "images" | "ps" | "run"
        ),
        "kubectl" => matches!(
            first,
            "apply" | "config" | "delete" | "describe" | "get" | "logs" | "rollout"
        ),
        "open" => false,
        "go" => matches!(first, "build" | "install" | "run" | "test"),
        _ => !first.is_empty() && !matches!(first, "is" | "was" | "means" | "looks" | "sounds"),
    }
}

fn collect_dates(text: &str, out: &mut Vec<ProtectedSpan>) {
    const CN_DATES: &[&str] = &[
        "下下周一",
        "下下周二",
        "下下周三",
        "下下周四",
        "下下周五",
        "下下周六",
        "下下周日",
        "下下周天",
        "下周一",
        "下周二",
        "下周三",
        "下周四",
        "下周五",
        "下周六",
        "下周日",
        "下周天",
        "本周一",
        "本周二",
        "本周三",
        "本周四",
        "本周五",
        "本周六",
        "本周日",
        "本周天",
        "这周一",
        "这周二",
        "这周三",
        "这周四",
        "这周五",
        "这周六",
        "这周日",
        "这周天",
        "上周一",
        "上周二",
        "上周三",
        "上周四",
        "上周五",
        "上周六",
        "上周日",
        "上周天",
        "星期一",
        "星期二",
        "星期三",
        "星期四",
        "星期五",
        "星期六",
        "星期日",
        "星期天",
        "礼拜一",
        "礼拜二",
        "礼拜三",
        "礼拜四",
        "礼拜五",
        "礼拜六",
        "礼拜日",
        "礼拜天",
        "周一",
        "周二",
        "周三",
        "周四",
        "周五",
        "周六",
        "周日",
        "周天",
        "今天",
        "明天",
        "后天",
        "昨天",
        "本周",
        "下周",
        "上周",
    ];
    for phrase in CN_DATES {
        for (start, _) in text.match_indices(phrase) {
            push(
                out,
                start,
                start + phrase.len(),
                ProtectedSpanKind::DateWord,
            );
        }
    }
    for (start, token) in ascii_tokens(text) {
        if is_date_word(token) {
            push(out, start, start + token.len(), ProtectedSpanKind::DateWord);
        }
        if token.bytes().any(|byte| byte.is_ascii_digit())
            && token.chars().any(|ch| matches!(ch, '-' | '/'))
            && token
                .chars()
                .all(|ch| ch.is_ascii_digit() || matches!(ch, '-' | '/'))
            && token.split(['-', '/']).all(|part| !part.is_empty())
        {
            push(out, start, start + token.len(), ProtectedSpanKind::Date);
        }
    }
    for (start, end) in number_ranges(text) {
        let mut cursor = end;
        let mut saw_unit = false;
        loop {
            let spaces = text[cursor..]
                .chars()
                .take_while(|ch| ch.is_whitespace())
                .map(char::len_utf8)
                .sum::<usize>();
            let unit_start = cursor + spaces;
            let Some(unit) = text[unit_start..].chars().next() else {
                break;
            };
            if matches!(unit, '年' | '月' | '日' | '号' | '点' | '时' | '分' | '秒') {
                cursor = unit_start + unit.len_utf8();
                saw_unit = true;
            } else if unit.is_ascii_digit() && saw_unit {
                cursor = number_end(text, unit_start);
            } else {
                break;
            }
        }
        if saw_unit {
            push(out, start, cursor, ProtectedSpanKind::Date);
        }
    }
}

fn collect_amounts(text: &str, out: &mut Vec<ProtectedSpan>) {
    const CURRENCIES: &[&str] = &[
        "人民币",
        "美元",
        "欧元",
        "港币",
        "元",
        "dollars",
        "dollar",
        "euros",
        "euro",
        "yuan",
        "usd",
        "cny",
    ];
    for (start, end) in number_ranges(text) {
        if let Some((prefix, _)) = text[..start].char_indices().next_back() {
            let symbol = text[..start].chars().next_back().unwrap();
            if matches!(symbol, '$' | '€' | '£' | '¥' | '￥') {
                let signed_prefix = text[..prefix]
                    .char_indices()
                    .next_back()
                    .filter(|(_, ch)| matches!(ch, '+' | '-'))
                    .filter(|(sign, _)| {
                        *sign == 0
                            || !text[..*sign].chars().next_back().is_some_and(|ch| {
                                ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ',')
                            })
                    })
                    .map(|(sign, _)| sign)
                    .unwrap_or(prefix);
                push(out, signed_prefix, end, ProtectedSpanKind::Amount);
            }
        }
        let spaces = text[end..]
            .chars()
            .take_while(|ch| ch.is_whitespace())
            .map(char::len_utf8)
            .sum::<usize>();
        let unit_start = end + spaces;
        for currency in CURRENCIES {
            if text[unit_start..]
                .get(..currency.len())
                .is_some_and(|value| value.eq_ignore_ascii_case(currency))
            {
                let amount_end = unit_start + currency.len();
                if !text[amount_end..]
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_alphanumeric())
                {
                    push(out, start, amount_end, ProtectedSpanKind::Amount);
                    break;
                }
            }
        }
    }
}

fn collect_versions(text: &str, out: &mut Vec<ProtectedSpan>) {
    for (start, token) in ascii_tokens(text) {
        let digits = token
            .strip_prefix('v')
            .or_else(|| token.strip_prefix('V'))
            .unwrap_or(token);
        if digits.bytes().any(|byte| byte.is_ascii_digit())
            && digits.contains('.')
            && digits.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
            && digits.split('.').all(|part| !part.is_empty())
        {
            push(out, start, start + token.len(), ProtectedSpanKind::Version);
        }
    }
}

fn collect_numbers(text: &str, out: &mut Vec<ProtectedSpan>) {
    for (start, end) in number_ranges(text) {
        push(out, start, end, ProtectedSpanKind::Number);
    }
}

fn collect_terms(text: &str, out: &mut Vec<ProtectedSpan>) {
    let tokens = ascii_tokens(text);
    for (index, (start, token)) in tokens.iter().enumerate() {
        let (start, token) = (*start, *token);
        if is_supported_contraction_term_fragment(text, &tokens, index) {
            continue;
        }
        let letters: Vec<char> = token
            .chars()
            .filter(|ch| ch.is_ascii_alphabetic())
            .collect();
        let internal_upper = token.chars().skip(1).any(|ch| ch.is_ascii_uppercase());
        let all_caps = letters.len() >= 2 && letters.iter().all(|ch| ch.is_ascii_uppercase());
        let shaped = internal_upper
            || all_caps
            || token.chars().any(|ch| ch.is_ascii_digit())
            || token.contains('_')
            || token.contains("::")
            || (token.contains('.') && !looks_like_domain(token));
        if token.len() >= 2 && (shaped || is_name_candidate(token)) {
            push(out, start, start + token.len(), ProtectedSpanKind::Term);
        }
    }
}

fn is_supported_contraction_term_fragment(
    text: &str,
    tokens: &[(usize, &str)],
    index: usize,
) -> bool {
    let (start, token) = tokens[index];
    let end = start + token.len();
    if AUXILIARY_NEGATION_PAIRS.iter().any(|(contracted, _)| {
        let prefix = contracted.strip_suffix("'t").unwrap_or("");
        token.eq_ignore_ascii_case(prefix)
    }) {
        if let Some((next_start, next_token)) = tokens.get(index + 1) {
            let separator = &text[end..*next_start];
            if next_token.eq_ignore_ascii_case("t") && matches!(separator, "'" | "’" | "‘" | "ʼ")
            {
                return true;
            }
        }
    }
    false
}

fn ascii_tokens(text: &str) -> Vec<(usize, &str)> {
    let mut result = Vec::new();
    let mut start = None;
    for (index, ch) in text.char_indices() {
        let token_ch =
            ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-' | '+' | ':' | '#');
        match (start, token_ch) {
            (None, true) => start = Some(index),
            (Some(token_start), false) => {
                push_ascii_token(text, token_start, index, &mut result);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(start) = start {
        push_ascii_token(text, start, text.len(), &mut result);
    }
    result
}

fn push_ascii_token<'a>(
    text: &'a str,
    start: usize,
    end: usize,
    result: &mut Vec<(usize, &'a str)>,
) {
    let raw = &text[start..end];
    let token = raw.trim_matches(|ch: char| ".,:;!?#".contains(ch));
    if !token.is_empty() {
        result.push((start + raw.find(token).unwrap_or(0), token));
    }
}

fn ascii_path_tokens(text: &str) -> Vec<(usize, usize)> {
    let mut result = Vec::new();
    let mut start = None;
    for (index, ch) in text.char_indices() {
        let token_ch = ch.is_ascii_alphanumeric()
            || matches!(ch, '_' | '.' | '-' | '+' | ':' | '/' | '\\' | '~');
        match (start, token_ch) {
            (None, true) => start = Some(index),
            (Some(token_start), false) => {
                push_path_token(text, token_start, index, &mut result);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(start) = start {
        push_path_token(text, start, text.len(), &mut result);
    }
    result
}

fn push_path_token(text: &str, start: usize, end: usize, result: &mut Vec<(usize, usize)>) {
    let raw = &text[start..end];
    let bytes = raw.as_bytes();
    let mut token_start = 0;
    let mut token_end = bytes.len();

    while token_start < token_end && matches!(bytes[token_start], b',' | b':' | b';' | b'!' | b'?')
    {
        token_start += 1;
    }
    while token_end > token_start
        && matches!(bytes[token_end - 1], b',' | b':' | b';' | b'!' | b'?')
    {
        token_end -= 1;
    }

    // A dot run before a path separator is a real component (`./`, `../`,
    // `../../`, or a directory literally named `...`). A single dot before
    // a basename is also meaningful for a hidden file such as `.gitignore`.
    // Other leading dots are sentence punctuation and stay outside the span.
    let first_separator = raw[token_start..token_end]
        .find(['/', '\\'])
        .map(|relative| token_start + relative);
    let leading_dot_component = first_separator.is_some_and(|separator| {
        separator > token_start
            && bytes[token_start..separator]
                .iter()
                .all(|byte| *byte == b'.')
    });
    let leading_dotfile = bytes.get(token_start) == Some(&b'.')
        && bytes
            .get(token_start + 1)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_');
    if !leading_dot_component && !leading_dotfile {
        while token_start < token_end && bytes[token_start] == b'.' {
            token_start += 1;
        }
    }

    // Keep `.` and `..` when they are complete trailing path components. A
    // period after a filename (`../src/main.rs.`) remains normal punctuation.
    let last_separator = raw[token_start..token_end]
        .rfind(['/', '\\'])
        .map(|relative| token_start + relative);
    let trailing_dot_component = last_separator
        .is_some_and(|separator| matches!(&raw[separator + 1..token_end], "." | ".."));
    if !trailing_dot_component {
        while token_end > token_start && bytes[token_end - 1] == b'.' {
            token_end -= 1;
        }
    }

    if token_start < token_end {
        let span_start = start + token_start;
        result.push((span_start, start + token_end));
    }
}

fn number_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut result = Vec::new();
    let mut index = 0;
    while index < text.len() {
        let ch = text[index..].chars().next().unwrap();
        let (start, numeric_start) = if matches!(ch, '+' | '-')
            && (index == 0
                || !text[..index].chars().next_back().is_some_and(|previous| {
                    previous.is_ascii_alphanumeric() || matches!(previous, '_' | '.' | ',')
                })) {
            (index, index + ch.len_utf8())
        } else {
            (index, index)
        };
        let percent_prefix = text[numeric_start..].starts_with("百分之");
        let number_start = if percent_prefix {
            numeric_start + "百分之".len()
        } else {
            numeric_start
        };
        let Some(first) = text[number_start..].chars().next() else {
            index += ch.len_utf8();
            continue;
        };
        if first.is_ascii_digit() || is_chinese_number_digit(first) {
            let mut end = chinese_or_ascii_number_end(text, number_start);
            if text[end..].starts_with('%') {
                end += 1;
            }
            if !(text.get(start..end) == Some("一") && is_non_scalar_chinese_yi(text, start, end))
            {
                result.push((start, end));
            }
            index = end;
        } else {
            index += ch.len_utf8();
        }
    }
    result
}

// Finite local exceptions for shared-activity and request-complement uses;
// unknown contexts retain the scalar number role.
fn is_non_scalar_chinese_yi(text: &str, start: usize, end: usize) -> bool {
    let before = &text[..start];
    let after = &text[end..];
    if after.starts_with('起') {
        let activities = [
            "开会", "讨论", "商量", "聊天", "学习", "跑步", "吃饭", "出发", "工作", "研究", "检查",
            "测试",
        ];
        let shared_subject = ["我们", "你们", "他们", "她们", "它们", "咱们", "大家"]
            .iter()
            .any(|subject| before.ends_with(subject));
        let activity_tail = &after['起'.len_utf8()..];
        if shared_subject
            && activities.iter().any(|activity| {
                activity_tail
                    .strip_prefix(activity)
                    .is_some_and(|tail| next_non_whitespace(tail).is_none_or(is_clause_boundary))
            })
        {
            return true;
        }
    }

    if !after.starts_with('下') || !before.ends_with('看') {
        return false;
    }
    let prefix = &before[..before.len() - '看'.len_utf8()];
    if prefix
        .trim_end()
        .chars()
        .next_back()
        .is_some_and(|ch| !is_clause_boundary(ch))
    {
        return false;
    }
    let rest = &after['下'.len_utf8()..];
    let complements = [
        "这个",
        "那个",
        "这些",
        "那些",
        "这里",
        "那里",
        "什么",
        "怎么",
        "如何",
        "是否",
        "为什么",
        "今天",
        "明天",
        "昨天",
        "现在",
        "目前",
        "当前",
        "具体",
        "详细",
    ];
    complements.iter().any(|complement| {
        rest.strip_prefix(complement)
            .is_some_and(has_same_clause_content)
    })
}

fn is_clause_boundary(ch: char) -> bool {
    matches!(ch, '，' | ',' | '。' | '；' | ';' | '？' | '?' | '！' | '!')
}

fn next_non_whitespace(text: &str) -> Option<char> {
    text.chars().find(|ch| !ch.is_whitespace())
}

fn has_same_clause_content(text: &str) -> bool {
    next_non_whitespace(text).is_some_and(|ch| !is_clause_boundary(ch))
}

fn is_chinese_number_digit(ch: char) -> bool {
    matches!(
        ch,
        '零' | '〇'
            | '一'
            | '二'
            | '两'
            | '三'
            | '四'
            | '五'
            | '六'
            | '七'
            | '八'
            | '九'
            | '十'
            | '百'
            | '千'
            | '万'
    )
}

fn chinese_or_ascii_number_end(text: &str, start: usize) -> usize {
    if text[start..]
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_digit())
    {
        return number_end(text, start);
    }
    let mut end = start;
    while end < text.len() {
        let ch = text[end..].chars().next().unwrap();
        if is_chinese_number_digit(ch) {
            end += ch.len_utf8();
        } else {
            break;
        }
    }
    end
}

fn matches_numeric_value(source: &str, candidate: &str, kind: ProtectedSpanKind) -> bool {
    if !matches!(
        kind,
        ProtectedSpanKind::Amount | ProtectedSpanKind::Date | ProtectedSpanKind::Number
    ) {
        return false;
    }
    match (numeric_span_value(source), numeric_span_value(candidate)) {
        (Some(source), Some(candidate)) => source == candidate,
        _ => false,
    }
}

fn numeric_span_value(token: &str) -> Option<(bool, bool, i128, i128, String)> {
    let (sign, token) = if let Some(body) = token.strip_prefix('-') {
        ("-", body)
    } else if let Some(body) = token.strip_prefix('+') {
        ("+", body)
    } else {
        ("", token)
    };
    let (percent_prefix, body) = token
        .strip_prefix("百分之")
        .map_or((false, token), |body| (true, body));
    let percent_suffix = body.ends_with('%');
    let body = body.strip_suffix('%').unwrap_or(body);
    let chars: Vec<char> = body.chars().collect();
    let number_end = chars
        .iter()
        .take_while(|ch| {
            is_chinese_number_digit(**ch) || ch.is_ascii_digit() || matches!(**ch, '.' | ',')
        })
        .count();
    if number_end == 0 {
        return None;
    }
    let number: String = chars[..number_end].iter().collect();
    let suffix: String = chars[number_end..].iter().collect();
    let value = parse_numeric_value(&number)?;
    let (numerator, denominator) = reduce_fraction(value.0, value.1)?;
    Some((
        percent_prefix || percent_suffix,
        false,
        numerator,
        denominator,
        format!("{sign}{suffix}"),
    ))
}

fn reduce_fraction(mut numerator: i128, mut denominator: i128) -> Option<(i128, i128)> {
    if denominator <= 0 {
        return None;
    }
    let mut left = numerator.checked_abs()?;
    let mut right = denominator;
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    if left != 0 {
        numerator /= left;
        denominator /= left;
    }
    Some((numerator, denominator))
}

fn parse_numeric_value(value: &str) -> Option<(i128, i128)> {
    if value
        .chars()
        .all(|ch| ch.is_ascii_digit() || matches!(ch, '.' | ','))
    {
        let normalized = value.replace(',', "");
        let (whole, fraction) = normalized.split_once('.').unwrap_or((&normalized, ""));
        if fraction.contains('.') {
            return None;
        }
        let denominator = 10_i128.checked_pow(u32::try_from(fraction.len()).ok()?)?;
        let whole = whole.parse::<i128>().ok()?;
        let fraction = if fraction.is_empty() {
            0
        } else {
            fraction.parse::<i128>().ok()?
        };
        return Some((
            whole.checked_mul(denominator)?.checked_add(fraction)?,
            denominator,
        ));
    }
    parse_chinese_integer(value).map(|number| (number, 1))
}

fn parse_chinese_integer(value: &str) -> Option<i128> {
    let digit_value = |ch| match ch {
        '零' | '〇' => Some(0),
        '一' => Some(1),
        '二' | '两' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
        _ => None,
    };
    if !value.is_empty() && value.chars().all(|ch| digit_value(ch).is_some()) {
        return value.chars().try_fold(0_i128, |number, ch| {
            number.checked_mul(10)?.checked_add(digit_value(ch)?)
        });
    }
    let mut total = 0_i128;
    let mut section = 0_i128;
    let mut digit = 0_i128;
    let mut last_unit = 100_000_i128;
    let mut pending_digit = false;
    for ch in value.chars() {
        match ch {
            '零' | '〇' => set_chinese_digit(&mut digit, &mut pending_digit, 0)?,
            '一' => set_chinese_digit(&mut digit, &mut pending_digit, 1)?,
            '二' | '两' => set_chinese_digit(&mut digit, &mut pending_digit, 2)?,
            '三' => set_chinese_digit(&mut digit, &mut pending_digit, 3)?,
            '四' => set_chinese_digit(&mut digit, &mut pending_digit, 4)?,
            '五' => set_chinese_digit(&mut digit, &mut pending_digit, 5)?,
            '六' => set_chinese_digit(&mut digit, &mut pending_digit, 6)?,
            '七' => set_chinese_digit(&mut digit, &mut pending_digit, 7)?,
            '八' => set_chinese_digit(&mut digit, &mut pending_digit, 8)?,
            '九' => set_chinese_digit(&mut digit, &mut pending_digit, 9)?,
            '十' | '百' | '千' => {
                let unit = match ch {
                    '十' => 10,
                    '百' => 100,
                    _ => 1000,
                };
                if unit >= last_unit || (pending_digit && digit == 0) {
                    return None;
                }
                section = section
                    .checked_add((if pending_digit { digit } else { 1 }).checked_mul(unit)?)?;
                digit = 0;
                pending_digit = false;
                last_unit = unit;
            }
            '万' => {
                if 10_000 >= last_unit
                    || (section == 0 && !pending_digit && total == 0)
                    || (pending_digit && digit == 0)
                {
                    return None;
                }
                total = total.checked_add(section.checked_add(digit)?.checked_mul(10_000)?)?;
                section = 0;
                digit = 0;
                pending_digit = false;
                last_unit = 10_000;
            }
            _ => return None,
        }
    }
    total.checked_add(section)?.checked_add(digit)
}

fn set_chinese_digit(digit: &mut i128, pending_digit: &mut bool, next: i128) -> Option<()> {
    if *pending_digit && *digit != 0 {
        return None;
    }
    *digit = next;
    *pending_digit = true;
    Some(())
}

fn number_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    while end + 1 < bytes.len()
        && matches!(bytes[end], b'.' | b',')
        && bytes[end + 1].is_ascii_digit()
    {
        end += 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
    }
    end
}

fn is_date_word(word: &str) -> bool {
    [
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ]
    .iter()
    .any(|value| value.eq_ignore_ascii_case(word))
}

fn is_name_candidate(word: &str) -> bool {
    if !(2..=24).contains(&word.len())
        || !word
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase())
        || !word.chars().skip(1).all(|ch| ch.is_ascii_lowercase())
    {
        return false;
    }
    ![
        "A", "An", "And", "At", "But", "Email", "For", "From", "Hello", "I", "In", "Is", "It",
        "Maybe", "My", "Of", "On", "Or", "Please", "Select", "Tell", "The", "Then", "This", "That",
        "To", "We", "You", "Your",
    ]
    .contains(&word)
}

fn looks_like_domain(token: &str) -> bool {
    let labels: Vec<&str> = token.split('.').collect();
    let tld = labels.last().copied().unwrap_or_default();
    labels.len() >= 2
        && tld.len() >= 2
        && tld.chars().all(|ch| ch.is_ascii_alphabetic())
        && labels.iter().all(|label| {
            !label.is_empty()
                && label
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        })
}

fn is_cjk_character(ch: char) -> bool {
    matches!(
        ch,
        '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_keeps_spans_at_sentence_boundaries() {
        for text in [
            "Use v1.2.",
            "Open https://example.com.",
            "The file is /tmp/a.",
            "It costs 1250美元.",
            "Set the value to 100%.",
        ] {
            assert!(preserves(text, text, None), "identity rejected: {text}");
        }
    }

    #[test]
    fn cleanup_accepts_bounded_chinese_and_arabic_numeric_equivalents() {
        for (source, candidate) in [
            ("预算是两百五十美元。", "预算是250美元。"),
            ("增长百分之二十。", "增长20%。"),
            ("增长百分之20。", "增长20%。"),
            ("向前三点。", "向前3点。"),
            ("预算是30元。", "预算是三十元。"),
            ("预算是三十元。", "预算是30.0元。"),
            ("预算是30.00元。", "预算是30元。"),
        ] {
            assert!(
                preserves(source, candidate, Some(CleanupOperation::Cleanup)),
                "equivalent value rejected: {source} -> {candidate}"
            );
        }

        for (contracted, expanded) in AUXILIARY_NEGATION_PAIRS {
            let source = format!("we {contracted} approve");
            let candidate = format!("we {expanded} approve");
            assert!(
                preserves(&source, &candidate, Some(CleanupOperation::Rewrite)),
                "finite auxiliary expansion must preserve its own marker: {source} -> {candidate}"
            );
            assert!(preserves(&candidate, &source, Some(CleanupOperation::Rewrite)),
                "finite auxiliary contraction must preserve its own marker: {candidate} -> {source}");
            let wrong_aux = "we do not approve".to_string();
            if *contracted != "don't" {
                assert!(
                    !preserves(&source, &wrong_aux, Some(CleanupOperation::Rewrite)),
                    "a different auxiliary must remain rejected: {source} -> {wrong_aux}"
                );
            }
        }
    }

    #[test]
    fn hyphenated_not_after_long_gap_expands_only_the_first_not() {
        let text = "do   not-not-not";
        assert_eq!(
            negation_expression_ranges(text),
            vec![(0, 8), (9, 12), (13, 16)]
        );
        assert_eq!(
            negation_bindings(text)
                .iter()
                .map(|(marker, _)| marker.as_str())
                .collect::<Vec<_>>(),
            vec!["don't", "not", "not"]
        );
        assert!(preserves_negation(text, text, None));
    }

    #[test]
    fn cleanup_rejects_changed_numeric_values_and_keeps_code_exact() {
        for (source, candidate) in [
            ("预算是30元。", "预算是二十九元。"),
            ("比例是百分之二十。", "比例是0.2%。"),
            ("净变化是-30元。", "净变化是+30元。"),
            ("费用是30美元。", "费用是30欧元。"),
            ("费用是$30。", "费用是€30。"),
            ("保留3天。", "保留3月。"),
            ("确认30元，再次确认30元。", "确认30元，再次确认31元。"),
            ("读取 `count=30`。", "读取 `count=31`。"),
            ("年份是二〇二六年。", "年份是6年。"),
            ("净变化是-三十元。", "净变化是+三十元。"),
            ("比例是二十%。", "比例是二十。"),
        ] {
            assert!(
                !preserves(source, candidate, Some(CleanupOperation::Cleanup)),
                "changed numeric value accepted: {source} -> {candidate}"
            );
        }
    }

    #[test]
    fn cleanup_percent_prefix_at_end_is_safe_under_partial_scope() {
        assert!(preserves(
            "增长百分之",
            "增长百分之",
            Some(CleanupOperation::Cleanup)
        ));
        // Candidate-only numeric additions are outside the approved G4 scope.
        assert!(preserves(
            "增长百分之",
            "增长百分之20。",
            Some(CleanupOperation::Cleanup)
        ));
    }

    #[test]
    fn cleanup_preserves_inline_and_fenced_code_literals_exactly() {
        let source = "第二个是`才是稳定性`。";
        let removed_inline_content = "第二个是`稳定性`。";
        assert!(!preserves(
            source,
            removed_inline_content,
            Some(CleanupOperation::Cleanup)
        ));
        assert_eq!(
            crate::llm::guard_final_output(
                source,
                removed_inline_content,
                Some(CleanupOperation::Cleanup)
            ),
            source
        );

        let fenced = "```text\n才是稳定性\n```";
        let dropped_fenced_content = "```text\n稳定性\n```";
        assert!(!preserves(
            fenced,
            dropped_fenced_content,
            Some(CleanupOperation::Cleanup)
        ));
        assert!(preserves(fenced, fenced, Some(CleanupOperation::Cleanup)));
    }

    #[test]
    fn cleanup_allows_markdown_wrappers_to_be_removed_when_payload_bytes_remain() {
        let source = "Please explain why `lookup_value(user_id)` returns `None` for an empty cache, and preserve `--dry-run` exactly.";
        let candidate = "Please explain why lookup_value(user_id) returns None for an empty cache, and preserve --dry-run exactly.";
        assert!(preserves(
            source,
            candidate,
            Some(CleanupOperation::Cleanup)
        ));

        let fenced = "```text\n  let value = 1;\n```";
        let unwrapped = "  let value = 1;\n";
        assert!(preserves(
            fenced,
            unwrapped,
            Some(CleanupOperation::Cleanup)
        ));
    }

    #[test]
    fn cleanup_rejects_code_payload_changes_even_when_wrappers_are_removed() {
        let source =
            "Please explain why `lookup_value(user_id)` returns `None`, and preserve `--dry-run`.";
        for candidate in [
            "Please explain why lookup_value(account_id) returns None, and preserve --dry-run.",
            "Please explain why lookup_value(user_id) returns Some, and preserve --dry-run.",
            "Please explain why lookup_value(user_id) returns None, and preserve --run.",
        ] {
            assert!(
                !preserves(source, candidate, Some(CleanupOperation::Cleanup)),
                "changed code payload was accepted: {candidate}"
            );
        }

        let source = "第二个是`才是稳定性`。";
        assert!(!preserves(
            source,
            "第二个是稳定性。",
            Some(CleanupOperation::Cleanup)
        ));
    }

    #[test]
    fn cleanup_keeps_code_payload_occurrences_order_and_indentation() {
        assert!(!preserves(
            "Say foo, then `foo`.",
            "Say foo, then.",
            Some(CleanupOperation::Cleanup)
        ));
        assert!(!preserves(
            "Run `alpha` then `beta`.",
            "Run beta then alpha.",
            Some(CleanupOperation::Cleanup)
        ));
        assert!(!preserves(
            "```text\n  let value = 1;\n```",
            "```text\n let value = 1;\n```",
            Some(CleanupOperation::Cleanup)
        ));
    }

    #[test]
    fn unwrapped_code_payloads_require_complete_token_and_expression_boundaries() {
        for (source, candidate) in [
            ("Print `alpha`.", "Print alphabet."),
            ("Print `alpha`.", "Print xalpha."),
            ("Print `alpha`.", "Print alpha_2."),
            ("Print `alpha`.", "Print alpha-beta."),
            ("Print `alpha`.", "Print alpha.member."),
            ("Print `alpha`.", "Print `alpha`.member."),
            ("Print `alpha`.", "Print alpha + beta."),
            ("Print `alpha`.", "Print alpha .member."),
            ("Print `alpha`.", "Print alpha\n.member."),
            ("Print `alpha`.", "Print alpha\t.member."),
            ("Print `alpha`.", "Print alpha (1)."),
            ("Print `alpha`.", "Print alpha [0]."),
            ("Run `--dry-run`.", "Run --dry-run=all."),
            ("Run `--dry-run`.", "Run --dry-run-extra."),
            ("Run `--dry-run`.", "Run --dry-run/child."),
            ("Run `--dry-run`.", "Run --dry-run = all."),
            ("Run `--dry-run`.", "Run `--dry-run`=all."),
            (
                "Call `lookup_value(user_id)`. ",
                "Call lookup_value(user_id).len(). ",
            ),
            (
                "Call `lookup_value(user_id)`. ",
                "Call lookup_value(user_id)(). ",
            ),
            (
                "Call `lookup_value(user_id)`. ",
                "Call lookup_value(user_id) (). ",
            ),
            (
                "Open `/var/tmp/result.json`.",
                "Open /var/tmp/result.json.backup.",
            ),
            (
                "Open `/var/tmp/result.json`.",
                "Open /var/tmp/result.json/child.",
            ),
        ] {
            assert!(
                !super::preserves_code_literals_exactly(source, candidate),
                "changed code unit was accepted: {source} -> {candidate}"
            );
        }

        for (source, candidate) in [
            ("Print `alpha`.", "Print alpha."),
            ("Print:`alpha`.", "Print:`alpha`."),
            ("Print:`alpha`.", "Print:alpha."),
            ("The value is \"`alpha`\".", "The value is \"`alpha`\"."),
            ("The value is \"`alpha`\".", "The value is \"alpha\"."),
            ("Use `alpha`+1.", "Use `alpha`+1."),
            ("Use `alpha`+1.", "Use alpha + 1."),
            ("Use `alpha`+1.", "Use alpha+1."),
            (
                "```text\n  alpha\n```\nPlease, continue.",
                "  alpha\nPlease continue.",
            ),
            (
                "```text\r\n  alpha\r\n```\r\nPlease, continue.",
                "```text\r\n  alpha\r\n```\r\nPlease continue.",
            ),
            (
                "```text\n漢字 literal\n```\nUm, I am ready.",
                "```text\n漢字 literal\n```\nI am ready.",
            ),
            (
                "```text\r\n漢字 literal\r\n```\r\nUm, I am ready.",
                "```text\r\n漢字 literal\r\n```\r\nI am ready.",
            ),
            ("Run `--dry-run`.", "Run --dry-run."),
            (
                "Run `--dry-run`, then continue.",
                "Run --dry-run, then continue.",
            ),
            ("请运行`alpha`后继续。", "请运行alpha后继续。"),
            ("Say foo, then `foo`.", "Say then foo."),
            ("Use `a` `aa`.", "Use a aa."),
            ("`foo` `foo`", "foo foo"),
        ] {
            assert!(
                super::preserves_code_literals_exactly(source, candidate),
                "source-bounded unchanged payload was rejected: {source} -> {candidate}"
            );
        }
        assert!(!super::preserves_code_literals_exactly(
            "请运行`alpha`后继续。",
            "请运行alphabet后继续。"
        ));
        assert!(!super::preserves_code_literals_exactly(
            "Print `alpha`.",
            "请运行alpha后继续。"
        ));
        assert!(!super::preserves_code_literals_exactly(
            "Say foo, then `foo`.",
            "Say foo, then."
        ));
        assert!(!super::preserves_code_literals_exactly(
            "Use `a` `aa`.",
            "Use aa."
        ));
        assert!(!super::preserves_code_literals_exactly(
            "`foo` `foo`",
            "foo"
        ));
        assert!(super::preserves_code_literals_exactly(
            "Use `alpha`.",
            "Use `alpha`."
        ));
    }

    #[test]
    fn inherited_balanced_nested_code_groups_survive_whitespace_normalization() {
        let source = "Call `alpha`(outer(inner[0])) and `beta`{key: 'x'}";
        let candidate = "Call alpha ( outer ( inner [ 0 ] ) ) and beta { key: 'x' }";
        assert!(super::preserves_code_literals_exactly(source, candidate));

        let quoted_group = r#"(outer(inner[0], {value: "close ) ] "}))"#;
        let (normalized, end) = normalized_group_suffix(quoted_group, 0, '(')
            .expect("balanced nested group with delimiter characters inside a string");
        assert_eq!(normalized, r#"(outer(inner[0],{value:"close ) ] "}))"#);
        assert_eq!(end, quoted_group.len());

        for (group, opening, expected) in [
            (
                "[first, {second: (third)}]",
                '[',
                "[first,{second:(third)}]",
            ),
            (
                "{first: [second, (third)]}",
                '{',
                "{first:[second,(third)]}",
            ),
        ] {
            let (normalized, end) =
                normalized_group_suffix(group, 0, opening).expect("balanced mixed delimiter group");
            assert_eq!(normalized, expected);
            assert_eq!(end, group.len());
        }
    }

    #[test]
    fn new_malformed_and_over_budget_code_groups_fail_closed() {
        for (group, opening) in [
            ("(outer[inner))", '('),
            ("([)]", '('),
            ("{value: [0}", '{'),
            ("(unfinished", '('),
            ("(value)", '['),
        ] {
            assert!(
                normalized_group_suffix(group, 0, opening).is_none(),
                "malformed group was accepted: {group}"
            );
        }

        let at_limit = format!("({})", "x".repeat(MAX_LOCAL_CODE_EDGE_CHARS - 2));
        let over_limit = format!("({})", "x".repeat(MAX_LOCAL_CODE_EDGE_CHARS - 1));
        assert_eq!(
            normalized_group_suffix(&at_limit, 0, '(')
                .expect("a complete group at the scan limit")
                .1,
            at_limit.len()
        );
        assert!(normalized_group_suffix(&over_limit, 0, '(').is_none());

        assert!(!super::preserves_code_literals_exactly(
            "Call `alpha`.",
            "Call alpha (outer[inner))."
        ));
        assert!(!super::preserves_code_literals_exactly(
            "Call `alpha`.",
            &format!("Call alpha({}).", "x".repeat(MAX_LOCAL_CODE_EDGE_CHARS - 1))
        ));

        let inherited_over_limit = format!(
            "Call `alpha`({}).",
            "x".repeat(MAX_LOCAL_CODE_EDGE_CHARS - 1)
        );
        assert!(super::preserves_code_literals_exactly(
            &inherited_over_limit,
            &inherited_over_limit
        ));
        assert!(!super::preserves_code_literals_exactly(
            "Call `alpha`.",
            "Call alpha(outer(inner[0]))."
        ));
    }

    #[test]
    fn unicode_code_boundaries_use_the_cached_xid_start_and_continue_tables() {
        for suffix in [
            "β",
            "é",
            "\u{301}",
            "\u{20dd}",
            "\u{203f}tail",
            "\u{2040}tail",
            "\u{2054}tail",
            "\u{fe33}tail",
            "\u{fe4d}tail",
            "\u{ff3f}tail",
        ] {
            let candidate = format!("Print alpha{suffix}.");
            assert!(
                !super::preserves_code_literals_exactly("Print `alpha`.", &candidate),
                "identifier continuation attached after body: {candidate}"
            );
        }
        for prefix in ["β", "é", "℘", "\u{203f}", "\u{ff3f}"] {
            let candidate = format!("Print {prefix}alpha.");
            assert!(
                !super::preserves_code_literals_exactly("Print `alpha`.", &candidate),
                "identifier prefix attached before body: {candidate}"
            );
        }

        assert!(super::preserves_code_literals_exactly(
            "请运行`alpha`后继续。",
            "请运行alpha后继续。"
        ));
        assert!(super::is_code_identifier_start('℘', false));
        assert!(super::is_code_identifier_continue('\u{203f}', false));
        assert!(super::is_code_identifier_continue('\u{301}', false));
        // Identifier continuation and grapheme attachment are distinct: the
        // enclosing circle is a Mark but not XID_Continue.
        assert!(!super::is_unicode_xid_continue('\u{20dd}'));
        assert!(super::is_unicode_mark('\u{20dd}'));
        assert!(super::is_code_identifier_continue('\u{20dd}', false));
        assert!(!super::is_code_identifier_start('\u{203f}', false));

        for candidate in [
            "Print \u{20dd}alpha.",
            "Print `alpha\u{20dd}`.",
            "```text\nalpha\u{20dd}\n```",
            "```text\nalpha\u{20dd}\n```\nContinue.",
        ] {
            let source = if candidate.starts_with("```") {
                "```text\nalpha\n```"
            } else {
                "Print `alpha`."
            };
            assert!(
                !super::preserves_code_literals_exactly(source, candidate),
                "new combining/enclosing mark changed the code unit: {candidate:?}"
            );
        }
        assert!(super::preserves_code_literals_exactly(
            "Print `alpha\u{301}`.",
            "Print alpha\u{301}."
        ));
    }

    #[test]
    fn whitespace_inside_left_edge_syntax_does_not_hide_new_attachments() {
        for candidate in [
            "Print obj . alpha.",
            "Print obj\t.\talpha.",
            "Print obj\n.\nalpha.",
            "Print obj:: alpha.",
            "Print obj -> alpha.",
            "Print obj / alpha.",
            "Print obj [ alpha.",
            "Print obj ( alpha.",
            "Print obj / `alpha`.",
        ] {
            assert!(
                !super::preserves_code_literals_exactly("Print `alpha`.", candidate),
                "new code continuation was accepted: {candidate:?}"
            );
        }
        assert!(super::preserves_code_literals_exactly(
            "Print obj . `alpha`(outer(inner[0]))",
            "Print obj . alpha ( outer ( inner [ 0 ] ) )"
        ));
    }

    #[test]
    fn binary_sign_reclassification_is_limited_to_a_validated_inline_code_edge() {
        for candidate in [
            "Use alpha+1.",
            "Use alpha +1.",
            "Use alpha+ 1.",
            "Use alpha + 1.",
        ] {
            assert!(
                super::preserves("Use `alpha`+1.", candidate, Some(CleanupOperation::Cleanup),),
                "inherited binary +1 should survive spacing change: {candidate:?}"
            );
        }
        assert!(super::preserves(
            "Use `alpha`+1.",
            "Use `alpha`+1.",
            Some(CleanupOperation::Cleanup),
        ));
        assert!(!super::preserves(
            "Use `alpha`+1.",
            "Use alpha - 1.",
            Some(CleanupOperation::Cleanup),
        ));
        assert!(!super::preserves(
            "The value is +1.",
            "The value is 1.",
            Some(CleanupOperation::Cleanup),
        ));
        assert!(!super::preserves(
            "The value is -1.",
            "The value is 1.",
            Some(CleanupOperation::Cleanup),
        ));
        assert!(!super::preserves(
            "The cost is -$12.",
            "The cost is $12.",
            Some(CleanupOperation::Cleanup),
        ));
    }

    #[test]
    fn code_wrapper_removal_uses_complete_payloads_through_adapter_and_finalizer() {
        use crate::context::{ContextFamily, FocusKind};

        let settings = crate::store::Settings::default();
        let prepare = |source: &str| {
            crate::prepare_cleanup_transcript_for_scene(
                None,
                &settings,
                source,
                ContextFamily::PromptOrCode,
                1.0,
                FocusKind::CodingPrompt,
                None,
                false,
            )
        };
        let finalize = |prepared: &crate::PreparedCleanupTranscript, candidate: &str| {
            crate::finalize_text_for_scene(
                &prepared.text,
                crate::CleanupDecision::Provider(candidate.to_owned()),
                crate::FinalizationContext {
                    scene: prepared.scene,
                    operation: crate::llm::CleanupOperation::Cleanup,
                    revision_source: Some(&prepared.revision_source),
                    prepared_transcript: Some(&prepared.text),
                    revision_authorizations: &prepared.revision_authorizations,
                    promoted_pair_protections: &prepared.promoted_pair_protections,
                    list_revision_proof: prepared.list_revision_proof.as_ref(),
                    structure_safe_source: None,
                    output_projection_list_revision_proof: None,
                    layout_source: None,
                    exact_fallback: false,
                },
            )
            .expect("nonempty deterministic final result")
        };

        for (source, candidate) in [
            ("Run `--dry-run`.", "Run --dry-run."),
            ("Print `alpha`.", "Print alpha."),
            ("Print:`alpha`.", "Print:`alpha`."),
            ("Print:`alpha`.", "Print:alpha."),
            ("The value is \"`alpha`\".", "The value is \"`alpha`\"."),
            ("The value is \"`alpha`\".", "The value is \"alpha\"."),
            ("Use `alpha`+1.", "Use `alpha`+1."),
            ("Use `alpha`+1.", "Use alpha + 1."),
            (
                "Run `--dry-run`, then continue.",
                "Run --dry-run, then continue.",
            ),
            ("请运行`alpha`后继续。", "请运行alpha后继续。"),
            (
                "嗯，please review `alpha` before shipping.",
                "Please review alpha before shipping.",
            ),
            (
                "Please, review `alpha` before shipping.",
                "Please review alpha before shipping.",
            ),
            (
                "Please review `alpha`, then continue.",
                "Please review alpha; then continue.",
            ),
            ("React:`alpha`.", "React:`alpha`."),
            ("Say foo, then `foo`.", "Say then foo."),
            ("Use `a` `aa`.", "Use a aa."),
            ("`foo` `foo`", "foo foo"),
        ] {
            let prepared = prepare(source);
            assert_eq!(prepared.text, source);
            let accepted =
                crate::llm::accept_cleanup_reply_with_facts(candidate.to_owned(), &prepared.intent)
                    .expect("adapter accepts unchanged complete code payloads");
            assert_eq!(accepted.accepted_text, candidate);
            let final_text = finalize(&prepared, &accepted.accepted_text);
            assert_eq!(final_text.text, candidate);
            assert!(!final_text.degraded);
        }

        for (source, candidate) in [
            ("Run `--dry-run`.", "Run --dry-run=all."),
            ("Run `--dry-run`.", "Run --dry-run-extra."),
            ("Run `--dry-run`.", "Run --dry-run = all."),
            ("Run `--dry-run`.", "Run `--dry-run`=all."),
            ("Print `alpha`.", "Print alphabet."),
            ("Print `alpha`.", "Print xalpha."),
            ("Print `alpha`.", "Print alpha_2."),
            ("Print `alpha`.", "Print alpha\u{20dd}."),
            ("Print `alpha`.", "Print alpha + beta."),
            ("Print `alpha`.", "Print alpha .member."),
            ("Print `alpha`.", "Print alpha\n.member."),
            ("Print `alpha`.", "Print alpha\t.member."),
            ("Print `alpha`.", "Print alpha (1)."),
            ("Print `alpha`.", "Print alpha [0]."),
            ("Print `alpha`.", "Print `alpha`.member."),
            (
                "Call `lookup_value(user_id)`.",
                "Call lookup_value(user_id).len().",
            ),
            (
                "Call `lookup_value(user_id)`.",
                "Call lookup_value(user_id)().",
            ),
            (
                "Call `lookup_value(user_id)`.",
                "Call lookup_value(user_id) ().",
            ),
            (
                "Open `/var/tmp/result.json`.",
                "Open /var/tmp/result.json.backup.",
            ),
            ("第二个是`才是稳定性`。", "第二个是稳定性。"),
            ("React:`alpha`.", "Preact:alpha."),
            ("Keep `React`.", "Keep Preact."),
            ("The cost is `$1250`.", "The cost is $12500."),
            ("Say foo, then `foo`.", "Say foo, then."),
            ("Use `a` `aa`.", "Use aa."),
            ("`foo` `foo`", "foo"),
            ("Run `alpha` then `beta`.", "Run beta then alpha."),
        ] {
            let prepared = prepare(source);
            assert_eq!(prepared.text, source);
            assert!(matches!(
                crate::llm::accept_cleanup_reply_with_facts(
                    candidate.to_owned(),
                    &prepared.intent
                ),
                Err(crate::llm::LlmError::CleanupRejected(fact))
                    if fact.reason == crate::llm::GuardReason::ProtectedToken
                        && fact.stage == crate::llm::CleanupRejectionStage::ProviderOutput
            ));
            let final_text = finalize(&prepared, candidate);
            assert!(final_text.degraded, "changed code unit passed: {source}");
            assert_eq!(final_text.text, prepared.text);
        }
    }

    #[test]
    fn literal_boundary_projection_keeps_original_waiver_byte_offsets() {
        let source = "```text\n漢字 literal\n```\nUm, React stays.";
        let candidate = "```text\n漢字 literal\n```\nReact stays.";
        let filler_start = source.find("Um").expect("source contains the filler");
        let exact_waiver = filler_start..filler_start + "Um".len();
        assert_eq!(source.get(exact_waiver.clone()), Some("Um"));
        assert!(preserves_with_source_term_waivers(
            source,
            candidate,
            Some(CleanupOperation::Cleanup),
            std::slice::from_ref(&exact_waiver),
        ));

        let shifted_waiver = (exact_waiver.start + 1)..(exact_waiver.end + 1);
        assert_ne!(source.get(shifted_waiver.clone()), Some("Um"));
        assert!(!preserves_with_source_term_waivers(
            source,
            candidate,
            Some(CleanupOperation::Cleanup),
            &[shifted_waiver],
        ));

        let unrelated_start = source.find("React").expect("source contains React");
        let unrelated_waiver = unrelated_start..unrelated_start + "React".len();
        assert_eq!(source.get(unrelated_waiver.clone()), Some("React"));
        assert!(!preserves_with_source_term_waivers(
            source,
            candidate,
            Some(CleanupOperation::Cleanup),
            &[unrelated_waiver],
        ));

        let supported_source = "```text\n漢字 literal\n```\nUm, I am ready.";
        let supported_candidate = "```text\n漢字 literal\n```\nI am ready.";
        let waivers = crate::spoken_layout::clause_leading_hesitation_ranges(supported_source);
        assert_eq!(waivers.len(), 1);
        let exact_waiver = waivers[0].clone();
        let supported_filler_start = supported_source
            .find("Um")
            .expect("supported source contains the filler");
        assert_eq!(
            exact_waiver,
            supported_filler_start..supported_filler_start + "Um".len()
        );
        assert_eq!(supported_source.get(exact_waiver.clone()), Some("Um"));
        assert!(preserves_with_source_term_waivers(
            supported_source,
            supported_candidate,
            Some(CleanupOperation::Cleanup),
            std::slice::from_ref(&exact_waiver),
        ));
        let shifted_waiver = (exact_waiver.start + 1)..(exact_waiver.end + 1);
        assert!(!preserves_with_source_term_waivers(
            supported_source,
            supported_candidate,
            Some(CleanupOperation::Cleanup),
            std::slice::from_ref(&shifted_waiver),
        ));
        assert!(crate::spoken_layout::preserves_required_layout(
            supported_source,
            supported_candidate
        ));
    }

    #[test]
    fn typed_tokens_do_not_match_substrings_or_changed_signs() {
        assert!(!preserves("1250", "12500", None));
        assert!(!preserves("/tmp/a", "/tmp/ab", None));
        assert!(!preserves("React", "Preact", None));
        assert!(!preserves("-50%", "50%", None));
        assert!(!preserves("1250美元", "-1250美元", None));
        assert!(!preserves("$1250", "-$1250", None));
    }

    #[test]
    fn final_guard_retains_negation_in_english_and_chinese() {
        assert!(!preserves("I do not approve", "I approve", None));
        assert!(!preserves("不要上线", "上线", None));
        assert!(!preserves("No release today", "Release today", None));
        assert!(!preserves("Don't deploy", "Deploy", None));
        assert!(!preserves("Never merge", "Merge", None));
        assert!(!preserves("NOT approved", "Approved", None));
        assert!(!preserves("Don’t ship", "Ship", None));
        assert!(preserves("I do not approve", "I do not approve.", None));
        assert!(
            preserves("WE DON’T approve", "WE DO NOT approve", None),
            "case-insensitive negation rewrite must not be rejected as mismatched acronym terms"
        );
        assert!(preserves("we don't approve", "we do not approve", None));
        for (source, candidate) in [
            ("WE SHOULDN’T approve", "WE shouldn't approve"),
            ("WE DOESN’T approve", "WE doesn't approve"),
            ("WE WON’T approve", "WE won't approve"),
        ] {
            assert!(
                preserves(source, candidate, None),
                "supported uppercase expression rejected: {source} -> {candidate}"
            );
        }
        assert!(!preserves("Don approved it", "approved it", None));
        assert!(!preserves("DON approved it", "approved it", None));
        assert!(!preserves(
            "we invited Will not Paul",
            "we invited won't Paul",
            None
        ));
        assert!(preserves("`WE SHOULDN’T`", "`WE SHOULDN’T`", None));
        assert!(preserves("we don't do that", "we do not do that", None));
        assert!(preserves("we do not deploy", "we don't deploy", None));
        assert!(preserves("we do\t not deploy", "we don't deploy", None));
        assert!(preserves("we do\nnot deploy", "we don't deploy", None));
        assert!(!preserves("go now", "don't go now", None));
        assert!(!preserves("don't go now", "go now", None));
        assert!(!preserves("we don't approve", "we not approve", None));
        assert!(!preserves("we don't approve", "we does not approve", None));
        assert!(!preserves("we shouldn't approve", "we s not approve", None));
        assert!(!preserves("we don't approve", "we do not deploy", None));
        assert!(preserves("we don't_approve", "we don't_approve", None));
        assert!(preserves("不要上线", "不要上线。", None));
        assert!(preserves("say no to this", "say no to this", None));
        for (source, candidate) in [
            ("I can't deploy", "I can deploy"),
            ("I haven't deployed", "I have deployed"),
            ("She hasn't deployed", "She has deployed"),
            ("We hadn't deployed", "We had deployed"),
            ("You needn't deploy", "You need deploy"),
            ("Neither build nor deploy", "Neither build deploy"),
            ("无法上线", "可以上线"),
            ("勿上线", "请上线"),
        ] {
            assert!(
                !preserves(source, candidate, None),
                "lost negation: {source}"
            );
        }
    }

    #[test]
    fn task35_negation_lcs_tie_and_edge_cases() {
        assert!(!negation_targets_have_unambiguous_lcs(
            &["x".into()],
            &[],
            &[Some(0)]
        ));
        assert!(negation_targets_have_unambiguous_lcs(
            &["x".into()],
            &["x".into()],
            &[Some(0)]
        ));
        // Matching the target at candidate index 1 ties a skip-source path
        // whose target is lost; matching index 0 ties a skip-candidate path.
        assert!(!negation_targets_have_unambiguous_lcs(
            &["x".into(), "x".into()],
            &["x".into()],
            &[Some(0), None]
        ));
        assert!(!negation_targets_have_unambiguous_lcs(
            &["x".into()],
            &["x".into(), "x".into()],
            &[Some(0)]
        ));
        assert!(!negation_targets_have_unambiguous_lcs(
            &["a".into(), "x".into()],
            &["a".into()],
            &[None, Some(0)]
        ));
        assert!(negation_targets_have_unambiguous_lcs(
            &["x".into()],
            &["x".into()],
            &[Some(0)]
        ));
        assert!(negation_targets_have_unambiguous_lcs(
            &["x".into(), "y".into()],
            &["y".into()],
            &[None, Some(0)]
        ));
        assert!(negation_targets_have_unambiguous_lcs(
            &["x".into(), "y".into()],
            &["x".into()],
            &[Some(0), None]
        ));
        assert!(negation_targets_have_unambiguous_lcs(
            &["x".into(), "y".into()],
            &["x".into(), "y".into()],
            &[Some(0), Some(1)]
        ));
        assert!(!preserves(
            "don't go, don't wait",
            "do not go, don't leave",
            None
        ));
        assert!(!preserves(
            "don't go, don't wait",
            "don't wait, don't go",
            None
        ));
        assert!(preserves(
            "don't go, don't wait",
            "do not go; don't wait",
            None
        ));
        assert!(!preserves(
            "don't go, never wait",
            "never wait, don't go",
            None
        ));
        assert!(!preserves("don't go", "won't go", None));
        assert!(!preserves("don't go", "do not leave", None));
        assert!(preserves("not never deploy", "not, never deploy", None));
        assert!(!preserves(
            "not never deploy merge",
            "not deploy never merge",
            None
        ));
        assert!(!preserves(
            "not alpha never beta",
            "not never alpha beta",
            None
        ));
    }

    #[cfg(test)]
    fn task35_measure_helper_case(name: &str, source: &str, candidate: &str) {
        task35_measure_helper_case_expect_path(name, source, candidate, true, false);
    }

    #[cfg(test)]
    fn task35_measure_helper_case_expect_path(
        name: &str,
        source: &str,
        candidate: &str,
        expected: bool,
        expected_dp: bool,
    ) {
        let source_bindings = negation_bindings(source);
        let candidate_bindings = negation_bindings(candidate);
        assert_eq!(
            source_bindings.len(),
            candidate_bindings.len(),
            "{name}: marker count"
        );
        assert_eq!(
            source_bindings
                .iter()
                .map(|(marker, _)| marker)
                .collect::<Vec<_>>(),
            candidate_bindings
                .iter()
                .map(|(marker, _)| marker)
                .collect::<Vec<_>>(),
            "{name}: canonical marker sequence"
        );
        assert_eq!(
            source_bindings
                .iter()
                .map(|(_, target)| target.is_some())
                .collect::<Vec<_>>(),
            candidate_bindings
                .iter()
                .map(|(_, target)| target.is_some())
                .collect::<Vec<_>>(),
            "{name}: target binding shape"
        );
        assert!(
            source_bindings.iter().all(|(_, target)| target.is_some()),
            "{name}: source targets"
        );
        assert!(
            candidate_bindings
                .iter()
                .all(|(_, target)| target.is_some()),
            "{name}: candidate targets"
        );
        let (source_content, source_targets) =
            negation_content_and_targets(source, &source_bindings);
        let (candidate_content, candidate_targets) =
            negation_content_and_targets(candidate, &candidate_bindings);
        assert!(
            !source_content.is_empty() && !candidate_content.is_empty(),
            "{name}: nonempty content"
        );
        let mut expected_candidates = vec![None; source_content.len()];
        for ((_, source_target), (_, candidate_target)) in
            source_bindings.iter().zip(&candidate_bindings)
        {
            if let (Some(source_index), Some(candidate_index)) = (source_target, candidate_target) {
                assert!(
                    *source_index < source_content.len(),
                    "{name}: source target in bounds"
                );
                assert!(
                    *candidate_index < candidate_content.len(),
                    "{name}: candidate target in bounds"
                );
                assert!(
                    expected_candidates[*source_index]
                        .is_none_or(|existing| existing == *candidate_index),
                    "{name}: source target has a conflicting candidate binding"
                );
                expected_candidates[*source_index] = Some(*candidate_index);
            }
        }
        assert_eq!(
            source_targets.len(),
            source_bindings.len(),
            "{name}: source target inventory"
        );
        assert_eq!(
            candidate_targets.len(),
            candidate_bindings.len(),
            "{name}: candidate target inventory"
        );
        let actual_dp = source_content != candidate_content;
        assert_eq!(actual_dp, expected_dp, "{name}: measured path");
        let start = std::time::Instant::now();
        let result = preserves_negation(source, candidate, None);
        let elapsed_us = start.elapsed().as_micros();
        let source_n = source_content.len();
        let candidate_m = candidate_content.len();
        let markers = source_bindings.len();
        let expected_scale = match name {
            "no_negation_64k" => Some((65_536, 65_536, 8_192, 8_192, 0)),
            "no_negation_256k" => Some((262_144, 262_144, 32_768, 32_768, 0)),
            "sparse_equal_approx_64k" => Some((64_013, 64_013, 8_001, 8_001, 1)),
            "sparse_equal_approx_256k" => Some((256_013, 256_013, 32_001, 32_001, 1)),
            "dense_equal_do_not_approx_4k" => Some((3_690, 3_690, 410, 410, 410)),
            "dense_equal_do_not_approx_16k" => Some((14_760, 14_760, 1_640, 1_640, 1_640)),
            "dense_equal_do_not_approx_64k" => Some((58_986, 58_986, 6_554, 6_554, 6_554)),
            "dense_equal_cjk_approx_8k" => Some((8_196, 8_196, 1_366, 1_366, 683)),
            "dense_equal_cjk_approx_32k" => Some((32_772, 32_772, 5_462, 5_462, 2_731)),
            "dense_equal_cjk_approx_128k" => Some((131_076, 131_076, 21_846, 21_846, 10_923)),
            "long_gap_hyphenated_not_8k" => Some((8_200, 8_200, 1, 1, 1_024)),
            _ if name.starts_with("changed_legal_prefix_events_") => {
                let events = name
                    .rsplit('_')
                    .next()
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap();
                Some((events * 14, events * 14 + 8, events, events + 1, events))
            }
            _ if name.starts_with("repeated_target_ambiguous_") => {
                let events = name
                    .rsplit('_')
                    .next()
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap();
                Some((events * 9, events * 9 + 2, events, events + 1, events))
            }
            _ if name.starts_with("dense_multiple_neg_cycles_") => {
                let cycles = name
                    .rsplit('_')
                    .next()
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap();
                Some((
                    cycles * 29,
                    cycles * 29 + 8,
                    cycles * 3,
                    cycles * 3 + 1,
                    cycles * 2,
                ))
            }
            _ => None,
        };
        if let Some((source_bytes, candidate_bytes, expected_n, expected_m, expected_k)) =
            expected_scale
        {
            assert_eq!(
                source.len(),
                source_bytes,
                "{name}: constructed source byte scale"
            );
            assert_eq!(
                candidate.len(),
                candidate_bytes,
                "{name}: constructed candidate byte scale"
            );
            assert_eq!(
                (source_n, candidate_m, markers),
                (expected_n, expected_m, expected_k),
                "{name}: constructed n/m/k scale"
            );
        }
        eprintln!(
            "TASK35_NEGATION_COST name={name} source_bytes={} candidate_bytes={} source_n={source_n} candidate_m={candidate_m} markers={markers} actual_path={} result={result} expected={expected} helper_us={elapsed_us}",
            source.len(), candidate.len(), if actual_dp { "lcs" } else { "equal_content" }
        );
        assert_eq!(result, expected, "{name}");
    }

    #[test]
    #[ignore = "manual Task35 parse-cost probe; run exact case in a standalone process"]
    fn task35_cost_no_negation_64k() {
        let source = "ordinary words and punctuation. ".repeat(2048);
        task35_measure_helper_case("no_negation_64k", &source, &source);
    }

    #[test]
    #[ignore = "manual Task35 parse-cost probe; run exact case in a standalone process"]
    fn task35_cost_no_negation_256k() {
        let source = "ordinary words and punctuation. ".repeat(8192);
        task35_measure_helper_case("no_negation_256k", &source, &source);
    }

    #[test]
    #[ignore = "manual Task35 parse-cost probe; run exact case in a standalone process"]
    fn task35_cost_sparse_equal_64k() {
        let source = format!("{} don't deploy", "ordinary words. ".repeat(4000));
        task35_measure_helper_case("sparse_equal_approx_64k", &source, &source);
    }

    #[test]
    #[ignore = "manual Task35 parse-cost probe; run exact case in a standalone process"]
    fn task35_cost_sparse_equal_256k() {
        let source = format!("{} don't deploy", "ordinary words. ".repeat(16000));
        task35_measure_helper_case("sparse_equal_approx_256k", &source, &source);
    }

    #[test]
    #[ignore = "manual Task35 parse-cost probe; run exact case in a standalone process"]
    fn task35_cost_dense_equal_do_not_4k() {
        let source = "do not x ".repeat(410);
        task35_measure_helper_case("dense_equal_do_not_approx_4k", &source, &source);
    }

    #[test]
    #[ignore = "manual Task35 parse-cost probe; run exact case in a standalone process"]
    fn task35_cost_dense_equal_do_not_16k() {
        let source = "do not x ".repeat(1640);
        task35_measure_helper_case("dense_equal_do_not_approx_16k", &source, &source);
    }

    #[test]
    #[ignore = "manual Task35 parse-cost probe; run exact case in a standalone process"]
    fn task35_cost_dense_equal_do_not_64k() {
        let source = "do not x ".repeat(6554);
        task35_measure_helper_case("dense_equal_do_not_approx_64k", &source, &source);
    }

    #[test]
    #[ignore = "manual Task35 parse-cost probe; run exact case in a standalone process"]
    fn task35_cost_long_gap_hyphenated_not_8k() {
        let source = format!(
            "do{}{} deploy",
            " ".repeat(4096),
            "not-".repeat(1023) + "not"
        );
        task35_measure_helper_case("long_gap_hyphenated_not_8k", &source, &source);
    }

    #[test]
    #[ignore = "manual Task35 parse-cost probe; run exact case in a standalone process"]
    fn task35_cost_dense_equal_cjk_approx_8k() {
        let source = "不发布。".repeat(683);
        task35_measure_helper_case("dense_equal_cjk_approx_8k", &source, &source);
    }

    #[test]
    #[ignore = "manual Task35 parse-cost probe; run exact case in a standalone process"]
    fn task35_cost_dense_equal_cjk_approx_32k() {
        let source = "不发布。".repeat(2731);
        task35_measure_helper_case("dense_equal_cjk_approx_32k", &source, &source);
    }

    #[test]
    #[ignore = "manual Task35 parse-cost probe; run exact case in a standalone process"]
    fn task35_cost_dense_equal_cjk_approx_128k() {
        let source = "不发布。".repeat(10923);
        task35_measure_helper_case("dense_equal_cjk_approx_128k", &source, &source);
    }

    macro_rules! task35_lcs_cost_case {
        ($test:ident, $label:literal, $tokens:expr, $scenario:ident) => {
            #[test]
            #[ignore = "manual Task35 LCS cost probe; run exact case in a standalone process"]
            fn $test() {
                let count = $tokens;
                let (source, candidate, expected) =
                    task35_lcs_cost_inputs(count, stringify!($scenario));
                task35_measure_helper_case_expect_path($label, &source, &candidate, expected, true);
            }
        };
    }

    #[cfg(test)]
    fn task35_lcs_cost_inputs(scale: usize, scenario: &str) -> (String, String, bool) {
        match scenario {
            "changed_legal_prefix" => {
                let source = "do not deploy ".repeat(scale);
                let candidate = format!("Updated {}", "do not deploy ".repeat(scale));
                (source, candidate, true)
            }
            "repeated_target_ambiguous" => {
                let source = "do not x ".repeat(scale);
                let candidate = format!("x {source}");
                (source, candidate, false)
            }
            "dense_multiple_neg_prefix" => {
                let body = "do not alpha but don't beta; ".repeat(scale);
                (body.clone(), format!("Updated {body}"), true)
            }
            _ => unreachable!("closed Task35 probe scenario"),
        }
    }

    task35_lcs_cost_case!(
        task35_cost_changed_prefix_256,
        "changed_legal_prefix_events_256",
        256,
        changed_legal_prefix
    );
    task35_lcs_cost_case!(
        task35_cost_changed_prefix_1024,
        "changed_legal_prefix_events_1024",
        1024,
        changed_legal_prefix
    );
    task35_lcs_cost_case!(
        task35_cost_changed_prefix_2048,
        "changed_legal_prefix_events_2048",
        2048,
        changed_legal_prefix
    );
    task35_lcs_cost_case!(
        task35_cost_changed_prefix_4096,
        "changed_legal_prefix_events_4096",
        4096,
        changed_legal_prefix
    );
    task35_lcs_cost_case!(
        task35_cost_repeated_ambiguous_256,
        "repeated_target_ambiguous_256",
        256,
        repeated_target_ambiguous
    );
    task35_lcs_cost_case!(
        task35_cost_repeated_ambiguous_1024,
        "repeated_target_ambiguous_1024",
        1024,
        repeated_target_ambiguous
    );
    task35_lcs_cost_case!(
        task35_cost_repeated_ambiguous_2048,
        "repeated_target_ambiguous_2048",
        2048,
        repeated_target_ambiguous
    );
    task35_lcs_cost_case!(
        task35_cost_repeated_ambiguous_4096,
        "repeated_target_ambiguous_4096",
        4096,
        repeated_target_ambiguous
    );
    task35_lcs_cost_case!(
        task35_cost_dense_multiple_neg_256,
        "dense_multiple_neg_cycles_256",
        256,
        dense_multiple_neg_prefix
    );
    task35_lcs_cost_case!(
        task35_cost_dense_multiple_neg_1024,
        "dense_multiple_neg_cycles_1024",
        1024,
        dense_multiple_neg_prefix
    );
    task35_lcs_cost_case!(
        task35_cost_dense_multiple_neg_2048,
        "dense_multiple_neg_cycles_2048",
        2048,
        dense_multiple_neg_prefix
    );
    task35_lcs_cost_case!(
        task35_cost_dense_multiple_neg_4096,
        "dense_multiple_neg_cycles_4096",
        4096,
        dense_multiple_neg_prefix
    );

    #[test]
    #[ignore = "manual Task35 selected Rewrite full-chain cost probe; source must remain within 16 KiB route cap"]
    fn task35_cost_selected_rewrite_accepted_chain_4k() {
        use crate::{
            context::{ContextFamily, FocusKind},
            llm::{CleanupIntent, CleanupOperation},
            CleanupDecision, FinalizationContext,
        };
        let settings = crate::store::Settings::default();
        let raw = "We do not deploy today. ".repeat(180);
        assert_eq!(raw.len(), 4_320, "selected_text raw route fixture bytes");
        assert!(raw.len() <= 16 * 1024, "selected_text route input byte cap");
        let whole_chain_t = std::time::Instant::now();
        let t = std::time::Instant::now();
        let prepared = crate::prepare_cleanup_transcript_for_scene(
            None,
            &settings,
            &raw,
            ContextFamily::General,
            1.0,
            FocusKind::Document,
            None,
            false,
        );
        let prepare_us = t.elapsed().as_micros();
        assert!(!prepared.text.is_empty());
        assert!(
            prepared.text.len() <= 16 * 1024,
            "prepared selected_text route input byte cap"
        );
        let intent = CleanupIntent::selected_text(CleanupOperation::Rewrite, &prepared.text);
        let candidate = format!("Updated: {}", prepared.text);
        assert_ne!(
            candidate, prepared.text,
            "adapter candidate must exercise Rewrite"
        );
        assert!(
            candidate.len() <= 16 * 1024,
            "selected_text Rewrite result remains within the 16 KiB input budget"
        );
        let helper_t = std::time::Instant::now();
        assert!(preserves_negation(&prepared.text, &candidate, None));
        let helper_us = helper_t.elapsed().as_micros();
        let t = std::time::Instant::now();
        let accepted = crate::llm::accept_cleanup_reply_with_facts(candidate.clone(), &intent)
            .expect("accepted selected-text Rewrite candidate passes the shared adapter guard");
        assert_eq!(
            accepted.accepted_text, candidate,
            "adapter must accept the actual candidate unchanged"
        );
        let adapter_us = t.elapsed().as_micros();
        let t = std::time::Instant::now();
        let finalized = crate::finalize_text_for_scene(
            &prepared.text,
            CleanupDecision::Provider(accepted.accepted_text.clone()),
            FinalizationContext {
                scene: prepared.scene,
                operation: CleanupOperation::Rewrite,
                revision_source: Some(&prepared.revision_source),
                prepared_transcript: Some(&prepared.text),
                revision_authorizations: &prepared.revision_authorizations,
                promoted_pair_protections: &prepared.promoted_pair_protections,
                list_revision_proof: prepared.list_revision_proof.as_ref(),
                structure_safe_source: None,
                output_projection_list_revision_proof: None,
                layout_source: None,
                exact_fallback: false,
            },
        )
        .expect("nonempty selected Rewrite candidate has a final result");
        let finalize_us = t.elapsed().as_micros();
        assert!(!finalized.degraded);
        assert_eq!(
            finalized.text, candidate,
            "final result must preserve the accepted candidate exactly"
        );
        let whole_chain_us = whole_chain_t.elapsed().as_micros();
        eprintln!(
            "TASK35_NEGATION_CHAIN name=selected_rewrite_accepted_chain_4k route=selected_text_rewrite input_cap_bytes=16384 raw_bytes={} prepared_bytes={} candidate_bytes={} content_n={} content_m={} markers={} static_expected_shared_guard_sites=3 (static_expectation_not_runtime_count) result=accepted helper_us={helper_us} prepare_us={prepare_us} adapter_us={adapter_us} finalize_us={finalize_us} stage_sum_us={} whole_chain_us={whole_chain_us}",
            raw.len(), prepared.text.len(), accepted.accepted_text.len(),
            negation_content_tokens(&prepared.text, &negation_expression_ranges(&prepared.text)).len(),
            negation_content_tokens(&accepted.accepted_text, &negation_expression_ranges(&accepted.accepted_text)).len(),
            negation_marker_ranges(&prepared.text).len(), prepare_us + adapter_us + finalize_us
        );
    }

    #[test]
    fn final_guard_rejects_moved_negation_and_ignores_resegmentation() {
        use crate::context::{ContextFamily, FocusKind};

        let settings = crate::store::Settings::default();
        let prepare = |source: &str| {
            crate::prepare_cleanup_transcript_for_scene(
                None,
                &settings,
                source,
                ContextFamily::General,
                1.0,
                FocusKind::Document,
                None,
                false,
            )
        };
        let finalize = |prepared: &crate::PreparedCleanupTranscript,
                        candidate: &str,
                        operation: crate::llm::CleanupOperation| {
            crate::finalize_text_for_scene(
                &prepared.text,
                crate::CleanupDecision::Provider(candidate.to_owned()),
                crate::FinalizationContext {
                    scene: prepared.scene,
                    operation,
                    revision_source: Some(&prepared.revision_source),
                    prepared_transcript: Some(&prepared.text),
                    revision_authorizations: &prepared.revision_authorizations,
                    promoted_pair_protections: &prepared.promoted_pair_protections,
                    list_revision_proof: prepared.list_revision_proof.as_ref(),
                    structure_safe_source: None,
                    output_projection_list_revision_proof: None,
                    layout_source: None,
                    exact_fallback: false,
                },
            )
            .expect("nonempty deterministic final result")
        };

        for (contracted, expanded) in AUXILIARY_NEGATION_PAIRS {
            let source = format!("we {contracted} approve");
            let candidate = format!("we {expanded} approve");
            let prepared = prepare(&source);
            let intent = crate::llm::CleanupIntent::selected_text(
                crate::llm::CleanupOperation::Rewrite,
                &prepared.text,
            );
            let accepted = crate::llm::accept_cleanup_reply_with_facts(candidate.clone(), &intent)
                .unwrap_or_else(|error| {
                    panic!("authorized finite expansion rejected: {source} -> {candidate}: {error}")
                });
            let finalized = finalize(
                &prepared,
                &accepted.accepted_text,
                crate::llm::CleanupOperation::Rewrite,
            );
            assert!(
                !finalized.degraded,
                "finite expansion degraded: {source} -> {candidate}"
            );
            assert_eq!(finalized.text, candidate);
        }

        let source = "周五不开会，周六开会";
        let moved = "周五开会，周六不开会";
        let prepared = prepare(source);
        assert_eq!(prepared.text, source);
        assert!(matches!(
            crate::llm::accept_cleanup_reply_with_facts(moved.to_owned(), &prepared.intent),
            Err(crate::llm::LlmError::CleanupRejected(fact))
                if fact.reason == crate::llm::GuardReason::ProtectedToken
        ));
        let rejected = finalize(&prepared, moved, crate::llm::CleanupOperation::Cleanup);
        assert!(rejected.degraded);
        assert_eq!(rejected.text, source);

        let resegmented = "周五不 开会；周六开会";
        let accepted =
            crate::llm::accept_cleanup_reply_with_facts(resegmented.to_owned(), &prepared.intent)
                .expect(
                    "punctuation and spacing changes preserve the same following content token",
                );
        let finalized = finalize(
            &prepared,
            &accepted.accepted_text,
            crate::llm::CleanupOperation::Cleanup,
        );
        assert!(!finalized.degraded);
        assert_eq!(finalized.text, resegmented);

        let prefix_source = "开头我们不开会，明天再说";
        let prefix_candidate = "首先我们不开会，明天再说";
        let prefix_prepared = prepare(prefix_source);
        let prefix_accepted = crate::llm::accept_cleanup_reply_with_facts(
            prefix_candidate.to_owned(),
            &prefix_prepared.intent,
        )
        .expect("an unrelated prefix edit preserves the local negation target");
        let prefix_finalized = finalize(
            &prefix_prepared,
            &prefix_accepted.accepted_text,
            crate::llm::CleanupOperation::Cleanup,
        );
        assert!(!prefix_finalized.degraded);
        assert_eq!(prefix_finalized.text, prefix_candidate);

        let migration_source = "开头我们不开会，明天开会";
        let migration_candidate = "首先我们开会，明天不开会";
        let migration_prepared = prepare(migration_source);
        assert!(matches!(
            crate::llm::accept_cleanup_reply_with_facts(
                migration_candidate.to_owned(),
                &migration_prepared.intent,
            ),
            Err(crate::llm::LlmError::CleanupRejected(fact))
                if fact.reason == crate::llm::GuardReason::ProtectedToken
        ));
        let migration_finalized = finalize(
            &migration_prepared,
            migration_candidate,
            crate::llm::CleanupOperation::Cleanup,
        );
        assert!(migration_finalized.degraded);
        assert_eq!(migration_finalized.text, migration_source);

        assert!(!preserves(
            "not deploy; not deploy",
            "not deploy; deploy",
            None
        ));
        assert!(!preserves(
            "周五不开会，周六开会，周日开会",
            "周五开会，周六开会，周日不开会",
            None
        ));
        assert!(preserves(
            "开头我们不开会，明天再说",
            "首先我们不开会，明天再说",
            None
        ));
        assert!(!preserves(
            "开头我们不开会，明天开会",
            "首先我们开会，明天不开会",
            None
        ));
        assert!(!preserves(
            "周五不开会，周六开会，周日开会",
            "周五开会，周六不开会，周日开会",
            None
        ));
        assert!(preserves(
            "not deploy; not deploy",
            "not deploy. not deploy",
            None
        ));
        assert!(!preserves("not deploy", "not deploy; not deploy", None));
        assert!(!preserves("deploy", "not deploy", None));
        assert!(!preserves("not deploy", "deploy", None));
        assert!(preserves("never", "never!", None));
        assert!(!preserves("never", "never deploy", None));
        assert!(preserves(
            "not deploy, never merge",
            "not deploy; never merge",
            None
        ));
        assert!(!preserves("not deploy", "not deploy; never merge", None));
        assert!(!preserves("not deploy, deploy", "deploy, not deploy", None));
        assert!(!preserves("not deploy", "not deploy, deploy", None));

        let english_source = "we don't approve";
        let english_prepared = prepare(english_source);
        let rewrite_intent = crate::llm::CleanupIntent::selected_text(
            crate::llm::CleanupOperation::Rewrite,
            &english_prepared.text,
        );
        let expanded = "we do not approve";
        let accepted =
            crate::llm::accept_cleanup_reply_with_facts(expanded.to_owned(), &rewrite_intent)
                .expect("the finite don't → do not expansion preserves the same target");
        let finalized = finalize(
            &english_prepared,
            &accepted.accepted_text,
            crate::llm::CleanupOperation::Rewrite,
        );
        assert!(!finalized.degraded);
        assert_eq!(finalized.text, expanded);

        let aux_source = "we don't do that";
        let aux_prepared = prepare(aux_source);
        let aux_intent = crate::llm::CleanupIntent::selected_text(
            crate::llm::CleanupOperation::Rewrite,
            &aux_prepared.text,
        );
        let aux_expanded = "we do not do that";
        let aux_accepted =
            crate::llm::accept_cleanup_reply_with_facts(aux_expanded.to_owned(), &aux_intent)
                .expect(
                    "the complete do not expression keeps its auxiliary outside content alignment",
                );
        let aux_finalized = finalize(
            &aux_prepared,
            &aux_accepted.accepted_text,
            crate::llm::CleanupOperation::Rewrite,
        );
        assert!(!aux_finalized.degraded);
        assert_eq!(aux_finalized.text, aux_expanded);

        for (source, candidate) in [
            ("we do not do that", "we don't do that"),
            ("We Don’t approve", "we don’t approve"),
            ("we do\tnot deploy", "we don't deploy"),
            ("we do\nnot deploy", "we don't deploy"),
        ] {
            let prepared = prepare(source);
            assert_eq!(
                prepared.text, source,
                "layout preparation keeps the authorized input"
            );
            let intent = crate::llm::CleanupIntent::selected_text(
                crate::llm::CleanupOperation::Rewrite,
                &prepared.text,
            );
            let accepted =
                crate::llm::accept_cleanup_reply_with_facts(candidate.to_owned(), &intent)
                    .unwrap_or_else(|error| {
                        panic!(
                            "authorized negation rewrite rejected by adapter: {source:?} -> {candidate:?}: {error}"
                        )
                    });
            assert_eq!(
                accepted.accepted_text, candidate,
                "adapter accepts candidate unchanged"
            );
            let finalized = finalize(
                &prepared,
                &accepted.accepted_text,
                crate::llm::CleanupOperation::Rewrite,
            );
            assert!(
                !finalized.degraded,
                "authorized rewrite remains non-degraded"
            );
            assert_eq!(
                finalized.text, candidate,
                "finalizer preserves accepted rewrite"
            );
        }

        for changed in ["we does not approve", "we do not deploy"] {
            assert!(matches!(
                crate::llm::accept_cleanup_reply_with_facts(
                    changed.to_owned(),
                    &rewrite_intent,
                ),
                Err(crate::llm::LlmError::CleanupRejected(fact))
                    if fact.reason == crate::llm::GuardReason::ProtectedToken
            ));
            let rejected = finalize(
                &english_prepared,
                changed,
                crate::llm::CleanupOperation::Rewrite,
            );
            assert!(rejected.degraded);
            assert_eq!(rejected.text, english_source);
        }
    }

    #[test]
    fn cleanup_retains_noun_references_but_allows_fillers_and_clause_that() {
        let source = "这个项目需要 review。那个项目需要 review。";
        let captured_provider_candidate = "这个项目需要 review。项目需要 review。";
        assert!(!preserves(
            source,
            captured_provider_candidate,
            Some(CleanupOperation::Cleanup)
        ));
        // Replay the captured provider output through the production final guard.
        assert_eq!(
            crate::llm::guard_final_output(
                source,
                captured_provider_candidate,
                Some(CleanupOperation::Cleanup)
            ),
            source
        );

        let english = "open this file, then review that file.";
        assert!(!preserves(
            english,
            "Open this file, then review the file.",
            Some(CleanupOperation::Cleanup)
        ));
        assert!(!preserves(
            english,
            "Open that file, then review this file.",
            Some(CleanupOperation::Cleanup)
        ));
        assert!(!preserves(
            "Open this file.",
            "Open this file and that folder.",
            Some(CleanupOperation::Cleanup)
        ));
        assert!(preserves(
            "Open This file.",
            "Open this file.",
            Some(CleanupOperation::Cleanup)
        ));

        let filler = "那个，麻烦你发一下文件。";
        assert_eq!(
            crate::llm::guard_final_output(
                filler,
                "麻烦你发一下文件。",
                Some(CleanupOperation::Cleanup)
            ),
            "麻烦你发一下文件。"
        );
        assert!(preserves(
            "I think that it is ready.",
            "I think it is ready.",
            Some(CleanupOperation::Cleanup)
        ));
        assert!(preserves(
            "I think that in this context, we should wait.",
            "I think in this context, we should wait.",
            Some(CleanupOperation::Cleanup)
        ));

        // Marker retention is for implicit cleanup; explicit rewriting and
        // shortening must remain able to restructure references.
        assert!(preserves(
            english,
            "review the file that I selected.",
            Some(CleanupOperation::Rewrite)
        ));
        assert!(preserves(
            english,
            "review this file.",
            Some(CleanupOperation::Shorten)
        ));
    }

    #[test]
    fn clause_leading_english_fillers_are_roles_while_names_and_literals_stay_terms() {
        let source =
            "Um, my I major in social science. Uh, this is my first year as well as my final year.";
        let candidate =
            "I major in social science. This is my first year as well as my final year.";
        let waivers = crate::spoken_layout::clause_leading_hesitation_ranges(source);
        assert_eq!(waivers, vec![0..2, 34..36]);
        assert!(preserves_with_source_term_waivers(
            source,
            candidate,
            Some(CleanupOperation::Cleanup),
            &waivers
        ));
        let accepted = crate::llm::accept_cleanup_reply_with_facts(
            candidate.to_owned(),
            &crate::llm::CleanupIntent::implicit(source),
        )
        .expect("source-backed hesitation cleanup passes shared acceptance");
        assert_eq!(accepted.accepted_text, candidate);
        assert_eq!(
            crate::llm::guard_final_output(
                source,
                &accepted.accepted_text,
                Some(CleanupOperation::Cleanup)
            ),
            candidate,
            "the final output guard must retain the accepted cleanup"
        );
        assert!(!preserves(
            source,
            candidate,
            Some(CleanupOperation::Cleanup)
        ));
        assert!(
            !preserves_with_source_term_waivers(
                source,
                candidate,
                Some(CleanupOperation::Rewrite),
                &waivers
            ),
            "non-cleanup operations cannot use hesitation source-term waivers"
        );
        assert!(
            !preserves_with_source_term_waivers(source, candidate, None, &waivers),
            "unknown operations cannot use hesitation source-term waivers"
        );
        let spans = protected_spans(source, Some(CleanupOperation::Cleanup));
        assert!(spans.iter().any(|span| {
            span.kind == ProtectedSpanKind::Term && &source[span.start_byte..span.end_byte] == "Um"
        }));
        assert!(spans.iter().any(|span| {
            span.kind == ProtectedSpanKind::Term && &source[span.start_byte..span.end_byte] == "Uh"
        }));

        let source =
            "Um, the umbrella and umami labels must stay exactly as written. Uh, keep \"Um\" in the quoted label.";
        let candidate =
            "The umbrella and umami labels must stay exactly as written. Keep \"Um\" in the quoted label.";
        let waivers = crate::spoken_layout::clause_leading_hesitation_ranges(source);
        assert_eq!(
            waivers
                .iter()
                .map(|range| source[range.clone()].to_owned())
                .collect::<Vec<_>>(),
            vec!["Um", "Uh"]
        );
        assert!(preserves_with_source_term_waivers(
            source,
            candidate,
            Some(CleanupOperation::Cleanup),
            &waivers
        ));
        let accepted = crate::llm::accept_cleanup_reply_with_facts(
            candidate.to_owned(),
            &crate::llm::CleanupIntent::implicit(source),
        )
        .expect("article-led and imperative clauses authorize only the spoken fillers");
        assert_eq!(accepted.accepted_text, candidate);
        assert_eq!(
            crate::llm::guard_final_output(
                source,
                &accepted.accepted_text,
                Some(CleanupOperation::Cleanup)
            ),
            candidate,
            "the final guard accepts the source-backed filler cleanup"
        );
        assert!(candidate.contains("\"Um\""), "quoted literal stays intact");

        for source in [
            "I met Um yesterday.",
            "‘Um, maybe.’",
            "`Uh` is a field name.",
            "UM is an identifier.",
            "Preferred name: Um, not Uh.",
            "Um umbrella, then umami.",
            "Um, I, am are listed tokens.",
            "Um, I. am ready.",
            "Um, `I am` ready.",
            "Uh, keep.",
            "Uh, keep. Next sentence.",
            "Uh, keep, or send this tomorrow.",
            "Um, the-I am are listed tokens.",
        ] {
            let spans = protected_spans(source, Some(CleanupOperation::Cleanup));
            let term = spans.into_iter().find(|span| {
                span.kind == ProtectedSpanKind::Term
                    && matches!(&source[span.start_byte..span.end_byte], "Um" | "Uh" | "UM")
            });
            assert!(
                term.is_some(),
                "expected a protected name/literal in {source}"
            );
            assert!(
                crate::spoken_layout::clause_leading_hesitation_ranges(source).is_empty(),
                "name/literal should not authorize removing a hesitation-like term: {source}"
            );
            if source == "Um, I, am are listed tokens." {
                assert_eq!(
                    crate::llm::guard_final_output(
                        source,
                        "I, am are listed tokens.",
                        Some(CleanupOperation::Cleanup)
                    ),
                    source,
                    "enumerated words do not establish a finite-clause filler role"
                );
            }
            if source == "Uh, keep. Next sentence." {
                assert_eq!(
                    crate::llm::guard_final_output(
                        source,
                        "keep. Next sentence.",
                        Some(CleanupOperation::Cleanup)
                    ),
                    source,
                    "a later sentence cannot complete an earlier imperative"
                );
            }
        }

        for compound in ["umbrella", "umami", "嗯哼", "啊哈"] {
            assert!(
                crate::spoken_layout::clause_leading_hesitation_ranges(compound).is_empty(),
                "compound was classified as a hesitation: {compound}"
            );
        }
    }

    #[test]
    fn translation_can_localize_weekdays_but_not_change_the_day_or_names() {
        let translate = Some(CleanupOperation::Translate);
        assert!(preserves(
            "The meeting is Friday with Mike",
            "La réunion est vendredi avec Mike",
            translate,
        ));
        assert!(preserves(
            "会议安排在周五和 Mike",
            "Réunion vendredi avec Mike",
            translate
        ));
        assert!(!preserves(
            "会议安排在周五和 Mike",
            "Réunion samedi avec Mike",
            translate
        ));
        assert!(!preserves(
            "会议安排在周五，日期是2026年3月3日",
            "Réunion vendredi, date du 2026年3月4日",
            translate
        ));
        assert!(!preserves(
            "The meeting is Friday with Mike",
            "La réunion est lundi avec Mike",
            translate,
        ));
        assert!(!preserves(
            "The meeting is Friday with Mike",
            "La réunion est vendredi avec Mark",
            translate,
        ));
    }

    #[test]
    fn ordinary_open_and_go_prose_are_not_command_spans() {
        for text in [
            "Open the file and go to the store.",
            "We can go over it later.",
        ] {
            assert!(!protected_spans(text, None)
                .iter()
                .any(|span| span.kind == ProtectedSpanKind::Command));
        }
        assert!(protected_spans("Run open -a VoiceFlow", None)
            .iter()
            .any(|span| span.kind == ProtectedSpanKind::Command));
    }

    #[test]
    fn contextual_chinese_yi_preserves_only_narrow_lexical_roles() {
        let kinds = |text: &str| {
            protected_spans(text, None)
                .into_iter()
                .filter(|span| span.kind == ProtectedSpanKind::Number)
                .map(|span| text[span.start_byte..span.end_byte].to_owned())
                .collect::<Vec<_>>()
        };
        for phrase in ["明天我们一起开会", "你们一起讨论", "大家一起跑步"] {
            assert!(kinds(phrase).is_empty(), "{phrase}");
        }
        for phrase in [
            "你们一起事故都没见过",
            "你们一起案件都没见过",
            "你们一起事件都没见过",
            "你们一起工作事故都没见过",
            "一起事故",
            "你们有一起事故",
        ] {
            assert_eq!(kinds(phrase), ["一"], "{phrase}");
        }
        assert_eq!(kinds("你们有一起事故"), ["一"]);
        assert_eq!(kinds("一起事故"), ["一"]);
        assert_eq!(kinds("明天我们两起事故"), ["两"]);
        for phrase in [
            "看一下今天安排",
            "看一下具体的流程是什么",
            "看一下详细的准确度是什么",
            "看一下这个报告",
            "看一下如何配置",
        ] {
            assert!(kinds(phrase).is_empty(), "{phrase}");
        }
        for phrase in ["看一下午视频", "看一下。稍后继续"] {
            assert_eq!(kinds(phrase), ["一"], "{phrase}");
        }
        for phrase in ["看一下", "看一下   ", "只看一下", "只 看一下", "看一下，。"]
        {
            assert_eq!(kinds(phrase), ["一"], "{phrase}");
        }
        assert_eq!(kinds("敲一下门"), ["一"]);
        assert_eq!(kinds("敲两下门"), ["两"]);
        assert_eq!(kinds("拍一下肩膀"), ["一"]);
        assert_eq!(kinds("拍两下肩膀"), ["两"]);
        assert_eq!(kinds("看一下门"), ["一"]);
        assert_eq!(kinds("一起事故"), ["一"]);
        assert_eq!(kinds("你们有一起事故"), ["一"]);
        let amount_spans = protected_spans("一元", None);
        assert!(amount_spans.iter().any(|span| {
            span.kind == ProtectedSpanKind::Amount
                && &"一元"[span.start_byte..span.end_byte] == "一元"
        }));
        assert!(!preserves("一元", "二元", None));
        for (source, candidate) in [
            ("日期一", "日期二"),
            ("第一个", "第二个"),
            ("单独一", "单独二"),
        ] {
            assert_eq!(kinds(source).len(), 1, "{source}");
            assert!(
                !preserves(source, candidate, None),
                "{source} -> {candidate}"
            );
        }
        assert!(preserves("明天我们一起开会", "明天我们一起讨论", None));
        assert!(!preserves("明天我们两起事故", "明天我们三起事故", None));
        assert!(preserves("大家一起跑步", "大家一起跑步", None));
        for phrase in [
            "你们一起事故都没见过",
            "你们一起案件都没见过",
            "你们一起事件都没见过",
            "你们一起工作事故都没见过",
        ] {
            assert!(
                !preserves(phrase, &phrase.replacen('一', "两", 1), None),
                "{phrase}"
            );
        }
        assert!(!preserves("看一下今天安排", "看一下明天安排", None));
        assert!(preserves("看一下今天安排", "看一下今天计划", None));
        assert!(!preserves("看一下这个报告", "看一下那个报告", None));
        assert!(preserves("看一下这个报告", "看一下这个文档", None));
        assert!(preserves("看一下如何配置", "看一下怎么配置", None));
        assert_eq!(kinds("只看一下具体流程"), ["一"]);
        assert!(!preserves("只看一下具体流程", "只看两下具体流程", None));
        assert!(!preserves("看一下午视频", "看两个下午视频", None));
        assert!(!preserves("看一下。稍后继续", "看两下。稍后继续", None));
        assert!(!preserves("看一下", "看两下", None));
        assert!(!preserves("只看一下", "只看两下", None));
        assert!(!preserves("只 看一下", "只 看两下", None));
        assert!(!preserves("敲一下门", "敲两下门", None));
        assert!(!preserves("Print `看一下`.", "Print `看两下`.", None));
        assert!(!preserves(
            "Print `你们一起讨论`.",
            "Print `你们两起讨论`.",
            None
        ));
    }

    #[test]
    fn task34_synthetic_provider_acceptance_baseline_observer() {
        use crate::{
            context::{ContextFamily, FocusKind},
            llm::{CleanupIntent, CleanupOperation},
            CleanupDecision, FinalizationContext, ResolvedCleanupScene,
        };
        use sha2::Digest;

        #[derive(serde::Serialize)]
        struct Row {
            id: String,
            source_kind: &'static str,
            candidate_kind: &'static str,
            source: String,
            candidate: String,
            adapter_decision: &'static str,
            adapter_reason: Option<&'static str>,
            final_decision: &'static str,
            final_reason: Option<&'static str>,
            adapter_guard_fallback: bool,
            final_preservation_fallback: bool,
            guard_fallback: bool,
        }

        let cases: Vec<(String, String, String)> = vec![
            (
                "positive_amount_250_dollars".to_owned(),
                "预算是两百五十美元。".to_owned(),
                "预算是250美元。".to_owned(),
            ),
            (
                "positive_percent_twenty".to_owned(),
                "增长百分之二十。".to_owned(),
                "增长20%。".to_owned(),
            ),
            (
                "positive_dateword_three_points".to_owned(),
                "向前三点。".to_owned(),
                "向前3点。".to_owned(),
            ),
            (
                "positive_line_head_ordinals_added".to_owned(),
                "准备材料。\n提交申请。".to_owned(),
                "1. 准备材料。\n2. 提交申请。".to_owned(),
            ),
            (
                "positive_arabic_to_chinese_equivalent_value".to_owned(),
                "预算是30元。".to_owned(),
                "预算是三十元。".to_owned(),
            ),
            (
                "positive_percent_reverse_equivalent".to_owned(),
                "增长20%。".to_owned(),
                "增长百分之二十。".to_owned(),
            ),
            (
                "negative_extra_value_250_from_brief".to_owned(),
                "预算是30美元。".to_owned(),
                "预算是250美元或30美元。".to_owned(),
            ),
            (
                "negative_arabic_to_chinese_wrong_value".to_owned(),
                "预算是30元。".to_owned(),
                "预算是二十九元。".to_owned(),
            ),
            (
                "negative_chinese_to_arabic_wrong_value".to_owned(),
                "预算是三十元。".to_owned(),
                "预算是31元。".to_owned(),
            ),
            (
                "negative_currency_word_changed".to_owned(),
                "费用是30美元。".to_owned(),
                "费用是30欧元。".to_owned(),
            ),
            (
                "negative_currency_symbol_changed".to_owned(),
                "费用是$30。".to_owned(),
                "费用是€30。".to_owned(),
            ),
            (
                "negative_sign_changed".to_owned(),
                "净变化是-30元。".to_owned(),
                "净变化是+30元。".to_owned(),
            ),
            (
                "negative_calendar_date_value_changed".to_owned(),
                "日期是2026年3月2日。".to_owned(),
                "日期是2026年3月3日。".to_owned(),
            ),
            (
                "negative_time_unit_changed".to_owned(),
                "保留3天。".to_owned(),
                "保留3月。".to_owned(),
            ),
            (
                "negative_percent_scale_changed".to_owned(),
                "比例是百分之二十。".to_owned(),
                "比例是0.2%。".to_owned(),
            ),
            (
                "negative_percent_value_changed".to_owned(),
                "比例是20%。".to_owned(),
                "比例是21%。".to_owned(),
            ),
            (
                "negative_repeated_value_changed".to_owned(),
                "数值30元，再次确认30元。".to_owned(),
                "数值30元，再次确认31元。".to_owned(),
            ),
            (
                "negative_repeated_value_added".to_owned(),
                "预算是30元。".to_owned(),
                "预算是30元和30元。".to_owned(),
            ),
            (
                "negative_multiple_values_reordered".to_owned(),
                "先支付30元，再支付20元。".to_owned(),
                "先支付20元，再支付30元。".to_owned(),
            ),
            (
                "negative_inline_ordinal_changed".to_owned(),
                "请检查第2行的记录。".to_owned(),
                "请检查第3行的记录。".to_owned(),
            ),
            (
                "negative_code_digit_changed".to_owned(),
                "请保留代码`item2`。".to_owned(),
                "请保留代码`item3`。".to_owned(),
            ),
        ];
        assert_eq!(
            cases.len(),
            21,
            "the fixed cohort contains 6 positives + 15 negatives"
        );

        let mut rows = Vec::with_capacity(cases.len());
        for (id, source, candidate) in cases {
            assert!(!source.trim().is_empty() && !candidate.trim().is_empty());
            let intent = CleanupIntent::implicit(&source);
            let (decision, adapter_decision, adapter_reason, adapter_fallback) =
                match crate::llm::accept_cleanup_reply_with_facts(candidate.clone(), &intent) {
                    Ok(accepted) => (
                        CleanupDecision::Provider(accepted.accepted_text),
                        "accepted",
                        None,
                        false,
                    ),
                    Err(crate::llm::LlmError::CleanupRejected(fact)) => {
                        let reason = fact.reason.code();
                        (
                            CleanupDecision::CleanupRejected(fact),
                            "rejected",
                            Some(reason),
                            true,
                        )
                    }
                    Err(crate::llm::LlmError::Guard(reason)) => {
                        let reason = reason.code();
                        (
                            CleanupDecision::GuardRejected,
                            "rejected",
                            Some(reason),
                            true,
                        )
                    }
                    Err(_) => panic!("synthetic nonempty candidate had a non-guard adapter error"),
                };
            let finalized = crate::finalize_text_for_scene(
                &source,
                decision,
                FinalizationContext {
                    scene: ResolvedCleanupScene::observed(
                        ContextFamily::General,
                        FocusKind::Unknown,
                    ),
                    operation: CleanupOperation::Cleanup,
                    revision_source: None,
                    prepared_transcript: None,
                    revision_authorizations: &[],
                    promoted_pair_protections: &[],
                    list_revision_proof: None,
                    structure_safe_source: None,
                    output_projection_list_revision_proof: None,
                    layout_source: None,
                    exact_fallback: false,
                },
            )
            .expect("nonempty synthetic source always has a final result");
            let final_preservation_fallback =
                finalized.degraded_reason == Some("preservation_guard");
            let final_decision = if finalized.degraded {
                "fallback"
            } else {
                "accepted"
            };
            let final_reason = finalized
                .cleanup_rejection
                .as_ref()
                .map(|fact| fact.reason.code())
                .or(finalized.degraded_reason);
            rows.push(Row {
                id: id.to_owned(),
                source_kind: "synthetic",
                candidate_kind: "synthetic",
                source,
                candidate,
                adapter_decision,
                adapter_reason,
                final_decision,
                final_reason,
                adapter_guard_fallback: adapter_fallback,
                final_preservation_fallback,
                guard_fallback: adapter_fallback || final_preservation_fallback,
            });
        }

        let cohort_material = rows
            .iter()
            .map(|row| format!("{}\t{}\t{}\n", row.id, row.source, row.candidate))
            .collect::<String>();
        let cohort_sha256 = format!("{:x}", sha2::Sha256::digest(cohort_material.as_bytes()));
        let fallback_rows = rows.iter().filter(|row| row.guard_fallback).count();
        let positive_rows = rows
            .iter()
            .filter(|row| row.id.starts_with("positive_"))
            .count();
        let negative_rows = rows.len() - positive_rows;
        let report = serde_json::json!({
            "observer": "task34_synthetic_provider_acceptance_baseline_v1",
            "cohort_id": "task34-synthetic-number-money-equivalence-v1",
            "source_and_candidate_are_synthetic": true,
            "provider_acceptance_eligible_rows": rows.len(),
            "eligible_rows_include_adapter_rejections": true,
            "excluded_routes": ["disabled", "provider_error", "snippet", "local_bypass"],
            "positive_rows": positive_rows,
            "negative_rows": negative_rows,
            "cohort_table_sha256": cohort_sha256,
            "guard_fallback_rows": fallback_rows,
            "guard_fallback_rate": fallback_rows as f64 / rows.len() as f64,
            "rows": rows,
        });
        println!("VOICEFLOW_TASK34_SYNTHETIC_BASELINE={report}");
    }
}
