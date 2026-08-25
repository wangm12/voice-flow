# Voice Harness plan review

Defect-first product/architecture review of `complete_voice_harness_b6de6a80.plan.md` against the current implementation. Not a diff review. Constraints checked: no screenshots / RL / global keylog / uploaded corrections; fail-closed paste; no window title / PID / raw URL to the LLM; full skill (vocab + human-reviewed style); accuracy and cleanup speed both matter.

---

## Findings

[P0] Gate chat skip on a working in-sentence CJK replace — Phase 1 local replace + Phase 3 skip

WeChat (PersonalChat) is the skip-by-default scene and the plan’s own lexicon example (`知呼→知乎`). Phase 1 says to replace only “独立 token（沿用 `extract_tokens` 边界）”. `extract_tokens` in `dictionary_learn.rs` is a learner for *changed spans*: it consumes a whole CJK run and keeps it only if length is 2–8. In `今天去知呼看看吧` the run is 9 characters (no tokens) or, if shorter, one token `去知呼看看` — never `知呼`. The Phase 1 test (`知呼` as the whole utterance) would pass and the real message would not. After Phase 3, cleanup never sees the pair either, so chat accuracy is only Groq prompt bias on the *after* form. That falsifies “准确度主要在 ASR + 本地替换”. Change the plan: specify a CJK matcher (contiguous `before` with a compound rule, not `extract_tokens`); require sentence tests (`今天去知呼看看吧`); make Phase 3’s chat skip a hard gate on those tests. Do not treat `spoken_punctuation`’s “CJK char is a boundary” as the compound rule — that would also rewrite `链接` inside `超链接`.

[P1] Store ranking metadata before ranking — Phase 1 / Phase 4 / 存储

Phase 1’s acceptance test is “微信学的新词能进 ASR 前 32，Slack 旧词让位”, and ranking uses `family` / `bundle` / `last_used_at` / `hits`. Those columns do not exist: `learn_pairs` is `pair_key, before_surface, after_surface, hits, promoted, last_at` (`store.rs`). `build_asr_prompt` only receives `settings.dictionary: &[String]` (oldest-first `.take(32)`). Phase 4 is when family/bundle is written; `last_used_at` is in the storage section but in no phase. Manual / imported words never get a `learn_pairs` row. As written, Phase 1 cannot pass its own test, and switching the ASR source to `learn_pairs` would drop manual entries. Change the plan: Phase 0.5 / fold into Phase 1 — migrate `learn_pairs` with `ALTER` (existing DBs hit `CREATE TABLE IF NOT EXISTS` and will not gain columns), snapshot scope on observe and History confirm, rank a *merged* list (promoted pairs + dictionary-only words). Put the WeChat-vs-Slack eviction test after that writer exists.

[P1] Do not skip PersonalChat if style only enters via LLM — 何时还打 LLM / Style / mermaid

User-locked “full skill” is vocab + style/punctuation/few-shot. Style is written to `AppMapping.style_example_*` and today only appears in the cleanup user message (`llm.rs` “Confirmed style example”). PersonalChat / SocialMedia always skip; WorkChat / Notes skip unless a style example exists. WeChat is `com.tencent.xinWeChat` → `PersonalChat`. Confirming `微信 · 少用句号` therefore never reaches cleanup, while Slack can. The mermaid and the settings copy (“聊天默认不打整理”) contradict the style loop and `docs/competitive-research.md` (微信口癖 / 按 App 换语气). Skip also drops the *existing* PersonalChat writing prompt that is the current anti-email guardrail. Change the plan: use the WorkChat rule for PersonalChat too (LLM when an approved example exists, or when the user asked for cleanup); or apply a local style path; and say explicitly that “始终整理” is required for 微信口癖 if skip stays absolute — then ship that override in the same phase.

[P1] Exempt every spoken command, not only rewrite/shorten/formalize — 何时还打 LLM

