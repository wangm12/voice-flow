# Dictation Cleanup Enhancement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** User-visible cleanup intensity (default Heavy), per-app override, own-edit style learning, and gated Accurate ASR cascade — with drafts only in the HUD and one paste.

**Architecture:** Add `CleanupIntensity` (`off|light|standard|heavy`) on settings and optional mapping override. `lexicon::decide_cleanup` resolves skip-LLM / effort. New `CleanupEffort::Heavy` plus `visible_context` in `llm.rs`. Cascade lives in a small `cascade.rs` that decides whether to fire a second ASR client; `lib.rs` still pastes once. Style/intensity learning extends `dictionary_learn.rs` and writes mapping fields, not a new event tap.

**Tech Stack:** Tauri 2 + Rust lib `voiceflow`, React 19 + Vitest. Verify: `cargo test --manifest-path src-tauri/Cargo.toml --lib`, `npm test -- --run`, `npm run lint`.

**Spec:** [2026-09-03-dictation-cleanup-enhancement-design.md](../specs/2026-09-03-dictation-cleanup-enhancement-design.md)  
**Companion plan:** [2026-09-03-screen-awareness.md](2026-09-03-screen-awareness.md) (Phase 1 tokens feed cascade + cleanup)

## Global Constraints

- Do not commit unless the user asks this tick to commit.
- macOS dictation only. No meetings, Ask Anything, Spark, image gen, emoji IME, default swear filter.
- Fail-closed paste, target lock, 3s undo, preview-first, protected facts: do not weaken.
- Never send window title, PID, or raw URL to ASR/LLM. No telemetry. No global keylog.
- One paste per session. Prefetch and cascade drafts stay in HUD/background.
- Do not silently change the primary ASR provider. Do not dual-run Accurate ASR on every utterance.
- Do not stream partials into the target field. Do not change default activation from tap to hold.
- `CleanupEffort::Command` is selected-text / spoken-command only. Slider 重 = `CleanupEffort::Heavy`.
- Skip LLM if `cleanup_enabled` is false OR resolved intensity is `off`.
- Terminal / form families still `LocalOnly` even when global intensity is Heavy.
- English UI strings need `t()` keys in `src/lib/i18n.tsx` covered by `src/lib/i18n.coverage.test.ts`.
- Cleanup prompt changes need fixtures in `src-tauri/src/cleanup_corpus.rs`.
- Loop implementers: TDD; skip git commit steps unless asked.
- Do not implement screen capture / OCR / vision hotkey in this plan.

## File map

- Modify: `src-tauri/src/llm.rs` — `CleanupIntensity`, `CleanupEffort::Heavy`, `as_label`, `visible_context`, Heavy few-shots
- Modify: `src-tauri/src/lexicon.rs` — `decide_cleanup` takes intensity; ASR prompt accepts screen tokens
- Modify: `src-tauri/src/context.rs` — `AppMapping.cleanup_intensity`, `style_example_pairs` (max 3)
- Modify: `src-tauri/src/store.rs` — schema 19, new settings fields, `SettingsView`
- Modify: `src-tauri/src/dictionary_learn.rs` — style pairs + intensity ±1 from own edits
- Create: `src-tauri/src/cascade.rs` — gate + winner selection (no HTTP)
- Modify: `src-tauri/src/asr.rs` / `src-tauri/src/engine.rs` — Accurate ASR request helper
- Modify: `src-tauri/src/lib.rs` — wire cascade, single cleanup, HUD events
- Modify: `src-tauri/src/cleanup_corpus.rs` — Heavy PersonalChat fixtures
- Modify: `src/types/settings.ts`, `src/components/ContextSettings.tsx`, `src/components/ContextSettings.test.tsx`
- Modify: `src/lib/hudContextLabel.ts`, `src/lib/hudContextLabel.test.ts`
- Modify: `src/lib/i18n.tsx`
- Modify: `src/components/Island/IslandWindow.tsx` — 「精确重打中」 (Task 8)

Normative settings names (do not rename):

