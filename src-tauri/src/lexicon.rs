//! Local lexicon replace, ASR ranking, cleanup hit-pairs, and cleanup effort.

use crate::context::{AppMapping, ContextFamily, ContextPolicy, ContextSnapshot};
use crate::dictionary_learn::{is_cjk, is_latin_cont, is_latin_start, pair_key};
use crate::llm::{CleanupEffort, CleanupIntent, IntentSource};
use crate::store::LearnPairRecord;
use std::collections::HashSet;

pub const MAX_ASR_PROMPT_TOKENS: usize = 200;
const ASR_PREFIX: &str = "不要翻译: ";
const ASR_MIXED_LANGUAGE_SEED: &str = "不要翻译。这个 API 的 latency 太高了。";
const MAX_CLEANUP_PAIRS: usize = 8;
const MAX_CLEANUP_PAIR_CHARS: usize = 400;

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
        let inferred_family = family
            .map(str::to_owned)
            .or_else(|| {
                profile_id.map(|id| crate::context::family_id(family_from_profile_id(id, &[])).to_owned())
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
    mapping: Option<&AppMapping>,
    family: ContextFamily,
    intent: &CleanupIntent,
    profile_confidence: f32,
) -> CleanupRoute {
    if !settings_cleanup_enabled {
        return CleanupRoute::LocalOnly;
    }
    if mapping.is_some_and(|item| !item.cleanup_enabled) {
        return CleanupRoute::LocalOnly;
    }
    if intent.source == IntentSource::SpokenCommand {
        return CleanupRoute::Provider(CleanupEffort::Command);
    }
    if intent.content.trim().is_empty() {
        return CleanupRoute::LocalOnly;
    }
    if let Some(effort) = mapping.and_then(|item| item.cleanup_effort) {
        return CleanupRoute::Provider(effort);
    }
    if skips_llm_scene(family) {
        return CleanupRoute::LocalOnly;
    }
    if profile_confidence < 0.75 {
        return CleanupRoute::Provider(CleanupEffort::Light);
    }
    CleanupRoute::Provider(CleanupEffort::default_for_family(family))
}

fn skips_llm_scene(family: ContextFamily) -> bool {
    matches!(
        family,
        ContextFamily::FormFilling | ContextFamily::Terminal
    )
}

pub fn mapping_for_profile<'a>(
    mappings: &'a [AppMapping],
    profile_id: &str,
) -> Option<&'a AppMapping> {
    profile_id
        .strip_prefix("user.")
        .and_then(|id| mappings.iter().find(|mapping| mapping.id == id))
}

pub fn mapping_allows_learn(mappings: &[AppMapping], profile_id: &str) -> bool {
    mapping_for_profile(mappings, profile_id)
        .map(|mapping| mapping.dictionary_learn_enabled)
        .unwrap_or(true)
}

pub fn apply_lexicon_replacements(
    text: &str,
    pairs: &[LexiconPair],
    blocking: &[String],
) -> String {
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
            index += rule.before.chars().count();
        } else {
            output.push(chars[index]);
            index += 1;
        }
    }
    output
}

pub fn replaceable_pairs<'a>(
    pairs: &'a [LearnPairRecord],
    dictionary: &[String],
) -> Vec<LexiconPair> {
    let dict: HashSet<&str> = dictionary.iter().map(String::as_str).collect();
    pairs
        .iter()
        .filter(|row| row.is_live_promoted() && dict.contains(row.after_surface.as_str()))
        .filter(|row| !row.before_surface.is_empty())
        .map(|row| LexiconPair::new(&row.before_surface, &row.after_surface))
        .collect()
}

