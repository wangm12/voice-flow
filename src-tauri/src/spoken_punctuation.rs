//! Spoken Chinese punctuation tokens applied to ASR text before cleanup.

const FIXED_TOKENS: &[(&str, &str)] = &[
    ("左括号", "（"),
    ("右括号", "）"),
    ("顿号", "、"),
    ("斜杠", "/"),
    ("逗号", "，"),
    ("句号", "。"),
    ("问号", "？"),
    ("感叹号", "！"),
    ("question mark", "?"),
    ("exclamation point", "!"),
    ("exclamation mark", "!"),
    ("comma", ","),
    ("period", "."),
];

struct TokenMatch {
    start: usize,
    end: usize,
    from: &'static str,
    to: &'static str,
}

/// Replace standalone spoken punctuation tokens. Surrounding chat is left as-is.
pub fn apply(text: &str) -> String {
    let matches = standalone_matches(text);
    let quote_count = matches.iter().filter(|item| item.from == "引号").count();
    let drop_quotes = quote_count % 2 == 1;
    let mut quote_open = true;
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for item in matches {
        if item.from == "引号" && drop_quotes {
            continue;
        }
        out.push_str(&text[last..item.start]);
        if item.from == "引号" {
            out.push_str(if quote_open { "「" } else { "」" });
            quote_open = !quote_open;
            last = item.end;
        } else {
            while out.ends_with(char::is_whitespace) {
                out.pop();
            }
            out.push_str(item.to);
            last = if item.from.is_ascii() {
                item.end
            } else {
                skip_leading_whitespace(text, item.end)
            };
        }
    }
    out.push_str(&text[last..]);
    out
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

fn standalone_matches(text: &str) -> Vec<TokenMatch> {
    let mut matches = Vec::new();
    let mut index = 0;
    while index < text.len() {
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
    if previous_ascii_word(text, start)
        .is_some_and(|word| DETERMINERS.iter().any(|item| word.eq_ignore_ascii_case(item)))
    {
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
    text[..start].chars().next_back() == Some('个')
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
    use super::apply;

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
}
