use super::authorization::{AuthorizationBlocks, CredentialProvider, KeyMaterial};
use super::execution::ExecutionError;
use super::journal::RunJournal;
use super::resume::ReplayDecision;
use super::{contracts::*, identity, legacy, validation::*};
use crate::engine::EngineProvider;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_RECORD_ROWS: u64 = 10_000;
pub const CLOUD_ASR_IDS: &[&str] = &[
    "groq_whisper_large_v3_turbo",
    "groq_whisper_large_v3",
    "openai_gpt_transcribe",
    "qwen3_asr_flash_dashscope",
];
pub const ASR_IDS: &[&str] = &[
    "groq_whisper_large_v3_turbo",
    "groq_whisper_large_v3",
    "openai_gpt_transcribe",
    "qwen3_asr_flash_dashscope",
    "local_qwen3_asr_0_6b",
    "local_qwen3_asr_1_7b",
];
pub const CLEANUP_IDS: &[&str] = &[
    "groq_gpt_oss_120b",
    "groq_gpt_oss_20b",
    "openai_gpt_4o_mini",
    "qwen_plus_dashscope",
    super::local_cleanup::ID,
];
pub fn preflight(
    inputs: &ValidatedInputs,
    path: &Path,
    progress: &mut super::admission_progress::Progress,
) -> Result<Option<BaselineIdentity>, InputError> {
    let Some(recipe) = &inputs.recipe else {
        return Ok(None);
    };
    if recipe.track != Track::FileAdapter || recipe.gui_fixture.is_some() {
        return Err(error(
            path,
            "unsupported_track",
            "GUI/learning execution belongs to the external harness",
        ));
    }
    let expanded = recipe.jobs.iter().try_fold(0u64, |total, job| {
        total.checked_add(inputs.cases.len() as u64 * job.repeats as u64)
    });
    if expanded.is_none_or(|rows| rows > MAX_RECORD_ROWS) {
        return Err(error(
            path,
            "input_budget",
            "total case/job/repeat expansion exceeds 10,000 run records",
        ));
    }
    super::resume::validate_policy(inputs, path)?;
    let current = identity::baseline(Some(progress))?;
    let mut ids = BTreeSet::new();
    for (index, job) in recipe.jobs.iter().enumerate() {
        if job.product != "voiceflow" {
            return Err(error(
                path,
                "identity_mismatch",
                "job product must be VoiceFlow",
            ));
        }
        identity::verify_artifact(&current, &job.artifact_identity, path)?;
        if !ids.insert((&job.candidate_id, &job.profile_state_id)) {
            return Err(error(
                path,
                "duplicate_id",
                "duplicate candidate/profile jobs",
            ));
        }
        if !ASR_IDS.contains(&job.candidate_id.as_str()) {
            return Err(error(
                path,
                "unsupported_candidate",
                "ASR candidate is not yet executable in this batch",
            ));
        }
        let fixtures = inputs
            .control_fixtures
            .get(index)
            .ok_or_else(|| error(path, "identity_mismatch", "job control snapshot missing"))?;
        if super::local_asr::model_id(&job.candidate_id).is_some() {
            super::local_asr::options(job)
                .map_err(|reason| error(path, "unsupported_control", reason))?;
        } else {
            let (_, provider, model, base) = asr_spec(&job.candidate_id);
            let endpoint = crate::providers::resolve_asr_endpoint(
                match provider {
                    CredentialProvider::Groq => EngineProvider::Groq,
                    CredentialProvider::OpenAi => EngineProvider::OpenAi,
                    CredentialProvider::Qwen => EngineProvider::Custom,
                },
                base,
            )
            .ok_or_else(|| {
                error(
                    path,
                    "unsupported_control",
                    "candidate has no file adapter endpoint",
                )
            })?;
            fixtures
                .asr_options(&endpoint, model, &job.recognition_language)
                .map_err(|reason| error(path, "unsupported_control", reason))?;
        }
        if job.decode_settings.temperature.is_some() || job.decode_settings.beam_size.is_some() {
            return Err(error(
                path,
                "unsupported_control",
                "custom decode controls are not connected to this protocol",
            ));
        }
        if job.local_rules.revision != "voiceflow-production-v1" {
            return Err(error(
                path,
                "unsupported_control",
                "local rules revision must identify voiceflow-production-v1",
            ));
        }
        let off = job.cleanup.intensity == CleanupIntensity::Off || job.mode == Mode::Verbatim;
        if off && job.cleanup.capture_provider_candidate {
            return Err(error(
                path,
                "schema",
                "cleanup off/verbatim cannot request provider reply capture",
            ));
        }
        if off && job.cleanup.candidate_id.is_some() {
            return Err(error(
                path,
                "schema",
                "cleanup off/verbatim must omit cleanup candidate",
            ));
        }
        if !off
            && !job
                .cleanup
                .candidate_id
                .as_ref()
                .is_some_and(|v| CLEANUP_IDS.contains(&v.as_str()))
        {
            return Err(error(
                path,
                "unsupported_candidate",
                "select an executable cleanup candidate or cleanup off",
            ));
        }
        if !off && job.run_budget.max_retries != 1 {
            return Err(error(
                path,
                "unsupported_control",
                "shared cleanup adapter currently permits one rate-limit retry; set max_retries:1",
            ));
        }
        let maximum = inputs.cases.len() as u64 * job.repeats as u64 * if off { 1 } else { 3 };
        if maximum > job.run_budget.max_requests as u64 {
            return Err(error(path,"input_budget","max_requests must cover all ASR calls, cleanup calls and one bounded cleanup retry"));
        }
    }
    Ok(Some(current))
}
pub(super) fn key(provider: CredentialProvider) -> Result<KeyMaterial, InputError> {
    provider.resolve()?.require(provider)
}
pub(super) fn legacy_required_keys(asr: &str, cleanup: &str) -> Result<(), InputError> {
    // Reject the research route before any legacy credential resolver is called.
    if cleanup
        .split(',')
        .map(str::trim)
        .any(|id| id == super::local_cleanup::ID)
    {
        return Err(error(
            Path::new("arguments"),
            "unsupported_candidate",
            "local cleanup requires an explicit experiment/manifest/owned workspace",
        ));
    }
    for id in asr.split(',').map(str::trim) {
        key(asr_spec(id).1)?;
    }
    for id in cleanup.split(',').map(str::trim) {
        let provider = cleanup_spec(id).credential().ok_or_else(|| {
            error(
                Path::new("arguments"),
                "unsupported_candidate",
                "local cleanup requires an explicit experiment/manifest/owned workspace",
            )
        })?;
        key(provider)?;
    }
    Ok(())
}
pub(super) fn asr_spec(id: &str) -> (&'static str, CredentialProvider, &'static str, &'static str) {
    match id {
        "groq_whisper_large_v3" => (
            "groq_whisper_large_v3",
            CredentialProvider::Groq,
            "whisper-large-v3",
            EngineProvider::Groq.default_base_url(),
        ),
        "openai_gpt_transcribe" => (
            "openai_gpt_transcribe",
            CredentialProvider::OpenAi,
            "gpt-transcribe",
            EngineProvider::OpenAi.default_base_url(),
        ),
        "qwen3_asr_flash_dashscope" => (
            "qwen3_asr_flash_dashscope",
            CredentialProvider::Qwen,
            "qwen3-asr-flash",
            legacy::QWEN_ASR_BASE,
        ),
        "groq_whisper_large_v3_turbo" => (
            "groq_whisper_large_v3_turbo",
            CredentialProvider::Groq,
            "whisper-large-v3-turbo",
            EngineProvider::Groq.default_base_url(),
        ),
        _ => unreachable!("cloud ASR candidate must be resolved from its explicit allowlist"),
    }
}
pub(super) enum CleanupDescriptor {
    Cloud(
        &'static str,
        CredentialProvider,
        EngineProvider,
        &'static str,
        &'static str,
    ),
    VerifiedLocal,
}
impl CleanupDescriptor {
    fn credential(&self) -> Option<CredentialProvider> {
        match self {
            Self::Cloud(_, key, _, _, _) => Some(*key),
            Self::VerifiedLocal => None,
        }
    }
    pub(super) fn candidate(
        &self,
        material: &KeyMaterial,
        local: Option<&std::sync::Arc<tokio::sync::Mutex<super::local_cleanup::OwnedServerLease>>>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<legacy::CleanupCandidate, InputError> {
        match self {
            Self::Cloud(id, _, provider, model, base) => {
                Ok(legacy::CleanupCandidate::from_material(
                    id,
                    *provider,
                    (*model).into(),
                    (*base).into(),
                    material,
                ))
            }
            Self::VerifiedLocal => Ok(legacy::CleanupCandidate::local(
                local.cloned().ok_or_else(|| {
                    error(
                        Path::new("local cleanup"),
                        "local_cleanup_missing_lease",
                        "explicit local candidate requires its held owned server lease",
                    )
                })?,
                cancel.clone(),
            )),
        }
    }
}
pub(super) fn cleanup_spec(id: &str) -> CleanupDescriptor {
    if id == super::local_cleanup::ID {
        return CleanupDescriptor::VerifiedLocal;
    }
    let (id, key, provider, model, base) = match id {
        "groq_gpt_oss_120b" => (
            "groq_gpt_oss_120b",
            CredentialProvider::Groq,
            EngineProvider::Groq,
            "openai/gpt-oss-120b",
            "",
        ),
        "openai_gpt_4o_mini" => (
            "openai_gpt_4o_mini",
            CredentialProvider::OpenAi,
            EngineProvider::OpenAi,
            "gpt-4o-mini",
            EngineProvider::OpenAi.default_base_url(),
        ),
        "qwen_plus_dashscope" => (
            "qwen_plus_dashscope",
            CredentialProvider::Qwen,
            EngineProvider::Custom,
            "qwen-plus",
            legacy::QWEN_CHAT_BASE,
        ),
        "groq_gpt_oss_20b" => (
            "groq_gpt_oss_20b",
            CredentialProvider::Groq,
            EngineProvider::Groq,
            "openai/gpt-oss-20b",
            "",
        ),
        _ => unreachable!("cleanup candidate must be selected from its allowlist"),
    };
    CleanupDescriptor::Cloud(id, key, provider, model, base)
}

fn unknown(reason: &str) -> ObservedText {
    ObservedText::Unknown {
        reason: reason.into(),
    }
}
fn observed(value: Option<&str>) -> ObservedText {
    match value {
        Some("") => ObservedText::KnownEmpty {},
        Some(v) => ObservedText::KnownValue { value: v.into() },
        None => unknown("not_observed"),
    }
}
pub fn output_path(path: &Path) -> Result<PathBuf, InputError> {
    let parent = external(
        path.parent()
            .filter(|v| !v.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?;
    let name = path
        .file_name()
        .ok_or_else(|| error(path, "path_escape", "output filename required"))?;
    let output = parent.join(name);
    if output.exists() {
        return Err(error(
            path,
            "output_exists",
            "existing output is preserved; choose a new report path",
        ));
    }
    Ok(output)
}
fn postprocessing_seed_reason(
    inputs: &ValidatedInputs,
    case: &super::validation::ValidatedCase,
) -> &'static str {
    if !inputs.selection.asr_only() {
        "not_run"
    } else if case.negative() {
        "negative_postprocessing_not_applicable"
    } else {
        "asr_only_speech_postprocessing_not_applicable"
    }
}
pub(crate) fn build_report(
    inputs: &ValidatedInputs,
    baseline: BaselineIdentity,
) -> Result<RunRecord, InputError> {
    let recipe = inputs.recipe.as_ref().ok_or_else(|| {
        error(
            Path::new("recipe"),
            "schema",
            "recipe execution requires an experiment",
        )
    })?;
    let input_identity = match &inputs.selection {
        super::validation::InputSelection::Speech {
            corpus_sha256,
            negative,
            ..
        } => RunInputIdentity::Speech(RunInputIdentityV1 {
            artifact_type: "run_input_identity".into(),
            schema_version: 1,
            baseline,
            corpus_sha256: corpus_sha256.clone(),
            recipe_sha256: inputs.recipe_sha256.clone(),
            non_speech_sha256: negative.as_ref().map(|n| n.sha256.clone()),
            local_cleanup_manifest_sha256: None,
        }),
        super::validation::InputSelection::AsrSpeech { snapshot, negative } => {
            RunInputIdentity::AsrSpeech(Box::new(RunInputIdentityV3 {
                artifact_type: "run_input_identity".into(),
                schema_version: 3,
                baseline,
                recipe_sha256: inputs.recipe_sha256.clone(),
                input: snapshot.identity(negative.as_ref()),
            }))
        }
        super::validation::InputSelection::NonSpeech(n) => {
            RunInputIdentity::NonSpeech(RunInputIdentityV2 {
                artifact_type: "run_input_identity".into(),
                schema_version: 2,
                baseline,
                input: ExperimentInputV2::NonSpeech {
                    index_sha256: n.sha256.clone(),
                },
                recipe_sha256: inputs.recipe_sha256.clone(),
            })
        }
    };
    use chacha20poly1305::{
        aead::{AeadCore, OsRng},
        XChaCha20Poly1305,
    };
    let invocation = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let run_id = super::resume::hash(&(
        "voiceflow-run-invocation-v1",
        &input_identity,
        hex::encode(invocation),
    ));
    let mut report = RunRecord {
        artifact_type: "run_record".into(),
        schema_version: inputs.selection.schema_version(),
        run_id,
        identity: input_identity,
        terminal: StageState::NotRun {},
        provider_attempts: 0,
        setup: Some(SetupRecord::new()),
        replay: None,
        scoring: Some(super::scoring::summary(inputs)),
        local_cleanup: None,
        cases: vec![],
    };
    for (index, job) in recipe.jobs.iter().enumerate() {
        let controls = inputs.control_fixtures.get(index).ok_or_else(|| {
            error(
                Path::new("recipe"),
                "identity_mismatch",
                "job control snapshot missing",
            )
        })?;
        for repeat in 0..job.repeats {
            for c in &inputs.cases {
                report.cases.push(CaseRecordV1 {
                    score_input: Some(super::scoring::input(
                        c,
                        &inputs.scoring,
                        !inputs.selection.asr_only(),
                    )),
                    scores: None,
                    case_id: c.case.id().to_owned(),
                    job_id: job.id(index),
                    repeat,
                    requested: job.clone(),
                    effective: EffectiveControls {
                        sent_model: None,
                        endpoint: None,
                        recognition_language: None,
                        vocabulary: None,
                        context: None,
                        local_rules_revision: job.local_rules.revision.clone(),
                        server_model: unknown("provider_not_reported"),
                    },
                    control_input_identities: controls.identities(),
                    local_cleanup: None,
                    local_model: None,
                    local_runtime: None,
                    asr_observation: None,
                    cleanup_observation: None,
                    asr_request: None,
                    cleanup_request: None,
                    audio: c.audio.clone(),
                    input_paths: if let super::validation::CaseData::AsrSpeech(source) = &c.case {
                        let super::validation::CaseDocument::AsrSpeech {
                            selected,
                            canonical,
                            sha256,
                            packet_sha256,
                        } = &c.document
                        else {
                            return Err(error(
                                Path::new("inputs"),
                                "identity_mismatch",
                                "ASR-only case document mismatch",
                            ));
                        };
                        CaseInputPaths::AsrSpeech(Box::new(super::speech_input::InputPaths {
                            document_kind: "asr_speech_index".into(),
                            selected_document: selected.display().to_string(),
                            canonical_document: canonical.display().to_string(),
                            index_sha256: sha256.clone(),
                            source_packet_sha256: packet_sha256.clone(),
                            purpose: source.purpose,
                            case: source.case.clone(),
                            source: source.source.clone(),
                            declared_audio: c.case.audio().into(),
                            canonical_audio: c.audio_path.display().to_string(),
                        }))
                    } else if inputs.selection.asr_only() {
                        let super::validation::CaseDocument::NonSpeech {
                            selected,
                            canonical,
                            sha256,
                            provenance,
                        } = &c.document
                        else {
                            return Err(error(
                                Path::new("inputs"),
                                "identity_mismatch",
                                "non-speech selection has a speech source",
                            ));
                        };
                        let super::validation::CaseData::NonSpeech(case) = &c.case else {
                            return Err(error(
                                Path::new("inputs"),
                                "identity_mismatch",
                                "non-speech selection has a speech case",
                            ));
                        };
                        CaseInputPaths::NonSpeech(Box::new(NonSpeechInputPaths {
                            document_kind: "non_speech_index".into(),
                            selected_document: selected.display().to_string(),
                            canonical_document: canonical.display().to_string(),
                            index_sha256: sha256.clone(),
                            provenance: provenance.clone(),
                            case: case.clone(),
                            declared_audio: c.case.audio().into(),
                            canonical_audio: c.audio_path.display().to_string(),
                        }))
                    } else {
                        CaseInputPaths::Speech(InputPaths {
                            manifest: c.document_path().display().to_string(),
                            declared_audio: c.case.audio().into(),
                            canonical_audio: c.audio_path.display().to_string(),
                        })
                    },
                    state: StageState::NotRun {},
                    asr_origin: "unknown".into(),
                    asr_terminal: "not_run".into(),
                    provider_text: unknown("not_run"),
                    filtered_text: unknown("not_run"),
                    prepared_text: unknown(postprocessing_seed_reason(inputs, c)),
                    adapter_candidate: unknown(postprocessing_seed_reason(inputs, c)),
                    final_text: unknown(postprocessing_seed_reason(inputs, c)),
                    finalizer_identity_sha256: None,
                    cleanup_stage: "not_requested".into(),
                    cleanup_adapter: if inputs.selection.asr_only() {
                        StageState::NotApplicable {}
                    } else {
                        StageState::NotRun {}
                    },
                    final_guard: if inputs.selection.asr_only() {
                        StageState::NotApplicable {}
                    } else {
                        StageState::NotRun {}
                    },
                    transformations: vec![],
                    attempts: 0,
                    attempt_facts: vec![],
                    fallback: None,
                    winner: None,
                    timings_ms: BTreeMap::new(),
                    diagnostics: vec![],
                    independent_oracle: StageState::NotApplicable {},
                    coverage: "not_run".into(),
                    cache_source: "fresh".into(),
                    asr_replay: None,
                    cleanup_replay: None,
                    cancellation_requested: false,
                    provider_request_termination_observed: StageState::NotApplicable {},
                });
            }
        }
    }
    Ok(report)
}
pub async fn run(
    inputs: &ValidatedInputs,
    baseline: BaselineIdentity,
    out: &Path,
    local: Option<super::local_asr::Plan>,
    admission: super::resume::Admission,
    cleanup_plan: Option<super::local_cleanup::Plan>,
) -> Result<RunRecord, ExecutionError> {
    inputs.recipe.as_ref().ok_or_else(|| {
        ExecutionError::input(
            error(out, "schema", "recipe execution requires an experiment"),
            0,
        )
    })?;
    if identity::executable_sha256().map_err(|e| ExecutionError::input(e, 0))?
        != baseline.executable_sha256
    {
        return Err(ExecutionError::input(
            error(
                out,
                "executable_identity_changed",
                "evaluator executable path content changed after admission",
            ),
            0,
        ));
    }
    let mut report = build_report(inputs, baseline).map_err(|e| ExecutionError::input(e, 0))?;
    report
        .identity
        .set_cleanup_manifest(cleanup_plan.as_ref().map(|p| p.manifest_sha256.clone()));
    let replay = admission
        .bind(inputs, &report.identity)
        .map_err(|e| ExecutionError::input(e, 0))?;
    report.replay = replay.summary();
    if replay
        .policy
        .as_ref()
        .is_some_and(|p| p.purpose == CachePurpose::QualityReplay)
    {
        for row in &mut report.cases {
            if super::local_asr::model_id(&row.requested.candidate_id).is_some()
                && !matches!(replay.identity, RunInputIdentity::AsrSpeech(_))
            {
                row.diagnostics
                    .push("local_cache_identity_incomplete_fresh_only".into());
            }
            if row.requested.cleanup.candidate_id.as_deref() == Some(super::local_cleanup::ID) {
                row.diagnostics
                    .push("local_cleanup_runtime_identity_incomplete_fresh_only".into());
            }
        }
    }
    let cleanup_fixture_sha = sha256(
        &serde_json::to_vec(&(&report.run_id, &report.identity))
            .map_err(|e| ExecutionError::input(error(out, "schema", e.to_string()), 0))?,
    );
    // A complete initial journal and cancellation owner precede setup side effects/keys.
    let journal = RunJournal::new_with_replay(
        out,
        report,
        Some(replay.clone()).filter(|p| {
            p.policy
                .as_ref()
                .is_some_and(|p| p.purpose == CachePurpose::QualityReplay)
        }),
    )?;
    let interrupt = tokio_util::sync::CancellationToken::new();
    let signal = interrupt.clone();
    let signal_task = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal.cancel();
        }
    });
    struct AbortSignal(tokio::task::JoinHandle<()>);
    impl Drop for AbortSignal {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _signal_owner = AbortSignal(signal_task);
    let (workspace, keys) = match setup_execution(local, &journal, &interrupt, || {
        resolve_keys(inputs, &interrupt, &replay, &journal)
    })
    .await
    {
        Ok(value) => value,
        Err(error) => {
            let error = journal.fail(error, None);
            if matches!(error.terminal, StageState::Cancelled { .. })
                && matches!(error.persistence, StageState::Completed {})
            {
                return journal.into_report();
            }
            return Err(error);
        }
    };
    let cleanup_server = if let Some(plan) = cleanup_plan {
        if let Err(error) = journal.persist() {
            if let Some(workspace) = &workspace {
                workspace.shutdown().await;
            }
            return Err(journal.fail(error, None));
        }
        if interrupt.is_cancelled() {
            if let Some(workspace) = &workspace {
                workspace.shutdown().await;
            }
            return Err(journal.fail(super::local_asr::setup_cancelled(&journal, out), None));
        }
        let mut owner = match plan.spawn(&cleanup_fixture_sha) {
            Ok(owner) => owner,
            Err(cause) => {
                if let Some(workspace) = &workspace {
                    workspace.shutdown().await;
                }
                return Err(journal.fail(journal.input_error(cause), None));
            }
        };
        journal.local_cleanup(owner.identity());
        let readiness = owner.ready(&interrupt).await;
        journal.local_cleanup(owner.identity());
        if let Err(cause) = readiness {
            let _ = owner.shutdown().await;
            journal.local_cleanup(owner.identity());
            if let Some(workspace) = &workspace {
                workspace.shutdown().await;
            }
            let failure = if interrupt.is_cancelled() {
                super::local_asr::setup_cancelled(&journal, out)
            } else {
                journal.input_error(cause)
            };
            return Err(journal.fail(failure, None));
        }
        Some(std::sync::Arc::new(tokio::sync::Mutex::new(owner)))
    } else {
        None
    };
    let mut cursor = 0usize;
    let mut pending_capture = None;
    let result = execute(
        inputs,
        &keys,
        &journal,
        &interrupt,
        &mut cursor,
        &replay,
        ExecutionResources {
            local_asr: workspace.as_ref(),
            local_cleanup: cleanup_server.as_ref(),
            pending_capture: &mut pending_capture,
        },
    )
    .await;
    let cleanup_failure = if let Some(server) = &cleanup_server {
        let mut owner = server.lock().await;
        // Outer cancellation/deadline may drop the adapter future before its
        // normal publication point. Preserve the actual owner's partial facts.
        if cursor < journal.case_count() {
            let row = journal.row(cursor);
            if row.requested.cleanup.candidate_id.as_deref() == Some(super::local_cleanup::ID)
                && row
                    .attempt_facts
                    .iter()
                    .any(|a| a.stage == AttemptStage::Cleanup)
            {
                journal.update(cursor, |row| row.local_cleanup = Some(owner.identity()));
            }
        }
        let cleanup = owner.shutdown().await;
        journal.local_cleanup(owner.identity());
        cleanup.err()
    } else {
        None
    };
    let shutdown_result = shutdown_then_publish(pending_capture.take(), async {
        if let Some(workspace) = &workspace {
            workspace.shutdown_with_status().await
        } else {
            Ok(())
        }
    })
    .await;
    if let Err(reason) = shutdown_result {
        let error = journal.input_error(error(
            out,
            "local_shutdown_unsettled",
            format!("local runtime shutdown did not settle: {reason}"),
        ));
        return Err(journal.fail(error, None));
    }
    if let Err(error) = result {
        return Err(journal.fail(error, (cursor < journal.case_count()).then_some(cursor)));
    }
    if let Some(cause) = cleanup_failure {
        return Err(journal.fail(
            journal.input_error(cause),
            (cursor < journal.case_count()).then_some(cursor),
        ));
    }
    let end = identity::baseline(None).map_err(|e| journal.fail(journal.input_error(e), None))?;
    if let Err(cause) = journal.finish_identity(end, out) {
        return Err(journal.fail(journal.input_error(cause), None));
    }
    journal.persist().map_err(|e| journal.fail(e, None))?;
    journal.into_report()
}
pub(super) fn resolve_keys(
    inputs: &ValidatedInputs,
    cancel: &tokio_util::sync::CancellationToken,
    replay: &super::resume::ReplayPlan,
    journal: &RunJournal,
) -> Result<BTreeMap<CredentialProvider, KeyMaterial>, InputError> {
    let recipe = inputs.recipe.as_ref().expect("validated experiment");
    replay.admit_boundary()?;
    let mut keys = BTreeMap::new();
    for (index, job) in recipe.jobs.iter().enumerate() {
        replay.admit_boundary()?;
        if cancel.is_cancelled() {
            return Err(error(
                Path::new("setup"),
                "setup_cancelled",
                "cancelled before credential access",
            ));
        }
        if super::local_asr::model_id(&job.candidate_id).is_none()
            && job_needs_fresh_asr(inputs, index, replay, journal)?
        {
            let provider = asr_spec(&job.candidate_id).1;
            replay.admit_boundary()?;
            keys.insert(provider, key(provider)?);
        }
        if let Some(id) = &job.cleanup.candidate_id {
            if job_needs_fresh_cleanup(inputs, index, replay, journal)? {
                if cancel.is_cancelled() {
                    return Err(error(
                        Path::new("setup"),
                        "setup_cancelled",
                        "cancelled before cleanup credential access",
                    ));
                }
                if let Some(provider) = cleanup_spec(id).credential() {
                    replay.admit_boundary()?;
                    keys.insert(provider, key(provider)?);
                }
            }
        }
    }
    Ok(keys)
}
fn job_rows(inputs: &ValidatedInputs, index: usize) -> std::ops::Range<usize> {
    let recipe = inputs.recipe.as_ref().unwrap();
    let first: usize = recipe.jobs[..index]
        .iter()
        .map(|j| j.repeats as usize * inputs.cases.len())
        .sum();
    first..first + recipe.jobs[index].repeats as usize * inputs.cases.len()
}
pub(super) fn job_needs_fresh_asr(
    inputs: &ValidatedInputs,
    index: usize,
    replay: &super::resume::ReplayPlan,
    journal: &RunJournal,
) -> Result<bool, InputError> {
    for index in job_rows(inputs, index) {
        let row = journal.row(index);
        if matches!(
            replay.decide(inputs, &row, CacheStage::Asr, None)?,
            ReplayDecision::Miss
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}
pub(super) fn job_needs_fresh_cleanup(
    inputs: &ValidatedInputs,
    index: usize,
    replay: &super::resume::ReplayPlan,
    journal: &RunJournal,
) -> Result<bool, InputError> {
    for row_index in job_rows(inputs, index) {
        let mut row = journal.row(row_index);
        let case = inputs
            .cases
            .iter()
            .find(|c| c.case.id() == row.case_id)
            .unwrap();
        if !case.case.cleanup_applicable() {
            continue;
        }
        let asr = replay
            .decide(inputs, &row, CacheStage::Asr, None)?
            .candidate();
        let e = eval_case(
            case.case.speech().expect("speech-only cleanup"),
            asr.as_ref().map_or("fixture probe", |a| a.value()),
            &row.requested,
        );
        let fixture =
            inputs.control_fixtures[index].preparation(&row.requested.local_rules, &row.case_id);
        let prepared = legacy::prepare_cleanup_case_with_fixture(&e, Some(&fixture));
        if matches!(prepared.route, crate::lexicon::CleanupRoute::LocalOnly) {
            continue;
        }
        let Some(asr) = asr else {
            return Ok(true);
        };
        row.filtered_text = ObservedText::KnownValue {
            value: asr.value().into(),
        };
        if matches!(
            replay.decide(inputs, &row, CacheStage::Cleanup, Some(prepared.source()))?,
            ReplayDecision::Miss
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}
/// Ordinary setup returns only after joining its worker. Dropping the outer
/// future cancels the worker cooperatively and makes no terminal/join promise.
pub(super) async fn setup_execution<T>(
    local: Option<super::local_asr::Plan>,
    journal: &std::sync::Arc<RunJournal>,
    cancel: &tokio_util::sync::CancellationToken,
    resolve: impl FnOnce() -> Result<T, InputError>,
) -> Result<(Option<super::local_asr::Workspace>, T), ExecutionError> {
    struct CancelOnDrop(tokio_util::sync::CancellationToken);
    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            self.0.cancel();
        }
    }
    // A child prevents normal successful setup teardown from cancelling the run token.
    let worker_cancel = cancel.child_token();
    let _worker_owner = CancelOnDrop(worker_cancel.clone());
    let workspace = if let Some(plan) = local {
        journal.setup(|s| {
            s.materialization = StageState::Unknown {
                reason: "materialization_in_progress".into(),
            };
            s.worker_join = StageState::Unknown {
                reason: "worker_not_joined".into(),
            };
        });
        journal.persist()?;
        let worker_journal = journal.clone();
        let worker_token = worker_cancel.clone();
        let result =
            tokio::task::spawn_blocking(move || plan.materialize(&worker_token, &worker_journal))
                .await;
        let result = result.map_err(|_| {
            journal.input_error(error(
                Path::new("setup"),
                "local_setup_worker_failed",
                "materialization worker terminated; partial workspace retained",
            ))
        });
        // join has completed, including failure/panic: no copy can continue after this fact.
        journal.setup(|s| s.worker_join = StageState::Completed {});
        let result = result.and_then(|result| result);
        match result {
            Ok(workspace) => {
                journal.setup(|s| s.materialization = StageState::Completed {});
                journal.attach_workspace(&workspace);
                journal.persist()?;
                Some(workspace)
            }
            Err(error) => {
                journal.setup(|s| {
                    s.materialization = error.terminal.clone();
                    s.cancellation_requested = worker_cancel.is_cancelled();
                    for copy in &mut s.copies {
                        if matches!(copy.state, StageState::Unknown { .. }) {
                            copy.state = error.terminal.clone();
                        }
                    }
                });
                return Err(error);
            }
        }
    } else {
        journal.setup(|s| {
            s.materialization = StageState::NotApplicable {};
            s.worker_join = StageState::NotApplicable {};
        });
        None
    };
    if cancel.is_cancelled() {
        journal.setup(|s| {
            s.cancellation_requested = true;
            s.credentials = StageState::Cancelled {
                reason: "setup_cancelled".into(),
            };
        });
        return Err(super::local_asr::setup_cancelled(
            journal,
            Path::new("setup"),
        ));
    }
    journal.setup(|s| {
        s.credentials = StageState::Unknown {
            reason: "credential_resolution_in_progress".into(),
        }
    });
    journal.persist()?;
    let keys = resolve().map_err(|cause| {
        let error = if cause.code == "setup_cancelled" {
            super::local_asr::setup_cancelled(journal, Path::new("setup"))
        } else {
            journal.input_error(cause)
        };
        journal.setup(|s| {
            s.credentials = error.terminal.clone();
            s.cancellation_requested = cancel.is_cancelled();
        });
        error
    })?;
    if cancel.is_cancelled() {
        journal.setup(|s| {
            s.cancellation_requested = true;
            s.credentials = StageState::Cancelled {
                reason: "setup_cancelled".into(),
            };
        });
        return Err(super::local_asr::setup_cancelled(
            journal,
            Path::new("setup"),
        ));
    }
    journal.setup(|s| s.credentials = StageState::Completed {});
    journal.persist()?;
    Ok((workspace, keys))
}

/// Borrow the admitted per-run capabilities together. The run keeps the real
/// workspace/child owners and remains responsible for cancellation and shutdown.
pub(super) struct ExecutionResources<'a> {
    pub local_asr: Option<&'a super::local_asr::Workspace>,
    pub local_cleanup:
        Option<&'a std::sync::Arc<tokio::sync::Mutex<super::local_cleanup::OwnedServerLease>>>,
    pub pending_capture: &'a mut Option<CaptureJournal>,
}

pub(super) async fn execute(
    inputs: &ValidatedInputs,
    keys: &BTreeMap<CredentialProvider, KeyMaterial>,
    journal: &std::sync::Arc<RunJournal>,
    interrupt: &tokio_util::sync::CancellationToken,
    cursor: &mut usize,
    replay: &super::resume::ReplayPlan,
    resources: ExecutionResources<'_>,
) -> Result<(), ExecutionError> {
    let recipe = inputs.recipe.as_ref().unwrap();
    let mut blocked = AuthorizationBlocks::default();
    let mut last_start = None;
    let mut cancel_reason = None;
    'jobs: for (job_index, job) in recipe.jobs.iter().enumerate() {
        let controls = inputs.control_fixtures.get(job_index).ok_or_else(|| {
            journal.input_error(error(
                Path::new("recipe"),
                "identity_mismatch",
                "job control snapshot missing",
            ))
        })?;
        let deadline = Instant::now() + Duration::from_millis(job.run_budget.deadline_ms);
        // Replay of local observations requires a current owned load, not a
        // nominal manifest label. This is charged setup work, never an inference.
        if replay.requires_local_attestation() {
            let workspace = resources.local_asr.ok_or_else(|| {
                journal.input_error(error(
                    Path::new("runtime"),
                    "local_resume_identity_incomplete",
                    "materialized local workspace required",
                ))
            })?;
            let model = super::local_asr::model_id(&job.candidate_id)
                .expect("V3 replay validates local jobs");
            let capture = crate::observations::FixtureCapture::new(false);
            let started = Instant::now();
            let preparation = tokio::select! {
                _ = interrupt.cancelled() => Err("sigint"),
                result = tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), capture.scope(workspace.attest(model))) => result.map_err(|_| "job_deadline"),
            };
            let facts = capture.snapshot();
            let model_identity = workspace.observed_identity(model);
            for (offset, index) in job_rows(inputs, job_index).enumerate() {
                journal.update(index, |row| {
                    row.local_model = model_identity.clone();
                    row.local_runtime = facts.local_asr.clone();
                    if let Some(model) = &mut row.local_model {
                        model.observe_loaded(row.local_runtime.as_ref());
                    }
                    if offset == 0 {
                        row.timings_ms.insert(
                            "local_replay_attestation".into(),
                            started.elapsed().as_secs_f64() * 1000.0,
                        );
                    }
                    row.diagnostics
                        .push("current_job_load_attestation_not_an_inference".into());
                });
            }
            journal.persist()?;
            match preparation {
                Err(reason) => {
                    cancel_reason = Some(reason);
                    break 'jobs;
                }
                Ok(Err(reason)) => {
                    return Err(journal.input_error(error(
                        Path::new("runtime"),
                        "local_resume_attestation_failed",
                        reason,
                    )))
                }
                Ok(Ok(())) => {}
            }
            replay
                .admit_boundary()
                .map_err(|e| journal.input_error(e))?;
            for index in job_rows(inputs, job_index) {
                let row = journal.row(index);
                replay
                    .validate_local_binding(&row)
                    .map_err(|e| journal.input_error(e))?;
            }
        }
        let candidate = if let Some(model) = super::local_asr::model_id(&job.candidate_id) {
            let workspace = resources.local_asr.ok_or_else(|| {
                journal.input_error(error(
                    Path::new("arguments"),
                    "missing_models_work_root",
                    "local execution requires its materialized workspace",
                ))
            })?;
            workspace.check().map_err(|e| journal.input_error(e))?;
            let id = super::local_asr::IDS
                .iter()
                .copied()
                .find(|id| *id == job.candidate_id)
                .unwrap();
            let options = super::local_asr::options(job).map_err(|reason| {
                journal.input_error(error(Path::new("recipe"), "unsupported_control", reason))
            })?;
            legacy::AsrCandidate::local(id, model, workspace.root.clone(), options)
        } else {
            let (id, provider, model, base) = asr_spec(&job.candidate_id);
            let mut candidate = legacy::asr_candidate(
                id,
                provider.as_str(),
                model.into(),
                base,
                keys.get(&provider)
                    .cloned()
                    .unwrap_or_else(|| KeyMaterial::unavailable("not_required_replay")),
            );
            let legacy::AsrExecution::Cloud { endpoint, .. } = &candidate.execution else {
                unreachable!()
            };
            let options = controls
                .asr_options(endpoint, &candidate.model, &job.recognition_language)
                .map_err(|reason| {
                    journal.input_error(error(Path::new("recipe"), "unsupported_control", reason))
                })?;
            candidate.language = options.language;
            candidate.prompt = options.prompt;
            candidate.keywords = options.keywords;
            candidate
        };
        for _ in 0..job.repeats {
            for c in &inputs.cases {
                if interrupt.is_cancelled() {
                    cancel_reason = Some("sigint");
                    break 'jobs;
                }
                if Instant::now() >= deadline {
                    cancel_reason = Some("job_deadline");
                    break 'jobs;
                }
                replay
                    .admit_boundary()
                    .map_err(|e| journal.input_error(e))?;
                let bytes = read_bounded(&c.audio_path, MAX_WAV_BYTES)
                    .map_err(|e| journal.input_error(e))?;
                if sha256(&bytes) != c.audio.audio_sha256 {
                    return Err(journal.input_error(error(
                        &c.audio_path,
                        "sha_mismatch",
                        "audio changed after input validation",
                    )));
                }
                let fixtures = [legacy::AsrAudioInput {
                    id: c.case.id(),
                    audio: c.case.audio(),
                    reference: c.case.reference().or_else(|| {
                        if !inputs.selection.asr_only() && c.negative() {
                            Some("")
                        } else {
                            None
                        }
                    }),
                    validated_audio: Some(&c.bytes),
                }];
                if let Some(start) = last_start {
                    let wait = Duration::from_millis(job.pacing.gap_ms)
                        .saturating_sub(Instant::now().duration_since(start));
                    tokio::select! {_ = interrupt.cancelled()=>{cancel_reason=Some("sigint");break 'jobs},_=tokio::time::sleep(wait)=>()}
                }
                last_start = Some(Instant::now());
                if let Some(workspace) = resources.local_asr {
                    workspace.check().map_err(|e| journal.input_error(e))?;
                }
                let observer = journal.observer(*cursor);
                let capture =
                    CaptureJournal::new(journal, *cursor, job, replay.requires_local_attestation());
                let asr_replay = replay
                    .decide(inputs, &journal.row(*cursor), CacheStage::Asr, None)
                    .map_err(|cause| journal.input_error(cause))?
                    .candidate();
                let future = legacy::run_asr_audio_inputs(
                    &candidate,
                    c.document_path(),
                    &fixtures,
                    blocked.reason(candidate.authorization_scope.as_ref()),
                    Some(&observer),
                    asr_replay.as_ref(),
                );
                let (response, capture) =
                    match select_capture(capture, interrupt, deadline, future).await {
                        CaptureSelection::Completed(Ok(response), capture) => {
                            capture.publish(true);
                            (response, capture)
                        }
                        CaptureSelection::Completed(Err(error), capture) => {
                            drop(capture);
                            return Err(error);
                        }
                        CaptureSelection::Cancelled(reason, capture) => {
                            cancel_reason = Some(reason);
                            defer_cancelled_capture(resources.pending_capture, capture);
                            break 'jobs;
                        }
                    };
                if interrupt.is_cancelled() {
                    cancel_reason = Some("sigint");
                    break 'jobs;
                }
                if Instant::now() >= deadline {
                    cancel_reason = Some("job_deadline");
                    break 'jobs;
                }
                if let Some(cached) = &asr_replay {
                    journal.update(*cursor, |row| {
                        row.filtered_text = ObservedText::known(cached.value());
                        row.provider_text = unknown("historical_provider_stage_not_current");
                        row.asr_origin = "replayed_filtered_candidate".into();
                        row.asr_terminal = if cached.known_empty() {
                            "replayed_complete_empty"
                        } else {
                            "speech_text"
                        }
                        .into();
                        row.effective.sent_model = Some(cached.request().constructed_model);
                        row.effective.recognition_language = Some(job.recognition_language.clone());
                        row.effective.vocabulary = Some("none".into());
                        row.effective.context = Some("disabled".into());
                        let mut source = cached.replay();
                        source.current_guard = StageState::NotApplicable {};
                        row.asr_replay = Some(source);
                        row.cache_source = "resumed_asr".into();
                    });
                    journal.replayed(CacheStage::Asr);
                }
                if let Some(workspace) = resources.local_asr {
                    workspace.check().map_err(|e| journal.input_error(e))?;
                    if let Some(model) = super::local_asr::model_id(&job.candidate_id) {
                        journal.update(*cursor, |row| {
                            row.local_model = workspace.observed_identity(model);
                            if let Some(model) = &mut row.local_model {
                                model.observe_loaded(row.local_runtime.as_ref());
                            }
                        });
                    }
                }
                blocked.observe(
                    candidate.authorization_scope.as_ref(),
                    response.stop_category.as_deref(),
                );
                let result = &response.cases[0];
                journal.update(*cursor, |row| {
                    if let Some(ms) = result.latency_ms {
                        row.timings_ms.insert("asr".into(), ms);
                    }
                    if result.transcription_state == "empty_result"
                        || row
                            .asr_observation
                            .as_ref()
                            .is_some_and(crate::observations::AsrObservation::confirmed_empty)
                    {
                        let historical = asr_replay.as_ref().and_then(|r| r.observation());
                        let (state, diagnostic) = empty_result_outcome(
                            c.negative(),
                            row.asr_observation.as_ref().or(historical.as_ref()),
                        );
                        row.diagnostics.extend(result.error_category.clone());
                        row.state = state;
                        row.diagnostics.push(diagnostic.into());
                    } else if result.transcription_state != "transcribed" {
                        row.state = if result.provider_invoked {
                            StageState::Failed {
                                reason: result
                                    .error_category
                                    .clone()
                                    .unwrap_or("provider_failed".into()),
                            }
                        } else {
                            StageState::NotRun {}
                        };
                        row.asr_terminal = if result.provider_invoked {
                            "provider_failed"
                        } else {
                            "not_run"
                        }
                        .into();
                        row.diagnostics.extend(result.error_category.clone());
                    } else {
                        if row.asr_observation.is_none() {
                            row.filtered_text = observed(result.sanitized_text.as_deref());
                        }
                        row.asr_terminal = "speech_text".into();
                        if c.case.asr_speech().is_some() {
                            row.state = StageState::Completed {};
                        }
                        if c.negative() {
                            row.state = StageState::Failed {
                                reason: "negative_false_output".into(),
                            };
                            if !inputs.selection.asr_only() {
                                // Preserve V1 mixed-negative compatibility only.
                                row.final_text = row.filtered_text.clone();
                            }
                        }
                    }
                });
                journal.capture_candidate(inputs, *cursor, CacheStage::Asr, None);
                journal.persist()?;
                if result.transcription_state == "transcribed" && c.case.cleanup_applicable() {
                    let e = eval_case(
                        c.case.speech().expect("speech-only cleanup"),
                        result.sanitized_text.as_deref().unwrap_or(""),
                        job,
                    );
                    let preparation = controls.preparation(&job.local_rules, c.case.id());
                    let prepared =
                        legacy::prepare_cleanup_case_with_fixture(&e, Some(&preparation));
                    let finalizer_identity = super::resume::hash(&(
                        "voiceflow-current-finalizer-v1",
                        journal.code_identity(),
                        job,
                        prepared.source(),
                    ));
                    journal.update(*cursor, |row| {
                        row.prepared_text = observed(Some(prepared.source()));
                        row.finalizer_identity_sha256 = Some(finalizer_identity);
                    });
                    let cleanup = if let Some(id) = &job.cleanup.candidate_id {
                        let descriptor = cleanup_spec(id);
                        let material = descriptor
                            .credential()
                            .and_then(|p| keys.get(&p))
                            .cloned()
                            .unwrap_or_else(|| {
                                KeyMaterial::unavailable("not_applicable_local_or_replay")
                            });
                        descriptor
                            .candidate(&material, resources.local_cleanup, interrupt)
                            .map_err(|e| journal.input_error(e))?
                    } else {
                        legacy::CleanupCandidate::from_material(
                            "cleanup_off",
                            EngineProvider::Custom,
                            "off".into(),
                            "".into(),
                            &KeyMaterial::unavailable("not_required"),
                        )
                    };
                    replay
                        .admit_boundary()
                        .map_err(|e| journal.input_error(e))?;
                    let cleanup_replay = replay
                        .decide(
                            inputs,
                            &journal.row(*cursor),
                            CacheStage::Cleanup,
                            Some(prepared.source()),
                        )
                        .map_err(|cause| journal.input_error(cause))?
                        .candidate();
                    // This state only serves the legacy consumer interface; SHA
                    // applicability comes exclusively from validated replay.
                    let mut resume = legacy::ResumeState::default();
                    let mut pacer =
                        legacy::RequestPacer::new(Duration::from_millis(job.pacing.gap_ms));
                    let code_identity = journal.code_identity();
                    let future = legacy::run_cleanup_candidate(
                        &cleanup,
                        legacy::CleanupCandidateRun {
                            cases: std::slice::from_ref(&e),
                            blocked_reason: blocked.reason(cleanup.authorization_scope.as_ref()),
                            resume_state: &mut resume,
                            checkpoint_path: None,
                            corpus_fingerprint: inputs
                                .selection
                                .corpus_sha()
                                .expect("speech-only cleanup"),
                            cleanup_code_fingerprint: &code_identity,
                            pacer: &mut pacer,
                            observer: Some(&observer),
                            preparation: Some(&preparation),
                            replay: cleanup_replay.as_ref(),
                        },
                    );
                    let cleaned = tokio::select! {_ = interrupt.cancelled()=>{cancel_reason=Some("sigint");break 'jobs},r=tokio::time::timeout_at(tokio::time::Instant::from_std(deadline),capture.capture.scope(future))=>match r{Ok(v)=>v?,Err(_)=>{cancel_reason=Some("job_deadline");break 'jobs}}};
                    if job.cleanup.candidate_id.as_deref() == Some(super::local_cleanup::ID) {
                        if let Some(server) = resources.local_cleanup {
                            let facts = server.lock().await.identity();
                            journal.local_cleanup(facts.clone());
                            journal.update(*cursor, |row| row.local_cleanup = Some(facts));
                        }
                    }
                    capture.publish(true);
                    if interrupt.is_cancelled() {
                        cancel_reason = Some("sigint");
                        break 'jobs;
                    }
                    if Instant::now() >= deadline {
                        cancel_reason = Some("job_deadline");
                        break 'jobs;
                    }
                    blocked.observe(
                        cleanup.authorization_scope.as_ref(),
                        cleaned.stop_category.as_deref(),
                    );
                    let cleaned = &cleaned.cases[0];
                    journal.update(*cursor, |row| {
                        row.adapter_candidate = observed(cleaned.adapter_candidate.as_deref());
                        row.final_text = observed(Some(&cleaned.final_text));
                        row.cleanup_stage = if e.mode == "ai-off" {
                            "off_prepared"
                        } else if cleaned.adapter_candidate.is_some() {
                            "adapter_accepted"
                        } else {
                            "local_fallback"
                        }
                        .into();
                        row.cleanup_adapter =
                            if cleaned.provider_call_state == "not_called_after_provider_error" {
                                StageState::NotRun {}
                            } else if cleaned.resumed_candidate {
                                if cleaned.adapter_candidate.is_some() {
                                    StageState::Completed {}
                                } else {
                                    StageState::Failed {
                                        reason: cleaned
                                            .error_category
                                            .clone()
                                            .unwrap_or("replay_guard_rejected".into()),
                                    }
                                }
                            } else if !cleaned.provider_invoked {
                                StageState::NotApplicable {}
                            } else if cleaned.adapter_candidate.is_some() {
                                StageState::Completed {}
                            } else {
                                StageState::Failed {
                                    reason: cleaned
                                        .error_category
                                        .clone()
                                        .unwrap_or("adapter_rejected".into()),
                                }
                            };
                        row.final_guard = if cleaned.app_final_guard_rejected {
                            StageState::Failed {
                                reason: "final_guard_rejected".into(),
                            }
                        } else {
                            StageState::Completed {}
                        };
                        if cleaned.app_final_guard_rejected {
                            row.cleanup_stage = "final_guard_rejected".into();
                        } else if cleaned.adapter_candidate.is_some() {
                            row.cleanup_stage = "final_guard_accepted".into();
                        }
                        row.fallback = cleaned.error_category.clone();
                        row.diagnostics.extend(cleaned.error_category.clone());
                        row.state =
                            if cleaned.provider_call_state == "not_called_after_provider_error" {
                                StageState::NotRun {}
                            } else if let Some(reason) = &cleaned.error_category {
                                StageState::Failed {
                                    reason: reason.clone(),
                                }
                            } else {
                                StageState::Completed {}
                            };
                        row.timings_ms
                            .insert("preparation".into(), cleaned.preparation_ms);
                        row.timings_ms
                            .insert("finalize".into(), cleaned.finalize_ms);
                        row.timings_ms
                            .insert("cleanup_provider_wait".into(), cleaned.provider_wait_ms);
                        row.timings_ms
                            .insert("cleanup_retry_wait".into(), cleaned.retry_wait_ms);
                        if let Some(ms) = cleaned.provider_ms {
                            row.timings_ms.insert("cleanup_provider".into(), ms);
                        }
                        row.timings_ms.insert(
                            "cleanup_total".into(),
                            cleaned.total_ms + cleaned.provider_wait_ms,
                        );
                        row.transformations.push(Transformation {
                            owner: "voiceflow preparation/finalizer".into(),
                            revision: job.local_rules.revision.clone(),
                            reason: "shared_production_rules_and_guards".into(),
                            source_stage: "filtered_asr".into(),
                        });
                    });
                    if let Some(cached) = &cleanup_replay {
                        journal.update(*cursor, |row| {
                            let mut source = cached.replay();
                            source.current_guard =
                                if matches!(row.final_guard, StageState::Failed { .. }) {
                                    row.final_guard.clone()
                                } else {
                                    row.cleanup_adapter.clone()
                                };
                            row.cleanup_replay = Some(source);
                            row.cache_source = if row.asr_replay.is_some() {
                                "resumed_asr_and_cleanup"
                            } else {
                                "resumed_cleanup"
                            }
                            .into();
                        });
                        journal.replayed(CacheStage::Cleanup);
                    }
                    journal.capture_candidate(
                        inputs,
                        *cursor,
                        CacheStage::Cleanup,
                        Some(prepared.source()),
                    );
                }
                journal.update(*cursor, |row| {
                    row.coverage = "file_adapter_only_no_capture_or_delivery".into();
                    if result.transcription_state == "transcribed" {
                        row.winner = Some(candidate.id.into());
                    }
                });
                *cursor += 1;
                journal.persist()?;
            }
        }
    }
    if let Some(reason) = cancel_reason {
        let len = journal.case_count();
        for index in *cursor..len {
            journal.update(index, |row| {
                mark_cancelled(row, reason);
            });
        }
        journal.terminal(StageState::Cancelled {
            reason: reason.into(),
        });
    } else {
        journal.terminal(StageState::Completed {});
    }
    Ok(())
}
pub(crate) struct CaptureJournal {
    capture: crate::observations::FixtureCapture,
    journal: std::sync::Arc<RunJournal>,
    index: usize,
    requested: JobV1,
    preparation: Option<crate::observations::LocalAsrObservation>,
    include_attestation: bool,
}

