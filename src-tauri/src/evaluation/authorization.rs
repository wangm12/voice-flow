//! Credential lookup and authorization applicability belong to candidate resolution.
use super::validation::{error, read_bounded, InputError};
use serde::Serialize;
use std::path::Path;
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CredentialProvider {
    Groq,
    OpenAi,
    Qwen,
}
impl CredentialProvider {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Groq => "groq",
            Self::OpenAi => "openai",
            Self::Qwen => "qwen",
        }
    }
    fn prefix(self) -> &'static str {
        match self {
            Self::Groq => "VOICEFLOW_EVAL_GROQ",
            Self::OpenAi => "VOICEFLOW_EVAL_OPENAI",
            Self::Qwen => "VOICEFLOW_EVAL_QWEN",
        }
    }
    pub(crate) fn resolve(self) -> Result<KeyMaterial, InputError> {
        let prefix = self.prefix();
        KeyMaterial::resolve(&format!("{prefix}_API_KEY"), &format!("{prefix}_KEY_FILE"))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum CredentialBinding {
    ExplicitEnvironment { variable: String },
    ExplicitKeyFile { canonical_path: String },
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct AuthorizationScope {
    binding: CredentialBinding,
    origin: String,
}
#[derive(Clone)]
pub(crate) struct KeyMaterial {
    pub(crate) key: Option<String>,
    pub(crate) source: String,
    binding: Option<CredentialBinding>,
}
impl KeyMaterial {
    pub(crate) fn resolve(key_env: &str, file_env: &str) -> Result<Self, InputError> {
        if let Ok(value) = std::env::var(key_env) {
            if !value.trim().is_empty() {
                if value.len() > 16_384 {
                    return Err(error(
                        Path::new("credentials"),
                        "credential_unavailable",
                        "explicit credential exceeds 16 KiB",
                    ));
                }
                return Ok(Self {
                    key: Some(value.trim().into()),
                    source: "explicit_environment".into(),
                    binding: Some(CredentialBinding::ExplicitEnvironment {
                        variable: key_env.into(),
                    }),
                });
            }
        }
        if let Some(path) = std::env::var_os(file_env) {
            let canonical = Path::new(&path).canonicalize().map_err(|_| {
                error(
                    Path::new(&path),
                    "credential_unavailable",
                    "explicit key file unavailable",
                )
            })?;
            let bytes = read_bounded(&canonical, 16_384)?;
            let value = String::from_utf8(bytes).map_err(|_| {
                error(
                    &canonical,
                    "credential_unavailable",
                    "explicit key file must be UTF-8",
                )
            })?;
            if value.trim().is_empty() {
                return Err(error(
                    &canonical,
                    "credential_unavailable",
                    "explicit key file empty",
                ));
            }
            return Ok(Self {
                key: Some(value.trim().into()),
                source: "explicit_key_file".into(),
                binding: Some(CredentialBinding::ExplicitKeyFile {
                    canonical_path: canonical.display().to_string(),
                }),
            });
        }
        Ok(Self::unavailable("no_explicit_key"))
    }
    #[cfg(test)]
    pub(crate) fn synthetic(binding: &str) -> Self {
        Self {
            key: Some("synthetic-test-key".into()),
            source: "synthetic_fixture".into(),
            binding: Some(CredentialBinding::ExplicitEnvironment {
                variable: binding.into(),
            }),
        }
    }
    pub(crate) fn unavailable(source: &str) -> Self {
        Self {
            key: None,
            source: source.into(),
            binding: None,
        }
    }
    pub(crate) fn require(self, provider: CredentialProvider) -> Result<Self, InputError> {
        if self.key.is_none() {
            return Err(error(
                Path::new("credentials"),
                "credential_unavailable",
                format!(
                    "Supply {}_API_KEY or {}_KEY_FILE explicitly.",
                    provider.prefix(),
                    provider.prefix()
                ),
            ));
        }
        Ok(self)
    }
    pub(crate) fn scope(&self, endpoint: &str) -> Option<AuthorizationScope> {
        let binding = self.binding.clone()?;
        let endpoint = reqwest::Url::parse(endpoint).ok()?;
        Some(AuthorizationScope {
            binding,
            origin: endpoint.origin().ascii_serialization(),
        })
    }
}
#[derive(Default)]
pub(crate) struct AuthorizationBlocks {
    blocked: std::collections::BTreeSet<AuthorizationScope>,
}
impl AuthorizationBlocks {
    pub(crate) fn reason(&self, scope: Option<&AuthorizationScope>) -> Option<String> {
        scope
            .filter(|s| self.blocked.contains(*s))
            .map(|_| "authorization_error".into())
    }
    pub(crate) fn observe(&mut self, scope: Option<&AuthorizationScope>, reason: Option<&str>) {
        if reason == Some("authorization_error") {
            if let Some(scope) = scope {
                self.blocked.insert(scope.clone());
            }
        }
    }
}
