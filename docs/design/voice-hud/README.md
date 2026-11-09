# Voice HUD design reference

[Beam v10](beam-v10/preview.html) is the approved 60-case HUD state atlas. The capsule is 220 × 48 pt with R24 corners, a 32 × 32 action button, and the same 10 × 10 stop square during recording and processing. Earlier iterations established the shared capsule and text anchor; v7's 10 × 10 square remained selected, v8's 14 × 14 experiment was rejected, and v9 restored the 10 × 10 square in both phases. Earlier prototype source is recoverable from Git history.

The atlas data is in [`states.json`](beam-v10/src/states.json), its relationship to current UI paths is in [`state-source-map.json`](beam-v10/state-source-map.json), and the selected source is under [`beam-v10/src/`](beam-v10/src/). The preview is a self-contained HTML file that opens directly in a browser. Its devices, timing, result text, and recovery actions are simulated; it does not access a microphone, provider, clipboard, authentication, system settings, models, or user history. A design reference does not show that its states or interactions are implemented in the app.

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
