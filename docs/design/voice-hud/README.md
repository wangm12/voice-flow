# Recording HUD design archive

This archive keeps the reviewed recording HUD mockups and their rebuild sources. **Beam v10 is the selected and approved design reference.** Earlier versions remain here to make the review decisions recoverable.

Open [the v10 state atlas](beam-v10/preview.html) directly for its 60 interactive visual cases. Its selected capsule is 220 × 48 / R24, monochrome, with the same 32 × 32 action button and 10 × 10 stop square during recording and processing. Recording uses the Mono rotate Beam at strength 1, brightness 1.3, and 1.96 seconds; processing uses the Mono line Beam at strength 0.65 and 3.1 seconds. The v7 10 × 10 stop square is retained; the 14 × 14 v8 experiment was rejected; v9 restored the 10 × 10 shape across both active phases.

## Reviewed versions

- [Original A/B/C comparison](preview.html) — compares three recording HUD treatments and their lifecycle states.
- [Beam exploration](beam/preview.html) — first focused Mono Beam treatment with recording and processing controls.
- [Beam v2](beam-v2/preview.html) — compares Beam presets and Orb states.
- [Beam v3](beam-v3/preview.html) — compares the current reference size with compact capsule proposals.
- [Beam v4](beam-v4/preview.html) — explores a shared capsule across recording and processing.
- [Beam v5](beam-v5/preview.html) — compares target-app icon and Orb treatments.
- [Beam v6](beam-v6/preview.html) — fixes the text anchor and aligns Beam behavior across both phases.
- [Beam v7](beam-v7/preview.html) — original 10 × 10 stop-square treatment and simulated cancel flow.
- [Beam v8](beam-v8/preview.html) — rejected 14 × 14 stop-square experiment; kept as review history.
- [Beam v9](beam-v9/preview.html) — restores the 10 × 10 stop square for both recording and processing.
- [Beam v10](beam-v10/preview.html) — approved 60-case state atlas and selected design reference.

## View and rebuild

Each `preview.html` is a self-contained interactive prototype and opens directly in a browser without a server. Controls, timing, devices, states, recovery actions, and any result text are fixtures. The prototypes do not access a microphone, provider, clipboard, authentication, system settings, models, installation, or stored user history. Every recovery action is simulated.

To serve the latest source through Vite from the repository root:

```sh
node_modules/.bin/vite --config docs/design/voice-hud/beam-v10/vite.config.mjs --host 127.0.0.1 --port 4184 --strictPort
```

To rebuild its self-contained preview with the existing VoiceFlow dependencies:

```sh
node_modules/.bin/vite build --config docs/design/voice-hud/beam-v10/vite.config.mjs
node docs/design/voice-hud/beam-v10/inline-preview.mjs
```

Replace `beam-v10` with another version folder to rebuild that mockup; the original comparison uses the archive root. These commands add no dependencies. A rebuild writes that variant's `preview.html` from its source.

## Scope and attribution

These are design references, not evidence that the shown states, copy, motion, recovery behavior, accessibility, or timing are implemented in the app. The v10 state catalogue records which current code paths informed the mockup when it was made; proposed cases still require product and native-event work. Consult the app source and living documentation for current behavior.

Apple Notes and Safari icons included under `beam-v5` through `beam-v10` are extracted from macOS app icon resources and are used only as design-reference imagery inside the mockups. They are not VoiceFlow assets or runtime product assets. The v10 recovery actions and example result text are prototype fixtures; the result text is the reviewed sample, not captured user dictation.
