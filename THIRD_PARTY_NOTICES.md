# Third-party notices

## Design references

- [OpenTypeless](https://github.com/tover0314-w/opentypeless) — MIT. VoiceFlow's context/profile/policy behavior is independently mapped to its own Rust types; no source file is vendored here.
- [OpenWhispr](https://github.com/OpenWhispr/openwhispr) — MIT. VoiceFlow references its recording and hotkey interaction patterns; it does not copy the Electron UI.

## Application frameworks

[Tauri](https://github.com/tauri-apps/tauri), its [global-shortcut](https://github.com/tauri-apps/tauri-plugin-global-shortcut) and [clipboard-manager](https://github.com/tauri-apps/tauri-plugin-clipboard-manager) plugins, and the JavaScript API are application dependencies. The Rust crate versions are pinned in `src-tauri/Cargo.lock`; the JavaScript API version is pinned in `package-lock.json`. Those lockfiles do not contain license texts.

## Shared interface primitives

`@radix-ui/react-select`, `@radix-ui/react-popover`, and their Radix, Floating UI, and React focus/scroll support dependencies are MIT-licensed interface dependencies, pinned in `package-lock.json`. VoiceFlow styles its own shared controls from the user's Codex screenshots; no Codex source code is included.

## Packaged Apple-silicon MLX sidecar

The macOS Release sidecar links the Swift package objects below into `voiceflow-mlx-sidecar`. Packaging copies the sidecar executable, `mlx-swift_Cmlx.bundle`, `swift-crypto_Crypto.bundle`, `swift-transformers_Hub.bundle`, and the MLX Metal shader into app resources. The component list below was checked against the pinned `src-tauri/mlx-sidecar/Package.resolved`, the Release executable's link-object list, the staged resources, and each pinned checkout's `LICENSE` or `NOTICE` file. License links point to the pinned source revision.

MIT-licensed components:

- `mlx-audio-swift` — MLX audio STT, core, codec, and VAD code; revision `01dec7c9bdce3088a6b6b7ab9f2e403458195efb`. [Pinned LICENSE](https://github.com/Blaizzy/mlx-audio-swift/blob/01dec7c9bdce3088a6b6b7ab9f2e403458195efb/LICENSE) (Copyright © 2025 Prince Canuma).
- `mlx-swift` — MLX, Cmlx, MLXNN, MLXFast, and MLXOptimizers code and the Cmlx resource bundle; version `0.31.4`, revision `dc43e62d7055353c7f99fa071a4e71d29dfddc44`. [Pinned LICENSE](https://github.com/ml-explore/mlx-swift/blob/dc43e62d7055353c7f99fa071a4e71d29dfddc44/LICENSE) (Copyright © 2023 ml-explore).
- `mlx-swift-lm` — MLX language-model support code; version `3.31.4`, revision `bd4b7434e6bdb588c7ef55706ff8904cb7fd4c57`. [Pinned LICENSE](https://github.com/ml-explore/mlx-swift-lm/blob/bd4b7434e6bdb588c7ef55706ff8904cb7fd4c57/LICENSE) (Copyright © 2024 ml-explore).
- `EventSource` — server-sent-events code; version `1.5.1`, revision `86b5096ac59ab46e66bd1f6377c604bc1dab0bc2`. [Pinned LICENSE](https://github.com/mattt/EventSource/blob/86b5096ac59ab46e66bd1f6377c604bc1dab0bc2/LICENSE.md) (Copyright © 2025 Mattt).
- `yyjson` — JSON code; version `0.12.0`, revision `8b4a38dc994a110abaec8a400615567bd996105f`. [Pinned LICENSE](https://github.com/ibireme/yyjson/blob/8b4a38dc994a110abaec8a400615567bd996105f/LICENSE) (Copyright © 2020 YaoYuan).

Apache-2.0-licensed components:

- `swift-numerics` — Numerics, RealModule, ComplexModule, and shims; version `1.1.1`, revision `0c0290ff6b24942dadb83a929ffaaa1481df04a2`. [Pinned LICENSE](https://github.com/apple/swift-numerics/blob/0c0290ff6b24942dadb83a929ffaaa1481df04a2/LICENSE.txt).
- `swift-huggingface` — HuggingFace support; version `0.11.0`, revision `f2f99991f2d7d8fdb3187e4fd539cd2facf5c13d`. [Pinned LICENSE](https://github.com/huggingface/swift-huggingface/blob/f2f99991f2d7d8fdb3187e4fd539cd2facf5c13d/LICENSE).
- `swift-crypto` — Crypto code and its staged privacy-manifest resource; version `4.5.2`, revision `da9d28d69ebe3894b18376c8f2395c2f37b8448f`. [Pinned LICENSE](https://github.com/apple/swift-crypto/blob/da9d28d69ebe3894b18376c8f2395c2f37b8448f/LICENSE.txt) and [pinned NOTICE](https://github.com/apple/swift-crypto/blob/da9d28d69ebe3894b18376c8f2395c2f37b8448f/NOTICE.txt) (Copyright © 2019 The SwiftCrypto Project). The upstream NOTICE records SwiftNIO-derived source attribution and test-vector attribution; the sidecar link/resource inventory contains the `Crypto` target and no Crypto test-target resources. Its macOS package target does not link the BoringSSL targets, `CryptoExtras`, or SwiftASN1.
- `swift-transformers` — Tokenizers, Generation, Models, Hub, and the staged Hub resource bundle; version `1.3.4`, revision `c21fdcde390313a6d98d8e33a346f2c3486c3ab0`. [Pinned LICENSE](https://github.com/huggingface/swift-transformers/blob/c21fdcde390313a6d98d8e33a346f2c3486c3ab0/LICENSE).
- `swift-jinja` — Jinja code; version `2.5.1`, revision `4588064a20f3fc093c95f2f7d3359999bf30cae5`. [Pinned LICENSE](https://github.com/huggingface/swift-jinja/blob/4588064a20f3fc093c95f2f7d3359999bf30cae5/LICENSE).
- `swift-collections` — OrderedCollections and InternalCollectionsUtilities; version `1.7.1`, revision `98ef3c98609a1e31b7e157b5b619579001a789d6`. [Pinned LICENSE](https://github.com/apple/swift-collections/blob/98ef3c98609a1e31b7e157b5b619579001a789d6/LICENSE.txt).

Other packages present only in the Swift package resolution are not listed as shipped sidecar components unless they have a linked object or staged resource. For example, `swift-asn1` and `swift-syntax` are pinned in `Package.resolved`, but neither appears in the inspected sidecar link-object or staged-resource inventory.

The inspected sidecar stage contains no model-weight files.

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
