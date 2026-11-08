use super::*;
use crate::evaluation::audio_legacy::*;
#[tokio::test]
#[ignore = "explicit opt-in reviewed WAV uploads; run with npm run eval:audio -- --run"]
async fn live_reviewed_audio_pipeline_uses_production_adapters() {
    let path = std::path::PathBuf::from(
        std::env::var_os("VOICEFLOW_AUDIO_EVAL_MANIFEST").expect("select external manifest"),
    );
    let inputs = crate::evaluation::validation::validate(&path, None, None, None)
        .expect("validate before credentials");
    crate::evaluation::audio_legacy::execute(&inputs)
        .await
        .expect("owned legacy audio evaluator");
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
        reference: Some("Human reference".into()),
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
        transport: crate::evaluation::legacy::CleanupTransport::Cloud(EngineProvider::Custom),
        model: "synthetic-model".into(),
        base_url: "http://127.0.0.1:1".into(),
        endpoint: "http://127.0.0.1:1/chat/completions".into(),
        key: None,
        key_source: "none".into(),
        authorization_scope: None,
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
            checkpoint_path: Some(&checkpoint),
            corpus_fingerprint: "test-corpus",
            cleanup_code_fingerprint: "test-code",
            pacer: &mut pacer,
            observer: None,
            preparation: None,
            replay: None,
        },
    )
    .await
    .expect("shared cleanup evaluator");
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
    assert_eq!(timing.p50_ms, Some(911.5));
    assert!((timing.p95_ms.unwrap() - 1443.85).abs() < 1e-9);
    let _ = std::fs::remove_file(checkpoint);
}
