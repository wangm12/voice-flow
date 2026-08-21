//! Single-token dictionary candidates from a before/after edit.
//!
//! History uses this for confirm-to-add suggestions. After a verified paste,
//! `observe_after_paste` watches only the same focused field for a short
//! window and silently appends an unambiguous single-token correction.
//! Never install a global key event tap.

use std::collections::HashSet;
use std::time::Duration;

use crate::{context, lock_recover, store, AppState};
use tauri::{Manager, State};

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

#[tauri::command]
pub(crate) fn suggest_dictionary_entries(
    state: State<'_, AppState>,
    before: String,
    after: String,
) -> Vec<String> {
    if !lock_recover(&state.settings).dictionary_learn_enabled {
        return Vec::new();
    }
    single_token_candidates(&before, &after)
}

const OBSERVE_WINDOW: Duration = Duration::from_millis(3000);
const OBSERVE_INTERVAL: Duration = Duration::from_millis(400);

pub fn observe_after_paste(
    post_insert_field: &str,
    expected_target: &context::TargetAppGuard,
    dictionary_learn_enabled: bool,
    window: Duration,
    interval: Duration,
    mut read_value: impl FnMut() -> Option<String>,
    mut read_target: impl FnMut() -> context::TargetAppGuard,
    mut sleep: impl FnMut(Duration),
) -> Option<String> {
    if !dictionary_learn_enabled || expected_target.secure_input || post_insert_field.is_empty() {
        return None;
    }
    if interval.is_zero() {
        return None;
    }

    let mut elapsed = Duration::ZERO;
    while elapsed < window {
        sleep(interval);
        elapsed = elapsed.saturating_add(interval);

        let current_target = read_target();
        if context::target_mismatch_reason(expected_target, &current_target).is_some() {
            return None;
        }
        let Some(current) = read_value() else {
            continue;
        };
        if current == post_insert_field {
            continue;
        }
        let mut candidates = single_token_candidates(post_insert_field, &current);
        if candidates.len() == 1 {
            return candidates.pop();
        }
        return None;
    }
    None
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
) {
    if !verified || target_guard.secure_input {
        return;
    }
    let Some(baseline) = post_insert_field.filter(|value| !value.is_empty()) else {
        return;
    };
    if !lock_recover(&state.settings).dictionary_learn_enabled {
        return;
    }
    let baseline = baseline.to_owned();
    let expected = target_guard.clone();
    let app = app.clone();
    let (mappings, browser_access_enabled) = {
        let current = lock_recover(&state.context);
        (current.mappings.clone(), current.browser_access_enabled)
    };
    tokio::task::spawn_blocking(move || {
        let Some(word) = observe_after_paste(
            &baseline,
            &expected,
            true,
            OBSERVE_WINDOW,
            OBSERVE_INTERVAL,
            context::focused_input_value,
            || context::detect_snapshot(&mappings, browser_access_enabled).target_guard,
            std::thread::sleep,
        ) else {
            return;
        };
        persist_learned_word(&app, &word);
    });
}

fn persist_learned_word(app: &tauri::AppHandle, word: &str) {
    let app = app.clone();
    let word = word.to_string();
    tauri::async_runtime::spawn(async move {
        persist_learned_word_locked(&app, &word).await;
    });
}

async fn persist_learned_word_locked(app: &tauri::AppHandle, word: &str) {
    let state = app.state::<AppState>();
    let _gate = state.settings_gate.lock().await;
    let snapshot = {
        let mut settings = lock_recover(&state.settings);
        if !settings.dictionary_learn_enabled {
            return;
        }
        if !append_dictionary_entry(&mut settings.dictionary, word) {
            return;
        }
        settings.normalize();
        settings.clone()
    };
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    if let Err(error) = store::save_settings(&dir, &snapshot) {
        log::warn!("dictionary learn persist failed: {error}");
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

fn is_latin_start(value: char) -> bool {
    value.is_ascii_alphabetic()
}

fn is_latin_cont(value: char) -> bool {
    value.is_ascii_alphanumeric() || matches!(value, '.' | '_' | '-')
}

fn is_cjk(value: char) -> bool {
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
        append_dictionary_entry, observe_after_paste, single_token_candidates,
    };
    use crate::context::TargetAppGuard;
    use std::cell::RefCell;
    use std::time::Duration;

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

    fn observe_with(
        post_insert_field: &str,
        expected: &TargetAppGuard,
        enabled: bool,
        values: Vec<Option<String>>,
        targets: Vec<TargetAppGuard>,
        polls: u32,
    ) -> Option<String> {
        let values = RefCell::new(values.into_iter());
        let targets = RefCell::new(targets.into_iter());
        let interval = Duration::from_millis(400);
        observe_after_paste(
            post_insert_field,
            expected,
            enabled,
            interval * polls,
            interval,
            || {
                values
                    .borrow_mut()
                    .next()
                    .unwrap_or_else(|| Some(post_insert_field.to_string()))
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
    fn observe_learns_zhihu_correction() {
        let expected = test_target(42, false);
        let learned = observe_with(
            "知呼",
            &expected,
            true,
            vec![Some("知呼".into()), Some("知乎".into())],
            vec![expected.clone(), expected.clone()],
            2,
        );
        assert_eq!(learned.as_deref(), Some("知乎"));
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
            1,
        );
        assert_eq!(learned.as_deref(), Some("知乎"));
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
            1,
        );
        assert!(learned.is_none());
    }

    #[test]
    fn observe_stops_when_target_changes() {
        let expected = test_target(42, false);
        let changed = test_target(99, false);
        let learned = observe_with(
            "知呼",
            &expected,
            true,
            vec![Some("知乎".into())],
            vec![changed],
            1,
        );
        assert!(learned.is_none());
    }

    #[test]
    fn observe_stops_when_focus_changes() {
        let expected = test_target(42, false);
        let mut changed = expected.clone();
        changed.input_token = Some(99);
        let learned = observe_with(
            "知呼",
            &expected,
            true,
            vec![Some("知乎".into())],
            vec![changed],
            1,
        );
        assert!(learned.is_none());
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
            1,
        );
        assert!(learned.is_none());
    }

    #[test]
    fn observe_skips_secure_input() {
        let expected = test_target(42, true);
        let learned = observe_after_paste(
            "知呼",
            &expected,
            true,
            Duration::from_millis(400),
            Duration::from_millis(400),
            || panic!("must not read focused value for secure_input"),
            || panic!("must not probe target for secure_input"),
            |_| panic!("must not poll for secure_input"),
        );
        assert!(learned.is_none());
    }

    #[test]
    fn observe_skips_when_learning_disabled() {
        let expected = test_target(42, false);
        let learned = observe_after_paste(
            "知呼",
            &expected,
            false,
            Duration::from_millis(400),
            Duration::from_millis(400),
            || panic!("must not read focused value when learning is off"),
            || panic!("must not probe target when learning is off"),
            |_| panic!("must not poll when learning is off"),
        );
        assert!(learned.is_none());
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
}
