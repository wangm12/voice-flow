use super::super::{
    fixtures::FixtureDirectory, journal::RunJournal, runner, tests::journal_fixture,
};
use super::*;

#[test]
fn filter_clopper_pearson_reference_values() {
    for (k, n, expected) in [
        (0, 1, 0.975),
        (0, 2, 0.841886116991581),
        (1, 2, 0.9874208829065749),
        (0, 300, 0.012220974694293554),
        (0, 367, 0.010001095732145986),
        (0, 368, 0.009974054826946353),
        (300, 300, 1.0),
    ] {
        let actual = filter_clopper_pearson_upper(k, n).unwrap();
        assert!((actual - expected).abs() < 1e-10, "{k}/{n}: {actual}");
    }
    assert!(filter_clopper_pearson_upper(0, 0).is_none());
    assert!(filter_clopper_pearson_upper(2, 1).is_none());
    let upper = filter_clopper_pearson_upper(1000, 2000).unwrap();
    let expected = 0.5221494457688903;
    assert!((upper - expected).abs() < 1e-10, "{upper}");
    let lower = 1.0 - upper;
    assert!(lower < 0.5 && 0.5 < upper, "{lower}, {upper}");
}

#[test]
fn filter_delta_bootstrap_preserves_signed_constant_samples() {
    let groups = BTreeMap::from([(
        "s".to_string(),
        BTreeMap::from([("x".to_string(), vec![-0.5, -0.5])]),
    )]);
    let result = filter_delta_bootstrap_with(&groups, |_| 0);
    assert_eq!(result, vec![-0.5]);
}

#[test]
fn filter_delta_bootstrap_keeps_repeated_session_occurrences() {
    let groups = BTreeMap::from([
        (
            "A".to_string(),
            BTreeMap::from([
                ("x".to_string(), vec![0.0, 0.0, 0.0]),
                ("y".to_string(), vec![1.0]),
                ("z".to_string(), vec![0.5]),
            ]),
        ),
        (
            "B".to_string(),
            BTreeMap::from([("x".to_string(), vec![-1.0])]),
        ),
    ]);
    let mut indices = [0, 0, 0, 1].into_iter();
    let a_only = filter_delta_bootstrap_with(
        &BTreeMap::from([("A".to_string(), groups["A"].clone())]),
        |_| indices.next().unwrap(),
    );
    assert_eq!(a_only, vec![1.0 / 7.0]);
    let point = (-7.0_f64) / 20.0;
    assert_eq!(point, -0.35);
    let mut indices = [0, 0, 1, 2].into_iter();
    let repeated = filter_delta_bootstrap_with(
        &BTreeMap::from([("A".to_string(), groups["A"].clone())]),
        |_| indices.next().unwrap(),
    );
    assert_eq!(repeated, vec![0.3]);
}

fn task73_observation(
    case_id: &str,
    repeat: u32,
    speaker_id: Option<&str>,
    session_id: Option<&str>,
    delta: Option<f64>,
    state: &'static str,
) -> FilterDeltaObservation {
    FilterDeltaObservation {
        case_id: case_id.to_string(),
        repeat,
        speaker_id: speaker_id.map(str::to_string),
        session_id: session_id.map(str::to_string),
        delta,
        state,
    }
}

#[test]
fn task73_filter_delta_interval_preserves_sign_and_classifies_boundaries() {
    let positive = filter_delta_interval(&[task73_observation(
        "a",
        0,
        Some("A"),
        Some("x"),
        Some(0.5),
        "observed",
    )]);
    assert_eq!(positive.point_estimate, Some(0.5));
    assert_eq!(positive.percentile_95, Some([0.5, 0.5]));

    let negative = filter_delta_interval(&[task73_observation(
        "a",
        0,
        Some("A"),
        Some("x"),
        Some(-0.5),
        "observed",
    )]);
    assert_eq!(negative.point_estimate, Some(-0.5));
    assert_eq!(negative.percentile_95, Some([-0.5, -0.5]));

    let empty = filter_delta_interval(&[]);
    assert_eq!(empty.point_estimate, None);
    assert_eq!(empty.percentile_95, None);

    let all_na = filter_delta_interval(&[task73_observation(
        "a",
        0,
        None,
        None,
        None,
        "not_applicable",
    )]);
    assert_eq!(all_na.state, "not_applicable");
    assert_eq!(all_na.point_estimate, None);
    assert_eq!(all_na.percentile_95, None);

    let mixed = filter_delta_interval(&[
        task73_observation("a", 0, Some("A"), Some("x"), Some(0.5), "observed"),
        task73_observation("b", 0, None, None, None, "unknown"),
    ]);
    assert_eq!(mixed.requested_count, 2);
    assert_eq!(mixed.point_estimate, None);

    for invalid in [
        task73_observation("a", 0, None, Some("x"), Some(0.5), "observed"),
        task73_observation("a", 0, Some("A"), None, Some(0.5), "observed"),
        task73_observation("a", 0, Some("A"), Some("x"), Some(f64::NAN), "observed"),
    ] {
        let result = filter_delta_interval(&[invalid]);
        assert_eq!(result.unknown_count, 1);
        assert_eq!(result.point_estimate, None);
    }

    let duplicate_states = [
        ("observed", Some("A"), Some("x"), Some(0.5)),
        ("not_applicable", None, None, None),
    ];
    let duplicate = filter_delta_interval(&[
        task73_observation(
            "a",
            0,
            duplicate_states[0].1,
            duplicate_states[0].2,
            duplicate_states[0].3,
            duplicate_states[0].0,
        ),
        task73_observation(
            "a",
            0,
            duplicate_states[1].1,
            duplicate_states[1].2,
            duplicate_states[1].3,
            duplicate_states[1].0,
        ),
    ]);
    assert_eq!(duplicate.requested_count, 2);
    assert_eq!(duplicate.unknown_count, 2);
    assert_eq!(duplicate.point_estimate, None);
}

#[test]
fn filter_delta_interval_uses_real_speaker_and_session_weights() {
    let observations = vec![
        task73_observation("a", 0, Some("A"), Some("x"), Some(-1.0), "observed"),
        task73_observation("a", 1, Some("A"), Some("x"), Some(1.0), "observed"),
        task73_observation("b", 0, Some("A"), Some("x"), Some(0.0), "observed"),
        task73_observation("c", 0, Some("A"), Some("x"), Some(0.0), "observed"),
        task73_observation("d", 0, Some("A"), Some("y"), Some(1.0), "observed"),
        task73_observation("e", 0, Some("A"), Some("z"), Some(0.5), "observed"),
        task73_observation("f", 0, Some("B"), Some("x"), Some(-1.0), "observed"),
    ];

    let result = filter_delta_interval(&observations);

    assert_eq!(result.requested_count, 7);
    assert_eq!(result.case_count, 6);
    assert_eq!(result.speaker_count, 2);
    assert_eq!(result.session_count, 4);
    assert_eq!(result.state, "observed");
    assert_eq!(result.upper_bound, "finite");
    assert!((result.point_estimate.unwrap() - (-7.0 / 20.0)).abs() < 1e-12);
    let interval = result.percentile_95.unwrap();
    assert!((interval[0] - (-1.0)).abs() < 1e-12);
    assert!((interval[1] - (37.0 / 60.0)).abs() < 1e-12);
    assert_eq!(result.bootstrap_replicates, 10_000);
    assert_eq!(result.seed, 19);
}

fn fixture() -> (FixtureDirectory, ValidatedInputs, RunRecord) {
    let directory = FixtureDirectory::new();
    let (inputs, journal) = journal_fixture(&directory);
    let baseline = journal.snapshot().identity.baseline().clone();
    let report = runner::build_report(&inputs, baseline).unwrap();
    (directory, inputs, report)
}
fn paired_fixture() -> (FixtureDirectory, ValidatedInputs, RunRecord) {
    let directory = FixtureDirectory::new();
    let (_inputs, journal) = journal_fixture(&directory);
    let baseline = journal.snapshot().identity.baseline().clone();

    let recipe_path = directory.0.join("recipe.json");
    let mut recipe: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&recipe_path).unwrap()).unwrap();
    let jobs = recipe["jobs"].as_array_mut().unwrap();
    let right_job = jobs.first().cloned().unwrap();
    jobs.push(right_job);
    recipe["scoring"] = serde_json::json!({
        "method": METHOD,
        "comparison": {
            "left_job": 0,
            "right_job": 1,
            "stage": "filtered_asr",
            "cluster_by": "speaker",
            "seed": 1,
            "bootstrap_replicates": 100
        }
    });
    let paired_recipe = directory.write("paired-recipe.json", &recipe);
    let inputs = super::super::validation::validate(
        &directory.0.join("speech.json"),
        Some(&directory.0.join("corpus.json")),
        Some(&paired_recipe),
        None,
    )
    .unwrap();
    let mut report = runner::build_report(&inputs, baseline).unwrap();
    report.terminal = StageState::Completed {};
    (directory, inputs, report)
}
fn speech(row: &mut CaseRecordV1, text: &str) {
    row.provider_text = ObservedText::known(text);
    row.filtered_text = ObservedText::known(text);
    row.prepared_text = ObservedText::known(text);
    row.final_text = ObservedText::known(text);
    row.asr_terminal = "speech_text".into();
    row.state = StageState::Completed {};
    row.final_guard = StageState::Completed {};
    row.attempts = 1;
    let mut fact = super::super::execution::fact(AttemptStage::Asr, 1);
    fact.local_adapter = StageState::Completed {};
    row.attempt_facts = vec![fact];
}
fn stage(row: &CaseRecordV1, which: ScoreStage) -> &StageScore {
    row.scores
        .as_ref()
        .unwrap()
        .stages
        .iter()
        .find(|s| s.stage == which)
        .unwrap()
}

fn bound_filter_evidence(report: &RunRecord) -> InternalFilterEvidence {
    let ids = report
        .scoring
        .as_ref()
        .unwrap()
        .comparison_job_ids
        .clone()
        .unwrap();
    let left_key = report
        .cases
        .iter()
        .find(|r| {
            r.job_id == ids[0]
                && report
                    .cases
                    .iter()
                    .any(|q| q.job_id == ids[1] && q.case_id == r.case_id && q.repeat == r.repeat)
        })
        .map(|r| (r.case_id.clone(), r.repeat))
        .unwrap();
    let left_audio = report
        .cases
        .iter()
        .find(|r| r.job_id == ids[0] && r.case_id == left_key.0 && r.repeat == left_key.1)
        .unwrap()
        .audio
        .clone();
    let mut arms = Vec::new();
    for (index, job_id) in ids.iter().enumerate() {
        let row = report
            .cases
            .iter()
            .find(|r| r.job_id == *job_id && r.case_id == left_key.0 && r.repeat == left_key.1)
            .unwrap();
        let text = match &row.filtered_text {
            ObservedText::KnownEmpty {} => "",
            ObservedText::KnownValue { value } => value.as_str(),
            ObservedText::Unknown { .. } => "",
        };
        let selection = if index == 0 {
            FilterSelection::Original
        } else {
            FilterSelection::Filtered
        };
        arms.push(InternalFilterArmEvidence {
            job_id: job_id.clone(),
            case_id: row.case_id.clone(),
            repeat: row.repeat,
            selected_audio: row.audio.clone(),
            original_audio: left_audio.clone(),
            filtered_audio: (selection == FilterSelection::Filtered).then(|| row.audio.clone()),
            reference_sha256: validation::sha256(
                row.score_input
                    .as_ref()
                    .and_then(|input| input.asr_reference.as_deref())
                    .unwrap_or_default()
                    .as_bytes(),
            ),
            controls_sha256: validation::sha256(b"same-explicit-controls"),
            selected_text_sha256: validation::sha256(text.as_bytes()),
            selection,
            eligibility: Some(true),
            outcome: Some(false),
            owner_eligibility: None,
            owner_false_reject: None,
            final_review: None,
            synthetic: true,
        });
    }
    InternalFilterEvidence {
        off_job_id: Some(ids[0].clone()),
        on_job_id: Some(ids[1].clone()),
        arms,
    }
}

fn selected_pair_mutations(report: &mut RunRecord) -> InternalFilterEvidence {
    let ids = report
        .scoring
        .as_ref()
        .unwrap()
        .comparison_job_ids
        .clone()
        .unwrap();
    let key = report
        .cases
        .iter()
        .find(|r| {
            r.job_id == ids[0]
                && report
                    .cases
                    .iter()
                    .any(|q| q.job_id == ids[1] && q.case_id == r.case_id && q.repeat == r.repeat)
        })
        .map(|r| (r.case_id.clone(), r.repeat))
        .unwrap();
    report.cases.retain(|r| {
        (r.job_id == ids[0] || r.job_id == ids[1]) && r.case_id == key.0 && r.repeat == key.1
    });
    let left_audio = report
        .cases
        .iter()
        .find(|r| r.job_id == ids[0] && r.case_id == key.0 && r.repeat == key.1)
        .unwrap()
        .audio
        .clone();
    for row in &mut report.cases {
        if (row.job_id == ids[0] || row.job_id == ids[1])
            && (row.case_id != key.0 || row.repeat != key.1)
        {
            continue;
        }
        if row.job_id == ids[0] || row.job_id == ids[1] {
            let input = row.score_input.as_mut().unwrap();
            input.asr_reference = Some("the cat".into());
            input.corpus = None;
            input.asr_only = Some(super::super::speech_input::ScoreScope {
                purpose: super::super::speech_input::Purpose::PublisherReferenceAsr,
                language: "en".into(),
                reference_eligible: true,
                reference_policy: "synthetic_test".into(),
            });
            row.filtered_text = ObservedText::known(if row.job_id == ids[0] {
                "the cat"
            } else {
                "the dog"
            });
            row.final_text = ObservedText::known("speaker text");
            if row.job_id == ids[1] {
                row.audio = left_audio.clone();
            }
        }
    }
    let mut evidence = bound_filter_evidence(report);
    for arm in &mut evidence.arms {
        let text = if arm.job_id == ids[0] {
            "the cat"
        } else {
            "the dog"
        };
        arm.selected_text_sha256 = validation::sha256(text.as_bytes());
    }
    evidence
}

