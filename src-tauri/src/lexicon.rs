//! Local lexicon replace, ASR ranking, cleanup hit-pairs, and cleanup effort.

use crate::context::{AppMapping, ContextFamily, ContextPolicy, ContextSnapshot};
use crate::dictionary_learn::{is_cjk, is_latin_cont, is_latin_start};
use crate::llm::{CleanupEffort, CleanupIntensity, CleanupIntent, IntentSource};
use crate::screen_text::ScreenTextContext;
use crate::store::LearnPairRecord;
use std::collections::HashSet;

pub const MAX_ASR_PROMPT_TOKENS: usize = 200;
const ASR_NO_TRANSLATE: &str = "不要翻译。";
const ASR_EMPTY_SEED: &str = "不要翻译。这个 API 的 latency 太高了。";
const GPT_TRANSCRIBE_SCENE: &str = "今天下午在看文档。中英混合听写";
const GPT_TRANSCRIBE_NEUTRAL_SCENE: &str =
    "Transcribe accurately in the spoken language; preserve proper nouns and technical terms.";
const MAX_CLEANUP_PAIRS: usize = 8;
const MAX_CLEANUP_PAIR_CHARS: usize = 400;
const MAX_CONTEXT_TERMS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsrPromptShape {
    WhisperTranscript,
    ContextTerms,
    InstantVocabulary,
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
    if provider == crate::providers::EngineProvider::DashScope {
        return AsrPromptShape::InstantVocabulary;
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
    pub match_policy: crate::store::LearnPairMatchPolicy,
    pub rule_key: Option<String>,
    pub(crate) expansion_policy: PairExpansionPolicy,
}

/// Controls whether a pair may expand beyond its declared alias.
///
/// Match casing is a separate rule. New and scoped stored pairs are alias-only;
/// legacy evaluation fixtures opt in through their existing fixture contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PairExpansionPolicy {
    AliasOnly,
    LegacyHeuristics,
}

impl LexiconPair {
    #[cfg(test)]
    pub fn new(before: impl Into<String>, after: impl Into<String>) -> Self {
        Self {
            before: before.into(),
            after: after.into(),
            match_policy: crate::store::LearnPairMatchPolicy::IgnoreCase,
            rule_key: None,
            expansion_policy: PairExpansionPolicy::AliasOnly,
        }
    }

    /// Construct a rule from the versioned evaluation fixture's legacy local
    /// replacement contract, which explicitly retains historical heuristics.
    #[cfg(any(test, feature = "evaluation"))]
    pub(crate) fn legacy_fixture(before: impl Into<String>, after: impl Into<String>) -> Self {
        Self {
            before: before.into(),
            after: after.into(),
            match_policy: crate::store::LearnPairMatchPolicy::IgnoreCase,
            rule_key: None,
            expansion_policy: PairExpansionPolicy::LegacyHeuristics,
        }
    }

