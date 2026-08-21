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
        } else {
            out.push_str(item.to);
        }
        last = item.end;
    }
    out.push_str(&text[last..]);
    out
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

fn match_token_at(text: &str, start: usize, from: &'static str, to: &'static str) -> Option<TokenMatch> {
    if !text.is_char_boundary(start) || !text[start..].starts_with(from) {
        return None;
    }
    let end = start + from.len();
    if !is_left_boundary(text, start) || !is_right_boundary(text, end) {
        return None;
    }
    Some(TokenMatch {
        start,
        end,
        from,
        to,
    })
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

fn is_boundary_char(value: char) -> bool {
    if value.is_whitespace() || value.is_ascii_punctuation() {
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
        assert_eq!(apply("你好 逗号 还好吗"), "你好 ， 还好吗");
    }
}
