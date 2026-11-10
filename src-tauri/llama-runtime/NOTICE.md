# llama.cpp runtime notice

VoiceFlow builds `llama-server` from the pinned llama.cpp source revision in
`BUILD-IDENTITY.json`. The helper is built for Apple Silicon macOS, with Metal
kernels embedded in the executable. Its linked native dependencies are limited
to macOS system frameworks and libraries; no user model weights are included.

The `ThirdPartyLicenses/` directory carries the full license and notice texts
for llama.cpp and the vendored code used by the server build. The bundle-level
`THIRD_PARTY_NOTICES.md` records the same source and redistribution boundary.
