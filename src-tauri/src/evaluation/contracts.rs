//! Versioned external evaluation artifacts. Unknown fields fail closed.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

macro_rules! strict {
    ($name:ident { $($field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Debug, Clone, Deserialize, Serialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name { $(pub $field: $ty),* }
    };
}
strict!(Provenance {
    kind: String,
    note: String,
    reviewed: bool
});
strict!(AudioManifestV1 { schema_version: u32, provenance: Provenance, cases: Vec<AudioCaseV1> });
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AudioCaseV1 {
    pub id: String,
    pub audio: String,
    pub reference: String,
    pub expected_final: String,
    pub scenario: String,
    pub family: String,
    pub focus_kind: String,
    pub cleanup: String,
    #[serde(default)]
    pub protected: Vec<String>,
    #[serde(default)]
    pub reference_variants: Vec<String>,
}
strict!(CorpusSidecarV1 { artifact_type: String, schema_version: u32, corpus_id: String,
    manifests: Vec<ManifestIdentity>, cases: Vec<CorpusCaseV1> });
strict!(ManifestIdentity {
    relative_path: String,
    sha256: String
});
strict!(CorpusCaseV1 {
    case_id: String,
    manifest_sha256: String,
    audio_sha256: String,
    pcm_sha256: String,
    source_kind: SourceKind,
    rights_review_id: String,
    speaker_id: String,
    source_session_id: String,
    template_family_id: String,
    split: Split,
    language: Language,
    conditions: Conditions,
    reference_sha256: String,
    annotation_revision: String
});
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Split {
    Dev,
    Test,
    Train,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    HumanRecorded,
    Synthetic,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Zh,
    En,
    Mixed,
}
strict!(Conditions {
    noise: String,
    microphone: String,
    speaking_style: String
});
strict!(NonSpeechIndexV1 { artifact_type: String, schema_version: u32, provenance: Provenance,
    cases: Vec<NonSpeechCaseV1> });
strict!(NonSpeechCaseV1 {
    case_id: String,
    adjacent_wav: String,
    audio_sha256: String,
    category: NonSpeechCategory,
    source_review_id: String
});
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NonSpeechCategory {
    Silence,
    Noise,
    Music,
    NonSpeech,
}
strict!(RunRecipeV1 { artifact_type: String, schema_version: u32, recipe_id: String,
    corpus_sidecar_sha256: String, track: Track, jobs: Vec<JobV1>, gui_fixture: Option<GuiFixture>, cache_policy: Option<CachePolicy>, scoring: Option<super::scoring::ScoringConfig> });
strict!(RunRecipeV2 { artifact_type: String, schema_version: u32, recipe_id: String,
    input: ExperimentInputV2, track: Track, jobs: Vec<JobV1>, gui_fixture: Option<GuiFixture>, cache_policy: Option<CachePolicy>, scoring: Option<super::scoring::ScoringConfig> });
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExperimentInputV2 {
    NonSpeech { index_sha256: String },
}
strict!(RunRecipeV3 { artifact_type: String, schema_version: u32, recipe_id: String,
    input: ExperimentInputV3, track: Track, jobs: Vec<JobV1>, gui_fixture: Option<GuiFixture>, cache_policy: Option<CachePolicy>, scoring: Option<super::scoring::ScoringConfig> });
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExperimentInputV3 {
    AsrSpeech {
        purpose: super::speech_input::Purpose,
        speech_index: ControlFixtureLocator,
        non_speech_sha256: Option<String>,
    },
}
/// Parsed recipe versions normalize once; the common job consumers never read a
/// corpus digest from a non-speech recipe. Serialize retains original V1 shape.
#[derive(Debug, Clone)]
pub struct ExperimentRecipe {
    pub artifact_type: String,
    pub schema_version: u32,
    pub recipe_id: String,
    pub input: RecipeInput,
    pub track: Track,
    pub jobs: Vec<JobV1>,
    pub gui_fixture: Option<GuiFixture>,
    pub cache_policy: Option<CachePolicy>,
    pub scoring: Option<super::scoring::ScoringConfig>,
}
#[derive(Debug, Clone)]
pub enum RecipeInput {
    SpeechCorpus { sidecar_sha256: String },
    NonSpeech { index_sha256: String },
    AsrSpeech(ExperimentInputV3),
}
impl From<RunRecipeV1> for ExperimentRecipe {
    fn from(r: RunRecipeV1) -> Self {
        Self {
            artifact_type: r.artifact_type,
            schema_version: r.schema_version,
            recipe_id: r.recipe_id,
            input: RecipeInput::SpeechCorpus {
                sidecar_sha256: r.corpus_sidecar_sha256,
            },
            track: r.track,
            jobs: r.jobs,
            gui_fixture: r.gui_fixture,
            cache_policy: r.cache_policy,
            scoring: r.scoring,
        }
    }
}
impl From<RunRecipeV2> for ExperimentRecipe {
    fn from(r: RunRecipeV2) -> Self {
        let ExperimentInputV2::NonSpeech { index_sha256 } = r.input;
        Self {
            artifact_type: r.artifact_type,
            schema_version: r.schema_version,
            recipe_id: r.recipe_id,
            input: RecipeInput::NonSpeech { index_sha256 },
            track: r.track,
            jobs: r.jobs,
            gui_fixture: r.gui_fixture,
            cache_policy: r.cache_policy,
            scoring: r.scoring,
        }
    }
}
impl From<RunRecipeV3> for ExperimentRecipe {
    fn from(r: RunRecipeV3) -> Self {
        Self {
            artifact_type: r.artifact_type,
            schema_version: r.schema_version,
            recipe_id: r.recipe_id,
            input: RecipeInput::AsrSpeech(r.input),
            track: r.track,
            jobs: r.jobs,
            gui_fixture: r.gui_fixture,
            cache_policy: r.cache_policy,
            scoring: r.scoring,
        }
    }
}
impl Serialize for ExperimentRecipe {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("ExperimentRecipe", 9)?;
        s.serialize_field("artifact_type", &self.artifact_type)?;
        s.serialize_field("schema_version", &self.schema_version)?;
        s.serialize_field("recipe_id", &self.recipe_id)?;
        match &self.input {
            RecipeInput::SpeechCorpus { sidecar_sha256 } => {
                s.serialize_field("corpus_sidecar_sha256", sidecar_sha256)?
            }
            RecipeInput::NonSpeech { index_sha256 } => s.serialize_field(
                "input",
                &ExperimentInputV2::NonSpeech {
                    index_sha256: index_sha256.clone(),
                },
            )?,
            RecipeInput::AsrSpeech(input) => s.serialize_field("input", input)?,
        }
        s.serialize_field("track", &self.track)?;
        s.serialize_field("jobs", &self.jobs)?;
        s.serialize_field("gui_fixture", &self.gui_fixture)?;
        s.serialize_field("cache_policy", &self.cache_policy)?;
        s.serialize_field("scoring", &self.scoring)?;
        s.end()
    }
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CachePurpose {
    QualityReplay,
    FreshQuality,
    FreshReliability,
    FreshCloudAnchor,
    FreshPerformance,
}
strict!(CachePolicy { purpose: CachePurpose, max_age_ms: Option<u64> });
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Track {
    FileAdapter,
    GuiE2e,
    Learning,
}
strict!(JobV1 {
    product: String,
    artifact_identity: ArtifactIdentity,
    candidate_id: String,
    mode: Mode,
    recognition_language: RecognitionLanguage,
    decode_settings: DecodeSettings,
    vocabulary: Vocabulary,
    context: Context,
    cleanup: CleanupControls,
    local_rules: LocalRules,
    profile_state_id: String,
    repeats: u32,
    pacing: Pacing,
    run_budget: RunBudget
});
impl JobV1 {
    pub(super) fn id(&self, index: usize) -> String {
        format!("job-{index}:{}", self.candidate_id)
    }
}
strict!(ArtifactIdentity { version: String, source_commit: String, binary_sha256: Option<String> });
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Dictation,
    Verbatim,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecognitionLanguage {
    Auto,
    Zh,
    En,
}
strict!(DecodeSettings { temperature: Option<f64>, beam_size: Option<u32> });
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Vocabulary {
    None {},
    Explicit {
        content_sha256: String,
        relative_path: String,
    },
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Context {
    Disabled {},
    Fixture {
        source_sha256: String,
        relative_path: String,
        permission_snapshot: PermissionSnapshot,
    },
}
strict!(PermissionSnapshot {
    accessibility: bool,
    screen_recording: bool,
    provider_text_granted: bool
});
strict!(CleanupControls { intensity: CleanupIntensity, candidate_id: Option<String>, capture_provider_candidate: bool });
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CleanupIntensity {
    Off,
    Auto,
    Light,
    Standard,
    Heavy,
}
impl CleanupIntensity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::Light => "light",
            Self::Standard => "standard",
            Self::Heavy => "heavy",
        }
    }
}
strict!(LocalRules {
    dictionary_replacements: bool,
    fuzzy: bool,
    spoken_layout: bool,
    revision: String,
    fixture: Option<ControlFixtureLocator>
});
strict!(ControlFixtureLocator {
    relative_path: String,
    sha256: String
});
strict!(Pacing { gap_ms: u64 });
strict!(RunBudget {
    max_cases: u32,
    max_requests: u32,
    max_retries: u32,
    deadline_ms: u64
});
strict!(GuiFixture {
    target_id: String,
    initial_text_sha256: String,
    cursor_utf16: u64,
    selection_utf16: [u64; 2],
    expected_text_sha256: String,
    clipboard_fixture_id: String,
    declared_stress_action: String
});

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum StageState {
    NotRun {},
    NotApplicable {},
    Unknown { reason: String },
    Failed { reason: String },
    Cancelled { reason: String },
    Completed {},
}
pub use crate::observations::ObservedText;
strict!(AudioIdentity {
    audio_sha256: String,
    pcm_sha256: String,
    sample_count: u64,
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16
});
pub use super::source_identity::{CheckoutObservation, SourceProvenance};
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineIdentity {
    #[serde(default = "legacy_baseline_version")]
    pub schema_version: u32,
    pub source_commit: Option<String>,
    pub dirty: Option<bool>,
    pub source_sha256: Option<String>,
    pub lockfile_sha256: BTreeMap<String, String>,
    // Missing provenance is explicitly the old runtime-checkout interpretation.
    #[serde(default)]
    pub source_provenance: SourceProvenance,
    #[serde(default)]
    pub source_unavailable_path: Option<std::path::PathBuf>,
    #[serde(default)]
    pub source_git_unavailable_reason: Option<String>,
    #[serde(default)]
    pub live_checkout: Option<CheckoutObservation>,
    #[serde(default)]
    pub live_checkout_end: Option<CheckoutObservation>,
    pub executable_sha256: String,
    pub version: String,
    pub os: String,
    pub architecture: String,
    pub os_version: Option<String>,
    pub os_build: Option<String>,
    pub cpu: Option<String>,
    pub ram_bytes: Option<u64>,
}
fn legacy_baseline_version() -> u32 {
    1
}
impl BaselineIdentity {
    pub fn has_build_source_snapshot(&self) -> bool {
        // Only the build-facts owner validates and assigns compiled provenance.
        // Old deserialized records default to runtime-checkout provenance.
        self.schema_version == 2
            && matches!(
                self.source_provenance,
                SourceProvenance::BuildScriptSnapshotV3 {}
            )
    }
}
strict!(RunInputIdentityV1 { artifact_type: String, schema_version: u32, baseline: BaselineIdentity,
    corpus_sha256: String, recipe_sha256: String, non_speech_sha256: Option<String>, local_cleanup_manifest_sha256: Option<String> });