#[test]
fn job_proxy_keeps_legacy_v1_wire_and_marks_human_usability_unreviewed() {
    const LEGACY_JOB_V1: &str = r#"{
      "job_id":"legacy-v1","requested_cases":1,"asr_success_cases":1,
      "workflow":{"eligible":1,"requested":1,"called":1,"completed":1,"nonempty":1,"failed":0,"cancelled":0,"unknown":0,"not_applicable":0,"not_run":0,"fresh":1,"replayed":0,"fallback":0,"mixed":0,"scored":1},
      "stages":{},"stage_quality":{},
      "cleanup_asr_success_conditional":{"eligible":1,"requested":1,"called":1,"completed":1,"nonempty":1,"failed":0,"cancelled":0,"unknown":0,"not_applicable":0,"not_run":0,"fresh":1,"replayed":0,"fallback":0,"mixed":0,"scored":1},
      "all_speech_e2e":{"eligible":2,"requested":2,"called":2,"completed":1,"nonempty":1,"failed":1,"cancelled":0,"unknown":0,"not_applicable":0,"not_run":0,"fresh":2,"replayed":0,"fallback":1,"mixed":0,"scored":1},
      "usable_final_rate_all_speech":{"numerator":1,"denominator":2,"value":0.5},
      "accepted_cleanup_rate_asr_success":{"numerator":1,"denominator":1,"value":1.0},
      "negative":{},"fresh_latency_ms":{},"replay_local_latency_ms":{},"mixed_local_latency_ms":{},
      "local_adapter_starts":0,"retry_starts":0,"observed_cleanup_usage_units":{},"usage_coverage_cases":0,"unavailable":{}
    }"#;

    let legacy: JobScores = serde_json::from_str(LEGACY_JOB_V1).unwrap();
    assert_eq!(legacy.nonempty_final_proxy_rate_all_speech.numerator, 1);
    assert_eq!(legacy.nonempty_final_proxy_rate_all_speech.denominator, 2);
    assert_eq!(legacy.nonempty_final_proxy_rate_all_speech.value, Some(0.5));

    let reserialized = serde_json::to_value(&legacy).unwrap();
    assert_eq!(reserialized["usable_final_rate_all_speech"]["value"], 0.5);
    assert!(reserialized
        .get("nonempty_final_proxy_rate_all_speech")
        .is_none());

    let mut unknown: serde_json::Value = serde_json::from_str(LEGACY_JOB_V1).unwrap();
    unknown["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<JobScores>(unknown).is_err());

    let mut missing_legacy_key: serde_json::Value = serde_json::from_str(LEGACY_JOB_V1).unwrap();
    missing_legacy_key
        .as_object_mut()
        .unwrap()
        .remove("usable_final_rate_all_speech");
    assert!(serde_json::from_value::<JobScores>(missing_legacy_key).is_err());

    let mut canonical_key: serde_json::Value = serde_json::from_str(LEGACY_JOB_V1).unwrap();
    let old_rate = canonical_key
        .as_object_mut()
        .unwrap()
        .remove("usable_final_rate_all_speech")
        .unwrap();
    canonical_key["nonempty_final_proxy_rate_all_speech"] = old_rate;
    assert!(serde_json::from_value::<JobScores>(canonical_key).is_err());

    let mut nested_unknown: serde_json::Value = serde_json::from_str(LEGACY_JOB_V1).unwrap();
    nested_unknown["usable_final_rate_all_speech"]["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<JobScores>(nested_unknown).is_err());

    let (_directory, _inputs, mut report) = fixture();
    refresh_summary(&mut report);
    let output = serde_json::to_value(report.scoring.as_ref().unwrap()).unwrap();
    assert_eq!(output["method"].as_str(), Some(METHOD));
    let mut strict_record = serde_json::to_value(&report).unwrap();
    strict_record["scoring"]["jobs"][0]["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<RunRecord>(strict_record).is_err());
    let job = &output["jobs"][0];
    assert!(job.get("usable_final_rate_all_speech").is_some());
    assert!(job.get("nonempty_final_proxy_rate_all_speech").is_none());
    assert_eq!(
        job["unavailable"]["human_usable_final_rate_all_speech"]["state"],
        "unknown"
    );
    let reason = job["unavailable"]["human_usable_final_rate_all_speech"]["reason"]
        .as_str()
        .unwrap();
    assert!(reason.contains("human_task_usability_not_reviewed"));
    assert!(reason.contains("legacy_usable_final_rate_all_speech_is_nonempty_proxy_only"));
    let summary = report.scoring.as_ref().unwrap();
    assert!(matches!(
        summary.semantic_correctness,
        StageState::Unknown { .. }
    ));
    assert!(matches!(summary.human_pilot, StageState::NotRun {}));
    assert_eq!(summary.human_agreement.paired_reviewed_labels, 0);
}
#[test]
fn independent_unicode_and_language_components_have_fixed_known_distances() {
    let (_d, inputs, mut report) = fixture();
    let row = &mut report.cases[0];
    let input = row.score_input.as_mut().unwrap();
    input.asr_reference = Some("你好 hello 😀".into());
    input.final_references = Some(vec!["你好 hello 😀".into()]);
    input.corpus.as_mut().unwrap().language = Language::Mixed;
    speech(row, "你号 HELLO 😀");
    refresh_row(row);
    let scores = &stage(row, ScoreStage::FilteredAsr).text;
    assert_eq!(scores.normalized_cer.edits, Some(1));
    assert_eq!(scores.normalized_cer.reference_units, Some(8));
    assert_eq!(scores.chinese_cer.rate, Some(0.5));
    assert_eq!(scores.english_wer.rate, Some(0.0));
    assert_eq!(scores.mixed_token_error_rate.reference_units, Some(4));
    assert_eq!(scores.mixed_token_error_rate.rate, Some(0.25));
    assert!(scores.raw_scalar_cer.edits.unwrap() > 1);
    assert_eq!(inputs.cases.len(), 1);
}
#[test]
fn raw_form_and_explicit_reference_variants_are_not_interchangeable() {
    let (_d, _, mut report) = fixture();
    let row = &mut report.cases[0];
    row.score_input.as_mut().unwrap().final_references =
        Some(vec!["Hello world!".into(), "hello earth".into()]);
    speech(row, "hello earth");
    refresh_row(row);
    let scores = &stage(row, ScoreStage::Final).text;
    assert_eq!(scores.normalized_cer.rate, Some(0.0));
    assert_eq!(scores.normalized_cer.reference_variant, Some(1));
    assert!(scores.raw_scalar_cer.rate.unwrap() > 0.0);
    assert_eq!(scores.exact_accepted_reference, Some(true));
}
#[test]
fn literal_source_spans_variants_and_raw_boundaries_do_not_use_legacy_substrings() {
    let (_d, _, mut report) = fixture();
    let row = &mut report.cases[0];
    row.score_input.as_mut().unwrap().final_references = Some(vec!["Keep API v1 and Ann.".into()]);
    row.score_input.as_mut().unwrap().annotations = vec![
        LiteralAnnotation {
            id: "version".into(),
            kind: LiteralKind::Version,
            target: ReferenceTarget::Final,
            utf8_range: [9, 11],
            source: "v1".into(),
            acceptable_variants: vec![],
            authorized_transforms: vec![LiteralTransform {
                from: "v1".into(),
                to: "v2".into(),
            }],
        },
        LiteralAnnotation {
            id: "name".into(),
            kind: LiteralKind::Name,
            target: ReferenceTarget::Final,
            utf8_range: [16, 19],
            source: "Ann".into(),
            acceptable_variants: vec![],
            authorized_transforms: vec![],
        },
    ];
    speech(row, "Keep API v10 and Anna.");
    refresh_row(row);
    assert!(stage(row, ScoreStage::Final)
        .literals
        .iter()
        .all(|v| v.bounded_exact_match == Some(false)));
    speech(row, "Keep API v2 and Ann.");
    refresh_row(row);
    let literals = &stage(row, ScoreStage::Final).literals;
    assert!(literals.iter().all(|v| v.bounded_exact_match == Some(true)));
    assert_eq!(literals[0].matched_variant, Some(1));
    // Han names inside unchanged surrounding text keep their explicit anchors.
    assert!(anchored_literal(
        "请找张三讨论",
        "张三",
        "请找张三讨论",
        [6, 12]
    ));
    assert!(!anchored_literal("👍🏽", "👍", "👍", [0, 4]));
}
#[test]
fn annotation_reader_uses_canonical_owner_strict_schema_and_exact_reference_sha_before_run() {
    let (d, inputs, _) = fixture();
    let mut recipe = inputs.recipe.as_ref().unwrap().clone();
    let path = d.0.join("annotations.json");
    let mut value = serde_json::json!({"artifact_type":"scoring_annotations","schema_version":1,"revision":"review-v1","reviewed":true,"cases":[{"case_id":"one","reference_sha256":validation::sha256(inputs.cases[0].case.speech().unwrap().reference.as_bytes()),"expected_final_sha256":validation::sha256(inputs.cases[0].case.speech().unwrap().expected_final.as_bytes()),"literals":[{"id":"first-character","kind":"other","target":"asr","utf8_range":[0,3],"source":"中","acceptable_variants":[],"authorized_transforms":[]}]}]});
    let selected = d.write("annotations.json", &value);
    recipe.scoring = Some(ScoringConfig {
        method: METHOD.into(),
        annotations: Some(ControlFixtureLocator {
            relative_path: "annotations.json".into(),
            sha256: validation::sha256(&std::fs::read(&selected).unwrap()),
        }),
        comparison: None,
    });
    let owner = validation::external(&d.0.join("recipe.json")).unwrap();
    assert_eq!(
        validate(&owner, &recipe, &inputs.cases)
            .unwrap()
            .annotations
            .len(),
        1
    );
    value["cases"][0]["literals"][0]["utf8_range"] = serde_json::json!([0, 1]);
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    recipe
        .scoring
        .as_mut()
        .unwrap()
        .annotations
        .as_mut()
        .unwrap()
        .sha256 = validation::sha256(&std::fs::read(&path).unwrap());
    assert_eq!(
        validate(&owner, &recipe, &inputs.cases).err().unwrap().code,
        "scoring_literal"
    );
    value["unexpected"] = serde_json::json!(true);
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    recipe
        .scoring
        .as_mut()
        .unwrap()
        .annotations
        .as_mut()
        .unwrap()
        .sha256 = validation::sha256(&std::fs::read(&path).unwrap());
    assert_eq!(
        validate(&owner, &recipe, &inputs.cases).err().unwrap().code,
        "unknown_field"
    );
    recipe
        .scoring
        .as_mut()
        .unwrap()
        .annotations
        .as_mut()
        .unwrap()
        .sha256 = "0".repeat(64);
    assert_eq!(
        validate(&owner, &recipe, &inputs.cases).err().unwrap().code,
        "sha_mismatch"
    );
}
#[test]
fn real_journal_keeps_tail_failures_speech_empty_and_negative_empty_in_all_denominators() {
    let (d, _, mut report) = fixture();
    let first = report.cases[0].clone();
    report.cases = vec![first.clone(), first.clone(), first.clone(), first];
    for (i, row) in report.cases.iter_mut().enumerate() {
        row.case_id = format!("case-{i}");
    }
    let journal = RunJournal::new(&d.0.join("scored-run.json"), report).unwrap();
    journal.update(0, |row| {
        let text = row
            .score_input
            .as_ref()
            .unwrap()
            .final_references
            .as_ref()
            .unwrap()[0]
            .clone();
        speech(row, &text);
    });
    journal.update(1, |row| {
        speech(row, "");
        row.asr_terminal = "provider_empty".into();
        row.state = StageState::Failed {
            reason: "speech_no_usable_text".into(),
        };
        row.final_text = ObservedText::unknown("no_usable_asr");
    });
    journal.update(2, |row| {
        row.state = StageState::Failed {
            reason: "network_failure".into(),
        };
        row.asr_terminal = "provider_failed".into();
        row.attempts = 1;
        row.attempt_facts = vec![super::super::execution::fact(AttemptStage::Asr, 1)];
    });
    journal.update(3, |row| {
        let input = row.score_input.as_mut().unwrap();
        input.negative = true;
        input.asr_reference = Some(String::new());
        input.final_references = Some(vec![String::new()]);
        speech(row, "");
        row.asr_terminal = "provider_empty".into();
        row.asr_observation = Some(crate::observations::AsrObservation {
            provider_text: ObservedText::KnownEmpty {},
            provider_text_source_kind: Some("text_string".into()),
            filtered_text: ObservedText::KnownEmpty {},
            origin: "provider_response".into(),
            terminal: "provider_empty".into(),
            filtering_reason: None,
            protocol_completion: "json_complete".into(),
            reported_model: ObservedText::unknown("unknown"),
        });
    });
    journal.terminal(StageState::Failed {
        reason: "partial_fixture_failure".into(),
    });
    journal.persist().unwrap();
    let (saved, _) = validation::json::<RunRecord>(&d.0.join("scored-run.json")).unwrap();
    assert_eq!(saved.cases.len(), 4);
    let summary = saved.scoring.as_ref().unwrap();
    let job = &summary.jobs[0];
    assert_eq!(job.workflow.requested, 4);
    assert_eq!(job.workflow.failed, 2);
    assert_eq!(job.all_speech_e2e.requested, 3);
    assert_eq!(
        job.nonempty_final_proxy_rate_all_speech.value,
        Some(1.0 / 3.0)
    );
    assert_eq!(job.asr_success_cases, 1);
    assert_eq!(job.negative.get("provider_empty"), Some(&1));
    assert_eq!(
        stage(&saved.cases[1], ScoreStage::FilteredAsr)
            .text
            .normalized_cer
            .rate,
        Some(1.0)
    );
    assert!(stage(&saved.cases[3], ScoreStage::FilteredAsr)
        .text
        .normalized_cer
        .rate
        .is_none());
    assert!(matches!(
        stage(&saved.cases[2], ScoreStage::AcceptedCleanup).state,
        StageState::NotApplicable {}
    ));
    assert!(matches!(summary.human_pilot, StageState::NotRun {}));
    assert!(summary.human_agreement.kappa.is_none());
}
#[test]
fn missing_candidate_never_borrows_fallback_quality_or_conditional_success() {
    let (_d, _, mut report) = fixture();
    let row = &mut report.cases[0];
    let text = row
        .score_input
        .as_ref()
        .unwrap()
        .final_references
        .as_ref()
        .unwrap()[0]
        .clone();
    speech(row, &text);
    row.requested.cleanup.candidate_id = Some("groq_gpt_oss_20b".into());
    row.requested.cleanup.intensity = CleanupIntensity::Standard;
    row.attempts = 2;
    let mut cleanup_fact = super::super::execution::fact(AttemptStage::Cleanup, 1);
    cleanup_fact.local_adapter = StageState::Completed {};
    row.attempt_facts.push(cleanup_fact);
    row.cleanup_adapter = StageState::Failed {
        reason: "protected_token".into(),
    };
    row.state = StageState::Failed {
        reason: "protected_token".into(),
    };
    row.fallback = Some("protected_token".into());
    row.provider_text = ObservedText::unknown("raw_capture_unavailable");
    refresh_row(row);
    assert!(matches!(
        stage(row, ScoreStage::ProviderAsr).state,
        StageState::Unknown { .. }
    ));
    refresh_summary(&mut report);
    let job = &report.scoring.as_ref().unwrap().jobs[0];
    assert_eq!(job.accepted_cleanup_rate_asr_success.value, Some(0.0));
    assert_eq!(job.all_speech_e2e.nonempty, 1);
    assert_eq!(job.all_speech_e2e.failed, 1);
    assert_eq!(job.nonempty_final_proxy_rate_all_speech.numerator, 1);
    assert_eq!(job.nonempty_final_proxy_rate_all_speech.denominator, 1);
    assert_eq!(job.nonempty_final_proxy_rate_all_speech.value, Some(1.0));
    assert!(matches!(
        job.unavailable.get("human_usable_final_rate_all_speech"),
        Some(StageState::Unknown { reason }) if reason.contains("human_task_usability_not_reviewed")
    ));
    assert_eq!(job.stages["accepted_cleanup"].nonempty, 0);
    assert_eq!(
        job.stage_quality["accepted_cleanup"]
            .fresh
            .normalized_cer
            .samples,
        0
    );
    assert_eq!(job.all_speech_e2e.fallback, 1);
    assert_eq!(job.nonempty_final_proxy_rate_all_speech.value, Some(1.0));
    assert!(stage(&report.cases[0], ScoreStage::AcceptedCleanup)
        .text
        .normalized_cer
        .rate
        .is_none());
}
#[test]
fn replayed_timings_are_excluded_and_zero_sample_distributions_are_null() {
    let (_d, _, mut report) = fixture();
    let mut a = report.cases[0].clone();
    let text = a
        .score_input
        .as_ref()
        .unwrap()
        .final_references
        .as_ref()
        .unwrap()[0]
        .clone();
    speech(&mut a, &text);
    a.timings_ms.insert("asr".into(), 10.0);
    let mut b = a.clone();
    b.repeat = 1;
    b.attempts = 0;
    b.attempt_facts.clear();
    b.timings_ms.insert("asr".into(), 9999.0);
    b.timings_ms.insert("preparation".into(), 2.0);
    b.asr_replay = Some(StageReplay {
        source_local_runtime: None,
        source_run: "0".repeat(64),
        source_created_unix_ms: 1,
        request_sha256: "0".repeat(64),
        input_sha256: "0".repeat(64),
        scope: "fixture_historical".into(),
        source_checkpoint_sha256: "0".repeat(64),
        source_request: crate::observations::ResolvedRequest::new(
            "fixture",
            "http://127.0.0.1/",
            "fixture",
            "fixture",
        ),
        source_asr_observation: None,
        current_guard: StageState::NotApplicable {},
    });
    report.cases = vec![a, b];
    for row in &mut report.cases {
        refresh_row(row);
    }
    refresh_summary(&mut report);
    let job = &report.scoring.as_ref().unwrap().jobs[0];
    assert_eq!(job.fresh_latency_ms["asr"].count, 1);
    assert_eq!(job.fresh_latency_ms["asr"].p95_ms, Some(10.0));
    assert_eq!(job.replay_local_latency_ms["asr"].count, 0);
    assert!(job.replay_local_latency_ms["asr"].p50_ms.is_none());
    assert_eq!(job.replay_local_latency_ms["preparation"].p50_ms, Some(2.0));
    assert_eq!(
        job.stage_quality["filtered_asr"]
            .historical
            .normalized_cer
            .samples,
        1
    );
    assert_eq!(job.stages["final"].fresh, 1);
    assert_eq!(job.stages["final"].replayed, 1);
    assert_eq!(job.stages["final"].mixed, 0);
    let row = &mut report.cases[1];
    row.attempt_facts
        .push(super::super::execution::fact(AttemptStage::Cleanup, 1));
    refresh_row(row);
    assert_eq!(stage(row, ScoreStage::Final).denominator.called, 0);
    assert_eq!(stage(row, ScoreStage::Final).denominator.replayed, 0);
    assert_eq!(stage(row, ScoreStage::Final).denominator.mixed, 1);
    assert_eq!(stage(row, ScoreStage::Prepared).denominator.replayed, 1);
    assert_eq!(stage(row, ScoreStage::Prepared).denominator.mixed, 0);
    let zero = distribution(vec![]);
    assert_eq!(zero.count, 0);
    assert!(zero.p50_ms.is_none() && zero.p95_ms.is_none());
    assert!(rate(0, 0).value.is_none());
}
#[test]
fn paired_cluster_expectation_is_independent_of_bootstrap_draws_and_exposes_coverage() {
    let (_d, _, mut report) = fixture();
    let original = report.cases[0].clone();
    report.cases.clear();
    for job in 0..2 {
        for (case, cluster) in [("a", "speaker-a"), ("b", "speaker-a"), ("c", "speaker-b")] {
            let mut row = original.clone();
            row.case_id = case.into();
            row.requested.profile_state_id = format!("pair-profile-{job}");
            row.job_id = row.requested.id(job);
            let input = row.score_input.as_mut().unwrap();
            input.final_references = Some(vec!["ab".into()]);
            input.corpus.as_mut().unwrap().speaker_id = cluster.into();
            speech(&mut row, if job == 0 { "ab" } else { "ac" });
            refresh_row(&mut row);
            report.cases.push(row);
        }
    }
    let ids = [original.requested.id(0), original.requested.id(1)];
    let spec = ComparisonSpec {
        left_job: 0,
        right_job: 1,
        stage: ScoreStage::Final,
        cluster_by: ClusterDimension::Speaker,
        seed: 123,
        bootstrap_replicates: 100,
    };
    let stats = pair_summary(
        &report.cases,
        Some(spec.clone()),
        Some(ids.clone()),
        true,
        true,
    );
    // Every case/cluster has delta 1/2, so every bootstrap mean is 1/2.
    assert_eq!(stats.mean_right_minus_left, Some(0.5));
    assert_eq!(stats.percentile_95, Some([0.5, 0.5]));
    assert_eq!(stats.eligible_clusters, 2);
    assert_eq!(stats.eligible_cases, 3);
    assert_eq!(stats.matched_repeat_pairs, 3);
    report
        .cases
        .retain(|r| !(r.case_id == "c" && r.job_id == ids[1]));
    let partial = pair_summary(&report.cases, Some(spec), Some(ids), true, true);
    assert_eq!(partial.excluded_cases, 1);
    assert_eq!(partial.eligible_clusters, 1);
    assert!(partial.percentile_95.is_none());
    assert!(matches!(partial.state, StageState::Unknown { .. }));
}
#[test]
fn bounded_scoring_does_not_truncate_or_turn_missing_identity_into_inapplicable() {
    let (_d, _, mut report) = fixture();
    let row = &mut report.cases[0];
    row.score_input.as_mut().unwrap().corpus = None;
    speech(row, "fixture");
    refresh_row(row);
    assert!(matches!(
        stage(row, ScoreStage::Final).text.english_wer.state,
        StageState::Unknown { .. }
    ));
    speech(row, &"a".repeat(MAX_HYPOTHESIS_BYTES + 1));
    refresh_row(row);
    assert!(matches!(
        stage(row, ScoreStage::Final).text.state,
        StageState::Unknown { .. }
    ));
    assert!(stage(row, ScoreStage::Final)
        .text
        .normalized_cer
        .rate
        .is_none());
    assert!(serde_json::from_value::<ComparisonSpec>(serde_json::json!({"left_job":0,"right_job":1,"stage":"final","cluster_by":["speaker","source_session"],"seed":1,"bootstrap_replicates":100})).is_err());
}

// A full-matrix scalar oracle, deliberately different from the production bit
// representation. Small cases exercise carry boundaries without sharing code.
fn matrix_distance<T: Eq>(a: &[T], b: &[T]) -> u64 {
    let mut matrix = vec![vec![0u64; b.len() + 1]; a.len() + 1];
    for (i, row) in matrix.iter_mut().enumerate() {
        row[0] = i as u64;
    }
    for (j, value) in matrix[0].iter_mut().enumerate() {
        *value = j as u64;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            matrix[i][j] = (matrix[i - 1][j] + 1)
                .min(matrix[i][j - 1] + 1)
                .min(matrix[i - 1][j - 1] + u64::from(a[i - 1] != b[j - 1]));
        }
    }
    matrix[a.len()][b.len()]
}
#[test]
fn exact_distance_matches_independent_matrix_across_word_carries_and_edit_types() {
    for length in [0, 1, 2, 63, 64, 65, 127, 128, 129, 257] {
        let a: Vec<char> = (0..length).map(|i| ['甲', '乙', '丙'][i % 3]).collect();
        let mut candidates = vec![
            a.clone(),
            vec![],
            vec!['丁'; length],
            a.iter().rev().copied().collect(),
        ];
        let mut insertion = a.clone();
        insertion.insert(length / 2, '丁');
        candidates.push(insertion);
        if length > 0 {
            let mut deletion = a.clone();
            deletion.remove(length / 2);
            candidates.push(deletion);
            let mut substitution = a.clone();
            substitution[length / 2] = '丁';
            candidates.push(substitution);
            let mut separated = a.clone();
            separated[0] = '丁';
            separated[length - 1] = '戊';
            if length > 64 {
                separated[64] = '己';
            }
            candidates.push(separated);
            let mut shifted = a.clone();
            shifted.insert(0, '丁');
            shifted.push('戊');
            candidates.push(shifted);
        }
        for b in candidates {
            let expected = matrix_distance(&a, &b);
            assert_eq!(distance(&a, &b, &mut 0), expected, "length {length}");
            assert_eq!(
                distance(&b, &a, &mut 0),
                expected,
                "reverse length {length}"
            );
            let aw: Vec<String> = a.iter().map(|v| format!("word-{v}")).collect();
            let bw: Vec<String> = b.iter().map(|v| format!("word-{v}")).collect();
            assert_eq!(distance(&aw, &bw, &mut 0), expected, "word units {length}");
        }
    }
}
#[test]
fn admitted_long_han_and_english_keep_original_denominators_and_neighbour_errors() {
    let (_d, _, mut report) = fixture();
    let row = &mut report.cases[0];
    for length in [513, MAX_REFERENCE_BYTES / 3] {
        let reference = "甲".repeat(length);
        let input = row.score_input.as_mut().unwrap();
        input.asr_reference = Some(reference.clone());
        input.final_references = Some(vec![reference.clone()]);
        input.corpus.as_mut().unwrap().language = Language::Zh;
        let exact = text_scores(input, ScoreStage::FilteredAsr, &reference, &mut 0);
        assert_eq!(exact.chinese_cer.rate, Some(0.0));
        assert_eq!(exact.chinese_cer.reference_units, Some(length as u64));
        let mut near = reference.clone();
        near.replace_range((length / 2) * 3..(length / 2 + 1) * 3, "乙");
        let neighbour = text_scores(input, ScoreStage::FilteredAsr, &near, &mut 0);
        assert_eq!(neighbour.chinese_cer.edits, Some(1));
        assert_eq!(neighbour.chinese_cer.reference_units, Some(length as u64));
        assert_eq!(neighbour.chinese_cer.rate, Some(1.0 / length as f64));
        // No shared affix: every distinct scalar must be substituted. This also
        // exercises the full-length bit pattern, rather than just a trim path.
        let different = text_scores(input, ScoreStage::FilteredAsr, &"乙".repeat(length), &mut 0);
        assert_eq!(different.chinese_cer.edits, Some(length as u64));
        assert_eq!(different.chinese_cer.rate, Some(1.0));
        assert_eq!(different.normalized_cer.rate, Some(1.0));
    }
    let count = MAX_REFERENCE_BYTES / 5;
    let reference = "word ".repeat(count);
    let input = row.score_input.as_mut().unwrap();
    input.asr_reference = Some(reference.clone());
    input.final_references = Some(vec![reference.clone()]);
    input.corpus.as_mut().unwrap().language = Language::En;
    let mut near = reference.clone();
    near.replace_range(500..504, "bird");
    let words = text_scores(input, ScoreStage::FilteredAsr, &near, &mut 0);
    assert_eq!(words.english_wer.reference_units, Some(count as u64));
    assert_eq!(words.english_wer.edits, Some(1));
    assert_eq!(words.english_wer.rate, Some(1.0 / count as f64));
    let entirely_different = text_scores(
        input,
        ScoreStage::FilteredAsr,
        &"bird ".repeat(count),
        &mut 0,
    );
    assert_eq!(entirely_different.english_wer.rate, Some(1.0));
    let reference = "a".repeat(MAX_REFERENCE_BYTES);
    input.asr_reference = Some(reference.clone());
    input.final_references = Some(vec![reference.clone()]);
    let boundary = text_scores(
        input,
        ScoreStage::Final,
        &"b".repeat(MAX_REFERENCE_BYTES),
        &mut 0,
    );
    assert_eq!(
        boundary.raw_scalar_cer.edits,
        Some(MAX_REFERENCE_BYTES as u64)
    );
    assert_eq!(boundary.raw_scalar_cer.rate, Some(1.0));
    // Known exact final outputs remain scored in all stages using that target.
    speech(row, &reference);
    row.requested.cleanup.candidate_id = Some("groq_gpt_oss_20b".into());
    row.requested.cleanup.intensity = CleanupIntensity::Standard;
    row.requested.cleanup.capture_provider_candidate = true;
    row.adapter_candidate = ObservedText::known(&reference);
    row.cleanup_observation = Some(crate::observations::CleanupObservation {
        provider_reply: ObservedText::known(&reference),
        accepted_text: ObservedText::known(&reference),
        protocol_complete: Some(true),
        completion_kind: "stop".into(),
        adapter_decision: "accepted".into(),
        ..Default::default()
    });
    refresh_row(row);
    for which in STAGES {
        assert_eq!(stage(row, which).text.raw_scalar_cer.rate, Some(0.0));
    }
}
#[test]
fn fresh_off_local_timings_survive_zero_cleanup_calls_and_mixed_origins_stay_separate() {
    let (_d, _, mut report) = fixture();
    let row = &mut report.cases[0];
    let text = row
        .score_input
        .as_ref()
        .unwrap()
        .final_references
        .as_ref()
        .unwrap()[0]
        .clone();
    speech(row, &text);
    for (key, value) in [
        ("asr", 10.0),
        ("preparation", 2.0),
        ("finalize", 3.0),
        ("cleanup_total", 7.0),
    ] {
        row.timings_ms.insert(key.into(), value);
    }
    refresh_row(row);
    refresh_summary(&mut report);
    let job = &report.scoring.as_ref().unwrap().jobs[0];
    for (key, value) in [
        ("preparation", 2.0),
        ("finalize", 3.0),
        ("cleanup_total", 7.0),
    ] {
        assert_eq!(job.fresh_latency_ms[key].count, 1);
        assert_eq!(job.fresh_latency_ms[key].p50_ms, Some(value));
    }
    assert_eq!(job.fresh_latency_ms["cleanup_provider"].count, 0);
    assert_eq!(job.stages["accepted_cleanup"].called, 0);
    assert_eq!(job.stages["prepared"].called, 0);
    assert_eq!(job.stages["final"].called, 0);
    // Reuse the already defined provenance control, not a fabricated live time.
    let row = &mut report.cases[0];
    row.cleanup_replay = Some(StageReplay {
        source_local_runtime: None,
        source_run: "0".repeat(64),
        source_created_unix_ms: 1,
        request_sha256: "0".repeat(64),
        input_sha256: "0".repeat(64),
        scope: "fixture_historical".into(),
        source_checkpoint_sha256: "0".repeat(64),
        source_request: crate::observations::ResolvedRequest::new(
            "fixture",
            "http://127.0.0.1/",
            "fixture",
            "fixture",
        ),
        source_asr_observation: None,
        current_guard: StageState::Completed {},
    });
    row.timings_ms.insert("cleanup_provider".into(), 9999.0);
    refresh_row(row);
    refresh_summary(&mut report);
    let job = &report.scoring.as_ref().unwrap().jobs[0];
    assert_eq!(job.fresh_latency_ms["preparation"].p50_ms, Some(2.0));
    assert_eq!(job.fresh_latency_ms["cleanup_provider"].count, 0);
    assert_eq!(job.mixed_local_latency_ms["finalize"].p50_ms, Some(3.0));
    assert_eq!(
        job.mixed_local_latency_ms["cleanup_total"].p50_ms,
        Some(7.0)
    );
    assert_eq!(job.replay_local_latency_ms["finalize"].count, 0);
}

async fn observed_cleanup(
    body: String,
    capture: bool,
) -> (
    crate::observations::CleanupObservation,
    crate::llm::LlmError,
) {
    let mut server = crate::test_http::spawn_scripted(vec![crate::test_http::ResponseStep {
        status: 200,
        body,
        delay: std::time::Duration::ZERO,
    }])
    .await;
    let observer = crate::observations::FixtureCapture::new(capture);
    let result = observer
        .scope(
            crate::llm::cleanup_with_model_and_limits_and_language_and_profile_and_intent(
                &server.url,
                "fixture-model",
                "Keep API v1.",
                "synthetic-fixture-key",
                None,
                None,
                None,
                None,
                None,
                crate::llm::CleanupEffort::Standard,
                None,
            ),
        )
        .await;
    assert_eq!(server.receipts.recv().await, Some(1));
    (
        observer.snapshot().cleanup.unwrap(),
        result.expect_err("declared rejected response"),
    )
}
#[tokio::test]
async fn completed_guard_rejected_raw_reply_remains_in_quality_while_partial_controls_do_not() {
    let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
    let body="data: {\"choices\":[{\"delta\":{\"content\":\"Keep API v2.\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let (observation, error) = observed_cleanup(body.into(), true).await;
    assert!(matches!(
        error,
        crate::llm::LlmError::CleanupRejected(crate::llm::CleanupRejectionFact {
            candidate,
            reason: crate::llm::GuardReason::ProtectedToken,
            stage: crate::llm::CleanupRejectionStage::ProviderOutput,
        }) if candidate == "Keep API v2."
    ));
    assert_eq!(observation.protocol_complete, Some(true));
    assert!(
        matches!(&observation.provider_reply,ObservedText::KnownValue{value} if value=="Keep API v2.")
    );
    let (_d, _, mut report) = fixture();
    let row = &mut report.cases[0];
    let input = row.score_input.as_mut().unwrap();
    input.asr_reference = Some("Keep API v1.".into());
    input.final_references = Some(vec!["Keep API v1.".into()]);
    speech(row, "Keep API v1.");
    row.requested.cleanup.candidate_id = Some("groq_gpt_oss_20b".into());
    row.requested.cleanup.intensity = CleanupIntensity::Standard;
    row.requested.cleanup.capture_provider_candidate = true;
    row.attempt_facts
        .push(super::super::execution::fact(AttemptStage::Cleanup, 1));
    row.attempt_facts.last_mut().unwrap().local_adapter = StageState::Completed {};
    row.cleanup_adapter = StageState::Failed {
        reason: "protected_token_changed".into(),
    };
    row.state = row.cleanup_adapter.clone();
    row.fallback = Some("protected_token_changed".into());
    row.cleanup_observation = Some(observation);
    refresh_row(row);
    refresh_summary(&mut report);
    let job = &report.scoring.as_ref().unwrap().jobs[0];
    assert_eq!(
        job.stage_quality["provider_cleanup"]
            .fresh
            .normalized_cer
            .samples,
        1
    );
    assert!(
        job.stage_quality["provider_cleanup"]
            .fresh
            .normalized_cer
            .micro_rate
            .unwrap()
            > 0.0
    );
    assert_eq!(
        job.stage_quality["accepted_cleanup"]
            .fresh
            .normalized_cer
            .samples,
        0
    );
    assert_eq!(job.all_speech_e2e.fallback, 1);
    assert!(matches!(
        stage(&report.cases[0], ScoreStage::ProviderCleanup).state,
        StageState::Completed {}
    ));
    // The same full parser receives a truncated finish, tool call, invalid JSON,
    // and a bad event AFTER stop. None establishes protocol completion.
    use crate::llm::GuardReason;
    for (invalid, expected) in [
        (body.replace("stop", "length"), GuardReason::Truncated),
        ("data: {\"choices\":[{\"delta\":{\"tool_calls\":[{}]},\"finish_reason\":\"stop\"}]}\n\n".into(), GuardReason::ToolCall),
        ("data: {not-json}\n\n".into(), GuardReason::InvalidResponse),
        (format!("{}data: {{not-json}}\n\n",body.replace("data: [DONE]\n\n", "")), GuardReason::InvalidResponse),
    ] {
        let (observation, error) = observed_cleanup(invalid, true).await;
        assert!(matches!(error, crate::llm::LlmError::Guard(actual) if actual == expected));
        assert_ne!(observation.protocol_complete, Some(true));
        let row = &mut report.cases[0];
        row.cleanup_observation = Some(observation);
        refresh_row(row);
        refresh_summary(&mut report);
        assert_eq!(
            report.scoring.as_ref().unwrap().jobs[0].stage_quality["provider_cleanup"]
                .fresh
                .normalized_cer
                .samples,
            0
        );
    }
    let empty =
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let (observation, error) = observed_cleanup(empty.into(), true).await;
    assert!(matches!(
        error,
        crate::llm::LlmError::Guard(crate::llm::GuardReason::EmptyCompletion)
    ));
    assert_eq!(observation.protocol_complete, Some(true));
    assert!(matches!(
        observation.provider_reply,
        ObservedText::KnownEmpty {}
    ));
    let row = &mut report.cases[0];
    row.cleanup_observation = Some(observation);
    row.cleanup_adapter = StageState::Failed {
        reason: "empty_completion".into(),
    };
    row.state = row.cleanup_adapter.clone();
    row.fallback = Some("empty_completion".into());
    refresh_row(row);
    refresh_summary(&mut report);
    let job = &report.scoring.as_ref().unwrap().jobs[0];
    assert_eq!(
        job.stage_quality["provider_cleanup"]
            .fresh
            .normalized_cer
            .samples,
        1
    );
    assert_eq!(
        job.stage_quality["provider_cleanup"]
            .fresh
            .normalized_cer
            .micro_rate,
        Some(1.0)
    );
    assert_eq!(job.stages["provider_cleanup"].completed, 1);
    assert_eq!(job.stages["provider_cleanup"].nonempty, 0);
    assert_eq!(
        job.stage_quality["accepted_cleanup"]
            .fresh
            .normalized_cer
            .samples,
        0
    );
    assert_eq!(job.workflow.failed, 1);
    let (observation, _) = observed_cleanup(body.into(), false).await;
    assert_eq!(observation.protocol_complete, Some(true));
    let row = &mut report.cases[0];
    row.requested.cleanup.capture_provider_candidate = false;
    row.cleanup_observation = Some(observation);
    refresh_row(row);
    assert!(matches!(
        stage(row, ScoreStage::ProviderCleanup).state,
        StageState::NotApplicable {}
    ));
    row.requested.cleanup.capture_provider_candidate = true;
    row.state = StageState::Cancelled {
        reason: "fixture_cancel".into(),
    };
    refresh_row(row);
    assert!(matches!(
        stage(row, ScoreStage::ProviderCleanup).state,
        StageState::Cancelled { .. }
    ));
}

#[test]
fn unequal_repeat_case_and_cluster_weighting_has_independent_eleven_sixteenths_mean() {
    let (_d, _, mut report) = fixture();
    let original = report.cases[0].clone();
    report.cases.clear();
    for job in 0..2 {
        for (case, group) in [("a", "group-0"), ("b", "group-0"), ("c", "group-1")] {
            for repeat in 0..2 {
                let mut row = original.clone();
                row.case_id = case.into();
                row.repeat = repeat;
                row.job_id = row.requested.id(job);
                let input = row.score_input.as_mut().unwrap();
                input.final_references = Some(vec!["abcd".into()]);
                input.corpus.as_mut().unwrap().speaker_id = group.into();
                if job == 1 && repeat == 1 && case != "a" {
                    row.state = StageState::Failed {
                        reason: "fixture_tail_failure".into(),
                    };
                } else {
                    let text = if job == 0 || (case == "a" && repeat == 0) {
                        "abcd"
                    } else if case == "c" {
                        "zzzz"
                    } else {
                        "zzcd"
                    };
                    speech(&mut row, text);
                }
                refresh_row(&mut row);
                report.cases.push(row);
            }
        }
    }
    let stats = pair_summary(
        &report.cases,
        Some(ComparisonSpec {
            left_job: 0,
            right_job: 1,
            stage: ScoreStage::Final,
            cluster_by: ClusterDimension::Speaker,
            seed: 19,
            bootstrap_replicates: 100,
        }),
        Some([original.requested.id(0), original.requested.id(1)]),
        true,
        true,
    );
    // a=(0+1/2)/2=1/4; b=1/2; group0=(1/4+1/2)/2=3/8.
    // c/group1=1, and equally weighted clusters yield (3/8+1)/2=11/16.
    assert_eq!(stats.mean_right_minus_left, Some(11.0 / 16.0));
    assert_eq!(stats.requested_left_rows, 6);
    assert_eq!(stats.requested_right_rows, 6);
    assert_eq!(stats.matched_repeat_pairs, 4);
    assert_eq!(stats.eligible_cases, 3);
    assert_eq!(stats.eligible_clusters, 2);
    let workflow = derive_paired_workflow_failures(
        &report.cases,
        Some(&[original.requested.id(0), original.requested.id(1)]),
        true,
        true,
    );
    assert_eq!(workflow.requested_left_rows, 6);
    assert_eq!(workflow.requested_right_rows, 6);
    assert_eq!(workflow.union_rows, 6);
    assert_eq!(workflow.right_only_failures, 2);
    assert_eq!(workflow.classified_rows, 6);
    assert_eq!(workflow.unresolved_rows, 0);
}

#[test]
fn paired_comparison_reports_right_only_failures() {
    let (_d, _, report) = fixture();
    let original = report.cases[0].clone();
    let mut left = original.clone();
    left.case_id = "only-case".into();
    left.repeat = 0;
    left.job_id = original.requested.id(0);
    left.state = StageState::Completed {};
    let mut right = left.clone();
    right.job_id = original.requested.id(1);
    right.state = StageState::Failed {
        reason: "provider_failure".into(),
    };
    right.scores = None;

    let stats = derive_paired_workflow_failures(
        &[left, right],
        Some(&[original.requested.id(0), original.requested.id(1)]),
        true,
        true,
    );
    assert_eq!(stats.right_only_failures, 1);
    assert_eq!(stats.left_only_failures, 0);
    assert_eq!(stats.both_failures, 0);
    assert_eq!(stats.requested_left_rows, 1);
    assert_eq!(stats.requested_right_rows, 1);
    assert_eq!(stats.union_rows, 1);
    assert_eq!(stats.classified_rows, 1);
    assert_eq!(stats.coverage(), "complete");
}

#[test]
fn paired_workflow_cli_report_line_uses_the_run_record_and_marks_unknown_coverage() {
    let (_d, _, mut report) = fixture();
    let original = report.cases[0].clone();
    let ids = [original.requested.id(0), original.requested.id(1)];
    let mut left = original.clone();
    left.case_id = "cli-case".into();
    left.repeat = 0;
    left.job_id = ids[0].clone();
    left.state = StageState::Completed {};
    let mut right = left.clone();
    right.job_id = ids[1].clone();
    right.state = StageState::Failed {
        reason: "fixture_failure".into(),
    };
    right.scores = None;
    report.cases = vec![left, right];
    let summary = report.scoring.as_mut().unwrap();
    summary.comparison_spec = Some(ComparisonSpec {
        left_job: 0,
        right_job: 1,
        stage: ScoreStage::Final,
        cluster_by: ClusterDimension::Speaker,
        seed: 1,
        bootstrap_replicates: 100,
    });
    summary.comparison_job_ids = Some(ids);
    report.terminal = StageState::Completed {};

    assert_eq!(
        paired_workflow_report_line(&report).as_deref(),
        Some("Paired workflow failures: unit=case_id+repeat left_only=0 right_only=1 both=0 requested_rows=1/1 union_case_repeat_rows=1 classified=1 unresolved=0 coverage=complete")
    );

    let mut nonterminal = report.clone();
    nonterminal.terminal = StageState::Unknown {
        reason: "fixture_nonterminal".into(),
    };
    assert_eq!(
        paired_workflow_report_line(&nonterminal).as_deref(),
        Some("Paired workflow failures: unavailable=unknown reason=run_not_terminal requested_rows=1/1 union_case_repeat_rows=1 classified=0 unresolved=0 coverage=unknown")
    );

    let mut replay_excluded = report.clone();
    replay_excluded.replay = Some(ReplaySummary {
        policy: CachePolicy {
            purpose: CachePurpose::FreshQuality,
            max_age_ms: None,
        },
        checkpoint_input_sha256: None,
        checkpoint_output: None,
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
        current_quality_eligible: false,
        current_performance_eligible: false,
        current_reliability_eligible: false,
    });
    assert_eq!(
        paired_workflow_report_line(&replay_excluded).as_deref(),
        Some("Paired workflow failures: unavailable=unknown reason=replay_policy_excludes_current_workflow_classification requested_rows=1/1 union_case_repeat_rows=1 classified=0 unresolved=0 coverage=unknown")
    );

    let mut missing_identity = report.clone();
    missing_identity
        .scoring
        .as_mut()
        .unwrap()
        .comparison_job_ids = None;
    assert_eq!(
        paired_workflow_report_line(&missing_identity).as_deref(),
        Some("Paired workflow failures: unavailable=unknown reason=comparison_job_identity_missing requested_rows=0/0 union_case_repeat_rows=0 classified=0 unresolved=0 coverage=unknown")
    );

    report.cases.pop();
    let line = paired_workflow_report_line(&report).unwrap();
    assert!(line.contains("left_only=0 right_only=0 both=0"));
    assert!(line
        .contains("union_case_repeat_rows=1 classified=0 unresolved=1 coverage=partial_unknown"));

    report.scoring.as_mut().unwrap().comparison_spec = None;
    assert_eq!(paired_workflow_report_line(&report), None);
}

#[test]
fn paired_workflow_failures_classify_symmetric_both_and_fallback_states() {
    let (_d, _, report) = fixture();
    let original = report.cases[0].clone();
    let ids = [original.requested.id(0), original.requested.id(1)];
    let mut rows = Vec::new();
    for (case_id, left_state, right_state) in [
        (
            "left-only",
            StageState::Failed {
                reason: "left".into(),
            },
            StageState::Completed {},
        ),
        (
            "both",
            StageState::Failed {
                reason: "left".into(),
            },
            StageState::Failed {
                reason: "right".into(),
            },
        ),
        (
            "successful-fallback",
            StageState::Completed {},
            StageState::Failed {
                reason: "workflow".into(),
            },
        ),
        (
            "completed-control",
            StageState::Completed {},
            StageState::Completed {},
        ),
    ] {
        for (job, state) in [left_state, right_state].into_iter().enumerate() {
            let mut row = original.clone();
            row.case_id = case_id.into();
            row.repeat = 0;
            row.job_id = ids[job].clone();
            row.state = state;
            if case_id == "successful-fallback" && job == 1 {
                row.final_text = ObservedText::known("fallback text");
                row.final_guard = StageState::Completed {};
                row.fallback = Some("adapter_candidate_rejected".into());
            }
            rows.push(row);
        }
    }
    let stats = derive_paired_workflow_failures(&rows, Some(&ids), true, true);
    assert_eq!(stats.left_only_failures, 1);
    assert_eq!(stats.right_only_failures, 1);
    assert_eq!(stats.both_failures, 1);
    assert_eq!(stats.classified_rows, 4);
    assert_eq!(stats.union_rows, 4);
    assert_eq!(stats.unresolved_rows, 0);
}

#[test]
fn paired_workflow_unknown_and_missing_rows_remain_partial() {
    let (_d, _, report) = fixture();
    let original = report.cases[0].clone();
    let ids = [original.requested.id(0), original.requested.id(1)];
    let mut rows = Vec::new();
    for (case_id, state) in [
        (
            "cancelled",
            StageState::Cancelled {
                reason: "fixture".into(),
            },
        ),
        (
            "unknown",
            StageState::Unknown {
                reason: "fixture".into(),
            },
        ),
        ("not-run", StageState::NotRun {}),
        ("not-applicable", StageState::NotApplicable {}),
    ] {
        let mut row = original.clone();
        row.case_id = case_id.into();
        row.repeat = 0;
        row.job_id = ids[0].clone();
        row.state = state;
        rows.push(row);
        let mut completed_peer = original.clone();
        completed_peer.case_id = case_id.into();
        completed_peer.repeat = 0;
        completed_peer.job_id = ids[1].clone();
        completed_peer.state = StageState::Completed {};
        rows.push(completed_peer);
    }
    let mut missing_peer = original.clone();
    missing_peer.case_id = "missing-peer".into();
    missing_peer.repeat = 1;
    missing_peer.job_id = ids[1].clone();
    rows.push(missing_peer);
    let mut replayed_left = original.clone();
    replayed_left.case_id = "replayed".into();
    replayed_left.repeat = 0;
    replayed_left.job_id = ids[0].clone();
    replayed_left.state = StageState::Completed {};
    replayed_left.asr_replay = Some(StageReplay {
        source_local_runtime: None,
        source_run: "0".repeat(64),
        source_created_unix_ms: 1,
        request_sha256: "0".repeat(64),
        input_sha256: "0".repeat(64),
        scope: "fixture_historical".into(),
        source_checkpoint_sha256: "0".repeat(64),
        source_request: crate::observations::ResolvedRequest::new(
            "fixture",
            "http://127.0.0.1/",
            "fixture",
            "fixture",
        ),
        source_asr_observation: None,
        current_guard: StageState::NotApplicable {},
    });
    let mut replayed_right = replayed_left.clone();
    replayed_right.job_id = ids[1].clone();
    replayed_right.asr_replay = None;
    rows.extend([replayed_left, replayed_right]);

    let stats = derive_paired_workflow_failures(&rows, Some(&ids), true, true);
    assert_eq!(stats.union_rows, 6);
    assert_eq!(stats.classified_rows, 0);
    assert_eq!(stats.unresolved_rows, 6);
    assert_eq!(stats.coverage(), "partial_unknown");
    assert_eq!(stats.requested_left_rows, 5);
    assert_eq!(stats.requested_right_rows, 6);
    assert_eq!(
        stats.left_only_failures + stats.right_only_failures + stats.both_failures,
        0
    );

    let nonterminal = derive_paired_workflow_failures(&rows, Some(&ids), false, true);
    assert_eq!(nonterminal.unavailable_reason, Some("run_not_terminal"));
    assert_eq!(nonterminal.coverage(), "unknown");
    let replay_excluded = derive_paired_workflow_failures(&rows, Some(&ids), true, false);
    assert_eq!(
        replay_excluded.unavailable_reason,
        Some("replay_policy_excludes_current_workflow_classification")
    );
    let missing_identity = derive_paired_workflow_failures(&rows, None, true, true);
    assert_eq!(
        missing_identity.unavailable_reason,
        Some("comparison_job_identity_missing")
    );
}

#[tokio::test]
async fn cancelled_response_owner_and_missing_old_completion_fact_remain_unconfirmed() {
    let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
    let mut server=crate::test_http::spawn_scripted(vec![crate::test_http::ResponseStep{status:200,body:"data: {\"choices\":[{\"delta\":{\"content\":\"Keep API v2.\"},\"finish_reason\":\"stop\"}]}\n\n".into(),delay:std::time::Duration::from_secs(5)}]).await;
    let capture = crate::observations::FixtureCapture::new(true);
    let endpoint = server.url.clone();
    capture.scope(async {
        let pending=crate::llm::cleanup_with_model_and_limits_and_language_and_profile_and_intent(&endpoint,"fixture-model","Keep API v1.","synthetic-fixture-key",None,None,None,None,None,crate::llm::CleanupEffort::Standard,None);
        tokio::pin!(pending);
        tokio::select! {
            response=&mut pending => panic!("fixture responded before controlled cancellation: {response:?}"),
            receipt=server.receipts.recv() => assert_eq!(receipt,Some(1)),
        }
        // Drop the pending client before the delayed body; the server's owned
        // task is aborted on drop and no later response may establish a fact.
    }).await;
    drop(server);
    let observed = capture.snapshot().cleanup.unwrap();
    assert_eq!(observed.protocol_complete, None);
    let (_d, _, mut report) = fixture();
    let row = &mut report.cases[0];
    row.requested.cleanup.candidate_id = Some("groq_gpt_oss_20b".into());
    row.requested.cleanup.intensity = CleanupIntensity::Standard;
    row.requested.cleanup.capture_provider_candidate = true;
    row.cleanup_observation = Some(observed);
    row.state = StageState::Cancelled {
        reason: "fixture_cancel".into(),
    };
    refresh_row(row);
    assert!(matches!(
        stage(row, ScoreStage::ProviderCleanup).state,
        StageState::Cancelled { .. }
    ));
    // Old complete-looking records did not observe protocol_complete; neither
    // stop nor a downstream guard string retroactively fills that unknown.
    row.state = StageState::Failed {
        reason: "protected_token_changed".into(),
    };
    let old = crate::observations::CleanupObservation {
        completion_kind: "stop".into(),
        provider_reply: ObservedText::known("Keep API v2."),
        adapter_decision: "rejected_protected_token_changed".into(),
        ..Default::default()
    };
    row.cleanup_observation = Some(old);
    refresh_row(row);
    assert!(matches!(
        stage(row, ScoreStage::ProviderCleanup).state,
        StageState::Unknown { .. }
    ));
}

#[tokio::test]
async fn vf14_anthropic_content_shape_reaches_raw_quality_without_partial_acceptance() {
    use crate::llm::{GuardReason, LlmError};
    use serde_json::json;
    let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
    // Actual HTTP response -> production cleanup owner -> capture -> existing
    // row/summary refresh. This synthetic boundary is not a real-provider claim.
    for capture_reply in [true, false] {
        for (body, failure, raw) in [
            (
                json!({"content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn"}),
                None,
                Some("hello"),
            ),
            (
                json!({"content":[{"type":"text","text":""}],"stop_reason":"stop_sequence"}),
                Some(GuardReason::EmptyCompletion),
                Some(""),
            ),
            (
                json!({"stop_reason":"end_turn"}),
                Some(GuardReason::InvalidResponse),
                None,
            ),
            (
                json!({"content":null,"stop_reason":"end_turn"}),
                Some(GuardReason::InvalidResponse),
                None,
            ),
            (
                json!({"content":"hello","stop_reason":"end_turn"}),
                Some(GuardReason::InvalidResponse),
                None,
            ),
            (
                json!({"content":[7],"stop_reason":"end_turn"}),
                Some(GuardReason::InvalidResponse),
                None,
            ),
            (
                json!({"content":[{"type":"text","text":7}],"stop_reason":"end_turn"}),
                Some(GuardReason::InvalidResponse),
                None,
            ),
            (
                json!({"content":[{"type":"text","text":"hello"},{"type":"text","text":7}],"stop_reason":"end_turn"}),
                Some(GuardReason::InvalidResponse),
                None,
            ),
            (
                json!({"content":[{"type":"thinking","thinking":"synthetic internal text","signature":"synthetic signature"},{"type":"redacted_thinking","data":"synthetic opaque data"},{"type":"text","text":"hello"}],"stop_reason":"end_turn"}),
                None,
                Some("hello"),
            ),
            (
                json!({"content":[{"type":"thinking","thinking":"synthetic internal text","signature":7},{"type":"text","text":"hello"}],"stop_reason":"end_turn"}),
                Some(GuardReason::InvalidResponse),
                None,
            ),
            (
                json!({"content":[{"type":"redacted_thinking","data":"synthetic opaque data"}],"stop_reason":"end_turn"}),
                Some(GuardReason::UnavailableText),
                None,
            ),
            (
                json!({"content":[],"stop_reason":"end_turn"}),
                Some(GuardReason::UnavailableText),
                None,
            ),
            (
                json!({"content":[{"type":"text","text":"hello"},{"type":"future_content","payload":"unknown"}],"stop_reason":"end_turn"}),
                Some(GuardReason::UnsupportedContent),
                None,
            ),
            (
                json!({"content":[{"type":"text","text":"hello"},{"type":"tool_use"}],"stop_reason":"end_turn"}),
                Some(GuardReason::ToolCall),
                Some("hello"),
            ),
            (
                json!({"content":[{"type":"text","text":"hello"}],"stop_reason":"max_tokens"}),
                Some(GuardReason::Truncated),
                Some("hello"),
            ),
            (
                json!({"content":[{"type":"text","text":"hello"}],"stop_reason":null}),
                Some(GuardReason::IncompleteStop),
                Some("hello"),
            ),
        ] {
            let mut server =
                crate::test_http::spawn_scripted(vec![crate::test_http::ResponseStep {
                    status: 200,
                    body: body.to_string(),
                    delay: std::time::Duration::ZERO,
                }])
                .await;
            let observer = crate::observations::FixtureCapture::new(capture_reply);
            let result = observer
                .scope(
                    crate::llm::cleanup_with_model_and_limits_and_language_and_profile_and_intent(
                        &format!("{}/messages", server.url),
                        "fixture-model",
                        "hello",
                        "synthetic-fixture-key",
                        None,
                        None,
                        None,
                        None,
                        None,
                        crate::llm::CleanupEffort::Standard,
                        None,
                    ),
                )
                .await;
            assert_eq!(server.receipts.recv().await, Some(1));
            let captured = observer.snapshot();
            let observation = captured.cleanup.unwrap();
            let protocol_complete =
                failure.is_none() || failure == Some(GuardReason::EmptyCompletion);
            assert_eq!(
                observation.protocol_complete,
                protocol_complete.then_some(true),
                "{body}"
            );
            match (failure, &result) {
                (Some(expected), Err(LlmError::Guard(actual))) => {
                    assert_eq!(*actual, expected, "{body}")
                }
                (None, Ok((text, _))) => assert_eq!(text, "hello"),
                _ => panic!("unexpected response outcome: {result:?}; {body}"),
            }
            if failure.is_some() {
                assert!(matches!(
                    observation.accepted_text,
                    ObservedText::Unknown { .. }
                ));
            }
            if capture_reply {
                match raw {
                    Some(text) => assert_eq!(observation.provider_reply, ObservedText::known(text)),
                    None => assert!(matches!(
                        observation.provider_reply,
                        ObservedText::Unknown { .. }
                    )),
                }
            } else {
                assert!(matches!(
                    observation.provider_reply,
                    ObservedText::Unknown { .. }
                ));
            }
            let (_directory, _, mut report) = fixture();
            let row = &mut report.cases[0];
            row.score_input.as_mut().unwrap().asr_reference = Some("hello".into());
            row.score_input.as_mut().unwrap().final_references = Some(vec!["hello".into()]);
            speech(row, "hello");
            row.requested.cleanup.candidate_id = Some("groq_gpt_oss_20b".into());
            row.requested.cleanup.intensity = CleanupIntensity::Standard;
            row.requested.cleanup.capture_provider_candidate = capture_reply;
            row.attempt_facts
                .push(super::super::execution::fact(AttemptStage::Cleanup, 1));
            row.attempt_facts.last_mut().unwrap().local_adapter = StageState::Completed {};
            row.cleanup_request = captured.cleanup_request;
            row.cleanup_observation = Some(observation);
            if let Ok((text, _)) = &result {
                row.adapter_candidate = ObservedText::known(text);
                row.cleanup_adapter = StageState::Completed {};
            } else {
                row.cleanup_adapter = StageState::Failed {
                    reason: "fixture_response_rejected".into(),
                };
                row.state = row.cleanup_adapter.clone();
            }
            refresh_row(row);
            refresh_summary(&mut report);
            let job = &report.scoring.as_ref().unwrap().jobs[0];
            assert_eq!(
                job.stage_quality["provider_cleanup"]
                    .fresh
                    .normalized_cer
                    .samples,
                u64::from(capture_reply && protocol_complete),
                "{body}"
            );
            assert_eq!(
                job.stage_quality["accepted_cleanup"]
                    .fresh
                    .normalized_cer
                    .samples,
                u64::from(failure.is_none()),
                "{body}"
            );
            if failure == Some(GuardReason::EmptyCompletion) && capture_reply {
                assert_eq!(job.stages["provider_cleanup"].completed, 1);
                assert_eq!(job.stages["provider_cleanup"].nonempty, 0);
                assert_eq!(
                    job.stage_quality["provider_cleanup"]
                        .fresh
                        .normalized_cer
                        .micro_rate,
                    Some(1.0)
                );
            }
        }
    }
}

#[tokio::test]
async fn vf14_anthropic_capacity_and_dropped_response_do_not_confirm_protocol() {
    use crate::llm::{GuardReason, LlmError};
    let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
    let mut server = crate::test_http::spawn_scripted(vec![crate::test_http::ResponseStep {
        status: 200,
        body: serde_json::json!({"content":[{"type":"text","text":"x".repeat(1024 * 1024)}],"stop_reason":"end_turn"}).to_string(),
        delay: std::time::Duration::ZERO,
    }]).await;
    let capture = crate::observations::FixtureCapture::new(true);
    let result = capture
        .scope(
            crate::llm::cleanup_with_model_and_limits_and_language_and_profile_and_intent(
                &format!("{}/messages", server.url),
                "fixture-model",
                "hello",
                "synthetic-fixture-key",
                None,
                None,
                None,
                None,
                None,
                crate::llm::CleanupEffort::Standard,
                None,
            ),
        )
        .await;
    assert_eq!(server.receipts.recv().await, Some(1));
    assert!(matches!(
        result,
        Err(LlmError::Guard(GuardReason::ResponseBudget))
    ));
    assert_eq!(capture.snapshot().cleanup.unwrap().protocol_complete, None);

    let mut server = crate::test_http::spawn_scripted(vec![crate::test_http::ResponseStep {
        status: 200,
        body:
            serde_json::json!({"content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn"})
                .to_string(),
        delay: std::time::Duration::from_secs(30),
    }])
    .await;
    let capture = crate::observations::FixtureCapture::new(true);
    let base = format!("{}/messages", server.url);
    {
        let pending = capture.scope(
            crate::llm::cleanup_with_model_and_limits_and_language_and_profile_and_intent(
                &base,
                "fixture-model",
                "hello",
                "synthetic-fixture-key",
                None,
                None,
                None,
                None,
                None,
                crate::llm::CleanupEffort::Standard,
                None,
            ),
        );
        tokio::pin!(pending);
        tokio::select! {
            receipt = server.receipts.recv() => assert_eq!(receipt, Some(1)),
            result = &mut pending => panic!("delayed response unexpectedly completed: {result:?}"),
        }
        // Drop the actual in-flight future after owned HTTP receipt. No global
        // offline policy or fake cancellation parser is involved.
    }
    let observed = capture.snapshot().cleanup.unwrap();
    assert_eq!(observed.protocol_complete, None);
    assert!(matches!(
        observed.accepted_text,
        ObservedText::Unknown { .. }
    ));
}

#[test]
fn internal_filter_quality_scores_bound_selected_text_and_keeps_original_roster() {
    let (_directory, _inputs, mut report) = paired_fixture();
    let evidence = selected_pair_mutations(&mut report);
    let result = derive_filter_quality(&report, Some(&evidence));
    assert!(result.requested_off.unwrap() > 0);
    assert!(result.requested_on.unwrap() > 0);
    assert!(result.union_rows.unwrap() >= 1);
    assert_eq!(result.cer_off, Some(0.0));
    assert!(result.cer_on.unwrap() > 0.0);
    assert_eq!(result.cer_on_minus_off, Some(0.5));
    assert_eq!(result.wer_off, Some(0.0));
    // One substitution over the two reference words in "the cat" is 1/2.
    assert_eq!(result.wer_on, Some(0.5));
    assert_eq!(result.wer_on_minus_off, Some(0.5));
    assert_eq!(result.synthetic_rows, 1);
    assert!(format_filter_quality(&report, Some(&evidence))
        .unwrap()
        .starts_with("Paired filter quality:"));
}

#[test]
fn internal_filter_quality_raw_fallback_is_counted_and_uses_its_bound_text() {
    let (_directory, _inputs, mut report) = paired_fixture();
    let mut evidence = selected_pair_mutations(&mut report);
    let ids = report
        .scoring
        .as_ref()
        .unwrap()
        .comparison_job_ids
        .as_ref()
        .unwrap();
    let right = evidence
        .arms
        .iter_mut()
        .find(|a| a.job_id == ids[1])
        .unwrap();
    right.selection = FilterSelection::RawFallback;
    right.selected_audio = right.original_audio.clone();
    let right_row = report
        .cases
        .iter_mut()
        .find(|r| r.job_id == ids[1])
        .unwrap();
    right_row.audio = right.original_audio.clone();
    right_row.filtered_text = ObservedText::known("the cat");
    right.selected_text_sha256 = validation::sha256("the cat".as_bytes());
    let result = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(result.observed_rows, 1);
    assert_eq!(result.cer_on, Some(0.0));
    assert!(result.union_rows.unwrap() >= 1);
}

#[test]
fn internal_filter_quality_unknown_outcome_is_worst_case_and_unknown_eligibility_has_no_rate() {
    let (_directory, _inputs, mut report) = paired_fixture();
    let mut evidence = selected_pair_mutations(&mut report);
    evidence.arms[0].outcome = None;
    let worst = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(worst.eligible_n, Some(1));
    assert_eq!(worst.binary_errors, 1);
    assert_eq!(worst.binary_rate, Some(1.0));
    evidence.arms[1].owner_eligibility = Some(true);
    evidence.arms[1].owner_false_reject = None;
    let owner_worst = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(owner_worst.owner_n, 1);
    assert_eq!(owner_worst.owner_false_rejects, 1);
    evidence.arms[0].eligibility = None;
    let unknown = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(unknown.eligibility_unknown, 1);
    assert_eq!(unknown.eligible_n, None);
    assert_eq!(unknown.binary_rate, None);
}

#[test]
fn internal_filter_quality_final_attribution_requires_exact_final_digest_and_utf8_ranges() {
    let (_directory, _inputs, mut report) = paired_fixture();
    let mut evidence = selected_pair_mutations(&mut report);
    let job_id = evidence.arms[0].job_id.clone();
    {
        let row = report
            .cases
            .iter_mut()
            .find(|r| r.job_id == job_id)
            .unwrap();
        row.provider_text = ObservedText::known("说话人旁白");
        row.final_text = ObservedText::known("clean final");
    }
    evidence
        .arms
        .iter_mut()
        .find(|a| a.job_id == job_id)
        .unwrap()
        .final_review = Some(InternalFinalReview {
        final_text_sha256: validation::sha256("clean final".as_bytes()),
        attribution_reviewed: true,
        damage_reviewed: false,
        bystander_ranges: vec![],
        damage: vec![],
        incomplete_damage_kinds: vec![],
    });
    let removed = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(removed.final_bystander_observed, 1);
    assert_eq!(removed.final_bystander_leaks, 0);
    report
        .cases
        .iter_mut()
        .find(|r| r.job_id == job_id)
        .unwrap()
        .final_text = ObservedText::known("说话人旁白");
    evidence
        .arms
        .iter_mut()
        .find(|a| a.job_id == job_id)
        .unwrap()
        .final_review = Some(InternalFinalReview {
        final_text_sha256: validation::sha256("说话人旁白".as_bytes()),
        attribution_reviewed: true,
        damage_reviewed: true,
        bystander_ranges: vec![[9, 15]],
        damage: vec![],
        incomplete_damage_kinds: vec![],
    });
    let observed = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(observed.final_bystander_observed, 1);
    assert_eq!(observed.final_bystander_leaks, 1);
    evidence
        .arms
        .iter_mut()
        .find(|a| a.job_id == job_id)
        .unwrap()
        .final_review
        .as_mut()
        .unwrap()
        .final_text_sha256 = validation::sha256("provider text".as_bytes());
    let mismatched = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(mismatched.final_bystander_observed, 0);
}

#[test]
fn internal_filter_quality_rejects_mismatched_selected_text_and_duplicate_arm_bindings() {
    let (_directory, _inputs, mut report) = paired_fixture();
    let mut evidence = selected_pair_mutations(&mut report);
    evidence.arms[0].selected_text_sha256 = "0".repeat(64);
    let mismatch = derive_filter_quality(&report, Some(&evidence));
    assert!(mismatch.union_rows.unwrap() >= 1);
    assert_eq!(mismatch.observed_rows, 0);
    assert_eq!(mismatch.unknown_rows, 1);
    let on_job_id = evidence.on_job_id.as_deref().unwrap();
    report
        .cases
        .iter_mut()
        .find(|r| r.job_id.as_str() == on_job_id)
        .unwrap()
        .filtered_text = ObservedText::unknown("unobserved");
    evidence.arms[0].selected_text_sha256 = validation::sha256("the cat".as_bytes());
    let unknown_text = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(unknown_text.union_rows, Some(1));
    assert_eq!(unknown_text.cer_on, None);
    assert!(unknown_text.cer_unbounded);
    evidence.arms[0].selected_text_sha256 = validation::sha256("the cat".as_bytes());
    evidence.arms.push(evidence.arms[0].clone());
    let duplicate = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(duplicate.observed_rows, 0);
}

#[test]
fn internal_filter_quality_missing_peer_and_nonterminal_keep_union_unknown() {
    let (_directory, _inputs, mut report) = paired_fixture();
    let evidence = selected_pair_mutations(&mut report);
    let right_id = evidence.on_job_id.clone().unwrap();
    report.cases.retain(|r| r.job_id != right_id);
    let missing = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(missing.union_rows, Some(1));
    assert_eq!(missing.unknown_rows, 1);
    assert_eq!(missing.cer_on, None);
    report.terminal = StageState::NotRun {};
    let nonterminal = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(nonterminal.unknown_rows, 1);
    assert_eq!(nonterminal.cer_off, None);
    report.terminal = StageState::Completed {};
    report.replay = Some(ReplaySummary {
        policy: CachePolicy {
            purpose: CachePurpose::FreshQuality,
            max_age_ms: None,
        },
        checkpoint_input_sha256: None,
        checkpoint_output: None,
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
        current_quality_eligible: false,
        current_performance_eligible: false,
        current_reliability_eligible: false,
    });
    let replayed = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(replayed.unknown_rows, 1);
    assert_eq!(replayed.cer_on, None);
}

#[test]
fn internal_filter_quality_damage_annotations_are_digest_and_utf8_bound() {
    let (_directory, _inputs, mut report) = paired_fixture();
    let mut evidence = selected_pair_mutations(&mut report);
    let row = report
        .cases
        .iter_mut()
        .find(|r| r.job_id == evidence.arms[0].job_id)
        .unwrap();
    row.final_text = ObservedText::known("the dog");
    let job_id = row.job_id.clone();
    let reference_sha256 = evidence
        .arms
        .iter()
        .find(|a| a.job_id == job_id)
        .unwrap()
        .reference_sha256
        .clone();
    let final_sha = validation::sha256("the dog".as_bytes());
    evidence
        .arms
        .iter_mut()
        .find(|a| a.job_id == job_id)
        .unwrap()
        .final_review = Some(InternalFinalReview {
        final_text_sha256: final_sha.clone(),
        attribution_reviewed: false,
        damage_reviewed: true,
        bystander_ranges: vec![],
        incomplete_damage_kinds: vec![],
        damage: [
            InternalDamageKind::Head,
            InternalDamageKind::Tail,
            InternalDamageKind::Number,
            InternalDamageKind::Entity,
        ]
        .into_iter()
        .map(|kind| InternalDamageReview {
            kind,
            reference_sha256: reference_sha256.clone(),
            final_text_sha256: final_sha.clone(),
            reference_range: if kind == InternalDamageKind::Tail {
                [4, 7]
            } else {
                [0, 3]
            },
            final_range: [0, 3],
            damaged: true,
        })
        .collect(),
    });
    let bound = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(bound.damage_observed, 1);
    assert_eq!(bound.damage_by_kind_observed, [1, 1, 1, 1]);
    assert_eq!(bound.damage_count, 4);
    let review = evidence
        .arms
        .iter_mut()
        .find(|a| a.job_id == job_id)
        .unwrap()
        .final_review
        .as_mut()
        .unwrap();
    review
        .incomplete_damage_kinds
        .push(InternalDamageKind::Head);
    let partial_kind = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(partial_kind.damage_observed, 0);
    assert_eq!(partial_kind.damage_by_kind_observed, [0, 1, 1, 1]);
    assert_eq!(partial_kind.damage_by_kind_count, [0, 1, 1, 1]);
    evidence
        .arms
        .iter_mut()
        .find(|a| a.job_id == job_id)
        .unwrap()
        .final_review
        .as_mut()
        .unwrap()
        .incomplete_damage_kinds
        .clear();
    evidence
        .arms
        .iter_mut()
        .find(|a| a.job_id == job_id)
        .unwrap()
        .final_review
        .as_mut()
        .unwrap()
        .damage[0]
        .final_range = [2, 1];
    let bad_range = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(bad_range.damage_observed, 0);
    assert_eq!(bad_range.damage_by_kind_observed.iter().sum::<u64>(), 3);

    let review = evidence
        .arms
        .iter_mut()
        .find(|a| a.job_id == job_id)
        .unwrap()
        .final_review
        .as_mut()
        .unwrap();
    review.damage[0].final_range = [0, 3];
    review.damage[0].reference_sha256 = "0".repeat(64);
    let wrong_digest = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(wrong_digest.damage_by_kind_observed[0], 0);
    let review = evidence
        .arms
        .iter_mut()
        .find(|a| a.job_id == job_id)
        .unwrap()
        .final_review
        .as_mut()
        .unwrap();
    review.damage[0].reference_sha256 = reference_sha256.clone();
    let mut invalid_duplicate = review.damage[0].clone();
    invalid_duplicate.reference_range = [1, 3];
    review.damage.push(invalid_duplicate);
    let valid_plus_invalid = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(valid_plus_invalid.damage_by_kind_observed[0], 0);
    evidence
        .arms
        .iter_mut()
        .find(|a| a.job_id == job_id)
        .unwrap()
        .final_review
        .as_mut()
        .unwrap()
        .damage
        .pop();

    let multibyte = "界the dog";
    report
        .cases
        .iter_mut()
        .find(|r| r.job_id == job_id)
        .unwrap()
        .final_text = ObservedText::known(multibyte);
    let multibyte_sha = validation::sha256(multibyte.as_bytes());
    let review = evidence
        .arms
        .iter_mut()
        .find(|a| a.job_id == job_id)
        .unwrap()
        .final_review
        .as_mut()
        .unwrap();
    review.final_text_sha256 = multibyte_sha.clone();
    for damage in &mut review.damage {
        damage.final_text_sha256 = multibyte_sha.clone();
        damage.final_range = [3, 6];
    }
    review.damage[0].final_range = [1, 2];
    let invalid_utf8_boundary = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(invalid_utf8_boundary.damage_by_kind_observed[0], 0);

    report
        .cases
        .iter_mut()
        .find(|r| r.job_id == job_id)
        .unwrap()
        .final_text = ObservedText::known("");
    let empty_sha = validation::sha256(b"");
    let review = evidence
        .arms
        .iter_mut()
        .find(|a| a.job_id == job_id)
        .unwrap()
        .final_review
        .as_mut()
        .unwrap();
    review.final_text_sha256 = empty_sha.clone();
    for damage in &mut review.damage {
        damage.final_text_sha256 = empty_sha.clone();
        damage.final_range = [0, 0];
    }
    let deleted_final = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(deleted_final.damage_observed, 1);
    assert_eq!(deleted_final.damage_count, 4);
}

#[test]
fn legacy_filter_quality_formatter_keeps_union_visible_and_all_unreviewed_fields_unknown() {
    let (_directory, _inputs, report) = paired_fixture();
    assert!(paired_filter_quality_report_line(&fixture().2).is_none());
    let before = serde_json::to_value(&report).unwrap();
    let line = paired_filter_quality_report_line(&report).unwrap();
    assert!(line.contains("actual_selected_audio=unknown"));
    assert!(line.contains("cer=unknown/unknown"));
    assert!(line.contains("wer=unknown/unknown"));
    assert!(line.contains("owner_false_reject=unknown"));
    assert!(line.contains("final_bystander_leaks=unknown"));
    for field in ["head", "tail", "number", "entity"] {
        assert!(line.contains(&format!("{field}_damage_spans=unknown")));
    }
    assert!(line.contains("union_case_repeat_rows="));
    assert_eq!(before, serde_json::to_value(&report).unwrap());
    let mut no_ids = report.clone();
    no_ids.scoring.as_mut().unwrap().comparison_job_ids = None;
    let no_ids_line = paired_filter_quality_report_line(&no_ids).unwrap();
    assert!(no_ids_line.contains("actual_selected_audio=unknown"));
    let mut serialized = serde_json::to_value(&report).unwrap();
    assert!(serialized["scoring"]
        .get("internal_filter_quality")
        .is_none());
    serialized["scoring"]["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<RunRecord>(serialized).is_err());
}

#[test]
fn internal_text_metrics_keep_english_mixed_chinese_and_unknown_language_contracts() {
    let (_directory, _inputs, report) = fixture();
    let mut input = report
        .cases
        .iter()
        .find_map(|r| r.score_input.clone().filter(|input| input.corpus.is_some()))
        .unwrap();
    input.asr_reference = Some("the cat".into());
    let mut work = 0;
    let mut en = input.clone();
    en.corpus.as_mut().unwrap().language = Language::En;
    let english = text_scores(&en, ScoreStage::FilteredAsr, "the dog", &mut work);
    assert_eq!(english.english_wer.rate, Some(0.5));
    let empty = text_scores(&en, ScoreStage::FilteredAsr, "", &mut work);
    assert_eq!(empty.english_wer.edits, Some(2));
    let insertions = text_scores(&en, ScoreStage::FilteredAsr, "x x x x x x", &mut work);
    assert!(insertions.english_wer.rate.unwrap() > 1.0);
    let cer_insertions = text_scores(
        &en,
        ScoreStage::FilteredAsr,
        "xxxxxxxxxxxxxxxxxxxx",
        &mut work,
    );
    assert!(cer_insertions.normalized_cer.rate.unwrap() > 1.0);
    let mut mixed = en.clone();
    mixed.corpus.as_mut().unwrap().language = Language::Mixed;
    let mixed_scores = text_scores(&mixed, ScoreStage::FilteredAsr, "the cat 中文", &mut work);
    assert_eq!(mixed_scores.english_wer.rate, Some(0.0));
    let mut zh = en.clone();
    zh.corpus.as_mut().unwrap().language = Language::Zh;
    let chinese = text_scores(&zh, ScoreStage::FilteredAsr, "the cat", &mut work);
    assert!(matches!(
        chinese.english_wer.state,
        StageState::NotApplicable {}
    ));
    let mut missing = input;
    missing.corpus = None;
    missing.asr_only = None;
    let unknown = text_scores(&missing, ScoreStage::FilteredAsr, "the cat", &mut work);
    assert!(matches!(
        unknown.english_wer.state,
        StageState::Unknown { .. }
    ));
}

#[test]
fn internal_filter_quality_formatter_keeps_partial_reviews_unknown_and_complete_zero_separate() {
    let (_directory, _inputs, mut report) = paired_fixture();
    let mut evidence = selected_pair_mutations(&mut report);
    let ids = report
        .scoring
        .as_ref()
        .unwrap()
        .comparison_job_ids
        .as_ref()
        .unwrap()
        .clone();
    for id in &ids {
        report
            .cases
            .iter_mut()
            .find(|row| row.job_id == *id)
            .unwrap()
            .final_text = ObservedText::known("the dog");
    }
    let make_review = |arm: &InternalFilterArmEvidence| {
        let final_digest = validation::sha256("the dog".as_bytes());
        InternalFinalReview {
            final_text_sha256: final_digest.clone(),
            attribution_reviewed: false,
            damage_reviewed: true,
            bystander_ranges: vec![],
            incomplete_damage_kinds: vec![],
            damage: [
                InternalDamageKind::Head,
                InternalDamageKind::Tail,
                InternalDamageKind::Number,
                InternalDamageKind::Entity,
            ]
            .into_iter()
            .map(|kind| InternalDamageReview {
                kind,
                reference_sha256: arm.reference_sha256.clone(),
                final_text_sha256: final_digest.clone(),
                reference_range: if kind == InternalDamageKind::Tail {
                    [4, 7]
                } else {
                    [0, 3]
                },
                final_range: [0, 3],
                damaged: false,
            })
            .collect(),
        }
    };
    let off_review = make_review(&evidence.arms[0]);
    evidence.arms[0].final_review = Some(off_review);
    let partial = format_filter_quality(&report, Some(&evidence)).unwrap();
    assert!(partial.contains("head_damage_spans=unknown head_reviewed_arm_units=1/2"));
    assert!(partial.contains("tail_damage_spans=unknown tail_reviewed_arm_units=1/2"));

    let on_review = make_review(&evidence.arms[1]);
    evidence.arms[1].final_review = Some(on_review);
    let complete = format_filter_quality(&report, Some(&evidence)).unwrap();
    assert!(complete.contains("head_damage_spans=0 head_reviewed_arm_units=2/2"));
    assert!(complete.contains("tail_damage_spans=0 tail_reviewed_arm_units=2/2"));
    assert!(complete
        .contains("synthetic_pairs=1 synthetic_provenance=internal_observed_synthetic_pairs"));

    let mut second_off_head = evidence.arms[0]
        .final_review
        .as_ref()
        .unwrap()
        .damage
        .iter()
        .find(|damage| damage.kind == InternalDamageKind::Head)
        .unwrap()
        .clone();
    evidence.arms[0]
        .final_review
        .as_mut()
        .unwrap()
        .damage
        .iter_mut()
        .find(|damage| damage.kind == InternalDamageKind::Head)
        .unwrap()
        .damaged = true;
    second_off_head.reference_range = [0, 1];
    second_off_head.damaged = true;
    evidence.arms[0]
        .final_review
        .as_mut()
        .unwrap()
        .damage
        .push(second_off_head);
    evidence.arms[1]
        .final_review
        .as_mut()
        .unwrap()
        .damage
        .iter_mut()
        .find(|damage| damage.kind == InternalDamageKind::Head)
        .unwrap()
        .damaged = true;
    let multiple_spans = format_filter_quality(&report, Some(&evidence)).unwrap();
    assert!(multiple_spans.contains("head_damage_spans=3 head_reviewed_arm_units=2/2"));
    assert!(!multiple_spans.contains("head_damage_spans=3/2"));

    let mut second_pair_rows = report
        .cases
        .iter()
        .filter(|row| row.job_id == ids[0] || row.job_id == ids[1])
        .cloned()
        .collect::<Vec<_>>();
    for row in &mut second_pair_rows {
        row.case_id = "second-synthetic-test-pair".into();
        row.repeat = 17;
    }
    report.cases.extend(second_pair_rows);
    let mut second_pair_arms = evidence.arms.clone();
    for arm in &mut second_pair_arms {
        arm.case_id = "second-synthetic-test-pair".into();
        arm.repeat = 17;
        arm.synthetic = false;
    }
    evidence.arms.extend(second_pair_arms);
    let mixed = format_filter_quality(&report, Some(&evidence)).unwrap();
    assert!(mixed.contains("synthetic_pairs=1 synthetic_provenance=internal_observed_mixed_pairs"));
}

#[test]
fn internal_filter_quality_formatter_keeps_missing_comparison_and_evidence_explicit() {
    let (_directory, _inputs, mut report) = paired_fixture();
    let mut evidence = selected_pair_mutations(&mut report);
    for arm in &mut evidence.arms {
        let final_text = "speaker text";
        let final_digest = validation::sha256(final_text.as_bytes());
        arm.owner_eligibility = Some(true);
        arm.owner_false_reject = Some(false);
        arm.final_review = Some(InternalFinalReview {
            final_text_sha256: final_digest.clone(),
            attribution_reviewed: true,
            damage_reviewed: true,
            bystander_ranges: vec![],
            incomplete_damage_kinds: vec![],
            damage: [
                InternalDamageKind::Head,
                InternalDamageKind::Tail,
                InternalDamageKind::Number,
                InternalDamageKind::Entity,
            ]
            .into_iter()
            .map(|kind| InternalDamageReview {
                kind,
                reference_sha256: arm.reference_sha256.clone(),
                final_text_sha256: final_digest.clone(),
                reference_range: if kind == InternalDamageKind::Tail {
                    [4, 7]
                } else {
                    [0, 3]
                },
                final_range: [0, 3],
                damaged: false,
            })
            .collect(),
        });
    }
    assert!(format_filter_quality(&report, Some(&evidence))
        .unwrap()
        .contains("reason=none"));
    assert!(format_filter_quality(&report, None)
        .unwrap()
        .contains("reason=filter_evidence_missing"));

    let mut nonterminal = report.clone();
    nonterminal.terminal = StageState::NotRun {};
    assert!(format_filter_quality(&nonterminal, Some(&evidence))
        .unwrap()
        .contains("reason=run_nonterminal"));
    let mut replay = report.clone();
    replay.replay = Some(ReplaySummary {
        policy: CachePolicy {
            purpose: CachePurpose::FreshQuality,
            max_age_ms: None,
        },
        checkpoint_input_sha256: None,
        checkpoint_output: None,
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
        current_quality_eligible: false,
        current_performance_eligible: false,
        current_reliability_eligible: false,
    });
    assert!(format_filter_quality(&replay, Some(&evidence))
        .unwrap()
        .contains("reason=replay_quality_ineligible"));
    let mut missing_peer = report.clone();
    let right_id = evidence.on_job_id.as_deref().unwrap();
    missing_peer.cases.retain(|row| row.job_id != right_id);
    assert!(format_filter_quality(&missing_peer, Some(&evidence))
        .unwrap()
        .contains("reason=filter_quality_incomplete_or_invalid"));
    let mut wrong_ids = evidence.clone();
    wrong_ids.on_job_id = Some("other-job".into());
    assert!(format_filter_quality(&report, Some(&wrong_ids))
        .unwrap()
        .contains("reason=filter_evidence_job_identity_mismatch"));

    let mut no_scoring = report.clone();
    no_scoring.scoring = None;
    assert!(format_filter_quality(&no_scoring, Some(&evidence)).is_none());

    let comparison_spec = report.scoring.as_ref().unwrap().comparison_spec.clone();
    report.scoring.as_mut().unwrap().comparison_spec = None;
    assert!(format_filter_quality(&report, Some(&evidence)).is_none());
    report.scoring.as_mut().unwrap().comparison_spec = comparison_spec;
    report.scoring.as_mut().unwrap().comparison_job_ids = None;
    let no_roster = format_filter_quality(&report, Some(&evidence)).unwrap();
    assert!(no_roster.contains("reason=comparison_job_identity_missing"));
    assert!(no_roster.contains("requested_rows=unknown/unknown union_case_repeat_rows=unknown"));
    assert!(no_roster.contains("head_damage_spans=unknown head_reviewed_arm_units=unknown"));
}

#[test]
fn internal_filter_quality_retains_case_repeat_roster_when_one_bound_pair_is_observed() {
    let (_directory, _inputs, mut report) = paired_fixture();
    let ids = report
        .scoring
        .as_ref()
        .unwrap()
        .comparison_job_ids
        .as_ref()
        .unwrap();
    let pair_key = report
        .cases
        .iter()
        .find(|row| row.job_id == ids[0])
        .map(|row| (row.case_id.clone(), row.repeat))
        .unwrap();
    for row in &mut report.cases {
        if (row.job_id == ids[0] || row.job_id == ids[1])
            && row.case_id == pair_key.0
            && row.repeat == pair_key.1
        {
            let input = row.score_input.as_mut().unwrap();
            input.asr_reference = Some("the cat".into());
            input.corpus = None;
            input.asr_only = Some(super::super::speech_input::ScoreScope {
                purpose: super::super::speech_input::Purpose::PublisherReferenceAsr,
                language: "en".into(),
                reference_eligible: true,
                reference_policy: "synthetic_test".into(),
            });
            row.filtered_text = ObservedText::known(if row.job_id == ids[0] {
                "the cat"
            } else {
                "the dog"
            });
        }
    }
    let evidence = bound_filter_evidence(&report);
    let expected_left = report
        .cases
        .iter()
        .filter(|row| row.job_id == ids[0])
        .count() as u64;
    let expected_right = report
        .cases
        .iter()
        .filter(|row| row.job_id == ids[1])
        .count() as u64;
    let quality = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(quality.requested_off, Some(expected_left));
    assert_eq!(quality.requested_on, Some(expected_right));
    assert_eq!(quality.union_rows, Some(expected_left.max(expected_right)));
    assert_eq!(quality.observed_rows, 1);
    assert_eq!(quality.unknown_rows, expected_left.max(expected_right) - 1);

    let mut duplicate = report.clone();
    duplicate.cases.push(duplicate.cases[0].clone());
    let invalid = derive_filter_quality(&duplicate, Some(&evidence));
    assert_eq!(invalid.union_rows, quality.union_rows);
    assert_eq!(invalid.observed_rows, 0);
    assert!(invalid
        .invalid_reasons
        .iter()
        .any(|reason| reason.contains("duplicate case/repeat")));
}

#[test]
fn aggregate_wer_bound_keeps_unknown_pair_in_either_input_order() {
    let (_directory, _, mut report) = paired_fixture();
    let mut evidence = selected_pair_mutations(&mut report);
    let ids = report
        .scoring
        .as_ref()
        .unwrap()
        .comparison_job_ids
        .clone()
        .unwrap();
    let original_rows = report.cases.clone();
    for row in original_rows {
        if row.job_id != ids[0] && row.job_id != ids[1] {
            continue;
        }
        let mut repeated = row.clone();
        repeated.repeat = 1;
        if repeated.job_id == ids[1] {
            repeated.filtered_text = ObservedText::unknown("not_observed");
        }
        report.cases.push(repeated);
        let mut arm = evidence
            .arms
            .iter()
            .find(|arm| arm.job_id == row.job_id)
            .unwrap()
            .clone();
        arm.repeat = 1;
        evidence.arms.push(arm);
    }

    let first = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(first.per_pair.len(), 2);
    assert!(!first.per_pair[0].wer_unbounded);
    assert!(first.per_pair[1].wer_unbounded);
    assert!(first.wer_on_minus_off.is_none());
    assert!(first.wer_unbounded);

    report.cases.reverse();
    evidence.arms.reverse();
    let reversed = derive_filter_quality(&report, Some(&evidence));
    assert_eq!(reversed.per_pair, first.per_pair);
    assert_eq!(reversed.wer_on_minus_off, first.wer_on_minus_off);
    assert_eq!(reversed.wer_unbounded, first.wer_unbounded);
}
