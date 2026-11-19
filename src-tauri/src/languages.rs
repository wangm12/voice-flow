/// Canonical BCP-47 recognition language metadata and provider mapping.
///
/// The frontend and backend share the exact same canonical language codes so
/// settings persistence, capability validation, and provider request builders
/// behave consistently across every language.
use std::collections::HashSet;

pub const MAX_PREFERRED_LANGUAGES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguageSpec {
    pub code: &'static str,
    pub native_name: &'static str,
    pub english_name: &'static str,
    pub chinese_name: &'static str,
    pub primary_iso639_1: &'static str,
    pub cjk_or_non_spaced: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeepgramLanguageParam {
    Language(&'static str),
    DetectLanguageTrue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelLanguageSupport {
    pub allowed_codes: &'static [&'static str],
    pub auto_allowed: bool,
    pub hints_allowed: bool,
}

const ALL_LANGUAGE_CODES: &[&str] = &[
    "auto", "zh", "zh-TW", "yue", "en", "ja", "ko", "es", "fr", "de", "pt", "it", "ru", "ar", "hi",
    "vi", "th", "id", "nl", "pl", "tr", "uk", "sv",
];
const ON_DEVICE_AUTO_OR_EXPLICIT_CODES: &[&str] = &["auto", "zh", "en"];
const ON_DEVICE_COHERE_CODES: &[&str] = &["zh", "en"];
const AUTO_ONLY_CODES: &[&str] = &["auto"];
const EN_ONLY_CODES: &[&str] = &["en"];
const DEEPGRAM_ALLOWED_CODES: &[&str] = &[
    "auto", "zh", "zh-TW", "en", "ja", "ko", "es", "fr", "de", "pt", "it", "ru", "hi", "vi", "th",
    "id", "nl", "pl", "tr", "uk", "sv",
];
const DEEPGRAM_NOVA3_MULTI_CODES: &[&str] =
    &["en", "es", "fr", "de", "hi", "ru", "pt", "ja", "it", "nl"];

pub fn model_language_support(
    provider: crate::providers::EngineProvider,
    model: &str,
) -> ModelLanguageSupport {
    let trimmed = model.trim();
    match provider {
        crate::providers::EngineProvider::OnDevice => {
            if trimmed.eq_ignore_ascii_case("sensevoice-small") {
                return ModelLanguageSupport {
                    allowed_codes: AUTO_ONLY_CODES,
                    auto_allowed: true,
                    hints_allowed: false,
                };
            }
            let normalized_id = trimmed.to_ascii_lowercase();
            match crate::ondevice_models::mlx_model(&normalized_id)
                .map(|entry| entry.language_policy)
            {
                Some(crate::ondevice_models::MlxLanguagePolicy::AutoOnly) => ModelLanguageSupport {
                    allowed_codes: AUTO_ONLY_CODES,
                    auto_allowed: true,
                    hints_allowed: false,
                },
                Some(crate::ondevice_models::MlxLanguagePolicy::ExplicitChineseOrEnglish) => {
                    ModelLanguageSupport {
                        allowed_codes: ON_DEVICE_COHERE_CODES,
                        auto_allowed: false,
                        hints_allowed: false,
                    }
                }
                _ => ModelLanguageSupport {
                    allowed_codes: ON_DEVICE_AUTO_OR_EXPLICIT_CODES,
                    auto_allowed: true,
                    hints_allowed: false,
                },
            }
        }
        crate::providers::EngineProvider::SiliconFlow
        | crate::providers::EngineProvider::Fireworks
        | crate::providers::EngineProvider::DashScope => ModelLanguageSupport {
            allowed_codes: AUTO_ONLY_CODES,
            auto_allowed: true,
            hints_allowed: false,
        },
        crate::providers::EngineProvider::Groq => {
            if trimmed.eq_ignore_ascii_case("distil-whisper-large-v3-en") {
                ModelLanguageSupport {
                    allowed_codes: EN_ONLY_CODES,
                    auto_allowed: false,
                    hints_allowed: false,
                }
            } else {
                ModelLanguageSupport {
                    allowed_codes: ALL_LANGUAGE_CODES,
                    auto_allowed: true,
                    hints_allowed: false,
                }
            }
        }
        crate::providers::EngineProvider::OpenAi => ModelLanguageSupport {
            allowed_codes: ALL_LANGUAGE_CODES,
            auto_allowed: true,
            hints_allowed: crate::asr::is_gpt_transcribe_model(trimmed),
        },
        crate::providers::EngineProvider::Soniox => ModelLanguageSupport {
            allowed_codes: ALL_LANGUAGE_CODES,
            auto_allowed: true,
            hints_allowed: true,
        },
        crate::providers::EngineProvider::AssemblyAi => ModelLanguageSupport {
            allowed_codes: ALL_LANGUAGE_CODES,
            auto_allowed: true,
            hints_allowed: true,
        },
        crate::providers::EngineProvider::Deepgram => ModelLanguageSupport {
            allowed_codes: DEEPGRAM_ALLOWED_CODES,
            auto_allowed: true,
            hints_allowed: crate::asr::is_deepgram_nova3_model(trimmed),
        },
        crate::providers::EngineProvider::Mistral
        | crate::providers::EngineProvider::LocalWhisper
        | crate::providers::EngineProvider::Custom
        | crate::providers::EngineProvider::DeepSeek
        | crate::providers::EngineProvider::Anthropic
        | crate::providers::EngineProvider::Ollama
        | crate::providers::EngineProvider::OnDeviceMlx
        | crate::providers::EngineProvider::OnDeviceLlamaCpp => ModelLanguageSupport {
            allowed_codes: ALL_LANGUAGE_CODES,
            auto_allowed: true,
            hints_allowed: false,
        },
    }
}

/// Resolve a saved or requested language to a code supported by `(provider, model)`,
/// mapping Chinese regional variants (`zh-TW`, `yue`) to `zh` when the model supports
/// `zh` only, and falling back to `"auto"` (or the first allowed fixed code) otherwise.
pub fn coerce_language_for_model(
    provider: crate::providers::EngineProvider,
    model: &str,
    raw: &str,
) -> &'static str {
    let support = model_language_support(provider, model);
    if let Some(canonical) = canonical_language_code(raw) {
        if let Some(&allowed) = support
            .allowed_codes
            .iter()
            .find(|code| **code == canonical)
        {
            return allowed;
        }
        if canonical == "yue" && support.allowed_codes.contains(&"zh-TW") {
            return "zh-TW";
        }
        if matches!(canonical, "zh-TW" | "yue") && support.allowed_codes.contains(&"zh") {
            return "zh";
        }
    }
    if support.auto_allowed {
        "auto"
    } else {
        support.allowed_codes.first().copied().unwrap_or("en")
    }
}

pub fn language_supported_by_model(
    provider: crate::providers::EngineProvider,
    model: &str,
    raw: &str,
) -> bool {
    let Some(canonical) = canonical_language_code(raw) else {
        return false;
    };
    let support = model_language_support(provider, model);
    support.allowed_codes.contains(&canonical)
}

pub const SUPPORTED_LANGUAGES: &[LanguageSpec] = &[
    LanguageSpec {
        code: "auto",
        native_name: "自动检测",
        english_name: "Auto-detect",
        chinese_name: "自动检测",
        primary_iso639_1: "auto",
        cjk_or_non_spaced: false,
    },
    LanguageSpec {
        code: "zh",
        native_name: "中文（简体）",
        english_name: "Chinese (Simplified)",
        chinese_name: "中文（简体）",
        primary_iso639_1: "zh",
        cjk_or_non_spaced: true,
    },
    LanguageSpec {
        code: "zh-TW",
        native_name: "繁體中文",
        english_name: "Chinese (Traditional)",
        chinese_name: "繁体中文",
        primary_iso639_1: "zh",
        cjk_or_non_spaced: true,
    },
    LanguageSpec {
        code: "yue",
        native_name: "粵語",
        english_name: "Cantonese",
        chinese_name: "粤语",
        primary_iso639_1: "yue",
        cjk_or_non_spaced: true,
    },
    LanguageSpec {
        code: "en",
        native_name: "English",
        english_name: "English",
        chinese_name: "英语",
        primary_iso639_1: "en",
        cjk_or_non_spaced: false,
    },
    LanguageSpec {
        code: "ja",
        native_name: "日本語",
        english_name: "Japanese",
        chinese_name: "日语",
        primary_iso639_1: "ja",
        cjk_or_non_spaced: true,
    },
    LanguageSpec {
        code: "ko",
        native_name: "한국어",
        english_name: "Korean",
        chinese_name: "韩语",
        primary_iso639_1: "ko",
        cjk_or_non_spaced: false,
    },
    LanguageSpec {
        code: "es",
        native_name: "Español",
        english_name: "Spanish",
        chinese_name: "西班牙语",
        primary_iso639_1: "es",
        cjk_or_non_spaced: false,
    },
    LanguageSpec {
        code: "fr",
        native_name: "Français",
        english_name: "French",
        chinese_name: "法语",
        primary_iso639_1: "fr",
        cjk_or_non_spaced: false,
    },
    LanguageSpec {
        code: "de",
        native_name: "Deutsch",
        english_name: "German",
        chinese_name: "德语",
        primary_iso639_1: "de",
        cjk_or_non_spaced: false,
    },
    LanguageSpec {
        code: "pt",
        native_name: "Português",
        english_name: "Portuguese",
        chinese_name: "葡萄牙语",
        primary_iso639_1: "pt",
        cjk_or_non_spaced: false,
    },
    LanguageSpec {
        code: "it",
        native_name: "Italiano",
        english_name: "Italian",
        chinese_name: "意大利语",
        primary_iso639_1: "it",
        cjk_or_non_spaced: false,
    },
    LanguageSpec {
        code: "ru",
        native_name: "Русский",
        english_name: "Russian",
        chinese_name: "俄语",
        primary_iso639_1: "ru",
        cjk_or_non_spaced: false,
    },
    LanguageSpec {
        code: "ar",
        native_name: "العربية",
        english_name: "Arabic",
        chinese_name: "阿拉伯语",
        primary_iso639_1: "ar",
        cjk_or_non_spaced: false,
    },
    LanguageSpec {
        code: "hi",
        native_name: "हिन्दी",
        english_name: "Hindi",
        chinese_name: "印地语",
        primary_iso639_1: "hi",
        cjk_or_non_spaced: false,
    },
    LanguageSpec {
        code: "vi",
        native_name: "Tiếng Việt",
        english_name: "Vietnamese",
        chinese_name: "越南语",
        primary_iso639_1: "vi",
        cjk_or_non_spaced: false,
    },
    LanguageSpec {
        code: "th",
        native_name: "ไทย",
        english_name: "Thai",
        chinese_name: "泰语",
        primary_iso639_1: "th",
        cjk_or_non_spaced: true,
    },
    LanguageSpec {
        code: "id",
        native_name: "Bahasa Indonesia",
        english_name: "Indonesian",
        chinese_name: "印度尼西亚语",
        primary_iso639_1: "id",
        cjk_or_non_spaced: false,
    },
    LanguageSpec {
        code: "nl",
        native_name: "Nederlands",
        english_name: "Dutch",
        chinese_name: "荷兰语",
        primary_iso639_1: "nl",
        cjk_or_non_spaced: false,
    },
    LanguageSpec {
        code: "pl",
        native_name: "Polski",
        english_name: "Polish",
        chinese_name: "波兰语",
        primary_iso639_1: "pl",
        cjk_or_non_spaced: false,
    },
    LanguageSpec {
        code: "tr",
        native_name: "Türkçe",
        english_name: "Turkish",
        chinese_name: "土耳其语",
        primary_iso639_1: "tr",
        cjk_or_non_spaced: false,
    },
    LanguageSpec {
        code: "uk",
        native_name: "Українська",
        english_name: "Ukrainian",
        chinese_name: "乌克兰语",
        primary_iso639_1: "uk",
        cjk_or_non_spaced: false,
    },
    LanguageSpec {
        code: "sv",
        native_name: "Svenska",
        english_name: "Swedish",
        chinese_name: "瑞典语",
        primary_iso639_1: "sv",
        cjk_or_non_spaced: false,
    },
];

/// Return the canonical BCP-47 language code for any supported tag or alias.
pub fn canonical_language_code(raw: &str) -> Option<&'static str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = trimmed.replace('_', "-").to_ascii_lowercase();
    match normalized.as_str() {
        "auto" => return Some("auto"),
        "zh" | "zh-cn" | "zh-sg" | "zh-hans" | "zh-hans-cn" | "zh-hans-sg" | "cmn" => {
            return Some("zh")
        }
        "zh-tw" | "zh-hk" | "zh-mo" | "zh-hant" | "zh-hant-tw" | "zh-hant-hk" => {
            return Some("zh-TW")
        }
        "yue" | "yue-hant" | "yue-hk" | "zh-yue" => return Some("yue"),
        "in" => return Some("id"),
        _ => {}
    }

    if let Some(exact) = SUPPORTED_LANGUAGES
        .iter()
        .find(|spec| spec.code.eq_ignore_ascii_case(&normalized))
    {
        return Some(exact.code);
    }

    let primary = normalized.split('-').next().unwrap_or("");
    if primary.is_empty() || primary == "auto" {
        return None;
    }
    if primary == "zh" {
        return Some("zh");
    }
    SUPPORTED_LANGUAGES
        .iter()
        .find(|spec| spec.code != "auto" && spec.primary_iso639_1 == primary)
        .map(|spec| spec.code)
}