`parse_cleanup_intent` already treats leading `整理一下` / `清理一下` as `CleanupOperation::Cleanup` + `SpokenCommand`, plus translate / casualize. The plan only keeps LLM for “改写/缩短/正式化”. In WeChat, “整理一下明天开会的事” is skipped: the command words stay in the paste and no cleanup runs. That is the opposite of user expectation while `cleanup_enabled` is on (“自动去掉口头禅…” in Engine Settings). Change the plan: skip only `IntentSource::Implicit` + `Cleanup`; any `SpokenCommand` (including Cleanup, Translate, Casualize) forces LLM. Do not use “has rewrite intent” as the predicate.

[P1] Keep ignored pairs out of local replace — Phase 1 / `ignore_learn_pair`

Local `知呼→知乎` cannot be implemented from `settings.dictionary` (after-only). It needs `learn_pairs.before_surface`. `ignore_learn_pair` already sets `promoted=1` without inserting the word — the same flag as a real promotion. A replace loop over `promoted` rows would silently apply ignored corrections. Change the plan: replace only when `after` is in `settings.dictionary` (or add a real `ignored` column); never treat `promoted` as “safe to apply”. Document multiple befores per after (current `pair_key` already allows that).

[P1] Snapshot mapping/host at observe time; do not rank browsers by Chrome bundle — 存储 / Phase 4

`maybe_observe_after_paste` only gets `TargetAppGuard` (bundle, optional `browser_host`, no family). Family comes from `resolve_profile` (user mapping, `browser_host`, native bundle, focus). Storing `bundle_id` alone makes every Chrome site share `com.google.Chrome`, so Gmail-learned terms outrank Notion-local terms. History confirm runs in VoiceFlow’s window; using “current app” would tag every chip as General. Change the plan: persist `ContextFamily` + mapping id and/or normalized `browser_host` from the *recording* snapshot (and from the dictation’s stored context on History confirm). Keep raw URL / title / PID off the wire (already true if they stay local). `bundle_id` is ranking-useful only for native apps.

[P1] Do not skip FormFilling with chat — 何时还打 LLM

Form policy is “Return only the concise value appropriate for the focused field.” Skip pastes `我的电话是13800138000` into a phone box. That is an accuracy regression, not a speed win, and it is unlike chat’s “写成邮件” accident. BrowserSearch has the same shape (`帮我搜一下附近的川菜`). Change the plan: keep FormFilling (and likely BrowserSearch) on the LLM path unless a later local extractor exists; skip list should be chat/social/terminal, not “every non-email scene”.

[P1] Do not mine style from undifferentiated `Ambiguous` — Phase 5 / `ObserveOutcome`

`ObserveOutcome::Ambiguous` is the leftover bucket: pure insertion (`好的`→`好的明天见`), multi-token adds, and paragraph rewrites all land here (`observe_does_not_learn_pure_insertion`, `observe_ignores_paragraph_rewrite`). The plan says “Ambiguous 里有一类可以变成 style 信号” without a predicate. A naive Phase 5 writer will promote email rewrites into `style_example` — the exact “永不学：大段改写、变正式” case, then send it to Groq after human confirm if the UI copy is just `微信 · 少用句号 · 2/3`. Change the plan: add `ObserveOutcome::StyleSignal` (or equivalent) that fires only when the token set is almost unchanged and punctuation / whitespace / particle *density* changed; cap excerpts at the changed span (≤120); require a clustering key so “少用句号” and “爱用问号” do not merge; ignore / rewrite stay out.

[P1] Specify skip residue and one decision function for every cleanup site — Phase 3 / `lib.rs` / `history_commands.rs`

There is no `cleanup_decision` function. The plan’s hook is an inline `if settings.cleanup_enabled` in the short path (`lib.rs` ~1999), the long-recording path (~2579), and history retry (`history_commands.rs` ~272). `CleanupDecision::Disabled` still runs `local_cleanup`, which substring-deletes `那个` / `啊` / `嗯` / `就是说` — not “本地 lexicon + 标点即可”, and hostile to `那个项目`. Long WeChat recordings would keep calling the LLM if only the short path is patched. History retry passes `profile: None` and `history_context` is policy JSON only (no family). `始终整理` is specified in prose, not on `AppMapping`, and not in any phase todo; `output_mode=email` would be silently ignored on skipped scenes. Change the plan: one function; skip means spoken punctuation + lexicon replace only (no `local_cleanup` unless separately specified); apply to short, long, and retry; persist family for retry; add `always_cleanup` (or equivalent) on `AppMapping` in the same phase as skip; decide whether global `output_mode` overrides skip.

