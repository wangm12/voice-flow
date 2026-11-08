use super::artifacts::{ArtifactFailure, ArtifactStore};
use super::authorization::{AuthorizationScope, CredentialProvider, KeyMaterial};
use super::contracts::AttemptStage;
use super::execution::{DispatchObserver, ExecutionError};
// Shared production-adapter evaluator extracted from cleanup_corpus::live_paired_eval.
use crate::asr::{AsrError, AsrOptions, AsrProvider, GroqAsrProvider};
use crate::context::{ContextFamily, ContextPolicy, ContextSnapshot, FocusKind};
use crate::engine::EngineProvider;
use crate::{llm, providers};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
pub(crate) const REPORT_SCHEMA_VERSION: u32 = 2;

pub(crate) const QWEN_CHAT_BASE: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";

pub(crate) const QWEN_ASR_BASE: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";

pub(crate) const REQUEST_BUILDER_REVISION: &str = "live-cleanup-request-v2";

pub(crate) const RATE_LIMIT_RETRIES_PER_CASE: usize = 1;

pub(crate) const DEFAULT_MIN_REQUEST_GAP_MS: u64 = 250;

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct EvalCase {
    pub(crate) id: String,
    pub(crate) group: String,
    pub(crate) input: String,
    pub(crate) expected: String,
    #[serde(default)]
    pub(crate) reference_variants: Vec<String>,
    pub(crate) cleanup: String,
    pub(crate) family: String,
    pub(crate) focus_kind: String,
    pub(crate) mode: String,
    #[serde(default)]
    pub(crate) protected: Vec<String>,
}

#[derive(Serialize)]
pub(crate) struct CleanupModelReport {
    pub(crate) candidate: String,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) sanitized_host: String,
    pub(crate) key_source: String,
    pub(crate) verification_status: String,
    pub(crate) stop_category: Option<String>,
    pub(crate) route_eligible_cases: usize,
    pub(crate) candidate_coverage_cases: usize,
    pub(crate) candidate_coverage_rate: Option<f64>,
    pub(crate) provider_invoked_cases: usize,
    pub(crate) provider_http_attempts: usize,
    pub(crate) rate_limit_retries: usize,
    pub(crate) rate_limit_retry_wait_ms: f64,
    pub(crate) minimum_request_gap_ms: u64,
    pub(crate) resumed_candidate_samples: usize,
    pub(crate) provider_skipped_after_error_cases: usize,
    pub(crate) adapter_candidate_successes: usize,
    pub(crate) adapter_guard_rejections: usize,
    pub(crate) app_final_guard_rejections: usize,
    pub(crate) provider_error_categories: BTreeMap<String, usize>,
    pub(crate) bypass_counts: BTreeMap<String, usize>,
    pub(crate) declared_protected_cases: usize,
    pub(crate) declared_protected_spans: usize,
    pub(crate) declared_protected_candidate_samples: usize,
    pub(crate) declared_protected_candidate_preserved: usize,
    pub(crate) declared_protected_candidate_span_samples: usize,
    pub(crate) declared_protected_candidate_spans_preserved: usize,
    pub(crate) declared_protected_final_samples: usize,
    pub(crate) declared_protected_final_preserved: usize,
    pub(crate) declared_protected_final_span_samples: usize,
    pub(crate) declared_protected_final_spans_preserved: usize,
    pub(crate) mixed_script_proxy_cases: usize,
    pub(crate) mixed_script_candidate_samples: usize,
    pub(crate) mixed_script_candidate_preserved: usize,
    pub(crate) mixed_script_final_samples: usize,
    pub(crate) mixed_script_final_preserved: usize,
    pub(crate) identity_candidate_cases: usize,
    pub(crate) identity_candidate_rewrites: usize,
    pub(crate) identity_final_rewrites: usize,
    pub(crate) adapter_candidate_quality: QualitySummary,
    pub(crate) finalized_provider_success_quality: QualitySummary,
    pub(crate) all_path_final_quality: QualitySummary,
    pub(crate) timings: BTreeMap<String, TimingSummary>,
    pub(crate) cases: Vec<CleanupCaseReport>,
}

#[derive(Serialize)]
pub(crate) struct CleanupCaseReport {
    pub(crate) id: String,
    pub(crate) group: String,
    pub(crate) mode: String,
    pub(crate) cleanup: String,
    pub(crate) family: String,
    pub(crate) focus_kind: String,
    pub(crate) input: String,
    pub(crate) request_fingerprint: String,
    pub(crate) route: String,
    pub(crate) provider_invoked: bool,
    pub(crate) resumed_candidate: bool,
    pub(crate) provider_call_state: String,
    pub(crate) provider_attempts: usize,
    pub(crate) adapter_candidate: Option<String>,
    pub(crate) final_text: String,
    pub(crate) expected: String,
    pub(crate) adapter_candidate_reference_match: Option<bool>,
    pub(crate) final_reference_match: bool,
    pub(crate) adapter_candidate_declared_protected_preserved: Option<bool>,
    pub(crate) final_declared_protected_preserved: bool,
    pub(crate) final_span_guard_preserved: bool,
    pub(crate) mixed_script_proxy_preserved: Option<bool>,
    pub(crate) identity_candidate_rewrite: Option<bool>,
    pub(crate) identity_final_rewrite: Option<bool>,
    pub(crate) adapter_guard_rejected: bool,
    pub(crate) app_final_guard_rejected: bool,
    pub(crate) error_category: Option<String>,
    pub(crate) status: Option<u16>,
    pub(crate) preparation_ms: f64,
    pub(crate) provider_ms: Option<f64>,
    pub(crate) provider_wait_ms: f64,
    pub(crate) retry_wait_ms: f64,
    pub(crate) finalize_ms: f64,
    pub(crate) total_ms: f64,
}

#[derive(Debug, Default)]
pub(crate) struct QualityAccumulator {
    pub(crate) sample_count: usize,
    pub(crate) reference_matches: usize,
    pub(crate) character_edits: usize,
    pub(crate) reference_characters: usize,
    pub(crate) word_edits: usize,
    pub(crate) reference_words: usize,
}

#[derive(Serialize, Default)]
pub(crate) struct QualitySummary {
    pub(crate) sample_count: usize,
    pub(crate) any_variant_reference_matches: usize,
    pub(crate) any_variant_reference_match_rate: Option<f64>,
    pub(crate) character_edits_min_variant: usize,
    pub(crate) reference_codepoints_min_variant: usize,
    pub(crate) character_error_rate_min_variant: Option<f64>,
    pub(crate) word_edits_min_variant: usize,
    pub(crate) reference_words_min_variant: usize,
    pub(crate) word_error_rate_min_variant: Option<f64>,
}

#[derive(Serialize)]
pub(crate) struct TimingSummary {
    pub(crate) sample_count: usize,
    pub(crate) p50_ms: Option<f64>,
    pub(crate) p95_ms: Option<f64>,
}

#[derive(Debug, Default)]
pub(crate) struct ResumeState {
    // Legacy ignored-test reader applicability; not the pending RunRecord resume contract.
    #[cfg(test)]
    pub(crate) compatible: bool,
    #[cfg(test)]
    pub(crate) ignored_reason: Option<String>,
    pub(crate) cases_by_candidate: BTreeMap<String, BTreeMap<String, ResumeCase>>,
    artifacts: std::sync::Mutex<ArtifactStore>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResumeCase {
    pub(crate) request_fingerprint: String,
    pub(crate) adapter_candidate: String,
}

#[derive(Serialize)]
pub(crate) struct ResumeCheckpoint<'a> {
    pub(crate) schema_version: u32,
    pub(crate) corpus_fingerprint: &'a str,
    pub(crate) cleanup_code_fingerprint: &'a str,
    pub(crate) checkpoint_candidates: Vec<ResumeCheckpointCandidate<'a>>,
}

#[derive(Serialize)]
pub(crate) struct ResumeCheckpointCandidate<'a> {
    pub(crate) candidate: &'a str,
    pub(crate) cases: Vec<ResumeCheckpointCase<'a>>,
}

#[derive(Serialize)]
pub(crate) struct ResumeCheckpointCase<'a> {
    pub(crate) id: &'a str,
    pub(crate) request_fingerprint: &'a str,
    pub(crate) adapter_candidate: &'a str,
}

#[derive(Serialize)]
pub(crate) struct CleanupRequestIdentity<'a> {
    pub(crate) request_builder_revision: &'static str,
    pub(crate) corpus_fingerprint: &'a str,
    pub(crate) cleanup_code_fingerprint: &'a str,
    pub(crate) candidate: &'a str,
    pub(crate) provider: &'a str,
    pub(crate) model: &'a str,
    pub(crate) endpoint: &'a str,
    pub(crate) case_id: &'a str,
    pub(crate) group: &'a str,
    pub(crate) mode: &'a str,
    pub(crate) cleanup: &'a str,
    pub(crate) family: &'a str,
    pub(crate) focus_kind: &'a str,
    pub(crate) source: &'a str,
    pub(crate) request_content: &'a str,
    pub(crate) intent: &'a llm::CleanupIntent,
    pub(crate) pairs_hint: Option<&'a str>,
    pub(crate) profile: &'a crate::context::ContextProfile,
    pub(crate) policy: &'a ContextPolicy,
    pub(crate) effort: Option<&'a str>,
    pub(crate) fixed_options: &'static str,
}

pub(crate) struct CleanupRequestFingerprintContext<'a> {
    pub(crate) endpoint: &'a str,
    pub(crate) model: &'a str,
    pub(crate) policy: &'a ContextPolicy,
    pub(crate) effort: Option<crate::llm::CleanupEffort>,
    pub(crate) corpus_fingerprint: &'a str,
    pub(crate) cleanup_code_fingerprint: &'a str,
}