strict!(RunInputIdentityV2 {
    artifact_type: String,
    schema_version: u32,
    baseline: BaselineIdentity,
    input: ExperimentInputV2,
    recipe_sha256: String
});
strict!(RunInputIdentityV3 {
    artifact_type: String,
    schema_version: u32,
    baseline: BaselineIdentity,
    recipe_sha256: String,
    input: super::speech_input::InputIdentity
});
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum RunInputIdentity {
    Speech(RunInputIdentityV1),
    NonSpeech(RunInputIdentityV2),
    AsrSpeech(Box<RunInputIdentityV3>),
}
impl RunInputIdentity {
    pub fn baseline(&self) -> &BaselineIdentity {
        match self {
            Self::Speech(i) => &i.baseline,
            Self::NonSpeech(i) => &i.baseline,
            Self::AsrSpeech(i) => &i.baseline,
        }
    }
    pub fn baseline_mut(&mut self) -> &mut BaselineIdentity {
        match self {
            Self::Speech(i) => &mut i.baseline,
            Self::NonSpeech(i) => &mut i.baseline,
            Self::AsrSpeech(i) => &mut i.baseline,
        }
    }
    pub fn speech(&self) -> Option<&RunInputIdentityV1> {
        if let Self::Speech(i) = self {
            Some(i)
        } else {
            None
        }
    }
    pub fn set_cleanup_manifest(&mut self, sha: Option<String>) {
        if let Self::Speech(i) = self {
            i.local_cleanup_manifest_sha256 = sha;
        }
    }
}
impl From<RunInputIdentityV1> for RunInputIdentity {
    fn from(i: RunInputIdentityV1) -> Self {
        Self::Speech(i)
    }
}
macro_rules! run_record {
    ($($field:ident:$ty:ty),* $(,)?)=>{
        #[derive(Debug,Clone,Serialize,Deserialize)]
        #[serde(try_from="RunRecordWire")]
        pub struct RunRecord {$(pub $field:$ty),*}
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RunRecordWire { $($field:$ty),* }
        impl TryFrom<RunRecordWire> for RunRecord {
            type Error=&'static str;
            fn try_from(w:RunRecordWire)->Result<Self,Self::Error>{
                if w.artifact_type!="run_record"||!w.input_aligned(){return Err("record version/input identity/path kind mismatch")}
                Ok(Self{$($field:w.$field),*})
            }
        }
    }
}
run_record! { artifact_type: String, schema_version: u32, run_id: String,
identity: RunInputIdentity, terminal: StageState, provider_attempts: u64, setup: Option<SetupRecord>, replay: Option<ReplaySummary>, scoring: Option<super::scoring::ScoringSummary>, local_cleanup: Option<super::local_cleanup::LocalResearchIdentity>, cases: Vec<CaseRecordV1>  }
// One wire field list and one version/input binding owner feed every reader.
impl RunRecordWire {
    fn input_aligned(&self) -> bool {
        match (&self.identity, self.schema_version) {
            (RunInputIdentity::Speech(identity), 1) => {
                identity.artifact_type == "run_input_identity"
                    && identity.schema_version == 1
                    && self.cases.iter().all(|case| {
                        matches!(&case.input_paths, CaseInputPaths::Speech(_))
                            && case.score_input.as_ref().is_none_or(|score| {
                                score.asr_only.is_none()
                                    && score.asr_reference.is_some()
                                    && score.final_references.is_some()
                            })
                    })
            }
            (RunInputIdentity::NonSpeech(identity), 2) => {
                let ExperimentInputV2::NonSpeech { index_sha256 } = &identity.input;
                identity.artifact_type == "run_input_identity"
                    && identity.schema_version == 2
                    && self.cases.iter().all(|case| {
                        matches!(&case.input_paths,CaseInputPaths::NonSpeech(paths)
                        if paths.document_kind=="non_speech_index"
                        && paths.index_sha256==*index_sha256
                        && paths.case.case_id==case.case_id
                        && paths.case.audio_sha256==case.audio.audio_sha256
                        && paths.declared_audio==paths.case.adjacent_wav)
                            && case.score_input.as_ref().is_some_and(|score| {
                                score.negative
                                    && score.asr_only.is_none()
                                    && score.asr_reference.is_none()
                                    && score.final_references.is_none()
                                    && score.corpus.is_none()
                                    && score.annotations.is_empty()
                                    && score.annotation_revision.is_none()
                            })
                    })
            }
            (RunInputIdentity::AsrSpeech(identity), 3) => {
                identity.artifact_type == "run_input_identity"
                    && identity.schema_version == 3
                    && identity.input.kind == "asr_speech"
                    && self.cases.iter().all(|case| match &case.input_paths {
                        CaseInputPaths::AsrSpeech(paths) => {
                            paths.document_kind == "asr_speech_index"
                                && paths.index_sha256 == identity.input.speech_index_sha256
                                && paths.source_packet_sha256 == identity.input.source_packet_sha256
                                && paths.purpose == identity.input.purpose
                                && paths.source.case_id == paths.case.case_id
                                && paths.declared_audio == paths.case.adjacent_wav
                                && paths.case.case_id == case.case_id
                                && super::speech_input::validate_reference(
                                    std::path::Path::new("run_record"),
                                    identity.input.purpose,
                                    &paths.case.reference,
                                    &paths.source,
                                )
                                .is_ok()
                                && super::resume::hash(&paths.case.audio)
                                    == super::resume::hash(&case.audio)
                                && case.score_input.as_ref().is_some_and(|score| {
                                    score.asr_only.as_ref().is_some_and(|scope| {
                                        *scope
                                            == super::speech_input::ScoreScope::for_source(
                                                identity.input.purpose,
                                                &paths.source,
                                            )
                                    }) && score.asr_reference.as_deref()
                                        == paths.case.reference.text()
                                        && !score.negative
                                        && score.final_references.is_none()
                                        && score.corpus.is_none()
                                        && score.annotations.is_empty()
                                })
                        }
                        CaseInputPaths::NonSpeech(paths) => {
                            identity.input.non_speech_sha256.as_ref() == Some(&paths.index_sha256)
                                && paths.case.case_id == case.case_id
                                && paths.case.audio_sha256 == case.audio.audio_sha256
                                && case.score_input.as_ref().is_some_and(|s| {
                                    s.negative
                                        && s.asr_only.is_none()
                                        && s.asr_reference.is_none()
                                        && s.final_references.is_none()
                                })
                        }
                        _ => false,
                    })
            }
            _ => false,
        }
    }
}
strict!(SetupRecord {
    materialization: StageState, credentials: StageState, worker_join: StageState,
    cancellation_requested: bool, workspace: Option<SetupWorkspace>, copies: Vec<SetupCopy>
});
strict!(SetupWorkspace {
    selected_path: String, held_identity: Option<super::artifacts::ArtifactFileIdentity>, creation: StageState,
    path_applicable: Option<bool>, retained: Option<bool>
});
strict!(SetupCopy {
    model_id: String,
    relative_path: String,
    expected_bytes: u64,
    written_bytes: u64,
    state: StageState
});
impl SetupRecord {
    pub(crate) fn new() -> Self {
        Self {
            materialization: StageState::NotRun {},
            credentials: StageState::NotRun {},
            worker_join: StageState::NotRun {},
            cancellation_requested: false,
            workspace: None,
            copies: vec![],
        }
    }
}
strict!(CaseRecordV1 { score_input: Option<super::scoring::ScoreInput>, scores: Option<super::scoring::CaseScores>, case_id: String, job_id: String, repeat: u32, requested: JobV1,
    effective: EffectiveControls, control_input_identities: BTreeMap<String,ControlInputIdentity>,
    local_cleanup: Option<super::local_cleanup::LocalResearchIdentity>, local_model: Option<super::local_asr::LocalModelIdentity>, local_runtime: Option<crate::observations::LocalAsrObservation>,
    asr_observation: Option<crate::observations::AsrObservation>, cleanup_observation: Option<crate::observations::CleanupObservation>,
    asr_request: Option<crate::observations::ResolvedRequest>, cleanup_request: Option<crate::observations::ResolvedRequest>, audio: AudioIdentity, input_paths: CaseInputPaths, state: StageState,
    asr_origin: String, asr_terminal: String, provider_text: ObservedText,
    filtered_text: ObservedText, prepared_text: ObservedText, adapter_candidate: ObservedText,
    final_text: ObservedText, finalizer_identity_sha256: Option<String>, cleanup_stage: String, cleanup_adapter: StageState, final_guard: StageState, transformations: Vec<Transformation>,
    attempts: u32, attempt_facts: Vec<AttemptFact>, fallback: Option<String>, winner: Option<String>, timings_ms: BTreeMap<String,f64>,
    diagnostics: Vec<String>, independent_oracle: StageState, coverage: String,
    cache_source: String, asr_replay: Option<StageReplay>, cleanup_replay: Option<StageReplay>, cancellation_requested: bool, provider_request_termination_observed: StageState });
