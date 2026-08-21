use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_SNIPPETS: usize = 128;
pub const MAX_TRIGGER_CHARS: usize = 128;
pub const MAX_EXPANSION_CHARS: usize = 16_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippet {
    pub id: String,
    pub trigger: String,
    pub expansion: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

/// Normalize only what is needed for an exact spoken phrase match. This keeps
/// punctuation and non-Latin words intact while making repeated whitespace and
/// casing differences harmless.
pub fn normalize_phrase(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn normalize_snippets(snippets: &mut Vec<Snippet>) {
    let mut seen = HashSet::new();
    snippets.retain_mut(|snippet| {
        snippet.id = snippet.id.trim().chars().take(128).collect();
        snippet.trigger = snippet
            .trigger
            .trim()
            .chars()
            .take(MAX_TRIGGER_CHARS)
            .collect();
        snippet.expansion = snippet
            .expansion
            .trim()
            .chars()
            .take(MAX_EXPANSION_CHARS)
            .collect();
        if snippet.id.is_empty() || snippet.trigger.is_empty() || snippet.expansion.is_empty() {
            return false;
        }
        let key = normalize_phrase(&snippet.trigger);
        seen.insert(key)
    });
    snippets.truncate(MAX_SNIPPETS);
}

pub fn validate_snippets(snippets: &[Snippet]) -> Result<(), String> {
    if snippets.len() > MAX_SNIPPETS {
        return Err("snippet list cannot contain more than 128 items".into());
    }
    let mut ids = HashSet::new();
    let mut triggers = HashSet::new();
    for snippet in snippets {
        if snippet.id.trim().is_empty()
            || snippet.id.chars().count() > 128
            || !ids.insert(&snippet.id)
        {
            return Err("snippet ids must be unique and non-empty".into());
        }
        if snippet.trigger.trim().is_empty()
            || snippet.trigger.chars().count() > MAX_TRIGGER_CHARS
            || !triggers.insert(normalize_phrase(&snippet.trigger))
        {
            return Err("snippet triggers must be unique and non-empty".into());
        }
        if snippet.expansion.trim().is_empty()
            || snippet.expansion.chars().count() > MAX_EXPANSION_CHARS
        {
            return Err("snippet expansions must contain between 1 and 16000 characters".into());
        }
    }
    Ok(())
}

/// Snippets only fire when the entire normalized utterance matches. A phrase
/// inside a longer dictation never expands.
pub fn resolve_exact(snippets: &[Snippet], utterance: &str) -> Option<String> {
    resolve_exact_with_clipboard(snippets, utterance, None)
}

pub fn resolve_exact_with_clipboard(
    snippets: &[Snippet],
    utterance: &str,
    clipboard: Option<&str>,
) -> Option<String> {
    let phrase = normalize_phrase(utterance);
    if phrase.is_empty() {
        return None;
    }
    snippets
        .iter()
        .find(|snippet| snippet.enabled && normalize_phrase(&snippet.trigger) == phrase)
        .map(|snippet| apply_placeholders(&snippet.expansion, clipboard))
}

/// True when an exact snippet match would substitute `{{clipboard}}`.
pub fn exact_match_needs_clipboard(snippets: &[Snippet], utterance: &str) -> bool {
    let phrase = normalize_phrase(utterance);
    if phrase.is_empty() {
        return false;
    }
    snippets.iter().any(|snippet| {
        snippet.enabled
            && normalize_phrase(&snippet.trigger) == phrase
            && snippet.expansion.contains("{{clipboard}}")
    })
}

pub fn read_clipboard_if_needed(
    snippets: &[Snippet],
    utterance: &str,
    read: impl FnOnce() -> Option<String>,
) -> Option<String> {
    if exact_match_needs_clipboard(snippets, utterance) {
        read()
    } else {
        None
    }
}

fn apply_placeholders(expansion: &str, clipboard: Option<&str>) -> String {
    let mut out = expansion.replace("{{date}}", &local_iso_date());
    if out.contains("{{clipboard}}") {
        if let Some(text) = clipboard {
            out = out.replace("{{clipboard}}", text);
        }
    }
    out
}

fn local_iso_date() -> String {
    #[cfg(unix)]
    {
        // SAFETY: `t` is a `time_t` from libc::time. `tm` is zeroed and only
        // read after localtime_r writes a calendar time into it.
        unsafe {
            let t = libc::time(std::ptr::null_mut());
            if t == -1 {
                return String::new();
            }
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&t, &mut tm).is_null() {
                return String::new();
            }
            format!(
                "{:04}-{:02}-{:02}",
                tm.tm_year + 1900,
                tm.tm_mon + 1,
                tm.tm_mday
            )
        }
    }
    #[cfg(not(unix))]
    {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippet(trigger: &str, expansion: &str) -> Snippet {
        Snippet {
            id: trigger.into(),
            trigger: trigger.into(),
            expansion: expansion.into(),
            enabled: true,
        }
    }

    #[test]
    fn only_full_phrase_matches() {
        let snippets = vec![snippet("insert email", "user@example.com")];
        assert_eq!(
            resolve_exact(&snippets, "Insert   Email"),
            Some("user@example.com".into())
        );
        assert_eq!(resolve_exact(&snippets, "please insert email now"), None);
    }

    #[test]
    fn disabled_and_duplicate_triggers_are_safe() {
        let mut snippets = vec![
            snippet("one", "1"),
            snippet(" ONE ", "duplicate"),
            Snippet {
                id: "two".into(),
                trigger: "two".into(),
                expansion: "2".into(),
                enabled: false,
            },
        ];
        normalize_snippets(&mut snippets);
        assert_eq!(snippets.len(), 2);
        assert_eq!(resolve_exact(&snippets, "two"), None);
        assert!(validate_snippets(&snippets).is_ok());
    }

    #[test]
    fn expands_date_placeholder() {
        let snippets = vec![snippet("today", "on {{date}}")];
        let got = resolve_exact(&snippets, "today").expect("snippet should match");
        let date = got.strip_prefix("on ").expect("keeps surrounding expansion");
        assert_eq!(date.len(), 10, "{got}");
        let parts: Vec<_> = date.split('-').collect();
        assert_eq!(parts.len(), 3, "{got}");
        assert_eq!(parts[0].len(), 4);
        assert_eq!(parts[1].len(), 2);
        assert_eq!(parts[2].len(), 2);
        assert!(parts.iter().all(|part| part.chars().all(|ch| ch.is_ascii_digit())));
        assert!(!got.contains("{{date}}"));
    }

    #[test]
    fn expands_clipboard_placeholder_when_provided() {
        let snippets = vec![snippet("clip", "see {{clipboard}}")];
        assert_eq!(
            resolve_exact_with_clipboard(&snippets, "clip", Some("hello")),
            Some("see hello".into())
        );
    }

    #[test]
    fn leaves_clipboard_placeholder_when_read_fails() {
        let snippets = vec![snippet("clip", "see {{clipboard}}")];
        assert_eq!(
            resolve_exact_with_clipboard(&snippets, "clip", None),
            Some("see {{clipboard}}".into())
        );
    }

    #[test]
    fn reads_clipboard_only_when_the_matched_expansion_needs_it() {
        let with_placeholder = vec![snippet("clip", "see {{clipboard}}")];
        let without_placeholder = vec![snippet("mail", "user@example.com")];
        assert!(exact_match_needs_clipboard(&with_placeholder, "clip"));
        assert!(!exact_match_needs_clipboard(&without_placeholder, "mail"));
        let mut read = false;
        let ignored = read_clipboard_if_needed(&without_placeholder, "mail", || {
            read = true;
            Some("secret".into())
        });
        assert!(!read);
        assert!(ignored.is_none());
        let clipboard = read_clipboard_if_needed(&with_placeholder, "clip", || Some("hello".into()));
        assert_eq!(clipboard.as_deref(), Some("hello"));
    }
}
