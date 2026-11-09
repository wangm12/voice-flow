//! Bounded local word corrections extracted from complete before/after edits.
//!
//! History keeps confirm-to-add suggestions. After a verified paste,
//! `observe_after_paste` watches only the same focused field and sends eligible
//! non-overlapping word corrections through the existing learning owner.
//! Mixed edits, whole-word insertions or deletions, and ambiguous alignments are rejected.
//! Never install a global key event tap.

use crate::dictionary_learning_audit::{
    self as audit, AuditAttempt, ClassificationV1, PersistenceFactV1,
};
use crate::learning_research::LearningStage;
#[cfg(test)]
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use crate::{context, lexicon, lock_recover, store, AppState};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};

pub(crate) fn manual_learn_applications() -> Vec<context::ApplicationOption> {
    context::available_applications()
        .into_iter()
        .filter(|application| !context::is_browser_application(Some(&application.bundle_id)))
        .collect()
}

#[cfg(test)]
pub fn single_token_candidates(before: &str, after: &str) -> Vec<String> {
    let Some(mut corrections) = extract_word_corrections(before, after) else {
        return Vec::new();
    };
    if corrections.len() != 1 {
        return Vec::new();
    }
    let correction = corrections.pop().expect("one correction");
    dictionary_suggestion_pair_allowed(&correction.before_span, &correction.after)
        .then_some(correction.after)
        .into_iter()
        .collect()
}

fn changed_spans(before: &str, after: &str) -> (String, String) {
    let before_chars: Vec<char> = before.chars().collect();
    let after_chars: Vec<char> = after.chars().collect();
    if let Some(CjkNameEditBoundary::WholeName { start, end }) =
        cjk_name_edit_boundary(before, after)
    {
        let before_span = before_chars[start..end].iter().collect();
        let after_span = after_chars[start..end].iter().collect();
        return (before_span, after_span);
    }
    let mut prefix = 0;
    let max_prefix = before_chars.len().min(after_chars.len());
    while prefix < max_prefix && before_chars[prefix] == after_chars[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    let max_suffix = (before_chars.len() - prefix).min(after_chars.len() - prefix);
    while suffix < max_suffix
        && before_chars[before_chars.len() - 1 - suffix]
            == after_chars[after_chars.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let mut start = prefix;
    let mut before_end = before_chars.len() - suffix;
    let mut after_end = after_chars.len() - suffix;
    // Don't split a Latin token that coincidentally shares a prefix/suffix char.
    while start > 0
        && start < after_end
        && is_latin_token_char(after_chars[start])
        && is_latin_token_char(after_chars[start - 1])
    {
        start -= 1;
    }
    while after_end < after_chars.len()
        && after_end > start
        && is_latin_token_char(after_chars[after_end - 1])
        && is_latin_token_char(after_chars[after_end])
    {
        after_end += 1;
        if before_end < before_chars.len() {
            before_end += 1;
        }
    }
    // A 1-char CJK fix is not a token under the 2–8 rule. Grow just enough.
    while after_end.saturating_sub(start) < 2 && start > 0 && is_cjk(after_chars[start - 1]) {
        start -= 1;
    }
    while after_end.saturating_sub(start) < 2
        && after_end < after_chars.len()
        && is_cjk(after_chars[after_end])
    {
        after_end += 1;
        if before_end < before_chars.len() {
            before_end += 1;
        }
    }
    let before_span = before_chars
        .get(start..before_end)
        .unwrap_or(&[])
        .iter()
        .collect();
    let after_span = after_chars
        .get(start..after_end)
        .unwrap_or(&[])
        .iter()
        .collect();
    (before_span, after_span)
}

const MAX_EDIT_ALIGNMENT_CHARS: usize = 512;
const MAX_WORD_CORRECTIONS: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
struct WordCorrectionRange {
    before: std::ops::Range<usize>,
    after: std::ops::Range<usize>,
    before_surface: String,
    after_surface: String,
}

#[derive(Clone, Debug)]
struct AlignmentUnit {
    range: std::ops::Range<usize>,
    text: String,
}

/// Returns lexical replacements only when the whole edit has one unique,
/// order-preserving alignment of lexical and separator units and every changed
/// hunk is a complete word replacement. Whole-word insertions/deletions, moved
/// text, and mixed content edits fail the entire extraction.
fn extract_word_corrections(before: &str, after: &str) -> Option<Vec<DictionarySuggestion>> {
    if before == after {
        return Some(Vec::new());
    }
    if before.contains("***") || after.contains("***") {
        return None;
    }
    let before_chars: Vec<char> = before.chars().collect();
    let after_chars: Vec<char> = after.chars().collect();
    if before_chars.len() > MAX_EDIT_ALIGNMENT_CHARS
        || after_chars.len() > MAX_EDIT_ALIGNMENT_CHARS
        || matches!(
            cjk_name_edit_boundary(before, after),
            Some(CjkNameEditBoundary::Ambiguous)
        )
    {
        return None;
    }

    if let Some(CjkNameEditBoundary::WholeName { start, end }) =
        cjk_name_edit_boundary(before, after)
    {
        if before_chars[..start] == after_chars[..start]
            && before_chars[end..] == after_chars[end..]
        {
            let name_range = start..end;
            let before_surface: String = before_chars[name_range.clone()].iter().collect();
            let after_surface: String = after_chars[name_range.clone()].iter().collect();
            if !safe_word_surface(&before_surface)
                || !safe_word_surface(&after_surface)
                || unsafe_unicode_boundary(&before_chars, &name_range)
                || unsafe_unicode_boundary(&after_chars, &name_range)
            {
                return None;
            }
            return make_word_corrections(vec![WordCorrectionRange {
                before: name_range.clone(),
                after: name_range,
                before_surface,
                after_surface,
            }]);
        }
    }

    let before_units = alignment_units(&before_chars);
    let after_units = alignment_units(&after_chars);
    let before_keys = before_units
        .iter()
        .map(|unit| unit.text.as_str())
        .collect::<Vec<_>>();
    let after_keys = after_units
        .iter()
        .map(|unit| unit.text.as_str())
        .collect::<Vec<_>>();
    let unit_hunks = unique_alignment_hunks(&before_keys, &after_keys)?;
    if unit_hunks.is_empty()
        || unit_hunks
            .iter()
            .any(|(left, right)| left.is_empty() && right.is_empty())
        || changed_words_cross_hunks(&before_units, &after_units, &unit_hunks)
    {
        return None;
    }

    let mut ranges = Vec::with_capacity(unit_hunks.len());
    for (before_unit_hunk, after_unit_hunk) in unit_hunks {
        let before_hunk =
            alignment_hunk_char_range(&before_units, before_unit_hunk, before_chars.len())?;
        let after_hunk =
            alignment_hunk_char_range(&after_units, after_unit_hunk, after_chars.len())?;
        let before_range = correction_word_range(&before_chars, before_hunk.clone())?;
        let after_range = correction_word_range(&after_chars, after_hunk.clone())?;
        if !before_range.contains_range(&before_hunk) || !after_range.contains_range(&after_hunk) {
            return None;
        }
        let before_is_cjk = before_chars[before_range.clone()]
            .iter()
            .all(|ch| is_cjk(*ch));
        let after_is_cjk = after_chars[after_range.clone()]
            .iter()
            .all(|ch| is_cjk(*ch));
        if before_is_cjk
            && after_is_cjk
            && (before_hunk.start - before_range.start != after_hunk.start - after_range.start
                || before_range.end - before_hunk.end != after_range.end - after_hunk.end)
        {
            return None;
        }
        if unsafe_unicode_boundary(&before_chars, &before_range)
            || unsafe_unicode_boundary(&after_chars, &after_range)
        {
            return None;
        }
        let before_surface: String = before_chars[before_range.clone()].iter().collect();
        let after_surface: String = after_chars[after_range.clone()].iter().collect();
        if !safe_word_surface(&before_surface) || !safe_word_surface(&after_surface) {
            return None;
        }
        ranges.push(WordCorrectionRange {
            before: before_range,
            after: after_range,
            before_surface,
            after_surface,
        });
    }
    make_word_corrections(ranges)
}

fn alignment_units(chars: &[char]) -> Vec<AlignmentUnit> {
    let latin_ranges = latin_word_ranges(chars);
    let mut latin_index = 0usize;
    let mut units = Vec::new();
    let mut index = 0usize;

    while index < chars.len() {
        while latin_ranges
            .get(latin_index)
            .is_some_and(|range| range.end <= index)
        {
            latin_index += 1;
        }
        if let Some(range) = latin_ranges
            .get(latin_index)
            .filter(|range| range.start == index)
        {
            units.push(AlignmentUnit {
                range: range.clone(),
                text: chars[range.clone()].iter().collect(),
            });
            index = range.end;
            latin_index += 1;
            continue;
        }

        let start = index;
        if is_cjk(chars[index]) {
            let repeated = chars[index];
            index += 1;
            while index < chars.len() && chars[index] == repeated && is_cjk(chars[index]) {
                index += 1;
            }
        } else {
            index += 1;
            while index < chars.len()
                && !is_cjk(chars[index])
                && !latin_ranges
                    .get(latin_index)
                    .is_some_and(|range| range.start == index)
            {
                index += 1;
            }
        }
        units.push(AlignmentUnit {
            range: start..index,
            text: chars[start..index].iter().collect(),
        });
    }
    units
}

fn alignment_hunk_char_range(
    units: &[AlignmentUnit],
    hunk: std::ops::Range<usize>,
    char_count: usize,
) -> Option<std::ops::Range<usize>> {
    if hunk.start > units.len() || hunk.end > units.len() {
        return None;
    }
    if hunk.is_empty() {
        let offset = if hunk.start == 0 {
            0
        } else {
            units.get(hunk.start - 1)?.range.end
        };
        return (offset <= char_count).then_some(offset..offset);
    }
    Some(units.get(hunk.start)?.range.start..units.get(hunk.end - 1)?.range.end)
}

fn changed_words_cross_hunks(
    before: &[AlignmentUnit],
    after: &[AlignmentUnit],
    hunks: &[(std::ops::Range<usize>, std::ops::Range<usize>)],
) -> bool {
    for (index, (before_hunk, after_hunk)) in hunks.iter().enumerate() {
        for (other_index, (other_before, other_after)) in hunks.iter().enumerate() {
            if index == other_index {
                continue;
            }
            if before_hunk.clone().any(|unit_index| {
                is_word_alignment_unit(&before[unit_index])
                    && other_after.clone().any(|other_unit_index| {
                        before[unit_index].text == after[other_unit_index].text
                    })
            }) || after_hunk.clone().any(|unit_index| {
                is_word_alignment_unit(&after[unit_index])
                    && other_before.clone().any(|other_unit_index| {
                        after[unit_index].text == before[other_unit_index].text
                    })
            }) {
                return true;
            }
        }
    }
    false
}

fn is_word_alignment_unit(unit: &AlignmentUnit) -> bool {
    unit.text
        .chars()
        .next()
        .is_some_and(|ch| is_latin_start(ch) || is_cjk(ch))
}

trait RangeContainsRange {
    fn contains_range(&self, other: &std::ops::Range<usize>) -> bool;
}

impl RangeContainsRange for std::ops::Range<usize> {
    fn contains_range(&self, other: &std::ops::Range<usize>) -> bool {
        self.start <= other.start && other.end <= self.end
    }
}

fn make_word_corrections(
    mut ranges: Vec<WordCorrectionRange>,
) -> Option<Vec<DictionarySuggestion>> {
    ranges.sort_by_key(|range| (range.before.start, range.after.start));
    let mut merged: Vec<WordCorrectionRange> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(previous) = merged.last() {
            let before_overlap = range.before.start < previous.before.end;
            let after_overlap = range.after.start < previous.after.end;
            if before_overlap || after_overlap {
                if range.before == previous.before && range.after == previous.after {
                    continue;
                }
                return None;
            }
            if range.before.start < previous.before.start
                || range.after.start < previous.after.start
            {
                return None;
            }
        }
        merged.push(range);
    }
    if merged.is_empty() || merged.len() > MAX_WORD_CORRECTIONS {
        return None;
    }

    let mut corrections = Vec::with_capacity(merged.len());
    for range in merged {
        let key = pair_key(&range.before_surface, &range.after_surface);
        corrections.push(DictionarySuggestion {
            pair_key: key,
            before_span: range.before_surface,
            after: range.after_surface,
        });
    }
    (!corrections.is_empty()).then_some(corrections)
}

/// Finds a unique LCS alignment over lexical and separator units. Word-internal
/// spelling differences stay one unit, while repeated anchors remain ambiguous.
fn unique_alignment_hunks<T: PartialEq>(
    before: &[T],
    after: &[T],
) -> Option<Vec<(std::ops::Range<usize>, std::ops::Range<usize>)>> {
    let rows = before.len().checked_add(1)?;
    let columns = after.len().checked_add(1)?;
    let cells = rows.checked_mul(columns)?;
    let mut prefix = vec![0u16; cells];
    let mut suffix = vec![0u16; cells];
    let index = |row: usize, column: usize| row * columns + column;

    for i in 0..before.len() {
        for j in 0..after.len() {
            let value = if before[i] == after[j] {
                prefix[index(i, j)].saturating_add(1)
            } else {
                prefix[index(i + 1, j)].max(prefix[index(i, j + 1)])
            };
            prefix[index(i + 1, j + 1)] = value;
        }
    }
    for i in (0..before.len()).rev() {
        for j in (0..after.len()).rev() {
            let value = if before[i] == after[j] {
                suffix[index(i + 1, j + 1)].saturating_add(1)
            } else {
                suffix[index(i + 1, j)].max(suffix[index(i, j + 1)])
            };
            suffix[index(i, j)] = value;
        }
    }

    let lcs_len = usize::from(prefix[index(before.len(), after.len())]);
    if lcs_len > 0 {
        let mut by_rank = vec![None; lcs_len];
        for i in 0..before.len() {
            for j in 0..after.len() {
                if before[i] != after[j]
                    || usize::from(prefix[index(i, j)])
                        + 1
                        + usize::from(suffix[index(i + 1, j + 1)])
                        != lcs_len
                {
                    continue;
                }
                let rank = usize::from(prefix[index(i, j)]);
                let slot = by_rank.get_mut(rank)?;
                if slot.replace((i, j)).is_some() {
                    return None;
                }
            }
        }
        if by_rank.iter().any(Option::is_none) {
            return None;
        }
        let anchors: Vec<(usize, usize)> = by_rank.into_iter().flatten().collect();
        if anchors
            .windows(2)
            .any(|pair| pair[0].0 >= pair[1].0 || pair[0].1 >= pair[1].1)
        {
            return None;
        }
        let mut gaps = Vec::new();
        let (mut before_start, mut after_start) = (0, 0);
        for (before_match, after_match) in anchors {
            if before_start != before_match || after_start != after_match {
                gaps.push((before_start..before_match, after_start..after_match));
            }
            before_start = before_match + 1;
            after_start = after_match + 1;
        }
        if before_start != before.len() || after_start != after.len() {
            gaps.push((before_start..before.len(), after_start..after.len()));
        }
        Some(gaps)
    } else {
        Some(vec![(0..before.len(), 0..after.len())])
    }
}

fn correction_word_range(
    chars: &[char],
    changed: std::ops::Range<usize>,
) -> Option<std::ops::Range<usize>> {
    if changed.end > chars.len() {
        return None;
    }

    let latin_ranges = latin_word_ranges(chars);
    if changed.is_empty() {
        return None;
    }
    let mut latin_matches = latin_ranges
        .into_iter()
        .filter(|range| range.start < changed.end && changed.start < range.end);
    if let Some(range) = latin_matches.next() {
        if latin_matches.next().is_some() || !range.contains_range(&changed) {
            return None;
        }
        return Some(range);
    }

    if !chars[changed.clone()].iter().all(|ch| is_cjk(*ch)) {
        return None;
    }
    let mut range = changed;
    if range.len() == 1 {
        if range.start > 0 && is_cjk(chars[range.start - 1]) {
            range.start -= 1;
        } else if range.end < chars.len() && is_cjk(chars[range.end]) {
            range.end += 1;
        } else {
            return None;
        }
    }
    (2..=8).contains(&range.len()).then_some(range)
}

fn latin_word_ranges(chars: &[char]) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if !is_latin_start(chars[index]) {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        while index < chars.len() {
            let current = chars[index];
            let inside_word = current.is_ascii_alphanumeric() || matches!(current, '_' | '-');
            let internal_dot = current == '.'
                && index > start
                && index + 1 < chars.len()
                && chars[index - 1].is_ascii_alphanumeric()
                && chars[index + 1].is_ascii_alphanumeric();
            if inside_word || internal_dot {
                index += 1;
            } else {
                break;
            }
        }
        ranges.push(start..index);
    }
    ranges
}

fn safe_word_surface(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= 32
        && !value.chars().any(char::is_numeric)
        && !has_cjk_number_or_scale(value)
        && !is_blocked(value)
        && !contains_negation_marker(value)
}

fn unsafe_unicode_boundary(chars: &[char], range: &std::ops::Range<usize>) -> bool {
    let is_boundary_sensitive = |ch: char| {
        matches!(ch,
            '\u{0300}'..='\u{036F}'
                | '\u{1AB0}'..='\u{1AFF}'
                | '\u{1DC0}'..='\u{1DFF}'
                | '\u{20D0}'..='\u{20FF}'
                | '\u{FE20}'..='\u{FE2F}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{E0100}'..='\u{E01EF}'
                | '\u{1F3FB}'..='\u{1F3FF}'
                | '\u{200D}')
    };
    let is_unsupported_unicode_boundary = |ch: char| {
        !ch.is_ascii()
            && !ch.is_whitespace()
            && !is_cjk(ch)
            && !matches!(
                ch,
                '。' | '、'
                    | '，'
                    | '！'
                    | '？'
                    | '；'
                    | '：'
                    | '…'
                    | '—'
                    | '～'
                    | '「'
                    | '」'
                    | '『'
                    | '』'
                    | '（'
                    | '）'
                    | '【'
                    | '】'
                    | '《'
                    | '》'
                    | '〈'
                    | '〉'
            )
    };
    range.start > 0 && is_boundary_sensitive(chars[range.start - 1])
        || range.end < chars.len() && is_boundary_sensitive(chars[range.end])
        || chars[range.clone()]
            .iter()
            .copied()
            .any(is_boundary_sensitive)
        || range.start > 0 && is_unsupported_unicode_boundary(chars[range.start - 1])
        || range.end < chars.len() && is_unsupported_unicode_boundary(chars[range.end])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CjkNameEditBoundary {
    WholeName { start: usize, end: usize },
    Ambiguous,
}

/// Keep recognized same-sound names whole when their CJK run is bounded, and
/// abstain when a name-shaped window sits inside a longer unsegmented run.
fn cjk_name_edit_boundary(before: &str, after: &str) -> Option<CjkNameEditBoundary> {
    let before_chars: Vec<char> = before.chars().collect();
    let after_chars: Vec<char> = after.chars().collect();
    if before_chars.len() != after_chars.len() {
        return None;
    }

    let mut changed = 0;
    while changed < before_chars.len() && before_chars[changed] == after_chars[changed] {
        changed += 1;
    }
    if changed == before_chars.len()
        || !is_cjk(before_chars[changed])
        || !is_cjk(after_chars[changed])
    {
        return None;
    }
    let mut suffix = 0;
    while suffix < before_chars.len() - changed
        && before_chars[before_chars.len() - 1 - suffix]
            == after_chars[after_chars.len() - 1 - suffix]
    {
        suffix += 1;
    }
    if before_chars.len() - suffix != changed + 1 {
        return None;
    }

    let mut run_start = changed;
    while run_start > 0
        && before_chars[run_start - 1] == after_chars[run_start - 1]
        && is_cjk(before_chars[run_start - 1])
    {
        run_start -= 1;
    }
    let mut run_end = changed + 1;
    while run_end < before_chars.len()
        && before_chars[run_end] == after_chars[run_end]
        && is_cjk(before_chars[run_end])
    {
        run_end += 1;
    }

    let name_pair_matches = |start: usize, end: usize| {
        let before_name: String = before_chars[start..end].iter().collect();
        let after_name: String = after_chars[start..end].iter().collect();
        looks_like_cjk_personal_name(&after_name)
            && plain_pinyin_key(&before_name)
                .zip(plain_pinyin_key(&after_name))
                .is_some_and(|(left, right)| left == right)
    };

    let run_len = run_end - run_start;
    if (2..=3).contains(&run_len) && name_pair_matches(run_start, run_end) {
        return Some(CjkNameEditBoundary::WholeName {
            start: run_start,
            end: run_end,
        });
    }
    if run_len > 3 {
        for window_len in 2..=3 {
            if window_len > run_len {
                continue;
            }
            let first_start = run_start.max(changed.saturating_sub(window_len - 1));
            let last_start = changed.min(run_end - window_len);
            if first_start <= last_start
                && (first_start..=last_start)
                    .any(|start| name_pair_matches(start, start + window_len))
            {
                return Some(CjkNameEditBoundary::Ambiguous);
            }
        }
    }
    None
}

fn is_latin_token_char(value: char) -> bool {
    is_latin_start(value) || is_latin_cont(value)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictionarySuggestion {
    pub pair_key: String,
    pub before_span: String,
    pub after: String,
}

pub fn dictionary_suggestions(before: &str, after: &str) -> Vec<DictionarySuggestion> {
    let Some(corrections) = extract_word_corrections(before, after) else {
        return Vec::new();
    };
    let mut seen_before = HashSet::new();
    if corrections
        .iter()
        .any(|correction| !seen_before.insert(correction.before_span.as_str()))
    {
        return Vec::new();
    }
    if corrections.iter().any(|correction| {
        !dictionary_suggestion_pair_allowed(&correction.before_span, &correction.after)
    }) {
        return Vec::new();
    }
    corrections
}

fn dictionary_suggestion_pair_allowed(before: &str, after: &str) -> bool {
    let before_is_cjk = before.chars().all(is_cjk);
    let after_is_cjk = after.chars().all(is_cjk);
    safe_word_surface(before)
        && safe_word_surface(after)
        && token_key(before) != token_key(after)
        && (before_is_cjk || !after_is_cjk)
}

fn deduplicate_correction_pairs(
    corrections: Vec<DictionarySuggestion>,
) -> Vec<DictionarySuggestion> {
    let mut seen = HashSet::new();
    corrections
        .into_iter()
        .filter(|correction| seen.insert(correction.pair_key.clone()))
        .collect()
}

/// History text is a user-selected comparison, so automatic learning accepts
/// only one independently safe lexical correction from the complete edit.
/// A mixed edit or a multi-hunk edit remains available for explicit review but
/// cannot be silently learned through History.
fn history_lexical_correction(before: &str, after: &str) -> Option<DictionarySuggestion> {
    if before.trim().is_empty() || after.trim().is_empty() || before == after {
        return None;
    }
    let mut corrections = extract_word_corrections(before, after)?;
    if corrections.len() != 1 {
        return None;
    }
    let correction = corrections.pop()?;
    history_lexical_pair_allowed(&correction.before_span, &correction.after).then_some(correction)
}

fn history_lexical_pair_allowed(before: &str, after: &str) -> bool {
    let before_token = before.trim();
    let after_token = after.trim();
    if before_token.is_empty()
        || after_token.is_empty()
        || before_token == after_token
        || before_token.chars().any(char::is_numeric)
        || after_token.chars().any(char::is_numeric)
        || contains_negation_marker(before_token)
        || contains_negation_marker(after_token)
        || is_blocked(before_token)
        || is_blocked(after_token)
        || is_direct_negation_pair(before_token, after_token)
        || is_written_number_token(before_token)
        || is_written_number_token(after_token)
        || has_cjk_number_or_scale(before_token)
        || has_cjk_number_or_scale(after_token)
        || classify_learn_pair_decision(before_token, after_token).class == LearnClass::Rejected
    {
        return false;
    }
    let before_is_cjk = before_token.chars().all(is_cjk);
    let after_is_cjk = after_token.chars().all(is_cjk);
    if before_is_cjk != after_is_cjk {
        // Cross-script terms remain confirmable in History suggestions, but
        // they do not establish automatic lexical intent.
        return false;
    }
    if before_is_cjk {
        let same_sound = plain_pinyin_key(before_token)
            .zip(plain_pinyin_key(after_token))
            .is_some_and(|(left, right)| left == right);
        let max_len = before_token
            .chars()
            .count()
            .max(after_token.chars().count());
        same_sound
            || (bounded_edit_distance(before_token, after_token, 1) <= 1
                && bounded_edit_distance(before_token, after_token, 1) * 4 <= max_len)
    } else {
        if is_plain_action_word_correction(before_token, after_token)
            || (!has_strong_latin_identifier_shape(before_token)
                && !has_strong_latin_identifier_shape(after_token))
        {
            return false;
        }
        let left = before_token.to_lowercase();
        let right = after_token.to_lowercase();
        if is_simple_singular_plural_pair(&left, &right) {
            return false;
        }
        let max_len = left.chars().count().max(right.chars().count());
        let max_distance = if max_len >= 6 { 2 } else { 1 };
        bounded_edit_distance(&left, &right, max_distance) <= max_distance
            && bounded_edit_distance(&left, &right, max_distance) * 4 <= max_len
    }
}

fn automatic_word_pair_allowed(before: &str, after: &str) -> bool {
    history_lexical_pair_allowed(before, after)
}

fn is_plain_action_word_correction(before: &str, after: &str) -> bool {
    const COMMON_ACTION_WORDS: &[&str] = &[
        "accept",
        "activate",
        "archive",
        "build",
        "cancel",
        "clean",
        "clear",
        "close",
        "clone",
        "compile",
        "connect",
        "copy",
        "create",
        "delete",
        "deploy",
        "disable",
        "disconnect",
        "download",
        "edit",
        "enable",
        "export",
        "fetch",
        "find",
        "format",
        "import",
        "install",
        "launch",
        "load",
        "merge",
        "move",
        "open",
        "paste",
        "pull",
        "push",
        "read",
        "remove",
        "replace",
        "reply",
        "reset",
        "restore",
        "retry",
        "revert",
        "run",
        "save",
        "search",
        "select",
        "send",
        "share",
        "start",
        "stop",
        "submit",
        "sync",
        "undo",
        "update",
        "upload",
        "write",
    ];
    let before = before.to_ascii_lowercase();
    let after = after.to_ascii_lowercase();
    before != after
        && COMMON_ACTION_WORDS.contains(&before.as_str())
        && COMMON_ACTION_WORDS.contains(&after.as_str())
}

fn has_strong_latin_identifier_shape(value: &str) -> bool {
    let letters: Vec<char> = value.chars().filter(char::is_ascii_alphabetic).collect();
    if value.contains('_')
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return true;
    }
    let uppercase_count = letters.iter().filter(|ch| ch.is_ascii_uppercase()).count();
    let all_uppercase = uppercase_count == letters.len() && letters.len() >= 2;
    let internal_uppercase = letters.iter().skip(1).any(|ch| ch.is_ascii_uppercase());
    all_uppercase || internal_uppercase
}

fn is_simple_singular_plural_pair(left: &str, right: &str) -> bool {
    fn singular(value: &str) -> Option<String> {
        if value.len() >= 4 && value.ends_with("ies") {
            return Some(format!("{}y", &value[..value.len() - 3]));
        }
        if value.len() >= 4 && value.ends_with("es") {
            return Some(value[..value.len() - 2].to_owned());
        }
        if value.len() >= 3 && value.ends_with('s') && !value.ends_with("ss") {
            return Some(value[..value.len() - 1].to_owned());
        }
        None
    }

    singular(left).as_deref() == Some(right)
        || singular(right).as_deref() == Some(left)
        || singular(left).is_some_and(|left| singular(right).as_deref() == Some(left.as_str()))
}

fn has_cjk_number_or_scale(value: &str) -> bool {
    value.chars().any(|ch| {
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
                | '億'
                | '亿'
                | '兆'
                | '第'
                | '廿'
                | '卅'
                | '卌'
        )
    })
}

fn is_written_number_token(value: &str) -> bool {
    const NUMBER_PARTS: &[&str] = &[
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
        "thirty",
        "forty",
        "fifty",
        "sixty",
        "seventy",
        "eighty",
        "ninety",
        "hundred",
        "thousand",
        "million",
        "billion",
        "trillion",
        "quadrillion",
        "quintillion",
        "sextillion",
        "septillion",
        "octillion",
        "nonillion",
        "decillion",
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
        "eleventh",
        "twelfth",
        "thirteenth",
        "fourteenth",
        "fifteenth",
        "sixteenth",
        "seventeenth",
        "eighteenth",
        "nineteenth",
        "twentieth",
        "thirtieth",
        "fortieth",
        "fiftieth",
        "sixtieth",
        "seventieth",
        "eightieth",
        "ninetieth",
        "hundredth",
        "thousandth",
        "millionth",
        "billionth",
        "trillionth",
    ];

    let normalized = value.to_ascii_lowercase();
    if normalized.is_empty()
        || normalized.len() > 64
        || normalized == "and"
        || !normalized.bytes().all(|byte| byte.is_ascii_alphabetic())
    {
        return false;
    }
    let mut reachable = vec![false; normalized.len() + 1];
    reachable[0] = true;
    for offset in 0..normalized.len() {
        if !reachable[offset] {
            continue;
        }
        for part in NUMBER_PARTS {
            if normalized[offset..].starts_with(part) {
                reachable[offset + part.len()] = true;
            }
        }
    }
    reachable[normalized.len()]
}

fn plain_pinyin_key(value: &str) -> Option<String> {
    use pinyin::ToPinyin;
    let mut output = String::new();
    for ch in value.chars() {
        if let Some(pinyin) = ch.to_pinyin() {
            output.push_str(pinyin.plain());
        } else if is_cjk(ch) {
            return None;
        }
    }
    (!output.is_empty()).then_some(output)
}

fn is_direct_negation_pair(before: &str, after: &str) -> bool {
    const CJK_NEGATION_PREFIXES: [&str; 8] = ["不", "没", "無", "无", "未", "别", "非", "勿"];
    const LATIN_NEGATION_PREFIXES: [&str; 7] = ["un", "in", "im", "il", "ir", "non", "dis"];
    CJK_NEGATION_PREFIXES.iter().any(|prefix| {
        before.strip_prefix(prefix) == Some(after) || after.strip_prefix(prefix) == Some(before)
    }) || {
        let before = before.to_lowercase();
        let after = after.to_lowercase();
        LATIN_NEGATION_PREFIXES.iter().any(|prefix| {
            before.strip_prefix(prefix) == Some(after.as_str())
                || after.strip_prefix(prefix) == Some(before.as_str())
        })
    }
}

fn contains_negation_marker(value: &str) -> bool {
    let lower = value.to_lowercase();
    ["不", "没", "無", "无", "未", "别", "非", "勿"]
        .iter()
        .any(|marker| lower.contains(marker))
        || [
            "not", "no", "never", "cannot", "can't", "don't", "doesn't", "won't",
        ]
        .iter()
        .any(|marker| lower.trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '\'') == *marker)
}

fn bounded_edit_distance(left: &str, right: &str, limit: usize) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.len().abs_diff(right.len()) > limit {
        return limit.saturating_add(1);
    }
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];
    for (left_index, left_char) in left.iter().enumerate() {
        current[0] = left_index + 1;
        let mut row_minimum = current[0];
        for (right_index, right_char) in right.iter().enumerate() {
            let substitution_cost = if left_char == right_char { 0 } else { 1 };
            current[right_index + 1] = (previous[right_index + 1] + 1)
                .min(current[right_index] + 1)
                .min(previous[right_index] + substitution_cost);
            row_minimum = row_minimum.min(current[right_index + 1]);
        }
        if row_minimum > limit {
            return limit.saturating_add(1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

#[tauri::command]
pub(crate) fn suggest_dictionary_entries(
    state: State<'_, AppState>,
    before: String,
    after: String,
) -> Vec<DictionarySuggestion> {
    if !lock_recover(&state.settings).dictionary_learn_enabled {
        return Vec::new();
    }
    dictionary_suggestions(&before, &after)
}

#[tauri::command]
pub(crate) async fn add_dictionary_entries(
    app: tauri::AppHandle,
    words: Vec<String>,
) -> Result<store::SettingsView, String> {
    let mut audit = AuditAttempt::review(&app, "add_dictionary_entries");
    let result = mutate_dictionary_sources(&app, &mut audit, |_, settings| {
        add_manual_dictionary_words(settings, &words)?;
        Ok(false)
    })
    .await;
    audit.result(result)
}

#[tauri::command]
pub(crate) async fn remove_dictionary_word(
    app: tauri::AppHandle,
    word: String,
) -> Result<store::SettingsView, String> {
    let mut audit = AuditAttempt::review(&app, "remove_dictionary_word");
    let result = mutate_dictionary_sources(&app, &mut audit, |dir, settings| {
        let cancelled_pending_add = store::cancel_pending_dictionary_effects_for_word(dir, &word)
            .map_err(|error| error.to_string())?;
        settings.remove_dictionary_word(&word);
        Ok(cancelled_pending_add)
    })
    .await;
    audit.result(result)
}

#[tauri::command]
pub(crate) fn list_learn_pairs(
    app: tauri::AppHandle,
) -> Result<Vec<store::LearnPairRecord>, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let rows = store::list_learn_pairs(&dir).map_err(|error| error.to_string())?;
    Ok(pairs_for_dictionary_settings(rows))
}

#[tauri::command]
pub(crate) fn list_learned_term_usage(
    app: tauri::AppHandle,
) -> Result<Vec<store::LearnedTermUsage>, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    store::list_learned_term_usage(&dir).map_err(|error| error.to_string())
}

pub(crate) fn pairs_for_dictionary_settings(
    rows: Vec<store::LearnPairRecord>,
) -> Vec<store::LearnPairRecord> {
    rows.into_iter()
        .filter(|row| {
            if row.is_pending() {
                return true;
            }
            row.is_live_promoted() && !row.before_surface.is_empty()
        })
        .map(|mut row| {
            row.promote_hits =
                promote_hits_for(classify_learn_pair(&row.before_surface, &row.after_surface));
            row
        })
        .collect()
}

#[tauri::command]
pub(crate) async fn promote_learn_pair(
    app: tauri::AppHandle,
    pair_key: String,
    before_surface: Option<String>,
    after_surface: Option<String>,
    history_id: Option<i64>,
) -> Result<store::SettingsView, String> {
    let mut audit = AuditAttempt::review(&app, "confirm_pair");
    let result = async {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())?;
        let state = app.state::<AppState>();
        let _gate = state.settings_gate.lock().await;
        let row = store::get_learn_pair(&dir, &pair_key).map_err(|error| error.to_string())?;
        if let Some(row) = &row {
            audit.pair(row);
        }
        let before = row
            .as_ref()
            .map(|item| item.before_surface.clone())
            .or(before_surface)
            .ok_or_else(|| "learn pair was not found".to_string())?;
        let after = row
            .as_ref()
            .map(|item| item.after_surface.clone())
            .or(after_surface)
            .ok_or_else(|| "learn pair was not found".to_string())?;
        let scope = if let Some(id) = history_id {
            let scene = store::history_scene(&dir, id).map_err(|error| error.to_string())?;
            audit.history_reference(true);
            scene.learn_scope()
        } else {
            row.as_ref()
                .map(|item| store::LearnPairScope {
                    family: item.family.clone(),
                    mapping_id: item.mapping_id.clone(),
                    browser_host: item.browser_host.clone(),
                    native_bundle: item.native_bundle.clone(),
                })
                .ok_or_else(|| "a known application scope is required".to_string())?
        };
        if confirm_scope_is_unknown(&scope) {
            return Err(
                "this correction has no known application scope; add a scoped rule instead".into(),
            );
        }
        if row.as_ref().is_some_and(|item| {
            item.family != scope.family
                || item.mapping_id != scope.mapping_id
                || item.browser_host != scope.browser_host
                || item.native_bundle != scope.native_bundle
        }) {
            return Err("the selected history scope does not match the recorded correction".into());
        }
        let global_dictionary_word = learned_rule_kind(&pair_key) == LearnedRuleKind::Word
            && !before.trim().is_empty()
            && !after.trim().is_empty()
            && row
                .as_ref()
                .is_none_or(|item| !item.manual_scoped && !item.auto_scoped);
        let mut snapshot = global_dictionary_word.then(|| lock_recover(&state.settings).clone());
        let settings_were_updated = snapshot.is_some();
        if let Some(settings) = snapshot.as_ref() {
            require_dictionary_confirmation_capacity(&dir, settings, &after)?;
        }
        audit
            .candidate_result(
                store::ensure_learn_pair_promoted(&dir, &pair_key, &before, &after, Some(&scope)),
                "db_promotion_recorded",
            )
            .map_err(|error| error.to_string())?;
        let settings_saved = if let Some(snapshot) = snapshot.as_mut() {
            let settings_save =
                save_settings_with_dictionary_effects(&dir, snapshot, true, true, |settings| {
                    store::save_settings(&dir, settings)
                });
            let outcome = match settings_save {
                Ok(outcome) => outcome,
                Err(error) => {
                    let _ = app.emit("learn_pairs://changed", ());
                    audit
                        .settings_result(Err::<(), anyhow::Error>(anyhow::anyhow!(error.clone())))
                        .map_err(|audit_error| audit_error.to_string())?;
                    return Err(error);
                }
            };
            if outcome.saved {
                crate::commit_learning_settings_snapshot(&state, snapshot.clone());
                let _ = app.emit("settings://changed", store::SettingsView::from(&*snapshot));
            }
            if let Some(error) = outcome.sync_error {
                let _ = app.emit("learn_pairs://changed", ());
                audit
                    .settings_result(Err::<(), anyhow::Error>(anyhow::anyhow!(error.clone())))
                    .map_err(|audit_error| audit_error.to_string())?;
                return Err(error);
            }
            outcome.saved
        } else {
            true
        };
        let _ = app.emit("learn_pairs://changed", ());
        if settings_were_updated && settings_saved {
            audit
                .settings_result(Ok::<(), anyhow::Error>(()))
                .map_err(|error| error.to_string())?;
        }
        if !settings_saved {
            return Err("dictionary Settings update was not persisted".into());
        }
        let view = snapshot
            .as_ref()
            .map(store::SettingsView::from)
            .unwrap_or_else(|| store::SettingsView::from(&lock_recover(&state.settings).clone()));
        audit.existing_pair_key(&pair_key);
        let _ = app.emit(
            "learn_pairs://promoted",
            serde_json::json!({
                "pair_key": pair_key,
                "before": before,
                "after": after,
            }),
        );
        crate::island_window::set_learn_toast_interactive(&app, true);
        Ok(view)
    }
    .await;
    audit.result(result)
}

fn require_dictionary_confirmation_capacity(
    dir: &std::path::Path,
    settings: &store::Settings,
    word: &str,
) -> Result<(), String> {
    let effects = store::list_pending_dictionary_effects(dir).map_err(|error| error.to_string())?;
    let mut projected = settings.clone();
    if apply_pending_dictionary_effects(&mut projected, &effects).is_err() {
        return Err(format!(
            "{DICTIONARY_SYNC_PARTIAL_ERROR}: dictionary capacity or word validation"
        ));
    }
    projected
        .can_add_dictionary_source(word)
        .then_some(())
        .ok_or_else(|| "dictionary_capacity_or_invalid_word".into())
}

fn scope_is_unknown(scope: &store::LearnPairScope) -> bool {
    scope
        .family
        .as_deref()
        .is_some_and(|family| !family.is_empty())
        && scope.mapping_id.as_deref().is_none_or(str::is_empty)
        && scope.browser_host.as_deref().is_none_or(str::is_empty)
        && scope.native_bundle.as_deref().is_none_or(str::is_empty)
}

fn confirm_scope_is_unknown(scope: &store::LearnPairScope) -> bool {
    [
        scope.mapping_id.as_deref(),
        scope.browser_host.as_deref(),
        scope.native_bundle.as_deref(),
    ]
    .iter()
    .all(|id| id.is_none_or(str::is_empty))
}

#[tauri::command]
pub(crate) async fn add_manual_scoped_learn_pair(
    app: tauri::AppHandle,
    before_surface: String,
    after_surface: String,
    bundle_id: String,
) -> Result<(), String> {
    let mut audit = AuditAttempt::review(&app, "add_manual_scoped_pair");
    let result = async {
        let before = before_surface.trim();
        let after = after_surface.trim();
        let bundle_id = bundle_id.trim();
        if before.is_empty()
            || after.is_empty()
            || before == after
            || before.len() > 256
            || after.len() > 256
            || before.chars().any(char::is_control)
            || after.chars().any(char::is_control)
        {
            return Err("enter two different terms, each up to 256 characters".to_string());
        }
        if bundle_id.is_empty()
            || context::is_browser_application(Some(bundle_id))
            || !manual_learn_applications()
                .iter()
                .any(|application| application.bundle_id == bundle_id)
        {
            return Err("choose a currently available native application".to_string());
        }
        let scope = store::LearnPairScope {
            native_bundle: Some(bundle_id.to_owned()),
            ..store::LearnPairScope::default()
        };
        let key = pair_key(before, after);
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())?;
        let persistence = store::ensure_manual_scoped_learn_pair(&dir, &key, before, after, &scope);
        let persistence_reason = match &persistence {
            Err(store::ManualScopedPairStoreError::ScopeConflict) => {
                "manual_scoped_pair_scope_conflict"
            }
            Err(store::ManualScopedPairStoreError::InvalidScope) => {
                "manual_scoped_pair_scope_unknown"
            }
            Err(store::ManualScopedPairStoreError::Storage(_)) => {
                "manual_scoped_pair_storage_failed"
            }
            Ok(_) => "manual_scoped_pair_saved",
        };
        let row = audit
            .candidate_result(
                persistence.map_err(|error| error.code().to_owned()),
                persistence_reason,
            )
            .map_err(|error| error.to_string())?;
        audit.pair(&row);
        let _ = app.emit("learn_pairs://changed", ());
        Ok(())
    }
    .await;
    audit.result(result)
}

#[tauri::command]
pub(crate) async fn ignore_learn_pair(
    app: tauri::AppHandle,
    pair_key: String,
) -> Result<(), String> {
    let mut audit = AuditAttempt::review(&app, "ignore_pair");
    let result = async {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())?;
        let changed = audit
            .candidate_result(
                store::tombstone_learn_pair(&dir, &pair_key),
                "tombstone_or_ignored",
            )
            .map_err(|error| error.to_string())?;
        if changed {
            audit.existing_pair_key(&pair_key);
        }
        let _ = app.emit("learn_pairs://changed", ());
        Ok(())
    }
    .await;
    audit.result(result)
}

#[tauri::command]
pub(crate) async fn undo_learn_pair(
    app: tauri::AppHandle,
    pair_key: String,
) -> Result<store::SettingsView, String> {
    let mut audit = AuditAttempt::review(&app, "undo_pair");
    let result = async {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())?;
        let row = store::get_learn_pair(&dir, &pair_key)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "learn pair was not found".to_string())?;
        let is_word_rule = learned_rule_kind(&pair_key) == LearnedRuleKind::Word;
        let state = app.state::<AppState>();
        let _word_settings_gate = if is_word_rule {
            Some(state.settings_gate.lock().await)
        } else {
            None
        };
        audit
            .candidate_result(
                store::tombstone_learn_pair(&dir, &pair_key),
                "tombstone_or_ignored",
            )
            .map_err(|error| error.to_string())?;
        let _ = app.emit("learn_pairs://changed", ());
        audit.pair(&row);
        if !is_word_rule {
            restore_mapping_learn(&app, &dir, &row, &mut audit).await?;
        }
        let view = if is_word_rule {
            let mut snapshot = lock_recover(&state.settings).clone();
            let outcome = match save_settings_with_dictionary_effects(
                &dir,
                &mut snapshot,
                true,
                false,
                |settings| store::save_settings(&dir, settings),
            ) {
                Ok(saved) => saved,
                Err(error) => {
                    audit
                        .settings_result(Err::<(), anyhow::Error>(anyhow::anyhow!(error.clone())))
                        .map_err(|audit_error| audit_error.to_string())?;
                    return Err(error);
                }
            };
            if outcome.saved {
                crate::commit_learning_settings_snapshot(&state, snapshot.clone());
                let _ = app.emit("settings://changed", store::SettingsView::from(&snapshot));
            }
            if let Some(error) = outcome.sync_error {
                audit
                    .settings_result(Err::<(), anyhow::Error>(anyhow::anyhow!(error.clone())))
                    .map_err(|audit_error| audit_error.to_string())?;
                return Err(error);
            }
            if outcome.saved {
                audit
                    .settings_result(Ok::<(), anyhow::Error>(()))
                    .map_err(|error| error.to_string())?;
                store::SettingsView::from(&snapshot)
            } else {
                store::SettingsView::from(&lock_recover(&state.settings).clone())
            }
        } else {
            store::SettingsView::from(&lock_recover(&state.settings).clone())
        };
        Ok(view)
    }
    .await;
    audit.result(result)
}

pub(crate) fn should_remove_dictionary_entry_after_undo(
    row: &store::LearnPairRecord,
    rows: &[store::LearnPairRecord],
) -> bool {
    !row.manual_scoped
        && !row.auto_scoped
        && !has_other_live_promoted_pair(rows, &row.pair_key, &row.after_surface)
}

fn has_other_live_promoted_pair(
    rows: &[store::LearnPairRecord],
    removed_key: &str,
    after: &str,
) -> bool {
    rows.iter().any(|row| {
        row.pair_key != removed_key
            && row.is_live_promoted()
            && !row.manual_scoped
            && !row.auto_scoped
            && store::dictionary_words_equal(&row.after_surface, after)
    })
}

fn remove_style_pair(pairs: &mut Vec<context::StyleExamplePair>, input: &str, output: &str) {
    if let Some(index) = pairs
        .iter()
        .rposition(|pair| pair.input == input && pair.output == output)
    {
        pairs.remove(index);
    }
}

fn confirm_style_example(mapping: &mut context::AppMapping, input: &str, output: &str) {
    if !mapping.style_examples_approved {
        mapping.style_example_pairs.clear();
    }
    push_style_pair(
        &mut mapping.style_example_pairs,
        input.to_owned(),
        output.to_owned(),
    );
    mapping.style_example_input = Some(input.to_owned());
    mapping.style_example_output = Some(output.to_owned());
    mapping.style_examples_approved = true;
}

async fn restore_mapping_learn(
    app: &tauri::AppHandle,
    dir: &std::path::Path,
    row: &store::LearnPairRecord,
    audit: &mut AuditAttempt,
) -> Result<(), String> {
    let mapping_id = row
        .mapping_id
        .clone()
        .or_else(|| {
            row.pair_key
                .rsplit(':')
                .next()
                .filter(|part| !part.is_empty())
                .map(str::to_owned)
        })
        .ok_or_else(|| "mapping was not found".to_string())?;
    let state = app.state::<AppState>();
    let _gate = state.settings_gate.lock().await;
    let mut snapshot = lock_recover(&state.settings).clone();
    let Some(mapping) = snapshot
        .context_mappings
        .iter_mut()
        .find(|mapping| mapping.id == mapping_id)
    else {
        return Ok(());
    };
    if learned_rule_kind(&row.pair_key) == LearnedRuleKind::Style {
        remove_style_pair(
            &mut mapping.style_example_pairs,
            &row.before_surface,
            &row.after_surface,
        );
    } else if learned_rule_kind(&row.pair_key) == LearnedRuleKind::Intensity {
        mapping.cleanup_intensity = if row.before_surface.trim().is_empty() {
            None
        } else {
            crate::llm::CleanupIntensity::parse(&row.before_surface)
        };
    }
    snapshot.normalize();
    audit
        .settings_result(store::save_settings(dir, &snapshot))
        .map_err(|error| error.to_string())?;
    crate::commit_learning_settings_snapshot(&state, snapshot.clone());
    {
        let mut context = lock_recover(&state.context);
        context.mappings = snapshot.context_mappings.clone();
    }
    let _ = app.emit("settings://changed", store::SettingsView::from(&snapshot));
    Ok(())
}

#[tauri::command]
pub(crate) fn list_pinned_terms(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let rows = store::list_learn_pairs(&dir).map_err(|error| error.to_string())?;
    Ok(rows
        .into_iter()
        .filter(|row| row.pinned && row.is_live_promoted())
        .map(|row| row.after_surface)
        .collect())
}

#[tauri::command]
pub(crate) async fn pin_dictionary_term(
    app: tauri::AppHandle,
    word: String,
    pinned: bool,
) -> Result<(), String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let result = store::ensure_pinned_dictionary_term(&dir, &word, pinned);
    audit::invalidate(&app);
    result.map_err(|error| error.to_string())?;
    let _ = app.emit("learn_pairs://changed", ());
    Ok(())
}

#[tauri::command]
pub(crate) fn list_style_drafts(
    app: tauri::AppHandle,
) -> Result<Vec<store::StyleDraftRecord>, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    store::list_style_drafts(&dir).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn confirm_style_draft(
    app: tauri::AppHandle,
    draft_key: String,
) -> Result<store::SettingsView, String> {
    let mut audit = AuditAttempt::review(&app, "confirm_style_draft");
    let result = async {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())?;
        let draft = store::get_style_draft(&dir, &draft_key)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "style draft was not found".to_string())?;
        let state = app.state::<AppState>();
        let _gate = state.settings_gate.lock().await;
        let mut snapshot = lock_recover(&state.settings).clone();
        if !snapshot.dictionary_learn_enabled || !snapshot.context_enabled {
            return Err("Dictionary learning is disabled".into());
        }
        if !snapshot
            .context_mappings
            .iter()
            .any(|mapping| mapping.id == draft.mapping_id)
        {
            let created = mapping_from_style_draft_id(&draft.mapping_id)
                .ok_or_else(|| "mapping was not found".to_string())?;
            snapshot.context_mappings.push(created);
        }
        let mapping = snapshot
            .context_mappings
            .iter_mut()
            .find(|mapping| mapping.id == draft.mapping_id)
            .ok_or_else(|| "mapping was not found".to_string())?;
        if !mapping.enabled || !mapping.dictionary_learn_enabled {
            return Err("App style learning is disabled".into());
        }
        if !(mapping.source_permissions.ax_text || snapshot.local_edit_learning_enabled) {
            return Err(
                "Accessibility text permission is required to adopt this style draft".into(),
            );
        }
        if lexicon::is_default_learn_off_target(
            mapping.bundle_id.as_deref(),
            mapping.browser_host.as_deref(),
        ) {
            return Err("Style learning is disabled for this protected App or site".into());
        }
        confirm_style_example(mapping, &draft.before_excerpt, &draft.after_excerpt);
        snapshot.normalize();
        audit
            .settings_result(store::save_settings(&dir, &snapshot))
            .map_err(|error| error.to_string())?;
        crate::commit_learning_settings_snapshot(&state, snapshot.clone());
        {
            let mut context = lock_recover(&state.context);
            context.mappings = snapshot.context_mappings.clone();
        }
        audit
            .candidate_result(
                store::delete_style_draft(&dir, &draft_key),
                "style_draft_returned",
            )
            .map_err(|error| error.to_string())?;
        let view = store::SettingsView::from(&snapshot);
        let _ = app.emit("settings://changed", view.clone());
        let _ = app.emit("style_drafts://changed", ());
        Ok(view)
    }
    .await;
    audit.result(result)
}

#[tauri::command]
pub(crate) async fn dismiss_style_draft(
    app: tauri::AppHandle,
    draft_key: String,
) -> Result<(), String> {
    let mut audit = AuditAttempt::review(&app, "dismiss_style_draft");
    let result = async {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())?;
        audit
            .candidate_result(
                store::delete_style_draft(&dir, &draft_key),
                "style_draft_returned",
            )
            .map_err(|error| error.to_string())?;
        let _ = app.emit("style_drafts://changed", ());
        Ok(())
    }
    .await;
    audit.result(result)
}

async fn mutate_dictionary_sources(
    app: &tauri::AppHandle,
    audit: &mut AuditAttempt,
    mutate: impl FnOnce(&std::path::Path, &mut store::Settings) -> Result<bool, String>,
) -> Result<store::SettingsView, String> {
    let state = app.state::<AppState>();
    let _gate = state.settings_gate.lock().await;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let mut snapshot = lock_recover(&state.settings).clone();
    let database_change_committed = mutate(&dir, &mut snapshot)?;
    let outcome = match save_settings_with_dictionary_effects(
        &dir,
        &mut snapshot,
        database_change_committed,
        true,
        |settings| store::save_settings(&dir, settings),
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            audit
                .settings_result(Err::<(), anyhow::Error>(anyhow::anyhow!(error.clone())))
                .map_err(|audit_error| audit_error.to_string())?;
            return Err(error);
        }
    };
    let saved = outcome.saved;
    let sync_error = outcome.sync_error;
    if saved {
        crate::commit_learning_settings_snapshot(&state, snapshot.clone());
    }
    let view = if saved {
        store::SettingsView::from(&snapshot)
    } else {
        store::SettingsView::from(&lock_recover(&state.settings).clone())
    };
    if saved {
        let _ = app.emit("settings://changed", view.clone());
    }
    if let Some(error) = sync_error {
        audit
            .settings_result(Err::<(), anyhow::Error>(anyhow::anyhow!(error.clone())))
            .map_err(|audit_error| audit_error.to_string())?;
        return Err(error);
    }
    audit
        .settings_result(Ok::<(), anyhow::Error>(()))
        .map_err(|error| error.to_string())?;
    if !saved {
        return Err("dictionary Settings update was not persisted".into());
    }
    Ok(view)
}

#[cfg(test)]
fn mutate_dictionary_settings(
    state: &AppState,
    dir: &std::path::Path,
    mutate: impl FnOnce(&mut Vec<String>),
) -> Result<store::Settings, String> {
    let mut snapshot = lock_recover(&state.settings).clone();
    mutate(&mut snapshot.dictionary);
    snapshot.normalize();
    store::save_settings(dir, &snapshot).map_err(|error| error.to_string())?;
    crate::commit_learning_settings_snapshot(state, snapshot.clone());
    Ok(snapshot)
}

const OBSERVE_INITIAL: Duration = Duration::from_millis(3000);
const OBSERVE_INTERVAL: Duration = Duration::from_millis(400);
const OBSERVE_IDLE_GRACE: Duration = Duration::from_millis(1500);
const OBSERVE_MAX_TOTAL: Duration = Duration::from_millis(12000);

pub(crate) const DICTIONARY_SETTINGS_PARTIAL_ERROR: &str =
    "learning_dictionary_partial:settings_save_failed";
pub(crate) const DICTIONARY_ACK_PARTIAL_ERROR: &str =
    "learning_dictionary_partial:pending_ack_failed";
pub(crate) const DICTIONARY_SYNC_PARTIAL_ERROR: &str =
    "learning_dictionary_partial:pending_sync_failed";

pub(crate) struct DictionarySettingsSaveOutcome {
    pub(crate) saved: bool,
    pub(crate) sync_error: Option<String>,
}

fn apply_pending_dictionary_effects(
    settings: &mut store::Settings,
    effects: &[store::PendingDictionaryEffect],
) -> Result<(), ()> {
    for effect in effects
        .iter()
        .filter(|effect| effect.action == store::DictionaryEffectAction::Remove)
    {
        settings.remove_dictionary_source(
            &effect.word,
            &store::DictionarySource::Rule(effect.pair_key.clone()),
        );
    }
    for effect in effects
        .iter()
        .filter(|effect| effect.action == store::DictionaryEffectAction::Add)
    {
        let source = store::DictionarySource::Rule(effect.pair_key.clone());
        let already_owned = settings.has_dictionary_source(&effect.word, &source);
        if !already_owned && !settings.add_dictionary_source(&effect.word, source) {
            return Err(());
        }
    }
    Ok(())
}

/// Persist one Settings snapshot while consuming the finite, versioned rule
/// effects written by the History owner. Callers hold `settings_gate`; a
/// failed Settings write leaves the effect unacknowledged for startup or the
/// next gated dictionary mutation to replay.
pub(crate) fn save_settings_with_dictionary_effects(
    dir: &std::path::Path,
    settings: &mut store::Settings,
    database_change_committed: bool,
    save_even_without_effects: bool,
    mut save: impl FnMut(&store::Settings) -> anyhow::Result<()>,
) -> Result<DictionarySettingsSaveOutcome, String> {
    let mut retrying_changed_effect = false;
    let mut previously_saved_settings = None;
    for attempt in 0..2 {
        let effects = match store::list_pending_dictionary_effects(dir) {
            Ok(effects) => effects,
            Err(error) => {
                if let Some(saved_settings) = previously_saved_settings.take() {
                    *settings = saved_settings;
                    return Ok(DictionarySettingsSaveOutcome {
                        saved: true,
                        sync_error: Some(format!("{DICTIONARY_SYNC_PARTIAL_ERROR}: {error}")),
                    });
                }
                return Err(if database_change_committed {
                    format!("{DICTIONARY_SETTINGS_PARTIAL_ERROR}: {error}")
                } else {
                    error.to_string()
                });
            }
        };
        if effects.is_empty() && !save_even_without_effects && !retrying_changed_effect {
            return Ok(DictionarySettingsSaveOutcome {
                saved: false,
                sync_error: None,
            });
        }
        if apply_pending_dictionary_effects(settings, &effects).is_err() {
            if let Some(saved_settings) = previously_saved_settings.take() {
                *settings = saved_settings;
                return Ok(DictionarySettingsSaveOutcome {
                    saved: true,
                    sync_error: Some(format!(
                        "{DICTIONARY_SYNC_PARTIAL_ERROR}: dictionary capacity or word validation"
                    )),
                });
            }
            return Err(if database_change_committed {
                format!(
                    "{DICTIONARY_SETTINGS_PARTIAL_ERROR}: dictionary capacity or word validation"
                )
            } else {
                "dictionary capacity or word validation".into()
            });
        }
        settings.normalize();
        if let Err(error) = save(settings) {
            if let Some(saved_settings) = previously_saved_settings.take() {
                *settings = saved_settings;
                return Ok(DictionarySettingsSaveOutcome {
                    saved: true,
                    sync_error: Some(format!("{DICTIONARY_SYNC_PARTIAL_ERROR}: {error}")),
                });
            }
            return Err(if database_change_committed {
                format!("{DICTIONARY_SETTINGS_PARTIAL_ERROR}: {error}")
            } else {
                error.to_string()
            });
        }
        previously_saved_settings = Some(settings.clone());
        if effects.is_empty() {
            return Ok(DictionarySettingsSaveOutcome {
                saved: true,
                sync_error: retrying_changed_effect.then(|| {
                    format!("{DICTIONARY_ACK_PARTIAL_ERROR}: effect was cancelled during sync")
                }),
            });
        }
        let mut version_changed = false;
        for effect in &effects {
            match store::acknowledge_dictionary_effect(dir, &effect.pair_key, effect.version) {
                Ok(true) => {}
                Ok(false) => version_changed = true,
                Err(error) => {
                    return Ok(DictionarySettingsSaveOutcome {
                        saved: true,
                        sync_error: Some(format!("{DICTIONARY_ACK_PARTIAL_ERROR}: {error}")),
                    });
                }
            }
        }
        if !version_changed {
            return Ok(DictionarySettingsSaveOutcome {
                saved: true,
                sync_error: None,
            });
        }
        retrying_changed_effect = true;
        if attempt == 1 {
            return Ok(DictionarySettingsSaveOutcome {
                saved: true,
                sync_error: Some(format!(
                    "{DICTIONARY_ACK_PARTIAL_ERROR}: pending effect version kept changing"
                )),
            });
        }
    }
    Ok(DictionarySettingsSaveOutcome {
        saved: previously_saved_settings.is_some(),
        sync_error: Some(format!(
            "{DICTIONARY_ACK_PARTIAL_ERROR}: reconciliation did not settle"
        )),
    })
}

fn add_manual_dictionary_words(
    settings: &mut store::Settings,
    words: &[String],
) -> Result<(), String> {
    let mut candidate = settings.clone();
    let manual = store::DictionarySource::Manual;
    for word in words {
        let word = word.trim();
        if word.is_empty() || candidate.has_dictionary_source(word, &manual) {
            continue;
        }
        if word.len() > 256 {
            return Err("dictionary word exceeds 256 UTF-8 bytes".into());
        }
        if !candidate.add_dictionary_source(word, manual.clone()) {
            return Err("dictionary capacity of 256 words reached".into());
        }
    }
    *settings = candidate;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObserveLimits {
    pub initial: Duration,
    pub interval: Duration,
    pub idle_grace: Duration,
    pub max_total: Duration,
}

impl Default for ObserveLimits {
    fn default() -> Self {
        Self {
            initial: OBSERVE_INITIAL,
            interval: OBSERVE_INTERVAL,
            idle_grace: OBSERVE_IDLE_GRACE,
            max_total: OBSERVE_MAX_TOTAL,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObserveOutcome {
    Unchanged,
    LeftTarget,
    SingleToken {
        before_span: String,
        after: String,
    },
    MultipleTokens {
        before: String,
        after: String,
        corrections: Vec<DictionarySuggestion>,
    },
    ShortStyleRewrite {
        before: String,
        after: String,
    },
    StyleSignal {
        excerpt: String,
        style_key: String,
        before_excerpt: String,
        after_excerpt: String,
    },
    Ambiguous,
}

#[cfg(test)]
pub fn observe_after_paste(
    post_insert_field: &str,
    expected_target: &context::TargetAppGuard,
    dictionary_learn_enabled: bool,
    limits: ObserveLimits,
    read_value: impl FnMut() -> Option<String>,
    read_target: impl FnMut() -> context::TargetAppGuard,
    sleep: impl FnMut(Duration),
) -> ObserveOutcome {
    observe_after_paste_with_identity(
        post_insert_field,
        post_insert_field,
        expected_target,
        ObserveOptions {
            dictionary_learn_enabled,
            require_browser_identity: false,
            inserted_range_after: None,
            limits,
        },
        read_value,
        read_target,
        sleep,
        || true,
        || true,
    )
}

struct ObserveOptions {
    dictionary_learn_enabled: bool,
    require_browser_identity: bool,
    inserted_range_after: Option<crate::paste::Utf16RangeV1>,
    limits: ObserveLimits,
}

pub(crate) fn invalidate_learning_observers(epoch: &AtomicU64) -> u64 {
    epoch.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
}

fn learning_observer_is_current(epoch: &AtomicU64, expected: u64) -> bool {
    epoch.load(Ordering::Acquire) == expected
}

fn learning_observer_may_register(
    epoch: &AtomicU64,
    expected_epoch: u64,
    cancelled: bool,
    originating_session_is_current: bool,
) -> bool {
    !cancelled
        && originating_session_is_current
        && learning_observer_is_current(epoch, expected_epoch)
}

#[allow(clippy::too_many_arguments)]
fn observe_after_paste_with_identity(
    post_insert_field: &str,
    inserted_text: &str,
    expected_target: &context::TargetAppGuard,
    options: ObserveOptions,
    mut read_value: impl FnMut() -> Option<String>,
    mut read_target: impl FnMut() -> context::TargetAppGuard,
    mut sleep: impl FnMut(Duration),
    mut same_native_field: impl FnMut() -> bool,
    mut keep_watching: impl FnMut() -> bool,
) -> ObserveOutcome {
    let ObserveOptions {
        dictionary_learn_enabled,
        require_browser_identity,
        inserted_range_after,
        limits,
    } = options;
    if !dictionary_learn_enabled
        || expected_target.secure_input
        || post_insert_field.is_empty()
        || inserted_text.is_empty()
        || context::same_field_mismatch_reason(
            expected_target,
            expected_target,
            require_browser_identity,
        )
        .is_some()
    {
        return ObserveOutcome::Unchanged;
    }

    observe_after_paste_samples(
        post_insert_field,
        inserted_text,
        inserted_range_after,
        limits,
        ObserveSampleCallbacks {
            read_value: &mut read_value,
            sleep: &mut sleep,
            keep_watching: &mut keep_watching,
            same_field_is_current: || {
                let current = read_target();
                context::same_field_mismatch_reason(
                    expected_target,
                    &current,
                    require_browser_identity,
                )
                .is_none()
                    && same_native_field()
            },
            #[cfg(test)]
            end_of_input: None,
        },
    )
}

fn inserted_segment_for_observation(
    baseline_field: &str,
    inserted_text: &str,
    current_field: &str,
    inserted_range_after: Option<crate::paste::Utf16RangeV1>,
) -> Option<String> {
    let Some(range) = inserted_range_after else {
        // Only legacy observer unit tests use substring location. Production
        // and controlled fixtures must supply a UTF-16 range owner provided.
        #[cfg(test)]
        {
            return edited_inserted_segment(baseline_field, inserted_text, current_field);
        }
        #[cfg(not(test))]
        {
            return None;
        }
    };
    if inserted_text.is_empty() {
        return None;
    }
    let start = usize::try_from(range.start).ok()?;
    let length = usize::try_from(range.length).ok()?;
    let end = start.checked_add(length)?;
    let baseline_units = baseline_field.encode_utf16().collect::<Vec<_>>();
    let baseline_segment = baseline_units.get(start..end)?;
    if baseline_segment != inserted_text.encode_utf16().collect::<Vec<_>>() {
        return None;
    }
    let prefix = baseline_units.get(..start)?;
    let suffix = baseline_units.get(end..)?;
    let current_units = current_field.encode_utf16().collect::<Vec<_>>();
    let fixed_surroundings_length = prefix.len().checked_add(suffix.len())?;
    if current_units.len() < fixed_surroundings_length
        || !current_units.starts_with(prefix)
        || !current_units.ends_with(suffix)
    {
        return None;
    }
    let current_end = current_units.len().checked_sub(suffix.len())?;
    String::from_utf16(current_units.get(prefix.len()..current_end)?).ok()
}

struct ObserveSampleCallbacks<ReadValue, Sleep, KeepWatching, SameFieldIsCurrent> {
    read_value: ReadValue,
    sleep: Sleep,
    keep_watching: KeepWatching,
    same_field_is_current: SameFieldIsCurrent,
    #[cfg(test)]
    end_of_input: Option<Box<dyn FnMut() -> bool>>,
}

#[cfg(test)]
struct ObserveSampleCapture {
    outcome: ObserveOutcome,
    settled_field: Option<String>,
}

fn observe_after_paste_samples<ReadValue, Sleep, KeepWatching, SameFieldIsCurrent>(
    post_insert_field: &str,
    inserted_text: &str,
    inserted_range_after: Option<crate::paste::Utf16RangeV1>,
    limits: ObserveLimits,
    callbacks: ObserveSampleCallbacks<ReadValue, Sleep, KeepWatching, SameFieldIsCurrent>,
) -> ObserveOutcome
where
    ReadValue: FnMut() -> Option<String>,
    Sleep: FnMut(Duration),
    KeepWatching: FnMut() -> bool,
    SameFieldIsCurrent: FnMut() -> bool,
{
    observe_after_paste_samples_with_hook(
        post_insert_field,
        inserted_text,
        inserted_range_after,
        limits,
        callbacks,
        |_, _| {},
    )
}

#[cfg(test)]
fn observe_after_paste_samples_capture<ReadValue, Sleep, KeepWatching, SameFieldIsCurrent>(
    post_insert_field: &str,
    inserted_text: &str,
    inserted_range_after: Option<crate::paste::Utf16RangeV1>,
    limits: ObserveLimits,
    callbacks: ObserveSampleCallbacks<ReadValue, Sleep, KeepWatching, SameFieldIsCurrent>,
) -> ObserveSampleCapture
where
    ReadValue: FnMut() -> Option<String>,
    Sleep: FnMut(Duration),
    KeepWatching: FnMut() -> bool,
    SameFieldIsCurrent: FnMut() -> bool,
{
    let mut settled_field = None;
    let outcome = observe_after_paste_samples_with_hook(
        post_insert_field,
        inserted_text,
        inserted_range_after,
        limits,
        callbacks,
        |field, stable| {
            settled_field = if stable {
                field.map(str::to_owned)
            } else {
                None
            };
        },
    );
    ObserveSampleCapture {
        outcome,
        settled_field,
    }
}

fn observe_after_paste_samples_with_hook<
    ReadValue,
    Sleep,
    KeepWatching,
    SameFieldIsCurrent,
    OnSample,
>(
    post_insert_field: &str,
    inserted_text: &str,
    inserted_range_after: Option<crate::paste::Utf16RangeV1>,
    limits: ObserveLimits,
    callbacks: ObserveSampleCallbacks<ReadValue, Sleep, KeepWatching, SameFieldIsCurrent>,
    mut on_sample: OnSample,
) -> ObserveOutcome
where
    ReadValue: FnMut() -> Option<String>,
    Sleep: FnMut(Duration),
    KeepWatching: FnMut() -> bool,
    SameFieldIsCurrent: FnMut() -> bool,
    OnSample: FnMut(Option<&str>, bool),
{
    let ObserveSampleCallbacks {
        mut read_value,
        mut sleep,
        mut keep_watching,
        mut same_field_is_current,
        #[cfg(test)]
        mut end_of_input,
    } = callbacks;
    let Some(inserted_baseline) = inserted_segment_for_observation(
        post_insert_field,
        inserted_text,
        post_insert_field,
        inserted_range_after,
    ) else {
        return ObserveOutcome::Ambiguous;
    };
    if limits.interval.is_zero() {
        return ObserveOutcome::Unchanged;
    }

    let mut elapsed = Duration::ZERO;
    let mut deadline = limits.initial;
    let mut last_settled = inserted_baseline;
    let mut last_change_elapsed = None;
    let mut last_successful_same_sample_elapsed = None;
    #[cfg(test)]
    let mut first_unchanged_sample_elapsed = None;
    while elapsed < deadline.min(limits.max_total) {
        #[cfg(test)]
        if end_of_input
            .as_mut()
            .is_some_and(|end_of_input| end_of_input())
        {
            break;
        }
        if !keep_watching() {
            on_sample(None, false);
            return ObserveOutcome::Unchanged;
        }
        let sample_deadline = deadline.min(limits.max_total);
        let remaining = sample_deadline.saturating_sub(elapsed);
        let sleep_duration = limits.interval.min(remaining);
        if sleep_duration.is_zero() {
            break;
        }
        sleep(sleep_duration);
        if !keep_watching() {
            on_sample(None, false);
            return ObserveOutcome::Unchanged;
        }
        elapsed = elapsed.saturating_add(sleep_duration);

        if !same_field_is_current() {
            on_sample(None, false);
            return ObserveOutcome::LeftTarget;
        }
        if !keep_watching() {
            on_sample(None, false);
            return ObserveOutcome::Unchanged;
        }

        let current = read_value();
        if !keep_watching() {
            on_sample(None, false);
            return ObserveOutcome::Unchanged;
        }
        if !same_field_is_current() {
            on_sample(None, false);
            return ObserveOutcome::LeftTarget;
        }

        let Some(current) = current else {
            on_sample(None, false);
            #[cfg(test)]
            {
                first_unchanged_sample_elapsed = None;
            }
            if last_change_elapsed.is_some_and(|changed_at: Duration| {
                elapsed.saturating_sub(changed_at) >= limits.idle_grace
            }) {
                // A missing AX value after a dirty sample cannot establish
                // stability. Keep the same-field observation bounded by the
                // original total window in case a later read succeeds.
                deadline = limits.max_total;
            }
            continue;
        };
        let Some(current_inserted_segment) = inserted_segment_for_observation(
            post_insert_field,
            inserted_text,
            &current,
            inserted_range_after,
        ) else {
            on_sample(None, false);
            return ObserveOutcome::Ambiguous;
        };
        let stable_sample;
        if current_inserted_segment != last_settled {
            last_settled = current_inserted_segment;
            last_change_elapsed = Some(elapsed);
            last_successful_same_sample_elapsed = None;
            #[cfg(test)]
            {
                first_unchanged_sample_elapsed = None;
            }
            deadline = limits
                .max_total
                .min(elapsed.saturating_add(limits.idle_grace));
            stable_sample = false;
        } else {
            let changed_sample_settled = last_change_elapsed
                .is_some_and(|changed_at| elapsed.saturating_sub(changed_at) >= limits.idle_grace);
            if changed_sample_settled {
                last_successful_same_sample_elapsed = Some(elapsed);
            }
            #[cfg(test)]
            {
                let initially_unchanged_sample_settled = last_change_elapsed.is_none()
                    && first_unchanged_sample_elapsed
                        .is_some_and(|first| elapsed.saturating_sub(first) >= limits.idle_grace);
                if last_change_elapsed.is_none() && first_unchanged_sample_elapsed.is_none() {
                    first_unchanged_sample_elapsed = Some(elapsed);
                }
                stable_sample = changed_sample_settled || initially_unchanged_sample_settled;
            }
            #[cfg(not(test))]
            {
                stable_sample = changed_sample_settled;
            }
        }
        on_sample(Some(&current), stable_sample);
    }

    if !keep_watching() {
        on_sample(None, false);
        ObserveOutcome::Unchanged
    } else if last_change_elapsed.is_some() && last_successful_same_sample_elapsed.is_none() {
        // A time deadline alone is not evidence that a dirty value settled.
        on_sample(None, false);
        ObserveOutcome::Ambiguous
    } else {
        finalize_observe(inserted_text, &last_settled, false)
    }
}

#[cfg(any(test, feature = "research-harness"))]
pub(crate) struct ControlledFixturePairResult {
    pub observer_outcome: &'static str,
    pub candidate_class: Option<&'static str>,
    pub promote_threshold: Option<u32>,
    pub candidate_write: &'static str,
    pub trajectory_samples_observed: usize,
    pub trajectory_states_consumed: usize,
    pub trajectory_settled: bool,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ControlledAttributionSample {
    FieldText(String),
    MissingRead,
    FocusLost,
}

#[cfg(test)]
pub(crate) struct ControlledAttributionResult {
    pub(crate) observer_outcome: &'static str,
    pub(crate) judgment: crate::edit_attribution::Judgment,
    pub(crate) samples_observed: usize,
    /// Counts ordered poll events consumed; the pinned delivery baseline is not
    /// an observer poll and is excluded.
    pub(crate) states_consumed: usize,
    pub(crate) settled: bool,
    pub(crate) candidate_writer_called: bool,
    pub(crate) candidate_write: &'static str,
}

#[cfg(any(test, feature = "research-harness"))]
struct ControlledFixtureTrajectoryObservation {
    outcome: ObserveOutcome,
    samples_observed: usize,
    states_consumed: usize,
    settled: bool,
}

/// Feeds declared full-field fixture snapshots through the existing observer.
/// The first state is its baseline; later states are returned one per actual
/// read poll. Only the last declared state is repeated after the iterator is
/// exhausted, so an observer stop never forces it to consume future states.
#[cfg(any(test, feature = "research-harness"))]
fn observe_controlled_fixture_trajectory(
    ordered_states: &[String],
    limits: ObserveLimits,
) -> Option<ControlledFixtureTrajectoryObservation> {
    let post_insert_field = ordered_states.first()?;
    if ordered_states.len() < 2 {
        return None;
    }
    let inserted_range_after = crate::paste::Utf16RangeV1 {
        start: 0,
        length: u32::try_from(post_insert_field.encode_utf16().count()).ok()?,
    };

    let mut samples_observed = 0usize;
    // The baseline is passed to the observer as its initial full-field state.
    let mut states_consumed = 1usize;
    let mut next_state = 1usize;
    let mut last_state = post_insert_field.clone();
    let outcome = observe_after_paste_samples(
        post_insert_field,
        post_insert_field,
        Some(inserted_range_after),
        limits,
        ObserveSampleCallbacks {
            read_value: || {
                samples_observed = samples_observed.saturating_add(1);
                if let Some(state) = ordered_states.get(next_state) {
                    next_state += 1;
                    states_consumed = states_consumed.saturating_add(1);
                    last_state.clone_from(state);
                }
                Some(last_state.clone())
            },
            sleep: |_| {},
            keep_watching: || true,
            same_field_is_current: || true,
            #[cfg(test)]
            end_of_input: None,
        },
    );
    let stable_outcome = matches!(
        &outcome,
        ObserveOutcome::Unchanged
            | ObserveOutcome::SingleToken { .. }
            | ObserveOutcome::MultipleTokens { .. }
            | ObserveOutcome::ShortStyleRewrite { .. }
            | ObserveOutcome::StyleSignal { .. }
    );
    Some(ControlledFixtureTrajectoryObservation {
        outcome,
        samples_observed,
        states_consumed,
        settled: states_consumed == ordered_states.len() && stable_outcome,
    })
}

#[cfg(any(test, feature = "research-harness"))]
fn controlled_fixture_pair_result(
    observer_outcome: &'static str,
    candidate_class: Option<&'static str>,
    promote_threshold: Option<u32>,
    candidate_write: &'static str,
    trajectory_samples_observed: usize,
    trajectory_states_consumed: usize,
    trajectory_settled: bool,
) -> ControlledFixturePairResult {
    ControlledFixturePairResult {
        observer_outcome,
        candidate_class,
        promote_threshold,
        candidate_write,
        trajectory_samples_observed,
        trajectory_states_consumed,
        trajectory_settled,
    }
}

#[cfg(any(test, feature = "research-harness"))]
pub(crate) struct ControlledFixtureEditInput<'a> {
    pub(crate) expected_before: &'a str,
    pub(crate) expected_after: &'a str,
    pub(crate) episode_id: &'a str,
    pub(crate) source_sha256: &'a str,
    pub(crate) operation_id: &'a str,
    pub(crate) stable_source_origin_id: Option<&'a str>,
}

#[cfg(any(test, feature = "research-harness"))]
pub(crate) struct ControlledFixtureTrajectoryInput<'a> {
    pub(crate) edit: ControlledFixtureEditInput<'a>,
    pub(crate) ordered_states: &'a [String],
}

#[cfg(any(test, feature = "research-harness"))]
fn controlled_candidate_scope(
    scope: &crate::learning_research::LearningScopeV2,
) -> store::LearnPairScope {
    store::LearnPairScope {
        family: Some(crate::context::family_id(scope.family).to_owned()),
        mapping_id: Some(scope.mapping_id.clone()),
        browser_host: None,
        native_bundle: Some(scope.native_bundle.clone()),
    }
}

#[cfg(any(test, feature = "research-harness"))]
fn record_controlled_candidate_owner(
    dir: &std::path::Path,
    dictionary: &mut Vec<String>,
    candidate: ControlledCandidate<'_>,
) -> anyhow::Result<&'static str> {
    let ControlledCandidate {
        before,
        after,
        scope: candidate_scope,
        observation_id,
        stable_source_origin_id,
        auto_scoped,
    } = candidate;
    let key = pair_key(before, after);
    let observation = stable_source_origin_id
        .map(|origin| {
            store::LearnPairObservation::research_fixture_origin_with_observation_id(
                observation_id,
                origin,
                &key,
            )
        })
        .unwrap_or_else(|| {
            store::LearnPairObservation::research_fixture_edit(observation_id, &key)
        });
    let already_recorded = store::has_learn_pair_observation(dir, &observation)?;
    let result = record_learn_pair_with_observation_audited(
        dir,
        dictionary,
        before,
        after,
        Some(candidate_scope),
        Some(&observation),
        auto_scoped,
        None,
    )?;
    let stored_row = store::get_learn_pair(dir, &key)?;
    let row_scope_matches = stored_row.as_ref().is_some_and(|row| {
        row.before_surface.as_str() == before
            && row.after_surface.as_str() == after
            && row.family.as_deref() == candidate_scope.family.as_deref()
            && row.mapping_id.as_deref() == candidate_scope.mapping_id.as_deref()
            && row.browser_host.as_deref() == candidate_scope.browser_host.as_deref()
            && row.native_bundle.as_deref() == candidate_scope.native_bundle.as_deref()
    });
    Ok(if stored_row.is_some() && !row_scope_matches {
        "candidate_row_scope_mismatch"
    } else if already_recorded {
        if row_scope_matches {
            "duplicate_observation"
        } else {
            "candidate_row_readback_missing"
        }
    } else {
        match result {
            RecordPairResult::Pending { .. } | RecordPairResult::Promoted { .. } => {
                if row_scope_matches {
                    "candidate_written"
                } else {
                    "candidate_row_readback_missing"
                }
            }
            RecordPairResult::AlreadyPromoted => {
                if row_scope_matches {
                    "already_promoted"
                } else {
                    "candidate_row_readback_missing"
                }
            }
            RecordPairResult::Ignored => "candidate_owner_ignored",
        }
    })
}

#[cfg(any(test, feature = "research-harness"))]
struct ControlledCandidate<'a> {
    before: &'a str,
    after: &'a str,
    scope: &'a store::LearnPairScope,
    observation_id: &'a str,
    stable_source_origin_id: Option<&'a str>,
    auto_scoped: bool,
}

/// Preserve the existing two-state research call by routing it through the
/// same trajectory adapter used by the versioned text operation.
#[cfg(any(test, feature = "research-harness"))]
pub(crate) fn record_controlled_fixture_edit(
    dir: &std::path::Path,
    settings: &mut store::Settings,
    scope: &crate::learning_research::LearningScopeV2,
    consent: &crate::learning_research::LearningConsent,
    edit: ControlledFixtureEditInput<'_>,
) -> anyhow::Result<ControlledFixturePairResult> {
    let ordered_states = [
        consent.post_insert_field.clone(),
        consent.settled_field.clone(),
    ];
    record_controlled_fixture_edit_trajectory(
        dir,
        settings,
        scope,
        consent,
        ControlledFixtureTrajectoryInput {
            edit,
            ordered_states: &ordered_states,
        },
    )
}

/// Run the same bounded observer and candidate owner against prevalidated
/// ordered full-field snapshots. This does not claim human timing, IME input,
/// or a native AX observation.
#[cfg(any(test, feature = "research-harness"))]
pub(crate) fn record_controlled_fixture_edit_trajectory(
    dir: &std::path::Path,
    settings: &mut store::Settings,
    scope: &crate::learning_research::LearningScopeV2,
    consent: &crate::learning_research::LearningConsent,
    trajectory: ControlledFixtureTrajectoryInput<'_>,
) -> anyhow::Result<ControlledFixturePairResult> {
    let ControlledFixtureTrajectoryInput {
        edit,
        ordered_states,
    } = trajectory;
    let ControlledFixtureEditInput {
        expected_before,
        expected_after,
        episode_id,
        source_sha256,
        operation_id,
        stable_source_origin_id,
    } = edit;
    if !settings.dictionary_learn_enabled
        || !settings.local_edit_learning_enabled
        || !settings.context_enabled
        || !consent.observer_enabled
        || consent.term(expected_before, expected_after).is_none()
        || !crate::learning_research::pair_allowed(expected_before, expected_after)
    {
        return Ok(controlled_fixture_pair_result(
            "learning_grant_or_term_not_admitted",
            None,
            None,
            "not_attempted",
            0,
            0,
            false,
        ));
    }
    if !(2..=6).contains(&ordered_states.len())
        || ordered_states.first() != Some(&consent.post_insert_field)
        || ordered_states.last() != Some(&consent.settled_field)
    {
        return Ok(controlled_fixture_pair_result(
            "fixture_trajectory_boundary_mismatch",
            None,
            None,
            "not_attempted",
            0,
            0,
            false,
        ));
    }
    let Some(mapping) = settings
        .context_mappings
        .iter()
        .find(|mapping| mapping.id == scope.mapping_id)
    else {
        return Ok(controlled_fixture_pair_result(
            "scope_not_admitted",
            None,
            None,
            "not_attempted",
            0,
            0,
            false,
        ));
    };
    if mapping.bundle_id.as_deref() != Some(scope.native_bundle.as_str())
        || mapping.family != scope.family
        || !mapping.enabled
        || !mapping.dictionary_learn_enabled
        || mapping.source_permissions.ax_text
        || mapping.source_permissions.local_ocr
        || mapping.source_permissions.cloud_vision
        || mapping.source_permissions.context_text_to_providers
    {
        return Ok(controlled_fixture_pair_result(
            "scope_or_source_permission_mismatch",
            None,
            None,
            "not_attempted",
            0,
            0,
            false,
        ));
    }
    if lexicon::is_default_learn_off_target(
        mapping.bundle_id.as_deref(),
        mapping.browser_host.as_deref(),
    ) {
        return Ok(controlled_fixture_pair_result(
            "protected_target",
            None,
            None,
            "not_attempted",
            0,
            0,
            false,
        ));
    }

    let Some(observation) = observe_controlled_fixture_trajectory(
        ordered_states,
        ObserveLimits {
            initial: Duration::from_millis(2),
            interval: Duration::from_millis(1),
            idle_grace: Duration::from_millis(1),
            max_total: Duration::from_millis(3_u64.max(ordered_states.len() as u64)),
        },
    ) else {
        return Ok(controlled_fixture_pair_result(
            "fixture_trajectory_boundary_mismatch",
            None,
            None,
            "not_attempted",
            0,
            0,
            false,
        ));
    };
    let samples_observed = observation.samples_observed;
    let states_consumed = observation.states_consumed;
    let trajectory_settled = observation.settled;
    let (before_span, after) = match observation.outcome {
        ObserveOutcome::SingleToken { before_span, after } => (before_span, after),
        outcome => {
            return Ok(controlled_fixture_pair_result(
                observe_outcome_code(&outcome),
                None,
                None,
                "not_attempted",
                samples_observed,
                states_consumed,
                trajectory_settled,
            ));
        }
    };
    if !crate::learning_research::pair_allowed(&before_span, &after) {
        return Ok(controlled_fixture_pair_result(
            "term_fixture_mismatch",
            None,
            None,
            "not_attempted",
            samples_observed,
            states_consumed,
            false,
        ));
    }

    let Some(suggestion) = history_lexical_correction(&before_span, &after) else {
        return Ok(controlled_fixture_pair_result(
            "local_edit_guard_rejected",
            None,
            None,
            "not_attempted",
            samples_observed,
            states_consumed,
            trajectory_settled,
        ));
    };
    let before_span = suggestion.before_span;
    let after = suggestion.after;
    if before_span != expected_before
        || after != expected_after
        || consent.term(&before_span, &after).is_none()
        || !crate::learning_research::pair_allowed(&before_span, &after)
    {
        return Ok(controlled_fixture_pair_result(
            "term_fixture_mismatch",
            None,
            None,
            "not_attempted",
            samples_observed,
            states_consumed,
            false,
        ));
    }

    let classification = classify_learn_pair_decision(&before_span, &after);
    let candidate_class = match classification.class {
        LearnClass::Rejected => "rejected",
        LearnClass::Name => "name",
        LearnClass::Term => "term",
    };
    let threshold = promote_hits_for(classification.class);
    let fallback_source_origin = format!("{episode_id}:{operation_id}:{source_sha256}");
    let proof_source_origin = stable_source_origin_id.unwrap_or(&fallback_source_origin);
    let observation_id = stable_source_origin_id
        .map(|origin| format!("stable_source:{origin}"))
        .unwrap_or_else(|| fallback_source_origin.clone());
    let candidate_scope = controlled_candidate_scope(scope);
    let auto_scoped = !mapping.source_permissions.ax_text && settings.local_edit_learning_enabled;
    let mut scoped_dictionary = Vec::new();
    let dictionary = if auto_scoped {
        &mut scoped_dictionary
    } else {
        &mut settings.dictionary
    };
    let candidate_write = record_controlled_candidate_owner(
        dir,
        dictionary,
        ControlledCandidate {
            before: &before_span,
            after: &after,
            scope: &candidate_scope,
            observation_id: &observation_id,
            stable_source_origin_id: Some(proof_source_origin),
            auto_scoped,
        },
    )?;
    Ok(controlled_fixture_pair_result(
        "single_token",
        Some(candidate_class),
        (classification.class != LearnClass::Rejected).then_some(threshold),
        candidate_write,
        samples_observed,
        states_consumed,
        trajectory_settled,
    ))
}

#[cfg(test)]
struct ControlledAttributionObservation {
    capture: ObserveSampleCapture,
    samples_observed: usize,
    states_consumed: usize,
    settled: bool,
}

#[cfg(test)]
fn observe_controlled_attribution_samples(
    baseline_field: &str,
    inserted_text: &str,
    inserted_range_after: crate::paste::Utf16RangeV1,
    ordered_samples: &[ControlledAttributionSample],
    limits: ObserveLimits,
) -> ControlledAttributionObservation {
    struct Cursor<'a> {
        samples: &'a [ControlledAttributionSample],
        next_event: usize,
        pending: Option<ControlledAttributionSample>,
        awaiting_read: bool,
        samples_observed: usize,
        states_consumed: usize,
        last_consumed_was_field_text: bool,
    }

    let cursor = std::rc::Rc::new(std::cell::RefCell::new(Cursor {
        samples: ordered_samples,
        next_event: 0,
        pending: None,
        awaiting_read: false,
        samples_observed: 0,
        states_consumed: 0,
        last_consumed_was_field_text: false,
    }));
    let eof_reached = std::rc::Rc::new(std::cell::Cell::new(ordered_samples.is_empty()));
    let read_cursor = std::rc::Rc::clone(&cursor);
    let field_cursor = std::rc::Rc::clone(&cursor);
    let read_eof = std::rc::Rc::clone(&eof_reached);
    let end_eof = std::rc::Rc::clone(&eof_reached);
    let capture = observe_after_paste_samples_capture(
        baseline_field,
        inserted_text,
        Some(inserted_range_after),
        limits,
        ObserveSampleCallbacks {
            read_value: move || {
                let mut cursor = read_cursor.borrow_mut();
                cursor.samples_observed = cursor.samples_observed.saturating_add(1);
                let value = match cursor.pending.take() {
                    Some(ControlledAttributionSample::FieldText(text)) => {
                        cursor.next_event = cursor.next_event.saturating_add(1);
                        cursor.states_consumed = cursor.states_consumed.saturating_add(1);
                        cursor.last_consumed_was_field_text = true;
                        Some(text)
                    }
                    Some(ControlledAttributionSample::MissingRead)
                    | Some(ControlledAttributionSample::FocusLost) => {
                        cursor.next_event = cursor.next_event.saturating_add(1);
                        cursor.states_consumed = cursor.states_consumed.saturating_add(1);
                        cursor.last_consumed_was_field_text = false;
                        None
                    }
                    None => None,
                };
                if cursor.next_event >= cursor.samples.len() {
                    read_eof.set(true);
                }
                value
            },
            sleep: |_| {},
            keep_watching: || true,
            same_field_is_current: move || {
                let mut cursor = field_cursor.borrow_mut();
                if cursor.awaiting_read {
                    cursor.awaiting_read = false;
                    return true;
                }
                let next = cursor.samples.get(cursor.next_event).cloned();
                match next {
                    Some(ControlledAttributionSample::FocusLost) => {
                        cursor.next_event = cursor.next_event.saturating_add(1);
                        cursor.states_consumed = cursor.states_consumed.saturating_add(1);
                        cursor.last_consumed_was_field_text = false;
                        false
                    }
                    Some(sample) => {
                        cursor.pending = Some(sample);
                        cursor.awaiting_read = true;
                        true
                    }
                    None => {
                        cursor.pending = None;
                        cursor.awaiting_read = true;
                        true
                    }
                }
            },
            end_of_input: Some(Box::new(move || end_eof.get())),
        },
    );
    let cursor = cursor.borrow();
    let settled = capture.settled_field.is_some()
        && cursor.states_consumed == ordered_samples.len()
        && cursor.next_event == ordered_samples.len()
        && cursor.last_consumed_was_field_text
        && !matches!(capture.outcome, ObserveOutcome::LeftTarget);
    ControlledAttributionObservation {
        capture,
        samples_observed: cursor.samples_observed,
        states_consumed: cursor.states_consumed,
        settled,
    }
}

/// Run the pure judge on the token-bound fixture and the existing same-field
/// observer. The manifest supplies evidence, scope, limits, and ordered poll
/// events; the learning grant arrives separately.
#[cfg(test)]
pub(crate) fn record_controlled_fixture_attribution_trajectory(
    dir: &std::path::Path,
    settings: &mut store::Settings,
    consent: &crate::learning_research::LearningConsent,
    verified_origin: &crate::learning_research::edit_attribution_tests::VerifiedAttributionOrigin<
        '_,
    >,
    _operation_id: &str,
) -> anyhow::Result<ControlledAttributionResult> {
    use crate::edit_attribution::{self, EditSettlement, EditSourceTrust, UnknownReason, Verdict};

    let evidence = verified_origin.evidence();
    let mut preflight = evidence.clone();
    preflight.edit.value.stable_text = preflight.delivery.value.inserted_text.clone();
    preflight.edit.value.settlement = EditSettlement::Unsettled;
    let preflight_judgment = edit_attribution::judge(&preflight);
    if !matches!(
        preflight_judgment.reason,
        Some(UnknownReason::EditUnsettled | UnknownReason::EditSourceUntrusted)
    ) {
        return Ok(ControlledAttributionResult {
            observer_outcome: "delivery_preflight_rejected",
            judgment: preflight_judgment,
            samples_observed: 0,
            states_consumed: 0,
            settled: false,
            candidate_writer_called: false,
            candidate_write: "not_attempted",
        });
    }

    let first = &evidence.delivery.value.first_snapshot;
    let inserted_range = &evidence.delivery.value.inserted_range;
    let prefix = first
        .get(..inserted_range.start)
        .ok_or_else(|| anyhow::anyhow!("attribution_inserted_range_not_utf8_boundary"))?;
    let inserted = first
        .get(inserted_range.start..inserted_range.end)
        .filter(|inserted| *inserted == evidence.delivery.value.inserted_text)
        .ok_or_else(|| anyhow::anyhow!("attribution_inserted_range_mismatch"))?;
    let utf16_range = crate::paste::Utf16RangeV1 {
        start: u32::try_from(prefix.encode_utf16().count())
            .map_err(|_| anyhow::anyhow!("attribution_utf16_range_overflow"))?,
        length: u32::try_from(inserted.encode_utf16().count())
            .map_err(|_| anyhow::anyhow!("attribution_utf16_range_overflow"))?,
    };
    let observation = observe_controlled_attribution_samples(
        first,
        &evidence.delivery.value.inserted_text,
        utf16_range,
        verified_origin.ordered_samples(),
        verified_origin.limits(),
    );

    let mut actual_evidence = evidence.clone();
    actual_evidence.edit.value.stable_text = observation
        .capture
        .settled_field
        .clone()
        .unwrap_or_else(|| actual_evidence.delivery.value.inserted_text.clone());
    actual_evidence.edit.value.settlement = if observation.settled {
        EditSettlement::Stable
    } else {
        EditSettlement::Unsettled
    };
    if observation.capture.outcome == ObserveOutcome::LeftTarget {
        actual_evidence.edit.value.source_trust = EditSourceTrust::Untrusted;
    }
    let judgment = edit_attribution::judge(&actual_evidence);

    let (candidate_writer_called, candidate_write) = crate::learning_research::with_scoped_consent(
        consent.clone(),
        || -> anyhow::Result<(bool, &'static str)> {
            if !observation.settled {
                return Ok((false, "observer_not_settled"));
            }
            let (before_span, after) = match &observation.capture.outcome {
                ObserveOutcome::SingleToken { before_span, after } => {
                    (before_span.as_str(), after.as_str())
                }
                _ => return Ok((false, "observer_not_single_token")),
            };
            if judgment.verdict != Verdict::AsrSubstitution {
                return Ok((false, "attribution_not_asr_substitution"));
            }
            let Some(pair) = judgment.proposed_learning_pair.as_ref() else {
                return Ok((false, "attribution_pair_not_proposed"));
            };
            let scope = verified_origin.scope();
            if pair.scope.as_str() != scope.mapping_id.as_str()
                || pair.observed.as_str() != before_span
                || pair.correction.as_str() != after
                || consent.mapping_id.as_str() != scope.mapping_id.as_str()
                || consent.bundle_id.as_str() != scope.native_bundle.as_str()
                || consent.post_insert_field.as_str()
                    != evidence.delivery.value.first_snapshot.as_str()
                || actual_evidence.edit.value.stable_text.as_str() != consent.settled_field.as_str()
                || !consent.observer_enabled
                || consent.term(before_span, after).is_none()
                || !crate::learning_research::pair_allowed(before_span, after)
            {
                return Ok((false, "learning_grant_or_scope_not_admitted"));
            }
            let Some(suggestion) = history_lexical_correction(before_span, after) else {
                return Ok((false, "local_edit_guard_rejected"));
            };
            if suggestion.before_span.as_str() != before_span || suggestion.after.as_str() != after
            {
                return Ok((false, "observer_pair_not_exact_lexical_correction"));
            }
            if !settings.dictionary_learn_enabled
                || !settings.local_edit_learning_enabled
                || !settings.context_enabled
            {
                return Ok((false, "learning_grant_or_term_not_admitted"));
            }
            let Some(mapping) = settings
                .context_mappings
                .iter()
                .find(|mapping| mapping.id == scope.mapping_id)
            else {
                return Ok((false, "scope_not_admitted"));
            };
            if mapping.bundle_id.as_deref() != Some(scope.native_bundle.as_str())
                || mapping.family != scope.family
                || !mapping.enabled
                || !mapping.dictionary_learn_enabled
                || mapping.source_permissions.ax_text
                || mapping.source_permissions.local_ocr
                || mapping.source_permissions.cloud_vision
                || mapping.source_permissions.context_text_to_providers
            {
                return Ok((false, "scope_or_source_permission_mismatch"));
            }
            if lexicon::is_default_learn_off_target(
                mapping.bundle_id.as_deref(),
                mapping.browser_host.as_deref(),
            ) {
                return Ok((false, "protected_target"));
            }

            let observation_id = format!("stable_source:{}", verified_origin.origin_id());
            let candidate_scope = controlled_candidate_scope(scope);
            let auto_scoped =
                !mapping.source_permissions.ax_text && settings.local_edit_learning_enabled;
            let mut scoped_dictionary = Vec::new();
            let dictionary = if auto_scoped {
                &mut scoped_dictionary
            } else {
                &mut settings.dictionary
            };
            let candidate_write = record_controlled_candidate_owner(
                dir,
                dictionary,
                ControlledCandidate {
                    before: before_span,
                    after,
                    scope: &candidate_scope,
                    observation_id: &observation_id,
                    stable_source_origin_id: Some(verified_origin.origin_id()),
                    auto_scoped,
                },
            )?;
            Ok((true, candidate_write))
        },
    )?;

    Ok(ControlledAttributionResult {
        observer_outcome: if observation.capture.outcome == ObserveOutcome::LeftTarget {
            "left_field"
        } else {
            observe_outcome_code(&observation.capture.outcome)
        },
        judgment,
        samples_observed: observation.samples_observed,
        states_consumed: observation.states_consumed,
        settled: observation.settled,
        candidate_writer_called,
        candidate_write,
    })
}

#[cfg(any(test, feature = "research-harness"))]
fn observe_outcome_code(outcome: &ObserveOutcome) -> &'static str {
    match outcome {
        ObserveOutcome::Unchanged => "unchanged",
        ObserveOutcome::LeftTarget => "identity_not_available_in_fixture",
        ObserveOutcome::SingleToken { .. } => "single_token",
        ObserveOutcome::MultipleTokens { .. } => {
            "multiple_tokens_unsupported_in_single_pair_fixture"
        }
        ObserveOutcome::ShortStyleRewrite { .. } => "short_style_rewrite",
        ObserveOutcome::StyleSignal { .. } => "style_signal",
        ObserveOutcome::Ambiguous => "ambiguous",
    }
}

/// Returns the current contents of the unique inserted range only when the
/// surrounding field text is unchanged. Multiple or overlapping baseline
/// matches are ambiguous, so a local-edit grant cannot learn pre-existing text.
#[cfg(test)]
fn edited_inserted_segment(
    baseline_field: &str,
    inserted_text: &str,
    current_field: &str,
) -> Option<String> {
    if inserted_text.is_empty() {
        return None;
    }
    let matches = baseline_field
        .char_indices()
        .filter_map(|(start, _)| {
            baseline_field[start..]
                .starts_with(inserted_text)
                .then_some((start, start + inserted_text.len()))
        })
        .take(2)
        .collect::<Vec<_>>();
    let [(start, end)] = matches.as_slice() else {
        return None;
    };
    let prefix = &baseline_field[..*start];
    let suffix = &baseline_field[*end..];
    let after_prefix = current_field.strip_prefix(prefix)?;
    let current_segment = if suffix.is_empty() {
        after_prefix
    } else {
        after_prefix.strip_suffix(suffix)?
    };
    Some(current_segment.to_owned())
}

fn finalize_observe(baseline: &str, settled: &str, left_target: bool) -> ObserveOutcome {
    if settled == baseline {
        return if left_target {
            ObserveOutcome::LeftTarget
        } else {
            ObserveOutcome::Unchanged
        };
    }
    if is_short_style_rewrite(baseline, settled) {
        return ObserveOutcome::ShortStyleRewrite {
            before: baseline.to_owned(),
            after: settled.to_owned(),
        };
    }
    if matches!(
        cjk_name_edit_boundary(baseline, settled),
        Some(CjkNameEditBoundary::Ambiguous)
    ) {
        return ObserveOutcome::Ambiguous;
    }
    if let Some(corrections) = extract_word_corrections(baseline, settled) {
        if corrections.len() == 1
            && dictionary_suggestion_pair_allowed(
                &corrections[0].before_span,
                &corrections[0].after,
            )
        {
            let correction = corrections.into_iter().next().expect("one correction");
            return ObserveOutcome::SingleToken {
                before_span: correction.before_span,
                after: correction.after,
            };
        }
        if !corrections.is_empty()
            && corrections.iter().all(|correction| {
                automatic_word_pair_allowed(&correction.before_span, &correction.after)
            })
        {
            let corrections = deduplicate_correction_pairs(corrections);
            if corrections.len() == 1 {
                let correction = corrections
                    .into_iter()
                    .next()
                    .expect("one unique correction");
                return ObserveOutcome::SingleToken {
                    before_span: correction.before_span,
                    after: correction.after,
                };
            }
            if !corrections.is_empty() {
                return ObserveOutcome::MultipleTokens {
                    before: baseline.to_owned(),
                    after: settled.to_owned(),
                    corrections,
                };
            }
        }
    }
    if let Some(signal) = style_signal(baseline, settled) {
        return signal;
    }
    if left_target {
        ObserveOutcome::LeftTarget
    } else {
        ObserveOutcome::Ambiguous
    }
}

pub fn is_short_style_rewrite(baseline: &str, settled: &str) -> bool {
    if baseline.trim() != baseline || settled.trim() != settled {
        return false;
    }
    if baseline.is_empty()
        || settled.is_empty()
        || baseline == settled
        || baseline.chars().count() > 40
        || settled.chars().count() > 40
        || looks_like_foreign_bubble(baseline)
        || looks_like_foreign_bubble(settled)
        || baseline.contains('\n')
        || settled.contains('\n')
        || !style_text_is_safe(baseline)
        || !style_text_is_safe(settled)
    {
        return false;
    }
    let (Some(before_key), Some(after_key)) = (
        explicit_style_content_key(baseline),
        explicit_style_content_key(settled),
    ) else {
        return false;
    };
    before_key == after_key && has_explicit_style_marker_change(baseline, settled)
}

fn explicit_style_content_key(value: &str) -> Option<String> {
    if !style_text_is_safe(value) {
        return None;
    }
    let without_laughter = strip_boundary_laughter(value);
    Some(
        without_laughter
            .trim()
            .replace("稍后", "{time}")
            .replace("晚点", "{time}")
            .replace(['您', '你'], "{you}"),
    )
}

fn has_explicit_style_marker_change(before: &str, after: &str) -> bool {
    before.matches("稍后").count() != after.matches("稍后").count()
        || before.matches("晚点").count() != after.matches("晚点").count()
        || before.matches('您').count() != after.matches('您').count()
        || before.matches('你').count() != after.matches('你').count()
        || boundary_laughter_count(before) != boundary_laughter_count(after)
}

fn boundary_laughter_count(value: &str) -> usize {
    let chars: Vec<char> = value.chars().collect();
    let mut count = 0;
    let mut index = 0;
    while index + 1 < chars.len() {
        if chars[index] == '哈' && chars[index + 1] == '哈' {
            let left_boundary = index == 0
                || chars[index - 1].is_whitespace()
                || is_style_boundary_punctuation(chars[index - 1]);
            let right = index + 2;
            let right_boundary = right == chars.len()
                || chars[right].is_whitespace()
                || is_style_boundary_punctuation(chars[right]);
            if left_boundary && right_boundary {
                count += 1;
                index += 2;
                continue;
            }
        }
        index += 1;
    }
    count
}

fn strip_boundary_laughter(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut output = String::with_capacity(value.len());
    let mut index = 0;
    while index < chars.len() {
        if index + 1 < chars.len() && chars[index] == '哈' && chars[index + 1] == '哈' {
            let left_boundary = index == 0
                || chars[index - 1].is_whitespace()
                || is_style_boundary_punctuation(chars[index - 1]);
            let right = index + 2;
            let right_boundary = right == chars.len()
                || chars[right].is_whitespace()
                || is_style_boundary_punctuation(chars[right]);
            if left_boundary && right_boundary {
                index += 2;
                continue;
            }
        }
        output.push(chars[index]);
        index += 1;
    }
    output
}

fn is_style_boundary_punctuation(value: char) -> bool {
    matches!(
        value,
        ',' | '，' | '。' | '.' | '!' | '！' | '?' | '？' | ';' | '；' | ':' | '：'
    )
}

fn style_text_is_safe(value: &str) -> bool {
    if value.chars().any(char::is_numeric)
        || has_cjk_number_or_scale(value)
        || value
            .split(|ch: char| !ch.is_ascii_alphabetic())
            .any(is_written_number_token)
        || value.contains([
            '`', '"', '\'', '“', '”', '‘', '’', '「', '」', '『', '』', '<', '>', '{', '}', '[',
            ']', '(', ')', '（', '）', '/', '\\', '|', '=', '#', '@', '+', '-', '%', '$', '€', '£',
            '¥', ':', '：',
        ])
    {
        return false;
    }
    let chars: Vec<char> = value.chars().collect();
    for (index, ch) in chars.iter().copied().enumerate() {
        if ch == '.'
            && index > 0
            && index + 1 < chars.len()
            && chars[index - 1].is_ascii_alphanumeric()
            && chars[index + 1].is_ascii_alphanumeric()
        {
            return false;
        }
        if (ch == '_' || ch == '-')
            && index > 0
            && index + 1 < chars.len()
            && chars[index - 1].is_ascii_alphanumeric()
            && chars[index + 1].is_ascii_alphanumeric()
        {
            return false;
        }
    }
    true
}

fn looks_like_foreign_bubble(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("对方:") || trimmed.starts_with("对方：") || trimmed.starts_with("Other:")
}

fn terminal_punctuation(value: &str) -> (&str, &str) {
    let end = value.trim_end_matches(char::is_whitespace).len();
    let trimmed = &value[..end];
    let start = trimmed
        .char_indices()
        .rev()
        .take_while(|(_, ch)| matches!(ch, '.' | '。' | '?' | '？' | '!' | '！'))
        .last()
        .map(|(index, _)| index)
        .unwrap_or(trimmed.len());
    (&value[..start], &trimmed[start..])
}

pub fn push_style_pair(pairs: &mut Vec<context::StyleExamplePair>, input: String, output: String) {
    let input = input.trim().to_string();
    let output = output.trim().to_string();
    if input.is_empty() || output.is_empty() || input == output {
        return;
    }
    if pairs.iter().any(|pair| pair.output == output) {
        return;
    }
    pairs.push(context::StyleExamplePair { input, output });
    if pairs.len() > 3 {
        pairs.remove(0);
    }
}

pub fn intensity_shift_from_edit(
    baseline: &str,
    settled: &str,
    current: crate::llm::CleanupIntensity,
) -> Option<i8> {
    if !is_short_style_rewrite(baseline, settled) {
        return None;
    }
    let casual_delta = casual_score(settled) - casual_score(baseline);
    let baseline_len = baseline.chars().count();
    let settled_len = settled.chars().count();
    if casual_delta > 0 || settled_len + 2 < baseline_len {
        return (current != crate::llm::CleanupIntensity::Off).then_some(-1);
    }
    if settled_len > baseline_len + 4 && casual_delta <= 0 {
        return (current != crate::llm::CleanupIntensity::Heavy).then_some(1);
    }
    None
}

fn casual_score(value: &str) -> i32 {
    let mut score = 0i32;
    for marker in ["哈哈", "晚点", "嗯", "啊", "lol", "haha"] {
        if value.contains(marker) {
            score += 1;
        }
    }
    if value.contains('你') && !value.contains('您') {
        score += 1;
    }
    if value.contains('您') {
        score -= 1;
    }
    if value.contains("稍后") {
        score -= 1;
    }
    score
}

const STYLE_LEARN_HITS: u32 = 3;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StyleLearnApply {
    pub pairs_changed: bool,
    pub intensity_changed: bool,
    pub style_pair_key: Option<String>,
    pub style_draft_key: Option<String>,
    pub intensity_pair_key: Option<String>,
}

#[cfg(test)]
fn apply_post_paste_style_learn(
    mappings: &mut Vec<context::AppMapping>,
    dir: &std::path::Path,
    mapping_id: &str,
    baseline: &str,
    settled: &str,
    global_intensity: crate::llm::CleanupIntensity,
    audit: Option<&mut AuditAttempt>,
) -> anyhow::Result<StyleLearnApply> {
    apply_post_paste_style_learn_for_session(
        mappings,
        dir,
        mapping_id,
        baseline,
        settled,
        global_intensity,
        None,
        audit,
    )
}

#[allow(clippy::too_many_arguments)]
fn apply_post_paste_style_learn_for_session(
    mappings: &mut Vec<context::AppMapping>,
    dir: &std::path::Path,
    mapping_id: &str,
    baseline: &str,
    settled: &str,
    global_intensity: crate::llm::CleanupIntensity,
    session_id: Option<&str>,
    mut audit: Option<&mut AuditAttempt>,
) -> anyhow::Result<StyleLearnApply> {
    if !is_short_style_rewrite(baseline, settled) {
        return Ok(StyleLearnApply::default());
    }
    ensure_mapping_for_learn(mappings, mapping_id);
    let Some(mapping_index) = mappings.iter().position(|mapping| mapping.id == mapping_id) else {
        return Ok(StyleLearnApply::default());
    };
    let scope = store::LearnPairScope {
        family: Some(context::family_id(mappings[mapping_index].family).to_owned()),
        mapping_id: Some(mapping_id.to_owned()),
        browser_host: mappings[mapping_index].browser_host.clone(),
        native_bundle: mappings[mapping_index].bundle_id.clone(),
    };
    let mut applied = StyleLearnApply::default();

    let style_key = format!("learn:style:{mapping_id}");
    let style_observation = session_id
        .map(|session_id| store::LearnPairObservation::paste_session(session_id, &style_key));
    if let store::LearnPairUpsert::Stored(row) =
        upsert_style_candidate_row_audited_with_observation(
            dir,
            &style_key,
            baseline,
            settled,
            Some(&scope),
            style_observation.as_ref(),
            audit.as_deref_mut(),
        )?
    {
        crate::learning_research::style_candidate(
            row.hits,
            STYLE_LEARN_HITS,
            "short_style_rewrite",
        );
        if row.hits >= STYLE_LEARN_HITS {
            let draft = store::upsert_style_draft(
                dir,
                mapping_id,
                "short_style_rewrite",
                settled,
                baseline,
                settled,
            )?;
            let _ = store::set_learn_pair_hits(dir, &style_key, 0);
            applied.style_pair_key = Some(style_key);
            applied.style_draft_key = Some(draft.draft_key);
        }
    }

    let current = mappings[mapping_index]
        .cleanup_intensity
        .unwrap_or(global_intensity);
    if let Some(shift) = intensity_shift_from_edit(baseline, settled, current) {
        let direction = if shift < 0 { "down" } else { "up" };
        let intensity_key = format!("learn:intensity:{direction}:{mapping_id}");
        let previous = mappings[mapping_index]
            .cleanup_intensity
            .map(|item| item.as_str().to_owned())
            .unwrap_or_default();
        let next = if shift < 0 {
            current.demote()
        } else {
            current.promote()
        };
        let intensity_observation = session_id.map(|session_id| {
            store::LearnPairObservation::paste_session(session_id, &intensity_key)
        });
        if let store::LearnPairUpsert::Stored(row) =
            upsert_style_candidate_row_audited_with_observation(
                dir,
                &intensity_key,
                &previous,
                next.as_str(),
                Some(&scope),
                intensity_observation.as_ref(),
                audit,
            )?
        {
            crate::learning_research::style_candidate(row.hits, STYLE_LEARN_HITS, "intensity");
            if row.hits >= STYLE_LEARN_HITS && next != current {
                mappings[mapping_index].cleanup_intensity = Some(next);
                let _ = store::set_learn_pair_hits(dir, &intensity_key, 0);
                applied.intensity_changed = true;
                applied.intensity_pair_key = Some(intensity_key);
            }
        }
    }

    Ok(applied)
}

fn ensure_mapping_for_learn(mappings: &mut Vec<context::AppMapping>, mapping_id: &str) {
    if mappings.iter().any(|mapping| mapping.id == mapping_id) {
        return;
    }
    if let Some(created) = mapping_from_style_draft_id(mapping_id) {
        mappings.push(created);
    }
}

fn style_signal(before: &str, after: &str) -> Option<ObserveOutcome> {
    if !style_text_is_safe(before) || !style_text_is_safe(after) {
        return None;
    }
    let (before_body, before_marks) = terminal_punctuation(before);
    let (after_body, after_marks) = terminal_punctuation(after);
    if before_body.is_empty() || before_body != after_body || before_marks == after_marks {
        return None;
    }
    let before_profile = punctuation_profile(before_marks);
    let after_profile = punctuation_profile(after_marks);
    if before_profile == after_profile {
        return None;
    }
    let (before_span, after_span) = changed_spans(before, after);
    let excerpt = if after_span.chars().count() <= 120 {
        after_span.clone()
    } else {
        after_span.chars().take(120).collect()
    };
    if excerpt.trim().is_empty() {
        return None;
    }
    let style_key = if after_profile.questions > before_profile.questions {
        "more_questions"
    } else if after_profile.periods < before_profile.periods {
        "fewer_periods"
    } else {
        "punct_density"
    };
    Some(ObserveOutcome::StyleSignal {
        excerpt,
        style_key: style_key.into(),
        before_excerpt: before_span.chars().take(120).collect(),
        after_excerpt: after_span.chars().take(120).collect(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PunctuationProfile {
    periods: u32,
    questions: u32,
    spaces: u32,
}

fn punctuation_profile(value: &str) -> PunctuationProfile {
    let mut periods = 0;
    let mut questions = 0;
    let mut spaces = 0;
    for ch in value.chars() {
        match ch {
            '.' | '。' => periods += 1,
            '?' | '？' => questions += 1,
            ' ' | '\n' | '\t' => spaces += 1,
            _ => {}
        }
    }
    PunctuationProfile {
        periods,
        questions,
        spaces,
    }
}

pub const PROMOTE_HITS: u32 = 3;
pub const NAME_PROMOTE_HITS: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LearnClass {
    Rejected,
    Name,
    Term,
}

pub fn classify_learn_pair(before: &str, after: &str) -> LearnClass {
    classify_learn_pair_decision(before, after).class
}
/// One classifier owns both the decision and its actual rejection branch.
pub(crate) fn classify_learn_pair_decision(before: &str, after: &str) -> ClassificationV1 {
    let after = after.trim();
    let before = before.trim();
    let reason = if after.is_empty() || after == before {
        Some("unchanged_or_empty")
    } else if is_case_only_change(before, after) {
        Some("case_only")
    } else if is_rejected_learn_surface(after) {
        Some("surface_rejected")
    } else {
        None
    };
    ClassificationV1 {
        class: if reason.is_some() {
            LearnClass::Rejected
        } else if looks_like_cjk_personal_name(after) {
            LearnClass::Name
        } else {
            LearnClass::Term
        },
        reason,
    }
}

pub fn seed_lexicon_from_screen(
    dir: &std::path::Path,
    dictionary: &mut Vec<String>,
    screen: &crate::screen_text::ScreenTextContext,
    style_pairs: &mut Vec<context::StyleExamplePair>,
    scope: Option<&store::LearnPairScope>,
    permissions: context::ContextSourcePermissions,
) -> anyhow::Result<()> {
    let _ = style_pairs;
    for token in screen.asr_terms(permissions) {
        let token = token.trim();
        if token.is_empty() || is_blocked(token) {
            continue;
        }
        let _ = record_learn_pair_with_scope(dir, dictionary, "", token, scope)?;
    }
    Ok(())
}

pub(crate) fn maybe_seed_screen_lexicon(
    app: &tauri::AppHandle,
    state: &AppState,
    recording_context: Option<&context::ContextSnapshot>,
) {
    let settings = lock_recover(&state.settings).clone();
    if !settings.dictionary_learn_enabled || !settings.context_enabled {
        return;
    }
    let Some(recording_context) =
        recording_context.filter(|snapshot| crate::context_target_is_current(snapshot, &settings))
    else {
        return;
    };
    let Some(screen) = crate::session_screen_text(state) else {
        return;
    };
    let Some(mapping) =
        lexicon::mapping_for_profile(&settings.context_mappings, &recording_context.profile.id)
            .filter(|mapping| mapping.enabled && mapping.dictionary_learn_enabled)
    else {
        return;
    };
    if lexicon::is_default_learn_off_target(
        recording_context.target_guard.bundle_id.as_deref(),
        recording_context.target_guard.browser_host.as_deref(),
    ) {
        return;
    }
    let permissions = mapping
        .source_permissions
        .intersect(screen.evidence.capture_permissions.unwrap_or_default());
    if screen.asr_terms(permissions).is_empty() {
        return;
    }
    if !crate::lexicon::scene_allows_learn(
        &settings.context_mappings,
        &recording_context.profile.id,
        recording_context.target_guard.bundle_id.as_deref(),
        recording_context.target_guard.browser_host.as_deref(),
    ) {
        return;
    }
    let scope = Some(scope_from_snapshot(recording_context));
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        persist_screen_lexicon_locked(&app, screen, scope, permissions).await;
    });
}

async fn persist_screen_lexicon_locked(
    app: &tauri::AppHandle,
    screen: crate::screen_text::ScreenTextContext,
    scope: Option<store::LearnPairScope>,
    permissions: context::ContextSourcePermissions,
) {
    let state = app.state::<AppState>();
    let _gate = state.settings_gate.lock().await;
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    let mut snapshot = lock_recover(&state.settings).clone();
    if !snapshot.dictionary_learn_enabled || !snapshot.context_enabled {
        return;
    }
    let Some(mapping_id) = scope.as_ref().and_then(|scope| scope.mapping_id.as_deref()) else {
        return;
    };
    let Some(mapping) = snapshot
        .context_mappings
        .iter()
        .find(|mapping| mapping.id == mapping_id)
    else {
        return;
    };
    if !mapping.enabled
        || !mapping.dictionary_learn_enabled
        || lexicon::is_default_learn_off_target(
            mapping.bundle_id.as_deref(),
            mapping.browser_host.as_deref(),
        )
    {
        return;
    }
    let permissions = permissions
        .intersect(mapping.source_permissions)
        .intersect(screen.evidence.capture_permissions.unwrap_or_default());
    if screen.asr_terms(permissions).is_empty() {
        return;
    }
    // The legacy recorder still uses a Vec to enforce its capacity behavior.
    // Keep that probe separate from the owned Settings projection so a
    // successful seed is attributed to this screen mapping below, not migrated
    // as a legacy word by Settings normalization.
    let mut dictionary_probe = snapshot.dictionary.clone();
    let mut unused_pairs = Vec::new();
    if let Err(error) = seed_lexicon_from_screen(
        &dir,
        &mut dictionary_probe,
        &screen,
        &mut unused_pairs,
        scope.as_ref(),
        permissions,
    ) {
        log::warn!("screen lexicon seed failed: {error}");
        return;
    }
    let screen_source = store::DictionarySource::Screen(mapping_id.to_owned());
    for token in screen.asr_terms(permissions) {
        let token = token.trim();
        let key = pair_key("", token);
        let is_live_screen_rule = match store::get_learn_pair(&dir, &key) {
            Ok(row) => row.is_some_and(|row| {
                row.is_live_promoted()
                    && row.before_surface.is_empty()
                    && store::dictionary_words_equal(&row.after_surface, token)
                    && row.mapping_id.as_deref() == Some(mapping_id)
            }),
            Err(error) => {
                log::warn!("screen dictionary source verification failed: {error}");
                return;
            }
        };
        if is_live_screen_rule && store::dictionary_contains_word(&dictionary_probe, token) {
            snapshot.add_dictionary_source(token, screen_source.clone());
        }
    }
    let outcome =
        match save_settings_with_dictionary_effects(&dir, &mut snapshot, false, true, |settings| {
            store::save_settings(&dir, settings)
        }) {
            Ok(outcome) => outcome,
            Err(error) => {
                log::warn!("screen lexicon persist failed: {error}");
                return;
            }
        };
    if !outcome.saved {
        return;
    }
    crate::commit_learning_settings_snapshot(&state, snapshot.clone());
    if let Some(error) = outcome.sync_error {
        log::warn!("screen dictionary effect acknowledgement remains pending: {error}");
    }
    let _ = app.emit("settings://changed", store::SettingsView::from(&snapshot));
    let _ = app.emit("learn_pairs://changed", ());
}

pub fn promote_hits_for(class: LearnClass) -> u32 {
    match class {
        LearnClass::Name => NAME_PROMOTE_HITS,
        LearnClass::Term => PROMOTE_HITS,
        LearnClass::Rejected => u32::MAX,
    }
}

fn is_case_only_change(before: &str, after: &str) -> bool {
    !before.is_empty()
        && before.chars().any(|ch| ch.is_ascii_alphabetic())
        && before.eq_ignore_ascii_case(after)
}

fn is_rejected_learn_surface(after: &str) -> bool {
    let folded = fold_learn_surface(after);
    REJECTED_LEARN_SURFACES
        .iter()
        .any(|item| fold_learn_surface(item) == folded)
}

fn fold_learn_surface(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '-' && *ch != '_')
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn looks_like_cjk_personal_name(after: &str) -> bool {
    let chars: Vec<char> = after.chars().collect();
    if chars.is_empty() || !chars.iter().copied().all(is_cjk) {
        return false;
    }
    match chars.len() {
        2 => is_single_cjk_surname(chars[0]),
        3 => is_single_cjk_surname(chars[0]) || is_compound_cjk_surname(&[chars[0], chars[1]]),
        _ => false,
    }
}

fn is_single_cjk_surname(ch: char) -> bool {
    SINGLE_CJK_SURNAMES.contains(&ch)
}

fn is_compound_cjk_surname(chars: &[char; 2]) -> bool {
    COMPOUND_CJK_SURNAMES
        .iter()
        .any(|name| name.chars().eq(chars.iter().copied()))
}

const REJECTED_LEARN_SURFACES: &[&str] = &[
    "um",
    "uh",
    "er",
    "ah",
    "hmm",
    "mm",
    "mhm",
    "yeah",
    "yup",
    "like",
    "basically",
    "actually",
    "kinda",
    "sorta",
    "嗯",
    "啊",
    "呃",
    "额",
    "哦",
    "哈",
    "哈哈",
    "嗯嗯",
    "那个",
    "就是",
    "就是说",
    "然后",
    "这个",
    "什么",
    "怎么",
    "一下",
    "对对",
    "the",
    "a",
    "an",
    "and",
    "or",
    "but",
    "if",
    "of",
    "to",
    "in",
    "on",
    "at",
    "for",
    "is",
    "are",
    "was",
    "were",
    "be",
    "been",
    "this",
    "that",
    "it",
    "we",
    "they",
    "you",
    "因为",
    "所以",
    "如果",
    "虽然",
    "但是",
    "而且",
    "不是",
    "没有",
    "可以",
    "应该",
    "需要",
];

const SINGLE_CJK_SURNAMES: &[char] = &[
    '李', '王', '张', '刘', '陈', '杨', '黄', '赵', '吴', '周', '徐', '孙', '马', '朱', '胡', '郭',
    '何', '高', '林', '罗', '郑', '梁', '谢', '宋', '唐', '许', '韩', '冯', '邓', '曹', '彭', '曾',
    '肖', '田', '董', '袁', '潘', '于', '蒋', '蔡', '余', '杜', '叶', '程', '魏', '苏', '吕', '丁',
    '任', '沈', '姚', '卢', '姜', '崔', '钟', '谭', '陆', '汪', '范', '金', '石', '廖', '贾', '夏',
    '韦', '付', '方', '白', '邹', '孟', '熊', '秦', '邱', '江', '尹', '薛', '闫', '段', '雷', '侯',
    '龙', '史', '陶', '黎', '贺', '顾', '毛', '郝', '龚', '邵', '万', '钱', '严', '覃', '武', '戴',
    '莫', '孔', '向', '汤',
];

const COMPOUND_CJK_SURNAMES: &[&str] = &[
    "欧阳", "司马", "上官", "诸葛", "司徒", "夏侯", "尉迟", "公孙", "慕容", "长孙", "宇文", "司空",
    "端木", "东方", "独孤", "南宫", "皇甫", "闻人",
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LearnedRuleKind {
    Word,
    Style,
    Intensity,
}
pub(crate) fn learned_rule_kind(key: &str) -> LearnedRuleKind {
    if key.starts_with("learn:style:") {
        LearnedRuleKind::Style
    } else if key.starts_with("learn:intensity:") {
        LearnedRuleKind::Intensity
    } else {
        LearnedRuleKind::Word
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordPairResult {
    Pending { hits: u32 },
    Promoted { after: String },
    AlreadyPromoted,
    Ignored,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum ManualHistoryLearningStatus {
    NotApplicable,
    Pending { hits: u32, required: u32 },
    Promoted,
    AlreadyPromoted,
    PartialSettingsFailure,
    PartialDictionarySyncFailure,
}

#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub struct LearnPairTable {
    by_key: HashMap<String, LearnPairRow>,
}

#[cfg(test)]
#[derive(Clone, Debug)]
struct LearnPairRow {
    hits: u32,
    promoted: bool,
}

#[cfg(test)]
impl LearnPairTable {
    pub fn hits(&self, pair_key: &str) -> Option<u32> {
        self.by_key.get(pair_key).map(|row| row.hits)
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}

pub fn token_key(surface: &str) -> String {
    surface.to_lowercase()
}

pub fn pair_key(before: &str, after: &str) -> String {
    format!("{}\u{1e}{}", token_key(before), token_key(after))
}

#[cfg(test)]
pub fn record_pair(table: &mut LearnPairTable, before: &str, after: &str) -> RecordPairResult {
    if after == before {
        return RecordPairResult::Ignored;
    }
    let class = classify_learn_pair(before, after);
    if class == LearnClass::Rejected {
        return RecordPairResult::Ignored;
    }
    let threshold = promote_hits_for(class);
    let key = pair_key(before, after);
    let row = table.by_key.entry(key).or_insert(LearnPairRow {
        hits: 0,
        promoted: false,
    });
    if row.promoted {
        return RecordPairResult::AlreadyPromoted;
    }
    row.hits = row.hits.saturating_add(1);
    if row.hits >= threshold {
        row.hits = threshold;
        row.promoted = true;
        return RecordPairResult::Promoted {
            after: after.to_string(),
        };
    }
    RecordPairResult::Pending { hits: row.hits }
}

#[cfg(test)]
pub fn record_learn_pair(
    dir: &std::path::Path,
    dictionary: &mut Vec<String>,
    before: &str,
    after: &str,
) -> anyhow::Result<RecordPairResult> {
    record_learn_pair_with_scope(dir, dictionary, before, after, None)
}

pub fn record_learn_pair_with_scope(
    dir: &std::path::Path,
    dictionary: &mut Vec<String>,
    before: &str,
    after: &str,
    scope: Option<&store::LearnPairScope>,
) -> anyhow::Result<RecordPairResult> {
    record_learn_pair_with_scope_audited(dir, dictionary, before, after, scope, None)
}

/// Creates one explicit research-fixture proof for unit tests that exercise
/// the independent-origin promotion threshold. Ordinary test calls through
/// `record_learn_pair_with_scope` remain proof-free, matching production.
#[cfg(test)]
pub(crate) fn record_test_proof_pair(
    dir: &std::path::Path,
    dictionary: &mut Vec<String>,
    before: &str,
    after: &str,
    scope: Option<&store::LearnPairScope>,
) -> anyhow::Result<RecordPairResult> {
    assert!(!before.is_empty(), "screen seeds are not dictation proofs");
    static TEST_ORIGIN_SEQUENCE: AtomicU64 = AtomicU64::new(1);
    let source_origin = format!(
        "dictionary-learning-test:{}",
        TEST_ORIGIN_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let observation = store::LearnPairObservation::research_fixture_origin(
        &source_origin,
        &pair_key(before, after),
    );
    record_learn_pair_with_observation_audited(
        dir,
        dictionary,
        before,
        after,
        scope,
        Some(&observation),
        false,
        None,
    )
}
fn record_learn_pair_with_scope_audited(
    dir: &std::path::Path,
    dictionary: &mut Vec<String>,
    before: &str,
    after: &str,
    scope: Option<&store::LearnPairScope>,
    audit: Option<&mut AuditAttempt>,
) -> anyhow::Result<RecordPairResult> {
    record_learn_pair_with_observation_audited(
        dir, dictionary, before, after, scope, None, false, audit,
    )
}

#[allow(clippy::too_many_arguments)]
fn record_learn_pair_with_observation_audited(
    dir: &std::path::Path,
    dictionary: &mut Vec<String>,
    before: &str,
    after: &str,
    scope: Option<&store::LearnPairScope>,
    observation: Option<&store::LearnPairObservation>,
    auto_scoped: bool,
    mut audit: Option<&mut AuditAttempt>,
) -> anyhow::Result<RecordPairResult> {
    let selected =
        crate::learning_research::consent().filter(|consent| consent.term(before, after).is_some());
    let old_row = if selected.is_some() {
        store::get_learn_pair(dir, &pair_key(before, after))?
    } else {
        None
    };
    let classification = classify_learn_pair_decision(before, after);
    let class = classification.class;
    let threshold = promote_hits_for(class);
    let result = (|| -> anyhow::Result<RecordPairResult> {
        if let Some(audit) = audit.as_deref_mut() {
            audit.candidate(
                "not_attempted",
                Some(classification.clone()),
                PersistenceFactV1::untouched(),
            );
        }
        if after == before || class == LearnClass::Rejected {
            if let Some(audit) = audit.as_deref_mut() {
                audit.candidate(
                    "rejected",
                    Some(classification.clone()),
                    PersistenceFactV1::untouched(),
                );
            }
            return Ok(RecordPairResult::Ignored);
        }
        let key = pair_key(before, after);
        if let Some(existing) = store::get_learn_pair(dir, &key)? {
            if let Some(audit) = audit.as_deref_mut() {
                audit.pair(&existing);
            }
            let recorded_scope = store::LearnPairScope {
                family: existing.family.clone(),
                mapping_id: existing.mapping_id.clone(),
                browser_host: existing.browser_host.clone(),
                native_bundle: existing.native_bundle.clone(),
            };
            if recorded_scope != scope.cloned().unwrap_or_default()
                || existing.before_surface != before
                || existing.after_surface != after
            {
                if let Some(audit) = audit.as_deref_mut() {
                    audit.candidate(
                        "scope_conflict",
                        Some(classification.clone()),
                        PersistenceFactV1::untouched(),
                    );
                }
                return Ok(RecordPairResult::Ignored);
            }
            if existing.ignored || existing.tombstoned_at.is_some() {
                if let Some(audit) = audit.as_deref_mut() {
                    audit.candidate(
                        "tombstone_or_ignored",
                        Some(classification.clone()),
                        PersistenceFactV1::untouched(),
                    );
                }
                return Ok(RecordPairResult::Ignored);
            }
            if existing.promoted {
                if let Some(observation) = observation {
                    match upsert_candidate_row_with_observation(
                        dir,
                        &key,
                        before,
                        after,
                        scope,
                        Some(observation),
                    )? {
                        store::LearnPairUpsert::Stored(row) => {
                            if let Some(audit) = audit.as_deref_mut() {
                                audit.pair(&row);
                                audit.candidate(
                                    "already_db_promoted",
                                    Some(classification.clone()),
                                    PersistenceFactV1::completed("candidate_db_returned"),
                                );
                            }
                        }
                        store::LearnPairUpsert::Skipped(reason) => {
                            if let Some(audit) = audit.as_deref_mut() {
                                audit.candidate(
                                    reason.code(),
                                    Some(classification.clone()),
                                    PersistenceFactV1::untouched(),
                                );
                            }
                        }
                    }
                } else if let Some(audit) = audit.as_deref_mut() {
                    audit.candidate(
                        "already_db_promoted",
                        Some(classification.clone()),
                        PersistenceFactV1::untouched(),
                    );
                }
                return Ok(RecordPairResult::AlreadyPromoted);
            }
            let evidence_hits = promotion_evidence_hits(dir, &key, existing.hits, observation)?;
            if evidence_hits >= threshold {
                return try_promote_learn_pair(
                    dir,
                    dictionary,
                    before,
                    after,
                    &key,
                    scope,
                    threshold,
                    auto_scoped,
                    audit.as_deref_mut(),
                );
            }
        }
        let row = match upsert_candidate_row_with_observation(
            dir,
            &key,
            before,
            after,
            scope,
            observation,
        )? {
            store::LearnPairUpsert::Stored(row) => {
                if let Some(audit) = audit.as_deref_mut() {
                    audit.pair(&row);
                    audit.candidate(
                        "pending",
                        Some(classification.clone()),
                        PersistenceFactV1::completed("candidate_db_returned"),
                    );
                }
                row
            }
            store::LearnPairUpsert::Skipped(reason) => {
                if let Some(audit) = audit.as_deref_mut() {
                    audit.candidate(
                        reason.code(),
                        Some(classification.clone()),
                        PersistenceFactV1::untouched(),
                    );
                }
                if reason == store::LearnPairSkipReason::DuplicateObservation {
                    let current = store::get_learn_pair(dir, &key)?;
                    if let Some(current) = current.filter(|row| {
                        row.is_pending()
                            && row.before_surface == before
                            && row.after_surface == after
                            && store::LearnPairScope {
                                family: row.family.clone(),
                                mapping_id: row.mapping_id.clone(),
                                browser_host: row.browser_host.clone(),
                                native_bundle: row.native_bundle.clone(),
                            } == scope.cloned().unwrap_or_default()
                    }) {
                        let hits = promotion_evidence_hits(dir, &key, current.hits, observation)?;
                        return Ok(RecordPairResult::Pending { hits });
                    }
                }
                return Ok(RecordPairResult::Ignored);
            }
        };
        let evidence_hits = promotion_evidence_hits(dir, &key, row.hits, observation)?;
        if evidence_hits < threshold {
            return Ok(RecordPairResult::Pending {
                hits: evidence_hits,
            });
        }
        try_promote_learn_pair(
            dir,
            dictionary,
            before,
            after,
            &key,
            scope,
            threshold,
            auto_scoped,
            audit.as_deref_mut(),
        )
    })();
    if let (Err(_), Some(audit)) = (&result, audit) {
        audit.candidate(
            "unknown",
            Some(classification.clone()),
            PersistenceFactV1::failed(),
        );
    }
    if let Some(consent) = selected {
        let mut record = crate::learning_research::LearningRecordV1::new(
            &consent,
            LearningStage::CandidateStore,
            if result.is_ok() {
                "record_owner_returned"
            } else {
                "store_error"
            },
        );
        record.term_fixture_id = consent
            .term(before, after)
            .map(|term| term.fixture_id.clone());
        record.candidate_class = Some(match class {
            LearnClass::Rejected => "rejected",
            LearnClass::Name => "name",
            LearnClass::Term => "term",
        });
        record.promote_threshold = (class != LearnClass::Rejected).then_some(threshold);
        record.hits_before = Some(old_row.as_ref().map_or(0, |row| row.hits));
        match store::get_learn_pair(dir, &pair_key(before, after)) {
            Ok(row) => {
                record.hits_after = Some(row.as_ref().map_or(0, |row| row.hits));
                record.promoted = row.as_ref().map(|row| row.is_live_promoted());
                record.persistence = "candidate_db_readback_settings_not_proven";
                use sha2::{Digest, Sha256};
                record.persistence_version = serde_json::to_vec(&row)
                    .ok()
                    .map(|bytes| hex::encode(Sha256::digest(bytes)));
            }
            Err(_) => record.persistence = "candidate_db_readback_failed",
        }
        if old_row
            .as_ref()
            .is_some_and(|row| row.ignored || row.tombstoned_at.is_some())
        {
            record.guard_effect = "tombstone_or_ignored_preserved";
        }
        crate::learning_research::emit(record);
    }
    result
}

fn promotion_evidence_hits(
    dir: &std::path::Path,
    pair_key: &str,
    legacy_hits: u32,
    observation: Option<&store::LearnPairObservation>,
) -> anyhow::Result<u32> {
    if observation.is_some() {
        store::count_independent_learn_pair_hits(dir, pair_key)
    } else {
        Ok(legacy_hits)
    }
}

fn upsert_style_candidate_row_audited_with_observation(
    dir: &std::path::Path,
    key: &str,
    before: &str,
    after: &str,
    scope: Option<&store::LearnPairScope>,
    observation: Option<&store::LearnPairObservation>,
    audit: Option<&mut AuditAttempt>,
) -> anyhow::Result<store::LearnPairUpsert> {
    let result = store::upsert_style_learning_pair_with_observation(
        dir,
        key,
        before,
        after,
        scope,
        observation,
    );
    if let Some(audit) = audit {
        match &result {
            Ok(store::LearnPairUpsert::Stored(row)) => {
                audit.pair(row);
                audit.candidate(
                    "pending",
                    None,
                    PersistenceFactV1::completed("candidate_db_returned"),
                );
            }
            Ok(store::LearnPairUpsert::Skipped(reason)) => {
                audit.candidate(reason.code(), None, PersistenceFactV1::untouched())
            }
            Err(_) => audit.candidate("unknown", None, PersistenceFactV1::failed()),
        }
    }
    if let Ok(store::LearnPairUpsert::Skipped(reason)) = &result {
        crate::learning_research::stage(LearningStage::StoreGuard, reason.code());
    }
    result
}
fn upsert_candidate_row_with_observation(
    dir: &std::path::Path,
    key: &str,
    before: &str,
    after: &str,
    scope: Option<&store::LearnPairScope>,
    observation: Option<&store::LearnPairObservation>,
) -> anyhow::Result<store::LearnPairUpsert> {
    let result =
        store::upsert_learn_pair_with_observation(dir, key, before, after, scope, observation)?;
    if let store::LearnPairUpsert::Skipped(reason) = &result {
        crate::learning_research::stage(LearningStage::StoreGuard, reason.code());
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn try_promote_learn_pair(
    dir: &std::path::Path,
    dictionary: &mut Vec<String>,
    before: &str,
    after: &str,
    key: &str,
    scope: Option<&store::LearnPairScope>,
    threshold: u32,
    auto_scoped: bool,
    mut audit: Option<&mut AuditAttempt>,
) -> anyhow::Result<RecordPairResult> {
    if scope.is_some_and(scope_is_unknown) {
        if let Some(audit) = audit.as_deref_mut() {
            audit.candidate("unknown_scope", None, PersistenceFactV1::untouched());
        }
        return Ok(RecordPairResult::Pending { hits: threshold });
    }
    let already_in_dictionary = store::dictionary_contains_word(dictionary, after);
    if !auto_scoped
        && !already_in_dictionary
        && (dictionary.len() >= 256 || after.trim().is_empty() || after.len() > 256)
    {
        return Ok(RecordPairResult::Pending { hits: threshold });
    }
    let promoted = store::promote_learn_pair_if_eligible(
        dir,
        key,
        before,
        after,
        scope,
        threshold,
        auto_scoped,
    );
    // The caller already validated scope for fast paths. Retrieve it again
    // here only to keep the store's promotion guard tied to the recorded row.
    let promoted = match promoted {
        Ok(promoted) => promoted,
        Err(error) => {
            if let Some(audit) = audit.as_deref_mut() {
                audit.candidate("db_promotion_failed", None, PersistenceFactV1::failed());
            }
            return Err(error);
        }
    };
    if !promoted {
        let current = store::get_learn_pair(dir, key)?;
        if current.as_ref().is_some_and(|row| {
            row.before_surface == before
                && row.after_surface == after
                && store::LearnPairScope {
                    family: row.family.clone(),
                    mapping_id: row.mapping_id.clone(),
                    browser_host: row.browser_host.clone(),
                    native_bundle: row.native_bundle.clone(),
                } == scope.cloned().unwrap_or_default()
                && row.is_live_promoted()
        }) {
            return Ok(RecordPairResult::AlreadyPromoted);
        }
        return Ok(RecordPairResult::Ignored);
    }
    let inserted = if already_in_dictionary {
        false
    } else {
        append_dictionary_entry(dictionary, after)
    };
    if let Some(audit) = audit {
        audit.candidate(
            "db_promoted",
            None,
            PersistenceFactV1::completed("db_promotion_recorded"),
        );
    }
    if already_in_dictionary || !inserted {
        Ok(RecordPairResult::AlreadyPromoted)
    } else {
        Ok(RecordPairResult::Promoted {
            after: after.to_string(),
        })
    }
}

#[cfg(test)]
pub(crate) fn add_dictionary_words(dictionary: &mut Vec<String>, words: &[String]) -> usize {
    words
        .iter()
        .filter(|word| append_dictionary_entry(dictionary, word))
        .count()
}

#[cfg(test)]
pub(crate) fn remove_dictionary_entry(dictionary: &mut Vec<String>, word: &str) -> bool {
    let before = dictionary.len();
    dictionary.retain(|item| !store::dictionary_words_equal(item, word));
    dictionary.len() != before
}

pub(crate) fn append_dictionary_entry(dictionary: &mut Vec<String>, word: &str) -> bool {
    let word = word.trim();
    if word.is_empty() || word.len() > 256 {
        return false;
    }
    if store::dictionary_contains_word(dictionary, word) {
        return false;
    }
    if dictionary.len() >= 256 {
        crate::learning_research::stage(LearningStage::StoreGuard, "dictionary_cap");
        return false;
    }
    dictionary.push(word.to_string());
    true
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn maybe_observe_after_paste(
    app: &tauri::AppHandle,
    state: &AppState,
    history_transaction_id: &str,
    post_insert_field: Option<&str>,
    inserted_text: &str,
    confirmed_insertion: Option<crate::paste::ConfirmedInsertionV1>,
    observation_ticket: Option<crate::paste::LearningObservationTicket>,
    target_guard: &context::TargetAppGuard,
    recording_context: Option<&context::ContextSnapshot>,
    expected_observer_epoch: u64,
    session_generation: u64,
    cancellation: &tokio_util::sync::CancellationToken,
) {
    let confirmed_insertion = confirmed_insertion.filter(|fact| {
        fact.readback == crate::delivery_diagnostics::PasteVerificationFact::Verified
    });
    let verified = confirmed_insertion.is_some();
    let mut audit = AuditAttempt::observation(app, verified, expected_observer_epoch);
    let Some(confirmed_insertion) = confirmed_insertion else {
        audit.skip("unverified");
        crate::learning_research::registration(false, "unverified");
        return;
    };
    let Some(observation_ticket) = observation_ticket else {
        audit.skip("read_owner_unavailable");
        crate::learning_research::registration(false, "read_owner_unavailable");
        return;
    };
    if !confirmed_insertion.is_bound_to(&observation_ticket) {
        audit.skip("delivery_owner_mismatch");
        crate::learning_research::registration(false, "delivery_owner_mismatch");
        return;
    }
    if std::time::Instant::now() >= observation_ticket.expires_at() {
        audit.skip("read_lease_expired");
        crate::learning_research::registration(false, "read_lease_expired");
        return;
    }
    if target_guard.secure_input || confirmed_insertion.target_identity.secure_input {
        audit.skip("secure");
        crate::learning_research::registration(false, "secure");
        return;
    }
    let fixture = crate::learning_research::consent();
    if fixture
        .as_ref()
        .is_some_and(|consent| !consent.observer_enabled)
    {
        audit.skip("no_grant");
        crate::learning_research::registration(false, "no_grant");
        return;
    }
    let Some(baseline) = post_insert_field.filter(|value| !value.is_empty()) else {
        audit.skip("unreadable");
        crate::learning_research::registration(false, "unreadable");
        return;
    };
    let inserted_text_utf16_length = u32::try_from(inserted_text.encode_utf16().count()).ok();
    let baseline_utf16_length = u32::try_from(baseline.encode_utf16().count()).ok();
    let replaced_end = confirmed_insertion
        .replaced_range_before
        .start
        .checked_add(confirmed_insertion.replaced_range_before.length);
    let inserted_end = confirmed_insertion
        .inserted_range_after
        .start
        .checked_add(confirmed_insertion.inserted_range_after.length);
    let expected_after_length = confirmed_insertion
        .source_field_utf16_length
        .checked_sub(confirmed_insertion.replaced_range_before.length)
        .and_then(|prefix| {
            confirmed_insertion
                .inserted_text_utf16_length
                .checked_add(prefix)
        });
    if inserted_text_utf16_length != Some(confirmed_insertion.inserted_text_utf16_length)
        || confirmed_insertion.inserted_range_after.length
            != confirmed_insertion.inserted_text_utf16_length
        || confirmed_insertion.inserted_range_after.start
            != confirmed_insertion.replaced_range_before.start
        || baseline_utf16_length != Some(confirmed_insertion.inserted_field_utf16_length)
        || replaced_end.is_none_or(|end| end > confirmed_insertion.source_field_utf16_length)
        || inserted_end.is_none_or(|end| end > confirmed_insertion.inserted_field_utf16_length)
        || expected_after_length != Some(confirmed_insertion.inserted_field_utf16_length)
        || context::same_field_mismatch_reason(
            &confirmed_insertion.target_identity,
            target_guard,
            false,
        )
        .is_some()
    {
        audit.skip("delivery_fact_mismatch");
        crate::learning_research::registration(false, "delivery_fact_mismatch");
        return;
    }
    if fixture
        .as_ref()
        .is_some_and(|consent| consent.post_insert_field != baseline)
    {
        audit.skip("fixture_baseline_mismatch");
        crate::learning_research::registration(false, "fixture_baseline_mismatch");
        return;
    }
    if inserted_segment_for_observation(
        baseline,
        inserted_text,
        baseline,
        Some(confirmed_insertion.inserted_range_after),
    )
    .is_none()
    {
        audit.skip("ambiguous_inserted_range");
        crate::learning_research::registration(false, "ambiguous_inserted_range");
        return;
    }
    let observer_epoch = Arc::clone(&state.learning_observer_epoch);
    let originating_session_is_current = {
        let manager = lock_recover(&state.manager);
        manager.phase == crate::dictation::Phase::Processing
            && manager.session_generation == session_generation
            && !manager.cancellation.is_cancelled()
    };
    if !learning_observer_may_register(
        &observer_epoch,
        expected_observer_epoch,
        cancellation.is_cancelled(),
        originating_session_is_current,
    ) {
        audit.skip("stale_or_cancelled");
        crate::learning_research::registration(false, "stale_or_cancelled");
        return;
    }
    let settings = lock_recover(&state.settings).clone();
    if !settings.dictionary_learn_enabled || !settings.context_enabled {
        audit.skip("no_grant");
        crate::learning_research::registration(false, "no_grant");
        return;
    }
    let expected_context = recording_context.cloned();
    if lexicon::is_default_learn_off_target(
        target_guard.bundle_id.as_deref(),
        target_guard.browser_host.as_deref(),
    ) {
        audit.skip("protected_target");
        crate::learning_research::registration(false, "protected_target");
        return;
    }
    let Some(mapping) = recording_context.and_then(|snapshot| {
        lexicon::mapping_for_profile(&settings.context_mappings, &snapshot.profile.id)
    }) else {
        audit.skip("no_mapping");
        crate::learning_research::registration(false, "no_mapping");
        return;
    };
    if !mapping.enabled
        || !mapping.dictionary_learn_enabled
        || !(mapping.source_permissions.ax_text || settings.local_edit_learning_enabled)
    {
        audit.skip("ax_or_learning_not_allowed");
        crate::learning_research::registration(false, "ax_or_learning_not_allowed");
        return;
    }
    if !crate::lexicon::scene_allows_learn(
        &settings.context_mappings,
        recording_context
            .map(|snapshot| snapshot.profile.id.as_str())
            .unwrap_or(""),
        target_guard.bundle_id.as_deref(),
        target_guard.browser_host.as_deref(),
    ) {
        audit.skip("scene_not_allowed");
        crate::learning_research::registration(false, "scene_not_allowed");
        return;
    }
    let scope = recording_context.map(scope_from_snapshot);
    let local_edit_only =
        !mapping.source_permissions.ax_text && settings.local_edit_learning_enabled;
    let mapping_id = recording_context
        .and_then(|snapshot| style_draft_mapping_id(snapshot, &settings.context_mappings));
    let (mappings, browser_access_enabled) = {
        let current = lock_recover(&state.context);
        (current.mappings.clone(), current.browser_access_enabled)
    };
    let settings_for_check = settings.clone();
    let baseline = baseline.to_owned();
    let inserted_text = inserted_text.to_owned();
    let history_transaction_id = history_transaction_id.to_owned();
    let expected = confirmed_insertion.target_identity.clone();
    let inserted_range_after = confirmed_insertion.inserted_range_after;
    let observation_deadline = observation_ticket.expires_at();
    let observer_cancellation = cancellation.clone();
    let app = app.clone();
    let task = crate::research_trace::learning_task();
    crate::research_trace::spawn_blocking(move || {
        let _task = task;
        if !learning_observer_is_current(&observer_epoch, expected_observer_epoch)
            || !crate::research_trace::allowed()
        {
            audit.skip("stale_or_cancelled");
            crate::learning_research::registration(false, "stale_or_cancelled");
            return;
        }
        if expected_context
            .as_ref()
            .is_none_or(|snapshot| !crate::context_target_is_current(snapshot, &settings_for_check))
        {
            audit.skip("target_changed");
            crate::learning_research::registration(false, "target_changed");
            return;
        }
        let read_target = || {
            if browser_access_enabled {
                context::detect_snapshot(&mappings, true).target_guard
            } else {
                context::probe_focus_guard()
            }
        };
        let initial_target = read_target();
        if context::same_field_mismatch_reason(&expected, &initial_target, browser_access_enabled)
            .is_some()
        {
            audit.skip("target_changed");
            crate::learning_research::registration(false, "target_changed");
            return;
        }
        if std::time::Instant::now() >= observation_deadline {
            audit.skip("read_lease_expired");
            crate::learning_research::registration(false, "read_lease_expired");
            return;
        }
        audit.registered();
        crate::learning_research::registration(true, "registered");
        let unselected_edit = std::cell::Cell::new(false);
        let readable_samples = std::cell::Cell::new(0usize);
        let read_failure = std::cell::Cell::new(None);
        // The original delivery owner established this focus identity. Every
        // later read is served by that same retained anchor and updates this
        // fact only after before/after focus checks succeed.
        let last_focus_matches = std::cell::Cell::new(Some(true));
        let outcome = observe_after_paste_with_identity(
            &baseline,
            &inserted_text,
            &expected,
            ObserveOptions {
                dictionary_learn_enabled: true,
                require_browser_identity: browser_access_enabled,
                inserted_range_after: Some(inserted_range_after),
                limits: ObserveLimits::default(),
            },
            || {
                if observer_cancellation.is_cancelled() {
                    read_failure.set(Some(crate::paste::FocusedFieldReadError::Cancelled));
                    return None;
                }
                if std::time::Instant::now() >= observation_deadline {
                    read_failure.set(Some(crate::paste::FocusedFieldReadError::Expired));
                    return None;
                }
                let sample = match observation_ticket.read_sample(
                    &observer_cancellation,
                    crate::paste::MAX_FULL_RANGE_READBACK_UTF16,
                    Arc::clone(&observer_epoch),
                    expected_observer_epoch,
                ) {
                    Ok(sample) => sample,
                    Err(error) => {
                        read_failure.set(Some(error));
                        return None;
                    }
                };
                last_focus_matches.set(sample.focus_matches);
                if sample.focus_matches != Some(true) {
                    read_failure.set(Some(match sample.focus_matches {
                        Some(false) => crate::paste::FocusedFieldReadError::FocusChanged,
                        _ => crate::paste::FocusedFieldReadError::FocusUnknown,
                    }));
                    return None;
                }
                let value = sample.value;
                let Some(value) = value else {
                    read_failure.set(Some(crate::paste::FocusedFieldReadError::ValueUnavailable));
                    return None;
                };
                if fixture.as_ref().is_some_and(|consent| {
                    value != consent.post_insert_field && value != consent.settled_field
                }) {
                    unselected_edit.set(true);
                    return None;
                }
                readable_samples.set(readable_samples.get().saturating_add(1));
                Some(value)
            },
            read_target,
            std::thread::sleep,
            || last_focus_matches.get() == Some(true),
            || {
                learning_observer_is_current(&observer_epoch, expected_observer_epoch)
                    && crate::research_trace::allowed()
                    && !observer_cancellation.is_cancelled()
                    && std::time::Instant::now() < observation_deadline
                    && read_failure.get().is_none()
                    && !unselected_edit.get()
            },
        );
        if let Some(error) = read_failure.get() {
            let (audit_outcome, stage) = match error {
                crate::paste::FocusedFieldReadError::Cancelled => {
                    audit.stale();
                    (None, "stale_or_cancelled")
                }
                crate::paste::FocusedFieldReadError::StaleEpoch => {
                    audit.stale();
                    (None, "stale_or_cancelled")
                }
                crate::paste::FocusedFieldReadError::Expired => {
                    audit.stale();
                    (None, "read_lease_expired")
                }
                crate::paste::FocusedFieldReadError::FocusChanged => {
                    (Some("target_changed"), "target_changed")
                }
                crate::paste::FocusedFieldReadError::FocusUnknown => {
                    (Some("unreadable"), "read_focus_unknown")
                }
                crate::paste::FocusedFieldReadError::ValueUnavailable => {
                    (Some("unreadable"), "read_value_unavailable")
                }
                crate::paste::FocusedFieldReadError::Busy => {
                    (Some("unreadable"), "read_queue_full")
                }
                crate::paste::FocusedFieldReadError::OwnerClosed => {
                    (Some("unreadable"), "read_owner_closed")
                }
                crate::paste::FocusedFieldReadError::TimedOut => {
                    (Some("unreadable"), "read_timeout")
                }
                crate::paste::FocusedFieldReadError::OwnerFailed => {
                    (Some("unreadable"), "read_owner_failed")
                }
            };
            if let Some(outcome) = audit_outcome {
                audit.observed(outcome);
                audit.finish();
            }
            crate::learning_research::stage(LearningStage::Observation, stage);
            return;
        }
        if unselected_edit.get() {
            crate::learning_research::stage(LearningStage::Observation, "unselected_edit");
            return;
        }
        if !crate::research_trace::allowed()
            || !learning_observer_is_current(&observer_epoch, expected_observer_epoch)
            || observer_cancellation.is_cancelled()
            || std::time::Instant::now() >= observation_deadline
        {
            audit.stale();
            crate::learning_research::stage(
                LearningStage::Observation,
                if std::time::Instant::now() >= observation_deadline {
                    "read_lease_expired"
                } else {
                    "stale_or_cancelled"
                },
            );
            return;
        }
        if readable_samples.get() == 0 && matches!(&outcome, ObserveOutcome::Unchanged) {
            audit.observed("unreadable");
            audit.finish();
            crate::learning_research::stage(LearningStage::Observation, "unreadable");
            return;
        }
        audit.observed(match &outcome {
            ObserveOutcome::Unchanged => "nochange",
            ObserveOutcome::LeftTarget => "target_changed",
            ObserveOutcome::SingleToken { .. } => "single_token",
            ObserveOutcome::MultipleTokens { .. } => "multiple_tokens",
            ObserveOutcome::ShortStyleRewrite { .. } => "short_style_rewrite",
            ObserveOutcome::StyleSignal { .. } => "style_signal",
            ObserveOutcome::Ambiguous => "ambiguous",
        });
        if let Some(consent) = &fixture {
            let mut record = crate::learning_research::LearningRecordV1::new(
                consent,
                LearningStage::Observation,
                match &outcome {
                    ObserveOutcome::Unchanged => "nochange",
                    ObserveOutcome::LeftTarget => "target_changed",
                    ObserveOutcome::SingleToken { .. } => "single_token",
                    ObserveOutcome::MultipleTokens { .. } => "multiple_tokens_unsupported",
                    ObserveOutcome::ShortStyleRewrite { .. } => "short_style_rewrite",
                    ObserveOutcome::StyleSignal { .. } => "style_signal",
                    ObserveOutcome::Ambiguous => "ambiguous",
                },
            );
            record.same_field_window = if matches!(&outcome, ObserveOutcome::LeftTarget) {
                "left_target"
            } else {
                "same_field_owner_completed"
            };
            crate::learning_research::emit(record);
        }
        match outcome {
            ObserveOutcome::SingleToken { before_span, after } => {
                if !crate::learning_research::pair_allowed(&before_span, &after) {
                    crate::learning_research::stage(
                        LearningStage::Classification,
                        "term_fixture_mismatch",
                    );
                    return;
                }
                if learning_observer_is_current(&observer_epoch, expected_observer_epoch) {
                    if local_edit_only {
                        let Some(suggestion) = history_lexical_correction(&before_span, &after)
                        else {
                            crate::learning_research::stage(
                                LearningStage::Classification,
                                "local_edit_guard_rejected",
                            );
                            audit.finish();
                            return;
                        };
                        persist_learn_pair(
                            &app,
                            &suggestion.before_span,
                            &suggestion.after,
                            scope,
                            history_transaction_id.clone(),
                            observation_ticket,
                            observer_cancellation.clone(),
                            session_generation,
                            Arc::clone(&observer_epoch),
                            expected_observer_epoch,
                            audit,
                        );
                    } else if automatic_word_pair_allowed(&before_span, &after) {
                        persist_learn_pair(
                            &app,
                            &before_span,
                            &after,
                            scope,
                            history_transaction_id.clone(),
                            observation_ticket,
                            observer_cancellation.clone(),
                            session_generation,
                            Arc::clone(&observer_epoch),
                            expected_observer_epoch,
                            audit,
                        );
                    } else {
                        crate::learning_research::stage(
                            LearningStage::Classification,
                            "lexical_pair_not_eligible",
                        );
                        audit.finish();
                    }
                } else {
                    audit.stale();
                }
            }
            ObserveOutcome::MultipleTokens {
                before,
                after,
                corrections,
            } => {
                if local_edit_only {
                    let _ = history_lexical_correction(&before, &after);
                    crate::learning_research::stage(
                        LearningStage::Classification,
                        "local_edit_guard_rejected",
                    );
                    audit.finish();
                    return;
                }
                if fixture.is_some() {
                    crate::learning_research::stage(
                        LearningStage::Classification,
                        "multiple_tokens_unsupported_in_single_pair_fixture",
                    );
                    audit.finish();
                    return;
                }
                let mut seen = HashSet::new();
                let corrections = corrections
                    .into_iter()
                    .filter(|correction| seen.insert(correction.pair_key.clone()))
                    .collect::<Vec<_>>();
                if corrections.len() < 2 {
                    audit.finish();
                    return;
                }
                if learning_observer_is_current(&observer_epoch, expected_observer_epoch) {
                    persist_learn_pairs(
                        &app,
                        corrections,
                        scope,
                        history_transaction_id.clone(),
                        observation_ticket,
                        observer_cancellation.clone(),
                        session_generation,
                        Arc::clone(&observer_epoch),
                        expected_observer_epoch,
                        audit,
                    );
                } else {
                    audit.stale();
                }
            }
            ObserveOutcome::ShortStyleRewrite { before, after } => {
                if learning_observer_is_current(&observer_epoch, expected_observer_epoch) {
                    persist_style_intensity_learn(
                        &app,
                        &before,
                        &after,
                        mapping_id,
                        observation_ticket,
                        observer_cancellation.clone(),
                        session_generation,
                        Arc::clone(&observer_epoch),
                        expected_observer_epoch,
                        audit,
                    );
                } else {
                    audit.stale();
                }
            }
            ObserveOutcome::StyleSignal {
                excerpt,
                style_key,
                before_excerpt,
                after_excerpt,
            } => {
                if let Some(mapping_id) = mapping_id.filter(|_| {
                    learning_observer_is_current(&observer_epoch, expected_observer_epoch)
                }) {
                    persist_style_draft(
                        &app,
                        &mapping_id,
                        &style_key,
                        &excerpt,
                        &before_excerpt,
                        &after_excerpt,
                        observation_ticket,
                        observer_cancellation.clone(),
                        Arc::clone(&observer_epoch),
                        expected_observer_epoch,
                        audit,
                    );
                } else {
                    audit.stale();
                }
            }
            _ => audit.finish(),
        }
    });
}

fn scope_from_snapshot(snapshot: &context::ContextSnapshot) -> store::LearnPairScope {
    store::LearnPairScope::from_snapshot(snapshot)
}

pub(crate) fn style_draft_mapping_id(
    snapshot: &context::ContextSnapshot,
    mappings: &[context::AppMapping],
) -> Option<String> {
    if let Some(id) = snapshot.profile.id.strip_prefix("user.") {
        return Some(id.to_owned());
    }
    if let Some(host) = snapshot.target_guard.browser_host.as_deref() {
        if let Some(mapping) = mappings
            .iter()
            .find(|mapping| mapping.browser_host.as_deref() == Some(host))
        {
            return Some(mapping.id.clone());
        }
        return Some(format!("host:{host}"));
    }
    if let Some(bundle) = snapshot.target_guard.bundle_id.as_deref() {
        if let Some(mapping) = mappings
            .iter()
            .find(|mapping| mapping.bundle_id.as_deref() == Some(bundle))
        {
            return Some(mapping.id.clone());
        }
        return Some(format!("bundle:{bundle}"));
    }
    None
}

fn paste_session_observation_id(session_generation: u64) -> String {
    static PROCESS_NAMESPACE: OnceLock<String> = OnceLock::new();
    let namespace = PROCESS_NAMESPACE.get_or_init(|| {
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("{}-{started}", std::process::id())
    });
    format!("{namespace}:{session_generation}")
}

fn history_scope_matches_mapping(
    scene: &store::HistoryScene,
    mapping: &context::AppMapping,
) -> bool {
    let profile_mapping = scene
        .profile_id
        .as_deref()
        .and_then(|profile| profile.strip_prefix("user."));
    if profile_mapping != Some(mapping.id.as_str())
        || scene.family.as_deref() != Some(context::family_id(mapping.family))
    {
        return false;
    }
    match (
        scene.browser_host.as_deref(),
        scene.native_bundle.as_deref(),
    ) {
        (Some(host), None) => {
            mapping
                .browser_host
                .as_deref()
                .and_then(context::normalize_host)
                == context::normalize_host(host)
        }
        (None, Some(bundle)) => mapping.bundle_id.as_deref() == Some(bundle),
        _ => false,
    }
}

/// Explicit History edits are a local source the user has already chosen to
/// save. They do not require AX text-context permission and never become scene
/// style evidence or cloud/provider context.
pub(crate) async fn learn_from_manual_history_edit(
    app: &tauri::AppHandle,
    state: &AppState,
    history_id: i64,
    revision_id: i64,
    previous_text: &str,
    edited_text: &str,
    scene: &store::HistoryScene,
) -> anyhow::Result<ManualHistoryLearningStatus> {
    let Some(suggestion) = history_lexical_correction(previous_text, edited_text) else {
        return Ok(ManualHistoryLearningStatus::NotApplicable);
    };
    let _gate = state.settings_gate.lock().await;
    let mut snapshot = lock_recover(&state.settings).clone();
    if !snapshot.dictionary_learn_enabled || !snapshot.context_enabled {
        return Ok(ManualHistoryLearningStatus::NotApplicable);
    }
    let scope = scene.learn_scope();
    let Some(mapping_id) = scope.mapping_id.as_deref() else {
        return Ok(ManualHistoryLearningStatus::NotApplicable);
    };
    let Some(mapping) = snapshot
        .context_mappings
        .iter()
        .find(|mapping| mapping.id == mapping_id)
    else {
        return Ok(ManualHistoryLearningStatus::NotApplicable);
    };
    if !mapping.enabled
        || !mapping.dictionary_learn_enabled
        || !history_scope_matches_mapping(scene, mapping)
        || (scope.browser_host.is_some() && !snapshot.browser_access_enabled)
        || lexicon::is_default_learn_off_target(
            mapping.bundle_id.as_deref(),
            mapping.browser_host.as_deref(),
        )
    {
        return Ok(ManualHistoryLearningStatus::NotApplicable);
    }
    let dir = app.path().app_data_dir()?;
    let Some(observation) = store::LearnPairObservation::history_edit_from_store(
        &dir,
        history_id,
        revision_id,
        &suggestion.pair_key,
    )?
    else {
        return Ok(ManualHistoryLearningStatus::NotApplicable);
    };
    // History-derived corrections stay in their verified scene pair; do not
    // append their target term to the global personal dictionary.
    let mut scoped_dictionary = Vec::new();
    let class = classify_learn_pair_decision(&suggestion.before_span, &suggestion.after).class;
    let required = promote_hits_for(class);
    let result = record_learn_pair_with_observation_audited(
        &dir,
        &mut scoped_dictionary,
        &suggestion.before_span,
        &suggestion.after,
        Some(&scope),
        Some(&observation),
        true,
        None,
    )?;
    let settings_outcome =
        match save_settings_with_dictionary_effects(&dir, &mut snapshot, false, false, |settings| {
            store::save_settings(&dir, settings)
        }) {
            Ok(saved) => saved,
            Err(error) if error.starts_with(DICTIONARY_SETTINGS_PARTIAL_ERROR) => {
                return Ok(ManualHistoryLearningStatus::PartialSettingsFailure);
            }
            Err(error) => return Err(anyhow::anyhow!(error)),
        };
    if settings_outcome.saved {
        crate::commit_learning_settings_snapshot(state, snapshot.clone());
        let _ = app.emit("settings://changed", store::SettingsView::from(&snapshot));
    }
    if settings_outcome.sync_error.is_some() {
        return Ok(ManualHistoryLearningStatus::PartialDictionarySyncFailure);
    }
    match result {
        RecordPairResult::Promoted { .. } => {
            let _ = app.emit("learn_pairs://changed", ());
            let _ = app.emit(
                "learn_pairs://promoted",
                serde_json::json!({
                    "pair_key": suggestion.pair_key,
                    "before": suggestion.before_span,
                    "after": suggestion.after,
                }),
            );
            crate::island_window::show_overlay(app);
            crate::island_window::set_learn_toast_interactive(app, true);
            Ok(ManualHistoryLearningStatus::Promoted)
        }
        RecordPairResult::Pending { hits } => {
            let _ = app.emit("learn_pairs://changed", ());
            Ok(ManualHistoryLearningStatus::Pending { hits, required })
        }
        RecordPairResult::AlreadyPromoted => Ok(ManualHistoryLearningStatus::AlreadyPromoted),
        RecordPairResult::Ignored => Ok(ManualHistoryLearningStatus::NotApplicable),
    }
}

fn mapping_from_style_draft_id(mapping_id: &str) -> Option<context::AppMapping> {
    if let Some(bundle) = mapping_id.strip_prefix("bundle:") {
        return Some(context::AppMapping {
            id: mapping_id.to_owned(),
            label: bundle_style_label(bundle),
            family: bundle_style_family(bundle),
            mode_id: None,
            bundle_id: Some(bundle.to_owned()),
            executable: None,
            browser_host: None,
            browser_path_prefix: None,
            focused_field: None,
            source_permissions: Default::default(),
            style_example_input: None,
            style_example_output: None,
            style_example_pairs: Vec::new(),
            style_examples_approved: false,
            enabled: true,
            cleanup_effort: None,
            cleanup_intensity: None,
            cleanup_enabled: true,
            dictionary_learn_enabled: true,
        });
    }
    if let Some(host) = mapping_id.strip_prefix("host:") {
        return Some(context::AppMapping {
            id: mapping_id.to_owned(),
            label: host.to_owned(),
            family: context::ContextFamily::General,
            mode_id: None,
            bundle_id: None,
            executable: None,
            browser_host: Some(host.to_owned()),
            browser_path_prefix: None,
            focused_field: None,
            source_permissions: Default::default(),
            style_example_input: None,
            style_example_output: None,
            style_example_pairs: Vec::new(),
            style_examples_approved: false,
            enabled: true,
            cleanup_effort: None,
            cleanup_intensity: None,
            cleanup_enabled: true,
            dictionary_learn_enabled: true,
        });
    }
    None
}

fn bundle_style_label(bundle: &str) -> String {
    match bundle {
        "com.tencent.xinWeChat" => "WeChat".into(),
        "com.apple.MobileSMS" => "Messages".into(),
        "com.hnc.Discord" => "Discord".into(),
        other => other.to_owned(),
    }
}

fn bundle_style_family(bundle: &str) -> context::ContextFamily {
    match bundle {
        "com.tencent.xinWeChat" | "com.apple.MobileSMS" | "com.hnc.Discord" => {
            context::ContextFamily::PersonalChat
        }
        _ => context::ContextFamily::General,
    }
}

#[allow(clippy::too_many_arguments)]
fn persist_learn_pair(
    app: &tauri::AppHandle,
    before: &str,
    after: &str,
    scope: Option<store::LearnPairScope>,
    history_transaction_id: String,
    observation_ticket: crate::paste::LearningObservationTicket,
    cancellation: tokio_util::sync::CancellationToken,
    session_generation: u64,
    observer_epoch: Arc<AtomicU64>,
    expected_observer_epoch: u64,
    mut audit: AuditAttempt,
) {
    let app = app.clone();
    let before = before.to_string();
    let after = after.to_string();
    let pair_key = pair_key(&before, &after);
    let observation = store::LearnPairObservation::paste_session_with_dictation_origin(
        &paste_session_observation_id(session_generation),
        &pair_key,
        &history_transaction_id,
    );
    crate::research_trace::learning_spawn(async move {
        persist_learn_pair_locked(
            &app,
            &before,
            &after,
            scope.as_ref(),
            &observation,
            &observation_ticket,
            &cancellation,
            observer_epoch,
            expected_observer_epoch,
            &mut audit,
        )
        .await;
        audit.finish();
    });
}

#[allow(clippy::too_many_arguments)]
fn persist_learn_pairs(
    app: &tauri::AppHandle,
    corrections: Vec<DictionarySuggestion>,
    scope: Option<store::LearnPairScope>,
    history_transaction_id: String,
    observation_ticket: crate::paste::LearningObservationTicket,
    cancellation: tokio_util::sync::CancellationToken,
    session_generation: u64,
    observer_epoch: Arc<AtomicU64>,
    expected_observer_epoch: u64,
    audit: AuditAttempt,
) {
    let app = app.clone();
    crate::research_trace::learning_spawn(async move {
        let session = paste_session_observation_id(session_generation);
        let mut seen = HashSet::new();
        let mut first_audit = Some(audit);
        for correction in corrections {
            if !seen.insert(correction.pair_key.clone()) {
                continue;
            }
            let mut pair_audit = first_audit.take().unwrap_or_else(|| {
                let mut audit = AuditAttempt::observation(&app, true, expected_observer_epoch);
                audit.registered();
                audit.observed("multiple_tokens");
                audit
            });
            if cancellation.is_cancelled()
                || !learning_observer_is_current(&observer_epoch, expected_observer_epoch)
                || !crate::research_trace::allowed()
                || std::time::Instant::now() >= observation_ticket.expires_at()
            {
                pair_audit.stale();
                pair_audit.finish();
                break;
            }
            let observation = store::LearnPairObservation::paste_session_with_dictation_origin(
                &session,
                &correction.pair_key,
                &history_transaction_id,
            );
            persist_learn_pair_locked(
                &app,
                &correction.before_span,
                &correction.after,
                scope.as_ref(),
                &observation,
                &observation_ticket,
                &cancellation,
                Arc::clone(&observer_epoch),
                expected_observer_epoch,
                &mut pair_audit,
            )
            .await;
            pair_audit.finish();
        }
        if let Some(mut audit) = first_audit {
            audit.finish();
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn persist_style_intensity_learn(
    app: &tauri::AppHandle,
    baseline: &str,
    settled: &str,
    mapping_id: Option<String>,
    observation_ticket: crate::paste::LearningObservationTicket,
    cancellation: tokio_util::sync::CancellationToken,
    session_generation: u64,
    observer_epoch: Arc<AtomicU64>,
    expected_observer_epoch: u64,
    mut audit: AuditAttempt,
) {
    let Some(mapping_id) = mapping_id else {
        audit.finish();
        return;
    };
    let app = app.clone();
    let baseline = baseline.to_owned();
    let settled = settled.to_owned();
    crate::research_trace::learning_spawn(async move {
        persist_style_intensity_learn_locked(
            &app,
            &baseline,
            &settled,
            &mapping_id,
            &observation_ticket,
            &cancellation,
            session_generation,
            observer_epoch,
            expected_observer_epoch,
            &mut audit,
        )
        .await;
        audit.finish();
    });
}

#[allow(clippy::too_many_arguments)]
async fn persist_style_intensity_learn_locked(
    app: &tauri::AppHandle,
    baseline: &str,
    settled: &str,
    mapping_id: &str,
    observation_ticket: &crate::paste::LearningObservationTicket,
    cancellation: &tokio_util::sync::CancellationToken,
    session_generation: u64,
    observer_epoch: Arc<AtomicU64>,
    expected_observer_epoch: u64,
    audit: &mut AuditAttempt,
) {
    let state = app.state::<AppState>();
    let _gate = state.settings_gate.lock().await;
    if !learning_observer_is_current(&observer_epoch, expected_observer_epoch)
        || !crate::research_trace::allowed()
        || cancellation.is_cancelled()
        || std::time::Instant::now() >= observation_ticket.expires_at()
    {
        audit.stale();
        crate::learning_research::stage(
            LearningStage::Persistence,
            if cancellation.is_cancelled() {
                "stale_or_cancelled"
            } else if std::time::Instant::now() >= observation_ticket.expires_at() {
                "read_lease_expired"
            } else {
                "stale_or_cancelled"
            },
        );
        return;
    }
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    let mut snapshot = lock_recover(&state.settings).clone();
    if !snapshot.dictionary_learn_enabled || !snapshot.context_enabled {
        return;
    }
    let Some(mapping) = snapshot
        .context_mappings
        .iter()
        .find(|mapping| mapping.id == mapping_id)
    else {
        return;
    };
    if !mapping.enabled
        || !mapping.dictionary_learn_enabled
        || !(mapping.source_permissions.ax_text || snapshot.local_edit_learning_enabled)
        || lexicon::is_default_learn_off_target(
            mapping.bundle_id.as_deref(),
            mapping.browser_host.as_deref(),
        )
    {
        return;
    }
    let global = crate::llm::CleanupIntensity::parse(&snapshot.cleanup_intensity)
        .unwrap_or(crate::llm::CleanupIntensity::Heavy);
    if cancellation.is_cancelled() || std::time::Instant::now() >= observation_ticket.expires_at() {
        audit.stale();
        crate::learning_research::stage(
            LearningStage::Persistence,
            if cancellation.is_cancelled() {
                "stale_or_cancelled"
            } else if std::time::Instant::now() >= observation_ticket.expires_at() {
                "read_lease_expired"
            } else {
                "stale_or_cancelled"
            },
        );
        return;
    }
    let session_id = paste_session_observation_id(session_generation);
    let applied = match apply_post_paste_style_learn_for_session(
        &mut snapshot.context_mappings,
        &dir,
        mapping_id,
        baseline,
        settled,
        global,
        Some(&session_id),
        Some(&mut *audit),
    ) {
        Ok(applied) => applied,
        Err(error) => {
            audit.candidate_store(PersistenceFactV1::failed());
            crate::learning_research::stage(LearningStage::StyleStore, "persist_failed");
            log::warn!("style learn persist failed: {error}");
            return;
        }
    };
    crate::learning_research::stage(
        LearningStage::StyleStore,
        if applied.style_draft_key.is_some() {
            "draft_persisted_unapproved"
        } else if applied.intensity_changed {
            "intensity_changed_settings_pending"
        } else {
            "candidate_no_settings_change"
        },
    );
    if !applied.pairs_changed && !applied.intensity_changed {
        if applied.style_draft_key.is_some() {
            let _ = app.emit("style_drafts://changed", ());
        }
        return;
    }
    snapshot.normalize();
    if let Err(error) = audit.settings_result(store::save_settings(&dir, &snapshot)) {
        crate::learning_research::stage(LearningStage::SettingsPersistence, "failed");
        log::warn!("style learn settings persist failed: {error}");
        return;
    }
    crate::commit_learning_settings_snapshot(&state, snapshot.clone());
    crate::learning_research::stage(
        LearningStage::SettingsPersistence,
        "save_settings_completed",
    );
    {
        let mut context = lock_recover(&state.context);
        context.mappings = snapshot.context_mappings.clone();
    }
    let _ = app.emit("settings://changed", store::SettingsView::from(&snapshot));
    let _ = app.emit("learn_pairs://changed", ());
    if applied.style_draft_key.is_some() {
        let _ = app.emit("style_drafts://changed", ());
    }
    let pair_keys = promoted_pair_keys(&applied);
    if let Some(pair_key) = pair_keys
        .first()
        .filter(|_| applied.pairs_changed || applied.intensity_changed)
    {
        let _ = app.emit(
            "learn_pairs://promoted",
            serde_json::json!({
                "pair_key": pair_key,
                "pair_keys": pair_keys,
                "before": baseline,
                "after": settled,
            }),
        );
        crate::island_window::show_overlay(app);
        crate::island_window::set_learn_toast_interactive(app, true);
    }
}

fn promoted_pair_keys(applied: &StyleLearnApply) -> Vec<String> {
    let mut keys = Vec::new();
    if applied.pairs_changed {
        keys.extend(applied.style_pair_key.clone());
    }
    if applied.intensity_changed {
        keys.extend(applied.intensity_pair_key.clone());
    }
    keys
}

#[allow(clippy::too_many_arguments)]
fn persist_style_draft(
    app: &tauri::AppHandle,
    mapping_id: &str,
    style_key: &str,
    excerpt: &str,
    before_excerpt: &str,
    after_excerpt: &str,
    observation_ticket: crate::paste::LearningObservationTicket,
    cancellation: tokio_util::sync::CancellationToken,
    observer_epoch: Arc<AtomicU64>,
    expected_observer_epoch: u64,
    mut audit: AuditAttempt,
) {
    let app = app.clone();
    let mapping_id = mapping_id.to_owned();
    let style_key = style_key.to_owned();
    let excerpt = excerpt.to_owned();
    let before_excerpt = before_excerpt.to_owned();
    let after_excerpt = after_excerpt.to_owned();
    crate::research_trace::learning_spawn(async move {
        let state = app.state::<AppState>();
        let _gate = state.settings_gate.lock().await;
        if !learning_observer_is_current(&observer_epoch, expected_observer_epoch)
            || !crate::research_trace::allowed()
            || cancellation.is_cancelled()
            || std::time::Instant::now() >= observation_ticket.expires_at()
        {
            audit.stale();
            crate::learning_research::stage(
                LearningStage::Persistence,
                if cancellation.is_cancelled() {
                    "stale_or_cancelled"
                } else if std::time::Instant::now() >= observation_ticket.expires_at() {
                    "read_lease_expired"
                } else {
                    "stale_or_cancelled"
                },
            );
            audit.finish();
            return;
        }
        let settings = lock_recover(&state.settings);
        if !settings.dictionary_learn_enabled || !settings.context_enabled {
            audit.finish();
            return;
        }
        let Some(mapping) = settings
            .context_mappings
            .iter()
            .find(|mapping| mapping.id == mapping_id)
        else {
            audit.finish();
            return;
        };
        if !mapping.enabled
            || !mapping.dictionary_learn_enabled
            || !(mapping.source_permissions.ax_text || settings.local_edit_learning_enabled)
            || lexicon::is_default_learn_off_target(
                mapping.bundle_id.as_deref(),
                mapping.browser_host.as_deref(),
            )
        {
            audit.finish();
            return;
        }
        let Ok(dir) = app.path().app_data_dir() else {
            audit.finish();
            return;
        };
        if cancellation.is_cancelled()
            || std::time::Instant::now() >= observation_ticket.expires_at()
        {
            audit.stale();
            crate::learning_research::stage(
                LearningStage::Persistence,
                if cancellation.is_cancelled() {
                    "stale_or_cancelled"
                } else if std::time::Instant::now() >= observation_ticket.expires_at() {
                    "read_lease_expired"
                } else {
                    "stale_or_cancelled"
                },
            );
            audit.finish();
            return;
        }
        if store::upsert_style_draft(
            &dir,
            &mapping_id,
            &style_key,
            &excerpt,
            &before_excerpt,
            &after_excerpt,
        )
        .is_ok()
        {
            audit.candidate_store(PersistenceFactV1::completed("style_draft_returned"));
            crate::learning_research::stage(
                LearningStage::StyleStore,
                "draft_persisted_unapproved",
            );
            let _ = app.emit("style_drafts://changed", ());
        } else {
            audit.candidate_store(PersistenceFactV1::failed());
            crate::learning_research::stage(LearningStage::StyleStore, "draft_persist_failed");
        }
        audit.finish();
    });
}

#[allow(clippy::too_many_arguments)]
async fn persist_learn_pair_locked(
    app: &tauri::AppHandle,
    before: &str,
    after: &str,
    scope: Option<&store::LearnPairScope>,
    observation: &store::LearnPairObservation,
    observation_ticket: &crate::paste::LearningObservationTicket,
    cancellation: &tokio_util::sync::CancellationToken,
    observer_epoch: Arc<AtomicU64>,
    expected_observer_epoch: u64,
    audit: &mut AuditAttempt,
) {
    let state = app.state::<AppState>();
    let _gate = state.settings_gate.lock().await;
    if !learning_observer_is_current(&observer_epoch, expected_observer_epoch)
        || !crate::research_trace::allowed()
        || cancellation.is_cancelled()
        || std::time::Instant::now() >= observation_ticket.expires_at()
    {
        audit.stale();
        crate::learning_research::stage(
            LearningStage::Persistence,
            if cancellation.is_cancelled() {
                "stale_or_cancelled"
            } else if std::time::Instant::now() >= observation_ticket.expires_at() {
                "read_lease_expired"
            } else {
                "stale_or_cancelled"
            },
        );
        return;
    }
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    let mut snapshot = lock_recover(&state.settings).clone();
    if !snapshot.dictionary_learn_enabled || !snapshot.context_enabled {
        return;
    }
    let Some(mapping_id) = scope.and_then(|scope| scope.mapping_id.as_deref()) else {
        return;
    };
    let Some(mapping) = snapshot
        .context_mappings
        .iter()
        .find(|mapping| mapping.id == mapping_id)
    else {
        return;
    };
    if !mapping.enabled
        || !mapping.dictionary_learn_enabled
        || !(mapping.source_permissions.ax_text || snapshot.local_edit_learning_enabled)
        || lexicon::is_default_learn_off_target(
            mapping.bundle_id.as_deref(),
            mapping.browser_host.as_deref(),
        )
    {
        return;
    }
    let auto_scoped = !mapping.source_permissions.ax_text && snapshot.local_edit_learning_enabled;
    // The local-only grant creates a scene preference; it never adds the
    // target to the global personal dictionary. Existing AX text permission
    // keeps its prior global-dictionary behavior. The Vec remains a capacity
    // probe; only the pending Rule effect may change the owned Settings list.
    let mut scoped_dictionary = Vec::new();
    let mut global_dictionary_probe = if auto_scoped {
        Vec::new()
    } else {
        let effects = match store::list_pending_dictionary_effects(&dir) {
            Ok(effects) => effects,
            Err(error) => {
                log::warn!("dictionary pending effect capacity check failed: {error}");
                crate::learning_research::stage(
                    LearningStage::SettingsPersistence,
                    "pending_effect_read_failed",
                );
                return;
            }
        };
        let mut projected_settings = snapshot.clone();
        if apply_pending_dictionary_effects(&mut projected_settings, &effects).is_err() {
            log::warn!("dictionary pending effects exceed the current capacity");
            crate::learning_research::stage(
                LearningStage::SettingsPersistence,
                "pending_effect_capacity_blocked",
            );
            return;
        }
        projected_settings.dictionary
    };
    let dictionary = if auto_scoped {
        &mut scoped_dictionary
    } else {
        &mut global_dictionary_probe
    };
    if cancellation.is_cancelled() || std::time::Instant::now() >= observation_ticket.expires_at() {
        audit.stale();
        crate::learning_research::stage(
            LearningStage::Persistence,
            if cancellation.is_cancelled() {
                "stale_or_cancelled"
            } else if std::time::Instant::now() >= observation_ticket.expires_at() {
                "read_lease_expired"
            } else {
                "stale_or_cancelled"
            },
        );
        return;
    }
    let result = match record_learn_pair_with_observation_audited(
        &dir,
        dictionary,
        before,
        after,
        scope,
        Some(observation),
        auto_scoped,
        Some(&mut *audit),
    ) {
        Ok(result) => result,
        Err(error) => {
            log::warn!("dictionary learn pair failed: {error}");
            return;
        }
    };
    let database_change_committed = matches!(result, RecordPairResult::Promoted { .. });
    let settings_outcome = match save_settings_with_dictionary_effects(
        &dir,
        &mut snapshot,
        database_change_committed,
        false,
        |settings| store::save_settings(&dir, settings),
    ) {
        Ok(saved) => saved,
        Err(error) => {
            let _ = audit.settings_result(Err::<(), anyhow::Error>(anyhow::anyhow!(error.clone())));
            crate::learning_research::stage(LearningStage::SettingsPersistence, "failed");
            log::warn!("dictionary learn persist failed: {error}");
            return;
        }
    };
    if settings_outcome.saved {
        if let Err(error) = audit.settings_result(Ok::<(), anyhow::Error>(())) {
            log::warn!("dictionary learn audit persistence failed: {error}");
        }
        crate::commit_learning_settings_snapshot(&state, snapshot.clone());
        crate::learning_research::stage(
            LearningStage::SettingsPersistence,
            "save_settings_completed",
        );
        let _ = app.emit("settings://changed", store::SettingsView::from(&snapshot));
    }
    if let Some(error) = settings_outcome.sync_error {
        log::warn!("dictionary effect acknowledgement remains pending: {error}");
    }
    match result {
        RecordPairResult::Promoted { .. } => {
            let _ = app.emit("learn_pairs://changed", ());
            let _ = app.emit(
                "learn_pairs://promoted",
                serde_json::json!({
                    "pair_key": pair_key(before, after),
                    "before": before,
                    "after": after,
                }),
            );
            crate::island_window::show_overlay(app);
            crate::island_window::set_learn_toast_interactive(app, true);
        }
        RecordPairResult::Pending { .. } => {
            let _ = app.emit("learn_pairs://changed", ());
        }
        RecordPairResult::AlreadyPromoted | RecordPairResult::Ignored => {}
    }
}

fn is_blocked(token: &str) -> bool {
    token.contains("***") || looks_like_url(token)
}

fn looks_like_url(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    lower.contains("://") || lower.starts_with("www.") || lower.contains('/')
}

pub(crate) fn is_latin_start(value: char) -> bool {
    value.is_ascii_alphabetic()
}

pub(crate) fn is_latin_cont(value: char) -> bool {
    value.is_ascii_alphanumeric() || matches!(value, '.' | '_' | '-')
}

pub(crate) fn is_cjk(value: char) -> bool {
    matches!(
        value,
        '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{20000}'..='\u{2A6DF}'
            | '\u{2A700}'..='\u{2B73F}'
            | '\u{2B740}'..='\u{2B81F}'
            | '\u{2B820}'..='\u{2CEAF}'
            | '\u{3005}'..='\u{3007}'
            | '\u{3040}'..='\u{309F}'
            | '\u{30A0}'..='\u{30FF}'
            | '\u{AC00}'..='\u{D7AF}'
            | '\u{FF66}'..='\u{FF9D}'
    )
}

#[cfg(test)]
mod tests {
    use super::{
        add_dictionary_words, append_dictionary_entry, apply_post_paste_style_learn,
        confirm_scope_is_unknown, intensity_shift_from_edit, is_short_style_rewrite,
        observe_after_paste, push_style_pair, remove_dictionary_entry, scope_is_unknown,
        seed_lexicon_from_screen, single_token_candidates, ObserveLimits, ObserveOutcome,
    };
    use crate::context::TargetAppGuard;
    use std::cell::{Cell, RefCell};
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    #[test]
    fn mutate_dictionary_failed_save_leaves_live_settings() {
        let root = temp_dir("failed-dictionary-save");
        std::fs::create_dir_all(&root).unwrap();
        let blocked_dir = root.join("not-a-directory");
        std::fs::write(&blocked_dir, b"blocks directory creation").unwrap();
        let settings = crate::store::Settings {
            dictionary: vec!["Rust".into()],
            ..crate::store::Settings::default()
        };
        let state = crate::AppState::new_dictionary_regression_fixture(
            settings.clone(),
            root.join("models"),
        );
        let before_settings_generation = state.settings_generation.load(Ordering::Acquire);
        let before_processing_generation = state
            .processing_configuration_generation
            .load(Ordering::Acquire);
        let before_context_generation = state.context_policy_generation.load(Ordering::Acquire);

        let error = super::mutate_dictionary_settings(&state, &blocked_dir, |dictionary| {
            dictionary.push("Tokio".into());
        })
        .expect_err("saving into a file path must fail");

        assert!(!error.is_empty());
        assert_eq!(
            serde_json::to_value(&*crate::lock_recover(&state.settings)).unwrap(),
            serde_json::to_value(&settings).unwrap()
        );
        assert_eq!(
            state.settings_generation.load(Ordering::Acquire),
            before_settings_generation
        );
        assert_eq!(
            state
                .processing_configuration_generation
                .load(Ordering::Acquire),
            before_processing_generation
        );
        assert_eq!(
            state.context_policy_generation.load(Ordering::Acquire),
            before_context_generation
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manual_dictionary_add_rejects_oversized_utf8_and_capacity_atomically() {
        let mut settings = crate::store::Settings::default();
        let original = settings.clone();
        let oversized = "界".repeat(86);
        assert_eq!(oversized.len(), 258);
        let error = super::add_manual_dictionary_words(&mut settings, &[oversized])
            .expect_err("terms over the UTF-8 byte limit must be rejected");
        assert!(error.contains("256 UTF-8 bytes"));
        assert_eq!(settings.dictionary, original.dictionary);
        assert_eq!(settings.dictionary_sources, original.dictionary_sources);

        let mut settings = crate::store::Settings {
            dictionary: (0..255).map(|index| format!("term-{index}")).collect(),
            ..crate::store::Settings::default()
        };
        let original = settings.clone();
        let error = super::add_manual_dictionary_words(
            &mut settings,
            &["last-slot".into(), "over-capacity".into()],
        )
        .expect_err("a batch that exceeds capacity must be rejected atomically");
        assert!(error.contains("capacity"));
        assert_eq!(settings.dictionary, original.dictionary);
        assert_eq!(settings.dictionary_sources, original.dictionary_sources);

        let mut exact_capacity = crate::store::Settings::default();
        let words: Vec<String> = (0..256).map(|index| format!("allowed-{index}")).collect();
        super::add_manual_dictionary_words(&mut exact_capacity, &words)
            .expect("exactly 256 unique terms fit");
        assert_eq!(exact_capacity.dictionary.len(), 256);
    }

    #[test]
    fn unrelated_pending_dictionary_effect_is_not_partial_manual_success() {
        let dir = temp_dir("unrelated-dictionary-effect-manual-write");
        let mut persisted = crate::store::Settings::default();
        crate::store::save_configuration(&dir, &persisted).unwrap();
        let before = "VoiceFlo";
        let after = "VoiceFlow";
        let key = super::pair_key(before, after);
        let scope = crate::store::LearnPairScope {
            family: Some("general".into()),
            mapping_id: Some("fixture".into()),
            native_bundle: Some("com.example.fixture".into()),
            ..Default::default()
        };
        assert!(matches!(
            crate::store::upsert_learn_pair_with_scope(&dir, &key, before, after, Some(&scope))
                .unwrap(),
            crate::store::LearnPairUpsert::Stored(_)
        ));
        crate::store::ensure_learn_pair_promoted(&dir, &key, before, after, Some(&scope)).unwrap();
        assert_eq!(
            crate::store::list_pending_dictionary_effects(&dir)
                .unwrap()
                .len(),
            1
        );

        super::add_manual_dictionary_words(&mut persisted, &["manual-term".into()]).unwrap();
        let error = match super::save_settings_with_dictionary_effects(
            &dir,
            &mut persisted,
            false,
            true,
            |_| Err(anyhow::anyhow!("injected Settings write failure")),
        ) {
            Ok(_) => panic!("the failed Settings write must fail this manual addition"),
            Err(error) => error,
        };
        assert_eq!(error, "injected Settings write failure");
        assert!(!error.starts_with(super::DICTIONARY_SETTINGS_PARTIAL_ERROR));

        let saved: crate::store::Settings =
            serde_json::from_slice(&std::fs::read(dir.join("settings.json")).unwrap()).unwrap();
        assert!(!saved.dictionary.iter().any(|word| word == "manual-term"));
        assert!(crate::store::list_pending_dictionary_effects(&dir)
            .unwrap()
            .iter()
            .any(|effect| effect.pair_key == key));

        let mut full_settings = crate::store::Settings {
            dictionary: (0..256).map(|index| format!("full-{index}")).collect(),
            ..crate::store::Settings::default()
        };
        crate::store::save_configuration(&dir, &full_settings).unwrap();
        let error = match super::save_settings_with_dictionary_effects(
            &dir,
            &mut full_settings,
            false,
            true,
            |_| panic!("capacity failure must happen before Settings write"),
        ) {
            Ok(_) => panic!("unrelated pending add cannot exceed a full dictionary"),
            Err(error) => error,
        };
        assert_eq!(error, "dictionary capacity or word validation");
        assert!(!error.starts_with(super::DICTIONARY_SETTINGS_PARTIAL_ERROR));
        assert!(crate::store::list_pending_dictionary_effects(&dir)
            .unwrap()
            .iter()
            .any(|effect| effect.pair_key == key));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learning_during_processing_does_not_cancel_or_leak_lease() {
        let _network_policy_guard = crate::network_policy::strict_offline_test_guard_blocking();
        let root = temp_dir("learning-during-processing");
        let state = crate::AppState::new_dictionary_regression_fixture(
            crate::store::Settings::default(),
            root.join("models"),
        );
        let processing_cancellation = {
            let mut manager = crate::lock_recover(&state.manager);
            manager.phase = crate::Phase::Processing;
            manager.session_generation = 41;
            manager.recording_learning_snapshot =
                Some(crate::dictation::SessionLearningSnapshot::from_settings(
                    &crate::lock_recover(&state.settings),
                ));
            manager.cancellation.clone()
        };
        assert!(crate::try_claim_operation(
            &state,
            crate::OperationLease::LiveDictation
        ));
        let active_configuration_generation = state
            .processing_configuration_generation
            .load(Ordering::Acquire);
        let saved = super::mutate_dictionary_settings(&state, &root, |dictionary| {
            dictionary.push("Tokio".into());
        })
        .expect("isolated settings save should succeed");

        assert!(saved.dictionary.iter().any(|word| word == "Tokio"));
        assert_eq!(
            state
                .processing_configuration_generation
                .load(Ordering::Acquire),
            active_configuration_generation
        );
        assert!(!processing_cancellation.is_cancelled());
        let captured = crate::lock_recover(&state.manager)
            .recording_learning_snapshot
            .clone()
            .expect("active processing retains captured learning settings");
        let mut processing_settings = crate::lock_recover(&state.settings).clone();
        captured.apply_to(&mut processing_settings);
        assert!(processing_settings.dictionary.is_empty());
        assert!(crate::lock_recover(&state.settings)
            .dictionary
            .iter()
            .any(|word| word == "Tokio"));
        assert_eq!(
            *crate::lock_recover(&state.operation_lease),
            crate::OperationLease::LiveDictation
        );

        crate::release_operation(&state, crate::OperationLease::LiveDictation);
        assert!(crate::try_claim_operation(
            &state,
            crate::OperationLease::LiveDictation
        ));
        crate::release_operation(&state, crate::OperationLease::LiveDictation);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn family_only_scope_is_unknown() {
        let scope = crate::store::LearnPairScope {
            family: Some("work_chat".into()),
            ..crate::store::LearnPairScope::default()
        };
        assert!(scope_is_unknown(&scope));
    }

    #[test]
    fn mapping_id_scope_is_known() {
        let scope = crate::store::LearnPairScope {
            family: Some("work_chat".into()),
            mapping_id: Some("codex".into()),
            ..crate::store::LearnPairScope::default()
        };
        assert!(!scope_is_unknown(&scope));
    }

    #[test]
    fn explicit_confirm_requires_a_nonempty_concrete_id() {
        assert!(confirm_scope_is_unknown(
            &crate::store::LearnPairScope::default()
        ));
        let mapping_only = crate::store::LearnPairScope {
            mapping_id: Some("codex".into()),
            ..crate::store::LearnPairScope::default()
        };
        assert!(!confirm_scope_is_unknown(&mapping_only));
        let empty_ids = crate::store::LearnPairScope {
            mapping_id: Some(String::new()),
            browser_host: Some(String::new()),
            native_bundle: Some(String::new()),
            ..crate::store::LearnPairScope::default()
        };
        assert!(confirm_scope_is_unknown(&empty_ids));
    }

    #[test]
    fn record_pair_preserves_global_scope_and_rejects_family_only_scope() {
        let dir = temp_dir("family-only-no-auto-promotion");
        let mut dictionary = Vec::new();
        let family_key = super::pair_key("知呼", "知乎");
        let scope = crate::store::LearnPairScope {
            family: Some("work_chat".into()),
            ..crate::store::LearnPairScope::default()
        };
        for _ in 0..4 {
            super::record_test_proof_pair(&dir, &mut dictionary, "知呼", "知乎", Some(&scope))
                .unwrap();
        }
        let family_only = crate::store::get_learn_pair(&dir, &family_key)
            .unwrap()
            .unwrap();
        assert!(!family_only.is_live_promoted());

        let default_key = super::pair_key("仿佛", "彷彿");
        for _ in 0..4 {
            super::record_test_proof_pair(
                &dir,
                &mut dictionary,
                "仿佛",
                "彷彿",
                Some(&crate::store::LearnPairScope::default()),
            )
            .unwrap();
        }
        assert!(crate::store::get_learn_pair(&dir, &default_key)
            .unwrap()
            .unwrap()
            .is_live_promoted());

        let none_key = super::pair_key("藉由", "借由");
        for _ in 0..4 {
            super::record_test_proof_pair(&dir, &mut dictionary, "藉由", "借由", None).unwrap();
        }
        assert!(crate::store::get_learn_pair(&dir, &none_key)
            .unwrap()
            .unwrap()
            .is_live_promoted());

        let mapping_key = super::pair_key("派森", "Python");
        let mapping_scope = crate::store::LearnPairScope {
            mapping_id: Some("codex".into()),
            ..crate::store::LearnPairScope::default()
        };
        for _ in 0..4 {
            super::record_test_proof_pair(
                &dir,
                &mut dictionary,
                "派森",
                "Python",
                Some(&mapping_scope),
            )
            .unwrap();
        }
        assert!(crate::store::get_learn_pair(&dir, &mapping_key)
            .unwrap()
            .unwrap()
            .is_live_promoted());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn test_target(pid: i32, secure_input: bool) -> TargetAppGuard {
        TargetAppGuard {
            pid,
            bundle_id: Some("com.example.app".into()),
            browser_host: None,
            browser_target_token: None,
            browser_tab_token: None,
            window_token: Some(1),
            window_id: Some(1),
            window_identity_ambiguous: false,
            input_token: Some(1),
            secure_input,
        }
    }

    fn zhihu_token() -> ObserveOutcome {
        ObserveOutcome::SingleToken {
            before_span: "知呼".into(),
            after: "知乎".into(),
        }
    }

    fn observe_with(
        post_insert_field: &str,
        expected: &TargetAppGuard,
        enabled: bool,
        values: Vec<Option<String>>,
        targets: Vec<TargetAppGuard>,
    ) -> ObserveOutcome {
        let last_value = RefCell::new(
            values
                .iter()
                .rev()
                .find_map(|value| value.clone())
                .unwrap_or_else(|| post_insert_field.to_string()),
        );
        let values = RefCell::new(values.into_iter());
        let targets = RefCell::new(targets.into_iter());
        observe_after_paste(
            post_insert_field,
            expected,
            enabled,
            ObserveLimits::default(),
            || match values.borrow_mut().next() {
                Some(Some(value)) => {
                    *last_value.borrow_mut() = value.clone();
                    Some(value)
                }
                Some(None) => None,
                None => Some(last_value.borrow().clone()),
            },
            || {
                targets
                    .borrow_mut()
                    .next()
                    .unwrap_or_else(|| expected.clone())
            },
            |_| {},
        )
    }

    #[test]
    fn add_words_keeps_entries_already_on_the_server() {
        let mut dictionary = vec!["后端已学".to_string()];
        assert_eq!(
            add_dictionary_words(&mut dictionary, &["UI添加".to_string()]),
            1
        );
        assert_eq!(
            dictionary,
            vec!["后端已学".to_string(), "UI添加".to_string()]
        );
    }

    #[test]
    fn remove_word_does_not_rebuild_from_a_stale_list() {
        let mut dictionary = vec!["保留".to_string(), "删除".to_string(), "也保留".to_string()];
        assert!(remove_dictionary_entry(&mut dictionary, "删除"));
        assert_eq!(dictionary, vec!["保留".to_string(), "也保留".to_string()]);
    }

    #[test]
    fn cjk_correction_zhihu() {
        assert_eq!(
            single_token_candidates("知呼", "知乎"),
            vec!["知乎".to_string()]
        );
    }

    #[test]
    fn cjk_sentence_correction_uses_changed_span_not_whole_utterance() {
        assert_eq!(
            single_token_candidates("今天去知呼看看吧", "今天去知乎看看吧"),
            vec!["知乎".to_string()]
        );
        assert!(
            !single_token_candidates("今天去知呼看看吧", "今天去知乎看看吧")
                .contains(&"今天去知乎看看吧".to_string())
        );
        assert!(single_token_candidates("abcdefgh", "今天去知乎看看吧").is_empty());
        assert!(super::dictionary_suggestions("abcdefgh", "今天去知乎看看吧").is_empty());
    }

    #[test]
    fn long_unspaced_sentence_with_one_char_fix_yields_the_word() {
        assert_eq!(
            single_token_candidates("我今天想去知呼看看风景", "我今天想去知乎看看风景"),
            vec!["知乎".to_string()]
        );
    }

    #[test]
    fn latin_correction_python() {
        assert_eq!(
            single_token_candidates("配森", "Python"),
            vec!["Python".to_string()]
        );
    }

    #[test]
    fn paragraph_rewrite_yields_empty() {
        let before = "今天天气不错，我想出去走走，顺便买一杯咖啡。";
        let after = "The quarterly roadmap is delayed, so we should renegotiate the timeline with design and engineering.";
        assert!(single_token_candidates(before, after).is_empty());
    }

    #[test]
    fn mixed_latin_token_typescript() {
        assert_eq!(
            single_token_candidates("类型脚本", "TypeScript"),
            vec!["TypeScript".to_string()]
        );
    }

    #[test]
    fn more_than_three_new_tokens_is_a_rewrite() {
        assert!(single_token_candidates("xx", "Ab Cd Ef Gh").is_empty());
    }

    #[test]
    fn pure_insertions_do_not_create_word_candidates() {
        assert!(single_token_candidates("xx", "Foo Bar Qux").is_empty());
        assert!(single_token_candidates("TypeScrit", "TypeScript and React").is_empty());
    }

    #[test]
    fn skips_urls_and_password_masks() {
        assert!(single_token_candidates("go here", "www.example.com").is_empty());
        assert!(single_token_candidates("secret", "my***token").is_empty());
    }

    #[test]
    fn latin_comparison_is_case_insensitive() {
        assert!(single_token_candidates("python", "Python").is_empty());
    }

    #[test]
    fn pure_insertion_is_not_a_candidate() {
        assert!(single_token_candidates("好的", "好的明天见").is_empty());
        assert_eq!(
            single_token_candidates("知呼", "知乎"),
            vec!["知乎".to_string()]
        );
        assert_eq!(
            single_token_candidates("OK 知呼", "OK 知乎"),
            vec!["知乎".to_string()]
        );
    }

    #[test]
    fn observe_learns_zhihu_correction() {
        let expected = test_target(42, false);
        let learned = observe_with(
            "知呼",
            &expected,
            true,
            vec![Some("知呼".into()), Some("知乎".into())],
            vec![expected.clone(), expected.clone()],
        );
        assert_eq!(learned, zhihu_token());
    }

    #[test]
    fn observe_does_not_learn_pure_insertion_after_paste() {
        let expected = test_target(42, false);
        let learned = observe_with(
            "好的",
            &expected,
            true,
            vec![Some("好的明天见".into())],
            vec![expected.clone()],
        );
        assert_eq!(learned, ObserveOutcome::Ambiguous);
    }

    #[test]
    fn observe_diffs_post_insert_field_not_the_pasted_snippet() {
        let expected = test_target(42, false);
        let learned = observe_with(
            "OK 知呼",
            &expected,
            true,
            vec![Some("OK 知乎".into())],
            vec![expected.clone()],
        );
        assert_eq!(learned, zhihu_token());
    }

    #[test]
    fn observer_only_learns_edits_inside_the_unique_inserted_span() {
        let expected = test_target(42, false);
        let outcome = super::observe_after_paste_with_identity(
            "前文 知呼 后文",
            "知呼",
            &expected,
            super::ObserveOptions {
                dictionary_learn_enabled: true,
                require_browser_identity: false,
                inserted_range_after: None,
                limits: ObserveLimits::default(),
            },
            || Some("前文 知乎 后文".into()),
            || expected.clone(),
            |_| {},
            || true,
            || true,
        );
        assert_eq!(outcome, zhihu_token());

        let outside_span_edit = super::observe_after_paste_with_identity(
            "前文 知呼 后文",
            "知呼",
            &expected,
            super::ObserveOptions {
                dictionary_learn_enabled: true,
                require_browser_identity: false,
                inserted_range_after: None,
                limits: ObserveLimits::default(),
            },
            || Some("改写前文 知乎 后文".into()),
            || expected.clone(),
            |_| {},
            || true,
            || true,
        );
        assert_eq!(outside_span_edit, ObserveOutcome::Ambiguous);
    }

    #[test]
    fn inserted_span_locator_requires_one_exact_unicode_match() {
        assert_eq!(
            super::edited_inserted_segment("前段 🧭 知呼 后段", "知呼", "前段 🧭 知乎 后段"),
            Some("知乎".into())
        );
        assert_eq!(
            super::edited_inserted_segment("前段 知呼 后段 知呼", "知呼", "前段 知乎 后段 知呼"),
            None
        );
        assert_eq!(
            super::edited_inserted_segment("ababa", "aba", "ababa"),
            None,
            "overlapping exact matches are ambiguous"
        );
        assert_eq!(
            super::edited_inserted_segment("前段 知呼 后段", "知呼", "前段 知乎 改写"),
            None
        );
    }

    #[test]
    fn confirmed_utf16_range_anchors_the_original_field_without_searching_text() {
        let prefix = "A🙂 前 ";
        let inserted = "VoiceFlo";
        let baseline = format!("{prefix}{inserted} 后");
        let range = crate::paste::Utf16RangeV1 {
            start: u32::try_from(prefix.encode_utf16().count()).unwrap(),
            length: u32::try_from(inserted.encode_utf16().count()).unwrap(),
        };

        assert_eq!(
            super::inserted_segment_for_observation(
                &baseline,
                inserted,
                "A🙂 前 VoiceFlow 后",
                Some(range),
            ),
            Some("VoiceFlow".into()),
            "the verified UTF-16 offset remains correct after a supplementary-plane prefix"
        );
        assert_eq!(
            super::inserted_segment_for_observation(
                &baseline,
                inserted,
                "B🙂 前 VoiceFlow 后",
                Some(range),
            ),
            None,
            "a changed prefix cannot be relocated by a unique text match"
        );
        assert_eq!(
            super::inserted_segment_for_observation(
                &baseline,
                inserted,
                "A🙂 前 VoiceFlow 旁",
                Some(range),
            ),
            None,
            "a changed suffix cannot be treated as part of the inserted range"
        );
    }

    #[test]
    fn observe_ignores_paragraph_rewrite() {
        let expected = test_target(42, false);
        let before = "今天天气不错，我想出去走走，顺便买一杯咖啡。";
        let after = "The quarterly roadmap is delayed, so we should renegotiate the timeline with design and engineering.";
        let learned = observe_with(
            before,
            &expected,
            true,
            vec![Some(after.into())],
            vec![expected.clone()],
        );
        assert_eq!(learned, ObserveOutcome::Ambiguous);
    }

    #[test]
    fn observe_never_reads_or_learns_after_the_target_changes() {
        let expected = test_target(42, false);
        let changed = test_target(99, false);
        let learned = observe_with(
            "知呼",
            &expected,
            true,
            vec![Some("知乎".into())],
            vec![changed],
        );
        assert_eq!(learned, ObserveOutcome::LeftTarget);
    }

    #[test]
    fn observe_never_reads_or_learns_after_focus_changes() {
        let expected = test_target(42, false);
        let mut changed = expected.clone();
        changed.input_token = Some(99);
        let learned = observe_with(
            "知呼",
            &expected,
            true,
            vec![Some("知乎".into())],
            vec![changed],
        );
        assert_eq!(learned, ObserveOutcome::LeftTarget);
    }

    #[test]
    fn observe_checks_identity_before_and_after_each_field_read() {
        let expected = test_target(42, false);
        let mut changed = expected.clone();
        changed.input_token = Some(99);
        let targets = RefCell::new(vec![expected.clone(), changed].into_iter());
        let reads = RefCell::new(0);
        let outcome = super::observe_after_paste_with_identity(
            "知呼",
            "知呼",
            &expected,
            super::ObserveOptions {
                dictionary_learn_enabled: true,
                require_browser_identity: false,
                inserted_range_after: None,
                limits: ObserveLimits::default(),
            },
            || {
                *reads.borrow_mut() += 1;
                Some("知乎".into())
            },
            || {
                targets
                    .borrow_mut()
                    .next()
                    .unwrap_or_else(|| expected.clone())
            },
            |_| {},
            || true,
            || true,
        );
        assert_eq!(outcome, ObserveOutcome::LeftTarget);
        assert_eq!(*reads.borrow(), 1);
    }

    #[test]
    fn text_action_start_expires_an_earlier_dictation_learning_observer() {
        let expected = test_target(42, false);
        let epoch = AtomicU64::new(0);
        let observer_epoch = epoch.load(Ordering::Acquire);
        let reads = RefCell::new(0);
        let result = super::observe_after_paste_with_identity(
            "deploy v2",
            "deploy v2",
            &expected,
            super::ObserveOptions {
                dictionary_learn_enabled: true,
                require_browser_identity: false,
                inserted_range_after: None,
                limits: ObserveLimits::default(),
            },
            || {
                *reads.borrow_mut() += 1;
                Some("deploy v2 with correction".into())
            },
            || expected.clone(),
            |_| {
                // This is the production invalidation seam called when a
                // text-action hotkey claims its transaction.
                super::invalidate_learning_observers(&epoch);
            },
            || true,
            || super::learning_observer_is_current(&epoch, observer_epoch),
        );
        assert_eq!(result, ObserveOutcome::Unchanged);
        assert_eq!(*reads.borrow(), 0);
        assert!(!super::learning_observer_is_current(&epoch, observer_epoch));
    }

    #[test]
    fn post_delivery_observer_survives_success_but_clear_epoch_stops_reads() {
        let expected = test_target(42, false);
        let epoch = AtomicU64::new(0);
        let observer_epoch = epoch.load(Ordering::Acquire);
        let recording_cancellation = tokio_util::sync::CancellationToken::new();
        let observer_cancellation = crate::new_learning_observer_cancellation();
        recording_cancellation.cancel();
        assert!(!observer_cancellation.is_cancelled());

        let reads = Cell::new(0usize);
        let outcome = super::observe_after_paste_with_identity(
            "前文 知呼 后文",
            "知呼",
            &expected,
            super::ObserveOptions {
                dictionary_learn_enabled: true,
                require_browser_identity: false,
                inserted_range_after: None,
                limits: ObserveLimits::default(),
            },
            || {
                reads.set(reads.get() + 1);
                Some("前文 知乎 后文".into())
            },
            || expected.clone(),
            |_| {},
            || true,
            || {
                !observer_cancellation.is_cancelled()
                    && super::learning_observer_is_current(&epoch, observer_epoch)
            },
        );
        assert_eq!(outcome, zhihu_token());
        assert!(reads.get() > 0);

        let current_epoch = epoch.load(Ordering::Acquire);
        super::invalidate_learning_observers(&epoch);
        let reads_after_clear = Cell::new(0usize);
        let stale_outcome = super::observe_after_paste_with_identity(
            "前文 知呼 后文",
            "知呼",
            &expected,
            super::ObserveOptions {
                dictionary_learn_enabled: true,
                require_browser_identity: false,
                inserted_range_after: None,
                limits: ObserveLimits::default(),
            },
            || {
                reads_after_clear.set(reads_after_clear.get() + 1);
                Some("前文 知乎 后文".into())
            },
            || expected.clone(),
            |_| {},
            || true,
            || super::learning_observer_is_current(&epoch, current_epoch),
        );
        assert_eq!(stale_outcome, ObserveOutcome::Unchanged);
        assert_eq!(reads_after_clear.get(), 0);
    }

    #[test]
    fn late_old_paste_cannot_register_learning_after_text_action_starts() {
        let epoch = AtomicU64::new(0);
        let paste_origin_epoch = epoch.load(Ordering::Acquire);
        let paste_worker_completed = true;
        let mut field_reads = 0;
        let mut learning_writes = 0;

        assert!(paste_worker_completed);
        super::invalidate_learning_observers(&epoch);
        let originating_session_is_current = true;
        if super::learning_observer_may_register(
            &epoch,
            paste_origin_epoch,
            false,
            originating_session_is_current,
        ) {
            // This is the production registration gate. A false result skips
            // both native field reads and the observer's persistence path.
            field_reads += 1;
            learning_writes += 1;
        }

        assert_eq!(field_reads, 0);
        assert_eq!(learning_writes, 0);
    }

    #[test]
    fn cancelled_or_stale_dictation_cannot_register_a_correction_observer() {
        let epoch = AtomicU64::new(4);
        assert!(!super::learning_observer_may_register(
            &epoch, 4, true, true
        ));
        assert!(!super::learning_observer_may_register(
            &epoch, 4, false, false
        ));
    }

    #[test]
    fn observe_does_not_read_when_target_is_already_mismatched() {
        let expected = test_target(42, false);
        let changed = test_target(99, false);
        let outcome = observe_after_paste(
            "知呼",
            &expected,
            true,
            ObserveLimits::default(),
            || panic!("must not read a field from another app"),
            || changed.clone(),
            |_| {},
        );
        assert_eq!(outcome, ObserveOutcome::LeftTarget);
    }

    #[test]
    fn observe_requires_the_retained_native_field_identity() {
        let expected = test_target(42, false);
        let outcome = super::observe_after_paste_with_identity(
            "知呼",
            "知呼",
            &expected,
            super::ObserveOptions {
                dictionary_learn_enabled: true,
                require_browser_identity: false,
                inserted_range_after: None,
                limits: ObserveLimits::default(),
            },
            || panic!("must not read when the AX element changed"),
            || expected.clone(),
            |_| {},
            || false,
            || true,
        );
        assert_eq!(outcome, ObserveOutcome::LeftTarget);
    }

    #[test]
    fn observe_disables_learning_when_field_identity_is_missing() {
        let mut expected = test_target(42, false);
        expected.input_token = None;
        let outcome = observe_after_paste(
            "知呼",
            &expected,
            true,
            ObserveLimits::default(),
            || panic!("must not read without a concrete focused field identity"),
            || panic!("must not probe after the expected identity is unavailable"),
            |_| panic!("must not poll without a concrete focused field identity"),
        );
        assert_eq!(outcome, ObserveOutcome::Unchanged);
    }

    #[test]
    fn observe_ignores_multi_token_rewrite() {
        let expected = test_target(42, false);
        let learned = observe_with(
            "xx",
            &expected,
            true,
            vec![Some("Foo Bar".into())],
            vec![expected.clone()],
        );
        assert_eq!(learned, ObserveOutcome::Ambiguous);
    }

    #[test]
    fn observe_skips_secure_input() {
        let expected = test_target(42, true);
        let learned = observe_after_paste(
            "知呼",
            &expected,
            true,
            ObserveLimits::default(),
            || panic!("must not read focused value for secure_input"),
            || panic!("must not probe target for secure_input"),
            |_| panic!("must not poll for secure_input"),
        );
        assert_eq!(learned, ObserveOutcome::Unchanged);
    }

    #[test]
    fn observe_skips_when_learning_disabled() {
        let expected = test_target(42, false);
        let learned = observe_after_paste(
            "知呼",
            &expected,
            false,
            ObserveLimits::default(),
            || panic!("must not read focused value when learning is off"),
            || panic!("must not probe target when learning is off"),
            |_| panic!("must not poll when learning is off"),
        );
        assert_eq!(learned, ObserveOutcome::Unchanged);
    }

    #[test]
    fn observe_waits_for_idle_before_learning_mid_edit() {
        let expected = test_target(42, false);
        let learned = observe_with(
            "知呼",
            &expected,
            true,
            vec![Some("知".into()), Some("知".into()), Some("知乎".into())],
            vec![expected.clone(), expected.clone(), expected.clone()],
        );
        assert_eq!(learned, zhihu_token());
    }

    #[test]
    fn observe_does_not_learn_on_first_dirty_poll() {
        let expected = test_target(42, false);
        let left = test_target(99, false);
        let learned = observe_with(
            "知呼",
            &expected,
            true,
            vec![Some("知".into())],
            vec![expected.clone(), left],
        );
        assert_eq!(learned, ObserveOutcome::LeftTarget);
    }

    #[test]
    fn observe_extends_then_caps_at_max_total() {
        let expected = test_target(42, false);
        let polls = RefCell::new(0_u32);
        let outcome = observe_after_paste(
            "知呼",
            &expected,
            true,
            ObserveLimits::default(),
            || {
                let count = *polls.borrow();
                if count < 30 {
                    Some(format!("x{count}"))
                } else {
                    Some("知乎".into())
                }
            },
            || expected.clone(),
            |_| {
                *polls.borrow_mut() += 1;
            },
        );
        assert_eq!(*polls.borrow(), 30);
        assert_eq!(outcome, ObserveOutcome::Ambiguous);
    }

    #[test]
    fn observe_dirty_values_without_successful_readback_stay_ambiguous() {
        let expected = test_target(42, false);
        let reads = Cell::new(0_u64);
        let mut samples = vec![
            Some("deploy v2 with correction".to_owned()),
            None,
            None,
            None,
        ]
        .into_iter();
        let outcome = observe_after_paste(
            "deploy v2",
            &expected,
            true,
            ObserveLimits {
                initial: Duration::from_millis(1),
                interval: Duration::from_millis(1),
                idle_grace: Duration::from_millis(2),
                max_total: Duration::from_millis(4),
            },
            || {
                reads.set(reads.get() + 1);
                samples.next().unwrap_or(None)
            },
            || expected.clone(),
            |_| {},
        );
        assert_eq!(reads.get(), 4);
        assert_eq!(outcome, ObserveOutcome::Ambiguous);
    }

    #[test]
    fn observe_requires_successful_same_sample_after_idle_grace() {
        let expected = test_target(42, false);
        let reads = Cell::new(0_u64);
        let mut samples = vec![
            "use VoiceF for dictation".to_owned(),
            "use VoiceFlow for dictation".to_owned(),
            "use VoiceFlow for dictation".to_owned(),
            "use VoiceFlow for dictation".to_owned(),
        ]
        .into_iter();
        let outcome = observe_after_paste(
            "use VioFlo for dictation",
            &expected,
            true,
            ObserveLimits {
                initial: Duration::from_millis(10),
                interval: Duration::from_millis(1),
                idle_grace: Duration::from_millis(2),
                max_total: Duration::from_millis(5),
            },
            || {
                reads.set(reads.get() + 1);
                samples
                    .next()
                    .map(Some)
                    .unwrap_or_else(|| Some("use VoiceFlow for dictation".to_owned()))
            },
            || expected.clone(),
            |_| {},
        );
        assert_eq!(reads.get(), 4);
        assert!(matches!(outcome, ObserveOutcome::SingleToken { .. }));
    }

    #[test]
    fn controlled_fixture_trajectory_stops_at_an_intermediate_stable_state() {
        let post_insert = "The note says VioFlo today.".to_owned();
        let intermediate = "The note says VoiceFlow today.".to_owned();
        let future_settled = "The note says VoiceFlow tomorrow.".to_owned();
        let states = vec![
            post_insert,
            intermediate.clone(),
            intermediate,
            future_settled,
        ];
        let observation = super::observe_controlled_fixture_trajectory(
            &states,
            ObserveLimits {
                initial: Duration::from_millis(2),
                interval: Duration::from_millis(1),
                idle_grace: Duration::from_millis(1),
                max_total: Duration::from_millis(3),
            },
        )
        .expect("validated trajectory has a baseline and terminal state");

        assert_eq!(observation.samples_observed, 2);
        assert_eq!(observation.states_consumed, 3);
        assert!(!observation.settled);
        assert!(matches!(
            observation.outcome,
            ObserveOutcome::SingleToken { ref before_span, ref after }
                if before_span == "VioFlo" && after == "VoiceFlow"
        ));

        let no_read = super::observe_controlled_fixture_trajectory(
            &[String::new(), "final".into()],
            ObserveLimits {
                initial: Duration::from_millis(2),
                interval: Duration::ZERO,
                idle_grace: Duration::from_millis(1),
                max_total: Duration::from_millis(3),
            },
        )
        .expect("bounded trajectory has two declared states");
        assert_eq!(no_read.samples_observed, 0);
        assert_eq!(no_read.states_consumed, 1);
        assert!(!no_read.settled);
        assert_eq!(no_read.outcome, ObserveOutcome::Ambiguous);

        let unchanged = super::observe_controlled_fixture_trajectory(
            &["same".into(), "same".into()],
            ObserveLimits {
                initial: Duration::from_millis(2),
                interval: Duration::from_millis(1),
                idle_grace: Duration::from_millis(1),
                max_total: Duration::from_millis(3),
            },
        )
        .expect("bounded trajectory has two declared states");
        assert_eq!(unchanged.samples_observed, 2);
        assert_eq!(unchanged.states_consumed, 2);
        assert!(unchanged.settled);
        assert_eq!(unchanged.outcome, ObserveOutcome::Unchanged);
    }

    #[test]
    fn controlled_fixture_trajectory_consumes_two_four_and_six_state_inputs() {
        let tokens = [
            "VoiceFlo",
            "VoiceF",
            "VoiceFl",
            "VoiceFll",
            "VoiceFloa",
            "VoiceFlow",
        ];
        let full_texts = tokens
            .iter()
            .map(|token| format!("The release note uses {token} today."))
            .collect::<Vec<_>>();
        for indices in [vec![0, 5], vec![0, 1, 2, 5], vec![0, 1, 2, 3, 4, 5]] {
            let states = indices
                .iter()
                .map(|index| full_texts[*index].clone())
                .collect::<Vec<_>>();
            let total_states = states.len();
            let observation = super::observe_controlled_fixture_trajectory(
                &states,
                ObserveLimits {
                    initial: Duration::from_millis(2),
                    interval: Duration::from_millis(1),
                    idle_grace: Duration::from_millis(1),
                    max_total: Duration::from_millis(3_u64.max(total_states as u64)),
                },
            )
            .expect("bounded trajectory has a baseline and terminal state");

            assert_eq!(observation.samples_observed, total_states);
            assert_eq!(observation.states_consumed, total_states);
            assert!(observation.settled);
            assert!(matches!(
                observation.outcome,
                ObserveOutcome::SingleToken { ref before_span, ref after }
                    if before_span == "VoiceFlo" && after == "VoiceFlow"
            ));
        }
    }

    #[test]
    fn observe_four_state_trajectory_is_ambiguous_under_legacy_three_ms_budget() {
        let states = [
            "The release note uses VoiceFlo today.".to_owned(),
            "The release note uses VoiceF today.".to_owned(),
            "The release note uses VoiceFl today.".to_owned(),
            "The release note uses VoiceFlow today.".to_owned(),
        ];
        let mut samples_observed = 0usize;
        let mut states_consumed = 1usize;
        let mut next_state = 1usize;
        let mut last_state = states[0].clone();
        let outcome = super::observe_after_paste_samples(
            &states[0],
            &states[0],
            None,
            ObserveLimits {
                initial: Duration::from_millis(2),
                interval: Duration::from_millis(1),
                idle_grace: Duration::from_millis(1),
                max_total: Duration::from_millis(3),
            },
            super::ObserveSampleCallbacks {
                read_value: || {
                    samples_observed = samples_observed.saturating_add(1);
                    if let Some(state) = states.get(next_state) {
                        next_state += 1;
                        states_consumed = states_consumed.saturating_add(1);
                        last_state.clone_from(state);
                    }
                    Some(last_state.clone())
                },
                sleep: |_| {},
                keep_watching: || true,
                same_field_is_current: || true,
                end_of_input: None,
            },
        );

        // Under the legacy cap, the normal observer cannot reach a stable
        // result for this valid four-state correction. The candidate owner is
        // downstream of SingleToken and therefore cannot run on this outcome.
        assert_eq!(outcome, ObserveOutcome::Ambiguous);
        assert_eq!(samples_observed, 3);
        assert_eq!(states_consumed, states.len());
    }

    #[test]
    fn controlled_fixture_trajectory_preserves_scoped_ownership_and_rejects_unselected_term() {
        use crate::learning_research::{
            LearningConsent, LearningPurpose, LearningScopeV2, TermFixture,
        };

        let dir = temp_dir("controlled-fixture-trajectory");
        std::fs::create_dir_all(&dir).unwrap();
        let tokens = [
            "VoiceFlo",
            "VoiceF",
            "VoiceFl",
            "VoiceFll",
            "VoiceFloa",
            "VoiceFlow",
        ];
        let states = tokens
            .iter()
            .map(|token| format!("The release note uses {token} today."))
            .collect::<Vec<_>>();
        let post_insert = states.first().unwrap().clone();
        let settled = states.last().unwrap().clone();
        let consent = LearningConsent {
            selected_state_id: "trajectory-state".into(),
            baseline_run: None,
            edit_fixture_id: "trajectory-edit".into(),
            purpose: LearningPurpose::Correction,
            mapping_id: "fixture".into(),
            bundle_id: "com.tencent.xinWeChat".into(),
            observer_enabled: true,
            post_insert_field: post_insert.clone(),
            settled_field: settled.clone(),
            terms: vec![TermFixture {
                fixture_id: "react-term".into(),
                before: "VoiceFlo".into(),
                after: "VoiceFlow".into(),
            }],
        };
        let scope = LearningScopeV2 {
            mapping_id: "fixture".into(),
            family: crate::context::ContextFamily::PersonalChat,
            native_bundle: "com.tencent.xinWeChat".into(),
        };
        let mut settings = crate::store::Settings {
            dictionary_learn_enabled: true,
            local_edit_learning_enabled: true,
            context_enabled: true,
            dictionary: vec!["VoiceFlow".into()],
            ..crate::store::Settings::default()
        };
        let global_dictionary_before = settings.dictionary.clone();
        settings.context_mappings.push(test_mapping("fixture"));

        let first = crate::learning_research::with_scoped_consent(consent.clone(), || {
            super::record_controlled_fixture_edit_trajectory(
                &dir,
                &mut settings,
                &scope,
                &consent,
                super::ControlledFixtureTrajectoryInput {
                    edit: super::ControlledFixtureEditInput {
                        expected_before: "VoiceFlo",
                        expected_after: "VoiceFlow",
                        episode_id: "episode-1",
                        source_sha256: &"a".repeat(64),
                        operation_id: "operation-1",
                        stable_source_origin_id: Some("stable-wav-source-1"),
                    },
                    ordered_states: &states,
                },
            )
        })
        .unwrap();
        assert_eq!(first.observer_outcome, "single_token");
        assert_eq!(first.trajectory_samples_observed, 6);
        assert_eq!(first.trajectory_states_consumed, 6);
        assert!(first.trajectory_settled);
        assert_eq!(first.candidate_write, "candidate_written");
        assert_eq!(settings.dictionary, global_dictionary_before);

        let key = super::pair_key("VoiceFlo", "VoiceFlow");
        let row = crate::store::get_learn_pair(&dir, &key)
            .unwrap()
            .expect("accepted pair is stored");
        assert_eq!(row.hits, 1);
        assert_eq!(
            row.family.as_deref(),
            Some(crate::context::family_id(scope.family))
        );
        assert_eq!(row.mapping_id.as_deref(), Some("fixture"));
        assert_eq!(row.browser_host, None);
        assert_eq!(row.native_bundle.as_deref(), Some("com.tencent.xinWeChat"));
        assert!(!row.is_live_promoted());

        let duplicate = crate::learning_research::with_scoped_consent(consent.clone(), || {
            super::record_controlled_fixture_edit_trajectory(
                &dir,
                &mut settings,
                &scope,
                &consent,
                super::ControlledFixtureTrajectoryInput {
                    edit: super::ControlledFixtureEditInput {
                        expected_before: "VoiceFlo",
                        expected_after: "VoiceFlow",
                        episode_id: "episode-2",
                        source_sha256: &"b".repeat(64),
                        operation_id: "operation-2",
                        stable_source_origin_id: Some("stable-wav-source-1"),
                    },
                    ordered_states: &states,
                },
            )
        })
        .unwrap();
        assert_eq!(duplicate.candidate_write, "duplicate_observation");
        assert_eq!(
            crate::store::get_learn_pair(&dir, &key)
                .unwrap()
                .unwrap()
                .hits,
            1
        );
        assert_eq!(settings.dictionary, global_dictionary_before);

        let promotion_threshold = first
            .promote_threshold
            .expect("selected correction has a word-promotion threshold")
            as usize;
        for observation_index in 2..=promotion_threshold {
            let source_origin = format!("stable-wav-source-{observation_index}");
            let episode_id = format!("episode-{observation_index}");
            let source_sha256 = "c".repeat(64);
            let operation_id = format!("operation-{observation_index}");
            let accepted = crate::learning_research::with_scoped_consent(consent.clone(), || {
                super::record_controlled_fixture_edit_trajectory(
                    &dir,
                    &mut settings,
                    &scope,
                    &consent,
                    super::ControlledFixtureTrajectoryInput {
                        edit: super::ControlledFixtureEditInput {
                            expected_before: "VoiceFlo",
                            expected_after: "VoiceFlow",
                            episode_id: &episode_id,
                            source_sha256: &source_sha256,
                            operation_id: &operation_id,
                            stable_source_origin_id: Some(&source_origin),
                        },
                        ordered_states: &states,
                    },
                )
            })
            .unwrap();
            assert_eq!(accepted.observer_outcome, "single_token");
            assert!(accepted.trajectory_settled);
            assert_eq!(accepted.candidate_write, "candidate_written");
        }
        let promoted_row = crate::store::get_learn_pair(&dir, &key)
            .unwrap()
            .expect("scoped rule remains stored");
        assert!(promoted_row.is_live_promoted());
        assert!(promoted_row.auto_scoped);
        assert!(promoted_row.hits >= promotion_threshold as u32);
        assert_eq!(settings.dictionary, global_dictionary_before);

        let unselected = crate::learning_research::with_scoped_consent(consent.clone(), || {
            super::record_controlled_fixture_edit_trajectory(
                &dir,
                &mut settings,
                &scope,
                &consent,
                super::ControlledFixtureTrajectoryInput {
                    edit: super::ControlledFixtureEditInput {
                        expected_before: "VoiceFl",
                        expected_after: "VoiceFlow",
                        episode_id: "episode-3",
                        source_sha256: &"c".repeat(64),
                        operation_id: "operation-3",
                        stable_source_origin_id: Some("stable-wav-source-2"),
                    },
                    ordered_states: &states,
                },
            )
        })
        .unwrap();
        assert_eq!(
            unselected.observer_outcome,
            "learning_grant_or_term_not_admitted"
        );
        assert_eq!(unselected.trajectory_samples_observed, 0);
        assert_eq!(unselected.trajectory_states_consumed, 0);
        assert!(!unselected.trajectory_settled);
        assert_eq!(unselected.candidate_write, "not_attempted");

        let ambiguous_final = "The entire field was rewritten into a different paragraph.";
        let ambiguous_consent = LearningConsent {
            settled_field: ambiguous_final.into(),
            ..consent.clone()
        };
        let ambiguous_states = vec![post_insert.to_owned(), ambiguous_final.to_owned()];
        let ambiguous =
            crate::learning_research::with_scoped_consent(ambiguous_consent.clone(), || {
                super::record_controlled_fixture_edit_trajectory(
                    &dir,
                    &mut settings,
                    &scope,
                    &ambiguous_consent,
                    super::ControlledFixtureTrajectoryInput {
                        edit: super::ControlledFixtureEditInput {
                            expected_before: "VoiceFlo",
                            expected_after: "VoiceFlow",
                            episode_id: "episode-4",
                            source_sha256: &"d".repeat(64),
                            operation_id: "operation-4",
                            stable_source_origin_id: Some("stable-wav-source-3"),
                        },
                        ordered_states: &ambiguous_states,
                    },
                )
            })
            .unwrap();
        assert_eq!(ambiguous.observer_outcome, "ambiguous");
        assert_eq!(ambiguous.trajectory_samples_observed, 2);
        assert_eq!(ambiguous.trajectory_states_consumed, 2);
        assert!(!ambiguous.trajectory_settled);
        assert_eq!(ambiguous.candidate_write, "not_attempted");
        assert_eq!(
            crate::store::get_learn_pair(&dir, &key)
                .unwrap()
                .unwrap()
                .hits,
            promotion_threshold as u32
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn controlled_fixture_local_only_guard_rejects_ordinary_typos_and_protected_targets() {
        use crate::learning_research::{
            LearningConsent, LearningPurpose, LearningScopeV2, TermFixture,
        };

        for (case_id, before, after) in [
            ("cobolt", "cobolt", "cobalt"),
            ("clone-close", "clone", "close"),
        ] {
            let dir = temp_dir(&format!("controlled-fixture-local-guard-{case_id}"));
            std::fs::create_dir_all(&dir).unwrap();
            let states = vec![
                format!("The release note mentions {before} today."),
                format!("The release note mentions {after} today."),
            ];
            let consent = LearningConsent {
                selected_state_id: format!("{case_id}-state"),
                baseline_run: None,
                edit_fixture_id: format!("{case_id}-edit"),
                purpose: LearningPurpose::Correction,
                mapping_id: "fixture".into(),
                bundle_id: "com.tencent.xinWeChat".into(),
                observer_enabled: true,
                post_insert_field: states[0].clone(),
                settled_field: states[1].clone(),
                terms: vec![TermFixture {
                    fixture_id: format!("{case_id}-term"),
                    before: before.into(),
                    after: after.into(),
                }],
            };
            let scope = LearningScopeV2 {
                mapping_id: "fixture".into(),
                family: crate::context::ContextFamily::PersonalChat,
                native_bundle: "com.tencent.xinWeChat".into(),
            };
            let mut settings = crate::store::Settings {
                dictionary_learn_enabled: true,
                local_edit_learning_enabled: true,
                context_enabled: true,
                ..crate::store::Settings::default()
            };
            settings.context_mappings.push(test_mapping("fixture"));

            let result = crate::learning_research::with_scoped_consent(consent.clone(), || {
                super::record_controlled_fixture_edit_trajectory(
                    &dir,
                    &mut settings,
                    &scope,
                    &consent,
                    super::ControlledFixtureTrajectoryInput {
                        edit: super::ControlledFixtureEditInput {
                            expected_before: before,
                            expected_after: after,
                            episode_id: &format!("{case_id}-episode"),
                            source_sha256: &"e".repeat(64),
                            operation_id: &format!("{case_id}-operation"),
                            stable_source_origin_id: Some(&format!("{case_id}-origin")),
                        },
                        ordered_states: &states,
                    },
                )
            })
            .unwrap();
            assert_eq!(result.observer_outcome, "local_edit_guard_rejected");
            assert_eq!(result.candidate_write, "not_attempted");
            assert!(result.trajectory_settled);
            assert!(
                crate::store::get_learn_pair(&dir, &super::pair_key(before, after))
                    .unwrap()
                    .is_none()
            );
            let _ = std::fs::remove_dir_all(dir);
        }

        let dir = temp_dir("controlled-fixture-protected-target");
        std::fs::create_dir_all(&dir).unwrap();
        let before = "VoiceFlo";
        let after = "VoiceFlow";
        let states = vec![
            "The release note mentions VoiceFlo today.".to_owned(),
            "The release note mentions VoiceFlow today.".to_owned(),
        ];
        let consent = LearningConsent {
            selected_state_id: "protected-state".into(),
            baseline_run: None,
            edit_fixture_id: "protected-edit".into(),
            purpose: LearningPurpose::Correction,
            mapping_id: "fixture".into(),
            bundle_id: "com.apple.Passwords".into(),
            observer_enabled: true,
            post_insert_field: states[0].clone(),
            settled_field: states[1].clone(),
            terms: vec![TermFixture {
                fixture_id: "protected-term".into(),
                before: before.into(),
                after: after.into(),
            }],
        };
        let scope = LearningScopeV2 {
            mapping_id: "fixture".into(),
            family: crate::context::ContextFamily::General,
            native_bundle: "com.apple.Passwords".into(),
        };
        let mut settings = crate::store::Settings {
            dictionary_learn_enabled: true,
            local_edit_learning_enabled: true,
            context_enabled: true,
            ..crate::store::Settings::default()
        };
        let mut mapping = test_mapping("fixture");
        mapping.family = crate::context::ContextFamily::General;
        mapping.bundle_id = Some("com.apple.Passwords".into());
        settings.context_mappings.push(mapping);

        let result = crate::learning_research::with_scoped_consent(consent.clone(), || {
            super::record_controlled_fixture_edit_trajectory(
                &dir,
                &mut settings,
                &scope,
                &consent,
                super::ControlledFixtureTrajectoryInput {
                    edit: super::ControlledFixtureEditInput {
                        expected_before: before,
                        expected_after: after,
                        episode_id: "protected-episode",
                        source_sha256: &"f".repeat(64),
                        operation_id: "protected-operation",
                        stable_source_origin_id: Some("protected-origin"),
                    },
                    ordered_states: &states,
                },
            )
        })
        .unwrap();
        assert_eq!(result.observer_outcome, "protected_target");
        assert_eq!(result.trajectory_samples_observed, 0);
        assert_eq!(result.trajectory_states_consumed, 0);
        assert!(!result.trajectory_settled);
        assert_eq!(result.candidate_write, "not_attempted");
        assert!(
            crate::store::get_learn_pair(&dir, &super::pair_key(before, after))
                .unwrap()
                .is_none()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn observe_unstable_date_rewrite_at_max_total_does_not_create_style_candidate() {
        let expected = test_target(42, false);
        let mut samples = vec![
            "Review 2026-11-04 with the team".to_owned(),
            "Review 2026-11-04 with the team".to_owned(),
            "Review 2026-11-05 with the team".to_owned(),
        ]
        .into_iter();
        let outcome = observe_after_paste(
            "Review 2026-11-04 with the team",
            &expected,
            true,
            ObserveLimits {
                initial: Duration::from_millis(3),
                interval: Duration::from_millis(1),
                idle_grace: Duration::from_millis(2),
                max_total: Duration::from_millis(3),
            },
            || samples.next().map(Some).unwrap_or(None),
            || expected.clone(),
            |_| {},
        );
        assert_eq!(outcome, ObserveOutcome::Ambiguous);
    }

    #[test]
    fn observe_ambiguous_after_settle_is_not_a_word() {
        let expected = test_target(42, false);
        let learned = observe_with(
            "xx",
            &expected,
            true,
            vec![Some("Foo Bar".into())],
            vec![expected.clone()],
        );
        assert_eq!(learned, ObserveOutcome::Ambiguous);
    }

    #[test]
    fn token_key_lowercases_latin_and_keeps_cjk() {
        assert_eq!(super::token_key("Python"), "python");
        assert_eq!(super::token_key("知乎"), "知乎");
    }

    #[test]
    fn classifier_keeps_products_abbreviations_and_cjk_names() {
        assert_eq!(
            super::classify_learn_pair("知呼", "知乎"),
            super::LearnClass::Term
        );
        assert_eq!(
            super::classify_learn_pair("类型脚本", "TypeScript"),
            super::LearnClass::Term
        );
        assert_eq!(
            super::classify_learn_pair("诶皮艾", "API"),
            super::LearnClass::Term
        );
        assert_eq!(
            super::classify_learn_pair("李铭", "李明"),
            super::LearnClass::Name
        );
        assert_eq!(
            super::classify_learn_pair("欧阳那", "欧阳娜"),
            super::LearnClass::Name
        );
    }

    #[test]
    fn classifier_rejects_filler_grammar_and_case_only() {
        assert_eq!(
            super::classify_learn_pair("那个那个", "那个"),
            super::LearnClass::Rejected
        );
        assert_eq!(
            super::classify_learn_pair("因为因为", "因为"),
            super::LearnClass::Rejected
        );
        assert_eq!(
            super::classify_learn_pair("python", "Python"),
            super::LearnClass::Rejected
        );
        assert_eq!(
            super::classify_learn_pair("um um", "um"),
            super::LearnClass::Rejected
        );
    }

    #[test]
    fn record_pair_ignores_filler_and_case_only() {
        let mut table = super::LearnPairTable::default();
        assert_eq!(
            super::record_pair(&mut table, "那个那个", "那个"),
            super::RecordPairResult::Ignored
        );
        assert_eq!(
            super::record_pair(&mut table, "python", "Python"),
            super::RecordPairResult::Ignored
        );
        assert!(table.is_empty());
    }

    #[test]
    fn record_pair_promotes_cjk_name_on_second_hit() {
        let mut table = super::LearnPairTable::default();
        assert_eq!(
            super::record_pair(&mut table, "李铭", "李明"),
            super::RecordPairResult::Pending { hits: 1 }
        );
        assert_eq!(
            super::record_pair(&mut table, "李铭", "李明"),
            super::RecordPairResult::Promoted {
                after: "李明".into()
            }
        );
        assert_eq!(table.hits(&super::pair_key("李铭", "李明")), Some(2));
    }

    #[test]
    fn record_pair_promotes_on_third_hit_and_freezes() {
        let mut table = super::LearnPairTable::default();
        assert_eq!(
            super::record_pair(&mut table, "知呼", "知乎"),
            super::RecordPairResult::Pending { hits: 1 }
        );
        assert_eq!(
            super::record_pair(&mut table, "知呼", "知乎"),
            super::RecordPairResult::Pending { hits: 2 }
        );
        assert_eq!(
            super::record_pair(&mut table, "知呼", "知乎"),
            super::RecordPairResult::Promoted {
                after: "知乎".into()
            }
        );
        assert_eq!(
            super::record_pair(&mut table, "知呼", "知乎"),
            super::RecordPairResult::AlreadyPromoted
        );
        assert_eq!(table.hits(&super::pair_key("知呼", "知乎")), Some(3));
    }

    #[test]
    fn dictionary_settings_lists_pending_and_promoted_pairs() {
        let pending = crate::store::LearnPairRecord {
            pair_key: super::pair_key("知呼", "知乎"),
            before_surface: "知呼".into(),
            after_surface: "知乎".into(),
            hits: 2,
            promoted: false,
            last_at: "2026-01-01".into(),
            family: None,
            mapping_id: None,
            browser_host: None,
            native_bundle: None,
            last_used_at: None,
            pinned: false,
            tombstoned_at: None,
            ignored: false,
            promote_hits: 0,
            manual_scoped: false,
            auto_scoped: false,
        };
        let promoted = crate::store::LearnPairRecord {
            promoted: true,
            hits: 3,
            ..pending.clone()
        };
        let name = crate::store::LearnPairRecord {
            pair_key: super::pair_key("李铭", "李明"),
            before_surface: "李铭".into(),
            after_surface: "李明".into(),
            hits: 2,
            promoted: true,
            last_at: "2026-01-01".into(),
            family: None,
            mapping_id: None,
            browser_host: None,
            native_bundle: None,
            last_used_at: None,
            pinned: false,
            tombstoned_at: None,
            ignored: false,
            promote_hits: 0,
            manual_scoped: false,
            auto_scoped: false,
        };
        let manual = crate::store::LearnPairRecord {
            pair_key: super::pair_key("", "手动导入"),
            before_surface: String::new(),
            after_surface: "手动导入".into(),
            hits: 0,
            promoted: true,
            last_at: "2026-01-01".into(),
            family: None,
            mapping_id: None,
            browser_host: None,
            native_bundle: None,
            last_used_at: None,
            pinned: false,
            tombstoned_at: None,
            ignored: false,
            promote_hits: 0,
            manual_scoped: false,
            auto_scoped: false,
        };
        let visible = super::pairs_for_dictionary_settings(vec![pending, promoted, name, manual]);
        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].promote_hits, 3);
        assert_eq!(visible[1].promote_hits, 3);
        assert_eq!(visible[2].after_surface, "李明");
        assert_eq!(visible[2].promote_hits, 2);
        assert!(visible.iter().all(|row| !row.before_surface.is_empty()));
    }

    #[test]
    fn record_pair_ignores_identical_surfaces() {
        let mut table = super::LearnPairTable::default();
        assert_eq!(
            super::record_pair(&mut table, "知乎", "知乎"),
            super::RecordPairResult::Ignored
        );
        assert!(table.is_empty());
    }

    #[test]
    fn record_pair_treats_different_afters_as_different_pairs() {
        let mut table = super::LearnPairTable::default();
        assert_eq!(
            super::record_pair(&mut table, "配森", "Python"),
            super::RecordPairResult::Pending { hits: 1 }
        );
        assert_eq!(
            super::record_pair(&mut table, "配森", "pytorch"),
            super::RecordPairResult::Pending { hits: 1 }
        );
        assert_eq!(table.hits(&super::pair_key("配森", "Python")), Some(1));
        assert_eq!(table.hits(&super::pair_key("配森", "pytorch")), Some(1));
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "voiceflow-learn-{name}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn explicit_global_confirm_preflights_full_dictionary_before_staging_add() {
        let dir = temp_dir("explicit-confirm-capacity");
        let before = "VoiceFlo";
        let after = "VoiceFlow";
        let key = super::pair_key(before, after);
        let scope = crate::store::LearnPairScope {
            family: Some("general".into()),
            mapping_id: Some("fixture".into()),
            native_bundle: Some("com.example.fixture".into()),
            ..Default::default()
        };
        assert!(matches!(
            crate::store::upsert_learn_pair_with_scope(&dir, &key, before, after, Some(&scope))
                .unwrap(),
            crate::store::LearnPairUpsert::Stored(_)
        ));

        let mut full = crate::store::Settings {
            dictionary: (0..256).map(|index| format!("word{index}")).collect(),
            ..crate::store::Settings::default()
        };
        let rejected = super::require_dictionary_confirmation_capacity(&dir, &full, after);
        if rejected.is_ok() {
            crate::store::ensure_learn_pair_promoted(&dir, &key, before, after, Some(&scope))
                .unwrap();
        }
        assert_eq!(rejected.unwrap_err(), "dictionary_capacity_or_invalid_word");
        assert!(crate::store::get_learn_pair(&dir, &key)
            .unwrap()
            .expect("pending pair remains present")
            .is_pending());
        assert!(crate::store::list_pending_dictionary_effects(&dir)
            .unwrap()
            .is_empty());

        full.dictionary[0] = "voiceflow".into();
        assert!(super::require_dictionary_confirmation_capacity(&dir, &full, after).is_ok());
        crate::store::ensure_learn_pair_promoted(&dir, &key, before, after, Some(&scope)).unwrap();
        let effects = crate::store::list_pending_dictionary_effects(&dir).unwrap();
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].pair_key, key);
        assert_eq!(effects[0].action, crate::store::DictionaryEffectAction::Add);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn explicit_confirm_counts_pending_adds_before_staging_another_rule() {
        let dir = temp_dir("explicit-confirm-pending-capacity");
        let scope = crate::store::LearnPairScope {
            family: Some("general".into()),
            mapping_id: Some("fixture".into()),
            native_bundle: Some("com.example.fixture".into()),
            ..Default::default()
        };
        let first_before = "RuleOen";
        let first_after = "RuleOne";
        let first_key = super::pair_key(first_before, first_after);
        let second_before = "RuleTwa";
        let second_after = "RuleTwo";
        let second_key = super::pair_key(second_before, second_after);
        for (key, before, after) in [
            (&first_key, first_before, first_after),
            (&second_key, second_before, second_after),
        ] {
            assert!(matches!(
                crate::store::upsert_learn_pair_with_scope(&dir, key, before, after, Some(&scope))
                    .unwrap(),
                crate::store::LearnPairUpsert::Stored(_)
            ));
        }

        let settings = crate::store::Settings {
            dictionary: (0..255).map(|index| format!("word{index}")).collect(),
            ..crate::store::Settings::default()
        };
        assert!(
            super::require_dictionary_confirmation_capacity(&dir, &settings, first_after).is_ok()
        );
        crate::store::ensure_learn_pair_promoted(
            &dir,
            &first_key,
            first_before,
            first_after,
            Some(&scope),
        )
        .unwrap();
        let mut failed_save = settings.clone();
        let first_error = match super::save_settings_with_dictionary_effects(
            &dir,
            &mut failed_save,
            true,
            true,
            |_| Err(anyhow::anyhow!("injected first confirmation save failure")),
        ) {
            Err(error) => error,
            Ok(_) => panic!("the injected first confirmation failure must be returned"),
        };
        assert!(first_error.starts_with(super::DICTIONARY_SETTINGS_PARTIAL_ERROR));

        let rejected =
            super::require_dictionary_confirmation_capacity(&dir, &settings, second_after);
        if rejected.is_ok() {
            crate::store::ensure_learn_pair_promoted(
                &dir,
                &second_key,
                second_before,
                second_after,
                Some(&scope),
            )
            .unwrap();
        }
        assert_eq!(rejected.unwrap_err(), "dictionary_capacity_or_invalid_word");
        assert!(crate::store::get_learn_pair(&dir, &second_key)
            .unwrap()
            .expect("second pending pair remains present")
            .is_pending());
        let effects = crate::store::list_pending_dictionary_effects(&dir).unwrap();
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].pair_key, first_key);
        assert_eq!(effects[0].action, crate::store::DictionaryEffectAction::Add);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn a0_fixture_input_str<'a>(input: &'a serde_json::Value, field: &str) -> &'a str {
        input[field]
            .as_str()
            .unwrap_or_else(|| panic!("A0 fixture input {field:?} must be a string"))
    }

    fn a0_fixture_limits(input: &serde_json::Value) -> ObserveLimits {
        let limits = &input["limits_ms"];
        ObserveLimits {
            initial: std::time::Duration::from_millis(
                limits["initial"]
                    .as_u64()
                    .expect("initial limit must be set"),
            ),
            interval: std::time::Duration::from_millis(
                limits["interval"]
                    .as_u64()
                    .expect("interval limit must be set"),
            ),
            idle_grace: std::time::Duration::from_millis(
                limits["idle_grace"]
                    .as_u64()
                    .expect("idle grace must be set"),
            ),
            max_total: std::time::Duration::from_millis(
                limits["max_total"]
                    .as_u64()
                    .expect("maximum total must be set"),
            ),
        }
    }

    fn a0_scope(input: &serde_json::Value) -> crate::store::LearnPairScope {
        let scope = &input["scope"];
        crate::store::LearnPairScope {
            family: scope["family"].as_str().map(str::to_owned),
            mapping_id: scope["mapping_id"].as_str().map(str::to_owned),
            browser_host: None,
            native_bundle: scope["native_bundle"].as_str().map(str::to_owned),
        }
    }

    fn a0_observe_kind(outcome: &ObserveOutcome) -> (&'static str, bool) {
        match outcome {
            ObserveOutcome::Unchanged => ("unchanged", false),
            ObserveOutcome::LeftTarget => ("left_target", false),
            ObserveOutcome::SingleToken { .. } => ("single_token", true),
            ObserveOutcome::MultipleTokens { .. } => ("multiple_tokens_unsupported", false),
            ObserveOutcome::ShortStyleRewrite { .. } => ("short_style_rewrite", false),
            ObserveOutcome::StyleSignal { .. } => ("style_signal", false),
            ObserveOutcome::Ambiguous => ("ambiguous", false),
        }
    }

    fn a0_style_candidate_detected(outcome: &ObserveOutcome) -> bool {
        matches!(
            outcome,
            ObserveOutcome::ShortStyleRewrite { .. } | ObserveOutcome::StyleSignal { .. }
        )
    }

    #[test]
    fn learning_a0_baseline_runs_production_probes_and_emits_observed_facts() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../evals/learning-a0-baseline.json"))
                .expect("A0 baseline fixture must be valid JSON");
        assert_eq!(
            fixture["artifact_type"].as_str(),
            Some("voiceflow_learning_a0_baseline_fixture")
        );
        assert_eq!(fixture["schema_version"].as_u64(), Some(1));
        assert_eq!(
            fixture["fixture_id"].as_str(),
            Some("learning-a0-deterministic-v1")
        );
        assert_eq!(
            fixture["frozen_contract"]["ordinary_term_promote_hits"].as_u64(),
            Some(super::promote_hits_for(super::LearnClass::Term) as u64)
        );
        assert_eq!(
            fixture["frozen_contract"]["chinese_name_promote_hits"].as_u64(),
            Some(super::promote_hits_for(super::LearnClass::Name) as u64)
        );
        assert_eq!(fixture["frozen_contract"]["asr_model"], "not_invoked");
        assert_eq!(fixture["frozen_contract"]["cleanup_prompt"], "not_invoked");
        assert_eq!(
            fixture["frozen_contract"]["owned_wav_learning"],
            "suppressed"
        );
        assert_eq!(fixture["native_smoke"]["status"], "not_run");

        let required_ids = [
            "manual_word_ownership_after_unscoped_undo",
            "same_dictation_paste_and_history_receipts",
            "legacy_pending_plus_new_origin",
            "ax_multi_hunk_content_edit",
            "ax_date_content_edit",
            "changing_at_observer_deadline",
        ];
        let cases = fixture["probes"]
            .as_array()
            .expect("A0 fixture probes must be an array");
        assert_eq!(cases.len(), required_ids.len());
        let mut fixture_ids = HashSet::new();
        let mut total_input_bytes = 0usize;
        for case in cases {
            let id = case["id"]
                .as_str()
                .expect("A0 fixture probe id must be a string");
            assert!(
                fixture_ids.insert(id.to_owned()),
                "duplicate A0 probe id: {id}"
            );
            assert_ne!(
                case["classification"].as_str(),
                Some("known_gap"),
                "fixture classification describes the case, not its observed result"
            );
            assert!(
                required_ids.contains(&id),
                "unknown A0 probe id must not be silently skipped: {id}"
            );
            let input_bytes = case["input"].to_string().len();
            assert!(
                input_bytes <= 4096,
                "A0 probe input exceeds its bound: {id}"
            );
            total_input_bytes += input_bytes;
        }
        assert_eq!(fixture_ids.len(), required_ids.len());
        assert!(
            total_input_bytes <= 16 * 1024,
            "A0 fixture input exceeds its bound"
        );

        let mut observed_cases = Vec::with_capacity(cases.len());
        let mut executed_ids = HashSet::new();
        for case in cases {
            let id = case["id"].as_str().expect("validated A0 probe id");
            assert!(
                executed_ids.insert(id.to_owned()),
                "A0 probe executed twice: {id}"
            );
            let input = &case["input"];
            let observed = match id {
                "manual_word_ownership_after_unscoped_undo" => {
                    let root = temp_dir("a0-manual-word-undo");
                    let mut dictionary = input["global_dictionary_seed"]
                        .as_array()
                        .expect("A0 dictionary seed must be an array")
                        .iter()
                        .map(|word| {
                            word.as_str()
                                .expect("seed word must be a string")
                                .to_owned()
                        })
                        .collect::<Vec<_>>();
                    let hits = input["candidate_hits"]
                        .as_u64()
                        .expect("candidate hit count must be set");
                    assert!(
                        (1..=8).contains(&hits),
                        "A0 candidate hit count is unbounded"
                    );
                    let before = a0_fixture_input_str(input, "before");
                    let after = a0_fixture_input_str(input, "after");
                    for _ in 0..hits {
                        super::record_learn_pair_with_scope(
                            &root,
                            &mut dictionary,
                            before,
                            after,
                            None,
                        )
                        .expect("production candidate owner must complete");
                    }
                    let key = super::pair_key(before, after);
                    let promoted_row = crate::store::get_learn_pair(&root, &key)
                        .expect("read isolated A0 candidate")
                        .expect("candidate row must be persisted");
                    let promoted_before_undo = promoted_row.is_live_promoted();
                    assert!(
                        crate::store::tombstone_learn_pair(&root, &key)
                            .expect("production tombstone owner must complete"),
                        "A0 undo probe must tombstone its candidate"
                    );
                    let rows_after_tombstone = crate::store::list_learn_pairs(&root)
                        .expect("read isolated A0 rows after tombstone");
                    let remove_dictionary_entry = super::should_remove_dictionary_entry_after_undo(
                        &promoted_row,
                        &rows_after_tombstone,
                    );
                    if remove_dictionary_entry {
                        super::remove_dictionary_entry(&mut dictionary, after);
                    }
                    let retained = crate::store::dictionary_contains_word(&dictionary, after);
                    let _ = std::fs::remove_dir_all(root);
                    serde_json::json!({
                        "row_promoted_before_undo": promoted_before_undo,
                        "undo_owner_removed_dictionary_entry": remove_dictionary_entry,
                        "global_dictionary_word_retained": retained
                    })
                }
                "same_dictation_paste_and_history_receipts" => {
                    let root = temp_dir("a0-same-origin-cross-channel");
                    let before = a0_fixture_input_str(input, "before");
                    let after = a0_fixture_input_str(input, "after");
                    let key = super::pair_key(before, after);
                    let scope = a0_scope(input);
                    crate::store::insert_history(
                        &root,
                        a0_fixture_input_str(input, "history_text_before"),
                        a0_fixture_input_str(input, "history_text_before"),
                        1.0,
                        false,
                    )
                    .expect("insert isolated A0 History row");
                    let history_id = crate::store::get_history(&root, 1)
                        .expect("read isolated A0 History row")
                        .first()
                        .expect("A0 History row must exist")
                        .id;
                    let revision =
                        crate::store::save_manual_history_revision(&root, history_id, after)
                            .expect("save isolated A0 History correction");
                    let revision_id = revision
                        .revision_id
                        .expect("A0 History correction must have a revision id");
                    let paste_receipt = crate::store::LearnPairObservation::paste_session(
                        a0_fixture_input_str(input, "dictation_origin_id"),
                        &key,
                    );
                    let history_receipt = crate::store::LearnPairObservation::history_edit(
                        history_id,
                        revision_id,
                        &key,
                    );
                    let mut dictionary = Vec::new();
                    super::record_learn_pair_with_observation_audited(
                        &root,
                        &mut dictionary,
                        before,
                        after,
                        Some(&scope),
                        Some(&paste_receipt),
                        true,
                        None,
                    )
                    .expect("record production paste-session receipt");
                    super::record_learn_pair_with_observation_audited(
                        &root,
                        &mut dictionary,
                        before,
                        after,
                        Some(&scope),
                        Some(&history_receipt),
                        true,
                        None,
                    )
                    .expect("record production History-edit receipt");
                    let row = crate::store::get_learn_pair(&root, &key)
                        .expect("read isolated A0 pair")
                        .expect("A0 cross-channel pair must exist");
                    let connection = rusqlite::Connection::open(root.join("history.sqlite"))
                        .expect("open isolated A0 receipt store");
                    let mut statement = connection
                        .prepare("SELECT source_kind FROM learn_observations ORDER BY source_kind")
                        .expect("prepare isolated A0 receipt query");
                    let receipt_sources = statement
                        .query_map([], |receipt| receipt.get::<_, String>(0))
                        .expect("query isolated A0 receipts")
                        .collect::<Result<Vec<_>, _>>()
                        .expect("read isolated A0 receipts");
                    let result = serde_json::json!({
                        "dictation_origin_id": input["dictation_origin_id"],
                        "history_id": history_id,
                        "receipt_sources": receipt_sources,
                        "hits_after": row.hits
                    });
                    drop(statement);
                    drop(connection);
                    let _ = std::fs::remove_dir_all(root);
                    result
                }
                "legacy_pending_plus_new_origin" => {
                    let root = temp_dir("a0-legacy-pending-origin");
                    let before = a0_fixture_input_str(input, "before");
                    let after = a0_fixture_input_str(input, "after");
                    let key = super::pair_key(before, after);
                    let scope = a0_scope(input);
                    let mut dictionary = Vec::new();
                    let legacy_hits = input["legacy_hits_without_receipt"]
                        .as_u64()
                        .expect("legacy hit count must be set");
                    let ordinary_threshold =
                        super::promote_hits_for(super::LearnClass::Term) as u64;
                    assert!(
                        legacy_hits > 0 && legacy_hits < ordinary_threshold && legacy_hits <= 8,
                        "legacy hits must be bounded below the ordinary promotion threshold"
                    );
                    for _ in 0..legacy_hits {
                        super::record_learn_pair_with_scope(
                            &root,
                            &mut dictionary,
                            before,
                            after,
                            Some(&scope),
                        )
                        .expect("record legacy pending candidate without a receipt");
                    }
                    let connection = rusqlite::Connection::open(root.join("history.sqlite"))
                        .expect("open isolated A0 legacy receipt store");
                    let receipts_before_new_origin: i64 = connection
                        .query_row("SELECT COUNT(*) FROM learn_observations", [], |row| {
                            row.get(0)
                        })
                        .expect("read receipts before new origin");
                    drop(connection);
                    let legacy_origin_known = receipts_before_new_origin > 0;
                    let new_origin = crate::store::LearnPairObservation::paste_session(
                        a0_fixture_input_str(input, "new_paste_session_id"),
                        &key,
                    );
                    super::record_learn_pair_with_observation_audited(
                        &root,
                        &mut dictionary,
                        before,
                        after,
                        Some(&scope),
                        Some(&new_origin),
                        true,
                        None,
                    )
                    .expect("record the new production paste-session receipt");
                    let row = crate::store::get_learn_pair(&root, &key)
                        .expect("read isolated A0 legacy pair")
                        .expect("A0 legacy pair must exist");
                    let connection = rusqlite::Connection::open(root.join("history.sqlite"))
                        .expect("reopen isolated A0 legacy receipt store");
                    let receipt_count_after_new_origin: i64 = connection
                        .query_row("SELECT COUNT(*) FROM learn_observations", [], |row| {
                            row.get(0)
                        })
                        .expect("read receipts after new origin");
                    drop(connection);
                    let result = serde_json::json!({
                        "receipts_before_new_origin": receipts_before_new_origin,
                        "hits_after": row.hits,
                        "live_promoted": row.is_live_promoted(),
                        "receipt_count_after_new_origin": receipt_count_after_new_origin,
                        "legacy_origin_known": legacy_origin_known
                    });
                    let _ = std::fs::remove_dir_all(root);
                    result
                }
                "ax_multi_hunk_content_edit" | "ax_date_content_edit" => {
                    let post_insert_field = a0_fixture_input_str(input, "post_insert_field");
                    let settled_field = a0_fixture_input_str(input, "settled_field").to_owned();
                    let expected_target = test_target(7201, false);
                    let read_target = expected_target.clone();
                    let sample_reads = Cell::new(0_u64);
                    let sample = settled_field.clone();
                    let outcome = observe_after_paste(
                        post_insert_field,
                        &expected_target,
                        true,
                        a0_fixture_limits(input),
                        || {
                            sample_reads.set(sample_reads.get() + 1);
                            Some(sample.clone())
                        },
                        || read_target.clone(),
                        |_| {},
                    );
                    let (kind, candidate_detected) = a0_observe_kind(&outcome);
                    let style_candidate_detected = a0_style_candidate_detected(&outcome);
                    let history_eligible = if id == "ax_date_content_edit" {
                        super::history_lexical_correction(post_insert_field, &settled_field)
                            .is_some()
                    } else {
                        false
                    };
                    if id == "ax_multi_hunk_content_edit" {
                        serde_json::json!({
                            "outcome": kind,
                            "lexical_candidate_detected": candidate_detected,
                            "style_candidate_detected": style_candidate_detected,
                            "controlled_sample_reads": sample_reads.get()
                        })
                    } else {
                        serde_json::json!({
                            "outcome": kind,
                            "lexical_candidate_detected": candidate_detected,
                            "history_lexical_correction_eligible": history_eligible,
                            "style_candidate_detected": style_candidate_detected,
                            "controlled_sample_reads": sample_reads.get()
                        })
                    }
                }
                "changing_at_observer_deadline" => {
                    let post_insert_field = a0_fixture_input_str(input, "post_insert_field");
                    let samples = input["samples"]
                        .as_array()
                        .expect("A0 deadline samples must be an array")
                        .iter()
                        .map(|sample| sample.as_str().expect("sample must be a string").to_owned())
                        .collect::<Vec<_>>();
                    assert!(!samples.is_empty() && samples.len() <= 8);
                    let final_sample = samples.last().expect("checked nonempty samples").clone();
                    let samples = std::cell::RefCell::new(samples.into_iter());
                    let sample_reads = Cell::new(0_u64);
                    let expected_target = test_target(7202, false);
                    let read_target = expected_target.clone();
                    let outcome = super::observe_after_paste_with_identity(
                        post_insert_field,
                        post_insert_field,
                        &expected_target,
                        super::ObserveOptions {
                            dictionary_learn_enabled: true,
                            require_browser_identity: false,
                            inserted_range_after: None,
                            limits: a0_fixture_limits(input),
                        },
                        || {
                            sample_reads.set(sample_reads.get() + 1);
                            Some(
                                samples
                                    .borrow_mut()
                                    .next()
                                    .unwrap_or_else(|| final_sample.clone()),
                            )
                        },
                        || read_target.clone(),
                        |_| {},
                        || true,
                        || true,
                    );
                    let (kind, candidate_detected) = a0_observe_kind(&outcome);
                    let style_candidate_detected = a0_style_candidate_detected(&outcome);
                    serde_json::json!({
                        "outcome": kind,
                        "lexical_candidate_detected": candidate_detected,
                        "style_candidate_detected": style_candidate_detected,
                        "controlled_sample_reads": sample_reads.get()
                    })
                }
                _ => panic!("unknown A0 probe id must not be silently skipped: {id}"),
            };
            let target = case["target"]
                .as_object()
                .expect("A0 target must be an object");
            let observed_object = observed
                .as_object()
                .expect("A0 observed facts must be an object");
            let target_met = target
                .iter()
                .all(|(key, expected)| observed_object.get(key) == Some(expected));
            observed_cases.push(serde_json::json!({
                "id": id,
                "input": case["input"],
                "classification": case["classification"],
                "target": case["target"],
                "observed": observed,
                "status": if target_met { "target_met" } else { "known_gap" }
            }));
        }
        assert_eq!(executed_ids, fixture_ids, "every fixture case must execute");
        assert_eq!(
            executed_ids,
            required_ids.into_iter().map(str::to_owned).collect(),
            "all required A0 probes must execute exactly once"
        );

        let report = serde_json::json!({
            "artifact_type": "voiceflow_learning_a0_observed_baseline",
            "schema_version": 1,
            "fixture_id": fixture["fixture_id"],
            "scope": "deterministic isolated production-function observations; not native E2E, microphone, ASR quality, or independent human evidence",
            "contract": fixture["frozen_contract"],
            "native_smoke": fixture["native_smoke"],
            "probes": observed_cases
        });
        println!(
            "VOICEFLOW_LEARNING_A0_BASELINE_JSON\t{}",
            serde_json::to_string(&report).expect("serialize observed A0 baseline")
        );
    }

    #[test]
    fn actual_store_rows_drive_same_after_undo_retention() {
        let dir = temp_dir("vf12-shared-after");
        let mut dictionary = Vec::new();
        for before in ["知呼", "知胡"] {
            for _ in 0..3 {
                super::record_test_proof_pair(&dir, &mut dictionary, before, "知乎", None).unwrap();
            }
        }
        let first = super::pair_key("知呼", "知乎");
        let second = super::pair_key("知胡", "知乎");
        crate::store::tombstone_learn_pair(&dir, &first).unwrap();
        let rows = crate::store::list_learn_pairs(&dir).unwrap();
        assert!(super::has_other_live_promoted_pair(&rows, &first, "知乎"));
        crate::store::tombstone_learn_pair(&dir, &second).unwrap();
        let rows = crate::store::list_learn_pairs(&dir).unwrap();
        assert!(!super::has_other_live_promoted_pair(&rows, &second, "知乎"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pair_promotion_fastpaths_reject_scope_conflicts() {
        let dir = temp_dir("scope-fastpath");
        let key = super::pair_key("知呼", "知乎");
        let scope = crate::store::LearnPairScope {
            family: Some("prompt_or_code".into()),
            mapping_id: Some("codex".into()),
            native_bundle: Some("com.openai.codex".into()),
            ..crate::store::LearnPairScope::default()
        };
        let foreign = crate::store::LearnPairScope {
            mapping_id: Some("other".into()),
            ..scope.clone()
        };
        for _ in 0..2 {
            crate::store::upsert_learn_pair_with_observation(
                &dir,
                &key,
                "知呼",
                "知乎",
                Some(&scope),
                None,
            )
            .unwrap();
        }

        let mut full_dictionary = (0..256).map(|index| format!("word-{index}")).collect();
        assert_eq!(
            super::record_learn_pair_with_scope(
                &dir,
                &mut full_dictionary,
                "知呼",
                "知乎",
                Some(&foreign),
            )
            .unwrap(),
            super::RecordPairResult::Ignored
        );
        let pending = crate::store::get_learn_pair(&dir, &key).unwrap().unwrap();
        assert_eq!(pending.hits, 2);
        assert!(!pending.promoted);
        assert_eq!(pending.mapping_id.as_deref(), Some("codex"));

        crate::store::ensure_learn_pair_promoted(&dir, &key, "知呼", "知乎", Some(&scope)).unwrap();
        let mut dictionary = vec!["知乎".to_owned()];
        assert_eq!(
            super::record_learn_pair_with_scope(
                &dir,
                &mut dictionary,
                "知呼",
                "知乎",
                Some(&scope),
            )
            .unwrap(),
            super::RecordPairResult::AlreadyPromoted
        );
        assert!(crate::store::get_learn_pair(&dir, &key)
            .unwrap()
            .unwrap()
            .is_live_promoted());
        assert_eq!(
            super::record_learn_pair_with_scope(
                &dir,
                &mut dictionary,
                "知呼",
                "知乎",
                Some(&foreign),
            )
            .unwrap(),
            super::RecordPairResult::Ignored
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn history_receipt_retries_promotion_without_adding_another_hit() {
        let dir = temp_dir("promotion-retry-receipt");
        crate::store::insert_history(&dir, "知呼", "知呼", 1.0, false).unwrap();
        let history_id = crate::store::get_history(&dir, 10).unwrap()[0].id;
        let first_save =
            crate::store::save_manual_history_revision(&dir, history_id, "知乎").unwrap();
        let revision_id = first_save.revision_id.unwrap();
        assert_eq!(first_save.learning_before_text, "知呼");
        let key = super::pair_key("知呼", "知乎");
        let scope = crate::store::LearnPairScope {
            family: Some("prompt_or_code".into()),
            mapping_id: Some("codex".into()),
            native_bundle: Some("com.openai.codex".into()),
            ..crate::store::LearnPairScope::default()
        };
        let mut dictionary = Vec::new();
        for index in 0..2 {
            crate::store::insert_history(&dir, "知呼", "知呼", 1.0, false).unwrap();
            let prior_history_id = crate::store::get_history(&dir, 1).unwrap()[0].id;
            let prior_save =
                crate::store::save_manual_history_revision(&dir, prior_history_id, "知乎").unwrap();
            assert!(prior_save.revision_id.is_some());
            let prior_observation = crate::store::LearnPairObservation::research_fixture_origin(
                &format!("history-promotion-retry-prior-{index}"),
                &key,
            );
            assert!(matches!(
                super::record_learn_pair_with_observation_audited(
                    &dir,
                    &mut dictionary,
                    "知呼",
                    "知乎",
                    Some(&scope),
                    Some(&prior_observation),
                    true,
                    None,
                )
                .unwrap(),
                super::RecordPairResult::Pending { .. }
            ));
        }
        let observation = crate::store::LearnPairObservation::research_fixture_origin(
            "history-promotion-retry-final",
            &key,
        );
        let connection = rusqlite::Connection::open(dir.join("history.sqlite")).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_pair_promotion BEFORE UPDATE OF promoted ON learn_pairs
                 WHEN NEW.promoted = 1 BEGIN SELECT RAISE(ABORT, 'injected promotion failure'); END;",
            )
            .unwrap();
        drop(connection);

        let first_attempt = super::record_learn_pair_with_observation_audited(
            &dir,
            &mut dictionary,
            "知呼",
            "知乎",
            Some(&scope),
            Some(&observation),
            true,
            None,
        );
        assert!(first_attempt.is_err());
        let after_failed_promotion = crate::store::get_learn_pair(&dir, &key).unwrap().unwrap();
        assert_eq!(after_failed_promotion.hits, 3);
        assert!(!after_failed_promotion.promoted);
        assert!(!after_failed_promotion.auto_scoped);

        let connection = rusqlite::Connection::open(dir.join("history.sqlite")).unwrap();
        connection
            .execute_batch("DROP TRIGGER fail_pair_promotion;")
            .unwrap();
        drop(connection);
        let retry_save =
            crate::store::save_manual_history_revision(&dir, history_id, "知乎").unwrap();
        assert!(!retry_save.changed);
        assert_eq!(retry_save.revision_id, Some(revision_id));
        assert_eq!(retry_save.learning_before_text, "知呼");
        assert!(matches!(
            super::record_learn_pair_with_observation_audited(
                &dir,
                &mut dictionary,
                "知呼",
                "知乎",
                Some(&scope),
                Some(&observation),
                true,
                None,
            )
            .unwrap(),
            super::RecordPairResult::Promoted { .. }
        ));
        let after_retry = crate::store::get_learn_pair(&dir, &key).unwrap().unwrap();
        assert_eq!(after_retry.hits, 3);
        assert!(after_retry.is_live_promoted());
        assert!(after_retry.auto_scoped);
        assert_eq!(
            crate::store::get_history_revisions(&dir, history_id)
                .unwrap()
                .len(),
            1
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn duplicate_test_origin_keeps_the_pending_hit_count() {
        let dir = temp_dir("duplicate-history-origin");
        let key = super::pair_key("知呼", "知乎");
        let scope = crate::store::LearnPairScope {
            family: Some("prompt_or_code".into()),
            mapping_id: Some("codex".into()),
            native_bundle: Some("com.openai.codex".into()),
            ..crate::store::LearnPairScope::default()
        };
        let observation = crate::store::LearnPairObservation::research_fixture_origin(
            "duplicate-history-origin",
            &key,
        );
        let mut dictionary = Vec::new();
        assert_eq!(
            super::record_learn_pair_with_observation_audited(
                &dir,
                &mut dictionary,
                "知呼",
                "知乎",
                Some(&scope),
                Some(&observation),
                true,
                None,
            )
            .unwrap(),
            super::RecordPairResult::Pending { hits: 1 }
        );
        assert_eq!(
            super::record_learn_pair_with_observation_audited(
                &dir,
                &mut dictionary,
                "知呼",
                "知乎",
                Some(&scope),
                Some(&observation),
                true,
                None,
            )
            .unwrap(),
            super::RecordPairResult::Pending { hits: 1 }
        );
        assert_eq!(
            crate::store::get_learn_pair(&dir, &key)
                .unwrap()
                .unwrap()
                .hits,
            1
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn forgetting_auto_scoped_pair_preserves_global_word_ownership() {
        let scoped = crate::store::LearnPairRecord {
            pair_key: super::pair_key("VoiceFlo", "VoiceFlow"),
            before_surface: "VoiceFlo".into(),
            after_surface: "VoiceFlow".into(),
            hits: 3,
            promoted: true,
            last_at: "2026-01-01".into(),
            family: Some("general".into()),
            mapping_id: Some("codex".into()),
            browser_host: None,
            native_bundle: Some("com.openai.codex".into()),
            last_used_at: None,
            pinned: false,
            tombstoned_at: None,
            ignored: false,
            promote_hits: 0,
            manual_scoped: false,
            auto_scoped: true,
        };
        let mut global_dictionary = vec!["VoiceFlow".to_owned()];
        assert!(!super::should_remove_dictionary_entry_after_undo(
            &scoped,
            std::slice::from_ref(&scoped)
        ));
        if super::should_remove_dictionary_entry_after_undo(&scoped, std::slice::from_ref(&scoped))
        {
            super::remove_dictionary_entry(&mut global_dictionary, &scoped.after_surface);
        }
        assert_eq!(global_dictionary, ["VoiceFlow"]);

        let global = crate::store::LearnPairRecord {
            pair_key: "global-pair".into(),
            before_surface: "VoysFlow".into(),
            family: None,
            mapping_id: None,
            browser_host: None,
            native_bundle: None,
            manual_scoped: false,
            auto_scoped: false,
            ..scoped.clone()
        };
        let other_scoped = crate::store::LearnPairRecord {
            pair_key: "other-scoped-pair".into(),
            before_surface: "VoiceFlo2".into(),
            ..scoped
        };
        let rows = [global.clone(), other_scoped];
        assert!(!super::has_other_live_promoted_pair(
            &rows,
            &global.pair_key,
            "VoiceFlow"
        ));
        assert!(super::should_remove_dictionary_entry_after_undo(
            &global, &rows
        ));
    }

    #[test]
    fn suggestions_use_changed_span_not_whole_utterance() {
        assert_eq!(
            super::dictionary_suggestions("今天去知呼看看吧", "今天去知乎看看吧"),
            vec![super::DictionarySuggestion {
                pair_key: super::pair_key("知呼", "知乎"),
                before_span: "知呼".into(),
                after: "知乎".into(),
            }]
        );
    }

    #[test]
    fn local_word_hunks_keep_their_own_before_after_pairs() {
        let corrections =
            super::dictionary_suggestions("TypeScrit and Reakt", "TypeScript and React");
        assert_eq!(
            corrections,
            vec![
                super::DictionarySuggestion {
                    pair_key: super::pair_key("TypeScrit", "TypeScript"),
                    before_span: "TypeScrit".into(),
                    after: "TypeScript".into(),
                },
                super::DictionarySuggestion {
                    pair_key: super::pair_key("Reakt", "React"),
                    before_span: "Reakt".into(),
                    after: "React".into(),
                },
            ]
        );
        assert_eq!(
            super::finalize_observe("TypeScrit and Reakt", "TypeScript and React", false),
            ObserveOutcome::Ambiguous,
            "React has no independent automatic eligibility evidence from TitleCase alone"
        );
        assert!(
            super::history_lexical_correction("TypeScrit and Reakt", "TypeScript and React")
                .is_none()
        );
        assert!(
            super::single_token_candidates("TypeScrit and Reakt", "TypeScript and React")
                .is_empty()
        );
    }

    #[test]
    fn same_pair_hunks_remain_visible_until_each_is_qualified() {
        let corrections = super::make_word_corrections(vec![
            super::WordCorrectionRange {
                before: 0..9,
                after: 0..10,
                before_surface: "TypeScrit".into(),
                after_surface: "TypeScript".into(),
            },
            super::WordCorrectionRange {
                before: 20..29,
                after: 21..31,
                before_surface: "TypeScrit".into(),
                after_surface: "TypeScript".into(),
            },
        ])
        .expect("non-overlapping same-pair hunks remain separate until eligibility checks");
        assert_eq!(corrections.len(), 2);
        assert_eq!(
            corrections[0].pair_key,
            super::pair_key("TypeScrit", "TypeScript")
        );
        assert_eq!(corrections[1].pair_key, corrections[0].pair_key);
        assert_eq!(
            super::finalize_observe(
                "TypeScrit and typescrit",
                "TypeScript and typescript",
                false
            ),
            ObserveOutcome::Ambiguous,
            "the weaker second hunk must fail before the shared pair key is deduplicated"
        );
        assert!(super::history_lexical_correction(
            "TypeScrit and typescrit",
            "TypeScript and typescript"
        )
        .is_none());
        assert_eq!(
            super::finalize_observe(
                "VoiceFlo and VoiceFlo today",
                "VoiceFlow and VoiceFlow today",
                false
            ),
            ObserveOutcome::SingleToken {
                before_span: "VoiceFlo".into(),
                after: "VoiceFlow".into(),
            },
            "eligible repeated pairs are checked independently then counted once"
        );
    }

    #[test]
    fn one_two_three_local_hunks_are_bounded_and_four_abstain() {
        assert_eq!(
            super::single_token_candidates("TypeScrit", "TypeScript"),
            vec!["TypeScript".to_string()],
            "an internal missing character can still be a complete word correction"
        );
        assert_eq!(
            super::finalize_observe("TypeScrit", "TypeScript", false),
            ObserveOutcome::SingleToken {
                before_span: "TypeScrit".into(),
                after: "TypeScript".into(),
            }
        );
        let two = super::finalize_observe(
            "Use TypeScrit and VoiceFlo today",
            "Use TypeScript and VoiceFlow today",
            false,
        );
        assert!(
            matches!(two, ObserveOutcome::MultipleTokens { ref corrections, .. } if corrections.len() == 2)
        );
        let three = super::finalize_observe(
            "Use TypeScrit from VoiceFlo with Acme.Reakt",
            "Use TypeScript from VoiceFlow with Acme.React",
            false,
        );
        assert!(
            matches!(three, ObserveOutcome::MultipleTokens { ref corrections, .. } if corrections.len() == 3)
        );
        assert!(super::dictionary_suggestions(
            "Use TypeScrit from Reakt with VoiceFlo and Javscript",
            "Use TypeScript from React with VoiceFlow and JavaScript",
        )
        .is_empty());
        assert_eq!(
            super::finalize_observe(
                "Use TypeScrit from Reakt with VoiceFlo and Javscript",
                "Use TypeScript from React with VoiceFlow and JavaScript",
                false
            ),
            ObserveOutcome::Ambiguous
        );
    }

    #[test]
    fn deletes_moves_duplicate_context_and_mixed_edits_abstain_as_a_whole() {
        for (before, after) in [
            ("jonn tomorrow please", "John tomorrow"),
            ("TypeScrit and React", "React and TypeScript"),
            ("Reakt and Reakt", "React and React"),
            ("TypeScrit 2026-11-05", "TypeScript 2026-11-06"),
            ("TypeScrit", "TypeScript please"),
            ("TypeScript please", "TypeScript"),
        ] {
            assert!(
                super::dictionary_suggestions(before, after).is_empty(),
                "unexpected candidate for {before:?} -> {after:?}"
            );
            assert_eq!(
                super::finalize_observe(before, after, false),
                ObserveOutcome::Ambiguous,
                "{before:?} -> {after:?} must not learn only a safe-looking side hunk"
            );
        }
    }

    #[test]
    fn titlecase_and_object_pairs_need_more_than_surface_similarity() {
        for (before, after) in [
            ("Plane", "Place"),
            ("Report", "Resort"),
            ("please send the report", "please send the React"),
            ("Alice", "Alicf"),
        ] {
            assert!(super::history_lexical_correction(before, after).is_none());
            assert!(!super::automatic_word_pair_allowed(before, after));
        }
        assert_eq!(
            super::finalize_observe("Use TypeScrit and Reakt", "Use TypeScript and React", false),
            ObserveOutcome::Ambiguous
        );
        assert_eq!(
            super::dictionary_suggestions("Use TypeScrit and Reakt", "Use TypeScript and React")
                .iter()
                .map(|pair| (pair.before_span.as_str(), pair.after.as_str()))
                .collect::<Vec<_>>(),
            vec![("TypeScrit", "TypeScript"), ("Reakt", "React")]
        );
    }

    #[test]
    fn terminal_period_is_not_part_of_a_word_but_internal_identifier_period_is() {
        assert_eq!(
            super::dictionary_suggestions("Use Reakt. today", "Use React. today"),
            vec![super::DictionarySuggestion {
                pair_key: super::pair_key("Reakt", "React"),
                before_span: "Reakt".into(),
                after: "React".into(),
            }]
        );
        assert_eq!(
            super::dictionary_suggestions("Use Acme.Reakt today", "Use Acme.React today"),
            vec![super::DictionarySuggestion {
                pair_key: super::pair_key("Acme.Reakt", "Acme.React"),
                before_span: "Acme.Reakt".into(),
                after: "Acme.React".into(),
            }]
        );
        assert!(super::dictionary_suggestions("Use 1.5 today", "Use 15 today").is_empty());
    }

    #[test]
    fn cjk_and_emoji_context_does_not_split_or_merge_word_candidates() {
        assert_eq!(
            super::dictionary_suggestions("👩🏽‍💻 今天去知呼看看吧", "👩🏽‍💻 今天去知乎看看吧",)[0]
                .before_span,
            "知呼"
        );
        assert!(super::dictionary_suggestions("e\u{301}mail", "email").is_empty());
        assert!(super::dictionary_suggestions("TypeScrité", "TypeScripté").is_empty());
        assert!(
            super::dictionary_suggestions("TypeScrit\u{0483}", "TypeScript\u{0483}").is_empty()
        );
        assert!(super::dictionary_suggestions("👩🏽‍💻 Reakt", "👩‍💻 React").is_empty());
    }

    #[test]
    fn asymmetric_cjk_expansion_and_mixed_hunks_abstain_as_a_whole() {
        assert!(super::dictionary_suggestions("知呼呼", "知乎").is_empty());
        assert_eq!(
            super::finalize_observe("知呼呼", "知乎", false),
            ObserveOutcome::Ambiguous
        );
        let before = "TypeScrit and abcdefgh";
        let after = "TypeScript and 今天去知乎看看吧";
        assert!(super::dictionary_suggestions(before, after).is_empty());
        assert_eq!(
            super::finalize_observe(before, after, false),
            ObserveOutcome::Ambiguous
        );
    }

    #[test]
    fn bounded_cjk_name_edits_keep_the_full_two_or_three_character_name() {
        for (before, after, expected_before, expected_after) in [
            ("黎明", "李明", "黎明", "李明"),
            ("梨明辉", "李明辉", "梨明辉", "李明辉"),
            ("「黎明」", "「李明」", "黎明", "李明"),
            ("王黎明", "王李明", "王黎明", "王李明"),
            ("「王黎明」", "「王李明」", "王黎明", "王李明"),
        ] {
            let correction = super::history_lexical_correction(before, after)
                .expect("a bounded, same-sound full name remains eligible");
            assert_eq!(correction.before_span, expected_before);
            assert_eq!(correction.after, expected_after);
            assert_eq!(
                super::dictionary_suggestions(before, after)[0].before_span,
                expected_before
            );
        }
        let marked_before = "黎明\u{0301}";
        let marked_after = "李明\u{0301}";
        assert!(super::dictionary_suggestions(marked_before, marked_after).is_empty());
        assert!(super::history_lexical_correction(marked_before, marked_after).is_none());
        assert_eq!(
            super::finalize_observe(marked_before, marked_after, false),
            ObserveOutcome::Ambiguous
        );
    }

    #[test]
    fn continuous_cjk_name_edits_abstain_across_history_suggestions_and_observer() {
        let edits = [
            (
                "V20-a036e693e5b9 姓名1：请提醒黎明核对版本。",
                "V20-a036e693e5b9 姓名1：请提醒李明核对版本。",
            ),
            ("请提醒梨明核对版本。", "请提醒李明核对版本。"),
            ("请联系黎明确认需求。", "请联系李明确认需求。"),
            ("请提醒王黎明核对版本。", "请提醒王李明核对版本。"),
        ];

        for (before, after) in edits {
            assert!(
                super::dictionary_suggestions(before, after).is_empty(),
                "unexpected candidate for {before:?} -> {after:?}"
            );
            assert!(super::history_lexical_correction(before, after).is_none());
            assert_eq!(
                super::finalize_observe(before, after, false),
                ObserveOutcome::Ambiguous,
                "ambiguous name boundaries must not fall through to lexical or style learning"
            );
        }

        assert_eq!(
            super::single_token_candidates("今天去知呼看看吧", "今天去知乎看看吧"),
            vec!["知乎".to_string()]
        );
        assert_eq!(
            super::finalize_observe("今天去知呼看看吧", "今天去知乎看看吧", false),
            ObserveOutcome::SingleToken {
                before_span: "知呼".into(),
                after: "知乎".into(),
            }
        );
    }

    #[test]
    fn history_edit_uses_previous_adopted_text_and_requires_safe_lexical_evidence() {
        let cjk = super::history_lexical_correction("今天去知呼看看吧", "今天去知乎看看吧")
            .expect("single CJK spelling correction");
        assert_eq!(cjk.before_span, "知呼");
        assert_eq!(cjk.after, "知乎");

        let homophone_name =
            super::history_lexical_correction("黎明", "李明").expect("same-pinyin CJK correction");
        assert_eq!(homophone_name.after, "李明");
        assert!(super::history_lexical_correction("北京", "南京").is_none());
        assert!(super::single_token_candidates("配森", "Python").contains(&"Python".into()));
        assert!(super::history_lexical_correction("配森", "Python").is_none());
        assert!(super::is_written_number_token("million"));
        assert!(super::is_written_number_token("twentyfirst"));
        assert!(super::has_cjk_number_or_scale("三百万"));
        assert!(super::is_simple_singular_plural_pair("api", "apis"));
        assert!(super::is_simple_singular_plural_pair("github", "githubs"));

        for (before, after) in [
            ("你好", "你好呀"),
            ("版本 2", "版本 3"),
            ("一百万", "两百万"),
            ("可以", "不可以"),
            ("we can", "we can't"),
            ("配森", "completely unrelated deployment"),
            ("active", "inactive"),
            ("likely", "unlikely"),
            ("possible", "impossible"),
            ("agree", "disagree"),
            ("million", "billion"),
            ("one", "two"),
            ("twentyfirst", "twentysecond"),
            ("clone", "close"),
            ("API", "APIs"),
            ("GitHub", "GitHubs"),
            ("短句", "An unrelated paragraph is a rewrite."),
            ("rough", "polished"),
        ] {
            assert!(
                super::history_lexical_correction(before, after).is_none(),
                "must not learn {before:?} → {after:?}"
            );
        }

        for (before, after) in [("clone", "close"), ("CLONE", "CLOSE"), ("Clone", "Close")] {
            assert!(super::is_plain_action_word_correction(before, after));
            assert!(
                super::history_lexical_correction(before, after).is_none(),
                "plain action-word changes must not be learned: {before:?} → {after:?}"
            );
        }

        for (before, after) in [
            ("VoiceFlo", "VoiceFlow"),
            ("TypeScrit", "TypeScript"),
            ("HttpClinet", "HttpClient"),
        ] {
            let correction = super::history_lexical_correction(before, after)
                .expect("identifier-shaped technical typo should be eligible");
            assert_eq!(correction.before_span, before);
            assert_eq!(correction.after, after);
        }
    }

    #[test]
    fn history_learning_requires_the_exact_recorded_mapping_scope() {
        let mut mapping = test_mapping("codex");
        mapping.family = crate::context::ContextFamily::General;
        mapping.bundle_id = Some("com.openai.codex".into());
        let scene = crate::store::HistoryScene {
            profile_id: Some("user.codex".into()),
            family: Some(crate::context::family_id(mapping.family).into()),
            native_bundle: mapping.bundle_id.clone(),
            ..crate::store::HistoryScene::default()
        };
        assert!(super::history_scope_matches_mapping(&scene, &mapping));

        let mut other_app = scene.clone();
        other_app.native_bundle = Some("com.apple.TextEdit".into());
        assert!(!super::history_scope_matches_mapping(&other_app, &mapping));
        let mut other_profile = scene.clone();
        other_profile.profile_id = Some("user.other".into());
        assert!(!super::history_scope_matches_mapping(
            &other_profile,
            &mapping
        ));
        let mut missing_scope = scene;
        missing_scope.native_bundle = None;
        assert!(!super::history_scope_matches_mapping(
            &missing_scope,
            &mapping
        ));
    }

    #[test]
    fn record_learn_pair_promotes_cjk_name_on_second_observe() {
        let dir = temp_dir("rlp-name-two");
        let mut dictionary = Vec::new();
        assert_eq!(
            super::record_test_proof_pair(&dir, &mut dictionary, "李铭", "李明", None).unwrap(),
            super::RecordPairResult::Pending { hits: 1 }
        );
        assert_eq!(
            super::record_test_proof_pair(&dir, &mut dictionary, "李铭", "李明", None).unwrap(),
            super::RecordPairResult::Promoted {
                after: "李明".into()
            }
        );
        assert_eq!(dictionary, vec!["李明".to_string()]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn record_learn_pair_ignores_filler() {
        let dir = temp_dir("rlp-filler");
        let mut dictionary = Vec::new();
        assert_eq!(
            super::record_learn_pair(&dir, &mut dictionary, "那个那个", "那个").unwrap(),
            super::RecordPairResult::Ignored
        );
        assert!(dictionary.is_empty());
        assert!(
            crate::store::get_learn_pair(&dir, &super::pair_key("那个那个", "那个"))
                .unwrap()
                .is_none()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn record_learn_pair_promotes_on_third_observe() {
        let dir = temp_dir("rlp-three");
        let mut dictionary = Vec::new();
        assert_eq!(
            super::record_test_proof_pair(&dir, &mut dictionary, "知呼", "知乎", None).unwrap(),
            super::RecordPairResult::Pending { hits: 1 }
        );
        assert!(dictionary.is_empty());
        assert_eq!(
            super::record_test_proof_pair(&dir, &mut dictionary, "知呼", "知乎", None).unwrap(),
            super::RecordPairResult::Pending { hits: 2 }
        );
        assert_eq!(
            super::record_test_proof_pair(&dir, &mut dictionary, "知呼", "知乎", None).unwrap(),
            super::RecordPairResult::Promoted {
                after: "知乎".into()
            }
        );
        assert_eq!(dictionary, vec!["知乎".to_string()]);
        assert_eq!(
            super::record_learn_pair(&dir, &mut dictionary, "知呼", "知乎").unwrap(),
            super::RecordPairResult::AlreadyPromoted
        );
        assert_eq!(dictionary, vec!["知乎".to_string()]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn record_learn_pair_does_not_revive_deleted_word() {
        let dir = temp_dir("rlp-delete");
        let mut dictionary = Vec::new();
        super::record_test_proof_pair(&dir, &mut dictionary, "知呼", "知乎", None).unwrap();
        super::record_test_proof_pair(&dir, &mut dictionary, "知呼", "知乎", None).unwrap();
        super::record_test_proof_pair(&dir, &mut dictionary, "知呼", "知乎", None).unwrap();
        dictionary.clear();
        assert_eq!(
            super::record_learn_pair(&dir, &mut dictionary, "知呼", "知乎").unwrap(),
            super::RecordPairResult::AlreadyPromoted
        );
        assert!(dictionary.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn record_learn_pair_stays_pending_when_dictionary_full() {
        let dir = temp_dir("rlp-full");
        let mut dictionary: Vec<String> = (0..256).map(|index| format!("w{index}")).collect();
        super::record_test_proof_pair(&dir, &mut dictionary, "知呼", "知乎", None).unwrap();
        super::record_test_proof_pair(&dir, &mut dictionary, "知呼", "知乎", None).unwrap();
        assert_eq!(
            super::record_test_proof_pair(&dir, &mut dictionary, "知呼", "知乎", None).unwrap(),
            super::RecordPairResult::Pending { hits: 3 }
        );
        assert_eq!(dictionary.len(), 256);
        assert!(!dictionary.iter().any(|word| word == "知乎"));
        let row = crate::store::get_learn_pair(&dir, &super::pair_key("知呼", "知乎"))
            .unwrap()
            .unwrap();
        assert!(!row.promoted);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn append_dictionary_dedupes_and_respects_cap() {
        let mut dictionary = vec!["知乎".to_string()];
        assert!(!append_dictionary_entry(&mut dictionary, "知乎"));
        assert!(append_dictionary_entry(&mut dictionary, "Python"));
        assert_eq!(dictionary, vec!["知乎".to_string(), "Python".to_string()]);
        dictionary = (0..256).map(|index| format!("w{index}")).collect();
        assert!(!append_dictionary_entry(&mut dictionary, "overflow"));
        assert_eq!(dictionary.len(), 256);
    }

    #[test]
    fn record_learn_pair_ignores_tombstoned_pairs() {
        let dir = temp_dir("rlp-tombstone");
        let mut dictionary = Vec::new();
        super::record_learn_pair(&dir, &mut dictionary, "知呼", "知乎").unwrap();
        let key = super::pair_key("知呼", "知乎");
        crate::store::tombstone_learn_pair(&dir, &key).unwrap();
        assert_eq!(
            super::record_learn_pair(&dir, &mut dictionary, "知呼", "知乎").unwrap(),
            super::RecordPairResult::Ignored
        );
        assert!(dictionary.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn phrase_split_merge_does_not_create_a_word_candidate() {
        assert!(super::dictionary_suggestions("type script", "TypeScript").is_empty());
        assert_eq!(
            super::finalize_observe("type script", "TypeScript", false),
            ObserveOutcome::Ambiguous
        );
    }

    #[test]
    fn observe_emits_style_signal_for_punctuation_only() {
        let expected = test_target(42, false);
        let learned = observe_with(
            "你好。",
            &expected,
            true,
            vec![Some("你好？".into())],
            vec![expected.clone()],
        );
        match learned {
            ObserveOutcome::StyleSignal {
                style_key, excerpt, ..
            } => {
                assert_eq!(style_key, "more_questions");
                assert!(excerpt.chars().count() <= 120);
            }
            other => panic!("expected style signal, got {other:?}"),
        }
    }

    #[test]
    fn observe_emits_style_signal_for_unpunctuated_long_cjk() {
        let expected = test_target(42, false);
        let learned = observe_with(
            "今天去知乎看看那个项目怎么样",
            &expected,
            true,
            vec![Some("今天去知乎看看那个项目怎么样。".into())],
            vec![expected.clone()],
        );
        match learned {
            ObserveOutcome::StyleSignal { style_key, .. } => {
                assert_eq!(style_key, "punct_density");
            }
            other => panic!("expected style signal, got {other:?}"),
        }
    }

    #[test]
    fn observe_does_not_learn_when_send_clears_after_leaving_the_field() {
        let expected = test_target(42, false);
        let left = test_target(99, false);
        let learned = observe_with(
            "知呼",
            &expected,
            true,
            vec![Some("知乎".into()), Some(String::new())],
            vec![expected.clone(), left],
        );
        assert_eq!(learned, ObserveOutcome::LeftTarget);
    }

    #[test]
    fn style_draft_uses_bundle_when_wechat_has_no_user_mapping() {
        let mut snapshot = crate::context::ContextSnapshot::general();
        snapshot.profile.id = "chat.personal".into();
        snapshot.profile.family = crate::context::ContextFamily::PersonalChat;
        snapshot.target_guard.bundle_id = Some("com.tencent.xinWeChat".into());
        snapshot.target_guard.browser_host = None;
        assert_eq!(
            super::style_draft_mapping_id(&snapshot, &[]).as_deref(),
            Some("bundle:com.tencent.xinWeChat")
        );
    }

    #[test]
    fn short_rewrite_requires_an_explicit_style_only_transformation() {
        assert!(!is_short_style_rewrite(
            "好的我会稍后回复您",
            "好的哈哈我晚点回你"
        ));
        assert!(!is_short_style_rewrite("好的", "好的哈哈我晚点回你"));
        assert!(is_short_style_rewrite(
            "好的我会稍后回复您",
            "好的我会晚点回复你"
        ));
        assert!(is_short_style_rewrite("好的", "好的 哈哈"));
        assert!(is_short_style_rewrite(
            "您好，稍后联系。",
            "你好，晚点联系。"
        ));
        assert!(!is_short_style_rewrite(" 好的稍后 ", " 好的晚点 "));
        assert!(!is_short_style_rewrite("好的稍后", "  好的晚点"));
        assert!(!is_short_style_rewrite("好的稍后", "好的晚点  "));
        assert!(!is_short_style_rewrite("好的", &"x".repeat(80)));
    }

    #[test]
    fn complete_explicit_style_edits_are_classified_before_lexical_pairs() {
        for (before, after) in [
            ("请稍后回复。", "请晚点回复。"),
            ("您好", "你好"),
            ("好的", "好的 哈哈"),
        ] {
            assert!(matches!(
                super::finalize_observe(before, after, false),
                ObserveOutcome::ShortStyleRewrite { .. }
            ));
        }
        assert_eq!(
            super::finalize_observe("请稍后发送合同。", "请晚点发送报价。", false),
            ObserveOutcome::Ambiguous,
            "a simultaneous object change is not absorbed by the style allowlist"
        );
    }

    #[test]
    fn content_numbers_objects_quotes_and_instructions_are_not_style_or_intensity() {
        for (before, after) in [
            ("Review 2026-11-05", "Review 2026-11-06"),
            ("The cost is $1,000.", "The cost is $1000."),
            ("Version 1.5 is ready.", "Version 15 is ready."),
            ("Send the report.", "Send the invoice."),
            ("Do not send it.", "Do send it."),
            ("Make it short.", "Expand it with more detail."),
            ("Use `draft` now.", "Use `final` now."),
            ("Use \"draft\" now.", "Use \"final\" now."),
            ("fn draft() {}", "fn final() {}"),
            ("Acme.v1 is ready.", "Acme.v2 is ready."),
            ("Let's eat, Bob.", "Let's eat; Bob."),
            ("one dollar.", "two dollars."),
        ] {
            assert!(
                !is_short_style_rewrite(before, after),
                "{before:?} -> {after:?}"
            );
            assert_eq!(
                intensity_shift_from_edit(before, after, crate::llm::CleanupIntensity::Heavy),
                None,
                "{before:?} -> {after:?} must not shift intensity"
            );
        }
        assert_eq!(
            super::finalize_observe("Version 1.5 is ready.", "Version 15 is ready.", false),
            ObserveOutcome::Ambiguous
        );
        assert_eq!(
            super::finalize_observe("The cost is $1,000.", "The cost is $1000.", false),
            ObserveOutcome::Ambiguous
        );
    }

    #[test]
    fn style_pairs_cap_at_three() {
        let mut pairs = vec![];
        for i in 0..4 {
            push_style_pair(&mut pairs, format!("in{i}"), format!("out{i}"));
        }
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[0].input, "in1");
    }

    #[test]
    fn foreign_bubble_is_not_a_style_pair() {
        assert!(!is_short_style_rewrite(
            "好的我晚点回你",
            "对方: 那你把合同发我一下谢谢"
        ));
    }

    fn test_mapping(id: &str) -> crate::context::AppMapping {
        crate::context::AppMapping {
            id: id.to_owned(),
            label: "WeChat".into(),
            family: crate::context::ContextFamily::PersonalChat,
            mode_id: None,
            bundle_id: Some("com.tencent.xinWeChat".into()),
            executable: None,
            browser_host: None,
            browser_path_prefix: None,
            focused_field: None,
            source_permissions: Default::default(),
            style_example_input: None,
            style_example_output: None,
            style_example_pairs: Vec::new(),
            style_examples_approved: false,
            enabled: true,
            cleanup_effort: None,
            cleanup_intensity: None,
            cleanup_enabled: true,
            dictionary_learn_enabled: true,
        }
    }

    #[test]
    fn promoted_pair_keys_excludes_style_when_unchanged() {
        let applied = super::StyleLearnApply {
            pairs_changed: false,
            intensity_changed: true,
            style_pair_key: Some("learn:style:wechat".to_string()),
            intensity_pair_key: Some("learn:intensity:down:wechat".to_string()),
            ..Default::default()
        };
        assert_eq!(
            super::promoted_pair_keys(&applied),
            vec!["learn:intensity:down:wechat".to_string()]
        );
    }

    #[test]
    fn confirming_draft_does_not_approve_unreviewed_examples() {
        let mut mapping = test_mapping("wechat");
        mapping.style_example_pairs = vec![
            crate::context::StyleExamplePair {
                input: "未审核输入一".to_string(),
                output: "未审核输出一".to_string(),
            },
            crate::context::StyleExamplePair {
                input: "未审核输入二".to_string(),
                output: "未审核输出二".to_string(),
            },
        ];
        assert!(!mapping.style_examples_approved);

        super::confirm_style_example(&mut mapping, "确认输入", "确认输出");

        assert!(mapping.style_examples_approved);
        assert_eq!(mapping.style_example_pairs.len(), 1);
        assert_eq!(mapping.style_example_pairs[0].input, "确认输入");
        assert_eq!(mapping.style_example_pairs[0].output, "确认输出");
    }

    #[test]
    fn undo_style_pair_does_not_remove_unrelated_examples() {
        let mut pairs = vec![
            crate::context::StyleExamplePair {
                input: "保留输入".to_string(),
                output: "相同输出".to_string(),
            },
            crate::context::StyleExamplePair {
                input: "其他输入".to_string(),
                output: "其他输出".to_string(),
            },
        ];
        super::remove_style_pair(&mut pairs, "不存在输入", "不存在输出");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].output, "相同输出");
        super::remove_style_pair(&mut pairs, "保留输入", "相同输出");
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].output, "其他输出");
    }

    #[test]
    fn content_changing_style_paraphrases_do_not_create_style_or_intensity_rows() {
        let dir = temp_dir("style-content-negative");
        let mut mappings = vec![test_mapping("wechat")];
        for (before, after) in [
            ("好的我会稍后回复您", "好的哈哈我晚点回你"),
            ("好的", "好的哈哈我晚点回你"),
            ("我会稍后联系您", "我会哈哈晚点联系你"),
            ("请稍后告诉我", "哈哈晚点告诉我"),
            ("我们稍后再聊", "我们哈哈晚点聊"),
            ("您稍后回复我", "哈哈你晚点回我"),
            ("稍后我再找您", "哈哈我晚点找你"),
        ] {
            assert!(!is_short_style_rewrite(before, after));
            assert_eq!(
                intensity_shift_from_edit(before, after, crate::llm::CleanupIntensity::Heavy),
                None
            );
            let applied = apply_post_paste_style_learn(
                &mut mappings,
                &dir,
                "wechat",
                before,
                after,
                crate::llm::CleanupIntensity::Heavy,
                None,
            )
            .unwrap();
            assert_eq!(applied, super::StyleLearnApply::default());
        }
        assert!(crate::store::list_style_drafts(&dir).unwrap().is_empty());
        assert!(crate::store::list_learn_pairs(&dir)
            .unwrap()
            .iter()
            .all(|row| !row.pair_key.starts_with("learn:intensity:")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn three_short_rewrites_create_an_unapproved_style_draft() {
        let dir = temp_dir("style-learn-3x");
        let mut mappings = vec![test_mapping("wechat")];
        for _ in 0..2 {
            let applied = apply_post_paste_style_learn(
                &mut mappings,
                &dir,
                "wechat",
                "好的我会稍后回复您",
                "好的我会晚点回复你",
                crate::llm::CleanupIntensity::Heavy,
                None,
            )
            .unwrap();
            assert!(!applied.pairs_changed);
        }
        let applied = apply_post_paste_style_learn(
            &mut mappings,
            &dir,
            "wechat",
            "好的我会稍后回复您",
            "好的我会晚点回复你",
            crate::llm::CleanupIntensity::Heavy,
            None,
        )
        .unwrap();
        assert!(!applied.pairs_changed);
        assert!(applied.style_draft_key.is_some());
        assert!(mappings[0].style_example_pairs.is_empty());
        assert!(!mappings[0].style_examples_approved);
        let drafts = crate::store::list_style_drafts(&dir).unwrap();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].before_excerpt, "好的我会稍后回复您");
        assert_eq!(drafts[0].after_excerpt, "好的我会晚点回复你");
    }

    #[test]
    fn three_distinct_short_rewrites_create_style_draft() {
        let dir = temp_dir("style-learn-distinct-3x");
        let mut mappings = vec![test_mapping("wechat")];
        for (baseline, settled) in [
            ("我们稍后联系你", "我们晚点联系你"),
            ("我会稍后回复您", "我会稍后回复你"),
            ("你稍后联系我", "你晚点联系我"),
        ] {
            apply_post_paste_style_learn(
                &mut mappings,
                &dir,
                "wechat",
                baseline,
                settled,
                crate::llm::CleanupIntensity::Heavy,
                None,
            )
            .unwrap();
        }

        let drafts = crate::store::list_style_drafts(&dir).unwrap();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].before_excerpt, "你稍后联系我");
        assert_eq!(drafts[0].after_excerpt, "你晚点联系我");
        assert_eq!(
            crate::store::get_learn_pair(&dir, "learn:style:wechat")
                .unwrap()
                .unwrap()
                .hits,
            0
        );
    }

    #[test]
    fn three_casual_edits_demote_intensity() {
        let dir = temp_dir("intensity-learn-3x");
        let mut mappings = vec![test_mapping("wechat")];
        for _ in 0..2 {
            let applied = apply_post_paste_style_learn(
                &mut mappings,
                &dir,
                "wechat",
                "好的我会稍后回复您",
                "好的我会晚点回复你",
                crate::llm::CleanupIntensity::Heavy,
                None,
            )
            .unwrap();
            assert!(!applied.intensity_changed);
        }
        let applied = apply_post_paste_style_learn(
            &mut mappings,
            &dir,
            "wechat",
            "好的我会稍后回复您",
            "好的我会晚点回复你",
            crate::llm::CleanupIntensity::Heavy,
            None,
        )
        .unwrap();
        assert!(applied.intensity_changed);
        assert!(applied.style_pair_key.is_some() && applied.intensity_pair_key.is_some());
        assert_eq!(
            mappings[0].cleanup_intensity,
            Some(crate::llm::CleanupIntensity::Standard)
        );
        assert_eq!(
            intensity_shift_from_edit(
                "好的我会稍后回复您",
                "好的我会晚点回复你",
                crate::llm::CleanupIntensity::Heavy
            ),
            Some(-1)
        );
    }

    #[test]
    fn intensity_can_shift_twice_in_same_direction() {
        let dir = temp_dir("intensity-learn-two-shifts");
        let mut mappings = vec![test_mapping("wechat")];
        let edits = [
            ("我会稍后回复您", "我会晚点回复你"),
            ("我们稍后联系您", "我们晚点联系你"),
            ("请稍后告诉您", "请晚点告诉你"),
            ("您稍后回复我", "你晚点回复我"),
            ("稍后我再找您", "晚点我再找你"),
            ("稍后请联系您", "晚点请联系你"),
        ];
        for (baseline, settled) in edits {
            apply_post_paste_style_learn(
                &mut mappings,
                &dir,
                "wechat",
                baseline,
                settled,
                crate::llm::CleanupIntensity::Heavy,
                None,
            )
            .unwrap();
        }
        assert_eq!(
            mappings[0].cleanup_intensity,
            Some(crate::llm::CleanupIntensity::Light)
        );
        let intensity = crate::store::get_learn_pair(&dir, "learn:intensity:down:wechat")
            .unwrap()
            .unwrap();
        assert_eq!(intensity.before_surface, "standard");
        assert_eq!(intensity.after_surface, "light");
    }

    #[test]
    fn repeated_style_observation_from_one_session_counts_once() {
        let dir = temp_dir("style-observation-idempotency");
        let mut mappings = vec![test_mapping("wechat")];
        let baseline = "好的我会稍后回复您";
        let settled = "好的我会晚点回复你";

        for _ in 0..2 {
            let applied = super::apply_post_paste_style_learn_for_session(
                &mut mappings,
                &dir,
                "wechat",
                baseline,
                settled,
                crate::llm::CleanupIntensity::Heavy,
                Some("process-nonce:session-1"),
                None,
            )
            .unwrap();
            assert!(!applied.intensity_changed);
        }
        for pair_key in ["learn:style:wechat", "learn:intensity:down:wechat"] {
            assert_eq!(
                crate::store::get_learn_pair(&dir, pair_key)
                    .unwrap()
                    .unwrap()
                    .hits,
                1
            );
        }

        let applied = super::apply_post_paste_style_learn_for_session(
            &mut mappings,
            &dir,
            "wechat",
            baseline,
            settled,
            crate::llm::CleanupIntensity::Heavy,
            Some("process-nonce:session-2"),
            None,
        )
        .unwrap();
        assert!(!applied.intensity_changed);
        assert_eq!(
            crate::store::get_learn_pair(&dir, "learn:style:wechat")
                .unwrap()
                .unwrap()
                .hits,
            2
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tagged_intensity_tombstone_suppresses_change_without_losing_style_work() {
        use chacha20poly1305::aead::{AeadCore, OsRng};
        struct OwnedDirectory(std::path::PathBuf);
        impl Drop for OwnedDirectory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let nonce = chacha20poly1305::XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let dir =
            std::env::temp_dir().join(format!("voiceflow-intensity-tagged-{}", hex::encode(nonce)));
        std::fs::create_dir(&dir).unwrap();
        let _owned = OwnedDirectory(dir.clone());
        let key = "learn:intensity:down:wechat";
        assert!(matches!(
            crate::store::upsert_learn_pair_with_scope(&dir, key, "heavy", "standard", None)
                .unwrap(),
            crate::store::LearnPairUpsert::Stored(_)
        ));
        assert!(crate::store::tombstone_learn_pair(&dir, key).unwrap());
        let before = crate::store::get_learn_pair(&dir, key).unwrap().unwrap();
        let mut mappings = vec![test_mapping("wechat")];
        for _ in 0..4 {
            let applied = apply_post_paste_style_learn(
                &mut mappings,
                &dir,
                "wechat",
                "好的我会稍后回复您",
                "好的我会晚点回复你",
                crate::llm::CleanupIntensity::Heavy,
                None,
            )
            .unwrap();
            assert!(!applied.intensity_changed);
            assert!(mappings[0].cleanup_intensity.is_none());
        }
        assert_eq!(
            crate::store::get_learn_pair(&dir, key).unwrap().unwrap(),
            before
        );
        assert!(!crate::store::list_style_drafts(&dir).unwrap().is_empty());
    }
    #[test]
    fn screen_token_does_not_become_style_pair() {
        let ctx = crate::screen_text::ScreenTextContext {
            evidence: crate::screen_text::ContextEvidence {
                items: vec![crate::screen_text::ContextEvidenceItem {
                    source: crate::screen_text::ContextEvidenceSource::Ax,
                    kind: crate::screen_text::ContextEvidenceKind::Term,
                    value: "晓雯".into(),
                    confidence_milli: None,
                    truncated: false,
                }],
                ..Default::default()
            },
            family: crate::context::ContextFamily::PersonalChat,
            ..crate::screen_text::ScreenTextContext::default()
        };
        let dir = temp_dir("screen-no-style");
        let mut dictionary = Vec::new();
        let mut pairs: Vec<crate::context::StyleExamplePair> = Vec::new();
        seed_lexicon_from_screen(
            &dir,
            &mut dictionary,
            &ctx,
            &mut pairs,
            None,
            crate::context::ContextSourcePermissions {
                ax_text: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(pairs.is_empty());
        assert!(!dictionary
            .iter()
            .any(|item| item == "在吗" || item == "晚点回你"));
        let rows = crate::store::list_learn_pairs(&dir).unwrap();
        assert!(!rows
            .iter()
            .any(|row| row.pair_key.starts_with("learn:style")));
        assert!(!rows
            .iter()
            .any(|row| row.after_surface == "在吗" || row.after_surface == "晚点回你"));
    }

    #[test]
    fn screen_terms_without_provider_grant_stay_out_of_dictionary() {
        let ctx = crate::screen_text::ScreenTextContext {
            evidence: crate::screen_text::ContextEvidence {
                items: vec![crate::screen_text::ContextEvidenceItem {
                    source: crate::screen_text::ContextEvidenceSource::Ax,
                    kind: crate::screen_text::ContextEvidenceKind::Term,
                    value: "晓雯".into(),
                    confidence_milli: None,
                    truncated: false,
                }],
                ..Default::default()
            },
            family: crate::context::ContextFamily::PersonalChat,
            ..crate::screen_text::ScreenTextContext::default()
        };
        let dir = temp_dir("screen-term-no-provider-grant");
        let mut dictionary = Vec::new();
        let mut pairs = Vec::new();
        let permissions = crate::context::ContextSourcePermissions {
            ax_text: true,
            ..Default::default()
        };
        assert!(ctx.asr_terms(permissions).is_empty());
        seed_lexicon_from_screen(&dir, &mut dictionary, &ctx, &mut pairs, None, permissions)
            .unwrap();
        assert!(dictionary.is_empty());
        let rows = crate::store::list_learn_pairs(&dir).unwrap();
        assert!(!rows.iter().any(|row| row.after_surface == "晓雯"));
    }

    #[test]
    fn person_name_promotes_at_two() {
        let ctx = crate::screen_text::ScreenTextContext {
            evidence: crate::screen_text::ContextEvidence {
                items: vec![crate::screen_text::ContextEvidenceItem {
                    source: crate::screen_text::ContextEvidenceSource::Ax,
                    kind: crate::screen_text::ContextEvidenceKind::Term,
                    value: "李明".into(),
                    confidence_milli: None,
                    truncated: false,
                }],
                ..Default::default()
            },
            family: crate::context::ContextFamily::PersonalChat,
            ..crate::screen_text::ScreenTextContext::default()
        };
        let dir = temp_dir("screen-name-2x");
        let mut dictionary = Vec::new();
        let mut pairs = Vec::new();
        let permissions = crate::context::ContextSourcePermissions {
            ax_text: true,
            context_text_to_providers: true,
            ..Default::default()
        };
        seed_lexicon_from_screen(&dir, &mut dictionary, &ctx, &mut pairs, None, permissions)
            .unwrap();
        assert!(dictionary.is_empty());
        seed_lexicon_from_screen(&dir, &mut dictionary, &ctx, &mut pairs, None, permissions)
            .unwrap();
        assert!(dictionary.contains(&"李明".to_string()));
        assert!(pairs.is_empty());
    }

    #[test]
    fn c0_manual_dictionary_owner_survives_rule_undo_and_retries_pending_add() {
        let dir = temp_dir("c0-manual-rule-undo-retry");
        let mut settings = crate::store::Settings::default();
        settings.add_dictionary_source("VoiceFlow", crate::store::DictionarySource::Manual);
        settings.add_dictionary_source("LegacyWord", crate::store::DictionarySource::LegacyUnknown);
        crate::store::save_configuration(&dir, &settings).unwrap();

        let before = "VoiceFlo";
        let after = "VoiceFlow";
        let key = super::pair_key(before, after);
        let scope = crate::store::LearnPairScope {
            family: Some("general".into()),
            mapping_id: Some("fixture".into()),
            native_bundle: Some("com.example.fixture".into()),
            ..Default::default()
        };
        assert!(matches!(
            crate::store::upsert_learn_pair_with_scope(&dir, &key, before, after, Some(&scope))
                .unwrap(),
            crate::store::LearnPairUpsert::Stored(_)
        ));
        crate::store::ensure_learn_pair_promoted(&dir, &key, before, after, Some(&scope)).unwrap();
        let auto_before = "Autocorret";
        let auto_after = "Autocorrect";
        let auto_key = super::pair_key(auto_before, auto_after);
        assert!(matches!(
            crate::store::upsert_learn_pair_with_scope(
                &dir,
                &auto_key,
                auto_before,
                auto_after,
                Some(&scope)
            )
            .unwrap(),
            crate::store::LearnPairUpsert::Stored(_)
        ));
        crate::store::ensure_learn_pair_promoted(
            &dir,
            &auto_key,
            auto_before,
            auto_after,
            Some(&scope),
        )
        .unwrap();

        let settings_path = dir.join("settings.json");
        let mut first_attempt: crate::store::Settings =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        let failed = match super::save_settings_with_dictionary_effects(
            &dir,
            &mut first_attempt,
            true,
            true,
            |_| Err(anyhow::anyhow!("injected Settings write failure")),
        ) {
            Err(error) => error,
            Ok(_) => panic!("the injected Settings write failure must be returned"),
        };
        assert!(failed.starts_with(super::DICTIONARY_SETTINGS_PARTIAL_ERROR));
        assert_eq!(
            crate::store::list_pending_dictionary_effects(&dir)
                .unwrap()
                .len(),
            2
        );

        let mut retry: crate::store::Settings =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        let outcome = super::save_settings_with_dictionary_effects(
            &dir,
            &mut retry,
            false,
            true,
            |settings| crate::store::save_configuration(&dir, settings),
        )
        .unwrap();
        assert!(outcome.saved);
        assert!(outcome.sync_error.is_none());
        assert_eq!(retry.dictionary, ["VoiceFlow", "LegacyWord", "Autocorrect"]);
        assert!(retry.has_dictionary_source("VoiceFlow", &crate::store::DictionarySource::Manual));
        assert!(retry
            .has_dictionary_source("LegacyWord", &crate::store::DictionarySource::LegacyUnknown));
        assert!(retry.has_dictionary_source(
            "VoiceFlow",
            &crate::store::DictionarySource::Rule(key.clone())
        ));
        assert!(retry.has_dictionary_source(
            "Autocorrect",
            &crate::store::DictionarySource::Rule(auto_key.clone())
        ));
        assert!(crate::store::list_pending_dictionary_effects(&dir)
            .unwrap()
            .is_empty());

        assert!(crate::store::tombstone_learn_pair(&dir, &key).unwrap());
        assert!(crate::store::tombstone_learn_pair(&dir, &auto_key).unwrap());
        let mut failed_undo: crate::store::Settings =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        let failed = match super::save_settings_with_dictionary_effects(
            &dir,
            &mut failed_undo,
            true,
            false,
            |_| {
                Err(anyhow::anyhow!(
                    "injected Settings write failure after undo"
                ))
            },
        ) {
            Err(error) => error,
            Ok(_) => panic!("the injected Settings write failure must be returned"),
        };
        assert!(failed.starts_with(super::DICTIONARY_SETTINGS_PARTIAL_ERROR));
        let pending_removes = crate::store::list_pending_dictionary_effects(&dir).unwrap();
        assert_eq!(pending_removes.len(), 2);
        assert!(pending_removes
            .iter()
            .all(|effect| effect.action == crate::store::DictionaryEffectAction::Remove));

        let delete_before = "RetryTrm";
        let delete_after = "RetryTerm";
        let delete_key = super::pair_key(delete_before, delete_after);
        assert!(matches!(
            crate::store::upsert_learn_pair_with_scope(
                &dir,
                &delete_key,
                delete_before,
                delete_after,
                Some(&scope)
            )
            .unwrap(),
            crate::store::LearnPairUpsert::Stored(_)
        ));
        crate::store::ensure_learn_pair_promoted(
            &dir,
            &delete_key,
            delete_before,
            delete_after,
            Some(&scope),
        )
        .unwrap();
        let mut attempted_delete: crate::store::Settings =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        attempted_delete.add_dictionary_source(
            delete_after,
            crate::store::DictionarySource::Rule(delete_key.clone()),
        );
        crate::store::save_configuration(&dir, &attempted_delete).unwrap();
        assert!(
            crate::store::cancel_pending_dictionary_effects_for_word(&dir, delete_after).unwrap()
        );
        let pending_after_cancel = crate::store::list_pending_dictionary_effects(&dir).unwrap();
        assert_eq!(pending_after_cancel.len(), 2);
        assert!(pending_after_cancel
            .iter()
            .all(|effect| effect.action == crate::store::DictionaryEffectAction::Remove));
        assert!(pending_after_cancel
            .iter()
            .any(|effect| effect.pair_key == key));
        assert!(pending_after_cancel
            .iter()
            .any(|effect| effect.pair_key == auto_key));

        attempted_delete.remove_dictionary_word(delete_after);
        let failed_delete = match super::save_settings_with_dictionary_effects(
            &dir,
            &mut attempted_delete,
            true,
            true,
            |_| {
                Err(anyhow::anyhow!(
                    "injected Settings write failure after delete"
                ))
            },
        ) {
            Err(error) => error,
            Ok(_) => panic!("the injected delete Settings failure must be returned"),
        };
        assert!(failed_delete.starts_with(super::DICTIONARY_SETTINGS_PARTIAL_ERROR));
        let pending_after_delete = crate::store::list_pending_dictionary_effects(&dir).unwrap();
        assert_eq!(pending_after_delete.len(), 2);
        assert!(pending_after_delete
            .iter()
            .all(|effect| effect.action == crate::store::DictionaryEffectAction::Remove));

        let mut after_undo: crate::store::Settings =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        assert!(after_undo.has_dictionary_source(
            delete_after,
            &crate::store::DictionarySource::Rule(delete_key.clone())
        ));
        after_undo.remove_dictionary_word(delete_after);
        let outcome = super::save_settings_with_dictionary_effects(
            &dir,
            &mut after_undo,
            false,
            false,
            |settings| crate::store::save_configuration(&dir, settings),
        )
        .unwrap();
        assert!(outcome.saved);
        assert_eq!(after_undo.dictionary, ["VoiceFlow", "LegacyWord"]);
        assert!(
            after_undo.has_dictionary_source("VoiceFlow", &crate::store::DictionarySource::Manual)
        );
        assert!(!after_undo
            .has_dictionary_source("VoiceFlow", &crate::store::DictionarySource::Rule(key)));
        assert!(!after_undo
            .dictionary
            .iter()
            .any(|word| word == "Autocorrect"));
        assert!(!after_undo.has_dictionary_source(
            "Autocorrect",
            &crate::store::DictionarySource::LegacyUnknown
        ));
        assert!(!after_undo.has_dictionary_source(
            delete_after,
            &crate::store::DictionarySource::Rule(delete_key)
        ));
        assert!(after_undo
            .has_dictionary_source("LegacyWord", &crate::store::DictionarySource::LegacyUnknown));
        assert!(crate::store::list_pending_dictionary_effects(&dir)
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn c0_explicit_delete_cancels_pending_add_and_reports_failed_settings_save() {
        let dir = temp_dir("c0-explicit-delete-pending-add");
        let before = "VoiceFlo";
        let after = "VoiceFlow";
        let key = super::pair_key(before, after);
        let scope = crate::store::LearnPairScope {
            family: Some("general".into()),
            mapping_id: Some("fixture".into()),
            native_bundle: Some("com.example.fixture".into()),
            ..Default::default()
        };
        let mut original = crate::store::Settings::default();
        original.add_dictionary_source(after, crate::store::DictionarySource::Rule(key.clone()));
        original.add_dictionary_source(after, crate::store::DictionarySource::Manual);
        original.add_dictionary_source("LegacyWord", crate::store::DictionarySource::LegacyUnknown);
        crate::store::save_configuration(&dir, &original).unwrap();
        assert!(matches!(
            crate::store::upsert_learn_pair_with_scope(&dir, &key, before, after, Some(&scope))
                .unwrap(),
            crate::store::LearnPairUpsert::Stored(_)
        ));
        crate::store::ensure_learn_pair_promoted(&dir, &key, before, after, Some(&scope)).unwrap();

        assert!(crate::store::cancel_pending_dictionary_effects_for_word(&dir, after).unwrap());
        assert!(crate::store::list_pending_dictionary_effects(&dir)
            .unwrap()
            .is_empty());
        let mut attempted_delete: crate::store::Settings =
            serde_json::from_slice(&std::fs::read(dir.join("settings.json")).unwrap()).unwrap();
        attempted_delete.remove_dictionary_word(after);
        let error = match super::save_settings_with_dictionary_effects(
            &dir,
            &mut attempted_delete,
            true,
            true,
            |_| {
                Err(anyhow::anyhow!(
                    "injected Settings write failure after delete"
                ))
            },
        ) {
            Err(error) => error,
            Ok(_) => panic!("the injected Settings write failure must be returned"),
        };
        assert!(error.starts_with(super::DICTIONARY_SETTINGS_PARTIAL_ERROR));
        assert!(crate::store::list_pending_dictionary_effects(&dir)
            .unwrap()
            .is_empty());

        let mut retry: crate::store::Settings =
            serde_json::from_slice(&std::fs::read(dir.join("settings.json")).unwrap()).unwrap();
        assert!(
            retry.has_dictionary_source(after, &crate::store::DictionarySource::Rule(key.clone()))
        );
        assert!(retry.has_dictionary_source(after, &crate::store::DictionarySource::Manual));
        retry.remove_dictionary_word(after);
        let outcome = super::save_settings_with_dictionary_effects(
            &dir,
            &mut retry,
            false,
            true,
            |settings| crate::store::save_configuration(&dir, settings),
        )
        .unwrap();
        assert!(outcome.saved);
        assert!(outcome.sync_error.is_none());
        assert!(!crate::store::dictionary_contains_word(
            &retry.dictionary,
            after
        ));
        assert!(!retry.has_dictionary_source(after, &crate::store::DictionarySource::Rule(key)));
        assert!(retry
            .has_dictionary_source("LegacyWord", &crate::store::DictionarySource::LegacyUnknown));
        assert!(crate::store::list_pending_dictionary_effects(&dir)
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn c0_paste_and_history_share_one_counted_dictation_origin() {
        let dir = temp_dir("c0-paste-history-origin");
        crate::store::insert_history(&dir, "VoiceFlo", "VoiceFlo", 0.1, false).unwrap();
        let history_id = crate::store::get_history(&dir, 5).unwrap()[0].id;
        let origin = "dictation-c0-origin";
        let connection = rusqlite::Connection::open(dir.join("history.sqlite")).unwrap();
        connection
            .execute(
                "UPDATE dictations SET transaction_id = ?1 WHERE id = ?2",
                rusqlite::params![origin, history_id],
            )
            .unwrap();
        drop(connection);
        let revision =
            crate::store::save_manual_history_revision(&dir, history_id, "VoiceFlow").unwrap();
        let revision_id = revision.revision_id.unwrap();
        let before = "VoiceFlo";
        let after = "VoiceFlow";
        let key = super::pair_key(before, after);
        let scope = crate::store::LearnPairScope {
            family: Some("general".into()),
            mapping_id: Some("fixture".into()),
            native_bundle: Some("com.example.fixture".into()),
            ..Default::default()
        };
        let paste = crate::store::LearnPairObservation::paste_session_with_dictation_origin(
            "paste-session-c0",
            &key,
            origin,
        );
        assert!(matches!(
            crate::store::upsert_learn_pair_with_observation(
                &dir,
                &key,
                before,
                after,
                Some(&scope),
                Some(&paste)
            )
            .unwrap(),
            crate::store::LearnPairUpsert::Stored(_)
        ));
        let history = crate::store::LearnPairObservation::history_edit_from_store(
            &dir,
            history_id,
            revision_id,
            &key,
        )
        .unwrap()
        .unwrap();
        assert!(matches!(
            crate::store::upsert_learn_pair_with_observation(
                &dir,
                &key,
                before,
                after,
                Some(&scope),
                Some(&history)
            )
            .unwrap(),
            crate::store::LearnPairUpsert::Skipped(
                crate::store::LearnPairSkipReason::DuplicateObservation
            )
        ));
        assert_eq!(
            crate::store::count_independent_learn_pair_hits(&dir, &key).unwrap(),
            1
        );
        crate::store::delete_history(&dir, history_id).unwrap();
        assert_eq!(
            crate::store::count_independent_learn_pair_hits(&dir, &key).unwrap(),
            0
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn c0_legacy_hits_plus_one_known_origin_do_not_promote() {
        let dir = temp_dir("c0-legacy-plus-one-origin");
        crate::store::insert_history(&dir, "VoiceFlo", "VoiceFlo", 0.1, false).unwrap();
        let history_id = crate::store::get_history(&dir, 5).unwrap()[0].id;
        let origin = "dictation-c0-single-proof";
        let connection = rusqlite::Connection::open(dir.join("history.sqlite")).unwrap();
        connection
            .execute(
                "UPDATE dictations SET transaction_id = ?1 WHERE id = ?2",
                rusqlite::params![origin, history_id],
            )
            .unwrap();
        drop(connection);

        let before = "VoiceFlo";
        let after = "VoiceFlow";
        let key = super::pair_key(before, after);
        let scope = crate::store::LearnPairScope {
            family: Some("general".into()),
            mapping_id: Some("fixture".into()),
            native_bundle: Some("com.example.fixture".into()),
            ..Default::default()
        };
        crate::store::upsert_learn_pair_with_scope(&dir, &key, before, after, Some(&scope))
            .unwrap();
        let connection = rusqlite::Connection::open(dir.join("history.sqlite")).unwrap();
        connection
            .execute(
                "UPDATE learn_pairs SET hits = 2 WHERE pair_key = ?1",
                [&key],
            )
            .unwrap();
        drop(connection);
        let observation = crate::store::LearnPairObservation::paste_session_with_dictation_origin(
            "paste-session-c0-legacy",
            &key,
            origin,
        );
        let result = super::record_learn_pair_with_observation_audited(
            &dir,
            &mut Vec::new(),
            before,
            after,
            Some(&scope),
            Some(&observation),
            false,
            None,
        )
        .unwrap();
        assert_eq!(result, super::RecordPairResult::Pending { hits: 1 });
        assert_eq!(
            crate::store::count_independent_learn_pair_hits(&dir, &key).unwrap(),
            1
        );
        let row = crate::store::get_learn_pair(&dir, &key).unwrap().unwrap();
        assert_eq!(row.hits, 3);
        assert!(!row.is_live_promoted());
        let _ = std::fs::remove_dir_all(dir);
    }
}