#[derive(Clone)]
pub(crate) enum CleanupTransport {
    Cloud(crate::engine::EngineProvider),
    VerifiedLocalOpenAi {
        lease: std::sync::Arc<tokio::sync::Mutex<super::local_cleanup::OwnedServerLease>>,
        cancel: tokio_util::sync::CancellationToken,
    },
}
#[derive(Clone)]
pub(crate) struct CleanupCandidate {
    pub(crate) id: &'static str,
    pub(crate) transport: CleanupTransport,
    pub(crate) model: String,
    pub(crate) base_url: String,
    pub(crate) endpoint: String,
    pub(crate) key: Option<String>,
    pub(crate) key_source: String,
    pub(crate) authorization_scope: Option<AuthorizationScope>,
}

enum PreparedCleanupSource {
    Transcript(Box<crate::PreparedCleanupTranscript>),
    Snippet {
        source: String,
        intent: llm::CleanupIntent,
    },
}

pub(crate) struct PreparedCleanupCase {
    pub(crate) family: ContextFamily,
    pub(crate) input_kind: FocusKind,
    source: PreparedCleanupSource,
    pub(crate) snapshot: ContextSnapshot,
    pub(crate) route: crate::lexicon::CleanupRoute,
    pub(crate) bypass: Option<&'static str>,
    pub(crate) snippet: bool,
}

impl PreparedCleanupCase {
    pub(crate) fn source(&self) -> &str {
        match &self.source {
            PreparedCleanupSource::Transcript(prepared) => &prepared.text,
            PreparedCleanupSource::Snippet { source, .. } => source,
        }
    }
    fn intent(&self) -> &llm::CleanupIntent {
        match &self.source {
            PreparedCleanupSource::Transcript(prepared) => &prepared.intent,
            PreparedCleanupSource::Snippet { intent, .. } => intent,
        }
    }
    fn pairs_hint(&self) -> Option<&str> {
        match &self.source {
            PreparedCleanupSource::Transcript(prepared) => prepared.pairs_hint.as_deref(),
            PreparedCleanupSource::Snippet { .. } => None,
        }
    }
    pub(crate) fn finalization_context(&self) -> crate::FinalizationContext<'_> {
        let prepared = match &self.source {
            PreparedCleanupSource::Transcript(prepared) => Some(prepared),
            PreparedCleanupSource::Snippet { .. } => None,
        };
        crate::FinalizationContext {
            scene: prepared.map(|prepared| prepared.scene).unwrap_or_else(|| {
                crate::ResolvedCleanupScene::observed(self.family, self.input_kind)
            }),
            operation: self.intent().operation,
            revision_source: prepared.map(|p| p.revision_source.as_str()),
            prepared_transcript: prepared.map(|p| p.text.as_str()),
            revision_authorizations: prepared
                .map(|p| p.revision_authorizations.as_slice())
                .unwrap_or(&[]),
            promoted_pair_protections: prepared
                .map(|p| p.promoted_pair_protections.as_slice())
                .unwrap_or(&[]),
            list_revision_proof: prepared.and_then(|p| p.list_revision_proof.as_ref()),
            structure_safe_source: None,
            output_projection_list_revision_proof: None,
            layout_source: None,
            exact_fallback: false,
        }
    }
}

pub(crate) struct CleanupCandidateRun<'a> {
    pub(crate) cases: &'a [EvalCase],
    pub(crate) blocked_reason: Option<String>,
    pub(crate) resume_state: &'a mut ResumeState,
    pub(crate) checkpoint_path: Option<&'a Path>,
    pub(crate) corpus_fingerprint: &'a str,
    pub(crate) cleanup_code_fingerprint: &'a str,
    pub(crate) pacer: &'a mut RequestPacer,
    pub(crate) observer: Option<&'a dyn DispatchObserver>,
    pub(crate) preparation: Option<&'a super::controls::PreparationFixture>,
    pub(crate) replay: Option<&'a super::resume::ValidatedCandidate>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AudioManifest {
    pub(crate) cases: Vec<AudioFixture>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AudioFixture {
    pub(crate) id: String,
    pub(crate) audio: String,
    pub(crate) reference: String,
    #[serde(skip)]
    pub(crate) validated_audio: Option<Vec<u8>>,
}

/// Provider execution input; absence of a reference never means non-speech.
pub(crate) struct AsrAudioInput<'a> {
    pub id: &'a str,
    pub audio: &'a str,
    pub reference: Option<&'a str>,
    pub validated_audio: Option<&'a [u8]>,
}
#[derive(Serialize)]
pub(crate) struct AsrModelReport {
    pub(crate) candidate: String,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) sanitized_host: String,
    pub(crate) key_source: String,
    pub(crate) verification_status: String,
    pub(crate) stop_category: Option<String>,
    pub(crate) provider_invoked_cases: usize,
    pub(crate) successful_transcriptions: usize,
    pub(crate) no_speech_results: usize,
    pub(crate) error_categories: BTreeMap<String, usize>,
    pub(crate) raw_asr_quality: QualitySummary,
    pub(crate) sanitized_text_quality: QualitySummary,
    pub(crate) timings: TimingSummary,
    pub(crate) cases: Vec<AsrCaseReport>,
}

#[derive(Serialize)]
pub(crate) struct AsrCaseReport {
    pub(crate) id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reference: Option<String>,
    pub(crate) provider_invoked: bool,
    pub(crate) transcription_state: String,
    pub(crate) raw_asr_text: Option<String>,
    pub(crate) sanitized_text: Option<String>,
    pub(crate) detected_language: Option<String>,
    pub(crate) confidence: Option<f32>,
    pub(crate) segment_count: Option<usize>,
    pub(crate) word_count: Option<usize>,
    pub(crate) error_category: Option<String>,
    pub(crate) status: Option<u16>,
    pub(crate) latency_ms: Option<f64>,
}

#[derive(Clone)]
pub(crate) struct AsrCandidate {
    pub(crate) id: &'static str,
    pub(crate) provider: &'static str,
    pub(crate) model: String,
    pub(crate) execution: AsrExecution,
    pub(crate) host: String,
    pub(crate) key_source: String,
    pub(crate) language: Option<String>,
    pub(crate) prompt: Option<String>,
    pub(crate) keywords: Vec<String>,
    pub(crate) authorization_scope: Option<AuthorizationScope>,
}

#[derive(Clone)]
pub(crate) enum AsrExecution {
    Cloud {
        endpoint: String,
        key: Option<String>,
    },
    Local {
        models_root: PathBuf,
    },
}
impl AsrCandidate {
    pub(crate) fn credentials_available(&self) -> bool {
        match &self.execution {
            AsrExecution::Cloud { key, .. } => key.is_some(),
            AsrExecution::Local { .. } => true,
        }
    }
    pub(crate) fn local(id: &'static str, model: &str, root: PathBuf, options: AsrOptions) -> Self {
        Self {
            id,
            provider: "ondevice",
            model: model.into(),
            execution: AsrExecution::Local { models_root: root },
            host: String::new(),
            key_source: "not_applicable_local".into(),
            language: options.language,
            prompt: None,
            keywords: vec![],
            authorization_scope: None,
        }
    }
}

impl CleanupCandidate {
    pub(crate) fn provider_name(&self) -> &str {
        match self.transport {
            CleanupTransport::Cloud(provider) => provider.as_str(),
            CleanupTransport::VerifiedLocalOpenAi { .. } => "verified_local_openai",
        }
    }
    pub(crate) fn credentials_available(&self) -> bool {
        !matches!(self.transport, CleanupTransport::Cloud(_)) || self.key.is_some()
    }
    fn request_model(&self, settings: &crate::store::Settings) -> String {
        match self.transport {
            CleanupTransport::Cloud(_) => settings.cleanup_request_model(),
            CleanupTransport::VerifiedLocalOpenAi { .. } => self.model.clone(),
        }
    }
    pub(crate) fn local(
        lease: std::sync::Arc<tokio::sync::Mutex<super::local_cleanup::OwnedServerLease>>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Self {
        Self {
            id: super::local_cleanup::ID,
            transport: CleanupTransport::VerifiedLocalOpenAi { lease, cancel },
            model: super::local_cleanup::ID.into(),
            base_url: "http://127.0.0.1:18084".into(),
            endpoint: "http://127.0.0.1:18084/v1/chat/completions".into(),
            key: None,
            key_source: "not_applicable_verified_owned_local".into(),
            authorization_scope: None,
        }
    }

    pub(crate) fn from_material(
        id: &'static str,
        provider: EngineProvider,
        model: String,
        base_url: String,
        key: &KeyMaterial,
    ) -> Self {
        let endpoint = providers::resolve_llm_endpoint(provider, &base_url);
        Self {
            id,
            transport: CleanupTransport::Cloud(provider),
            model,
            base_url,
            key: key.key.clone(),
            key_source: key.source.clone(),
            authorization_scope: key.scope(&endpoint),
            endpoint,
        }
    }
}
pub(crate) fn cleanup_candidates() -> Vec<CleanupCandidate> {
    let optional = |provider: CredentialProvider| {
        provider
            .resolve()
            .unwrap_or_else(|_| KeyMaterial::unavailable("explicit_key_file_unavailable"))
    };
    let groq = optional(CredentialProvider::Groq);
    let openai = optional(CredentialProvider::OpenAi);
    let qwen = optional(CredentialProvider::Qwen);
    vec![
        CleanupCandidate::from_material(
            "groq_gpt_oss_120b",
            EngineProvider::Groq,
            model_override("VOICEFLOW_EVAL_GROQ_PRIMARY_MODEL", "openai/gpt-oss-120b"),
            String::new(),
            &groq,
        ),
        CleanupCandidate::from_material(
            "groq_gpt_oss_20b",
            EngineProvider::Groq,
            model_override("VOICEFLOW_EVAL_GROQ_SECONDARY_MODEL", "openai/gpt-oss-20b"),
            String::new(),
            &groq,
        ),
        CleanupCandidate::from_material(
            "openai_gpt_4o_mini",
            EngineProvider::OpenAi,
            model_override("VOICEFLOW_EVAL_OPENAI_MODEL", "gpt-4o-mini"),
            EngineProvider::OpenAi.default_base_url().into(),
            &openai,
        ),
        CleanupCandidate::from_material(
            "qwen_plus_dashscope",
            EngineProvider::Custom,
            model_override("VOICEFLOW_EVAL_QWEN_MODEL", "qwen-plus"),
            QWEN_CHAT_BASE.into(),
            &qwen,
        ),
    ]
}

pub(crate) fn parse_id_filter(
    raw_filter: &str,
    supported_ids: &[&str],
    label: &str,
) -> Result<Vec<String>, String> {
    if raw_filter.trim().is_empty() {
        return Err(format!("{label} ID filter is empty"));
    }
    let requested = raw_filter.split(',').map(str::trim).collect::<Vec<_>>();
    if requested.iter().any(|id| id.is_empty()) {
        return Err(format!("{label} ID filter contains an empty entry"));
    }
    let mut seen = BTreeMap::new();
    for id in &requested {
        if seen.insert(*id, ()).is_some() {
            return Err(format!("{label} ID filter repeats {id}"));
        }
    }
    let unknown = requested
        .iter()
        .copied()
        .filter(|id| !supported_ids.contains(id))
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(format!("unknown {label} IDs: {}", unknown.join(",")));
    }
    Ok(requested.into_iter().map(str::to_owned).collect())
}

