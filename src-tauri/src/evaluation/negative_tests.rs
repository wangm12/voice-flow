//! Explicit negative-only inputs exercise the same admission/journal/scoring owners.
use super::{contracts::*, fixtures::FixtureDirectory, validation::*};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn fixture() -> (FixtureDirectory, PathBuf, PathBuf, Value, Value) {
    let directory = FixtureDirectory::new();
    let audio = super::tests::audio();
    std::fs::write(directory.0.join("silence.wav"), &audio).unwrap();
    let index = json!({"artifact_type":"non_speech_index","schema_version":1,"provenance":{"kind":"synthetic","note":"owned generated silence; no speech gold","reviewed":true},"cases":[{"case_id":"silence","adjacent_wav":"silence.wav","audio_sha256":sha256(&audio),"category":"silence","source_review_id":"fixture-owner-reviewed"}]});
    let selected = directory.write("negative.json", &index);
    let recipe = json!({"artifact_type":"run_recipe","schema_version":2,"recipe_id":"negative-only-fixture","input":{"kind":"non_speech","index_sha256":sha256(&std::fs::read(&selected).unwrap())},"track":"file_adapter","jobs":[{"product":"voiceflow","artifact_identity":{"version":"0.1.4","source_commit":"declared-fixture","binary_sha256":null},"candidate_id":"local_qwen3_asr_0_6b","mode":"dictation","recognition_language":"auto","decode_settings":{"temperature":null,"beam_size":null},"vocabulary":{"kind":"none"},"context":{"kind":"disabled"},"cleanup":{"intensity":"off","candidate_id":null,"capture_provider_candidate":false},"local_rules":{"dictionary_replacements":false,"fuzzy":false,"spoken_layout":false,"revision":"voiceflow-production-v1"},"profile_state_id":"declared-negative-fixture","repeats":1,"pacing":{"gap_ms":0},"run_budget":{"max_cases":200,"max_requests":200,"max_retries":0,"deadline_ms":1000}}],"gui_fixture":null,"cache_policy":{"purpose":"fresh_reliability","max_age_ms":null},"scoring":null});
    let recipe_path = directory.write("recipe-v2.json", &recipe);
    (directory, selected, recipe_path, index, recipe)
}
fn admit(recipe: &Path, index: &Path) -> Result<ValidatedInputs, InputError> {
    validate_selection(None, None, Some(recipe), Some(index))
}
fn baseline() -> BaselineIdentity {
    BaselineIdentity {
        schema_version: 2,
        source_commit: Some("explicit-test-fixture".into()),
        dirty: Some(false),
        source_sha256: Some("1".repeat(64)),
        lockfile_sha256: Default::default(),
        source_provenance: SourceProvenance::BuildScriptSnapshotV3 {},
        source_unavailable_path: None,
        source_git_unavailable_reason: None,
        live_checkout: None,
        live_checkout_end: None,
        executable_sha256: "2".repeat(64),
        version: "fixture".into(),
        os: "fixture".into(),
        architecture: "fixture".into(),
        os_version: None,
        os_build: None,
        cpu: None,
        ram_bytes: None,
    }
}
fn observation(
    raw: ObservedText,
    filtered: ObservedText,
    terminal: &str,
) -> crate::observations::AsrObservation {
    crate::observations::AsrObservation {
        provider_text: raw,
        provider_text_source_kind: Some("text_string".into()),
        filtered_text: filtered,
        origin: "local_model_output".into(),
        terminal: terminal.into(),
        filtering_reason: None,
        protocol_completion: "complete".into(),
        reported_model: ObservedText::known("explicit-fixture-model"),
    }
}
#[test]
fn standalone_negative_snapshot_and_versioned_report_have_no_phantom_speech_identity() {
    let (directory, index, recipe, _, _) = fixture();
    let inputs = admit(&recipe, &index).unwrap();
    assert!(inputs.selection.non_speech_only());
    assert!(inputs.selection.speech_manifest().is_none());
    assert!(inputs.selection.corpus_sha().is_none());
    assert!(inputs.cases[0].case.speech().is_none());
    let report = super::runner::build_report(&inputs, baseline()).unwrap();
    let encoded = serde_json::to_value(&report).unwrap();
    assert_eq!(encoded["schema_version"], 2);
    assert_eq!(encoded["identity"]["input"]["kind"], "non_speech");
    assert!(encoded["identity"].get("corpus_sha256").is_none());
    assert!(encoded["identity"].get("non_speech_sha256").is_none());
    let row = &encoded["cases"][0];
    assert_eq!(row["input_paths"]["document_kind"], "non_speech_index");
    for stage in ["prepared_text", "adapter_candidate", "final_text"] {
        assert_eq!(
            row[stage]["reason"],
            "negative_postprocessing_not_applicable"
        );
    }
    assert!(row["input_paths"].get("manifest").is_none());
    assert!(row["score_input"].get("asr_reference").is_none());
    assert!(row["score_input"].get("final_references").is_none());
    assert_eq!(row["score_input"]["corpus"], Value::Null);
    assert!(serde_json::from_value::<RunRecord>(encoded.clone()).is_ok());
    let mut wrong = encoded.clone();
    wrong["schema_version"] = json!(1);
    assert!(serde_json::from_value::<RunRecord>(wrong).is_err());
    let mut wrong = encoded;
    wrong["cases"][0]["input_paths"]["index_sha256"] = json!("0".repeat(64));
    assert!(serde_json::from_value::<RunRecord>(wrong).is_err());
    // The selected document may change after admission; the held SHA/parsed/WAV
    // facts remain one snapshot, rather than a new body under the old identity.
    let held = inputs.selection.negative_sha().unwrap().to_owned();
    std::fs::write(&index, b"{changed}").unwrap();
    let InputSelection::NonSpeech(snapshot) = &inputs.selection else {
        panic!()
    };
    assert_eq!(sha256(&snapshot.raw), held);
    assert_eq!(snapshot.parsed.cases[0].case_id, "silence");
    assert_eq!(snapshot.selected_path, index);
    assert!(admit(&recipe, &index).is_err());
    assert!(!directory.0.join("report.json").exists());
}
#[test]
fn negative_v2_rejects_unknown_duplicate_version_kind_digest_and_dormant_factors() {
    let (d, index, recipe_path, _, recipe) = fixture();
    for (pointer, value, code) in [
        ("/unexpected", json!(true), "unknown_field"),
        ("/schema_version", json!(1), "schema"),
        ("/input/kind", json!("speech"), "schema"),
        ("/input/index_sha256", json!("0".repeat(64)), "sha_mismatch"),
        (
            "/jobs/0/cleanup/candidate_id",
            json!("groq_gpt_oss_20b"),
            "not_applicable",
        ),
        (
            "/jobs/0/cleanup/intensity",
            json!("standard"),
            "not_applicable",
        ),
        (
            "/jobs/0/cleanup/capture_provider_candidate",
            json!(true),
            "not_applicable",
        ),
        (
            "/jobs/0/local_rules/spoken_layout",
            json!(true),
            "not_applicable",
        ),
        (
            "/cache_policy/purpose",
            json!("quality_replay"),
            "resume_not_applicable",
        ),
    ] {
        let mut changed = recipe.clone();
        if pointer == "/unexpected" {
            changed["unexpected"] = value;
        } else {
            *changed.pointer_mut(pointer).unwrap() = value;
        }
        d.write("recipe-v2.json", &changed);
        let error = admit(&recipe_path, &index).unwrap_err();
        assert_eq!(error.provider_attempts, 0);
        // Invalid enum kinds retain the existing strict parser schema category.
        assert_eq!(error.code, code, "{pointer}");
    }
    let raw = serde_json::to_string(&recipe).unwrap();
    std::fs::write(
        &recipe_path,
        raw.replacen(
            "\"schema_version\":2",
            "\"schema_version\":2,\"schema_version\":2",
            1,
        ),
    )
    .unwrap();
    assert!(admit(&recipe_path, &index).is_err());
    d.write("recipe-v2.json", &recipe);
    assert_eq!(
        validate_selection(
            None,
            Some(&d.0.join("unused-sidecar")),
            Some(&recipe_path),
            Some(&index)
        )
        .unwrap_err()
        .code,
        "conflicting_arguments"
    );
    assert_eq!(
        validate_selection(None, None, Some(&recipe_path), None)
            .unwrap_err()
            .code,
        "missing_argument"
    );
    let inputs = admit(&recipe_path, &index).unwrap();
    let checkpoint = d.0.join("checkpoint.json");
    for (source, target) in [
        (Some(index.as_path()), None),
        (None, Some(checkpoint.as_path())),
    ] {
        assert_eq!(
            super::resume::Admission::read(&inputs, source, target)
                .err()
                .unwrap()
                .code,
            "resume_not_applicable"
        );
    }
    assert!(!d.0.join("checkpoint.json").exists());
}
#[test]
fn all_negative_wavs_rights_and_global_ids_validate_before_any_effect() {
    let (d, index_path, recipe_path, mut index, mut recipe) = fixture();
    let good = index["cases"][0].clone();
    let mut last = good.clone();
    last["case_id"] = json!("last");
    last["adjacent_wav"] = json!("last.wav");
    index["cases"] = json!([good, last]);
    std::fs::write(d.0.join("last.wav"), b"RIFFbroken").unwrap();
    d.write("negative.json", &index);
    recipe["input"]["index_sha256"] = json!(sha256(&std::fs::read(&index_path).unwrap()));
    d.write("recipe-v2.json", &recipe);
    let error = admit(&recipe_path, &index_path).unwrap_err();
    assert_eq!(error.code, "audio_format");
    assert_eq!(error.provider_attempts, 0);
    assert!(!d.0.join("run.json").exists());
    std::fs::write(d.0.join("last.wav"), super::tests::audio()).unwrap();
    index["cases"][1]["source_review_id"] = json!("");
    d.write("negative.json", &index);
    assert_eq!(
        admit(&recipe_path, &index_path).unwrap_err().code,
        "rights_not_reviewed"
    );
    index["cases"][1]["source_review_id"] = json!("explicit-review");
    index["cases"][1]["case_id"] = json!("silence");
    d.write("negative.json", &index);
    assert_eq!(
        admit(&recipe_path, &index_path).unwrap_err().code,
        "duplicate_id"
    );
}
#[test]
fn negative_local_cleanup_off_has_zero_keys_and_cancel_preserves_unattempted_tails() {
    let (d, index, recipe, _, _) = fixture();
    let inputs = admit(&recipe, &index).unwrap();
    let report = super::runner::build_report(&inputs, baseline()).unwrap();
    let replay = super::resume::Admission::read(&inputs, None, None)
        .unwrap()
        .bind(&inputs, &report.identity)
        .unwrap();
    let journal = super::journal::RunJournal::new_with_replay(
        &d.0.join("run.json"),
        report,
        Some(replay.clone()),
    )
    .unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    assert!(
        super::runner::resolve_keys(&inputs, &cancel, &replay, &journal)
            .unwrap()
            .is_empty()
    );
    cancel.cancel();
    let error = super::runner::resolve_keys(&inputs, &cancel, &replay, &journal)
        .err()
        .expect("cancelled setup must fail before credential access");
    assert_eq!(error.code, "setup_cancelled");
    assert!(journal
        .snapshot()
        .replay
        .as_ref()
        .unwrap()
        .checkpoint_acquisition
        .is_none());
    assert!(journal
        .snapshot()
        .replay
        .as_ref()
        .unwrap()
        .checkpoint_artifact_outcome
        .is_none());
    journal.update(0, |row| {
        row.state = StageState::Cancelled {
            reason: "sigint".into(),
        };
        row.cancellation_requested = true;
    });
    journal.terminal(StageState::Cancelled {
        reason: "sigint".into(),
    });
    journal.persist().unwrap();
    let (saved, _) = json::<RunRecord>(&d.0.join("run.json")).unwrap();
    assert_eq!(saved.provider_attempts, 0);
    assert_eq!(saved.cases[0].attempts, 0);
    assert_eq!(
        saved.scoring.unwrap().jobs[0].negative.get("cancelled"),
        Some(&1)
    );
}
#[test]
fn negative_journal_measures_qualified_empty_false_output_suppression_errors_and_tail_separately() {
    let (d, index, recipe, _, mut document) = fixture();
    document["jobs"][0]["repeats"] = json!(7);
    d.write("recipe-v2.json", &document);
    let inputs = admit(&recipe, &index).unwrap();
    let report = super::runner::build_report(&inputs, baseline()).unwrap();
    let journal = super::journal::RunJournal::new(&d.0.join("outcomes.json"), report).unwrap();
    journal.update(0, |row| {
        let fact = observation(
            ObservedText::KnownEmpty {},
            ObservedText::KnownEmpty {},
            "local_complete_empty",
        );
        row.state = super::runner::empty_result_outcome(true, Some(&fact)).0;
        assert!(matches!(
            super::runner::empty_result_outcome(false, Some(&fact)).0,
            StageState::Failed { .. }
        ));
        row.asr_terminal = fact.terminal.clone();
        row.provider_text = fact.provider_text.clone();
        row.filtered_text = fact.filtered_text.clone();
        row.asr_observation = Some(fact);
        row.diagnostics
            .push("on_device_inference_unavailable".into());
    });
    journal.update(1, |row| {
        row.state = StageState::Failed {
            reason: "negative_false_output".into(),
        };
        row.asr_terminal = "speech_text".into();
        row.provider_text = ObservedText::known("false output");
        row.filtered_text = row.provider_text.clone();
    });
    journal.update(2, |row| {
        let fact = observation(
            ObservedText::known("nonempty hallucination"),
            ObservedText::KnownEmpty {},
            "locally_suppressed",
        );
        row.state = super::runner::empty_result_outcome(true, Some(&fact)).0;
        row.asr_terminal = fact.terminal.clone();
        row.provider_text = fact.provider_text.clone();
        row.filtered_text = fact.filtered_text.clone();
        row.asr_observation = Some(fact);
    });
    journal.update(3, |row| {
        row.state = StageState::Failed {
            reason: "local_coverage_incomplete".into(),
        };
        row.asr_terminal = "provider_failed".into();
    });
    journal.update(4, |row| {
        row.state = StageState::Unknown {
            reason: "empty_origin_unknown".into(),
        };
        row.filtered_text = ObservedText::KnownEmpty {};
    });
    journal.update(6, |row| {
        let fact = observation(
            ObservedText::KnownEmpty {},
            ObservedText::KnownEmpty {},
            "local_complete_empty",
        );
        row.asr_observation = Some(fact);
        row.state = StageState::Cancelled {
            reason: "sigint".into(),
        };
    });
    journal.terminal(StageState::Completed {});
    journal.persist().unwrap();
    let (saved, _) = json::<RunRecord>(&d.0.join("outcomes.json")).unwrap();
    let job = &saved.scoring.as_ref().unwrap().jobs[0];
    assert_eq!(job.workflow.requested, 7);
    assert_eq!(job.all_speech_e2e.requested, 0);
    assert_eq!(job.negative.get("requested"), Some(&7));
    for (kind, count) in [
        ("confirmed_expected_empty", 2),
        ("false_output", 1),
        ("nonempty", 1),
        ("locally_suppressed", 1),
        ("local_complete_empty", 1),
        ("unknown", 1),
        ("not_run", 1),
        ("cancelled", 1),
        ("observed_provider_nonempty", 2),
        ("observed_postfilter_nonempty", 1),
    ] {
        assert_eq!(job.negative.get(kind), Some(&count), "{kind}");
    }
    assert_eq!(job.nonempty_final_proxy_rate_all_speech.numerator, 0);
    assert_eq!(job.nonempty_final_proxy_rate_all_speech.denominator, 0);
    assert!(job.nonempty_final_proxy_rate_all_speech.value.is_none());
    assert!(saved.cases.iter().all(|c| c.cleanup_request.is_none()
        && c.attempt_facts
            .iter()
            .all(|a| a.stage != AttemptStage::Cleanup)));
    assert_eq!(
        saved.cases[0].diagnostics,
        ["on_device_inference_unavailable"]
    );
}
#[test]
fn normalized_v1_recipe_and_identity_serialize_exactly_like_old_wire_owners() {
    let d = FixtureDirectory::new();
    let (inputs, journal) = super::tests::journal_fixture(&d);
    let raw = std::fs::read(d.0.join("recipe.json")).unwrap();
    let original: RunRecipeV1 = serde_json::from_slice(&raw).unwrap();
    assert_eq!(
        serde_json::to_vec(&original).unwrap(),
        serde_json::to_vec(inputs.recipe.as_ref().unwrap()).unwrap()
    );
    let identity = journal.snapshot().identity;
    assert_eq!(
        super::resume::hash(identity.speech().unwrap()),
        super::resume::hash(&identity)
    );
    // Strict speech admission is still required; no zero-case or empty-reference
    // generator fixture can become speech gold through the V2 selector.
    let mut m: Value =
        serde_json::from_slice(&std::fs::read(d.0.join("speech.json")).unwrap()).unwrap();
    m["cases"] = json!([]);
    let path = d.write("empty.json", &m);
    assert_eq!(
        validate_selection(Some(&path), None, None, None)
            .unwrap_err()
            .code,
        "input_budget"
    );
    assert!(serde_json::from_slice::<RunRecipeV2>(&raw).is_err());
}