/// Normalize a selected recognition language, defaulting unsupported values to `"auto"`.
pub fn normalize_selected_language(raw: &str) -> String {
    canonical_language_code(raw).unwrap_or("auto").to_owned()
}

/// Look up full language metadata by canonical code or alias.
pub fn find_language(code: &str) -> Option<&'static LanguageSpec> {
    let canonical = canonical_language_code(code)?;
    SUPPORTED_LANGUAGES
        .iter()
        .find(|spec| spec.code == canonical)
}

/// Normalize a list of preferred spoken languages (non-`"auto"`, unique, max 4).
pub fn normalize_preferred_languages(values: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for raw in values {
        let Some(canonical) = canonical_language_code(raw) else {
            continue;
        };
        if canonical == "auto" {
            continue;
        }
        if seen.insert(canonical) {
            out.push(canonical.to_owned());
            if out.len() >= MAX_PREFERRED_LANGUAGES {
                break;
            }
        }
    }
    out
}

/// Validate a preferred spoken languages patch/setting payload.
pub fn validate_preferred_languages(values: &[String]) -> anyhow::Result<()> {
    if values.len() > MAX_PREFERRED_LANGUAGES {
        anyhow::bail!(
            "preferred_languages allows at most {} languages",
            MAX_PREFERRED_LANGUAGES
        );
    }
    let mut seen = HashSet::new();
    for raw in values {
        let Some(canonical) = canonical_language_code(raw) else {
            anyhow::bail!("unsupported preferred language: {}", raw.trim());
        };
        if canonical == "auto" {
            anyhow::bail!("preferred_languages cannot include auto");
        }
        if !seen.insert(canonical) {
            anyhow::bail!("duplicate preferred language: {}", canonical);
        }
    }
    Ok(())
}

