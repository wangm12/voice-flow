//! Conservative transcript-only evidence that a Codex-targeted dictation is
//! natural-language prompt text rather than source code or ambiguous data.
//!
//! This is deliberately narrower than a general language detector: it never
//! changes the captured Accessibility classification or applies to other apps.

use std::ops::Range;

pub(crate) fn has_strong_natural_language_evidence(transcript: &str) -> bool {
    let transcript = transcript.trim();
    if transcript.is_empty() {
        return false;
    }

    let literals = transcript_literal_ranges(transcript);
    let evidence_text = without_literals(transcript, &literals);
    if has_code_structure(&evidence_text)
        || looks_like_standalone_code_expression(&evidence_text)
        || looks_like_standalone_structured_data(&evidence_text)
    {
        return false;
    }
    has_chinese_request_evidence(&evidence_text) || has_english_request_evidence(&evidence_text)
}

/// Reject a bare call/member expression before looking at language cues. A
/// Codex prompt may mention a call inside a framed sentence, but a symbol such
/// as `review_handler(item, context)` is not a natural-language request.
fn looks_like_standalone_code_expression(transcript: &str) -> bool {
    let expression = transcript.trim().trim_end_matches(';').trim();
    if expression.is_empty() {
        return false;
    }

    if let Some(open) = expression.find('(') {
        let callee = expression[..open].trim();
        let arguments = &expression[open..];
        if is_member_path(callee) && has_balanced_outer_call(arguments) {
            return true;
        }
    }

    is_member_path(expression) && expression.contains('.')
}

fn looks_like_standalone_structured_data(transcript: &str) -> bool {
    let value = transcript.trim();
    if value.starts_with('[') && value.ends_with(']') {
        return true;
    }

    let mut lines = value.lines().map(str::trim).filter(|line| !line.is_empty());
    let Some(header) = lines.next() else {
        return false;
    };
    if !header.ends_with(':') {
        return false;
    }
    let items = lines.collect::<Vec<_>>();
    !items.is_empty() && items.iter().all(|line| line.starts_with('-'))
}

fn is_member_path(value: &str) -> bool {
    let mut saw_segment = false;
    for segment in value.split('.') {
        if segment.is_empty()
            || !segment.chars().all(|character| {
                character == '_' || character == '$' || character.is_alphanumeric()
            })
        {
            return false;
        }
        saw_segment = true;
    }
    saw_segment
}

fn has_balanced_outer_call(value: &str) -> bool {
    let mut depth = 0usize;
    let mut closing_at_end = false;
    for (index, character) in value.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                let Some(next_depth) = depth.checked_sub(1) else {
                    return false;
                };
                depth = next_depth;
                if depth == 0 {
                    closing_at_end = value[index + character.len_utf8()..].trim().is_empty();
                    if !closing_at_end {
                        return false;
                    }
                }
            }
            _ => {}
        }
    }
    depth == 0 && closing_at_end
}

fn transcript_literal_ranges(transcript: &str) -> Vec<Range<usize>> {
    let mut ranges = crate::spoken_revision::literal_ranges(transcript);
    ranges.extend(inline_backtick_ranges(transcript));
    ranges.sort_by_key(|range| range.start);

    let mut merged = Vec::<Range<usize>>::with_capacity(ranges.len());
    for range in ranges {
        if let Some(previous) = merged.last_mut() {
            if range.start <= previous.end {
                previous.end = previous.end.max(range.end);
                continue;
            }
        }
        merged.push(range);
    }
    merged
}

