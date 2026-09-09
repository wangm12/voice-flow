//! Deterministic restatement, hallucination, and repeat cleanup.
//!
//! Runs after spoken punctuation/layout and before lexicon / LLM. Terminal
//! and form-filling stay LocalOnly; prose still needs these markers when the
//! LLM is skipped or fails.

const HALLUCINATIONS: &[&str] = &[
    "thanks for watching",
    "thank you for watching",
    "thanks for listening",
    "thank you for listening",
    "please subscribe",
    "thanks for watching!",
    "sous-titres par",
    "字幕志愿者",
    "请不吝点赞",
    "感谢收看",
    "谢谢观看",
];

const MARKERS: &[&str] = &[
    "scratch that",
    "i mean",
    "no wait",
    "哦,不对",
    "哦，不对",
    "哦不对",
    "我说不对",
    "我说错了",
    "我是说",
    "不对",
];

const REPLACEMENT_REQUIRED_MARKERS: &[&str] = &["i meant", "删掉", "算了"];

#[derive(Clone, Copy)]
struct Protect {
    needle: &'static str,
    token: &'static str,
}

const PROTECT: &[Protect] = &[
    Protect {
        needle: "看它对不对",
        token: "\u{E000}A",
    },
    Protect {
        needle: "你说不对的时候",
        token: "\u{E000}B",
    },
    Protect {
        needle: "我说不对的时候",
        token: "\u{E000}D",
    },
    Protect {
        needle: "对不对",
        token: "\u{E000}C",
    },
];

pub fn apply(text: &str) -> String {
    let protected = protect(text);
    let stripped = strip_hallucinations(&protected);
    let resolved = resolve_all_sentences(&stripped);
    let collapsed = collapse_repeats(&resolved);
    unprotect(&collapsed)
}

fn protect(text: &str) -> String {
    let mut out = text.to_owned();
    for item in PROTECT {
        out = out.replace(item.needle, item.token);
    }
    out
}

fn unprotect(text: &str) -> String {
    let mut out = text.to_owned();
    for item in PROTECT {
        out = out.replace(item.token, item.needle);
    }
    out
}

fn strip_hallucinations(text: &str) -> String {
    let mut kept = Vec::new();
    for sentence in split_sentences(text) {
        let trimmed = sentence.trim();
        if trimmed.is_empty() {
            continue;
        }
        if is_hallucination_text(trimmed) {
            continue;
        }
        kept.push(sentence);
    }
    kept.join("")
}

pub fn is_hallucination_text(text: &str) -> bool {
    let folded = fold_compare(text);
    if folded.is_empty() {
        return false;
    }
    HALLUCINATIONS.iter().any(|phrase| {
        let needle = fold_compare(phrase);
        folded == needle || folded.starts_with(&needle) || folded.ends_with(&needle)
    })
}

fn fold_compare(text: &str) -> String {
    text.chars()
        .filter(|ch| {
            !ch.is_whitespace() && !matches!(*ch, '.' | '!' | '?' | '。' | '！' | '？' | ',' | '，')
        })
        .flat_map(char::to_lowercase)
        .collect()
}

fn resolve_all_sentences(text: &str) -> String {
    let mut kept = Vec::new();
    let mut pending_introducer = false;
    for sentence in split_sentences(text) {
        if pending_introducer {
            let (body, closer) = peel_closer(&sentence);
            let stripped = strip_replacement_introducer(body.trim());
            if is_blank_or_markers(&stripped) {
                continue;
            }
            pending_introducer = false;
            kept.push(attach_closer(&stripped, closer));
            continue;
        }
        let retract = starts_with_correction(&sentence);
        let resolved = resolve_one_sentence(&sentence);
        if retract {
            kept.pop();
        }
        if is_blank_or_markers(&resolved) {
            pending_introducer = retract;
            continue;
        }
        kept.push(resolved);
    }
    kept.join("")
}

