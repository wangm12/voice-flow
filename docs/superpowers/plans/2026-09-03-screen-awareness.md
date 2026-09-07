# Screen Awareness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Adaptive Accessibility screen text on every dictate (fail-open), optional on-device window OCR when that text is thin, and a separate look-at-screen hotkey that never runs on the default dictate path.

**Architecture:** New `screen_text.rs` builds an in-memory `ScreenTextContext` from fixture-friendly extractors (live AX behind a trait). Caps: 40 tokens, 2000 chars. Tokens go to ASR prompt + cleanup `visible_context` + cascade noun count. Phase 2 is a one-shot `window_capture` of the locked `window_id` plus Vision OCR; the image is dropped. Phase 3 is a new hotkey + preview, not callable from `dictation::stop`.

**Tech Stack:** Tauri 2 + Rust, React 19 + Vitest. CI uses fixtures only (no live AX, mic, or Screen Recording). Verify: `cargo test --manifest-path src-tauri/Cargo.toml --lib`, `npm test -- --run`, `npm run lint`.

**Spec:** [2026-09-03-screen-awareness-design.md](../specs/2026-09-03-screen-awareness-design.md)  
**Companion plan:** [2026-09-03-dictation-cleanup-enhancement.md](2026-09-03-dictation-cleanup-enhancement.md)

## Global Constraints

- Do not commit unless the user asks this tick to commit.
- Default dictate path NEVER screenshots and NEVER calls Phase 3 capture.
- Phase 3 must not be callable from `dictation::stop`.
- `ScreenTextContext` never crosses Tauri IPC as raw bubble/email text. HUD may show a count only (`看见 6 个词`).
- Never send window title, PID, or raw URL to ASR/LLM.
- Other people's on-screen sentences: this utterance only. Never style few-shots, History, or exports.
- Screen names may seed lexicon (2× classified person name, else 3×). Not style.
- Layer 1 timeout 350ms; any error → empty context, dictation continues.
- Image never on disk, never in History, never uploaded (Phase 2). Phase 3 uploads one window image only to the user-configured vision provider after preview rules.
- Do not copy GPLv3 source. No Python. No full-display capture. No continuous recording.
- Terminal / Form / Secure / banking presets → empty Layer 1. No OCR.
- Loop implementers: TDD; skip git commit steps unless asked.
- Implement Phase 1 before Phase 2 before Phase 3. Update `asr-cleanup-later-and-wont.md` in the same change set as Phase 3.

## File map

- Create: `src-tauri/src/screen_text.rs` — `ScreenTextContext`, extractors, caps, fixtures
- Create: `src-tauri/src/window_capture.rs` — Phase 2/3 one-shot window PNG in memory (Phase 2+)
- Modify: `src-tauri/src/lib.rs` — `mod screen_text`; lock-time extract; do not call capture from stop
- Modify: `src-tauri/src/lexicon.rs` — tokens first in `build_asr_prompt_shaped`
- Modify: `src-tauri/src/llm.rs` — consume `visible_context` (companion Task 2)
- Modify: `src-tauri/src/permissions.rs` — Screen Recording preflight (Phase 2)
- Modify: `src-tauri/Info.plist` — `NSScreenCaptureUsageDescription` (Phase 2)
- Modify: `src-tauri/src/store.rs` — `window_ocr_enabled` default false; `screen_action_hotkey`; `vision_provider` / `vision_model`
- Modify: `src-tauri/src/hotkey.rs` / `selected_action.rs` — Phase 3 gesture
- Modify: `src/components/SelectedPreviewDialog.tsx` — thumbnail + Replace / Copy / Cancel
- Modify: `docs/privacy.md`, `docs/end-to-end-workflows.md`, `docs/asr-cleanup-later-and-wont.md`, `docs/context-e2e-checklist.md` §18 (Phase 3)

Normative settings:

- `window_ocr_enabled`: default `false`
- `screen_action_hotkey`: empty until the user records one
- `vision_provider` / `vision_model`: empty until configured

---

### Task 1: ScreenTextContext + family extractors (fixtures)

**Files:** Create `src-tauri/src/screen_text.rs`; `mod screen_text;` in `lib.rs`

**Interfaces:**