fn inline_backtick_ranges(transcript: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut open: Option<(usize, usize)> = None;
    let mut offset = 0;
    let bytes = transcript.as_bytes();
    while offset < bytes.len() {
        if bytes[offset] != b'`' {
            offset += transcript[offset..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
            continue;
        }
        let run = bytes[offset..]
            .iter()
            .take_while(|byte| **byte == b'`')
            .count();
        match open {
            Some((start, delimiter_len)) if delimiter_len == run => {
                ranges.push(start..offset + run);
                open = None;
            }
            None => open = Some((offset, run)),
            _ => {}
        }
        offset += run;
    }
    if let Some((start, _)) = open {
        ranges.push(start..transcript.len());
    }
    ranges
}

fn without_literals(transcript: &str, literals: &[Range<usize>]) -> String {
    let mut result = String::with_capacity(transcript.len());
    let mut range_index = 0;
    for (offset, character) in transcript.char_indices() {
        while literals
            .get(range_index)
            .is_some_and(|range| offset >= range.end)
        {
            range_index += 1;
        }
        if literals
            .get(range_index)
            .is_some_and(|range| range.start <= offset && offset < range.end)
        {
            result.push(' ');
        } else {
            result.push(character);
        }
    }
    result
}

fn has_code_structure(transcript: &str) -> bool {
    if transcript.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("//")
            || line.starts_with("/*")
            || line.starts_with("* ")
            || line.starts_with("<!--")
            || line.starts_with("-- ")
            || line.starts_with('#')
    }) {
        return true;
    }

    let lower = transcript.to_ascii_lowercase();
    let code_statement_prefixes = [
        "const ",
        "let ",
        "var ",
        "function ",
        "class ",
        "interface ",
        "enum ",
        "struct ",
        "impl ",
        "pub fn ",
        "async fn ",
        "fn ",
        "def ",
        "import ",
        "export ",
        "return ",
        "select ",
        "insert into ",
        "update ",
        "delete from ",
    ];
    if lower.lines().any(|line| {
        code_statement_prefixes
            .iter()
            .any(|prefix| line.trim_start().starts_with(prefix))
    }) {
        return true;
    }

    if transcript.lines().any(looks_like_shell_command_line) {
        return true;
    }

    let request_evidence_text =
        transcript
            .split_once("//")
            .map_or(transcript, |(before_comment, _)| {
                if before_comment.contains("--") {
                    before_comment
                } else {
                    transcript
                }
            });
    let natural_request = has_chinese_request_evidence(request_evidence_text)
        || has_english_request_evidence(request_evidence_text);
    let bytes = transcript.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let rest = &transcript[index..];
        if [
            "=>", "->", "==", "!=", "<=", ">=", "&&", "||", ":=", "++", "{", "}",
        ]
        .iter()
        .any(|marker| rest.starts_with(marker))
        {
            return true;
        }
        if let Some(option_name) = rest.strip_prefix("--") {
            let option_name_follows = option_name
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_alphanumeric());
            let option_boundary_precedes = transcript[..index]
                .chars()
                .next_back()
                .is_none_or(|character| !character.is_alphanumeric() && character != '_');
            if !(natural_request && option_name_follows && option_boundary_precedes) {
                return true;
            }
        }
        if [";", "="].iter().any(|marker| rest.starts_with(marker)) {
            return true;
        }
        index += rest.chars().next().map(char::len_utf8).unwrap_or(1);
    }
    false
}

fn looks_like_shell_command_line(line: &str) -> bool {
    let line = line.trim_start();
    let Some(command) = line
        .split(|character: char| character.is_whitespace() || matches!(character, ';' | '|'))
        .next()
    else {
        return false;
    };
    let command = command.rsplit('/').next().unwrap_or(command);
    matches!(
        command.to_ascii_lowercase().as_str(),
        "echo"
            | "printf"
            | "git"
            | "python"
            | "python3"
            | "node"
            | "npm"
            | "npx"
            | "pnpm"
            | "yarn"
            | "cargo"
            | "rustc"
            | "curl"
            | "wget"
            | "cat"
            | "grep"
            | "rg"
            | "sed"
            | "awk"
            | "find"
            | "docker"
            | "kubectl"
            | "make"
            | "mkdir"
            | "rm"
            | "cd"
            | "ls"
            | "touch"
            | "chmod"
            | "ssh"
            | "psql"
            | "sqlite3"
            | "swift"
            | "xcodebuild"
            | "open"
    )
}

