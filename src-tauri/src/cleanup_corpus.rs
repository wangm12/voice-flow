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
];

#[cfg(test)]
mod tests {
    use super::CASES;

    #[test]
    fn corpus_covers_the_p0_quality_shapes() {
        assert!(CASES.len() >= 7);
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