pub(crate) fn configure_cleanup_candidates(
    mut candidates: Vec<CleanupCandidate>,
    filter: Option<&str>,
    preference: Option<&str>,
) -> Vec<CleanupCandidate> {
    if let Some(filter) = filter {
        let requested: Vec<&str> = filter
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect();
        candidates.retain(|candidate| requested.contains(&candidate.id));
        assert!(
            !candidates.is_empty(),
            "live cleanup candidate filter did not match a supported candidate id"
        );
    }
    if let Some(preference) = preference {
        let preferred_ids: Vec<&str> = preference
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect();
        candidates.sort_by_key(|candidate| {
            preferred_ids
                .iter()
                .position(|id| *id == candidate.id)
                .unwrap_or(usize::MAX)
        });
    }
    candidates
}

fn cleanup_settings(candidate: &CleanupCandidate) -> crate::store::Settings {
    // Settings here own cleanup style/policy only. A local transport never
    // becomes Custom, receives a fake key or changes saved product settings.
    let mut settings = crate::store::Settings {
        cleanup_model: candidate.model.clone(),
        cleanup_base_url: candidate.base_url.clone(),
        cleanup_intensity: "auto".into(),
        cleanup_enabled: true,
        ..crate::store::Settings::default()
    };
    if let CleanupTransport::Cloud(provider) = candidate.transport {
        settings.cleanup_provider = provider;
        settings.provider_api_keys.insert(
            provider.as_str().into(),
            candidate.key.clone().unwrap_or_default(),
        );
        if provider == EngineProvider::Custom {
            settings.custom_base_url = candidate.base_url.clone();
            settings.custom_llm = true;
        }
    }
    settings
}
pub(crate) fn cleanup_candidate_request_facts(
    candidate: &CleanupCandidate,
    prepared: &PreparedCleanupCase,
) -> Option<crate::observations::ResolvedRequest> {
    if prepared.bypass.is_some()
        || matches!(
            candidate.transport,
            CleanupTransport::VerifiedLocalOpenAi { .. }
        )
    {
        return None;
    }
    let settings = cleanup_settings(candidate);
    let model = candidate.request_model(&settings);
    let policy = crate::cleanup_policy_for(&settings, &prepared.snapshot);
    let crate::lexicon::CleanupRoute::Provider(effort) = prepared.route else {
        return None;
    };
    llm::cleanup_request_facts(
        &candidate.endpoint,
        &model,
        prepared.intent(),
        &llm::CleanupPromptContext {
            dictionary: &[],
            context: None,
            policy: Some(&policy),
            profile: Some(&prepared.snapshot.profile),
            pairs_hint: prepared.pairs_hint(),
            effort,
            visible_context: None,
            source_transcript: None,
        },
    )
}

