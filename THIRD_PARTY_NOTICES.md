# Third-party notices

## Design references

- [OpenTypeless](https://github.com/tover0314-w/opentypeless) — MIT. VoiceFlow's context/profile/policy behavior is independently mapped to its own Rust types; no source file is vendored here.
- [OpenWhispr](https://github.com/OpenWhispr/openwhispr) — MIT. VoiceFlow references its recording and hotkey interaction patterns; it does not copy the Electron UI.

## Application frameworks

[Tauri](https://github.com/tauri-apps/tauri), its [global-shortcut](https://github.com/tauri-apps/tauri-plugin-global-shortcut) and [clipboard-manager](https://github.com/tauri-apps/tauri-plugin-clipboard-manager) plugins, and the JavaScript API are application dependencies. The Rust crate versions are pinned in `src-tauri/Cargo.lock`; the JavaScript API version is pinned in `package-lock.json`. Those lockfiles do not contain license texts.

## Shared interface primitives

`@radix-ui/react-select`, `@radix-ui/react-popover`, and their Radix, Floating UI, and React focus/scroll support dependencies are MIT-licensed interface dependencies, pinned in `package-lock.json`. VoiceFlow styles its own shared controls from the user's Codex screenshots; no Codex source code is included.

## Packaged Apple-silicon MLX sidecar

For Apple-silicon macOS builds, the selected Xcode toolchain's `swift-stdlib-tool` scans the helper and copies its required Swift libraries beside the executable. The Xcode target keeps its macOS 14 deployment target and sets an `@executable_path` runpath for the bundled compatibility runtime, including `libswiftCompatibilitySpan.dylib`. The staged sidecar includes the helper, Xcode resource bundles, MLX Metal library, required Swift runtime libraries, full pinned upstream license/notice texts, and a generated dependency identity manifest. The existing Tauri stage-directory resource mapping carries these files into app resources. This packaging change is not an older-system runtime acceptance result.

The full upstream text files are in [`ThirdPartyLicenses/`](ThirdPartyLicenses/README.md). During packaging, every included `Upstream/` package license and notice is compared byte-for-byte with the matching pinned `SourcePackages/checkouts` file; missing or mismatched text fails the package step. The staged `ThirdPartyLicenses/DependencyManifest/` contains exact copies of both SwiftPM lockfiles, their SHA-256 values, and the nested MLX GitLink revisions. The local links below point to the included source copies; each bullet also identifies its pinned revision.

MIT-licensed components:

- `mlx-audio-swift` — MLX audio STT, core, codec, and VAD code; upstream revision `01dec7c9bdce3088a6b6b7ab9f2e403458195efb`. VoiceFlow applies the repository-owned `src-tauri/mlx-sidecar/sdk-overlay/voiceflow-mlx-audio-cancellation-v2.patch` to the three source files declared in its hash-bound manifest; the upstream MIT LICENSE is preserved unchanged. [Local LICENSE](ThirdPartyLicenses/Upstream/mlx-audio-swift/LICENSE) (Copyright © 2025 Prince Canuma).
- `mlx-swift` — MLX, Cmlx, MLXNN, MLXFast, and MLXOptimizers code and the Cmlx resource bundle; version `0.31.4`, revision `dc43e62d7055353c7f99fa071a4e71d29dfddc44`. [Local LICENSE](ThirdPartyLicenses/Upstream/mlx-swift/LICENSE) (Copyright © 2023 ml-explore).
- `mlx-swift-lm` — MLX language-model support code; version `3.31.4`, revision `bd4b7434e6bdb588c7ef55706ff8904cb7fd4c57`. [Local LICENSE](ThirdPartyLicenses/Upstream/mlx-swift-lm/LICENSE) (Copyright © 2024 ml-explore).
- `EventSource` — server-sent-events code; version `1.5.1`, revision `86b5096ac59ab46e66bd1f6377c604bc1dab0bc2`. [Local LICENSE](ThirdPartyLicenses/Upstream/EventSource/LICENSE.md) (Copyright © 2025 Mattt).
- `yyjson` — JSON code; version `0.12.0`, revision `8b4a38dc994a110abaec8a400615567bd996105f`. [Local LICENSE](ThirdPartyLicenses/Upstream/yyjson/LICENSE) (Copyright © 2020 YaoYuan).
- Nested Cmlx MIT components retain their own text files: [`fmt/LICENSE`](ThirdPartyLicenses/Upstream/mlx-swift/Source/Cmlx/fmt/LICENSE) (Copyright © 2012–present Victor Zverovich and fmt contributors), [`json/LICENSE.MIT`](ThirdPartyLicenses/Upstream/mlx-swift/Source/Cmlx/json/LICENSE.MIT), [`mlx/LICENSE`](ThirdPartyLicenses/Upstream/mlx-swift/Source/Cmlx/mlx/LICENSE), and [`mlx-c/LICENSE`](ThirdPartyLicenses/Upstream/mlx-swift/Source/Cmlx/mlx-c/LICENSE).

Apache-2.0-licensed components:

- `swift-numerics` — Numerics, RealModule, ComplexModule, and shims; version `1.1.1`, revision `0c0290ff6b24942dadb83a929ffaaa1481df04a2`. [Local LICENSE](ThirdPartyLicenses/Upstream/swift-numerics/LICENSE.txt).
- `swift-huggingface` — HuggingFace support; version `0.11.0`, revision `f2f99991f2d7d8fdb3187e4fd539cd2facf5c13d`. [Local LICENSE](ThirdPartyLicenses/Upstream/swift-huggingface/LICENSE).
- `swift-crypto` — Crypto code and its staged privacy-manifest resource; version `4.5.2`, revision `da9d28d69ebe3894b18376c8f2395c2f37b8448f`. [Local LICENSE](ThirdPartyLicenses/Upstream/swift-crypto/LICENSE.txt) and [local NOTICE](ThirdPartyLicenses/Upstream/swift-crypto/NOTICE.txt) (Copyright © 2019 The SwiftCrypto Project). The upstream NOTICE records SwiftNIO-derived source attribution and test-vector attribution; the sidecar link/resource inventory contains the `Crypto` target and no Crypto test-target resources. Its macOS package target does not link the BoringSSL targets, `CryptoExtras`, or SwiftASN1.
- `swift-transformers` — Tokenizers, Generation, Models, Hub, and the staged Hub resource bundle; version `1.3.4`, revision `c21fdcde390313a6d98d8e33a346f2c3486c3ab0`. [Local LICENSE](ThirdPartyLicenses/Upstream/swift-transformers/LICENSE).
- `swift-jinja` — Jinja code; version `2.5.1`, revision `4588064a20f3fc093c95f2f7d3359999bf30cae5`. [Local LICENSE](ThirdPartyLicenses/Upstream/swift-jinja/LICENSE).
- `swift-collections` — OrderedCollections and InternalCollectionsUtilities; version `1.7.1`, revision `98ef3c98609a1e31b7e157b5b619579001a789d6`. [Local LICENSE](ThirdPartyLicenses/Upstream/swift-collections/LICENSE.txt).
- Nested Cmlx `metal-cpp` headers retain their full Apache 2.0 [LICENSE.txt](ThirdPartyLicenses/Upstream/mlx-swift/Source/Cmlx/metal-cpp/LICENSE.txt).

The selected Xcode Swift compatibility runtime is included from the active toolchain. [`SwiftRuntime/LICENSE.txt`](ThirdPartyLicenses/SwiftRuntime/LICENSE.txt) preserves the complete Apache 2.0 and Runtime Library Exception text copied byte-for-byte from the pinned `swift-syntax` checkout. This is a conservative reference text; the exact source release and license mapping for the selected Xcode runtime binary remain unverified. The conservative license set also includes the pinned [`swift-asn1 LICENSE`](ThirdPartyLicenses/Upstream/swift-asn1/LICENSE.txt) and [`NOTICE`](ThirdPartyLicenses/Upstream/swift-asn1/NOTICE.txt), plus the pinned [`swift-syntax LICENSE`](ThirdPartyLicenses/Upstream/swift-syntax/LICENSE.txt); those files preserve resolved/build-time license texts and do not imply that SwiftASN1 or SwiftSyntax runtime code ships in the sidecar. `fmt/doc/python-license.txt` is a documentation-tool license, not evidence of an additional linked runtime component.

This section documents the packaged MLX sidecar, the selected Swift compatibility runtime bytes, and the conservative Swift license reference described above; it does not claim an exact license mapping for that runtime. It also does not claim that the broader Rust or JavaScript dependency redistribution-license review is complete. The inspected sidecar stage contains no model-weight files.

## License texts for shared interface dependencies

### aria-hidden, react-remove-scroll, react-style-singleton, use-callback-ref, use-sidecar

```text
MIT License

Copyright (c) 2017 Anton Korzunov

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### @radix-ui/number, @radix-ui/primitive, @radix-ui/react-arrow, @radix-ui/react-collection, @radix-ui/react-compose-refs, @radix-ui/react-context, @radix-ui/react-direction, @radix-ui/react-dismissable-layer, @radix-ui/react-focus-guards, @radix-ui/react-focus-scope, @radix-ui/react-id, @radix-ui/react-popover, @radix-ui/react-popper, @radix-ui/react-portal, @radix-ui/react-presence, @radix-ui/react-primitive, @radix-ui/react-select, @radix-ui/react-slot, @radix-ui/react-use-callback-ref, @radix-ui/react-use-controllable-state, @radix-ui/react-use-effect-event, @radix-ui/react-use-layout-effect, @radix-ui/react-use-previous, @radix-ui/react-use-rect, @radix-ui/react-use-size, @radix-ui/react-visually-hidden, @radix-ui/rect

```text
MIT License

Copyright (c) 2022 WorkOS

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### detect-node-es

```text
MIT License

Copyright (c) 2017 Ilya Kantor

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### core, dom, react-dom, utils

```text
MIT License

Copyright (c) 2021-present Floating UI contributors

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

### get-nonce

```text
MIT License

Copyright (c) 2020 Anton Korzunov

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### tslib

```text
Copyright (c) Microsoft Corporation.

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY
AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM
LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR
OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR
PERFORMANCE OF THIS SOFTWARE.
```

## Unicode character tables

VoiceFlow copies Unicode 16.0.0 XID_Start, XID_Continue, and General_Category=Mark ranges in `src-tauri/src/protected_span.rs` from the lockfile-cached `regex-syntax` 0.8.11 generated Unicode tables (`src/unicode_tables/property_bool.rs` and `src/unicode_tables/general_category.rs`; generated with `ucd-generate` 0.3.1). The following is the original `regex-syntax` 0.8.11 `src/unicode_tables/LICENSE-UNICODE` notice, retained verbatim.

```text
UNICODE, INC. LICENSE AGREEMENT - DATA FILES AND SOFTWARE

Unicode Data Files include all data files under the directories
http://www.unicode.org/Public/, http://www.unicode.org/reports/,
http://www.unicode.org/cldr/data/, http://source.icu-project.org/repos/icu/, and
http://www.unicode.org/utility/trac/browser/.

Unicode Data Files do not include PDF online code charts under the
directory http://www.unicode.org/Public/.

Software includes any source code published in the Unicode Standard
or under the directories
http://www.unicode.org/Public/, http://www.unicode.org/reports/,
http://www.unicode.org/cldr/data/, http://source.icu-project.org/repos/icu/, and
http://www.unicode.org/utility/trac/browser/.

NOTICE TO USER: Carefully read the following legal agreement.
BY DOWNLOADING, INSTALLING, COPYING OR OTHERWISE USING UNICODE INC.'S
DATA FILES ("DATA FILES"), AND/OR SOFTWARE ("SOFTWARE"),
YOU UNEQUIVOCALLY ACCEPT, AND AGREE TO BE BOUND BY, ALL OF THE
TERMS AND CONDITIONS OF THIS AGREEMENT.
IF YOU DO NOT AGREE, DO NOT DOWNLOAD, INSTALL, COPY, DISTRIBUTE OR USE
THE DATA FILES OR SOFTWARE.

COPYRIGHT AND PERMISSION NOTICE

Copyright © 1991-2018 Unicode, Inc. All rights reserved.
Distributed under the Terms of Use in http://www.unicode.org/copyright.html.

Permission is hereby granted, free of charge, to any person obtaining
a copy of the Unicode data files and any associated documentation
(the "Data Files") or Unicode software and any associated documentation
(the "Software") to deal in the Data Files or Software
without restriction, including without limitation the rights to use,
copy, modify, merge, publish, distribute, and/or sell copies of
the Data Files or Software, and to permit persons to whom the Data Files
or Software are furnished to do so, provided that either
(a) this copyright and permission notice appear with all copies
of the Data Files or Software, or
(b) this copyright and permission notice appear in associated
Documentation.

THE DATA FILES AND SOFTWARE ARE PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT OF THIRD PARTY RIGHTS.
IN NO EVENT SHALL THE COPYRIGHT HOLDER OR HOLDERS INCLUDED IN THIS
NOTICE BE LIABLE FOR ANY CLAIM, OR ANY SPECIAL INDIRECT OR CONSEQUENTIAL
DAMAGES, OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE,
DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR
PERFORMANCE OF THE DATA FILES OR SOFTWARE.

Except as contained in this notice, the name of a copyright holder
shall not be used in advertising or otherwise to promote the sale,
use or other dealings in these Data Files or Software without prior
written authorization of the copyright holder.

```

## Fixed local model notices

The packaged software notices cover four fixed, user-selected local model artifacts documented under [`ThirdPartyLicenses/Models/`](ThirdPartyLicenses/Models/source-manifest.json). The current reviewed App inventory contains no model weights. The original Qwen3-ASR 0.6B and Qwen3.8 cleanup entries retain their existing, bounded source/license evidence. The two additional converted ASR entries below have metadata-only notices and remain **UNREVIEWED / 许可未审查**; the original review evidence does not cover them.

- `mlx-community/Qwen3-ASR-0.6B-bf16` declares Apache-2.0 in its converter and observed base cards. The ASR model repository had no full LICENSE/NOTICE listed in the reviewed snapshots; the base LICENSE request returned 404. The packaged Apache file is canonical terms; the Qwen3-ASR code license is explicitly a code reference only. See [`Qwen3-ASR-0.6B/NOTICE.md`](ThirdPartyLicenses/Models/Qwen3-ASR-0.6B/NOTICE.md).
- `lmstudio-community/Qwen3.8-27B-MLX-6bit` declares Apache-2.0. Its exact base model name is Qwen3.8-27B; the full license from the reviewed official base snapshot is included. See [`Qwen3.8-27B/NOTICE.md`](ThirdPartyLicenses/Models/Qwen3.8-27B/NOTICE.md).
- `mlx-community/Qwen3-ASR-1.7B-bf16` is pinned to converted repository revision `e1f6c266914abc5a46e8756e02580f834a6cf8a7`; the upstream Qwen base snapshot is recorded separately. The upstream publisher declares Apache-2.0, but the converter input commit, converter license, converted artifact license, and artifact LICENSE/NOTICE are unknown. See [`Qwen3-ASR-1.7B/NOTICE.md`](ThirdPartyLicenses/Models/Qwen3-ASR-1.7B/NOTICE.md).
- `beshkenadze/cohere-transcribe-03-2026-mlx-fp16` is pinned to converted repository revision `06bbe0d853a93cbbf7c1f40cffccd4c5481459ef`; the upstream Cohere base snapshot is recorded separately. The upstream publisher declares Apache-2.0, but the converter input commit, converter license, converted artifact license, and artifact LICENSE/NOTICE are unknown. The upstream model files are gated; no gated files or terms were accessed or accepted. See [`Cohere-Transcribe-2B/NOTICE.md`](ThirdPartyLicenses/Models/Cohere-Transcribe-2B/NOTICE.md).

These notices record source attribution and review boundaries, not legal certification or permission to download, bundle, host, or redistribute model weights. VoiceFlow does not currently materialize these notices beside managed model downloads/imports.

## Handy-derived onboarding components

VoiceFlow adapts portions of Handy's onboarding `ModelCard`, `PermissionGuide`, and `MicrophoneSelector` at upstream commit [`14f6f0d31cb22a4268acfbddba3052820fd0a06e`](https://github.com/cjpais/Handy/commit/14f6f0d31cb22a4268acfbddba3052820fd0a06e). The exact MIT notice and source-hash bindings are in [`ThirdPartyLicenses/Handy/`](ThirdPartyLicenses/Handy/source-manifest.json).

## Locked Rust and frontend dependency texts

VoiceFlow includes a lock-bound attribution inventory for 407 Cargo metadata packages (388 normal dependency graph nodes and 19 build-only nodes) and 58 non-development root npm lock entries. These are lock/metadata populations, not a claim that every record contributes linked runtime code. Package-distributed license/notice text candidates and explicitly identified same-repository, same-version peer texts are included under [`ThirdPartyLicenses/DependencyTexts/`](ThirdPartyLicenses/DependencyTexts/DEPENDENCY_LICENSES.md); package name, version, source class, declared expression, lock identity, per-text source-relative path, byte count, and SHA-256 are in [`license-index.json`](ThirdPartyLicenses/DependencyTexts/license-index.json).

The inventory preserves declared expressions when text is absent and lists 6 third-party records without a full license body. It separately records 1 first-party workspace package(s) without a declared project license. A missing text is not treated as permission, and the inventory is not legal clearance. It does not include model weights or certify exact runtime linkage. Existing Swift/MLX and other source notices are preserved.

## MIT terms reference and package-author attribution overlay

The accompanying `ThirdPartyLicenses/DependencyTexts/AttributionOverlay/standard/MIT.txt` is the official SPDX MIT standard text, retained byte-for-byte as a canonical terms reference. Its copyright line remains the literal `Copyright (c) <year> <copyright holders>`. This standard reference is not the package-distributed license body for any of the locked packages below and does not supply a package-specific copyright notice.

The exact locked package archives and lock identities were checked in the cited source audit. The packages declare MIT and provide the following author metadata:

| Locked package | Declared license | Author metadata as declared |
| --- | --- | --- |
| `react-remove-scroll-bar@2.3.8` | MIT | Anton Korzunov <thekashey@gmail.com> |
| `realfft@3.5.0` | MIT | HEnquist <henrik.enquist@gmail.com> |
| `block@0.1.6` | MIT | Steven Sheldon |
| `malloc_buf@0.0.6` | MIT | Steven Sheldon |
| `objc-foundation@0.1.1` | MIT | Steven Sheldon |
| `objc_id@0.1.1` | MIT | Steven Sheldon |

These names come from package metadata fields (`author`/`authors`). They are not substituted for copyright owners. The exact-package audit found no full MIT grant and no copyright notice in any of these six locked archives. All six remain `unresolved_no_full_text_candidate` in the current license index: `react-remove-scroll-bar@2.3.8`, `realfft@3.5.0`, `block@0.1.6`, `malloc_buf@0.0.6`, `objc-foundation@0.1.1`, and `objc_id@0.1.1`. The canonical reference does not close those gaps.

### Same-project supplemental notices

The overlay carries three separate upstream notice snapshots for attribution context. They are supplemental project notices and are not represented as text distributed in the locked releases:

- `react-remove-scroll-bar@2.3.8`: the MIT notice is from same-project revision `8ca9ba5ea52de03308fe8ced94f7b159a44d28ff`, whose package metadata identifies version `2.3.7`. The lock is `2.3.8`; exact-release applicability is unverified.
- `malloc_buf@0.0.6`: the MIT notice is from later same-project revision `d9a3e539642bd90e07df458d226b19cdfa606863`. The recorded history identifies a later license addition; the `0.0.6` release-tag tree has no license file.
- `realfft@3.5.0`: the MIT notice is from upstream's missing-license repair at `9fdcf59c60c7ecacbf75774d378a068811a2e6ea`, which bumps the package to 3.5.1. That commit changes Cargo.toml version, LICENSE, and README only; Rust source is unchanged from the locked 3.5.0 revision. The authentic notice is useful supplemental attribution evidence, not the historical 3.5.0 archive body. The exact 3.5.0 license-index status remains unresolved.

The react and malloc notices do not apply to RealFFT; none of the three notices is evidence for `block`, `objc-foundation`, or `objc_id`. The accompanying source bindings identify the saved bytes, lock identities, source revisions, and archive checks. This attribution material is not legal clearance and does not claim the dependency-license inventory is complete.
