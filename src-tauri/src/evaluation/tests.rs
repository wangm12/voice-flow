use super::{contracts::*, validation::*};
use std::path::Path;
pub(super) fn audio() -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(
            &mut cursor,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..160 {
            writer.write_sample(0i16).unwrap();
        }
        writer.finalize().unwrap();
    }
    cursor.into_inner()
}
#[test]
fn strict_contract_rejects_unknown_and_duplicate_json_fields() {
    let v = serde_json::json!({"artifact_type":"non_speech_index","schema_version":1,"provenance":{"kind":"synthetic","note":"fixture","reviewed":true},"cases":[],"reference":"fake speech"});
    assert!(serde_json::from_value::<NonSpeechIndexV1>(v).is_err());
    assert!(serde_json::from_str::<Provenance>(
        r#"{"kind":"synthetic","kind":"human_recorded","note":"fixture","reviewed":true}"#
    )
    .is_err());
    assert!(serde_json::from_value::<Vocabulary>(
        serde_json::json!({"kind":"none","content_sha256":"unexpected"})
    )
    .is_err());
    assert!(serde_json::from_value::<Context>(
        serde_json::json!({"kind":"disabled","source_sha256":"unexpected"})
    )
    .is_err());
    assert!(serde_json::from_value::<StageState>(
        serde_json::json!({"state":"not_run","reason":"unexpected"})
    )
    .is_err());
    assert!(serde_json::from_value::<StageState>(
        serde_json::json!({"state":"completed","reason":"unexpected"})
    )
    .is_err());
    assert!(serde_json::from_value::<ObservedText>(
        serde_json::json!({"presence":"known_empty","value":"unexpected"})
    )
    .is_err());
}
#[test]
fn wav_rejects_changed_format_truncated_chunks_and_riff_trailing_data() {
    let p = Path::new("declared.wav");
    let b = audio();
    let identity = wav(p, &b).unwrap();
    assert_eq!(identity.sample_count, 160);
    assert_eq!(identity.audio_sha256, sha256(&b));
    assert_eq!(identity.pcm_sha256, sha256(&vec![0; 320]));
    let mut corrupted = b.clone();
    corrupted[24..28].copy_from_slice(&48_000u32.to_le_bytes());
    assert_eq!(wav(p, &corrupted).unwrap_err().code, "audio_format");
    assert_eq!(wav(p, &b[..b.len() - 1]).unwrap_err().code, "audio_format");
    let mut tail = b;
    tail.push(0);
    assert_eq!(wav(p, &tail).unwrap_err().code, "audio_format");
}
#[test]
fn legacy_speech_requires_reviewed_nonempty_reference_but_negative_has_no_reference() {
    let mut m:AudioManifestV1=serde_json::from_value(serde_json::json!({"schema_version":1,"provenance":{"kind":"synthetic","note":"test","reviewed":true},"cases":[{"id":"one","audio":"one.wav","reference":"test","expected_final":"test","scenario":"test","family":"general","focus_kind":"unknown","cleanup":"off"}]})).unwrap();
    assert!(manifest_fields(Path::new("manifest"), &m).is_ok());
    m.cases[0].reference.clear();
    assert!(manifest_fields(Path::new("manifest"), &m).is_err());
    let n:NonSpeechIndexV1=serde_json::from_value(serde_json::json!({"artifact_type":"non_speech_index","schema_version":1,"provenance":{"kind":"synthetic","note":"test","reviewed":true},"cases":[{"case_id":"negative","adjacent_wav":"negative.wav","audio_sha256":"0".repeat(64),"category":"silence","source_review_id":"self-reviewed"}]})).unwrap();
    assert_eq!(n.cases.len(), 1);
}
#[test]
fn argument_conflicts_reject_before_inputs_or_credentials() {
    assert!(
        super::parse(&["--experiment", "recipe", "--asr", "candidate"].map(str::to_owned)).is_err()
    );
    assert!(super::parse(&["--validate", "--run"].map(str::to_owned)).is_err());
    assert!(super::parse(&["--run", "--run"].map(str::to_owned)).is_err());
    assert!(super::parse(
        &["--filter-evidence", "e.json", "--filter-report", "r.json"].map(str::to_owned)
    )
    .is_ok());
    assert!(super::parse(&["--filter-evidence", "e.json"].map(str::to_owned)).is_err());
    assert!(super::parse(
        &[
            "--filter-evidence",
            "e.json",
            "--filter-report",
            "r.json",
            "--run"
        ]
        .map(str::to_owned)
    )
    .is_err());
}
#[test]
fn vf14_legacy_local_selection_rejects_before_any_credential_or_asr_lookup() {
    // An unsupported ASR ID would panic if the ASR lookup ran first. This
    // exercises the real legacy resolver's no-credential early rejection.
    let error = super::runner::legacy_required_keys("must-not-resolve", super::local_cleanup::ID)
        .unwrap_err();
    assert_eq!(error.code, "unsupported_candidate");
}
#[test]
fn vf14_real_single_resolver_skips_all_cloud_keys_for_local_asr_and_cleanup() {
    let d = FixtureDirectory::new();
    let (mut inputs, journal) = journal_fixture(&d);
    let job = &mut inputs.recipe.as_mut().unwrap().jobs[0];
    job.candidate_id = "local_qwen3_asr_0_6b".into();
    job.cleanup.intensity = CleanupIntensity::Standard;
    job.cleanup.candidate_id = Some(super::local_cleanup::ID.into());
    let admission = super::resume::Admission::read(&inputs, None, None).unwrap();
    let replay = admission
        .bind(&inputs, &journal.snapshot().identity)
        .unwrap();
    assert!(super::runner::resolve_keys(
        &inputs,
        &tokio_util::sync::CancellationToken::new(),
        &replay,
        &journal
    )
    .unwrap()
    .is_empty());
}