pub(crate) enum CaptureSelection<T, E> {
    Completed(Result<T, E>, CaptureJournal),
    Cancelled(&'static str, CaptureJournal),
}

pub(crate) fn defer_cancelled_capture(
    pending: &mut Option<CaptureJournal>,
    capture: CaptureJournal,
) {
    debug_assert!(
        pending.is_none(),
        "serial execution can defer only one capture"
    );
    *pending = Some(capture);
}

/// Share the real cancellation/timeout boundary with the evaluator regression:
/// the scoped adapter future is dropped before its capture can be transferred.
pub(crate) async fn select_capture<F, T, E>(
    capture: CaptureJournal,
    interrupt: &tokio_util::sync::CancellationToken,
    deadline: Instant,
    future: F,
) -> CaptureSelection<T, E>
where
    F: Future<Output = Result<T, E>>,
{
    tokio::select! {
        _ = interrupt.cancelled() => CaptureSelection::Cancelled("sigint", capture),
        result = tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            capture.capture.scope(future),
        ) => match result {
            Ok(result) => CaptureSelection::Completed(result, capture),
            Err(_) => CaptureSelection::Cancelled("job_deadline", capture),
        },
    }
}

pub(crate) async fn shutdown_then_publish<F: Future<Output = Result<(), E>>, E>(
    capture: Option<CaptureJournal>,
    shutdown: F,
) -> Result<(), E> {
    let result = shutdown.await;
    if let Some(capture) = capture {
        capture.publish(result.is_ok());
    }
    result
}