- `cleanup_intensity`: `"off" | "light" | "standard" | "heavy"`, default `"heavy"`
- `accurate_asr_provider`, `accurate_asr_model`, `accurate_asr_base_url`
- `cascade_timeout_ms`: default `5000`
- `cascade_proper_noun_threshold`: default `3`

---

### Task 1: CleanupIntensity + decide_cleanup

**Files:**
- Modify: `src-tauri/src/llm.rs` (`CleanupEffort` enum ~61–90)
- Modify: `src-tauri/src/lexicon.rs` (`decide_cleanup` ~139–168)
- Test: add cases in `lexicon.rs` existing `#[cfg(test)]` module

**Interfaces:**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CleanupIntensity {
    Off,
    Light,
    Standard,
    #[default]
    Heavy,
}

impl CleanupIntensity {
    pub fn as_str(self) -> &'static str { /* off light standard heavy */ }
    pub fn parse(value: &str) -> Option<Self> { /* ... */ }
    pub fn as_effort(self) -> Option<CleanupEffort> {
        match self {
            Self::Off => None,
            Self::Light => Some(CleanupEffort::Light),
            Self::Standard => Some(CleanupEffort::Standard),
            Self::Heavy => Some(CleanupEffort::Heavy),
        }
    }
    pub fn demote(self) -> Self { /* heavy→standard→light→off */ }
    pub fn promote(self) -> Self { /* off→light→standard→heavy */ }
}

impl CleanupEffort {
    // add Heavy; as_label => "heavy"
}

pub fn decide_cleanup(
    settings_cleanup_enabled: bool,
    global_intensity: CleanupIntensity,
    mapping: Option<&AppMapping>,
    family: ContextFamily,
    intent: &CleanupIntent,
) -> CleanupRoute
```

Remove the `profile_confidence < 0.75 → Light` branch. Unmapped WeChat (PersonalChat) with default Heavy must return `Provider(Heavy)`.

Resolution order:

1. `!settings_cleanup_enabled` → `LocalOnly`
2. mapping `cleanup_enabled == false` → `LocalOnly`
3. `intent.source == SpokenCommand` or `SelectedText` → `Provider(Command)`
4. empty content → `LocalOnly`
5. `skips_llm_scene(family)` → `LocalOnly`
6. `resolved = mapping.cleanup_intensity.unwrap_or(global_intensity)` (once field exists; until Task 3, pass intensity only via the new arg)
7. `resolved == Off` → `LocalOnly`
8. else `Provider(resolved.as_effort().unwrap())`

Legacy `mapping.cleanup_effort` (Task 3): if `cleanup_intensity` is `None` and `cleanup_effort` is `Some(Light|Standard)`, treat that as the mapping override. Ignore legacy `Command` on the dictate path.

- [x] **Step 1: Write failing tests** in `lexicon.rs`:

```rust
#[test]
fn unmapped_wechat_uses_global_heavy() {
    let route = decide_cleanup(
        true,
        CleanupIntensity::Heavy,
        None,
        ContextFamily::PersonalChat,
        &CleanupIntent::implicit("好的哈哈我晚点回你"),
    );
    assert_eq!(route, CleanupRoute::Provider(CleanupEffort::Heavy));
}

#[test]
fn mapping_light_overrides_global_heavy() {
    let mut mapping = sample_mapping();
    mapping.cleanup_intensity = Some(CleanupIntensity::Light);
    let route = decide_cleanup(
        true,
        CleanupIntensity::Heavy,
        Some(&mapping),
        ContextFamily::PersonalChat,
        &CleanupIntent::implicit("好的"),
    );
    assert_eq!(route, CleanupRoute::Provider(CleanupEffort::Light));
}

#[test]
fn intensity_off_or_cleanup_disabled_is_local_only() {
    let intent = CleanupIntent::implicit("hello");
    assert_eq!(
        decide_cleanup(true, CleanupIntensity::Off, None, ContextFamily::Email, &intent),
        CleanupRoute::LocalOnly
    );
    assert_eq!(
        decide_cleanup(false, CleanupIntensity::Heavy, None, ContextFamily::Email, &intent),
        CleanupRoute::LocalOnly
    );
}

