//! Single-token dictionary candidates from a before/after edit.
//!
//! History uses this for confirm-to-add suggestions. After a verified paste,
//! `observe_after_paste` watches only the same focused field for a short
//! window and silently appends an unambiguous single-token *correction*.
//! Pure insertions (append-only typing) are never learned.
//! Never install a global key event tap.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::{context, lock_recover, store, AppState};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};

const MAX_CANDIDATES: usize = 3;
const MAX_LENGTH_DELTA: usize = 12;

#[derive(Clone, Debug)]
struct Token {
    surface: String,
    key: String,
}

pub fn single_token_candidates(before: &str, after: &str) -> Vec<String> {
    if after.contains("***") || before.contains("***") {
        return Vec::new();
    }
    if before.chars().count().abs_diff(after.chars().count()) > MAX_LENGTH_DELTA {
        return Vec::new();
    }

    let (before_span, after_span) = changed_spans(before, after);
    if before_span.is_empty() {
        return Vec::new();
    }
    let before_keys: HashSet<String> = extract_tokens(&before_span)
        .into_iter()
        .map(|token| token.key)
        .collect();

    let mut seen = HashSet::new();
    let mut added = Vec::new();
    for token in extract_tokens(&after_span) {
        if before_keys.contains(&token.key) || !seen.insert(token.key.clone()) {
            continue;
        }
        if is_blocked(&token.surface) {
            continue;
        }
        added.push(token.surface);
    }

    if added.len() > MAX_CANDIDATES {
        return Vec::new();
    }
    if added.len() == 1 && added[0] == after {
        let cjk_count = after.chars().filter(|ch| is_cjk(*ch)).count();
        if cjk_count > 4 {
            return Vec::new();
        }
    }
    added
}

fn changed_spans(before: &str, after: &str) -> (String, String) {
    let before_chars: Vec<char> = before.chars().collect();
    let after_chars: Vec<char> = after.chars().collect();
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
    let before_span = before_chars.get(start..before_end).unwrap_or(&[]).iter().collect();
    let after_span = after_chars.get(start..after_end).unwrap_or(&[]).iter().collect();
    (before_span, after_span)
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
    let (before_span, _) = changed_spans(before, after);
    single_token_candidates(before, after)
        .into_iter()
        .map(|word| DictionarySuggestion {
            pair_key: pair_key(&before_span, &word),
            before_span: before_span.clone(),
            after: word,
        })
        .collect()
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
    mutate_dictionary(&app, |dictionary| {
        add_dictionary_words(dictionary, &words);
    })
    .await
}

#[tauri::command]
pub(crate) async fn remove_dictionary_word(
    app: tauri::AppHandle,
    word: String,
) -> Result<store::SettingsView, String> {
    mutate_dictionary(&app, |dictionary| {
        remove_dictionary_entry(dictionary, &word);
    })
    .await
}

#[tauri::command]
pub(crate) fn list_learn_pairs(app: tauri::AppHandle) -> Result<Vec<store::LearnPairRecord>, String> {
    let dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    let rows = store::list_learn_pairs(&dir).map_err(|error| error.to_string())?;
    Ok(rows.into_iter().filter(|row| row.is_pending()).collect())
}

#[tauri::command]
pub(crate) async fn promote_learn_pair(
    app: tauri::AppHandle,
    pair_key: String,
    before_surface: Option<String>,
    after_surface: Option<String>,
    history_id: Option<i64>,
) -> Result<store::SettingsView, String> {
    let dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    let row = store::get_learn_pair(&dir, &pair_key).map_err(|error| error.to_string())?;
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
    let view = mutate_dictionary(&app, |dictionary| {
        append_dictionary_entry(dictionary, &after);
    })
    .await?;
    let scope = history_id.and_then(|id| store::history_scene(&dir, id).ok())
        .map(|scene| scene.learn_scope());
    store::ensure_learn_pair_promoted(&dir, &pair_key, &before, &after, scope.as_ref())
        .map_err(|error| error.to_string())?;
    let _ = app.emit("learn_pairs://changed", ());
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

#[tauri::command]
pub(crate) async fn ignore_learn_pair(app: tauri::AppHandle, pair_key: String) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    store::tombstone_learn_pair(&dir, &pair_key).map_err(|error| error.to_string())?;
    let _ = app.emit("learn_pairs://changed", ());
    Ok(())
}

