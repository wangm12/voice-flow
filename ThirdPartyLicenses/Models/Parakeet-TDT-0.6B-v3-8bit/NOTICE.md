# Model attribution — NVIDIA Parakeet TDT 0.6B v3, MLX 8-bit

VoiceFlow app identifier: `parakeet-tdt-0.6b-v3-8bit`.

The downloaded artifact is pinned to `transcriber-app/parakeet-tdt-0.6b-v3-mlx-8bit` revision `b56210e2fad422736810524963c24b5e2a1985ab`. Its model card identifies NVIDIA's `nvidia/parakeet-tdt-0.6b-v3` as the base model, and describes the MLX checkpoint `mlx-community/parakeet-tdt-0.6b-v3` at revision `ed2b7e8c15f9aaa0b5772e2efb986255eaef7e15` as the source. It says the published weights quantize Linear and Embedding modules to 8-bit with group size 64, keep other tensors in bfloat16, and add a quantization block to `config.json`.

The pinned model card declares CC-BY-4.0 and credits NVIDIA for the model, mlx-community (via `parakeet-mlx`) for the MLX conversion, and this repository for quantization. The model card lists 25 European languages. VoiceFlow has not measured this model's accuracy, latency, or memory use in this change; those upstream details are not VoiceFlow results.

This notice records attribution and source metadata. It is not the full CC-BY-4.0 legal text or a legal clearance. VoiceFlow does not bundle the model weights; users download them on demand.