pub(crate) async fn run_cleanup_candidate(
    candidate: &CleanupCandidate,
    run: CleanupCandidateRun<'_>,
) -> Result<CleanupModelReport, ExecutionError> {
    let CleanupCandidateRun {
        cases,
        blocked_reason,
        resume_state,
        checkpoint_path,
        corpus_fingerprint,
        cleanup_code_fingerprint,
        pacer,
        observer,
        preparation,
        replay,
    } = run;
    let host = cleanup_host(candidate);
    let settings = cleanup_settings(candidate);

    let mut results = Vec::with_capacity(cases.len());
    let mut candidate_quality = QualityAccumulator::default();
    let mut finalized_provider_quality = QualityAccumulator::default();
    let mut all_path_quality = QualityAccumulator::default();
    let mut provider_latencies = Vec::new();
    let mut provider_wait_latencies = Vec::new();
    let mut pacing_wait_latencies = Vec::new();
    let mut retry_wait_latencies = Vec::new();
    let mut preparation_latencies = Vec::new();
    let mut finalize_latencies = Vec::new();
    let mut total_latencies = Vec::new();
    let mut provider_error_categories = BTreeMap::new();
    let mut bypass_counts = BTreeMap::new();
    let mut route_eligible_cases = 0usize;
    let mut candidate_coverage_cases = 0usize;
    let mut provider_invoked_cases = 0usize;
    let mut provider_http_attempts = 0usize;
    let mut rate_limit_retries = 0usize;
    let mut rate_limit_retry_wait_ms = 0.0;
    let mut resumed_candidate_samples = 0usize;
    let mut provider_skipped_after_error_cases = 0usize;
    let mut candidate_successes = 0usize;
    let mut adapter_guard_rejections = 0usize;
    let mut app_final_guard_rejections = 0usize;
    let declared_protected_cases = cases
        .iter()
        .filter(|case| !case.protected.is_empty())
        .count();
    let declared_protected_spans = cases.iter().map(|case| case.protected.len()).sum();
    let mut declared_protected_candidate_samples = 0usize;
    let mut declared_protected_candidate_preserved = 0usize;
    let mut declared_protected_candidate_span_samples = 0usize;
    let mut declared_protected_candidate_spans_preserved = 0usize;
    let mut declared_protected_final_samples = 0usize;
    let mut declared_protected_final_preserved = 0usize;
    let mut declared_protected_final_span_samples = 0usize;
    let mut declared_protected_final_spans_preserved = 0usize;
    let mixed_script_proxy_cases = cases
        .iter()
        .filter(|case| has_cjk(&case.input) && has_latin(&case.input))
        .count();
    let mut mixed_script_candidate_samples = 0usize;
    let mut mixed_script_candidate_preserved = 0usize;
    let mut mixed_script_final_samples = 0usize;
    let mut mixed_script_final_preserved = 0usize;
    let mut identity_candidate_cases = 0usize;
    let mut identity_candidate_rewrites = 0usize;
    let mut identity_final_rewrites = 0usize;
    let blocked_before_start = blocked_reason.is_some();
    let mut stop_category: Option<String> = blocked_reason;
    let had_unverified_key = !candidate.credentials_available();

    for case in stratified_case_order(cases) {
        let preparation_started = Instant::now();
        let prepared = prepare_cleanup_case_with_fixture(case, preparation);
        let preparation_ms = elapsed_ms(preparation_started);
        preparation_latencies.push(preparation_ms);
        let route = if prepared.snippet {
            "snippet_bypass".to_owned()
        } else {
            match prepared.route {
                crate::lexicon::CleanupRoute::LocalOnly => "local_only".to_owned(),
                crate::lexicon::CleanupRoute::Provider(effort) => {
                    format!("provider:{}", effort.as_label())
                }
            }
        };

        let should_invoke = prepared.bypass.is_none()
            && matches!(prepared.route, crate::lexicon::CleanupRoute::Provider(_));
        route_eligible_cases += usize::from(should_invoke && case.mode != "provider-failure");
        let endpoint = candidate.endpoint.clone();
        let model = candidate.request_model(&settings);
        let policy = crate::cleanup_policy_for(&settings, &prepared.snapshot);
        let effort = match prepared.route {
            crate::lexicon::CleanupRoute::Provider(effort) => Some(effort),
            crate::lexicon::CleanupRoute::LocalOnly => None,
        };
        let request_fingerprint = cleanup_request_fingerprint(
            candidate,
            case,
            &prepared,
            CleanupRequestFingerprintContext {
                endpoint: &endpoint,
                model: &model,
                policy: &policy,
                effort,
                corpus_fingerprint,
                cleanup_code_fingerprint,
            },
        );
        let resumed = if should_invoke {
            replay
                .map(|candidate| candidate.value().to_owned())
                .or_else(|| {
                    resume_candidate(resume_state, candidate.id, &case.id, &request_fingerprint)
                })
        } else {
            None
        };
        let mut provider_invoked = false;
        let mut resumed_candidate = false;
        let mut provider_call_state = String::new();
        let mut adapter_candidate = None;
        let mut provider_ms = None;
        let mut provider_wait_ms = 0.0;
        let mut retry_wait_ms = 0.0;
        let mut provider_attempts = 0usize;
        let mut error_category = None;
        let mut status = None;
        let mut adapter_guard_rejected = false;
        let decision = if prepared.snippet {
            *bypass_counts.entry("snippet_bypass".into()).or_insert(0) += 1;
            crate::CleanupDecision::Disabled
        } else if case.mode == "provider-failure" && should_invoke {
            provider_call_state = "simulated_provider_failure_no_live_call".into();
            *bypass_counts
                .entry("simulated_provider_failure".into())
                .or_insert(0) += 1;
            crate::CleanupDecision::Failed
        } else if case.mode == "provider-failure" || !should_invoke {
            if let Some(bypass) = prepared.bypass {
                provider_call_state = bypass.to_owned();
                *bypass_counts.entry(bypass.to_owned()).or_insert(0) += 1;
            } else {
                provider_call_state = "production_route_local_only".into();
                *bypass_counts
                    .entry("production_route_local_only".into())
                    .or_insert(0) += 1;
            }
            crate::CleanupDecision::Disabled
        } else if let Some(text) = resumed {
            // Reuse only the text candidate: rerun the exact live acceptance
            // owner, then the same production finalizer below. Never old final text.
            resumed_candidate = true;
            resumed_candidate_samples += 1;
            match llm::accept_cleanup_reply(text, prepared.intent()) {
                Ok(text) => {
                    candidate_successes += 1;
                    provider_call_state = "resumed_compatible_adapter_candidate".into();
                    adapter_candidate = Some(text.clone());
                    crate::CleanupDecision::Provider(text)
                }
                Err(error) => {
                    let (category, http_status, is_guard_rejection, _) = classify_llm_error(&error);
                    error_category = Some(category.to_owned());
                    status = http_status;
                    adapter_guard_rejected = is_guard_rejection;
                    adapter_guard_rejections += usize::from(is_guard_rejection);
                    provider_call_state = "replayed_candidate_current_guard_rejected".into();
                    if is_guard_rejection {
                        crate::CleanupDecision::GuardRejected
                    } else {
                        crate::CleanupDecision::Failed
                    }
                }
            }
        } else if had_unverified_key {
            provider_call_state = "not_verified_missing_key".into();
            error_category = Some("no_explicit_key".into());
            *bypass_counts
                .entry("not_verified_missing_key".into())
                .or_insert(0) += 1;
            crate::CleanupDecision::Failed
        } else if let Some(stopped) = stop_category.as_deref() {
            provider_call_state = "not_called_after_provider_error".into();
            error_category = Some(stopped.to_owned());
            provider_skipped_after_error_cases += 1;
            crate::CleanupDecision::Failed
        } else {
            let key = settings.cleanup_credential().to_owned();
            let effort = effort.expect("provider routes have cleanup effort");
            let mut call_total_ms = 0.0;
            let mut retries_this_case = 0usize;
            let result = loop {
                let pacing_wait = pacer.wait_before_request().await;
                let pacing_ms = duration_ms(pacing_wait);
                provider_wait_ms += pacing_ms;
                if pacing_ms > 0.0 {
                    pacing_wait_latencies.push(pacing_ms);
                }
                if !provider_invoked {
                    provider_invoked = true;
                    provider_invoked_cases += 1;
                }
                provider_attempts += 1;
                provider_http_attempts += 1;
                let provider_started = Instant::now();
                let future = async {
                    match &candidate.transport {
                        CleanupTransport::Cloud(_) => {
                            llm::cleanup_with_model_and_limits_and_language_and_profile_and_intent(
                                &endpoint,
                                &model,
                                &prepared.intent().content,
                                &key,
                                &[],
                                None,
                                Some(&policy),
                                Some("auto"),
                                Some(&prepared.snapshot.profile),
                                Some(prepared.intent()),
                                prepared.pairs_hint(),
                                effort,
                                None,
                            )
                            .await
                        }
                        CleanupTransport::VerifiedLocalOpenAi { lease, cancel } => {
                            let mut owner = lease.lock().await;
                            let mut capability = owner
                                .capability()
                                .map_err(|e| llm::LlmError::Other(e.to_string()))?;
                            llm::cleanup_verified_local(
                                &mut capability,
                                prepared.intent(),
                                &llm::CleanupPromptContext {
                                    dictionary: &[],
                                    context: None,
                                    policy: Some(&policy),
                                    profile: Some(&prepared.snapshot.profile),
                                    pairs_hint: prepared.pairs_hint(),
                                    effort,
                                    visible_context: None,
                                    source_transcript: None,
                                },
                                cancel,
                            )
                            .await
                        }
                    }
                };
                if let Some(observer) = observer {
                    observer.started(&case.id, AttemptStage::Cleanup, provider_attempts as u32)?;
                }
                let response = future.await;
                if let Some(observer) = observer {
                    observer.completed(
                        &case.id,
                        AttemptStage::Cleanup,
                        provider_attempts as u32,
                    )?;
                }
                let call_ms = elapsed_ms(provider_started);
                call_total_ms += call_ms;
                provider_latencies.push(call_ms);
                let retry_delay = match &response {
                    Err(llm::LlmError::RateLimited(retry_after))
                        if retries_this_case < RATE_LIMIT_RETRIES_PER_CASE =>
                    {
                        Some(bounded_retry_delay(retry_after))
                    }
                    _ => None,
                };
                if let Some(delay) = retry_delay {
                    retries_this_case += 1;
                    let wait_ms = duration_ms(delay);
                    rate_limit_retries += 1;
                    rate_limit_retry_wait_ms += wait_ms;
                    retry_wait_ms += wait_ms;
                    provider_wait_ms += wait_ms;
                    retry_wait_latencies.push(wait_ms);
                    tokio::time::sleep(delay).await;
                    continue;
                }
                break response;
            };
            provider_ms = Some(call_total_ms);
            match result {
                Ok((text, _limits)) => {
                    candidate_successes += 1;
                    candidate_coverage_cases += 1;
                    provider_call_state = if provider_attempts > 1 {
                        "adapter_candidate_returned_after_rate_limit_retry".into()
                    } else {
                        "adapter_candidate_returned".into()
                    };
                    adapter_candidate = Some(text.clone());
                    resume_state.insert(
                        candidate.id.to_owned(),
                        case.id.clone(),
                        request_fingerprint.clone(),
                        text.clone(),
                    );
                    if let Some(checkpoint_path) = checkpoint_path {
                        write_resume_checkpoint(
                            checkpoint_path,
                            resume_state,
                            corpus_fingerprint,
                            cleanup_code_fingerprint,
                        )
                        .map_err(|failure| {
                            ExecutionError::artifact(
                                checkpoint_path,
                                failure,
                                provider_http_attempts as u64,
                            )
                        })?;
                    }
                    crate::CleanupDecision::Provider(text)
                }
                Err(error) => {
                    let (category, http_status, is_guard_rejection, stop_after_error) =
                        classify_llm_error(&error);
                    error_category = Some(category.to_owned());
                    status = http_status;
                    *provider_error_categories
                        .entry(category.to_owned())
                        .or_insert(0) += 1;
                    if is_guard_rejection {
                        adapter_guard_rejections += 1;
                        adapter_guard_rejected = true;
                        provider_call_state = "adapter_guard_rejected_candidate_unavailable".into();
                        crate::CleanupDecision::GuardRejected
                    } else {
                        provider_call_state = "provider_error_fallback".into();
                        if stop_after_error {
                            stop_category = Some(category.to_owned());
                        }
                        crate::CleanupDecision::Failed
                    }
                }
            }
        };
        if adapter_candidate.is_some() && resumed_candidate {
            candidate_coverage_cases += 1;
        }
        if provider_wait_ms > 0.0 {
            provider_wait_latencies.push(provider_wait_ms);
        }

        let finalize_started = Instant::now();
        let finalized = crate::finalize_text_for_scene(
            prepared.source(),
            decision,
            prepared.finalization_context(),
        )
        .unwrap_or_else(|error| {
            assert_eq!(error, "no_speech");
            crate::FinalText {
                text: String::new(),
                degraded: false,
                degraded_reason: None,
                cleanup_rejection: None,
                final_guard_reason: None,
            }
        });
        let finalize_ms = elapsed_ms(finalize_started);
        finalize_latencies.push(finalize_ms);
        if finalized.degraded_reason == Some("preservation_guard") {
            app_final_guard_rejections += 1;
        }

        let candidate_protected = adapter_candidate.as_deref().map(|candidate| {
            case.protected
                .iter()
                .all(|span| candidate.to_lowercase().contains(&span.to_lowercase()))
        });
        let final_protected = case
            .protected
            .iter()
            .all(|span| finalized.text.to_lowercase().contains(&span.to_lowercase()));
        if !case.protected.is_empty() {
            if let Some(candidate_preserved) = candidate_protected {
                declared_protected_candidate_samples += 1;
                declared_protected_candidate_preserved += usize::from(candidate_preserved);
                let candidate = adapter_candidate
                    .as_deref()
                    .expect("candidate preservation is Some only for a candidate");
                for span in &case.protected {
                    declared_protected_candidate_span_samples += 1;
                    declared_protected_candidate_spans_preserved +=
                        usize::from(candidate.to_lowercase().contains(&span.to_lowercase()));
                }
            }
            declared_protected_final_samples += 1;
            declared_protected_final_preserved += usize::from(final_protected);
            let finalized_lower = finalized.text.to_lowercase();
            for span in &case.protected {
                declared_protected_final_span_samples += 1;
                declared_protected_final_spans_preserved +=
                    usize::from(finalized_lower.contains(&span.to_lowercase()));
            }
        }

        let mixed = has_cjk(&case.input) && has_latin(&case.input);
        let candidate_mixed = adapter_candidate
            .as_deref()
            .map(|candidate| has_cjk(candidate) && has_latin(candidate));
        let final_mixed = has_cjk(&finalized.text) && has_latin(&finalized.text);
        if mixed {
            if let Some(candidate_preserved) = candidate_mixed {
                mixed_script_candidate_samples += 1;
                mixed_script_candidate_preserved += usize::from(candidate_preserved);
            }
            mixed_script_final_samples += 1;
            mixed_script_final_preserved += usize::from(final_mixed);
        }
        let identity_candidate_rewrite = if case.mode == "identity" {
            adapter_candidate.as_deref().map(|candidate| {
                identity_candidate_cases += 1;
                let changed = candidate != prepared.source();
                identity_candidate_rewrites += usize::from(changed);
                changed
            })
        } else {
            None
        };
        let identity_final_rewrite = if case.mode == "identity" && adapter_candidate.is_some() {
            let changed = finalized.text != prepared.source();
            identity_final_rewrites += usize::from(changed);
            Some(changed)
        } else {
            None
        };

        let expected_metrics = QualityAccumulator::observation(case, &finalized.text);
        all_path_quality.add(expected_metrics);
        let final_reference_match = any_reference_match(case, &finalized.text);
        let adapter_reference_match = adapter_candidate
            .as_deref()
            .map(|candidate| any_reference_match(case, candidate));
        if let Some(candidate) = adapter_candidate.as_deref() {
            let candidate_observation = QualityAccumulator::observation(case, candidate);
            candidate_quality.add(candidate_observation);
            let final_observation = QualityAccumulator::observation(case, &finalized.text);
            finalized_provider_quality.add(final_observation);
        }
        let final_span_guard_preserved = crate::protected_span::preserves(
            prepared.source(),
            &finalized.text,
            Some(prepared.intent().operation),
        );
        let total_ms = preparation_ms + provider_ms.unwrap_or(0.0) + finalize_ms;
        total_latencies.push(total_ms);
        results.push(CleanupCaseReport {
            id: case.id.clone(),
            group: case.group.clone(),
            mode: case.mode.clone(),
            cleanup: case.cleanup.clone(),
            family: case.family.clone(),
            focus_kind: case.focus_kind.clone(),
            input: case.input.clone(),
            request_fingerprint,
            route,
            provider_invoked,
            resumed_candidate,
            provider_call_state,
            provider_attempts,
            adapter_candidate,
            final_text: finalized.text,
            expected: case.expected.clone(),
            adapter_candidate_reference_match: adapter_reference_match,
            final_reference_match,
            adapter_candidate_declared_protected_preserved: candidate_protected,
            final_declared_protected_preserved: final_protected,
            final_span_guard_preserved,
            mixed_script_proxy_preserved: candidate_mixed,
            identity_candidate_rewrite,
            identity_final_rewrite,
            adapter_guard_rejected,
            app_final_guard_rejected: finalized.degraded_reason == Some("preservation_guard"),
            error_category,
            status,
            preparation_ms,
            provider_ms,
            provider_wait_ms,
            retry_wait_ms,
            finalize_ms,
            total_ms,
        });
    }

    let all_candidates_covered =
        route_eligible_cases > 0 && candidate_coverage_cases == route_eligible_cases;
    let verification_status =
        if all_candidates_covered && provider_invoked_cases == 0 && resumed_candidate_samples > 0 {
            "resumed"
        } else if all_candidates_covered {
            "completed"
        } else if stop_category.is_some() {
            "partial"
        } else if !candidate.credentials_available() || blocked_before_start {
            if candidate_coverage_cases > 0 {
                "partial"
            } else {
                "not_verified"
            }
        } else if candidate_coverage_cases > 0 {
            "partial"
        } else {
            "not_verified"
        };
    let mut timings = BTreeMap::new();
    timings.insert("preparation".into(), timing_summary(preparation_latencies));
    timings.insert("provider".into(), timing_summary(provider_latencies));
    timings.insert(
        "provider_wait".into(),
        timing_summary(provider_wait_latencies),
    );
    timings.insert(
        "request_pacing_wait".into(),
        timing_summary(pacing_wait_latencies),
    );
    timings.insert(
        "retry_after_wait".into(),
        timing_summary(retry_wait_latencies),
    );
    timings.insert("finalize".into(), timing_summary(finalize_latencies));
    timings.insert("total".into(), timing_summary(total_latencies));
    Ok(CleanupModelReport {
        candidate: candidate.id.into(),
        provider: candidate.provider_name().into(),
        model: candidate.model.clone(),
        sanitized_host: host,
        key_source: candidate.key_source.clone(),
        verification_status: verification_status.into(),
        stop_category,
        route_eligible_cases,
        candidate_coverage_cases,
        candidate_coverage_rate: (route_eligible_cases != 0)
            .then(|| candidate_coverage_cases as f64 / route_eligible_cases as f64),
        provider_invoked_cases,
        provider_http_attempts,
        rate_limit_retries,
        rate_limit_retry_wait_ms,
        minimum_request_gap_ms: pacer.minimum_gap.as_millis().min(u64::MAX as u128) as u64,
        resumed_candidate_samples,
        provider_skipped_after_error_cases,
        adapter_candidate_successes: candidate_successes,
        adapter_guard_rejections,
        app_final_guard_rejections,
        provider_error_categories,
        bypass_counts,
        declared_protected_cases,
        declared_protected_spans,
        declared_protected_candidate_samples,
        declared_protected_candidate_preserved,
        declared_protected_candidate_span_samples,
        declared_protected_candidate_spans_preserved,
        declared_protected_final_samples,
        declared_protected_final_preserved,
        declared_protected_final_span_samples,
        declared_protected_final_spans_preserved,
        mixed_script_proxy_cases,
        mixed_script_candidate_samples,
        mixed_script_candidate_preserved,
        mixed_script_final_samples,
        mixed_script_final_preserved,
        identity_candidate_cases,
        identity_candidate_rewrites,
        identity_final_rewrites,
        adapter_candidate_quality: candidate_quality.summary(),
        finalized_provider_success_quality: finalized_provider_quality.summary(),
        all_path_final_quality: all_path_quality.summary(),
        timings,
        cases: results,
    })
}

