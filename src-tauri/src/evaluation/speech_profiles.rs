//! Compiled source-authority profiles, not a caller-controlled approval registry.
//! Membership comes from pinned independent metadata proof, never model output.
use super::{
    speech_input::{Packet, ProjectedCase, SelectionCoverage},
    validation::{self as v, error, InputError},
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
macro_rules! strict {($n:ident{$($(#[$m:meta])* $f:ident:$t:ty),*$(,)?})=>{#[derive(Debug,Deserialize)]#[serde(deny_unknown_fields)]struct $n{$($(#[$m])* $f:$t),*}}}
strict!(Catalog{artifact_type:String,schema_version:u32,profiles:Vec<Profile>});
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Profile {
    pub id: String,
    pub packet_sha256: Option<String>,
    pub packet_state: String,
    pub member_count: usize,
    pub evidence_pins: BTreeMap<String, String>,
    cohort_proof: Option<ProofRoles>,
}
strict!(ProofRoles {
    proposal_role: String,
    audit_role: String,
    verification_role: String
});
strict!(Proposal{artifact_type:String,schema_version:u32,profile:String,state:String,member_ids:Vec<String>,members:Vec<ProposalMember>,existing_three_profile_unchanged:bool,admitted:bool,audit_sha256:String});
strict!(ProposalMember {
    case_id: String,
    parent_comparison: String,
    binding_state: String,
    reference: serde_json::Value,
    audio_receipt: serde_json::Value
});
strict!(Audit{artifact_type:String,schema_version:u32,profile:String,state:String,requested:usize,parent_comparison_counts:BTreeMap<String,u64>,evidence_binding_conflicts:u64,cases:Vec<AuditedMember>,#[serde(rename="coverage")] _coverage:serde_json::Value,source_authority:serde_json::Value,#[serde(rename="evidence")] _evidence:serde_json::Value,#[serde(rename="selected_plan_snapshot")] _selected_plan_snapshot:serde_json::Value,#[serde(rename="research_owner_source_identities")] _research_owner_source_identities:serde_json::Value,#[serde(rename="typed_page_snapshots")] _typed_page_snapshots:serde_json::Value,#[serde(rename="original_selection_audit")] _original_selection_audit:serde_json::Value,#[serde(rename="claims")] _claims:serde_json::Value});
strict!(AuditedMember{case_id:String,parent_comparison:String,field_matches:BTreeMap<String,bool>,reason:Option<String>,evidence_bindings:BTreeMap<String,bool>,binding_state:String,converted_candidate_record:Candidate,#[serde(rename="publisher_parent_record")] _publisher_parent_record:serde_json::Value,#[serde(rename="review_queue_record")] _review_queue_record:serde_json::Value,#[serde(rename="WAV_receipt")] wav_receipt:serde_json::Value,reference_use:serde_json::Value,#[serde(rename="group_limits")] _group_limits:serde_json::Value});
// This is the existing projection's source-row schema, not a second CSV/filename
// interpreter. Values have already been bound to typed parent rows by the audit.
strict!(Annotation {
    text: String,
    text_sha256: String,
    origin: String,
    project_review_status: String,
    contains_unk: bool
});
strict!(Candidate{case_id:String,source_split:String,project_split:String,source_parquet:String,source_parquet_sha256:String,row_ordinal:u64,source_id:String,source_path:String,embedded_audio_path:String,language:String,original_speaker_id:u64,session_id:u64,topic:String,publisher_duration_seconds:f64,publisher_duration_minus_pcm_seconds:f64,filename_components_observed:serde_json::Value,raw_recording_group:String,conversation_partner_id:Option<String>,conversation_pair_id:Option<String>,global_conversation_session_id:Option<String>,template_family_id:Option<String>,source_annotation:Annotation,audio:serde_json::Value,conversion:serde_json::Value,conditions:serde_json::Value,adjacent_wav:String});
fn require(path: &Path, ok: bool, detail: &str) -> Result<(), InputError> {
    if ok {
        Ok(())
    } else {
        Err(error(path, "source_profile_not_admitted", detail))
    }
}
impl Profile {
    // Called only after the existing index membership/WAV admission loop.
    // Source proof coverage and caller-selected execution order are distinct.
    pub(super) fn selection_coverage(
        &self,
        selected_case_ids: Vec<String>,
    ) -> Option<SelectionCoverage> {
        self.cohort_proof.as_ref().map(|_| SelectionCoverage {
            profile: self.id.clone(),
            cohort_members: self.member_count,
            selected_members: selected_case_ids.len(),
            unselected_members: self.member_count - selected_case_ids.len(),
            selected_case_ids,
            selection_order: "caller_index_order".into(),
        })
    }
    pub(super) fn select(path: &Path, packet: &Packet, sha: &str) -> Result<Self, InputError> {
        let catalog: Catalog = v::decode_json(
            Path::new("compiled speech_profiles.json"),
            include_bytes!("speech_profiles.json"),
        )?;
        require(
            path,
            catalog.artifact_type == "compiled_asr_speech_profiles" && catalog.schema_version == 1,
            "unsupported compiled profile catalog",
        )?;
        let profile = catalog
            .profiles
            .into_iter()
            .find(|p| p.id == packet.profile)
            .ok_or_else(|| {
                error(
                    path,
                    "source_profile_not_admitted",
                    "source profile is not in the compiled audited catalog",
                )
            })?;
        require(
            path,
            profile.packet_sha256.as_ref().is_none_or(|pin| pin == sha)
                && packet.artifact_type == "asr_speech_source_packet"
                && packet.schema_version == 1
                && packet.state == profile.packet_state
                && packet.case_count == profile.member_count
                && packet.cases.len() == profile.member_count,
            "source packet/profile digest or declared member coverage mismatch",
        )?;
        require(
            path,
            profile.packet_sha256.is_some() || profile.cohort_proof.is_some(),
            "profile without packet or independent member proof is not admitted",
        )?;
        require(
            path,
            !packet.purposes.is_empty()
                && packet.purposes.len() <= 2
                && (packet.purposes.len() == 1 || packet.purposes[0] != packet.purposes[1]),
            "explicit distinct supported purposes required",
        )?;
        require(
            path,
            (1..=200).contains(&profile.member_count)
                && profile.cohort_proof.as_ref().is_none_or(|r| {
                    [&r.proposal_role, &r.audit_role, &r.verification_role]
                        .iter()
                        .all(|role| profile.evidence_pins.contains_key(*role))
                }),
            "compiled profile proof roles/member envelope is invalid",
        )?;
        Ok(profile)
    }
    pub(super) fn verify_members(
        &self,
        path: &Path,
        packet: &Packet,
        raw: &BTreeMap<String, &[u8]>,
    ) -> Result<(), InputError> {
        let Some(roles) = &self.cohort_proof else {
            return Ok(());
        };
        let proposal: Proposal = v::decode_json(path, raw[&roles.proposal_role])?;
        let audit: Audit = v::decode_json(path, raw[&roles.audit_role])?;
        // The verification bytes were already SHA-checked against this compiled role.
        // Its declarations remain independent source evidence, not listening review.
        let verification: serde_json::Value = v::decode_json(path, raw[&roles.verification_role])?;
        require(
            path,
            verification["decision"]
                == "accepted_scoped_source_and_one_actual_offline_metadata_audit"
                && verification["admitted"] == false,
            "independent verification scope changed",
        )?;
        require(
            path,
            proposal.artifact_type == "asr_pilot60_native_cohort_proposal"
                && proposal.schema_version == 1
                && proposal.profile == "ascend-pending-pilot60-publisher-metadata-v1"
                && proposal.state
                    == "research_only_requires_independent_audit_and_product_profile_change"
                && !proposal.admitted
                && proposal.existing_three_profile_unchanged
                && proposal.audit_sha256 == self.evidence_pins[&roles.audit_role],
            "research proposal must remain distinct and admitted:false with its exact audit",
        )?;
        require(
            path,
            audit.artifact_type == "ascend_pilot60_publisher_audit"
                && audit.schema_version == 1
                && audit.profile == proposal.profile
                && audit.state == "metadata_only_complete"
                && audit.requested == self.member_count
                && audit.parent_comparison_counts.get("match") == Some(&(self.member_count as u64))
                && audit.parent_comparison_counts.get("mismatch") == Some(&0)
                && audit.parent_comparison_counts.get("unknown") == Some(&0)
                && audit.evidence_binding_conflicts == 0,
            "independent complete member audit required",
        )?;
        let source_doc: serde_json::Value = v::decode_json(path, raw["audit"])?;
        let candidates: serde_json::Value = v::decode_json(path, raw["candidates"])?;
        require(
            path,
            audit.source_authority["review"] == source_doc,
            "member proof/source authority review differs from the pinned source review",
        )?;
        let mut decisions = serde_json::Map::new();
        for purpose in &packet.purposes {
            let (name, scope) = match purpose {
                super::speech_input::Purpose::FunctionalUnscored => (
                    "functional_unscored",
                    "unscored_public_human_asr_functional_smoke",
                ),
                super::speech_input::Purpose::PublisherReferenceAsr => (
                    "publisher_reference_asr",
                    "publisher_reference_asr_benchmark",
                ),
            };
            let decision = source_doc["admission_decisions"]
                .as_array()
                .and_then(|rows| rows.iter().find(|d| d["purpose"] == scope))
                .ok_or_else(|| {
                    error(
                        path,
                        "rights_not_reviewed",
                        "requested source-document purpose absent",
                    )
                })?;
            require(
                path,
                decision["publisher_evidence_eligible"] == true
                    && decision["admitted_by_current_pending_artifacts"] == false,
                "source authority cannot become project content approval",
            )?;
            decisions.insert(name.into(), decision.clone());
        }
        let expected_authority = serde_json::json!({"kind":"retained_independent_source_document_review","originating_role":source_doc["role"],"observed_at":source_doc["created_utc"],"review_artifact_type":source_doc["artifact_type"],"source_ids":source_doc["source_ids"],"decisions":decisions,"scope":"publisher/source metadata authority only; no project listening/cleanup-intent declaration"});
        let expected_source = serde_json::json!({"dataset_id":"CAiRE/ASCEND","revision":candidates["revision"],"license_id":candidates["license"],"attribution":candidates["attribution"],"declared_original_receipt_path":candidates["source_receipt"],"declared_receipt_sha256":candidates["source_receipt_sha256"]});
        require(
            path,
            packet.source_authority == expected_authority
                && packet.source == expected_source
                && packet.metadata_snapshot_output_owner_sha256
                    == "dfca44713f27ebd1ba138ad5ac652115985969db43fd92badbd031af295c6c88",
            "packet source envelope must retain the pinned publisher/source-document scope",
        )?;
        let speakers: BTreeSet<_> = packet
            .cases
            .iter()
            .map(|c| c.publisher_record.original_speaker_id)
            .collect();
        let expected_coverage = serde_json::json!({"candidate_rows_selected":self.member_count,"candidate_rows_not_projected":0,"publisher_speaker_ids":speakers,"WAV_or_PCM_revalidated":false,"product_validate_or_run":"not_run","cleanup_gold":"not_applicable","training_eligibility":"not_assessed","holdout_certification":"unknown"});
        require(
            path,
            packet.coverage == expected_coverage,
            "metadata coverage cannot claim native/audio/training/holdout admission",
        )?;
        validate_members(path, packet, &proposal, &audit, self.member_count)
    }
}
fn validate_members(
    path: &Path,
    packet: &Packet,
    proposal: &Proposal,
    audit: &Audit,
    count: usize,
) -> Result<(), InputError> {
    let ids: Vec<_> = packet.cases.iter().map(|c| c.case_id.as_str()).collect();
    require(
        path,
        ids.len() == count
            && ids.iter().copied().collect::<BTreeSet<_>>().len() == count
            && proposal
                .member_ids
                .iter()
                .map(String::as_str)
                .eq(ids.iter().copied())
            && proposal.members.len() == count
            && audit.cases.len() == count,
        "all original ordered members required; duplicates, missing or reordered rows rejected",
    )?;
    for ((case, member), audited) in packet.cases.iter().zip(&proposal.members).zip(&audit.cases) {
        require(
            path,
            case.case_id == member.case_id
                && case.case_id == audited.case_id
                && member.parent_comparison == "match"
                && member.binding_state == "match"
                && member.reference == audited.reference_use
                && member.audio_receipt == audited.wav_receipt
                && audited.parent_comparison == "match"
                && audited.binding_state == "match"
                && audited.reason.is_none()
                && audited.field_matches.values().all(|v| *v)
                && audited.evidence_bindings.values().all(|v| *v),
            "member comparison/binding must retain independent match facts",
        )?;
        require(
            path,
            matches_candidate(case, &audited.converted_candidate_record, packet),
            "packet projected facts differ from the independently audited member",
        )?;
    }
    Ok(())
}
fn matches_candidate(p: &ProjectedCase, c: &Candidate, packet: &Packet) -> bool {
    use serde_json::json;
    let record = &p.publisher_record;
    let s = &p.source_locator;
    let a = &c.source_annotation;
    let expected_observations = json!({"embedded_audio_path":c.embedded_audio_path,"filename_components_observed":c.filename_components_observed,"raw_recording_group":c.raw_recording_group,"publisher_duration_minus_pcm_seconds":c.publisher_duration_minus_pcm_seconds,"conditions":c.conditions});
    let expected_groups = json!({"speaker":{"kind":"publisher_id","namespace":"CAiRE/ASCEND@737e9800ae31be9932ba8464c80366559bd28424","value":c.original_speaker_id.to_string(),"source_field":"original_speaker_id"},"recording":{"kind":"derived","value":c.raw_recording_group,"owner":"retained_public_candidate_projection","creator_identity":"not_observed_by_this_tool","independence":"unproven"},"conversation":{"kind":"unknown","reason":"global_conversation_and_partner_unavailable"},"template":{"kind":"unknown","reason":"topic_is_not_template_identity"}});
    let mut refs = serde_json::Map::new();
    for purpose in &packet.purposes {
        let (name, value) = match purpose {
            super::speech_input::Purpose::FunctionalUnscored => (
                "functional_unscored",
                json!({"kind":"not_provided","selection_reason":"functional_unscored_not_selected"}),
            ),
            super::speech_input::Purpose::PublisherReferenceAsr => (
                "publisher_reference_asr",
                json!({"kind":"publisher_asr","text":a.text,"text_sha256":a.text_sha256,"source_label_origin":a.origin,"annotation_protocol_id":"ascend-paper-v6","source_transform":"none","source_record_case_id":c.case_id}),
            ),
        };
        refs.insert(name.into(), value);
    }
    p.case_id == c.case_id
        && s.source_split == c.source_split
        && s.source_parquet == c.source_parquet
        && s.source_parquet_sha256 == c.source_parquet_sha256
        && s.row_ordinal == c.row_ordinal
        && s.source_id == c.source_id
        && record.id == c.source_id
        && record.path == c.source_path
        && record.transcription == a.text
        && record.duration == c.publisher_duration_seconds
        && record.language == c.language
        && record.original_speaker_id == c.original_speaker_id
        && record.session_id == c.session_id
        && record.topic == c.topic
        && p.source_observations == expected_observations
        && serde_json::to_value(&p.audio_identity_declared_not_revalidated)
            .ok()
            .as_ref()
            == Some(&c.audio)
        && serde_json::to_value(&p.conversion_declared).ok().as_ref() == Some(&c.conversion)
        && p.grouping == expected_groups
        && p.references_by_purpose == serde_json::Value::Object(refs)
        && p.audio_locator.owner_evidence_role == "candidates"
        && p.audio_locator.relative_path == c.adjacent_wav
        && p.audio_locator.access == "metadata_only_not_resolved"
        && p.project_partition == "development_observation"
        && p.project_content_review == "pending"
        && c.project_split == "dev"
        && c.source_split == "validation"
        && a.project_review_status == "pending"
        && a.origin == "publisher_manual_asr_annotation"
        && !a.contains_unk
        && v::sha256(a.text.as_bytes()) == a.text_sha256
        && c.conversation_partner_id.is_none()
        && c.conversation_pair_id.is_none()
        && c.global_conversation_session_id.is_none()
        && c.template_family_id.is_none()
}

#[cfg(test)]
mod tests {
    use super::super::speech_input::Purpose;
    use super::*;
    fn fixture() -> (Packet, Proposal, Audit) {
        let literal: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/speech_profile_member.json")).unwrap();
        let mut cases = Vec::new();
        let mut audited = Vec::new();
        let mut members = Vec::new();
        let mut ids = Vec::new();
        for i in 0..60 {
            let id = format!("explicit-fixture-{i}");
            ids.push(id.clone());
            let mut p = literal["projected"].clone();
            p["case_id"] = serde_json::json!(id);
            p["references_by_purpose"]["publisher_reference_asr"]["source_record_case_id"] =
                serde_json::json!(id);
            cases.push(p);
            let mut c = literal["candidate"].clone();
            c["case_id"] = serde_json::json!(id);
            let reference = serde_json::json!({"kind":"publisher_asr","text":"hello"});
            let receipt = serde_json::json!({"revalidated_now":false});
            members.push(serde_json::json!({"case_id":id,"parent_comparison":"match","binding_state":"match","reference":reference,"audio_receipt":receipt}));
            audited.push(serde_json::json!({"case_id":id,"parent_comparison":"match","field_matches":{"fixture_bound":true},"reason":null,"evidence_bindings":{"fixture_bound":true},"binding_state":"match","converted_candidate_record":c,"publisher_parent_record":{},"review_queue_record":{},"WAV_receipt":receipt,"reference_use":reference,"group_limits":{}}));
        }
        let packet=serde_json::from_value(serde_json::json!({"artifact_type":"asr_speech_source_packet","schema_version":1,"profile":"synthetic-not-admitted","state":"fixture","purposes":["publisher_reference_asr"],"source":{},"source_authority":{},"evidence":[],"metadata_snapshot_output_owner_sha256":"0".repeat(64),"case_count":60,"cases":cases,"coverage":{},"producer_source_observation":{}})).unwrap();
        let proposal=serde_json::from_value(serde_json::json!({"artifact_type":"asr_pilot60_native_cohort_proposal","schema_version":1,"profile":"synthetic-not-admitted","state":"fixture","member_ids":ids,"members":members,"existing_three_profile_unchanged":true,"admitted":false,"audit_sha256":"0".repeat(64)})).unwrap();
        let audit=serde_json::from_value(serde_json::json!({"artifact_type":"ascend_pilot60_publisher_audit","schema_version":1,"profile":"synthetic-not-admitted","state":"fixture","requested":60,"parent_comparison_counts":{"match":60,"mismatch":0,"unknown":0},"evidence_binding_conflicts":0,"cases":audited,"coverage":{},"source_authority":{},"evidence":[],"selected_plan_snapshot":{},"research_owner_source_identities":{},"typed_page_snapshots":[],"original_selection_audit":{},"claims":{}})).unwrap();
        (packet, proposal, audit)
    }
    #[test]
    fn all60_member_binding_rejects_missing_duplicate_reordered_and_changed_facts() {
        let path = Path::new("explicit-synthetic-member-binding");
        let (packet, proposal, audit) = fixture();
        assert!(validate_members(path, &packet, &proposal, &audit, 60).is_ok());
        assert!(Profile::select(path, &packet, "0").is_err()); // never admitted proof
        for mutation in 0..5 {
            let (mut packet, proposal, audit) = fixture();
            match mutation {
                0 => {
                    packet.cases.pop();
                }
                1 => packet.cases[59].case_id = packet.cases[0].case_id.clone(),
                2 => packet.cases.swap(0, 1),
                3 => packet.cases[59].publisher_record.transcription = "Hello".into(),
                _ => {
                    packet.cases[59].grouping["conversation"] =
                        serde_json::json!({"kind":"certified","value":"made-up"})
                }
            }
            assert!(validate_members(path, &packet, &proposal, &audit, 60).is_err());
        }
    }
    #[test]
    fn compiled_profiles_keep_fixed3_and_audited60_as_distinct_authorities() {
        let catalog: Catalog =
            serde_json::from_slice(include_bytes!("speech_profiles.json")).unwrap();
        assert_eq!(catalog.profiles.len(), 2);
        let fixed = &catalog.profiles[0];
        let cohort = &catalog.profiles[1];
        assert_eq!(fixed.member_count, 3);
        assert!(fixed.packet_sha256.is_some());
        assert!(fixed.cohort_proof.is_none());
        assert_eq!(cohort.member_count, 60);
        assert!(cohort.packet_sha256.is_none());
        assert_eq!(cohort.evidence_pins.len(), 10);
        let selected = vec!["synthetic-59".into(), "synthetic-0".into()];
        assert!(fixed.selection_coverage(selected.clone()).is_none());
        let coverage = cohort.selection_coverage(selected.clone()).unwrap();
        assert_eq!(coverage.selected_case_ids, selected);
        assert_eq!(coverage.cohort_members, 60);
        assert_eq!(coverage.selected_members, 2);
        assert_eq!(coverage.unselected_members, 58);
        assert_eq!(coverage.selection_order, "caller_index_order");
        let (packet, _, _) = fixture();
        let mut unknown = serde_json::to_value(&packet).unwrap();
        unknown["cohort_admitted"] = serde_json::json!(true);
        assert!(serde_json::from_value::<Packet>(unknown).is_err());
        assert_eq!(packet.purposes, vec![Purpose::PublisherReferenceAsr]);
    }
}