    fn from_record(row: &LearnPairRecord) -> Self {
        let is_global_rule = row.is_live_promoted() && row.is_provider_egress_eligible_word_pair();
        let is_legacy_unscoped_rule = row.match_policy
            == crate::store::LearnPairMatchPolicy::IgnoreCase
            && row.pair_key
                == crate::dictionary_learn::pair_key(&row.before_surface, &row.after_surface)
            && !row.manual_scoped
            && !row.auto_scoped
            && !row.has_declared_scope();
        Self {
            before: row.before_surface.clone(),
            after: row.after_surface.clone(),
            match_policy: row.match_policy,
            rule_key: Some(row.pair_key.clone()),
            expansion_policy: if is_global_rule || is_legacy_unscoped_rule {
                PairExpansionPolicy::LegacyHeuristics
            } else {
                PairExpansionPolicy::AliasOnly
            },
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

pub(crate) fn mapping_matching_target<'a>(
    mappings: &'a [AppMapping],
    bundle_id: Option<&str>,
    browser_host: Option<&str>,
) -> Option<&'a AppMapping> {
    mappings.iter().find(|mapping| {
        if !mapping.enabled
            // This fallback has no executable, URL path, or focus evidence.
            // Leave those selectors to the exact profile-ID path above.
            || mapping.executable.is_some()
            || mapping.browser_path_prefix.is_some()
            || mapping.focused_field.is_some()
        {
            return false;
        }
        let bundle_matches = mapping.bundle_id.as_deref().is_none_or(|expected| {
            bundle_id.is_some_and(|actual| expected.eq_ignore_ascii_case(actual))
        });
        let host_matches = mapping.browser_host.as_deref().is_none_or(|expected| {
            host_is_or_under(
                browser_host
                    .and_then(crate::context::normalize_host)
                    .as_deref(),
                crate::context::normalize_host(expected).as_deref(),
            )
        });
        (mapping.bundle_id.is_some() || mapping.browser_host.is_some())
            && bundle_matches
            && host_matches
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
    let (output, applied_terms, _) = apply_lexicon_replacements_with_facts(text, pairs, blocking);
    (output, applied_terms)
}

fn apply_lexicon_replacements_with_facts(
    text: &str,
    pairs: &[LexiconPair],
    blocking: &[String],
) -> (String, Vec<String>, Vec<LexiconPair>) {
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
    let byte_offsets = text
        .char_indices()
        .map(|(offset, _)| offset)
        .chain(std::iter::once(text.len()))
        .collect::<Vec<_>>();
    let mut protected_ranges = crate::spoken_revision::literal_ranges(text);
    protected_ranges.sort_by_key(|range| range.start);
    let mut output = String::new();
    let mut applied_terms = std::collections::BTreeSet::new();
    let mut applied_pairs = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let mut matched: Option<&LexiconPair> = None;
        for rule in &rules {
            let needle: Vec<char> = rule.before.chars().collect();
            let Some(byte_start) = byte_offsets.get(index).copied() else {
                continue;
            };
            let Some(byte_end) = byte_offsets.get(index + needle.len()).copied() else {
                continue;
            };
            if protected_ranges
                .iter()
                .any(|range| byte_start < range.end && range.start < byte_end)
                || text[byte_start..byte_end].contains(['\n', '\r'])
            {
                continue;
            }
            if !region_matches(&chars, index, &needle, rule.match_policy) {
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
            if !applied_pairs.iter().any(|applied: &LexiconPair| {
                match (&applied.rule_key, &rule.rule_key) {
                    (Some(left), Some(right)) => left == right,
                    _ => {
                        applied.before == rule.before
                            && applied.after == rule.after
                            && applied.match_policy == rule.match_policy
                    }
                }
            }) {
                applied_pairs.push((*rule).clone());
            }
            index += rule.before.chars().count();
        } else {
            output.push(chars[index]);
            index += 1;
        }
    }
    (output, applied_terms.into_iter().collect(), applied_pairs)
}

pub(crate) fn pair_is_replaceable(row: &LearnPairRecord, dictionary_contains_after: bool) -> bool {
    row.is_live_promoted()
        && (dictionary_contains_after || row.manual_scoped || row.auto_scoped)
        && !row.before_surface.is_empty()
}

pub fn replaceable_pairs(pairs: &[LearnPairRecord], dictionary: &[String]) -> Vec<LexiconPair> {
    pairs
        .iter()
        .filter(|row| {
            pair_is_replaceable(
                row,
                crate::store::dictionary_contains_word(dictionary, &row.after_surface),
            )
        })
        .map(LexiconPair::from_record)
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

#[cfg(test)]
pub fn apply_promoted_replacements_with_usage(
    text: &str,
    pairs: &[LearnPairRecord],
    dictionary: &[String],
) -> (String, Vec<String>) {
    let replaceable = replaceable_pairs(pairs, dictionary);
    apply_explicit_replacements_with_usage(text, &replaceable, dictionary)
}

pub(crate) fn apply_explicit_replacements_with_usage(
    text: &str,
    pairs: &[LexiconPair],
    dictionary: &[String],
) -> (String, Vec<String>) {
    let mut rules = pairs.to_vec();
    for slice in nonliteral_line_slices(text) {
        rules.extend(phonetic_replace_rules(slice, pairs));
    }
    apply_lexicon_replacements_with_usage(text, &rules, dictionary)
}

pub(crate) fn apply_explicit_replacements_with_facts(
    text: &str,
    pairs: &[LexiconPair],
    dictionary: &[String],
) -> (String, Vec<String>, Vec<LexiconPair>) {
    let mut rules = pairs.to_vec();
    for slice in nonliteral_line_slices(text) {
        rules.extend(phonetic_replace_rules(slice, pairs));
    }
    apply_lexicon_replacements_with_facts(text, &rules, dictionary)
}

/// Explicitly enabled fuzzy matching cannot cross protected text or layout boundaries.
pub fn apply_ascii_fuzzy_dictionary(text: &str, dictionary: &[String]) -> String {
    let mut protected: Vec<(usize, usize)> = crate::protected_span::protected_spans(text, None)
        .into_iter()
        .map(|span| (span.start_byte, span.end_byte))
        .collect();
    protected.extend(
        crate::spoken_revision::literal_ranges(text)
            .into_iter()
            .map(|range| (range.start, range.end)),
    );
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
    hit_pairs_with_matching_universe(text, pairs, pairs, blocking)
}

/// Select and budget eligible cleanup hints while keeping the full applicable
/// rule set available to matching and overlap checks. A local-only rule may
/// block a shorter cloud-eligible alias without itself being emitted.
pub(crate) fn hit_pairs_with_matching_universe(
    text: &str,
    candidates: &[LexiconPair],
    matching_universe: &[LexiconPair],
    blocking: &[String],
) -> Vec<LexiconPair> {
    let searchable = nonliteral_line_slices(text);
    let mut hits = Vec::new();
    let mut chars = 0usize;
    for pair in candidates {
        if pair.before.is_empty() {
            continue;
        }
        let matched = searchable.iter().any(|slice| match pair.match_policy {
            crate::store::LearnPairMatchPolicy::Exact => independent_surface(
                slice,
                &pair.before,
                matching_universe,
                blocking,
                pair.match_policy,
            ),
            crate::store::LearnPairMatchPolicy::IgnoreCase => {
                independent_surface(
                    slice,
                    &pair.before,
                    matching_universe,
                    blocking,
                    pair.match_policy,
                ) || (pair.expansion_policy == PairExpansionPolicy::LegacyHeuristics
                    && (independent_surface(
                        slice,
                        &pair.after,
                        matching_universe,
                        blocking,
                        pair.match_policy,
                    ) || phonetic_surface_hit(slice, pair, matching_universe)
                        || latin_fuzzy_hit(slice, pair)))
            }
        });
        if !matched {
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
    let mut terms = Vec::new();
    let scope = scope.cloned().unwrap_or_default();

    for row in pairs
        .iter()
        .filter(|row| row.is_live_promoted() && row.is_provider_egress_eligible_dictionary_owner())
    {
        if !crate::store::dictionary_contains_word(dictionary, &row.after_surface) {
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
        if terms
            .iter()
            .any(|term: &RankedTerm| crate::store::dictionary_words_equal(&term.term, word))
        {
            continue;
        }
        let row = pairs.iter().find(|item| {
            crate::store::dictionary_words_equal(&item.after_surface, word)
                && item.before_surface.is_empty()
                && item.is_live_promoted()
                && item.is_provider_egress_eligible_dictionary_owner()
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
    let mut seen = Vec::new();
    terms.retain(|term| {
        if crate::store::dictionary_contains_word(&seen, &term.term) {
            false
        } else {
            seen.push(term.term.clone());
            true
        }
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

#[cfg(test)]
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

#[cfg(test)]
pub fn build_asr_prompt_bundle_with_permissions(
    dictionary: &[String],
    policy: Option<&ContextPolicy>,
    pairs: &[LearnPairRecord],
    scope: Option<&PromptScope>,
    shape: AsrPromptShape,
    screen: Option<&ScreenTextContext>,
    screen_permissions: crate::context::ContextSourcePermissions,
) -> AsrPromptBundle {
    build_asr_prompt_bundle_with_permissions_and_language(
        dictionary,
        policy,
        pairs,
        scope,
        shape,
        screen,
        screen_permissions,
        Some("zh"),
        &[],
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "Keep ASR lexicon prompt builder inputs explicit across language, permissions, and scope."
)]
pub fn build_asr_prompt_bundle_with_permissions_and_language(
    dictionary: &[String],
    _policy: Option<&ContextPolicy>,
    pairs: &[LearnPairRecord],
    scope: Option<&PromptScope>,
    shape: AsrPromptShape,
    screen: Option<&ScreenTextContext>,
    screen_permissions: crate::context::ContextSourcePermissions,
    language: Option<&str>,
    preferred_languages: &[String],
) -> AsrPromptBundle {
    let chinese_session = crate::languages::is_chinese_session(language, preferred_languages);
    let ranked = collect_ranked_terms(dictionary, pairs, scope);
    let build_keywords = |ranked: &[RankedTerm]| -> Vec<String> {
        let mut keywords = keywords_for_asr(ranked, MAX_CONTEXT_TERMS);
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
        keywords
    };
    if matches!(
        shape,
        AsrPromptShape::GptTranscribeKeywords
            | AsrPromptShape::MistralContextBias
            | AsrPromptShape::InstantVocabulary
    ) {
        let keywords = build_keywords(&ranked);
        return AsrPromptBundle {
            prompt: (shape == AsrPromptShape::GptTranscribeKeywords).then(|| {
                if chinese_session {
                    GPT_TRANSCRIBE_SCENE.to_owned()
                } else {
                    GPT_TRANSCRIBE_NEUTRAL_SCENE.to_owned()
                }
            }),
            keywords,
        };
    }
    let keywords = if shape == AsrPromptShape::ContextTerms {
        build_keywords(&ranked)
    } else {
        Vec::new()
    };
    AsrPromptBundle {
        prompt: weave_shaped_prompt(ranked, shape, screen, screen_permissions, chinese_session),
        keywords,
    }
}

fn weave_shaped_prompt(
    ranked: Vec<RankedTerm>,
    shape: AsrPromptShape,
    screen: Option<&ScreenTextContext>,
    screen_permissions: crate::context::ContextSourcePermissions,
    chinese_session: bool,
) -> Option<String> {
    let glue_tokens = match shape {
        AsrPromptShape::WhisperTranscript => {
            if chinese_session {
                estimate_prompt_tokens("今天下午在看文档。不要翻译。")
            } else {
                0
            }
        }
        AsrPromptShape::ContextTerms
        | AsrPromptShape::InstantVocabulary
        | AsrPromptShape::GptTranscribeKeywords
        | AsrPromptShape::MistralContextBias => 0,
    };
    let budget = MAX_ASR_PROMPT_TOKENS.saturating_sub(glue_tokens);
    if budget == 0 {
        return match shape {
            AsrPromptShape::WhisperTranscript => chinese_session.then(|| ASR_EMPTY_SEED.to_owned()),
            AsrPromptShape::ContextTerms
            | AsrPromptShape::InstantVocabulary
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
    fit_woven_prompt(hints, shape, chinese_session)
}

fn fit_woven_prompt(
    mut terms: Vec<String>,
    shape: AsrPromptShape,
    chinese_session: bool,
) -> Option<String> {
    match shape {
        AsrPromptShape::WhisperTranscript => {
            while !terms.is_empty() {
                let woven = if chinese_session {
                    weave_whisper_transcript(&terms)
                } else {
                    weave_neutral_whisper_transcript(&terms)
                };
                if estimate_prompt_tokens(&woven) <= MAX_ASR_PROMPT_TOKENS {
                    crate::learning_research::usage(&terms, true);
                    return Some(woven);
                }
                terms.remove(0);
            }
            chinese_session.then(|| ASR_EMPTY_SEED.to_owned())
        }
        AsrPromptShape::ContextTerms => {
            if terms.len() > MAX_CONTEXT_TERMS {
                let start = terms.len() - MAX_CONTEXT_TERMS;
                terms = terms.split_off(start);
            }
            while !terms.is_empty() {
                let woven = terms.join(" ");
                if estimate_prompt_tokens(&woven) <= MAX_ASR_PROMPT_TOKENS {
                    crate::learning_research::usage(&terms, true);
                    return Some(woven);
                }
                terms.remove(0);
            }
            None
        }
        AsrPromptShape::InstantVocabulary | AsrPromptShape::MistralContextBias => None,
        AsrPromptShape::GptTranscribeKeywords => Some(if chinese_session {
            GPT_TRANSCRIBE_SCENE.to_owned()
        } else {
            GPT_TRANSCRIBE_NEUTRAL_SCENE.to_owned()
        }),
    }
}

fn weave_neutral_whisper_transcript(terms: &[String]) -> String {
    // Use a bare term list so non-Chinese or unbiased-auto sessions receive
    // no English carrier sentence biasing recognition toward English.
    terms.join(", ")
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

pub fn used_pair_keys(applied_pairs: &[LexiconPair], pairs: &[LearnPairRecord]) -> Vec<String> {
    pairs
        .iter()
        .filter(|row| row.is_live_promoted())
        .filter(|row| {
            applied_pairs.iter().any(|applied| {
                if let Some(rule_key) = applied.rule_key.as_deref() {
                    rule_key == row.pair_key
                } else {
                    applied.before == row.before_surface
                        && applied.after == row.after_surface
                        && applied.match_policy == row.match_policy
                }
            })
        })
        .map(|row| row.pair_key.clone())
        .collect()
}

fn nonliteral_line_slices(text: &str) -> Vec<&str> {
    let mut literals = crate::spoken_revision::literal_ranges(text);
    literals.sort_by_key(|range| range.start);
    let mut slices = Vec::new();
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let line_end = line_start + line.len();
        let mut content_end = line_end;
        while content_end > line_start {
            if matches!(text.as_bytes()[content_end - 1], b'\n' | b'\r') {
                content_end -= 1;
            } else {
                break;
            }
        }
        let mut cursor = line_start;
        for literal in literals
            .iter()
            .filter(|range| range.start < content_end && line_start < range.end)
        {
            let literal_start = literal.start.max(line_start).min(content_end);
            if cursor < literal_start {
                slices.push(&text[cursor..literal_start]);
            }
            cursor = literal.end.max(cursor).min(content_end);
        }
        if cursor < content_end {
            slices.push(&text[cursor..content_end]);
        }
        line_start = line_end;
    }
    if text.is_empty() {
        slices.push(text);
    }
    slices
}

fn independent_surface(
    text: &str,
    needle: &str,
    pairs: &[LexiconPair],
    blocking: &[String],
    match_policy: crate::store::LearnPairMatchPolicy,
) -> bool {
    if needle.is_empty()
        || (match_policy == crate::store::LearnPairMatchPolicy::Exact && !text.contains(needle))
    {
        return false;
    }
    let probe = [LexiconPair {
        before: needle.to_owned(),
        after: "\u{FFFC}".to_owned(),
        match_policy,
        rule_key: None,
        expansion_policy: PairExpansionPolicy::AliasOnly,
    }];
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

fn region_matches(
    haystack: &[char],
    start: usize,
    needle: &[char],
    match_policy: crate::store::LearnPairMatchPolicy,
) -> bool {
    if start + needle.len() > haystack.len() || needle.is_empty() {
        return false;
    }
    haystack[start..start + needle.len()]
        .iter()
        .zip(needle)
        .all(|(left, right)| {
            if match_policy == crate::store::LearnPairMatchPolicy::IgnoreCase
                && (is_latin_start(*left)
                    || is_latin_cont(*left)
                    || is_latin_start(*right)
                    || is_latin_cont(*right))
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
    let after_ok = match chars.get(after_index) {
        None => true,
        Some(&'.') => period_run_is_sentence_boundary(chars, after_index, start),
        Some(&value) => !is_latin_token_char(value),
    };
    before_ok && after_ok
}

fn period_run_is_sentence_boundary(
    chars: &[char],
    period_index: usize,
    alias_start: usize,
) -> bool {
    let mut next_index = period_index + 1;
    while chars.get(next_index) == Some(&'.') {
        next_index += 1;
    }
    let followed_by_sentence_gap = chars
        .get(next_index)
        .is_none_or(|value| value.is_whitespace());
    let follows_path_separator = alias_start
        .checked_sub(1)
        .and_then(|index| chars.get(index))
        .is_some_and(|value| matches!(value, '/' | '\\'));
    followed_by_sentence_gap && !follows_path_separator
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
        if pair.expansion_policy != PairExpansionPolicy::LegacyHeuristics
            || pair.match_policy == crate::store::LearnPairMatchPolicy::Exact
        {
            continue;
        }
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
            extra.push(LexiconPair {
                before: window,
                after: pair.after.clone(),
                match_policy: pair.match_policy,
                rule_key: pair.rule_key.clone(),
                expansion_policy: pair.expansion_policy,
            });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ContextFamily;
    use crate::dictionary_learn::pair_key;
    use crate::llm::{CleanupIntensity, CleanupIntent};
    use crate::store::LearnPairMatchPolicy;

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
            writing_task_id: None,
            writing_preferences: Vec::new(),
            preference_guidance_provider_approved: false,
            preference_revision: 0,
            deleted: false,
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
            manual_scoped: false,
            auto_scoped: false,
            match_policy: LearnPairMatchPolicy::default(),
            binding_invalidated_at: None,
            binding_invalidated_reason: None,
            replacement_runs: 0,
        }
    }

    fn global_live_pair(before: &str, after: &str, hits: u32) -> LearnPairRecord {
        let mut row = live_pair(before, after, "general", hits);
        row.family = None;
        row
    }

    #[test]
    fn c1_word_identity_and_matching_keep_ordinary_casefold_and_exact_casing_distinct() {
        let scope = crate::store::LearnPairScope {
            family: Some("developer_collaboration".into()),
            mapping_id: Some("codex".into()),
            native_bundle: Some("com.openai.codex".into()),
            ..crate::store::LearnPairScope::default()
        };
        let ordinary_policy = crate::store::LearnPairMatchPolicy::IgnoreCase;
        let ordinary_key =
            crate::store::scoped_word_rule_key("voysflow", "VoiceFlow", &scope, ordinary_policy);
        let differently_cased_key =
            crate::store::scoped_word_rule_key("VOYSFLOW", "VoiceFlow", &scope, ordinary_policy);
        assert_eq!(ordinary_key, differently_cased_key);
        let mut ordinary_row = live_pair("voysflow", "VoiceFlow", "developer_collaboration", 3);
        ordinary_row.pair_key = ordinary_key.into_string();
        ordinary_row.mapping_id = scope.mapping_id.clone();
        ordinary_row.native_bundle = scope.native_bundle.clone();
        ordinary_row.manual_scoped = true;
        let ordinary = replaceable_pairs(&[ordinary_row], &[]);
        assert_eq!(
            apply_explicit_replacements_with_usage("Use VOYSFLOW today", &ordinary, &[]).0,
            "Use VoiceFlow today"
        );
        assert_eq!(hit_pairs("Use VOYSFLOW today", &ordinary, &[]).len(), 1);

        let exact_policy = crate::store::LearnPairMatchPolicy::Exact;
        let exact_key =
            crate::store::scoped_word_rule_key("github", "GitHub", &scope, exact_policy);
        let capitalized_alias_key =
            crate::store::scoped_word_rule_key("GitHub", "github", &scope, exact_policy);
        assert_ne!(exact_key, capitalized_alias_key);
        let mut exact_row = live_pair("github", "GitHub", "developer_collaboration", 3);
        exact_row.pair_key = exact_key.into_string();
        exact_row.mapping_id = scope.mapping_id.clone();
        exact_row.native_bundle = scope.native_bundle.clone();
        exact_row.match_policy = exact_policy;
        exact_row.manual_scoped = true;
        let exact = replaceable_pairs(&[exact_row], &[]);
        assert_eq!(
            apply_explicit_replacements_with_usage("github", &exact, &[]).0,
            "GitHub"
        );
        assert_eq!(
            apply_explicit_replacements_with_usage("GitHub", &exact, &[]).0,
            "GitHub"
        );
        assert_eq!(hit_pairs("github", &exact, &[]).len(), 1);
        assert!(hit_pairs("GitHub", &exact, &[]).is_empty());
        assert!(hit_pairs("GITHUB", &exact, &[]).is_empty());
    }

    #[test]
    fn latin_alias_period_boundaries_preserve_identifiers_across_match_policies() {
        let scope = crate::store::LearnPairScope {
            mapping_id: Some("codex".into()),
            native_bundle: Some("com.openai.codex".into()),
            ..crate::store::LearnPairScope::default()
        };
        let scoped_rule = |before: &str, after: &str, match_policy: LearnPairMatchPolicy| {
            let mut row = live_pair(before, after, "developer_collaboration", 3);
            row.pair_key = crate::store::scoped_word_rule_key(before, after, &scope, match_policy)
                .into_string();
            row.mapping_id = scope.mapping_id.clone();
            row.native_bundle = scope.native_bundle.clone();
            row.match_policy = match_policy;
            row.manual_scoped = true;
            replaceable_pairs(&[row], &[])
                .into_iter()
                .next()
                .expect("manual scoped alias is locally replaceable")
        };

        let alias_policy = crate::store::word_rule_match_policy("Reakt", "React");
        assert_eq!(alias_policy, LearnPairMatchPolicy::IgnoreCase);
        let alias = scoped_rule("Reakt", "React", alias_policy);
        for (source, expected) in [
            ("Use Reakt.", "Use React."),
            ("Use Reakt. today.", "Use React. today."),
            ("Use Reakt...", "Use React..."),
        ] {
            let (replaced, used, applied) =
                apply_explicit_replacements_with_facts(source, std::slice::from_ref(&alias), &[]);
            assert_eq!(replaced, expected, "{source}");
            assert_eq!(used, vec!["React"], "{source}");
            assert_eq!(applied, vec![alias.clone()], "{source}");
            assert_eq!(
                hit_pairs(source, std::slice::from_ref(&alias), &[]),
                vec![alias.clone()],
                "{source}"
            );
        }

        for protected in [
            "Acme.Reakt",
            ".Reakt",
            "foo._Reakt",
            "Reakt.dev",
            "Reakt._internal",
            "Reakt.équipe",
            "Reakt.中文",
            "./Reakt./src",
            "./Reakt.",
            "https://Reakt.",
        ] {
            let (replaced, used, applied) = apply_explicit_replacements_with_facts(
                protected,
                std::slice::from_ref(&alias),
                &[],
            );
            assert_eq!(replaced, protected, "{protected}");
            assert!(used.is_empty(), "{protected}");
            assert!(applied.is_empty(), "{protected}");
            assert!(
                hit_pairs(protected, std::slice::from_ref(&alias), &[]).is_empty(),
                "{protected}"
            );
        }

        let exact_policy = crate::store::word_rule_match_policy("github", "GitHub");
        assert_eq!(exact_policy, LearnPairMatchPolicy::Exact);
        let exact_alias = scoped_rule("github", "GitHub", exact_policy);
        let exact_source = "Use github.";
        let (replaced, used, applied) = apply_explicit_replacements_with_facts(
            exact_source,
            std::slice::from_ref(&exact_alias),
            &[],
        );
        assert_eq!(replaced, "Use GitHub.");
        assert_eq!(used, vec!["GitHub"]);
        assert_eq!(applied, vec![exact_alias.clone()]);
        assert_eq!(
            hit_pairs(exact_source, std::slice::from_ref(&exact_alias), &[]),
            vec![exact_alias]
        );

        let dotted_policy = crate::store::word_rule_match_policy("Reakt.dev", "React.dev");
        let dotted_alias = scoped_rule("Reakt.dev", "React.dev", dotted_policy);
        let source = "Use Reakt.dev.";
        let (replaced, used, applied) = apply_explicit_replacements_with_facts(
            source,
            std::slice::from_ref(&dotted_alias),
            &[],
        );
        assert_eq!(replaced, "Use React.dev.");
        assert_eq!(used, vec!["React.dev"]);
        assert_eq!(applied, vec![dotted_alias.clone()]);
        assert_eq!(
            hit_pairs(source, std::slice::from_ref(&dotted_alias), &[]),
            vec![dotted_alias]
        );
    }

    #[test]
    fn scoped_stored_aliases_match_only_the_declared_surface() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "voiceflow-lexicon-scoped-alias-{}-{now}",
            std::process::id()
        ));
        let scope_a = crate::store::LearnPairScope {
            mapping_id: Some("codex".into()),
            native_bundle: Some("com.openai.codex".into()),
            ..crate::store::LearnPairScope::default()
        };
        let scope_b = crate::store::LearnPairScope {
            mapping_id: Some("textedit".into()),
            native_bundle: Some("com.apple.TextEdit".into()),
            ..crate::store::LearnPairScope::default()
        };
        let declared = [
            ("知呼", "知乎"),
            ("之乎", "之湖"),
            ("voysflow", "VoiceFlow"),
            ("github", "GitHub"),
        ];
        for (before, after) in declared {
            let policy = crate::store::word_rule_match_policy(before, after);
            let key = crate::store::scoped_word_rule_key(before, after, &scope_a, policy);
            crate::store::ensure_manual_scoped_learn_pair(
                &dir,
                key.as_str(),
                before,
                after,
                &scope_a,
            )
            .expect("save actual manual scoped pair");
        }

        let rows = crate::store::list_learn_pairs(&dir).expect("read stored manual pairs");
        assert_eq!(rows.len(), declared.len());
        assert!(rows.iter().all(|row| row.manual_scoped));
        let selected_a = scope_a.applicable_promoted_records(&rows);
        let rules_a = replaceable_pairs(&selected_a, &[]);
        assert_eq!(selected_a.len(), declared.len());

        let rule = |before: &str| {
            rules_a
                .iter()
                .find(|pair| pair.before == before)
                .expect("stored alias is applicable")
                .clone()
        };
        let zhihu_alias = rule("知呼");
        let zhi_hu_alias = rule("之乎");
        let latin_ignore_case = rule("voysflow");
        let latin_exact = rule("github");

        let (positive, used, applied) =
            apply_explicit_replacements_with_facts("今天去知呼", &rules_a, &[]);
        assert_eq!(positive, "今天去知乎");
        assert_eq!(used, vec!["知乎"]);
        assert_eq!(applied, vec![zhihu_alias.clone()]);
        assert_eq!(
            hit_pairs("今天去知呼", &rules_a, &[]),
            vec![zhihu_alias.clone()]
        );

        // A one-name mapping must not turn another same-sound name into its target.
        let (unrelated, unused, unapplied) = apply_explicit_replacements_with_facts(
            "我提到之乎",
            std::slice::from_ref(&zhihu_alias),
            &[],
        );
        assert_eq!(unrelated, "我提到之乎");
        assert!(unused.is_empty());
        assert!(unapplied.is_empty());
        assert!(hit_pairs("我提到之乎", std::slice::from_ref(&zhihu_alias), &[]).is_empty());
        assert!(hit_pairs("今天提到知乎", std::slice::from_ref(&zhihu_alias), &[]).is_empty());
        assert!(used_pair_keys(&unapplied, &selected_a).is_empty());

        // When both names are explicitly declared, each surface keeps its own owner.
        let (second_name, _, second_applied) =
            apply_explicit_replacements_with_facts("我提到之乎", &rules_a, &[]);
        assert_eq!(second_name, "我提到之湖");
        assert_eq!(second_applied, vec![zhi_hu_alias.clone()]);
        assert_eq!(hit_pairs("我提到之乎", &rules_a, &[]), vec![zhi_hu_alias]);

        let (casefolded, _, casefolded_applied) =
            apply_explicit_replacements_with_facts("Use VOYSFLOW", &rules_a, &[]);
        assert_eq!(casefolded, "Use VoiceFlow");
        assert_eq!(casefolded_applied, vec![latin_ignore_case.clone()]);
        assert_eq!(
            hit_pairs("Use VOYSFLOW", &rules_a, &[]),
            vec![latin_ignore_case]
        );
        let (fuzzy_latin, fuzzy_used, fuzzy_applied) =
            apply_explicit_replacements_with_facts("Use VoiceF1o", &rules_a, &[]);
        assert_eq!(fuzzy_latin, "Use VoiceF1o");
        assert!(fuzzy_used.is_empty());
        assert!(fuzzy_applied.is_empty());
        assert!(hit_pairs("Use VoiceF1o", &rules_a, &[]).is_empty());

        let (exact_alias, _, exact_applied) =
            apply_explicit_replacements_with_facts("github", &rules_a, &[]);
        assert_eq!(exact_alias, "GitHub");
        assert_eq!(exact_applied, vec![latin_exact.clone()]);
        let (wrong_case, _, wrong_case_applied) =
            apply_explicit_replacements_with_facts("GitHub", &rules_a, &[]);
        assert_eq!(wrong_case, "GitHub");
        assert!(wrong_case_applied.is_empty());
        assert!(hit_pairs("GitHub", &[latin_exact], &[]).is_empty());

        let protected = "`知呼` and \"知呼\"";
        let (protected_output, protected_used, protected_applied) =
            apply_explicit_replacements_with_facts(
                protected,
                std::slice::from_ref(&zhihu_alias),
                &[],
            );
        assert_eq!(protected_output, protected);
        assert!(protected_used.is_empty());
        assert!(protected_applied.is_empty());
        assert!(hit_pairs(protected, &[zhihu_alias], &[]).is_empty());

        crate::store::bump_learn_pairs_used(&dir, &[]).expect("apply empty usage update");
        let after_unrelated = crate::store::list_learn_pairs(&dir).expect("read usage counters");
        assert!(after_unrelated
            .iter()
            .all(|row| row.replacement_runs == 0 && row.last_used_at.is_none()));

        let selected_b = scope_b.applicable_promoted_records(&rows);
        assert!(
            selected_b.is_empty(),
            "the existing scope owner rejects App B"
        );
        let rules_b = replaceable_pairs(&selected_b, &[]);
        let (wrong_app, _, wrong_app_applied) =
            apply_explicit_replacements_with_facts("今天去知呼", &rules_b, &[]);
        assert_eq!(wrong_app, "今天去知呼");
        assert!(wrong_app_applied.is_empty());
        assert!(hit_pairs("今天去知呼", &rules_b, &[]).is_empty());

        let _ = std::fs::remove_dir_all(dir);
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
        let mut pair = live_pair("知呼", "知乎", "personal_chat", 3);
        pair.family = None;
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
    fn ascii_case_membership_preserves_rule_surface_and_ranks_before_deduplication() {
        let mut older = live_pair("VoiceFlo", "VoiceFlow", "general", 100);
        older.family = None;
        older.last_used_at = Some("2026-01-01".into());
        let mut pinned = live_pair("VoiceFlo beta", "voiceflow", "general", 1);
        pinned.family = None;
        pinned.pinned = true;
        pinned.last_used_at = Some("2026-01-02".into());
        let pairs = vec![older, pinned];
        let dictionary = vec!["VOICEFLOW".into()];

        let replacements = replaceable_pairs(&pairs, &dictionary);
        assert_eq!(replacements[0].after, "VoiceFlow");
        assert_eq!(replacements[1].after, "voiceflow");
        assert_eq!(
            apply_promoted_replacements("VoiceFlo", &pairs, &dictionary),
            "VoiceFlow"
        );

        let ranked = collect_ranked_terms(&dictionary, &pairs, None);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].term, "voiceflow");
        assert!(ranked[0].pinned);

        let mut screen_term = live_pair("", "voiceflow", "general", 1);
        screen_term.family = None;
        screen_term.pinned = true;
        screen_term.last_used_at = Some("2026-02-01".into());
        let with_screen_term = collect_ranked_terms(
            &dictionary,
            &[pairs[0].clone(), pairs[1].clone(), screen_term],
            None,
        );
        assert_eq!(with_screen_term.len(), 1);
        assert!(with_screen_term[0].pinned);
        assert_eq!(
            with_screen_term[0].last_used_at.as_deref(),
            Some("2026-02-01")
        );
    }

    #[test]
    fn scoped_pair_does_not_promote_manual_dictionary_casing_in_asr_ranking() {
        let dictionary = vec!["acme".to_owned()];
        let scoped = live_pair("Acme alias", "AcMe", "general", 100);
        let ranked = collect_ranked_terms(&dictionary, &[scoped], None);

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].term, "acme");
        assert_eq!(ranked[0].scope_score, 0);
        assert_eq!(ranked[0].hits, 0);
        assert!(!ranked[0].pinned);
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
        let pairs = vec![LexiconPair::legacy_fixture("知呼", "知乎")];
        assert_eq!(
            hit_pairs("今天去之乎看看吧", &pairs, &[]),
            vec![LexiconPair::legacy_fixture("知呼", "知乎")]
        );
    }

    #[test]
    fn legacy_expansions_require_canonical_legacy_provenance_and_ignorecase() {
        let mut legacy = live_pair("知呼", "知乎", "personal_chat", 3);
        legacy.family = None;
        let dictionary = ["知乎".to_owned()];
        let legacy_rules = replaceable_pairs(std::slice::from_ref(&legacy), &dictionary);
        assert_eq!(
            apply_explicit_replacements_with_usage("之乎", &legacy_rules, &dictionary).0,
            "知乎"
        );
        assert_eq!(
            hit_pairs("之乎", &legacy_rules, &dictionary),
            legacy_rules,
            "canonical unscoped legacy rows retain their historical phonetic expansion"
        );

        let alias_only = LexiconPair::new("知呼", "知乎");
        assert_eq!(
            apply_explicit_replacements_with_usage("之乎", std::slice::from_ref(&alias_only), &[])
                .0,
            "之乎"
        );
        assert!(hit_pairs("之乎", &[alias_only], &[]).is_empty());

        let mut exact_legacy = legacy.clone();
        exact_legacy.match_policy = crate::store::LearnPairMatchPolicy::Exact;
        let exact_rules = replaceable_pairs(std::slice::from_ref(&exact_legacy), &dictionary);
        assert_eq!(
            apply_explicit_replacements_with_usage("之乎", &exact_rules, &dictionary).0,
            "之乎"
        );

        let mut malformed_legacy = legacy;
        malformed_legacy.pair_key = "unrelated\u{1e}key".into();
        let malformed_rules = replaceable_pairs(&[malformed_legacy], &dictionary);
        assert_eq!(
            apply_explicit_replacements_with_usage("之乎", &malformed_rules, &dictionary).0,
            "之乎"
        );
    }

    #[test]
    fn apply_replaces_unique_same_pinyin_cjk_surface() {
        let mut pair = live_pair("知呼", "知乎", "personal_chat", 3);
        pair.family = None;
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
        let mut pairs = [
            live_pair("医士", "医师", "personal_chat", 3),
            live_pair("意示", "意识", "personal_chat", 3),
        ];
        pairs.iter_mut().for_each(|pair| pair.family = None);
        let dictionary = vec!["医师".into(), "意识".into()];
        assert_eq!(
            apply_promoted_replacements("一时再说", &pairs, &dictionary),
            "一时再说"
        );
    }

    #[test]
    fn hit_pairs_retrieves_latin_within_edit_distance_two() {
        let pairs = vec![LexiconPair::legacy_fixture("配森", "Python")];
        assert_eq!(
            hit_pairs("install pyton now", &pairs, &[]),
            vec![LexiconPair::legacy_fixture("配森", "Python")]
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
    fn cloud_hint_candidate_keeps_local_long_alias_as_a_matching_blocker() {
        let manual_short = LexiconPair::new("链接", "link");
        let local_long = LexiconPair::new("超链接", "超连结");
        let eligible = [manual_short.clone()];
        let full_universe = [manual_short.clone(), local_long];

        assert_eq!(
            hit_pairs_with_matching_universe("点击超链接", &eligible, &full_universe, &[],),
            Vec::<LexiconPair>::new(),
            "the private longer alias must block a spurious short cloud hint"
        );
        assert_eq!(
            hit_pairs("点击超链接", &eligible, &[]),
            vec![manual_short],
            "without the longer blocker the short alias would be emitted"
        );
    }

    #[test]
    fn exact_pair_replacement_hits_and_usage_skip_quoted_and_code_literals() {
        let pair = LexiconPair::new("LOM", "LLM");
        let source = "LOM `LOM` \"LOM\"\nLOM";
        let (output, terms, applied) =
            apply_explicit_replacements_with_facts(source, std::slice::from_ref(&pair), &[]);
        assert_eq!(output, "LLM `LOM` \"LOM\"\nLLM");
        assert_eq!(terms, vec!["LLM"]);
        assert_eq!(applied, vec![pair.clone()]);
        assert_eq!(
            hit_pairs(source, std::slice::from_ref(&pair), &[]),
            vec![pair.clone()]
        );

        let literal_only = "`LOM` and \"LOM\"";
        let (unchanged, terms, applied) =
            apply_explicit_replacements_with_facts(literal_only, std::slice::from_ref(&pair), &[]);
        assert_eq!(unchanged, literal_only);
        assert!(terms.is_empty());
        assert!(applied.is_empty());
        assert!(hit_pairs(literal_only, &[pair], &[]).is_empty());
    }

    #[test]
    fn asr_prompt_is_end_weighted_and_stays_under_200_tokens() {
        let mut pairs = Vec::new();
        let mut dictionary = Vec::new();
        for index in 0..80 {
            let after = format!("旧词{index}超长家庭词条");
            dictionary.push(after.clone());
            pairs.push(global_live_pair(&format!("旧{index}"), &after, 1));
        }
        for index in 0..3 {
            let after = format!("微信词{index}");
            dictionary.push(after.clone());
            pairs.push(global_live_pair(&format!("微{index}"), &after, 9));
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
            global_live_pair("小文", "晓雯", 1),
            global_live_pair("知呼", "知乎", 5),
            global_live_pair("类型脚本", "TypeScript", 9),
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
            global_live_pair("小文", "晓雯", 1),
            global_live_pair("知呼", "知乎", 5),
            global_live_pair("类型脚本", "TypeScript", 9),
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
            global_live_pair("小文", "晓雯", 1),
            global_live_pair("知呼", "知乎", 5),
            global_live_pair("类型脚本", "TypeScript", 9),
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
            writing_task_id: None,
            writing_preferences: Vec::new(),
            preference_guidance_provider_approved: false,
            preference_revision: 0,
            deleted: false,
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
    fn coarse_learning_fallback_requires_all_available_app_and_host_anchors() {
        let mut mapping = sample_mapping();
        mapping.bundle_id = Some("com.example.browser".into());
        mapping.browser_host = Some("mail.example.com".into());
        mapping.dictionary_learn_enabled = false;

        assert!(!scene_allows_learn(
            &[mapping.clone()],
            "browser.unknown",
            Some("com.example.browser"),
            Some("mail.example.com")
        ));
        assert!(scene_allows_learn(
            &[mapping.clone()],
            "browser.unknown",
            Some("com.example.other-browser"),
            Some("mail.example.com")
        ));
        assert!(scene_allows_learn(
            &[mapping],
            "browser.unknown",
            Some("com.example.browser"),
            Some("docs.example.com")
        ));
    }

    #[test]
    fn coarse_learning_fallback_does_not_ignore_path_or_focus_selectors() {
        let mut mapping = sample_mapping();
        mapping.bundle_id = Some("com.example.browser".into());
        mapping.browser_host = Some("mail.example.com".into());
        mapping.browser_path_prefix = Some("/inbox".into());
        mapping.focused_field = Some(crate::context::FocusKind::Email);
        mapping.dictionary_learn_enabled = false;

        assert!(scene_allows_learn(
            &[mapping.clone()],
            "browser.unknown",
            Some("com.example.browser"),
            Some("mail.example.com")
        ));
        assert!(!scene_allows_learn(
            &[mapping],
            "user.wechat",
            Some("com.example.browser"),
            Some("mail.example.com")
        ));
    }

    #[test]
    fn mapping_can_disable_learning() {
        let mut mapping = AppMapping {
            id: "wechat".into(),
            label: "微信".into(),
            family: ContextFamily::PersonalChat,
            writing_task_id: None,
            writing_preferences: Vec::new(),
            preference_guidance_provider_approved: false,
            preference_revision: 0,
            deleted: false,
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

    #[test]
    fn fuzzy_dictionary_uses_shared_quote_and_fence_literals() {
        let dictionary = vec!["TypeScript".into()];
        let input = "type script \"type script\"\n~~~text\ntype script\n~~~\ntype script";
        assert_eq!(
            apply_ascii_fuzzy_dictionary(input, &dictionary),
            "TypeScript \"type script\"\n~~~text\ntype script\n~~~\nTypeScript"
        );
    }

    #[test]
    fn explicit_pair_slicing_is_safe_for_multibyte_line_endings() {
        let pairs = vec![LexiconPair::new("demo", "Demo")];
        for input in ["中文。", "日本語です。", "你好，世界！"] {
            let (output, _, _) = apply_explicit_replacements_with_facts(input, &pairs, &[]);
            assert_eq!(output, input);
        }
    }

    #[test]
    fn dashscope_shape_populates_instant_vocabulary_keywords() {
        let bundle = build_asr_prompt_bundle_with_permissions_and_language(
            &["VoiceFlow".into(), "Claude Code".into()],
            None,
            &[],
            None,
            AsrPromptShape::InstantVocabulary,
            None,
            crate::context::ContextSourcePermissions::default(),
            Some("auto"),
            &[],
        );
        assert!(bundle.prompt.is_none());
        assert_eq!(bundle.keywords, vec!["Claude Code", "VoiceFlow"]);
    }

    #[test]
    fn seed_prompt_respects_language_and_preferred_languages() {
        let dict = vec!["VoiceFlow".to_owned()];
        let auto_unbiased = build_asr_prompt_bundle_with_permissions_and_language(
            &dict,
            None,
            &[],
            None,
            AsrPromptShape::WhisperTranscript,
            None,
            crate::context::ContextSourcePermissions::default(),
            Some("auto"),
            &[],
        );
        assert_eq!(
            auto_unbiased.prompt.as_deref(),
            Some("VoiceFlow"),
            "unbiased auto session must emit bare dictionary terms without Chinese or English glue: {:?}",
            auto_unbiased.prompt
        );

        let english_only = build_asr_prompt_bundle_with_permissions_and_language(
            &dict,
            None,
            &[],
            None,
            AsrPromptShape::WhisperTranscript,
            None,
            crate::context::ContextSourcePermissions::default(),
            Some("en"),
            &[],
        );
        assert_eq!(
            english_only.prompt.as_deref(),
            Some("VoiceFlow"),
            "English session must emit bare dictionary terms without Chinese or English glue: {:?}",
            english_only.prompt
        );

        let zh_preferred = build_asr_prompt_bundle_with_permissions_and_language(
            &dict,
            None,
            &[],
            None,
            AsrPromptShape::WhisperTranscript,
            None,
            crate::context::ContextSourcePermissions::default(),
            Some("auto"),
            &["zh".into(), "en".into()],
        );
        assert!(
            zh_preferred
                .prompt
                .as_deref()
                .is_some_and(|p| p.contains("今天下午在开会")),
            "zh-preferred session should keep Chinese Whisper seed: {:?}",
            zh_preferred.prompt
        );
    }

    #[test]
    fn confirmed_global_rule_gets_legacy_heuristics_while_scoped_rule_stays_alias_only() {
        let dir = std::env::temp_dir().join(format!(
            "voiceflow-lexicon-global-policy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let global_scope = crate::store::LearnPairScope::default();
        let global_key = crate::store::scoped_word_rule_key(
            "知呼",
            "知乎",
            &global_scope,
            crate::store::word_rule_match_policy("知呼", "知乎"),
        )
        .into_string();
        let global_row =
            crate::store::ensure_manual_global_learn_pair(&dir, &global_key, "知呼", "知乎")
                .unwrap();
        let global_pair = LexiconPair::from_record(&global_row);
        assert_eq!(
            global_pair.expansion_policy,
            PairExpansionPolicy::LegacyHeuristics
        );

        let local_scope = crate::store::LearnPairScope {
            family: None,
            mapping_id: None,
            browser_host: Some("notes.example.com".into()),
            native_bundle: None,
        };
        let local_key = crate::store::scoped_word_rule_key(
            "知呼",
            "知乎",
            &local_scope,
            crate::store::word_rule_match_policy("知呼", "知乎"),
        )
        .into_string();
        let local_row = crate::store::ensure_manual_scoped_learn_pair(
            &dir,
            &local_key,
            "知呼",
            "知乎",
            &local_scope,
        )
        .unwrap();
        let local_pair = LexiconPair::from_record(&local_row);
        assert_eq!(local_pair.expansion_policy, PairExpansionPolicy::AliasOnly);
        let _ = std::fs::remove_dir_all(dir);
    }
}
