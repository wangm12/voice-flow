//! Opt-in fixture facts from producing request/parsing boundaries. Ordinary calls
//! have no capture scope; poisoning or dropping a capture never changes delivery.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[cfg(any(test, feature = "evaluation"))]
use std::future::Future;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "presence", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObservedText {
    KnownEmpty {},
    KnownValue { value: String },
    Unknown { reason: String },
}
impl ObservedText {
    pub(crate) fn known(text: &str) -> Self {
        if text.is_empty() {
            Self::KnownEmpty {}
        } else {
            Self::KnownValue { value: text.into() }
        }
    }
    pub(crate) fn unknown(reason: &str) -> Self {
        Self::Unknown {
            reason: reason.into(),
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DroppedControl {
    pub field: String,
    pub reason: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedRequest {
    pub protocol: String,
    pub endpoint_identity: String,
    pub requested_model: String,
    pub constructed_model: String,
    /// Constructed payload/config fields; not proof of transport or server receipt.
    pub fields_constructed: BTreeMap<String, serde_json::Value>,
    pub dropped_controls: Vec<DroppedControl>,
    pub decode_metadata: BTreeMap<String, ObservedText>,
}
impl ResolvedRequest {
    pub(crate) fn new(
        protocol: &str,
        endpoint: &str,
        requested_model: &str,
        constructed_model: &str,
    ) -> Self {
        let endpoint_identity = reqwest::Url::parse(endpoint)
            .map(|url| format!("{}{}", url.origin().ascii_serialization(), url.path()))
            .unwrap_or_else(|_| "local_runtime".into());
        Self {
            protocol: protocol.into(),
            endpoint_identity,
            requested_model: requested_model.into(),
            constructed_model: constructed_model.into(),
            fields_constructed: BTreeMap::new(),
            dropped_controls: vec![],
            decode_metadata: BTreeMap::from([(
                "provider_defaults".into(),
                ObservedText::unknown("not_reported_by_provider"),
            )]),
        }
    }
    pub(crate) fn drop_control(&mut self, field: &str, reason: &str) {
        self.dropped_controls.push(DroppedControl {
            field: field.into(),
            reason: reason.into(),
        });
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AsrObservation {
    pub provider_text: ObservedText,
    /// Parsed representation, when the adapter distinguishes it; not inferred from text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_text_source_kind: Option<String>,
    pub filtered_text: ObservedText,
    pub origin: String,
    pub terminal: String,
    pub filtering_reason: Option<String>,
    pub protocol_completion: String,
    pub reported_model: ObservedText,
}
#[cfg(any(test, feature = "evaluation"))]
impl AsrObservation {
    pub(crate) fn confirmed_empty(&self) -> bool {
        matches!(self.filtered_text, ObservedText::KnownEmpty {})
            && match self.terminal.as_str() {
                "provider_empty" => matches!(self.provider_text, ObservedText::KnownEmpty {}),
                "local_complete_empty" => {
                    self.origin == "local_model_output"
                        && self.protocol_completion == "complete"
                        && matches!(self.provider_text, ObservedText::KnownEmpty {})
                }
                "locally_suppressed" => {
                    matches!(self.provider_text, ObservedText::KnownValue { .. })
                }
                _ => false,
            }
    }
}
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalStage {
    ModelAdmission,
    FilesValidated,
    RuntimeSession,
    RuntimeHandshake,
    ModelLoad,
    Inference,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalStageState {
    Started,
    Completed,
    Failed,
    Unknown,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalStageEvent {
    pub stage: LocalStage,
    pub state: LocalStageState,
    pub reason: Option<String>,
    pub runtime_version: Option<String>,
    pub elapsed_ms: f64,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalCoverage {
    pub expected_samples: u64,
    pub covered_samples: u64,
    pub chunks_completed: usize,
    pub chunks_total: usize,
    pub generation_tokens: usize,
    pub maximum_tokens: usize,
    pub completion: String,
    pub accepted: bool,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateAudioResourceFact {
    pub selected_path: String,
    pub action: String,
    pub path_applicable: Option<bool>,
    pub reason: Option<String>,
    pub last_selected_relative_name: Option<String>,
    pub displaced_unknown_relative_name: Option<String>,
}
pub(crate) fn private_audio_resource(
    path: &std::path::Path,
    action: &str,
    applicable: Option<bool>,
    reason: Option<&str>,
    last_selected_name: Option<&str>,
    displaced_unknown_name: Option<&str>,
) {
    observe(|record, _| {
        if let Some(local) = &mut record.local_asr {
            if local.private_audio_resources.len() < 16 {
                local
                    .private_audio_resources
                    .push(PrivateAudioResourceFact {
                        selected_path: path.display().to_string(),
                        action: action.into(),
                        path_applicable: applicable,
                        reason: reason.map(str::to_owned),
                        last_selected_relative_name: last_selected_name.map(str::to_owned),
                        displaced_unknown_relative_name: displaced_unknown_name.map(str::to_owned),
                    });
            } else {
                local.events_truncated = true;
            }
        }
    });
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalAsrObservation {
    pub requested_model_id: String,
    pub requested_revision: String,
    pub observed_model_id: Option<String>,
    pub observed_revision: Option<String>,
    pub identity_checks: Vec<LocalIdentityCheck>,
    pub private_audio_resources: Vec<PrivateAudioResourceFact>,
    pub events: Vec<LocalStageEvent>,
    pub events_truncated: bool,
    pub coverage: Option<LocalCoverage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded_attestation: Option<LoadedAttestation>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalIdentityCheck {
    pub stage: LocalStage,
    pub observed_model_id: Option<String>,
    pub observed_revision: Option<String>,
    pub applicable: bool,
    pub reason: Option<String>,
}
pub(crate) fn local_identity_check(
    model: &str,
    revision: &str,
    stage: LocalStage,
    actual_id: Option<&str>,
    actual_revision: Option<&str>,
    reason: Option<&str>,
) {
    local_observe(model, revision, |r| {
        if r.identity_checks.len() < 8 {
            r.identity_checks.push(LocalIdentityCheck {
                stage,
                observed_model_id: actual_id.map(str::to_owned),
                observed_revision: actual_revision.map(str::to_owned),
                applicable: reason.is_none(),
                reason: reason.map(str::to_owned),
            });
        } else {
            r.events_truncated = true;
        }
    });
}
fn local_observe(model: &str, revision: &str, update: impl FnOnce(&mut LocalAsrObservation)) {
    observe(|record, _| {
        let value = record.local_asr.get_or_insert_with(|| LocalAsrObservation {
            requested_model_id: model.into(),
            requested_revision: revision.into(),
            observed_model_id: None,
            observed_revision: None,
            identity_checks: vec![],
            private_audio_resources: vec![],
            events: vec![],
            events_truncated: false,
            coverage: None,
            loaded_attestation: None,
        });
        update(value);
    });
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LoadedAttestation {
    pub schema_version: u32,
    pub executable_sha256: String,
    pub metal_sha256: BTreeMap<String, String>,
    pub loaded_images: Vec<String>,
    pub parameter_layout_sha256: String,
    pub parameter_count: usize,
    pub parameter_dtypes: BTreeMap<String, usize>,
    pub decoder_recipe: String,
}
impl LoadedAttestation {
    #[cfg(any(test, feature = "evaluation"))]
    pub(crate) fn complete(&self) -> bool {
        let digest = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
        self.schema_version == 1 && digest(&self.executable_sha256)
            && digest(&self.parameter_layout_sha256)
            && self.parameter_count > 0 && self.parameter_count <= 100_000
            && !self.parameter_dtypes.is_empty() && self.parameter_dtypes.len() <= 16
            && self.parameter_dtypes.values().try_fold(0usize, |a, b| a.checked_add(*b)) == Some(self.parameter_count)
            && self.parameter_dtypes.iter().all(|(k,v)| !k.is_empty() && k.len() <= 32 && *v > 0)
            && self.metal_sha256.len() == 2
            && ["mlx.metallib", "mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib"].iter().all(|k| self.metal_sha256.get(*k).is_some_and(|v| digest(v)))
            && !self.loaded_images.is_empty() && self.loaded_images.len() <= 4096
            && self.loaded_images.windows(2).all(|p| p[0] < p[1])
            && self.loaded_images.iter().all(|s| s.len() <= 8192 && s.rsplit_once('|').is_some_and(|(name,uuid)| !name.is_empty() && uuid.len() == 36))
            && self
                .loaded_images
                .iter()
                .try_fold(0usize, |total, image| total.checked_add(image.len()))
                .is_some_and(|total| total <= 256 * 1024)
            && self.decoder_recipe == "voiceflow-mlx-decoder-v1:chunks30s/min1s;temperature0;context-empty;qwen-repetition1.05/window32;inner-chunk1200s;cohere-topP1/topK0/vad-nil;ordered-trim-language-join"
    }
}
pub(crate) fn local_attestation(
    model: &str,
    revision: &str,
    attestation: Option<LoadedAttestation>,
) {
    local_observe(model, revision, |r| r.loaded_attestation = attestation);
}
pub(crate) fn local_loaded_identity(
    model: &str,
    revision: &str,
    actual_model: &str,
    actual_revision: &str,
) {
    local_observe(model, revision, |r| {
        r.observed_model_id = Some(actual_model.into());
        r.observed_revision = Some(actual_revision.into());
    });
}
pub(crate) fn local_coverage(model: &str, revision: &str, coverage: LocalCoverage) {
    local_observe(model, revision, |r| r.coverage = Some(coverage));
}
/// Observation only: dropping this guard records an unknown local terminal,
/// never changes the runtime, result, cancellation token or child ownership.
pub(crate) struct LocalStageGuard(Option<(String, String, LocalStage, std::time::Instant)>);
impl LocalStageGuard {
    pub(crate) fn start(model: &str, revision: &str, stage: LocalStage) -> Self {
        if !enabled() {
            return Self(None);
        }
        local_observe(model, revision, |r| {
            if r.events.len() < 32 {
                r.events.push(LocalStageEvent {
                    stage,
                    state: LocalStageState::Started,
                    reason: None,
                    runtime_version: None,
                    elapsed_ms: 0.0,
                });
            } else {
                r.events_truncated = true;
            }
        });
        Self(Some((
            model.into(),
            revision.into(),
            stage,
            std::time::Instant::now(),
        )))
    }
    fn finish(&mut self, state: LocalStageState, reason: Option<&str>, version: Option<&str>) {
        if let Some((model, revision, stage, start)) = self.0.take() {
            local_observe(&model, &revision, |r| {
                if r.events.len() < 32 {
                    r.events.push(LocalStageEvent {
                        stage,
                        state,
                        reason: reason.map(str::to_owned),
                        runtime_version: version.map(str::to_owned),
                        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                    });
                } else {
                    r.events_truncated = true;
                }
            });
        }
    }
    pub(crate) fn completed(&mut self, reason: Option<&str>, version: Option<&str>) {
        self.finish(LocalStageState::Completed, reason, version);
    }
    pub(crate) fn failed(&mut self, reason: &str) {
        self.finish(LocalStageState::Failed, Some(reason), None);
    }
}
impl Drop for LocalStageGuard {
    fn drop(&mut self) {
        self.finish(
            LocalStageState::Unknown,
            Some("local_future_dropped_or_terminal_not_observed"),
            None,
        );
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupObservation {
    pub provider_reply: ObservedText,
    pub accepted_text: ObservedText,
    pub completion_kind: String,
    /// Client response-owner protocol success, independent of text acceptance.
    /// Missing historical/cancelled/failed observations remain unknown.
    pub protocol_complete: Option<bool>,
    pub reported_model: ObservedText,
    pub usage: BTreeMap<String, f64>,
    pub transformations: Vec<String>,
    pub adapter_decision: String,
}
impl Default for CleanupObservation {
    fn default() -> Self {
        Self {
            provider_reply: ObservedText::unknown("not_observed"),
            accepted_text: ObservedText::unknown("not_accepted"),
            completion_kind: "not_observed".into(),
            protocol_complete: None,
            reported_model: ObservedText::unknown("provider_not_reported"),
            usage: BTreeMap::new(),
            transformations: vec![],
            adapter_decision: "not_resolved".into(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub(crate) struct ObservationRecord {
    requested_asr_model: Option<String>,
    #[cfg(any(test, feature = "evaluation"))]
    pub loss_reason: Option<String>,
    pub local_asr: Option<LocalAsrObservation>,
    pub asr_request: Option<ResolvedRequest>,
    pub asr: Option<AsrObservation>,
    pub cleanup_request: Option<ResolvedRequest>,
    pub cleanup: Option<CleanupObservation>,
}
#[derive(Clone)]
pub(crate) struct FixtureCapture {
    record: Arc<Mutex<ObservationRecord>>,
    capture_reply: bool,
}
tokio::task_local! { static CAPTURE: FixtureCapture; }
#[cfg(any(test, feature = "evaluation"))]
impl FixtureCapture {
    pub(crate) fn new(capture_reply: bool) -> Self {
        Self {
            record: Arc::new(Mutex::new(ObservationRecord::default())),
            capture_reply,
        }
    }
    pub(crate) async fn scope<F: Future>(&self, future: F) -> F::Output {
        CAPTURE.scope(self.clone(), future).await
    }
    pub(crate) fn current() -> Option<Self> {
        CAPTURE.try_with(|capture| capture.clone()).ok()
    }
    #[cfg(test)]
    pub(crate) fn poison_for_test(&self) {
        let record = self.record.clone();
        let _ = std::thread::spawn(move || {
            let _guard = record.lock().unwrap();
            panic!("controlled capture failure");
        })
        .join();
    }
    pub(crate) fn snapshot(&self) -> ObservationRecord {
        self.record
            .lock()
            .map(|r| r.clone())
            .unwrap_or_else(|_| ObservationRecord {
                loss_reason: Some("capture_unavailable".into()),
                ..Default::default()
            })
    }
}
pub(crate) fn enabled() -> bool {
    CAPTURE.try_with(|_| true).unwrap_or(false)
}
fn observe(update: impl FnOnce(&mut ObservationRecord, bool)) {
    let _ = CAPTURE.try_with(|capture| {
        if let Ok(mut record) = capture.record.lock() {
            update(&mut record, capture.capture_reply);
        }
    });
}
pub(crate) fn requested_asr_model(model: &str) {
    observe(|record, _| record.requested_asr_model = Some(model.into()));
}
pub(crate) fn asr_request(mut request: ResolvedRequest) {
    observe(|record, _| {
        if let Some(model) = &record.requested_asr_model {
            request.requested_model = model.clone();
        }
        record.asr_request = Some(request);
    });
}
pub(crate) fn asr(observation: AsrObservation) {
    observe(|record, _| record.asr = Some(observation));
}
pub(crate) fn cleanup_request(request: ResolvedRequest) {
    observe(|record, _| {
        record.cleanup_request = Some(request);
        record
            .cleanup
            .get_or_insert_with(Default::default)
            .protocol_complete = None;
    });
}
pub(crate) fn cleanup(update: impl FnOnce(&mut CleanupObservation, bool)) {
    observe(|record, capture| update(record.cleanup.get_or_insert_with(Default::default), capture));
}
pub(crate) fn completion(reply: &str, kind: &str) {
    cleanup(|record, capture| {
        record.provider_reply = if capture {
            ObservedText::known(reply)
        } else {
            ObservedText::unknown("capture_disabled")
        };
        record.completion_kind = kind.into();
    });
}
/// Only the complete response consumer may call this, after protocol validation
/// and before adapter acceptance. A stop chunk alone is insufficient.
pub(crate) fn cleanup_protocol_completed() {
    cleanup(|record, _| record.protocol_complete = Some(true));
}

pub(crate) fn asr_filter(raw: &str, filtered: &str, terminal: &str, reason: Option<&str>) {
    observe(|record, _| {
        let mut observation = record.asr.take().unwrap_or_else(|| AsrObservation {
            provider_text: ObservedText::known(raw),
            provider_text_source_kind: None,
            filtered_text: ObservedText::unknown("not_filtered"),
            origin: "provider_response".into(),
            terminal: "parsed_not_filtered".into(),
            filtering_reason: None,
            protocol_completion: "parser_complete".into(),
            reported_model: ObservedText::unknown("provider_not_reported"),
        });
        if terminal == "provider_empty"
            && matches!(observation.provider_text, ObservedText::Unknown { .. })
        {
            observation.filtered_text = ObservedText::unknown("provider_text_missing");
            observation.terminal = "empty_origin_unknown".into();
            observation.filtering_reason = Some("provider_text_missing".into());
        } else {
            observation.filtered_text = ObservedText::known(filtered);
            observation.terminal = terminal.into();
            observation.filtering_reason = reason.map(str::to_owned);
        }
        record.asr = Some(observation);
    });
}
pub(crate) fn asr_failure(terminal: &str, reason: &str) {
    observe(|record, _| {
        if let Some(asr) = &mut record.asr {
            asr.terminal = terminal.into();
            asr.filtering_reason = Some(reason.into());
            asr.filtered_text = ObservedText::unknown("response_not_accepted");
        }
    });
}

#[cfg(test)]
mod loaded_attestation_image_bounds_tests {
    use super::LoadedAttestation;
    use std::collections::BTreeMap;

    const DECODER_RECIPE: &str = concat!(
        "voiceflow-mlx-decoder-v1:chunks30s/min1s;temperature0;context-empty;",
        "qwen-repetition1.05/window32;inner-chunk1200s;cohere-topP1/topK0/vad-nil;",
        "ordered-trim-language-join",
    );
    const UUID: &str = "00000000-0000-0000-0000-000000000000";

    fn image(index: usize) -> String {
        format!("/dyld/{index:04x}|00000000-0000-0000-0000-{index:012x}")
    }

    fn attestation(loaded_images: Vec<String>) -> LoadedAttestation {
        LoadedAttestation {
            schema_version: 1,
            executable_sha256: "a".repeat(64),
            metal_sha256: BTreeMap::from([
                ("mlx.metallib".into(), "b".repeat(64)),
                (
                    "mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib".into(),
                    "c".repeat(64),
                ),
            ]),
            loaded_images,
            parameter_layout_sha256: "d".repeat(64),
            parameter_count: 1,
            parameter_dtypes: BTreeMap::from([("float32".into(), 1)]),
            decoder_recipe: DECODER_RECIPE.into(),
        }
    }

    #[test]
    fn complete_inventory_above_old_limit_is_preserved() {
        let images = (0..1_145).map(image).collect::<Vec<_>>();
        let original = attestation(images.clone());
        assert!(original.complete());
        let encoded = serde_json::to_vec(&original).unwrap();
        let decoded: LoadedAttestation = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.loaded_images, images);
        assert!(decoded.complete());
    }

    #[test]
    fn count_limit_accepts_4096_and_rejects_4097() {
        assert!(attestation((0..4_096).map(image).collect()).complete());
        assert!(!attestation((0..4_097).map(image).collect()).complete());
    }

    #[test]
    fn utf8_entry_and_total_byte_limits_are_inclusive_and_fail_closed() {
        let exact_entry = format!("{}|{UUID}", "x".repeat(8_155));
        assert_eq!(exact_entry.len(), 8_192);
        assert!(attestation(vec![exact_entry]).complete());

        let over_entry = format!("{}|{UUID}", "x".repeat(8_156));
        assert_eq!(over_entry.len(), 8_193);
        assert!(!attestation(vec![over_entry]).complete());

        let multibyte_over_entry = format!("{}|{UUID}", "é".repeat(4_078));
        assert_eq!(multibyte_over_entry.len(), 8_193);
        assert!(!attestation(vec![multibyte_over_entry]).complete());

        let exact_total = (0..32)
            .map(|index| format!("{:02}{}|{UUID}", index, "a".repeat(8_153)))
            .collect::<Vec<_>>();
        assert!(exact_total.iter().all(|image| image.len() == 8_192));
        assert_eq!(
            exact_total.iter().map(String::len).sum::<usize>(),
            256 * 1024
        );
        assert!(attestation(exact_total.clone()).complete());
        let mut over_total = exact_total;
        over_total.push(format!("32|{UUID}"));
        assert!(!attestation(over_total).complete());
    }

    #[test]
    fn malformed_image_identity_stays_rejected() {
        assert!(!attestation(vec!["/dyld/image-without-uuid".into()]).complete());
        assert!(!attestation(vec!["/dyld/image|short".into()]).complete());
    }
}
