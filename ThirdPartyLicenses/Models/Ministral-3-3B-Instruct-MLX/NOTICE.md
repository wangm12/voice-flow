# Ministral 3 3B Instruct MLX 4-bit model notice

VoiceFlow model ID: `ministral_3_3b_instruct_mlx_4bit`.

## Current development candidate

- Download source: [`mlx-community/Ministral-3-3B-Instruct-2512-4bit`](https://huggingface.co/mlx-community/Ministral-3-3B-Instruct-2512-4bit/tree/a962dcb09eee4169c890e544c9eb938f1113fdee), revision `a962dcb09eee4169c890e544c9eb938f1113fdee`.
- The pinned `model.safetensors` is 2,745,210,934 bytes; SHA-256 `69a68c0a961a64bb1145a697a83cf6cec45220d425c77a6c0a3568924078c3b4`. The required-file inventory is in [`source-manifest.json`](../source-manifest.json); manifest SHA-256: `9c190f28458ff6cd6e1d17fe90913d445cbe745728803364e948cb1e87c16ab2`.
- The pinned model card declares Apache-2.0. Its README says this MLX artifact was converted from [`mistralai/Ministral-3-3B-Instruct-2512`](https://huggingface.co/mistralai/Ministral-3-3B-Instruct-2512) with `mlx-vlm` 0.3.9; the conversion input revision is not identified. The Mistral base card also declares Apache-2.0. The shared canonical terms reference is [`Apache-2.0-canonical-reference.txt`](../Gemma-4-E4B-it/Apache-2.0-canonical-reference.txt).
- The artifact is a multimodal container, not a text-only conversion. Its pinned safetensors header has 602 `language_model.model.*` tensors, 218 `vision_tower.*` tensors, and 10 `multi_modal_projector.*` tensors. The pinned `mlx-swift-lm` 3.31.4 text cleanup smoke loaded the model and returned the expected result through EOS. This establishes a runtime compatibility smoke only; memory, quality, and product acceptance remain pending. The candidate is not release-qualified and is unavailable to release downloads or folder import.

## Historical failed candidate

- The earlier [`Oscilla/Ministral-3-3B-Instruct-2512-mlx-4Bit`](https://huggingface.co/Oscilla/Ministral-3-3B-Instruct-2512-mlx-4Bit/tree/10889ee5bad71785905c2fb444439e81823cdd7a) snapshot at revision `10889ee5bad71785905c2fb444439e81823cdd7a` remains recorded in [`source-manifest.json`](../source-manifest.json). Its 1,929,127,299-byte weight file had SHA-256 `36617994740c1ace583a4bd1ced2fad1662f3900a4ded555e827517229f771e2`; its pinned Swift run repeated `ipi` until the generation cap. It is excluded from the development catalog.

No model weights are bundled. These pinned records do not provide blanket legal clearance for future weight redistribution.
