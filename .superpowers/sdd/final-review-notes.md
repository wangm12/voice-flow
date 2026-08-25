# Final review notes (controller)

Work is **uncommitted** on `feat/dictation-slices`. HEAD `4d25811` is also the merge-base with `main`. Review the working tree, not a commit range.

**Plan:** `docs/superpowers/plans/2026-08-21-voiceflow-all-slices.md`

**Tracked diff package:** `.superpowers/sdd/final-review-working-tree.diff` (~570KB). Also read untracked slice modules listed in that file.

**Do not commit. Do not mutate git. Do not “fix” the two known unrelated suite failures:**
1. `store::tests::spool_writes_are_atomic_and_path_bounded` — umask `0o755` vs expected `0o700`
2. `SelectedPreviewDialog` — two buttons named `取消`

## Slices shipped (parent-reviewed)

1. Per-app tone: PersonalChat vs WorkChat prompts; HUD context label
2. Hybrid hotkey: short tap toggle / hold PTT; recapture keeps hybrid; Starting-phase PTT stop
3. HUD partials: `dictation://partial`; widen pill while words show
4. History dictionary: `single_token_candidates`; confirm-to-add; `dictionary_learn_enabled`
5. ASR BYOK: `asr_base_url` + Keychain `asr_api_key`; empty = Groq
6. Post-paste learn: 3s same-field poll; exactly one token; no keylog
7. CJK paste: safe AX insert else CJK IME → ABC around Cmd+V, restore
8. Punctuation, `input_gain`, Fn hint, `{{date}}` / `{{clipboard}}`; skipped optional `vad_sensitivity`

## Parent-noted concerns (triage, do not rubber-stamp)

- CJK dictionary: 9+ unspaced sentence one-char fix yields no candidate
- Compatible ASR still sends Groq Whisper multipart fields
- Post-paste learn compares pasted vs full field; persist may race `settings_gate`
- AX insert skipped for web/Electron; TIS is session-wide
- Spoken punctuation matches keyword strings at any char boundary (unspaced ASR)