use super::fixtures::FixtureDirectory;
fn speech_json(id: &str, wav: &str) -> serde_json::Value {
    serde_json::json!({"schema_version":1,"provenance":{"kind":"synthetic","note":"reviewed test-only fixture","reviewed":true},"cases":[{"id":id,"audio":wav,"reference":"中文 emoji 😀","expected_final":"中文 emoji 😀。","scenario":"boundary fixture","family":"general","focus_kind":"unknown","cleanup":"off"}]})
}
fn recipe_json(sidecar: &std::path::Path) -> serde_json::Value {
    serde_json::json!({"artifact_type":"run_recipe","schema_version":1,"recipe_id":"boundary-fixture","corpus_sidecar_sha256":sha256(&std::fs::read(sidecar).unwrap()),"track":"file_adapter","jobs":[{"product":"voiceflow","artifact_identity":{"version":"0.1.4","source_commit":"declared-test-source","binary_sha256":null},"candidate_id":"groq_whisper_large_v3_turbo","mode":"dictation","recognition_language":"auto","decode_settings":{"temperature":null,"beam_size":null},"vocabulary":{"kind":"none"},"context":{"kind":"disabled"},"cleanup":{"intensity":"off","candidate_id":null,"capture_provider_candidate":false},"local_rules":{"dictionary_replacements":false,"fuzzy":false,"spoken_layout":true,"revision":"voiceflow-production-v1"},"profile_state_id":"explicit-fixture-profile","repeats":1,"pacing":{"gap_ms":0},"run_budget":{"max_cases":1000,"max_requests":1000,"max_retries":0,"deadline_ms":1000}}],"gui_fixture":null})
}
fn sidecar_json(manifests: &[(std::path::PathBuf, serde_json::Value)]) -> serde_json::Value {
    let mut manifest_entries = vec![];
    let mut cases = vec![];
    for (index, (path, manifest)) in manifests.iter().enumerate() {
        let mh = sha256(&std::fs::read(path).unwrap());
        manifest_entries.push(serde_json::json!({"relative_path":path.file_name().unwrap().to_str().unwrap(),"sha256":mh}));
        for case in manifest["cases"].as_array().unwrap() {
            let wav = path.parent().unwrap().join(case["audio"].as_str().unwrap());
            let bytes = std::fs::read(&wav).unwrap();
            let a = super::validation::wav(&wav, &bytes).unwrap();
            cases.push(serde_json::json!({"case_id":case["id"],"manifest_sha256":mh,"audio_sha256":a.audio_sha256,"pcm_sha256":a.pcm_sha256,"source_kind":"synthetic","rights_review_id":"self-reviewed-fixture","speaker_id":format!("speaker-{index}"),"source_session_id":format!("session-{index}"),"template_family_id":format!("template-{index}"),"split":"dev","language":"mixed","conditions":{"noise":"silence","microphone":"generated","speaking_style":"fixture"},"reference_sha256":sha256(case["reference"].as_str().unwrap().as_bytes()),"annotation_revision":"human-review-v1"}));
        }
    }
    serde_json::json!({"artifact_type":"corpus_sidecar","schema_version":1,"corpus_id":"fixture-corpus","manifests":manifest_entries,"cases":cases})
}
#[test]
fn multi_manifest_sidecar_validates_full_coverage_and_exact_input_hashes() {
    let d = FixtureDirectory::new();
    std::fs::write(d.0.join("one.wav"), audio()).unwrap();
    let m = speech_json("one", "one.wav");
    let p = d.write("speech.json", &m);
    let s = sidecar_json(&[(p.clone(), m)]);
    let side = d.write("corpus.json", &s);
    let recipe = d.write("recipe.json", &recipe_json(&side));
    let inputs = validate(&p, Some(&side), Some(&recipe), None).unwrap();
    assert_eq!(inputs.cases.len(), 1);
    assert_eq!(inputs.cases[0].bytes, audio());
    let mut mismatch = s.clone();
    mismatch["cases"][0]["reference_sha256"] = serde_json::json!("0".repeat(64));
    d.write("corpus.json", &mismatch);
    d.write("recipe.json", &recipe_json(&side));
    let error = validate(&p, Some(&side), Some(&recipe), None).unwrap_err();
    assert_eq!(error.code, "sha_mismatch");
    assert_eq!(error.provider_attempts, 0);
    let mut missing = s.clone();
    missing["cases"] = serde_json::json!([]);
    d.write("corpus.json", &missing);
    d.write("recipe.json", &recipe_json(&side));
    assert!(validate(&p, Some(&side), Some(&recipe), None).is_err());
    let mut unknown = s;
    unknown["cases"][0]["conditions"]["unrecognized_condition"] = serde_json::json!(true);
    d.write("corpus.json", &unknown);
    d.write("recipe.json", &recipe_json(&side));
    assert_eq!(
        validate(&p, Some(&side), Some(&recipe), None)
            .unwrap_err()
            .code,
        "unknown_field"
    );
}
#[test]
fn cross_manifest_duplicate_and_last_corrupt_wav_fail_before_execution() {
    let d = FixtureDirectory::new();
    std::fs::write(d.0.join("one.wav"), audio()).unwrap();
    std::fs::write(d.0.join("two.wav"), audio()).unwrap();
    let first = speech_json("duplicate", "one.wav");
    let second = speech_json("duplicate", "two.wav");
    let p = d.write("first.json", &first);
    let q = d.write("second.json", &second);
    let s = sidecar_json(&[(p.clone(), first.clone()), (q.clone(), second)]);
    let side = d.write("corpus.json", &s);
    let recipe = d.write("recipe.json", &recipe_json(&side));
    assert_eq!(
        validate(&p, Some(&side), Some(&recipe), None)
            .unwrap_err()
            .code,
        "duplicate_id"
    );
    let second = speech_json("last", "two.wav");
    d.write("second.json", &second);
    let s = sidecar_json(&[(p.clone(), first), (q, second)]);
    d.write("corpus.json", &s);
    d.write("recipe.json", &recipe_json(&side));
    std::fs::write(d.0.join("two.wav"), b"RIFFbroken").unwrap();
    let error = validate(&p, Some(&side), Some(&recipe), None).unwrap_err();
    assert_eq!(error.code, "audio_format");
    assert_eq!(error.provider_attempts, 0);
}
#[test]
fn speaker_session_template_and_pcm_groups_cannot_cross_splits() {
    let d = FixtureDirectory::new();
    std::fs::write(d.0.join("one.wav"), audio()).unwrap();
    std::fs::write(d.0.join("two.wav"), audio()).unwrap();
    let first = speech_json("one", "one.wav");
    let second = speech_json("two", "two.wav");
    let p = d.write("first.json", &first);
    let q = d.write("second.json", &second);
    let mut s = sidecar_json(&[(p.clone(), first), (q, second)]);
    s["cases"][1]["split"] = serde_json::json!("test");
    let side = d.write("corpus.json", &s);
    let recipe = d.write("recipe.json", &recipe_json(&side));
    assert_eq!(
        validate(&p, Some(&side), Some(&recipe), None)
            .unwrap_err()
            .code,
        "split_overlap"
    );
}
#[test]
fn negative_index_uses_shared_wav_rules_without_fabricated_speech_reference() {
    let d = FixtureDirectory::new();
    let bytes = audio();
    std::fs::write(d.0.join("one.wav"), &bytes).unwrap();
    std::fs::write(d.0.join("negative.wav"), &bytes).unwrap();
    let m = speech_json("one", "one.wav");
    let p = d.write("speech.json", &m);
    let side = d.write("corpus.json", &sidecar_json(&[(p.clone(), m)]));
    let recipe = d.write("recipe.json", &recipe_json(&side));
    let negative=d.write("negative.json",&serde_json::json!({"artifact_type":"non_speech_index","schema_version":1,"provenance":{"kind":"synthetic","note":"reviewed generated silence","reviewed":true},"cases":[{"case_id":"negative","adjacent_wav":"negative.wav","audio_sha256":sha256(&bytes),"category":"silence","source_review_id":"self-reviewed"}]}));
    let inputs = validate(&p, Some(&side), Some(&recipe), Some(&negative)).unwrap();
    assert_eq!(inputs.cases.len(), 2);
    assert!(inputs.cases[1].negative());
    assert!(inputs.cases[1].case.speech().is_none());
    std::fs::write(d.0.join("negative.wav"), b"broken").unwrap();
    assert_eq!(
        validate(&p, Some(&side), Some(&recipe), Some(&negative))
            .unwrap_err()
            .provider_attempts,
        0
    );
}
#[cfg(unix)]
#[test]
fn audio_symlink_cannot_escape_artifact_directory() {
    let d = FixtureDirectory::new();
    let other = FixtureDirectory::new();
    std::fs::write(other.0.join("outside.wav"), audio()).unwrap();
    std::os::unix::fs::symlink(other.0.join("outside.wav"), d.0.join("linked.wav")).unwrap();
    let p = d.write("speech.json", &speech_json("one", "linked.wav"));
    assert_eq!(
        validate(&p, None, None, None).unwrap_err().code,
        "path_escape"
    );
}
#[tokio::test]
async fn public_executable_boundary_rejects_last_bad_fixture_before_output_or_credentials() {
    let d = FixtureDirectory::new();
    std::fs::write(d.0.join("one.wav"), audio()).unwrap();
    std::fs::write(d.0.join("bad.wav"), b"broken").unwrap();
    let mut m = speech_json("one", "one.wav");
    m["cases"]
        .as_array_mut()
        .unwrap()
        .push(speech_json("last", "bad.wav")["cases"][0].clone());
    let p = d.write("speech.json", &m);
    let out = d.0.join("run.json");
    let args = vec![
        "--manifest".into(),
        p.display().to_string(),
        "--asr".into(),
        "groq_whisper_large_v3_turbo".into(),
        "--cleanup".into(),
        "groq_gpt_oss_20b".into(),
        "--out".into(),
        out.display().to_string(),
        "--run".into(),
    ];
    let error = super::main(args).await.unwrap_err();
    assert_eq!(error.provider_attempts(), 0);
    assert_eq!(error.code(), "audio_format");
    assert!(!out.exists());
}

