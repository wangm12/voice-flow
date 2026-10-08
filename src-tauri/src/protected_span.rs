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
    if should_preserve_deictic_references(operation)
        && deictic_reference_signature(source) != deictic_reference_signature(candidate)
    {
        return false;
    }
    let source_spans = protected_spans(source, operation);
    let candidate_spans = protected_spans(candidate, operation);
    let mut candidate_index = 0;
    for source_span in source_spans {
        let source_token = &source[source_span.start_byte..source_span.end_byte];
        let found = candidate_spans[candidate_index..]
            .iter()
            .position(|candidate_span| {
                if candidate_span.kind != source_span.kind {
                    return false;
                }
                let candidate_token =
                    &candidate[candidate_span.start_byte..candidate_span.end_byte];
                if source_span.kind == ProtectedSpanKind::DeicticReference {
                    source_token.eq_ignore_ascii_case(candidate_token)
                } else {
                    candidate_token == source_token
                }
            });
        let Some(relative) = found else {
            return false;
        };
        candidate_index += relative + 1;
    }
    preserves_negation(source, candidate, operation)
        && (operation != Some(CleanupOperation::Translate)
            || weekday_counts(source) == weekday_counts(candidate))
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
    negation_count(source) == negation_count(candidate)
}

fn negation_count(text: &str) -> usize {
    const NEGATIONS: &[&str] = &[
        "cannot",
        "shouldn't",
        "wouldn't",
        "couldn't",
        "doesn't",
        "don't",
        "didn't",
        "isn't",
        "aren't",
        "wasn't",
        "weren't",
        "won't",
        "mustn't",
        "without",
        "neither",
        "nobody",
        "nothing",
        "never",
        "none",
        "not",
        "no",
        "禁止",
        "并非",
        "不要",
        "不能",
        "不会",
        "没有",
        "不是",
        "未曾",
        "未",
        "别",
        "没",
        "不",
    ];
    // Fold English casing and common curly apostrophes before checking count.
    // Equal counts are a retention proxy, not semantic proof.
    let normalized = text.to_ascii_lowercase().replace(['’', '‘', 'ʼ'], "'");
    let text = normalized.as_str();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for marker in NEGATIONS {
        for (start, _) in text.match_indices(marker) {
            let end = start + marker.len();
            let ascii_marker = marker.is_ascii();
            let before = text[..start].chars().next_back();
            let after = text[end..].chars().next();
            if ascii_marker
                && (before.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                    || after.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_'))
            {
                continue;
            }
            ranges.push((start, end));
        }
    }
    ranges.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| right.1.cmp(&left.1)));
    let mut selected: Vec<(usize, usize)> = Vec::new();
    for range in ranges {
        if !selected.last().is_some_and(|previous| range.0 < previous.1) {
            selected.push(range);
        }
    }
    selected.len()
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
    for (start, token) in ascii_tokens(text) {
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
        if ch.is_ascii_digit() {
            let (start, mut end) = if index > 0
                && matches!(text.as_bytes()[index - 1], b'+' | b'-')
                && (index == 1
                    || !text[..index - 1]
                        .chars()
                        .next_back()
                        .is_some_and(|previous| {
                            previous.is_ascii_alphanumeric() || matches!(previous, '_' | '.' | ',')
                        })) {
                (index - 1, number_end(text, index))
            } else {
                (index, number_end(text, index))
            };
            if text[end..].starts_with('%') {
                end += 1;
            }
            result.push((start, end));
            index = end;
        } else {
            index += ch.len_utf8();
        }
    }
    result
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
        assert!(preserves("不要上线", "不要上线。", None));
        assert!(preserves("say no to this", "say no to this", None));
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
}
