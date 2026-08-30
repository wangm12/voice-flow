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
        expected: "1. 我们需要先验证数据\n2. 再发布结果",
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
        expected: "1. 完成设计\n2. 写测试\n3. 发布",
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
    CleanupCase {
        name: "zh_wechat_casual",
        raw: "好的哈哈我晚点回你",
        expected: "好的哈哈我晚点回你",
        protected_tokens: &["哈哈", "晚点"],
        context_family: "personal_chat",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "zh_wechat_swear_kept",
        raw: "这破需求我晚点再改",
        expected: "这破需求我晚点再改",
        protected_tokens: &["破"],
        context_family: "personal_chat",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "work_chat_not_email",
        raw: "can you ping Maya about the launch when you have a minute",
        expected: "Can you ping Maya about the launch when you have a minute?",
        protected_tokens: &["Maya"],
        context_family: "work_chat",
        allow_rewrite: true,
        preserve_structure: true,
    },
    CleanupCase {
        name: "work_chat_thanks_stays_chat",
        raw: "thanks Maya can you ping the launch when you have a minute",
        expected: "Thanks Maya, can you ping the launch when you have a minute?",
        protected_tokens: &["Maya"],
        context_family: "work_chat",
        allow_rewrite: true,
        preserve_structure: true,
    },
    CleanupCase {
        name: "spoken_newline",
        raw: "the reading club new line should be tomorrow",
        expected: "The reading club\nshould be tomorrow.",
        protected_tokens: &[],
        context_family: "general",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "spoken_new_paragraph",
        raw: "第一段内容 新段落 第二段内容",
        expected: "第一段内容\n\n第二段内容。",
        protected_tokens: &[],
        context_family: "document",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "list_false_positive_first_thought",
        raw: "我第一个想到的是先验证数据",
        expected: "我第一个想到的是先验证数据。",
        protected_tokens: &[],
        context_family: "document",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "list_incomplete_only_first",
        raw: "第一我们先验证数据",
        expected: "第一我们先验证数据。",
        protected_tokens: &[],
        context_family: "document",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "email_with_spoken_greeting",
        raw: "Hi John looking forward to chatting tomorrow Best Allan",
        expected: "Hi John\nlooking forward to chatting tomorrow\nBest\nAllan",
        protected_tokens: &["John", "Allan"],
        context_family: "email",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "email_without_greeting",
        raw: "looking forward to chatting tomorrow",
        expected: "Looking forward to chatting tomorrow.",
        protected_tokens: &[],
        context_family: "email",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "wechat_with_ordinals",
        raw: "第一完成设计第二写测试",
        expected: "1. 完成设计\n2. 写测试。",
        protected_tokens: &[],
        context_family: "personal_chat",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "wechat_keeps_spoken_newline",
        raw: "先发你 换行 明天再改",
        expected: "先发你\n明天再改",
        protected_tokens: &[],
        context_family: "personal_chat",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "zh_restatement_after_bu_dui",
        raw: "做一个完整的 cloud 测试，不对，不对，不对。做一个完整的 cursor 测试",
        expected: "做一个完整的 cursor 测试。",
        protected_tokens: &[],
        context_family: "prompt_or_code",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "zh_bu_dui_as_question",
        raw: "看上它对不对",
        expected: "看它对不对？",
        protected_tokens: &[],
        context_family: "prompt_or_code",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "zh_bu_dui_as_topic",
        raw: "你看我说不对的时候应该把之前的删了",
        expected: "你看我说不对的时候应该把之前的删了。",
        protected_tokens: &[],
        context_family: "prompt_or_code",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "en_scratch_that_restate",
        raw: "write a cloud test scratch that write a cursor test",
        expected: "Write a cursor test.",
        protected_tokens: &[],
        context_family: "prompt_or_code",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "en_actually_is_content",
        raw: "I actually enjoyed the movie",
        expected: "I actually enjoyed the movie.",
        protected_tokens: &[],
        context_family: "prompt_or_code",
        allow_rewrite: false,
        preserve_structure: true,
    },
    CleanupCase {
        name: "zh_oh_bu_dui_list_backtrack",
        raw: "我们来测试一下,看一下具体的 ASR 流程。1. 是 prompt。看一下 prompt 到底怎么样。哦,不对, 2. 是 system。哦,不对, 3. 是 system prompt,看一下具体的流程怎么样。4. 是看一下 style,和它的逻辑是怎么样。5. 是看一下它整个的识别率怎么样。",
        expected: "我们来测试一下，看一下具体的 ASR 流程。\n1. 是 system prompt，看一下具体的流程怎么样。\n2. 是看一下 style 和它的逻辑是怎么样。\n3. 是看一下它整个的识别率怎么样。",
        protected_tokens: &["ASR"],
        context_family: "prompt_or_code",
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
        assert!(CASES
            .iter()
            .any(|case| case.context_family == "personal_chat"));
        assert!(CASES.iter().any(|case| case.name == "zh_self_correction"));
        assert!(CASES.iter().any(|case| case.name == "long_chunk_boundary"));
        assert!(CASES.iter().any(|case| case.name == "spoken_newline"));
        assert!(CASES.iter().any(|case| case.name == "wechat_with_ordinals"));
        assert!(CASES
            .iter()
            .any(|case| case.name == "work_chat_thanks_stays_chat"));
        assert!(CASES
            .iter()
            .any(|case| case.name == "email_with_spoken_greeting"));
        assert!(CASES
            .iter()
            .any(|case| case.name == "zh_restatement_after_bu_dui"));
        assert!(CASES
            .iter()
            .any(|case| case.name == "zh_bu_dui_as_question"));
        assert!(CASES.iter().any(|case| case.name == "en_scratch_that_restate"));
    }

    #[test]
    fn self_correction_fixtures_drop_restatement_but_keep_bu_dui_as_content() {
        let restatement = CASES
            .iter()
            .find(|case| case.name == "zh_restatement_after_bu_dui")
            .expect("zh_restatement_after_bu_dui");
        assert_eq!(restatement.context_family, "prompt_or_code");
        assert!(!restatement.expected.contains("cloud"));
        assert!(!restatement.expected.contains("不对"));
        assert!(restatement.expected.contains("cursor"));

        let question = CASES
            .iter()
            .find(|case| case.name == "zh_bu_dui_as_question")
            .expect("zh_bu_dui_as_question");
        assert!(question.expected.contains("不对"));
        assert!(question.expected.contains('？'));

        let topic = CASES
            .iter()
            .find(|case| case.name == "zh_bu_dui_as_topic")
            .expect("zh_bu_dui_as_topic");
        assert!(topic.expected.contains("不对"));
        assert!(topic.expected.contains("删了"));

        let contrast = CASES
            .iter()
            .find(|case| case.name == "amount_preservation")
            .expect("amount_preservation");
        assert!(contrast.expected.contains("1250"));
        assert!(contrast.expected.contains("不是"));
        assert!(contrast.expected.contains("1500"));

        let scratch = CASES
            .iter()
            .find(|case| case.name == "en_scratch_that_restate")
            .expect("en_scratch_that_restate");
        assert!(!scratch.expected.to_ascii_lowercase().contains("cloud"));
        assert!(!scratch.expected.to_ascii_lowercase().contains("scratch"));
        assert!(scratch.expected.to_ascii_lowercase().contains("cursor"));

        let actually = CASES
            .iter()
            .find(|case| case.name == "en_actually_is_content")
            .expect("en_actually_is_content");
        assert!(actually.expected.to_ascii_lowercase().contains("actually"));
        assert!(actually.expected.to_ascii_lowercase().contains("enjoyed"));

        let list_backtrack = CASES
            .iter()
            .find(|case| case.name == "zh_oh_bu_dui_list_backtrack")
            .expect("zh_oh_bu_dui_list_backtrack");
        assert!(!list_backtrack.expected.contains("哦"));
        assert!(!list_backtrack.expected.contains("不对"));
        assert!(!list_backtrack.expected.contains("是 prompt"));
        assert!(list_backtrack.expected.contains("system prompt"));
        assert!(list_backtrack.expected.contains("ASR"));
    }

    #[test]
    fn personal_chat_cases_keep_casual_voice() {
        let casual = CASES
            .iter()
            .find(|case| case.name == "zh_wechat_casual")
            .expect("zh_wechat_casual");
        assert_eq!(casual.raw, "好的哈哈我晚点回你");
        assert_eq!(casual.context_family, "personal_chat");
        assert!(!casual.allow_rewrite);
        assert!(casual.expected.contains("哈哈"));
        assert!(casual.expected.contains("晚点"));
        assert!(!casual.expected.contains("您好"));
        assert!(!casual.expected.contains("稍后回复"));

        let swear = CASES
            .iter()
            .find(|case| case.name == "zh_wechat_swear_kept")
            .expect("zh_wechat_swear_kept");
        assert_eq!(swear.context_family, "personal_chat");
        assert!(!swear.allow_rewrite);
        assert!(!swear.expected.contains("有待商榷"));

        let work = CASES
            .iter()
            .find(|case| case.name == "work_chat_not_email")
            .expect("work_chat_not_email");
        assert_eq!(work.context_family, "work_chat");
        let lower = work.expected.to_ascii_lowercase();
        assert!(!lower.contains("dear "));
        assert!(!lower.contains("best regards"));
        assert!(!lower.starts_with("hello"));
        assert!(!lower.starts_with("hi "));

        let thanks = CASES
            .iter()
            .find(|case| case.name == "work_chat_thanks_stays_chat")
            .expect("work_chat_thanks_stays_chat");
        assert_eq!(thanks.context_family, "work_chat");
        assert!(thanks.raw.to_ascii_lowercase().contains("thanks"));
        assert!(!thanks.expected.contains('\n'));
        assert!(!thanks.expected.to_ascii_lowercase().contains("best regards"));

        let ordinals = CASES
            .iter()
            .find(|case| case.name == "wechat_with_ordinals")
            .expect("wechat_with_ordinals");
        assert_eq!(ordinals.context_family, "personal_chat");
        assert!(ordinals.expected.contains("1. "));
        assert!(ordinals.expected.contains('\n'));
        assert!(!ordinals.expected.contains("您好"));
    }

    #[test]
    fn layout_regression_cases_follow_spoken_structure() {
        use crate::context::{builtin_family_for_id, ContextFamily};
        use crate::spoken_layout::apply_after_punctuation;

        let layout = |name: &str, confidence: f32| {
            let case = CASES
                .iter()
                .find(|item| item.name == name)
                .unwrap_or_else(|| panic!("missing case {name}"));
            let family =
                builtin_family_for_id(case.context_family).unwrap_or(ContextFamily::General);
            apply_after_punctuation(case.raw, family, confidence)
        };

        assert_eq!(
            layout("spoken_newline", 1.0),
            "the reading club\nshould be tomorrow"
        );
        assert_eq!(
            layout("spoken_new_paragraph", 1.0),
            "第一段内容\n\n第二段内容"
        );
        assert_eq!(
            layout("document_paragraph", 0.9),
            "1. 我们需要先验证数据\n2. 再发布结果"
        );
        assert_eq!(
            layout("bullet_like_actions", 0.9),
            "1. 完成设计\n2. 写测试\n3. 发布"
        );
        assert_eq!(
            layout("list_false_positive_first_thought", 0.9),
            "我第一个想到的是先验证数据"
        );
        assert_eq!(
            layout("list_incomplete_only_first", 0.9),
            "第一我们先验证数据"
        );
        assert_eq!(
            layout("chunk_boundary_punctuation", 0.9),
            "第一部分是数据库迁移。第二部分，嗯，是回滚方案"
        );
        assert_eq!(
            layout("email_with_spoken_greeting", 0.9),
            "Hi John\nlooking forward to chatting tomorrow\nBest\nAllan"
        );
        assert_eq!(
            layout("email_without_greeting", 0.9),
            "looking forward to chatting tomorrow"
        );
        let wechat_list = layout("wechat_with_ordinals", 0.9);
        assert_eq!(wechat_list, "1. 完成设计\n2. 写测试");
        assert!(!wechat_list.contains("您好"));
        assert_eq!(
            layout("wechat_keeps_spoken_newline", 0.9),
            "先发你\n明天再改"
        );
        assert_eq!(layout("zh_wechat_casual", 0.9), "好的哈哈我晚点回你");
        assert_eq!(
            layout("work_chat_not_email", 0.9),
            "can you ping Maya about the launch when you have a minute"
        );
        assert_eq!(
            layout("work_chat_thanks_stays_chat", 0.9),
            "thanks Maya can you ping the launch when you have a minute"
        );
        assert_eq!(
            layout("zh_oh_bu_dui_list_backtrack", 1.0),
            "我们来测试一下,看一下具体的 ASR 流程。\n1. 是 system prompt,看一下具体的流程怎么样。\n2. 是看一下 style,和它的逻辑是怎么样。\n3. 是看一下它整个的识别率怎么样。"
        );
    }

    #[test]
    fn prepare_spoken_transcript_keeps_restatement_and_content_bu_dui_invariants() {
        use crate::context::{builtin_family_for_id, ContextFamily};
        use crate::prepare_spoken_transcript;

        let prepared = |name: &str, confidence: f32| {
            let case = CASES
                .iter()
                .find(|item| item.name == name)
                .unwrap_or_else(|| panic!("missing case {name}"));
            let family =
                builtin_family_for_id(case.context_family).unwrap_or(ContextFamily::General);
            prepare_spoken_transcript(case.raw, family, confidence)
        };

        let restatement = prepared("zh_restatement_after_bu_dui", 1.0);
        assert!(!restatement.contains("cloud"), "{restatement}");
        assert!(restatement.contains("cursor"), "{restatement}");

        let self_correction = prepared("zh_self_correction", 1.0);
        assert!(!self_correction.contains("周四"), "{self_correction}");
        assert!(self_correction.contains("周五"), "{self_correction}");

        let topic = prepared("zh_bu_dui_as_topic", 1.0);
        assert!(topic.contains("不对"), "{topic}");
        assert!(topic.contains("删了") || topic.contains("之前"), "{topic}");

        let wechat = prepared("zh_wechat_casual", 0.9);
        assert_eq!(wechat, "好的哈哈我晚点回你");

        let list = prepared("zh_oh_bu_dui_list_backtrack", 1.0);
        assert!(list.contains("1. "), "{list}");
        assert!(list.contains("2. "), "{list}");
        assert!(list.contains("3. "), "{list}");
        assert!(list.contains("ASR"), "{list}");
        assert!(!list.contains("cloud"), "{list}");
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