/// Return true if a single canonical code or alias is Chinese (Mandarin/Traditional/Cantonese).
pub fn is_chinese_language_code(code: &str) -> bool {
    matches!(canonical_language_code(code), Some("zh" | "zh-TW" | "yue"))
}

/// Return true only when the active session explicitly targets Chinese or prefers Chinese in auto mode.
///
/// Unbiased `auto` mode (`language == None | Some("auto")` and empty `preferred_languages`) returns
/// `false` so Chinese Whisper/GPT seed text is never injected into arbitrary multilingual sessions.
pub fn is_chinese_session(language: Option<&str>, preferred_languages: &[String]) -> bool {
    if let Some(fixed) = language.and_then(canonical_language_code) {
        if fixed != "auto" {
            return is_chinese_language_code(fixed);
        }
    }
    preferred_languages
        .iter()
        .any(|lang| is_chinese_language_code(lang))
}

/// Convert a language code into the Whisper/OpenAI ISO-639-1 parameter (`"zh-TW"` -> `"zh"`).
pub fn whisper_language_code(code: &str) -> Option<&'static str> {
    let spec = find_language(code)?;
    if spec.code == "auto" {
        return None;
    }
    if spec.code == "yue" {
        return Some("zh");
    }
    Some(spec.primary_iso639_1)
}