#[cfg(test)]
pub(crate) fn prepare_cleanup_case(case: &EvalCase) -> PreparedCleanupCase {
    prepare_cleanup_case_with_fixture(case, None)
}
pub(crate) fn prepare_cleanup_case_with_fixture(
    case: &EvalCase,
    fixture: Option<&super::controls::PreparationFixture>,
) -> PreparedCleanupCase {
    let family = corpus_family(case);
    let input_kind = corpus_focus_kind(&case.focus_kind);
    let mut snapshot = ContextSnapshot::general();
    snapshot.profile.family = family;
    snapshot.profile.confidence = 1.0;
    snapshot.policy = ContextPolicy::for_family(family);
    snapshot.policy.input_kind = input_kind;
    let settings = crate::store::Settings {
        cleanup_intensity: case.cleanup.clone(),
        cleanup_enabled: case.mode != "ai-off",
        dictionary: fixture.map(|f| f.dictionary.clone()).unwrap_or_default(),
        fuzzy_dictionary_enabled: fixture.is_some_and(|f| f.fuzzy),
        ..crate::store::Settings::default()
    };
    if case.mode == "snippet" {
        let snippet = crate::snippets::Snippet {
            id: case.id.clone(),
            trigger: case.input.clone(),
            expansion: case.expected.clone(),
            enabled: true,
        };
        let expanded = crate::snippets::resolve_exact_with_clipboard(&[snippet], &case.input, None)
            .unwrap_or_default();
        let intent = llm::parse_cleanup_intent(&expanded, None);
        return PreparedCleanupCase {
            family,
            input_kind,
            source: PreparedCleanupSource::Snippet {
                source: expanded,
                intent,
            },
            snapshot,
            route: crate::lexicon::CleanupRoute::LocalOnly,
            bypass: Some("snippet_bypass"),
            snippet: true,
        };
    }

    // Consume the same preparation owner used by dictation, preview and the
    // grouped production evaluator. Keep its authorizations for finalization.
    let scene =
        crate::resolve_cleanup_scene_for_transcript(&settings, Some(&snapshot), &case.input);
    let preparation = crate::prepare_cleanup_transcript_with_resolved_rules(
        None,
        &settings,
        &case.input,
        scene,
        1.0,
        crate::PreparationRules {
            spoken_layout: fixture.is_none_or(|f| f.spoken_layout),
            explicit_pairs: Some(fixture.map(|f| f.pairs.as_slice()).unwrap_or(&[])),
            learn_scope: None,
            fuzzy_dictionary_enabled: settings.fuzzy_dictionary_enabled,
        },
    );
    let route =
        crate::cleanup_route_for_resolved_scene(&settings, preparation.scene, &preparation.intent);
    let bypass = match case.mode.as_str() {
        "ai-off" => Some("cleanup_off"),
        "provider-failure" if matches!(route, crate::lexicon::CleanupRoute::LocalOnly) => {
            Some("provider_failure_but_local_route")
        }
        "snippet" => Some("snippet_bypass"),
        _ => None,
    };
    PreparedCleanupCase {
        family,
        input_kind,
        source: PreparedCleanupSource::Transcript(Box::new(preparation)),
        snapshot,
        route,
        bypass,
        snippet: false,
    }
}

pub(crate) fn cleanup_host(candidate: &CleanupCandidate) -> String {
    providers::host_of(&candidate.endpoint)
}

pub(crate) fn model_override(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .unwrap_or_else(|| default.to_owned())
}

