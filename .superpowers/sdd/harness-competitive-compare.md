# Voice harness competitive compare

Date: 2026-08-23  
Scope: how production and experimental voice-dictation systems learn personal vocabulary / style, and how that should change VoiceFlow’s planned harness.  
Plan compared: `~/.cursor/plans/complete_voice_harness_b6de6a80.plan.md`  
Existing product notes: `docs/competitive-research.md`  
Constraints honored: no screenshots, no RL / weight fine-tune, no global keylog, no uploading corrections to train a model. Groq Whisper `prompt` is a ~224-token class budget.

This report does **not** change the plan or product code.

---

## 1. VoiceFlow today vs the planned harness

### Today (shipping)

| Piece | Behavior |
| --- | --- |
| ASR | Groq Whisper batch. `build_asr_prompt` takes the **first 32** `settings.dictionary` words, plus a technical-preserve clause when the context policy asks for it, then hard-cuts to **2000 characters**. |
| Cleanup | Optional Groq LLM. `bounded_dictionary` also takes the **first 32** dictionary words (~2048 chars). |
| Lexicon | 256-word global list. No `before→after` apply path. No local token replace. |
| Learn | After **verified** paste, same-field AX poll: 3s initial, 400ms interval, 1.5s idle-extend, 12s max. Only unambiguous **single-token** corrections. 3 hits → promote. History / settings can confirm. Secure Input and unverified paste never learn. |
| Style | `AppMapping.style_example` exists; nothing learns punctuation/density yet. |
| Ranking | None. New WeChat names sit at the end of a 256-list and never reach the 32-slot prompt. |
| Privacy | No telemetry. No raw URL / title / PID to the LLM. Observe is local. Audio goes to the configured ASR (default Groq). |

### Planned harness

Two skill types, one global lexicon, two outbound budgets:

1. **Lexicon** — 3× or human confirm. Applied at **ranked Whisper prompt + local token replace**. Cleanup only sees terms that appear in **this** transcript (max 8 / ~400 chars).
2. **Style** — punctuation / density / habit drafts from `ObserveOutcome::Ambiguous`. 3× similar pattern → pending review. Confirm writes **one** `style_example` on that mapping. Pending drafts never enter a prompt.
3. **Scene skip** — PersonalChat / SocialMedia / BrowserSearch / FormFilling / Terminal skip LLM unless rewrite intent (or a per-app “always clean” override). Email / Document / PromptOrCode / CustomerSupport / CalendarTask still clean. WorkChat / Notes clean only if an approved style example exists.
4. **Scope** — store `family` + optional `bundle_id` on pairs for **ranking only**. Do not isolate dictionaries per app. Do not send window title / URL to the model.
5. **Out of scope** — screenshots, RL, fine-tune, global keyboard, cloud training, FunASR hotwords as a completion condition, DPO, undo-decrements.

The plan is already stronger than most shipping products on **privacy, cleanup slimness, and chat-not-email**. The gaps below are things real products do that still fit these constraints — plus one Whisper-budget bug the plan currently inherits.

---

## 2. How each system works

For each: what they learn, how they promote, where skill is applied, per-app vs global, privacy, speed tricks.

### 2.1 Open Typeless / OpenCodexLabs `open-typeless-harness`