fn starts_with_correction(sentence: &str) -> bool {
    let (body, _) = peel_closer(sentence);
    let lower = body.to_ascii_lowercase();
    let start = skip_junk(body, 0);
    match_one_marker(body, &lower, start).is_some()
}

fn resolve_one_sentence(sentence: &str) -> String {
    let (body, closer) = peel_closer(sentence);
    if let Some((_prefix, suffix)) = split_on_last_marker_run(body) {
        if suffix.trim().is_empty() {
            if starts_with_correction(sentence) || !is_glued_content_marker(&_prefix) {
                return String::new();
            }
            return sentence.to_owned();
        }
        return attach_closer(&strip_replacement_introducer(suffix.trim()), closer);
    }
    if let Some(replaced) = apply_contrast_not_a_but_b(body) {
        return attach_closer(&replaced, closer);
    }
    sentence.to_owned()
}

fn is_glued_content_marker(prefix: &str) -> bool {
    let trimmed = prefix.trim_end_matches(|ch: char| {
        ch.is_whitespace() || matches!(ch, '啊' | '呀' | '呢' | '吧' | '嘛')
    });
    if trimmed.ends_with([',', '，', '。', '.', '、', '!', '?', '！', '？']) {
        return false;
    }
    !trimmed.is_empty()
}

fn apply_contrast_not_a_but_b(text: &str) -> Option<String> {
    let mut pos = 0;
    let mut last_shi: Option<usize> = None;
    while let Some(rel) = text[pos..].find("不是") {
        let after_start = pos + rel + "不是".len();
        if let Some(shi_rel) = find_positive_shi(&text[after_start..]) {
            last_shi = Some(after_start + shi_rel);
        }
        pos = after_start;
    }
    let shi = last_shi?;
    let replacement = text[shi + "是".len()..].trim();
    if replacement.is_empty() || replacement.starts_with(['的', '否', '不']) {
        return None;
    }
    Some(replacement.to_owned())
}

fn find_positive_shi(after: &str) -> Option<usize> {
    let mut index = 0;
    while index < after.len() {
        if !after.is_char_boundary(index) {
            index += 1;
            continue;
        }
        if after[index..].starts_with("不是") || after[index..].starts_with("是否") {
            index += "不是".len();
            continue;
        }
        if after[index..].starts_with("是") {
            return Some(index);
        }
        index += after[index..]
            .chars()
            .next()
            .map(|ch| ch.len_utf8())
            .unwrap_or(1);
    }
    None
}

fn strip_replacement_introducer(suffix: &str) -> String {
    let rest = suffix.trim_start_matches([' ', ',', '，']);
    for intro in ["应该是", "我是说", "是"] {
        if let Some(after) = rest.strip_prefix(intro) {
            let after = after.trim_start();
            if !after.is_empty() && !after.starts_with(['的', '否', '不']) {
                return after.to_owned();
            }
        }
    }
    suffix.trim().to_owned()
}

fn peel_closer(sentence: &str) -> (&str, &str) {
    let trimmed = sentence.trim_end();
    for closer in ["。", "！", "？", ".", "!", "?", "\n"] {
        if let Some(body) = trimmed.strip_suffix(closer) {
            return (body, &sentence[body.len()..]);
        }
    }
    (sentence, "")
}

fn attach_closer(body: &str, closer: &str) -> String {
    let mut out = body.trim().to_owned();
    out.push_str(closer);
    out
}

fn split_on_last_marker_run(text: &str) -> Option<(String, String)> {
    let lower = text.to_ascii_lowercase();
    let mut best: Option<(usize, usize)> = None;
    let mut index = 0;
    while index < text.len() {
        if !text.is_char_boundary(index) {
            index += 1;
            continue;
        }
        if let Some((start, end)) = match_marker_run(text, &lower, index) {
            best = Some((start, end));
            index = end;
            continue;
        }
        index += text[index..]
            .chars()
            .next()
            .map(|ch| ch.len_utf8())
            .unwrap_or(1);
    }
    let (start, end) = best?;
    Some((text[..start].to_owned(), text[end..].to_owned()))
}