// Historical algorithm witness only: current checkpoints must not accept this
// whole-V1-identity namespace. Shared with the checkpoint admission regression.
pub(super) fn historical_v1_stage_input_sha(
    identity: &RunInputIdentityV1,
    row: &CaseRecordV1,
    case: &ValidatedCase,
) -> String {
    super::resume::hash(&(
        "voiceflow-stage-input-sha256-v1",
        identity,
        &row.requested,
        &case.case.legacy().expect("legacy V1 case projection"),
        &case.audio,
        &case.legacy_manifest_sha(),
        &case.source_kind,
        case.negative(),
        &row.control_input_identities
            .iter()
            .map(|(name, id)| (name, &id.sha256, id.bytes))
            .collect::<Vec<_>>(),
        CacheStage::Asr,
        None::<&str>,
    ))
}
#[test]
fn v1_mixed_negative_projection_preserves_legacy_member_and_wire_without_old_cache_keys() {
    let d = FixtureDirectory::new();
    let (_, journal) = super::tests::journal_fixture(&d);
    let audio = super::tests::audio();
    std::fs::write(d.0.join("negative.wav"), &audio).unwrap();
    let negative=d.write("negative.json",&json!({"artifact_type":"non_speech_index","schema_version":1,"provenance":{"kind":"synthetic","note":"V1 compatibility fixture","reviewed":true},"cases":[{"case_id":"negative","adjacent_wav":"negative.wav","audio_sha256":sha256(&audio),"category":"silence","source_review_id":"fixture-review"}]}));
    let inputs = validate(
        &d.0.join("speech.json"),
        Some(&d.0.join("corpus.json")),
        Some(&d.0.join("recipe.json")),
        Some(&negative),
    )
    .unwrap();
    let report =
        super::runner::build_report(&inputs, journal.snapshot().identity.baseline().clone())
            .unwrap();
    let plan = super::resume::Admission::read(&inputs, None, None)
        .unwrap()
        .bind(&inputs, &report.identity)
        .unwrap();
    let row = &report.cases[1];
    let case = &inputs.cases[1];
    let old_case = AudioCaseV1 {
        id: "negative".into(),
        audio: "negative.wav".into(),
        reference: String::new(),
        expected_final: String::new(),
        scenario: "negative:Silence".into(),
        family: "general".into(),
        focus_kind: "unknown".into(),
        cleanup: "off".into(),
        protected: vec![],
        reference_variants: vec![],
    };
    // Input/member compatibility is independent of a deliberately new cache
    // namespace. Keep the exact historical negative bridge and audio SHA.
    assert!(case.negative() && case.case.speech().is_none());
    assert_eq!(
        serde_json::to_vec(&case.case.legacy().unwrap()).unwrap(),
        serde_json::to_vec(&old_case).unwrap()
    );
    assert_eq!(case.legacy_manifest_sha(), sha256(&audio));
    assert_eq!(row.audio.audio_sha256, sha256(&audio));
    let old_stage = historical_v1_stage_input_sha(report.identity.speech().unwrap(), row, case);
    assert_ne!(
        plan.input_sha(&inputs, row, CacheStage::Asr, None),
        old_stage
    );
    assert_eq!(
        serde_json::to_value(&row.input_paths).unwrap()["manifest"],
        external(&negative).unwrap().display().to_string()
    );
    assert_eq!(
        serde_json::to_value(row.score_input.as_ref().unwrap()).unwrap()["asr_reference"],
        ""
    );
}
#[cfg(unix)]
#[test]
fn standalone_negative_path_escape_and_index_unknown_fields_stay_strict() {
    let (d, index, recipe, mut document, _) = fixture();
    let foreign = FixtureDirectory::new();
    std::fs::write(foreign.0.join("outside.wav"), super::tests::audio()).unwrap();
    std::os::unix::fs::symlink(foreign.0.join("outside.wav"), d.0.join("linked.wav")).unwrap();
    document["cases"][0]["adjacent_wav"] = json!("linked.wav");
    d.write("negative.json", &document);
    assert_eq!(admit(&recipe, &index).unwrap_err().code, "path_escape");
    document["cases"][0]["adjacent_wav"] = json!("silence.wav");
    document["cases"][0]["reference"] = json!("invented speech");
    d.write("negative.json", &document);
    assert_eq!(admit(&recipe, &index).unwrap_err().code, "unknown_field");
}