**Sources:** [GitHub README](https://github.com/OpenCodexLabs/open-typeless-harness) (MIT, experimental; DeepWiki not indexed), `USAGE.md`, `OPENTYPELESS_FUSION.md`, `FUSION_COMPLETION_AUDIT.md`, `CLAUDE.md`. Inspected 2026-08-23.

Open Typeless Harness is a **correction-native experiment**, not a finished product. Runtime is OpenLess (Tauri 2: hotkey → ASR → LLM polish → insert). Learning is fused from OpenTypeless “speech skills” plus an OpenWhispr-style focused-field monitor.

| | |
| --- | --- |
| **Learns** | Post-insertion edit trail. High-confidence word/phrase swaps (`知呼→知乎`, `type script→TypeScript`). Low-confidence / ambiguous edits stay reviewable. Explicitly **not** global autocorrect. |
| **Promote** | README: repeated stable corrections auto-promote; ambiguous stay visible. **Code today is thinner:** `learning_probe.rs` writes `correction_candidate` JSONL after the monitor closes and **does not** mutate dictionaries or inject future prompts. Promotion is still “future.” Smoke window is **30 seconds**. |
| **Applied where** | Intended: retrieve relevant skills and inject them into the **LLM polish** prompt. Local replace is not the primary path. Optional VIH rewrite is **off by default**. |
| **Scope** | Local speech-skill memory (`~/.openless/opentypeless-speech-skills.json`). Contextual, not per-app isolated. |
| **Privacy** | Skills stay on machine “by default.” Monitor writes field values to local JSONL (`opentypeless-edit-monitor.jsonl`) — useful for an experiment, too chatty for a shipping app. PID-scoped AX; `AXObserver` + 500ms poll fallback; `AXStringForRange` when `AXValue` is empty. |
| **Speed** | Still polishes with an LLM on the default path. Silent ASR/polish fallbacks so words are not lost. |

**Takeaway for VoiceFlow:** the *product loop* is the same one VoiceFlow already started (same-field observe → two-track skills). VoiceFlow is **ahead of this repo on promotion** (3× lexicon already ships). Do not copy “skills only enter the LLM” — that is the slow path VoiceFlow is trying to leave.

---

### 2.2 Wispr Flow (dictionary / learning / snippets)

**Sources:** [Teach Flow your words](https://docs.wisprflow.ai/articles/4052411709-teach-flow-your-words-with-the-dictionary) (updated 2026-08-21), [Features](https://wisprflow.ai/features), [What’s new](https://wisprflow.ai/whats-new), [Flow Styles](https://docs.wisprflow.ai/articles/2368263928-how-to-setup-flow-styles), [Personalized Style](https://wisprflow.ai/post/personalized-style), desktop/iOS/Android nav docs.

Wispr is the category definition of “correct it and it sticks,” plus snippets and per-category style.

| | |
| --- | --- |
| **Learns** | Desktop auto-learn from correcting dictated text: **phrases up to 4 words, up to 4 per edit**. Classifier keeps proper nouns, product/project names, acronyms, technical terms. Drops ordinary wording, grammar/style, capitalization-only, fillers, generic phrases, pure insertions, pure deletions, blanks, and anything previously deleted. |
| **Promote** | **First qualifying edit.** Notification + undo the whole batch. Toggle off in desktop preferences. Sparkle icon on iOS for auto-learned rows. Manual add, CSV import (≤1000 rows), “Correct a misspelling” attach a wrong form. |
| **Applied where** | Two mechanisms, documented separately: **word/phrase boosting** during recognition, and **misspelling replacement** after dictate. Starred words get higher recognition priority when the list is large. Usage-based ranking in the UI. Android sends dictionary + snippets **once per session**. |
| **Scope** | Global personal dictionary, synced across Mac / Win / iOS / Android. Team/Business shared dictionary; **personal wins** on the same word. |
| **Privacy** | Cloud SaaS. Dictionary, snippets, and custom prompts sync even if Cloud Sync is off. No public claim of on-device-only corrections. |
| **Speed / other** | Snippets: speak a cue, paste up to 4000 chars (rich text on desktop). Styles: Formal / Casual / Very Casual / Excited on Personal / Work / Email / Other. **Auto Cleanup is global** (None / Light / Medium / High) — not per-app. |

**Takeaway:** Wispr’s classifier + undo is why 1× auto-learn is tolerable. VoiceFlow should copy **classifier discipline, phrase length, star/usage ranking, and undo** — not cloud sync, team dictionaries, or a single global cleanup intensity.

---

### 2.3 Willow Voice

**Sources:** [Dictation feature page](https://willowvoice.com/features/dictation), [Personal Dictionary help](https://help.willowvoice.com/en/articles/13183918-using-personal-dictionary-and-shortcuts) (2026-03-09), marketing blogs, founder LinkedIn (auto-dictionary launch).

| | |
| --- | --- |
| **Learns** | Marketing: names, product terms, abbreviations, “how you write,” tone per app. Help center documents **manual** Personal Terms + Shortcuts (say `<phrase> shortcut`). |
| **Promote** | **Inconsistent in public copy:** one blog says “correct twice and Willow locks it in”; iMessage / founder copy says “correct once… remembers forever.” Help docs do not publish a threshold. Treat “1×–2× lock-in” as marketing, not a spec. |
| **Applied where** | Opaque cloud pipeline. Terms “recognized more accurately during dictation.” Shortcuts are a **voice-triggered snippet** (explicit `shortcut` keyword), not silent replace. Tone-matching is automatic from destination app. |
| **Scope** | Personal terms + workspace Collaboration terms. Sync across Mac / Win / iOS. |
| **Privacy** | Cloud. SOC 2 / HIPAA claimed on marketing pages. Founder language (“every correction trains Willow”) sounds like server-side adaptation; there is no public “we never upload corrections” statement. |
| **Speed** | Hold-to-talk, ~200ms body-feel claim. Scribe is a separate intent-to-draft hotkey. |

**Takeaway:** copy the **manual term + shortcut split** and the idea of destination tone — not the “your corrections train our model” posture, team dictionaries, or Scribe-as-default-in-chat.

---

### 2.4 Superwhisper / MacWhisper vocabulary

#### Superwhisper

**Sources:** [Vocabulary docs](https://superwhisper.com/docs/get-started/interface-vocabulary), [Modes](https://superwhisper.com/docs/modes/modes), [Custom mode](https://superwhisper.com/docs/modes/custom), [user request: Learn Entities Automatically](https://superwhisper.userjot.com/board/p/learn-entities-automatically) (still pending as of research date).

| | |
| --- | --- |
| **Learns** | **Nothing automatically.** Users still request Wispr-style auto-add. |
| **Promote** | Manual only. |
| **Applied where** | **Vocabulary** = pre-transcription hints to the ASR model. **Replacements** = programmatic, case-insensitive find-replace after ASR; output uses the stored case. Docs say: use vocabulary **sparingly** (too many words confuse the model, can perturb punctuation / language detect); use replacements as the **primary** fix because they are consistent regardless of the model. Also used as spoken-symbol / email expansions. |
| **Scope** | Global lists. Modes (Voice to Text / Message / Email / Note / Super / Meeting / Custom) change **AI processing**, not the dictionary. Custom mode supports few-shot input/output examples. |
| **Privacy** | Local models available; vocabulary is sent to whichever transcription model is selected. |
| **Speed** | Replacements are free. Voice-to-Text mode skips AI entirely. |

This is the clearest public statement of the architecture VoiceFlow’s plan is converging on: **sparse ASR hints + deterministic replace, LLM optional.**

#### MacWhisper

**Sources:** [Find and Replace](https://docs.macwhisper.com/article/37-find-and-replace-in-transcriptions); Handy discussion comparing the two.

| | |
| --- | --- |
| **Learns** | Nothing automatically. |
| **Promote** | Manual Global Replace pairs. |
| **Applied where** | Post-transcription only. Toggles: case-sensitive, “only replace separate words.” No dedicated vocab-to-Whisper-prompt list on the same page; people use app-specific prompts for bias. |
| **Scope** | Global, all new transcriptions. |
| **Privacy** | Local Whisper. Used as a privacy example in the Handy community. |
| **Speed** | Replace is immediate; no LLM required. |

**Takeaway:** ship **whole-word + stored-case** replace (MacWhisper / Superwhisper). Do not treat a 256-word dump as a Whisper vocabulary.

---

### 2.5 TalaX 3× replace rule

**Sources:** [puretensor/talax-dictation README](https://github.com/puretensor/talax-dictation) (BSL 1.1 public snapshot, 2026).

| | |
| --- | --- |
| **Learns** | Word-level diffs from **reviewed** transcriptions (History / editor), not post-paste AX. Patterns stored in SQLite. |
| **Promote** | **Same `(wrong→right)` 3+ times with high confidence → auto-apply.** Fresh profile has no L2. |
| **Applied where** | After local Whisper, **before paste**, 3 layers: L1 regex dictionary **&lt;1ms**, longest-match-first, case preservation; L2 interpolated trigram on reviewed corpus **&lt;50ms** (inert until enough reviews); L3 Levenshtein ≤2, Double Metaphone, compounds, acronyms, number normalize **&lt;10ms**. |
| **Scope** | **Voice profiles isolate correction DBs** (work / personal / project). Planned cross-profile sharing is not shipped. |
| **Privacy** | Fully local whisper.cpp. Default delivery is **clipboard / review-first**, not blind inject. Planned “audio excerpt extraction for fine-tuning” is listed as future — do not copy. |
| **Speed** | No LLM. L1 is the harness VoiceFlow wants. L2/L3 are extra local intelligence. |

**Takeaway:** VoiceFlow already copied the **3× rule**. Keep it. Copy L1 longest-match and case preservation. Do not copy isolated profile databases or n-gram / fuzzy layers until lexicon + skip-LLM are done.

---

### 2.6 typwrtr / TypeWhisper post-paste observe

These are the two public implementations closest to VoiceFlow’s observe window.

#### typwrtr (`kaidhar/typwrtr`)

**Sources:** [README](https://github.com/kaidhar/typwrtr), [author write-up](https://www.kartikaydhar.com/blog/typwrtr).

| | |
| --- | --- |
| **Learns** | After paste, poll the **same focused editable control** (UI Automation on Windows; NSAccessibility / AT-SPI documented in the blog). If edited text is highly similar to what was pasted, extract `(wrong→right)` plus **up to 4 words of context**. Fix-up hotkey is the fallback when AX is blind. |
| **Promote** | Bump `count`. Proper-noun-shaped right tokens (mixed case or ALL CAPS, length ≥3, not a stopword) go to **per-app vocabulary**. Replacement table used to require `count ≥ 3`; **lowered to ≥ 1** after Forget/tombstone made false positives one-click recoverable. |
| **Applied where** | **Whisper `initial_prompt`:** top-20 per-app vocab + top-10 global vocab + top-10 per-app correction targets, deduped, **≈800 characters** to stay under Whisper’s ~224-token budget. **Replacement table** pre-paste: case-insensitive, word-boundary, context-gated. Plus Metaphone phonetic replacements. **No LLM** on the hot path (they removed Groq, Ollama, and an on-device T5). |
| **Scope** | Per-app profiles (vocab prompt, markdown/plain/code mode, identifier case, preferred model, **learning on/off**, **auto-apply on/off**). Sensitive apps can disable learning. |
| **Privacy** | Fully local. No field JSONL. Forget tombstones a row so it will not re-learn. |
| **Speed** | `collapse_repeats` + Aho–Corasick Whisper-hallucination scrub in O(n). Persistent Whisper context. Streaming captions every 700ms optional. |

**Takeaway:** typwrtr is the best open spec for **Whisper budget arithmetic, per-app ranking, tombstone, and “skip the LLM.”** VoiceFlow should not copy 1× promote until undo/tombstone exists, and should not isolate vocab the way typwrtr does (names learned in WeChat must still help Slack).

#### TypeWhisper (`TypeWhisper/typewhisper-mac`)

**Sources:** DeepWiki wiki (indexed), [PR #752](https://github.com/TypeWhisper/typewhisper-mac/pull/752), [PR #770](https://github.com/TypeWhisper/typewhisper-mac/pull/770), [PR #878](https://github.com/TypeWhisper/typewhisper-mac/pull/878), [typewhisper.com](https://www.typewhisper.com/en/).

| | |
| --- | --- |
| **Learns** | (1) History edit → `TextDiffService` word swaps → `learnCorrection` **immediately**. (2) Premium: after plain-text insert, same AX element, **high-confidence single-token only**. Broader rewrites and deletions skipped. |
| **Promote** | History: 1×. Target-app: accumulate candidates during the watch window, **commit only on Return / keypad Enter / Tab / focus loss / app change** — not on timeout (PR #770). Undo toast. Latest attempt outcome is visible in Premium settings (PR #878). |
| **Applied where** | **Terms** → engine prompt (`getTermsForPrompt`, **600-character** cap for the 224-token class limit). Parakeet maps prompt terms to CTC boost tokens. **Corrections** → `PostProcessingPipeline` string replace (`usageCount++`). Snippets are a separate step. |
| **Scope** | Global dictionary. `MemoryService` stores app/bundle where the correction happened (context, not isolation). Profiles switch engine / prompt / language per app or URL. |
| **Privacy** | Local SwiftData. Auto-learn is a **commercial** unlock. Folder sync (iCloud / Dropbox / …) is also premium. Diagnostics export a text-free `correctionLearning` block. |
| **Speed** | Corrections are local. LLM is a separate Prompt Action, not every dictation. |

**Takeaway:** split **term vs correction** (already in VoiceFlow’s plan). Copy **commit-gated observe** so a WeChat send still learns. Copy **600–800 character Whisper caps**, not VoiceFlow’s 2000-character illusion.

---

### 2.7 YazSes paper (on-device tuner, no keylog)

**Sources:** [arXiv:2607.28878v1](https://arxiv.org/html/2607.28878v1) (July 2026), [MSKazemi/yazses](https://github.com/MSKazemi/yazses), privacy statement.

YazSes is an Apache-2.0 hold-to-talk daemon (faster-whisper CPU int8). Section 5 is the relevant design.

| | |
| --- | --- |
| **Learns** | Opt-in, **off by default**. Encrypted local SQLite corpus of transcripts (optional audio). Tuner proposes **configuration diffs**: vocabulary additions, VAD threshold, model upgrade, disfluency tweaks. |
| **Promote** | Human must approve diffs. Each proposal is checked on a **chronological held-out** slice; duplicates across the split are rejected; uncorroborated proposals are flagged unverified. **No accuracy eval of the tuner is claimed** (future work). |
| **Applied where** | Whisper `initial_prompt` assembled from **app name + personal vocabulary + optional editor context**. Cleanup is artefact strip + three-pass disfluency. Commands: Tier-1 regex first; small LM router only if unsure. |
| **Scope** | Global config. |
| **Privacy** | **Correction signals are never keystrokes.** They come from an explicit “mark wrong,” re-transcription with a larger model as pseudo-GT, or a passive **re-dictation** heuristic. Corpus AES-256-GCM, machine-bound key. Zero telemetry. CI network-namespace gate. |
| **Speed** | Non-decode pipeline **0.289 ms**. Grammar classify 0.021 ms. Decode dominates. |

**Takeaway:** VoiceFlow already matches the important invariant (no keylog; human confirm for style). Copy the **held-out / don’t overfit** idea for style drafts, and the **regex-before-LLM** instinct. Do not copy an always-on encrypted audio corpus.

---

### 2.8 FunASR / sherpa-onnx hotwords vs Whisper prompt bias

**Sources:** DeepWiki for [modelscope/FunASR](https://deepwiki.com/modelscope/FunASR) and [k2-fsa/sherpa-onnx](https://deepwiki.com/k2-fsa/sherpa-onnx); [sherpa homophone replacer docs](https://k2-fsa.github.io/sherpa/onnx/homophone-replacer/index.html); Groq STT docs; [Lucidnote on Whisper’s 224-token front truncation](https://lucidnote.net/en/whisper-prompt-silent-truncation).

These are **four different machines**. Calling them all “hotwords” hides why VoiceFlow’s current 32-word prompt is weak for 知乎 / 晓雯.

| Mechanism | What it actually does | Limits | Fit for VoiceFlow |
| --- | --- | --- | --- |
| **Whisper / Groq `prompt`** | Soft decoder bias / style priming. Audio wins if it contradicts the prompt. **Silent truncate at ~224 tokens, from the front.** CJK often costs **1.2–1.4 tokens per character**. | Groq documents 224 tokens. A 2000-character prompt is not safe. Front-truncation drops whatever you put first. | **This is the only ASR lever VoiceFlow has today.** Budget in tokens, put the **highest-priority terms last**. |
| **Paraformer / ContextualParaformer / SeacoParaformer hotwords** | Decoder-time embedding / logit bias. `generate(hotword=…)` or a weighted `.txt` (`阿里巴巴 20`). | Recommend ≤ **1000** terms, ≤ **10** characters each, weights 1–100. | Real biasing for Chinese. Not this harness’s completion condition (plan already says so). Same lexicon rows should feed it later. |
| **Fun-ASR-Nano** | Hotwords go into a **structured LLM user prompt** (language / ITN / hotwords), ChatML-style. | Prompt-length limited like any LLM-ASR. | Closer to Whisper than to Paraformer. Still better structured than a comma list. |
| **SenseVoice + sherpa Homophone Replacer** | **Post-ASR**, not beam hotwords. `lexicon.txt` maps words → pinyin; user `replace.fst` maps pinyin sequences → canonical Hanzi (`玄界新片` → `玄戒芯片`). O(n) in text length, independent of rule count. | Rules are pinyin-exact (tone matters). Need multiple rules for tone variants. | The right tool for 知呼→知乎 once a local FunASR path exists. Equivalent *today* is **local token replace**. |
| **FunASR `postprocess_hotwords`** | Deterministic `wrong=>right` after ASR. Explicitly for large vocabularies. | After decode; cannot recover a totally different hypothesis. | Same job as VoiceFlow local replace. |

sherpa-onnx also exposes `hotwords_file` / `hotwords_score` / `hotwords_buf` (per-utterance) on transducer/online paths, plus Fun-ASR-Nano `system_prompt` / `user_prompt`. Whisper in sherpa is configured with `language` + `task`, not the same initial-prompt surface.

**Takeaway:** until FunASR is wired, VoiceFlow must treat Groq Whisper as a **tiny, front-truncating bias channel** and treat lexicon apply as **replace**. Do not assume “32 words / 2000 chars” reaches the model.

---

### 2.9 Other 2025–2026 apps that learn from corrections

| Product | Learn / promote | Apply | Notes vs VoiceFlow |
| --- | --- | --- | --- |
| **VoiceInk** | Experimental auto-learn: after paste, diff + **Apple NER** (names/places/orgs). Users report junk (“Pattern”). Manual Vocabulary + Word Replacements are the real product. | Vocabulary is **LLM enhancement context only** — does **not** change raw ASR. Replacements run after paragraph format, **before** AI, case-insensitive, word-boundary (CJK falls back to substring), longest-first. Enhancement skipped on short text. | Do **not** copy NER auto-add. Do copy “replace before LLM” and “skip enhancement when short.” VoiceFlow’s hit-only cleanup is already better than dumping the whole vocab into the enhancer. |
| **platx-ai/Talk** (“Open Typeless” on-device) | Post-inject field observe; **background LLM extracts** corrections; ⚡ capsule. Manual vocab + history. | **Top corrections injected into the LLM system prompt** and auto-applied. Per-app polish profiles (Terminal / VS Code / WeChat). MLX Qwen3-ASR + Qwen3.5-4B; ~1s warm pipeline. | Fast local polish, but **LLM-extract + LLM-apply** is the opposite of VoiceFlow’s latency/privacy goals. Do not send observed field diffs to a model to decide what to learn. |
| **Vowrite** | Two AX snapshots (1s baseline, 5s current). Auto-add Replacement. Toast. Toggle. | Replacement manager only; dictation engine untouched. | Simple 1× learn. No 3×, no tombstone in the commit notes. |
| **Voquill** | Manual glossary + replacement. No public auto-learn. | Glossary + rules. | BYOK / local option. Not a harness. |
| **typeless-but-free** | Click lingering card to correct; remembers. | Replacement rule + faster-whisper hotwords. 100% local. | Explicit user confirm — closer to History chips than silent observe. |
| **typelessless** | Manual vocabulary. | Soniox custom-vocab bias + 4 cleanup modes (`chatting`…`polish`) routed by focused window. | Mode routing is the Wispr-style alternative to VoiceFlow scene skip. No learn loop. |
| **Handy** | None. Global Replace is a requested MacWhisper clone ([discussion #1235](https://github.com/cjpais/Handy/discussions/1235)). | — | Twin stack, no harness. |
| **FluidVoice** | No documented correction memory. | Speed path: local stream + optional Groq/LLM. | Latency, not learning. |
| **OpenTypeless (tover0314 / opentypeless.com)** | Manual terms + local correction rules. | Dictionary sent to **LLM polish** as context. | Product cousin of VoiceFlow today, not a learn loop. |
| **Typeless (closed)** | Auto + manual (from VoiceFlow’s 2026-08-20 notes). Speak-to-edit, per-app word choice. | Cloud. | Don’t copy Ask Anything / profanity-as-feature. |

---

## 3. Comparison table

Legend: **ASR** = prompt / decoder bias. **Replace** = deterministic local rewrite. **LLM** = cleanup / polish prompt. **HR** = homophone FST.

| System | What is learned | Promote | ASR | Replace | LLM | Scope | Privacy | Speed tricks |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| **VoiceFlow today** | Single-token after-forms | 3× observe or History | First 32 words, 2000 chars | Spoken punct only | First 32 words every cleanup | Global list, no affinity | Same-field AX; no keylog; Groq audio | None |
| **VoiceFlow plan** | Lexicon + style drafts | Lexicon 3×; style human-only | Ranked 32 / ~2000 chars | Token replace before LLM | Hit terms ≤8 + 1 approved example; scene skip | Global + family/bundle rank | Same + no title/URL to model | Skip LLM in chat/search/form/terminal |
| **Open Typeless Harness** | Edit-trail skills | README 2-track; **code is candidate-only** | — | — | Retrieve skills into polish | Local memory | Local JSONL of field text; 30s window | VIH off by default |
| **Wispr Flow** | ≤4-word entities + misspell rules + snippets + style category | 1× if classifier accepts; undo toast | Boost + star/usage rank | Misspelling rules | Global cleanup intensity + category style | Global + team share | Cloud sync | Session-scoped dict on Android; snippets skip compose |
| **Willow** | Terms + shortcuts + tone | 1× or 2× (marketing) | Cloud recognize | Shortcuts via “shortcut” keyword | Auto tone per app | Personal + team | Cloud; “trains Willow” | ~200ms claim; hold-PTT |
| **Superwhisper** | Manual vocab + replacements | Manual | Sparse hints | Primary fix | Per-mode; Voice-to-Text skips | Global | Local optional | Replacements free |
| **MacWhisper** | Manual replace pairs | Manual | Ad-hoc prompts | Global Replace, whole-word toggle | Optional | Global | Local | Replace only |
| **TalaX** | Word diffs from review | **3×** auto-apply | — | L1 &lt;1ms + L2 n-gram + L3 phonetic | None | Isolated profiles | Local; review-first paste | No LLM |
| **typwrtr** | Pairs + proper-noun vocab | **1×** after tombstone; was 3× | Top-20 app + 10 global + 10 targets, **~800 chars** | Word-boundary + Metaphone | Removed | Per-app profile + global | Local; Forget tombstone; per-app learn off | Deterministic scrubs |
| **TypeWhisper** | Terms + corrections | History 1×; AX **commit-gated** single-token; undo | Terms, **600 chars**; Parakeet boost | Corrections in pipeline | Separate Prompt Actions | Global; memory stores app | Local; premium auto-learn | LLM not on default path |
| **YazSes** | Tuner proposals from opt-in corpus | Human approve; held-out check | App name + vocab (+ editor) | Disfluency / commands | Optional loopback LLM | Global config | No keylog; encrypted corpus; no upload | Regex commands first |
| **FunASR / sherpa** | Operator-supplied lists | Manual / file | Paraformer decode bias; Nano prompt | SenseVoice HR FST; postprocess map | Nano is LLM-ASR | Whatever you load | On-device if sidecar | FST O(n); skip LLM if punc-model enough |
| **VoiceInk** | Manual; experimental NER | NER 1× (noisy) | No | Before enhancement | Full vocabulary if enhancement runs | Global | Local + optional cloud enhance | Skip enhance on short text |
| **Talk (platx)** | Observe + **LLM extract** | Immediate + toast | — | Implied apply | Top corrections in **system** prompt | Per-app polish profiles | On-device MLX; still LLM-reads edits | ~1s warm local |

---

## 4. What VoiceFlow’s plan already does better (keep)

1. **Global lexicon + family/bundle affinity, not isolated DBs.** TalaX profiles and typwrtr per-app vocab hide a name learned in WeChat from Slack. Wispr/Willow go the other way and sync a team dictionary to the cloud. The plan’s “one table, rank by where you just were” is the right third path.
2. **Cleanup is hit-only and capped (≤8).** VoiceInk and OpenTypeless-style products dump vocabulary into the enhancer. Talk stuffs top corrections into the **system** prompt. Wispr’s cleanup intensity is global. VoiceFlow’s “only terms that appear in this transcript” is the only design that stays fast as the 256-list grows.
3. **Scene skip instead of a global cleanup knob.** Wispr None/Light/Medium/High still runs one policy everywhere. Superwhisper modes are manual. Skipping PersonalChat / search / form / terminal unless rewrite intent is the actual fix for “微信写成邮件,” and it is the largest cleanup-latency win in the plan.
4. **Style never silent-promotes.** YazSes is the academic version of this (tuner emits diffs). Wispr/Willow auto-tone is why chat sounds like support email. One confirmed `style_example` is enough.
5. **Same-field, verified-paste, Secure Input, no keylog.** Stronger than Open Typeless’s 30s JSONL monitor, Talk’s LLM-on-edits, and YazSes’ optional audio corpus. History confirm remains the safer first knife.
6. **3× default, not 1×.** Matches TalaX; matches typwrtr *before* they had tombstones. Correct for CJK homophones where a one-off edit is often “I changed my mind,” not “Whisper was wrong.”
7. **Writing mode still owns aggression.** Lexicon may know TypeScript; WeChat still cannot grow a 您好. Harness ≠ rewrite policy.
8. **No screenshot / RL / cloud training.** Keeps the product in the YazSes / Superwhisper privacy class while still using Groq as BYOK.

---

## 5. What the plan is missing that real products do (and that fits)

These fit the constraints. They are not FunASR-in-process, not team sync, not keylog.

1. **Token-aware Whisper prompt (typwrtr 800 chars, TypeWhisper 600 chars, Groq 224 tokens).** The plan still says “32 items / ~2000 chars.” Whisper **silently drops the front**. CJK is expensive. If family terms are prepended, they are the first thing truncated. Superwhisper’s “too many vocab words confuse the model” is the same bug.
2. **Multiple `before` surfaces → one `after` (Superwhisper, Wispr misspelling rules, FunASR postprocess).** The plan stores `before` history but does not say every variant is applied. `配森` and `派森` must both become `Python`.
3. **Undo toast + tombstone (Wispr, TypeWhisper, typwrtr).** Without this, 3× is correct but 1×-class proper nouns stay slow, and a bad promote reappears forever. typwrtr only dropped 3→1 after Forget existed.
4. **Short phrases, not only single tokens (Wispr ≤4 words; Open Typeless `type script→TypeScript`; VoiceInk longest-first).** The plan’s single-token rule is safe but misses the highest-value English tech terms.
5. **User pin / star in the same ranker Wispr shipped.** Affinity + recency is good; a pinned `晓雯` must never fall out of the 224-token window.
6. **Commit-gated observe (TypeWhisper PRs 770/878).** Idle-extend is good; WeChat users hit send and leave the field. Return / send / focus-loss should **commit** the last high-confidence candidate, not discard it as `LeftTarget`.
7. **Replace before LLM, then give cleanup the pair not just the after-form (VoiceInk, Superwhisper).** If replace missed (boundary, extra space), the LLM still needs `知呼→知乎`, not a bare `知乎` sitting in a list of “preferred words.”
8. **Per-app learning off (typwrtr, YazSes opt-in).** Password managers / HR tools / 1Password should not observe. VoiceFlow already skips Secure Input; a mapping-level gate is the remaining hole.
9. **Cheap deterministic scrubs so more turns skip LLM (typwrtr, YazSes, TalaX L1).** Collapse immediate repeats and a tiny Whisper-hallucination bag (`Thanks for watching.`) are O(n) and make “skip cleanup” safer.
10. **Whole-word / CJK-span replace with longest-match-first (MacWhisper, VoiceInk, TalaX).** The plan already says “independent token, no mid-word.” Spell longest-first if phrases land.

---

## 6. What VoiceFlow should **not** copy

| Tempting idea | Who does it | Why not |
| --- | --- | --- |
| Upload corrections / “trains the model” | Willow marketing, Wispr cloud | Constraint + privacy.md |
| Team / shared dictionary | Wispr, Willow | Cloud identity, surprise term injection |
| Isolated per-app or per-profile lexicons | typwrtr, TalaX | Names die at the app boundary; 32 slots fragment |
| 1× auto-promote without undo/tombstone | Wispr, typwrtr-now, Vowrite | CJK false friends; VoiceFlow chat is high-stakes |
| NER auto-add | VoiceInk experimental | Adds “Pattern”; fights the 224-token budget |
| LLM extracts corrections from the field | Talk | Extra Groq call; field text leaves the machine; slow |
| Skills only applied in LLM polish | Open Typeless, OpenTypeless.com, VoiceInk vocab | That is today’s VoiceFlow failure mode |
| Global cleanup intensity as the only knob | Wispr Auto Cleanup | Recreates 微信-as-email |
| Silent style / auto tone | Willow, Wispr defaults | Plan’s human-confirm is the differentiator |
| N-gram / fuzzy / Metaphone as P0 | TalaX L2/L3, typwrtr Metaphone | Accuracy theater until replace + skip-LLM ship |
| Fine-tune / audio excerpts / DPO | TalaX roadmap, FunASR training | Explicitly out of scope |
| Always-on encrypted audio corpus | YazSes opt-in | Recovery spool already exists; don’t add a second corpus |
| 30s monitor + JSONL of field values | Open Typeless experiment | Privacy and battery; VoiceFlow’s 3–12s idle-extend is enough |
| Screenshots / screen assistant as learn signal | — | Plan + existing security bar |
| Global keylog / event tap | — | Constraint |
| FunASR Python in Tauri / weight hotwords as harness done | FunASR toolkit | Already deferred correctly |
| Meeting notes, Ask Anything, agent VIH | Wispr, Typeless, Open Typeless VIH | Different product |

---

## 7. Ranked plan amendments

Each item is a **plan change**, not an implementation. Priority is “accuracy and/or cleanup latency under current constraints.”

### P0 — change these before or with Phase 1

**P0-1. Token-budget the ASR prompt; put the highest-priority terms last.**

- Why accuracy: Groq/Whisper **silently front-truncates at ~224 tokens**. A 2000-character / 32-word cap is not a token cap. 32 CJK names can blow the window; family-first **prepend** then drops the terms you just ranked up.
- Why latency: a confused prompt also makes cleanup more likely to run and to rewrite.
- Concrete: estimate tokens (Latin ≈ 0.75–1 tok/word; CJK ≈ 1.2–1.4 tok/char). Hard cap **~200 tokens** (leave 24 of slack). Compose `Recognize these terms exactly when spoken: ` + ranked terms **lowest → highest**, so truncation eats stale Slack words, not today’s 知乎. Keep the technical-preserve clause **after** the list only if it still fits; otherwise drop it on chat families. typwrtr’s 800-char / TypeWhisper’s 600-char English caps are the right order of magnitude — **tighter for CJK**.

**P0-2. Apply every stored `before` variant locally, longest-first, then pass **pairs** (not after-only) into hit-only cleanup.**

- Why accuracy: Superwhisper/Wispr/FunASR postprocess all treat replace as the reliable layer. One `after` with many `before`s is the real lexicon. Cleanup seeing only `知乎` cannot fix a leftover `知呼` if replace missed a boundary.
- Why latency: a hit replace often makes chat **skippable**. Cleanup user message stays tiny: at most 8 `before→after` lines that actually appear.
- Concrete: Phase 1 replace walks promoted pairs (and user-confirmed misspellings), independent-token / CJK-span, stored case on the right-hand side. `bounded_dictionary(transcript)` becomes `bounded_pairs(transcript)`.

**P0-3. Undo toast + tombstone on auto-promote (and on user delete).**

- Why accuracy: Wispr/TypeWhisper/typwrtr all treat recoverability as what makes auto-learn safe. A tombstoned pair must not climb 3× again from the same observe.
- Why latency: none directly; it is what later lets a 2× proper-noun fast path exist without wrecking chat.
- Concrete: HUD or banner “已学 知呼→知乎 · 撤销”; delete in settings writes `tombstoned_at`; observe ignores tombstones.

### P1 — after Phase 1–3 work, still inside this harness

**P1-1. High-confidence short phrases (2–4 Latin words or 2–8 CJK chars), longest-match replace.**

- Why accuracy: Wispr’s “Project Northstar” and Open Typeless’s `type script→TypeScript` are the terms Whisper splits. Single-token-only leaves the best English tech corrections on the floor.
- Gate: same conservative classifier Wispr publishes (no grammar/style/caps-only/fillers/pure insert). History chips can confirm phrases; observe still requires repetition.

**P1-2. Pin/star in the existing ranker.**

- Why accuracy: Wispr shipped this when dictionaries got large. A spouse name or `晓雯` must occupy the last (surviving) slots of the 224-token prompt every time.
- Cheap: one bool on the lexicon row; sort `pinned > family+bundle > last_used_at > hits`.

**P1-3. Commit-signal closes the observe window.**

- Why accuracy: TypeWhisper learned that timeout-only observe looks “flaky.” VoiceFlow already returns `LeftTarget` on focus mismatch and **drops** the candidate. If the last settled diff was a single token and the user hit Return / the field committed, **promote the hit**.
- Why latency: none; more correct lexicon → more replace hits → more LLM skips.

**P1-4. Mapping-level “don’t learn here.”**

- Why accuracy/privacy: typwrtr’s per-app learning gate. Secure Input is not enough (HR web forms, unlocked notes).
- Observe and History auto-suggest off; manual dictionary add still allowed.

**P1-5. Tiny deterministic scrubs on the skip path.**

- Why latency: typwrtr deleted a 3–5s T5 for `collapse_repeats` + hallucination lines. YazSes’ non-decode path is &lt;1 ms. More PersonalChat turns can skip Groq cleanup without looking broken.
- Why accuracy: residual Whisper ads should not require an LLM.
- Keep this **smaller than TalaX L3**. No Metaphone/Levenshtein as a default.

**P1-6. Fast-path promote for unique proper nouns *after* P0-3.**

- Why accuracy: Willow/Wispr feel “instant” on names. 3× is painful for `晓雯`.
- Concrete: if the right-hand side looks like a name (CJK 2–3 chars, or Latin mixed-case ≥3, not a stopword) **and** the user has undo/tombstone, allow **2×** or one History confirm. Never 1× silent. Never for style.

### P2 — later, same data, not harness-complete

**P2-1. Same lexicon → FunASR / sherpa when that backend exists.** Paraformer hotwords (≤1000) + SenseVoice `replace.fst` from pinyin of `before` → `after`. Plan already defers this; keep it deferred, but **don’t invent a second word list**.

**P2-2. AXObserver + fix-up hotkey.** Open Typeless / TypeWhisper reliability. typwrtr’s select-and-fix fallback for Electron/Chrome boxes that lie about `AXValue`.

**P2-3. Held-out check on style drafts (YazSes).** Third similar punct/density pattern must not be the same copied sentence. Reject drafts that look like a one-off rewrite.

**P2-4. Optional English phonetic assist.** TalaX/typwrtr Metaphone is useful for `Rishab→Rishabh`-class errors replace cannot see. Only after pair-replace is measured.

**P2-5. Snippet placeholders** (`{{date}}` / `{{clipboard}}`) as in TypeWhisper / typwrtr / Wispr. Latency win (already skip LLM on snippet hit) but not a lexicon problem.

---

## 8. Suggested phase mapping onto the existing plan

Do **not** add phases. Fold P0 into Phases 1–2; P1 into 3–5.

| Plan phase | Keep | Amend |
| --- | --- | --- |
| 1 Rank + local replace | Family/recency rank; token replace; bump `last_used_at` | **P0-1** token budget + **end-weighted** order; **P0-2** all befores + longest-first; **P0-3** undo/tombstone |
| 2 Cleanup hit-only | Max 8 / ~400 chars; no full table | Hits are **pairs**; never pending style |
| 3 Scene skip | Chat/search/form/terminal skip | **P1-5** cheap scrubs on skip path |
| 4 Family/bundle on pairs | Rank only; no title/URL | **P1-2** pin; **P1-4** learn-off |
| 5 Style drafts | Human confirm; one example | **P1-3** commit-signal; **P1-1** phrases; **P1-6** name 2× only after tombstone |

Verification stays: `cargo test --manifest-path src-tauri/Cargo.toml --lib`, `npm test`, `npm run lint`, plus `metrics::MetricKind::Cleanup` skip rate. Add a unit test that a CJK-heavy 32-word list is **token-capped** and that the last slots are the current-family terms.

---

## 9. Source list

| Source | Used for | Retrieved |
| --- | --- | --- |
| VoiceFlow plan `complete_voice_harness_b6de6a80.plan.md` | Planned harness | 2026-08-23 |
| `docs/competitive-research.md` | Prior product framing | 2026-08-20 |
| VoiceFlow `dictionary_learn.rs`, `lib.rs` `build_asr_prompt`, `llm.rs` `bounded_dictionary` | Current behavior | 2026-08-23 |
| [OpenCodexLabs/open-typeless-harness](https://github.com/OpenCodexLabs/open-typeless-harness) README, USAGE, FUSION_*, CLAUDE.md | Experimental harness | 2026-08-23 |
| [Wispr dictionary article](https://docs.wisprflow.ai/articles/4052411709-teach-flow-your-words-with-the-dictionary) | Auto-learn rules | 2026-08-21 (page) / 2026-08-23 |
| [Wispr features](https://wisprflow.ai/features), what’s-new, styles docs | Snippets, star, cleanup levels | 2026-08-23 |
| [Willow help: dictionary & shortcuts](https://help.willowvoice.com/en/articles/13183918-using-personal-dictionary-and-shortcuts) | Manual terms / shortcuts | 2026-03-09 / 2026-08-23 |
| [Superwhisper vocabulary](https://superwhisper.com/docs/get-started/interface-vocabulary) | Vocab vs replace | 2026-08-23 |
| [MacWhisper Global Replace](https://docs.macwhisper.com/article/37-find-and-replace-in-transcriptions) | Whole-word replace | 2026-08-23 |
| [TalaX README](https://github.com/puretensor/talax-dictation) | 3× + 3-layer pipeline | 2026-08-23 |
| [typwrtr README](https://github.com/kaidhar/typwrtr) + [blog](https://www.kartikaydhar.com/blog/typwrtr) | Observe, 800-char prompt, tombstone | 2026-08-23 |
| TypeWhisper DeepWiki + PRs #752, #770, #878 | Commit-gated AX learn | 2026-08-23 |
| [YazSes arXiv:2607.28878](https://arxiv.org/html/2607.28878v1) | Tuner, no keylog | July 2026 / 2026-08-23 |
| FunASR + sherpa-onnx DeepWiki; [sherpa HR docs](https://k2-fsa.github.io/sherpa/onnx/homophone-replacer/index.html) | Hotword vs FST vs prompt | 2026-08-23 |
| [Groq STT docs](https://console.groq.com/docs/speech-to-text); [Lucidnote 224-token writeup](https://lucidnote.net/en/whisper-prompt-silent-truncation) | Prompt budget | 2026-08-23 |
| [VoiceInk vocabulary](https://tryvoiceink.com/docs/vocabulary) / [word replacements](https://tryvoiceink.com/docs/word-replacements); GitHub #637 | LLM-only vocab; NER | 2026-08-23 |
| [platx-ai/Talk](https://github.com/platx-ai/Talk) | LLM-extract learn | 2026-08-23 |

DeepWiki was used for TypeWhisper, FunASR, and sherpa-onnx. `open-typeless-harness` is **not indexed** there; GitHub raw files were used instead.
