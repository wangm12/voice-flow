# VoiceFlow native speaker-filter helper

This subtree is a standalone Rust crate for the phase-1 CPU pilot. It has no Tauri dependency, microphone access, Keychain access, network client, or cloud API. The CLI reads local WAV/profile/model files and writes local private files. Build outputs belong outside the repository.

## Build and run

The Alkove Rust crates are pinned to commit `28fe044787952af5b6b6e59b3dd92de2b12eee36`; `Cargo.lock` pins the complete dependency graph. Three model files must already be present in a local absolute directory and pass byte length, SHA-256, and model-name checks before the engine loads them:

| Model | File bytes | SHA-256 | Model ID |
| --- | ---: | --- | --- |
| Focus | 1,111,696 | `a63cda32e023b8d76089f4c3df0cc2c69ba36017b8f6cdea1da1094ef8022b6a` | `alkove-focus-1-preview@a63cda32e023` |
| Cue | 1,066,516 | `977cd614996498f468536effbc2fb05138331751ca21572cf07b24184b4e7911` | `alkove-cue-1-preview@977cd6149964` |
| Timbre | 739,632 | `d498a7dd8b20528d783052b1e1d49e54ab7ee4e47aa12712d04f3d43edc68bd8` | `alkove-timbre-1-preview@d498a7dd8b20` |

Build without placing artifacts in the application tree:

```sh
cargo test --manifest-path src-tauri/speaker-filter/Cargo.toml \
  --target-dir /absolute/private/research/build/native
cargo build --release --locked --manifest-path src-tauri/speaker-filter/Cargo.toml \
  --target-dir /absolute/private/research/build/native
```

The binary interface is:

```text
voiceflow-speaker-filter enroll --models ABS --input ABS_WAV --output ABS_PROFILE
voiceflow-speaker-filter filter --models ABS --profile ABS_PROFILE --input ABS_WAV --output ABS_WAV --recipe g0|g1 [--paced] [--report ABS_JSON]
voiceflow-speaker-filter serve --models ABS
```

Input WAV must be mono 16 kHz PCM16, PCM24, PCM32, or IEEE float32 and at most 15 minutes. Filter output is mono 16 kHz float32 WAV. Enrollment output, filtered WAV, and report are created with mode `0600` on Unix. Enrollment profile creation is create-new: an existing profile is preserved. Filtering writes through a private temporary file and atomically replaces the designated output path. Input, profile, output, and report paths must be distinct.

## Enrollment profile

Enrollment operates on the raw input timeline. The source must be 15–30 seconds. The whole WAV must contain at least one nonzero sample, all samples must be finite, and the fraction with `abs(sample) >= 0.999` must be at most 1%.

Raw Cue runs independently from Focus. Each Cue posterior at or above `SPEECH_DEFAULTS.confidence` (`0.5`) accepts its corresponding 480-sample source interval. The profile stores the union of accepted half-open sample intervals and requires at least 160,000 accepted samples. The final delayed Cue frame is flushed and clipped to the source length. Each of three non-overlapping source thirds must have at least 24,000 Cue-accepted samples.

Timbre embeds the whole source and each third as `raw`. Pairwise third consistency uses upstream `timbre::verify` with `Level::Balanced`; every pair must pass. This temporal consistency check is not identity authentication. The profile stores the exact source WAV SHA-256, duration, sample format, accepted intervals/counts, clipping count/fraction, model IDs and set digest, three score/threshold verdicts, and both vectors. `focus_embedding` is the 192-value vector passed to Focus; `verification_id` is the separate Timbre ID vector. Both are finite, nonzero, L2-normalized, and validated before `Focus::set_print`.

The JSON profile has schema version 1 and a SHA-256 checksum over the typed payload (excluding the checksum field). File size is capped at 512 KiB. Any source, quality, model, vector, or checksum mismatch rejects the profile. No raw audio is stored in the profile; embeddings and speaker IDs are sensitive local biometric data.

## Filter recipes and timeline

- `g0`: 16 kHz mono input goes through upstream Focus in 256-sample hops. Cue observes Focus output only. Cue probabilities never gate output.
- `g1`: applies the upstream `InputGain` before Focus, including its upstream input-floor tracker. The extracted source and attribution are in `src/gain.rs`. It does not add output normalization.

Both recipes preserve the complete sample timeline. The helper pads only enough to complete the last 256-sample Focus hop, flushes one further 256-sample hop for Focus's one-hop delay, discards the initial 256 delayed output samples, then truncates to the exact valid input sample count. Filtered output length must equal input length. A new helper stream constructs fresh Focus, Cue, input-gain, and floor state.

`--paced` feeds file audio in batches of at most 1,024 samples and sleeps to keep each batch on its source-time schedule when processing is faster than real time. The report includes sample counts, model delay, padding, model/profile/source/output hashes, Cue observation frame count, initialization/processing elapsed times, and processing real-time factor. It includes no audio samples or embeddings. In paced runs, processing elapsed time and derived wall-clock RTF include intentional replay waits, so they cannot be compared with the compute RTF threshold of `≤ 0.2`. Unpaced helper RTF describes native compute and buffering only; it does not measure stop-to-drain `≤ 200 ms` or complete end-to-end latency.

## Binary IPC (`serve`)

One helper process handles one active stream at a time. The host supervises and can kill the process. Frames are length-prefixed with a 4-byte big-endian payload length (tag included), maximum payload 1 MiB. Integers are big-endian; audio is little-endian IEEE float32. Reads support fragmented pipes and reject truncation. There are no background worker threads or unbounded audio queues; the helper reads, processes, and writes synchronously.