#[test]
fn terminal_stays_local_even_when_heavy() {
    assert_eq!(
        decide_cleanup(
            true,
            CleanupIntensity::Heavy,
            None,
            ContextFamily::Terminal,
            &CleanupIntent::implicit("ls -la"),
        ),
        CleanupRoute::LocalOnly
    );
}
```

- [x] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml --lib decide_cleanup -- --nocapture`  
  Expected: compile fail (`CleanupIntensity` / `Heavy` missing) or FAIL.

- [x] **Step 3: Implement** enum + `decide_cleanup` signature change. Update every call site in `lib.rs` / tests to pass `CleanupIntensity::Heavy` or `settings` value. Keep compiling.

- [x] **Step 4: Re-run** the four tests. Expected: PASS.

- [ ] **Step 5: Commit** (skip unless asked) `feat: resolve cleanup intensity including default heavy`

---

### Task 2: Heavy prompts + visible_context + corpus

**Files:** `src-tauri/src/llm.rs` (`family_few_shot`, user prompt builder ~715–800), `src-tauri/src/cleanup_corpus.rs`

**Interfaces:**

- `CleanupEffort::Heavy` already from Task 1.
- `build_cleanup_user_prompt` (or the existing private builder) gains `visible_context: Option<&str>`.
- When `Some` and non-empty, append:

```text
Visible context (spell names and address terms only; do not quote, summarize, or answer the screen):
{visible_context}
```

- `family_few_shot(PersonalChat | WorkChat | SocialMedia)` for Heavy must include the substring `好的哈哈我晚点回你` and must not contain `您好`.
- Heavy system/user guidance: Typeless-like polish; forbidden additions unchanged (no new facts, no unsolicited 您好, no swear sanitization).

- [x] **Step 1: Failing tests** in `llm.rs` / `cleanup_corpus.rs`:

```rust
#[test]
fn personal_chat_heavy_few_shot_stays_chat_shaped() {
    let shot = family_few_shot(ContextFamily::PersonalChat).expect("shot");
    assert!(shot.contains("好的哈哈我晚点回你"));
    assert!(!shot.contains("您好"));
}

#[test]
fn visible_context_is_spell_only() {
    let user = assemble_user_prompt_for_test(
        "hi alex",
        CleanupEffort::Heavy,
        Some("Alex Chen"),
    );
    assert!(user.contains("spell names"));
    assert!(user.contains("do not quote, summarize, or answer the screen"));
    assert!(user.contains("Alex Chen"));
    assert!(!user.contains("window_title"));
    assert!(!user.contains("pid"));
    assert!(!user.to_ascii_lowercase().contains("http://"));
}
```

Add a Heavy PersonalChat corpus row: spoken casual chat must not become `您好…`.

- [x] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml --lib personal_chat_heavy -- --nocapture`  
  Expected: FAIL.

- [x] **Step 3: Implement** Heavy branch in the existing `effort == Light` split (~624). Add `Heavy` arm with the sendable-polish instructions. Thread `visible_context` through `cleanup` / `selected_text_action` without sending it for selected-text unless already in the selection.

- [x] **Step 4: Tests PASS.** Run `cleanup_corpus` tests.

- [ ] **Step 5: Commit** (skip unless asked) `feat: heavy cleanup prompts and visible_context`

---

### Task 3: Settings schema 19 + AppMapping fields

**Files:** `src-tauri/src/store.rs`, `src-tauri/src/context.rs`, `src/types/settings.ts`

**Interfaces:**

```rust
// Settings (plus SettingsView + Default + From)
pub cleanup_intensity: String, // "heavy"
pub accurate_asr_provider: crate::engine::EngineProvider, // default Groq, unused until configured model/url/key
pub accurate_asr_model: String, // empty = cascade disabled
pub accurate_asr_base_url: String,
pub cascade_timeout_ms: u64, // 5000
pub cascade_proper_noun_threshold: usize, // 3

pub const SETTINGS_SCHEMA_VERSION: u32 = 19;

