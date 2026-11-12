# Third-party notices

## Design references

- [OpenTypeless](https://github.com/tover0314-w/opentypeless) — MIT. VoiceFlow's context/profile/policy behavior is independently mapped to its own Rust types; no source file is vendored here.
- [OpenWhispr](https://github.com/OpenWhispr/openwhispr) — MIT. VoiceFlow references its recording and hotkey interaction patterns; it does not copy the Electron UI.

## Vendored glass refraction

VoiceFlow adapts the one-surface WebGL2 rounded-box refraction shader and rendering setup in [`src/components/settings/hud-glasswork/GlassworkRenderer.ts`](src/components/settings/hud-glasswork/GlassworkRenderer.ts) from [Glasswork `refract.js`](https://github.com/kubai087/glasswork/blob/4f0b6744b898b3a9b98fbd913ab24fdb83c53162/refract.js), upstream commit `4f0b6744b898b3a9b98fbd913ab24fdb83c53162` (Copyright © 2026 Kubai087). The locally authored preview scene and React lifecycle code are not upstream Glasswork code. No Glasswork wallpapers, external images, or runtime CDN resources are included.

### Glasswork MIT License

```text
MIT License

Copyright (c) 2026 Kubai087

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

## Application frameworks

[Tauri](https://github.com/tauri-apps/tauri), its [global-shortcut](https://github.com/tauri-apps/tauri-plugin-global-shortcut) and [clipboard-manager](https://github.com/tauri-apps/tauri-plugin-clipboard-manager) plugins, and the JavaScript API are application dependencies. The Rust crate versions are pinned in `src-tauri/Cargo.lock`; the JavaScript API version is pinned in `package-lock.json`. Those lockfiles do not contain license texts.

## Shared interface primitives

`@radix-ui/react-select`, `@radix-ui/react-popover`, and their Radix, Floating UI, and React focus/scroll support dependencies are MIT-licensed interface dependencies, pinned in `package-lock.json`. VoiceFlow styles its own shared controls from the user's Codex screenshots; no Codex source code is included.

## Packaged Apple-silicon MLX sidecar

For Apple-silicon macOS builds, the selected Xcode toolchain's `swift-stdlib-tool` scans the helper and copies its required Swift libraries beside the executable. The Xcode target keeps its macOS 14 deployment target and sets an `@executable_path` runpath for the bundled compatibility runtime, including `libswiftCompatibilitySpan.dylib`. The staged sidecar includes the helper, Xcode resource bundles, MLX Metal library, required Swift runtime libraries, full pinned upstream license/notice texts, and a generated dependency identity manifest. The existing Tauri stage-directory resource mapping carries these files into app resources. This packaging change is not an older-system runtime acceptance result.

The full upstream text files are in [`ThirdPartyLicenses/`](ThirdPartyLicenses/README.md). During packaging, every included `Upstream/` package license and notice is compared byte-for-byte with the matching pinned `SourcePackages/checkouts` file; missing or mismatched text fails the package step. The staged `ThirdPartyLicenses/DependencyManifest/` contains exact copies of both SwiftPM lockfiles, their SHA-256 values, and the nested MLX GitLink revisions. The local links below point to the included source copies; each bullet also identifies its pinned revision.

MIT-licensed components:

- `mlx-audio-swift` — MLX audio STT, core, codec, and VAD code; upstream revision `01dec7c9bdce3088a6b6b7ab9f2e403458195efb`. VoiceFlow applies the repository-owned `src-tauri/mlx-sidecar/sdk-overlay/voiceflow-mlx-audio-cancellation-v2.patch` to four pinned source files (DSP, Qwen3-ASR, Cohere Transcribe, and Parakeet); it also adds three Fun-ASR-Nano implementation files introduced by upstream contributor `lucasnewman` in revision `262a391a6a63c2e6923e826bcc7122caee0f18f6`, with narrowly recorded import and tokenizer-qualification compatibility changes. The hash-bound overlay manifest verifies the patch and every source file. The upstream MIT LICENSE is preserved unchanged and covers these integrated sources. [Local LICENSE](ThirdPartyLicenses/Upstream/mlx-audio-swift/LICENSE) (Copyright © 2025 Prince Canuma).
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

## Bundled llama.cpp runtime

VoiceFlow bundles the Apple silicon `llama-server` helper built from [`ggml-org/llama.cpp`](https://github.com/ggml-org/llama.cpp) release `v0.9.0`, annotated tag object `c4261a74a59c5a719e8f73038e28633b08f2bea4`, commit `cd959bae99ff4bed22bef2604f45241c09bbb8c8`. llama.cpp is licensed under MIT. The helper includes embedded Metal kernels, is built for arm64 with a macOS 13.3 minimum, and does not include model weights. Its exact build identity is packaged as `llama-runtime/BUILD-IDENTITY.json`; the runtime notice and license texts for llama.cpp and the vendored components are under `llama-runtime/NOTICE.md` and `llama-runtime/ThirdPartyLicenses/` in the app resources. Users do not need Ollama for VoiceFlow-managed GGUF cleanup.

## Fixed local model notices

The model source notices cover fifteen pinned local artifacts or candidate sources documented under [`ThirdPartyLicenses/Models/`](ThirdPartyLicenses/Models/source-manifest.json). The current reviewed App inventory contains no model weights. The original Qwen3-ASR 0.6B and Qwen3.8 cleanup entries retain their existing, bounded source/license evidence. The Qwen3-ASR 1.7B and Cohere Transcribe 2B converted ASR entries remain **UNREVIEWED / 许可未审查**; their existing review boundary is unchanged. The three GGUF cleanup downloads and three Qwen3.5 MLX manifests are separately pinned. The Gemma and current community Ministral MLX candidates are development-only and are not release-qualified; the Gemma E4B text-only conversion retains a license-lineage discrepancy. The earlier Oscilla Ministral pin remains a failed historical candidate; the current community pin passed a direct pinned-Swift text-submodel smoke through EOS, with memory, quality, and resource qualification pending. S1-mini by Superwhisper has a separate exact-license naming term and a test-only candidate descriptor; it remains nonselectable pending its constrained prompt, English language, control, and fallback end-to-end checks.

- `mlx-community/Qwen3-ASR-0.6B-bf16` declares Apache-2.0 in its converter and observed base cards. The ASR model repository had no full LICENSE/NOTICE listed in the reviewed snapshots; the base LICENSE request returned 404. The packaged Apache file is canonical terms; the Qwen3-ASR code license is explicitly a code reference only. See [`Qwen3-ASR-0.6B/NOTICE.md`](ThirdPartyLicenses/Models/Qwen3-ASR-0.6B/NOTICE.md).
- `lmstudio-community/Qwen3.8-27B-MLX-6bit` declares Apache-2.0. Its exact base model name is Qwen3.8-27B; the full license from the reviewed official base snapshot is included. See [`Qwen3.8-27B/NOTICE.md`](ThirdPartyLicenses/Models/Qwen3.8-27B/NOTICE.md).
- `mlx-community/Qwen3-ASR-1.7B-bf16` is pinned to converted repository revision `e1f6c266914abc5a46e8756e02580f834a6cf8a7`; the upstream Qwen base snapshot is recorded separately. The upstream publisher declares Apache-2.0, but the converter input commit, converter license, converted artifact license, and artifact LICENSE/NOTICE are unknown. See [`Qwen3-ASR-1.7B/NOTICE.md`](ThirdPartyLicenses/Models/Qwen3-ASR-1.7B/NOTICE.md).
- `beshkenadze/cohere-transcribe-03-2026-mlx-fp16` is pinned to converted repository revision `06bbe0d853a93cbbf7c1f40cffccd4c5481459ef`; the upstream Cohere base snapshot is recorded separately. The upstream publisher declares Apache-2.0, but the converter input commit, converter license, converted artifact license, and artifact LICENSE/NOTICE are unknown. The upstream model files are gated; no gated files or terms were accessed or accepted. See [`Cohere-Transcribe-2B/NOTICE.md`](ThirdPartyLicenses/Models/Cohere-Transcribe-2B/NOTICE.md).
- VoiceFlow cleanup model `qwen3.5-2b` downloads `Qwen3.5-2B-Q4_K_M.gguf` from [`lmstudio-community/Qwen3.5-2B-GGUF`](https://huggingface.co/lmstudio-community/Qwen3.5-2B-GGUF/tree/bb84e11355a036e28f080c7793fa6d22b7c4e344), revision `bb84e11355a036e28f080c7793fa6d22b7c4e344`; size 1,270,808,032 bytes; SHA-256 `0bfe35afc9f05b7fac3fa04925e051ac7939a42a8a17ea11afc99701bea826cc`. Its declared base is `Qwen/Qwen3.5-2B` at observed revision `15852e8c16360a2fea060d615a32b45270f8a8fc`. Both repository metadata records declare Apache-2.0; the exact base LICENSE is included. The GGUF card attributes quantization to the LM Studio team using llama.cpp `b8185` but does not pin the converter input commit. See [`Qwen3.5-2B/NOTICE.md`](ThirdPartyLicenses/Models/Qwen3.5-2B/NOTICE.md).
- VoiceFlow cleanup model `qwen3.5-4b` downloads `Qwen3.5-4B-Q4_K_M.gguf` from [`lmstudio-community/Qwen3.5-4B-GGUF`](https://huggingface.co/lmstudio-community/Qwen3.5-4B-GGUF/tree/f9f88ac3e234be915e23811a6d28ea287bdb927e), revision `f9f88ac3e234be915e23811a6d28ea287bdb927e`; size 2,707,513,696 bytes; SHA-256 `25082a7dd3776cc3c741c6347d3bd04523f05796607b3fbc32fa3a25dfa1418c`. Its declared base is `Qwen/Qwen3.5-4B` at observed revision `851bf6e806efd8d0a36b00ddf55e13ccb7b8cd0a`. Both repository metadata records declare Apache-2.0; the exact base LICENSE is included. The GGUF card attributes quantization to the LM Studio team using llama.cpp `b8185` but does not pin the converter input commit. See [`Qwen3.5-4B/NOTICE.md`](ThirdPartyLicenses/Models/Qwen3.5-4B/NOTICE.md).
- VoiceFlow cleanup model `gemma-4-e4b-it` downloads `gemma-4-E4B-it-Q4_0.gguf` from [`ggml-org/gemma-4-E4B-it-GGUF`](https://huggingface.co/ggml-org/gemma-4-E4B-it-GGUF/tree/27cd6836f8e2a6e5e6b57ea0e77fa72490706985), revision `27cd6836f8e2a6e5e6b57ea0e77fa72490706985`; size 4,590,807,392 bytes; SHA-256 `a555b900214b477d8880e7832e0b8925e139b0159640036b09fe472b6f2097f2`. Its declared base is `google/gemma-4-E4B-it` at observed revision `4ecc2ba63bcac23ba4a767fc87025b1450f84c9c`. Both repository metadata records declare Apache-2.0. The base snapshot has no LICENSE sibling, so the included Apache text is a canonical terms reference; the Gemma 4 license page is linked in [`Gemma-4-E4B-it/NOTICE.md`](ThirdPartyLicenses/Models/Gemma-4-E4B-it/NOTICE.md). The GGUF card identifies `ggml-org/convert` but does not give a converter commit.
- The native MLX development candidates `qwen3_5_2b_mlx_4bit`, `qwen3_5_4b_mlx_4bit`, and `qwen3_5_9b_mlx_4bit` are pinned to `mlx-community/Qwen3.5-{2B,4B,9B}-MLX-4bit` revisions `93760be4f1f69842a46bc13dbdc0f19e291392a3`, `32f3e8ecf65426fc3306969496342d504bfa13f3`, and `938d8919941c6e7efd3c7150eff7fe9d12afa631`. Each publisher card declares Apache-2.0 and names the corresponding Qwen3.5 base. The file manifests and base-license evidence are recorded in [`source-manifest.json`](ThirdPartyLicenses/Models/source-manifest.json); model-specific notices are in [`Qwen3.5-2B-MLX/NOTICE.md`](ThirdPartyLicenses/Models/Qwen3.5-2B-MLX/NOTICE.md), [`Qwen3.5-4B-MLX/NOTICE.md`](ThirdPartyLicenses/Models/Qwen3.5-4B-MLX/NOTICE.md), and [`Qwen3.5-9B-MLX/NOTICE.md`](ThirdPartyLicenses/Models/Qwen3.5-9B-MLX/NOTICE.md). These are development catalog entries, not release-qualified models.
- Native MLX candidate `gemma_4_e2b_it_mlx_4bit` is pinned to [`DreamFoundries/gemma-4-E2B-it-4bit`](https://huggingface.co/DreamFoundries/gemma-4-E2B-it-4bit/tree/75a479632d6a594ea323942b883cdbc1bdd97a14), revision `75a479632d6a594ea323942b883cdbc1bdd97a14`; `model.safetensors` is 2,623,906,147 bytes with SHA-256 `c5faac877578591aea5b16bd4b55b672f8d74a0e2cf72ed122ed19d265bc89d2`. Its card declares Apache-2.0 and identifies `google/gemma-4-E2B-it` as the base. See [`Gemma-4-E2B-it-MLX/NOTICE.md`](ThirdPartyLicenses/Models/Gemma-4-E2B-it-MLX/NOTICE.md).
- Native MLX candidate `gemma_4_e4b_it_mlx_4bit` is pinned to [`Anotos/Gemma-4-E4B-it-Text-MLX-4bit`](https://huggingface.co/Anotos/Gemma-4-E4B-it-Text-MLX-4bit/tree/da76496efe4486235e6e0aa9315203e27b957bcd), revision `da76496efe4486235e6e0aa9315203e27b957bcd`; `model.safetensors` is 4,198,925,844 bytes with SHA-256 `16900c33eaabdb06be5378c4261aa73b09566d4c8b668382e2f4c86bb53759cd`. The Google base snapshot `fee6332c1abaafb77f6f9624236c63aa2f1d0187` and Anotos metadata declare Apache-2.0, and the exact Anotos LICENSE is included. However, Anotos identifies `mlx-community/gemma-4-e4b-it-4bit` revision `475b9088d29754a3379866cf5aeb6b41acd313c2` as its input; that intermediate snapshot is tagged `gemma` and has no LICENSE file. The discrepancy remains unresolved. See [`Gemma-4-E4B-it-MLX/NOTICE.md`](ThirdPartyLicenses/Models/Gemma-4-E4B-it-MLX/NOTICE.md).
- Pinned candidate `Oscilla/Ministral-3-3B-Instruct-2512-mlx-4Bit` revision `10889ee5bad71785905c2fb444439e81823cdd7a` declares Apache-2.0. In the observed native run it produced repeated `ipi` until the generation cap; it remains a failed historical pin. See [`Ministral-3-3B-Instruct-MLX/NOTICE.md`](ThirdPartyLicenses/Models/Ministral-3-3B-Instruct-MLX/NOTICE.md).
- Replacement candidate `mlx-community/Ministral-3-3B-Instruct-2512-4bit` revision `a962dcb09eee4169c890e544c9eb938f1113fdee` declares Apache-2.0 and its README reports `mlx-vlm` 0.3.9 conversion from the official Ministral 3 Instruct model without pinning the input commit. The pinned Swift text cleanup smoke returned the expected result and stopped at EOS. Its multimodal safetensors contains 602 language-model tensors, 218 vision tensors, and 10 projector tensors; it remains a development candidate pending memory, quality, and resource qualification. See [`Ministral-3-3B-Instruct-MLX/NOTICE.md`](ThirdPartyLicenses/Models/Ministral-3-3B-Instruct-MLX/NOTICE.md).
- S1-mini by Superwhisper’s pinned MLX conversion is [`mlx-community/S1-mini-MLX-4bit`](https://huggingface.co/mlx-community/S1-mini-MLX-4bit/tree/5cbd7aec3401144f88a331d385c40b65fd2548eb). Its exact LICENSE marks it `other` / `s1-mini-license`, identifies a Qwen3-0.6B derivative under Apache-2.0, and adds the original-name requirement **“S1-mini” by “Superwhisper”** with exact capitalization for any use, distribution, or integration. The conversion README does not pin the BF16 input revision. A separate test-only descriptor remains nonselectable until the constrained prompt, English language, control, and fallback end-to-end checks pass. See [`S1-mini-MLX/NOTICE.md`](ThirdPartyLicenses/Models/S1-mini-MLX/NOTICE.md).

These notices record source attribution and review boundaries, not legal certification or permission to download, bundle, host, or redistribute model weights. The app does not bundle model weights, and notices are not currently materialized beside managed weight files. In development builds, catalog entries may be visible for managed download and local selection; release builds expose only models that pass their independent release qualification.

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
