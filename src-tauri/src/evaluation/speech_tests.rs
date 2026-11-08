//! Explicit synthetic structural fixtures; not publisher authority or acoustic gold.
use super::{contracts::*, fixtures::FixtureDirectory, speech_input::*, validation::*};
use serde_json::json;

fn source(reference: &str, language: &str) -> ProjectedCase {
    serde_json::from_value(json!({"case_id":"synthetic-source-row","source_locator":{"source_split":"fixture","source_parquet":"not-read.parquet","source_parquet_sha256":"1".repeat(64),"row_ordinal":0,"source_id":"fixture"},"publisher_record":{"id":"fixture","path":"publisher-metadata-not-local.wav","transcription":reference,"duration":1.0,"language":language,"original_speaker_id":7,"session_id":1,"topic":"not-a-template"},"source_observations":{},"audio_identity_declared_not_revalidated":{"audio_sha256":"1".repeat(64),"pcm_sha256":"2".repeat(64),"sample_count":16000,"sample_rate":16000,"channels":1,"bits_per_sample":16,"duration_seconds":1.0},"audio_locator":{"owner_evidence_role":"candidates","relative_path":"synthetic.wav","access":"metadata_only_not_resolved"},"conversion_declared":{"recipe":"identity-preserve-v1","changes":[],"reason":"explicit fixture","source_sample_interval":[0,16000]},"grouping":{"speaker":{"kind":"publisher_id","value":"7"},"conversation":{"kind":"unknown"},"template":{"kind":"unknown"}},"project_partition":"development_observation","project_content_review":"pending","references_by_purpose":{}})).unwrap()
}
pub(super) fn fixture(purpose: Purpose) -> (FixtureDirectory, ValidatedInputs, RunRecord) {
    let dir = FixtureDirectory::new();
    let (mut inputs, journal) = super::tests::journal_fixture(&dir);
    let mut source = source("你好 hello", "mixed");
    let audio = inputs.cases[0].audio.clone();
    source.audio_identity_declared_not_revalidated.audio_sha256 = audio.audio_sha256.clone();
    source.audio_identity_declared_not_revalidated.pcm_sha256 = audio.pcm_sha256.clone();
    let reference = if purpose == Purpose::FunctionalUnscored {
        Reference::NotProvided {}
    } else {
        Reference::PublisherAsr {
            text: source.publisher_record.transcription.clone(),
            text_sha256: sha256(source.publisher_record.transcription.as_bytes()),
            source_label_origin: "publisher_manual_asr_annotation".into(),
            source_record_evidence_id: "candidates".into(),
            annotation_protocol_id: "ascend-paper-v6".into(),
            source_transform: "none".into(),
        }
    };
    let case = Case {
        case_id: source.case_id.clone(),
        adjacent_wav: "synthetic.wav".into(),
        audio,
        source_record_evidence: SourceRecordEvidence {
            evidence_id: "candidates".into(),
            case_id: source.case_id.clone(),
        },
        reference,
    };
    let index = Index {
        artifact_type: "asr_speech_index".into(),
        schema_version: 1,
        index_id: "explicit-test-not-admitted".into(),
        source_packet: ControlFixtureLocator {
            relative_path: "packet.json".into(),
            sha256: "3".repeat(64),
        },
        admission: Admission {
            kind: "source_document_review".into(),
            actor_id: "synthetic-fixture-no-authority".into(),
            reviewed_at: "not-a-real-review".into(),
            revision: "fixture".into(),
            purposes: vec![purpose],
            evidence_ids: vec![],
            decision: "fixture_only".into(),
        },
        cases: vec![case.clone()],
    };
    let packet = Packet {
        artifact_type: "asr_speech_source_packet".into(),
        schema_version: 1,
        profile: "synthetic-not-admitted".into(),
        state: "fixture_not_admitted".into(),
        purposes: vec![purpose],
        source: json!({}),
        source_authority: json!({}),
        evidence: vec![],
        metadata_snapshot_output_owner_sha256: "4".repeat(64),
        case_count: 1,
        cases: vec![source.clone()],
        coverage: json!({}),
        producer_source_observation: json!({}),
    };
    let document = dir.0.canonicalize().unwrap().join("index.json");
    inputs.cases[0].case = CaseData::AsrSpeech(Box::new(AdmittedCase {
        case,
        source,
        purpose,
    }));
    inputs.cases[0].corpus = None;
    inputs.cases[0].source_kind = "publisher_human_speech".into();
    inputs.cases[0].document = CaseDocument::AsrSpeech {
        selected: document.clone(),
        canonical: document.clone(),
        sha256: "5".repeat(64),
        packet_sha256: "3".repeat(64),
    };
    inputs.selection = InputSelection::AsrSpeech {
        snapshot: Box::new(Snapshot {
            selected: document.clone(),
            canonical: document,
            sha256: "5".repeat(64),
            index,
            packet_sha256: "3".repeat(64),
            packet,
            evidence_bytes: vec![],
            selection_coverage: None,
        }),
        negative: None,
    };
    let recipe = inputs.recipe.as_mut().unwrap();
    recipe.schema_version = 3;
    recipe.input = RecipeInput::AsrSpeech(ExperimentInputV3::AsrSpeech {
        purpose,
        speech_index: ControlFixtureLocator {
            relative_path: "index.json".into(),
            sha256: "5".repeat(64),
        },
        non_speech_sha256: None,
    });
    recipe.cache_policy = None;
    recipe.scoring = if purpose == Purpose::PublisherReferenceAsr {
        Some(super::scoring::ScoringConfig {
            method: super::scoring::METHOD.into(),
            annotations: None,
            comparison: None,
        })
    } else {
        None
    };
    for j in &mut recipe.jobs {
        j.cleanup = CleanupControls {
            intensity: CleanupIntensity::Off,
            candidate_id: None,
            capture_provider_candidate: false,
        };
        j.local_rules.dictionary_replacements = false;
        j.local_rules.fuzzy = false;
        j.local_rules.spoken_layout = false;
        j.local_rules.fixture = None;
    }
    let report =
        super::runner::build_report(&inputs, journal.snapshot().identity.baseline().clone())
            .unwrap();
    (dir, inputs, report)
}
#[test]
fn functional_speech_reference_absence_is_not_negative_or_cleanup_gold() {
    let (_dir, inputs, mut report) = fixture(Purpose::FunctionalUnscored);
    assert!(!inputs.cases[0].negative());
    assert!(inputs.cases[0].case.speech().is_none());
    assert!(inputs.cases[0].case.legacy().is_none());
    assert!(inputs.cases[0].case.reference().is_none());
    for text in [
        &report.cases[0].prepared_text,
        &report.cases[0].adapter_candidate,
        &report.cases[0].final_text,
    ] {
        assert!(
            matches!(text,ObservedText::Unknown{reason} if reason=="asr_only_speech_postprocessing_not_applicable")
        );
    }
    let row = &mut report.cases[0];
    row.asr_observation = Some(crate::observations::AsrObservation {
        provider_text: ObservedText::KnownEmpty {},
        provider_text_source_kind: Some("explicit_synthetic_empty_fixture".into()),
        filtered_text: ObservedText::KnownEmpty {},
        origin: "local_model_output".into(),
        terminal: "local_complete_empty".into(),
        filtering_reason: None,
        protocol_completion: "complete".into(),
        reported_model: ObservedText::Unknown {
            reason: "not_a_runtime_fixture".into(),
        },
    });
    row.filtered_text = ObservedText::KnownEmpty {};
    row.state = StageState::Failed {
        reason: "speech_empty".into(),
    };
    super::scoring::refresh_row(row);
    let scores = row.scores.as_ref().unwrap();
    for stage in &scores.stages {
        assert_eq!(stage.denominator.eligible, 0);
        assert_eq!(stage.denominator.scored, 0);
        assert!(stage.text.normalized_cer.rate.is_none());
    }
    assert!(matches!(
        scores.negative_outcome,
        StageState::NotApplicable {}
    ));
    assert!(matches!(row.final_text, ObservedText::Unknown { .. }));
    let wire = serde_json::to_value(&report).unwrap();
    assert_eq!(wire["schema_version"], 3);
    assert!(wire["identity"].get("corpus_sha256").is_none());
    assert!(wire["cases"][0]["score_input"]
        .get("asr_reference")
        .is_none());
    assert!(serde_json::from_value::<RunRecord>(wire.clone()).is_ok());
    let mut bad = wire;
    bad["cases"][0]["input_paths"]["purpose"] = json!("publisher_reference_asr");
    assert!(serde_json::from_value::<RunRecord>(bad).is_err());
    assert!(super::resume::Admission::read(
        &inputs,
        Some(std::path::Path::new("not-opened.json")),
        None
    )
    .is_err());
}
#[test]
fn publisher_asr_scores_original_reference_with_tail_failures_and_no_final_score() {
    let (_dir, _inputs, mut report) = fixture(Purpose::PublisherReferenceAsr);
    for text in [
        &report.cases[0].prepared_text,
        &report.cases[0].adapter_candidate,
        &report.cases[0].final_text,
    ] {
        assert!(
            matches!(text,ObservedText::Unknown{reason} if reason=="asr_only_speech_postprocessing_not_applicable")
        );
    }
    let row = &mut report.cases[0];
    row.filtered_text = ObservedText::KnownValue {
        value: "你号 hello".into(),
    };
    row.asr_terminal = "speech_text".into();
    row.state = StageState::Completed {};
    super::scoring::refresh_row(row);
    let filtered = &row.scores.as_ref().unwrap().stages[1];
    assert_eq!(filtered.text.chinese_cer.edits, Some(1));
    assert_eq!(filtered.text.chinese_cer.reference_units, Some(2));
    assert_eq!(filtered.text.english_wer.edits, Some(0));
    assert_eq!(filtered.denominator.eligible, 1);
    assert!(!row.scores.as_ref().unwrap().cleanup_conditional_eligible);
    for s in &row.scores.as_ref().unwrap().stages[2..] {
        assert!(matches!(s.state, StageState::NotApplicable {}));
        assert_eq!(s.denominator.requested, 0);
    }
    // A complete observed empty string is measurable deletion, and failed speech.
    row.asr_observation = Some(crate::observations::AsrObservation {
        provider_text: ObservedText::KnownEmpty {},
        provider_text_source_kind: Some("explicit_synthetic_empty_fixture".into()),
        filtered_text: ObservedText::KnownEmpty {},
        origin: "local_model_output".into(),
        terminal: "local_complete_empty".into(),
        filtering_reason: None,
        protocol_completion: "complete".into(),
        reported_model: ObservedText::Unknown {
            reason: "not_a_runtime_fixture".into(),
        },
    });
    row.filtered_text = ObservedText::KnownEmpty {};
    row.state = StageState::Failed {
        reason: "speech_empty".into(),
    };
    super::scoring::refresh_row(row);
    let s = &row.scores.as_ref().unwrap().stages[1];
    assert_eq!(s.text.chinese_cer.edits, Some(2));
    assert_eq!(s.text.english_wer.edits, Some(1));
    assert_eq!(s.denominator.scored, 1);
    assert!(matches!(row.state, StageState::Failed { .. }));
    // An unobserved tail error is not an empty candidate or numeric success.
    row.filtered_text = ObservedText::Unknown {
        reason: "transport_failed".into(),
    };
    row.asr_terminal = "provider_failed".into();
    super::scoring::refresh_row(row);
    let s = &row.scores.as_ref().unwrap().stages[1];
    assert_eq!(s.denominator.eligible, 1);
    assert_eq!(s.denominator.failed, 1);
    assert_eq!(s.denominator.scored, 0);
    assert_eq!(s.text.normalized_cer.rate, None);
}
#[test]
fn strict_new_recipe_and_reference_variants_do_not_weaken_old_contracts() {
    let (_d, inputs, _) = fixture(Purpose::FunctionalUnscored);
    let recipe = serde_json::to_value(inputs.recipe.as_ref().unwrap()).unwrap();
    assert!(serde_json::from_value::<RunRecipeV3>(recipe.clone()).is_ok());
    assert!(serde_json::from_value::<RunRecipeV2>(recipe.clone()).is_err());
    assert!(serde_json::from_value::<RunRecipeV1>(recipe.clone()).is_err());
    let mut bad = recipe;
    bad["input"]["corpus_sidecar_sha256"] = json!("0".repeat(64));
    assert!(serde_json::from_value::<RunRecipeV3>(bad).is_err());
    assert!(serde_json::from_value::<Reference>(
        json!({"kind":"not_provided","text":"fake empty gold"})
    )
    .is_err());
    assert!(serde_json::from_value::<Index>(
        json!({"artifact_type":"asr_speech_index","schema_version":1,"cases":[]})
    )
    .is_err());
}