[P2] Name the families the skip table omits — 何时还打 LLM

`ContextFamily` also has `ProjectManagement`, `DeveloperCollaboration`, and `General`. Unspecified defaults will either skip unknown apps (lossy) or always LLM (fine, but must be written down). `DeveloperCollaboration` is closer to WorkChat than to PromptOrCode.

[P2] Specify a mixed ASR budget so global names still fit — 出站 ASR

“人名在微信学到，Slack 里也该认” is true for local replace if the before appears. It is false for Whisper if the current family fills all 32 slots. Write the mix (e.g. N current mapping/host, remainder global by `last_used_at` / `hits`). Bump `last_used_at` on ASR/prompt hit as well as replace, or correctly recognized names go stale.

[P2] Fix the “already built” inventory before sequencing work — plan intro / 分阶段

Phase 0 lexicon observe is real. These are not: a `cleanup_decision` API; ranking metadata; `始终整理`; style drafts; `Ambiguous` as a style channel; cleanup as “前 32 条” from `learn_pairs` (it is oldest `dictionary` words, 32 / 2048 chars). `style_example_*` (2000 cap) and “pending 不进 prompt” are already true. `SYSTEM_PROMPT` already leaves style in the user message. Correcting this list avoids implementing Phase 1/3 against APIs that do not exist.

[P2] Include `style_drafts` in `clear_all_data` and keep family-default style from leaking — 存储 / Style / `store.rs`

`clear_all_data` deletes `learn_pairs` only. A new excerpt table would survive “clear all data” with chat text. Confirm should write the *mapping* pair only; a family-default `style_example` would apply 微信口癖 to iMessage / Discord. Ship Phase 5 after the skip/style rule in Finding 3 is decided, or WorkChat/Notes go raw for a whole increment (Phase 3 then 5).

[P2] Gate scene skip on the same 0.75 confidence as `cleanup_profile_for` — 何时还打 LLM / `lib.rs`

Skip-by-family at low confidence will skip a misdetected Email-as-chat, or LLM a misdetected chat-as-Email (the accident the skip exists to stop). Reuse the existing profile confidence gate: low confidence → current faithful cleanup, not scene skip.

---

## Overall assessment

The direction is right: one global lexicon, scope for ranking only, cleanup on hit terms, style human-reviewed, no cloud corrections. Phase 0 is actually landed. The plan as sequenced will not deliver “accurate *and* fast” on WeChat. Chat skip is specified before a CJK replace that works in sentences, before scope exists to rank, and in a way that orphans the style skill the user asked for. Form skip copies the chat rule onto a scene that still needs LLM extraction. `promoted` cannot mean both “user ignored this” and “safe to auto-replace.”

Fix order implied by the findings: (1) lexicon data model + CJK replace + ignore semantics, with sentence tests; (2) merged, scoped ASR ranking; (3) cleanup hit-only; (4) skip rules that preserve spoken commands, Form/Search, confidence, and an explicit per-App override, applied at every call site; (5) a real `StyleSignal` and mapping-local confirm. Do not ship Phase 3 chat skip until (1) is green.

## Residual risks

- Groq Whisper prompt bias on after-forms is weak for Chinese homophones; even a correct replace list only helps when the before string actually appears.
- One `style_example` slot means a later confirm overwrites the previous habit; the UI must say that.
- Human-confirmed excerpts still go to Groq; review UI has to show the real text, not only `少用句号`.
- Compound CJK false positives (`链接` in `超链接`) remain even after leaving `extract_tokens`; the matcher needs an explicit non-goal or a longest-lexicon rule.
- Incremental phase shipping can land ranking-without-columns or skip-without-replace if the gates above are treated as optional.