#[test]
fn independent_negative_index_keeps_existing_one_to_two_hundred_admission_bound() {
    let (d, index, recipe, mut document, mut selected) = fixture();
    let case = document["cases"][0].clone();
    for count in [0, 200, 201] {
        document["cases"] = Value::Array(
            (0..count)
                .map(|i| {
                    let mut row = case.clone();
                    row["case_id"] = json!(format!("declared-negative-{i}"));
                    row
                })
                .collect(),
        );
        d.write("negative.json", &document);
        selected["input"]["index_sha256"] = json!(sha256(&std::fs::read(&index).unwrap()));
        d.write("recipe-v2.json", &selected);
        let result = admit(&recipe, &index);
        if count == 200 {
            assert_eq!(result.unwrap().cases.len(), 200);
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.code, "input_budget");
            assert_eq!(error.provider_attempts, 0);
        }
    }
}
#[tokio::test]
async fn public_negative_run_rejects_last_corrupt_wav_before_local_setup_or_journal() {
    let (d, index, recipe, mut document, mut selected) = fixture();
    let mut last = document["cases"][0].clone();
    last["case_id"] = json!("last");
    last["adjacent_wav"] = json!("last.wav");
    document["cases"].as_array_mut().unwrap().push(last);
    d.write("negative.json", &document);
    std::fs::write(d.0.join("last.wav"), b"RIFFbroken").unwrap();
    selected["input"]["index_sha256"] = json!(sha256(&std::fs::read(&index).unwrap()));
    d.write("recipe-v2.json", &selected);
    let out = d.0.join("new-run.json");
    let args = vec![
        "--experiment".into(),
        recipe.display().to_string(),
        "--non-speech".into(),
        index.display().to_string(),
        "--run".into(),
        "--out".into(),
        out.display().to_string(),
    ];
    // No source/work root is supplied. This cannot invoke a real local model,
    // even if a future ordering regression reaches local admission first.
    let error = super::main(args).await.unwrap_err();
    assert_eq!(error.code(), "audio_format");
    assert_eq!(error.provider_attempts(), 0);
    assert!(!out.exists());
}