#[tokio::test]
async fn shared_asr_execution_view_has_no_reference_proxy_or_calls_without_a_key() {
    use super::legacy::{AsrAudioInput, AsrCandidate, AsrExecution};
    let candidate = AsrCandidate {
        id: "explicit-no-key-fixture",
        provider: "fixture",
        model: "fixture".into(),
        execution: AsrExecution::Cloud {
            endpoint: "https://not-contacted.invalid/transcriptions".into(),
            key: None,
        },
        host: "not-contacted.invalid".into(),
        key_source: "not_available".into(),
        language: None,
        prompt: None,
        keywords: vec![],
        authorization_scope: None,
    };
    let fixture = [AsrAudioInput {
        id: "speech-without-reference",
        audio: "not-opened.wav",
        reference: None,
        validated_audio: None,
    }];
    let report = super::legacy::run_asr_audio_inputs(
        &candidate,
        std::path::Path::new("not-opened.json"),
        &fixture,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(report.provider_invoked_cases, 0);
    assert_eq!(report.raw_asr_quality.sample_count, 0);
    assert_eq!(report.sanitized_text_quality.sample_count, 0);
    assert!(report.cases[0].reference.is_none());
    assert_eq!(
        report.cases[0].error_category.as_deref(),
        Some("no_explicit_key")
    );
    let wire = serde_json::to_value(&report).unwrap();
    assert!(wire["cases"][0].get("reference").is_none());
    assert_eq!(
        report.raw_asr_quality.character_error_rate_min_variant,
        None
    );
}

#[test]
fn asr_only_summary_keeps_success_failed_tail_and_reference_free_denominators() {
    for purpose in [Purpose::FunctionalUnscored, Purpose::PublisherReferenceAsr] {
        let (_directory, mut inputs, original) = fixture(purpose);
        inputs.recipe.as_mut().unwrap().jobs[0].repeats = 2;
        let mut report =
            super::runner::build_report(&inputs, original.identity.baseline().clone()).unwrap();
        assert_eq!(report.cases.len(), 2);
        for (i, row) in report.cases.iter_mut().enumerate() {
            row.attempts = 1;
            let mut fact = super::execution::fact(AttemptStage::Asr, 1);
            fact.local_adapter = StageState::Completed {};
            row.attempt_facts = vec![fact];
            if i == 0 {
                row.filtered_text = ObservedText::known("你好 hello");
                row.asr_terminal = "speech_text".into();
                row.state = StageState::Completed {};
            } else {
                row.filtered_text = ObservedText::Unknown {
                    reason: "explicit_tail_transport_fixture".into(),
                };
                row.asr_terminal = "provider_failed".into();
                row.state = StageState::Failed {
                    reason: "transport_fixture".into(),
                };
            }
            super::scoring::refresh_row(row);
        }
        super::scoring::refresh_summary(&mut report);
        let job = &report.scoring.as_ref().unwrap().jobs[0];
        assert_eq!(job.workflow.requested, 2);
        assert_eq!(job.workflow.called, 2);
        assert_eq!(job.workflow.nonempty, 1);
        assert_eq!(job.workflow.failed, 1);
        assert_eq!(job.asr_success_cases, 1);
        assert_eq!(job.all_speech_e2e.requested, 0);
        assert_eq!(job.nonempty_final_proxy_rate_all_speech.numerator, 0);
        assert_eq!(job.nonempty_final_proxy_rate_all_speech.denominator, 0);
        assert!(job.nonempty_final_proxy_rate_all_speech.value.is_none());
        let filtered = &job.stages["filtered_asr"];
        assert_eq!(filtered.requested, 2);
        assert_eq!(filtered.failed, 1);
        if purpose == Purpose::FunctionalUnscored {
            assert_eq!(filtered.eligible, 0);
            assert_eq!(filtered.scored, 0);
            assert_eq!(
                job.stage_quality["filtered_asr"]
                    .fresh
                    .normalized_cer
                    .micro_rate,
                None
            );
        } else {
            assert_eq!(filtered.eligible, 2);
            assert_eq!(filtered.scored, 1);
            assert_eq!(
                job.stage_quality["filtered_asr"]
                    .fresh
                    .normalized_cer
                    .samples,
                1
            );
        }
    }
}

#[test]
fn unqualified_empty_and_cancelled_speech_never_gain_negative_or_reference_credit() {
    let (_d, _inputs, mut report) = fixture(Purpose::PublisherReferenceAsr);
    let row = &mut report.cases[0];
    row.filtered_text = ObservedText::KnownEmpty {};
    row.state = StageState::Failed {
        reason: "unknown_empty_fixture".into(),
    };
    super::scoring::refresh_row(row);
    let score = &row.scores.as_ref().unwrap().stages[1];
    assert!(matches!(score.state, StageState::Unknown { .. }));
    assert_eq!(score.denominator.scored, 0);
    assert_eq!(score.text.normalized_cer.rate, None);
    assert!(matches!(
        row.scores.as_ref().unwrap().negative_outcome,
        StageState::NotApplicable {}
    ));
    row.filtered_text = ObservedText::Unknown {
        reason: "cancelled_before_result".into(),
    };
    row.state = StageState::Cancelled {
        reason: "explicit_fixture_cancel".into(),
    };
    row.cancellation_requested = true;
    super::scoring::refresh_row(row);
    assert_eq!(
        row.scores.as_ref().unwrap().stages[1].denominator.cancelled,
        1
    );
    assert_eq!(row.scores.as_ref().unwrap().stages[1].denominator.scored, 0);
}

#[test]
fn legacy_cleanup_capable_case_seeds_not_run_without_asr_only_relabeling() {
    let d = FixtureDirectory::new();
    let (_, journal) = super::tests::journal_fixture(&d);
    let report = journal.snapshot();
    for text in [
        &report.cases[0].prepared_text,
        &report.cases[0].adapter_candidate,
        &report.cases[0].final_text,
    ] {
        assert!(matches!(text,ObservedText::Unknown{reason} if reason=="not_run"));
    }
}