/// Resolve OpenAI `gpt-transcribe` `languages[]` candidate hints.
///
/// Fixed language -> `[iso]`.
/// Auto + non-empty `preferred_languages` -> deduplicated ISO codes.
/// Auto + empty `preferred_languages` -> empty (unbiased autodetection).
pub fn candidate_language_hints(
    language: Option<&str>,
    preferred_languages: &[String],
) -> Vec<String> {
    if let Some(fixed) = language.and_then(canonical_language_code) {
        if fixed != "auto" {
            return whisper_language_code(fixed)
                .map(|iso| vec![iso.to_owned()])
                .unwrap_or_default();
        }
    }
    let mut seen = HashSet::new();
    let mut hints = Vec::new();
    for pref in normalize_preferred_languages(preferred_languages) {
        if let Some(iso) = whisper_language_code(&pref) {
            if seen.insert(iso) {
                hints.push(iso.to_owned());
            }
        }
    }
    hints
}

/// Resolve AssemblyAI `language_codes` for fixed or preferred-language sessions.
///
/// Returns `None` when `language` is `"auto"` and `preferred_languages` is empty so the caller
/// can omit `language_codes` and request unbiased `language_detection: true`.
pub fn assemblyai_language_codes(
    language: Option<&str>,
    preferred_languages: &[String],
) -> Option<Vec<&'static str>> {
    const ASSEMBLYAI_SUPPORTED: &[&str] = &[
        "en", "zh", "yue", "ja", "ko", "es", "fr", "de", "pt", "it", "ru", "ar", "hi", "vi", "th",
        "id", "nl", "pl", "tr", "uk", "sv",
    ];
    let map_code = |raw: &str| -> Option<&'static str> {
        let canonical = canonical_language_code(raw)?;
        if canonical == "auto" {
            return None;
        }
        let target = if canonical == "zh-TW" {
            "zh"
        } else if canonical == "yue" {
            "yue"
        } else {
            find_language(canonical)?.primary_iso639_1
        };
        ASSEMBLYAI_SUPPORTED
            .iter()
            .copied()
            .find(|candidate| *candidate == target)
    };

    if let Some(fixed) = language.and_then(canonical_language_code) {
        if fixed != "auto" {
            return map_code(fixed).map(|code| vec![code]);
        }
    }
    let mut seen = HashSet::new();
    let mut codes = Vec::new();
    for pref in normalize_preferred_languages(preferred_languages) {
        if let Some(code) = map_code(&pref) {
            if seen.insert(code) {
                codes.push(code);
            }
        }
    }
    (!codes.is_empty()).then_some(codes)
}

