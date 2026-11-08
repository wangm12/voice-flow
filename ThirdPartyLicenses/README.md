# MLX sidecar license and identity files

This directory supplies the license and notice texts staged with the native MLX sidecar. `Upstream/` preserves the package checkout-relative paths. Before replacing the stage, `package.sh` compares every copied `Upstream/` text byte-for-byte with the corresponding file under the pinned Xcode `SourcePackages/checkouts` tree; a missing or changed text stops packaging.

The set contains all 20 license/notice files recorded in the current pinned source inventory, including the nested Cmlx `fmt`, JSON, `metal-cpp`, MLX, and MLX-C texts, plus a separate copy of the Swift Apache 2.0 license with its Runtime Library Exception at `SwiftRuntime/LICENSE.txt`. That complete text is copied byte-for-byte from the pinned `swift-syntax/LICENSE.txt` file as a conservative reference. The exact source release and license mapping for the selected Xcode compatibility runtime remains unverified; SwiftSyntax is a build-time dependency, and its presence here does not mean SwiftSyntax code ships in the helper.

After a package build, `DependencyManifest/` contains byte-exact copies of both SwiftPM `Package.resolved` files, a generated `dependency-identities.txt` with their SHA-256 values and the MLX nested GitLink revisions, and a hash listing of the staged `Models/`, `Upstream/`, and `SwiftRuntime/` files. This records the inputs used for the sidecar package.

This inventory lists the MLX sidecar license texts and includes a conservative Swift license reference; the exact license mapping for the selected Xcode runtime remains unverified. It does not claim that the broader Rust or JavaScript redistribution-license review is complete.

## Fixed model notices

`Models/` contains notices and source-manifest hashes for four fixed local model artifacts. The original two entries retain their existing, bounded source/license evidence: the Qwen ASR code LICENSE is a code-only reference, and the cleanup license is copied from the exact observed Qwen3.8-27B base snapshot. The added Qwen3-ASR 1.7B and Cohere Transcribe 2B converted artifacts have metadata-only notices and remain UNREVIEWED / 许可未审查; their converter input commits, converter licenses, artifact licenses, and artifact LICENSE/NOTICE are unknown. The original review evidence does not cover these additions. No model weights are bundled, and these notice files do not clear future weight redistribution.

## Handy component notice

`Handy/` carries the byte-exact MIT notice for the three adapted onboarding components, with the upstream commit and source-file hashes in `Handy/source-manifest.json`.

## Locked Rust and frontend dependency texts

`DependencyTexts/` holds the lock-bound package/version/source/license index, a human-readable package table, and deduplicated copies of discovered license and notice files. SPDX tag-value metadata and short license pointers are labeled separately; they are not counted as full license bodies. The current inventory has 6 third-party lock records without a full license text and 1 first-party workspace package(s) without a declared project license. It is an attribution inventory, not legal clearance or proof that each lock entry is linked into the binary. Model weights are outside this inventory.

The existing `package.sh` stage copy carries this directory into the staged resources. Its byte-exact checkout verification remains scoped to the pinned Swift/MLX `Upstream/` texts and the compatibility-runtime reference; the package hash listing also records `Models/` and `SwiftRuntime/`. The notarized macOS release wrapper checks the packaged SwiftPM lock copies and their identity SHA declarations, the root `THIRD_PARTY_NOTICES.md`, and complete packaged `Models/` and `Upstream/` file inventories against repository sources before submission and against the final mounted App. It also runs no-model ASR and cleanup hello handshakes; those checks do not load or evaluate models. Package text provenance and SHA-256 values are recorded in `DependencyTexts/license-index.json`.

The separately labeled `DependencyTexts/AttributionOverlay/` contains an official SPDX MIT terms reference, package-author metadata for six MIT-declared lock records, and two same-project supplemental notices. These materials are not exact-release package license bodies; all six full-body statuses remain unresolved. Author fields are metadata credits, not copyright notices. See [`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md) for the attribution table and version caveats.