#[test]
fn recipe_expansion_budget_rejects_before_baseline_or_provider_resolution() {
    let d = FixtureDirectory::new();
    std::fs::write(d.0.join("one.wav"), audio()).unwrap();
    let m = speech_json("one", "one.wav");
    let p = d.write("speech.json", &m);
    let side = d.write("corpus.json", &sidecar_json(&[(p.clone(), m)]));
    let recipe = d.write("recipe.json", &recipe_json(&side));
    let mut inputs = validate(&p, Some(&side), Some(&recipe), None).unwrap();
    let job = inputs.recipe.as_ref().unwrap().jobs[0].clone();
    inputs.recipe.as_mut().unwrap().jobs = vec![JobV1 { repeats: 20, ..job }; 32];
    let first = inputs.cases.remove(0);
    for index in 0..20 {
        inputs.cases.push(ValidatedCase {
            corpus: None,
            bytes: first.bytes.clone(),
            case: CaseData::Speech(AudioCaseV1 {
                id: format!("case-{index}"),
                ..first.case.speech().unwrap().clone()
            }),
            audio_path: first.audio_path.clone(),
            audio: first.audio.clone(),
            document: first.document.clone(),
            source_kind: first.source_kind.clone(),
        });
    }
    let failure = super::runner::preflight(
        &inputs,
        &recipe,
        &mut super::admission_progress::Progress::disabled(),
    )
    .unwrap_err();
    assert_eq!(failure.code, "input_budget");
    assert_eq!(failure.provider_attempts, 0);
}

