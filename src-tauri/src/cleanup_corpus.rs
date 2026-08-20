//! Small, de-identified cleanup quality corpus.
//!
//! The expected values are review targets, not claims about a particular
//! provider response. Keeping them in source makes prompt/regression changes
//! auditable without storing recordings, app titles, or user history.

#[derive(Debug, Clone, Copy)]
struct CleanupCase {
    name: &'static str,
    raw: &'static str,
    expected: &'static str,
    protected_tokens: &'static [&'static str],
    context_family: &'static str,
    allow_rewrite: bool,
    preserve_structure: bool,
}

const CASES: &[CleanupCase] = &[
    CleanupCase {
        name: "zh_self_correction",
        raw: "嗯，我周四，不对，周五下午开会",
        expected: "我周五下午开会",
        protected_tokens: &[],
        context_family: "calendar_task",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "en_fillers",
        raw: "um, I mean, send the update to the team tomorrow",
        expected: "Send the update to the team tomorrow.",
        protected_tokens: &[],
        context_family: "work_chat",
        allow_rewrite: true,
        preserve_structure: true,
    },
    CleanupCase {
        name: "mixed_language_code",
        raw: "帮我 fix 这个 TypeScript error",
        expected: "帮我 fix 这个 TypeScript error",
        protected_tokens: &["TypeScript", "error"],
        context_family: "prompt_or_code",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "url_and_path",
        raw: "打开 https://docs.example.com 然后运行 /Users/test/app --dry-run",
        expected: "打开 https://docs.example.com，然后运行 /Users/test/app --dry-run。",
        protected_tokens: &["https://docs.example.com", "/Users/test/app", "--dry-run"],
        context_family: "prompt_or_code",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "email_body",
        raw: "那个请告诉 Mingjie 我周五之前会发 final report",
        expected: "请告诉 Mingjie，我周五之前会发 final report。",
        protected_tokens: &["Mingjie", "final", "report"],
        context_family: "email",
        allow_rewrite: true,
        preserve_structure: true,
    },
    CleanupCase {
        name: "mixed_repeated_phrase",
        raw: "我们需要 review review 这个 PR 然后 merge it",
        expected: "我们需要 review 这个 PR，然后 merge it。",
        protected_tokens: &["PR", "merge"],
        context_family: "developer_collaboration",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "long_chunk_boundary",
        raw: "先把本周数据整理一下。然后，不对，应该是上周数据，发给财务团队。",
        expected: "先把上周数据整理一下，发给财务团队。",
        protected_tokens: &[],
        context_family: "document",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "zh_fillers_and_spacing",
        raw: "那个 嗯 我们明天上午 10 点开会",
        expected: "我们明天上午 10 点开会。",
        protected_tokens: &["10"],
        context_family: "calendar_task",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "zh_accidental_repetition",
        raw: "请请把这个文档发给团队",
        expected: "请把这个文档发给团队。",
        protected_tokens: &[],
        context_family: "work_chat",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "en_self_correction",
        raw: "Send it on Tuesday, no Wednesday morning",
        expected: "Send it on Wednesday morning.",
        protected_tokens: &[],
        context_family: "email",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "mixed_date_and_name",
        raw: "告诉 Mike 我们 5 月 12 日见",
        expected: "告诉 Mike，我们 5 月 12 日见。",
        protected_tokens: &["Mike", "5", "12"],
        context_family: "work_chat",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "amount_preservation",
        raw: "预算是 1250 美元，不是 1500 美元",
        expected: "预算是 1250 美元，不是 1500 美元。",
        protected_tokens: &["1250", "1500"],
        context_family: "document",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "proper_name_preservation",
        raw: "把合同发给 Mingjie Wang 然后提醒 Olivia",
        expected: "把合同发给 Mingjie Wang，然后提醒 Olivia。",
        protected_tokens: &["Mingjie", "Wang", "Olivia"],
        context_family: "email",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "email_address",
        raw: "发给 api@example.com 然后抄送 ops@example.org",
        expected: "发给 api@example.com，然后抄送 ops@example.org。",
        protected_tokens: &["api@example.com", "ops@example.org"],
        context_family: "email",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "version_number",
        raw: "升级到 version 2.4.1 之后再测试",
        expected: "升级到 version 2.4.1 之后再测试。",
        protected_tokens: &["2.4.1"],
        context_family: "prompt_or_code",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "command_flags",
        raw: "运行 cargo test --manifest-path src-tauri/Cargo.toml --all",
        expected: "运行 cargo test --manifest-path src-tauri/Cargo.toml --all。",
        protected_tokens: &[
            "cargo",
            "test",
            "--manifest-path",
            "src-tauri/Cargo.toml",
            "--all",
        ],
        context_family: "terminal",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "url_and_email",
        raw: "把 https://voiceflow.app 的链接发给 team@example.com",
        expected: "把 https://voiceflow.app 的链接发给 team@example.com。",
        protected_tokens: &["https://voiceflow.app", "team@example.com"],
        context_family: "work_chat",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "unix_path",
        raw: "打开 /Users/mingjie/Documents/personal/voice-flow/src-tauri",
        expected: "打开 /Users/mingjie/Documents/personal/voice-flow/src-tauri。",
        protected_tokens: &["/Users/mingjie/Documents/personal/voice-flow/src-tauri"],
        context_family: "terminal",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "chat_concise",
        raw: "um can you send the notes to the team when you have a chance",
        expected: "Can you send the notes to the team when you have a chance?",
        protected_tokens: &[],
        context_family: "work_chat",
        allow_rewrite: true,
        preserve_structure: true,
    },
    CleanupCase {
        name: "search_query",
        raw: "帮我搜索 2026 年 8 月的 macOS Accessibility 文档",
        expected: "搜索 2026 年 8 月的 macOS Accessibility 文档",
        protected_tokens: &["2026", "8", "macOS", "Accessibility"],
        context_family: "browser_search",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "document_paragraph",
        raw: "第一点我们需要先验证数据第二点再发布结果",
        expected: "第一点，我们需要先验证数据；第二点，再发布结果。",
        protected_tokens: &[],
        context_family: "document",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "code_identifier",
        raw: "把 parseCleanupIntent 的测试加到 llm.rs",
        expected: "把 parseCleanupIntent 的测试加到 llm.rs。",
        protected_tokens: &["parseCleanupIntent", "llm.rs"],
        context_family: "prompt_or_code",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "terminal_pipeline",
        raw: "先运行 rg TODO src 然后执行 cargo fmt",
        expected: "先运行 rg TODO src，然后执行 cargo fmt。",
        protected_tokens: &["rg", "TODO", "src", "cargo", "fmt"],
        context_family: "terminal",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "explicit_formalize",
        raw: "帮我把这封邮件写得正式一点，我想告诉 Mike 会议改到周五",
        expected: "我想告诉 Mike 会议改到周五",
        protected_tokens: &["Mike"],
        context_family: "email",
        allow_rewrite: true,
        preserve_structure: true,
    },
    CleanupCase {
        name: "explicit_shorten",
        raw: "帮我缩短这段话，项目已经完成初步测试但还需要修复两个问题",
        expected: "项目已完成初步测试，但还需修复两个问题",
        protected_tokens: &["两个"],
        context_family: "document",
        allow_rewrite: true,
        preserve_structure: true,
    },
    CleanupCase {
        name: "explicit_casualize",
        raw: "make it casual: I would appreciate your response by Friday",
        expected: "Could you get back to me by Friday?",
        protected_tokens: &[],
        context_family: "email",
        allow_rewrite: true,
        preserve_structure: true,
    },
    CleanupCase {
        name: "explicit_translate",
        raw: "translate to Japanese: the meeting moved to Friday",
        expected: "会議は金曜日に変更されました。",
        protected_tokens: &[],
        context_family: "email",
        allow_rewrite: true,
        preserve_structure: true,
    },
    CleanupCase {
        name: "ambiguous_instruction_word",
        raw: "我的工作流程需要改写一下才能适应新团队",
        expected: "我的工作流程需要改写一下才能适应新团队。",
        protected_tokens: &[],
        context_family: "document",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "sentence_mentions_rewrite",
        raw: "我想改写一下我的工作流程，然后再和团队讨论",
        expected: "我想改写一下我的工作流程，然后再和团队讨论。",
        protected_tokens: &[],
        context_family: "work_chat",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "quoted_command",
        raw: "他说请把这段话改写得正式一点，但我只是记录原话",
        expected: "他说请把这段话改写得正式一点，但我只是记录原话。",
        protected_tokens: &[],
        context_family: "notes_journaling",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "email_quote_preserved",
        raw: "邮件里写着 do not deploy to production before approval",
        expected: "邮件里写着 do not deploy to production before approval。",
        protected_tokens: &["do", "not", "deploy", "production", "approval"],
        context_family: "email",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "low_confidence_general",
        raw: "嗯把这个想法记下来以后再决定要不要展开",
        expected: "把这个想法记下来，以后再决定要不要展开。",
        protected_tokens: &[],
        context_family: "general",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "chunk_boundary_sentence",
        raw: "我们先完成登录流程，然后下一段，不对，先完成权限校验",
        expected: "我们先完成登录流程，然后先完成权限校验。",
        protected_tokens: &[],
        context_family: "developer_collaboration",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "chunk_boundary_punctuation",
        raw: "第一部分是数据库迁移。第二部分，嗯，是回滚方案",
        expected: "第一部分是数据库迁移。第二部分是回滚方案。",
        protected_tokens: &[],
        context_family: "document",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "english_email_request",
        raw: "uh please let Sarah know that the invoice is ready",
        expected: "Please let Sarah know that the invoice is ready.",
        protected_tokens: &["Sarah"],
        context_family: "email",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "meeting_notes",
        raw: "今天的会议决定是周三发布，然后 Alex 负责检查回滚",
        expected: "今天的会议决定是周三发布；Alex 负责检查回滚。",
        protected_tokens: &["Alex"],
        context_family: "calendar_task",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "bullet_like_actions",
        raw: "第一完成设计第二写测试第三发布",
        expected: "第一，完成设计；第二，写测试；第三，发布。",
        protected_tokens: &[],
        context_family: "project_management",
        allow_rewrite: true,
        preserve_structure: true,
    },
    CleanupCase {
        name: "social_reply",
        raw: "那个这个周末终于可以休息一下了",
        expected: "这个周末终于可以休息一下了。",
        protected_tokens: &[],
        context_family: "social_media",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "support_response",
        raw: "我们已经收到你的请求会在周五前回复",
        expected: "我们已经收到你的请求，会在周五前回复。",
        protected_tokens: &[],
        context_family: "customer_support",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "form_value",
        raw: "我的地址是 123 Main Street Apt 4B",
        expected: "123 Main Street Apt 4B",
        protected_tokens: &["123", "4B"],
        context_family: "form_filling",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "calendar_attendees",
        raw: "下周一上午九点和 Mike 还有 Olivia 开会",
        expected: "下周一上午九点和 Mike、Olivia 开会。",
        protected_tokens: &["Mike", "Olivia"],
        context_family: "calendar_task",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "mixed_language_product",
        raw: "把 VoiceFlow cleanup 的 latency 降到 200ms 以下",
        expected: "把 VoiceFlow cleanup 的 latency 降到 200ms 以下。",
        protected_tokens: &["VoiceFlow", "cleanup", "latency", "200ms"],
        context_family: "developer_collaboration",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "question_punctuation",
        raw: "你能不能明天把报告发给我",
        expected: "你能不能明天把报告发给我？",
        protected_tokens: &[],
        context_family: "work_chat",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "false_start_without_change",
        raw: "我想说的是我们今天下午可以开始测试",
        expected: "我想说的是，我们今天下午可以开始测试。",
        protected_tokens: &[],
        context_family: "document",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "url_with_query",
        raw: "打开 https://example.com/search?q=voiceflow&page=2",
        expected: "打开 https://example.com/search?q=voiceflow&page=2。",
        protected_tokens: &["https://example.com/search?q=voiceflow&page=2"],
        context_family: "browser_search",
        allow_rewrite: false,
        preserve_structure: true,
    },
];

#[cfg(test)]
mod tests {
    use super::CASES;

    #[test]
    fn corpus_covers_the_p0_quality_shapes() {
        assert!(CASES.len() >= 40);
        assert!(CASES.iter().any(|case| case.context_family == "email"));
        assert!(CASES
            .iter()
            .any(|case| case.context_family == "prompt_or_code"));
        assert!(CASES.iter().any(|case| case.name == "zh_self_correction"));
        assert!(CASES.iter().any(|case| case.name == "long_chunk_boundary"));
    }

    #[test]
    fn corpus_expected_text_preserves_declared_tokens() {
        for case in CASES {
            assert!(!case.raw.trim().is_empty(), "{} has no raw text", case.name);
            assert!(
                !case.expected.trim().is_empty(),
                "{} has no target",
                case.name
            );
            for token in case.protected_tokens {
                assert!(
                    case.expected.contains(token),
                    "{} dropped protected token {token}",
                    case.name
                );
            }
            if !case.allow_rewrite {
                assert!(
                    case.preserve_structure,
                    "{} must preserve structure",
                    case.name
                );
            }
        }
    }
}
