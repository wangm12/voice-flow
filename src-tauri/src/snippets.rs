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
    let phrase = normalize_phrase(utterance);
    if phrase.is_empty() {
        return None;
    }
    snippets
        .iter()
        .find(|snippet| snippet.enabled && normalize_phrase(&snippet.trigger) == phrase)
        .map(|snippet| snippet.expansion.clone())
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
}