#[test]
fn evaluator_retains_production_correction_authorizations_for_final_guard() {
    let case = super::legacy::EvalCase {
        id: "reviewed-correction".into(),
        group: "correction provenance".into(),
        input: "我周四，不对，周五下午开会".into(),
        expected: "我周五下午开会".into(),
        reference_variants: vec![],
        cleanup: "off".into(),
        family: "calendar_task".into(),
        focus_kind: "editable".into(),
        mode: "ai-off".into(),
        protected: vec![],
    };
    let prepared = super::legacy::prepare_cleanup_case(&case);
    assert!(!prepared.source().contains("周四"));
    assert!(prepared.source().contains("周五"));
    let context = prepared.finalization_context();
    assert!(context.revision_source.unwrap().contains("周四"));
    assert_eq!(context.prepared_transcript, Some(prepared.source()));
    assert!(!context.revision_authorizations.is_empty());
    let final_text = crate::finalize_text_for_scene(
        prepared.source(),
        crate::CleanupDecision::Disabled,
        context,
    )
    .unwrap();
    assert_eq!(final_text.text, prepared.source());
    assert!(!final_text.degraded);
}

#[tokio::test]
async fn shared_evaluator_off_and_explicit_snippet_bypass_need_no_provider_or_checkpoint() {
    let candidate = super::legacy::CleanupCandidate {
        id: "local-no-provider",
        transport: super::legacy::CleanupTransport::Cloud(crate::engine::EngineProvider::Custom),
        model: "unused".into(),
        base_url: "http://127.0.0.1:1".into(),
        endpoint: "http://127.0.0.1:1/chat/completions".into(),
        key: None,
        key_source: "none".into(),
        authorization_scope: None,
    };
    let cases = [
        super::legacy::EvalCase {
            id: "cleanup-off".into(),
            group: "no provider".into(),
            input: "保持 React 和 /tmp/test 原样".into(),
            expected: "保持 React 和 /tmp/test 原样".into(),
            reference_variants: vec![],
            cleanup: "off".into(),
            family: "general".into(),
            focus_kind: "unknown".into(),
            mode: "ai-off".into(),
            protected: vec!["React".into(), "/tmp/test".into()],
        },
        super::legacy::EvalCase {
            id: "explicit-snippet".into(),
            group: "no provider".into(),
            input: "fixture trigger".into(),
            expected: "第一点 保留这些字面内容\n第二点 周四，不对，周五".into(),
            reference_variants: vec![],
            cleanup: "standard".into(),
            family: "general".into(),
            focus_kind: "unknown".into(),
            mode: "snippet".into(),
            protected: vec![],
        },
    ];
    let snippet = super::legacy::prepare_cleanup_case(&cases[1]);
    assert_eq!(snippet.source(), cases[1].expected);
    assert!(snippet.finalization_context().revision_source.is_none());
    assert!(snippet.finalization_context().prepared_transcript.is_none());
    let mut resume = super::legacy::ResumeState::default();
    let mut pacer = super::legacy::RequestPacer::new(std::time::Duration::ZERO);
    let result = super::legacy::run_cleanup_candidate(
        &candidate,
        super::legacy::CleanupCandidateRun {
            cases: &cases,
            blocked_reason: None,
            resume_state: &mut resume,
            checkpoint_path: None,
            corpus_fingerprint: "declared-fixture",
            cleanup_code_fingerprint: "source-owner-fixture",
            pacer: &mut pacer,
            observer: None,
            preparation: None,
            replay: None,
        },
    )
    .await
    .expect("shared cleanup evaluator");
    assert_eq!(result.provider_http_attempts, 0);
    assert_eq!(result.provider_invoked_cases, 0);
    assert_eq!(result.cases.len(), 2);
    for case in &cases {
        let result = result.cases.iter().find(|row| row.id == case.id).unwrap();
        assert_eq!(result.final_text, case.expected);
        assert!(result.adapter_candidate.is_none());
        assert!(result.error_category.is_none());
    }
}