/// Resolve Soniox `language_hints`.
///
/// Fixed language -> `[iso]`.
/// Auto + non-empty `preferred_languages` -> deduplicated hints.
/// Auto + empty `preferred_languages` -> `None` (true unbiased multilingual auto-detection).
pub fn soniox_language_hints(
    language: Option<&str>,
    preferred_languages: &[String],
) -> Option<Vec<&'static str>> {
    let map_code = |raw: &str| -> Option<&'static str> {
        let canonical = canonical_language_code(raw)?;
        if canonical == "auto" {
            return None;
        }
        if canonical == "zh-TW" || canonical == "yue" {
            return Some("zh");
        }
        find_language(canonical).map(|spec| spec.primary_iso639_1)
    };

    if let Some(fixed) = language.and_then(canonical_language_code) {
        if fixed != "auto" {
            return map_code(fixed).map(|code| vec![code]);
        }
    }
    let mut seen = HashSet::new();
    let mut hints = Vec::new();
    for pref in normalize_preferred_languages(preferred_languages) {
        if let Some(code) = map_code(&pref) {
            if seen.insert(code) {
                hints.push(code);
            }
        }
    }
    (!hints.is_empty()).then_some(hints)
}

/// Resolve Deepgram `language` vs `detect_language=true` (including `nova-3` `language=multi`).
pub fn deepgram_language_param(
    is_nova3: bool,
    language: Option<&str>,
    preferred_languages: &[String],
) -> DeepgramLanguageParam {
    let map_code = |raw: &str| -> Option<&'static str> {
        let spec = find_language(raw)?;
        if spec.code == "auto" {
            return None;
        }
        let mapped = match spec.code {
            "zh-TW" => "zh-TW",
            "yue" => "zh",
            other => other,
        };
        DEEPGRAM_ALLOWED_CODES.contains(&mapped).then_some(mapped)
    };

    if let Some(fixed) = language.and_then(map_code) {
        return DeepgramLanguageParam::Language(fixed);
    }

    let prefs = normalize_preferred_languages(preferred_languages);

    // Deepgram Nova-3 `language=multi` covers multilingual Latin/Japanese/Hindi/Russian
    // code-switching, NOT Chinese (`zh`/`zh-TW`/`yue`) or Korean (`ko`). Use `multi`
    // only when non-empty `preferred_languages` all fall within its supported multi set;
    // otherwise use unbiased `detect_language=true`.
    if is_nova3
        && !prefs.is_empty()
        && prefs
            .iter()
            .all(|code| DEEPGRAM_NOVA3_MULTI_CODES.contains(&code.as_str()))
    {
        DeepgramLanguageParam::Language("multi")
    } else {
        DeepgramLanguageParam::DetectLanguageTrue
    }
}