impl CaptureJournal {
    pub(crate) fn new(
        journal: &std::sync::Arc<RunJournal>,
        index: usize,
        job: &JobV1,
        include_attestation: bool,
    ) -> Self {
        Self {
            capture: crate::observations::FixtureCapture::new(
                job.cleanup.capture_provider_candidate,
            ),
            journal: std::sync::Arc::clone(journal),
            index,
            requested: job.clone(),
            preparation: journal.row(index).local_runtime,
            include_attestation,
        }
    }
    pub(crate) fn publish(&self, settled: bool) {
        let facts = self.capture.snapshot();
        self.journal.update(self.index, |row| {
            if !settled
                && !row
                    .diagnostics
                    .iter()
                    .any(|v| v == "local_shutdown_unsettled")
            {
                row.diagnostics.push("local_shutdown_unsettled".into());
            }
            if let Some(reason) = &facts.loss_reason {
                if !row.diagnostics.contains(reason) {
                    row.diagnostics.push(reason.clone());
                }
            }
            if let Some(request) = &facts.asr_request {
                row.effective.sent_model = Some(request.constructed_model.clone());
                row.effective.endpoint = if request.protocol == "local_sidecar" {
                    None
                } else {
                    Some(request.endpoint_identity.clone())
                };
                row.effective.recognition_language =
                    Some(self.requested.recognition_language.clone());
                row.effective.vocabulary = Some(
                    if matches!(self.requested.vocabulary, Vocabulary::None {}) {
                        "none"
                    } else {
                        "explicit"
                    }
                    .into(),
                );
                row.effective.context = Some(
                    if matches!(self.requested.context, Context::Disabled {}) {
                        "disabled"
                    } else {
                        "fixture"
                    }
                    .into(),
                );
            }
            if let Some(asr) = &facts.asr {
                row.provider_text = asr.provider_text.clone();
                row.filtered_text = asr.filtered_text.clone();
                row.asr_origin = asr.origin.clone();
                row.asr_terminal = asr.terminal.clone();
                row.effective.server_model = asr.reported_model.clone();
            }
            if let Some(asr) = &facts.asr {
                if let Some(reason) = &asr.filtering_reason {
                    if !row
                        .transformations
                        .iter()
                        .any(|t| t.owner == "asr_parser_filter" && t.reason == *reason)
                    {
                        row.transformations.push(Transformation {
                            owner: "asr_parser_filter".into(),
                            revision: "voiceflow-asr-v1".into(),
                            reason: reason.clone(),
                            source_stage: "provider_asr".into(),
                        });
                    }
                }
            }
            if let Some(cleanup) = &facts.cleanup {
                for reason in &cleanup.transformations {
                    if !row
                        .transformations
                        .iter()
                        .any(|t| t.owner == "cleanup_adapter" && t.reason == *reason)
                    {
                        row.transformations.push(Transformation {
                            owner: "cleanup_adapter".into(),
                            revision: "voiceflow-cleanup-v1".into(),
                            reason: reason.clone(),
                            source_stage: "provider_cleanup".into(),
                        });
                    }
                }
            }
            if facts
                .local_asr
                .as_ref()
                .is_some_and(|local| local.events_truncated)
                && !row
                    .diagnostics
                    .iter()
                    .any(|s| s == "local_observation_partial")
            {
                row.diagnostics.push("local_observation_partial".into());
            }
            if let Some(mut runtime) = facts.local_asr {
                if !self.include_attestation {
                    runtime.loaded_attestation = None;
                }
                if let Some(preparation) = &self.preparation {
                    let mut events = preparation.events.clone();
                    events.extend(runtime.events);
                    runtime.events = events;
                    runtime.events_truncated |= preparation.events_truncated;
                }
                row.local_runtime = Some(runtime);
            }
            if facts.asr_request.is_some() {
                row.asr_request = facts.asr_request;
            }
            if facts.asr.is_some() {
                row.asr_observation = facts.asr;
            }
            row.cleanup_request = facts.cleanup_request;
            row.cleanup_observation = facts.cleanup;
        });
    }

