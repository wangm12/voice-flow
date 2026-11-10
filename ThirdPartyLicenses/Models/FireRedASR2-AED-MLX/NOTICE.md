# Model attribution — FireRedASR2-AED MLX conversion

VoiceFlow app identifier: `firered-asr2-aed-mlx`.

The downloaded artifact is pinned to `mlx-community/FireRedASR2-AED-mlx` revision `f3212eacfa49b851130b97c63653c8e06ee09bdb`. The pinned converter repository contains the five runtime files but no README, LICENSE, or NOTICE. Hugging Face metadata identifies the commit uploader as `mm65x`; the repository does not state the exact upstream input commit or include converter source or license text.

The upstream model is `FireRedTeam/FireRedASR2-AED` at revision `32150b6cebfdf60a23b1bb8019cf759824563f8a`. Its model card declares Apache-2.0 and describes FireRedASR2 as supporting Mandarin, Chinese dialects and accents, English, code-switching, and singing recognition. That upstream license declaration does not establish the license of the community-converted MLX artifact or its converter, so those remain unreviewed here.

The Swift runtime does not pass a language control to this decoder and does not report a detected language. VoiceFlow has not measured accuracy, latency, or memory use in this change; upstream results are not VoiceFlow results.

This notice records upstream attribution and the missing converter evidence. It is not a legal clearance. VoiceFlow does not bundle the model weights; users download them on demand.