fn has_chinese_request_evidence(transcript: &str) -> bool {
    let han_count = transcript
        .chars()
        .filter(|character| is_han(*character))
        .count();
    if han_count < 7 {
        return false;
    }

    let lower = transcript.to_lowercase();
    [
        "帮我",
        "请帮",
        "请你",
        "请检查",
        "请解释",
        "请分析",
        "请修复",
        "请实现",
        "请说明",
        "能不能",
        "可以帮",
        "我想",
        "我需要",
        "为什么",
        "怎么",
        "如何",
        "告诉我",
        "帮忙",
        "看一下",
        "修一下",
        "改一下",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn has_english_request_evidence(transcript: &str) -> bool {
    let words = transcript
        .split(|character: char| {
            !character.is_ascii_alphanumeric() && character != '\'' && character != '_'
        })
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    if words.len() < 3 {
        return false;
    }

    [
        "please explain",
        "please review",
        "please analyze",
        "please analyse",
        "please summarize",
        "please describe",
        "please check",
        "please fix",
        "please implement",
        "please refactor",
        "could you",
        "can you",
        "would you",
        "help me",
        "tell me",
        "explain why",
        "explain how",
        "review this",
        "analyze this",
        "analyse this",
        "summarize this",
        "describe this",
        "check whether",
        "check why",
        "fix this",
        "implement this",
        "refactor this",
        "why does",
        "why is",
        "how does",
        "how can",
        "what does",
    ]
    .iter()
    .any(|marker| has_word_sequence(&words, marker))
}

fn has_word_sequence(words: &[String], phrase: &str) -> bool {
    let wanted = phrase.split_whitespace().collect::<Vec<_>>();
    words
        .windows(wanted.len())
        .any(|window| window.iter().map(String::as_str).eq(wanted.iter().copied()))
}

fn is_han(character: char) -> bool {
    matches!(
        character as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F
    )
}

#[cfg(test)]
mod tests {
    use super::has_strong_natural_language_evidence;

    #[test]
    fn recognizes_clear_chinese_and_english_requests_with_technical_literals() {
        for transcript in [
            "帮我分析这个函数为什么会在空响应时一直等待。",
            "请帮我检查 /Users/test/project/api/v2 为什么会返回 429 错误。",
            "Please explain why this parser keeps waiting after an empty response.",
            "Could you review the retry logic and preserve `--dry-run` exactly?",
            "Please explain why parser(foo) fails on empty input.",
            "请帮我检查这个函数(参数)为什么会一直等待。",
        ] {
            assert!(
                has_strong_natural_language_evidence(transcript),
                "expected request evidence for {transcript:?}"
            );
        }
    }

    #[test]
    fn does_not_treat_code_data_or_ambiguous_fragments_as_prompt_evidence() {
        for transcript in [
            "const 名字 = 1;",
            "function wait() { return true; }",
            "async fn main() -> Result<(), Error>",
            "数组长度",
            "Please explain why this parser waits = return value",
            "echo please fix this bug",
            "git commit -m please fix API",
            "python -c 'please explain why this waits'",
            "// Please review this implementation.",
            "const value = 1; // 请帮我修复这个问题",
            "`const value = 1; 请帮我修复这个问题`",
            "const prompt = \"Please explain why this parser waits.\";",
            "please_explain_parser(thing)",
            "check_status(user_id)",
            "review_handler(item, context)",
            "检查一下这个函数(参数)",
            "object.review_handler(item, context)",
            "Please check_status(user_id)",
            "[review, handler, context]",
            "review:\n  - alpha\n  - beta",
            "[please, review, handler]",
            "please review:\n  - alpha\n  - beta",
            "please review:\n  - alpha",
        ] {
            assert!(
                !has_strong_natural_language_evidence(transcript),
                "unexpected request evidence for code or ambiguous text {transcript:?}"
            );
        }
    }

    #[test]
    fn natural_request_with_command_flag_is_not_misclassified_as_code() {
        for transcript in [
            "帮我看一下为什么 --force 不生效。",
            "Please explain why --force is ignored by this command.",
        ] {
            assert!(
                has_strong_natural_language_evidence(transcript),
                "request prose containing a command flag remains natural language: {transcript:?}"
            );
        }
        assert!(!has_strong_natural_language_evidence("git push --force"));
        assert!(!has_strong_natural_language_evidence(
            "Please fix this bug in counter-- // the counter should decrement"
        ));
        assert!(!has_strong_natural_language_evidence(
            "--counter // please fix this bug"
        ));
    }

    #[test]
    fn recognizes_requests_while_ignoring_inline_code_and_preserving_technical_tokens() {
        for transcript in [
            "请帮我检查 `--dry-run` 为什么会跳过 /Users/test/project/api/v2 的配置。",
            "Please explain why `/v2/users` returns 429 after updating API version 2.4.1.",
            "帮我分析 `fn update()` 在 HTTP 429 时为什么没有恢复。",
        ] {
            assert!(
                has_strong_natural_language_evidence(transcript),
                "expected natural-language request evidence for {transcript:?}"
            );
        }
    }
}