#[tauri::command]
pub(crate) async fn undo_learn_pair(
    app: tauri::AppHandle,
    pair_key: String,
) -> Result<store::SettingsView, String> {
    let dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    let row = store::get_learn_pair(&dir, &pair_key)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "learn pair was not found".to_string())?;
    store::tombstone_learn_pair(&dir, &pair_key).map_err(|error| error.to_string())?;
    let still_used = store::list_learn_pairs(&dir)
        .map_err(|error| error.to_string())?
        .into_iter()
        .any(|item| {
            item.pair_key != pair_key
                && item.is_live_promoted()
                && item.after_surface == row.after_surface
        });
    let view = if still_used {
        let state = app.state::<AppState>();
        let snapshot = lock_recover(&state.settings).clone();
        store::SettingsView::from(&snapshot)
    } else {
        mutate_dictionary(&app, |dictionary| {
            remove_dictionary_entry(dictionary, &row.after_surface);
        })
        .await?
    };
    let _ = app.emit("learn_pairs://changed", ());
    Ok(view)
}

#[tauri::command]
pub(crate) fn list_pinned_terms(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    let dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
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
    let dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    store::ensure_pinned_dictionary_term(&dir, &word, pinned).map_err(|error| error.to_string())?;
    let _ = app.emit("learn_pairs://changed", ());
    Ok(())
}

#[tauri::command]
pub(crate) fn list_style_drafts(app: tauri::AppHandle) -> Result<Vec<store::StyleDraftRecord>, String> {
    let dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    store::list_style_drafts(&dir).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn confirm_style_draft(
    app: tauri::AppHandle,
    draft_key: String,
) -> Result<store::SettingsView, String> {
    let dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    let draft = store::get_style_draft(&dir, &draft_key)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "style draft was not found".to_string())?;
    let state = app.state::<AppState>();
    let _gate = state.settings_gate.lock().await;
    let mut snapshot = lock_recover(&state.settings).clone();
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
    mapping.style_example_input = Some(draft.before_excerpt.clone());
    mapping.style_example_output = Some(draft.after_excerpt.clone());
    snapshot.normalize();
    store::save_settings(&dir, &snapshot).map_err(|error| error.to_string())?;
    *lock_recover(&state.settings) = snapshot.clone();
    {
        let mut context = lock_recover(&state.context);
        context.mappings = snapshot.context_mappings.clone();
    }
    store::delete_style_draft(&dir, &draft_key).map_err(|error| error.to_string())?;
    let view = store::SettingsView::from(&snapshot);
    let _ = app.emit("settings://changed", view.clone());
    let _ = app.emit("style_drafts://changed", ());
    Ok(view)
}

#[tauri::command]
pub(crate) async fn dismiss_style_draft(app: tauri::AppHandle, draft_key: String) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    store::delete_style_draft(&dir, &draft_key).map_err(|error| error.to_string())?;
    let _ = app.emit("style_drafts://changed", ());
    Ok(())
}

async fn mutate_dictionary(
    app: &tauri::AppHandle,
    mutate: impl FnOnce(&mut Vec<String>),
) -> Result<store::SettingsView, String> {
    let state = app.state::<AppState>();
    let _gate = state.settings_gate.lock().await;
    let snapshot = {
        let mut settings = lock_recover(&state.settings);
        mutate(&mut settings.dictionary);
        settings.normalize();
        settings.clone()
    };
    let dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    store::save_settings(&dir, &snapshot).map_err(|error| error.to_string())?;
    *lock_recover(&state.settings) = snapshot.clone();
    let view = store::SettingsView::from(&snapshot);
    let _ = app.emit("settings://changed", view.clone());
    Ok(view)
}

const OBSERVE_INITIAL: Duration = Duration::from_millis(3000);
const OBSERVE_INTERVAL: Duration = Duration::from_millis(400);
const OBSERVE_IDLE_GRACE: Duration = Duration::from_millis(1500);
const OBSERVE_MAX_TOTAL: Duration = Duration::from_millis(12000);

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
    SingleToken { before_span: String, after: String },
    StyleSignal {
        excerpt: String,
        style_key: String,
        before_excerpt: String,
        after_excerpt: String,
    },
    Ambiguous,
}