```rust
pub const MAX_TOKENS: usize = 40;
pub const MAX_CHARS: usize = 2000;
pub const EXTRACT_TIMEOUT: Duration = Duration::from_millis(350);

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScreenTextContext {
    pub tokens: Vec<String>,
    pub snippets: Vec<String>,
    pub family: ContextFamily,
    pub source: ScreenTextSource, // Ax | AxOcr
    pub truncated: bool,
}

impl ScreenTextContext {
    pub fn proper_noun_count(&self) -> usize { self.tokens.len() }
    pub fn usable_chars(&self) -> usize { /* tokens + snippets chars */ }
    pub fn visible_context_text(&self) -> String { /* join, still ≤ MAX_CHARS */ }
    pub fn is_thin(&self) -> bool { self.usable_chars() < 20 }
}

pub struct AxWindowFixture {
    pub family: ContextFamily,
    pub counterpart: Option<String>,
    pub bubbles: Vec<String>,          // last 1–2 visible
    pub email_recipients: Vec<String>,
    pub email_subject: Option<String>,
    pub ide_filenames: Vec<String>,    // must include extension, no spaces
    pub ide_symbols: Vec<String>,
    pub selected_text: Option<String>,
    pub document_name: Option<String>,
    pub focused_role: String,          // "AXTextField" / "AXSecureTextField"
    pub secure: bool,
    pub banking_preset: bool,
    pub window_title: String,          // must never appear in visible_context_text
    pub raw_url: Option<String>,       // must never appear
    pub pid: i32,                      // must never appear
}

pub fn extract_from_fixture(fix: &AxWindowFixture) -> ScreenTextContext
```

Family rules (verbatim from spec):

| Family | Read |
| --- | --- |
| PersonalChat / WorkChat / SocialMedia | counterpart + last 1–2 bubbles, each truncated |
| Email | recipients + subject only |
| PromptOrCode / DeveloperCollaboration | filenames with extension and no spaces; selection; nearby symbols |
| Document / Notes | document name + selection |
| BrowserSearch | selection only here (host stays Layer 0) |
| Terminal / FormFilling | empty |
| Secure / banking preset / `AXSecureTextField` | empty |

Strip Notion-like placeholders (`Reply to Claude`, empty hint). Cap tokens at 40 and total chars at 2000; set `truncated`.

- [x] **Step 1: Write failing tests** in `screen_text.rs`:

```rust
#[test]
fn chat_reads_counterpart_and_two_bubbles() {
    let ctx = extract_from_fixture(&AxWindowFixture {
        family: ContextFamily::PersonalChat,
        counterpart: Some("晓雯".into()),
        bubbles: vec!["在吗".into(), "晚点回你".into()],
        window_title: "晓雯 - 微信".into(),
        raw_url: Some("https://wx.qq.com/chat/secret".into()),
        pid: 4242,
        ..empty_fix()
    });
    assert!(ctx.tokens.iter().any(|t| t == "晓雯"));
    assert_eq!(ctx.snippets.len(), 2);
    let text = ctx.visible_context_text();
    assert!(!text.contains("微信"));
    assert!(!text.contains("4242"));
    assert!(!text.contains("https://"));
}

#[test]
fn email_excludes_body() {
    let mut fix = empty_fix();
    fix.family = ContextFamily::Email;
    fix.email_recipients = vec!["alex@example.com".into()];
    fix.email_subject = Some("Q3 plan".into());
    fix.bubbles = vec!["THIS IS THE BODY AND MUST NOT APPEAR".into()];
    let ctx = extract_from_fixture(&fix);
    assert!(ctx.tokens.iter().any(|t| t.contains("alex@example.com")));
    assert!(ctx.snippets.iter().any(|s| s.contains("Q3 plan")));
    assert!(!ctx.visible_context_text().contains("THIS IS THE BODY"));
}

#[test]
fn ide_filenames_need_extension_and_no_spaces() {
    let mut fix = empty_fix();
    fix.family = ContextFamily::PromptOrCode;
    fix.ide_filenames = vec!["foo.ts".into(), "bad name.rs".into(), ".eslintrc".into()];
    fix.ide_symbols = vec!["handleUserAuthCallback".into()];
    let ctx = extract_from_fixture(&fix);
    assert!(ctx.tokens.contains(&"foo.ts".into()));
    assert!(ctx.tokens.contains(&"handleUserAuthCallback".into()));
    assert!(!ctx.tokens.iter().any(|t| t.contains(' ')));
    assert!(!ctx.tokens.iter().any(|t| t == ".eslintrc"));
}

#[test]
fn terminal_secure_banking_are_empty() { /* three fixtures → tokens+snippets empty */ }

#[test]
fn caps_forty_tokens_and_two_thousand_chars() {
    let mut fix = empty_fix();
    fix.family = ContextFamily::Document;
    fix.ide_symbols = (0..80).map(|i| format!("Token{i}")).collect();
    let ctx = extract_from_fixture(&fix);
    assert!(ctx.tokens.len() <= 40);
    assert!(ctx.usable_chars() <= 2000);
    assert!(ctx.truncated);
}
```

