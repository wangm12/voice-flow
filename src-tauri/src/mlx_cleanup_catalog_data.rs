//! Pinned native MLX cleanup artifacts and their required metadata.
//!
//! User download availability is curated separately from release qualification;
//! only models marked `importable` accept folder imports.

use crate::modelstore;

pub(super) const FILES_GEMMA_4_E2B_IT_MLX_4BIT: &[modelstore::ModelFile] = &[
    modelstore::ModelFile {
        path: "chat_template.jinja",
        bytes: 17_336,
        sha256: "2f1b4d75d067bae3fe44e676721c7f077d243bc007156cb9c2f8b5836613d082",
    },
    modelstore::ModelFile {
        path: "config.json",
        bytes: 4_696,
        sha256: "07839b8d16a85479b09c93ebf99aa89c1260004bc97028f74745939cd5ab4d67",
    },
    modelstore::ModelFile {
        path: "generation_config.json",
        bytes: 208,
        sha256: "d4226bbe3117d2d253ba4609720ba82c6c4ce4627a9a6ae05387c78983ac03de",
    },
    modelstore::ModelFile {
        path: "model.safetensors",
        bytes: 2_623_906_147,
        sha256: "c5faac877578591aea5b16bd4b55b672f8d74a0e2cf72ed122ed19d265bc89d2",
    },
    modelstore::ModelFile {
        path: "model.safetensors.index.json",
        bytes: 95_402,
        sha256: "44a4cb301066541d52a55667441c66c9712fd09dfd8214a48d6185c3eaa6d7ba",
    },
    modelstore::ModelFile {
        path: "tokenizer.json",
        bytes: 32_169_626,
        sha256: "cc8d3a0ce36466ccc1278bf987df5f71db1719b9ca6b4118264f45cb627bfe0f",
    },
    modelstore::ModelFile {
        path: "tokenizer_config.json",
        bytes: 2_773,
        sha256: "8155d42e19a25623e76fc0460ce24ddfd34a7c85b496456684249c68e5f93fc5",
    },
];

pub(super) const FILES_GEMMA_4_E4B_IT_MLX_4BIT: &[modelstore::ModelFile] = &[
    modelstore::ModelFile {
        path: "chat_template.jinja",
        bytes: 17_336,
        sha256: "2f1b4d75d067bae3fe44e676721c7f077d243bc007156cb9c2f8b5836613d082",
    },
    modelstore::ModelFile {
        path: "config.json",
        bytes: 6_628,
        sha256: "780ccb3a514a5f1ced161d383f948fc22eca9b84b752ca19494f625bd9bad7a6",
    },
    modelstore::ModelFile {
        path: "generation_config.json",
        bytes: 208,
        sha256: "d4226bbe3117d2d253ba4609720ba82c6c4ce4627a9a6ae05387c78983ac03de",
    },
    modelstore::ModelFile {
        path: "model.safetensors",
        bytes: 4_198_925_844,
        sha256: "16900c33eaabdb06be5378c4261aa73b09566d4c8b668382e2f4c86bb53759cd",
    },
    modelstore::ModelFile {
        path: "tokenizer.json",
        bytes: 32_169_626,
        sha256: "cc8d3a0ce36466ccc1278bf987df5f71db1719b9ca6b4118264f45cb627bfe0f",
    },
    modelstore::ModelFile {
        path: "tokenizer_config.json",
        bytes: 2_740,
        sha256: "080d9e1aff284e2f6043889cd05367966f7c7b80e025fbc0b06745e218158656",
    },
];

pub(super) const FILES_MINISTRAL_3_3B_INSTRUCT_MLX_4BIT: &[modelstore::ModelFile] = &[
    modelstore::ModelFile {
        path: "chat_template.jinja",
        bytes: 7_759,
        sha256: "c101a09b4ff42e8ed714cb67cd7144176e169b7d7be45d62e1e13326a5d7afd7",
    },
    modelstore::ModelFile {
        path: "config.json",
        bytes: 1_988,
        sha256: "cdd4bfa0a702d0241faaa3b80d15d481d7f8abf2659fba0958b1e7f24a28055f",
    },
    modelstore::ModelFile {
        path: "generation_config.json",
        bytes: 131,
        sha256: "e0923390059f84a9180b00e5501778acc45ea9856cd7f2fd68208b360927c677",
    },
    modelstore::ModelFile {
        path: "model.safetensors",
        bytes: 2_745_210_934,
        sha256: "69a68c0a961a64bb1145a697a83cf6cec45220d425c77a6c0a3568924078c3b4",
    },
    modelstore::ModelFile {
        path: "model.safetensors.index.json",
        bytes: 74_906,
        sha256: "22e571d413d8aa4e5f81f5ae678aa8b4f5b1d67598a67c08b31182075592ec93",
    },
    modelstore::ModelFile {
        path: "tokenizer.json",
        bytes: 17_077_402,
        sha256: "286acad9b0e27fce778ac429763536accf618ccb6ed72963b6f94685e531c5c7",
    },
    modelstore::ModelFile {
        path: "tokenizer_config.json",
        bytes: 21_188,
        sha256: "54b6ee53caf87c338527472386b6adcc4253bf48b93cc9c27fe1d25db4ff36fc",
    },
];

