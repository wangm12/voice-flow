//! Spoken line, list, and email-block layout applied after punctuation and
//! before lexicon / LLM cleanup.

use crate::context::ContextFamily;

const LINE_COMMANDS: &[(&str, &str)] = &[
    ("start a new paragraph", "\n\n"),
    ("new paragraph", "\n\n"),
    ("skip a line", "\n"),
    ("line break", "\n"),
    ("next line", "\n"),
    ("new line", "\n"),
    ("new-line", "\n"),
    ("newline", "\n"),
    ("另起一段", "\n\n"),
    ("新段落", "\n\n"),
    ("另起一行", "\n"),
    ("下一行", "\n"),
    ("换行", "\n"),
];

/// Apply spoken layout to already-punctuated ASR text.
pub fn apply(text: &str, family: ContextFamily, confidence: f32) -> String {
    let spoken = apply_line_commands(text);
    if family == ContextFamily::Email && confidence >= 0.75 {
        let (greeting, rest) = peel_greeting(&spoken);
        let (body, closing) = peel_closing(&rest);
        return drop_spoken_item_backtracks(&join_layout_parts(&[
            greeting,
            apply_lists(&body, family),
            closing,
        ]));
    }
    drop_spoken_item_backtracks(&apply_lists(&spoken, family))
}

