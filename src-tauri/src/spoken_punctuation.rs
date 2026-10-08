//! Spoken Chinese punctuation tokens applied to ASR text before cleanup.

use crate::context::ContextFamily;
use crate::dictionary_learn::is_cjk;

const FIXED_TOKENS: &[(&str, &str)] = &[
    ("左括号", "（"),
    ("右括号", "）"),
    ("顿号", "、"),
    ("斜杠", "/"),
    ("逗号", "，"),
    ("句号", "。"),
    ("问号", "？"),
    ("感叹号", "！"),
    ("破折号", "——"),
    ("冒号", "："),
    ("分号", "；"),
    ("question mark", "?"),
    ("exclamation point", "!"),
    ("exclamation mark", "!"),
    ("comma", ","),
    ("period", "."),
    ("semicolon", ";"),
    ("colon", ":"),
    ("dash", "—"),
];

struct TokenMatch {
    start: usize,
    end: usize,
    from: &'static str,
    to: &'static str,
}

pub(crate) struct AppliedPunctuation {
    pub(crate) text: String,
    pub(crate) consumed_syntax_ranges: Vec<std::ops::Range<usize>>,
}

/// Replace standalone spoken punctuation tokens. Surrounding chat is left as-is.
#[cfg(test)]
pub fn apply(text: &str) -> String {
    apply_with_provenance(text).text
}

pub(crate) fn apply_with_provenance(text: &str) -> AppliedPunctuation {
    let code_ranges = crate::spoken_layout::code_literal_ranges(text);
    let matches = standalone_matches(text, &code_ranges);
    let quote_count = matches.iter().filter(|item| item.from == "引号").count();
    let drop_quotes = quote_count % 2 == 1;
    let mut quote_open = true;
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut consumed_syntax_ranges = Vec::new();
    for item in matches {
        if item.from == "引号" && drop_quotes {
            continue;
        }
        out.push_str(&text[last..item.start]);
        if item.from == "引号" {
            out.push_str(if quote_open { "「" } else { "」" });
            quote_open = !quote_open;
            consumed_syntax_ranges.push(item.start..item.end);
            last = item.end;
        } else {
            while out.ends_with(char::is_whitespace) {
                out.pop();
            }
            out.push_str(item.to);
            consumed_syntax_ranges.push(item.start..item.end);
            last = if item.from.is_ascii() {
                item.end
            } else {
                skip_leading_whitespace(text, item.end)
            };
        }
    }
    out.push_str(&text[last..]);
    AppliedPunctuation {
        text: out,
        consumed_syntax_ranges,
    }
}

/// Add a light terminal mark on local-only cleanup. Skip command-like scenes.
pub fn ensure_terminal(text: &str, family: ContextFamily) -> String {
    if matches!(
        family,
        ContextFamily::Terminal
            | ContextFamily::BrowserSearch
            | ContextFamily::FormFilling
            | ContextFamily::PromptOrCode
    ) {
        return text.to_owned();
    }
    let leading_end = text.len() - text.trim_start().len();
    let content_end = text.trim_end().len();
    if content_end <= leading_end {
        return text.to_owned();
    }
    let content = &text[leading_end..content_end];
    if has_terminal_mark(content) {
        return text.to_owned();
    }
    let mark = if is_question(content) {
        if cjk_count(content) > 0 {
            "？"
        } else {
            "?"
        }
    } else if cjk_count(content) >= 4 && cjk_count(content) >= latin_letter_count(content) {
        "。"
    } else if looks_like_latin_sentence(content) {
        "."
    } else {
        return text.to_owned();
    };
    format!(
        "{}{content}{mark}{}",
        &text[..leading_end],
        &text[content_end..]
    )
}

fn has_terminal_mark(text: &str) -> bool {
    text.ends_with(['。', '！', '？', '.', '!', '?', '…'])
}

fn is_question(text: &str) -> bool {
    let not_a_me_question =
        text.ends_with("没什么") || text.ends_with("那么") || text.ends_with("要么");
    if !not_a_me_question
        && (text.ends_with('吗')
            || text.ends_with(['呢', '嘛'])
            || text.ends_with("什么")
            || text.ends_with("怎么")
            || text.ends_with("为什么")
            || text.ends_with("干什么"))
    {
        return true;
    }
    let lower = text.to_ascii_lowercase();
    let do_not_imperative = lower == "do not" || lower.starts_with("do not ");
    lower.starts_with("what ")
        || lower.starts_with("why ")
        || lower.starts_with("how ")
        || lower.starts_with("when ")
        || lower.starts_with("where ")
        || lower.starts_with("who ")
        || lower.starts_with("is ")
        || lower.starts_with("are ")
        || (!do_not_imperative && lower.starts_with("do "))
        || lower.starts_with("does ")
        || lower.starts_with("can ")
        || lower.starts_with("could ")
        || lower.starts_with("would ")
        || lower.starts_with("will ")
}