pub fn observe_after_paste(
    post_insert_field: &str,
    expected_target: &context::TargetAppGuard,
    dictionary_learn_enabled: bool,
    limits: ObserveLimits,
    mut read_value: impl FnMut() -> Option<String>,
    mut read_target: impl FnMut() -> context::TargetAppGuard,
    mut sleep: impl FnMut(Duration),
) -> ObserveOutcome {
    if !dictionary_learn_enabled || expected_target.secure_input || post_insert_field.is_empty() {
        return ObserveOutcome::Unchanged;
    }
    if limits.interval.is_zero() {
        return ObserveOutcome::Unchanged;
    }

    let mut elapsed = Duration::ZERO;
    let mut deadline = limits.initial;
    let mut last_settled = post_insert_field.to_string();
    while elapsed < deadline.min(limits.max_total) {
        sleep(limits.interval);
        elapsed = elapsed.saturating_add(limits.interval);

        let current_target = read_target();
        if context::focus_mismatch_reason(expected_target, &current_target).is_some() {
            if let Some(current) = read_value() {
                if !current.trim().is_empty() {
                    last_settled = current;
                }
            }
            return finalize_observe(post_insert_field, &last_settled, true);
        }
        let Some(current) = read_value() else {
            continue;
        };
        if current != last_settled {
            last_settled = current;
            deadline = limits.max_total.min(elapsed.saturating_add(limits.idle_grace));
        }
    }

    finalize_observe(post_insert_field, &last_settled, false)
}