fn lists_disabled(family: ContextFamily) -> bool {
    matches!(
        family,
        ContextFamily::Terminal | ContextFamily::BrowserSearch | ContextFamily::FormFilling
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MarkerKind {
    Numbered,
    Bullet,
}

#[derive(Clone, Copy)]
struct Marker {
    start: usize,
    end: usize,
    kind: MarkerKind,
}

fn apply_lists(text: &str, family: ContextFamily) -> String {
    if lists_disabled(family) {
        return text.to_string();
    }
    text.split("\n\n")
        .map(format_list_block)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn format_list_block(block: &str) -> String {
    let markers = collect_markers(block);
    let numbered = markers
        .iter()
        .filter(|item| item.kind == MarkerKind::Numbered)
        .copied()
        .collect::<Vec<_>>();
    let bullets = markers
        .iter()
        .filter(|item| item.kind == MarkerKind::Bullet)
        .copied()
        .collect::<Vec<_>>();
    if numbered.len() >= 2 {
        return render_list(block, &numbered, MarkerKind::Numbered);
    }
    if bullets.len() >= 2 {
        return render_list(block, &bullets, MarkerKind::Bullet);
    }
    block.to_string()
}

fn render_list(block: &str, markers: &[Marker], kind: MarkerKind) -> String {
    let preamble = block[..markers[0].start].trim();
    let mut items = Vec::new();
    for (index, marker) in markers.iter().enumerate() {
        let end = markers
            .get(index + 1)
            .map(|next| next.start)
            .unwrap_or(block.len());
        let item = block[marker.end..end].trim();
        if !item.is_empty() {
            items.push(item);
        }
    }
        if items.len() < 2 {
            return block.to_string();
        }
        let mut out = String::new();
    if !preamble.is_empty() {
        out.push_str(preamble);
        out.push('\n');
    }
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        match kind {
            MarkerKind::Numbered => {
                out.push_str(&(index + 1).to_string());
                out.push_str(". ");
            }
            MarkerKind::Bullet => out.push_str("- "),
        }
        out.push_str(item);
    }
    out
}

fn drop_spoken_item_backtracks(text: &str) -> String {
    text.split("\n\n")
        .map(rewrite_list_backtrack_block)
        .collect::<Vec<_>>()
        .join("\n\n")
}

struct ListItem {
    body: String,
    discarded: bool,
}

fn rewrite_list_backtrack_block(block: &str) -> String {
    let mut preamble = Vec::new();
    let mut items: Vec<ListItem> = Vec::new();
    let mut numbered_source = false;
    let mut bullet_source = false;
    for line in block.lines() {
        let trimmed = line.trim_end();
        let stripped = strip_trailing_backtrack(line);
        let discarded_here = stripped != trimmed;
        if is_list_line(&stripped) {
            if stripped.trim_start().starts_with("- ") {
                bullet_source = true;
            } else {
                numbered_source = true;
            }
            items.push(ListItem {
                body: strip_list_prefix(&stripped),
                discarded: discarded_here,
            });
        } else if !items.is_empty() {
            if let Some(last) = items.last_mut() {
                last.body.push('\n');
                last.body.push_str(&stripped);
                last.discarded |= discarded_here;
            }
        } else if !stripped.is_empty() {
            preamble.push(stripped);
        }
    }
    if items.is_empty() {
        return preamble.join("\n");
    }
    let mut candidates = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let body = item.body.trim();
        if body.is_empty() {
            continue;
        }
        let has_replacement = index + 1 < items.len();
        if item.discarded && has_replacement {
            continue;
        }
        candidates.push(body.to_string());
    }
    let mut kept = Vec::new();
    for (index, body) in candidates.iter().enumerate() {
        let superseded = candidates.get(index + 1).is_some_and(|next| {
            let a = strip_trailing_punct(body);
            !a.is_empty() && next.starts_with(&a) && next.len() > a.len()
        });
        if !superseded {
            kept.push(body.clone());
        }
    }
    if kept.is_empty() {
        return preamble.join("\n");
    }
    let mut out = preamble;
    for (index, item) in kept.iter().enumerate() {
        if numbered_source {
            out.push(format!("{}. {item}", index + 1));
        } else if bullet_source {
            out.push(format!("- {item}"));
        } else {
            out.push(item.clone());
        }
    }
    out.join("\n")
}

fn strip_trailing_punct(text: &str) -> String {
    text.trim_end_matches(|ch: char| {
        matches!(ch, ',' | '，' | '.' | '。' | '!' | '！' | ';' | '；' | ' ' | '\t')
    })
    .to_string()
}

fn strip_trailing_backtrack(text: &str) -> String {
    let mut value = text.trim_end().to_string();
    loop {
        let Some(next) = strip_one_trailing_backtrack(&value) else {
            break;
        };
        if next == value {
            break;
        }
        value = next;
    }
    value.trim_end().to_string()
}

fn is_trailing_marker_punct(ch: char) -> bool {
    matches!(ch, ',' | '，' | '.' | '。' | '!' | '！' | ' ' | '\t')
}

fn strip_one_trailing_backtrack(text: &str) -> Option<String> {
    const MARKER: &str = "不对";
    let trimmed = text.trim_end_matches(is_trailing_marker_punct);
    let Some(index) = trimmed.rfind(MARKER) else {
        return strip_trailing_english_backtrack(text);
    };
    let after = &trimmed[index + MARKER.len()..];
    if !after.chars().all(is_trailing_marker_punct) {
        return strip_trailing_english_backtrack(text);
    }
    let before = trimmed[..index].trim_end_matches(is_trailing_marker_punct);
    let Some(particle) = before.chars().next_back() else {
        return strip_trailing_english_backtrack(text);
    };
    if !matches!(particle, '哦' | '啊' | '喔' | '唔') {
        return strip_trailing_english_backtrack(text);
    }
    let cut = before.len() - particle.len_utf8();
    Some(before[..cut].trim_end().to_string())
}

fn strip_trailing_english_backtrack(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    for marker in [
        "scratch that",
        "oh, wait",
        "oh wait",
        "oh, no",
        "oh no",
    ] {
        if let Some(index) = lower.rfind(marker) {
            let after = lower[index + marker.len()..].trim_end_matches(is_trailing_marker_punct);
            if after.is_empty() {
                return Some(text[..index].trim_end().to_string());
            }
        }
    }
    None
}

fn collect_markers(text: &str) -> Vec<Marker> {
    let mut markers = Vec::new();
    let mut index = 0;
    while index < text.len() {
        if let Some(marker) = match_marker_at(text, index) {
            index = marker.end;
            markers.push(marker);
            continue;
        }
        let ch = text[index..].chars().next().expect("rest is non-empty");
        index += ch.len_utf8();
    }
    markers
}

fn match_marker_at(text: &str, start: usize) -> Option<Marker> {
    if let Some(end) = match_chinese_ordinal(text, start) {
        return Some(Marker {
            start,
            end,
            kind: MarkerKind::Numbered,
        });
    }
    if let Some(end) = match_digit_list_marker(text, start) {
        return Some(Marker {
            start,
            end,
            kind: MarkerKind::Numbered,
        });
    }
    if let Some(end) = match_yi_shi_marker(text, start) {
        return Some(Marker {
            start,
            end,
            kind: MarkerKind::Numbered,
        });
    }
    if let Some(end) = match_english_ordinal(text, start) {
        return Some(Marker {
            start,
            end,
            kind: MarkerKind::Numbered,
        });
    }
    if let Some(end) = match_bullet(text, start) {
        return Some(Marker {
            start,
            end,
            kind: MarkerKind::Bullet,
        });
    }
    None
}

fn is_cn_digit(value: char) -> bool {
    matches!(
        value,
        '零' | '一' | '二' | '三' | '四' | '五' | '六' | '七' | '八' | '九' | '十' | '百'
    )
}

fn match_chinese_ordinal(text: &str, start: usize) -> Option<usize> {
    if !text.is_char_boundary(start) || !text[start..].starts_with('第') {
        return None;
    }
    let mut end = start + '第'.len_utf8();
    let mut digits = 0usize;
    for ch in text[end..].chars() {
        if is_cn_digit(ch) || ch.is_ascii_digit() {
            end += ch.len_utf8();
            digits += 1;
        } else {
            break;
        }
    }
    if digits == 0 {
        return None;
    }
    let after = &text[end..];
    if let Some(suffix) = after.chars().next().filter(|ch| {
        matches!(*ch, '点' | '条' | '项' | '个' | '步')
    }) {
        return Some(end + suffix.len_utf8());
    }
    const DENY: &[&str] = &[
        "时间", "部分", "次", "天", "段", "名", "周", "轮", "年", "页", "章", "节", "批", "遍",
        "回", "季", "月", "位", "种", "类", "场",
    ];
    if DENY.iter().any(|suffix| after.starts_with(suffix)) {
        return None;
    }
    Some(end)
}

fn match_digit_list_marker(text: &str, start: usize) -> Option<usize> {
    if !text.is_char_boundary(start) {
        return None;
    }
    if text[..start]
        .chars()
        .next_back()
        .is_some_and(|ch| ch.is_ascii_digit())
    {
        return None;
    }
    let rest = &text[start..];
    let digits = rest.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let after_digits = rest.get(digits..)?;
    let sep = after_digits.chars().next()?;
    if sep != '.' && sep != '、' {
        return None;
    }
    let end = start + digits + sep.len_utf8();
    match text[end..].chars().next() {
        Some(ch) if ch.is_ascii_alphanumeric() => None,
        _ => Some(end),
    }
}

fn match_yi_shi_marker(text: &str, start: usize) -> Option<usize> {
    const MARKERS: &[&str] = &[
        "一是", "二是", "三是", "四是", "五是", "六是", "七是", "八是", "九是", "十是",
    ];
    if !text.is_char_boundary(start) {
        return None;
    }
    let rest = &text[start..];
    for &from in MARKERS {
        if !rest.starts_with(from) {
            continue;
        }
        let end = start + from.len();
        if is_cjk_left_boundary(text, start) && is_cjk_right_boundary(text, end) {
            return Some(end);
        }
    }
    None
}

const ENGLISH_ORDINALS: &[&str] = &[
    "number ten",
    "number nine",
    "number eight",
    "number seven",
    "number six",
    "number five",
    "number four",
    "number three",
    "number two",
    "number one",
    "first",
    "second",
    "third",
    "fourth",
    "fifth",
    "sixth",
    "seventh",
    "eighth",
    "ninth",
    "tenth",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
];

const ORDINAL_FOLLOW_DENY: &[&str] = &[
    "of", "all", "people", "person", "time", "times", "more", "day", "days", "week", "weeks",
    "month", "months", "year", "years", "thing", "things", "place", "half",
];

fn match_english_ordinal(text: &str, start: usize) -> Option<usize> {
    if !is_ascii_word_left(text, start) {
        return None;
    }
    let rest = &text[start..];
    for &from in ENGLISH_ORDINALS {
        if !starts_with_ignore_ascii_case(rest, from) {
            continue;
        }
        let end = start + from.len();
        if !is_ascii_word_right(text, end) {
            continue;
        }
        if following_word_denied(text, end) {
            continue;
        }
        return Some(end);
    }
    None
}

fn following_word_denied(text: &str, end: usize) -> bool {
    next_ascii_word(text, end).is_some_and(|word| {
        ORDINAL_FOLLOW_DENY
            .iter()
            .any(|item| word.eq_ignore_ascii_case(item))
    })
}

fn following_word_denied_for_bullet(text: &str, end: usize) -> bool {
    const DENY: &[&str] = &["train", "point", "points", "list", "hole", "journal"];
    next_ascii_word(text, end)
        .is_some_and(|word| DENY.iter().any(|item| word.eq_ignore_ascii_case(item)))
}

fn next_ascii_word(text: &str, start: usize) -> Option<&str> {
    let rest = text[start..].trim_start();
    let end = rest
        .find(|ch: char| !ch.is_ascii_alphabetic())
        .unwrap_or(rest.len());
    let word = rest.get(..end)?;
    (!word.is_empty()).then_some(word)
}

fn match_bullet(text: &str, start: usize) -> Option<usize> {
    const BULLETS: &[&str] = &["项目符号", "要点", "bullets", "bullet"];
    if !text.is_char_boundary(start) {
        return None;
    }
    let rest = &text[start..];
    for &from in BULLETS {
        if !starts_with_ignore_ascii_case(rest, from) {
            continue;
        }
        let end = start + from.len();
        if from == "要点" && (text[end..].starts_with('是') || text[end..].starts_with("说明"))
        {
            continue;
        }
        if from.eq_ignore_ascii_case("bullet") && following_word_denied_for_bullet(text, end) {
            continue;
        }
        if is_cjk_command(from)
            && is_cjk_left_boundary(text, start)
            && is_cjk_right_boundary(text, end)
        {
            return Some(end);
        } else if is_ascii_word_left(text, start) && is_ascii_word_right(text, end) {
            return Some(end);
        }
    }
    None
}

/// Punctuate, then apply spoken layout. Used by the three cleanup entry points.
pub fn apply_after_punctuation(text: &str, family: ContextFamily, confidence: f32) -> String {
    apply(&crate::spoken_punctuation::apply(text), family, confidence)
}

pub fn has_structural_layout(text: &str) -> bool {
    text.contains('\n') || list_prefix_count(text) > 0
}

/// If the LLM flattened spoken line breaks or list prefixes, restore the
/// breaks while keeping in-line polish. Invented breaks on unstructured
/// transcripts are collapsed.
pub fn restore_if_flattened(before: &str, after: &str) -> String {
    if !has_structural_layout(before) {
        if newline_count(after) > 0 || list_prefix_count(after) > 0 {
            return collapse_invented_structure(after);
        }
        return after.to_string();
    }
    if newline_count(after) == newline_count(before)
        && list_prefix_count(after) >= list_prefix_count(before)
    {
        return after.to_string();
    }
    reinsert_breaks(before, after).unwrap_or_else(|| after.to_string())
}

fn collapse_extra_breaks(piece: &str) -> String {
    let mut lines = piece.lines();
    let Some(first) = lines.next() else {
        return String::new();
    };
    let rest = lines
        .map(strip_list_prefix)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if rest.is_empty() {
        first.trim().to_string()
    } else {
        format!("{} {}", first.trim(), rest.join(" "))
    }
}

fn collapse_invented_structure(after: &str) -> String {
    after
        .lines()
        .map(strip_list_prefix)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_list_prefix(line: &str) -> String {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix("- ") {
        return rest.trim().to_string();
    }
    let digits = trimmed
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .count();
    if digits > 0 {
        if let Some(rest) = trimmed.get(digits..) {
            if let Some(rest) = rest.strip_prefix(". ") {
                return rest.trim().to_string();
            }
        }
    }
    trimmed.to_string()
}

fn is_content_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || is_cjk_letter(ch)
}

fn content_chars(text: &str) -> Vec<char> {
    text.chars()
        .filter(|ch| is_content_char(*ch))
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn reinsert_breaks(before: &str, after: &str) -> Option<String> {
    let lines: Vec<&str> = before.split('\n').collect();
    let after_chars: Vec<char> = after.chars().collect();
    let mut index = 0usize;
    let mut out = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        if line.is_empty() {
            out.push(String::new());
            continue;
        }
        while index < after_chars.len() && after_chars[index].is_whitespace() {
            index += 1;
        }
        let needed = content_chars(line);
        if needed.is_empty() {
            out.push((*line).to_string());
            continue;
        }
        let start = index;
        let mut matched = 0usize;
        while index < after_chars.len() && matched < needed.len() {
            let ch = after_chars[index];
            if is_content_char(ch) {
                let lower = ch.to_lowercase().next().unwrap_or(ch);
                if lower == needed[matched] {
                    matched += 1;
                    index += 1;
                } else {
                    break;
                }
            } else {
                index += 1;
            }
        }
        let last_content = lines[line_index + 1..]
            .iter()
            .all(|item| item.is_empty() || content_chars(item).is_empty());
        if matched == 0 {
            index = start;
            continue;
        }
        if matched < needed.len() {
            if last_content {
                index = after_chars.len();
            }
        } else {
            while index < after_chars.len()
                && !is_content_char(after_chars[index])
                && !after_chars[index].is_whitespace()
            {
                index += 1;
            }
            if last_content {
                index = after_chars.len();
            }
        }
        let piece: String = after_chars[start..index].iter().collect();
        out.push(if last_content {
            collapse_extra_breaks(&piece)
        } else {
            piece.trim().to_string()
        });
    }
    Some(out.join("\n"))
}

fn newline_count(text: &str) -> usize {
    text.chars().filter(|&ch| ch == '\n').count()
}

fn list_prefix_count(text: &str) -> usize {
    text.lines().filter(|line| is_list_line(line)).count()
}

fn is_list_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("- ") {
        return true;
    }
    let digits = trimmed
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .count();
    digits > 0
        && trimmed
            .get(digits..)
            .is_some_and(|rest| rest.starts_with(". "))
}