fn match_marker_run(text: &str, lower: &str, start: usize) -> Option<(usize, usize)> {
    let mut cursor = skip_junk(text, start);
    let Some((_, end)) = match_one_marker(text, lower, cursor) else {
        return None;
    };
    let run_start = cursor;
    cursor = end;
    loop {
        cursor = skip_junk(text, cursor);
        let Some((_, end)) = match_one_marker(text, lower, cursor) else {
            break;
        };
        cursor = end;
    }
    Some((run_start, cursor))
}

fn match_one_marker(text: &str, lower: &str, start: usize) -> Option<(usize, usize)> {
    if start >= text.len() || !text.is_char_boundary(start) {
        return None;
    }
    for marker in REPLACEMENT_REQUIRED_MARKERS {
        if let Some(end) = match_plain_marker(text, lower, start, marker) {
            if has_replacement_after(text, end) {
                return Some((start, end));
            }
        }
    }
    for marker in MARKERS {
        if let Some(end) = match_plain_marker(text, lower, start, marker) {
            if *marker == "不对" && !cjk_bu_dui_right_ok(text, end) {
                continue;
            }
            return Some((start, end));
        }
    }
    if let Some(end) = match_no_capital_replacement(text, start) {
        return Some((start, end));
    }
    if let Some(end) = match_actually_replacement(text, start) {
        return Some((start, end));
    }
    None
}

fn match_plain_marker(text: &str, lower: &str, start: usize, marker: &str) -> Option<usize> {
    if marker.is_ascii() {
        if lower[start..].starts_with(marker) && ascii_boundaries(text, start, start + marker.len())
        {
            return Some(start + marker.len());
        }
        return None;
    }
    if text[start..].starts_with(marker) {
        return Some(start + marker.len());
    }
    None
}

fn has_replacement_after(text: &str, end: usize) -> bool {
    let after = skip_junk(text, end);
    if after >= text.len() {
        return false;
    }
    !is_blank_or_markers(&text[after..])
}

fn match_no_capital_replacement(text: &str, start: usize) -> Option<usize> {
    let rest = text[start..].trim_start_matches([' ', ',', '，']);
    let offset = start + (text[start..].len() - rest.len());
    let lower = rest.to_ascii_lowercase();
    if !lower.starts_with("no ") {
        return None;
    }
    let after = rest.get(3..)?;
    let next = after.chars().next()?;
    if !next.is_ascii_uppercase() {
        return None;
    }
    Some(offset + 3)
}

fn match_actually_replacement(text: &str, start: usize) -> Option<usize> {
    let rest = text[start..].trim_start_matches([' ', ',', '，']);
    let offset = start + (text[start..].len() - rest.len());
    if !rest.to_ascii_lowercase().starts_with("actually") {
        return None;
    }
    let before = text[..start].trim_end();
    if before.to_ascii_lowercase().ends_with("i") {
        return None;
    }
    let after = rest.get("actually".len()..)?.trim_start();
    after.chars().next().filter(|ch| ch.is_alphanumeric())?;
    Some(offset + "actually".len())
}

fn cjk_bu_dui_right_ok(text: &str, end: usize) -> bool {
    match text[end..].chars().next() {
        None => true,
        Some(ch) if ch.is_whitespace() => true,
        Some(ch)
            if matches!(
                ch,
                ',' | '，' | '。' | '.' | '、' | '!' | '?' | '！' | '？' | '\n'
            ) =>
        {
            true
        }
        Some('是') => true,
        Some(ch) if matches!(ch, '啊' | '呀' | '呢' | '吧' | '嘛') => true,
        Some(ch) if is_cjk(ch) => false,
        _ => true,
    }
}

fn skip_junk(text: &str, start: usize) -> usize {
    let mut index = start;
    for ch in text[start..].chars() {
        if ch.is_whitespace()
            || matches!(
                ch,
                ',' | '，' | '。' | '.' | '、' | '哦' | '啊' | '呀' | '呢' | '吧' | '嘛'
            )
        {
            index += ch.len_utf8();
        } else {
            break;
        }
    }
    index
}

