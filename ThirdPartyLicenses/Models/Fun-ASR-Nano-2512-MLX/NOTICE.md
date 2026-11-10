# Model and runtime attribution — Fun-ASR-Nano-2512 MLX

VoiceFlow app identifier: `fun-asr-nano-2512-mlx`.

The downloaded model artifact is pinned to `mlx-community/Fun-ASR-Nano-2512` revision `a7bc96fceaafce39ed6748e0c0fa9a9508b67f86`. The pinned MLX repository and the original `FunAudioLLM/Fun-ASR-Nano-2512` Hugging Face model card declare Apache-2.0. The pinned artifact metadata does not identify the exact conversion input commit; this notice records the published declarations and does not establish independent license clearance for conversion or redistribution.

The Swift runtime implementation is adapted from `Blaizzy/mlx-audio-swift` commit `262a391a6a63c2e6923e826bcc7122caee0f18f6` (“Add Fun-ASR-Nano STT model (#300)”), authored under the GitHub handle `lucasnewman`, under MIT. The source files are `FunASRNanoAudio.swift`, `FunASRNanoConfig.swift`, and `FunASRNanoModel.swift`. VoiceFlow's compatibility changes add the `MLXLMCommon` import and qualify `Tokenizers.Tokenizer`; source and post-edit hashes are recorded in the sidecar overlay manifest. The upstream MIT license is the pinned `mlx-audio-swift` package license already included with the sidecar.

The sidecar route was compile-verified. No model weights were downloaded or loaded, and VoiceFlow has not measured accuracy, latency, or memory use. The route accepts automatic, Chinese, or English prompt language and does not return a detected-language result.

This notice records attribution and pin metadata; it is not legal clearance or permission for future weight redistribution. VoiceFlow does not bundle these model weights; users download them on demand.
