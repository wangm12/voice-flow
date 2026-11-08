//! ASR-only admission: a native selection of independently projected source rows.
//! The fixed profile is source-document authority, never project listening/gold.
use super::contracts::{AudioIdentity, ControlFixtureLocator};
use super::validation::{self as v, error, InputError};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
macro_rules! strict {($n:ident{$($f:ident:$t:ty),*$(,)?})=>{#[derive(Debug,Clone,Serialize,Deserialize)] #[serde(deny_unknown_fields)] pub struct $n{$(pub $f:$t),*}}}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    FunctionalUnscored,
    PublisherReferenceAsr,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Reference {
    NotProvided {},
    PublisherAsr {
        text: String,
        text_sha256: String,
        source_label_origin: String,
        source_record_evidence_id: String,
        annotation_protocol_id: String,
        source_transform: String,
    },
}
impl Reference {
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::PublisherAsr { text, .. } => Some(text),
            _ => None,
        }
    }
}
strict!(Admission { kind:String, actor_id:String, reviewed_at:String, revision:String, purposes:Vec<Purpose>, evidence_ids:Vec<String>, decision:String });
strict!(Index {artifact_type:String,schema_version:u32,index_id:String,source_packet:ControlFixtureLocator,admission:Admission,cases:Vec<Case>});
strict!(Case {
    case_id: String,
    adjacent_wav: String,
    audio: AudioIdentity,
    source_record_evidence: SourceRecordEvidence,
    reference: Reference
});
strict!(SourceRecordEvidence {
    evidence_id: String,
    case_id: String
});
strict!(Evidence {
    role: String,
    path: String,
    canonical_path: String,
    sha256: String,
    bytes: u64
});
strict!(PublisherRecord {
    id: String,
    path: String,
    transcription: String,
    duration: f64,
    language: String,
    original_speaker_id: u64,
    session_id: u64,
    topic: String
});
strict!(SourceLocator {
    source_split: String,
    source_parquet: String,
    source_parquet_sha256: String,
    row_ordinal: u64,
    source_id: String
});
strict!(DeclaredAudio {
    audio_sha256: String,
    pcm_sha256: String,
    sample_count: u64,
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    duration_seconds: f64
});
impl DeclaredAudio {
    fn identity(&self) -> AudioIdentity {
        AudioIdentity {
            audio_sha256: self.audio_sha256.clone(),
            pcm_sha256: self.pcm_sha256.clone(),
            sample_count: self.sample_count,
            sample_rate: self.sample_rate,
            channels: self.channels,
            bits_per_sample: self.bits_per_sample,
        }
    }
}
strict!(AudioLocator {
    owner_evidence_role: String,
    relative_path: String,
    access: String
});
strict!(Conversion {recipe:String,changes:Vec<String>,reason:String,source_sample_interval:[u64;2]});
strict!(ProjectedCase {
    case_id: String,
    source_locator: SourceLocator,
    publisher_record: PublisherRecord,
    source_observations: serde_json::Value,
    audio_identity_declared_not_revalidated: DeclaredAudio,
    audio_locator: AudioLocator,
    conversion_declared: Conversion,
    grouping: serde_json::Value,
    project_partition: String,
    project_content_review: String,
    references_by_purpose: serde_json::Value
});
strict!(Packet {artifact_type:String,schema_version:u32,profile:String,state:String,purposes:Vec<Purpose>,source:serde_json::Value,source_authority:serde_json::Value,evidence:Vec<Evidence>,metadata_snapshot_output_owner_sha256:String,case_count:usize,cases:Vec<ProjectedCase>,coverage:serde_json::Value,producer_source_observation:serde_json::Value});
strict!(SelectionCoverage {profile:String,cohort_members:usize,selected_members:usize,unselected_members:usize,selected_case_ids:Vec<String>,selection_order:String});
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputIdentity {
    pub kind: String,
    pub purpose: Purpose,
    pub speech_index_sha256: String,
    pub non_speech_sha256: Option<String>,
    pub source_packet_sha256: String,
    pub evidence_sha256: BTreeMap<String, String>,
    // Absent for all pre-existing fixed-three identities: serialization unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_coverage: Option<SelectionCoverage>,
}
strict!(InputPaths {
    document_kind: String,
    selected_document: String,
    canonical_document: String,
    index_sha256: String,
    source_packet_sha256: String,
    purpose: Purpose,
    case: Case,
    source: ProjectedCase,
    declared_audio: String,
    canonical_audio: String
});
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScoreScope {
    pub purpose: Purpose,
    pub language: String,
    pub reference_eligible: bool,
    pub reference_policy: String,
}
#[derive(Debug, Clone)]
pub struct AdmittedCase {
    pub case: Case,
    pub source: ProjectedCase,
    pub purpose: Purpose,
}
impl ScoreScope {
    pub fn for_source(purpose: Purpose, source: &ProjectedCase) -> Self {
        Self {
            purpose,
            language: source.publisher_record.language.clone(),
            reference_eligible: purpose == Purpose::PublisherReferenceAsr
                && !source.publisher_record.transcription.contains("[UNK]"),
            reference_policy: if purpose == Purpose::FunctionalUnscored {
                "functional_unscored_reference_not_selected"
            } else if source.publisher_record.transcription.contains("[UNK]") {
                "publisher_marker_policy_unsupported"
            } else {
                "ascend-paper-v6_original_text_no_transforms"
            }
            .into(),
        }
    }
}
impl AdmittedCase {
    pub fn score_scope(&self) -> ScoreScope {
        ScoreScope::for_source(self.purpose, &self.source)
    }
}
#[derive(Debug)]
pub struct Snapshot {
    pub selected: PathBuf,
    pub canonical: PathBuf,
    pub sha256: String,
    pub index: Index,
    pub packet_sha256: String,
    pub packet: Packet,
    pub evidence_bytes: Vec<Vec<u8>>,
    pub selection_coverage: Option<SelectionCoverage>,
}
impl Snapshot {
    pub fn identity(
        &self,
        negative: Option<&super::validation::NonSpeechSnapshot>,
    ) -> InputIdentity {
        InputIdentity {
            kind: "asr_speech".into(),
            purpose: self.purpose(),
            speech_index_sha256: self.sha256.clone(),
            non_speech_sha256: negative.map(|n| n.sha256.clone()),
            source_packet_sha256: self.packet_sha256.clone(),
            evidence_sha256: self
                .packet
                .evidence
                .iter()
                .map(|e| (e.role.clone(), e.sha256.clone()))
                .collect(),
            selection_coverage: self.selection_coverage.clone(),
        }
    }
    pub fn purpose(&self) -> Purpose {
        self.index.admission.purposes[0]
    }
}
fn require(path: &Path, ok: bool, code: &'static str, detail: &str) -> Result<(), InputError> {
    if ok {
        Ok(())
    } else {
        Err(error(path, code, detail))
    }
}
pub(super) fn read(
    selected: &Path,
    expected_sha: &str,
    purpose: Purpose,
    cases: &mut Vec<v::ValidatedCase>,
    ids: &mut BTreeSet<String>,
    total: &mut u64,
) -> Result<Snapshot, InputError> {
    let path = v::external(selected)?;
    let (index, raw): (Index, _) = v::json(&path)?;
    require(
        &path,
        v::digest(expected_sha) && v::sha256(&raw) == expected_sha,
        "sha_mismatch",
        "speech index SHA mismatch",
    )?;
    require(
        &path,
        index.artifact_type == "asr_speech_index"
            && index.schema_version == 1
            && v::bounded(&index.index_id, 128),
        "schema",
        "strict ASRSpeechIndexV1 required",
    )?;
    let packet_path = v::relative_file(&path, &index.source_packet.relative_path)?;
    let (packet, packet_raw): (Packet, _) = v::json(&packet_path)?;
    let packet_sha256 = v::sha256(&packet_raw);
    require(
        &path,
        index.source_packet.sha256 == packet_sha256,
        "sha_mismatch",
        "source packet SHA differs from selected bytes",
    )?;
    let profile = super::speech_profiles::Profile::select(&path, &packet, &packet_sha256)?;
    require(
        &path,
        packet.purposes.contains(&purpose),
        "source_identity",
        "requested purpose absent from source packet",
    )?;
    // Retain the reviewed actor's declaration from the actual packet. A caller
    // cannot substitute a listening reviewer or elevate this to cleanup gold.
    let a = &index.admission;
    let authority = &packet.source_authority;
    require(&path,a.kind=="source_document_review"&&a.actor_id==authority["originating_role"].as_str().unwrap_or("")&&a.reviewed_at==authority["observed_at"].as_str().unwrap_or("")&&a.revision=="public-reference-admission-audit-01"&&a.purposes==vec![purpose]&&a.decision=="allowed_for_declared_evaluation","rights_not_reviewed","source-document admission must name the retained audit actor, date, scope and purpose; it is not listening review")?;
    let expected = &profile.evidence_pins;
    require(
        &path,
        packet.evidence.len() == expected.len(),
        "source_identity",
        "closed compiled-profile evidence count required before evidence reads",
    )?;
    let mut evidence = BTreeMap::new();
    let mut evidence_bytes = Vec::new();
    let mut evidence_total = 0usize;
    for e in &packet.evidence {
        require(
            &path,
            evidence.insert(e.role.clone(), e).is_none(),
            "duplicate_id",
            "duplicate source evidence role",
        )?;
        require(
            &path,
            expected.get(&e.role) == Some(&e.sha256),
            "source_identity",
            "source evidence role and SHA must belong to the compiled profile",
        )?;
        let ep = v::external(Path::new(&e.path))?;
        require(
            &path,
            ep == Path::new(&e.canonical_path),
            "identity_mismatch",
            "source evidence canonical path changed",
        )?;
        let remaining = (4 * 1024 * 1024usize).saturating_sub(evidence_total);
        let b = v::read_bounded(&ep, v::MAX_JSON_BYTES.min(remaining as u64))?;
        evidence_total += b.len();
        require(
            &ep,
            e.bytes == b.len() as u64 && v::sha256(&b) == e.sha256,
            "sha_mismatch",
            "retained source evidence changed",
        )?;
        evidence_bytes.push(b);
    }
    require(
        &path,
        evidence_total <= 4 * 1024 * 1024
            && evidence.len() == expected.len()
            && evidence
                .iter()
                .all(|(role, fact)| expected.get(role) == Some(&fact.sha256))
            && a.evidence_ids.len() == expected.len()
            && a.evidence_ids.iter().collect::<BTreeSet<_>>().len() == expected.len()
            && expected.keys().all(|role| a.evidence_ids.contains(role)),
        "source_identity",
        "closed compiled-profile evidence roles and SHAs required (aggregate4MiB)",
    )?;
    let raw_by_role: BTreeMap<_, _> = packet
        .evidence
        .iter()
        .zip(&evidence_bytes)
        .map(|(fact, raw)| (fact.role.clone(), raw.as_slice()))
        .collect();
    profile.verify_members(&path, &packet, &raw_by_role)?;
    require(
        &path,
        !index.cases.is_empty() && index.cases.len() <= profile.member_count,
        "input_budget",
        "selection must fit the admitted source-profile membership",
    )?;
    let mut eligible = 0;
    for c in &index.cases {
        require(
            &path,
            v::bounded(&c.case_id, 128) && ids.insert(c.case_id.clone()),
            "duplicate_id",
            "case IDs must be globally unique",
        )?;
        let source = packet
            .cases
            .iter()
            .find(|s| s.case_id == c.source_record_evidence.case_id)
            .ok_or_else(|| error(&path, "source_identity", "selected source row is absent"))?;
        require(&path,c.case_id==source.case_id&&c.source_record_evidence.evidence_id=="candidates"&&c.adjacent_wav==source.audio_locator.relative_path&&source.audio_locator.owner_evidence_role=="candidates"&&super::resume::hash(&c.audio)==super::resume::hash(&source.audio_identity_declared_not_revalidated.identity())&&source.conversion_declared.source_sample_interval==[0,c.audio.sample_count]&&source.conversion_declared.recipe=="identity-preserve-v1"&&source.conversion_declared.changes.is_empty()&&source.project_content_review=="pending","source_identity","native case must bind exact projected row/audio/source interval; pending project review is preserved")?;
        validate_reference(&path, purpose, &c.reference, source)?;
        if purpose == Purpose::PublisherReferenceAsr
            && !source.publisher_record.transcription.contains("[UNK]")
        {
            eligible += 1;
        }
        let owner = Path::new(&evidence["candidates"].canonical_path);
        let audio_path = v::adjacent(owner, &c.adjacent_wav)?;
        let bytes = v::read_bounded(&audio_path, v::MAX_WAV_BYTES)?;
        *total += bytes.len() as u64;
        require(
            &path,
            *total <= v::MAX_TOTAL_AUDIO_BYTES && cases.len() < v::MAX_CORPUS_CASES,
            "input_budget",
            "aggregate audio/case budget exceeded",
        )?;
        let audio = v::wav(&audio_path, &bytes)?;
        require(
            &audio_path,
            super::resume::hash(&audio) == super::resume::hash(&c.audio),
            "sha_mismatch",
            "complete WAV/PCM/sample identity differs from source-bound selection",
        )?;
        cases.push(v::ValidatedCase {
            corpus: None,
            bytes,
            case: v::CaseData::AsrSpeech(Box::new(AdmittedCase {
                case: c.clone(),
                source: source.clone(),
                purpose,
            })),
            audio_path,
            audio,
            document: v::CaseDocument::AsrSpeech {
                selected: selected.into(),
                canonical: path.clone(),
                sha256: expected_sha.into(),
                packet_sha256: packet_sha256.clone(),
            },
            source_kind: "publisher_human_speech".into(),
        });
    }
    require(
        &path,
        purpose != Purpose::PublisherReferenceAsr || eligible > 0,
        "reference_not_eligible",
        "no reference eligible under the explicit publisher marker policy",
    )?;
    let selection_coverage = profile.selection_coverage(
        index
            .cases
            .iter()
            .map(|case| case.case_id.clone())
            .collect(),
    );
    Ok(Snapshot {
        selected: selected.into(),
        canonical: path,
        sha256: expected_sha.into(),
        index,
        packet_sha256,
        packet,
        evidence_bytes,
        selection_coverage,
    })
}
pub(super) fn validate_reference(
    path: &Path,
    purpose: Purpose,
    reference: &Reference,
    source: &ProjectedCase,
) -> Result<(), InputError> {
    let ok = match (purpose, reference) {
        (Purpose::FunctionalUnscored, Reference::NotProvided {}) => true,
        (
            Purpose::PublisherReferenceAsr,
            Reference::PublisherAsr {
                text,
                text_sha256,
                source_label_origin,
                source_record_evidence_id,
                annotation_protocol_id,
                source_transform,
            },
        ) => {
            v::bounded(text, 16_384)
                && *text == source.publisher_record.transcription
                && v::sha256(text.as_bytes()) == *text_sha256
                && source_label_origin == "publisher_manual_asr_annotation"
                && source_record_evidence_id == "candidates"
                && annotation_protocol_id == "ascend-paper-v6"
                && source_transform == "none"
        }
        _ => false,
    };
    require(path,ok,"reference_identity","purpose/reference must match the exact publisher row and annotation protocol; no cleanup reference is supplied")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn additive_cohort_selection_facts_preserve_old_fixed3_identity_serialization() {
        let previous = serde_json::json!({"kind":"asr_speech","purpose":"publisher_reference_asr","speech_index_sha256":"1".repeat(64),"non_speech_sha256":null,"source_packet_sha256":"2".repeat(64),"evidence_sha256":{"candidates":"3".repeat(64)}});
        let old: InputIdentity = serde_json::from_value(previous.clone()).unwrap();
        assert!(old.selection_coverage.is_none());
        assert_eq!(serde_json::to_value(&old).unwrap(), previous);
        let expected_wire = format!(
            r#"{{"kind":"asr_speech","purpose":"publisher_reference_asr","speech_index_sha256":"{}","non_speech_sha256":null,"source_packet_sha256":"{}","evidence_sha256":{{"candidates":"{}"}}}}"#,
            "1".repeat(64),
            "2".repeat(64),
            "3".repeat(64)
        );
        assert_eq!(serde_json::to_string(&old).unwrap(), expected_wire);
        let mut cohort = old;
        cohort.selection_coverage = Some(SelectionCoverage {
            profile: "ascend-audited-pilot60-metadata-v1".into(),
            cohort_members: 60,
            selected_members: 2,
            unselected_members: 58,
            selected_case_ids: vec!["last".into(), "first".into()],
            selection_order: "caller_index_order".into(),
        });
        let current = serde_json::to_value(&cohort).unwrap();
        let roundtrip: InputIdentity = serde_json::from_value(current.clone()).unwrap();
        assert_eq!(
            roundtrip.selection_coverage.unwrap().selected_case_ids,
            vec!["last", "first"]
        );
        let mut unknown = current;
        unknown["selection_coverage"]["holdout_certified"] = serde_json::json!(true);
        assert!(serde_json::from_value::<InputIdentity>(unknown).is_err());
    }

    #[test]
    fn source_reference_admission_distinguishes_unselected_original_text_from_gold() {
        let source:ProjectedCase=serde_json::from_value(serde_json::json!({"case_id":"fixture","source_locator":{"source_split":"fixture","source_parquet":"unread.parquet","source_parquet_sha256":"0".repeat(64),"row_ordinal":1,"source_id":"1"},"publisher_record":{"id":"1","path":"publisher-not-a-local-path","transcription":"Original text","duration":1.0,"language":"en","original_speaker_id":7,"session_id":1,"topic":"not-template"},"source_observations":{},"audio_identity_declared_not_revalidated":{"audio_sha256":"0".repeat(64),"pcm_sha256":"1".repeat(64),"sample_count":16000,"sample_rate":16000,"channels":1,"bits_per_sample":16,"duration_seconds":1.0},"audio_locator":{"owner_evidence_role":"candidates","relative_path":"fixture.wav","access":"metadata_only_not_resolved"},"conversion_declared":{"recipe":"identity-preserve-v1","changes":[],"reason":"fixture","source_sample_interval":[0,16000]},"grouping":{"conversation":{"kind":"unknown"},"template":{"kind":"unknown"}},"project_partition":"development_observation","project_content_review":"pending","references_by_purpose":{}})).unwrap();
        let path = Path::new("explicit-synthetic-reference-fixture");
        assert!(validate_reference(
            path,
            Purpose::FunctionalUnscored,
            &Reference::NotProvided {},
            &source
        )
        .is_ok());
        assert_eq!(
            validate_reference(
                path,
                Purpose::PublisherReferenceAsr,
                &Reference::NotProvided {},
                &source
            )
            .unwrap_err()
            .code,
            "reference_identity"
        );
        let mut reference = Reference::PublisherAsr {
            text: "Original text".into(),
            text_sha256: v::sha256(b"Original text"),
            source_label_origin: "publisher_manual_asr_annotation".into(),
            source_record_evidence_id: "candidates".into(),
            annotation_protocol_id: "ascend-paper-v6".into(),
            source_transform: "none".into(),
        };
        assert!(
            validate_reference(path, Purpose::PublisherReferenceAsr, &reference, &source).is_ok()
        );
        assert!(
            validate_reference(path, Purpose::FunctionalUnscored, &reference, &source).is_err()
        );
        if let Reference::PublisherAsr { text, .. } = &mut reference {
            *text = "original text".into();
        }
        assert_eq!(
            validate_reference(path, Purpose::PublisherReferenceAsr, &reference, &source)
                .unwrap_err()
                .code,
            "reference_identity"
        );
    }
}
