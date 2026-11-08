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
    match trailing_numbered_list_item(text, leading_end..content_end) {
        TrailingNumberedListTail::Item(item) => {
            let body = &text[item.body];
            let Some(mark) = list_item_terminal_mark(body) else {
                return text.to_owned();
            };
            let separator_start = item.separator.start;
            let separator_end = item.separator.end;
            let without_separator =
                format!("{}{}", &text[..separator_start], &text[separator_end..]);
            let adjusted_end = without_separator.trim_end().len();
            return format!(
                "{}{mark}{}",
                &without_separator[..adjusted_end],
                &without_separator[adjusted_end..]
            );
        }
        TrailingNumberedListTail::Abstain => return text.to_owned(),
        TrailingNumberedListTail::NotList => {}
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

struct TrailingNumberedListItem {
    body: std::ops::Range<usize>,
    separator: std::ops::Range<usize>,
}

enum TrailingNumberedListTail {
    NotList,
    Abstain,
    Item(TrailingNumberedListItem),
}

fn trailing_numbered_list_item(
    text: &str,
    content_range: std::ops::Range<usize>,
) -> TrailingNumberedListTail {
    let content = &text[content_range.clone()];
    if !content
        .chars()
        .next_back()
        .is_some_and(|ch| matches!(ch, ',' | '，'))
    {
        return TrailingNumberedListTail::NotList;
    }
    let literals = crate::spoken_revision::literal_ranges(text);
    let mut last_item_body_start = None;
    let mut saw_protected_list_marker = false;
    let mut offset = content_range.start;
    for line in content.split_inclusive('\n') {
        let line_without_newline = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = line_without_newline.trim_start();
        let indentation = line_without_newline.len() - trimmed.len();
        let digits = trimmed.chars().take_while(|ch| ch.is_ascii_digit()).count();
        let is_numbered_item = digits > 0
            && trimmed
                .get(digits..)
                .is_some_and(|rest| rest.starts_with(". "));
        if is_numbered_item {
            let body_start = offset + indentation + digits + 2;
            let marker = offset + indentation..body_start;
            if literals
                .iter()
                .any(|literal| ranges_overlap(literal, &marker))
            {
                saw_protected_list_marker = true;
            } else {
                last_item_body_start = Some(body_start);
            }
        }
        offset += line.len();
    }
    let Some(body_start) = last_item_body_start else {
        return if saw_protected_list_marker {
            TrailingNumberedListTail::Abstain
        } else {
            TrailingNumberedListTail::NotList
        };
    };
    let Some((separator_offset, separator)) = content.char_indices().next_back() else {
        return TrailingNumberedListTail::Abstain;
    };
    if !matches!(separator, ',' | '，') {
        return TrailingNumberedListTail::Abstain;
    }
    let separator_start = content_range.start + separator_offset;
    let separator_range = separator_start..separator_start + separator.len_utf8();
    let Some(raw_body) = text.get(body_start..separator_start) else {
        return TrailingNumberedListTail::Abstain;
    };
    let mut continuation_offset = body_start;
    let mut is_first_line = true;
    for continuation in raw_body.split_inclusive('\n') {
        if is_first_line {
            is_first_line = false;
            continuation_offset += continuation.len();
            continue;
        }
        let line_without_newline = continuation.strip_suffix('\n').unwrap_or(continuation);
        if !line_without_newline.trim().is_empty() {
            let line_range = continuation_offset..continuation_offset + continuation.len();
            let has_indentation = line_without_newline
                .chars()
                .next()
                .is_some_and(char::is_whitespace);
            let protected_line = literals
                .iter()
                .any(|literal| ranges_overlap(literal, &line_range));
            if !has_indentation && !protected_line {
                return TrailingNumberedListTail::Abstain;
            }
        }
        continuation_offset += continuation.len();
    }
    let body_leading = raw_body.len() - raw_body.trim_start().len();
    let body_end = raw_body.trim_end().len();
    if body_leading >= body_end {
        return TrailingNumberedListTail::Abstain;
    }
    let body_range = body_start + body_leading..body_start + body_end;
    if literals.iter().any(|literal| {
        ranges_overlap(literal, &body_range) || ranges_overlap(literal, &separator_range)
    }) {
        return TrailingNumberedListTail::Abstain;
    }
    TrailingNumberedListTail::Item(TrailingNumberedListItem {
        body: body_range,
        separator: separator_range,
    })
}

fn ranges_overlap(left: &std::ops::Range<usize>, right: &std::ops::Range<usize>) -> bool {
    left.start < right.end && right.start < left.end
}

fn list_item_terminal_mark(body: &str) -> Option<&'static str> {
    if has_terminal_mark(body) || looks_like_numeric_or_technical_literal(body) {
        return None;
    }
    if is_question(body) {
        return Some(if cjk_count(body) > 0 { "？" } else { "?" });
    }
    if cjk_count(body) >= 2 {
        Some("。")
    } else if looks_like_latin_sentence(body) {
        Some(".")
    } else {
        None
    }
}