fn finalize_observe(baseline: &str, settled: &str, left_target: bool) -> ObserveOutcome {
    if settled == baseline {
        return if left_target {
            ObserveOutcome::LeftTarget
        } else {
            ObserveOutcome::Unchanged
        };
    }
    if let Some((before_span, after)) = lexeme_candidate(baseline, settled) {
        return ObserveOutcome::SingleToken { before_span, after };
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

fn lexeme_candidate(before: &str, after: &str) -> Option<(String, String)> {
    let (before_span, after_span) = changed_spans(before, after);
    if before_span.is_empty() || after_span.is_empty() || before_span == after_span {
        return None;
    }
    let mut tokens = single_token_candidates(before, after);
    if tokens.len() == 1 {
        return Some((before_span, tokens.pop().expect("single candidate")));
    }
    if is_short_phrase(&before_span) && is_short_phrase(&after_span) {
        if after_span == after.trim() && extract_tokens(after).len() > 2 {
            return None;
        }
        return Some((before_span, after_span));
    }
    None
}

fn is_short_phrase(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.contains(['。', '！', '？', '.', '!', '?'])
    {
        return false;
    }
    let latin_words = extract_tokens(trimmed)
        .into_iter()
        .filter(|token| token.surface.chars().any(|ch| is_latin_start(ch)))
        .count();
    let cjk_count = trimmed.chars().filter(|ch| is_cjk(*ch)).count();
    if latin_words > 0 && cjk_count == 0 {
        (2..=4).contains(&latin_words)
    } else if latin_words == 0 && cjk_count > 0 {
        (2..=8).contains(&cjk_count)
            && trimmed
                .chars()
                .all(|ch| is_cjk(ch) || ch.is_whitespace())
    } else {
        false
    }
}

fn style_signal(before: &str, after: &str) -> Option<ObserveOutcome> {
    let before_content = style_content_key(before);
    let after_content = style_content_key(after);
    if before_content.is_empty() || before_content != after_content {
        return None;
    }
    let before_profile = punctuation_profile(before);
    let after_profile = punctuation_profile(after);
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

fn style_content_key(value: &str) -> String {
    value.chars().filter(|ch| !is_style_noise(*ch)).collect()
}

fn is_style_noise(value: char) -> bool {
    value.is_whitespace()
        || matches!(
            value,
            '.' | '。'
                | '?' | '？'
                | '!' | '！'
                | ',' | '，'
                | ';' | '；'
                | ':' | '：'
                | '"' | '\''
                | '“' | '”'
                | '、'
                | '(' | ')'
                | '（' | '）'
        )
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordPairResult {
    Pending { hits: u32 },
    Promoted { after: String },
    AlreadyPromoted,
    Ignored,
}

#[derive(Clone, Debug, Default)]
pub struct LearnPairTable {
    by_key: HashMap<String, LearnPairRow>,
}

#[derive(Clone, Debug)]
struct LearnPairRow {
    hits: u32,
    promoted: bool,
}

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

pub fn record_pair(table: &mut LearnPairTable, before: &str, after: &str) -> RecordPairResult {
    if after == before {
        return RecordPairResult::Ignored;
    }
    let key = pair_key(before, after);
    let row = table.by_key.entry(key).or_insert(LearnPairRow {
        hits: 0,
        promoted: false,
    });
    if row.promoted {
        return RecordPairResult::AlreadyPromoted;
    }
    row.hits = row.hits.saturating_add(1);
    if row.hits >= PROMOTE_HITS {
        row.hits = PROMOTE_HITS;
        row.promoted = true;
        return RecordPairResult::Promoted {
            after: after.to_string(),
        };
    }
    RecordPairResult::Pending { hits: row.hits }
}

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
    if after == before {
        return Ok(RecordPairResult::Ignored);
    }
    let key = pair_key(before, after);
    if let Some(existing) = store::get_learn_pair(dir, &key)? {
        if existing.ignored || existing.tombstoned_at.is_some() {
            return Ok(RecordPairResult::Ignored);
        }
        if existing.promoted {
            return Ok(RecordPairResult::AlreadyPromoted);
        }
        if existing.hits >= PROMOTE_HITS {
            return try_promote_learn_pair(dir, dictionary, after, &key);
        }
    }
    let Some(row) = store::upsert_learn_pair_with_scope(dir, &key, before, after, scope)? else {
        return Ok(RecordPairResult::Ignored);
    };
    if row.hits < PROMOTE_HITS {
        return Ok(RecordPairResult::Pending { hits: row.hits });
    }
    try_promote_learn_pair(dir, dictionary, after, &key)
}

fn try_promote_learn_pair(
    dir: &std::path::Path,
    dictionary: &mut Vec<String>,
    after: &str,
    key: &str,
) -> anyhow::Result<RecordPairResult> {
    if dictionary.iter().any(|word| word == after) {
        store::mark_learn_pair_promoted(dir, key)?;
        return Ok(RecordPairResult::AlreadyPromoted);
    }
    if !append_dictionary_entry(dictionary, after) {
        return Ok(RecordPairResult::Pending { hits: PROMOTE_HITS });
    }
    store::mark_learn_pair_promoted(dir, key)?;
    Ok(RecordPairResult::Promoted {
        after: after.to_string(),
    })
}

pub(crate) fn add_dictionary_words(dictionary: &mut Vec<String>, words: &[String]) -> usize {
    words
        .iter()
        .filter(|word| append_dictionary_entry(dictionary, word))
        .count()
}

pub(crate) fn remove_dictionary_entry(dictionary: &mut Vec<String>, word: &str) -> bool {
    let before = dictionary.len();
    dictionary.retain(|item| item != word);
    dictionary.len() != before
}

pub(crate) fn append_dictionary_entry(dictionary: &mut Vec<String>, word: &str) -> bool {
    let word = word.trim();
    if word.is_empty() || word.len() > 256 {
        return false;
    }
    if dictionary.iter().any(|existing| existing == word) {
        return false;
    }
    if dictionary.len() >= 256 {
        return false;
    }
    dictionary.push(word.to_string());
    true
}

pub(crate) fn maybe_observe_after_paste(
    app: &tauri::AppHandle,
    state: &AppState,
    post_insert_field: Option<&str>,
    verified: bool,
    target_guard: &context::TargetAppGuard,
    recording_context: Option<&context::ContextSnapshot>,
) {
    if !verified || target_guard.secure_input {
        return;
    }
    let Some(baseline) = post_insert_field.filter(|value| !value.is_empty()) else {
        return;
    };
    let settings = lock_recover(&state.settings);
    if !settings.dictionary_learn_enabled {
        return;
    }
    if let Some(snapshot) = recording_context {
        if !crate::lexicon::mapping_allows_learn(&settings.context_mappings, &snapshot.profile.id) {
            return;
        }
    }
    let scope = recording_context.map(scope_from_snapshot);
    let mapping_id = recording_context
        .and_then(|snapshot| style_draft_mapping_id(snapshot, &settings.context_mappings));
    let baseline = baseline.to_owned();
    let expected = target_guard.clone();
    let app = app.clone();
    tokio::task::spawn_blocking(move || {
        match observe_after_paste(
            &baseline,
            &expected,
            true,
            ObserveLimits::default(),
            context::focused_input_value,
            context::probe_focus_guard,
            std::thread::sleep,
        ) {
            ObserveOutcome::SingleToken { before_span, after } => {
                persist_learn_pair(&app, &before_span, &after, scope);
            }
            ObserveOutcome::StyleSignal {
                excerpt,
                style_key,
                before_excerpt,
                after_excerpt,
            } => {
                if let Some(mapping_id) = mapping_id {
                    persist_style_draft(
                        &app,
                        &mapping_id,
                        &style_key,
                        &excerpt,
                        &before_excerpt,
                        &after_excerpt,
                    );
                }
            }
            _ => {}
        }
    });
}

fn scope_from_snapshot(snapshot: &context::ContextSnapshot) -> store::LearnPairScope {
    store::LearnPairScope {
        family: Some(context::family_id(snapshot.profile.family).to_owned()),
        mapping_id: snapshot
            .profile
            .id
            .strip_prefix("user.")
            .map(str::to_owned),
        browser_host: snapshot.target_guard.browser_host.clone(),
        native_bundle: if snapshot.target_guard.browser_host.is_some() {
            None
        } else {
            snapshot.target_guard.bundle_id.clone()
        },
    }
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
            style_example_input: None,
            style_example_output: None,
            enabled: true,
            cleanup_effort: None,
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
            style_example_input: None,
            style_example_output: None,
            enabled: true,
            cleanup_effort: None,
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

fn persist_learn_pair(
    app: &tauri::AppHandle,
    before: &str,
    after: &str,
    scope: Option<store::LearnPairScope>,
) {
    let app = app.clone();
    let before = before.to_string();
    let after = after.to_string();
    tauri::async_runtime::spawn(async move {
        persist_learn_pair_locked(&app, &before, &after, scope.as_ref()).await;
    });
}

fn persist_style_draft(
    app: &tauri::AppHandle,
    mapping_id: &str,
    style_key: &str,
    excerpt: &str,
    before_excerpt: &str,
    after_excerpt: &str,
) {
    let app = app.clone();
    let mapping_id = mapping_id.to_owned();
    let style_key = style_key.to_owned();
    let excerpt = excerpt.to_owned();
    let before_excerpt = before_excerpt.to_owned();
    let after_excerpt = after_excerpt.to_owned();
    tauri::async_runtime::spawn(async move {
        let Ok(dir) = app.path().app_data_dir() else {
            return;
        };
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
            let _ = app.emit("style_drafts://changed", ());
        }
    });
}

async fn persist_learn_pair_locked(
    app: &tauri::AppHandle,
    before: &str,
    after: &str,
    scope: Option<&store::LearnPairScope>,
) {
    let state = app.state::<AppState>();
    let _gate = state.settings_gate.lock().await;
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    let mut snapshot = lock_recover(&state.settings).clone();
    if !snapshot.dictionary_learn_enabled {
        return;
    }
    let result = match record_learn_pair_with_scope(
        &dir,
        &mut snapshot.dictionary,
        before,
        after,
        scope,
    ) {
        Ok(result) => result,
        Err(error) => {
            log::warn!("dictionary learn pair failed: {error}");
            return;
        }
    };
    match result {
        RecordPairResult::Promoted { .. } => {
            snapshot.normalize();
            if let Err(error) = store::save_settings(&dir, &snapshot) {
                log::warn!("dictionary learn persist failed: {error}");
                return;
            }
            *lock_recover(&state.settings) = snapshot.clone();
            let _ = app.emit("settings://changed", store::SettingsView::from(&snapshot));
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

fn extract_tokens(text: &str) -> Vec<Token> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let current = chars[index];
        if is_latin_start(current) {
            let start = index;
            index += 1;
            while index < chars.len() && is_latin_cont(chars[index]) {
                index += 1;
            }
            if index - start >= 2 {
                let surface: String = chars[start..index].iter().collect();
                let key = surface.to_lowercase();
                tokens.push(Token { surface, key });
            }
        } else if is_cjk(current) {
            let start = index;
            while index < chars.len() && is_cjk(chars[index]) {
                index += 1;
            }
            let len = index - start;
            if (2..=8).contains(&len) {
                let surface: String = chars[start..index].iter().collect();
                let key = surface.clone();
                tokens.push(Token { surface, key });
            }
        } else {
            index += 1;
        }
    }
    tokens
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
        add_dictionary_words, append_dictionary_entry, observe_after_paste, ObserveLimits,
        ObserveOutcome, remove_dictionary_entry, single_token_candidates,
    };
    use crate::context::TargetAppGuard;
    use std::cell::RefCell;

    fn test_target(pid: i32, secure_input: bool) -> TargetAppGuard {
        TargetAppGuard {
            pid,
            bundle_id: Some("com.example.app".into()),
            browser_host: None,
            browser_target_token: None,
            window_token: Some(1),
            window_id: Some(1),
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
        assert_eq!(
            dictionary,
            vec!["保留".to_string(), "也保留".to_string()]
        );
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
    fn three_new_tokens_are_returned() {
        assert_eq!(
            single_token_candidates("xx", "Foo Bar Qux"),
            vec!["Foo".to_string(), "Bar".to_string(), "Qux".to_string()]
        );
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
    fn observe_commits_a_finished_word_when_target_changes() {
        let expected = test_target(42, false);
        let changed = test_target(99, false);
        let learned = observe_with(
            "知呼",
            &expected,
            true,
            vec![Some("知乎".into())],
            vec![changed],
        );
        assert_eq!(learned, zhihu_token());
    }

    #[test]
    fn observe_commits_a_finished_word_when_focus_changes() {
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
        assert_eq!(learned, zhihu_token());
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
        assert_eq!(outcome, zhihu_token());
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
        assert_eq!(
            table.hits(&super::pair_key("配森", "Python")),
            Some(1)
        );
        assert_eq!(
            table.hits(&super::pair_key("配森", "pytorch")),
            Some(1)
        );
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
    fn record_learn_pair_promotes_on_third_observe() {
        let dir = temp_dir("rlp-three");
        let mut dictionary = Vec::new();
        assert_eq!(
            super::record_learn_pair(&dir, &mut dictionary, "知呼", "知乎").unwrap(),
            super::RecordPairResult::Pending { hits: 1 }
        );
        assert!(dictionary.is_empty());
        assert_eq!(
            super::record_learn_pair(&dir, &mut dictionary, "知呼", "知乎").unwrap(),
            super::RecordPairResult::Pending { hits: 2 }
        );
        assert_eq!(
            super::record_learn_pair(&dir, &mut dictionary, "知呼", "知乎").unwrap(),
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
        super::record_learn_pair(&dir, &mut dictionary, "知呼", "知乎").unwrap();
        super::record_learn_pair(&dir, &mut dictionary, "知呼", "知乎").unwrap();
        super::record_learn_pair(&dir, &mut dictionary, "知呼", "知乎").unwrap();
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
        super::record_learn_pair(&dir, &mut dictionary, "知呼", "知乎").unwrap();
        super::record_learn_pair(&dir, &mut dictionary, "知呼", "知乎").unwrap();
        assert_eq!(
            super::record_learn_pair(&dir, &mut dictionary, "知呼", "知乎").unwrap(),
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
    fn observe_learns_a_short_latin_phrase() {
        let expected = test_target(42, false);
        let learned = observe_with(
            "type script",
            &expected,
            true,
            vec![Some("TypeScript".into())],
            vec![expected.clone()],
        );
        assert_eq!(
            learned,
            ObserveOutcome::SingleToken {
                before_span: "type script".into(),
                after: "TypeScript".into(),
            }
        );
    }

    #[test]
    fn observe_emits_style_signal_for_punctuation_only() {
        let expected = test_target(42, false);
        let learned = observe_with(
            "你好。今天去吗。",
            &expected,
            true,
            vec![Some("你好 今天去吗？".into())],
            vec![expected.clone()],
        );
        match learned {
            ObserveOutcome::StyleSignal { style_key, excerpt, .. } => {
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
    fn observe_commits_last_settled_word_when_send_clears_the_field() {
        let expected = test_target(42, false);
        let left = test_target(99, false);
        let learned = observe_with(
            "知呼",
            &expected,
            true,
            vec![Some("知乎".into()), Some(String::new())],
            vec![expected.clone(), left],
        );
        assert_eq!(learned, zhihu_token());
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
}