fn default_cleanup_intensity() -> String { "heavy".into() }
fn default_cascade_timeout_ms() -> u64 { 5000 }
fn default_cascade_proper_noun_threshold() -> usize { 3 }

// AppMapping
pub cleanup_intensity: Option<crate::llm::CleanupIntensity>,
pub style_example_pairs: Vec<StyleExamplePair>, // max 3; validate each side ≤ 2000 chars, pair short-rewrite rules enforced at learn time

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleExamplePair {
    pub input: String,
    pub output: String,
}
```

Normalize: bump schema to 19; if `cleanup_intensity` empty, set `"heavy"`. If `style_example_pairs` empty and old `style_example_input/output` both set, push that one pair (do not delete the old fields this task — keep them in sync: pairs[0] mirrors the legacy fields).

Validate `cleanup_intensity` ∈ {off,light,standard,heavy}. Empty Accurate model means cascade off (do not require provider).

- [x] **Step 1: Failing test** in `store.rs`:

```rust
#[test]
fn schema_19_defaults_cleanup_intensity_heavy() {
    let settings = Settings::default();
    assert_eq!(settings.cleanup_intensity, "heavy");
    assert_eq!(settings.cascade_timeout_ms, 5000);
    assert_eq!(settings.cascade_proper_noun_threshold, 3);
    assert!(settings.accurate_asr_model.is_empty());
}
```

- [x] **Step 2: Run** that test. Expected: compile fail.

- [x] **Step 3: Add fields** with `#[serde(default = "...")]`. Update `SettingsView`, `From`, `Default`, `validate`, settings patch allowlist in `lib.rs` (`cleanup_intensity`, accurate ASR fields, cascade numbers). Frontend `Settings` type.

- [x] **Step 4: Test PASS.** Existing store normalize tests still pass (`SETTINGS_SCHEMA_VERSION` assertions).

- [ ] **Step 5: Commit** (skip unless asked) `feat: settings schema 19 cleanup intensity and cascade fields`

---

### Task 4: Settings UI + HUD resolved intensity

**Files:** `src/components/ContextSettings.tsx`, `src/components/ContextSettings.test.tsx`, `src/lib/hudContextLabel.ts`, `src/lib/hudContextLabel.test.ts`, `src/lib/i18n.tsx`, Island HUD payload if it already sends style

**Interfaces:**

```ts
export function formatHudIntensityLabel(
  app: string | null | undefined,
  intensity: "off" | "light" | "standard" | "heavy" | null | undefined,
  translate: (source: string) => string = (s) => s,
): string | null
// WeChat + heavy → "WeChat · 重"
// missing app → "未知应用 · 重"
```

Keep `formatHudContextLabel` for family (口语). Island should show **one** resolved intensity pair per spec (`微信 · 重`), not global+override.

ContextSettings: on 智能整理, a four-option control 关/轻/中/重 bound to `cleanup_intensity`. Each mapping row: optional override select including “跟随全局”.

i18n keys: `关`, `轻`, `中`, `重`, `跟随全局`, `整理强度`.

- [x] **Step 1: Failing tests**

```ts
it("shows app and resolved intensity only", () => {
  expect(formatHudIntensityLabel("WeChat", "heavy")).toBe("WeChat · 重");
  expect(formatHudIntensityLabel("WeChat", "light")).toBe("WeChat · 轻");
});
```

ContextSettings test: slider default heavy; changing calls `saveSettings({ cleanup_intensity: "light" })`.

- [x] **Step 2: Run** `npx vitest run src/lib/hudContextLabel.test.ts` Expected: FAIL.

- [x] **Step 3: Implement** helper + UI + i18n. Wire Island to intensity if the HUD event already has a field; otherwise add `cleanup_intensity` to the existing HUD state event in `lib.rs` (smallest JSON field).

- [x] **Step 4:** `npm test -- --run` and `npm run lint` PASS.

- [ ] **Step 5: Commit** (skip unless asked) `feat: cleanup intensity slider and HUD label`

---

### Task 5: Learn style pairs and intensity from own edits

