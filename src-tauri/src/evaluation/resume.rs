//! Explicit, bounded SHA candidate replay. No final text, implicit cache or legacy FNV admission.
use super::{
    artifacts::AtomicArtifact,
    contracts::*,
    validation::{self, error, sha256, InputError, ValidatedInputs},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};
const SCOPE: &str = "same_request_text_candidate_server_weights_unknown";
const LOCAL_SCOPE: &str = "same_measured_local_model_runtime_request_observation";
const TEXT_LIMIT: usize = 256 * 1024;
type Key = (String, String, u32, CacheStage);
fn key(case: &str, job: &str, repeat: u32, stage: CacheStage) -> Key {
    (case.into(), job.into(), repeat, stage)
}
pub(super) fn now() -> Result<u64, InputError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| d.as_millis().try_into().ok())
        .ok_or_else(|| {
            error(
                Path::new("clock"),
                "time_identity_unavailable",
                "checkpoint freshness requires a valid UTC clock",
            )
        })
}
pub(super) fn hash<T: Serialize>(value: &T) -> String {
    sha256(&serde_json::to_vec(value).expect("serialize typed evaluation identity"))
}
// Source snapshots describe provenance. The independently observed executable
// binds Rust code; loaded sidecar/model/request identities retain their existing
// fresh-only/complete-identity guards below. Live checkout/lock observations must
// never relabel that executable or make a same-artifact request a different code.
#[derive(Serialize)]
struct ExecutionBaseline<'a> {
    executable_sha256: &'a str,
    version: &'a str,
    os: &'a str,
    architecture: &'a str,
    os_version: &'a Option<String>,
    os_build: &'a Option<String>,
    cpu: &'a Option<String>,
    ram_bytes: Option<u64>,
}
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ExecutionIdentity<'a> {
    Speech {
        baseline: ExecutionBaseline<'a>,
        corpus_sha256: &'a str,
        recipe_sha256: &'a str,
        non_speech_sha256: &'a Option<String>,
        local_cleanup_manifest_sha256: &'a Option<String>,
    },
    NonSpeech {
        baseline: ExecutionBaseline<'a>,
        recipe_sha256: &'a str,
        input: &'a ExperimentInputV2,
    },
    AsrSpeech {
        baseline: ExecutionBaseline<'a>,
        recipe_sha256: &'a str,
        input: &'a super::speech_input::InputIdentity,
    },
}
fn execution_baseline(b: &BaselineIdentity) -> ExecutionBaseline<'_> {
    ExecutionBaseline {
        executable_sha256: &b.executable_sha256,
        version: &b.version,
        os: &b.os,
        architecture: &b.architecture,
        os_version: &b.os_version,
        os_build: &b.os_build,
        cpu: &b.cpu,
        ram_bytes: b.ram_bytes,
    }
}
// Checkpoints store the strict speech member, while current runs wrap it in the
// multi-input enum. Both boundaries borrow the same execution projection.
fn speech_execution_identity(i: &RunInputIdentityV1) -> ExecutionIdentity<'_> {
    ExecutionIdentity::Speech {
        baseline: execution_baseline(&i.baseline),
        corpus_sha256: &i.corpus_sha256,
        recipe_sha256: &i.recipe_sha256,
        non_speech_sha256: &i.non_speech_sha256,
        local_cleanup_manifest_sha256: &i.local_cleanup_manifest_sha256,
    }
}
fn execution_identity(identity: &RunInputIdentity) -> ExecutionIdentity<'_> {
    match identity {
        RunInputIdentity::Speech(i) => speech_execution_identity(i),
        RunInputIdentity::NonSpeech(i) => ExecutionIdentity::NonSpeech {
            baseline: execution_baseline(&i.baseline),
            recipe_sha256: &i.recipe_sha256,
            input: &i.input,
        },
        RunInputIdentity::AsrSpeech(i) => ExecutionIdentity::AsrSpeech {
            baseline: execution_baseline(&i.baseline),
            recipe_sha256: &i.recipe_sha256,
            input: &i.input,
        },
    }
}
pub(super) fn policy(inputs: &ValidatedInputs) -> Option<&CachePolicy> {
    inputs.recipe.as_ref()?.cache_policy.as_ref()
}
pub(super) fn replay_enabled(inputs: &ValidatedInputs) -> bool {
    policy(inputs).is_some_and(|p| p.purpose == CachePurpose::QualityReplay)
}
pub(super) fn validate_policy(inputs: &ValidatedInputs, path: &Path) -> Result<(), InputError> {
    if let Some(p) = policy(inputs) {
        let valid = if p.purpose == CachePurpose::QualityReplay {
            p.max_age_ms
                .is_some_and(|age| age > 0 && now().ok().and_then(|n| n.checked_add(age)).is_some())
                && inputs
                    .recipe
                    .as_ref()
                    .is_some_and(|r| r.track == Track::FileAdapter && r.gui_fixture.is_none())
        } else {
            p.max_age_ms.is_none()
        };
        if !valid {
            return Err(error(path, "cache_policy_not_applicable", "quality_replay requires an explicit positive TTL and file-adapter-only track; fresh purposes cannot set a cache TTL"));
        }
    }
    Ok(())
}
// V1 stays byte/schema compatible. V2 is the explicit local ASR-only wire;
// one normalized checkpoint owner handles TTL, stages and atomic persistence.
#[derive(Debug, Clone, serde::Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LocalReplayIdentity {
    model_id: String,
    revision: String,
    files: Vec<super::local_asr::LocalPinnedFile>,
    production_marker_sha256: String,
    derived_tokenizer_recipe: String,
    derived_tokenizer_sha256: String,
    loaded: crate::observations::LoadedAttestation,
}
impl LocalReplayIdentity {
    fn from_row(row: &CaseRecordV1) -> Result<Self, &'static str> {
        let model = row
            .local_model
            .as_ref()
            .ok_or("local_model_identity_missing")?;
        let runtime = row
            .local_runtime
            .as_ref()
            .ok_or("local_runtime_observation_missing")?;
        let loaded = runtime
            .loaded_attestation
            .as_ref()
            .ok_or("local_loaded_attestation_missing")?;
        if !loaded.complete() {
            return Err("local_loaded_attestation_incomplete");
        }
        if runtime.events_truncated {
            return Err("local_runtime_observation_truncated");
        }
        if runtime.observed_model_id.as_deref() != Some(model.model_id.as_str())
            || runtime.observed_revision.as_deref() != Some(model.revision.as_str())
        {
            return Err("local_loaded_model_identity_unresolved");
        }
        if !matches!(model.derived_tokenizer_state, StageState::Completed {}) {
            return Err("local_derived_tokenizer_unmeasured");
        }
        let value = Self {
            model_id: model.model_id.clone(),
            revision: model.revision.clone(),
            files: model.files.clone(),
            production_marker_sha256: model.production_marker_sha256.clone(),
            derived_tokenizer_recipe: model.derived_tokenizer_recipe.clone(),
            derived_tokenizer_sha256: model
                .derived_tokenizer_sha256
                .clone()
                .ok_or("local_derived_tokenizer_missing")?,
            loaded: loaded.clone(),
        };
        if value.complete() {
            Ok(value)
        } else {
            Err("local_model_identity_incomplete")
        }
    }
    fn complete(&self) -> bool {
        self.loaded.complete()
            && !self.files.is_empty()
            && self.files.len() <= 64
            && validation::digest(&self.production_marker_sha256)
            && validation::digest(&self.derived_tokenizer_sha256)
            && !self.derived_tokenizer_recipe.is_empty()
            && self
                .files
                .iter()
                .all(|f| f.bytes > 0 && validation::digest(&f.sha256))
            && crate::ondevice_models::mlx_model(&self.model_id)
                .is_some_and(|m| m.revision == self.revision)
    }
}
#[derive(Debug, Clone, serde::Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LocalStageSource {
    case_id: String,
    job_id: String,
    repeat: u32,
    identity: LocalReplayIdentity,
    observation: crate::observations::LocalAsrObservation,
}
#[derive(Debug, Clone, serde::Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CheckpointV2 {
    artifact_type: String,
    schema_version: u32,
    created_unix_ms: u64,
    identity: Box<RunInputIdentityV3>,
    successful_stage_candidates: Vec<StageCandidate>,
    local_sources: Vec<LocalStageSource>,
    complete_jobs: Vec<String>,
    terminal: StageState,
    provider_attempts: u64,
}
#[derive(Clone)]
struct CheckpointData {
    artifact_type: String,
    schema_version: u32,
    created_unix_ms: u64,
    identity: RunInputIdentity,
    successful_stage_candidates: Vec<StageCandidate>,
    local_sources: Vec<LocalStageSource>,
    complete_jobs: Vec<String>,
    terminal: StageState,
    provider_attempts: u64,
}
impl From<CheckpointV1> for CheckpointData {
    fn from(c: CheckpointV1) -> Self {
        Self {
            artifact_type: c.artifact_type,
            schema_version: c.schema_version,
            created_unix_ms: c.created_unix_ms,
            identity: c.identity.into(),
            successful_stage_candidates: c.successful_stage_candidates,
            local_sources: vec![],
            complete_jobs: c.complete_jobs,
            terminal: c.terminal,
            provider_attempts: c.provider_attempts,
        }
    }
}
impl From<CheckpointV2> for CheckpointData {
    fn from(c: CheckpointV2) -> Self {
        Self {
            artifact_type: c.artifact_type,
            schema_version: c.schema_version,
            created_unix_ms: c.created_unix_ms,
            identity: RunInputIdentity::AsrSpeech(c.identity),
            successful_stage_candidates: c.successful_stage_candidates,
            local_sources: c.local_sources,
            complete_jobs: c.complete_jobs,
            terminal: c.terminal,
            provider_attempts: c.provider_attempts,
        }
    }
}
impl Serialize for CheckpointData {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let local = matches!(self.identity, RunInputIdentity::AsrSpeech(_));
        if matches!(self.identity, RunInputIdentity::NonSpeech(_)) {
            return Err(serde::ser::Error::custom(
                "negative-only checkpoint is not applicable",
            ));
        }
        let mut wire = serializer.serialize_struct("Checkpoint", if local { 9 } else { 8 })?;
        wire.serialize_field("artifact_type", &self.artifact_type)?;
        wire.serialize_field("schema_version", &(if local { 2u32 } else { 1u32 }))?;
        wire.serialize_field("created_unix_ms", &self.created_unix_ms)?;
        wire.serialize_field("identity", &self.identity)?;
        wire.serialize_field(
            "successful_stage_candidates",
            &self.successful_stage_candidates,
        )?;
        if local {
            wire.serialize_field("local_sources", &self.local_sources)?;
        }
        wire.serialize_field("complete_jobs", &self.complete_jobs)?;
        wire.serialize_field("terminal", &self.terminal)?;
        wire.serialize_field("provider_attempts", &self.provider_attempts)?;
        wire.end()
    }
}
pub(super) struct Admission {
    loaded: Option<CheckpointData>,
    input_sha: Option<String>,
    pub output: Option<PathBuf>,
}
impl Admission {
    pub fn read(
        inputs: &ValidatedInputs,
        input: Option<&Path>,
        output: Option<&Path>,
    ) -> Result<Self, InputError> {
        validate_policy(inputs, Path::new("recipe"))?;
        if matches!(inputs.selection, validation::InputSelection::NonSpeech(_))
            && (input.is_some() || output.is_some())
        {
            return Err(error(
                Path::new("arguments"),
                "resume_not_applicable",
                "negative-only measurements require fresh execution without resume/checkpoint",
            ));
        }
        if (input.is_some() || output.is_some()) && !replay_enabled(inputs) {
            return Err(error(Path::new("arguments"), "cache_policy_not_applicable", "--resume/--checkpoint require explicit recipe cache_policy quality_replay with TTL; reliability/cloud-anchor/performance stay fresh"));
        }
        let output = output.map(super::runner::output_path).transpose()?;
        let (loaded, input_sha) = if let Some(selected) = input {
            let path = validation::external(selected)?;
            let raw = validation::read_bounded(&path, super::artifacts::MAX_ARTIFACT_BYTES as u64)?;
            #[derive(serde::Deserialize)]
            struct Version {
                schema_version: u32,
            }
            let decoded = (|| {
                let version: Version = validation::decode_json(&path, &raw)?;
                match version.schema_version {
                    1 => validation::decode_json::<CheckpointV1>(&path, &raw)
                        .map(CheckpointData::from),
                    2 => validation::decode_json::<CheckpointV2>(&path, &raw)
                        .map(CheckpointData::from),
                    _ => Err(error(
                        &path,
                        "checkpoint_version",
                        "strict CheckpointV1 or local CheckpointV2 required",
                    )),
                }
            })();
            let checkpoint = match decoded {
                Ok(checkpoint) => checkpoint,
                Err(cause) => {
                    // Legacy diagnostic uses the same captured bytes for every
                    // schema failure, including historically versioned FNV data.
                    #[derive(serde::Deserialize)]
                    struct Legacy {
                        corpus_fingerprint: String,
                        cleanup_code_fingerprint: String,
                    }
                    if serde_json::from_slice::<Legacy>(&raw).is_ok_and(|l| {
                        !l.corpus_fingerprint.is_empty() && !l.cleanup_code_fingerprint.is_empty()
                    }) {
                        return Err(error(
                            &path,
                            "legacy_identity_incomplete",
                            "legacy FNV checkpoint is display-only and cannot authorize SHA replay",
                        ));
                    }
                    return Err(cause);
                }
            };
            validate_data(&path, &checkpoint, inputs, now()?)?;
            (Some(checkpoint), Some(sha256(&raw)))
        } else {
            (None, None)
        };
        Ok(Self {
            loaded,
            input_sha,
            output,
        })
    }
    pub fn bind(
        self,
        inputs: &ValidatedInputs,
        identity: &RunInputIdentity,
    ) -> Result<Arc<ReplayPlan>, InputError> {
        if matches!(identity, RunInputIdentity::AsrSpeech(_)) && replay_enabled(inputs) {
            validate_local_platform(identity.baseline())?;
        }
        if let Some(cp) = &self.loaded {
            if hash(&execution_identity(&cp.identity)) != hash(&execution_identity(identity)) {
                return Err(error(Path::new("checkpoint"), "resume_identity_mismatch", "corpus/recipe/executable/runtime identity changed; checkpoint preserved, no requests made"));
            }
        }
        let (entries, local_sources): (
            BTreeMap<Key, Arc<StageCandidate>>,
            BTreeMap<Key, LocalStageSource>,
        ) = self
            .loaded
            .map(|c| {
                (
                    c.successful_stage_candidates
                        .into_iter()
                        .map(|s| (key(&s.case_id, &s.job_id, s.repeat, s.stage), Arc::new(s)))
                        .collect(),
                    c.local_sources
                        .into_iter()
                        .map(|s| (key(&s.case_id, &s.job_id, s.repeat, CacheStage::Asr), s))
                        .collect(),
                )
            })
            .unwrap_or_default();
        let oldest_unix_ms = entries.values().map(|e| e.source_created_unix_ms).min();
        let newest_unix_ms = entries.values().map(|e| e.source_created_unix_ms).max();
        let plan = Arc::new(ReplayPlan {
            identity: identity.clone(),
            policy: policy(inputs).cloned(),
            entries,
            local_sources,
            oldest_unix_ms,
            newest_unix_ms,
            #[cfg(test)]
            clock: std::sync::Mutex::new(TestClock::System),
            input_sha: self.input_sha,
            output: self.output,
        });
        plan.validate_bindings(inputs)?;
        Ok(plan)
    }
}
pub(super) struct ReplayPlan {
    pub identity: RunInputIdentity,
    pub policy: Option<CachePolicy>,
    entries: BTreeMap<Key, Arc<StageCandidate>>,
    local_sources: BTreeMap<Key, LocalStageSource>,
    oldest_unix_ms: Option<u64>,
    newest_unix_ms: Option<u64>,
    #[cfg(test)]
    clock: std::sync::Mutex<TestClock>,
    input_sha: Option<String>,
    pub output: Option<PathBuf>,
}
pub(super) enum ReplayDecision {
    Hit(ValidatedCandidate),
    Miss,
}
impl ReplayDecision {
    pub fn candidate(self) -> Option<ValidatedCandidate> {
        match self {
            Self::Hit(candidate) => Some(candidate),
            Self::Miss => None,
        }
    }
}
#[cfg(test)]
enum TestClock {
    System,
    Known(u64),
    Unavailable,
}
#[derive(Clone)]
pub(crate) struct ValidatedCandidate(Arc<StageCandidate>, String, Option<Box<LocalStageSource>>);
impl ValidatedCandidate {
    pub(crate) fn local_source(&self) -> Option<&crate::observations::LocalAsrObservation> {
        self.2.as_deref().map(|s| &s.observation)
    }
    pub(crate) fn known_empty(&self) -> bool {
        self.0
            .asr_observation
            .as_ref()
            .is_some_and(crate::observations::AsrObservation::confirmed_empty)
    }
    pub(crate) fn value(&self) -> &str {
        &self.0.value
    }
    pub(crate) fn replay(&self) -> StageReplay {
        StageReplay {
            source_run: self.0.source_run.clone(),
            source_created_unix_ms: self.0.source_created_unix_ms,
            request_sha256: self.0.request_sha256.clone(),
            input_sha256: self.0.input_sha256.clone(),
            scope: self.0.scope.clone(),
            source_checkpoint_sha256: self.1.clone(),
            source_request: self.request(),
            source_asr_observation: self.observation(),
            source_local_runtime: self.local_source().cloned(),
            current_guard: StageState::NotRun {},
        }
    }
    pub(crate) fn observation(&self) -> Option<crate::observations::AsrObservation> {
        self.0.asr_observation.clone()
    }
    pub(crate) fn request(&self) -> crate::observations::ResolvedRequest {
        self.0.request.clone()
    }
}
impl ReplayPlan {
    fn validate_bindings(&self, inputs: &ValidatedInputs) -> Result<(), InputError> {
        if self.entries.is_empty() {
            return Ok(());
        }
        let report = super::runner::build_report(inputs, self.identity.baseline().clone())?;
        for entry in self.entries.values() {
            let row = report
                .cases
                .iter()
                .find(|r| {
                    r.case_id == entry.case_id
                        && r.job_id == entry.job_id
                        && r.repeat == entry.repeat
                })
                .ok_or_else(|| {
                    error(
                        Path::new("checkpoint"),
                        "resume_stage_identity_mismatch",
                        "stage row missing",
                    )
                })?;
            let asr_text = self
                .entries
                .get(&key(&row.case_id, &row.job_id, row.repeat, CacheStage::Asr))
                .map(|c| c.value.as_str());
            let (request, parent) = expected_request(inputs, row, entry.stage, asr_text)
                .ok_or_else(|| {
                    error(
                        Path::new("checkpoint"),
                        "request_identity_incomplete",
                        "no shared producer builder is available for this stage",
                    )
                })?;
            if entry.input_sha256 != self.input_sha(inputs, row, entry.stage, parent.as_deref())
                || hash(&request) != entry.request_sha256
            {
                return Err(error(
                    Path::new("checkpoint"),
                    "resume_stage_identity_mismatch",
                    "candidate input/actual request is incompatible; no key access or requests",
                ));
            }
        }
        Ok(())
    }
    fn time(&self) -> Result<u64, InputError> {
        #[cfg(test)]
        match &*self.clock.lock().unwrap_or_else(|p| p.into_inner()) {
            TestClock::System => {}
            TestClock::Known(time) => return Ok(*time),
            TestClock::Unavailable => {
                return Err(error(
                    Path::new("clock"),
                    "time_identity_unavailable",
                    "fixture clock unavailable",
                ))
            }
        }
        now()
    }
    #[cfg(test)]
    fn set_clock(&self, clock: TestClock) {
        *self.clock.lock().unwrap_or_else(|p| p.into_inner()) = clock;
    }
    /// The same owner admits credential access, cache selection and every
    /// adapter start/retry. Expiry/unknown age is an error, never a cache miss.
    pub fn admit_boundary(&self) -> Result<(), InputError> {
        if let Some(oldest) = self.oldest_unix_ms {
            let time = self.time().map_err(|_| {
                error(
                    Path::new("checkpoint"),
                    "resume_time_unknown",
                    "checkpoint age cannot be established",
                )
            })?;
            let age = time
                .checked_sub(oldest)
                .filter(|_| self.newest_unix_ms.is_none_or(|newest| time >= newest))
                .ok_or_else(|| {
                    error(
                        Path::new("checkpoint"),
                        "resume_time_unknown",
                        "clock precedes a source candidate; age is unknown",
                    )
                })?;
            let ttl = self
                .policy
                .as_ref()
                .and_then(|p| p.max_age_ms)
                .ok_or_else(|| {
                    error(
                        Path::new("checkpoint"),
                        "cache_policy_not_applicable",
                        "candidate TTL missing",
                    )
                })?;
            if age > ttl {
                return Err(error(
                    Path::new("checkpoint"),
                    "resume_expired",
                    "checkpoint TTL elapsed; no further key access or adapter dispatch admitted",
                ));
            }
        }
        Ok(())
    }
    pub fn input_sha(
        &self,
        inputs: &ValidatedInputs,
        row: &CaseRecordV1,
        stage: CacheStage,
        parent: Option<&str>,
    ) -> String {
        let case = inputs
            .cases
            .iter()
            .find(|c| c.case.id() == row.case_id)
            .expect("validated report case");
        if let Some(source) = case.case.asr_speech() {
            return hash(&(
                "voiceflow-local-asr-stage-input-sha256-v1",
                execution_identity(&self.identity),
                &source.case,
                &row.requested,
                &case.audio,
                stage,
                parent,
            ));
        }
        if matches!(self.identity, RunInputIdentity::AsrSpeech(_)) {
            return hash(&(
                "voiceflow-local-asr-stage-input-sha256-v1",
                execution_identity(&self.identity),
                &row.requested,
                &case.audio,
                case.case.id(),
                case.negative(),
                stage,
                parent,
            ));
        }
        // Top identity binds all manifests/sidecar/annotations/splits/reference bytes,
        // complete WAV+PCM and code/runtime. No output, work or credential paths.
        hash(&(
            "voiceflow-stage-input-sha256-v2",
            execution_identity(&self.identity),
            &row.requested,
            &case.case.legacy().expect("V1 checkpoint cases only"),
            &case.audio,
            &case.legacy_manifest_sha(),
            &case.source_kind,
            case.negative(),
            &row.control_input_identities
                .iter()
                .map(|(name, id)| (name, &id.sha256, id.bytes))
                .collect::<Vec<_>>(),
            stage,
            parent,
        ))
    }
    pub fn decide(
        &self,
        inputs: &ValidatedInputs,
        row: &CaseRecordV1,
        stage: CacheStage,
        parent: Option<&str>,
    ) -> Result<ReplayDecision, InputError> {
        self.admit_boundary()?;
        self.validate_local_binding(row)?;
        let candidate = self.matching_candidate(inputs, row, stage, parent);
        self.admit_boundary()?;
        Ok(match candidate {
            Some(candidate) => ReplayDecision::Hit(candidate),
            None => ReplayDecision::Miss,
        })
    }
    fn matching_candidate(
        &self,
        inputs: &ValidatedInputs,
        row: &CaseRecordV1,
        stage: CacheStage,
        parent: Option<&str>,
    ) -> Option<ValidatedCandidate> {
        if self.policy.as_ref()?.purpose != CachePurpose::QualityReplay
            || (super::local_asr::model_id(&row.requested.candidate_id).is_some()
                && !self.requires_local_attestation())
            || (stage == CacheStage::Cleanup
                && row.requested.cleanup.candidate_id.as_deref() == Some(super::local_cleanup::ID))
        {
            return None;
        }
        let entry = self
            .entries
            .get(&key(&row.case_id, &row.job_id, row.repeat, stage))?;
        let text = match &row.filtered_text {
            ObservedText::KnownValue { value } => Some(value.as_str()),
            _ => None,
        };
        let (request, expected_parent) = expected_request(inputs, row, stage, text)?;
        if hash(&request) != entry.request_sha256 || expected_parent.as_deref() != parent {
            return None;
        }
        if entry.input_sha256 != self.input_sha(inputs, row, stage, parent) {
            return None;
        }
        let source = self
            .local_sources
            .get(&key(&row.case_id, &row.job_id, row.repeat, stage));
        if super::local_asr::model_id(&row.requested.candidate_id).is_some() && source.is_none() {
            return None;
        }
        Some(ValidatedCandidate(
            entry.clone(),
            self.input_sha.clone()?,
            source.cloned().map(Box::new),
        ))
    }
    /// Policy owns local replay applicability. Fresh inference does not need
    /// an attestation, including when the selected runtime predates this field.
    pub fn requires_local_attestation(&self) -> bool {
        self.policy
            .as_ref()
            .is_some_and(|p| p.purpose == CachePurpose::QualityReplay)
            && matches!(self.identity, RunInputIdentity::AsrSpeech(_))
    }
    pub fn validate_local_binding(&self, row: &CaseRecordV1) -> Result<(), InputError> {
        if !self.requires_local_attestation()
            || super::local_asr::model_id(&row.requested.candidate_id).is_none()
        {
            return Ok(());
        }
        let current = LocalReplayIdentity::from_row(row).map_err(|reason| {
            error(
                Path::new("runtime"),
                "local_resume_identity_incomplete",
                reason,
            )
        })?;
        if self
            .local_sources
            .get(&key(&row.case_id, &row.job_id, row.repeat, CacheStage::Asr))
            .is_some_and(|source| hash(&current) != hash(&source.identity))
        {
            return Err(error(Path::new("runtime"),"local_resume_identity_mismatch","measured loaded model/runtime/precision/tokenizer changed; checkpoint preserved, no inference dispatched"));
        }
        Ok(())
    }
    pub fn summary(&self) -> Option<ReplaySummary> {
        self.policy.clone().map(|p| {
            let fresh = p.purpose != CachePurpose::QualityReplay;
            ReplaySummary {
                policy: p,
                checkpoint_input_sha256: self.input_sha.clone(),
                checkpoint_output: self.output.as_ref().map(|p| p.display().to_string()),
                checkpoint_acquisition: None,
                checkpoint_artifact_outcome: None,
                resumed_asr: 0,
                resumed_cleanup: 0,
                fresh_asr_cases: 0,
                fresh_cleanup_cases: 0,
                replay_guard_rejected: 0,
                failed_cases: 0,
                cancelled_cases: 0,
                unknown_cases: 0,
                not_run_cases: 0,
                current_quality_eligible: fresh,
                current_performance_eligible: fresh,
                current_reliability_eligible: fresh,
            }
        })
    }
}
fn expected_request(
    inputs: &ValidatedInputs,
    row: &CaseRecordV1,
    stage: CacheStage,
    asr_text: Option<&str>,
) -> Option<(crate::observations::ResolvedRequest, Option<String>)> {
    use super::{
        authorization::KeyMaterial,
        legacy::{self, AsrExecution},
    };
    let recipe = inputs.recipe.as_ref()?;
    let index = recipe
        .jobs
        .iter()
        .enumerate()
        .find(|(i, j)| j.id(*i) == row.job_id)?
        .0;
    let fixture = &inputs.control_fixtures[index];
    let job = &row.requested;
    match stage {
        CacheStage::Asr => {
            if let Some(model) = super::local_asr::model_id(&job.candidate_id) {
                let options = super::local_asr::options(job).ok()?;
                return Some((
                    crate::ondevice_runtime::file_request_facts(
                        model,
                        options.language.as_deref(),
                        row.audio.sample_count,
                    ),
                    None,
                ));
            }
            let (id, provider, model, base) = super::runner::asr_spec(&job.candidate_id);
            let candidate = legacy::asr_candidate(
                id,
                provider.as_str(),
                model.into(),
                base,
                KeyMaterial::unavailable("identity_only"),
            );
            let AsrExecution::Cloud { endpoint, .. } = candidate.execution else {
                return None;
            };
            let options = fixture
                .asr_options(&endpoint, &candidate.model, &job.recognition_language)
                .ok()?;
            let case = inputs.cases.iter().find(|c| c.case.id() == row.case_id)?;
            Some((
                crate::asr::file_request_facts(&endpoint, case.bytes.len(), &options)?,
                None,
            ))
        }
        CacheStage::Cleanup => {
            let id = job.cleanup.candidate_id.as_ref()?;
            let candidate = super::runner::cleanup_spec(id)
                .candidate(
                    &KeyMaterial::unavailable("identity_only"),
                    None,
                    &tokio_util::sync::CancellationToken::new(),
                )
                .ok()?;
            let case = inputs.cases.iter().find(|c| c.case.id() == row.case_id)?;
            let e = super::runner::eval_case(case.case.speech()?, asr_text?, job);
            let rules = fixture.preparation(&job.local_rules, &row.case_id);
            let prepared = legacy::prepare_cleanup_case_with_fixture(&e, Some(&rules));
            Some((
                legacy::cleanup_candidate_request_facts(&candidate, &prepared)?,
                Some(prepared.source().into()),
            ))
        }
    }
}
#[cfg(test)]
fn validate_checkpoint(
    path: &Path,
    cp: &CheckpointV1,
    inputs: &ValidatedInputs,
    clock: u64,
) -> Result<(), InputError> {
    validate_data(path, &cp.clone().into(), inputs, clock)
}
fn validate_data(
    path: &Path,
    cp: &CheckpointData,
    inputs: &ValidatedInputs,
    clock: u64,
) -> Result<(), InputError> {
    let local = matches!(cp.identity, RunInputIdentity::AsrSpeech(_)) && cp.schema_version == 2;
    if cp.artifact_type != "voiceflow_checkpoint"
        || !(local
            || (matches!(cp.identity, RunInputIdentity::Speech(_)) && cp.schema_version == 1))
    {
        return Err(error(
            path,
            "checkpoint_version",
            "strict SHA checkpoint version/identity pair required",
        ));
    }
    if let RunInputIdentity::AsrSpeech(identity) = &cp.identity {
        if identity.schema_version != 3
            || identity.artifact_type != "run_input_identity"
            || identity.input.kind != "asr_speech"
        {
            return Err(error(
                path,
                "checkpoint_version",
                "local checkpoint requires exact V3 input identity schema",
            ));
        }
    }
    if local {
        validate_local_platform(cp.identity.baseline())?;
    }
    let ttl = policy(inputs)
        .and_then(|p| p.max_age_ms)
        .ok_or_else(|| error(path, "cache_policy_not_applicable", "TTL required"))?;
    if cp.created_unix_ms > clock
        || cp.successful_stage_candidates.len() > 20_000
        || cp.complete_jobs.len() > 32
        || cp.local_sources.len() > 20_000
    {
        return Err(error(
            path,
            "checkpoint_budget_or_time",
            "checkpoint bounds/clock rejected",
        ));
    }
    let recipe = inputs.recipe.as_ref().expect("experiment required");
    let jobs: BTreeMap<_, _> = recipe
        .jobs
        .iter()
        .enumerate()
        .map(|(i, j)| (j.id(i), j))
        .collect();
    let mut complete = BTreeSet::new();
    if cp
        .complete_jobs
        .iter()
        .any(|j| !jobs.contains_key(j) || !complete.insert(j))
    {
        return Err(error(
            path,
            "duplicate_or_unknown_job",
            "complete_jobs must contain unique declared jobs",
        ));
    }
    let candidate_keys: BTreeSet<_> = cp
        .successful_stage_candidates
        .iter()
        .map(|s| key(&s.case_id, &s.job_id, s.repeat, s.stage))
        .collect();
    for id in &cp.complete_jobs {
        let job = jobs[id];
        for repeat in 0..job.repeats {
            for case in &inputs.cases {
                let has = |stage| candidate_keys.contains(&key(case.case.id(), id, repeat, stage));
                if (!local && case.negative())
                    || !has(CacheStage::Asr)
                    || (job.cleanup.candidate_id.is_some() && !has(CacheStage::Cleanup))
                {
                    return Err(error(path,"incomplete_job","complete_jobs requires all declared cacheable stages; it never authorizes request skipping"));
                }
            }
        }
    }
    let mut sources = BTreeSet::new();
    for source in &cp.local_sources {
        let k = key(
            &source.case_id,
            &source.job_id,
            source.repeat,
            CacheStage::Asr,
        );
        if !local || !sources.insert(k.clone()) || !candidate_keys.contains(&k) {
            return Err(error(
                path,
                "duplicate_or_orphan_local_source",
                "local source must identify exactly one declared ASR candidate",
            ));
        }
    }
    let mut seen = BTreeSet::new();
    for s in &cp.successful_stage_candidates {
        let job = jobs.get(&s.job_id).ok_or_else(|| {
            error(
                path,
                "identity_mismatch",
                "checkpoint references an undeclared job",
            )
        })?;
        let case = inputs
            .cases
            .iter()
            .find(|c| c.case.id() == s.case_id)
            .ok_or_else(|| {
                error(
                    path,
                    "identity_mismatch",
                    "checkpoint references an undeclared case",
                )
            })?;
        if !seen.insert(key(&s.case_id, &s.job_id, s.repeat, s.stage)) {
            return Err(error(
                path,
                "duplicate_stage",
                "duplicate case/job/repeat/stage candidate",
            ));
        }
        if s.repeat >= job.repeats
            || (!local && case.negative())
            || (!local && super::local_asr::model_id(&job.candidate_id).is_some())
            || (local
                && (s.stage != CacheStage::Asr
                    || super::local_asr::model_id(&job.candidate_id).is_none()))
            || (s.stage == CacheStage::Cleanup
                && job.cleanup.candidate_id.as_deref() == Some(super::local_cleanup::ID))
            || s.scope != if local { LOCAL_SCOPE } else { SCOPE }
            || !matches!(s.state, StageState::Completed {})
            || (s.value.trim().is_empty() && !(local && case.negative() && s.value.is_empty()))
            || s.value.len() > TEXT_LIMIT
            || !validation::bounded(&s.origin, 256)
            || !validation::digest(&s.input_sha256)
            || hash(&s.request) != s.request_sha256
            || sha256(s.value.as_bytes()) != s.value_sha256
            || !validation::digest(&s.source_run)
            || s.source_created_unix_ms > cp.created_unix_ms
        {
            return Err(error(path,"candidate_not_applicable","only complete accepted cloud text candidates with bound request/value identities can be replayed"));
        }
        if clock
            .checked_sub(s.source_created_unix_ms)
            .is_none_or(|age| age > ttl)
        {
            return Err(error(path,"resume_expired","candidate exceeds explicit TTL; source time is not refreshed by checkpoint copying"));
        }
        if s.request.protocol.is_empty()
            || s.request.endpoint_identity.is_empty()
            || s.request.constructed_model.is_empty()
            || s.request.fields_constructed.is_empty()
        {
            return Err(error(
                path,
                "request_identity_incomplete",
                "actual constructed request facts are required",
            ));
        }
        if local {
            let source = cp
                .local_sources
                .iter()
                .find(|o| o.case_id == s.case_id && o.job_id == s.job_id && o.repeat == s.repeat)
                .ok_or_else(|| {
                    error(
                        path,
                        "local_source_missing",
                        "local ASR candidate lacks its producer source",
                    )
                })?;
            validate_local_source(path, source, s, case)?;
            continue;
        }
        match s.stage {
            CacheStage::Asr => {
                let obs = s.asr_observation.as_ref().ok_or_else(|| {
                    error(
                        path,
                        "candidate_observation_missing",
                        "ASR producer observation required",
                    )
                })?;
                if s.completion != obs.protocol_completion
                    || obs.terminal != "speech_text"
                    || !matches!(&obs.filtered_text, ObservedText::KnownValue{value} if value==&s.value)
                    || obs.origin != "provider_response"
                    || s.origin != obs.origin
                    || !matches!(&obs.provider_text, ObservedText::KnownValue { value } if !value.trim().is_empty())
                    || !matches!(
                        obs.protocol_completion.as_str(),
                        "json_complete" | "stop" | "complete" | "completed"
                    )
                {
                    return Err(error(
                        path,
                        "candidate_completion_unknown",
                        "ASR completion/filter provenance is not a successful complete candidate",
                    ));
                }
            }
            CacheStage::Cleanup => {
                if s.asr_observation.is_some()
                    || s.origin != "cleanup_adapter_accepted"
                    || job.cleanup.candidate_id.is_none()
                    || !matches!(s.completion.as_str(), "stop" | "end_turn" | "stop_sequence")
                {
                    return Err(error(
                        path,
                        "candidate_not_applicable",
                        "cleanup stage is not requested",
                    ));
                }
            }
        }
    }
    Ok(())
}
fn validate_local_platform(b: &BaselineIdentity) -> Result<(), InputError> {
    let known = |value: &Option<String>| value.as_ref().is_some_and(|s| !s.trim().is_empty());
    if !validation::digest(&b.executable_sha256)
        || b.version.is_empty()
        || b.os.is_empty()
        || b.architecture.is_empty()
        || !known(&b.os_version)
        || !known(&b.os_build)
        || !known(&b.cpu)
        || !b.ram_bytes.is_some_and(|n| n > 0)
    {
        return Err(error(
            Path::new("baseline"),
            "local_resume_platform_identity_incomplete",
            "current executable/OS build/CPU/RAM identity required for local replay",
        ));
    }
    Ok(())
}
/// One local case dispatch owns one runtime.transcribe call. The current
/// format has no inference invocation IDs/retries: preload emits other stages
/// and Swift chunking stays inside that one call. LocalStageGuard emits exactly
/// Started then one consumed terminal; Drop emits Unknown, never completion.
#[derive(Debug, Clone, Copy)]
enum LocalInferenceOutcome {
    Accepted,
    EmptyRejected,
}
impl LocalInferenceOutcome {
    fn parse(runtime: &crate::observations::LocalAsrObservation) -> Result<Self, &'static str> {
        use crate::observations::{LocalStage, LocalStageState};
        let mut inference = runtime
            .events
            .iter()
            .filter(|e| matches!(e.stage, LocalStage::Inference));
        let start = inference.next().ok_or("inference_history_missing_start")?;
        if !matches!(start.state, LocalStageState::Started)
            || start.reason.is_some()
            || start.runtime_version.is_some()
            || start.elapsed_ms != 0.0
        {
            return Err("inference_history_invalid_or_orphan_start");
        }
        let terminal = inference
            .next()
            .ok_or("inference_history_terminal_unobserved")?;
        if inference.next().is_some() {
            return Err("inference_history_multiple_or_conflicting_events");
        }
        if !terminal.elapsed_ms.is_finite() || terminal.elapsed_ms < 0.0 {
            return Err("inference_history_invalid_terminal_timing");
        }
        match terminal.state {
            LocalStageState::Completed if terminal.reason.is_none() => Ok(Self::Accepted),
            LocalStageState::Failed
                if terminal.reason.as_deref() == Some("local_empty_output_not_accepted")
                    && terminal.runtime_version.is_none() =>
            {
                Ok(Self::EmptyRejected)
            }
            LocalStageState::Unknown => Err("inference_history_terminal_unknown"),
            LocalStageState::Started => Err("inference_history_terminal_unobserved"),
            _ => Err("inference_history_terminal_not_cacheable"),
        }
    }
}
fn validate_local_source(
    path: &Path,
    source: &LocalStageSource,
    s: &StageCandidate,
    case: &validation::ValidatedCase,
) -> Result<(), InputError> {
    let fail = || {
        error(path,"local_candidate_not_applicable","complete measured local identity, exact coverage and unmodified producer observations required")
    };
    let obs = s.asr_observation.as_ref().ok_or_else(fail)?;
    let runtime = &source.observation;
    let coverage = runtime.coverage.as_ref().ok_or_else(fail)?;
    let inference = LocalInferenceOutcome::parse(runtime)
        .map_err(|reason| error(path, "local_inference_history_not_applicable", reason))?;
    if !source.identity.complete()
        || runtime.events_truncated
        || runtime.requested_model_id != source.identity.model_id
        || runtime.requested_revision != source.identity.revision
        || runtime.identity_checks.is_empty()
        || runtime.identity_checks.iter().any(|c| {
            !c.applicable
                || c.reason.is_some()
                || c.observed_model_id.as_deref() != Some(source.identity.model_id.as_str())
                || c.observed_revision.as_deref() != Some(source.identity.revision.as_str())
        })
        || runtime.observed_model_id.as_deref() != Some(source.identity.model_id.as_str())
        || runtime.observed_revision.as_deref() != Some(source.identity.revision.as_str())
        || runtime
            .loaded_attestation
            .as_ref()
            .is_none_or(|a| hash(a) != hash(&source.identity.loaded))
        || coverage.expected_samples != case.audio.sample_count
        || coverage.covered_samples != coverage.expected_samples
        || coverage.chunks_total == 0
        || coverage.chunks_total != coverage.chunks_completed
        || coverage.maximum_tokens == 0
        || coverage.generation_tokens >= coverage.maximum_tokens
        || coverage.completion != "complete"
        || obs.protocol_completion != "complete"
        || s.completion != "complete"
        || obs.origin != "local_model_output"
        || s.origin != obs.origin
        || obs.filtered_text != ObservedText::known(&s.value)
        || obs.provider_text != obs.filtered_text
        || match inference {
            LocalInferenceOutcome::EmptyRejected => {
                !case.negative()
                    || !s.value.is_empty()
                    || coverage.accepted
                    || !obs.confirmed_empty()
                    || obs.filtering_reason.as_deref() != Some("local_empty_output_not_accepted")
            }
            LocalInferenceOutcome::Accepted => {
                !coverage.accepted
                    || obs.terminal != "speech_text"
                    || s.value.trim().is_empty()
                    || obs.filtering_reason.is_some()
            }
        }
    {
        return Err(fail());
    }
    Ok(())
}
fn checkpoint_error(
    path: &Path,
    failure: super::artifacts::ArtifactFailure,
    attempts: u64,
) -> super::ExecutionError {
    let mut error = super::ExecutionError::artifact(path, failure, attempts);
    error.code = "checkpoint_error";
    error.terminal = StageState::Failed {
        reason: "checkpoint_error".into(),
    };
    error.checkpoint_outcome = Some(error.artifact_outcome.clone());
    error
}
pub(super) struct CheckpointOwner {
    plan: Arc<ReplayPlan>,
    data: CheckpointData,
    artifact: Option<(PathBuf, AtomicArtifact)>,
}
impl CheckpointOwner {
    pub fn new(plan: Arc<ReplayPlan>) -> Result<Self, super::ExecutionError> {
        let created_unix_ms = now().map_err(|cause| {
            let mut failure = super::ExecutionError::input(cause, 0);
            failure.checkpoint_acquisition =
                plan.output.as_ref().map(|path| CheckpointAcquisition {
                    selected_path: path.display().to_string(),
                    state: StageState::Failed {
                        reason: "checkpoint_clock_unavailable_before_acquisition".into(),
                    },
                    writer_acquired: false,
                    path_applicability: StageState::Unknown {
                        reason: "checkpoint_writer_not_acquired".into(),
                    },
                });
            failure
        })?;
        let artifact = plan
            .output
            .as_ref()
            .map(|p| {
                AtomicArtifact::new(p).map(|a| (p.clone(), a)).map_err(|f| {
                    let mut failure = checkpoint_error(p, f, 0);
                    failure.checkpoint_acquisition = Some(CheckpointAcquisition {
                        selected_path: p.display().to_string(),
                        state: StageState::Failed {
                            reason: failure.safe_detail.clone(),
                        },
                        writer_acquired: false,
                        path_applicability: StageState::Unknown {
                            reason: "checkpoint_writer_not_acquired".into(),
                        },
                    });
                    failure
                })
            })
            .transpose()?;
        let data = CheckpointData {
            artifact_type: "voiceflow_checkpoint".into(),
            schema_version: if matches!(plan.identity, RunInputIdentity::AsrSpeech(_)) {
                2
            } else {
                1
            },
            created_unix_ms,
            identity: Some(plan.identity.clone())
                .filter(|i| !matches!(i, RunInputIdentity::NonSpeech(_)))
                .ok_or_else(|| {
                    super::ExecutionError::input(
                        error(
                            Path::new("checkpoint"),
                            "resume_not_applicable",
                            "non-speech input has no candidate checkpoint",
                        ),
                        0,
                    )
                })?,
            local_sources: plan.local_sources.values().cloned().collect(),
            successful_stage_candidates: plan
                .entries
                .values()
                .map(|e| e.as_ref().clone())
                .collect(),
            complete_jobs: vec![],
            terminal: StageState::NotRun {},
            provider_attempts: 0,
        };
        Ok(Self {
            plan,
            data,
            artifact,
        })
    }
    pub fn acquisition(&self) -> Option<CheckpointAcquisition> {
        self.artifact
            .as_ref()
            .map(|(path, _)| CheckpointAcquisition {
                selected_path: path.display().to_string(),
                state: StageState::Completed {},
                writer_acquired: true,
                path_applicability: StageState::Unknown {
                    reason: "acquisition_does_not_establish_installed_content".into(),
                },
            })
    }
    pub fn capture(
        &mut self,
        inputs: &ValidatedInputs,
        row: &CaseRecordV1,
        run_id: &str,
        stage: CacheStage,
        parent: Option<&str>,
    ) {
        if stage == CacheStage::Cleanup
            && row.cleanup_replay.is_some()
            && (!matches!(row.cleanup_adapter, StageState::Completed {})
                || !matches!(row.final_guard, StageState::Completed {}))
        {
            self.data.successful_stage_candidates.retain(|s| {
                key(&s.case_id, &s.job_id, s.repeat, s.stage)
                    != key(&row.case_id, &row.job_id, row.repeat, stage)
            });
            return;
        }
        if self
            .plan
            .policy
            .as_ref()
            .is_none_or(|p| p.purpose != CachePurpose::QualityReplay)
            || (super::local_asr::model_id(&row.requested.candidate_id).is_some()
                && !self.plan.requires_local_attestation())
            || (stage == CacheStage::Cleanup
                && row.requested.cleanup.candidate_id.as_deref() == Some(super::local_cleanup::ID))
            || row.cancellation_requested
            || (!matches!(self.plan.identity, RunInputIdentity::AsrSpeech(_))
                && inputs
                    .cases
                    .iter()
                    .any(|c| c.case.id() == row.case_id && c.negative()))
        {
            return;
        }
        let local_identity = if super::local_asr::model_id(&row.requested.candidate_id).is_some() {
            let Ok(identity) = LocalReplayIdentity::from_row(row) else {
                return;
            };
            Some(identity)
        } else {
            None
        };
        let local_empty = local_identity.is_some()
            && inputs
                .cases
                .iter()
                .any(|c| c.case.id() == row.case_id && c.negative())
            && row
                .asr_observation
                .as_ref()
                .is_some_and(crate::observations::AsrObservation::confirmed_empty);
        let (value, request, observation, origin) = match stage {
            CacheStage::Asr => {
                let Some(obs) = &row.asr_observation else {
                    return;
                };
                if row.asr_replay.is_some()
                    || (row.asr_terminal != "speech_text" && !local_empty)
                    || !matches!(
                        obs.protocol_completion.as_str(),
                        "json_complete" | "stop" | "complete" | "completed"
                    )
                    || obs.origin
                        != if local_identity.is_some() {
                            "local_model_output"
                        } else {
                            "provider_response"
                        }
                    || row.asr_origin != obs.origin
                    || (!local_empty
                        && !matches!(&obs.provider_text, ObservedText::KnownValue { value } if !value.trim().is_empty()))
                {
                    return;
                }
                let value = match &row.filtered_text {
                    ObservedText::KnownValue { value } => value.as_str(),
                    ObservedText::KnownEmpty {} if local_empty => "",
                    _ => return,
                };
                let Some(request) = &row.asr_request else {
                    return;
                };
                (value, request, Some(obs.clone()), row.asr_origin.clone())
            }
            CacheStage::Cleanup => {
                if !self.data.successful_stage_candidates.iter().any(|s| {
                    s.case_id == row.case_id
                        && s.job_id == row.job_id
                        && s.repeat == row.repeat
                        && s.stage == CacheStage::Asr
                }) {
                    return;
                }
                if row.cleanup_replay.is_some()
                    || !matches!(row.cleanup_adapter, StageState::Completed {})
                    || !matches!(row.final_guard, StageState::Completed {})
                {
                    return;
                }
                let Some(obs) = &row.cleanup_observation else {
                    return;
                };
                if obs.adapter_decision != "accepted" {
                    return;
                }
                let (ObservedText::KnownValue { value }, Some(request)) =
                    (&row.adapter_candidate, &row.cleanup_request)
                else {
                    return;
                };
                (
                    value.as_str(),
                    request,
                    None,
                    "cleanup_adapter_accepted".into(),
                )
            }
        };
        if (value.trim().is_empty() && !local_empty)
            || value.len() > TEXT_LIMIT
            || request.fields_constructed.is_empty()
        {
            return;
        }
        let text = match &row.filtered_text {
            ObservedText::KnownValue { value } => Some(value.as_str()),
            _ => None,
        };
        let Some((expected, expected_parent)) = expected_request(inputs, row, stage, text) else {
            return;
        };
        if hash(&expected) != hash(request) || expected_parent.as_deref() != parent {
            return;
        }
        let id = key(&row.case_id, &row.job_id, row.repeat, stage);
        let entry = StageCandidate {
            case_id: row.case_id.clone(),
            job_id: row.job_id.clone(),
            repeat: row.repeat,
            stage,
            input_sha256: self.plan.input_sha(inputs, row, stage, parent),
            request_sha256: hash(request),
            request: request.clone(),
            origin,
            completion: match stage {
                CacheStage::Asr => observation
                    .as_ref()
                    .map(|o| o.protocol_completion.clone())
                    .unwrap_or_default(),
                CacheStage::Cleanup => row
                    .cleanup_observation
                    .as_ref()
                    .map(|o| o.completion_kind.clone())
                    .unwrap_or_default(),
            },
            value: value.to_owned(),
            value_sha256: sha256(value.as_bytes()),
            source_run: run_id.into(),
            source_created_unix_ms: match now() {
                Ok(time) => time,
                Err(_) => return,
            },
            state: StageState::Completed {},
            scope: if local_identity.is_some() {
                LOCAL_SCOPE
            } else {
                SCOPE
            }
            .into(),
            asr_observation: observation,
        };
        if let Some(identity) = local_identity {
            let source = LocalStageSource {
                case_id: row.case_id.clone(),
                job_id: row.job_id.clone(),
                repeat: row.repeat,
                identity,
                observation: row
                    .local_runtime
                    .as_ref()
                    .expect("identity requires observation")
                    .clone(),
            };
            let Some(case) = inputs.cases.iter().find(|c| c.case.id() == row.case_id) else {
                return;
            };
            if validate_local_source(Path::new("checkpoint"), &source, &entry, case).is_err() {
                return;
            }
            self.data
                .local_sources
                .retain(|s| key(&s.case_id, &s.job_id, s.repeat, CacheStage::Asr) != id);
            self.data.local_sources.push(source);
        }
        self.data
            .successful_stage_candidates
            .retain(|s| key(&s.case_id, &s.job_id, s.repeat, s.stage) != id);
        self.data.successful_stage_candidates.push(entry);
    }
    pub fn outcome(&self) -> Option<super::artifacts::ArtifactOutcome> {
        self.artifact.as_ref().map(|(_, owner)| owner.outcome())
    }
    pub fn persist(&mut self, report: &RunRecord) -> Result<(), super::ExecutionError> {
        self.data.created_unix_ms =
            now().map_err(|e| super::ExecutionError::input(e, report.provider_attempts))?;
        self.data.terminal = report.terminal.clone();
        self.data.provider_attempts = report.provider_attempts;
        let time = self.data.created_unix_ms;
        let ttl = self
            .plan
            .policy
            .as_ref()
            .and_then(|p| p.max_age_ms)
            .unwrap_or(0);
        self.data.successful_stage_candidates.retain(|s| {
            time.checked_sub(s.source_created_unix_ms)
                .is_some_and(|age| age <= ttl)
        });
        self.data.local_sources.retain(|o| {
            self.data.successful_stage_candidates.iter().any(|s| {
                s.case_id == o.case_id
                    && s.job_id == o.job_id
                    && s.repeat == o.repeat
                    && s.stage == CacheStage::Asr
            })
        });
        let parents: BTreeSet<_> = self
            .data
            .successful_stage_candidates
            .iter()
            .filter(|s| s.stage == CacheStage::Asr)
            .map(|s| key(&s.case_id, &s.job_id, s.repeat, CacheStage::Asr))
            .collect();
        self.data.successful_stage_candidates.retain(|s| {
            s.stage == CacheStage::Asr
                || parents.contains(&key(&s.case_id, &s.job_id, s.repeat, CacheStage::Asr))
        });
        let candidate_keys: BTreeSet<_> = self
            .data
            .successful_stage_candidates
            .iter()
            .map(|s| key(&s.case_id, &s.job_id, s.repeat, s.stage))
            .collect();
        let mut jobs: BTreeMap<String, bool> = BTreeMap::new();
        for row in &report.cases {
            let has =
                |stage| candidate_keys.contains(&key(&row.case_id, &row.job_id, row.repeat, stage));
            let done = matches!(row.state, StageState::Completed {})
                && has(CacheStage::Asr)
                && (row.requested.cleanup.candidate_id.is_none() || has(CacheStage::Cleanup));
            jobs.entry(row.job_id.clone())
                .and_modify(|v| *v &= done)
                .or_insert(done);
        }
        self.data.complete_jobs = jobs
            .into_iter()
            .filter_map(|(j, done)| done.then_some(j))
            .collect();
        if let Some((path, artifact)) = &mut self.artifact {
            artifact
                .write_json_with_attempts(&self.data, Some(report.provider_attempts))
                .map_err(|f| checkpoint_error(path, f, report.provider_attempts))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{fixtures::FixtureDirectory, journal::RunJournal, legacy, runner};
    use super::*;
    fn fixture(
        d: &FixtureDirectory,
        cleanup: bool,
        execution_deadline_ms: u64,
    ) -> (ValidatedInputs, RunRecord, CheckpointV1) {
        let (mut inputs, journal) = super::super::tests::journal_fixture(d);
        let baseline = journal.snapshot().identity.baseline().clone();
        let recipe = inputs.recipe.as_mut().unwrap();
        recipe.jobs[0].run_budget.deadline_ms = execution_deadline_ms;
        recipe.cache_policy = Some(CachePolicy {
            purpose: CachePurpose::QualityReplay,
            max_age_ms: Some(60_000),
        });
        if cleanup {
            recipe.jobs[0].cleanup.intensity = CleanupIntensity::Standard;
            recipe.jobs[0].cleanup.candidate_id = Some("groq_gpt_oss_20b".into());
        }
        inputs.recipe_sha256 = hash(inputs.recipe.as_ref().unwrap());
        let report = runner::build_report(&inputs, baseline).unwrap();
        let plan = Admission {
            loaded: None,
            input_sha: None,
            output: None,
        }
        .bind(&inputs, &report.identity)
        .unwrap();
        let mut row = report.cases[0].clone();
        let source = "Do not change API v1.";
        let request = expected_request(&inputs, &row, CacheStage::Asr, None)
            .unwrap()
            .0;
        // Declared producer-shaped candidate for admission tests; no cloud
        // request/receipt or model-quality evidence is asserted by this fixture.
        let observation = crate::observations::AsrObservation {
            provider_text: ObservedText::known(source),
            provider_text_source_kind: Some("fixture_declared_string".into()),
            filtered_text: ObservedText::known(source),
            origin: "provider_response".into(),
            terminal: "speech_text".into(),
            filtering_reason: None,
            protocol_completion: "json_complete".into(),
            reported_model: ObservedText::unknown("server_weights_unknown"),
        };
        let time = now().unwrap();
        let mut candidates = vec![StageCandidate {
            case_id: row.case_id.clone(),
            job_id: row.job_id.clone(),
            repeat: 0,
            stage: CacheStage::Asr,
            input_sha256: plan.input_sha(&inputs, &row, CacheStage::Asr, None),
            request_sha256: hash(&request),
            request,
            origin: "provider_response".into(),
            completion: "json_complete".into(),
            value: source.into(),
            value_sha256: sha256(source.as_bytes()),
            source_run: "a".repeat(64),
            source_created_unix_ms: time,
            state: StageState::Completed {},
            scope: SCOPE.into(),
            asr_observation: Some(observation),
        }];
        if cleanup {
            row.filtered_text = ObservedText::known(source);
            let (request, parent) =
                expected_request(&inputs, &row, CacheStage::Cleanup, Some(source)).unwrap();
            candidates.push(StageCandidate {
                case_id: row.case_id.clone(),
                job_id: row.job_id.clone(),
                repeat: 0,
                stage: CacheStage::Cleanup,
                input_sha256: plan.input_sha(&inputs, &row, CacheStage::Cleanup, parent.as_deref()),
                request_sha256: hash(&request),
                request,
                origin: "cleanup_adapter_accepted".into(),
                completion: "stop".into(),
                value: source.into(),
                value_sha256: sha256(source.as_bytes()),
                source_run: "a".repeat(64),
                source_created_unix_ms: time,
                state: StageState::Completed {},
                scope: SCOPE.into(),
                asr_observation: None,
            });
        }
        let cp = CheckpointV1 {
            artifact_type: "voiceflow_checkpoint".into(),
            schema_version: 1,
            created_unix_ms: time,
            identity: report.identity.speech().unwrap().clone(),
            successful_stage_candidates: candidates,
            complete_jobs: vec![],
            terminal: StageState::Cancelled {
                reason: "earlier_stage_was_complete_before_cancel".into(),
            },
            provider_attempts: 2,
        };
        (inputs, report, cp)
    }
    fn admitted(
        inputs: &ValidatedInputs,
        report: &RunRecord,
        cp: CheckpointV1,
        output: Option<PathBuf>,
    ) -> Arc<ReplayPlan> {
        validate_checkpoint(Path::new("fixture-checkpoint"), &cp, inputs, now().unwrap()).unwrap();
        Admission {
            loaded: Some(cp.into()),
            input_sha: Some("b".repeat(64)),
            output,
        }
        .bind(inputs, &report.identity)
        .unwrap()
    }
    // These are declared producer-shaped structural fixtures. They do not
    // establish a real MLX load, public ASR quality or actual runtime precision.
    fn local_fixture(negative: bool) -> (FixtureDirectory, ValidatedInputs, RunRecord) {
        use crate::observations::*;
        let (dir, mut inputs, report) = super::super::speech_tests::fixture(
            super::super::speech_input::Purpose::FunctionalUnscored,
        );
        let recipe = inputs.recipe.as_mut().unwrap();
        recipe.jobs[0].candidate_id = "local_qwen3_asr_0_6b".into();
        recipe.jobs[0].recognition_language = RecognitionLanguage::Auto;
        recipe.cache_policy = Some(CachePolicy {
            purpose: CachePurpose::QualityReplay,
            max_age_ms: Some(60_000),
        });
        if negative {
            let index = dir.write(
                "negative.json",
                &serde_json::json!({
                    "artifact_type": "non_speech_index", "schema_version": 1,
                    "provenance": {
                        "kind": "synthetic", "note": "owned generated silence; structural fixture",
                        "reviewed": true
                    },
                    "cases": [{
                        "case_id": "structural-negative", "adjacent_wav": "one.wav",
                        "audio_sha256": inputs.cases[0].audio.audio_sha256,
                        "category": "silence", "source_review_id": "structural-fixture"
                    }]
                }),
            );
            // Reuse the shared validator's case/document/snapshot projection
            // through the original journal fixture's strict V1 input files.
            let admitted = validation::validate(
                &dir.0.join("speech.json"),
                Some(&dir.0.join("corpus.json")),
                Some(&dir.0.join("recipe.json")),
                Some(&index),
            )
            .unwrap();
            let validation::InputSelection::Speech {
                negative: Some(snapshot),
                ..
            } = admitted.selection
            else {
                unreachable!()
            };
            let case = admitted.cases.into_iter().find(|c| c.negative()).unwrap();
            let RecipeInput::AsrSpeech(ExperimentInputV3::AsrSpeech {
                non_speech_sha256, ..
            }) = &mut recipe.input
            else {
                unreachable!()
            };
            *non_speech_sha256 = Some(snapshot.sha256.clone());
            let validation::InputSelection::AsrSpeech { negative, .. } = &mut inputs.selection
            else {
                unreachable!()
            };
            *negative = Some(snapshot);
            // Keep the admitted speech member; the selected negative is first
            // so the shared local-source fixtures keep exercising that row.
            inputs.cases.insert(0, case);
        }
        inputs.recipe_sha256 = hash(inputs.recipe.as_ref().unwrap());
        let mut baseline = report.identity.baseline().clone();
        baseline.os_version = Some("declared-os".into());
        baseline.os_build = Some("declared-build".into());
        baseline.cpu = Some("declared-cpu".into());
        baseline.ram_bytes = Some(64);
        let mut report = runner::build_report(&inputs, baseline).unwrap();
        let row = &mut report.cases[0];
        let model = crate::ondevice_models::QWEN_ASR_06_ID;
        let revision = crate::ondevice_models::mlx_model(model).unwrap().revision;
        let attestation = LoadedAttestation {
            schema_version:1, executable_sha256:"1".repeat(64),
            metal_sha256:BTreeMap::from([("mlx.metallib".into(),"2".repeat(64)),("mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib".into(),"2".repeat(64))]),
            loaded_images:vec!["/declared/runtime|00000000-0000-0000-0000-000000000001".into()],
            parameter_layout_sha256:"3".repeat(64), parameter_count:2, parameter_dtypes:BTreeMap::from([("bfloat16".into(),2)]),
            decoder_recipe:"voiceflow-mlx-decoder-v1:chunks30s/min1s;temperature0;context-empty;qwen-repetition1.05/window32;inner-chunk1200s;cohere-topP1/topK0/vad-nil;ordered-trim-language-join".into(),
        };
        row.local_model = Some(serde_json::from_value(serde_json::json!({
            "model_id":model,"repository":"fixture","revision":revision,"precision_label":"declared label is not loaded evidence","loaded_precision":null,
            "selected_source_root":"/declared/source","canonical_source_root":"/declared/source","working_root":"/declared/work-one",
            "working_root_identity":{"device":1,"inode":2},"source_integrity_ms":null,"materialization_ms":0.0,
            "files":[{"path":"model.safetensors","bytes":4,"sha256":"4".repeat(64)}],
            "production_marker_sha256":"5".repeat(64),"derived_tokenizer_recipe":"declared-fixture-recipe","derived_tokenizer_sha256":"6".repeat(64),
            "derived_tokenizer_state":{"state":"completed"},"workspace_policy":"fresh_exclusive_retained_no_resume"
        })).unwrap());
        row.local_runtime = Some(LocalAsrObservation {
            requested_model_id: model.into(),
            requested_revision: revision.into(),
            observed_model_id: Some(model.into()),
            observed_revision: Some(revision.into()),
            identity_checks: vec![LocalIdentityCheck {
                stage: LocalStage::Inference,
                observed_model_id: Some(model.into()),
                observed_revision: Some(revision.into()),
                applicable: true,
                reason: None,
            }],
            private_audio_resources: vec![],
            events: vec![
                LocalStageEvent {
                    stage: LocalStage::Inference,
                    state: LocalStageState::Started,
                    reason: None,
                    runtime_version: None,
                    elapsed_ms: 0.0,
                },
                LocalStageEvent {
                    stage: LocalStage::Inference,
                    state: if negative {
                        LocalStageState::Failed
                    } else {
                        LocalStageState::Completed
                    },
                    reason: negative.then(|| "local_empty_output_not_accepted".into()),
                    runtime_version: (!negative).then(|| "declared-runtime".into()),
                    elapsed_ms: 2.0,
                },
            ],
            events_truncated: false,
            coverage: Some(LocalCoverage {
                expected_samples: row.audio.sample_count,
                covered_samples: row.audio.sample_count,
                chunks_completed: 1,
                chunks_total: 1,
                generation_tokens: 2,
                maximum_tokens: 4096,
                completion: "complete".into(),
                accepted: !negative,
            }),
            loaded_attestation: Some(attestation),
        });
        let value = if negative { "" } else { "Declared speech." };
        let observation = AsrObservation {
            provider_text: ObservedText::known(value),
            provider_text_source_kind: Some("fixture_declared_string".into()),
            filtered_text: ObservedText::known(value),
            origin: "local_model_output".into(),
            terminal: if negative {
                "local_complete_empty"
            } else {
                "speech_text"
            }
            .into(),
            filtering_reason: negative.then(|| "local_empty_output_not_accepted".into()),
            protocol_completion: "complete".into(),
            reported_model: ObservedText::known(model),
        };
        row.provider_text = observation.provider_text.clone();
        row.filtered_text = observation.filtered_text.clone();
        row.asr_terminal = observation.terminal.clone();
        row.asr_origin = observation.origin.clone();
        row.asr_observation = Some(observation);
        row.asr_request = Some(
            expected_request(&inputs, row, CacheStage::Asr, None)
                .unwrap()
                .0,
        );
        row.state = StageState::Completed {};
        (dir, inputs, report)
    }
    fn write_local_checkpoint(
        dir: &FixtureDirectory,
        inputs: &ValidatedInputs,
        report: &RunRecord,
    ) -> PathBuf {
        let path = dir.0.join("local-checkpoint.json");
        let plan = Admission::read(inputs, None, Some(&path))
            .unwrap()
            .bind(inputs, &report.identity)
            .unwrap();
        let mut owner = CheckpointOwner::new(plan).unwrap();
        owner.capture(
            inputs,
            &report.cases[0],
            &report.run_id,
            CacheStage::Asr,
            None,
        );
        owner.persist(report).unwrap();
        path
    }
    #[tokio::test]
    async fn local_v3_checkpoint_roundtrip_preserves_empty_rejection_and_skips_adapter() {
        for negative in [false, true] {
            let (dir, inputs, report) = local_fixture(negative);
            let path = write_local_checkpoint(&dir, &inputs, &report);
            let saved: CheckpointV2 = validation::json(&path).unwrap().0;
            assert_eq!(saved.schema_version, 2);
            assert_eq!(saved.successful_stage_candidates.len(), 1);
            assert_eq!(
                saved.local_sources[0]
                    .observation
                    .coverage
                    .as_ref()
                    .unwrap()
                    .accepted,
                !negative
            );
            let plan = Admission::read(&inputs, Some(&path), None)
                .unwrap()
                .bind(&inputs, &report.identity)
                .unwrap();
            let mut current = report.cases[0].clone();
            current.local_model.as_mut().unwrap().working_root = "/different/new-work".into();
            current.local_model.as_mut().unwrap().materialization_ms = 123.0;
            // These current facts were observed by a load, not an inference.
            current.local_runtime.as_mut().unwrap().coverage = None;
            let hit = plan
                .decide(&inputs, &current, CacheStage::Asr, None)
                .unwrap()
                .candidate()
                .unwrap();
            let candidate = legacy::AsrCandidate::local(
                "local_qwen3_asr_0_6b",
                crate::ondevice_models::QWEN_ASR_06_ID,
                PathBuf::from("/must-not-be-read"),
                super::super::local_asr::options(&current.requested).unwrap(),
            );
            let fixture = legacy::AsrAudioInput {
                id: &current.case_id,
                audio: "must-not-be-read.wav",
                reference: None,
                validated_audio: None,
            };
            let result = legacy::run_asr_audio_inputs(
                &candidate,
                Path::new("/must-not-be-read"),
                &[fixture],
                None,
                None,
                Some(&hit),
            )
            .await
            .unwrap();
            assert_eq!(result.provider_invoked_cases, 0);
            assert_eq!(result.cases[0].latency_ms, None);
            assert_eq!(
                result.cases[0].transcription_state,
                if negative {
                    "empty_result"
                } else {
                    "transcribed"
                }
            );
            assert_eq!(
                hit.replay()
                    .source_local_runtime
                    .unwrap()
                    .coverage
                    .unwrap()
                    .accepted,
                !negative
            );
            assert_eq!(hit.known_empty(), negative);
            current.asr_replay = Some(hit.replay());
            current.asr_request = None;
            current.asr_observation = None;
            current.provider_text = ObservedText::unknown("historical_provider_stage_not_current");
            super::super::scoring::refresh_row(&mut current);
            let scores = current.scores.as_ref().unwrap();
            let filtered = scores
                .stages
                .iter()
                .find(|s| s.stage == super::super::scoring::ScoreStage::FilteredAsr)
                .unwrap();
            assert!(matches!(filtered.state, StageState::Completed {}));
            assert_eq!(filtered.denominator.replayed, 1);
            assert_eq!(filtered.denominator.called, 0);
            if negative {
                assert!(matches!(scores.negative_outcome, StageState::Completed {}));
            }
            assert!(
                validation::decode_json::<CheckpointV1>(&path, &std::fs::read(&path).unwrap())
                    .is_err()
            );
        }
    }
    #[test]
    fn fresh_local_v3_decision_and_runner_admit_missing_optional_attestation() {
        // Structural fixture only. No runtime or provider is executed. Exercise
        // the same decision and runner selector used before adapter dispatch.
        let (dir, mut inputs, source) = local_fixture(false);
        let mut old_runtime = source.cases[0].local_runtime.clone().unwrap();
        old_runtime.loaded_attestation = None;
        old_runtime.coverage = None;
        for event in &mut old_runtime.events {
            if event.runtime_version.is_some() {
                event.runtime_version = Some("1.0.0".into());
            }
        }
        for (policy_index, purpose) in [
            None,
            Some(CachePurpose::FreshQuality),
            Some(CachePurpose::FreshReliability),
            Some(CachePurpose::FreshCloudAnchor),
            Some(CachePurpose::FreshPerformance),
        ]
        .into_iter()
        .enumerate()
        {
            inputs.recipe.as_mut().unwrap().cache_policy = purpose.map(|purpose| CachePolicy {
                purpose,
                max_age_ms: None,
            });
            inputs.recipe_sha256 = hash(inputs.recipe.as_ref().unwrap());
            for (runtime_index, runtime) in
                [None, Some(old_runtime.clone())].into_iter().enumerate()
            {
                let mut report =
                    runner::build_report(&inputs, source.identity.baseline().clone()).unwrap();
                report.cases[0].local_model = source.cases[0].local_model.clone();
                report.cases[0].local_runtime = runtime;
                let plan = Admission::read(&inputs, None, None)
                    .unwrap()
                    .bind(&inputs, &report.identity)
                    .unwrap();
                assert!(!plan.requires_local_attestation());
                assert!(matches!(
                    plan.decide(&inputs, &report.cases[0], CacheStage::Asr, None)
                        .unwrap(),
                    ReplayDecision::Miss
                ));
                let journal = RunJournal::new(
                    &dir.0
                        .join(format!("fresh-{policy_index}-{runtime_index}.json")),
                    report,
                )
                .unwrap();
                assert!(runner::job_needs_fresh_asr(&inputs, 0, &plan, &journal).unwrap());
                let current = journal.snapshot();
                assert_eq!(current.provider_attempts, 0);
                assert!(current.cases[0].asr_replay.is_none());
                assert_eq!(current.cases[0].local_runtime.is_some(), runtime_index == 1);
                if let Some(runtime) = &current.cases[0].local_runtime {
                    assert!(runtime.loaded_attestation.is_none());
                    assert_eq!(runtime.requested_revision, old_runtime.requested_revision);
                }
            }
        }
    }
    #[test]
    fn local_quality_replay_without_checkpoint_still_requires_identity_before_dispatch_and_capture()
    {
        let (dir, inputs, source) = local_fixture(false);
        let mut report = runner::build_report(&inputs, source.identity.baseline().clone()).unwrap();
        report.cases[0].local_model = source.cases[0].local_model.clone();
        let plan = Admission::read(&inputs, None, None)
            .unwrap()
            .bind(&inputs, &report.identity)
            .unwrap();
        assert!(plan.requires_local_attestation());
        assert_eq!(
            plan.decide(&inputs, &report.cases[0], CacheStage::Asr, None)
                .err()
                .unwrap()
                .safe_detail,
            "local_runtime_observation_missing"
        );
        let journal = RunJournal::new(&dir.0.join("quality-before-load.json"), report).unwrap();
        assert_eq!(
            runner::job_needs_fresh_asr(&inputs, 0, &plan, &journal)
                .err()
                .unwrap()
                .code,
            "local_resume_identity_incomplete"
        );
        assert_eq!(journal.snapshot().provider_attempts, 0);
        let mut unmeasured = source.cases[0].clone();
        unmeasured
            .local_runtime
            .as_mut()
            .unwrap()
            .loaded_attestation = None;
        assert_eq!(
            plan.decide(&inputs, &unmeasured, CacheStage::Asr, None)
                .err()
                .unwrap()
                .safe_detail,
            "local_loaded_attestation_missing"
        );
        let mut owner = CheckpointOwner::new(plan).unwrap();
        owner.capture(&inputs, &unmeasured, &source.run_id, CacheStage::Asr, None);
        assert!(owner.data.successful_stage_candidates.is_empty());
    }
    #[test]
    fn local_resume_unknown_or_changed_loaded_identity_is_error_not_fresh_fallback() {
        let (dir, inputs, report) = local_fixture(false);
        let path = write_local_checkpoint(&dir, &inputs, &report);
        let plan = Admission::read(&inputs, Some(&path), None)
            .unwrap()
            .bind(&inputs, &report.identity)
            .unwrap();
        let mut row = report.cases[0].clone();
        row.local_runtime.as_mut().unwrap().loaded_attestation = None;
        assert_eq!(
            plan.decide(&inputs, &row, CacheStage::Asr, None)
                .err()
                .unwrap()
                .code,
            "local_resume_identity_incomplete"
        );
        row = report.cases[0].clone();
        row.local_runtime
            .as_mut()
            .unwrap()
            .loaded_attestation
            .as_mut()
            .unwrap()
            .parameter_layout_sha256 = "a".repeat(64);
        assert_eq!(
            plan.decide(&inputs, &row, CacheStage::Asr, None)
                .err()
                .unwrap()
                .code,
            "local_resume_identity_mismatch"
        );
        row = report.cases[0].clone();
        row.local_model.as_mut().unwrap().derived_tokenizer_sha256 = Some("b".repeat(64));
        assert_eq!(
            plan.validate_local_binding(&row).err().unwrap().code,
            "local_resume_identity_mismatch"
        );
    }
    #[tokio::test]
    async fn local_source_checkpoint_consumes_real_guard_start_and_terminal_facts() {
        use crate::observations::{FixtureCapture, LocalStage, LocalStageGuard};
        for negative in [false, true] {
            let (dir, inputs, mut report) = local_fixture(negative);
            let capture = FixtureCapture::new(false);
            let model = report.cases[0].local_model.as_ref().unwrap();
            // Actual producing observation guard only; no runtime/model/provider
            // call is made and the remaining load/coverage facts stay declared.
            capture
                .scope(async {
                    let mut guard = LocalStageGuard::start(
                        &model.model_id,
                        &model.revision,
                        LocalStage::Inference,
                    );
                    if negative {
                        guard.failed("local_empty_output_not_accepted");
                    } else {
                        guard.completed(None, Some("declared-runtime"));
                    }
                })
                .await;
            let events = capture.snapshot().local_asr.unwrap().events;
            assert_eq!(events.len(), 2);
            report.cases[0].local_runtime.as_mut().unwrap().events = events;
            let path = write_local_checkpoint(&dir, &inputs, &report);
            let plan = Admission::read(&inputs, Some(&path), None)
                .unwrap()
                .bind(&inputs, &report.identity)
                .unwrap();
            let hit = plan
                .decide(&inputs, &report.cases[0], CacheStage::Asr, None)
                .unwrap()
                .candidate()
                .unwrap();
            assert_eq!(hit.known_empty(), negative);
            assert_eq!(
                hit.local_source()
                    .unwrap()
                    .coverage
                    .as_ref()
                    .unwrap()
                    .accepted,
                !negative
            );
        }
    }
    #[test]
    fn local_source_inference_history_admission_preserves_one_complete_invocation() {
        use crate::observations::{LocalStage, LocalStageEvent, LocalStageState};
        for negative in [false, true] {
            let (dir, inputs, mut report) = local_fixture(negative);
            // Other-stage history is not a second inference invocation. Preserve
            // these declared partial facts rather than filtering/relabeling them.
            let events = &mut report.cases[0].local_runtime.as_mut().unwrap().events;
            events.insert(
                1,
                LocalStageEvent {
                    stage: LocalStage::RuntimeSession,
                    state: LocalStageState::Unknown,
                    reason: Some("declared_other_stage_unknown".into()),
                    runtime_version: None,
                    elapsed_ms: 1.0,
                },
            );
            events.push(LocalStageEvent {
                stage: LocalStage::ModelLoad,
                state: LocalStageState::Failed,
                reason: Some("declared_other_stage_failure".into()),
                runtime_version: None,
                elapsed_ms: 1.0,
            });
            let source_runtime_sha = hash(&report.cases[0].local_runtime);
            let path = write_local_checkpoint(&dir, &inputs, &report);
            let (wire, _) = validation::json::<CheckpointV2>(&path).unwrap();
            assert_eq!(wire.successful_stage_candidates.len(), 1);
            assert_eq!(
                hash(&Some(&wire.local_sources[0].observation)),
                source_runtime_sha
            );
            assert_eq!(
                wire.local_sources[0]
                    .observation
                    .coverage
                    .as_ref()
                    .unwrap()
                    .accepted,
                !negative
            );
            let plan = Admission::read(&inputs, Some(&path), None)
                .unwrap()
                .bind(&inputs, &report.identity)
                .unwrap();
            let hit = plan
                .decide(&inputs, &report.cases[0], CacheStage::Asr, None)
                .unwrap()
                .candidate()
                .unwrap();
            assert_eq!(hit.known_empty(), negative);
            assert_eq!(hash(&hit.local_source()), source_runtime_sha);
        }
    }
    #[test]
    fn local_source_capture_and_strict_read_reject_ambiguous_inference_histories() {
        use crate::observations::{LocalStage, LocalStageEvent, LocalStageState};
        for negative in [false, true] {
            let (dir, inputs, report) = local_fixture(negative);
            let path = write_local_checkpoint(&dir, &inputs, &report);
            let (original, _) = validation::json::<CheckpointV2>(&path).unwrap();
            let valid = &report.cases[0].local_runtime.as_ref().unwrap().events;
            let unknown = LocalStageEvent {
                stage: LocalStage::Inference,
                state: LocalStageState::Unknown,
                reason: Some("local_future_dropped_or_terminal_not_observed".into()),
                runtime_version: None,
                elapsed_ms: 3.0,
            };
            let opposite = LocalStageEvent {
                stage: LocalStage::Inference,
                state: if negative {
                    LocalStageState::Completed
                } else {
                    LocalStageState::Failed
                },
                reason: (!negative).then(|| "local_empty_output_not_accepted".into()),
                runtime_version: None,
                elapsed_ms: 3.0,
            };
            let mut variants = Vec::new();
            let mut events = valid.clone();
            events.push(unknown.clone());
            variants.push(("appended-unknown", events));
            let mut events = valid.clone();
            events.push(opposite.clone());
            variants.push(("appended-opposite", events));
            let mut events = valid.clone();
            events.push(valid[1].clone());
            variants.push(("duplicate-terminal", events));
            let mut events = valid.clone();
            events.reverse();
            variants.push(("reordered-terminal", events));
            variants.push(("orphan-terminal", vec![valid[1].clone()]));
            variants.push(("unfinished-start", vec![valid[0].clone()]));
            let mut events = valid.clone();
            events.extend(valid.clone());
            variants.push(("second-unattributed-invocation", events));
            variants.push((
                "nested-start",
                vec![valid[0].clone(), valid[0].clone(), valid[1].clone()],
            ));
            variants.push(("unknown-terminal", vec![valid[0].clone(), unknown]));
            variants.push(("opposite-terminal", vec![valid[0].clone(), opposite]));
            let mut events = valid.clone();
            events[0].reason = Some("not-a-guard-start".into());
            variants.push(("invalid-start-facts", events));
            let mut events = valid.clone();
            events[1].reason = Some("conflicting_terminal_reason".into());
            variants.push(("conflicting-terminal-reason", events));
            let mut events = valid.clone();
            events[1].elapsed_ms = -1.0;
            variants.push(("invalid-terminal-timing", events));
            for (name, events) in variants {
                let mut row = report.cases[0].clone();
                row.local_runtime.as_mut().unwrap().events = events.clone();
                let before = hash(&row.local_runtime);
                let plan = Admission::read(&inputs, None, None)
                    .unwrap()
                    .bind(&inputs, &report.identity)
                    .unwrap();
                let mut owner = CheckpointOwner::new(plan).unwrap();
                owner.capture(&inputs, &row, &report.run_id, CacheStage::Asr, None);
                assert!(
                    owner.data.successful_stage_candidates.is_empty(),
                    "capture {negative}/{name}"
                );
                assert!(
                    owner.data.local_sources.is_empty(),
                    "source capture {negative}/{name}"
                );
                assert_eq!(
                    hash(&row.local_runtime),
                    before,
                    "source facts were changed"
                );
                let mut malformed = original.clone();
                malformed.local_sources[0].observation.events = events;
                let bytes = serde_json::to_vec(&malformed).unwrap();
                let selected = dir.0.join(format!("invalid-history-{name}.json"));
                // These are schema-valid histories. Strict public semantic
                // admission, rather than JSON syntax, must reject the source.
                validation::decode_json::<CheckpointV2>(&selected, &bytes).unwrap();
                std::fs::write(&selected, &bytes).unwrap();
                let failure = Admission::read(&inputs, Some(&selected), None)
                    .err()
                    .unwrap();
                let expected_code = if name == "opposite-terminal" {
                    "local_candidate_not_applicable"
                } else {
                    "local_inference_history_not_applicable"
                };
                assert_eq!(failure.code, expected_code, "read {negative}/{name}");
                assert_eq!(failure.provider_attempts, 0);
                assert_eq!(
                    std::fs::read(&selected).unwrap(),
                    bytes,
                    "rejected evidence changed"
                );
            }
        }
    }
    #[test]
    fn local_checkpoint_rejects_partial_cancelled_duplicate_and_invalid_empty_source() {
        let (dir, inputs, report) = local_fixture(true);
        let path = write_local_checkpoint(&dir, &inputs, &report);
        let bytes = std::fs::read(&path).unwrap();
        let original: CheckpointV2 = validation::decode_json(&path, &bytes).unwrap();
        for mutation in 0..6 {
            let mut cp = original.clone();
            match mutation {
                0 => {
                    cp.successful_stage_candidates[0].state = StageState::Cancelled {
                        reason: "cancelled".into(),
                    }
                }
                1 => {
                    cp.local_sources[0]
                        .observation
                        .coverage
                        .as_mut()
                        .unwrap()
                        .covered_samples -= 1
                }
                2 => {
                    cp.local_sources[0]
                        .observation
                        .coverage
                        .as_mut()
                        .unwrap()
                        .accepted = true
                }
                3 => cp
                    .successful_stage_candidates
                    .push(cp.successful_stage_candidates[0].clone()),
                4 => cp.local_sources.push(cp.local_sources[0].clone()),
                _ => cp.local_sources[0].identity.loaded.parameter_dtypes.clear(),
            }
            assert!(
                validate_data(&path, &cp.into(), &inputs, now().unwrap()).is_err(),
                "mutation {mutation}"
            );
        }
        assert!(validation::decode_json::<CheckpointV2>(&path, &bytes[..bytes.len() / 2]).is_err());
        let mut unknown: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        unknown["allow_unknown_identity"] = serde_json::json!(true);
        assert!(validation::decode_json::<CheckpointV2>(
            &path,
            &serde_json::to_vec(&unknown).unwrap()
        )
        .is_err());
        let mut row = report.cases[0].clone();
        row.cancellation_requested = true;
        let plan = Admission::read(&inputs, None, None)
            .unwrap()
            .bind(&inputs, &report.identity)
            .unwrap();
        let mut owner = CheckpointOwner::new(plan).unwrap();
        owner.capture(&inputs, &row, &report.run_id, CacheStage::Asr, None);
        assert!(owner.data.successful_stage_candidates.is_empty());
    }
    #[test]
    fn local_checkpoint_request_audio_and_executable_changes_invalidate() {
        let (dir, inputs, report) = local_fixture(false);
        let path = write_local_checkpoint(&dir, &inputs, &report);
        let mut code = report.identity.clone();
        code.baseline_mut().executable_sha256 = "f".repeat(64);
        assert_eq!(
            Admission::read(&inputs, Some(&path), None)
                .unwrap()
                .bind(&inputs, &code)
                .err()
                .unwrap()
                .code,
            "resume_identity_mismatch"
        );
        let mut audio = inputs;
        audio.cases[0].audio.pcm_sha256 = "e".repeat(64);
        assert_eq!(
            Admission::read(&audio, Some(&path), None)
                .unwrap()
                .bind(&audio, &report.identity)
                .err()
                .unwrap()
                .code,
            "resume_stage_identity_mismatch"
        );
    }

    #[test]
    fn checkpoint_admission_rejects_historical_stage_key_and_replays_current_writer_key() {
        let d = FixtureDirectory::new();
        let (inputs, mut report, fixture_checkpoint) = fixture(&d, false, 1000);
        let current_path = d.0.join("current-stage-key.json");
        let plan = Admission::read(&inputs, None, Some(&current_path))
            .unwrap()
            .bind(&inputs, &report.identity)
            .unwrap();
        let mut owner = CheckpointOwner::new(plan).unwrap();
        // Declared producer-shaped candidate from the existing admission
        // fixture, not a cloud receipt or model-quality observation.
        let source = &fixture_checkpoint.successful_stage_candidates[0];
        let observation = source.asr_observation.as_ref().unwrap();
        let row = &mut report.cases[0];
        row.state = StageState::Completed {};
        row.provider_text = observation.provider_text.clone();
        row.filtered_text = observation.filtered_text.clone();
        row.asr_origin = observation.origin.clone();
        row.asr_terminal = observation.terminal.clone();
        row.asr_observation = Some(observation.clone());
        row.asr_request = Some(source.request.clone());
        owner.capture(&inputs, row, &report.run_id, CacheStage::Asr, None);
        owner.persist(&report).unwrap();
        drop(owner);
        let (current, _) = validation::json::<CheckpointV1>(&current_path).unwrap();
        assert_eq!(current.successful_stage_candidates.len(), 1);
        let current_key = &current.successful_stage_candidates[0].input_sha256;
        let historical_key = super::super::negative_tests::historical_v1_stage_input_sha(
            &current.identity,
            &report.cases[0],
            &inputs.cases[0],
        );
        assert_ne!(current_key, &historical_key);

        // Reconstruct the old algorithm's key without changing its other
        // candidate facts; admission must reject it, never restamp or migrate.
        let mut historical = current.clone();
        historical.successful_stage_candidates[0].input_sha256 = historical_key;
        let historical_path = d.0.join("historical-stage-key.json");
        let historical_bytes = serde_json::to_vec(&historical).unwrap();
        std::fs::write(&historical_path, &historical_bytes).unwrap();
        assert_eq!(
            Admission::read(&inputs, Some(&historical_path), None)
                .unwrap()
                .bind(&inputs, &report.identity)
                .err()
                .unwrap()
                .code,
            "resume_stage_identity_mismatch"
        );
        assert_eq!(std::fs::read(&historical_path).unwrap(), historical_bytes);

        let replay = Admission::read(&inputs, Some(&current_path), None)
            .unwrap()
            .bind(&inputs, &report.identity)
            .unwrap();
        let candidate = replay
            .decide(&inputs, &report.cases[0], CacheStage::Asr, None)
            .unwrap()
            .candidate()
            .expect("current writer's candidate must replay");
        assert_eq!(candidate.value(), source.value);
        assert_eq!(&candidate.replay().input_sha256, current_key);
    }
    #[test]
    fn replay_binds_actual_executable_without_promoting_or_matching_live_source_metadata() {
        let d = FixtureDirectory::new();
        let (inputs, report, cp) = fixture(&d, false, 1000);
        // Exercise the producer's strict V1 wire identity and the real decoded
        // checkpoint admission boundary, not only an in-memory member.
        let checkpoint = d.0.join("artifact-bound-checkpoint.json");
        let original_plan = admitted(&inputs, &report, cp.clone(), Some(checkpoint.clone()));
        let mut owner = CheckpointOwner::new(original_plan).unwrap();
        owner.persist(&report).unwrap();
        drop(owner);
        let (saved, _) = validation::json::<CheckpointV1>(&checkpoint).unwrap();
        assert_eq!(hash(&saved.identity), hash(&cp.identity));
        let mut same_binary = report.identity.clone();
        let baseline = same_binary.baseline_mut();
        baseline.schema_version = 1;
        baseline.source_provenance = SourceProvenance::RuntimeCheckoutLegacy;
        baseline.source_sha256 = Some(sha256(b"later live checkout, not compiled code"));
        baseline.source_commit = None;
        baseline.dirty = None;
        baseline.lockfile_sha256.clear();
        let plan = Admission::read(&inputs, Some(&checkpoint), None)
            .unwrap()
            .bind(&inputs, &same_binary)
            .unwrap();
        assert_eq!(plan.entries.len(), cp.successful_stage_candidates.len());
        assert!(plan
            .decide(&inputs, &report.cases[0], CacheStage::Asr, None)
            .unwrap()
            .candidate()
            .is_some());
        assert!(!plan.identity.baseline().has_build_source_snapshot());
        let mut different_binary = same_binary;
        different_binary.baseline_mut().executable_sha256 = sha256(b"different artifact");
        assert_eq!(
            Admission::read(&inputs, Some(&checkpoint), None)
                .unwrap()
                .bind(&inputs, &different_binary)
                .err()
                .unwrap()
                .code,
            "resume_identity_mismatch"
        );
    }
    #[test]
    fn checkpoint_rejects_duplicate_failed_unknown_truncated_and_legacy_inputs() {
        let d = FixtureDirectory::new();
        let (inputs, report, cp) = fixture(&d, false, 1000);
        let mut duplicate = cp.clone();
        duplicate
            .successful_stage_candidates
            .push(duplicate.successful_stage_candidates[0].clone());
        assert_eq!(
            validate_checkpoint(&d.0, &duplicate, &inputs, now().unwrap())
                .unwrap_err()
                .code,
            "duplicate_stage"
        );
        for state in [
            StageState::Failed {
                reason: "failure".into(),
            },
            StageState::Cancelled {
                reason: "late".into(),
            },
            StageState::Unknown {
                reason: "partial".into(),
            },
        ] {
            let mut bad = cp.clone();
            bad.successful_stage_candidates[0].state = state;
            assert_eq!(
                validate_checkpoint(&d.0, &bad, &inputs, now().unwrap())
                    .unwrap_err()
                    .code,
                "candidate_not_applicable"
            );
        }
        let mut v = serde_json::to_value(&cp).unwrap();
        v["successful_stage_candidates"][0]["stage"] = serde_json::json!("final_text");
        assert!(serde_json::from_value::<CheckpointV1>(v).is_err());
        let file = d.0.join("checkpoint.json");
        std::fs::write(&file, b"{\"artifact_type\":\"voiceflow_checkpoint\",").unwrap();
        assert_eq!(
            Admission::read(&inputs, Some(&file), None)
                .err()
                .unwrap()
                .code,
            "schema"
        );
        std::fs::write(&file,br#"{"schema_version":1,"corpus_fingerprint":"fnv","cleanup_code_fingerprint":"fnv","checkpoint_candidates":[]}"#).unwrap();
        assert_eq!(
            Admission::read(&inputs, Some(&file), None)
                .err()
                .unwrap()
                .code,
            "legacy_identity_incomplete"
        );
        let mut wrong = cp.clone();
        wrong.complete_jobs = vec![
            report.cases[0].job_id.clone(),
            report.cases[0].job_id.clone(),
        ];
        assert_eq!(
            validate_checkpoint(&d.0, &wrong, &inputs, now().unwrap())
                .unwrap_err()
                .code,
            "duplicate_or_unknown_job"
        );
    }
    #[test]
    fn changed_request_reference_audio_context_code_and_clock_fail_before_lookup() {
        let d = FixtureDirectory::new();
        let (inputs, report, cp) = fixture(&d, false, 1000);
        let mut changed = cp.clone();
        changed.successful_stage_candidates[0]
            .request
            .fields_constructed
            .insert("language".into(), serde_json::json!("zh"));
        changed.successful_stage_candidates[0].request_sha256 =
            hash(&changed.successful_stage_candidates[0].request);
        // Even an updated stored digest cannot override the current production builder.
        assert_eq!(
            Admission {
                loaded: Some(changed.into()),
                input_sha: Some("b".repeat(64)),
                output: None
            }
            .bind(&inputs, &report.identity)
            .err()
            .unwrap()
            .code,
            "resume_stage_identity_mismatch"
        );
        let mut changed_source = report.identity.clone();
        changed_source.baseline_mut().executable_sha256 =
            sha256(b"different actual compiled artifact");
        assert_eq!(
            Admission {
                loaded: Some(cp.clone().into()),
                input_sha: Some("b".repeat(64)),
                output: None
            }
            .bind(&inputs, &changed_source)
            .err()
            .unwrap()
            .code,
            "resume_identity_mismatch"
        );
        // Exercise stage-input and current-builder admission independently of
        // the outer identity gate, without relabeling arbitrary hashes as inputs.
        for factor in ["language", "reference", "pcm", "context"] {
            let d = FixtureDirectory::new();
            let (mut current, current_report, saved) = fixture(&d, false, 1000);
            match factor {
                "language" => {
                    current.recipe.as_mut().unwrap().jobs[0].recognition_language =
                        RecognitionLanguage::En
                }
                "reference" => {
                    let super::super::validation::CaseData::Speech(case) =
                        &mut current.cases[0].case
                    else {
                        unreachable!()
                    };
                    case.reference.push_str(" revised reference");
                }
                "pcm" => {
                    let mut bytes = current.cases[0].bytes.to_vec();
                    *bytes.last_mut().unwrap() = 1;
                    current.cases[0].audio =
                        validation::wav(Path::new("changed.wav"), &bytes).unwrap();
                }
                "context" => {
                    current.control_fixtures[0].context =
                        Some(super::super::controls::FixtureSnapshot {
                            canonical_path: d.0.join("explicit-context.json"),
                            relative_path: "explicit-context.json".into(),
                            raw: b"fixture snapshot".to_vec(),
                            sha256: sha256(b"fixture snapshot"),
                            parsed: super::super::controls::ContextFixtureV1 {
                                artifact_type: "context_fixture".into(),
                                schema_version: 1,
                                text: "changed explicit context".into(),
                            },
                        });
                }
                _ => unreachable!(),
            }
            assert_eq!(
                Admission {
                    loaded: Some(saved.into()),
                    input_sha: Some("b".repeat(64)),
                    output: None
                }
                .bind(&current, &current_report.identity)
                .err()
                .unwrap()
                .code,
                "resume_stage_identity_mismatch",
                "{factor}"
            );
        }
        let clock = now().unwrap();
        let mut expired = cp.clone();
        expired.successful_stage_candidates[0].source_created_unix_ms = clock - 60_001;
        expired.created_unix_ms = clock;
        assert_eq!(
            validate_checkpoint(&d.0, &expired, &inputs, clock)
                .unwrap_err()
                .code,
            "resume_expired"
        );
        let mut future = cp.clone();
        future.created_unix_ms = clock + 1;
        assert_eq!(
            validate_checkpoint(&d.0, &future, &inputs, clock)
                .unwrap_err()
                .code,
            "checkpoint_budget_or_time"
        );
        let plan = admitted(&inputs, &report, cp, None);
        assert!(plan
            .decide(&inputs, &report.cases[0], CacheStage::Asr, None)
            .unwrap()
            .candidate()
            .is_some());
        let mut row = report.cases[0].clone();
        row.requested.local_rules.revision = "changed-current-rules".into();
        assert!(plan
            .decide(&inputs, &row, CacheStage::Asr, None)
            .unwrap()
            .candidate()
            .is_none());
    }
    #[tokio::test]
    async fn production_replay_has_zero_dispatch_and_current_guard_can_reject_without_old_final_text(
    ) {
        for reject in [false, true] {
            let d = FixtureDirectory::new();
            // This is a replay/guard semantic fixture, not a 1s performance
            // requirement. Select its bounded budget before identity capture;
            // separate deadline/TTL/cancellation fixtures retain 1000ms.
            let (inputs, mut report, mut cp) = fixture(&d, true, 60_000);
            if reject {
                let s = &mut cp.successful_stage_candidates[1];
                s.value = "Change API v2.".into();
                s.value_sha256 = sha256(s.value.as_bytes());
            }
            let output = d.0.join("new-checkpoint.json");
            let plan = admitted(&inputs, &report, cp, Some(output.clone()));
            report.replay = plan.summary();
            let journal = RunJournal::new_with_replay(
                &d.0.join("replay-run.json"),
                report,
                Some(plan.clone()),
            )
            .unwrap();
            let needs_asr = runner::job_needs_fresh_asr(&inputs, 0, &plan, &journal);
            let needs_cleanup = runner::job_needs_fresh_cleanup(&inputs, 0, &plan, &journal);
            eprintln!(
                "VF14_REPLAY_PREFLIGHT {}",
                serde_json::json!({
                    "reject": reject, "needs_asr": &needs_asr,
                    "needs_cleanup": &needs_cleanup, "row": journal.row(0),
                    "admitted_candidates": plan.entries.values().map(AsRef::as_ref).collect::<Vec<_>>()
                })
            );
            assert!(!needs_asr.unwrap(), "reject={reject}: ASR must replay");
            assert!(
                !needs_cleanup.unwrap(),
                "reject={reject}: cleanup must replay"
            );
            let mut cursor = 0;
            let mut pending_capture = None;
            // Empty credential map guarantees no external provider can be called,
            // even if a regression mistakenly takes a fresh branch.
            let executed = runner::execute(
                &inputs,
                &BTreeMap::new(),
                &journal,
                &tokio_util::sync::CancellationToken::new(),
                &mut cursor,
                &plan,
                runner::ExecutionResources {
                    local_asr: None,
                    local_cleanup: None,
                    pending_capture: &mut pending_capture,
                },
            )
            .await;
            // Keep the original execute-then-persist success requirements, but
            // retain the error and full synthetic row before either unwrap.
            let persisted = executed.as_ref().ok().map(|_| journal.persist());
            let record = journal.snapshot();
            let row = &record.cases[0];
            let prepared = match &row.prepared_text {
                ObservedText::KnownValue { value } => Some(value.as_str()),
                ObservedText::KnownEmpty {} => Some(""),
                ObservedText::Unknown { .. } => None,
            };
            let asr_lookup = plan
                .decide(&inputs, row, CacheStage::Asr, None)
                .map(|decision| decision.candidate().map(|candidate| candidate.replay()));
            let cleanup_lookup = prepared.map(|source| {
                plan.decide(&inputs, row, CacheStage::Cleanup, Some(source))
                    .map(|decision| decision.candidate().map(|candidate| candidate.replay()))
            });
            let cleanup_identity = prepared.and_then(|source| expected_request(&inputs, row, CacheStage::Cleanup, Some(source)))
                .map(|(request, parent)| serde_json::json!({
                    "request": request, "prepared_parent": &parent,
                    "input_sha256": plan.input_sha(&inputs, row, CacheStage::Cleanup, parent.as_deref())
                }));
            let diagnostic = serde_json::json!({
                "reject": reject, "cursor": cursor, "execute": &executed,
                "persist": &persisted, "record": &record,
                "post_execution_asr_lookup": asr_lookup,
                "post_execution_cleanup_lookup": cleanup_lookup,
                "current_cleanup_identity": cleanup_identity,
                "lookup_scope": "post-execution diagnostic at current clock, not a claim of earlier dispatch selection"
            });
            eprintln!("VF14_REPLAY_RESULT {diagnostic}");
            executed.unwrap();
            persisted
                .expect("successful execution must attempt persistence")
                .unwrap();
            assert_eq!(record.provider_attempts, 0);
            assert_eq!(row.attempts, 0);
            assert!(row.asr_request.is_none() && row.cleanup_request.is_none());
            assert!(row.asr_replay.is_some(), "ASR replay missing: {diagnostic}");
            assert!(
                row.cleanup_replay.is_some(),
                "cleanup replay missing: {diagnostic}"
            );
            assert!(row.finalizer_identity_sha256.is_some());
            let ObservedText::KnownValue { value } = &row.final_text else {
                panic!("current finalizer output required")
            };
            assert!(value.contains("not") && value.contains("v1"));
            assert!(!value.contains("v2"));
            assert_eq!(
                matches!(
                    row.cleanup_replay.as_ref().unwrap().current_guard,
                    StageState::Failed { .. }
                ),
                reject
            );
            let summary = record.replay.unwrap();
            assert_eq!(summary.resumed_asr, 1);
            assert_eq!(summary.resumed_cleanup, 1);
            assert!(
                !summary.current_quality_eligible
                    && !summary.current_performance_eligible
                    && !summary.current_reliability_eligible
            );
            let (saved, _) = validation::json::<CheckpointV1>(&output).unwrap();
            assert_eq!(
                saved.successful_stage_candidates.len(),
                if reject { 1 } else { 2 }
            );
            assert!(saved
                .successful_stage_candidates
                .iter()
                .all(|s| s.source_created_unix_ms <= saved.created_unix_ms));
        }
    }
    #[tokio::test]
    async fn cancellation_before_replay_preserves_prior_complete_stage_without_current_delivery() {
        let d = FixtureDirectory::new();
        let (inputs, mut report, cp) = fixture(&d, false, 1000);
        let output = d.0.join("cancel-checkpoint.json");
        let plan = admitted(&inputs, &report, cp, Some(output.clone()));
        report.replay = plan.summary();
        let journal =
            RunJournal::new_with_replay(&d.0.join("cancel-run.json"), report, Some(plan.clone()))
                .unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        let mut cursor = 0;
        let mut pending_capture = None;
        runner::execute(
            &inputs,
            &BTreeMap::new(),
            &journal,
            &cancel,
            &mut cursor,
            &plan,
            runner::ExecutionResources {
                local_asr: None,
                local_cleanup: None,
                pending_capture: &mut pending_capture,
            },
        )
        .await
        .unwrap();
        journal.persist().unwrap();
        let record = journal.snapshot();
        assert_eq!(record.provider_attempts, 0);
        assert!(matches!(record.terminal, StageState::Cancelled { .. }));
        assert!(record.cases[0].cancellation_requested);
        assert!(matches!(
            record.cases[0].final_text,
            ObservedText::Unknown { .. }
        ));
        assert_eq!(record.replay.unwrap().resumed_asr, 0);
        let (saved, _) = validation::json::<CheckpointV1>(&output).unwrap();
        assert_eq!(saved.successful_stage_candidates.len(), 1);
        assert!(saved.complete_jobs.is_empty());
    }
    #[test]
    fn fresh_candidate_capture_rejects_cancelled_partial_and_negative_outcomes() {
        let d = FixtureDirectory::new();
        let (mut inputs, report, cp) = fixture(&d, false, 1000);
        let plan = Admission {
            loaded: None,
            input_sha: None,
            output: None,
        }
        .bind(&inputs, &report.identity)
        .unwrap();
        let mut owner = CheckpointOwner::new(plan).unwrap();
        let mut row = report.cases[0].clone();
        let source = &cp.successful_stage_candidates[0];
        row.filtered_text = ObservedText::known(&source.value);
        row.asr_request = Some(source.request.clone());
        row.asr_observation = source.asr_observation.clone();
        row.asr_origin = "provider_response".into();
        row.asr_terminal = "speech_text".into();
        row.cancellation_requested = true;
        owner.capture(&inputs, &row, &report.run_id, CacheStage::Asr, None);
        assert!(owner.data.successful_stage_candidates.is_empty());
        row.cancellation_requested = false;
        row.asr_observation.as_mut().unwrap().protocol_completion = "length".into();
        owner.capture(&inputs, &row, &report.run_id, CacheStage::Asr, None);
        assert!(owner.data.successful_stage_candidates.is_empty());
        row.asr_observation = source.asr_observation.clone();
        let speech = inputs.cases[0].case.clone();
        inputs.cases[0].case = super::super::validation::CaseData::NonSpeech(NonSpeechCaseV1 {
            case_id: row.case_id.clone(),
            adjacent_wav: "one.wav".into(),
            audio_sha256: row.audio.audio_sha256.clone(),
            category: NonSpeechCategory::Silence,
            source_review_id: "fixture".into(),
        });
        owner.capture(&inputs, &row, &report.run_id, CacheStage::Asr, None);
        assert!(owner.data.successful_stage_candidates.is_empty());
        inputs.cases[0].case = speech;
        owner.capture(&inputs, &row, &report.run_id, CacheStage::Asr, None);
        assert_eq!(owner.data.successful_stage_candidates.len(), 1);
    }
    #[test]
    fn atomic_checkpoint_collision_preserves_existing_and_no_payload_is_published() {
        let d = FixtureDirectory::new();
        let (inputs, report, cp) = fixture(&d, false, 1000);
        let checkpoint = d.0.join("checkpoint.json");
        std::fs::write(&checkpoint, b"foreign sentinel").unwrap();
        assert_eq!(
            Admission::read(&inputs, None, Some(&checkpoint))
                .err()
                .unwrap()
                .code,
            "output_exists"
        );
        assert_eq!(std::fs::read(&checkpoint).unwrap(), b"foreign sentinel");
        let plan = admitted(&inputs, &report, cp, Some(d.0.join("new-checkpoint.json")));
        let journal =
            RunJournal::new_with_replay(&d.0.join("new-run.json"), report.clone(), Some(plan))
                .unwrap();
        journal.persist().unwrap();
        let checkpoint = d.0.join("new-checkpoint.json");
        let moved = d.0.join("held-checkpoint.json");
        std::fs::rename(&checkpoint, &moved).unwrap();
        std::fs::write(&checkpoint, b"foreign replacement").unwrap();
        let failed = journal.persist().unwrap_err();
        let failed = journal.fail(failed, None);
        assert_eq!(failed.provider_attempts, 0);
        let serialized = serde_json::to_value(&failed).unwrap();
        assert_eq!(
            serialized["checkpoint_outcome"],
            serde_json::to_value(&failed.checkpoint_outcome).unwrap()
        );
        assert_eq!(
            serialized["artifact_outcome"],
            serde_json::to_value(&failed.artifact_outcome).unwrap()
        );
        assert_eq!(std::fs::read(&checkpoint).unwrap(), b"foreign replacement");
        assert!(moved.is_file());
        assert!(failed
            .checkpoint_outcome
            .as_ref()
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .applicable_path
            .is_none());
        assert!(failed
            .artifact_outcome
            .current
            .as_ref()
            .unwrap()
            .applicable_path
            .is_some());
        let (terminal, _) = validation::json::<RunRecord>(&d.0.join("new-run.json")).unwrap();
        assert!(matches!(terminal.terminal, StageState::Failed { .. }));
        assert!(terminal
            .replay
            .unwrap()
            .checkpoint_artifact_outcome
            .unwrap()
            .current
            .unwrap()
            .applicable_path
            .is_none());
    }
    #[test]
    fn checkpoint_acquisition_failure_still_installs_independent_terminal_report() {
        let d = FixtureDirectory::new();
        let mut checkpoint_parent = FixtureDirectory::new();
        let (inputs, report, _) = fixture(&d, false, 1000);
        let selected = checkpoint_parent.0.join("checkpoint.json");
        let plan = Admission::read(&inputs, None, Some(&selected))
            .unwrap()
            .bind(&inputs, &report.identity)
            .unwrap();
        let sentinel = checkpoint_parent.0.join("caller-sentinel");
        std::fs::write(&sentinel, b"preserve parent content").unwrap();
        let relocated = checkpoint_parent.0.with_extension("moved");
        std::fs::rename(&checkpoint_parent.0, &relocated).unwrap();
        checkpoint_parent.relocate(&relocated);
        let output = d.0.join("acquisition-failure-run.json");
        let failure = RunJournal::new_with_replay(&output, report, Some(plan))
            .err()
            .unwrap();
        assert_eq!(failure.code, "checkpoint_error");
        assert_eq!(failure.provider_attempts, 0);
        assert!(matches!(failure.persistence, StageState::Completed {}));
        let acquisition = failure.checkpoint_acquisition.as_ref().unwrap();
        assert!(!acquisition.writer_acquired);
        assert!(matches!(acquisition.state, StageState::Failed { .. }));
        assert!(matches!(
            acquisition.path_applicability,
            StageState::Unknown { .. }
        ));
        let checkpoint = failure.checkpoint_outcome.as_ref().unwrap();
        assert!(checkpoint.current.is_none() && checkpoint.last_confirmed.is_none());
        assert!(failure.artifact_outcome.last_confirmed.is_some());
        let (saved, _) = validation::json::<RunRecord>(&output).unwrap();
        assert!(matches!(saved.terminal, StageState::Failed { .. }));
        assert_eq!(saved.provider_attempts, 0);
        assert!(saved
            .cases
            .iter()
            .all(|r| r.attempts == 0 && matches!(r.state, StageState::NotRun {})));
        assert!(
            !saved
                .replay
                .unwrap()
                .checkpoint_acquisition
                .unwrap()
                .writer_acquired
        );
        assert_eq!(
            std::fs::read(relocated.join("caller-sentinel")).unwrap(),
            b"preserve parent content"
        );
        assert!(!selected.exists());
    }
    #[tokio::test]
    async fn ttl_expiry_during_real_pacing_is_terminal_not_a_fresh_cache_miss() {
        let d = FixtureDirectory::new();
        let (mut inputs, original, mut checkpoint) = fixture(&d, false, 1000);
        let job = &mut inputs.recipe.as_mut().unwrap().jobs[0];
        job.repeats = 2;
        job.pacing.gap_ms = 10;
        job.run_budget.deadline_ms = 60_000;
        inputs.recipe_sha256 = hash(inputs.recipe.as_ref().unwrap());
        let report = runner::build_report(&inputs, original.identity.baseline().clone()).unwrap();
        let identity = Admission {
            loaded: None,
            input_sha: None,
            output: None,
        }
        .bind(&inputs, &report.identity)
        .unwrap();
        checkpoint.identity = report.identity.speech().unwrap().clone();
        checkpoint.successful_stage_candidates[0].input_sha256 =
            identity.input_sha(&inputs, &report.cases[0], CacheStage::Asr, None);
        let mut repeat = checkpoint.successful_stage_candidates[0].clone();
        repeat.repeat = 1;
        checkpoint.successful_stage_candidates.push(repeat);
        let start = checkpoint.successful_stage_candidates[0].source_created_unix_ms;
        let plan = admitted(&inputs, &report, checkpoint, None);
        plan.set_clock(TestClock::Known(start));
        let journal =
            RunJournal::new_with_replay(&d.0.join("paced-run.json"), report, Some(plan.clone()))
                .unwrap();
        assert!(!runner::job_needs_fresh_asr(&inputs, 0, &plan, &journal).unwrap());
        let mut cursor = 0;
        let keys = BTreeMap::new();
        let cancel = tokio_util::sync::CancellationToken::new();
        // The real runner reaches its pacing await before the clock changes.
        // Current-thread join/yield needs no wall-clock TTL sleep or fake pacer.
        let advance_clock = async {
            tokio::task::yield_now().await;
            plan.set_clock(TestClock::Known(start + 60_001));
        };
        let mut pending_capture = None;
        let (result, ()) = tokio::join!(
            runner::execute(
                &inputs,
                &keys,
                &journal,
                &cancel,
                &mut cursor,
                &plan,
                runner::ExecutionResources {
                    local_asr: None,
                    local_cleanup: None,
                    pending_capture: &mut pending_capture,
                }
            ),
            advance_clock,
        );
        let failure = journal.fail(result.err().unwrap(), Some(cursor));
        assert_eq!(failure.code, "resume_expired");
        assert_eq!(failure.provider_attempts, 0);
        let record = journal.snapshot();
        assert_eq!(record.cases.len(), 2);
        assert!(matches!(record.cases[0].state, StageState::Completed {}));
        assert!(
            matches!(&record.cases[1].state, StageState::Failed { reason } if reason == "resume_expired")
        );
        assert!(record.cases.iter().all(|r| r.attempts == 0));
        let summary = record.replay.unwrap();
        assert_eq!(summary.resumed_asr, 1);
        assert_eq!(summary.failed_cases, 1);
        assert_eq!(summary.fresh_asr_cases, 0);
    }
    #[test]
    fn unavailable_or_backwards_clock_is_error_for_hits_and_misses() {
        let d = FixtureDirectory::new();
        let (inputs, report, checkpoint) = fixture(&d, false, 1000);
        let source_time = checkpoint.successful_stage_candidates[0].source_created_unix_ms;
        let plan = admitted(&inputs, &report, checkpoint, None);
        for clock in [TestClock::Unavailable, TestClock::Known(source_time - 1)] {
            plan.set_clock(clock);
            for stage in [CacheStage::Asr, CacheStage::Cleanup] {
                let cause = plan
                    .decide(&inputs, &report.cases[0], stage, None)
                    .err()
                    .unwrap();
                assert_eq!(cause.code, "resume_time_unknown");
            }
        }
    }
    #[tokio::test]
    async fn expiry_after_loopback_receipt_prevents_the_actual_cleanup_retry_dispatch() {
        let d = FixtureDirectory::new();
        let (inputs, report, checkpoint) = fixture(&d, false, 1000);
        let source_time = checkpoint.successful_stage_candidates[0].source_created_unix_ms;
        let plan = admitted(&inputs, &report, checkpoint, None);
        plan.set_clock(TestClock::Known(source_time));
        let journal = RunJournal::new_with_replay(
            &d.0.join("retry-expiry-run.json"),
            report,
            Some(plan.clone()),
        )
        .unwrap();
        let mut server = crate::test_http::spawn_scripted(vec![
            crate::test_http::ResponseStep {
                status: 429,
                body: "{}".into(),
                delay: std::time::Duration::from_millis(20),
            },
            crate::test_http::ResponseStep {
                status: 200,
                body: "{}".into(),
                delay: std::time::Duration::ZERO,
            },
        ])
        .await;
        let candidate = legacy::CleanupCandidate::from_material(
            "fixture-cleanup",
            crate::engine::EngineProvider::Custom,
            "fixture".into(),
            server.url.clone(),
            &super::super::authorization::KeyMaterial::synthetic("fixture-key"),
        );
        let mut case = runner::eval_case(
            inputs.cases[0]
                .case
                .speech()
                .expect("429 TTL fixture requires its declared speech case"),
            "Keep this fixture sentence.",
            &inputs.recipe.as_ref().unwrap().jobs[0],
        );
        case.cleanup = "standard".into();
        case.mode = "normal".into();
        let cases = [case];
        let mut legacy_state = legacy::ResumeState::default();
        let mut pacer = legacy::RequestPacer::new(std::time::Duration::ZERO);
        let observer = journal.observer(0);
        let future = legacy::run_cleanup_candidate(
            &candidate,
            legacy::CleanupCandidateRun {
                cases: &cases,
                blocked_reason: None,
                resume_state: &mut legacy_state,
                checkpoint_path: None,
                corpus_fingerprint: inputs.selection.corpus_sha().expect("speech-only cleanup"),
                cleanup_code_fingerprint: "fixture",
                pacer: &mut pacer,
                observer: Some(&observer),
                preparation: None,
                replay: None,
            },
        );
        tokio::pin!(future);
        tokio::select! {
            _ = &mut future => panic!("cleanup ended before controlled first receipt"),
            receipt = server.receipts.recv() => assert_eq!(receipt, Some(1)),
            _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => panic!("no loopback receipt"),
        }
        plan.set_clock(TestClock::Known(source_time + 60_001));
        let result = tokio::time::timeout(std::time::Duration::from_secs(3), &mut future)
            .await
            .unwrap();
        let failure = journal.fail(result.err().unwrap(), Some(0));
        assert_eq!(failure.code, "resume_expired");
        assert_eq!(failure.provider_attempts, 1);
        assert!(server.receipts.try_recv().is_err());
        let record = journal.snapshot();
        assert_eq!(record.cases[0].attempt_facts.len(), 1);
        assert_eq!(record.cases[0].attempt_facts[0].ordinal, 1);
        assert!(matches!(
            record.cases[0].attempt_facts[0].server_received,
            StageState::Unknown { .. }
        ));
    }
    #[test]
    fn fresh_purposes_and_unknown_local_runtime_never_admit_candidate_replay() {
        let d = FixtureDirectory::new();
        let (mut inputs, report, cp) = fixture(&d, false, 1000);
        for purpose in [
            CachePurpose::FreshQuality,
            CachePurpose::FreshReliability,
            CachePurpose::FreshCloudAnchor,
            CachePurpose::FreshPerformance,
        ] {
            inputs.recipe.as_mut().unwrap().cache_policy = Some(CachePolicy {
                purpose,
                max_age_ms: None,
            });
            assert_eq!(
                Admission::read(&inputs, Some(&d.0.join("not-read")), None)
                    .err()
                    .unwrap()
                    .code,
                "cache_policy_not_applicable"
            );
        }
        inputs.recipe.as_mut().unwrap().cache_policy = Some(CachePolicy {
            purpose: CachePurpose::QualityReplay,
            max_age_ms: Some(60_000),
        });
        inputs.recipe.as_mut().unwrap().jobs[0].candidate_id = "local_qwen3_asr_0_6b".into();
        let mut local = report.cases[0].clone();
        local.requested = inputs.recipe.as_ref().unwrap().jobs[0].clone();
        let plan = Admission {
            loaded: None,
            input_sha: None,
            output: None,
        }
        .bind(&inputs, &report.identity)
        .unwrap();
        assert!(plan
            .decide(&inputs, &local, CacheStage::Asr, None)
            .unwrap()
            .candidate()
            .is_none());
        let mut wrong = cp;
        wrong.successful_stage_candidates[0].job_id = "job-0:local_qwen3_asr_0_6b".into();
        assert_eq!(
            validate_checkpoint(&d.0, &wrong, &inputs, now().unwrap())
                .unwrap_err()
                .code,
            "candidate_not_applicable"
        );
    }
}
