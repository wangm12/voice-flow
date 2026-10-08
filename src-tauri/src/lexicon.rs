//! Local lexicon replace, ASR ranking, cleanup hit-pairs, and cleanup effort.

use crate::context::{AppMapping, ContextFamily, ContextPolicy, ContextSnapshot};
use crate::dictionary_learn::{is_cjk, is_latin_cont, is_latin_start, pair_key};
use crate::llm::{CleanupEffort, CleanupIntensity, CleanupIntent, IntentSource};
use crate::screen_text::ScreenTextContext;
use crate::store::LearnPairRecord;
use std::collections::HashSet;

pub const MAX_ASR_PROMPT_TOKENS: usize = 200;
const ASR_NO_TRANSLATE: &str = "不要翻译。";
const ASR_EMPTY_SEED: &str = "不要翻译。这个 API 的 latency 太高了。";
const GPT_TRANSCRIBE_SCENE: &str = "今天下午在看文档。中英混合听写";
const MAX_CLEANUP_PAIRS: usize = 8;
const MAX_CLEANUP_PAIR_CHARS: usize = 400;
const MAX_CONTEXT_TERMS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsrPromptShape {
    WhisperTranscript,
    ContextTerms,
    GptTranscribeKeywords,
    MistralContextBias,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AsrPromptBundle {
    pub prompt: Option<String>,
    pub keywords: Vec<String>,
}

pub fn asr_prompt_shape_for(
    provider: crate::providers::EngineProvider,
    model: &str,
) -> AsrPromptShape {
    let model = model.trim().to_ascii_lowercase();
    if model == "gpt-transcribe" || model.starts_with("gpt-transcribe-") {
        return AsrPromptShape::GptTranscribeKeywords;
    }
    if model.contains("qwen")
        || model.contains("sensevoice")
        || model.contains("fun-asr")
        || model.contains("funasr")
        || model.contains("paraformer")
    {
        return AsrPromptShape::ContextTerms;
    }
    if provider == crate::providers::EngineProvider::Mistral {
        return AsrPromptShape::MistralContextBias;
    }
    match provider {
        crate::providers::EngineProvider::AssemblyAi => AsrPromptShape::GptTranscribeKeywords,
        crate::providers::EngineProvider::OnDevice => AsrPromptShape::ContextTerms,
        crate::providers::EngineProvider::Deepgram => AsrPromptShape::ContextTerms,
        crate::providers::EngineProvider::SiliconFlow if !model.contains("whisper") => {
            AsrPromptShape::ContextTerms
        }
        _ => AsrPromptShape::WhisperTranscript,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexiconPair {
    pub before: String,
    pub after: String,
}

impl LexiconPair {
    pub fn new(before: impl Into<String>, after: impl Into<String>) -> Self {
        Self {
            before: before.into(),
            after: after.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptScope {
    pub family: Option<String>,
    pub mapping_id: Option<String>,
    pub browser_host: Option<String>,
}

impl PromptScope {
    pub fn from_snapshot(snapshot: &ContextSnapshot) -> Self {
        Self::from_history(
            Some(snapshot.profile.id.as_str()),
            Some(crate::context::family_id(snapshot.profile.family)),
            snapshot.target_guard.browser_host.as_deref(),
        )
    }

    pub fn from_history(
        profile_id: Option<&str>,
        family: Option<&str>,
        browser_host: Option<&str>,
    ) -> Self {
        let inferred_family = family.map(str::to_owned).or_else(|| {
            profile_id
                .map(|id| crate::context::family_id(family_from_profile_id(id, &[])).to_owned())
        });
        Self {
            family: inferred_family,
            mapping_id: profile_id
                .and_then(|id| id.strip_prefix("user."))
                .map(str::to_owned),
            browser_host: browser_host
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
        }
    }
}

pub fn family_from_profile_id(profile_id: &str, mappings: &[AppMapping]) -> ContextFamily {
    if let Some(id) = profile_id.strip_prefix("user.") {
        return mappings
            .iter()
            .find(|mapping| mapping.id == id)
            .map(|mapping| mapping.family)
            .unwrap_or(ContextFamily::General);
    }
    if let Some(id) = profile_id.strip_prefix("manual.") {
        return crate::context::builtin_family_for_id(id).unwrap_or(ContextFamily::General);
    }
    match profile_id {
        "chat.personal" => ContextFamily::PersonalChat,
        "chat.team" | "chat.focused" | "chat.native" | "chat.slack" | "chat.teams" => {
            ContextFamily::WorkChat
        }
        id if id.starts_with("email.") => ContextFamily::Email,
        id if id.starts_with("code.") || id == "developer.web" => ContextFamily::PromptOrCode,
        id if id.starts_with("terminal.") => ContextFamily::Terminal,
        "browser.search" => ContextFamily::BrowserSearch,
        id if id.starts_with("document.") => ContextFamily::Document,
        id if id.starts_with("form.") => ContextFamily::FormFilling,
        id if id.starts_with("calendar.")
            || id.starts_with("task.")
            || id.starts_with("reminders.") =>
        {
            ContextFamily::CalendarTask
        }
        id if id.starts_with("project.") => ContextFamily::ProjectManagement,
        id if id.starts_with("social.") => ContextFamily::SocialMedia,
        id if id.starts_with("support.") => ContextFamily::CustomerSupport,
        _ => crate::context::builtin_family_for_id(profile_id).unwrap_or(ContextFamily::General),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupRoute {
    LocalOnly,
    Provider(CleanupEffort),
}

pub fn decide_cleanup(
    settings_cleanup_enabled: bool,
    global_intensity: CleanupIntensity,
    mapping: Option<&AppMapping>,
    family: ContextFamily,
    intent: &CleanupIntent,
) -> CleanupRoute {
    if !settings_cleanup_enabled {
        return CleanupRoute::LocalOnly;
    }
    if mapping.is_some_and(|item| !item.cleanup_enabled) {
        return CleanupRoute::LocalOnly;
    }
    if matches!(
        intent.source,
        IntentSource::SpokenCommand | IntentSource::SelectedText
    ) {
        return CleanupRoute::Provider(CleanupEffort::Command);
    }
    if intent.content.trim().is_empty() {
        return CleanupRoute::LocalOnly;
    }
    if skips_llm_scene(family) {
        return CleanupRoute::LocalOnly;
    }
    let resolved = mapping
        .and_then(|item| item.cleanup_intensity)
        .or_else(|| {
            mapping
                .and_then(|item| item.cleanup_effort)
                .and_then(CleanupEffort::as_intensity)
        })
        .unwrap_or(global_intensity);
    match resolved.as_effort() {
        None => CleanupRoute::LocalOnly,
        Some(effort) => CleanupRoute::Provider(effort),
    }
}

fn skips_llm_scene(family: ContextFamily) -> bool {
    matches!(family, ContextFamily::FormFilling | ContextFamily::Terminal)
}

pub fn mapping_for_profile<'a>(
    mappings: &'a [AppMapping],
    profile_id: &str,
) -> Option<&'a AppMapping> {
    profile_id
        .strip_prefix("user.")
        .and_then(|id| mappings.iter().find(|mapping| mapping.id == id))
}

#[cfg(test)]
pub fn mapping_allows_learn(mappings: &[AppMapping], profile_id: &str) -> bool {
    scene_allows_learn(mappings, profile_id, None, None)
}

pub fn scene_allows_learn(
    mappings: &[AppMapping],
    profile_id: &str,
    bundle_id: Option<&str>,
    browser_host: Option<&str>,
) -> bool {
    if let Some(mapping) = mapping_for_profile(mappings, profile_id) {
        return mapping.enabled && mapping.dictionary_learn_enabled;
    }
    if let Some(mapping) = mapping_matching_target(mappings, bundle_id, browser_host) {
        return mapping.dictionary_learn_enabled;
    }
    !is_default_learn_off_target(bundle_id, browser_host)
}

fn mapping_matching_target<'a>(
    mappings: &'a [AppMapping],
    bundle_id: Option<&str>,
    browser_host: Option<&str>,
) -> Option<&'a AppMapping> {
    mappings.iter().find(|mapping| {
        mapping.enabled
            && (bundle_id.is_some() && mapping.bundle_id.as_deref() == bundle_id
                || host_is_or_under(
                    browser_host
                        .and_then(crate::context::normalize_host)
                        .as_deref(),
                    mapping
                        .browser_host
                        .as_deref()
                        .and_then(crate::context::normalize_host)
                        .as_deref(),
                ))
    })
}

pub(crate) fn is_default_learn_off_target(
    bundle_id: Option<&str>,
    browser_host: Option<&str>,
) -> bool {
    if bundle_id.is_some_and(is_learn_off_bundle) {
        return true;
    }
    browser_host
        .and_then(crate::context::normalize_host)
        .is_some_and(|host| is_learn_off_host(&host))
}

fn is_learn_off_bundle(bundle_id: &str) -> bool {
    matches!(
        bundle_id,
        "com.1password.1password"
            | "com.1password.1password-launcher"
            | "com.agilebits.onepassword7"
            | "com.agilebits.onepassword-osx"
            | "com.lastpass.LastPass"
            | "com.bitwarden.desktop"
            | "com.apple.Passwords"
            | "com.dashlane.dashlanephonefinal"
            | "com.callpod.android_apps.keeper"
    )
}

fn is_learn_off_host(host: &str) -> bool {
    const EXACT: &[&str] = &[
        "accounts.google.com",
        "login.microsoftonline.com",
        "login.live.com",
    ];
    const SUFFIX: &[&str] = &[
        "1password.com",
        "lastpass.com",
        "bitwarden.com",
        "dashlane.com",
        "keepersecurity.com",
        "workday.com",
        "myworkday.com",
        "okta.com",
        "auth0.com",
        "onelogin.com",
        "rippling.com",
        "gusto.com",
        "bamboohr.com",
        "greenhouse.io",
        "lever.co",
        "adp.com",
        "paylocity.com",
        "ukg.com",
        "ultipro.com",
        "successfactors.com",
        "paycom.com",
        "namely.com",
        "justworks.com",
        "chase.com",
        "bankofamerica.com",
        "wellsfargo.com",
        "usbank.com",
        "capitalone.com",
        "citi.com",
        "citibank.com",
        "schwab.com",
        "fidelity.com",
        "vanguard.com",
        "paypal.com",
        "venmo.com",
    ];
    EXACT.contains(&host)
        || SUFFIX
            .iter()
            .any(|parent| host == *parent || host.ends_with(&format!(".{parent}")))
}

fn host_is_or_under(actual: Option<&str>, expected: Option<&str>) -> bool {
    match (actual, expected) {
        (Some(host), Some(parent)) => host == parent || host.ends_with(&format!(".{parent}")),
        _ => false,
    }
}

pub fn apply_lexicon_replacements(
    text: &str,
    pairs: &[LexiconPair],
    blocking: &[String],
) -> String {
    apply_lexicon_replacements_with_usage(text, pairs, blocking).0
}

fn apply_lexicon_replacements_with_usage(
    text: &str,
    pairs: &[LexiconPair],
    blocking: &[String],
) -> (String, Vec<String>) {
    let mut rules: Vec<&LexiconPair> = pairs
        .iter()
        .filter(|pair| !pair.before.is_empty() && pair.before != pair.after)
        .collect();
    rules.sort_by(|left, right| {
        right
            .before
            .chars()
            .count()
            .cmp(&left.before.chars().count())
            .then_with(|| left.before.cmp(&right.before))
    });
    let mut blockers: HashSet<String> = blocking.iter().cloned().collect();
    for pair in pairs {
        if !pair.before.is_empty() {
            blockers.insert(pair.before.clone());
        }
        if !pair.after.is_empty() {
            blockers.insert(pair.after.clone());
        }
    }

    let chars: Vec<char> = text.chars().collect();
    let mut output = String::new();
    let mut applied_terms = std::collections::BTreeSet::new();
    let mut index = 0;
    while index < chars.len() {
        let mut matched: Option<&LexiconPair> = None;
        for rule in &rules {
            let needle: Vec<char> = rule.before.chars().collect();
            if !region_matches(&chars, index, &needle) {
                continue;
            }
            if is_latin_surface(&rule.before) {
                if !latin_boundaries_ok(&chars, index, needle.len()) {
                    continue;
                }
            } else if is_cjk_surface(&rule.before)
                && cjk_embedded_in_blocker(&chars, index, needle.len(), &blockers)
            {
                continue;
            }
            matched = Some(*rule);
            break;
        }
        if let Some(rule) = matched {
            output.push_str(&rule.after);
            applied_terms.insert(rule.after.clone());
            index += rule.before.chars().count();
        } else {
            output.push(chars[index]);
            index += 1;
        }
    }
    (output, applied_terms.into_iter().collect())
}

pub fn replaceable_pairs(pairs: &[LearnPairRecord], dictionary: &[String]) -> Vec<LexiconPair> {
    let dict: HashSet<&str> = dictionary.iter().map(String::as_str).collect();
    pairs
        .iter()
        .filter(|row| row.is_live_promoted() && dict.contains(row.after_surface.as_str()))
        .filter(|row| !row.before_surface.is_empty())
        .map(|row| LexiconPair::new(&row.before_surface, &row.after_surface))
        .collect()
}

#[cfg(test)]
pub fn apply_promoted_replacements(
    text: &str,
    pairs: &[LearnPairRecord],
    dictionary: &[String],
) -> String {
    apply_promoted_replacements_with_usage(text, pairs, dictionary).0
}

pub fn apply_promoted_replacements_with_usage(
    text: &str,
    pairs: &[LearnPairRecord],
    dictionary: &[String],
) -> (String, Vec<String>) {
    let replaceable = replaceable_pairs(pairs, dictionary);
    let mut rules = replaceable.clone();
    rules.extend(phonetic_replace_rules(text, &replaceable));
    apply_lexicon_replacements_with_usage(text, &rules, dictionary)
}

/// Explicitly enabled fuzzy matching cannot cross protected text or layout boundaries.
pub fn apply_ascii_fuzzy_dictionary(text: &str, dictionary: &[String]) -> String {
    let mut protected: Vec<(usize, usize)> = crate::protected_span::protected_spans(text, None)
        .into_iter()
        .map(|span| (span.start_byte, span.end_byte))
        .collect();
    // Inline/fenced code and unfinished code delimiters are never fuzzy prose.
    let mut cursor = 0;
    while let Some(offset) = text[cursor..].find('`') {
        let start = cursor + offset;
        let count = text[start..]
            .bytes()
            .take_while(|byte| *byte == b'`')
            .count();
        let content = start + count;
        let end = text[content..]
            .find(&text[start..content])
            .map(|offset| content + offset + count)
            .unwrap_or(text.len());
        protected.push((start, end));
        cursor = end;
    }
    protected.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in protected {
        if let Some(last) = merged.last_mut().filter(|last| start <= last.1) {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    let mut result = String::with_capacity(text.len());
    let mut cursor = 0;
    let append_plain = |result: &mut String, plain: &str| {
        for line in plain.split_inclusive([
            '\n', '\r', '\t', '\u{b}', '\u{c}', '\u{85}', '\u{2028}', '\u{2029}',
        ]) {
            result.push_str(&apply_ascii_fuzzy_dictionary_unprotected(
                line, dictionary, 0.18,
            ));
        }
    };
    for (start, end) in merged {
        append_plain(&mut result, &text[cursor..start]);
        result.push_str(&text[start..end]);
        cursor = end;
    }
    append_plain(&mut result, &text[cursor..]);
    result
}

fn apply_ascii_fuzzy_dictionary_unprotected(
    text: &str,
    dictionary: &[String],
    threshold: f64,
) -> String {
    if dictionary.is_empty() {
        return text.to_string();
    }

    let dictionary_match_keys: Vec<DictionaryMatchKey> = dictionary
        .iter()
        .enumerate()
        .flat_map(|(index, word)| build_dictionary_match_keys(word, index))
        .collect();

    let (leading, tokens) = split_tokens_preserving_separators(text);
    if tokens.is_empty() {
        return text.to_string();
    }

    let words: Vec<&str> = tokens.iter().map(|(token, _)| *token).collect();
    let mut result = String::new();
    result.push_str(leading);
    let mut index = 0;

    while index < words.len() {
        let mut best_match: Option<(usize, &String, f64)> = None;

        for n in (1..=3).rev() {
            if index + n > words.len() {
                continue;
            }

            let ngram_words = &words[index..index + n];
            if ngram_words[..n.saturating_sub(1)]
                .iter()
                .any(|word| !extract_fuzzy_punctuation(word).1.is_empty())
            {
                continue;
            }
            let ngram = build_fuzzy_ngram(ngram_words);

            if let Some((replacement, score)) =
                find_fuzzy_dictionary_match(&ngram, dictionary, &dictionary_match_keys, threshold)
            {
                let is_better = best_match
                    .as_ref()
                    .is_none_or(|(_, _, best_score)| score < *best_score);
                if is_better {
                    best_match = Some((n, replacement, score));
                }
            }
        }

        if let Some((n, replacement, _)) = best_match {
            let ngram_words = &words[index..index + n];
            let (prefix, _) = extract_fuzzy_punctuation(ngram_words[0]);
            let (_, suffix) = extract_fuzzy_punctuation(ngram_words[n - 1]);
            let corrected = preserve_fuzzy_case_pattern(ngram_words[0], replacement);
            result.push_str(&format!("{}{}{}", prefix, corrected, suffix));
            result.push_str(tokens[index + n - 1].1);
            index += n;
        } else {
            result.push_str(words[index]);
            result.push_str(tokens[index].1);
            index += 1;
        }
    }

    result
}

/// Split `text` into a leading whitespace slice and `(token, following_ws)` pairs.
/// Following slices are the original separators (spaces, tabs, newlines).
fn split_tokens_preserving_separators(text: &str) -> (&str, Vec<(&str, &str)>) {
    let leading_end = text
        .char_indices()
        .find(|(_, ch)| !ch.is_whitespace())
        .map(|(idx, _)| idx)
        .unwrap_or(text.len());
    let leading = &text[..leading_end];

    let mut tokens = Vec::new();
    let mut rest = &text[leading_end..];
    while !rest.is_empty() {
        let token_len = rest
            .char_indices()
            .find(|(_, ch)| ch.is_whitespace())
            .map(|(idx, _)| idx)
            .unwrap_or(rest.len());
        let token = &rest[..token_len];
        rest = &rest[token_len..];
        let sep_len = rest
            .char_indices()
            .find(|(_, ch)| !ch.is_whitespace())
            .map(|(idx, _)| idx)
            .unwrap_or(rest.len());
        let sep = &rest[..sep_len];
        rest = &rest[sep_len..];
        tokens.push((token, sep));
    }

    (leading, tokens)
}

struct DictionaryMatchKey {
    word_index: usize,
    key: String,
}

fn build_fuzzy_ngram(words: &[&str]) -> String {
    words
        .iter()
        .map(|word| build_fuzzy_match_key(word))
        .collect::<Vec<_>>()
        .concat()
}

fn build_fuzzy_match_key(word: &str) -> String {
    word.chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn build_dictionary_match_keys(word: &str, word_index: usize) -> Vec<DictionaryMatchKey> {
    let primary_key = build_fuzzy_match_key(word);
    let mut keys = Vec::with_capacity(2);

    if is_supported_fuzzy_key(&primary_key) {
        keys.push(DictionaryMatchKey {
            word_index,
            key: primary_key.clone(),
        });
    }

    if word.contains('&') {
        let expanded_key = build_fuzzy_match_key(&word.replace('&', " and "));
        if is_supported_fuzzy_key(&expanded_key) && expanded_key != primary_key {
            keys.push(DictionaryMatchKey {
                word_index,
                key: expanded_key,
            });
        }
    }

    keys
}

fn is_supported_fuzzy_key(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|ch| ch.is_ascii_alphanumeric())
}

fn supports_soundex(key: &str) -> bool {
    key.chars().filter(|ch| ch.is_ascii_alphabetic()).count() >= 6
        && key.chars().all(|ch| ch.is_ascii_alphabetic())
}

fn find_fuzzy_dictionary_match<'a>(
    candidate: &str,
    dictionary: &'a [String],
    dictionary_match_keys: &[DictionaryMatchKey],
    threshold: f64,
) -> Option<(&'a String, f64)> {
    if !is_supported_fuzzy_key(candidate) || candidate.chars().count() > 50 {
        return None;
    }

    let mut best_match: Option<&String> = None;
    let mut best_score = f64::MAX;

    for dictionary_key in dictionary_match_keys {
        let candidate_len = candidate.chars().count();
        let dictionary_len = dictionary_key.key.chars().count();
        let len_diff = candidate_len.abs_diff(dictionary_len) as f64;
        let max_len = candidate_len.max(dictionary_len) as f64;
        let max_allowed_diff = (max_len * 0.25).max(2.0);
        if len_diff > max_allowed_diff {
            continue;
        }

        let edit_distance = levenshtein(candidate, &dictionary_key.key);
        let levenshtein_score = if max_len > 0.0 {
            edit_distance as f64 / max_len
        } else {
            1.0
        };

        let phonetic_match = supports_soundex(candidate)
            && supports_soundex(&dictionary_key.key)
            && soundex_match(candidate, &dictionary_key.key);

        let combined_score = if phonetic_match {
            levenshtein_score * 0.3
        } else {
            levenshtein_score
        };

        if combined_score < threshold && combined_score < best_score {
            best_match = Some(&dictionary[dictionary_key.word_index]);
            best_score = combined_score;
        }
    }

    best_match.map(|matched| (matched, best_score))
}

fn preserve_fuzzy_case_pattern(original: &str, replacement: &str) -> String {
    if original.chars().all(|ch| ch.is_uppercase()) {
        replacement.to_uppercase()
    } else if original.chars().next().is_some_and(|ch| ch.is_uppercase()) {
        let mut chars: Vec<char> = replacement.chars().collect();
        if let Some(first_char) = chars.first_mut() {
            *first_char = first_char.to_uppercase().next().unwrap_or(*first_char);
        }
        chars.into_iter().collect()
    } else {
        replacement.to_string()
    }
}

fn extract_fuzzy_punctuation(word: &str) -> (&str, &str) {
    let prefix_end = word
        .char_indices()
        .find(|(_, ch)| ch.is_alphanumeric())
        .map(|(index, _)| index)
        .unwrap_or(word.len());
    let suffix_start = word
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_alphanumeric())
        .map(|(index, ch)| index + ch.len_utf8())
        .unwrap_or(0);

    let prefix = if prefix_end > 0 {
        &word[..prefix_end]
    } else {
        ""
    };
    let suffix = if suffix_start < word.len() {
        &word[suffix_start..]
    } else {
        ""
    };

    (prefix, suffix)
}

fn soundex_match(left: &str, right: &str) -> bool {
    soundex_code(left) == soundex_code(right)
}

fn soundex_code(input: &str) -> String {
    let mut chars = input.chars().filter(|ch| ch.is_ascii_alphabetic());
    let Some(first) = chars.next() else {
        return String::new();
    };

    let mut out = String::new();
    out.push(first.to_ascii_uppercase());
    let mut previous = Some(soundex_digit(first));

    for ch in chars {
        let digit = soundex_digit(ch);
        if digit == b'0' {
            continue;
        }
        if Some(digit) != previous {
            out.push(digit as char);
            previous = Some(digit);
        }
    }

    while out.len() < 4 {
        out.push('0');
    }
    out.truncate(4);
    out
}

fn soundex_digit(ch: char) -> u8 {
    match ch.to_ascii_uppercase() {
        'B' | 'F' | 'P' | 'V' => b'1',
        'C' | 'G' | 'J' | 'K' | 'Q' | 'S' | 'X' | 'Z' => b'2',
        'D' | 'T' => b'3',
        'L' => b'4',
        'M' | 'N' => b'5',
        'R' => b'6',
        _ => b'0',
    }
}

pub fn hit_pairs(text: &str, pairs: &[LexiconPair], blocking: &[String]) -> Vec<LexiconPair> {
    let mut hits = Vec::new();
    let mut chars = 0usize;
    for pair in pairs {
        if pair.before.is_empty() {
            continue;
        }
        if !independent_surface(text, &pair.before, pairs, blocking)
            && !independent_surface(text, &pair.after, pairs, blocking)
            && !phonetic_surface_hit(text, pair, pairs)
            && !latin_fuzzy_hit(text, pair)
        {
            continue;
        }
        let rendered = format!("{}→{}", pair.before, pair.after);
        let rendered_chars = rendered.chars().count();
        let separator = usize::from(!hits.is_empty()) * 2;
        if hits.len() >= MAX_CLEANUP_PAIRS
            || chars
                .saturating_add(separator)
                .saturating_add(rendered_chars)
                > MAX_CLEANUP_PAIR_CHARS
        {
            break;
        }
        chars = chars
            .saturating_add(separator)
            .saturating_add(rendered_chars);
        hits.push(pair.clone());
    }
    hits
}

pub fn format_cleanup_pairs(pairs: &[LexiconPair]) -> Option<String> {
    if pairs.is_empty() {
        return None;
    }
    Some(
        pairs
            .iter()
            .map(|pair| format!("{}→{}", pair.before, pair.after))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

pub fn estimate_prompt_tokens(text: &str) -> usize {
    let mut tokens = 0.0_f64;
    let mut in_latin_word = false;
    for ch in text.chars() {
        if is_cjk(ch) {
            if in_latin_word {
                tokens += 1.0;
                in_latin_word = false;
            }
            tokens += 1.3;
        } else if ch.is_whitespace() || matches!(ch, ',' | ':' | ';' | '/') {
            if in_latin_word {
                tokens += 1.0;
                in_latin_word = false;
            }
        } else {
            in_latin_word = true;
        }
    }
    if in_latin_word {
        tokens += 1.0;
    }
    tokens.ceil() as usize
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedTerm {
    pub term: String,
    pub pinned: bool,
    pub scope_score: u8,
    pub last_used_at: Option<String>,
    pub hits: u32,
}

pub fn collect_ranked_terms(
    dictionary: &[String],
    pairs: &[LearnPairRecord],
    scope: Option<&PromptScope>,
) -> Vec<RankedTerm> {
    let mut seen = HashSet::new();
    let mut terms = Vec::new();
    let scope = scope.cloned().unwrap_or_default();

    for row in pairs.iter().filter(|row| row.is_live_promoted()) {
        if !dictionary.iter().any(|word| word == &row.after_surface) {
            continue;
        }
        if !seen.insert(row.after_surface.clone()) {
            continue;
        }
        terms.push(RankedTerm {
            term: row.after_surface.clone(),
            pinned: row.pinned,
            scope_score: scope_score(row, &scope),
            last_used_at: row.last_used_at.clone(),
            hits: row.hits,
        });
    }

    for word in dictionary
        .iter()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
    {
        if !seen.insert(word.to_owned()) {
            continue;
        }
        let row = pairs.iter().find(|item| {
            item.after_surface == word && item.before_surface.is_empty() && item.is_live_promoted()
        });
        terms.push(RankedTerm {
            term: word.to_owned(),
            pinned: row.map(|item| item.pinned).unwrap_or(false),
            scope_score: 0,
            last_used_at: row.and_then(|item| item.last_used_at.clone()),
            hits: row.map(|item| item.hits).unwrap_or(0),
        });
    }

    terms.sort_by(|left, right| {
        right
            .pinned
            .cmp(&left.pinned)
            .then(right.scope_score.cmp(&left.scope_score))
            .then(right.last_used_at.cmp(&left.last_used_at))
            .then(right.hits.cmp(&left.hits))
            .then(left.term.cmp(&right.term))
    });
    terms
}

pub fn keywords_for_asr(terms: &[RankedTerm], max: usize) -> Vec<String> {
    let mut keywords = Vec::new();
    for term in terms {
        if keywords.len() >= max {
            break;
        }
        let raw = term.term.as_str();
        if raw.contains('<') || raw.contains('>') || raw.contains('\n') || raw.contains('\r') {
            continue;
        }
        let item = raw.trim();
        if item.is_empty() || item.contains("不要翻译") {
            continue;
        }
        keywords.push(item.to_owned());
    }
    keywords
}

#[cfg(test)]
pub fn build_asr_prompt(
    dictionary: &[String],
    policy: Option<&ContextPolicy>,
    pairs: &[LearnPairRecord],
    scope: Option<&PromptScope>,
) -> Option<String> {
    build_asr_prompt_bundle(
        dictionary,
        policy,
        pairs,
        scope,
        AsrPromptShape::WhisperTranscript,
        None,
    )
    .prompt
}

#[cfg(test)]
pub fn build_asr_prompt_shaped(
    dictionary: &[String],
    policy: Option<&ContextPolicy>,
    pairs: &[LearnPairRecord],
    scope: Option<&PromptScope>,
    shape: AsrPromptShape,
    screen: Option<&ScreenTextContext>,
) -> Option<String> {
    build_asr_prompt_bundle(dictionary, policy, pairs, scope, shape, screen).prompt
}

pub fn build_asr_prompt_bundle(
    dictionary: &[String],
    _policy: Option<&ContextPolicy>,
    pairs: &[LearnPairRecord],
    scope: Option<&PromptScope>,
    shape: AsrPromptShape,
    screen: Option<&ScreenTextContext>,
) -> AsrPromptBundle {
    build_asr_prompt_bundle_with_permissions(
        dictionary,
        _policy,
        pairs,
        scope,
        shape,
        screen,
        crate::context::ContextSourcePermissions {
            ax_text: true,
            local_ocr: true,
            cloud_vision: true,
            context_text_to_providers: true,
        },
    )
}

pub fn build_asr_prompt_bundle_with_permissions(
    dictionary: &[String],
    _policy: Option<&ContextPolicy>,
    pairs: &[LearnPairRecord],
    scope: Option<&PromptScope>,
    shape: AsrPromptShape,
    screen: Option<&ScreenTextContext>,
    screen_permissions: crate::context::ContextSourcePermissions,
) -> AsrPromptBundle {
    let ranked = collect_ranked_terms(dictionary, pairs, scope);
    if matches!(
        shape,
        AsrPromptShape::GptTranscribeKeywords | AsrPromptShape::MistralContextBias
    ) {
        let mut keywords = keywords_for_asr(&ranked, MAX_CONTEXT_TERMS);
        if let Some(screen) = screen {
            for term in screen.asr_terms(screen_permissions) {
                if keywords.len() >= MAX_CONTEXT_TERMS {
                    break;
                }
                if !keywords.contains(&term)
                    && !term.contains(['<', '>'])
                    && !term.contains(['\n', '\r'])
                    && !term.trim().is_empty()
                {
                    keywords.push(term);
                }
            }
        }
        return AsrPromptBundle {
            prompt: (shape == AsrPromptShape::GptTranscribeKeywords)
                .then(|| GPT_TRANSCRIBE_SCENE.to_owned()),
            keywords,
        };
    }
    AsrPromptBundle {
        prompt: weave_shaped_prompt(ranked, shape, screen, screen_permissions),
        keywords: Vec::new(),
    }
}

fn weave_shaped_prompt(
    ranked: Vec<RankedTerm>,
    shape: AsrPromptShape,
    screen: Option<&ScreenTextContext>,
    screen_permissions: crate::context::ContextSourcePermissions,
) -> Option<String> {
    let glue_tokens = match shape {
        AsrPromptShape::WhisperTranscript => estimate_prompt_tokens("今天下午在看文档。不要翻译。"),
        AsrPromptShape::ContextTerms
        | AsrPromptShape::GptTranscribeKeywords
        | AsrPromptShape::MistralContextBias => 0,
    };
    let budget = MAX_ASR_PROMPT_TOKENS.saturating_sub(glue_tokens);
    if budget == 0 {
        return match shape {
            AsrPromptShape::WhisperTranscript => Some(ASR_EMPTY_SEED.to_owned()),
            AsrPromptShape::ContextTerms
            | AsrPromptShape::GptTranscribeKeywords
            | AsrPromptShape::MistralContextBias => None,
        };
    }

    let mut screen_kept = Vec::new();
    let mut screen_used = 0usize;
    if let Some(screen) = screen {
        for token in screen.asr_terms(screen_permissions) {
            let cost = estimate_prompt_tokens(&token) + usize::from(!screen_kept.is_empty()) * 2;
            if screen_used.saturating_add(cost) > budget {
                break;
            }
            screen_used = screen_used.saturating_add(cost);
            screen_kept.push(token);
        }
    }
    let budget = budget.saturating_sub(screen_used);

    let scene_budget = (budget * 7) / 10;
    let global_budget = budget.saturating_sub(scene_budget);
    let scene: Vec<RankedTerm> = ranked
        .iter()
        .filter(|term| term.pinned || term.scope_score > 0)
        .cloned()
        .collect();
    let global: Vec<RankedTerm> = ranked
        .iter()
        .filter(|term| !term.pinned && term.scope_score == 0)
        .cloned()
        .collect();

    let mut scene_kept = pack_highest_first(&scene, scene_budget);
    let mut global_kept = pack_highest_first(&global, global_budget);
    let leftover = budget
        .saturating_sub(terms_token_cost(&scene_kept))
        .saturating_sub(terms_token_cost(&global_kept));
    if leftover > 0 {
        let extra_global = pack_highest_first(
            &global
                .into_iter()
                .filter(|term| !global_kept.iter().any(|kept| kept.term == term.term))
                .collect::<Vec<_>>(),
            leftover,
        );
        global_kept.extend(extra_global);
    }

    let mut kept = global_kept;
    kept.append(&mut scene_kept);
    kept.sort_by(|left, right| {
        left.pinned
            .cmp(&right.pinned)
            .then(left.scope_score.cmp(&right.scope_score))
            .then(left.last_used_at.cmp(&right.last_used_at))
            .then(left.hits.cmp(&right.hits))
            .then(left.term.cmp(&right.term))
    });

    let mut hints: Vec<String> = screen_kept;
    for term in kept {
        if !hints.iter().any(|existing| existing == &term.term) {
            hints.push(term.term);
        }
    }
    fit_woven_prompt(hints, shape)
}

fn fit_woven_prompt(mut terms: Vec<String>, shape: AsrPromptShape) -> Option<String> {
    match shape {
        AsrPromptShape::WhisperTranscript => {
            while !terms.is_empty() {
                let woven = weave_whisper_transcript(&terms);
                if estimate_prompt_tokens(&woven) <= MAX_ASR_PROMPT_TOKENS {
                    return Some(woven);
                }
                terms.remove(0);
            }
            Some(ASR_EMPTY_SEED.to_owned())
        }
        AsrPromptShape::ContextTerms => {
            if terms.len() > MAX_CONTEXT_TERMS {
                let start = terms.len() - MAX_CONTEXT_TERMS;
                terms = terms.split_off(start);
            }
            while !terms.is_empty() {
                let woven = terms.join(" ");
                if estimate_prompt_tokens(&woven) <= MAX_ASR_PROMPT_TOKENS {
                    return Some(woven);
                }
                terms.remove(0);
            }
            None
        }
        AsrPromptShape::MistralContextBias => None,
        AsrPromptShape::GptTranscribeKeywords => Some(GPT_TRANSCRIBE_SCENE.to_owned()),
    }
}

fn weave_whisper_transcript(terms: &[String]) -> String {
    if terms.is_empty() {
        return ASR_EMPTY_SEED.to_owned();
    }
    let body = match terms {
        [one] => format!("{one}今天下午在开会。"),
        [one, two] => format!("{one}今天下午在{two}看文档。"),
        _ => {
            let extras = &terms[..terms.len() - 3];
            let subject = &terms[terms.len() - 3];
            let place = &terms[terms.len() - 2];
            let object = &terms[terms.len() - 1];
            let prefix = if extras.is_empty() {
                String::new()
            } else {
                format!("{}。", extras.join("、"))
            };
            format!("{prefix}{subject}今天下午在{place}看{object}。")
        }
    };
    format!("{body}{ASR_NO_TRANSLATE}")
}

pub fn used_pair_keys(text: &str, pairs: &[LearnPairRecord]) -> Vec<String> {
    pairs
        .iter()
        .filter(|row| row.is_live_promoted())
        .filter(|row| {
            (!row.before_surface.is_empty() && text.contains(&row.before_surface))
                || text.contains(&row.after_surface)
        })
        .map(|row| row.pair_key.clone())
        .collect()
}

fn independent_surface(
    text: &str,
    needle: &str,
    pairs: &[LexiconPair],
    blocking: &[String],
) -> bool {
    if needle.is_empty() || !text.contains(needle) {
        return false;
    }
    let probe = [LexiconPair::new(needle, "\u{FFFC}")];
    let mut blockers = blocking.to_vec();
    for pair in pairs {
        if !pair.before.is_empty() {
            blockers.push(pair.before.clone());
        }
        if !pair.after.is_empty() {
            blockers.push(pair.after.clone());
        }
    }
    apply_lexicon_replacements(text, &probe, &blockers) != text
}

fn pack_highest_first(terms: &[RankedTerm], budget: usize) -> Vec<RankedTerm> {
    let mut kept = Vec::new();
    let mut used = 0usize;
    for term in terms {
        let cost = estimate_prompt_tokens(&term.term) + usize::from(!kept.is_empty()) * 2;
        if used.saturating_add(cost) > budget {
            continue;
        }
        used = used.saturating_add(cost);
        kept.push(term.clone());
    }
    kept
}

fn terms_token_cost(terms: &[RankedTerm]) -> usize {
    if terms.is_empty() {
        return 0;
    }
    let joined = terms
        .iter()
        .map(|term| term.term.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    estimate_prompt_tokens(&joined)
}

fn scope_score(row: &LearnPairRecord, scope: &PromptScope) -> u8 {
    if scope.mapping_id.is_some() && row.mapping_id == scope.mapping_id {
        return 3;
    }
    if scope.browser_host.is_some() && row.browser_host == scope.browser_host {
        return 2;
    }
    if scope.family.is_some() && row.family == scope.family {
        return 1;
    }
    0
}

fn region_matches(haystack: &[char], start: usize, needle: &[char]) -> bool {
    if start + needle.len() > haystack.len() || needle.is_empty() {
        return false;
    }
    haystack[start..start + needle.len()]
        .iter()
        .zip(needle)
        .all(|(left, right)| {
            if is_latin_start(*left)
                || is_latin_cont(*left)
                || is_latin_start(*right)
                || is_latin_cont(*right)
            {
                left.eq_ignore_ascii_case(right)
            } else {
                left == right
            }
        })
}

fn latin_boundaries_ok(chars: &[char], start: usize, len: usize) -> bool {
    let before_ok = start == 0 || !is_latin_token_char(chars[start - 1]);
    let after_index = start + len;
    let after_ok = after_index >= chars.len() || !is_latin_token_char(chars[after_index]);
    before_ok && after_ok
}

fn is_latin_token_char(value: char) -> bool {
    is_latin_start(value) || is_latin_cont(value)
}

fn is_latin_surface(value: &str) -> bool {
    value
        .chars()
        .any(|ch| is_latin_start(ch) || is_latin_cont(ch))
        && !value.chars().any(is_cjk)
}

fn is_cjk_surface(value: &str) -> bool {
    value.chars().any(is_cjk)
}

fn phonetic_replace_rules(text: &str, pairs: &[LexiconPair]) -> Vec<LexiconPair> {
    let mut extra = Vec::new();
    let mut seen = HashSet::new();
    for pair in pairs {
        let window_len = pair.after.chars().count();
        if !(2..=8).contains(&window_len) || !is_cjk_surface(&pair.after) {
            continue;
        }
        let Some(after_py) = plain_pinyin(&pair.after) else {
            continue;
        };
        let before_py = plain_pinyin(&pair.before);
        for window in cjk_windows(text, window_len) {
            if window == pair.after || window == pair.before {
                continue;
            }
            let Some(window_py) = plain_pinyin(&window) else {
                continue;
            };
            if window_py != after_py && before_py.as_deref() != Some(window_py.as_str()) {
                continue;
            }
            if afters_for_pinyin(pairs, &window_py).len() != 1 {
                continue;
            }
            if !seen.insert(window.clone()) {
                continue;
            }
            extra.push(LexiconPair::new(window, pair.after.clone()));
        }
    }
    extra
}

fn phonetic_surface_hit(text: &str, pair: &LexiconPair, pairs: &[LexiconPair]) -> bool {
    let window_len = pair.after.chars().count();
    if !(2..=8).contains(&window_len) || !is_cjk_surface(&pair.after) {
        return false;
    }
    let Some(after_py) = plain_pinyin(&pair.after) else {
        return false;
    };
    let before_py = plain_pinyin(&pair.before);
    cjk_windows(text, window_len).any(|window| {
        if window == pair.after || window == pair.before {
            return false;
        }
        let Some(window_py) = plain_pinyin(&window) else {
            return false;
        };
        (window_py == after_py || before_py.as_deref() == Some(window_py.as_str()))
            && afters_for_pinyin(pairs, &window_py).contains(pair.after.as_str())
    })
}

fn latin_fuzzy_hit(text: &str, pair: &LexiconPair) -> bool {
    let target = if is_latin_surface(&pair.after) {
        pair.after.as_str()
    } else if is_latin_surface(&pair.before) {
        pair.before.as_str()
    } else {
        return false;
    };
    let target_key = target.to_ascii_lowercase();
    latin_words(text).any(|word| {
        let chars = word.chars().count();
        if chars < 3 {
            return false;
        }
        let key = word.to_ascii_lowercase();
        key != target_key && levenshtein(&key, &target_key) <= 2
    })
}

fn afters_for_pinyin<'a>(pairs: &'a [LexiconPair], pinyin: &str) -> HashSet<&'a str> {
    pairs
        .iter()
        .filter(|pair| {
            plain_pinyin(&pair.after).as_deref() == Some(pinyin)
                || plain_pinyin(&pair.before).as_deref() == Some(pinyin)
        })
        .map(|pair| pair.after.as_str())
        .collect()
}

fn cjk_windows(text: &str, len: usize) -> impl Iterator<Item = String> + '_ {
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    std::iter::from_fn(move || {
        while index + len <= chars.len() {
            let start = index;
            index += 1;
            if chars[start..start + len].iter().all(|ch| is_cjk(*ch)) {
                return Some(chars[start..start + len].iter().collect());
            }
        }
        None
    })
}

fn latin_words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|ch: char| !(is_latin_start(ch) || is_latin_cont(ch)))
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
}

fn plain_pinyin(text: &str) -> Option<String> {
    use pinyin::ToPinyin;
    let mut out = String::new();
    for ch in text.chars() {
        if let Some(py) = ch.to_pinyin() {
            out.push_str(py.plain());
        } else if is_cjk(ch) {
            return None;
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn levenshtein(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.is_empty() {
        return right.len();
    }
    if right.is_empty() {
        return left.len();
    }
    let mut prev: Vec<usize> = (0..=right.len()).collect();
    let mut curr = vec![0; right.len() + 1];
    for (i, left_ch) in left.iter().enumerate() {
        curr[0] = i + 1;
        for (j, right_ch) in right.iter().enumerate() {
            let cost = usize::from(left_ch != right_ch);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[right.len()]
}

fn cjk_embedded_in_blocker(
    chars: &[char],
    start: usize,
    len: usize,
    blockers: &HashSet<String>,
) -> bool {
    let end = start + len;
    for blocker in blockers {
        let needle: Vec<char> = blocker.chars().collect();
        if needle.len() <= len {
            continue;
        }
        let earliest = start.saturating_sub(needle.len().saturating_sub(1));
        for term_start in earliest..=start {
            let term_end = term_start + needle.len();
            if term_end > chars.len() || term_end < end {
                continue;
            }
            if chars[term_start..term_end] == needle[..] {
                return true;
            }
        }
    }
    false
}

#[allow(dead_code)]
pub fn pair_key_for(before: &str, after: &str) -> String {
    pair_key(before, after)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ContextFamily;
    use crate::dictionary_learn::pair_key;
    use crate::llm::{CleanupIntensity, CleanupIntent};

    fn decide(
        enabled: bool,
        mapping: Option<&AppMapping>,
        family: ContextFamily,
        intent: &CleanupIntent,
    ) -> CleanupRoute {
        decide_cleanup(enabled, CleanupIntensity::Heavy, mapping, family, intent)
    }

    fn sample_mapping() -> AppMapping {
        AppMapping {
            id: "wechat".into(),
            label: "微信".into(),
            family: ContextFamily::PersonalChat,
            mode_id: None,
            bundle_id: None,
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

    fn live_pair(before: &str, after: &str, family: &str, hits: u32) -> LearnPairRecord {
        LearnPairRecord {
            pair_key: pair_key(before, after),
            before_surface: before.into(),
            after_surface: after.into(),
            hits,
            promoted: true,
            last_at: "2026-01-01".into(),
            family: Some(family.into()),
            mapping_id: None,
            browser_host: None,
            native_bundle: None,
            last_used_at: None,
            pinned: false,
            tombstoned_at: None,
            ignored: false,
            promote_hits: 0,
        }
    }

    fn ranked(term: &str) -> RankedTerm {
        RankedTerm {
            term: term.into(),
            pinned: false,
            scope_score: 0,
            last_used_at: None,
            hits: 0,
        }
    }

    #[test]
    fn keywords_for_asr_strips_forbidden_and_caps() {
        let mut terms = vec![
            ranked("晓雯"),
            ranked("不要翻译"),
            ranked("foo<bar>"),
            ranked("bad>item"),
            ranked("line\nbreak"),
            ranked("知乎"),
        ];
        for index in 0..80 {
            terms.push(ranked(&format!("词{index}")));
        }
        let keywords = keywords_for_asr(&terms, 64);
        assert!(!keywords.iter().any(|item| item.contains("foo<bar>")));
        assert!(!keywords.iter().any(|item| item.contains('<')));
        assert!(!keywords.iter().any(|item| item.contains('>')));
        assert!(!keywords.iter().any(|item| item.contains('\n')));
        assert!(!keywords.iter().any(|item| item.contains("不要翻译")));
        assert_eq!(keywords.first().map(String::as_str), Some("晓雯"));
        assert!(keywords.contains(&"知乎".to_owned()));
        assert_eq!(keywords.len(), 64);
    }

    #[test]
    fn keywords_for_asr_discards_original_term_with_newline_or_cr() {
        let keywords = keywords_for_asr(
            &[
                ranked("foo\n"),
                ranked("bar\r"),
                ranked("  baz\n  "),
                ranked("ok"),
            ],
            64,
        );
        assert_eq!(keywords, vec!["ok"]);
        assert!(!keywords.iter().any(|item| item == "foo"));
        assert!(!keywords.iter().any(|item| item == "bar"));
        assert!(!keywords.iter().any(|item| item == "baz"));
    }

    #[test]
    fn keywords_for_asr_max_zero_returns_empty() {
        assert!(keywords_for_asr(&[ranked("晓雯")], 0).is_empty());
    }

    #[test]
    fn replaces_cjk_span_inside_a_sentence() {
        let pairs = [LexiconPair::new("知呼", "知乎")];
        assert_eq!(
            apply_lexicon_replacements("今天去知呼看看吧", &pairs, &[]),
            "今天去知乎看看吧"
        );
    }

    #[test]
    fn does_not_replace_shorter_term_inside_a_longer_dictionary_word() {
        let pairs = [LexiconPair::new("链接", "link")];
        assert_eq!(
            apply_lexicon_replacements("点击超链接", &pairs, &["超链接".into()]),
            "点击超链接"
        );
    }

    #[test]
    fn applies_multiple_befores_for_one_after() {
        let pairs = [
            LexiconPair::new("配森", "Python"),
            LexiconPair::new("派森", "Python"),
        ];
        assert_eq!(
            apply_lexicon_replacements("学配森和派森", &pairs, &[]),
            "学Python和Python"
        );
    }

    #[test]
    fn replacement_usage_counts_actual_selected_rules_once_per_term() {
        let rules = [
            LexiconPair::new("type", "kind"),
            LexiconPair::new("type script", "TypeScript"),
            LexiconPair::new("配森", "Python"),
        ];
        let (text, used) =
            apply_lexicon_replacements_with_usage("use type script and 配森配森", &rules, &[]);
        assert_eq!(text, "use TypeScript and PythonPython");
        assert_eq!(used, vec!["Python", "TypeScript"]);
        assert!(
            apply_lexicon_replacements_with_usage("TypeScript Python prototype", &rules, &[])
                .1
                .is_empty()
        );
        assert!(apply_lexicon_replacements_with_usage(
            "点击超链接",
            &[LexiconPair::new("链接", "link")],
            &["超链接".into()]
        )
        .1
        .is_empty());
    }

    #[test]
    fn usage_only_includes_live_dictionary_replacements_including_phonetic_rules() {
        let pair = live_pair("知呼", "知乎", "personal_chat", 3);
        let (text, used) = apply_promoted_replacements_with_usage(
            "今天去之乎看看",
            std::slice::from_ref(&pair),
            &["知乎".into()],
        );
        assert_eq!(text, "今天去知乎看看");
        assert_eq!(used, vec!["知乎"]);
        assert!(apply_promoted_replacements_with_usage(
            "知乎",
            std::slice::from_ref(&pair),
            &["知乎".into()]
        )
        .1
        .is_empty());
        assert!(
            apply_promoted_replacements_with_usage("知呼", std::slice::from_ref(&pair), &[])
                .1
                .is_empty()
        );
        let mut ignored = pair;
        ignored.ignored = true;
        assert!(
            apply_promoted_replacements_with_usage("知呼", &[ignored], &["知乎".into()])
                .1
                .is_empty()
        );
    }

    #[test]
    fn prefers_the_longest_before() {
        let pairs = [
            LexiconPair::new("type", "kind"),
            LexiconPair::new("type script", "TypeScript"),
        ];
        assert_eq!(
            apply_lexicon_replacements("use type script today", &pairs, &[]),
            "use TypeScript today"
        );
    }

    #[test]
    fn latin_replace_respects_word_boundaries() {
        let pairs = [LexiconPair::new("script", "shell")];
        assert_eq!(
            apply_lexicon_replacements("TypeScript script", &pairs, &[]),
            "TypeScript shell"
        );
    }

    #[test]
    fn uses_stored_after_casing() {
        let pairs = [LexiconPair::new("python", "Python")];
        assert_eq!(
            apply_lexicon_replacements("install python now", &pairs, &[]),
            "install Python now"
        );
    }

    #[test]
    fn skips_tombstoned_or_unpromoted_pairs() {
        let mut ignored = live_pair("知呼", "知乎", "personal_chat", 3);
        ignored.ignored = true;
        let mut pending = live_pair("配森", "Python", "personal_chat", 2);
        pending.promoted = false;
        let dictionary = vec!["知乎".into(), "Python".into()];
        assert_eq!(
            apply_promoted_replacements("今天去知呼学配森", &[ignored, pending], &dictionary),
            "今天去知呼学配森"
        );
    }

    #[test]
    fn hit_pairs_retrieves_same_pinyin_cjk_that_is_not_the_stored_before() {
        let pairs = vec![LexiconPair::new("知呼", "知乎")];
        assert_eq!(
            hit_pairs("今天去之乎看看吧", &pairs, &[]),
            vec![LexiconPair::new("知呼", "知乎")]
        );
    }

    #[test]
    fn apply_replaces_unique_same_pinyin_cjk_surface() {
        let pair = live_pair("知呼", "知乎", "personal_chat", 3);
        let dictionary = vec!["知乎".into()];
        assert_eq!(
            apply_promoted_replacements("今天去之乎看看", &[pair], &dictionary),
            "今天去知乎看看"
        );
    }

    #[test]
    fn apply_does_not_replace_cjk_with_a_different_pinyin() {
        let pair = live_pair("知呼", "知乎", "personal_chat", 3);
        let dictionary = vec!["知乎".into()];
        assert_eq!(
            apply_promoted_replacements("之后再去", &[pair], &dictionary),
            "之后再去"
        );
    }

    #[test]
    fn apply_does_not_replace_when_two_afters_share_the_same_pinyin() {
        let pairs = [
            live_pair("医士", "医师", "personal_chat", 3),
            live_pair("意示", "意识", "personal_chat", 3),
        ];
        let dictionary = vec!["医师".into(), "意识".into()];
        assert_eq!(
            apply_promoted_replacements("一时再说", &pairs, &dictionary),
            "一时再说"
        );
    }

    #[test]
    fn hit_pairs_retrieves_latin_within_edit_distance_two() {
        let pairs = vec![LexiconPair::new("配森", "Python")];
        assert_eq!(
            hit_pairs("install pyton now", &pairs, &[]),
            vec![LexiconPair::new("配森", "Python")]
        );
    }

    #[test]
    fn apply_does_not_fuzzy_replace_latin() {
        let pair = live_pair("配森", "Python", "personal_chat", 3);
        let dictionary = vec!["Python".into()];
        assert_eq!(
            apply_promoted_replacements("install pyton now", &[pair], &dictionary),
            "install pyton now"
        );
    }

    #[test]
    fn hit_pairs_keep_only_terms_in_this_transcript() {
        let pairs = vec![
            LexiconPair::new("知呼", "知乎"),
            LexiconPair::new("配森", "Python"),
        ];
        assert_eq!(
            hit_pairs("今天去知呼看看吧", &pairs, &[]),
            vec![LexiconPair::new("知呼", "知乎")]
        );
        assert_eq!(
            format_cleanup_pairs(&hit_pairs("今天去知呼看看吧", &pairs, &[])).as_deref(),
            Some("知呼→知乎")
        );
    }

    #[test]
    fn hit_pairs_skip_a_shorter_term_embedded_in_a_dictionary_word() {
        let pairs = [LexiconPair::new("链接", "link")];
        assert!(hit_pairs("点击超链接", &pairs, &["超链接".into()]).is_empty());
    }

    #[test]
    fn asr_prompt_is_end_weighted_and_stays_under_200_tokens() {
        let mut pairs = Vec::new();
        let mut dictionary = Vec::new();
        for index in 0..80 {
            let after = format!("旧词{index}超长家庭词条");
            dictionary.push(after.clone());
            pairs.push(live_pair(&format!("旧{index}"), &after, "work_chat", 1));
        }
        for index in 0..3 {
            let after = format!("微信词{index}");
            dictionary.push(after.clone());
            pairs.push(live_pair(&format!("微{index}"), &after, "personal_chat", 9));
        }
        dictionary.push("手动导入".into());
        let scope = PromptScope {
            family: Some("personal_chat".into()),
            mapping_id: None,
            browser_host: None,
        };
        let prompt = build_asr_prompt(&dictionary, None, &pairs, Some(&scope)).unwrap();
        assert!(estimate_prompt_tokens(&prompt) <= MAX_ASR_PROMPT_TOKENS);
        assert!(prompt.contains("微信词"));
        assert!(prompt.contains("手动导入"));
        let wechat_at = prompt.rfind("微信词").unwrap();
        if let Some(old_at) = prompt.find("旧词") {
            assert!(old_at < wechat_at);
        }
    }

    #[test]
    fn asr_prompt_shape_follows_the_selected_plug() {
        assert_eq!(
            asr_prompt_shape_for(
                crate::providers::EngineProvider::Groq,
                "whisper-large-v3-turbo"
            ),
            AsrPromptShape::WhisperTranscript
        );
        assert_eq!(
            asr_prompt_shape_for(
                crate::providers::EngineProvider::SiliconFlow,
                "FunAudioLLM/SenseVoiceSmall"
            ),
            AsrPromptShape::ContextTerms
        );
        assert_eq!(
            asr_prompt_shape_for(crate::providers::EngineProvider::Custom, "Qwen3-ASR"),
            AsrPromptShape::ContextTerms
        );
        assert_eq!(
            asr_prompt_shape_for(crate::providers::EngineProvider::Deepgram, "nova-3"),
            AsrPromptShape::ContextTerms
        );
        assert_eq!(
            asr_prompt_shape_for(crate::providers::EngineProvider::OpenAi, "gpt-transcribe"),
            AsrPromptShape::GptTranscribeKeywords
        );
        assert_eq!(
            asr_prompt_shape_for(
                crate::providers::EngineProvider::OpenAi,
                "gpt-transcribe-latest"
            ),
            AsrPromptShape::GptTranscribeKeywords
        );
        assert_eq!(
            asr_prompt_shape_for(
                crate::providers::EngineProvider::OpenAi,
                "gpt-4o-mini-transcribe"
            ),
            AsrPromptShape::WhisperTranscript
        );
    }

    #[test]
    fn asr_prompt_shape_on_device_is_context_terms() {
        assert_eq!(
            asr_prompt_shape_for(
                crate::providers::EngineProvider::OnDevice,
                "sensevoice-small"
            ),
            AsrPromptShape::ContextTerms
        );
        assert_eq!(
            asr_prompt_shape_for(crate::providers::EngineProvider::OnDevice, "whisper-1"),
            AsrPromptShape::ContextTerms
        );
    }

    #[test]
    fn asr_prompt_weaves_a_fictional_transcript_not_a_comma_list() {
        let dictionary = vec!["晓雯".into(), "知乎".into(), "TypeScript".into()];
        let pairs = vec![
            live_pair("小文", "晓雯", "work_chat", 1),
            live_pair("知呼", "知乎", "personal_chat", 5),
            live_pair("类型脚本", "TypeScript", "personal_chat", 9),
        ];
        let scope = PromptScope {
            family: Some("personal_chat".into()),
            mapping_id: None,
            browser_host: None,
        };
        let prompt = build_asr_prompt(&dictionary, None, &pairs, Some(&scope)).unwrap();
        assert!(prompt.contains("晓雯"));
        assert!(prompt.contains("知乎"));
        assert!(prompt.contains("TypeScript"));
        assert!(prompt.contains("不要翻译"));
        assert!(
            prompt.contains("今天下午"),
            "Whisper should continue from a spoken seed, got {prompt}"
        );
        assert!(
            !prompt.contains("不要翻译: "),
            "comma inventory is not a Whisper prefix: {prompt}"
        );
        assert!(
            !prompt.contains("晓雯, 知乎"),
            "terms must be woven, not listed: {prompt}"
        );
        let xiaowen = prompt.find("晓雯").unwrap();
        let zhihu = prompt.find("知乎").unwrap();
        let typescript = prompt.find("TypeScript").unwrap();
        assert!(xiaowen < zhihu && zhihu < typescript);
        assert!(prompt.ends_with("不要翻译。"), "{prompt}");
    }

    #[test]
    fn gpt_transcribe_prompt_is_a_short_scene_with_ranked_keywords() {
        let dictionary = vec!["晓雯".into(), "知乎".into(), "TypeScript".into()];
        let pairs = vec![
            live_pair("小文", "晓雯", "work_chat", 1),
            live_pair("知呼", "知乎", "personal_chat", 5),
            live_pair("类型脚本", "TypeScript", "personal_chat", 9),
        ];
        let scope = PromptScope {
            family: Some("personal_chat".into()),
            mapping_id: None,
            browser_host: None,
        };
        let bundle = build_asr_prompt_bundle(
            &dictionary,
            None,
            &pairs,
            Some(&scope),
            AsrPromptShape::GptTranscribeKeywords,
            None,
        );
        let prompt = bundle.prompt.expect("gpt-transcribe keeps a short scene");
        assert!(
            prompt.contains("中英混合听写"),
            "gpt-transcribe scene should mark mixed dictation: {prompt}"
        );
        assert!(
            !prompt.contains("晓雯"),
            "proper nouns belong in keywords, not the scene: {prompt}"
        );
        assert!(
            !prompt.contains("知乎"),
            "proper nouns belong in keywords, not the scene: {prompt}"
        );
        assert!(
            !prompt.contains("晓雯今天下午在知乎看文档"),
            "do not weave a fake Whisper transcript: {prompt}"
        );
        assert!(bundle.keywords.contains(&"晓雯".to_owned()));
        assert!(bundle.keywords.contains(&"知乎".to_owned()));
        assert!(bundle.keywords.contains(&"TypeScript".to_owned()));
    }

    #[test]
    fn gpt_transcribe_keywords_include_only_bounded_granted_screen_terms() {
        let ctx = crate::screen_text::extract_from_fixture(&crate::screen_text::AxWindowFixture {
            family: ContextFamily::PersonalChat,
            focus_kind: crate::context::FocusKind::Chat,
            known_ide: false,
            counterpart: Some("晓雯".into()),
            bubbles: vec!["在吗".into()],
            email_recipients: Vec::new(),
            email_subject: None,
            ide_filenames: Vec::new(),
            ide_symbols: Vec::new(),
            selected_text: None,
            document_name: None,
            focused_role: "AXTextField".into(),
            secure: false,
            banking_preset: false,
            window_title: "晓雯 - 微信".into(),
            raw_url: Some("https://wx.qq.com/chat/secret".into()),
            pid: 4242,
        });
        let bundle = build_asr_prompt_bundle(
            &["TypeScript".into()],
            None,
            &[],
            None,
            AsrPromptShape::GptTranscribeKeywords,
            Some(&ctx),
        );
        assert!(bundle.keywords.contains(&"TypeScript".into()));
        assert!(bundle.keywords.contains(&"晓雯".into()));
        // The legacy test builder has an explicit all-source grant; production
        // requests use the permission-aware builder and a bound session.
        let granted = build_asr_prompt_bundle_with_permissions(
            &["TypeScript".into()],
            None,
            &[],
            None,
            AsrPromptShape::GptTranscribeKeywords,
            Some(&ctx),
            crate::context::ContextSourcePermissions {
                ax_text: true,
                context_text_to_providers: true,
                ..Default::default()
            },
        );
        assert!(granted.keywords.contains(&"晓雯".into()));
        assert!(!granted.keywords.iter().any(|term| term == "在吗"));
        let denied = build_asr_prompt_bundle_with_permissions(
            &["TypeScript".into()],
            None,
            &[],
            None,
            AsrPromptShape::GptTranscribeKeywords,
            Some(&ctx),
            crate::context::ContextSourcePermissions {
                ax_text: true,
                context_text_to_providers: false,
                ..Default::default()
            },
        );
        assert_eq!(denied.keywords, vec!["TypeScript"]);
    }

    #[test]
    fn asr_prompt_for_context_engines_is_a_term_list() {
        let dictionary = vec!["晓雯".into(), "知乎".into(), "TypeScript".into()];
        let pairs = vec![
            live_pair("小文", "晓雯", "work_chat", 1),
            live_pair("知呼", "知乎", "personal_chat", 5),
            live_pair("类型脚本", "TypeScript", "personal_chat", 9),
        ];
        let scope = PromptScope {
            family: Some("personal_chat".into()),
            mapping_id: None,
            browser_host: None,
        };
        let prompt = build_asr_prompt_shaped(
            &dictionary,
            None,
            &pairs,
            Some(&scope),
            AsrPromptShape::ContextTerms,
            None,
        )
        .unwrap();
        assert!(prompt.contains("晓雯"));
        assert!(prompt.contains("知乎"));
        assert!(prompt.contains("TypeScript"));
        assert!(
            !prompt.contains("今天下午"),
            "Qwen/SenseVoice get terms, not a Whisper story: {prompt}"
        );
        let xiaowen = prompt.find("晓雯").unwrap();
        let typescript = prompt.rfind("TypeScript").unwrap();
        assert!(xiaowen < typescript);
    }

    #[test]
    fn asr_prompt_puts_screen_tokens_first_without_secrets() {
        let ctx = crate::screen_text::extract_from_fixture(&crate::screen_text::AxWindowFixture {
            family: ContextFamily::PersonalChat,
            focus_kind: crate::context::FocusKind::Chat,
            known_ide: false,
            counterpart: Some("晓雯".into()),
            bubbles: vec!["在吗".into()],
            email_recipients: Vec::new(),
            email_subject: None,
            ide_filenames: Vec::new(),
            ide_symbols: Vec::new(),
            selected_text: None,
            document_name: None,
            focused_role: "AXTextField".into(),
            secure: false,
            banking_preset: false,
            window_title: "晓雯 - 微信".into(),
            raw_url: Some("https://wx.qq.com/chat/secret".into()),
            pid: 4242,
        });
        let prompt = build_asr_prompt_shaped(
            &["TypeScript".into()],
            None,
            &[],
            None,
            AsrPromptShape::WhisperTranscript,
            Some(&ctx),
        )
        .unwrap();
        assert!(prompt.find("晓雯").unwrap() < prompt.find("TypeScript").unwrap_or(usize::MAX));
        assert!(!prompt.contains("https://"));
        assert!(!prompt.contains("4242"));
        assert!(!prompt.contains("微信"));
    }

    #[test]
    fn asr_prompt_ends_with_mixed_language_seed_and_never_english_only() {
        let policy = ContextPolicy {
            preserve_technical_tokens: true,
            ..ContextPolicy::default()
        };
        let prompt = build_asr_prompt(&["VoiceFlow".into()], Some(&policy), &[], None).unwrap();
        assert!(prompt.contains("不要翻译"));
        assert!(
            prompt.contains("VoiceFlow"),
            "Whisper continues from the prompt tail: {prompt}"
        );
        assert!(
            !prompt.ends_with("versions"),
            "English technical clause must not be the decoder prefix: {prompt}"
        );
        assert!(prompt.contains("今天下午") || prompt.ends_with("不要翻译。"));

        let empty = build_asr_prompt(&[], None, &[], None).unwrap();
        assert!(empty.ends_with("这个 API 的 latency 太高了。"));
        assert!(empty.contains("不要翻译"));
    }

    #[test]
    fn decide_cleanup_sends_prose_including_short_chat_to_provider() {
        let short = CleanupIntent::implicit("好的哈哈我晚点回你");
        let long =
            CleanupIntent::implicit("请帮我看一下这份季度报告里的几个数字然后在周五之前把意见发我");
        assert_eq!(
            decide(true, None, ContextFamily::PersonalChat, &short),
            CleanupRoute::Provider(CleanupEffort::Heavy)
        );
        assert_eq!(
            decide(true, None, ContextFamily::PersonalChat, &long),
            CleanupRoute::Provider(CleanupEffort::Heavy)
        );
        assert_eq!(
            decide(true, None, ContextFamily::Email, &short),
            CleanupRoute::Provider(CleanupEffort::Heavy)
        );
        assert_eq!(
            decide(true, None, ContextFamily::Email, &long),
            CleanupRoute::Provider(CleanupEffort::Heavy)
        );
        assert_eq!(
            decide(true, None, ContextFamily::WorkChat, &long),
            CleanupRoute::Provider(CleanupEffort::Heavy)
        );
        assert_eq!(
            decide(true, None, ContextFamily::SocialMedia, &short),
            CleanupRoute::Provider(CleanupEffort::Heavy)
        );
        assert_eq!(
            decide(true, None, ContextFamily::BrowserSearch, &short),
            CleanupRoute::Provider(CleanupEffort::Heavy)
        );
        assert_eq!(
            decide(true, None, ContextFamily::Terminal, &long),
            CleanupRoute::LocalOnly
        );
        assert_eq!(
            decide(true, None, ContextFamily::FormFilling, &short),
            CleanupRoute::LocalOnly
        );
        assert_eq!(
            decide(true, None, ContextFamily::PromptOrCode, &long),
            CleanupRoute::Provider(CleanupEffort::Heavy)
        );
        assert_eq!(
            decide(true, None, ContextFamily::DeveloperCollaboration, &long,),
            CleanupRoute::Provider(CleanupEffort::Heavy)
        );
        assert_eq!(
            decide(
                true,
                None,
                ContextFamily::PersonalChat,
                &CleanupIntent::implicit("   "),
            ),
            CleanupRoute::LocalOnly
        );
        let spoken = CleanupIntent {
            source: IntentSource::SpokenCommand,
            ..CleanupIntent::implicit("改正式")
        };
        assert_eq!(
            decide(true, None, ContextFamily::PersonalChat, &spoken),
            CleanupRoute::Provider(CleanupEffort::Command)
        );
        assert_eq!(
            decide(false, None, ContextFamily::PersonalChat, &long),
            CleanupRoute::LocalOnly
        );
        let mapping = AppMapping {
            cleanup_effort: Some(CleanupEffort::Standard),
            ..sample_mapping()
        };
        assert_eq!(
            decide(true, Some(&mapping), ContextFamily::PersonalChat, &long),
            CleanupRoute::Provider(CleanupEffort::Standard)
        );
        let mapping_off = AppMapping {
            cleanup_enabled: false,
            cleanup_effort: None,
            ..mapping
        };
        assert_eq!(
            decide(true, Some(&mapping_off), ContextFamily::PersonalChat, &long,),
            CleanupRoute::LocalOnly
        );
    }

    #[test]
    fn history_personal_chat_uses_global_heavy() {
        let intent =
            CleanupIntent::implicit("请帮我看一下这份季度报告里的几个数字然后在周五之前把意见发我");
        let scope = PromptScope::from_history(Some("chat.personal"), Some("personal_chat"), None);
        assert_eq!(scope.family.as_deref(), Some("personal_chat"));
        assert_eq!(
            decide(
                true,
                None,
                family_from_profile_id("chat.personal", &[]),
                &intent,
            ),
            CleanupRoute::Provider(CleanupEffort::Heavy)
        );
    }

    #[test]
    fn unmapped_wechat_uses_global_heavy() {
        let route = decide_cleanup(
            true,
            CleanupIntensity::Heavy,
            None,
            ContextFamily::PersonalChat,
            &CleanupIntent::implicit("好的哈哈我晚点回你"),
        );
        assert_eq!(route, CleanupRoute::Provider(CleanupEffort::Heavy));
    }

    #[test]
    fn mapping_light_overrides_global_heavy() {
        let mut mapping = sample_mapping();
        mapping.cleanup_intensity = Some(CleanupIntensity::Light);
        let route = decide_cleanup(
            true,
            CleanupIntensity::Heavy,
            Some(&mapping),
            ContextFamily::PersonalChat,
            &CleanupIntent::implicit("好的"),
        );
        assert_eq!(route, CleanupRoute::Provider(CleanupEffort::Light));
    }

    #[test]
    fn intensity_off_or_cleanup_disabled_is_local_only() {
        let intent = CleanupIntent::implicit("hello");
        assert_eq!(
            decide_cleanup(
                true,
                CleanupIntensity::Off,
                None,
                ContextFamily::Email,
                &intent
            ),
            CleanupRoute::LocalOnly
        );
        assert_eq!(
            decide_cleanup(
                false,
                CleanupIntensity::Heavy,
                None,
                ContextFamily::Email,
                &intent
            ),
            CleanupRoute::LocalOnly
        );
    }

    #[test]
    fn terminal_stays_local_even_when_heavy() {
        assert_eq!(
            decide_cleanup(
                true,
                CleanupIntensity::Heavy,
                None,
                ContextFamily::Terminal,
                &CleanupIntent::implicit("ls -la"),
            ),
            CleanupRoute::LocalOnly
        );
    }

    #[test]
    fn default_learn_off_covers_password_managers_and_hr() {
        assert!(!scene_allows_learn(
            &[],
            "browser.unknown",
            Some("com.1password.1password"),
            None
        ));
        assert!(!scene_allows_learn(
            &[],
            "browser.unknown",
            Some("com.agilebits.onepassword7"),
            None
        ));
        assert!(!scene_allows_learn(
            &[],
            "browser.unknown",
            None,
            Some("company.myworkday.com")
        ));
        assert!(!scene_allows_learn(
            &[],
            "browser.unknown",
            None,
            Some("acme.okta.com")
        ));
        assert!(!scene_allows_learn(
            &[],
            "browser.unknown",
            None,
            Some("accounts.google.com")
        ));
        assert!(scene_allows_learn(
            &[],
            "chat.personal",
            Some("com.tencent.xinWeChat"),
            None
        ));
        assert!(scene_allows_learn(
            &[],
            "chat.personal",
            None,
            Some("mail.google.com")
        ));
    }

    #[test]
    fn user_mapping_can_reenable_learn_on_a_denied_host() {
        let mapping = AppMapping {
            id: "workday".into(),
            label: "Workday".into(),
            family: ContextFamily::FormFilling,
            mode_id: None,
            bundle_id: None,
            executable: None,
            browser_host: Some("company.myworkday.com".into()),
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
        };
        assert!(scene_allows_learn(
            &[mapping],
            "user.workday",
            None,
            Some("company.myworkday.com")
        ));
    }

    #[test]
    fn mapping_can_disable_learning() {
        let mut mapping = AppMapping {
            id: "wechat".into(),
            label: "微信".into(),
            family: ContextFamily::PersonalChat,
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
            dictionary_learn_enabled: false,
        };
        assert!(!mapping_allows_learn(&[mapping.clone()], "user.wechat"));
        mapping.enabled = false;
        mapping.dictionary_learn_enabled = true;
        assert!(!mapping_allows_learn(&[mapping], "user.wechat"));
    }
}

#[cfg(test)]
mod fuzzy_regressions {
    use super::*;
    #[test]
    fn fuzzy_only_matches_unprotected_words_on_same_line() {
        let dictionary = vec!["TypeScript".into()];
        assert_eq!(
            apply_ascii_fuzzy_dictionary("type script", &dictionary),
            "TypeScript"
        );
        for input in [
            "type\nscript",
            "type\r\nscript",
            "type\tscript",
            "type\u{b}script",
            "type\u{c}script",
            "type\u{85}script",
            "type\u{2028}script",
            "type\u{2029}script",
            "https://type.script",
            "/type/script",
            "`type script`",
            "12345 中文",
        ] {
            assert_eq!(
                apply_ascii_fuzzy_dictionary(input, &dictionary),
                input,
                "{input}"
            );
        }
    }
}
