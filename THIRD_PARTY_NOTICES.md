# Third-party notices

VoiceFlow uses the following projects as documented design and implementation references:

- [OpenTypeless](https://github.com/tover0314-w/opentypeless) — MIT. VoiceFlow's context/profile/policy behavior is independently mapped to its own Rust types; no source file is vendored here.
- [OpenWhispr](https://github.com/OpenWhispr/openwhispr) — MIT. VoiceFlow references its recording and hotkey interaction patterns; it does not copy the Electron UI.
- [Tauri](https://github.com/tauri-apps/tauri), [Tauri global shortcut](https://github.com/tauri-apps/tauri-plugin-global-shortcut), and [Tauri clipboard manager](https://github.com/tauri-apps/tauri-plugin-clipboard-manager) — licenses are recorded in `Cargo.lock` and `package-lock.json`.

If future changes copy source code from an external project, add that project's license text and attribution here before merging.