fn looks_like_numeric_or_technical_literal(text: &str) -> bool {
    let compact: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
    if compact.is_empty() {
        return true;
    }
    let data_label = [
        "金额为",
        "星期",
        "版本",
        "日期",
        "金额",
        "价格",
        "编号",
        "数量",
        "总额",
        "余额",
        "时间",
    ]
    .iter()
    .find(|label| compact.starts_with(*label));
    let candidate = data_label.map_or(compact.as_str(), |label| {
        compact.strip_prefix(*label).unwrap_or(compact.as_str())
    });
    let candidate = candidate.trim_start_matches([':', '：']);
    if data_label.is_some() && candidate.is_empty() {
        return true;
    }
    let numeric_chars_only = candidate.chars().all(|ch| {
        ch.is_ascii_digit()
            || matches!(
                ch,
                ',' | '，'
                    | '.'
                    | '。'
                    | '-'
                    | '－'
                    | '+'
                    | '/'
                    | ':'
                    | '%'
                    | '$'
                    | '¥'
                    | '￥'
                    | '€'
                    | '£'
                    | '零'
                    | '〇'
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
                    | '亿'
                    | '兆'
                    | '年'
                    | '月'
                    | '日'
                    | '号'
                    | '时'
                    | '秒'
                    | '周'
                    | '元'
                    | '圆'
                    | '角'
                    | '分'
            )
    });
    if numeric_chars_only {
        return true;
    }
    let has_digit = candidate.chars().any(|ch| ch.is_ascii_digit());
    let no_spaces = data_label.is_some() || text.chars().all(|ch| !ch.is_whitespace());
    has_digit
        && no_spaces
        && candidate
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ':' | '/' | '+'))
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
    if !from.is_ascii() && follows_spoken_edit_verb(text, start) {
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

fn follows_spoken_edit_verb(text: &str, start: usize) -> bool {
    let before = text[..start].trim_end();
    ["换成", "改成", "删掉", "输入", "把", "用", "加"]
        .iter()
        .any(|verb| before.ends_with(verb))
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
    fn punctuation_words_after_spoken_edit_verbs_remain_literal() {
        for input in [
            "把逗号换成分号",
            "用逗号",
            "加句号",
            "换成问号",
            "改成冒号",
            "删掉顿号",
            "输入感叹号",
        ] {
            assert_eq!(apply(input), input, "{input}");
        }
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
    fn closes_only_the_final_numbered_list_separator_before_local_terminal_punctuation() {
        let chinese = "測試一下,看一下具体的流程是什么,看一下详细的准确度是什么。\n\n1. 看它的稳定性,\n2. 看它的准确度,";
        assert_eq!(
            ensure_terminal(chinese, ContextFamily::General),
            "測試一下,看一下具体的流程是什么,看一下详细的准确度是什么。\n\n1. 看它的稳定性,\n2. 看它的准确度。"
        );
        assert_eq!(
            ensure_terminal(
                "1. Please review the system stability carefully,\n2. Please review the accuracy results carefully,",
                ContextFamily::General
            ),
            "1. Please review the system stability carefully,\n2. Please review the accuracy results carefully."
        );
        assert_eq!(
            ensure_terminal("1. 检查稳定性，\n2. 检查准确度，", ContextFamily::General),
            "1. 检查稳定性，\n2. 检查准确度。"
        );
        assert_eq!(
            ensure_terminal("前言。\r\n\r\n1. 看它的准确度,\r\n", ContextFamily::General,),
            "前言。\r\n\r\n1. 看它的准确度。\r\n"
        );
    }

    #[test]
    fn list_tail_punctuation_preserves_terminal_marks_quotes_and_numeric_literals() {
        let already_terminal = "1. 检查稳定性,\n2. 检查准确度。";
        assert_eq!(
            ensure_terminal(already_terminal, ContextFamily::General),
            already_terminal
        );

        assert_eq!(
            ensure_terminal(
                "1. 保留 \"准确度,\"\n2. 继续检查 \"x,y\",",
                ContextFamily::General,
            ),
            "1. 保留 \"准确度,\"\n2. 继续检查 \"x,y\","
        );
        assert_eq!(
            ensure_terminal(
                "1. 保留金额 $1,234.50 和版本 v2.7,\n2. 保留日期 2026-10-20 和 0xCafe,",
                ContextFamily::General,
            ),
            "1. 保留金额 $1,234.50 和版本 v2.7,\n2. 保留日期 2026-10-20 和 0xCafe。"
        );

        let command = "1. Run `echo \"a,b\"`,\n2. Keep `--format json`,";
        assert_eq!(
            ensure_terminal(command, ContextFamily::PromptOrCode),
            command
        );
    }

    #[test]
    fn numbered_list_tail_uses_only_clear_prose_in_the_final_item() {
        let csv = "1. 1,2,\r\n2. 3,4,";
        assert_eq!(ensure_terminal(csv, ContextFamily::General), csv);

        let amount_only = "中文前言。\n\n1. 已有预算\n2. $1,234.50,";
        assert_eq!(
            ensure_terminal(amount_only, ContextFamily::General),
            amount_only
        );
        for data_tail in [
            "1. 1,234元,",
            "1. 日期 2026年10月20日,",
            "1. 版本 v2.7,",
            "1. 金额：1,234元,",
            "1. 版本：v2.7,",
            "1. 金额 : 1,234元,",
            "1. 版本 ： v2.7,",
        ] {
            assert_eq!(
                ensure_terminal(data_tail, ContextFamily::General),
                data_tail,
                "data-like list item should not inherit punctuation from outside its body"
            );
        }

        assert_eq!(
            ensure_terminal(
                "中文前言。\n\n1. Do you have the results,",
                ContextFamily::General,
            ),
            "中文前言。\n\n1. Do you have the results?"
        );
        assert_eq!(
            ensure_terminal(
                "中文前言。\n\n1. Please review the detailed accuracy results carefully,",
                ContextFamily::General,
            ),
            "中文前言。\n\n1. Please review the detailed accuracy results carefully."
        );

        let empty = "中文前言。\n\n1. ,";
        assert_eq!(ensure_terminal(empty, ContextFamily::General), empty);
    }

    #[test]
    fn numbered_lists_without_trailing_separators_keep_legacy_terminal_fallback() {
        assert_eq!(
            ensure_terminal("1. 检查稳定性\n2. 检查准确度", ContextFamily::General,),
            "1. 检查稳定性\n2. 检查准确度。"
        );
        assert_eq!(
            ensure_terminal(
                "1. 检查稳定性\n2. 检查准确度\n\n还有一件事需要补充",
                ContextFamily::General,
            ),
            "1. 检查稳定性\n2. 检查准确度\n\n还有一件事需要补充。"
        );
        assert_eq!(
            ensure_terminal(
                "1. 检查稳定性\n2. 检查准确度\n\n接下来怎么",
                ContextFamily::General,
            ),
            "1. 检查稳定性\n2. 检查准确度\n\n接下来怎么？"
        );
    }

    #[test]
    fn numbered_list_tail_preserves_literals_and_ambiguous_multiline_items() {
        assert_eq!(
            ensure_terminal(
                "中文前言 \"x,y\" 仍保留。\n\n1. 检查准确度,",
                ContextFamily::General,
            ),
            "中文前言 \"x,y\" 仍保留。\n\n1. 检查准确度。"
        );

        let quoted = "中文前言。\n\n1. Keep \"x,y\",";
        assert_eq!(ensure_terminal(quoted, ContextFamily::General), quoted);

        let fenced = "中文前言。\n\n1. Keep this:\n   ```text\n   3,4\n   ```,";
        assert_eq!(ensure_terminal(fenced, ContextFamily::General), fenced);

        let unclosed_quote = "中文前言。\n\n1. Keep \"an unfinished literal,";
        assert_eq!(
            ensure_terminal(unclosed_quote, ContextFamily::General),
            unclosed_quote
        );

        let unclosed_fence = "中文前言。\n\n1. Keep this:\n```text\n3,4,";
        assert_eq!(
            ensure_terminal(unclosed_fence, ContextFamily::General),
            unclosed_fence
        );
    }

    #[test]
    fn original_row12_native_output_remains_byte_identical() {
        use sha2::{Digest, Sha256};

        let row12 = "详细的情况帮我看一下。然后这个具体的UI为什么变了？\n\n1. 看一下稳定性怎么样。\n2. 看一下它的这个准确率怎么样。";
        assert_eq!(row12.len(), 155);
        assert_eq!(
            hex::encode(Sha256::digest(row12.as_bytes())),
            "5c3e1e7fc6755f54a59a14372f29d7a30e15e0a0ba20fc9bc2f5ebdb7a4be3ff"
        );
        for family in [ContextFamily::PromptOrCode, ContextFamily::General] {
            assert_eq!(ensure_terminal(row12, family).as_bytes(), row12.as_bytes());
        }
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