pub(crate) fn minimum_request_gap() -> Duration {
    let milliseconds = std::env::var("VOICEFLOW_LIVE_MIN_REQUEST_GAP_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_MIN_REQUEST_GAP_MS)
        .min(5_000);
    Duration::from_millis(milliseconds)
}

pub(crate) fn stable_fingerprint(parts: &[&[u8]]) -> String {
    // FNV-1a is used as a stable compatibility fingerprint, not as a
    // security boundary or a hash of credentials.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for part in parts {
        for byte in *part {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fnv1a64:{hash:016x}")
}

pub(crate) fn cleanup_code_fingerprint() -> String {
    let lib = include_str!("../lib.rs");
    let corpus_eval = include_str!("../cleanup_corpus.rs");
    let sources: &[(&[u8], &[u8])] = &[
        (
            b"request_builder_revision",
            REQUEST_BUILDER_REVISION.as_bytes(),
        ),
        (
            b"finalizer_context_and_guards",
            source_between(
                lib,
                "pub(crate) struct FinalizationContext<'a> {",
                "fn scene_allows_automatic_lexicon(",
            )
            .as_bytes(),
        ),
        (
            b"scene_cleanup_policy_translation",
            source_between(
                lib,
                "fn scene_allows_automatic_lexicon(",
                "fn delivery_fallback_reason(",
            )
            .as_bytes(),
        ),
        (
            b"preparation_functions",
            source_between(
                lib,
                "pub(crate) fn load_learn_pairs(",
                "fn asr_prompt_for_snapshot(",
            )
            .as_bytes(),
        ),
        (
            b"cleanup_routing",
            source_between(lib, "fn cleanup_route_for(", "fn abort_processing_manager(").as_bytes(),
        ),
        (
            b"grouped_finalizer",
            source_between(
                corpus_eval,
                "pub(super) fn finalize(",
                "fn context_snapshot(",
            )
            .as_bytes(),
        ),
        (
            b"evaluation/legacy.rs",
            include_str!("legacy.rs").as_bytes(),
        ),
        (b"llm.rs", include_str!("../llm.rs").as_bytes()),
        (
            b"cleanup_prompt.rs",
            include_str!("../cleanup_prompt.rs").as_bytes(),
        ),
        (
            b"cleanup_output_layout.rs",
            include_str!("../cleanup_output_layout.rs").as_bytes(),
        ),
        (
            b"cleanup_output_resolver.rs",
            include_str!("../cleanup_output_resolver.rs").as_bytes(),
        ),
        (
            b"cleanup_structure_guard.rs",
            include_str!("../cleanup_structure_guard.rs").as_bytes(),
        ),
        (b"context.rs", include_str!("../context.rs").as_bytes()),
        (b"providers.rs", include_str!("../providers.rs").as_bytes()),
        (b"store.rs", include_str!("../store.rs").as_bytes()),
        (b"lexicon.rs", include_str!("../lexicon.rs").as_bytes()),
        (
            b"protected_span.rs",
            include_str!("../protected_span.rs").as_bytes(),
        ),
        (
            b"spoken_layout.rs",
            include_str!("../spoken_layout.rs").as_bytes(),
        ),
        (
            b"spoken_punctuation.rs",
            include_str!("../spoken_punctuation.rs").as_bytes(),
        ),
        (
            b"spoken_revision.rs",
            include_str!("../spoken_revision.rs").as_bytes(),
        ),
        (b"snippets.rs", include_str!("../snippets.rs").as_bytes()),
        (b"queue.rs", include_str!("../queue.rs").as_bytes()),
    ];
    let parts: Vec<&[u8]> = sources
        .iter()
        .flat_map(|(name, source)| [*name, *source])
        .collect();
    stable_fingerprint(&parts)
}

pub(crate) fn source_between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start_index = source
        .find(start)
        .unwrap_or_else(|| panic!("missing cleanup fingerprint section {start}"));
    let content = &source[start_index..];
    let end_index = content
        .find(end)
        .unwrap_or_else(|| panic!("missing cleanup fingerprint boundary {end}"));
    &content[..end_index]
}

pub(crate) fn cleanup_request_fingerprint(
    candidate: &CleanupCandidate,
    case: &EvalCase,
    prepared: &PreparedCleanupCase,
    context: CleanupRequestFingerprintContext<'_>,
) -> String {
    let identity = CleanupRequestIdentity {
        request_builder_revision: REQUEST_BUILDER_REVISION,
        corpus_fingerprint: context.corpus_fingerprint,
        cleanup_code_fingerprint: context.cleanup_code_fingerprint,
        candidate: candidate.id,
        provider: candidate.provider_name(),
        model: context.model,
        endpoint: context.endpoint,
        case_id: &case.id,
        group: &case.group,
        mode: &case.mode,
        cleanup: &case.cleanup,
        family: &case.family,
        focus_kind: &case.focus_kind,
        source: prepared.source(),
        request_content: &prepared.intent().content,
        intent: prepared.intent(),
        pairs_hint: prepared.pairs_hint(),
        profile: &prepared.snapshot.profile,
        policy: context.policy,
        effort: context.effort.map(crate::llm::CleanupEffort::as_label),
        fixed_options: "dictionary=[];context=None;language=auto;visible_context=None;temperature=0;max_completion_tokens=4096;stream=true",
    };
    let bytes = serde_json::to_vec(&identity).expect("serialize cleanup request identity");
    stable_fingerprint(&[&bytes])
}

impl ResumeState {
    pub(crate) fn insert(
        &mut self,
        candidate: String,
        case_id: String,
        request_fingerprint: String,
        adapter_candidate: String,
    ) {
        self.cases_by_candidate
            .entry(candidate)
            .or_default()
            .insert(
                case_id,
                ResumeCase {
                    request_fingerprint,
                    adapter_candidate,
                },
            );
    }
}

pub(crate) fn resume_candidate(
    state: &ResumeState,
    candidate: &str,
    case_id: &str,
    request_fingerprint: &str,
) -> Option<String> {
    state
        .cases_by_candidate
        .get(candidate)?
        .get(case_id)
        .filter(|previous| previous.request_fingerprint == request_fingerprint)
        .map(|previous| previous.adapter_candidate.clone())
}

pub(crate) fn write_resume_checkpoint(
    path: &Path,
    state: &ResumeState,
    corpus_fingerprint: &str,
    cleanup_code_fingerprint: &str,
) -> Result<(), ArtifactFailure> {
    let candidates: Vec<ResumeCheckpointCandidate<'_>> = state
        .cases_by_candidate
        .iter()
        .map(|(candidate, cases)| ResumeCheckpointCandidate {
            candidate,
            cases: cases
                .iter()
                .map(|(id, value)| ResumeCheckpointCase {
                    id,
                    request_fingerprint: &value.request_fingerprint,
                    adapter_candidate: &value.adapter_candidate,
                })
                .collect(),
        })
        .collect();
    let checkpoint = ResumeCheckpoint {
        schema_version: REPORT_SCHEMA_VERSION,
        corpus_fingerprint,
        cleanup_code_fingerprint,
        checkpoint_candidates: candidates,
    };
    state
        .artifacts
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .write_json(path, &checkpoint)
}

pub(crate) fn unique_output_path(path: &Path, reserved: Option<&Path>) -> PathBuf {
    if !path.exists() && reserved != Some(path) {
        return path.to_path_buf();
    }
    for index in 2..10_000 {
        let candidate = with_path_suffix(path, "run", index);
        if !candidate.exists() && reserved != Some(candidate.as_path()) {
            return candidate;
        }
    }
    panic!("unable to choose a unique external evaluation output path");
}

pub(crate) fn with_path_suffix(path: &Path, suffix: &str, index: usize) -> PathBuf {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("voiceflow-live-eval.json");
    let file_path = Path::new(file_name);
    let stem = file_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(file_name);
    let extension = file_path.extension().and_then(|value| value.to_str());
    let name = match extension {
        Some(extension) => format!("{stem}.{suffix}-{index}.{extension}"),
        None => format!("{stem}.{suffix}-{index}"),
    };
    parent.join(name)
}

pub(crate) struct RequestPacer {
    pub(crate) minimum_gap: Duration,
    pub(crate) last_request_started: Option<Instant>,
}

impl RequestPacer {
    pub(crate) fn new(minimum_gap: Duration) -> Self {
        Self {
            minimum_gap,
            last_request_started: None,
        }
    }

    pub(crate) async fn wait_before_request(&mut self) -> Duration {
        let Some(last_started) = self.last_request_started else {
            self.last_request_started = Some(Instant::now());
            return Duration::ZERO;
        };
        let remaining = self.minimum_gap.saturating_sub(last_started.elapsed());
        if !remaining.is_zero() {
            tokio::time::sleep(remaining).await;
        }
        self.last_request_started = Some(Instant::now());
        remaining
    }
}

pub(crate) fn bounded_retry_delay(value: &str) -> Duration {
    crate::queue::bounded_retry_after(Some(value)).unwrap_or(Duration::from_secs(1))
}

pub(crate) fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

pub(crate) fn stratified_case_order(cases: &[EvalCase]) -> Vec<&EvalCase> {
    let mut mixed = Vec::new();
    let mut chinese = Vec::new();
    let mut identity = Vec::new();
    let mut remaining = Vec::new();
    for case in cases {
        if case.group == "mixed" || (has_cjk(&case.input) && has_latin(&case.input)) {
            mixed.push(case);
        } else if case.group == "chinese" || has_cjk(&case.input) {
            chinese.push(case);
        } else if case.mode == "identity" {
            identity.push(case);
        } else {
            remaining.push(case);
        }
    }
    let mut ordered = Vec::with_capacity(cases.len());
    let bucket_count = mixed
        .len()
        .max(chinese.len())
        .max(identity.len())
        .max(remaining.len());
    for index in 0..bucket_count {
        if let Some(case) = mixed.get(index) {
            ordered.push(*case);
        }
        if let Some(case) = chinese.get(index) {
            ordered.push(*case);
        }
        if let Some(case) = identity.get(index) {
            ordered.push(*case);
        }
        if let Some(case) = remaining.get(index) {
            ordered.push(*case);
        }
    }
    ordered
}

pub(crate) fn write_json_report<T: Serialize>(
    artifacts: &mut ArtifactStore,
    path: &Path,
    report: &T,
) -> Result<(), ArtifactFailure> {
    artifacts.write_json(path, report)
}

pub(crate) fn asr_candidates() -> Vec<AsrCandidate> {
    let optional = |provider: CredentialProvider| {
        provider
            .resolve()
            .unwrap_or_else(|_| KeyMaterial::unavailable("explicit_key_file_unavailable"))
    };
    let groq_key = optional(CredentialProvider::Groq);
    let openai_key = optional(CredentialProvider::OpenAi);
    let qwen_key = optional(CredentialProvider::Qwen);
    let mut candidates = Vec::new();
    for (id, model) in [
        (
            "groq_whisper_large_v3_turbo",
            model_override(
                "VOICEFLOW_EVAL_GROQ_ASR_PRIMARY_MODEL",
                "whisper-large-v3-turbo",
            ),
        ),
        (
            "groq_whisper_large_v3",
            model_override(
                "VOICEFLOW_EVAL_GROQ_ASR_SECONDARY_MODEL",
                "whisper-large-v3",
            ),
        ),
    ] {
        candidates.push(asr_candidate(
            id,
            "groq",
            model,
            EngineProvider::Groq.default_base_url(),
            groq_key.clone(),
        ));
    }
    candidates.push(asr_candidate(
        "openai_gpt_transcribe",
        "openai",
        model_override("VOICEFLOW_EVAL_OPENAI_ASR_MODEL", "gpt-transcribe"),
        EngineProvider::OpenAi.default_base_url(),
        openai_key,
    ));
    candidates.push(asr_candidate(
        "qwen3_asr_flash_dashscope",
        "qwen",
        model_override("VOICEFLOW_EVAL_QWEN_ASR_MODEL", "qwen3-asr-flash"),
        QWEN_ASR_BASE,
        qwen_key,
    ));
    candidates
}

pub(crate) fn asr_candidate(
    id: &'static str,
    provider: &'static str,
    model: String,
    base: &str,
    key: KeyMaterial,
) -> AsrCandidate {
    let engine_provider = match provider {
        "groq" => EngineProvider::Groq,
        "openai" => EngineProvider::OpenAi,
        "qwen" => EngineProvider::Custom,
        _ => unreachable!(),
    };
    let endpoint = providers::resolve_asr_endpoint(engine_provider, base)
        .expect("HTTP ASR candidate must have an endpoint");
    let host = providers::host_of(&endpoint);
    AsrCandidate {
        id,
        provider,
        model,
        authorization_scope: key.scope(&endpoint),
        execution: AsrExecution::Cloud {
            endpoint,
            key: key.key,
        },
        host,
        language: None,
        prompt: Some("VoiceFlow, TypeScript, API".into()),
        keywords: vec!["VoiceFlow".into(), "TypeScript".into(), "API".into()],
        key_source: key.source,
    }
}

pub(crate) async fn run_asr_candidate(
    candidate: &AsrCandidate,
    manifest_path: &Path,
    manifest: &AudioManifest,
    blocked_reason: Option<String>,
    observer: Option<&dyn DispatchObserver>,
) -> Result<AsrModelReport, ExecutionError> {
    run_asr_candidate_with_replay(
        candidate,
        manifest_path,
        manifest,
        blocked_reason,
        observer,
        None,
    )
    .await
}
pub(crate) async fn run_asr_candidate_with_replay(
    candidate: &AsrCandidate,
    manifest_path: &Path,
    manifest: &AudioManifest,
    blocked_reason: Option<String>,
    observer: Option<&dyn DispatchObserver>,
    replay: Option<&super::resume::ValidatedCandidate>,
) -> Result<AsrModelReport, ExecutionError> {
    let fixtures: Vec<_> = manifest
        .cases
        .iter()
        .map(|f| AsrAudioInput {
            id: &f.id,
            audio: &f.audio,
            reference: Some(&f.reference),
            validated_audio: f.validated_audio.as_deref(),
        })
        .collect();
    run_asr_audio_inputs(
        candidate,
        manifest_path,
        &fixtures,
        blocked_reason,
        observer,
        replay,
    )
    .await
}
pub(crate) async fn run_asr_audio_inputs(
    candidate: &AsrCandidate,
    manifest_path: &Path,
    fixtures: &[AsrAudioInput<'_>],
    blocked_reason: Option<String>,
    observer: Option<&dyn DispatchObserver>,
    replay: Option<&super::resume::ValidatedCandidate>,
) -> Result<AsrModelReport, ExecutionError> {
    let mut raw_quality = QualityAccumulator::default();
    let mut sanitized_quality = QualityAccumulator::default();
    let mut latencies = Vec::new();
    let mut results = Vec::new();
    let mut errors = BTreeMap::new();
    let mut invoked = 0usize;
    let mut successful = 0usize;
    let mut no_speech = 0usize;
    let blocked_before_start = blocked_reason.is_some();
    let mut stop_category = blocked_reason;
    for fixture in fixtures {
        let mut result = AsrCaseReport {
            id: fixture.id.to_owned(),
            reference: fixture.reference.map(str::to_owned),
            provider_invoked: false,
            transcription_state: String::new(),
            raw_asr_text: None,
            sanitized_text: None,
            detected_language: None,
            confidence: None,
            segment_count: None,
            word_count: None,
            error_category: None,
            status: None,
            latency_ms: None,
        };
        if let Some(replay) = replay {
            result.transcription_state = if replay.known_empty() {
                "empty_result"
            } else {
                "transcribed"
            }
            .into();
            result.error_category = replay.observation().and_then(|o| o.filtering_reason);
            result.sanitized_text = Some(replay.value().to_owned());
            // Historical parser/filter facts are attached separately by the
            // replay owner. No new provider raw text, timing or attempt is invented.
            if let Some(reference) = fixture.reference {
                sanitized_quality.add(quality_for_pair(reference, &[], replay.value()));
            }
            if replay.known_empty() {
                no_speech += 1;
            } else {
                successful += 1;
            }
            results.push(result);
            continue;
        }
        if !candidate.credentials_available() {
            result.transcription_state = "not_verified_missing_key".into();
            result.error_category = Some("no_explicit_key".into());
            *errors.entry("no_explicit_key".into()).or_insert(0) += 1;
            results.push(result);
            continue;
        }
        if let Some(stopped) = stop_category.as_deref() {
            result.transcription_state = "not_called_after_provider_error".into();
            result.error_category = Some(stopped.to_owned());
            *errors.entry(stopped.to_owned()).or_insert(0) += 1;
            results.push(result);
            continue;
        }
        let audio = match fixture.validated_audio {
            Some(bytes) => bytes.to_vec(),
            None => std::fs::read(safe_audio_fixture_path(manifest_path, fixture.audio))
                .expect("read synthetic WAV fixture"),
        };
        result.provider_invoked = true;
        invoked += 1;
        let provider: Box<dyn AsrProvider> = match &candidate.execution {
            AsrExecution::Cloud { endpoint, .. } => {
                Box::new(GroqAsrProvider::from_resolved_endpoint(endpoint))
            }
            AsrExecution::Local { models_root } => {
                Box::new(crate::ondevice_asr::OnDeviceAsrProvider::new(
                    models_root.clone(),
                    candidate.model.clone(),
                ))
            }
        };
        let started = Instant::now();
        let future = provider.transcribe_batch(
            audio,
            AsrOptions {
                api_key: match &candidate.execution {
                    AsrExecution::Cloud { key, .. } => key.clone().unwrap_or_default(),
                    AsrExecution::Local { .. } => String::new(),
                },
                language: candidate.language.clone(),
                prompt: candidate.prompt.clone(),
                keywords: candidate.keywords.clone(),
                model: candidate.model.clone(),
                session_generation: None,
                trace: None,
            },
        );
        if let Some(observer) = observer {
            observer.started(fixture.id, AttemptStage::Asr, 1)?;
        }
        let response = future.await;
        if let Some(observer) = observer {
            observer.completed(fixture.id, AttemptStage::Asr, 1)?;
        }
        let elapsed = elapsed_ms(started);
        result.latency_ms = Some(elapsed);
        latencies.push(elapsed);
        match response {
            Ok(transcript) => {
                successful += 1;
                result.transcription_state = "transcribed".into();
                let raw_text = transcript.original_text().to_owned();
                let sanitized_text = transcript.text.clone();
                result.raw_asr_text = Some(raw_text.clone());
                result.sanitized_text = Some(sanitized_text.clone());
                result.detected_language = transcript.language.clone();
                result.confidence = transcript.confidence;
                result.segment_count = Some(transcript.segments.len());
                result.word_count = Some(transcript.words.len());
                if let Some(reference) = fixture.reference {
                    raw_quality.add(quality_for_pair(reference, &[], &raw_text));
                }
                if let Some(reference) = fixture.reference {
                    sanitized_quality.add(quality_for_pair(reference, &[], &sanitized_text));
                }
            }
            Err(AsrError::EmptyResult) => {
                no_speech += 1;
                successful += 1;
                result.transcription_state = "empty_result".into();
                result.raw_asr_text = Some(String::new());
                result.sanitized_text = Some(String::new());
                if let Some(reference) = fixture.reference {
                    raw_quality.add(quality_for_pair(reference, &[], ""));
                }
                if let Some(reference) = fixture.reference {
                    sanitized_quality.add(quality_for_pair(reference, &[], ""));
                }
            }
            Err(error) => {
                let (category, http_status, should_stop) = classify_asr_error(&error);
                result.transcription_state = "provider_error".into();
                result.error_category = Some(category.to_owned());
                result.status = http_status;
                *errors.entry(category.to_owned()).or_insert(0) += 1;
                if should_stop {
                    stop_category = Some(category.to_owned());
                }
            }
        }
        results.push(result);
    }
    let verification_status = if replay.is_some() {
        "replayed_text_candidate_not_live_verified"
    } else if !candidate.credentials_available() || blocked_before_start {
        "not_verified"
    } else if stop_category.is_some() {
        "partial"
    } else {
        "completed"
    };
    Ok(AsrModelReport {
        candidate: candidate.id.into(),
        provider: candidate.provider.into(),
        model: candidate.model.clone(),
        sanitized_host: candidate.host.clone(),
        key_source: candidate.key_source.clone(),
        verification_status: verification_status.into(),
        stop_category,
        provider_invoked_cases: invoked,
        successful_transcriptions: successful,
        no_speech_results: no_speech,
        error_categories: errors,
        raw_asr_quality: raw_quality.summary(),
        sanitized_text_quality: sanitized_quality.summary(),
        timings: timing_summary(latencies),
        cases: results,
    })
}

pub(crate) fn safe_audio_fixture_path(manifest_path: &Path, audio: &str) -> PathBuf {
    let relative = Path::new(audio);
    assert!(
        !relative.is_absolute()
            && relative.components().count() == 1
            && relative.extension().and_then(|value| value.to_str()) == Some("wav"),
        "audio manifest entries must be WAV filenames adjacent to the manifest"
    );
    manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(relative)
}

pub(crate) fn classify_llm_error(error: &llm::LlmError) -> (&'static str, Option<u16>, bool, bool) {
    match error {
        llm::LlmError::Network(_) => ("network_error", None, false, true),
        llm::LlmError::Timeout => ("timeout", None, false, true),
        llm::LlmError::Unauthorized => ("authorization_error", Some(401), false, true),
        llm::LlmError::RateLimited(_) => ("rate_limited", Some(429), false, true),
        llm::LlmError::ContextAuthorizationChanged => {
            ("context_authorization_changed", None, true, true)
        }
        llm::LlmError::Server(message) => ("server_error", parse_http_status(message), false, true),
        llm::LlmError::Guard(_) if llm::is_preservation_guard_error(error) => {
            ("adapter_guard_rejected", None, true, false)
        }
        llm::LlmError::Guard(_) => ("completion_guard_rejected", None, false, true),
        llm::LlmError::CleanupRejected(_) if llm::is_preservation_guard_error(error) => {
            // Legacy evaluation keeps the rejection category, not model text.
            ("adapter_guard_rejected", None, true, false)
        }
        llm::LlmError::CleanupRejected(_) => ("completion_guard_rejected", None, false, true),
        llm::LlmError::NativeLocal(failure) => (failure.legacy_error_category(), None, false, true),
        llm::LlmError::Other(message) => {
            let status = parse_http_status(message);
            if status.is_some() {
                ("http_error", status, false, true)
            } else {
                ("provider_response_error", None, false, true)
            }
        }
    }
}

pub(crate) fn classify_asr_error(error: &AsrError) -> (&'static str, Option<u16>, bool) {
    match error {
        AsrError::Network(_) => ("network_error", None, true),
        AsrError::Timeout => ("timeout", None, true),
        AsrError::Unauthorized(_) => ("authorization_error", Some(401), true),
        AsrError::RateLimited(_) => ("rate_limited", Some(429), true),
        AsrError::RetryableServer { message, .. } => {
            ("server_error", parse_http_status(message), true)
        }
        AsrError::Server(message) => ("server_error", parse_http_status(message), true),
        AsrError::EmptyResult => ("no_speech", None, false),
        AsrError::ContextAuthorizationChanged => ("context_authorization_changed", None, true),
        AsrError::OnDeviceModelMissing(_) => ("on_device_model_missing", None, true),
        AsrError::OnDeviceInferenceUnavailable(_) => {
            ("on_device_inference_unavailable", None, true)
        }
        AsrError::Other(message) => {
            let status = parse_http_status(message);
            if status.is_some() {
                ("http_error", status, true)
            } else {
                ("provider_response_error", None, true)
            }
        }
    }
}

pub(crate) fn parse_http_status(message: &str) -> Option<u16> {
    let marker = "HTTP status ";
    let start = message.find(marker)? + marker.len();
    let digits: String = message[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.len() == 3 {
        digits.parse().ok()
    } else {
        None
    }
}

pub(crate) fn quality_for_pair(
    reference: &str,
    variants: &[String],
    candidate: &str,
) -> QualityObservation {
    let mut refs = Vec::with_capacity(variants.len() + 1);
    refs.push(reference);
    refs.extend(variants.iter().map(String::as_str));
    let reference_matches = refs.contains(&candidate);
    let (char_edits, ref_characters) = best_normalized_distance(&refs, candidate, false);
    let (word_edits, ref_words) = best_normalized_distance(&refs, candidate, true);
    QualityObservation {
        reference_matches,
        char_edits,
        ref_characters,
        word_edits,
        ref_words,
    }
}

impl QualityAccumulator {
    pub(crate) fn observation(case: &EvalCase, candidate: &str) -> QualityObservation {
        quality_for_pair(&case.expected, &case.reference_variants, candidate)
    }

    pub(crate) fn add(&mut self, observation: QualityObservation) {
        self.sample_count += 1;
        self.reference_matches += usize::from(observation.reference_matches);
        self.character_edits += observation.char_edits;
        self.reference_characters += observation.ref_characters;
        self.word_edits += observation.word_edits;
        self.reference_words += observation.ref_words;
    }

    pub(crate) fn summary(&self) -> QualitySummary {
        QualitySummary {
            sample_count: self.sample_count,
            any_variant_reference_matches: self.reference_matches,
            any_variant_reference_match_rate: (self.sample_count != 0)
                .then(|| self.reference_matches as f64 / self.sample_count as f64),
            character_edits_min_variant: self.character_edits,
            reference_codepoints_min_variant: self.reference_characters,
            character_error_rate_min_variant: (self.sample_count != 0)
                .then(|| self.character_edits as f64 / self.reference_characters.max(1) as f64),
            word_edits_min_variant: self.word_edits,
            reference_words_min_variant: self.reference_words,
            word_error_rate_min_variant: (self.sample_count != 0)
                .then(|| self.word_edits as f64 / self.reference_words.max(1) as f64),
        }
    }
}

pub(crate) struct QualityObservation {
    pub(crate) reference_matches: bool,
    pub(crate) char_edits: usize,
    pub(crate) ref_characters: usize,
    pub(crate) word_edits: usize,
    pub(crate) ref_words: usize,
}

pub(crate) fn any_reference_match(case: &EvalCase, actual: &str) -> bool {
    case.expected == actual || case.reference_variants.iter().any(|item| item == actual)
}

pub(crate) fn best_normalized_distance(
    references: &[&str],
    candidate: &str,
    words: bool,
) -> (usize, usize) {
    if words {
        let candidate_tokens = normalized_words(candidate);
        let mut best: Option<(usize, usize, f64)> = None;
        for reference in references {
            let reference_tokens = normalized_words(reference);
            let edits = levenshtein(&reference_tokens, &candidate_tokens);
            let denominator = reference_tokens.len().max(1);
            let rate = edits as f64 / denominator as f64;
            if best.is_none_or(|(_, _, best_rate)| rate < best_rate) {
                best = Some((edits, reference_tokens.len(), rate));
            }
        }
        best.map(|(edits, count, _)| (edits, count))
            .unwrap_or((0, 0))
    } else {
        let candidate_tokens = normalized_characters(candidate);
        let mut best: Option<(usize, usize, f64)> = None;
        for reference in references {
            let reference_tokens = normalized_characters(reference);
            let edits = levenshtein(&reference_tokens, &candidate_tokens);
            let denominator = reference_tokens.len().max(1);
            let rate = edits as f64 / denominator as f64;
            if best.is_none_or(|(_, _, best_rate)| rate < best_rate) {
                best = Some((edits, reference_tokens.len(), rate));
            }
        }
        best.map(|(edits, count, _)| (edits, count))
            .unwrap_or((0, 0))
    }
}

pub(crate) fn normalized_characters(value: &str) -> Vec<char> {
    value
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub(crate) fn normalized_words(value: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    for ch in value.chars() {
        if is_cjk(ch) {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            words.push(ch.to_lowercase().collect());
        } else if ch.is_alphanumeric() {
            current.extend(ch.to_lowercase());
        } else if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

pub(crate) fn is_cjk(ch: char) -> bool {
    matches!(ch, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}')
}

pub(crate) fn has_cjk(value: &str) -> bool {
    value.chars().any(is_cjk)
}

pub(crate) fn has_latin(value: &str) -> bool {
    value.chars().any(|ch| ch.is_ascii_alphabetic())
}

pub(crate) fn levenshtein<T: Eq>(reference: &[T], candidate: &[T]) -> usize {
    let mut previous: Vec<usize> = (0..=candidate.len()).collect();
    for (row, left) in reference.iter().enumerate() {
        let mut current = vec![row + 1; candidate.len() + 1];
        for (column, right) in candidate.iter().enumerate() {
            current[column + 1] = (previous[column + 1] + 1)
                .min(current[column] + 1)
                .min(previous[column] + usize::from(left != right));
        }
        previous = current;
    }
    previous[candidate.len()]
}

pub(crate) fn timing_summary(mut samples: Vec<f64>) -> TimingSummary {
    samples.retain(|value| value.is_finite() && *value >= 0.0);
    samples.sort_by(f64::total_cmp);
    TimingSummary {
        sample_count: samples.len(),
        p50_ms: super::quantile::linear_quantile(&samples, 0.50),
        p95_ms: super::quantile::linear_quantile(&samples, 0.95),
    }
}

pub(crate) fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1_000.0
}

pub(crate) fn corpus_family(case: &EvalCase) -> ContextFamily {
    crate::context::builtin_family_for_id(&case.family)
        .unwrap_or_else(|| panic!("unknown family {}", case.family))
}

pub(crate) fn corpus_focus_kind(value: &str) -> FocusKind {
    match value {
        "secure" => FocusKind::Secure,
        "search" => FocusKind::Search,
        "code" => FocusKind::Code,
        "coding_prompt" => FocusKind::CodingPrompt,
        "terminal" => FocusKind::Terminal,
        "email" => FocusKind::Email,
        "chat" => FocusKind::Chat,
        "document" => FocusKind::Document,
        "form" => FocusKind::Form,
        "editable" => FocusKind::Editable,
        "unknown" => FocusKind::Unknown,
        other => panic!("unknown focus kind {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_llm_error, timing_summary};

    #[test]
    fn native_local_failure_keeps_typed_reason_without_inventing_http_status() {
        let input_error =
            crate::llm::map_lmstudio_error(crate::lmstudio_local::LocalError::InputTooLarge {
                input_bytes: 4097,
                max_bytes: 4096,
            });
        assert_eq!(
            classify_llm_error(&input_error),
            ("local_native_input_too_large", None, false, true)
        );

        let unreachable =
            crate::llm::map_lmstudio_error(crate::lmstudio_local::LocalError::Unreachable);
        assert_eq!(
            classify_llm_error(&unreachable),
            ("local_native_unreachable", None, false, true)
        );
    }

    #[test]
    fn legacy_timing_summary_uses_shared_interpolation_and_admitted_sample_count() {
        let summary = timing_summary(vec![100.0, 0.0, 20.0, 10.0, -1.0, f64::NAN]);
        assert_eq!(summary.sample_count, 4);
        assert_eq!(summary.p50_ms, Some(15.0));
        assert!((summary.p95_ms.unwrap() - 88.0).abs() < 1e-9);
        let empty = timing_summary(Vec::new());
        assert_eq!(empty.sample_count, 0);
        assert_eq!(empty.p50_ms, None);
        assert_eq!(empty.p95_ms, None);
    }
}