fn ascii_boundaries(text: &str, start: usize, end: usize) -> bool {
    let left_ok = text[..start]
        .chars()
        .next_back()
        .is_none_or(|ch| !ch.is_ascii_alphabetic());
    let right_ok = text[end..]
        .chars()
        .next()
        .is_none_or(|ch| !ch.is_ascii_alphabetic());
    left_ok && right_ok
}

fn is_blank_or_markers(text: &str) -> bool {
    let stripped = text
        .trim()
        .trim_matches(|ch: char| {
            ch.is_whitespace()
                || matches!(ch, ',' | '，' | '。' | '.' | '、' | '!' | '?' | '！' | '？')
        })
        .to_ascii_lowercase();
    stripped.is_empty() || MARKERS.iter().any(|marker| stripped == *marker)
}

fn collapse_repeats(text: &str) -> String {
    let sentences = split_sentences(text);
    let mut kept: Vec<String> = Vec::new();
    for sentence in sentences {
        let norm = normalize_repeat(&sentence);
        if !norm.is_empty() {
            if let Some(prev) = kept.last() {
                let prev_norm = normalize_repeat(prev);
                if prev_norm == norm || is_repeat_tail(&prev_norm, &norm) {
                    continue;
                }
            }
        }
        kept.push(sentence);
    }
    kept.join("")
}

fn is_repeat_tail(previous: &str, current: &str) -> bool {
    current.chars().count() >= 8 && previous.ends_with(current)
}

fn normalize_repeat(text: &str) -> String {
    let mut out = text.to_owned();
    for filler in ["这个", "这种", "那个", "一下"] {
        out = out.replace(filler, "");
    }
    out.chars()
        .filter(|ch| {
            !ch.is_whitespace()
                && !matches!(
                    *ch,
                    '，' | ',' | '。' | '.' | '！' | '!' | '？' | '?' | '、'
                )
        })
        .collect()
}