**Files:** `src-tauri/src/dictionary_learn.rs`, `src-tauri/src/context.rs` (pair helpers), `src-tauri/src/store.rs` if mappings persist through existing save

**Interfaces:**

```rust
pub fn is_short_style_rewrite(baseline: &str, settled: &str) -> bool {
    // ignore if settled == baseline
    // ignore if settled.chars() > 40
    // accept Latin 2–4 word replacement OR CJK 2–8 char replacement
    // OR whole-sentence rewrite with settled.chars() <= 40
}

pub fn push_style_pair(pairs: &mut Vec<StyleExamplePair>, input: String, output: String) {
    // drop exact-duplicate output; push; if len > 3 remove index 0
}

pub fn intensity_shift_from_edit(baseline: &str, settled: &str, current: CleanupIntensity) -> Option<i8> {
    // 3× handled by caller counts; this returns -1 if settled is shorter/more casual toward raw,
    // +1 if settled expands fragments into full sentences; None for topic change / added facts
}
```

Learning **only** from `observe_after_paste` baseline vs settled on the locked field. Do **not** take Layer 1 bubble text as input/output.

Screen names → existing lexicon promotion (2× person name via existing classifier, else 3×). That hook is in the screen plan; this task only ensures style path ignores tokens that were not user edits.

Persist: after 3 identical-direction short rewrites for the same `mapping.id`, write `style_example_pairs`. After 3 intensity demote signals, set `mapping.cleanup_intensity = current.demote()` (if None, start from global Heavy). HUD undo/tombstone: reuse existing learn-undo event; store previous pairs/intensity on the toast payload.

- [x] **Step 1: Failing tests** in `dictionary_learn.rs`:

```rust
#[test]
fn short_rewrite_is_style_candidate() {
    assert!(is_short_style_rewrite("好的我会稍后回复您", "好的哈哈我晚点回你"));
    assert!(!is_short_style_rewrite("好的", &"x".repeat(80)));
}

#[test]
fn style_pairs_cap_at_three() {
    let mut pairs = vec![];
    for i in 0..4 {
        push_style_pair(&mut pairs, format!("in{i}"), format!("out{i}"));
    }
    assert_eq!(pairs.len(), 3);
    assert_eq!(pairs[0].input, "in1");
}

#[test]
fn foreign_bubble_is_not_a_style_pair() {
    // fixture: baseline is what we pasted; settled is unrelated other-person text → None
    assert!(!is_short_style_rewrite(
        "好的我晚点回你",
        "对方: 那你把合同发我一下谢谢"
    ));
}
```

- [x] **Step 2: Run tests.** Expected: FAIL.

- [x] **Step 3: Implement** helpers + call from the existing promote path after observe. 3× counter: store on `LearnPairRecord`-like rows with class `style` / `intensity` **or** count in-memory per mapping id in the existing learn DB table if a text class already exists. Prefer the smallest extension of `learn_pairs` (e.g. `kind = "style"`). Do not add a global key tap.

- [x] **Step 4: Tests PASS.**

- [ ] **Step 5: Commit** (skip unless asked) `feat: auto-learn short style pairs and intensity`

---

### Task 6: Cascade gate (no HTTP)

**Files:** Create `src-tauri/src/cascade.rs`; `mod cascade;` in `lib.rs`

**Interfaces:**

```rust
pub struct CascadeInput<'a> {
    pub accurate_asr_configured: bool, // !accurate_asr_model.trim().is_empty()
    pub primary_failed: bool,
    pub low_confidence: bool,          // existing asr no_speech / avg_logprob signals
    pub hallucination_hit: bool,       // spoken_revision bag
    pub mixed_cjk_english: bool,
    pub proper_noun_count: usize,
    pub noun_threshold: usize,         // settings.cascade_proper_noun_threshold
}

pub fn should_run_accurate(input: &CascadeInput) -> bool {
    if !input.accurate_asr_configured {
        return false;
    }
    if input.primary_failed {
        return true;
    }
    input.low_confidence
        || input.hallucination_hit
        || input.mixed_cjk_english
        || input.proper_noun_count >= input.noun_threshold
}

pub fn is_mixed_cjk_english(text: &str) -> bool {
    let has_cjk = text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
    let has_latin = text.chars().any(|c| c.is_ascii_alphabetic());
    has_cjk && has_latin
}
```