    #[cfg(test)]
    pub(crate) fn observation_capture(&self) -> crate::observations::FixtureCapture {
        self.capture.clone()
    }

    #[cfg(test)]
    pub(crate) fn journal_handle(&self) -> (std::sync::Arc<RunJournal>, usize) {
        (std::sync::Arc::clone(&self.journal), self.index)
    }
}
impl Drop for CaptureJournal {
    fn drop(&mut self) {
        self.publish(true);
    }
}

pub(super) fn empty_result_outcome(
    negative: bool,
    observation: Option<&crate::observations::AsrObservation>,
) -> (StageState, &'static str) {
    let known_empty = observation.is_some_and(crate::observations::AsrObservation::confirmed_empty);
    match (negative, known_empty) {
        (true, true) => (StageState::Completed {}, "negative_confirmed_empty"),
        (true, false) => (
            StageState::Unknown {
                reason: "empty_origin_unknown".into(),
            },
            "empty_origin_unknown",
        ),
        (false, true) => (
            StageState::Failed {
                reason: "speech_no_usable_text".into(),
            },
            "speech_no_usable_text",
        ),
        (false, false) => (
            StageState::Failed {
                reason: "empty_origin_unknown".into(),
            },
            "empty_origin_unknown",
        ),
    }
}

fn mark_cancelled(row: &mut CaseRecordV1, reason: &str) {
    row.state = StageState::Cancelled {
        reason: reason.into(),
    };
    row.cancellation_requested = reason == "sigint";
    row.provider_request_termination_observed = StageState::Unknown {
        reason: "local_future_dropped_server_termination_unobserved".into(),
    };
}
pub(super) fn eval_case(c: &AudioCaseV1, input: &str, job: &JobV1) -> legacy::EvalCase {
    legacy::EvalCase {
        id: c.id.clone(),
        group: c.scenario.clone(),
        input: input.into(),
        expected: c.expected_final.clone(),
        reference_variants: c.reference_variants.clone(),
        cleanup: job.cleanup.intensity.as_str().into(),
        family: c.family.clone(),
        focus_kind: c.focus_kind.clone(),
        mode: if job.cleanup.intensity == CleanupIntensity::Off || job.mode == Mode::Verbatim {
            "ai-off"
        } else {
            "normal"
        }
        .into(),
        protected: c.protected.clone(),
    }
}

