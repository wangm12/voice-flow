# Gemma 4 E4B IT text-only MLX 4-bit model notice

VoiceFlow model ID: `gemma_4_e4b_it_mlx_4bit`.

- Download source: [`Anotos/Gemma-4-E4B-it-Text-MLX-4bit`](https://huggingface.co/Anotos/Gemma-4-E4B-it-Text-MLX-4bit/tree/da76496efe4486235e6e0aa9315203e27b957bcd), revision `da76496efe4486235e6e0aa9315203e27b957bcd`.
- The pinned text-only `model.safetensors` is 4,198,925,844 bytes; SHA-256 `16900c33eaabdb06be5378c4261aa73b09566d4c8b668382e2f4c86bb53759cd`. The exact required-file inventory is in [`source-manifest.json`](../source-manifest.json); manifest SHA-256: `f29888227b0d076a257463fb3f414c0916b298145a283bc4352bf6bc7a54e55e`.
- The Google base snapshot [`google/gemma-4-E4B-it`](https://huggingface.co/google/gemma-4-E4B-it/tree/fee6332c1abaafb77f6f9624236c63aa2f1d0187) declares Apache-2.0 in metadata and has no `LICENSE` sibling. Anotos also declares Apache-2.0 and includes the exact [`Anotos-LICENSE.txt`](Anotos-LICENSE.txt). Its README credits Google DeepMind for the model, mlx-community for the 4-bit conversion, and Axly’s Customs for the text-only repackaging for Erato.
- The Anotos README identifies the immediate input as `mlx-community/gemma-4-e4b-it-4bit` at `475b9088d29754a3379866cf5aeb6b41acd313c2`. That intermediate snapshot declares `license: gemma` and contains no license file. The Anotos README says its text-only repack removed audio/vision tensors and copied the remaining language-model tensors byte for byte. The differing intermediate license metadata remains a provenance/release gate; this notice does not resolve it.

The model weights are not bundled. This pin is a development candidate only: it is not release-qualified or available for download or folder import in release builds.