impl CaseRecordV1 {
    /// Selected historical candidate provenance is distinct from current provider facts.
    pub(super) fn selected_asr_observation(&self) -> Option<&crate::observations::AsrObservation> {
        self.asr_replay
            .as_ref()
            .and_then(|r| r.source_asr_observation.as_ref())
            .or(self.asr_observation.as_ref())
    }
}
strict!(ReplaySummary { policy: CachePolicy, checkpoint_input_sha256: Option<String>, checkpoint_output: Option<String>, checkpoint_acquisition: Option<CheckpointAcquisition>, checkpoint_artifact_outcome: Option<super::artifacts::ArtifactOutcome>,
    resumed_asr: u64, resumed_cleanup: u64, fresh_asr_cases: u64, fresh_cleanup_cases: u64, replay_guard_rejected: u64, failed_cases: u64, cancelled_cases: u64, unknown_cases: u64, not_run_cases: u64, current_quality_eligible: bool, current_performance_eligible: bool, current_reliability_eligible: bool });
strict!(CheckpointAcquisition {
    selected_path: String,
    state: StageState,
    writer_acquired: bool,
    path_applicability: StageState
});
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StageReplay {
    pub source_run: String,
    pub source_created_unix_ms: u64,
    pub request_sha256: String,
    pub input_sha256: String,
    pub scope: String,
    pub source_checkpoint_sha256: String,
    pub source_request: crate::observations::ResolvedRequest,
    pub source_asr_observation: Option<crate::observations::AsrObservation>,
    pub current_guard: StageState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_local_runtime: Option<crate::observations::LocalAsrObservation>,
}
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum CacheStage {
    Asr,
    Cleanup,
}
strict!(CheckpointV1 { artifact_type: String, schema_version: u32, created_unix_ms: u64, identity: RunInputIdentityV1,
    successful_stage_candidates: Vec<StageCandidate>, complete_jobs: Vec<String>, terminal: StageState, provider_attempts: u64 });