fn split_sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut current = String::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        current.push(ch);
        let next = chars.get(index + 1).copied();
        let prev = if index > 0 {
            Some(chars[index - 1])
        } else {
            None
        };
        let is_version_dot = ch == '.'
            && prev.is_some_and(|item| item.is_ascii_digit())
            && next.is_some_and(|item| item.is_ascii_digit());
        let is_list_dot = ch == '.'
            && prev.is_some_and(|item| item.is_ascii_digit())
            && next.is_some_and(|item| item.is_whitespace());
        let english_end = matches!(ch, '.' | '!' | '?')
            && !is_version_dot
            && !is_list_dot
            && next.is_none_or(|item| item.is_whitespace() || is_cjk(item));
        if matches!(ch, '。' | '！' | '？' | '\n') || english_end {
            out.push(std::mem::take(&mut current));
        }
        index += 1;
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn is_cjk(ch: char) -> bool {
    crate::dictionary_learn::is_cjk(ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_level_chinese_correction() {
        assert_eq!(apply("周四，不对，周五"), "周五");
        assert_eq!(apply("嗯，我周四，不对，周五下午开会"), "周五下午开会");
    }

    #[test]
    fn drops_false_start_plus_repeated_markers() {
        let raw = "现在做一个完整的这种 cloud 的，不对，不对，不对。我现在在做一种完整的这个 cursor 的这个测试，看一下它这个具体的 cleanup。看一下它具体的 cleanup。";
        let out = apply(raw);
        assert!(!out.contains("cloud"), "{out}");
        assert!(!out.contains("不对"), "{out}");
        assert!(out.contains("cursor"), "{out}");
        assert!(
            out.contains("看一下它具体的 cleanup") || out.contains("看一下它这个具体的 cleanup"),
            "{out}"
        );
        assert_eq!(out.matches("cleanup").count(), 1, "{out}");
    }

    #[test]
    fn keeps_content_bu_dui() {
        assert_eq!(apply("看它对不对"), "看它对不对");
        assert_eq!(apply("你说不对的时候再改"), "你说不对的时候再改");
        assert_eq!(apply("这件事不对。"), "这件事不对。");
        assert_eq!(apply("我觉得这样不对。"), "我觉得这样不对。");
        assert_eq!(apply("这件事不对劲。"), "这件事不对劲。");
        assert_eq!(apply("你说的不对"), "你说的不对");
        assert!(apply("预算是 1250，不是 1500").contains("1250"));
        assert!(apply("预算是 1250，不是 1500").contains("1500"));
    }

    #[test]
    fn english_markers() {
        assert_eq!(apply("Tuesday, no Wednesday"), "Wednesday");
        assert_eq!(apply("let's meet Tuesday, actually Wednesday"), "Wednesday");
        assert_eq!(
            apply("I actually enjoyed the movie"),
            "I actually enjoyed the movie"
        );
        assert_eq!(
            apply("write a cloud test, scratch that, write a cursor test"),
            "write a cursor test"
        );
        assert_eq!(
            apply("write a cloud test, I meant write a cursor test"),
            "write a cursor test"
        );
    }

    #[test]
    fn revision_markers_need_a_replacement() {
        assert_eq!(
            apply("写 cloud 测试，删掉，写 cursor 测试"),
            "写 cursor 测试"
        );
        assert_eq!(
            apply("写 cloud 测试，算了，写 cursor 测试"),
            "写 cursor 测试"
        );
        assert_eq!(apply("这件事删掉"), "这件事删掉");
        assert_eq!(apply("算了"), "算了");
        assert!(apply("I actually enjoyed the movie").contains("actually"));
    }

    #[test]
    fn strips_whisper_ads_and_repeats() {
        assert_eq!(apply("hello world. Thanks for watching!"), "hello world.");
        assert_eq!(
            apply("hello world. Thanks for watching the show."),
            "hello world."
        );
        assert_eq!(apply("感谢收看本期节目"), "");
        assert_eq!(
            apply("看一下具体的 cleanup。看一下具体的 cleanup。"),
            "看一下具体的 cleanup。"
        );
        assert!(is_hallucination_text("Thanks for watching the show."));
        assert!(is_hallucination_text("感谢收看本期节目"));
        assert!(!is_hallucination_text("hello world"));
    }

    #[test]
    fn does_not_split_numbered_list_markers() {
        let out = apply("1. 是 prompt\n2. 是 system");
        assert!(out.contains("1. "), "{out}");
        assert!(out.contains("2. "), "{out}");
        assert!(out.contains("是 prompt"), "{out}");
        assert!(out.contains("是 system"), "{out}");
        assert!(!out.contains("1.2."), "{out}");
    }

    #[test]
    fn sentence_initial_bu_dui_shi_retracts_the_previous_sentence() {
        assert_eq!(
            apply("做一个 cloud 测试。不对，是 cursor 测试。"),
            "cursor 测试。"
        );
        assert_eq!(
            apply("做一个 cloud 测试。我说不对，是 cursor 测试。"),
            "cursor 测试。"
        );
        assert_eq!(
            apply("做一个 cloud 测试。不对是 cursor 测试。"),
            "cursor 测试。"
        );
        assert_eq!(apply("周四，不对，是周五"), "周五");
        assert_eq!(
            apply("做一个 cloud 测试。不对。是 cursor 测试。"),
            "cursor 测试。"
        );
        assert_eq!(apply("周四，我是说周五"), "周五");
        assert_eq!(apply("做一个 cloud 测试，不是 cloud，是 cursor"), "cursor");
        assert_eq!(apply("Tuesday, I mean Wednesday"), "Wednesday");
    }

    #[test]
    fn trailing_markers_after_a_period_still_drop_the_false_start() {
        let out = apply("现在做一个 cloud 的。不对，不对，不对。我现在在做 cursor 的测试。");
        assert!(!out.contains("cloud"), "{out}");
        assert!(!out.contains("不对"), "{out}");
        assert!(out.contains("cursor"), "{out}");
    }
}
