//! All input validation completes before credential resolution or provider construction.
use super::contracts::*;
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub const MAX_JSON_BYTES: u64 = 1024 * 1024;
pub const MAX_WAV_BYTES: u64 = 25 * 1024 * 1024;
pub const MAX_CORPUS_CASES: usize = 1000;
pub const MAX_TOTAL_AUDIO_BYTES: u64 = 512 * 1024 * 1024;
#[derive(Debug, Serialize, thiserror::Error)]
#[error("{code}: {safe_detail} ({artifact_path})")]
pub struct InputError {
    pub code: &'static str,
    pub artifact_path: String,
    pub case_id: Option<String>,
    pub safe_detail: String,
    pub provider_attempts: u64,
}
pub fn error(path: &Path, code: &'static str, detail: impl Into<String>) -> InputError {
    InputError {
        code,
        artifact_path: path.display().to_string(),
        case_id: None,
        safe_detail: detail.into(),
        provider_attempts: 0,
    }
}
pub fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub fn bounded(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit
}
pub fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}
pub fn external(path: &Path) -> Result<PathBuf, InputError> {
    let canonical = path
        .canonicalize()
        .map_err(|_| error(path, "path_unavailable", "existing external path required"))?;
    let repo = repository().canonicalize().map_err(|_| {
        error(
            path,
            "identity_unavailable",
            "repository identity unavailable",
        )
    })?;
    if canonical.starts_with(repo) {
        return Err(error(
            path,
            "path_escape",
            "Keep audio, manifests and reports outside the repository.",
        ));
    }
    Ok(canonical)
}
pub fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, InputError> {
    let file = std::fs::File::open(path)
        .map_err(|_| error(path, "input_unavailable", "cannot read selected file"))?;
    let meta = file
        .metadata()
        .map_err(|_| error(path, "input_unavailable", "cannot inspect selected file"))?;
    if !meta.is_file() || meta.len() > limit {
        return Err(error(
            path,
            "input_budget",
            "selected file exceeds the visible size budget",
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| error(path, "input_unavailable", "cannot read selected file"))?;
    if bytes.len() as u64 > limit {
        return Err(error(
            path,
            "input_budget",
            "selected file grew beyond the size budget",
        ));
    }
    Ok(bytes)
}
pub fn json<T: DeserializeOwned>(path: &Path) -> Result<(T, Vec<u8>), InputError> {
    let bytes = read_bounded(path, MAX_JSON_BYTES)?;
    let value = decode_json(path, &bytes)?;
    Ok((value, bytes))
}
pub(super) fn decode_json<T: DeserializeOwned>(path: &Path, bytes: &[u8]) -> Result<T, InputError> {
    serde_json::from_slice(bytes).map_err(|e| {
        error(
            path,
            if e.to_string().contains("unknown field") {
                "unknown_field"
            } else {
                "schema"
            },
            format!(
                "strict JSON contract rejected at line {}, column {}",
                e.line(),
                e.column()
            ),
        )
    })
}
fn check(path: &Path, condition: bool, code: &'static str, detail: &str) -> Result<(), InputError> {
    if condition {
        Ok(())
    } else {
        Err(error(path, code, detail))
    }
}
fn adjacent_name(name: &str) -> bool {
    !name.contains(['/', '\\'])
        && Path::new(name).components().count() == 1
        && Path::new(name).extension().and_then(|s| s.to_str()) == Some("wav")
}
pub fn adjacent(path: &Path, name: &str) -> Result<PathBuf, InputError> {
    check(
        path,
        adjacent_name(name),
        "path_escape",
        "Audio must be an adjacent .wav filename.",
    )?;
    let audio = external(&path.parent().unwrap().join(name))?;
    check(
        path,
        audio.parent() == path.parent(),
        "path_escape",
        "Audio symlinks must remain adjacent to the manifest.",
    )?;
    Ok(audio)
}
pub(super) fn relative_file(owner: &Path, name: &str) -> Result<PathBuf, InputError> {
    let relative = Path::new(name);
    check(
        owner,
        bounded(name, 4096)
            && relative
                .components()
                .all(|c| matches!(c, Component::Normal(_))),
        "path_escape",
        "use a bounded relative path without traversal",
    )?;
    let root = owner.parent().unwrap();
    let file = external(&root.join(relative))?;
    check(
        owner,
        file.starts_with(root),
        "path_escape",
        "declared relative path escapes its artifact directory",
    )?;
    Ok(file)
}
// Format gate adapted from Handy v0.9.8 run_headless_transcription; notice in HANDY-NOTICE.txt.
// RIFF topology is checked before hound reads every PCM sample, including the tail.
pub fn wav(path: &Path, bytes: &[u8]) -> Result<AudioIdentity, InputError> {
    check(
        path,
        bytes.len() > 44 && bytes.len() as u64 <= MAX_WAV_BYTES,
        "audio_format",
        "WAV must be nonempty and at most 25 MiB.",
    )?;
    check(
        path,
        &bytes[0..4] == b"RIFF"
            && &bytes[8..12] == b"WAVE"
            && u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize + 8 == bytes.len(),
        "audio_format",
        "RIFF size must match the complete WAV file",
    )?;
    let mut offset = 12usize;
    let mut fmt = 0;
    let mut data = 0;
    while offset < bytes.len() {
        check(
            path,
            offset + 8 <= bytes.len(),
            "audio_format",
            "truncated WAV chunk header",
        )?;
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let end = offset
            .checked_add(8)
            .and_then(|v| v.checked_add(size))
            .and_then(|v| v.checked_add(size % 2))
            .ok_or_else(|| error(path, "audio_format", "WAV chunk size overflow"))?;
        check(
            path,
            end <= bytes.len(),
            "audio_format",
            "WAV contains a truncated chunk.",
        )?;
        match &bytes[offset..offset + 4] {
            b"fmt " => fmt += 1,
            b"data" => data += 1,
            _ => (),
        }
        offset = end;
    }
    check(
        path,
        fmt == 1 && data == 1,
        "audio_format",
        "WAV must contain one format and one data chunk",
    )?;
    let mut reader = hound::WavReader::new(std::io::Cursor::new(bytes))
        .map_err(|_| error(path, "audio_format", "invalid WAV encoding"))?;
    let spec = reader.spec();
    check(
        path,
        spec.sample_rate == 16_000
            && spec.channels == 1
            && spec.bits_per_sample == 16
            && spec.sample_format == hound::SampleFormat::Int,
        "audio_format",
        "Use 16 kHz mono 16-bit PCM WAV, matching dictation audio.",
    )?;
    let mut pcm = Sha256::new();
    let mut count = 0u64;
    for sample in reader.samples::<i16>() {
        pcm.update(
            sample
                .map_err(|_| error(path, "audio_format", "corrupt PCM sample"))?
                .to_le_bytes(),
        );
        count += 1;
    }
    check(
        path,
        count > 0 && count <= 16_000 * 600,
        "audio_format",
        "WAV must contain audio of at most 10 minutes.",
    )?;
    Ok(AudioIdentity {
        audio_sha256: sha256(bytes),
        pcm_sha256: hex::encode(pcm.finalize()),
        sample_count: count,
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        bits_per_sample: spec.bits_per_sample,
    })
}
pub fn provenance(path: &Path, p: &Provenance) -> Result<(), InputError> {
    check(
        path,
        matches!(p.kind.as_str(), "human_recorded" | "synthetic")
            && p.reviewed
            && bounded(&p.note, 4096),
        "rights_not_reviewed",
        "Record audio provenance, review the fixtures and set reviewed: true.",
    )
}
pub fn manifest_fields(path: &Path, m: &AudioManifestV1) -> Result<(), InputError> {
    check(
        path,
        m.schema_version == 1,
        "schema",
        "use legacy audio manifest schema_version 1",
    )?;
    provenance(path, &m.provenance)?;
    check(
        path,
        !m.cases.is_empty() && m.cases.len() <= 200,
        "input_budget",
        "Select 1–200 audio cases.",
    )?;
    let mut ids = BTreeSet::new();
    for c in &m.cases {
        check(
            path,
            bounded(&c.id, 128) && ids.insert(&c.id),
            "duplicate_id",
            "Case IDs must be unique and nonempty.",
        )?;
        check(
            path,
            adjacent_name(&c.audio),
            "path_escape",
            "Audio must be an adjacent .wav filename.",
        )?;
        check(
            path,
            [&c.reference, &c.expected_final, &c.scenario]
                .iter()
                .all(|t| bounded(t, 16_384))
                && c.protected.len() <= 128
                && c.reference_variants.len() <= 16
                && c.protected
                    .iter()
                    .chain(&c.reference_variants)
                    .all(|t| bounded(t, 16_384)),
            "schema",
            "Review bounded references, final text, scenarios and protected values.",
        )?;
        check(
            path,
            crate::context::builtin_family_for_id(&c.family).is_some()
                && matches!(
                    c.focus_kind.as_str(),
                    "secure"
                        | "search"
                        | "code"
                        | "coding_prompt"
                        | "terminal"
                        | "email"
                        | "chat"
                        | "document"
                        | "form"
                        | "editable"
                        | "unknown"
                )
                && matches!(
                    c.cleanup.as_str(),
                    "auto" | "off" | "light" | "standard" | "heavy"
                ),
            "schema",
            "Unsupported family, focus kind or cleanup intensity.",
        )?;
    }
    Ok(())
}
#[derive(Debug, Clone)]
pub enum CaseData {
    Speech(AudioCaseV1),
    NonSpeech(NonSpeechCaseV1),
    AsrSpeech(Box<super::speech_input::AdmittedCase>),
}
impl CaseData {
    pub fn id(&self) -> &str {
        match self {
            Self::Speech(c) => &c.id,
            Self::NonSpeech(c) => &c.case_id,
            Self::AsrSpeech(c) => &c.case.case_id,
        }
    }
    pub fn audio(&self) -> &str {
        match self {
            Self::Speech(c) => &c.audio,
            Self::NonSpeech(c) => &c.adjacent_wav,
            Self::AsrSpeech(c) => &c.case.adjacent_wav,
        }
    }
    pub fn speech(&self) -> Option<&AudioCaseV1> {
        if let Self::Speech(c) = self {
            Some(c)
        } else {
            None
        }
    }
    pub fn asr_speech(&self) -> Option<&super::speech_input::AdmittedCase> {
        if let Self::AsrSpeech(c) = self {
            Some(c)
        } else {
            None
        }
    }
    pub fn cleanup_applicable(&self) -> bool {
        matches!(self, Self::Speech(_))
    }
    pub fn reference(&self) -> Option<&str> {
        match self {
            Self::Speech(c) => Some(&c.reference),
            Self::AsrSpeech(c) => c.case.reference.text(),
            _ => None,
        }
    }
    pub fn negative(&self) -> bool {
        matches!(self, Self::NonSpeech(_))
    }
    /// Compatibility only: V1 stage hashes used this exact negative bridge.
    /// V2 never supplies a speech reference and is never a candidate cache.
    pub fn legacy(&self) -> Option<AudioCaseV1> {
        match self {
            Self::AsrSpeech(_) => None,
            Self::Speech(c) => Some(c.clone()),
            Self::NonSpeech(c) => Some(AudioCaseV1 {
                id: c.case_id.clone(),
                audio: c.adjacent_wav.clone(),
                reference: String::new(),
                expected_final: String::new(),
                scenario: format!("negative:{:?}", c.category),
                family: "general".into(),
                focus_kind: "unknown".into(),
                cleanup: "off".into(),
                protected: vec![],
                reference_variants: vec![],
            }),
        }
    }
}
#[derive(Debug, Clone)]
pub enum CaseDocument {
    AsrSpeech {
        selected: PathBuf,
        canonical: PathBuf,
        sha256: String,
        packet_sha256: String,
    },
    Speech {
        path: PathBuf,
        sha256: String,
    },
    NonSpeech {
        selected: PathBuf,
        canonical: PathBuf,
        sha256: String,
        provenance: Provenance,
    },
}
#[derive(Debug)]
pub struct ValidatedCase {
    pub corpus: Option<CorpusCaseV1>,
    pub bytes: Vec<u8>,
    pub case: CaseData,
    pub audio_path: PathBuf,
    pub audio: AudioIdentity,
    pub document: CaseDocument,
    pub source_kind: String,
}
impl ValidatedCase {
    pub fn negative(&self) -> bool {
        self.case.negative()
    }
    pub fn document_path(&self) -> &Path {
        match &self.document {
            CaseDocument::AsrSpeech { canonical, .. } => canonical,
            CaseDocument::Speech { path, .. } => path,
            CaseDocument::NonSpeech { canonical, .. } => canonical,
        }
    }
    pub fn legacy_manifest_sha(&self) -> &str {
        match &self.document {
            CaseDocument::AsrSpeech { sha256, .. } => sha256,
            CaseDocument::Speech { sha256, .. } => sha256,
            CaseDocument::NonSpeech { .. } => &self.audio.audio_sha256,
        }
    }
}
#[derive(Debug)]
pub struct NonSpeechSnapshot {
    pub selected_path: PathBuf,
    pub canonical_path: PathBuf,
    pub raw: Vec<u8>,
    pub parsed: NonSpeechIndexV1,
    pub sha256: String,
}
#[derive(Debug)]
pub enum InputSelection {
    AsrSpeech {
        snapshot: Box<super::speech_input::Snapshot>,
        negative: Option<NonSpeechSnapshot>,
    },
    Speech {
        manifests: Vec<ManifestSnapshot>,
        selected: usize,
        corpus_sha256: String,
        negative: Option<NonSpeechSnapshot>,
    },
    NonSpeech(NonSpeechSnapshot),
}
impl InputSelection {
    pub fn asr_only(&self) -> bool {
        !matches!(self, Self::Speech { .. })
    }
    pub fn schema_version(&self) -> u32 {
        match self {
            Self::Speech { .. } => 1,
            Self::NonSpeech(_) => 2,
            Self::AsrSpeech { .. } => 3,
        }
    }
    pub fn non_speech_only(&self) -> bool {
        matches!(self, Self::NonSpeech(_))
    }
    pub fn speech_manifest(&self) -> Option<&ManifestSnapshot> {
        if let Self::Speech {
            manifests,
            selected,
            ..
        } = self
        {
            manifests.get(*selected)
        } else {
            None
        }
    }
    pub fn corpus_sha(&self) -> Option<&str> {
        if let Self::Speech { corpus_sha256, .. } = self {
            Some(corpus_sha256)
        } else {
            None
        }
    }
    pub fn negative_sha(&self) -> Option<&str> {
        match self {
            Self::Speech { negative, .. } | Self::AsrSpeech { negative, .. } => {
                negative.as_ref().map(|s| s.sha256.as_str())
            }
            Self::NonSpeech(n) => Some(&n.sha256),
        }
    }
}
/// One bounded read supplies both parsed facts and their content identity.
#[derive(Debug)]
pub struct ManifestSnapshot {
    pub selected_path: PathBuf,
    pub canonical_path: PathBuf,
    pub raw: Vec<u8>,
    pub parsed: AudioManifestV1,
    pub sha256: String,
}
impl ManifestSnapshot {
    fn read(selected: &Path) -> Result<Self, InputError> {
        let canonical_path = external(selected)?;
        let (parsed, raw) = json(&canonical_path)?;
        Ok(Self {
            selected_path: selected.into(),
            canonical_path,
            sha256: sha256(&raw),
            raw,
            parsed,
        })
    }
}
#[derive(Debug)]
pub struct ValidatedInputs {
    pub scoring: super::scoring::ScoringInputs,
    pub control_fixtures: Vec<super::controls::JobFixtureInputs>,
    pub selection: InputSelection,
    pub cases: Vec<ValidatedCase>,
    pub recipe: Option<ExperimentRecipe>,
    pub recipe_sha256: String,
}
pub fn validate(
    manifest: &Path,
    sidecar: Option<&Path>,
    recipe: Option<&Path>,
    negative: Option<&Path>,
) -> Result<ValidatedInputs, InputError> {
    let selected_path = manifest.to_owned();
    let manifest = external(manifest)?;
    let mut cases = Vec::new();
    let mut ids = BTreeSet::new();
    let mut total = 0u64;
    let mut side = None;
    let mut corpus_bytes = Vec::new();
    let mut manifests = Vec::new();
    if let Some(path) = sidecar {
        let path = external(path)?;
        let (s, b): (CorpusSidecarV1, _) = json(&path)?;
        check(
            &path,
            s.artifact_type == "corpus_sidecar"
                && s.schema_version == 1
                && bounded(&s.corpus_id, 128),
            "schema",
            "use CorpusSidecarV1",
        )?;
        check(
            &path,
            !s.manifests.is_empty()
                && s.manifests.len() <= 20
                && !s.cases.is_empty()
                && s.cases.len() <= MAX_CORPUS_CASES,
            "input_budget",
            "sidecar limit: 20 manifests and 1000 cases",
        )?;
        let mut seen = BTreeSet::new();
        for entry in &s.manifests {
            let p = relative_file(&path, &entry.relative_path)?;
            check(
                &path,
                seen.insert(p.clone()),
                "duplicate_id",
                "duplicate manifest path",
            )?;
            let mut snapshot = ManifestSnapshot::read(&p)?;
            if p == manifest {
                snapshot.selected_path = selected_path.clone();
            }
            check(
                &path,
                digest(&entry.sha256) && snapshot.sha256 == entry.sha256,
                "sha_mismatch",
                "manifest digest mismatch",
            )?;
            manifests.push(snapshot);
        }
        check(
            &path,
            manifests.iter().any(|s| s.canonical_path == manifest),
            "identity_mismatch",
            "selected manifest must be declared by the corpus sidecar",
        )?;
        corpus_bytes = b;
        side = Some((path, s));
    }
    if side.is_none() {
        manifests.push(ManifestSnapshot::read(&selected_path)?);
    }
    let selected_manifest = manifests
        .iter()
        .position(|s| s.canonical_path == manifest)
        .ok_or_else(|| {
            error(
                &manifest,
                "identity_mismatch",
                "selected manifest snapshot unavailable",
            )
        })?;
    for snapshot in &manifests {
        let path = &snapshot.canonical_path;
        let m = &snapshot.parsed;
        manifest_fields(path, m)?;
        if side.is_none() {
            corpus_bytes = snapshot.raw.clone();
        }
        let hash = &snapshot.sha256;
        for case in &m.cases {
            check(
                path,
                ids.insert(case.id.clone()),
                "duplicate_id",
                "duplicate case ID across manifests",
            )?;
            let p = adjacent(path, &case.audio)?;
            let bytes = read_bounded(&p, MAX_WAV_BYTES)?;
            total += bytes.len() as u64;
            check(
                path,
                total <= MAX_TOTAL_AUDIO_BYTES && cases.len() < MAX_CORPUS_CASES,
                "input_budget",
                "total corpus limit: 1000 cases and 512 MiB audio",
            )?;
            let audio = wav(&p, &bytes)?;
            cases.push(ValidatedCase {
                corpus: None,
                bytes,
                case: CaseData::Speech(case.clone()),
                audio_path: p,
                audio,
                document: CaseDocument::Speech {
                    path: path.clone(),
                    sha256: hash.clone(),
                },
                source_kind: m.provenance.kind.clone(),
            });
        }
    }
    if let Some((path, s)) = &side {
        check(
            path,
            s.cases.len() == cases.len(),
            "identity_mismatch",
            "sidecar must cover every speech case exactly once",
        )?;
        let mut seen = BTreeSet::new();
        let mut groups: BTreeMap<(String, String), Split> = BTreeMap::new();
        for c in &s.cases {
            check(
                path,
                seen.insert(&c.case_id),
                "duplicate_id",
                "duplicate sidecar case ID",
            )?;
            let actual = cases
                .iter_mut()
                .find(|v| v.case.id() == c.case_id)
                .ok_or_else(|| {
                    error(
                        path,
                        "identity_mismatch",
                        "sidecar references an unknown case",
                    )
                })?;
            check(
                path,
                matches!(
                    (&c.source_kind, actual.source_kind.as_str()),
                    (SourceKind::HumanRecorded, "human_recorded")
                        | (SourceKind::Synthetic, "synthetic")
                ),
                "identity_mismatch",
                "sidecar source kind must match reviewed manifest provenance",
            )?;
            check(
                path,
                c.manifest_sha256 == actual.legacy_manifest_sha()
                    && c.audio_sha256 == actual.audio.audio_sha256
                    && c.pcm_sha256 == actual.audio.pcm_sha256
                    && c.reference_sha256
                        == sha256(
                            actual
                                .case
                                .speech()
                                .expect("speech sidecar case")
                                .reference
                                .as_bytes(),
                        ),
                "sha_mismatch",
                "sidecar manifest/audio/PCM/reference digest mismatch",
            )?;
            check(path,[&c.rights_review_id,&c.speaker_id,&c.source_session_id,&c.template_family_id,&c.annotation_revision].iter().all(|v|bounded(v,256)) && [&c.conditions.noise,&c.conditions.microphone,&c.conditions.speaking_style].iter().all(|v|bounded(v,128)),"rights_not_reviewed","bounded rights, speaker, session, template, conditions and annotation identities required")?;
            actual.corpus = Some(c.clone());
            for (kind, value) in [
                ("speaker", &c.speaker_id),
                ("session", &c.source_session_id),
                ("template", &c.template_family_id),
                ("pcm", &c.pcm_sha256),
            ] {
                let key = (kind.into(), value.clone());
                if let Some(split) = groups.insert(key, c.split.clone()) {
                    check(
                        path,
                        split == c.split,
                        "split_overlap",
                        "speaker/session/template/audio group crosses splits",
                    )?;
                }
            }
        }
    }
    let negative_snapshot = negative
        .map(|p| read_negative(p, &mut cases, &mut ids, &mut total))
        .transpose()?;
    let mut scoring = super::scoring::ScoringInputs::default();
    let mut control_fixtures = Vec::new();
    let mut recipe_hash = String::new();
    let recipe = if let Some(path) = recipe {
        let path = external(path)?;
        let (document, b): (RunRecipeV1, _) = json(&path)?;
        let r: ExperimentRecipe = document.into();
        check(
            &path,
            side.is_some(),
            "identity_mismatch",
            "--experiment requires explicit --corpus-sidecar",
        )?;
        check(
            &path,
            r.artifact_type == "run_recipe" && r.schema_version == 1 && bounded(&r.recipe_id, 128),
            "schema",
            "use RunRecipeV1",
        )?;
        check(
            &path,
            matches!(&r.input,RecipeInput::SpeechCorpus{sidecar_sha256} if digest(sidecar_sha256) && *sidecar_sha256==sha256(&corpus_bytes)),
            "sha_mismatch",
            "recipe corpus sidecar digest mismatch",
        )?;
        control_fixtures = validate_jobs(&path, &r, &cases)?;
        scoring = super::scoring::validate(&path, &r, &cases)?;
        recipe_hash = sha256(&b);
        Some(r)
    } else {
        check(
            &manifest,
            sidecar.is_none() && negative.is_none(),
            "schema",
            "sidecar/non-speech require --experiment",
        )?;
        None
    };
    Ok(ValidatedInputs {
        scoring,
        control_fixtures,
        selection: InputSelection::Speech {
            manifests,
            selected: selected_manifest,
            corpus_sha256: sha256(&corpus_bytes),
            negative: negative_snapshot,
        },
        cases,
        recipe,
        recipe_sha256: recipe_hash,
    })
}

fn validate_jobs(
    path: &Path,
    r: &ExperimentRecipe,
    cases: &[ValidatedCase],
) -> Result<Vec<super::controls::JobFixtureInputs>, InputError> {
    let mut control_fixtures = Vec::new();
    check(
        path,
        !r.jobs.is_empty() && r.jobs.len() <= 32,
        "input_budget",
        "recipe limit: 1–32 jobs",
    )?;
    for j in &r.jobs {
        check(
            path,
            bounded(&j.product, 128)
                && bounded(&j.candidate_id, 128)
                && bounded(&j.profile_state_id, 256)
                && bounded(&j.local_rules.revision, 256)
                && bounded(&j.artifact_identity.version, 128)
                && bounded(&j.artifact_identity.source_commit, 128)
                && j.artifact_identity
                    .binary_sha256
                    .as_ref()
                    .is_none_or(|v| digest(v)),
            "schema",
            "bounded job and artifact identity required",
        )?;
        check(
            path,
            j.repeats > 0
                && j.repeats <= 20
                && j.pacing.gap_ms <= 5000
                && j.run_budget.max_cases >= cases.len() as u32
                && j.run_budget.max_cases <= 1000
                && j.run_budget.max_requests > 0
                && j.run_budget.max_requests <= 10000
                && j.run_budget.max_retries <= 1
                && (1..=3_600_000).contains(&j.run_budget.deadline_ms),
            "input_budget",
            "bounded repeats, pacing, cases, requests, retries and deadline required",
        )?;
        control_fixtures.push(super::controls::validate(path, j, cases)?);
        let control_bytes: usize = control_fixtures
            .iter()
            .map(|f| {
                f.vocabulary.as_ref().map(|s| s.raw.len()).unwrap_or(0)
                    + f.context.as_ref().map(|s| s.raw.len()).unwrap_or(0)
                    + f.local.as_ref().map(|s| s.raw.len()).unwrap_or(0)
            })
            .sum();
        check(
            path,
            control_bytes <= 4 * 1024 * 1024,
            "input_budget",
            "aggregate control fixtures exceed 4 MiB",
        )?;
    }
    Ok(control_fixtures)
}
fn read_negative(
    selected: &Path,
    cases: &mut Vec<ValidatedCase>,
    ids: &mut BTreeSet<String>,
    total: &mut u64,
) -> Result<NonSpeechSnapshot, InputError> {
    let path = external(selected)?;
    let (n, raw): (NonSpeechIndexV1, _) = json(&path)?;
    check(
        &path,
        n.artifact_type == "non_speech_index" && n.schema_version == 1,
        "schema",
        "use NonSpeechIndexV1",
    )?;
    provenance(&path, &n.provenance)?;
    check(
        &path,
        !n.cases.is_empty() && n.cases.len() <= 200,
        "input_budget",
        "negative index limit: 1–200 cases",
    )?;
    let index_sha = sha256(&raw);
    for c in &n.cases {
        check(
            &path,
            bounded(&c.case_id, 128) && ids.insert(c.case_id.clone()),
            "duplicate_id",
            "negative case IDs must be globally unique",
        )?;
        check(
            &path,
            bounded(&c.source_review_id, 256),
            "rights_not_reviewed",
            "negative source review required",
        )?;
        let audio_path = adjacent(&path, &c.adjacent_wav)?;
        let bytes = read_bounded(&audio_path, MAX_WAV_BYTES)?;
        *total += bytes.len() as u64;
        check(
            &path,
            *total <= MAX_TOTAL_AUDIO_BYTES && cases.len() < MAX_CORPUS_CASES,
            "input_budget",
            "total corpus limit: 1000 cases and 512 MiB audio",
        )?;
        let audio = wav(&audio_path, &bytes)?;
        check(
            &path,
            digest(&c.audio_sha256) && c.audio_sha256 == audio.audio_sha256,
            "sha_mismatch",
            "negative audio digest mismatch",
        )?;
        cases.push(ValidatedCase {
            corpus: None,
            bytes,
            case: CaseData::NonSpeech(c.clone()),
            audio_path,
            audio,
            document: CaseDocument::NonSpeech {
                selected: selected.into(),
                canonical: path.clone(),
                sha256: index_sha.clone(),
                provenance: n.provenance.clone(),
            },
            source_kind: n.provenance.kind.clone(),
        });
    }
    Ok(NonSpeechSnapshot {
        selected_path: selected.into(),
        canonical_path: path,
        raw,
        parsed: n,
        sha256: index_sha,
    })
}
/// V1 entry remains strict speech-only; this selector adds an explicit V2 path.
pub fn validate_selection(
    manifest: Option<&Path>,
    sidecar: Option<&Path>,
    recipe: Option<&Path>,
    negative: Option<&Path>,
) -> Result<ValidatedInputs, InputError> {
    if let Some(manifest) = manifest {
        return validate(manifest, sidecar, recipe, negative);
    }
    let recipe = recipe.ok_or_else(|| {
        error(
            Path::new("arguments"),
            "missing_argument",
            "select --manifest, or explicit V2 --experiment and --non-speech",
        )
    })?;
    check(
        recipe,
        sidecar.is_none(),
        "conflicting_arguments",
        "negative-only selection cannot use --corpus-sidecar",
    )?;
    let path = external(recipe)?;
    let raw = read_bounded(&path, MAX_JSON_BYTES)?;
    // Version selection never replaces strict parsing of the same original bytes.
    #[derive(serde::Deserialize)]
    struct Version {
        schema_version: u32,
    }
    let version: Version = decode_json(&path, &raw)?;
    if version.schema_version == 3 {
        return validate_asr_speech(&path, &raw, negative);
    }
    let document: RunRecipeV2 = decode_json(&path, &raw)?;
    check(
        &path,
        document.artifact_type == "run_recipe"
            && document.schema_version == 2
            && bounded(&document.recipe_id, 128),
        "schema",
        "strict RunRecipeV2 required without a speech manifest",
    )?;
    let negative = negative.ok_or_else(|| {
        error(
            &path,
            "missing_argument",
            "RunRecipeV2 requires --non-speech",
        )
    })?;
    let mut cases = Vec::new();
    let snapshot = read_negative(negative, &mut cases, &mut BTreeSet::new(), &mut 0)?;
    let ExperimentInputV2::NonSpeech { index_sha256 } = &document.input;
    check(
        &path,
        digest(index_sha256) && *index_sha256 == snapshot.sha256,
        "sha_mismatch",
        "recipe non-speech index digest mismatch",
    )?;
    let r: ExperimentRecipe = document.into();
    check(
        &path,
        r.track == Track::FileAdapter && r.gui_fixture.is_none(),
        "unsupported_track",
        "negative-only execution is file-adapter only",
    )?;
    check(
        &path,
        r.cache_policy
            .as_ref()
            .is_none_or(|p| p.purpose != CachePurpose::QualityReplay && p.max_age_ms.is_none()),
        "resume_not_applicable",
        "non-speech measurements require fresh execution",
    )?;
    check(
        &path,
        r.scoring
            .as_ref()
            .is_none_or(|s| s.annotations.is_none() && s.comparison.is_none()),
        "not_applicable",
        "speech scoring annotations/comparisons do not apply to negative-only inputs",
    )?;
    for job in &r.jobs {
        check(
            &path,
            job.cleanup.intensity == CleanupIntensity::Off
                && job.cleanup.candidate_id.is_none()
                && !job.cleanup.capture_provider_candidate,
            "not_applicable",
            "negative-only requires cleanupOff with no candidate or reply capture",
        )?;
        check(
            &path,
            !job.local_rules.dictionary_replacements
                && !job.local_rules.fuzzy
                && !job.local_rules.spoken_layout
                && job.local_rules.fixture.is_none(),
            "not_applicable",
            "negative-only has no local postprocessing stage",
        )?;
    }
    let control_fixtures = validate_jobs(&path, &r, &cases)?;
    let scoring = super::scoring::validate(&path, &r, &cases)?;
    Ok(ValidatedInputs {
        selection: InputSelection::NonSpeech(snapshot),
        scoring,
        control_fixtures,
        cases,
        recipe: Some(r),
        recipe_sha256: sha256(&raw),
    })
}

fn validate_asr_speech(
    path: &Path,
    raw: &[u8],
    negative: Option<&Path>,
) -> Result<ValidatedInputs, InputError> {
    let document: RunRecipeV3 = decode_json(path, raw)?;
    check(
        path,
        document.artifact_type == "run_recipe"
            && document.schema_version == 3
            && bounded(&document.recipe_id, 128),
        "schema",
        "strict RunRecipeV3 required",
    )?;
    let ExperimentInputV3::AsrSpeech {
        purpose,
        speech_index,
        non_speech_sha256,
    } = &document.input;
    let mut cases = Vec::new();
    let mut ids = BTreeSet::new();
    let mut total = 0;
    let selected = relative_file(path, &speech_index.relative_path)?;
    let snapshot = super::speech_input::read(
        &selected,
        &speech_index.sha256,
        *purpose,
        &mut cases,
        &mut ids,
        &mut total,
    )?;
    check(
        path,
        negative.is_some() == non_speech_sha256.is_some(),
        "conflicting_arguments",
        "V3 optional non-speech input must be explicitly selected and SHA-bound",
    )?;
    let negative = negative
        .map(|p| read_negative(p, &mut cases, &mut ids, &mut total))
        .transpose()?;
    check(
        path,
        negative
            .as_ref()
            .is_none_or(|n| Some(&n.sha256) == non_speech_sha256.as_ref()),
        "sha_mismatch",
        "V3 negative index SHA mismatch",
    )?;
    let purpose = *purpose;
    let r: ExperimentRecipe = document.into();
    check(
        path,
        r.track == Track::FileAdapter && r.gui_fixture.is_none(),
        "unsupported_track",
        "ASR-only speech supports file_adapter only",
    )?;
    check(
        path,
        r.cache_policy.as_ref().is_none_or(|p| {
            if p.purpose == CachePurpose::QualityReplay {
                p.max_age_ms.is_some_and(|age| age > 0)
                    && r.jobs.iter().all(|j| super::local_asr::model_id(&j.candidate_id).is_some())
            } else { p.max_age_ms.is_none() }
        }),
        "resume_not_applicable",
        "V3 replay requires explicit positive TTL and only local ASR jobs with measured identity; fresh/cloud scopes retain fresh policy",
    )?;
    check(path,match(purpose,&r.scoring){(super::speech_input::Purpose::FunctionalUnscored,None)=>true,(super::speech_input::Purpose::PublisherReferenceAsr,Some(c))=>c.method==super::scoring::METHOD&&c.annotations.is_none()&&c.comparison.is_none(),_=>false},"not_applicable","functional V3 requires scoring:null; publisher V3 requires the versioned ASR method without annotations/comparisons")?;
    for j in &r.jobs {
        check(
            path,
            j.cleanup.intensity == CleanupIntensity::Off
                && j.cleanup.candidate_id.is_none()
                && !j.cleanup.capture_provider_candidate
                && !j.local_rules.dictionary_replacements
                && !j.local_rules.fuzzy
                && !j.local_rules.spoken_layout
                && j.local_rules.fixture.is_none(),
            "not_applicable",
            "V3 ASR-only requires cleanupOff and no local postprocessing/capture/candidate",
        )?;
    }
    let control_fixtures = validate_jobs(path, &r, &cases)?;
    let scoring = super::scoring::validate(path, &r, &cases)?;
    Ok(ValidatedInputs {
        selection: InputSelection::AsrSpeech {
            snapshot: Box::new(snapshot),
            negative,
        },
        scoring,
        control_fixtures,
        cases,
        recipe: Some(r),
        recipe_sha256: sha256(raw),
    })
}
