// Opt-in reviewed audio fixtures, chaining actual ASR output through the
// existing production-adapter cleanup evaluator. Explicit fixture-only, no History access.
use super::artifacts::ArtifactStore;
use super::authorization::AuthorizationBlocks;
use super::execution::ExecutionError;
use super::legacy::*;

pub(crate) use super::contracts::{AudioManifestV1 as ReviewedAudioManifest, Provenance};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
impl ReviewedAudioManifest {
    #[cfg(test)]
    pub(crate) fn validate(&self) -> Result<(), super::validation::InputError> {
        super::validation::manifest_fields(Path::new("selected manifest"), self)
    }
    pub(crate) fn fixtures(&self) -> AudioManifest {
        AudioManifest {
            cases: self
                .cases
                .iter()
                .map(|case| AudioFixture {
                    id: case.id.clone(),
                    audio: case.audio.clone(),
                    reference: case.reference.clone(),
                    validated_audio: None,
                })
                .collect(),
        }
    }

    pub(crate) fn cleanup_cases(&self, asr: &AsrModelReport) -> Vec<EvalCase> {
        asr.cases
            .iter()
            .filter_map(|result| {
                if result.transcription_state != "transcribed" {
                    return None;
                }
                let input = result.sanitized_text.as_ref()?.trim();
                if input.is_empty() {
                    return None;
                }
                let case = self.cases.iter().find(|case| case.id == result.id)?;
                Some(EvalCase {
                    id: case.id.clone(),
                    group: case.scenario.clone(),
                    input: input.into(),
                    expected: case.expected_final.clone(),
                    reference_variants: case.reference_variants.clone(),
                    cleanup: case.cleanup.clone(),
                    family: case.family.clone(),
                    focus_kind: case.focus_kind.clone(),
                    mode: "normal".into(),
                    protected: case.protected.clone(),
                })
            })
            .collect()
    }
}

#[derive(Serialize)]
pub(crate) struct AudioPipelineReport {
    pub(crate) asr_candidate: String,
    pub(crate) audio_samples_with_nonempty_transcription: usize,
    pub(crate) additive_stage_latency: TimingSummary,
    pub(crate) cleanup: CleanupModelReport,
}

#[derive(Serialize)]
pub(crate) struct AudioTrialReport {
    pub(crate) control_mode: &'static str,
    pub(crate) baseline: super::contracts::BaselineIdentity,
    pub(crate) manifest_sha256: String,
    pub(crate) input_audio: BTreeMap<String, super::contracts::AudioIdentity>,
    pub(crate) schema_version: u32,
    pub(crate) runner: &'static str,
    pub(crate) note: &'static str,
    pub(crate) provenance: Provenance,
    pub(crate) sample_count: usize,
    pub(crate) case_ids: Vec<String>,
    pub(crate) manifest_fingerprint: String,
    pub(crate) cleanup_code_fingerprint: String,
    pub(crate) asr_models: Vec<AsrModelReport>,
    pub(crate) pipelines: Vec<AudioPipelineReport>,
}

pub(crate) fn additive_stage_latency(
    asr_cases: &[AsrCaseReport],
    cleanup_cases: &[CleanupCaseReport],
) -> TimingSummary {
    let times = cleanup_cases
        .iter()
        .filter_map(|case| {
            asr_cases
                .iter()
                .find(|audio| audio.id == case.id)?
                .latency_ms
                // provider_wait_ms already includes retry_wait_ms.
                .map(|asr_ms| asr_ms + case.total_ms + case.provider_wait_ms)
        })
        .collect();
    timing_summary(times)
}

fn require_external_path(path: &Path) -> PathBuf {
    let canonical =
        std::fs::canonicalize(path).expect("existing external fixture/output directory");
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap();
    assert!(
        !canonical.starts_with(repository),
        "keep reviewed audio, manifests and reports outside the repository"
    );
    canonical
}