/// Resolve DashScope Qwen ASR `language` option parameter.
pub fn qwen_language_code(raw: &str) -> Option<&'static str> {
    const SUPPORTED: &[&str] = &[
        "zh", "yue", "en", "ja", "de", "ko", "ru", "fr", "pt", "ar", "it", "es", "hi", "id", "th",
        "tr", "uk", "vi", "cs", "da", "fil", "fi", "is", "ms", "no", "pl", "sv",
    ];
    let canonical = canonical_language_code(raw)?;
    if canonical == "auto" {
        return None;
    }
    let primary = if canonical == "zh-TW" {
        "zh"
    } else {
        find_language(canonical)?.primary_iso639_1
    };
    SUPPORTED
        .iter()
        .copied()
        .find(|candidate| candidate.eq_ignore_ascii_case(primary))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_supported_bcp47_codes_and_aliases() {
        assert_eq!(canonical_language_code("auto"), Some("auto"));
        assert_eq!(canonical_language_code("zh-CN"), Some("zh"));
        assert_eq!(canonical_language_code("zh_TW"), Some("zh-TW"));
        assert_eq!(canonical_language_code("zh-Hant-HK"), Some("zh-TW"));
        assert_eq!(canonical_language_code("en-US"), Some("en"));
        assert_eq!(canonical_language_code("ja-JP"), Some("ja"));
        assert_eq!(canonical_language_code("pt-BR"), Some("pt"));
        assert_eq!(canonical_language_code("in"), Some("id"));
        assert_eq!(canonical_language_code("yue-Hant"), Some("yue"));
        assert_eq!(canonical_language_code("xx-unknown"), None);
    }

    #[test]
    fn preferred_languages_normalize_dedup_and_cap_at_four() {
        let normalized = normalize_preferred_languages(&[
            "auto".into(),
            "ja-JP".into(),
            "zh_TW".into(),
            "zh-Hant".into(),
            "en-US".into(),
            "fr".into(),
            "de".into(),
            "es".into(),
        ]);
        assert_eq!(normalized, vec!["ja", "zh-TW", "en", "fr"]);
        assert!(validate_preferred_languages(&normalized).is_ok());
        assert!(validate_preferred_languages(&[
            "ja".into(),
            "en".into(),
            "fr".into(),
            "de".into(),
            "es".into(),
        ])
        .is_err());
        assert!(validate_preferred_languages(&["auto".into()]).is_err());
        assert!(validate_preferred_languages(&["zh-TW".into(), "zh-Hant".into()]).is_err());
    }

    #[test]
    fn unbiased_auto_mode_avoids_hardcoded_zh_en_and_chinese_seed_bias() {
        assert_eq!(soniox_language_hints(Some("auto"), &[]), None);
        assert_eq!(assemblyai_language_codes(Some("auto"), &[]), None);
        assert!(candidate_language_hints(Some("auto"), &[]).is_empty());
        assert!(!is_chinese_session(Some("auto"), &[]));
        assert!(!is_chinese_session(Some("ja"), &["zh".into()]));
        assert!(is_chinese_session(Some("zh-TW"), &[]));
        assert!(is_chinese_session(
            Some("auto"),
            &["zh".into(), "en".into()]
        ));

        assert_eq!(
            soniox_language_hints(Some("auto"), &["ja".into(), "en".into()]),
            Some(vec!["ja", "en"])
        );
        assert_eq!(
            assemblyai_language_codes(Some("auto"), &["fr".into(), "de".into()]),
            Some(vec!["fr", "de"])
        );
        assert_eq!(
            candidate_language_hints(Some("auto"), &["ja".into(), "ko".into()]),
            vec!["ja".to_owned(), "ko".to_owned()]
        );
        assert_eq!(
            deepgram_language_param(true, Some("auto"), &[]),
            DeepgramLanguageParam::DetectLanguageTrue
        );
        assert_eq!(
            deepgram_language_param(true, Some("auto"), &["en".into(), "es".into()]),
            DeepgramLanguageParam::Language("multi")
        );
        assert_eq!(
            deepgram_language_param(true, Some("auto"), &["en".into()]),
            DeepgramLanguageParam::Language("multi")
        );
        assert_eq!(
            deepgram_language_param(true, Some("auto"), &["zh".into()]),
            DeepgramLanguageParam::DetectLanguageTrue
        );
        assert_eq!(
            deepgram_language_param(false, Some("auto"), &["en".into()]),
            DeepgramLanguageParam::DetectLanguageTrue
        );
        assert_eq!(
            deepgram_language_param(true, Some("auto"), &["zh".into(), "en".into()]),
            DeepgramLanguageParam::DetectLanguageTrue
        );
        assert_eq!(
            deepgram_language_param(false, Some("auto"), &[]),
            DeepgramLanguageParam::DetectLanguageTrue
        );
        assert_eq!(
            deepgram_language_param(true, Some("zh-TW"), &[]),
            DeepgramLanguageParam::Language("zh-TW")
        );
    }

    #[test]
    fn model_language_support_and_coercion_enforce_per_model_bounds() {
        use crate::providers::EngineProvider;
        let ondevice = model_language_support(EngineProvider::OnDevice, "qwen3-asr-1.7b");
        assert!(ondevice.auto_allowed);
        assert!(!ondevice.hints_allowed);
        assert!(ondevice.allowed_codes.contains(&"zh"));
        assert!(ondevice.allowed_codes.contains(&"en"));
        assert!(!ondevice.allowed_codes.contains(&"zh-TW"));
        assert!(!language_supported_by_model(
            EngineProvider::OnDevice,
            "qwen3-asr-1.7b",
            "zh-TW"
        ));
        assert_eq!(
            coerce_language_for_model(EngineProvider::OnDevice, "qwen3-asr-1.7b", "zh-TW"),
            "zh"
        );
        assert_eq!(
            coerce_language_for_model(EngineProvider::OnDevice, "qwen3-asr-1.7b", "ar"),
            "auto"
        );
        let sensevoice = model_language_support(EngineProvider::OnDevice, "sensevoice-small");
        assert_eq!(sensevoice.allowed_codes, &["auto"]);
        assert!(sensevoice.auto_allowed);

        let assemblyai = model_language_support(EngineProvider::AssemblyAi, "universal-3-5-pro");
        assert!(assemblyai.allowed_codes.contains(&"zh-TW"));
        assert!(assemblyai.allowed_codes.contains(&"ko"));
        assert!(assemblyai.allowed_codes.contains(&"uk"));

        let deepgram_nova3 = model_language_support(EngineProvider::Deepgram, "nova-3");
        assert!(deepgram_nova3.auto_allowed);
        assert!(deepgram_nova3.hints_allowed);
        assert!(deepgram_nova3.allowed_codes.contains(&"zh-TW"));
        assert!(deepgram_nova3.allowed_codes.contains(&"ja"));
        assert!(!deepgram_nova3.allowed_codes.contains(&"yue"));
        assert_eq!(
            deepgram_language_param(true, Some("auto"), &["ar".into()]),
            DeepgramLanguageParam::DetectLanguageTrue
        );
        assert_eq!(
            coerce_language_for_model(EngineProvider::Deepgram, "nova-3", "yue"),
            "zh-TW"
        );
        assert_eq!(
            coerce_language_for_model(EngineProvider::Deepgram, "nova-3", "zh-TW"),
            "zh-TW"
        );
    }
}
