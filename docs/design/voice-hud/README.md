# Voice HUD design reference

VoiceFlow 当前的整体设置界面风格和 Liquid Glass 规则见 [视觉设计规范与 Liquid Glass 指南](../voiceflow-design-system.md)。本文只记录 HUD 状态图集、胶囊方案和可重建预览。

[Beam v10](beam-v10/preview.html) is the approved 60-case HUD state atlas. The capsule is 220 × 48 pt with R24 corners, a 32 × 32 action button, and the same 10 × 10 stop square during recording and processing. Earlier iterations established the shared capsule and text anchor; v7's 10 × 10 square remained selected, v8's 14 × 14 experiment was rejected, and v9 restored the 10 × 10 square in both phases. Earlier prototype source is recoverable from Git history.

The atlas data is in [`states.json`](beam-v10/src/states.json), its relationship to current UI paths is in [`state-source-map.json`](beam-v10/state-source-map.json), and the selected source is under [`beam-v10/src/`](beam-v10/src/). The preview is a self-contained HTML file that opens directly in a browser. Its devices, timing, result text, and recovery actions are simulated; it does not access a microphone, provider, clipboard, authentication, system settings, models, or user history. A design reference does not show that its states or interactions are implemented in the app.

For the live macOS HUD, the foreground palette follows the effective appearance and system color scheme. The primary capsule keeps one 92% opaque neutral tint while native glass is visible and as it leaves; the tint limits raw desktop contribution to the remaining 8%, so a native visibility event cannot switch the capsule to a lighter solid shell. Native glass stays visible while an existing HUD action, details, or recovery owner is active, including the pre-paste focus handoff; focus relinquish and click-through remain separate. Synthetic beams and the capsule rim stay suppressed for the full Liquid Glass appearance lifetime. The monochrome processing orb mirrors its ink for light Glass surfaces and keeps its original light ink on dark Classic and dark preview surfaces. Details, recovery, and learning surfaces use the matching opaque theme shell because native material is anchored to the primary capsule; their transparent border keeps the one-pixel inset and their buttons retain a keyboard focus ring. A recording caption uses a headphone glyph for names that identify AirPods, headphones, headsets, earbuds, or earphones, and keeps the generic input-device glyph for other names. While waiting for the first audio frame, the HUD keeps the target-app avatar when available or shows the ordinary microphone glyph; capture failures use the crossed microphone cue. The Settings preview remains simulated, and the isolated native glass POC keeps its separate behavior.

## Serve and rebuild

Run from the repository root with the existing dependencies:

```sh
node_modules/.bin/vite --config docs/design/voice-hud/beam-v10/vite.config.mjs --host 127.0.0.1 --port 4184 --strictPort
```

To rebuild the self-contained preview:

```sh
node_modules/.bin/vite build --config docs/design/voice-hud/beam-v10/vite.config.mjs
node docs/design/voice-hud/beam-v10/inline-preview.mjs
```

The v10 config imports the one shared [`vite.config.shared.mjs`](vite.config.shared.mjs). Build output is temporary under `beam-v10/.inline-build/`; `preview.html` is tracked and should be updated only when the rebuilt design is reviewed. See the [document index](../../README.md) and living docs for current product behavior.
