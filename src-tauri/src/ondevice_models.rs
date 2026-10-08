//! Pinned manifests for the existing SenseVoice archive and local MLX models.

pub const SENSEVOICE_ID: &str = "sensevoice-small";
pub const SENSEVOICE_ARCHIVE_SHA256: &str =
    "7d1efa2138a65b0b488df37f8b89e3d91a60676e416f515b952358d83dfd347e";
pub const SENSEVOICE_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2";
pub const SENSEVOICE_ARCHIVE_BYTES: u64 = 163_002_883;
pub const SENSEVOICE_INNER_ONNX: &str =
    "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17/model.int8.onnx";
pub const SENSEVOICE_INNER_TOKENS: &str =
    "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17/tokens.txt";

pub const QWEN_ASR_06_ID: &str = "qwen3-asr-0.6b";
pub const QWEN_ASR_17_ID: &str = "qwen3-asr-1.7b";
pub const COHERE_TRANSCRIBE_ID: &str = "cohere-transcribe-2b";
/// First locally executable model offered to new On Device selections.
pub const DEFAULT_LOCAL_MODEL_ID: &str = QWEN_ASR_06_ID;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelFile {
    pub path: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MlxModelManifest {
    pub id: &'static str,
    pub label: &'static str,
    pub repository: &'static str,
    pub revision: &'static str,
    pub bytes: u64,
    pub files: &'static [ModelFile],
    pub language_policy: MlxLanguagePolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MlxLanguagePolicy {
    AutoOrExplicit,
    ExplicitChineseOrEnglish,
}

const QWEN_SHARED_FILES: &[ModelFile] = &[
    ModelFile {
        path: "chat_template.json",
        bytes: 1_161,
        sha256: "75a8cfca24f00de72d796fbfed6858fc9614ef3dabd8696684cc3bc03a9c58ff",
    },
    ModelFile {
        path: "generation_config.json",
        bytes: 142,
        sha256: "1da527824d81e07118facff437e03f2e24a23311e3bdeb2368973fe77e5f275c",
    },
    ModelFile {
        path: "merges.txt",
        bytes: 1_671_853,
        sha256: "8831e4f1a044471340f7c0a83d7bd71306a5b867e95fd870f74d0c5308a904d5",
    },
    ModelFile {
        path: "preprocessor_config.json",
        bytes: 330,
        sha256: "45e120a4eda2c20c5d7f2ea9354e63536bf35e27aa573fb7cdf78017b378770d",
    },
    ModelFile {
        path: "tokenizer_config.json",
        bytes: 12_487,
        sha256: "4942d005604266809309cabc9f4e9cb89ce855d59b14681fdc0e1cc62ea26c4c",
    },
    ModelFile {
        path: "vocab.json",
        bytes: 2_776_833,
        sha256: "ca10d7e9fb3ed18575dd1e277a2579c16d108e32f27439684afa0e10b1440910",
    },
];

const QWEN_06_FILES: &[ModelFile] = &[
    ModelFile {
        path: "config.json",
        bytes: 6_982,
        sha256: "dae1fbfa8b858aceefe5afcc7ecc9184e4a6045bc4a1bca8a41834724f490874",
    },
    ModelFile {
        path: "model.safetensors",
        bytes: 1_564_921_888,
        sha256: "a6e635fd9c8dfd5cdd7465db9bd8c947ab30737b90b83b6b09c304e836bb8a7f",
    },
    ModelFile {
        path: "model.safetensors.index.json",
        bytes: 44_231,
        sha256: "4722310af2af946f54385d2aed98485961f5af0c0a340390ccf3c95f475db6a4",
    },
];

const QWEN_17_FILES: &[ModelFile] = &[
    ModelFile {
        path: "config.json",
        bytes: 6_983,
        sha256: "62fe9a2ba2de536820229e126fdfb35f9910b9a337dc46d3737c3b1ac0a9caa9",
    },
    ModelFile {
        path: "model.safetensors",
        bytes: 4_076_186_653,
        sha256: "2f080a3b769ae469aeaaa2dcb9e13a94141e54c9e6d5a7aa63392e0dc5a51789",
    },
    ModelFile {
        path: "model.safetensors.index.json",
        bytes: 51_384,
        sha256: "a3ee6037ec82e95dc8577216ac84cf0b3c92cfefc099cf20b2296571f2fb3fc0",
    },
];

const COHERE_FILES: &[ModelFile] = &[
    ModelFile {
        path: "config.json",
        bytes: 3_998,
        sha256: "5de7e586cec6d8f51225c8d5fe17a56a3043dda9af8c42f9cb01dd545905eb18",
    },
    ModelFile {
        path: "conversion_summary.json",
        bytes: 5_237,
        sha256: "f3f763b9ff233b194df209277ab670d7768745f92eab0efb52b769991743b159",
    },
    ModelFile {
        path: "key_map.json",
        bytes: 179_652,
        sha256: "42cf585ab25335db650b353abcaa9d219d51a04ef04eae5869de6122e85a7be8",
    },
    ModelFile {
        path: "model.safetensors",
        bytes: 4_131_827_448,
        sha256: "1ec6ba9ee27da02b21b3ffdb5183b77020351d3331d05a74ad8d58a09394a2b8",
    },
    ModelFile {
        path: "preprocessor_config.json",
        bytes: 420,
        sha256: "9f297d330646ecc8ebb9dc5784f48b7c35b118c913e306a1ccd0192f2c976332",
    },
    ModelFile {
        path: "special_tokens_map.json",
        bytes: 4_091,
        sha256: "1814ce01458ff6a72b04a6618e75f18ce627be4dc17619cd3a7cd7f71e137f0f",
    },
    ModelFile {
        path: "tokenizer.model",
        bytes: 492_827,
        sha256: "6d21e6a83b2d0d3e1241a7817e4bef8eb63bcb7cfe4a2675af9a35ff3bbf0e14",
    },
    ModelFile {
        path: "tokenizer_config.json",
        bytes: 48_141,
        sha256: "0dfeb3eeba07bccaa1b4bf78f3135ad3059acf8d18f681675832b285ac0035b0",
    },
];

const QWEN_06_ALL: &[ModelFile] = &[
    QWEN_SHARED_FILES[0],
    QWEN_SHARED_FILES[1],
    QWEN_SHARED_FILES[2],
    QWEN_06_FILES[0],
    QWEN_06_FILES[1],
    QWEN_06_FILES[2],
    QWEN_SHARED_FILES[3],
    QWEN_SHARED_FILES[4],
    QWEN_SHARED_FILES[5],
];
const QWEN_17_ALL: &[ModelFile] = &[
    QWEN_SHARED_FILES[0],
    QWEN_SHARED_FILES[1],
    QWEN_SHARED_FILES[2],
    QWEN_17_FILES[0],
    QWEN_17_FILES[1],
    QWEN_17_FILES[2],
    QWEN_SHARED_FILES[3],
    QWEN_SHARED_FILES[4],
    QWEN_SHARED_FILES[5],
];

pub const MLX_MODELS: &[MlxModelManifest] = &[
    MlxModelManifest {
        id: QWEN_ASR_06_ID,
        label: "Qwen3-ASR 0.6B BF16",
        repository: "mlx-community/Qwen3-ASR-0.6B-bf16",
        revision: "eae2b51f96265328f1e7beced788adb0e4536f92",
        bytes: 1_569_435_907,
        files: QWEN_06_ALL,
        language_policy: MlxLanguagePolicy::AutoOrExplicit,
    },
    MlxModelManifest {
        id: QWEN_ASR_17_ID,
        label: "Qwen3-ASR 1.7B BF16",
        repository: "mlx-community/Qwen3-ASR-1.7B-bf16",
        revision: "e1f6c266914abc5a46e8756e02580f834a6cf8a7",
        bytes: 4_080_707_826,
        files: QWEN_17_ALL,
        language_policy: MlxLanguagePolicy::AutoOrExplicit,
    },
    MlxModelManifest {
        id: COHERE_TRANSCRIBE_ID,
        label: "Cohere Transcribe 2B FP16",
        repository: "beshkenadze/cohere-transcribe-03-2026-mlx-fp16",
        revision: "06bbe0d853a93cbbf7c1f40cffccd4c5481459ef",
        bytes: 4_132_561_814,
        files: COHERE_FILES,
        language_policy: MlxLanguagePolicy::ExplicitChineseOrEnglish,
    },
];

pub fn mlx_model(id: &str) -> Option<&'static MlxModelManifest> {
    MLX_MODELS.iter().find(|model| model.id == id)
}

pub fn is_mlx_model(id: &str) -> bool {
    mlx_model(id).is_some()
}

pub fn inner_file_name(inner: &str) -> &str {
    std::path::Path::new(inner)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_sha256_is_64_lowercase_hex() {
        assert_eq!(SENSEVOICE_ARCHIVE_SHA256.len(), 64);
        assert!(
            SENSEVOICE_ARCHIVE_SHA256
                .chars()
                .all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "sha256 must be lowercase hex, got {SENSEVOICE_ARCHIVE_SHA256}"
        );
    }

    #[test]
    fn catalog_url_bytes_and_id_are_pinned() {
        assert_eq!(SENSEVOICE_ID, "sensevoice-small");
        assert_eq!(
            SENSEVOICE_URL,
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2"
        );
        assert!(
            SENSEVOICE_URL.starts_with("https://") && !SENSEVOICE_URL.is_empty(),
            "url must be a non-empty https link"
        );
        assert!(
            SENSEVOICE_URL.ends_with(".tar.bz2"),
            "official archive must be .tar.bz2, not Handy .tar.gz"
        );
        assert!(!SENSEVOICE_URL.contains("handy.computer"));
        assert_eq!(SENSEVOICE_ARCHIVE_BYTES, 163_002_883);
        const { assert!(SENSEVOICE_ARCHIVE_BYTES > 0) };
        assert_eq!(
            SENSEVOICE_ARCHIVE_SHA256,
            "7d1efa2138a65b0b488df37f8b89e3d91a60676e416f515b952358d83dfd347e"
        );
    }

    #[test]
    fn catalog_inner_paths_are_ready_files() {
        assert!(
            SENSEVOICE_INNER_ONNX.ends_with("model.int8.onnx")
                || SENSEVOICE_INNER_ONNX.ends_with("model.onnx"),
            "inner onnx must be model.int8.onnx or model.onnx, got {SENSEVOICE_INNER_ONNX}"
        );
        assert!(
            SENSEVOICE_INNER_TOKENS.ends_with("tokens.txt"),
            "inner tokens must be tokens.txt, got {SENSEVOICE_INNER_TOKENS}"
        );
        assert!(!SENSEVOICE_INNER_ONNX.is_empty());
        assert!(!SENSEVOICE_INNER_TOKENS.is_empty());
    }
}
