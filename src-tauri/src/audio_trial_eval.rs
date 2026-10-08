// Opt-in reviewed audio fixtures, chaining actual ASR output through the
// existing production-adapter cleanup evaluator. Test-only, no History access.
use super::*;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    kind: String,
    note: String,
    reviewed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewedAudioManifest {
    schema_version: u32,
    provenance: Provenance,
    cases: Vec<ReviewedAudioCase>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewedAudioCase {
    id: String,
    audio: String,
    reference: String,
    expected_final: String,
    scenario: String,
    family: String,
    focus_kind: String,
    cleanup: String,
    #[serde(default)]
    protected: Vec<String>,
    #[serde(default)]
    reference_variants: Vec<String>,
}

impl ReviewedAudioManifest {
    fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != 1
            || !matches!(
                self.provenance.kind.as_str(),
                "human_recorded" | "synthetic"
            )
            || !self.provenance.reviewed
            || self.provenance.note.trim().is_empty()
            || self.provenance.note.len() > 4096
        {
            return Err("record reviewed audio provenance and use schema version 1");
        }
        if self.cases.is_empty() || self.cases.len() > 200 {
            return Err("select between 1 and 200 audio cases");
        }
        let mut ids = std::collections::BTreeSet::new();
        for case in &self.cases {
            let filename = Path::new(&case.audio);
            if case.id.trim().is_empty() || case.id.len() > 128 || !ids.insert(&case.id) {
                return Err("audio case IDs must be unique and nonempty");
            }
            if case.audio.contains(['/', '\\'])
                || filename.components().count() != 1
                || filename.extension().and_then(|ext| ext.to_str()) != Some("wav")
            {
                return Err("audio must be an adjacent WAV filename");
            }
            if [&case.reference, &case.expected_final, &case.scenario]
                .iter()
                .any(|text| text.trim().is_empty() || text.len() > 16 * 1024)
                || case.protected.len() > 128
                || case.reference_variants.len() > 16
                || case
                    .protected
                    .iter()
                    .chain(&case.reference_variants)
                    .any(|text| text.trim().is_empty() || text.len() > 16 * 1024)
            {
                return Err("review bounded references, scenarios and protected values");
            }
            if crate::context::builtin_family_for_id(&case.family).is_none()
                || !matches!(
                    case.focus_kind.as_str(),
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
                || !matches!(
                    case.cleanup.as_str(),
                    "auto" | "off" | "light" | "standard" | "heavy"
                )
            {
                return Err("use a supported family, focus kind and cleanup intensity");
            }
        }
        Ok(())
    }

    fn fixtures(&self) -> AudioManifest {
        AudioManifest {
            cases: self
                .cases
                .iter()
                .map(|case| AudioFixture {
                    id: case.id.clone(),
                    audio: case.audio.clone(),
                    reference: case.reference.clone(),
                })
                .collect(),
        }
    }

    fn cleanup_cases(&self, asr: &AsrModelReport) -> Vec<EvalCase> {
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
struct AudioPipelineReport {
    asr_candidate: String,
    audio_samples_with_nonempty_transcription: usize,
    additive_stage_latency: TimingSummary,
    cleanup: CleanupModelReport,
}

#[derive(Serialize)]
struct AudioTrialReport {
    schema_version: u32,
    runner: &'static str,
    note: &'static str,
    provenance: Provenance,
    sample_count: usize,
    case_ids: Vec<String>,
    manifest_fingerprint: String,
    cleanup_code_fingerprint: String,
    asr_models: Vec<AsrModelReport>,
    pipelines: Vec<AudioPipelineReport>,
}

fn additive_stage_latency(
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

#[tokio::test]
#[ignore = "explicit opt-in reviewed WAV uploads; run with npm run eval:audio -- --run"]
async fn live_reviewed_audio_pipeline_uses_production_adapters() {
    assert_eq!(
        std::env::var("VOICEFLOW_AUDIO_EVAL_OPT_IN").as_deref(),
        Ok("1"),
        "use the explicit audio evaluation command"
    );
    let manifest_path = require_external_path(&PathBuf::from(
        std::env::var_os("VOICEFLOW_AUDIO_EVAL_MANIFEST").expect("select an external manifest"),
    ));
    let bytes = std::fs::read(&manifest_path).expect("read selected manifest");
    assert!(bytes.len() <= 1024 * 1024, "manifest limit is 1 MiB");
    let manifest: ReviewedAudioManifest =
        serde_json::from_slice(&bytes).expect("reviewed audio manifest schema");
    manifest
        .validate()
        .expect("reviewed audio fixture validation");
    // Validate every fixture before resolving credentials or sending a request.
    for case in &manifest.cases {
        let path = require_external_path(&safe_audio_fixture_path(&manifest_path, &case.audio));
        assert_eq!(
            path.parent(),
            manifest_path.parent(),
            "audio symlinks must remain adjacent to the manifest"
        );
        let size = std::fs::metadata(&path).unwrap().len();
        assert!(
            size > 44 && size <= 25 * 1024 * 1024,
            "WAV fixtures must be nonempty and at most 25 MiB"
        );
        let audio = std::fs::read(path).expect("read WAV fixture");
        assert!(
            crate::asr::wav_duration_seconds(&audio).is_some_and(|duration| duration > 0.0),
            "valid PCM WAV fixture required"
        );
    }
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
        .map(|candidate| (candidate.key.is_some(), &candidate.key_source))
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
    let fingerprint = stable_fingerprint(&[&bytes]);
    let code_fingerprint = cleanup_code_fingerprint();
    let fixtures = manifest.fixtures();
    let mut report = AudioTrialReport {
        schema_version: 1, runner: "opt_in_reviewed_audio_asr_to_cleanup",
        note: "Explicitly selected reviewed audio only. ASR reference is the human verbatim transcript; cleanup reference is expected_final with optional variants. Only successful nonempty sanitized ASR outputs enter cleanup. Empty/error ASR cases stay visible in asr_models and never count as cleanup successes. Candidate and fallback/finalized scores are separate. Additive stage latency sums each ASR request duration and its cleanup total (including cleanup pacing/retry waits), excluding time between batch stages, microphone capture, prefetch, encoding, paste, and OS target verification. These adapter measurements are not native stop-to-insert latency or broad accuracy evidence. Scene metadata is supplied by the fixture, not captured from an App. Fixed ASR vocabulary hints use the existing live evaluator. Reports contain fixture text and transcripts, never keys or copied audio.",
        sample_count: manifest.cases.len(), case_ids: manifest.cases.iter().map(|case| case.id.clone()).collect(),
        manifest_fingerprint: fingerprint.clone(), cleanup_code_fingerprint: code_fingerprint.clone(),
        provenance: Provenance { kind: manifest.provenance.kind.clone(), note: manifest.provenance.note.clone(), reviewed: true },
        asr_models: vec![], pipelines: vec![],
    };
    let mut blocked_asr = BTreeMap::new();
    let mut blocked_cleanup = BTreeMap::new();
    let mut pacers = BTreeMap::new();
    write_json_report(&report_path, &report);
    for candidate in asr {
        let asr_report = run_asr_candidate(
            &candidate,
            &manifest_path,
            &fixtures,
            blocked_asr.get(candidate.provider).cloned(),
        )
        .await;
        if asr_report.stop_category.as_deref() == Some("authorization_error") {
            blocked_asr.insert(candidate.provider, "authorization_error".to_owned());
        }
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
        write_json_report(&report_path, &report);
        if cases.is_empty() {
            continue;
        }
        let mut resume = ResumeState::default();
        let checkpoint = unique_output_path(
            &report_path.with_extension(format!("{}.checkpoint.json", candidate.id)),
            None,
        );
        for model in &cleanup {
            let provider = model.provider.as_str().to_owned();
            let pacer = pacers
                .entry(provider.clone())
                .or_insert_with(|| RequestPacer::new(minimum_request_gap()));
            let cleaned = run_cleanup_candidate(
                model,
                CleanupCandidateRun {
                    cases: &cases,
                    blocked_reason: blocked_cleanup.get(&provider).cloned(),
                    resume_state: &mut resume,
                    checkpoint_path: &checkpoint,
                    corpus_fingerprint: &pipeline_fingerprint,
                    cleanup_code_fingerprint: &code_fingerprint,
                    pacer,
                },
            )
            .await;
            if cleaned.stop_category.as_deref() == Some("authorization_error") {
                blocked_cleanup.insert(provider, "authorization_error".to_owned());
            }
            let current_asr = report.asr_models.last().unwrap();
            let latency = additive_stage_latency(&current_asr.cases, &cleaned.cases);
            report.pipelines.push(AudioPipelineReport {
                asr_candidate: candidate.id.into(),
                audio_samples_with_nonempty_transcription: cases.len(),
                additive_stage_latency: latency,
                cleanup: cleaned,
            });
            write_json_report(&report_path, &report);
        }
    }
    eprintln!("Reviewed audio report: {} ({} selected audio cases; {} ASR candidates; {} cleanup pipelines)", report_path.display(), report.sample_count, report.asr_models.len(), report.pipelines.len());
}

#[test]
fn reviewed_audio_manifest_rejects_unreviewed_and_unsafe_fixtures() {
    let json = serde_json::json!({
        "schema_version": 1, "provenance": {"kind": "human_recorded", "note": "De-identified self-recorded fixture", "reviewed": true},
        "cases": [{"id":"one", "audio":"one.wav", "reference":"Friday at 3", "expected_final":"Friday at 3.", "scenario":"work chat", "family":"work_chat", "focus_kind":"chat", "cleanup":"standard", "protected":["3"]}]
    });
    let mut manifest: ReviewedAudioManifest = serde_json::from_value(json).unwrap();
    assert!(manifest.validate().is_ok());
    manifest.cases[0].family = "prompt_or_code".into();
    manifest.cases[0].focus_kind = "coding_prompt".into();
    assert!(manifest.validate().is_ok());
    manifest.provenance.reviewed = false;
    assert!(manifest.validate().is_err());
    manifest.provenance.reviewed = true;
    manifest.cases[0].audio = "../one.wav".into();
    assert!(manifest.validate().is_err());
    manifest.cases[0].audio = "one.wav".into();
    manifest.cases[0].expected_final.clear();
    assert!(manifest.validate().is_err());
}

#[test]
fn cleanup_input_is_actual_asr_output_and_errors_never_become_successes() {
    let manifest: ReviewedAudioManifest = serde_json::from_value(serde_json::json!({
        "schema_version":1, "provenance":{"kind":"synthetic","note":"test fixture","reviewed":true},
        "cases":[{"id":"one","audio":"one.wav","reference":"Human reference","expected_final":"Reviewed final","scenario":"mixed language","family":"general","focus_kind":"unknown","cleanup":"standard"}]
    })).unwrap();
    let mut asr = synthetic_asr_report();
    let cases = manifest.cleanup_cases(&asr);
    assert_eq!(cases[0].input, "actual ASR");
    assert_eq!(cases[0].expected, "Reviewed final");
    asr.cases[0].transcription_state = "provider_error".into();
    assert!(manifest.cleanup_cases(&asr).is_empty());
    asr.cases[0].transcription_state = "empty_result".into();
    assert!(manifest.cleanup_cases(&asr).is_empty());
}

fn synthetic_asr_report() -> AsrModelReport {
    let result = AsrCaseReport {
        id: "one".into(),
        reference: "Human reference".into(),
        provider_invoked: true,
        transcription_state: "transcribed".into(),
        raw_asr_text: Some("um actual ASR".into()),
        sanitized_text: Some("actual ASR".into()),
        detected_language: None,
        confidence: None,
        segment_count: None,
        word_count: None,
        error_category: None,
        status: None,
        latency_ms: Some(10.0),
    };
    AsrModelReport {
        candidate: "test".into(),
        provider: "test".into(),
        model: "test".into(),
        sanitized_host: "test.invalid".into(),
        key_source: "none".into(),
        verification_status: "completed".into(),
        stop_category: None,
        provider_invoked_cases: 1,
        successful_transcriptions: 1,
        no_speech_results: 0,
        error_categories: BTreeMap::new(),
        raw_asr_quality: QualitySummary::default(),
        sanitized_text_quality: QualitySummary::default(),
        timings: timing_summary(vec![]),
        cases: vec![result],
    }
}

#[test]
fn every_reviewed_focus_kind_reaches_live_cleanup_preparation() {
    let asr = synthetic_asr_report();
    for (focus, expected) in [
        ("secure", FocusKind::Secure),
        ("search", FocusKind::Search),
        ("code", FocusKind::Code),
        ("coding_prompt", FocusKind::CodingPrompt),
        ("terminal", FocusKind::Terminal),
        ("email", FocusKind::Email),
        ("chat", FocusKind::Chat),
        ("document", FocusKind::Document),
        ("form", FocusKind::Form),
        ("editable", FocusKind::Editable),
        ("unknown", FocusKind::Unknown),
    ] {
        let family = if focus == "coding_prompt" {
            "prompt_or_code"
        } else {
            "general"
        };
        let manifest: ReviewedAudioManifest = serde_json::from_value(serde_json::json!({
            "schema_version": 1, "provenance": {"kind":"synthetic","note":"reviewed test fixture","reviewed":true},
            "cases": [{"id":"one","audio":"one.wav","reference":"Human reference","expected_final":"actual ASR.","scenario":"fixture routing","family":family,"focus_kind":focus,"cleanup":"standard"}]
        })).unwrap();
        assert!(manifest.validate().is_ok());
        let cases = manifest.cleanup_cases(&asr);
        let prepared = prepare_cleanup_case(&cases[0]);
        assert_eq!(prepared.input_kind, expected, "{focus}");
        if focus == "coding_prompt" {
            assert!(matches!(
                prepared.route,
                crate::lexicon::CleanupRoute::Provider(_)
            ));
        }
    }
}

#[tokio::test]
async fn additive_latency_matches_ids_includes_waits_once_and_skips_missing_asr_timings() {
    let mut asr = synthetic_asr_report();
    asr.cases[0].latency_ms = Some(100.0);
    let mut second = synthetic_asr_report().cases.remove(0);
    second.id = "two".into();
    second.latency_ms = Some(20.0);
    asr.cases.insert(0, second);
    let mut missing = synthetic_asr_report().cases.remove(0);
    missing.id = "missing".into();
    missing.latency_ms = None;
    asr.cases.push(missing);
    let cases = ["one", "two", "missing", "unmatched"]
        .map(|id| fixture_case(id, "english", "actual ASR", "provider-failure"));
    let candidate = CleanupCandidate {
        id: "synthetic",
        provider: EngineProvider::Custom,
        model: "synthetic-model".into(),
        base_url: "http://127.0.0.1:1".into(),
        key: None,
        key_source: "none".into(),
    };
    let mut resume = ResumeState::default();
    let mut pacer = RequestPacer::new(Duration::ZERO);
    let checkpoint = std::env::temp_dir().join(format!(
        "voiceflow-audio-timing-test-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let mut cleanup = run_cleanup_candidate(
        &candidate,
        CleanupCandidateRun {
            cases: &cases,
            blocked_reason: None,
            resume_state: &mut resume,
            checkpoint_path: &checkpoint,
            corpus_fingerprint: "test-corpus",
            cleanup_code_fingerprint: "test-code",
            pacer: &mut pacer,
        },
    )
    .await;
    assert_eq!(cleanup.provider_invoked_cases, 0);
    // Reviewed metric observations: total excludes pacing/backoff, and backoff
    // is a subset of provider_wait_ms rather than another additive stage.
    for case in &mut cleanup.cases {
        match case.id.as_str() {
            "one" => {
                case.total_ms = 403.0;
                case.provider_wait_ms = 1000.0;
                case.retry_wait_ms = 750.0;
            }
            "two" => {
                case.total_ms = 200.0;
                case.provider_wait_ms = 100.0;
            }
            _ => {
                case.total_ms = 999.0;
                case.provider_wait_ms = 500.0;
            }
        }
    }
    let timing = additive_stage_latency(&asr.cases, &cleanup.cases);
    assert_eq!(timing.sample_count, 2);
    assert_eq!(timing.p50_ms, Some(320.0));
    assert_eq!(timing.p95_ms, Some(1503.0));
    let _ = std::fs::remove_file(checkpoint);
}