strict!(StageCandidate { case_id: String, job_id: String, repeat: u32, stage: CacheStage, input_sha256: String,
    request_sha256: String, request: crate::observations::ResolvedRequest, origin: String, completion: String, value: String, value_sha256: String,
    source_run: String, source_created_unix_ms: u64, state: StageState, scope: String,
    asr_observation: Option<crate::observations::AsrObservation> });
strict!(ControlInputIdentity {
    relative_path: String,
    canonical_path: String,
    sha256: String,
    bytes: u64
});
strict!(EffectiveControls {
    sent_model: Option<String>,
    endpoint: Option<String>,
    recognition_language: Option<RecognitionLanguage>,
    vocabulary: Option<String>,
    context: Option<String>,
    local_rules_revision: String,
    server_model: ObservedText
});
strict!(Transformation {
    owner: String,
    revision: String,
    reason: String,
    source_stage: String
});

strict!(NonSpeechInputPaths {
    document_kind: String,
    selected_document: String,
    canonical_document: String,
    index_sha256: String,
    provenance: Provenance,
    case: NonSpeechCaseV1,
    declared_audio: String,
    canonical_audio: String
});
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum CaseInputPaths {
    Speech(InputPaths),
    NonSpeech(Box<NonSpeechInputPaths>),
    AsrSpeech(Box<super::speech_input::InputPaths>),
}
strict!(InputPaths {
    manifest: String,
    declared_audio: String,
    canonical_audio: String
});

#[derive(Debug, Clone, Copy, serde::Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStage {
    Asr,
    Cleanup,
}
strict!(AttemptFact {
    stage: AttemptStage,
    ordinal: u32,
    local_adapter: StageState,
    transport_submission: StageState,
    server_received: StageState,
    cache_source: String
});
