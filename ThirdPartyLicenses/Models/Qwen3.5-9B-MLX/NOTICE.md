# Qwen3.5 9B MLX 4-bit model notice

VoiceFlow model ID: `qwen3_5_9b_mlx_4bit`.

- Download source: [`mlx-community/Qwen3.5-9B-MLX-4bit`](https://huggingface.co/mlx-community/Qwen3.5-9B-MLX-4bit/tree/938d8919941c6e7efd3c7150eff7fe9d12afa631), revision `938d8919941c6e7efd3c7150eff7fe9d12afa631`.
- The two pinned weight shards are 5,349,771,222 bytes (SHA-256 `a68b87558c6ef43f74c2bd63ce7e9092ceddc3101f3def0030774bae5f42aadd`) and 600,449,850 bytes (SHA-256 `b0a770bf8469c7f3f18756a0e0283f1c1174344a83e059a4e483f6af4907352d`). The exact required-file inventory is in [`source-manifest.json`](../source-manifest.json); manifest SHA-256: `9aa0cac83df1d4f1fb154eeecef2735051badd49cdeed925feacd0327d6db848`.
- The repository card declares Apache-2.0 and names `Qwen/Qwen3.5-9B` as its base. The observed base snapshot is `c202236235762e1c871ad0ccb60c8ee5ba337b9a`; its exact Apache license text is [`Qwen3.5-9B-base-LICENSE.txt`](Qwen3.5-9B-base-LICENSE.txt). The MLX card does not pin the conversion input commit.

The model weights are not bundled. This pin is a development candidate only: it is not release-qualified or available for download or folder import in release builds.
