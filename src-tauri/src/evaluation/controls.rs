//! Bounded immutable explicit fixture inputs. No App dictionary/context import.
use super::{
    contracts::*,
    validation::{self, InputError, ValidatedCase},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VocabularyFixtureV1 {
    pub artifact_type: String,
    pub schema_version: u32,
    pub terms: Vec<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContextFixtureV1 {
    pub artifact_type: String,
    pub schema_version: u32,
    pub text: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Replacement {
    pub before: String,
    pub after: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalCaseRules {
    pub case_id: String,
    pub dictionary: Vec<String>,
    pub replacements: Vec<Replacement>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalRuleFixtureV1 {
    pub artifact_type: String,
    pub schema_version: u32,
    pub revision: String,
    pub cases: Vec<LocalCaseRules>,
}
#[derive(Debug)]
pub struct FixtureSnapshot<T> {
    pub canonical_path: PathBuf,
    pub relative_path: String,
    pub raw: Vec<u8>,
    pub sha256: String,
    pub parsed: T,
}
#[derive(Debug, Default)]
pub struct JobFixtureInputs {
    pub vocabulary: Option<FixtureSnapshot<VocabularyFixtureV1>>,
    pub context: Option<FixtureSnapshot<ContextFixtureV1>>,
    pub local: Option<FixtureSnapshot<LocalRuleFixtureV1>>,
}
#[derive(Debug, Clone, Default)]
pub(crate) struct PreparationFixture {
    pub dictionary: Vec<String>,
    pub pairs: Vec<crate::lexicon::LexiconPair>,
    pub fuzzy: bool,
    pub spoken_layout: bool,
}
fn read<T: serde::de::DeserializeOwned>(
    recipe: &Path,
    relative: &str,
    expected: &str,
    limit: u64,
) -> Result<FixtureSnapshot<T>, InputError> {
    let path = validation::relative_file(recipe, relative)?;
    let raw = validation::read_bounded(&path, limit)?;
    let sha256 = validation::sha256(&raw);
    if !validation::digest(expected) || sha256 != expected {
        return Err(validation::error(
            &path,
            "sha_mismatch",
            "control fixture digest mismatch",
        ));
    }
    let parsed = serde_json::from_slice(&raw).map_err(|_| {
        validation::error(&path, "schema", "strict control fixture schema required")
    })?;
    Ok(FixtureSnapshot {
        canonical_path: path,
        relative_path: relative.into(),
        raw,
        sha256,
        parsed,
    })
}
fn terms(path: &Path, terms: &[String], allow_empty: bool) -> Result<(), InputError> {
    let mut seen = BTreeSet::new();
    if (!allow_empty && terms.is_empty())
        || terms.len() > 100
        || terms.iter().any(|term| {
            !validation::bounded(term, 256)
                || term.trim() != term
                || term.contains(['\n', '\r'])
                || !seen.insert(term)
        })
    {
        return Err(validation::error(
            path,
            "schema",
            "terms must be unique, bounded explicit strings (1-100 for vocabulary)",
        ));
    }
    Ok(())
}
pub fn validate(
    recipe: &Path,
    job: &JobV1,
    cases: &[ValidatedCase],
) -> Result<JobFixtureInputs, InputError> {
    let mut inputs = JobFixtureInputs::default();
    if let Vocabulary::Explicit {
        relative_path,
        content_sha256,
    } = &job.vocabulary
    {
        let snapshot: FixtureSnapshot<VocabularyFixtureV1> =
            read(recipe, relative_path, content_sha256, 16_384)?;
        if snapshot.parsed.artifact_type != "vocabulary_fixture"
            || snapshot.parsed.schema_version != 1
        {
            return Err(validation::error(
                &snapshot.canonical_path,
                "schema",
                "use VocabularyFixtureV1",
            ));
        }
        terms(&snapshot.canonical_path, &snapshot.parsed.terms, false)?;
        inputs.vocabulary = Some(snapshot);
    }
    if let Context::Fixture {
        relative_path,
        source_sha256,
        permission_snapshot,
    } = &job.context
    {
        let snapshot: FixtureSnapshot<ContextFixtureV1> =
            read(recipe, relative_path, source_sha256, 16_384)?;
        if snapshot.parsed.artifact_type != "context_fixture"
            || snapshot.parsed.schema_version != 1
            || !validation::bounded(&snapshot.parsed.text, 4096)
            || !permission_snapshot.provider_text_granted
        {
            return Err(validation::error(
                &snapshot.canonical_path,
                "schema",
                "use bounded ContextFixtureV1 with explicit provider_text_granted",
            ));
        }
        inputs.context = Some(snapshot);
    }
    if let Some(locator) = &job.local_rules.fixture {
        let snapshot: FixtureSnapshot<LocalRuleFixtureV1> =
            read(recipe, &locator.relative_path, &locator.sha256, 256 * 1024)?;
        if snapshot.parsed.artifact_type != "local_rule_fixture"
            || snapshot.parsed.schema_version != 1
            || snapshot.parsed.revision != job.local_rules.revision
        {
            return Err(validation::error(
                &snapshot.canonical_path,
                "identity_mismatch",
                "local rule fixture kind/revision mismatch",
            ));
        }
        let expected: BTreeSet<_> = cases
            .iter()
            .filter(|case| !case.negative())
            .map(|case| case.case.id())
            .collect();
        let mut seen = BTreeSet::new();
        for row in &snapshot.parsed.cases {
            if !expected.contains(row.case_id.as_str()) || !seen.insert(row.case_id.as_str()) {
                return Err(validation::error(
                    &snapshot.canonical_path,
                    "identity_mismatch",
                    "local rules must associate known speech case IDs exactly once",
                ));
            }
            terms(&snapshot.canonical_path, &row.dictionary, true)?;
            let mut before = BTreeSet::new();
            if row.replacements.len() > 100
                || row.replacements.iter().any(|p| {
                    !validation::bounded(&p.before, 256)
                        || !validation::bounded(&p.after, 256)
                        || p.before == p.after
                        || !before.insert(&p.before)
                        || !row.dictionary.contains(&p.after)
                })
            {
                return Err(validation::error(&snapshot.canonical_path,"schema","bounded unique replacement sources and dictionary-listed destinations required"));
            }
        }
        if seen != expected {
            return Err(validation::error(
                &snapshot.canonical_path,
                "identity_mismatch",
                "local rule fixture must cover every speech case",
            ));
        }
        inputs.local = Some(snapshot);
    } else if job.local_rules.dictionary_replacements || job.local_rules.fuzzy {
        return Err(validation::error(
            recipe,
            "unsupported_control",
            "dictionary replacements/fuzzy require an explicit local rule fixture",
        ));
    }
    Ok(inputs)
}
impl JobFixtureInputs {
    pub(crate) fn asr_options(
        &self,
        endpoint: &str,
        model: &str,
        language: &RecognitionLanguage,
    ) -> Result<crate::asr::AsrOptions, &'static str> {
        crate::asr::fixture_options(
            endpoint,
            model,
            match language {
                RecognitionLanguage::Auto => None,
                RecognitionLanguage::Zh => Some("zh"),
                RecognitionLanguage::En => Some("en"),
            },
            self.vocabulary
                .as_ref()
                .map(|s| s.parsed.terms.as_slice())
                .unwrap_or(&[]),
            self.context.as_ref().map(|s| s.parsed.text.as_str()),
        )
    }
    pub(crate) fn preparation(&self, rules: &LocalRules, case_id: &str) -> PreparationFixture {
        let row = self
            .local
            .as_ref()
            .and_then(|s| s.parsed.cases.iter().find(|c| c.case_id == case_id));
        PreparationFixture {
            dictionary: row.map(|r| r.dictionary.clone()).unwrap_or_default(),
            pairs: if rules.dictionary_replacements {
                row.map(|r| {
                    r.replacements
                        .iter()
                        .map(|p| crate::lexicon::LexiconPair::new(&p.before, &p.after))
                        .collect()
                })
                .unwrap_or_default()
            } else {
                vec![]
            },
            fuzzy: rules.fuzzy,
            spoken_layout: rules.spoken_layout,
        }
    }
    pub(crate) fn identities(&self) -> std::collections::BTreeMap<String, ControlInputIdentity> {
        let mut result = std::collections::BTreeMap::new();
        macro_rules! add {
            ($field:ident) => {
                if let Some(snapshot) = &self.$field {
                    result.insert(
                        stringify!($field).into(),
                        ControlInputIdentity {
                            relative_path: snapshot.relative_path.clone(),
                            canonical_path: snapshot.canonical_path.display().to_string(),
                            sha256: snapshot.sha256.clone(),
                            bytes: snapshot.raw.len() as u64,
                        },
                    );
                }
            };
        }
        add!(vocabulary);
        add!(context);
        add!(local);
        result
    }
}