fn join_layout_parts(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

const EMAIL_GREETINGS: &[&str] = &["hello", "hey", "dear", "hi", "您好", "你好"];
const EMAIL_CLOSINGS: &[&str] = &[
    "best regards",
    "kind regards",
    "thank you",
    "regards",
    "thanks",
    "best",
    "此致",
    "谢谢",
];
const GREETING_BODY_STARTERS: &[&str] = &[
    "looking", "please", "can", "could", "would", "will", "let", "just", "the", "this", "that",
    "i", "we", "i'm", "im", "my", "thanks", "thank", "um", "uh", "a",
];
const CJK_NAME_STOP: &[char] = &[
    '我', '请', '帮', '今', '明', '那', '这', '有', '会', '已', '先', '谢', '赶', '把', '给', '能',
    '要', '想', '看', '开',
];

fn peel_greeting(text: &str) -> (String, String) {
    let start = skip_leading_whitespace(text, 0);
    let rest = &text[start..];
    for &from in EMAIL_GREETINGS {
        if !starts_with_ignore_ascii_case(rest, from) {
            continue;
        }
        let mut end = start + from.len();
        if is_cjk_command(from) {
            if !is_cjk_right_boundary(text, end) {
                continue;
            }
            if cjk_greeting_is_question(text, end) {
                continue;
            }
            end = consume_cjk_name(text, end);
        } else {
            if !is_ascii_word_right(text, end) {
                continue;
            }
            end = consume_english_name(text, end);
        }
        let greeting = text[start..end].trim().to_string();
        let body = text[end..].trim().to_string();
        if greeting.is_empty() {
            break;
        }
        return (greeting, body);
    }
    (String::new(), text.to_string())
}

fn consume_english_name(text: &str, start: usize) -> usize {
    let mut end = start;
    for _ in 0..2 {
        let next_start = skip_leading_whitespace(text, end);
        if next_start >= text.len() {
            break;
        }
        let rest = &text[next_start..];
        let word_len = rest
            .find(|ch: char| !ch.is_ascii_alphabetic() && ch != '\'')
            .unwrap_or(rest.len());
        if word_len == 0 {
            break;
        }
        let word = &rest[..word_len];
        if is_body_starter(word) || is_signature_function_word(word) {
            break;
        }
        end = next_start + word_len;
        if rest[word_len..].starts_with(',') {
            end += 1;
        }
    }
    end
}

fn cjk_greeting_is_question(text: &str, end: usize) -> bool {
    text[end..]
        .chars()
        .next()
        .is_some_and(|ch| matches!(ch, '吗' | '啊' | '呀' | '呢' | '吧' | '嘛'))
}

fn is_body_starter(word: &str) -> bool {
    GREETING_BODY_STARTERS
        .iter()
        .any(|item| word.eq_ignore_ascii_case(item))
}

fn is_signature_function_word(word: &str) -> bool {
    const DENY: &[&str] = &[
        "option", "way", "for", "to", "of", "if", "when", "that", "this", "please", "us",
        "me", "you", "it", "one", "the", "an", "and", "or", "but", "with", "about", "at",
        "on", "in", "by", "from", "as", "so", "not",
    ];
    DENY.iter().any(|item| word.eq_ignore_ascii_case(item))
}

fn consume_cjk_name(text: &str, start: usize) -> usize {
    let mut end = start;
    let mut count = 0usize;
    for ch in text[start..].chars() {
        if !is_cjk_letter(ch) || CJK_NAME_STOP.contains(&ch) || count >= 4 {
            break;
        }
        end += ch.len_utf8();
        count += 1;
    }
    end
}

fn peel_closing(text: &str) -> (String, String) {
    let Some((start, token_end)) = find_last_closing(text) else {
        return (text.to_string(), String::new());
    };
    let signature = text[token_end..].trim();
    if !is_short_signature(signature) {
        return (text.to_string(), String::new());
    }
    let body = text[..start].trim().to_string();
    let closing_word = text[start..token_end].trim().to_string();
    let closing = if signature.is_empty() {
        closing_word
    } else {
        format!("{closing_word}\n{signature}")
    };
    (body, closing)
}

fn find_last_closing(text: &str) -> Option<(usize, usize)> {
    let mut found = None;
    let mut index = 0;
    while index < text.len() {
        if let Some(end) = match_token_from(text, index, EMAIL_CLOSINGS) {
            let cjk = is_cjk_command(&text[index..end]);
            let ok = if cjk {
                is_cjk_left_boundary(text, index) && is_cjk_right_boundary(text, end)
            } else {
                is_ascii_word_left(text, index) && is_ascii_word_right(text, end)
            };
            if ok {
                found = Some((index, end));
                index = end;
                continue;
            }
        }
        let ch = text[index..].chars().next().expect("rest is non-empty");
        index += ch.len_utf8();
    }
    found
}

fn is_short_signature(text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    if cjk_len(text) > 8 {
        return false;
    }
    let words = text.split_whitespace().collect::<Vec<_>>();
    if words.len() > 3 {
        return false;
    }
    words.iter().all(|word| {
        let first = word.chars().next();
        first.is_some_and(|ch| ch.is_ascii_alphabetic() || is_cjk_letter(ch))
            && !is_body_starter(word)
            && !is_signature_function_word(word)
    })
}

fn match_token_from(text: &str, start: usize, tokens: &[&str]) -> Option<usize> {
    let rest = &text[start..];
    for &from in tokens {
        if starts_with_ignore_ascii_case(rest, from) {
            return Some(start + from.len());
        }
    }
    None
}

fn cjk_len(text: &str) -> usize {
    text.chars().filter(|ch| is_cjk_letter(*ch)).count()
}

fn apply_line_commands(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        if let Some((end, replacement)) = match_line_command_at(text, index) {
            while out.ends_with(char::is_whitespace) {
                out.pop();
            }
            out.push_str(replacement);
            index = skip_leading_whitespace(text, end);
            continue;
        }
        let ch = text[index..].chars().next().expect("rest is non-empty");
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

fn match_line_command_at(text: &str, start: usize) -> Option<(usize, &'static str)> {
    if !text.is_char_boundary(start) {
        return None;
    }
    let rest = &text[start..];
    for &(from, to) in LINE_COMMANDS {
        if !starts_with_ignore_ascii_case(rest, from) {
            continue;
        }
        let end = start + from.len();
        if is_cjk_command(from) {
            if !is_cjk_left_boundary(text, start) || !is_cjk_right_boundary(text, end) {
                continue;
            }
            if negated_cjk_break(text, start) {
                continue;
            }
            if text[end..].starts_with('符') {
                continue;
            }
        } else {
            if !is_ascii_word_left(text, start) || !is_ascii_word_right(text, end) {
                continue;
            }
            if followed_by_character_word(text, end) {
                continue;
            }
            if negated_english_break(text, start) {
                continue;
            }
        }
        return Some((end, to));
    }
    None
}

fn starts_with_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    haystack.len() >= needle.len()
        && haystack.is_char_boundary(needle.len())
        && haystack[..needle.len()].eq_ignore_ascii_case(needle)
}

fn is_cjk_command(from: &str) -> bool {
    from.chars().any(is_cjk_letter)
}

fn negated_cjk_break(text: &str, start: usize) -> bool {
    text[..start].ends_with("不要") || text[..start].ends_with('不')
}

fn negated_english_break(text: &str, start: usize) -> bool {
    let before = text[..start].trim_end().to_ascii_lowercase();
    before.ends_with("do not")
        || before.ends_with("don't")
        || before.ends_with("dont")
        || before.ends_with(" not")
}

fn followed_by_character_word(text: &str, end: usize) -> bool {
    let rest = text[end..].trim_start();
    starts_with_ignore_ascii_case(rest, "character")
        && is_ascii_word_right(rest, "character".len())
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

fn is_cjk_letter(value: char) -> bool {
    matches!(
        value,
        '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}'
    )
}

fn is_boundary_punct(value: char) -> bool {
    value.is_whitespace()
        || value.is_ascii_punctuation()
        || matches!(
            value,
            '\u{3000}'..='\u{303F}'
                | '\u{FF01}'..='\u{FF0F}'
                | '\u{FF1A}'..='\u{FF20}'
                | '\u{FF3B}'..='\u{FF40}'
                | '\u{FF5B}'..='\u{FF65}'
        )
}

fn is_cjk_left_boundary(text: &str, start: usize) -> bool {
    text[..start]
        .chars()
        .next_back()
        .map(|ch| is_boundary_punct(ch) || is_cjk_letter(ch))
        .unwrap_or(true)
}

fn is_cjk_right_boundary(text: &str, end: usize) -> bool {
    text[end..]
        .chars()
        .next()
        .map(|ch| is_boundary_punct(ch) || is_cjk_letter(ch))
        .unwrap_or(true)
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

#[cfg(test)]
mod tests {
    use super::{apply, restore_if_flattened};
    use crate::context::ContextFamily;

    fn layout(text: &str) -> String {
        apply(text, ContextFamily::General, 1.0)
    }

    #[test]
    fn mid_sentence_english_new_line_inserts_a_break() {
        assert_eq!(
            layout("the reading club new line should be tomorrow"),
            "the reading club\nshould be tomorrow"
        );
    }

    #[test]
    fn accepts_wispr_line_command_aliases() {
        assert_eq!(layout("hello next line world"), "hello\nworld");
        assert_eq!(layout("hello line break world"), "hello\nworld");
        assert_eq!(layout("hello skip a line world"), "hello\nworld");
        assert_eq!(layout("hello newline world"), "hello\nworld");
        assert_eq!(layout("hello new-line world"), "hello\nworld");
    }

    #[test]
    fn mid_sentence_chinese_line_break_does_not_need_spaces() {
        assert_eq!(layout("请换行然后继续"), "请\n然后继续");
        assert_eq!(layout("先写完另起一行再改"), "先写完\n再改");
    }

    #[test]
    fn spoken_new_paragraph_inserts_a_blank_line() {
        assert_eq!(
            layout("第一段内容 新段落 第二段内容"),
            "第一段内容\n\n第二段内容"
        );
        assert_eq!(
            layout("first block new paragraph second block"),
            "first block\n\nsecond block"
        );
        assert_eq!(
            layout("first start a new paragraph second"),
            "first\n\nsecond"
        );
    }

    #[test]
    fn rejects_newline_character_and_negated_chinese_break() {
        assert_eq!(
            layout("use a newline character here"),
            "use a newline character here"
        );
        assert_eq!(layout("这里不要换行谢谢"), "这里不要换行谢谢");
        assert_eq!(layout("这里不换行"), "这里不换行");
        assert_eq!(layout("使用换行符即可"), "使用换行符即可");
        assert_eq!(layout("please do not new line here"), "please do not new line here");
        assert_eq!(layout("please don't new line here"), "please don't new line here");
    }

    #[test]
    fn calendar_and_section_classifiers_are_not_lists() {
        assert_eq!(
            layout("第一天做设计第二天写测试"),
            "第一天做设计第二天写测试"
        );
        assert_eq!(
            layout("第一段写背景第二段写结论"),
            "第一段写背景第二段写结论"
        );
    }

    #[test]
    fn ge_ordinals_list_but_first_thought_stays_prose() {
        assert_eq!(
            layout("第一个完成设计第二个写测试"),
            "1. 完成设计\n2. 写测试"
        );
        assert_eq!(
            layout("我第一个想到的是先验证数据"),
            "我第一个想到的是先验证数据"
        );
    }

    #[test]
    fn first_time_and_first_of_are_not_lists() {
        assert_eq!(
            layout("the first time I tried and the second time I failed"),
            "the first time I tried and the second time I failed"
        );
        assert_eq!(layout("first of all we should wait"), "first of all we should wait");
    }

    #[test]
    fn a_single_bullet_or_yaodian_stays_prose() {
        assert_eq!(
            layout("今天会议的要点是进度"),
            "今天会议的要点是进度"
        );
        assert_eq!(layout("the bullet train is late"), "the bullet train is late");
    }

    #[test]
    fn two_chinese_ordinals_become_a_numbered_list() {
        assert_eq!(
            layout("第一完成设计第二写测试第三发布"),
            "1. 完成设计\n2. 写测试\n3. 发布"
        );
        assert_eq!(
            layout("第一点我们需要先验证数据第二点再发布结果"),
            "1. 我们需要先验证数据\n2. 再发布结果"
        );
    }

    #[test]
    fn english_ordinals_and_number_words_become_lists() {
        assert_eq!(
            layout("first finish the design second write tests"),
            "1. finish the design\n2. write tests"
        );
        assert_eq!(
            layout("one finish the report two send it"),
            "1. finish the report\n2. send it"
        );
        assert_eq!(
            layout("number one buy milk number two buy eggs"),
            "1. buy milk\n2. buy eggs"
        );
    }

    #[test]
    fn spoken_digit_and_yi_shi_markers_become_lists() {
        assert_eq!(
            layout("1. 是 prompt 2. 是标点符号 3. 是逻辑"),
            "1. 是 prompt\n2. 是标点符号\n3. 是逻辑"
        );
        assert_eq!(
            layout("1、完成设计 2、写测试"),
            "1. 完成设计\n2. 写测试"
        );
        assert_eq!(
            layout("一是 prompt 二是标点符号 三是逻辑"),
            "1. prompt\n2. 标点符号\n3. 逻辑"
        );
    }

    #[test]
    fn spoken_digit_list_false_positives_stay_prose() {
        assert_eq!(layout("预算是 1250"), "预算是 1250");
        assert_eq!(layout("看它对不对"), "看它对不对");
        assert_eq!(layout("一是先验证数据"), "一是先验证数据");
        assert_eq!(layout("version 1.2 and 1.3"), "version 1.2 and 1.3");
        assert_eq!(
            layout("this is one of the options"),
            "this is one of the options"
        );
    }

    #[test]
    fn spoken_bullets_become_dash_list() {
        assert_eq!(
            layout("bring bullet computer bullet charger"),
            "bring\n- computer\n- charger"
        );
    }

    #[test]
    fn list_false_positives_and_incomplete_ordinals_stay_prose() {
        assert_eq!(
            layout("我第一个想到的是先验证数据"),
            "我第一个想到的是先验证数据"
        );
        assert_eq!(layout("第一我们先验证数据"), "第一我们先验证数据");
        assert_eq!(
            layout("第一部分是数据库迁移。第二部分是回滚方案"),
            "第一部分是数据库迁移。第二部分是回滚方案"
        );
        assert_eq!(
            layout("this is one of the options"),
            "this is one of the options"
        );
        assert_eq!(layout("two people are waiting"), "two people are waiting");
    }

    #[test]
    fn wechat_still_lists_spoken_ordinals() {
        assert_eq!(
            apply("第一完成设计第二写测试", ContextFamily::PersonalChat, 0.9),
            "1. 完成设计\n2. 写测试"
        );
    }

    #[test]
    fn terminal_search_and_forms_do_not_auto_list() {
        for family in [
            ContextFamily::Terminal,
            ContextFamily::BrowserSearch,
            ContextFamily::FormFilling,
        ] {
            assert_eq!(
                apply("第一完成设计第二写测试", family, 0.9),
                "第一完成设计第二写测试"
            );
        }
        assert_eq!(
            apply("hello new line world", ContextFamily::Terminal, 0.9),
            "hello\nworld"
        );
    }

    #[test]
    fn email_splits_spoken_greeting_and_closing_only() {
        assert_eq!(
            apply(
                "Hi John looking forward to chatting tomorrow Best Allan",
                ContextFamily::Email,
                0.9
            ),
            "Hi John\nlooking forward to chatting tomorrow\nBest\nAllan"
        );
        assert_eq!(
            apply(
                "Hello looking forward to chatting tomorrow",
                ContextFamily::Email,
                0.9
            ),
            "Hello\nlooking forward to chatting tomorrow"
        );
        assert_eq!(
            apply(
                "looking forward to chatting tomorrow",
                ContextFamily::Email,
                0.9
            ),
            "looking forward to chatting tomorrow"
        );
        assert_eq!(
            apply("您好明天三点开会谢谢", ContextFamily::Email, 0.9),
            "您好\n明天三点开会\n谢谢"
        );
        assert_eq!(
            apply(
                "hi john looking forward to chatting tomorrow best allan",
                ContextFamily::Email,
                0.9
            ),
            "hi john\nlooking forward to chatting tomorrow\nbest\nallan"
        );
        assert_eq!(
            apply("你好吗我周五有空", ContextFamily::Email, 0.9),
            "你好吗我周五有空"
        );
    }

    #[test]
    fn chat_does_not_split_on_thanks() {
        assert_eq!(
            apply("can you ping Maya thanks", ContextFamily::WorkChat, 0.9),
            "can you ping Maya thanks"
        );
        assert_eq!(
            apply("谢谢你周五前发报告", ContextFamily::PersonalChat, 0.9),
            "谢谢你周五前发报告"
        );
        assert_eq!(
            apply(
                "Hi John looking forward Best Allan",
                ContextFamily::Email,
                0.5
            ),
            "Hi John looking forward Best Allan"
        );
    }

    #[test]
    fn restore_if_flattened_keeps_spoken_breaks_and_list_prefixes() {
        assert_eq!(
            restore_if_flattened(
                "the reading club\nshould be tomorrow",
                "The reading club should be tomorrow."
            ),
            "The reading club\nshould be tomorrow."
        );
        assert_eq!(
            restore_if_flattened("1. a\n2. b", "1. a 2. b"),
            "1. a\n2. b"
        );
        assert_eq!(
            restore_if_flattened("hello world", "Hello world."),
            "Hello world."
        );
        assert_eq!(
            restore_if_flattened("1. a\n2. b", "1. a\n2. b."),
            "1. a\n2. b."
        );
        assert_eq!(
            restore_if_flattened("hello world", "Hello.\nWorld."),
            "Hello. World."
        );
        assert_eq!(
            restore_if_flattened(
                "the reading club\nshould be tomorrow",
                "The reading club\n\nshould be tomorrow."
            ),
            "The reading club\nshould be tomorrow."
        );
        assert_eq!(
            restore_if_flattened("1. a\n2. b", "1. a\n2. b\n3. c"),
            "1. a\n2. b c"
        );
        assert_eq!(
            restore_if_flattened(
                "1. 是 system prompt\n2. 是看一下 style",
                "1. 是 system prompt. 2. 是看一下 style."
            ),
            "1. 是 system prompt.\n2. 是看一下 style."
        );
        assert_eq!(
            restore_if_flattened(
                "1. 是 system prompt\n2. 是看一下 style",
                "1. 是 system prompt\n2. 是看一下 style\n3. 是 prompt"
            ),
            "1. 是 system prompt\n2. 是看一下 style 是 prompt"
        );
    }

    #[test]
    fn oh_bu_dui_list_backtracks_drop_the_false_start() {
        assert_eq!(
            layout(
                "我们来测试一下,看一下具体的 ASR 流程。1. 是 prompt。看一下 prompt 到底怎么样。哦,不对, 2. 是 system。哦,不对, 3. 是 system prompt,看一下具体的流程怎么样。4. 是看一下 style,和它的逻辑是怎么样。5. 是看一下它整个的识别率怎么样。"
            ),
            "我们来测试一下,看一下具体的 ASR 流程。\n1. 是 system prompt,看一下具体的流程怎么样。\n2. 是看一下 style,和它的逻辑是怎么样。\n3. 是看一下它整个的识别率怎么样。"
        );
        assert_eq!(
            layout(
                "我们来测试一下,看一下具体的 ASR 流程。\n1. 是 prompt。看一下 prompt 到底怎么样。哦,不对,\n2. 是 system。哦,不对,\n3. 是 system prompt,看一下具体的流程怎么样。\n4. 是看一下 style,和它的\n逻辑是怎么样。\n5. 是看一下它整个的识别率怎么样。"
            ),
            "我们来测试一下,看一下具体的 ASR 流程。\n1. 是 system prompt,看一下具体的流程怎么样。\n2. 是看一下 style,和它的\n逻辑是怎么样。\n3. 是看一下它整个的识别率怎么样。"
        );
        assert_eq!(
            layout("1. write a cloud test oh wait 2. write a cursor test"),
            "1. write a cursor test"
        );
        assert_eq!(
            layout(
                "1. 是 system prompt，看一下具体的流程怎么样。2. 是 system prompt，看一下具体的流程怎么样。再确认一下。哦,不对 3. 是看一下 style"
            ),
            "1. 是 system prompt，看一下具体的流程怎么样。\n2. 是看一下 style"
        );
        assert_eq!(
            layout("1. 看它对不对 oh wait 2. 再看 style"),
            "1. 再看 style"
        );
        assert_eq!(
            layout("1. 是 prompt。哦,不对, 2. 是 system。哦,不对,"),
            "1. 是 system。"
        );
        assert_eq!(
            layout("1. 是 system prompt 2. 是看一下 style。哦,不对,"),
            "1. 是 system prompt\n2. 是看一下 style。"
        );
        assert_eq!(
            layout("1. 是 prompt，不对 2. 是 system"),
            "1. 是 prompt，不对\n2. 是 system"
        );
        assert_eq!(
            layout("1. write a cloud test oh wait"),
            "1. write a cloud test"
        );
        assert_eq!(
            layout("1. write a cloud test scratch that 2. write a cursor test"),
            "1. write a cursor test"
        );
        assert_eq!(
            layout("1. write a cloud test oh wait。 2. write a cursor test"),
            "1. write a cursor test"
        );
        assert_eq!(layout("看它对不对"), "看它对不对");
        assert_eq!(
            layout("周四，不对，周五下午开会"),
            "周四，不对，周五下午开会"
        );
        assert_eq!(
            layout("1. 看它对不对 2. 再看 style"),
            "1. 看它对不对\n2. 再看 style"
        );
    }
}