- [x] **Step 2:** `cargo test --manifest-path src-tauri/Cargo.toml --lib extract_from_fixture -- --nocapture`  
  Expected: compile fail.

- [x] **Step 3: Implement** extractors + caps. Do not call live AX yet.

- [x] **Step 4: Tests PASS.**

- [ ] **Step 5: Commit** (skip unless asked) `feat: screen text context extractors with fixtures`

---

### Task 2: Fail-open live extract + wire into ASR/cleanup

**Files:** `src-tauri/src/screen_text.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/lexicon.rs`, `src-tauri/src/llm.rs`

**Interfaces:**

```rust
pub fn extract_live(family: ContextFamily, guard: &TargetAppGuard) -> ScreenTextContext {
    // spawn/blocking AX read with EXTRACT_TIMEOUT
    // on timeout/error → ScreenTextContext { family, ..Default::default() }
}

pub fn build_asr_prompt_shaped(..., screen: Option<&ScreenTextContext>, ...)
// tokens first, then existing dictionary terms, stay in MAX_ASR_PROMPT_TOKENS
```

Call `extract_live` after target lock, in parallel with mic start. Refresh once at stop if the lock is still valid; if stale, drop context (do not recapture the new app). Selected-text path: do not Cmd+C again.

HUD: optional count event `screen_token_count` — never raw snippets.

Privacy tests: assembled ASR + cleanup strings must not contain the fixture `window_title`, `raw_url`, or `pid`.

- [x] **Step 1: Failing tests**

```rust
#[test]
fn extract_live_timeout_returns_empty_not_error() {
    let ctx = extract_with_reader(|| {
        std::thread::sleep(Duration::from_millis(400));
        panic!("should have been timed out");
    });
    assert!(ctx.tokens.is_empty());
}

#[test]
fn asr_prompt_puts_screen_tokens_first_without_secrets() {
    let ctx = /* 晓雯 token + secret title in fixture */;
    let prompt = build_asr_prompt_shaped(..., Some(&ctx), ...).unwrap();
    assert!(prompt.find("晓雯") < prompt.find("TypeScript").unwrap_or(usize::MAX));
    assert!(!prompt.contains("https://"));
}
```

Also: dictation helper / existing session test — extractor `Err` does not fail the session.

- [x] **Step 2–4:** Implement timeout wrapper + wire lock/stop. PASS.

- [ ] **Step 5: Commit** (skip unless asked) `feat: fail-open screen text on dictate lock`

---

### Task 3: Lexicon seed from screen names (not style)

**Files:** `src-tauri/src/lexicon.rs`, `src-tauri/src/dictionary_learn.rs`

**Behavior:** After a successful session, tokens from `ScreenTextContext` may increment learn hits. Person-name classifier → `promote_hits_for` 2×; else 3×. Do **not** call `push_style_pair` with snippets or bubbles.

- [x] **Step 1: Test** `screen_token_does_not_become_style_pair` and `person_name_promotes_at_two`.
- [x] **Step 2–4:** Implement. PASS.
- [ ] **Step 5: Commit** (skip unless asked) `feat: seed lexicon from screen tokens only`

---

### Task 4: Phase 2 OCR opt-in

**Files:** `src-tauri/src/store.rs` (`window_ocr_enabled: bool` default false), `src-tauri/src/permissions.rs`, `src-tauri/Info.plist`, Create `src-tauri/src/window_capture.rs`, settings UI toggle