pub(super) const MODELS: &[modelstore::MlxCleanupModel] = &[
    modelstore::MlxCleanupModel {
        id: modelstore::GEMMA_4_E2B_IT_MLX_ID,
        label: "Gemma 4 E2B IT · MLX 4-bit",
        repository: "DreamFoundries/gemma-4-E2B-it-4bit",
        revision: "75a479632d6a594ea323942b883cdbc1bdd97a14",
        manifest_sha256: "61d223991b167eca1bbda92dfd2417e63bc6815260b48cb8f84688f8e28e3695",
        files: FILES_GEMMA_4_E2B_IT_MLX_4BIT,
        language: "Multilingual; product language coverage pending verification",
        recipe: modelstore::GEMMA_4_CLEANUP_RECIPE,
        development_reserve_bytes: 6 * 1024 * 1024 * 1024,
        release_qualified: false,
        importable: false,
    },
    modelstore::MlxCleanupModel {
        id: modelstore::GEMMA_4_E4B_IT_MLX_ID,
        label: "Gemma 4 E4B IT · MLX 4-bit",
        repository: "Anotos/Gemma-4-E4B-it-Text-MLX-4bit",
        revision: "da76496efe4486235e6e0aa9315203e27b957bcd",
        manifest_sha256: "f29888227b0d076a257463fb3f414c0916b298145a283bc4352bf6bc7a54e55e",
        files: FILES_GEMMA_4_E4B_IT_MLX_4BIT,
        language: "Multilingual; product language coverage pending verification",
        recipe: modelstore::GEMMA_4_CLEANUP_RECIPE,
        development_reserve_bytes: 8 * 1024 * 1024 * 1024,
        release_qualified: false,
        importable: false,
    },
];

/// Pinned Ministral 3 candidate with a registered model-layout proof.
/// User availability is curated separately from its release qualification.
pub(super) const MISTRAL3_CANDIDATES: &[modelstore::MlxCleanupModel] =
    &[modelstore::MlxCleanupModel {
        id: modelstore::MINISTRAL_3_3B_INSTRUCT_MLX_ID,
        label: "Ministral 3 3B Instruct · MLX 4-bit",
        repository: "mlx-community/Ministral-3-3B-Instruct-2512-4bit",
        revision: "a962dcb09eee4169c890e544c9eb938f1113fdee",
        manifest_sha256: "9c190f28458ff6cd6e1d17fe90913d445cbe745728803364e948cb1e87c16ab2",
        files: FILES_MINISTRAL_3_3B_INSTRUCT_MLX_4BIT,
        language: "Multilingual; product language coverage pending verification",
        recipe: modelstore::MINISTRAL_3_CLEANUP_RECIPE,
        development_reserve_bytes: 8 * 1024 * 1024 * 1024,
        release_qualified: false,
        importable: false,
    }];

/// Test-only S1 candidate. Keep it out of the runtime catalog until the
/// constrained prompt, language, and fallback paths pass end-to-end checks.
#[cfg(test)]
pub(super) const S1_CANDIDATES: &[modelstore::MlxCleanupModel] = &[modelstore::MlxCleanupModel {
    id: modelstore::S1_MINI_MLX_ID,
    label: "S1-mini by Superwhisper · MLX 4-bit",
    repository: "mlx-community/S1-mini-MLX-4bit",
    revision: "5cbd7aec3401144f88a331d385c40b65fd2548eb",
    manifest_sha256: "5b35df3618cf8fa184b62fcdefc4875dd1adb7a174e9bc6249f7189cea711ca3",
    files: FILES_S1_MINI_MLX_4BIT,
    language: "English only; strict route screening required",
    recipe: modelstore::S1_MINI_CLEANUP_RECIPE,
    development_reserve_bytes: 4 * 1024 * 1024 * 1024,
    release_qualified: false,
    importable: false,
}];

#[cfg(test)]
pub(super) const FILES_S1_MINI_MLX_4BIT: &[modelstore::ModelFile] = &[
    modelstore::ModelFile {
        path: "chat_template.jinja",
        bytes: 4_168,
        sha256: "a55ee1b1660128b7098723e0abcd92caa0788061051c62d51cbe87d9cf1974d8",
    },
    modelstore::ModelFile {
        path: "config.json",
        bytes: 1_021,
        sha256: "d8cbd096f758add51a940656710a0319cfb3ad4ae4591507ab29c2e8bebecc54",
    },
    modelstore::ModelFile {
        path: "generation_config.json",
        bytes: 181,
        sha256: "6ca52b0bcb818c9e52db8ea18413494110c5f2def581b769f621dda951a7863b",
    },
    modelstore::ModelFile {
        path: "model.safetensors",
        bytes: 335_450_548,
        sha256: "284990f04854426d6164d634d8c472d7eb6e79c68f8fd8b4c8a80a4f234276ba",
    },
    modelstore::ModelFile {
        path: "model.safetensors.index.json",
        bytes: 49_770,
        sha256: "5ee67e4efeb75dfacb8dd3892dd6208473771264fdeff6b9eba4a79db5b1435e",
    },
    modelstore::ModelFile {
        path: "tokenizer.json",
        bytes: 11_422_650,
        sha256: "be75606093db2094d7cd20f3c2f385c212750648bd6ea4fb2bf507a6a4c55506",
    },
    modelstore::ModelFile {
        path: "tokenizer_config.json",
        bytes: 729,
        sha256: "ee8f6d44bf2353e6d3686c3adaf70e1ccfe9e6ed6822d0ab2f28cafdd7754792",
    },
];