- [x] **Step 1: Failing tests** in `cascade.rs`:

```rust
#[test]
fn no_accurate_configured_never_runs() {
    assert!(!should_run_accurate(&CascadeInput {
        accurate_asr_configured: false,
        primary_failed: true,
        low_confidence: true,
        hallucination_hit: true,
        mixed_cjk_english: true,
        proper_noun_count: 9,
        noun_threshold: 3,
    }));
}

#[test]
fn each_trigger_fires_when_configured() { /* four cases + primary_failed */ }

#[test]
fn mixed_cjk_english_detects_晓雯_and_python() {
    assert!(is_mixed_cjk_english("晓雯在写 Python"));
    assert!(!is_mixed_cjk_english("只有中文"));
}
```

- [x] **Step 2–4:** TDD implement. PASS.

- [ ] **Step 5: Commit** (skip unless asked) `feat: gated accurate-asr cascade decision`

---

### Task 7: Accurate ASR shot + 5s timeout + cleanup once

**Files:** `src-tauri/src/asr.rs` or `engine.rs`, `src-tauri/src/lib.rs` dictation stop path

**Interfaces:**

- Reuse the existing ASR HTTP client with `accurate_asr_provider` / model / base_url / Keychain key for that provider (same `provider_api_keys` map). Do not add a new secret kind.
- After primary transcript (merged prefetch), if `should_run_accurate`, start Accurate with `tokio::time::timeout(Duration::from_millis(settings.cascade_timeout_ms), ...)`.
- Winner: Accurate non-empty before timeout → Accurate; else primary; primary fail + Accurate ok → Accurate; both fail → existing recovery, no empty paste.
- Call cleanup **once** on the winner.

- [x] **Step 1:** Unit-test a pure helper:

```rust
pub enum CascadeWinner { Primary, Accurate, None }
pub fn pick_winner(primary: Option<&str>, accurate: Result<Option<String>, Timeout>) -> CascadeWinner
```

Cases: accurate ok → Accurate; timeout → Primary; primary none + accurate ok → Accurate; both none → None.

- [x] **Step 2–4:** Implement helper + wire stop path. Add a test double / counter that cleanup is invoked once (extract the “run cleanup on transcript” call so a unit test can count). Do not hit live Groq.

- [ ] **Step 5: Commit** (skip unless asked) `feat: accurate asr cascade with timeout and single cleanup`

---

### Task 8: HUD draft + 精确重打中

**Files:** `src/components/Island/IslandWindow.tsx`, Island events, `src/lib/i18n.tsx`

**Behavior:** While Accurate runs, HUD may show primary draft text (existing prefetch channel) plus status `精确重打中`. When winner is ready, existing processing → paste-once → done. No second insert.

- [x] **Step 1:** Frontend test: given event `{ phase: "cascade_accurate" }`, render `精确重打中`.
- [x] **Step 2–4:** Emit that phase from Rust only when Accurate is in flight. PASS `npm test`.
- [ ] **Step 5: Commit** (skip unless asked) `feat: hud cascade status`

---

## Spec coverage

| Spec rule | Task |
| --- | --- |
| Default Heavy including WeChat | 1, 3, 4 |
| Mapping override; HUD one label | 1, 4 |
| Slider 关 skips HTTP | 1 |
| Heavy chat-shaped few-shot | 2 |
| visible_context spell-only | 2 (wired from screen plan) |
| Style 3× / max 3 / ignore paragraphs / ignore others | 5 |
| Intensity 3× demote + Undo | 5 |
| Cascade 4 triggers; unset Accurate = no shot | 6 |
| 5s timeout; cleanup once; failover | 7 |
| HUD draft; one paste | 7, 8 |
| No streaming / no hold-default / no engine swap | constraints |

## Out of this plan

ScreenCaptureKit, OCR, vision hotkey, changing tap default, Sogou import, snippet placeholders.