pub(crate) async fn execute(
    validated: &super::validation::ValidatedInputs,
) -> Result<(), ExecutionError> {
    assert_eq!(
        std::env::var("VOICEFLOW_AUDIO_EVAL_OPT_IN").as_deref(),
        Ok("1"),
        "use the explicit audio evaluation command"
    );
    let snapshot = validated
        .selection
        .speech_manifest()
        .expect("legacy speech manifest");
    let manifest_path = &snapshot.canonical_path;
    let manifest = &snapshot.parsed;
    let bytes = &snapshot.raw;
    let requested = std::env::var("VOICEFLOW_AUDIO_EVAL_ASR_IDS").expect("select ASR candidates");
    let asr_ids = parse_id_filter(
        &requested,
        &[
            "groq_whisper_large_v3_turbo",
            "groq_whisper_large_v3",
            "openai_gpt_transcribe",
            "qwen3_asr_flash_dashscope",
        ],
        "ASR candidate",
    )
    .expect("valid ASR selection");
    let cleanup_filter =
        std::env::var("VOICEFLOW_LIVE_CLEANUP_CANDIDATE_IDS").expect("select cleanup candidates");
    let cleanup = configure_cleanup_candidates(
        cleanup_candidates(),
        Some(&cleanup_filter),
        Some(&cleanup_filter),
    );
    let asr = asr_candidates()
        .into_iter()
        .filter(|candidate| asr_ids.iter().any(|id| id == candidate.id))
        .collect::<Vec<_>>();
    assert!(
        !asr.is_empty() && !cleanup.is_empty(),
        "select both ASR and cleanup candidates"
    );
    for (has_key, source) in asr
        .iter()
        .map(|candidate| (candidate.credentials_available(), &candidate.key_source))
        .chain(
            cleanup
                .iter()
                .map(|candidate| (candidate.key.is_some(), &candidate.key_source)),
        )
    {
        assert!(
            has_key
                && matches!(
                    source.as_str(),
                    "explicit_environment" | "explicit_key_file"
                ),
            "audio evaluation requires explicitly supplied credentials for every selected provider"
        );
    }
    let requested_report =
        PathBuf::from(std::env::var_os("VOICEFLOW_AUDIO_EVAL_REPORT").expect("select report path"));
    require_external_path(requested_report.parent().expect("absolute report path"));
    let report_path = unique_output_path(&requested_report, None);
    let fingerprint = stable_fingerprint(&[bytes]);
    let code_fingerprint = cleanup_code_fingerprint();
    let mut fixtures = manifest.fixtures();
    for fixture in &mut fixtures.cases {
        fixture.validated_audio = validated
            .cases
            .iter()
            .find(|c| c.case.id() == fixture.id)
            .map(|c| c.bytes.clone());
    }
    let mut report = AudioTrialReport {
        manifest_sha256: super::validation::sha256(bytes),
        input_audio: validated.cases.iter().map(|c|(c.case.id().to_owned(),c.audio.clone())).collect(),
        control_mode: "legacy_fixed", baseline: super::identity::baseline(None).expect("baseline identity"),
        schema_version: 1, runner: "opt_in_reviewed_audio_asr_to_cleanup",
        note: "Explicitly selected reviewed audio only. ASR reference is the human verbatim transcript; cleanup reference is expected_final with optional variants. Only successful nonempty sanitized ASR outputs enter cleanup. Empty/error ASR cases stay visible in asr_models and never count as cleanup successes. Candidate and fallback/finalized scores are separate. Additive stage latency sums each ASR request duration and its cleanup total (including cleanup pacing/retry waits), excluding time between batch stages, microphone capture, prefetch, encoding, paste, and OS target verification. These adapter measurements are not native stop-to-insert latency or broad accuracy evidence. Scene metadata is supplied by the fixture, not captured from an App. Fixed ASR vocabulary hints use the existing live evaluator. Reports contain fixture text and transcripts, never keys or copied audio.",
        sample_count: manifest.cases.len(), case_ids: manifest.cases.iter().map(|case| case.id.clone()).collect(),
        manifest_fingerprint: fingerprint.clone(), cleanup_code_fingerprint: code_fingerprint.clone(),
        provenance: Provenance { kind: manifest.provenance.kind.clone(), note: manifest.provenance.note.clone(), reviewed: true },
        asr_models: vec![], pipelines: vec![],
    };
    let mut artifacts = ArtifactStore::default();
    let mut blocked = AuthorizationBlocks::default();
    let mut pacers = BTreeMap::new();
    write_json_report(&mut artifacts, &report_path, &report)
        .map_err(|failure| ExecutionError::artifact(&report_path, failure, attempts(&report)))?;
    for candidate in asr {
        let asr_report = run_asr_candidate(
            &candidate,
            manifest_path,
            &fixtures,
            blocked.reason(candidate.authorization_scope.as_ref()),
            None,
        )
        .await
        .map_err(|mut error| {
            error.provider_attempts += attempts(&report);
            error
        })?;
        blocked.observe(
            candidate.authorization_scope.as_ref(),
            asr_report.stop_category.as_deref(),
        );
        let cases = manifest.cleanup_cases(&asr_report);
        let case_bytes = serde_json::to_vec(
            &cases
                .iter()
                .map(|case| (&case.id, &case.input))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let pipeline_fingerprint =
            stable_fingerprint(&[fingerprint.as_bytes(), candidate.id.as_bytes(), &case_bytes]);
        report.asr_models.push(asr_report);
        write_json_report(&mut artifacts, &report_path, &report).map_err(|failure| {
            ExecutionError::artifact(&report_path, failure, attempts(&report))
        })?;
        if cases.is_empty() {
            continue;
        }
        let mut resume = ResumeState::default();
        let checkpoint = unique_output_path(
            &report_path.with_extension(format!("{}.checkpoint.json", candidate.id)),
            None,
        );
        for model in &cleanup {
            let pacer = pacers
                .entry(model.authorization_scope.clone())
                .or_insert_with(|| RequestPacer::new(minimum_request_gap()));
            let cleaned = run_cleanup_candidate(
                model,
                CleanupCandidateRun {
                    cases: &cases,
                    blocked_reason: blocked.reason(model.authorization_scope.as_ref()),
                    resume_state: &mut resume,
                    checkpoint_path: Some(&checkpoint),
                    corpus_fingerprint: &pipeline_fingerprint,
                    cleanup_code_fingerprint: &code_fingerprint,
                    pacer,
                    observer: None,
                    preparation: None,
                    replay: None,
                },
            )
            .await
            .map_err(|mut error| {
                error.provider_attempts += attempts(&report);
                error
            })?;
            blocked.observe(
                model.authorization_scope.as_ref(),
                cleaned.stop_category.as_deref(),
            );
            let current_asr = report.asr_models.last().unwrap();
            let latency = additive_stage_latency(&current_asr.cases, &cleaned.cases);
            report.pipelines.push(AudioPipelineReport {
                asr_candidate: candidate.id.into(),
                audio_samples_with_nonempty_transcription: cases.len(),
                additive_stage_latency: latency,
                cleanup: cleaned,
            });
            write_json_report(&mut artifacts, &report_path, &report).map_err(|failure| {
                ExecutionError::artifact(&report_path, failure, attempts(&report))
            })?;
        }
    }
    eprintln!("Reviewed audio report: {} ({} selected audio cases; {} ASR candidates; {} cleanup pipelines)", report_path.display(), report.sample_count, report.asr_models.len(), report.pipelines.len());
    Ok(())
}
fn attempts(report: &AudioTrialReport) -> u64 {
    report
        .asr_models
        .iter()
        .map(|m| m.provider_invoked_cases as u64)
        .sum::<u64>()
        + report
            .pipelines
            .iter()
            .map(|p| p.cleanup.provider_http_attempts as u64)
            .sum::<u64>()
}