pub(super) fn journal_fixture(
    d: &FixtureDirectory,
) -> (ValidatedInputs, std::sync::Arc<super::journal::RunJournal>) {
    std::fs::write(d.0.join("one.wav"), audio()).unwrap();
    let m = speech_json("one", "one.wav");
    let p = d.write("speech.json", &m);
    let side = d.write("corpus.json", &sidecar_json(&[(p.clone(), m)]));
    let recipe = d.write("recipe.json", &recipe_json(&side));
    let inputs = validate(&p, Some(&side), Some(&recipe), None).unwrap();
    let baseline = BaselineIdentity {
        schema_version: 2,
        source_commit: Some("synthetic-source-fixture".into()),
        dirty: Some(false),
        source_sha256: Some("0".repeat(64)),
        lockfile_sha256: Default::default(),
        source_provenance: SourceProvenance::BuildScriptSnapshotV3 {},
        source_unavailable_path: None,
        source_git_unavailable_reason: None,
        live_checkout: None,
        live_checkout_end: None,
        executable_sha256: "0".repeat(64),
        version: "fixture".into(),
        os: "fixture".into(),
        architecture: "fixture".into(),
        os_version: None,
        os_build: None,
        cpu: None,
        ram_bytes: None,
    };
    let report = super::runner::build_report(&inputs, baseline).unwrap();
    let journal = super::journal::RunJournal::new(&d.0.join("run.json"), report).unwrap();
    (inputs, journal)
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn dropped_local_transcribe_publishes_owner_wav_cleanup_after_shutdown() {
    use super::contracts::{AttemptStage, StageState};
    use crate::observations::ObservedText;

    let directory = FixtureDirectory::new();
    let (inputs, journal) = journal_fixture(&directory);
    let job = &inputs.recipe.as_ref().unwrap().jobs[0];
    let publisher = super::runner::CaptureJournal::new(&journal, 0, job, false);
    journal.update(0, |row| {
        row.attempts = 1;
        row.attempt_facts
            .push(super::execution::fact(AttemptStage::Asr, 1));
        row.state = StageState::Cancelled {
            reason: "sigint".into(),
        };
    });

    crate::ondevice_runtime::dropped_public_transcribe_capture(publisher, false)
        .await
        .expect("runner defers publication through public caller drop and actual owner shutdown");
    let row = journal.row(0);
    let local = row
        .local_runtime
        .expect("final owner facts published to the same row");
    let actions = local
        .private_audio_resources
        .iter()
        .map(|fact| fact.action.as_str())
        .collect::<Vec<_>>();
    assert!(actions.contains(&"created"));
    assert!(actions.contains(&"removed_owned_file"));
    assert_eq!(row.attempts, 1);
    assert_eq!(row.attempt_facts.len(), 1);
    assert!(matches!(row.state, StageState::Cancelled { ref reason } if reason == "sigint"));
    assert!(matches!(row.filtered_text, ObservedText::Unknown { .. }));
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn dropped_local_transcribe_reports_unsettled_bounded_shutdown() {
    use super::contracts::{AttemptStage, StageState};
    let directory = FixtureDirectory::new();
    let (inputs, journal) = journal_fixture(&directory);
    let job = &inputs.recipe.as_ref().unwrap().jobs[0];
    let publisher = super::runner::CaptureJournal::new(&journal, 0, job, false);
    journal.update(0, |row| {
        row.attempts = 1;
        row.attempt_facts
            .push(super::execution::fact(AttemptStage::Asr, 1));
        row.state = StageState::Cancelled {
            reason: "sigint".into(),
        };
    });
    crate::ondevice_runtime::dropped_public_transcribe_capture(publisher, true)
        .await
        .expect(
            "held retirement reports bounded shutdown as unsettled, then actual owner is reaped",
        );
    let row = journal.row(0);
    assert!(row
        .diagnostics
        .iter()
        .any(|value| value == "local_shutdown_unsettled"));
    assert_eq!(row.attempts, 1);
    assert_eq!(row.attempt_facts.len(), 1);
    assert!(matches!(row.state, StageState::Cancelled { ref reason } if reason == "sigint"));
    let error = journal.fail(
        journal.input_error(super::validation::error(
            std::path::Path::new("fixture"),
            "local_shutdown_unsettled",
            "local runtime shutdown did not settle",
        )),
        None,
    );
    assert!(
        matches!(error.terminal, StageState::Failed { ref reason } if reason == "local_shutdown_unsettled")
    );
    assert!(
        matches!(journal.row(0).state, StageState::Cancelled { ref reason } if reason == "sigint")
    );
    let local = row
        .local_runtime
        .expect("partial owner observation published");
    let actions = local
        .private_audio_resources
        .iter()
        .map(|fact| fact.action.as_str())
        .collect::<Vec<_>>();
    assert!(actions.contains(&"created"));
    assert!(!actions.contains(&"removed_owned_file"));
}
fn asr_fixture(inputs: &ValidatedInputs) -> super::legacy::AudioManifest {
    super::legacy::AudioManifest {
        cases: vec![super::legacy::AudioFixture {
            id: "one".into(),
            audio: "one.wav".into(),
            reference: "fixture".into(),
            validated_audio: Some(inputs.cases[0].bytes.clone()),
        }],
    }
}
#[test]
fn manifest_snapshot_keeps_validated_bytes_and_parsed_identity_after_path_changes() {
    let d = FixtureDirectory::new();
    std::fs::write(d.0.join("one.wav"), audio()).unwrap();
    let p = d.write("speech.json", &speech_json("one", "one.wav"));
    let inputs = validate(&p, None, None, None).unwrap();
    let selected = inputs.selection.speech_manifest().unwrap();
    std::fs::write(&p, b"{invalid replacement}").unwrap();
    assert_eq!(selected.parsed.cases[0].id, "one");
    assert_eq!(selected.sha256, sha256(&selected.raw));
    assert_eq!(inputs.cases[0].legacy_manifest_sha(), selected.sha256);
    assert_eq!(selected.selected_path, p);
    assert_eq!(inputs.selection.corpus_sha().unwrap(), selected.sha256);
}
#[tokio::test]
async fn in_flight_asr_start_is_durable_on_cancel_and_later_io_failure() {
    let d = FixtureDirectory::new();
    let (inputs, journal) = journal_fixture(&d);
    let mut server = crate::test_http::spawn_scripted(vec![crate::test_http::ResponseStep {
        status: 200,
        body: r#"{"text":"fixture"}"#.into(),
        delay: std::time::Duration::from_secs(30),
    }])
    .await;
    let candidate = super::legacy::asr_candidate(
        "fixture-qwen",
        "qwen",
        "fixture".into(),
        &server.url,
        super::authorization::KeyMaterial::synthetic("fixture-key"),
    );
    let fixtures = asr_fixture(&inputs);
    let observer = journal.observer(0);
    {
        let future = super::legacy::run_asr_candidate(
            &candidate,
            inputs.cases[0].document_path(),
            &fixtures,
            None,
            Some(&observer),
        );
        tokio::pin!(future);
        tokio::select! {
            _ = &mut future => panic!("adapter completed before controlled cancellation"),
            receipt = server.receipts.recv() => assert_eq!(receipt, Some(1)),
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => panic!("no local request receipt"),
        }
        let durable: RunRecord =
            serde_json::from_slice(&std::fs::read(d.0.join("run.json")).unwrap()).unwrap();
        assert_eq!(durable.provider_attempts, 1);
        assert_eq!(durable.cases[0].attempts, 1);
        assert!(matches!(
            durable.cases[0].attempt_facts[0].server_received,
            StageState::Unknown { .. }
        ));
    }
    journal.update(0, |row| {
        row.cancellation_requested = true;
        row.state = StageState::Cancelled {
            reason: "fixture_cancel".into(),
        };
    });
    journal.terminal(StageState::Cancelled {
        reason: "fixture_cancel".into(),
    });
    journal.persist().unwrap();
    assert_eq!(journal.snapshot().provider_attempts, 1);
    // A post-dispatch input failure retains accumulated facts even when the next owned write fails.
    let temp = d.0.join(format!("run.tmp-{}", std::process::id()));
    std::fs::write(&temp, b"caller sentinel").unwrap();
    let failure = journal.fail(
        journal.input_error(error(Path::new("missing.wav"), "io", "fixture I/O failure")),
        Some(0),
    );
    assert_eq!(failure.provider_attempts, 1);
    assert_eq!(failure.durable_provider_attempts, 1);
    assert!(failure.durable_record.is_some());
    assert!(!failure.artifact_failures.is_empty());
    assert_eq!(std::fs::read(temp).unwrap(), b"caller sentinel");
}
#[tokio::test]
async fn logical_qwen_and_custom_cleanup_share_only_the_selected_authorization_scope() {
    let d = FixtureDirectory::new();
    let (inputs, _) = journal_fixture(&d);
    for status in [401, 403] {
        let server = crate::test_http::spawn_scripted(vec![crate::test_http::ResponseStep {
            status,
            body: "{}".into(),
            delay: std::time::Duration::ZERO,
        }])
        .await;
        let key = super::authorization::KeyMaterial::synthetic("selected-key");
        let asr = super::legacy::asr_candidate(
            "fixture-qwen",
            "qwen",
            "fixture".into(),
            &server.url,
            key.clone(),
        );
        let cleanup = super::legacy::CleanupCandidate::from_material(
            "fixture-custom",
            crate::engine::EngineProvider::Custom,
            "fixture".into(),
            server.url.clone(),
            &key,
        );
        assert_eq!(asr.authorization_scope, cleanup.authorization_scope);
        let response = super::legacy::run_asr_candidate(
            &asr,
            inputs.cases[0].document_path(),
            &asr_fixture(&inputs),
            None,
            None,
        )
        .await
        .unwrap();
        let mut blocks = super::authorization::AuthorizationBlocks::default();
        blocks.observe(
            asr.authorization_scope.as_ref(),
            response.stop_category.as_deref(),
        );
        assert_eq!(
            blocks
                .reason(cleanup.authorization_scope.as_ref())
                .as_deref(),
            Some("authorization_error")
        );
        let other_key = super::legacy::CleanupCandidate::from_material(
            "other-key",
            crate::engine::EngineProvider::Custom,
            "fixture".into(),
            server.url.clone(),
            &super::authorization::KeyMaterial::synthetic("different-key"),
        );
        assert!(blocks
            .reason(other_key.authorization_scope.as_ref())
            .is_none());
        let other_origin = super::legacy::CleanupCandidate::from_material(
            "other-origin",
            crate::engine::EngineProvider::Custom,
            "fixture".into(),
            "http://127.0.0.1:1".into(),
            &super::authorization::KeyMaterial::synthetic("selected-key"),
        );
        assert!(blocks
            .reason(other_origin.authorization_scope.as_ref())
            .is_none());
        let fixtures = asr_fixture(&inputs);
        let blocked = super::legacy::run_asr_candidate(
            &asr,
            inputs.cases[0].document_path(),
            &fixtures,
            blocks.reason(asr.authorization_scope.as_ref()),
            None,
        )
        .await
        .unwrap();
        assert_eq!(blocked.provider_invoked_cases, 0);
        let cases = [super::legacy::EvalCase {
            id: "one".into(),
            group: "fixture".into(),
            input: "please keep this original sentence".into(),
            expected: "please keep this original sentence".into(),
            reference_variants: vec![],
            cleanup: "standard".into(),
            family: "general".into(),
            focus_kind: "editable".into(),
            mode: "normal".into(),
            protected: vec![],
        }];
        let mut resume = super::legacy::ResumeState::default();
        let mut pacer = super::legacy::RequestPacer::new(std::time::Duration::ZERO);
        let blocked_cleanup = super::legacy::run_cleanup_candidate(
            &cleanup,
            super::legacy::CleanupCandidateRun {
                cases: &cases,
                blocked_reason: blocks.reason(cleanup.authorization_scope.as_ref()),
                resume_state: &mut resume,
                checkpoint_path: None,
                corpus_fingerprint: "fixture",
                cleanup_code_fingerprint: "fixture",
                pacer: &mut pacer,
                observer: None,
                preparation: None,
                replay: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(blocked_cleanup.provider_http_attempts, 0);
        assert_eq!(
            blocked_cleanup.stop_category.as_deref(),
            Some("authorization_error")
        );
    }
}

#[tokio::test]
async fn cleanup_retry_start_survives_dropping_the_in_flight_adapter() {
    let d = FixtureDirectory::new();
    let (_, journal) = journal_fixture(&d);
    let mut server = crate::test_http::spawn_scripted(vec![
        crate::test_http::ResponseStep {
            status: 429,
            body: "{}".into(),
            delay: std::time::Duration::ZERO,
        },
        crate::test_http::ResponseStep {
            status: 200,
            body: r#"{"choices":[{"message":{"content":"fixture"}}]}"#.into(),
            delay: std::time::Duration::from_secs(30),
        },
    ])
    .await;
    let candidate = super::legacy::CleanupCandidate::from_material(
        "fixture-cleanup",
        crate::engine::EngineProvider::Custom,
        "fixture".into(),
        server.url.clone(),
        &super::authorization::KeyMaterial::synthetic("fixture-key"),
    );
    let cases = [super::legacy::EvalCase {
        id: "one".into(),
        group: "fixture".into(),
        input: "please keep this original sentence".into(),
        expected: "please keep this original sentence".into(),
        reference_variants: vec![],
        cleanup: "standard".into(),
        family: "general".into(),
        focus_kind: "editable".into(),
        mode: "normal".into(),
        protected: vec![],
    }];
    let mut resume = super::legacy::ResumeState::default();
    let mut pacer = super::legacy::RequestPacer::new(std::time::Duration::ZERO);
    let observer = journal.observer(0);
    {
        let future = super::legacy::run_cleanup_candidate(
            &candidate,
            super::legacy::CleanupCandidateRun {
                cases: &cases,
                blocked_reason: None,
                resume_state: &mut resume,
                checkpoint_path: None,
                corpus_fingerprint: "fixture",
                cleanup_code_fingerprint: "fixture",
                pacer: &mut pacer,
                observer: Some(&observer),
                preparation: None,
                replay: None,
            },
        );
        tokio::pin!(future);
        let receipt = async {
            assert_eq!(server.receipts.recv().await, Some(1));
            assert_eq!(server.receipts.recv().await, Some(2));
        };
        tokio::select! {
            _ = &mut future => panic!("adapter completed before retry cancellation"),
            _ = receipt => {},
            _ = tokio::time::sleep(std::time::Duration::from_secs(8)) => panic!("no retry request receipt"),
        }
    }
    let record = journal.snapshot();
    assert_eq!(record.provider_attempts, 2);
    assert_eq!(record.cases[0].attempts, 2);
    let facts = &record.cases[0].attempt_facts;
    assert_eq!(facts[0].ordinal, 1);
    assert!(matches!(facts[0].local_adapter, StageState::Completed {}));
    assert_eq!(facts[1].ordinal, 2);
    assert!(matches!(facts[1].local_adapter, StageState::Unknown { .. }));
    assert!(facts
        .iter()
        .all(|f| matches!(f.transport_submission, StageState::Unknown { .. })));
}

#[test]
fn recipe_control_snapshots_are_typed_immutable_and_bound_to_cases_before_credentials() {
    let d = FixtureDirectory::new();
    std::fs::write(d.0.join("one.wav"), audio()).unwrap();
    let m = speech_json("one", "one.wav");
    let p = d.write("speech.json", &m);
    let side = d.write("corpus.json", &sidecar_json(&[(p.clone(), m)]));
    let vocab=d.write("vocab.json",&serde_json::json!({"artifact_type":"vocabulary_fixture","schema_version":1,"terms":["VoiceFlow","TypeScript"]}));
    let context=d.write("context.json",&serde_json::json!({"artifact_type":"context_fixture","schema_version":1,"text":"meeting with Alex"}));
    let local=d.write("rules.json",&serde_json::json!({"artifact_type":"local_rule_fixture","schema_version":1,"revision":"voiceflow-production-v1","cases":[{"case_id":"one","dictionary":["知乎"],"replacements":[{"before":"知呼","after":"知乎"}]}]}));
    let mut recipe = recipe_json(&side);
    recipe["jobs"][0]["recognition_language"] = serde_json::json!("zh");
    recipe["jobs"][0]["vocabulary"] = serde_json::json!({"kind":"explicit","relative_path":"vocab.json","content_sha256":sha256(&std::fs::read(&vocab).unwrap())});
    recipe["jobs"][0]["context"] = serde_json::json!({"kind":"fixture","relative_path":"context.json","source_sha256":sha256(&std::fs::read(&context).unwrap()),"permission_snapshot":{"accessibility":false,"screen_recording":false,"provider_text_granted":true}});
    recipe["jobs"][0]["local_rules"] = serde_json::json!({"dictionary_replacements":true,"fuzzy":false,"spoken_layout":false,"revision":"voiceflow-production-v1","fixture":{"relative_path":"rules.json","sha256":sha256(&std::fs::read(&local).unwrap())}});
    let path = d.write("recipe.json", &recipe);
    let inputs = validate(&p, Some(&side), Some(&path), None).unwrap();
    std::fs::write(&vocab, b"{corrupt replacement}").unwrap();
    std::fs::write(&context, b"{corrupt replacement}").unwrap();
    let controls = &inputs.control_fixtures[0];
    let options = controls
        .asr_options(
            "https://api.groq.com/openai/v1/audio/transcriptions",
            "whisper-large-v3-turbo",
            &RecognitionLanguage::Zh,
        )
        .unwrap();
    assert_eq!(options.language.as_deref(), Some("zh"));
    assert_eq!(
        options.prompt.as_deref(),
        Some("meeting with Alex\nVoiceFlow\nTypeScript")
    );
    assert!(options.keywords.is_empty());
    let rules = controls.preparation(&inputs.recipe.as_ref().unwrap().jobs[0].local_rules, "one");
    let case = super::legacy::EvalCase {
        id: "one".into(),
        group: "fixture".into(),
        input: "请保存知呼 API".into(),
        expected: "unused reference".into(),
        reference_variants: vec![],
        cleanup: "off".into(),
        family: "general".into(),
        focus_kind: "editable".into(),
        mode: "ai-off".into(),
        protected: vec![],
    };
    let prepared = super::legacy::prepare_cleanup_case_with_fixture(&case, Some(&rules));
    assert!(prepared.source().contains("知乎"));
    assert!(!prepared.source().contains("unused reference"));
    assert!(!prepared
        .finalization_context()
        .promoted_pair_protections
        .is_empty());
    // A fresh validation never accepts the changed path under the old bound digest.
    assert_eq!(
        validate(&p, Some(&side), Some(&path), None)
            .unwrap_err()
            .code,
        "sha_mismatch"
    );
}
#[test]
fn local_rule_fixture_rejects_duplicate_or_foreign_case_association() {
    let d = FixtureDirectory::new();
    let (inputs, _) = journal_fixture(&d);
    // Match the canonical recipe-owner boundary used by public validation.
    let recipe_owner = external(&d.0.join("recipe.json")).unwrap();
    for cases in [
        serde_json::json!([{"case_id":"other","dictionary":[],"replacements":[]}]),
        serde_json::json!([{"case_id":"one","dictionary":[],"replacements":[]},{"case_id":"one","dictionary":[],"replacements":[]}]),
    ] {
        let file=d.write("local.json",&serde_json::json!({"artifact_type":"local_rule_fixture","schema_version":1,"revision":"voiceflow-production-v1","cases":cases}));
        let mut job = inputs.recipe.as_ref().unwrap().jobs[0].clone();
        job.local_rules.fixture = Some(ControlFixtureLocator {
            relative_path: "local.json".into(),
            sha256: sha256(&std::fs::read(file).unwrap()),
        });
        assert_eq!(
            super::controls::validate(&recipe_owner, &job, &inputs.cases)
                .unwrap_err()
                .code,
            "identity_mismatch"
        );
    }
}