| Direction | Tag | Payload after tag |
| --- | ---: | --- |
| Host → helper | `0x01 Begin` | session ID `u64`, audio epoch `u64`, recipe `u8` (`0=g0`, `1=g1`), model-set SHA-256 (32 raw bytes), exact profile-file SHA-256 (32 raw bytes), profile JSON length `u32`, profile JSON bytes |
| Host → helper | `0x02 Audio` | session ID `u64`, audio epoch `u64`, sequence `u64`, source start sample `u64`, valid sample count `u16` (1–1024), that many float32 samples |
| Host → helper | `0x03 Finish` | session ID `u64`, audio epoch `u64`, total valid input samples `u64`, next sequence `u64` |
| Host → helper | `0x04 Cancel` | session ID `u64`, audio epoch `u64` |
| Helper → host | `0x80 Ready` | session ID, epoch, model-set digest, profile-file digest, recipe |
| Helper → host | `0x81 Output` | session ID, epoch, output sequence, source start sample, valid count (1–1024), float32 samples |
| Helper → host | `0x82 Finished` | session ID, epoch, status (`0=complete`, `1=cancelled`), received input sample count, output sample count |
| Helper → host | `0x83 Failure` | session ID, epoch, numeric code; no text or payload details |

Failure codes are `1=malformed frame`, `2=invalid stream state`, `3=session/epoch/model/profile mismatch`, `4=noncontiguous sequence or sample offset`, `5=invalid audio`, and `6=filter/flush failure`.

Audio sequence and source offsets must be contiguous from zero. `Begin` validates all identities and profile bytes before starting. Mismatched session IDs or epochs are rejected without feeding or flushing their audio into the active session. `Finish` flushes and asserts equal valid input/output counts. `Cancel` drops stream state without flushing. A process restart or new `Begin` creates fresh state. Failure messages contain no audio, embeddings, paths, or error strings.

## Host-side supervision foundation

`src/supervisor.rs` adds a reusable host contract for a future per-session helper. It is not called by the Tauri application and does not enable the failed Alkove filtering recipe. `StreamProtocol` separates the host lifecycle from one candidate's Begin/Audio/Finish/Cancel encoding; `ExistingHelperProtocol` maps the existing binary protocol and checks the model/profile/recipe proof returned by `Ready`.

`HostSession::push` copies each raw block into host-owned memory before attempting helper IPC. An IPC queue overflow, timeout, child crash, malformed or stale result, or timeline mismatch latches one fallback reason and stops the helper; later `push` calls continue adding raw audio. Non-finite raw samples also latch `InvalidRawAudio`; capture can continue for bounded diagnostics, but finalization returns `FinalizeError::InvalidRawAudio` and exposes no audio selection. The default session limit is 15 minutes (14.4 million samples). Raw and filtered timelines each use fixed 1,024-sample blocks, with a 57,600,000-byte sample limit and at most 2,048 bytes of final-block padding; block metadata is bounded by the same sample limit. Together they use at most 115,204,096 bytes for block sample arrays, plus bounded metadata, one profile-sized Begin frame, and at most eight queued input frames. The response parser holds one frame up to 4,131 bytes and drains at most 64 KiB or 32 frames per pump. This is in-memory retention only: there is no durable or atomic raw-audio spool in this crate.

The parent pipe ends are nonblocking and use bounded `poll` reads/writes. A separate watchdog owns the child handle, checks deadlines with nonblocking `try_wait`, and kills the helper process group. Each session uses a fresh child, so a previous epoch cannot write into the next session's pipe. A `Finished` response is accepted only after the complete host `Finish` frame has been written; duplicate completion, completion during capture, mismatched totals, duplicate `Ready`, and output after completion trigger raw fallback. `finish(self)` makes one filtered-or-raw selection when the retained raw timeline is complete and finite, within a budget capped at 200 ms. Its tail drain budget is `finish_timeout - shutdown_reserve`; the configurable shutdown reserve covers stop and reap attempts, and `finish_elapsed` reports the measured selection time. Incomplete or non-finite raw input returns a typed `FinalizeError` without exposing an `AudioSelection`. `AudioSelection::child_reaped` reports whether the OS confirmed direct-child closure before return. If closure is unresolved or `try_wait` fails, a complete finite raw timeline is returned and one process-wide child lease keeps the handle; invalid or incomplete raw input returns a typed error instead. New helper launches then return `SupervisorBusy` until a later nonblocking reap check confirms closure; the supervisor never starts an unbounded waiter or join. OS scheduling can exceed the requested wall-clock budget. `cancel(self)` returns no audio selection and uses the same bounded teardown path.

The supervisor is a library foundation, not a production audio integration. It does not capture a microphone, select a filter candidate, store user profiles, add a setting, or change the installed app's data flow. The phase-1 candidate remains no-go; no quality, latency, or release claim follows from helper tests.

## Upstream notices

The helper links to Alkove core and engine from the pinned upstream source. The private `InputGain` and input-floor code are extracted from that same source revision because the upstream module is private. `ALKOVE-LICENSE.txt` and `ALKOVE-NOTICE.txt` retain upstream licensing and third-party notices.

## Phase-1 pilot outcome

The 2026-10-25 exploratory run compared raw and G0-filtered inputs with local native ASR: 90 prepared public publisher-reference cases, 180 rows, three target publisher speaker IDs, and two interferer IDs. It was not a locked cohort or release qualification. Twelve of the 90 cases were other-only; all 12 G0 results were nonempty, above the intended 1% ceiling for this candidate. A short Chinese case also lost its initial `我`. The same six selected short/soft cases were processed with G1; native float and canonical PCM16 outputs were byte-identical to G0 on those cases, so G1 did not resolve the observed onset loss. G1 was not run across all 90 cases. This Focus/Cue/Timbre candidate is therefore no-go and is not integrated into VoiceFlow. The result does not rule out all future filtering approaches. Detailed audio and per-case reports remain outside the repository.