pub fn apply_promoted_replacements(
    text: &str,
    pairs: &[LearnPairRecord],
    dictionary: &[String],
) -> String {
    let replaceable = replaceable_pairs(pairs, dictionary);
    apply_lexicon_replacements(text, &replaceable, dictionary)
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

    for word in dictionary.iter().map(|item| item.trim()).filter(|item| !item.is_empty())
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

pub fn build_asr_prompt(
    dictionary: &[String],
    _policy: Option<&ContextPolicy>,
    pairs: &[LearnPairRecord],
    scope: Option<&PromptScope>,
) -> Option<String> {
    let ranked = collect_ranked_terms(dictionary, pairs, scope);
    let prefix_tokens = estimate_prompt_tokens(ASR_PREFIX);
    let seed_tokens = estimate_prompt_tokens(ASR_MIXED_LANGUAGE_SEED);
    let budget = MAX_ASR_PROMPT_TOKENS
        .saturating_sub(prefix_tokens)
        .saturating_sub(seed_tokens.saturating_add(1));
    if budget == 0 {
        return Some(ASR_MIXED_LANGUAGE_SEED.to_owned());
    }

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

    let hints: Vec<String> = kept.into_iter().map(|term| term.term).collect();
    if hints.is_empty() {
        return Some(ASR_MIXED_LANGUAGE_SEED.to_owned());
    }
    Some(format!(
        "{ASR_PREFIX}{} {ASR_MIXED_LANGUAGE_SEED}",
        hints.join(", ")
    ))
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
            if is_latin_start(*left) || is_latin_cont(*left) || is_latin_start(*right) || is_latin_cont(*right)
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
    value.chars().any(|ch| is_latin_start(ch) || is_latin_cont(ch))
        && !value.chars().any(is_cjk)
}

fn is_cjk_surface(value: &str) -> bool {
    value.chars().any(is_cjk)
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
    use crate::llm::CleanupIntent;

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
        }
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
            pairs.push(live_pair(
                &format!("微{index}"),
                &after,
                "personal_chat",
                9,
            ));
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
    fn asr_prompt_ends_with_mixed_language_seed_and_never_english_only() {
        let policy = ContextPolicy {
            preserve_technical_tokens: true,
            ..ContextPolicy::default()
        };
        let prompt = build_asr_prompt(&["VoiceFlow".into()], Some(&policy), &[], None).unwrap();
        assert!(prompt.contains("不要翻译"));
        assert!(
            prompt.ends_with("这个 API 的 latency 太高了。"),
            "Whisper continues from the prompt tail: {prompt}"
        );
        assert!(
            !prompt.ends_with("versions"),
            "English technical clause must not be the decoder prefix: {prompt}"
        );
        assert!(prompt.contains("VoiceFlow"));

        let empty = build_asr_prompt(&[], None, &[], None).unwrap();
        assert!(empty.ends_with("这个 API 的 latency 太高了。"));
        assert!(empty.contains("不要翻译"));
    }

    #[test]
    fn decide_cleanup_sends_prose_including_short_chat_to_provider() {
        let short = CleanupIntent::implicit("好的哈哈我晚点回你");
        let long = CleanupIntent::implicit(
            "请帮我看一下这份季度报告里的几个数字然后在周五之前把意见发我",
        );
        assert_eq!(
            decide_cleanup(true, None, ContextFamily::PersonalChat, &short, 0.9),
            CleanupRoute::Provider(CleanupEffort::Light)
        );
        assert_eq!(
            decide_cleanup(true, None, ContextFamily::PersonalChat, &long, 0.9),
            CleanupRoute::Provider(CleanupEffort::Light)
        );
        assert_eq!(
            decide_cleanup(true, None, ContextFamily::Email, &short, 0.9),
            CleanupRoute::Provider(CleanupEffort::Standard)
        );
        assert_eq!(
            decide_cleanup(true, None, ContextFamily::Email, &long, 0.9),
            CleanupRoute::Provider(CleanupEffort::Standard)
        );
        assert_eq!(
            decide_cleanup(true, None, ContextFamily::WorkChat, &long, 0.9),
            CleanupRoute::Provider(CleanupEffort::Light)
        );
        assert_eq!(
            decide_cleanup(true, None, ContextFamily::SocialMedia, &short, 0.9),
            CleanupRoute::Provider(CleanupEffort::Light)
        );
        assert_eq!(
            decide_cleanup(true, None, ContextFamily::BrowserSearch, &short, 0.9),
            CleanupRoute::Provider(CleanupEffort::Standard)
        );
        assert_eq!(
            decide_cleanup(true, None, ContextFamily::Terminal, &long, 0.9),
            CleanupRoute::LocalOnly
        );
        assert_eq!(
            decide_cleanup(true, None, ContextFamily::FormFilling, &short, 0.9),
            CleanupRoute::LocalOnly
        );
        assert_eq!(
            decide_cleanup(true, None, ContextFamily::PromptOrCode, &long, 0.9),
            CleanupRoute::Provider(CleanupEffort::Light)
        );
        assert_eq!(
            decide_cleanup(
                true,
                None,
                ContextFamily::DeveloperCollaboration,
                &long,
                0.9
            ),
            CleanupRoute::Provider(CleanupEffort::Light)
        );
        assert_eq!(
            decide_cleanup(
                true,
                None,
                ContextFamily::PersonalChat,
                &CleanupIntent::implicit("   "),
                0.9
            ),
            CleanupRoute::LocalOnly
        );
        let spoken = CleanupIntent {
            source: IntentSource::SpokenCommand,
            ..CleanupIntent::implicit("改正式")
        };
        assert_eq!(
            decide_cleanup(true, None, ContextFamily::PersonalChat, &spoken, 0.9),
            CleanupRoute::Provider(CleanupEffort::Command)
        );
        assert_eq!(
            decide_cleanup(false, None, ContextFamily::PersonalChat, &long, 0.9),
            CleanupRoute::LocalOnly
        );
        let mapping = AppMapping {
            id: "wechat".into(),
            label: "微信".into(),
            family: ContextFamily::PersonalChat,
            mode_id: None,
            bundle_id: None,
            executable: None,
            browser_host: None,
            style_example_input: None,
            style_example_output: None,
            enabled: true,
            cleanup_effort: Some(CleanupEffort::Standard),
            cleanup_enabled: true,
            dictionary_learn_enabled: true,
        };
        assert_eq!(
            decide_cleanup(true, Some(&mapping), ContextFamily::PersonalChat, &long, 0.9),
            CleanupRoute::Provider(CleanupEffort::Standard)
        );
        let mapping_off = AppMapping {
            cleanup_enabled: false,
            cleanup_effort: None,
            ..mapping
        };
        assert_eq!(
            decide_cleanup(
                true,
                Some(&mapping_off),
                ContextFamily::PersonalChat,
                &long,
                0.9
            ),
            CleanupRoute::LocalOnly
        );
    }

    #[test]
    fn history_personal_chat_still_routes_to_light() {
        let intent = CleanupIntent::implicit(
            "请帮我看一下这份季度报告里的几个数字然后在周五之前把意见发我",
        );
        let scope = PromptScope::from_history(
            Some("chat.personal"),
            Some("personal_chat"),
            None,
        );
        assert_eq!(scope.family.as_deref(), Some("personal_chat"));
        assert_eq!(
            decide_cleanup(
                true,
                None,
                family_from_profile_id("chat.personal", &[]),
                &intent,
                0.9,
            ),
            CleanupRoute::Provider(CleanupEffort::Light)
        );
    }

    #[test]
    fn mapping_can_disable_learning() {
        let mapping = AppMapping {
            id: "wechat".into(),
            label: "微信".into(),
            family: ContextFamily::PersonalChat,
            mode_id: None,
            bundle_id: Some("com.tencent.xinWeChat".into()),
            executable: None,
            browser_host: None,
            style_example_input: None,
            style_example_output: None,
            enabled: true,
            cleanup_effort: None,
            cleanup_enabled: true,
            dictionary_learn_enabled: false,
        };
        assert!(!mapping_allows_learn(&[mapping], "user.wechat"));
    }
}