#[cfg(test)]
mod empty_outcome_tests {
    use super::*;
    use crate::observations::{AsrObservation, ObservedText};
    #[test]
    fn negative_completion_requires_bound_text_facts_and_speech_empty_is_failure() {
        let mut observed = AsrObservation {
            provider_text: ObservedText::KnownEmpty {},
            provider_text_source_kind: Some("message_content_string".into()),
            filtered_text: ObservedText::KnownEmpty {},
            origin: "provider_response".into(),
            terminal: "provider_empty".into(),
            filtering_reason: None,
            protocol_completion: "stop".into(),
            reported_model: ObservedText::unknown("provider_not_reported"),
        };
        let (state, diagnostic) = empty_result_outcome(true, Some(&observed));
        assert!(matches!(state, StageState::Completed {}));
        assert_eq!(diagnostic, "negative_confirmed_empty");
        assert!(matches!(empty_result_outcome(false, Some(&observed)).0,
            StageState::Failed { reason } if reason == "speech_no_usable_text"));
        observed.provider_text = ObservedText::unknown("provider_text_null");
        // Even a stale terminal label cannot turn unknown provider content into silence.
        assert!(matches!(
            empty_result_outcome(true, Some(&observed)).0,
            StageState::Unknown { .. }
        ));
        observed.terminal = "empty_origin_unknown".into();
        observed.filtered_text = ObservedText::unknown("provider_text_null");
        assert!(matches!(
            empty_result_outcome(true, Some(&observed)).0,
            StageState::Unknown { .. }
        ));
        assert!(matches!(
            empty_result_outcome(false, Some(&observed)).0,
            StageState::Failed { .. }
        ));
        assert!(matches!(
            empty_result_outcome(true, None).0,
            StageState::Unknown { .. }
        ));
        observed.provider_text = ObservedText::known("suppressed hallucination");
        observed.filtered_text = ObservedText::KnownEmpty {};
        observed.terminal = "locally_suppressed".into();
        assert!(matches!(
            empty_result_outcome(true, Some(&observed)).0,
            StageState::Completed {}
        ));
        observed.terminal = "provider_failed".into();
        assert!(matches!(
            empty_result_outcome(true, Some(&observed)).0,
            StageState::Unknown { .. }
        ));
    }
}