**Interfaces:**

```rust
pub struct MemoryImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub fn capture_locked_window(window_id: Option<u64>) -> Result<MemoryImage, CaptureError> {
    // Err(NoWindow) if None
    // never full-display
    // long edge ≤ 1280
}

pub fn ocr_memory_image(image: &MemoryImage) -> Vec<String> { /* Vision; tests inject a stub */ }

pub fn maybe_ocr(
    enabled: bool,
    recording_ok: bool,
    family: ContextFamily,
    ctx: &ScreenTextContext,
    window_id: Option<u64>,
) -> Option<ScreenTextContext> // None = skip
```

Skip OCR when: setting off, no permission, family Terminal/Form/Secure/banking, `!ctx.is_thin()`, window_id none.

Image: no filesystem write. Unit test a fake capturer that records `wrote_path: false`.

Info.plist string: VoiceFlow may capture the **front dictation window** to read on-screen words locally when the user enables window text recognition.

- [x] **Step 1: Failing tests**

```rust
#[test]
fn ocr_skipped_when_phase1_has_fifty_chars() { /* ctx usable_chars=50 → None */ }

#[test]
fn capture_refuses_without_window_id() {
    assert!(matches!(capture_locked_window(None), Err(CaptureError::NoWindow)));
}

#[test]
fn ocr_does_not_write_disk() { /* stub */ }
```

- [x] **Step 2–4:** Settings + preflight + merge OCR tokens into the same caps (`source = AxOcr`). Live ScreenCaptureKit can be `#[cfg(target_os = "macos")]` with a stub on other cfgs / tests.

- [ ] **Step 5: Commit** (skip unless asked) `feat: opt-in window ocr when ax is thin`

---

### Task 5: Phase 3 look-at-screen hotkey + preview + docs

**Files:** `hotkey.rs`, `selected_action.rs` or new `screen_action.rs`, `SelectedPreviewDialog.tsx`, store vision fields, docs listed in file map

**Behavior:**

1. `screen_action_hotkey` empty → not registered.
2. On fire: lock target; require Screen Recording + Accessibility or deep-link settings; **no capture** if vision_model empty (“configure a vision model”).
3. One-shot locked window PNG in memory; ASR user utterance; vision HTTP to configured provider; preview with thumbnail.
4. Replace only if guard still matches; else copy-only. Drop PNG when dialog closes.
5. Do not auto-write History (same as selected-text).
6. `dictation::stop` must not call `capture_locked_window` — add `CAPTURE_COUNT` atomic in tests.

**Docs (same change set):**

- `asr-cleanup-later-and-wont.md`: delete “截图进 LLM = 明确不做”. Write: default dictate never; look-at-screen hotkey + preview only; meetings/Spark/image-gen still never.
- `privacy.md`, `end-to-end-workflows.md` §5, `context-e2e-checklist.md` §18: match shipped phases.

- [x] **Step 1: Failing tests**

```rust
#[test]
fn dictate_stop_does_not_increment_capture_count() { /* session stop with hook */ }

#[test]
fn vision_unset_refuses_before_capture() { /* capture_count stays 0 */ }
```

Frontend: preview shows Replace / 只复制 / 取消; cancel drops image (prop `thumbnail` becomes null after onClose).

- [x] **Step 2–4:** Implement. PASS cargo + npm. Update the four docs.

- [ ] **Step 5: Commit** (skip unless asked) `feat: look-at-screen hotkey with preview-first vision`

---

## Spec coverage

| Spec rule | Task |
| --- | --- |
| Adaptive Layer 1 family table | 1 |
| 350ms fail-open | 2 |
| Caps 40 / 2000; no title/URL/PID | 1, 2 |
| Tokens → ASR + cleanup + cascade count | 2 (cascade in companion) |
| HUD count only | 2 |
| Names → lexicon, not style | 3 |
| OCR opt-in, thin gate, no disk, locked window | 4 |
| Phase 3 hotkey, preview, not on dictate stop | 5 |
| later-and-wont red-line + privacy docs | 5 |

## Out of this plan

Cleanup slider, cascade HTTP, style learning (companion). Reading IDE files from disk. All-displays capture. iOS/Windows.