fn cjk_count(text: &str) -> usize {
    text.chars().filter(|ch| is_cjk(*ch)).count()
}

fn latin_letter_count(text: &str) -> usize {
    text.chars().filter(|ch| ch.is_ascii_alphabetic()).count()
}

fn looks_like_latin_sentence(text: &str) -> bool {
    let words = text
        .split_whitespace()
        .filter(|word| word.chars().any(|ch| ch.is_ascii_alphabetic()))
        .count();
    words >= 5
        && text
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase())
}

fn skip_leading_whitespace(text: &str, start: usize) -> usize {
    let mut index = start;
    for ch in text[start..].chars() {
        if !ch.is_whitespace() {
            break;
        }
        index += ch.len_utf8();
    }
    index
}

fn standalone_matches(text: &str, code_ranges: &[std::ops::Range<usize>]) -> Vec<TokenMatch> {
    if contains_spoken_url_prefix(text) {
        // Without a complete URL parser, changing one spoken delimiter can
        // leave a partly punctuated literal that is harder to recognize.
        return Vec::new();
    }
    let mut matches = Vec::new();
    let mut index = 0;
    while index < text.len() {
        if is_in_literal(text, index, code_ranges) {
            index += text[index..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        if let Some(found) = match_token_at(text, index, "引号", "") {
            index = found.end;
            matches.push(found);
            continue;
        }
        let mut found = None;
        for &(from, to) in FIXED_TOKENS {
            if let Some(item) = match_token_at(text, index, from, to) {
                found = Some(item);
                break;
            }
        }
        if let Some(found) = found {
            index = found.end;
            matches.push(found);
            continue;
        }
        let ch = text[index..].chars().next().expect("rest is non-empty");
        index += ch.len_utf8();
    }
    matches
}

fn is_in_literal(text: &str, index: usize, code_ranges: &[std::ops::Range<usize>]) -> bool {
    crate::spoken_layout::is_inside_explicit_quote(text, index)
        || code_ranges
            .iter()
            .any(|range| range.start <= index && index < range.end)
}

fn contains_spoken_url_prefix(text: &str) -> bool {
    let words = text
        .split_whitespace()
        .map(|word| {
            word.trim_matches(|ch: char| ch.is_ascii_punctuation())
                .to_ascii_lowercase()
        })
        .collect::<Vec<_>>();
    words.windows(4).any(|tokens| {
        matches!(tokens[0].as_str(), "http" | "https")
            && tokens[1] == "colon"
            && tokens[2] == "slash"
            && tokens[3] == "slash"
    })
}

fn match_token_at(
    text: &str,
    start: usize,
    from: &'static str,
    to: &'static str,
) -> Option<TokenMatch> {
    if !text.is_char_boundary(start) {
        return None;
    }
    let rest = &text[start..];
    let matched = if from.is_ascii() {
        rest.len() >= from.len()
            && rest.is_char_boundary(from.len())
            && rest[..from.len()].eq_ignore_ascii_case(from)
    } else {
        rest.starts_with(from)
    };
    if !matched {
        return None;
    }
    let end = start + from.len();
    if precedes_classifier_ge(text, start) {
        return None;
    }
    if from.is_ascii() {
        if !is_ascii_word_left(text, start) || !is_ascii_word_right(text, end) {
            return None;
        }
        if from.eq_ignore_ascii_case("period") && period_is_false_positive(text, start, end) {
            return None;
        }
    } else if !is_left_boundary(text, start) || !is_right_boundary(text, end) {
        return None;
    }
    Some(TokenMatch {
        start,
        end,
        from,
        to,
    })
}

fn is_ascii_word_left(text: &str, start: usize) -> bool {
    text[..start]
        .chars()
        .next_back()
        .map(|ch| !ch.is_ascii_alphanumeric())
        .unwrap_or(true)
}

fn is_ascii_word_right(text: &str, end: usize) -> bool {
    text[end..]
        .chars()
        .next()
        .map(|ch| !ch.is_ascii_alphanumeric())
        .unwrap_or(true)
}

fn previous_ascii_word(text: &str, start: usize) -> Option<&str> {
    let before = text[..start].trim_end();
    let word_start = before
        .char_indices()
        .rev()
        .find(|(_, ch)| !ch.is_ascii_alphabetic())
        .map(|(index, ch)| index + ch.len_utf8())
        .unwrap_or(0);
    let word = before.get(word_start..)?;
    (!word.is_empty() && word.chars().all(|ch| ch.is_ascii_alphabetic())).then_some(word)
}

fn next_ascii_word(text: &str, start: usize) -> Option<&str> {
    let rest = text[start..].trim_start();
    let end = rest
        .find(|ch: char| !ch.is_ascii_alphabetic())
        .unwrap_or(rest.len());
    let word = rest.get(..end)?;
    (!word.is_empty()).then_some(word)
}

fn period_is_false_positive(text: &str, start: usize, end: usize) -> bool {
    const BEFORE: &[&str] = &["time", "trial", "grace", "notice", "waiting", "class"];
    const AFTER: &[&str] = &["of", "in", "from"];
    const DETERMINERS: &[&str] = &["this", "that", "the", "a", "an"];
    if previous_ascii_word(text, start)
        .is_some_and(|word| BEFORE.iter().any(|item| word.eq_ignore_ascii_case(item)))
    {
        return true;
    }
    if next_ascii_word(text, end)
        .is_some_and(|word| AFTER.iter().any(|item| word.eq_ignore_ascii_case(item)))
    {
        return true;
    }
    if previous_ascii_word(text, start).is_some_and(|word| {
        DETERMINERS
            .iter()
            .any(|item| word.eq_ignore_ascii_case(item))
    }) {
        return match next_ascii_word(text, end) {
            None => false,
            Some(word) => word.chars().next().is_some_and(|ch| !ch.is_uppercase()),
        };
    }
    false
}

fn is_left_boundary(text: &str, start: usize) -> bool {
    text[..start]
        .chars()
        .next_back()
        .map(is_boundary_char)
        .unwrap_or(true)
}

fn is_right_boundary(text: &str, end: usize) -> bool {
    text[end..]
        .chars()
        .next()
        .map(is_boundary_char)
        .unwrap_or(true)
}

fn precedes_classifier_ge(text: &str, start: usize) -> bool {
    text[..start].ends_with('个')
}

fn is_cjk_letter(value: char) -> bool {
    matches!(
        value,
        '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}'
    )
}

fn is_boundary_char(value: char) -> bool {
    if value.is_whitespace() || value.is_ascii_punctuation() || is_cjk_letter(value) {
        return true;
    }
    matches!(
        value,
        '\u{3000}'..='\u{303F}'
            | '\u{FF01}'..='\u{FF0F}'
            | '\u{FF1A}'..='\u{FF20}'
            | '\u{FF3B}'..='\u{FF40}'
            | '\u{FF5B}'..='\u{FF65}'
    )
}

#[cfg(test)]
mod tests {
    use super::{apply, ensure_terminal};
    use crate::context::ContextFamily;

    #[test]
    fn maps_standalone_spoken_punctuation() {
        assert_eq!(apply("左括号"), "（");
        assert_eq!(apply("右括号"), "）");
        assert_eq!(apply("顿号"), "、");
        assert_eq!(apply("斜杠"), "/");
        assert_eq!(apply("逗号"), "，");
        assert_eq!(apply("句号"), "。");
        assert_eq!(apply("问号"), "？");
        assert_eq!(apply("感叹号"), "！");
    }

    #[test]
    fn pairs_quotes_opening_then_closing() {
        assert_eq!(apply("引号 hello 引号"), "「 hello 」");
        assert_eq!(apply("引号 引号"), "「 」");
    }

    #[test]
    fn unpaired_quote_is_left_unchanged() {
        assert_eq!(apply("引号"), "引号");
        assert_eq!(apply("请说 引号"), "请说 引号");
    }

    #[test]
    fn leaves_embedded_punctuation_words_unchanged() {
        assert_eq!(apply("画个句号"), "画个句号");
        assert_eq!(apply("打个问号"), "打个问号");
        assert_eq!(apply("这个逗号"), "这个逗号");
        assert_eq!(apply("括号里"), "括号里");
        assert_eq!(apply("写在括号里"), "写在括号里");
    }

    #[test]
    fn does_not_rewrite_surrounding_chat() {
        assert_eq!(apply("今晚吃饭吗"), "今晚吃饭吗");
        assert_eq!(apply("嗯就这样吧哈哈"), "嗯就这样吧哈哈");
        assert_eq!(apply("你好 逗号 还好吗"), "你好，还好吗");
    }

    #[test]
    fn maps_unspaced_chinese_spoken_punctuation() {
        assert_eq!(apply("你好逗号还好吗"), "你好，还好吗");
        assert_eq!(apply("先这样句号明天见"), "先这样。明天见");
    }

    #[test]
    fn maps_english_spoken_punctuation_commands() {
        assert_eq!(
            apply("reading club period tomorrow"),
            "reading club. tomorrow"
        );
        assert_eq!(apply("are you coming question mark"), "are you coming?");
        assert_eq!(apply("watch out exclamation point"), "watch out!");
        assert_eq!(apply("watch out exclamation mark"), "watch out!");
        assert_eq!(apply("hello comma world"), "hello, world");
        assert_eq!(apply("meeting colon agenda"), "meeting: agenda");
        assert_eq!(apply("risk semicolon dependency"), "risk; dependency");
        assert_eq!(apply("pause dash continue"), "pause— continue");
    }

    #[test]
    fn preserves_spoken_url_delimiters_without_partial_conversion() {
        let input =
            "Open https colon slash slash status dot example dot net slash incident slash 42";
        assert_eq!(apply(input), input);
        assert_eq!(
            crate::prepare_spoken_transcript(input, ContextFamily::General, 0.9),
            input
        );
    }

    #[test]
    fn maps_standalone_colon_semicolon_and_dash() {
        assert_eq!(apply("会议纪要冒号预算"), "会议纪要：预算");
        assert_eq!(apply("风险分号依赖"), "风险；依赖");
        assert_eq!(apply("停顿破折号继续"), "停顿——继续");
        assert_eq!(apply("画个冒号"), "画个冒号");
        assert_eq!(apply("colonial rule"), "colonial rule");
    }

    #[test]
    fn leaves_period_of_time_unchanged() {
        assert_eq!(apply("a period of time"), "a period of time");
        assert_eq!(apply("the trial period ended"), "the trial period ended");
        assert_eq!(
            apply("during this period we should wait"),
            "during this period we should wait"
        );
    }

    #[test]
    fn adds_light_terminal_punctuation_except_command_scenes() {
        assert_eq!(
            ensure_terminal("今晚吃饭吗", ContextFamily::PersonalChat),
            "今晚吃饭吗？"
        );
        assert_eq!(
            ensure_terminal("我晚点回你", ContextFamily::PersonalChat),
            "我晚点回你。"
        );
        assert_eq!(ensure_terminal("hello", ContextFamily::General), "hello");
        assert_eq!(ensure_terminal("ls -la", ContextFamily::Terminal), "ls -la");
        assert_eq!(
            ensure_terminal("今晚吃饭吗。", ContextFamily::PersonalChat),
            "今晚吃饭吗。"
        );
        assert_eq!(
            ensure_terminal("没什么", ContextFamily::PersonalChat),
            "没什么"
        );
        assert_eq!(ensure_terminal("那么", ContextFamily::WorkChat), "那么");
        assert_eq!(ensure_terminal("要么", ContextFamily::General), "要么");
        assert_eq!(
            ensure_terminal("为什么", ContextFamily::PersonalChat),
            "为什么？"
        );
        assert_eq!(
            ensure_terminal("怎么", ContextFamily::PersonalChat),
            "怎么？"
        );
        assert_eq!(
            ensure_terminal("什么", ContextFamily::PersonalChat),
            "什么？"
        );
        assert_eq!(
            ensure_terminal("  你好世界\n", ContextFamily::PersonalChat),
            "  你好世界。\n"
        );
    }

    #[test]
    fn local_terminal_mark_keeps_do_not_imperatives_as_statements() {
        assert_eq!(
            ensure_terminal(
                "Do not reopen the incident until the owner replies",
                ContextFamily::WorkChat,
            ),
            "Do not reopen the incident until the owner replies."
        );
        assert_eq!(
            ensure_terminal("Do not send $900 to Maya", ContextFamily::PersonalChat),
            "Do not send $900 to Maya."
        );
        assert_eq!(
            ensure_terminal("Do you have the notes", ContextFamily::WorkChat),
            "Do you have the notes?"
        );
        assert_eq!(
            ensure_terminal("Do you have the notes?", ContextFamily::WorkChat),
            "Do you have the notes?"
        );
    }
}
