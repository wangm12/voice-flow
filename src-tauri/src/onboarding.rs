//! Session-local onboarding identity. Dictation owns recording/cancellation;
//! this owner only binds a trial lease to those existing sessions.
use crate::{dictation, lock_recover, permissions, AppState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{atomic::Ordering, Arc, Mutex};
use tauri::{Emitter, Manager, State};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct TrialLease {
    pub trial_id: String,
    pub config_generation: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrialSnapshot {
    pub lease: TrialLease,
    pub recording_generation: u64,
    pub trigger_source: &'static str,
    permissions: (bool, bool, bool),
}
#[derive(Default)]
pub(crate) struct TrialOwner {
    lease: Option<TrialLease>,
    selected_text: Option<String>,
    revoking: bool,
    external: Option<ExternalContinuation>,
}
impl TrialOwner {
    fn matches(&self, lease: &TrialLease) -> bool {
        !self.revoking && self.lease.as_ref() == Some(lease)
    }
    fn release_matches(&self, id: &str) -> bool {
        self.lease
            .as_ref()
            .is_some_and(|lease| lease.trial_id == id)
    }
    fn begin_revocation(&mut self) {
        self.external = None;
        self.revoking = true;
        self.selected_text = None;
    }
    fn replace(&mut self, lease: Option<TrialLease>) {
        self.external = None;
        self.lease = lease;
        self.selected_text = None;
        self.revoking = false;
    }
}
#[derive(Serialize)]
pub(crate) struct LeaseReceipt {
    trial_id: String,
    config_generation: String,
    enabled: bool,
}
pub(crate) fn validate_id(id: &str) -> Result<(), String> {
    if id.len() != 36
        || !id.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        })
        || !matches!(id.as_bytes().get(14), Some(b'1'..=b'8'))
        || !matches!(id.as_bytes().get(19), Some(b'8' | b'9' | b'a' | b'b'))
        || id == "00000000-0000-0000-0000-000000000000"
    {
        return Err("trialId must be a canonical non-nil lowercase UUID".into());
    }
    Ok(())
}
fn permission_identity() -> (bool, bool, bool) {
    let value = permissions::check();
    (
        value.microphone,
        value.accessibility,
        value.screen_recording,
    )
}
pub(crate) fn generation(state: &AppState) -> String {
    state
        .processing_configuration_generation
        .load(Ordering::Acquire)
        .to_string()
}
/// Called at the session claim boundary, before any asynchronous preparation.
pub(crate) fn snapshot(state: &AppState, trigger_source: &'static str) -> Option<TrialSnapshot> {
    let _settings = lock_recover(&state.settings);
    let lease = lock_recover(&state.onboarding_trial).lease.clone()?;
    // Retain a stale lease as a preview-only route. It must never turn into an
    // external delivery merely because its configuration was superseded.
    Some(TrialSnapshot {
        lease,
        recording_generation: 0,
        trigger_source,
        permissions: permission_identity(),
    })
}
pub(crate) fn is_current(state: &AppState, trial: &TrialSnapshot) -> bool {
    let _settings = lock_recover(&state.settings);
    let owner = lock_recover(&state.onboarding_trial);
    owner.matches(&trial.lease)
        && generation(state) == trial.lease.config_generation
        && permission_identity() == trial.permissions
        && lock_recover(&state.manager).onboarding_trial.as_ref() == Some(trial)
}
pub(crate) fn take_selected_text(
    state: &AppState,
    expected: &TrialSnapshot,
    recording_generation: u64,
) -> Result<Option<String>, ()> {
    let _settings = lock_recover(&state.settings);
    let mut owner = lock_recover(&state.onboarding_trial);
    let manager = lock_recover(&state.manager);
    if generation(state) != expected.lease.config_generation
        || permission_identity() != expected.permissions
    {
        return Err(());
    }
    owner.take_selected_for_start(&manager, expected, recording_generation)
}
impl TrialOwner {
    fn take_selected_for_start(
        &mut self,
        manager: &dictation::DictationManager,
        expected: &TrialSnapshot,
        recording_generation: u64,
    ) -> Result<Option<String>, ()> {
        if !self.matches(&expected.lease)
            || !trial_start_matches(manager, expected, recording_generation)
        {
            return Err(());
        }
        Ok(self.selected_text.take())
    }
}
fn trial_start_matches(
    manager: &dictation::DictationManager,
    expected: &TrialSnapshot,
    recording_generation: u64,
) -> bool {
    manager.phase == dictation::Phase::Starting
        && manager.session_generation == recording_generation
        && expected.recording_generation == recording_generation
        && manager.onboarding_trial.as_ref() == Some(expected)
        && !manager.cancellation.is_cancelled()
}
pub(crate) fn trial_operation_matches(
    manager: &dictation::DictationManager,
    trial: &TrialSnapshot,
    identity: &crate::TextActionIdentity,
) -> bool {
    identity.recording_generation == trial.recording_generation
        && manager.onboarding_trial.as_ref() == Some(trial)
        && manager.phase == dictation::Phase::Processing
        && manager.session_generation == identity.recording_generation.wrapping_add(1)
        && !manager.cancellation.is_cancelled()
}
/// Only captured operation metadata can certify an onboarding event. Generic
/// HUD emitters intentionally have no access to a current-global trial identity.
#[derive(Clone, Copy)]
pub(crate) enum EventValidity {
    LiveResult,
    CurrentRequired,
    CurrentRecording,
    IdentityOwned,
}
/// Hold the parsed owner through dispatch. A cancelled A cannot acquire B's
/// identity, even if the same lease starts a newer recording between checks.
pub(crate) fn emit_trial_payload(
    app: &tauri::AppHandle,
    state: &AppState,
    event: &str,
    mut payload: serde_json::Value,
    trial: &TrialSnapshot,
    validity: EventValidity,
) -> bool {
    let _settings = lock_recover(&state.settings);
    let owner = lock_recover(&state.onboarding_trial);
    let manager = lock_recover(&state.manager);
    if manager.onboarding_trial.as_ref() != Some(trial) {
        return false;
    }
    let current = owner.matches(&trial.lease)
        && generation(state) == trial.lease.config_generation
        && permission_identity() == trial.permissions;
    if matches!(
        validity,
        EventValidity::CurrentRequired
            | EventValidity::CurrentRecording
            | EventValidity::LiveResult
    ) && (!current || payload["session_generation"].as_u64() != Some(manager.session_generation))
    {
        return false;
    }
    if matches!(validity, EventValidity::CurrentRecording)
        && !recording_event_is_current(
            payload["session_generation"].as_u64(),
            manager.session_generation,
            manager.phase,
            manager.cancellation.is_cancelled(),
        )
    {
        return false;
    }
    if matches!(validity, EventValidity::LiveResult)
        && (manager.phase != dictation::Phase::Processing || manager.cancellation.is_cancelled())
    {
        return false;
    }
    attach_snapshot(&mut payload, trial);
    payload["trial_current"] = serde_json::json!(current);
    // Trial receipts belong to the onboarding window; sparse source receipts
    // must not overwrite the separate global HUD presentation payload.
    let _ = app.emit_to("main", event, payload);
    true
}

fn recording_event_is_current(
    payload_generation: Option<u64>,
    current_generation: u64,
    phase: dictation::Phase,
    cancelled: bool,
) -> bool {
    payload_generation == Some(current_generation)
        && phase == dictation::Phase::Recording
        && !cancelled
}
pub(crate) fn emit_state(
    app: &tauri::AppHandle,
    state: &AppState,
    event: &str,
    phase: &str,
    session_generation: u64,
    trial: Option<&TrialSnapshot>,
) {
    let Some(trial) = trial else {
        return;
    };
    let payload = serde_json::json!({"state": phase, "session_generation": session_generation});
    emit_trial_payload(
        app,
        state,
        event,
        payload,
        trial,
        if phase == "recording" {
            EventValidity::CurrentRecording
        } else {
            EventValidity::IdentityOwned
        },
    );
}
pub(crate) fn attach_snapshot(payload: &mut serde_json::Value, trial: &TrialSnapshot) {
    payload["trial_id"] = serde_json::json!(trial.lease.trial_id);
    payload["config_generation"] = serde_json::json!(trial.lease.config_generation);
    payload["recording_generation"] = serde_json::json!(trial.recording_generation.to_string());
    payload["trigger_source"] = serde_json::json!(trial.trigger_source);
}
pub(crate) fn emit_result(
    app: &tauri::AppHandle,
    state: &AppState,
    event: &str,
    trial: &TrialSnapshot,
    session_generation: u64,
    raw: &str,
    final_text: &str,
) -> bool {
    if crate::processing_aborted(state, session_generation) {
        return false;
    }
    let payload = serde_json::json!({"raw_text": raw, "final_text": final_text,
        "session_generation": session_generation,
        "delivery_method": "onboarding", "external_delivery": "not_tested"});
    emit_trial_payload(app, state, event, payload, trial, EventValidity::LiveResult)
}
pub(crate) fn emit_error(
    app: &tauri::AppHandle,
    state: &AppState,
    generation: u64,
    code: &str,
    message: &str,
    trial: Option<&TrialSnapshot>,
) {
    let Some(trial) = trial else {
        return;
    };
    let payload = serde_json::json!({"code": code, "message": message,
        "session_generation": generation});
    emit_trial_payload(
        app,
        state,
        "onboarding://error",
        payload,
        trial,
        EventValidity::CurrentRequired,
    );
}
#[tauri::command]
pub(crate) fn get_onboarding_configuration_generation(state: State<'_, AppState>) -> String {
    let _settings = lock_recover(&state.settings);
    generation(&state)
}
#[tauri::command]
pub(crate) async fn set_onboarding_test_mode(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
    trial_id: String,
) -> Result<LeaseReceipt, String> {
    validate_id(&trial_id)?;
    // Serializes replacement/release through completion of native cancellation.
    // A stale React cleanup cannot revoke or cancel a newer lease.
    let _gate = state.onboarding_gate.lock().await;
    let current = lock_recover(&state.onboarding_trial).lease.clone();
    if enabled
        && current.as_ref().is_some_and(|value| {
            value.trial_id == trial_id && value.config_generation == generation(&state)
        })
    {
        return Ok(LeaseReceipt {
            trial_id,
            config_generation: current.as_ref().unwrap().config_generation.clone(),
            enabled: true,
        });
    }
    if !enabled && !lock_recover(&state.onboarding_trial).release_matches(&trial_id) {
        return Ok(LeaseReceipt {
            trial_id,
            config_generation: generation(&state),
            enabled: false,
        });
    }
    let active_trial = { lock_recover(&state.manager).onboarding_trial.clone() };
    if enabled
        && lock_recover(&state.manager).phase != dictation::Phase::Idle
        && active_trial.is_none()
    {
        return Err(
            "A normal dictation session is active; cancel it before starting a trial".into(),
        );
    }
    // Keep the revoked identity as a preview route during cancellation. A
    // native input in the replacement interval cannot become external paste.
    lock_recover(&state.onboarding_trial).begin_revocation();
    if let Some(trial) = active_trial.as_ref() {
        dictation::cancel_onboarding_trial(&app, &state, trial).await;
    }
    let (config_generation, late_trial) = {
        let _input_gate = state.hotkey_gate.lock().await;
        let _settings = lock_recover(&state.settings);
        let config_generation = generation(&state);
        let late_trial = { lock_recover(&state.manager).onboarding_trial.clone() };
        let mut owner = lock_recover(&state.onboarding_trial);
        owner.replace(enabled.then(|| TrialLease {
            trial_id: trial_id.clone(),
            config_generation: config_generation.clone(),
        }));
        (config_generation, late_trial)
    };
    if let Some(trial) = late_trial.filter(|trial| current.as_ref() == Some(&trial.lease)) {
        dictation::cancel_onboarding_trial(&app, &state, &trial).await;
    }
    Ok(LeaseReceipt {
        trial_id,
        config_generation,
        enabled,
    })
}
#[tauri::command]
pub(crate) fn set_onboarding_selected_text(
    state: State<'_, AppState>,
    text: String,
    trial_id: String,
) -> Result<(), String> {
    validate_id(&trial_id)?;
    let _settings = lock_recover(&state.settings);
    let mut owner = lock_recover(&state.onboarding_trial);
    if owner.revoking
        || !owner.lease.as_ref().is_some_and(|lease| {
            lease.trial_id == trial_id && lease.config_generation == generation(&state)
        })
    {
        return Err("onboarding trial lease is stale or inactive".into());
    }
    owner.selected_text = (!text.trim().is_empty()).then_some(text);
    Ok(())
}
/// Main-window close or process exit owns unconditional revocation.
pub(crate) fn revoke(state: &AppState) {
    let mut owner = lock_recover(&state.onboarding_trial);
    owner.replace(None);
}

// Explicit, user-prepared TextEdit fixture. This is a continuation of TrialOwner,
// not a second runtime/session owner: DictationManager + TextActionControl own
// the actual claim, cancellation, commit gate and joined delivery resource.
const EXTERNAL_TTL: std::time::Duration = std::time::Duration::from_secs(180);
const CHROME_FIXTURE_NONCE: &str = "f45d5da3-fe13-445c-b105-acf6a1894c64";
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExternalTargetApp {
    TextEdit,
    Chrome,
}
impl ExternalTargetApp {
    fn bundle_id(self) -> &'static str {
        match self {
            Self::TextEdit => "com.apple.TextEdit",
            Self::Chrome => "com.google.Chrome",
        }
    }
}
const EXTERNAL_FIXTURE_TEXT_LIMIT_UTF16: usize = 512;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExternalHistorySourceRequest {
    dictation_id: i64,
    revision_id: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
struct ExternalSourceIdentity {
    kind: &'static str,
    dictation_id: i64,
    revision_id: i64,
    text_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ExternalFixture {
    lease: TrialLease,
    target_app: ExternalTargetApp,
    before: String,
    selected: String,
    inserted: String,
    after: String,
    selection_start_utf16: i64,
    preparation_ms: u64,
    switch_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_identity: Option<ExternalSourceIdentity>,
}
impl ExternalFixture {
    fn new(lease: TrialLease, target_app: ExternalTargetApp) -> Self {
        Self::with_inserted(
            lease,
            target_app,
            "VoiceFlow 外部插入一次。".to_owned(),
            None,
        )
    }

    fn with_inserted(
        lease: TrialLease,
        target_app: ExternalTargetApp,
        inserted: String,
        source_identity: Option<ExternalSourceIdentity>,
    ) -> Self {
        let selected = "REPLACE ONLY THIS".to_owned();
        let (prefix, suffix) = match target_app {
            ExternalTargetApp::TextEdit => (
                format!("VoiceFlow controlled fixture {}\n前缀🙂[", lease.trial_id),
                "]后缀\nEnd of controlled fixture.".to_owned(),
            ),
            ExternalTargetApp::Chrome => (
                format!("VoiceFlow V7 oracle {CHROME_FIXTURE_NONCE}\n前缀🙂["),
                format!("]后缀\nEnd {CHROME_FIXTURE_NONCE}"),
            ),
        };
        Self {
            target_app,
            before: format!("{prefix}{selected}{suffix}"),
            after: format!("{prefix}{inserted}{suffix}"),
            selection_start_utf16: prefix.encode_utf16().count() as i64,
            selected,
            inserted,
            lease,
            preparation_ms: 180_000,
            switch_ms: 5_000,
            source_identity,
        }
    }

    fn fits_ui_payload(&self) -> bool {
        [
            self.before.as_str(),
            self.selected.as_str(),
            self.inserted.as_str(),
            self.after.as_str(),
        ]
        .into_iter()
        .all(|text| text.encode_utf16().count() <= EXTERNAL_FIXTURE_TEXT_LIMIT_UTF16)
    }
    fn admits_source(&self, full: &str, source: &crate::paste::CapturedTextActionSource) -> bool {
        full == self.before
            && source.kind == crate::paste::TextActionSourceKind::Selection
            && source.text == self.selected
            && source.selection_range
                == Some((
                    self.selection_start_utf16,
                    self.selected.encode_utf16().count() as i64,
                ))
            && source.editable
            && !source.copy_only_selection
    }
}

fn external_history_identity(
    revision: &crate::store::HistoryRevision,
) -> Result<ExternalSourceIdentity, String> {
    let intent = revision
        .intent
        .as_ref()
        .ok_or("external_history_revision_not_cleanup")?;
    if revision.dictation_id <= 0
        || revision.revision_id <= 0
        || revision.revision_reason != "ai_reclean"
        || revision.cleanup_status.as_deref() != Some("ai_success")
        || intent.get("operation").and_then(serde_json::Value::as_str) != Some("cleanup")
        || intent.get("source").and_then(serde_json::Value::as_str) != Some("selected_text")
        || revision.final_text.trim().is_empty()
        || revision.final_text.encode_utf16().count() > EXTERNAL_FIXTURE_TEXT_LIMIT_UTF16
    {
        return Err("external_history_revision_not_applicable".into());
    }
    Ok(ExternalSourceIdentity {
        kind: "history_revision",
        dictation_id: revision.dictation_id,
        revision_id: revision.revision_id,
        text_sha256: hex::encode(Sha256::digest(revision.final_text.as_bytes())),
    })
}

fn read_external_history_revision(
    app: &tauri::AppHandle,
    dictation_id: i64,
    revision_id: i64,
) -> Result<crate::store::HistoryRevision, String> {
    if dictation_id <= 0 || revision_id <= 0 {
        return Err("external_history_revision_not_found".into());
    }
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|_| "external_history_revision_unavailable")?;
    crate::store::get_history_revision(&dir, dictation_id, revision_id)
        .map_err(|_| "external_history_revision_unavailable".to_owned())?
        .ok_or_else(|| "external_history_revision_not_found".into())
}

fn verify_external_history_source(
    app: &tauri::AppHandle,
    fixture: &ExternalFixture,
) -> Result<(), crate::paste::PasteError> {
    let Some(expected) = fixture.source_identity.as_ref() else {
        return Ok(());
    };
    let revision = read_external_history_revision(app, expected.dictation_id, expected.revision_id)
        .map_err(|_| crate::paste::PasteError::InputChanged)?;
    verify_external_history_revision(fixture, Some(&revision))
}

fn verify_external_history_revision(
    fixture: &ExternalFixture,
    revision: Option<&crate::store::HistoryRevision>,
) -> Result<(), crate::paste::PasteError> {
    let Some(expected) = fixture.source_identity.as_ref() else {
        return Ok(());
    };
    let Some(revision) = revision else {
        return Err(crate::paste::PasteError::InputChanged);
    };
    let actual =
        external_history_identity(revision).map_err(|_| crate::paste::PasteError::InputChanged)?;
    if &actual != expected || revision.final_text != fixture.inserted || !fixture.fits_ui_payload()
    {
        return Err(crate::paste::PasteError::InputChanged);
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct ExternalTarget {
    pid: i32,
    bundle_id: String,
    window_id: String,
}
impl ExternalTarget {
    fn from_guard(
        target: &crate::context::TargetAppGuard,
        target_app: ExternalTargetApp,
    ) -> Result<Self, crate::paste::PasteError> {
        external_target(target, target_app)?;
        Ok(Self {
            pid: target.pid,
            bundle_id: target_app.bundle_id().to_owned(),
            window_id: target
                .window_id
                .ok_or(crate::paste::PasteError::TargetUnavailable)?
                .to_string(),
        })
    }
}
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ExternalReceipt {
    lease: TrialLease,
    recording_generation: String,
    trigger_source: &'static str,
    // Actual permission snapshot, not config-generation-derived permission.
    permissions: (bool, bool, bool),
    target: Option<ExternalTarget>,
    // Full native same-field identity stays in this lease owner. IPC projects
    // only the admitted fixed fixture's PID/bundle/window, never local AX or
    // browser fingerprints. Receipt omission is deliberate, not unknown proof.
    #[serde(skip)]
    native_target: Option<crate::context::TargetAppGuard>,
    trial_current: bool,
    readback: &'static str,
    clipboard_restoration: crate::paste::ClipboardRestoration,
    capture_stage: crate::context::NativeFocusCaptureStage,
    capture_reason: Option<crate::context::NativeFocusCaptureReason>,
    delivery_stage: ExternalDeliveryStage,
    reason: Option<&'static str>,
    source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_identity: Option<ExternalSourceIdentity>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ExternalDeliveryStage {
    NotEntered,
    Entered,
}

#[derive(Clone, Copy)]
struct ExternalCaptureFacts {
    stage: crate::context::NativeFocusCaptureStage,
    reason: Option<crate::context::NativeFocusCaptureReason>,
    delivery_stage: ExternalDeliveryStage,
}

impl Default for ExternalCaptureFacts {
    fn default() -> Self {
        Self {
            stage: crate::context::NativeFocusCaptureStage::NotStarted,
            reason: None,
            delivery_stage: ExternalDeliveryStage::NotEntered,
        }
    }
}
struct ExternalContinuation {
    fixture: ExternalFixture,
    permissions: (bool, bool, bool),
    deadline: std::time::Instant,
    result: Option<ExternalReceipt>,
    // Borrowed identity of the existing TextActionControl, not a new owner.
    operation: Option<crate::TextActionIdentity>,
}
impl ExternalContinuation {
    fn current(
        &self,
        lease: &TrialLease,
        permissions: (bool, bool, bool),
        now: std::time::Instant,
    ) -> bool {
        self.fixture.lease == *lease
            && self.permissions == permissions
            && permissions.1
            && now < self.deadline
    }
}
fn external_scope(state: &AppState, lease: &TrialLease) -> bool {
    let _settings = lock_recover(&state.settings);
    let owner = lock_recover(&state.onboarding_trial);
    owner.matches(lease)
        && generation(state) == lease.config_generation
        && owner.external.as_ref().is_some_and(|phase| {
            phase.current(lease, permission_identity(), std::time::Instant::now())
        })
}
fn external_lease(state: &AppState, id: &str) -> Result<TrialLease, String> {
    validate_id(id)?;
    let lease = lock_recover(&state.onboarding_trial)
        .lease
        .clone()
        .filter(|lease| lease.trial_id == id)
        .ok_or("external_trial_inactive")?;
    if !external_scope(state, &lease) {
        return Err("external_trial_not_current".into());
    }
    Ok(lease)
}
#[tauri::command]
pub(crate) fn prepare_onboarding_external(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    trial_id: String,
    target_app: ExternalTargetApp,
    history_source: Option<ExternalHistorySourceRequest>,
) -> Result<ExternalFixture, String> {
    validate_id(&trial_id)?;
    let _settings = lock_recover(&state.settings);
    let permissions = permission_identity();
    let mut owner = lock_recover(&state.onboarding_trial);
    let lease = owner
        .lease
        .clone()
        .filter(|lease| {
            owner.matches(lease)
                && lease.trial_id == trial_id
                && lease.config_generation == generation(&state)
        })
        .ok_or("external_trial_inactive")?;
    if !permissions.1 {
        return Err("external_accessibility_required".into());
    }
    if owner.external.is_some() {
        return Err("external_trial_already_prepared_restart_lease".into());
    }
    let fixture = if let Some(source) = history_source {
        let revision =
            read_external_history_revision(&app, source.dictation_id, source.revision_id)?;
        let source_identity = external_history_identity(&revision)?;
        if source_identity.dictation_id != source.dictation_id
            || source_identity.revision_id != source.revision_id
        {
            return Err("external_history_revision_changed".into());
        }
        let fixture = ExternalFixture::with_inserted(
            lease,
            target_app,
            revision.final_text,
            Some(source_identity),
        );
        if !fixture.fits_ui_payload() {
            return Err("external_history_revision_exceeds_payload_limit".into());
        }
        fixture
    } else {
        ExternalFixture::new(lease, target_app)
    };
    owner.external = Some(ExternalContinuation {
        fixture: fixture.clone(),
        permissions,
        deadline: std::time::Instant::now() + EXTERNAL_TTL,
        result: None,
        operation: None,
    });
    Ok(fixture)
}
#[tauri::command]
pub(crate) fn copy_onboarding_external_fixture(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    trial_id: String,
) -> Result<(), String> {
    let lease = external_lease(&state, &trial_id)?;
    let text = lock_recover(&state.onboarding_trial)
        .external
        .as_ref()
        .ok_or("external_trial_inactive")?
        .fixture
        .before
        .clone();
    // An explicit button owns this intentional clipboard replacement. It is
    // separate from the delivery owner's later test-start snapshot/restore.
    crate::paste::copy_if_valid(&app, &text, || {
        if external_scope(&state, &lease) {
            Ok(())
        } else {
            Err(crate::paste::PasteError::Cancelled)
        }
    })
    .map_err(|_| "external_fixture_copy_unconfirmed".into())
}
fn external_target(
    target: &crate::context::TargetAppGuard,
    target_app: ExternalTargetApp,
) -> Result<(), crate::paste::PasteError> {
    if target.bundle_id.as_deref() != Some(target_app.bundle_id())
        || target.pid <= 0
        || target.window_id.is_none()
        || target.window_token.is_none()
        || target.input_token.is_none()
    {
        return Err(crate::paste::PasteError::TargetUnavailable);
    }
    if target.secure_input {
        return Err(crate::paste::PasteError::SecureInput);
    }
    Ok(())
}
fn native_capture_error(
    reason: Option<crate::context::NativeFocusCaptureReason>,
) -> crate::paste::PasteError {
    use crate::context::NativeFocusCaptureReason as Reason;
    match reason {
        Some(Reason::SecureInput) => crate::paste::PasteError::SecureInput,
        Some(Reason::IdentityDrift | Reason::SelectedApplicationMismatch) => {
            crate::paste::PasteError::TargetChanged
        }
        Some(Reason::FocusedInputNotEditable | Reason::FocusedRoleUnknown) => {
            crate::paste::PasteError::InputUnavailable
        }
        Some(Reason::AccessibilityObservationUnknown) => crate::paste::PasteError::Accessibility,
        Some(
            Reason::ApplicationUnavailable
            | Reason::ApplicationIdentityUnknown
            | Reason::FocusedElementUnavailable
            | Reason::WindowIdentityUnknown
            | Reason::WindowIdentityAmbiguous
            | Reason::UnsupportedPlatform,
        )
        | None => crate::paste::PasteError::TargetUnavailable,
    }
}
fn record_capture_facts(
    facts: &Arc<Mutex<ExternalCaptureFacts>>,
    capture: &crate::context::NativeFocusCapture,
) {
    let mut facts = lock_recover(facts);
    facts.stage = capture.stage;
    facts.reason = capture.reason;
}
fn record_capture_failure(
    facts: &Arc<Mutex<ExternalCaptureFacts>>,
    stage: crate::context::NativeFocusCaptureStage,
    reason: crate::context::NativeFocusCaptureReason,
) {
    let mut facts = lock_recover(facts);
    facts.stage = stage;
    facts.reason = Some(reason);
}
fn record_capture_failure_if_missing(
    facts: &Arc<Mutex<ExternalCaptureFacts>>,
    stage: crate::context::NativeFocusCaptureStage,
    reason: crate::context::NativeFocusCaptureReason,
) {
    let mut facts = lock_recover(facts);
    if facts.reason.is_none() {
        facts.stage = stage;
        facts.reason = Some(reason);
    }
}
fn capture_external_target(
    target_app: ExternalTargetApp,
    facts: &Arc<Mutex<ExternalCaptureFacts>>,
) -> Result<crate::context::NativeFocusCapture, crate::paste::PasteError> {
    let capture = crate::context::capture_frontmost_focus(Some(target_app.bundle_id()));
    record_capture_facts(facts, &capture);
    if !capture.stable_frontmost_identity || capture.reason.is_some() {
        if capture.reason.is_none() {
            record_capture_failure(
                facts,
                crate::context::NativeFocusCaptureStage::FrontmostRecheck,
                crate::context::NativeFocusCaptureReason::ApplicationIdentityUnknown,
            );
        }
        return Err(native_capture_error(capture.reason));
    }
    external_target(&capture.target, target_app)?;
    Ok(capture)
}
fn verify_external_target(
    target: &crate::context::TargetAppGuard,
    target_app: ExternalTargetApp,
    facts: &Arc<Mutex<ExternalCaptureFacts>>,
) -> Result<(), crate::paste::PasteError> {
    external_target(target, target_app)?;
    let current = capture_external_target(target_app, facts)?;
    match crate::context::same_field_mismatch_reason(target, &current.target, false) {
        None => Ok(()),
        Some("secure_input") => {
            record_capture_failure(
                facts,
                current.stage,
                crate::context::NativeFocusCaptureReason::SecureInput,
            );
            Err(crate::paste::PasteError::SecureInput)
        }
        Some("input_unavailable") => {
            record_capture_failure(
                facts,
                current.stage,
                crate::context::NativeFocusCaptureReason::FocusedElementUnavailable,
            );
            Err(crate::paste::PasteError::InputUnavailable)
        }
        Some("target_unavailable") => {
            record_capture_failure(
                facts,
                current.stage,
                crate::context::NativeFocusCaptureReason::WindowIdentityUnknown,
            );
            Err(crate::paste::PasteError::TargetUnavailable)
        }
        Some("input_changed") => {
            record_capture_failure(
                facts,
                current.stage,
                crate::context::NativeFocusCaptureReason::IdentityDrift,
            );
            Err(crate::paste::PasteError::InputChanged)
        }
        Some(_) => {
            record_capture_failure(
                facts,
                current.stage,
                crate::context::NativeFocusCaptureReason::IdentityDrift,
            );
            Err(crate::paste::PasteError::TargetChanged)
        }
    }
}
fn capture_external_source(
    fixture: &ExternalFixture,
    facts: &Arc<Mutex<ExternalCaptureFacts>>,
) -> Result<
    (
        crate::context::TargetAppGuard,
        crate::paste::CapturedTextActionSource,
    ),
    crate::paste::PasteError,
> {
    let capture = capture_external_target(fixture.target_app, facts)?;
    let target = capture.target.clone();
    let Some(anchor) = capture.focused_anchor else {
        record_capture_failure_if_missing(
            facts,
            crate::context::NativeFocusCaptureStage::FocusedElement,
            crate::context::NativeFocusCaptureReason::FocusedElementUnavailable,
        );
        return Err(crate::paste::PasteError::InputUnavailable);
    };
    let (source, full) = crate::paste::capture_text_action_source_from_anchor(
        anchor,
        &fixture.before,
        fixture.before.encode_utf16().count(),
        || verify_external_target(&target, fixture.target_app, facts),
    )
    .inspect_err(|error| {
        let reason = match error {
            crate::paste::PasteError::SecureInput => {
                crate::context::NativeFocusCaptureReason::SecureInput
            }
            crate::paste::PasteError::TargetChanged
            | crate::paste::PasteError::InputChanged
            | crate::paste::PasteError::SelectionChanged => {
                crate::context::NativeFocusCaptureReason::IdentityDrift
            }
            crate::paste::PasteError::InputUnavailable
            | crate::paste::PasteError::SelectionUnavailable => {
                crate::context::NativeFocusCaptureReason::FocusedElementUnavailable
            }
            crate::paste::PasteError::Accessibility => {
                crate::context::NativeFocusCaptureReason::AccessibilityObservationUnknown
            }
            _ => return,
        };
        record_capture_failure_if_missing(
            facts,
            crate::context::NativeFocusCaptureStage::FocusedElement,
            reason,
        );
    })?;
    if !fixture.admits_source(&full, &source) {
        record_capture_failure_if_missing(
            facts,
            crate::context::NativeFocusCaptureStage::FocusedElement,
            crate::context::NativeFocusCaptureReason::IdentityDrift,
        );
        return Err(crate::paste::PasteError::SelectionChanged);
    }
    Ok((target, source))
}

fn apply_external_delivery_error(
    receipt: &mut ExternalReceipt,
    delivery_stage: ExternalDeliveryStage,
    error: &crate::paste::PasteError,
) {
    if delivery_stage == ExternalDeliveryStage::Entered {
        receipt.clipboard_restoration = error.clipboard_restoration();
    }
    receipt.reason = Some(error.bounded_code());
}

fn apply_external_capture_facts(receipt: &mut ExternalReceipt, facts: ExternalCaptureFacts) {
    receipt.capture_stage = facts.stage;
    receipt.capture_reason = facts.reason;
    receipt.delivery_stage = facts.delivery_stage;
}
#[tauri::command]
pub(crate) async fn run_onboarding_external(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    trial_id: String,
) -> Result<ExternalReceipt, String> {
    // Release/replacement cannot acknowledge resource closure before this
    // owned blocking delivery has joined. Cancel remains a separate request.
    let _gate = state.onboarding_gate.lock().await;
    let lease = external_lease(&state, &trial_id)?;
    let fixture = {
        let owner = lock_recover(&state.onboarding_trial);
        let phase = owner.external.as_ref().ok_or("external_trial_inactive")?;
        if phase.result.is_some() {
            return Err("external_trial_requires_new_lease".into());
        }
        phase.fixture.clone()
    };
    verify_external_history_source(&app, &fixture)
        .map_err(|_| "external_history_revision_changed")?;
    let generation = dictation::claim_onboarding_external_start_entry(&state)
        .await
        .ok_or("external_operation_busy")?;
    let claim = {
        let manager = lock_recover(&state.manager);
        crate::TextActionStartClaim {
            session_generation: generation,
            onboarding_trial: manager.onboarding_trial.clone(),
            cancellation: manager.cancellation.clone(),
        }
    };
    let Some(trial) = claim
        .onboarding_trial
        .clone()
        .filter(|trial| trial.lease == lease)
    else {
        crate::reset_selected_action_start(&app, &state, generation).await;
        return Err("external_trial_not_current".into());
    };
    let Some(identity) = crate::begin_text_action(&state, "onboarding_external", &claim).await
    else {
        crate::reset_selected_action_start(&app, &state, generation).await;
        return Err("external_operation_cancelled".into());
    };
    let attached = {
        let mut owner = lock_recover(&state.onboarding_trial);
        owner
            .external
            .as_mut()
            .filter(|phase| phase.fixture.lease == lease)
            .map(|phase| {
                phase.operation = Some(identity.clone());
            })
            .is_some()
    };
    if !attached {
        claim.cancellation.cancel();
    }
    tokio::select! {
        _ = tokio::time::sleep(std::time::Duration::from_millis(fixture.switch_ms)) => {},
        _ = claim.cancellation.cancelled() => {},
    }
    {
        let _input_gate = state.hotkey_gate.lock().await;
        let mut manager = lock_recover(&state.manager);
        if claim.matches(&manager) {
            manager.phase = dictation::Phase::Processing;
            manager.session_generation = manager.session_generation.wrapping_add(1);
        }
    }
    let handle = app.clone();
    let owned_identity = identity.clone();
    let owned_trial = trial.clone();
    let cancellation = claim.cancellation.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        use tauri::Manager;
        let state = handle.state::<AppState>();
        let valid = || {
            external_scope(&state, &owned_trial.lease)
                && !cancellation.is_cancelled()
                && trial_operation_matches(
                    &lock_recover(&state.manager),
                    &owned_trial,
                    &owned_identity,
                )
        };
        let mut receipt = ExternalReceipt {
            lease: owned_trial.lease.clone(),
            recording_generation: owned_trial.recording_generation.to_string(),
            trigger_source: "app_command",
            permissions: owned_trial.permissions,
            target: None,
            native_target: None,
            trial_current: false,
            readback: "unknown",
            clipboard_restoration: crate::paste::ClipboardRestoration::NotNeeded,
            capture_stage: crate::context::NativeFocusCaptureStage::NotStarted,
            capture_reason: None,
            delivery_stage: ExternalDeliveryStage::NotEntered,
            reason: None,
            source: "product_same_field_ax_readback_not_independent_oracle",
            source_identity: fixture.source_identity.clone(),
        };
        let capture_facts = Arc::new(Mutex::new(ExternalCaptureFacts::default()));
        let delivered = (|| {
            if !valid() {
                return Err(crate::paste::PasteError::Cancelled);
            }
            let (target, source) = capture_external_source(&fixture, &capture_facts)?;
            // Only the selected fixed-app identity and exact full fixture can cross IPC.
            receipt.target = Some(ExternalTarget::from_guard(&target, fixture.target_app)?);
            receipt.native_target = Some(target.clone());
            let before_handle = handle.clone();
            let after_handle = handle.clone();
            let before_trial = owned_trial.clone();
            let after_trial = owned_trial.clone();
            let before_identity = owned_identity.clone();
            let after_identity = owned_identity.clone();
            let before_cancel = cancellation.clone();
            let after_cancel = cancellation.clone();
            let before_target = target.clone();
            let after_target = target.clone();
            let before_fixture = fixture.clone();
            let before_capture_facts = capture_facts.clone();
            let after_capture_facts = capture_facts.clone();
            let delivery_facts = capture_facts.clone();
            crate::with_text_action_commit(&state, &owned_identity, || {
                lock_recover(&delivery_facts).delivery_stage = ExternalDeliveryStage::Entered;
                let value = crate::paste::insert_with_post_verify_timed(
                    &handle,
                    &fixture.inserted,
                    true,
                    cancellation.clone(),
                    crate::paste::DeliveryPolicy {
                        recovery: crate::paste::RecoveryIntent::NoImplicitCopy,
                        readback: None,
                        dispatch_gate: None,
                        on_shortcut_dispatched: None,
                        on_owner_lifecycle: None,
                        before: move || {
                            let state = before_handle.state::<AppState>();
                            if before_cancel.is_cancelled()
                                || !external_scope(&state, &before_trial.lease)
                                || !trial_operation_matches(
                                    &lock_recover(&state.manager),
                                    &before_trial,
                                    &before_identity,
                                )
                            {
                                return Err(crate::paste::PasteError::Cancelled);
                            }
                            let (current_target, current) =
                                capture_external_source(&before_fixture, &before_capture_facts)?;
                            match crate::context::same_field_mismatch_reason(
                                &before_target,
                                &current_target,
                                false,
                            ) {
                                None => {}
                                Some("input_changed") => {
                                    return Err(crate::paste::PasteError::InputChanged)
                                }
                                Some("target_changed") => {
                                    return Err(crate::paste::PasteError::TargetChanged)
                                }
                                Some("secure_input") => {
                                    return Err(crate::paste::PasteError::SecureInput)
                                }
                                Some(_) => return Err(crate::paste::PasteError::TargetUnavailable),
                            }
                            if !crate::paste::text_action_source_matches(&source, &current) {
                                return Err(crate::paste::PasteError::InputChanged);
                            }
                            verify_external_history_source(&before_handle, &before_fixture)?;
                            Ok(())
                        },
                        after: move || {
                            let state = after_handle.state::<AppState>();
                            if after_cancel.is_cancelled()
                                || !external_scope(&state, &after_trial.lease)
                                || !trial_operation_matches(
                                    &lock_recover(&state.manager),
                                    &after_trial,
                                    &after_identity,
                                )
                            {
                                return Err(crate::paste::PasteError::Cancelled);
                            }
                            verify_external_target(
                                &after_target,
                                fixture.target_app,
                                &after_capture_facts,
                            )
                        },
                    },
                    target.pid,
                );
                (value, true)
            })
            .map_err(|_| crate::paste::PasteError::Cancelled)?
        })();
        match delivered {
            Ok(value) => {
                receipt.clipboard_restoration = value.clipboard_restoration;
                if value.outcome.verified
                    && value.outcome.value_after.as_deref() == Some(fixture.after.as_str())
                {
                    receipt.readback = "exact_once";
                } else {
                    receipt.reason = Some("external_readback_unconfirmed");
                }
            }
            Err(error) => {
                let delivery_stage = lock_recover(&capture_facts).delivery_stage;
                apply_external_delivery_error(&mut receipt, delivery_stage, &error);
            }
        }
        let capture_facts = lock_recover(&capture_facts);
        apply_external_capture_facts(&mut receipt, *capture_facts);
        receipt
    })
    .await;
    // A failed join leaves the existing operation owner retained for explicit
    // cancellation/release; it cannot be acknowledged as a completed action.
    let mut receipt = result.map_err(|_| "external_delivery_owner_join_unknown")?;
    receipt.trial_current = external_scope(&state, &lease)
        && trial_operation_matches(&lock_recover(&state.manager), &trial, &identity);
    crate::clear_text_action(&state, &identity);
    let finished =
        dictation::finish_onboarding_fixture_operation(&mut lock_recover(&state.manager), &trial);
    if let Some(generation) = finished {
        crate::release_live_operation(&app, &state, generation).await;
    }
    if external_scope(&state, &lease) {
        if let Some(phase) = lock_recover(&state.onboarding_trial).external.as_mut() {
            phase.operation = None;
            phase.result = Some(receipt.clone());
        }
    }
    Ok(receipt)
}
#[tauri::command]
pub(crate) fn cancel_onboarding_external(
    state: State<'_, AppState>,
    trial_id: String,
) -> Result<(), String> {
    validate_id(&trial_id)?;
    // Cancellation is a request, never a terminal/drop acknowledgement. The
    // request token is cancelled before waiting on the synchronous commit gate.
    let operation = {
        let mut owner = lock_recover(&state.onboarding_trial);
        if owner
            .lease
            .as_ref()
            .is_some_and(|lease| lease.trial_id == trial_id)
        {
            owner.external.take().and_then(|phase| phase.operation)
        } else {
            None
        }
    };
    if let Some(identity) = operation {
        {
            let manager = lock_recover(&state.manager);
            if manager.onboarding_trial == identity.onboarding_trial
                && manager.onboarding_trial.as_ref().is_some_and(|trial| {
                    trial.recording_generation == identity.recording_generation
                })
            {
                manager.cancellation.cancel();
            }
        }
        crate::cancel_text_action_control_for_session(&state, &identity.transaction_id);
    }
    Ok(())
}
#[tauri::command]
pub(crate) fn confirm_onboarding_external(
    state: State<'_, AppState>,
    trial_id: String,
) -> Result<ExternalReceipt, String> {
    let lease = external_lease(&state, &trial_id)?;
    let _settings = lock_recover(&state.settings);
    let mut owner = lock_recover(&state.onboarding_trial);
    if !owner.matches(&lease) || generation(&state) != lease.config_generation {
        return Err("external_trial_not_current".into());
    }
    let phase = owner.external.as_ref().ok_or("external_trial_inactive")?;
    if !phase.current(&lease, permission_identity(), std::time::Instant::now()) {
        return Err("external_permission_or_deadline_changed".into());
    }
    let receipt = phase
        .result
        .clone()
        .ok_or("external_delivery_not_terminal")?;
    if receipt.readback == "exact_once"
        && (receipt.native_target.is_none()
            || receipt.delivery_stage != ExternalDeliveryStage::Entered)
    {
        return Err("external_native_target_unavailable".into());
    }
    if receipt.delivery_stage == ExternalDeliveryStage::NotEntered
        && receipt.clipboard_restoration != crate::paste::ClipboardRestoration::NotNeeded
    {
        return Err("external_pre_delivery_clipboard_fact_invalid".into());
    }
    owner.external = None;
    Ok(receipt)
}

#[cfg(test)]
pub(crate) fn test_trial_snapshot(
    id: &str,
    generation: &str,
    recording_generation: u64,
) -> TrialSnapshot {
    TrialSnapshot {
        lease: TrialLease {
            trial_id: id.into(),
            config_generation: generation.into(),
        },
        recording_generation,
        trigger_source: "native_hotkey",
        permissions: (true, false, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_event_requires_its_captured_generation_to_still_be_recording() {
        assert!(recording_event_is_current(
            Some(11),
            11,
            dictation::Phase::Recording,
            false
        ));
        assert!(!recording_event_is_current(
            Some(10),
            11,
            dictation::Phase::Recording,
            false
        ));
        assert!(!recording_event_is_current(
            Some(11),
            11,
            dictation::Phase::Processing,
            false
        ));
        assert!(!recording_event_is_current(
            Some(11),
            11,
            dictation::Phase::Recording,
            true
        ));
    }

    #[test]
    fn controlled_fixture_admission_uses_exact_utf16_range_and_immutable_full_value() {
        let fixture = ExternalFixture::new(
            lease("10000000-0000-4000-8000-000000000001", "7"),
            ExternalTargetApp::TextEdit,
        );
        let source = crate::paste::CapturedTextActionSource {
            kind: crate::paste::TextActionSourceKind::Selection,
            text: fixture.selected.clone(),
            text_fingerprint: 1,
            field_fingerprint: Some(2),
            selection_range: Some((
                fixture.selection_start_utf16,
                fixture.selected.encode_utf16().count() as i64,
            )),
            editable: true,
            copy_only_selection: false,
        };
        assert!(fixture.admits_source(&fixture.before, &source));
        assert!(
            fixture.selection_start_utf16
                > fixture.before.chars().position(|c| c == 'R').unwrap() as i64
        );
        let mut moved = source.clone();
        moved.selection_range.as_mut().unwrap().0 += 1;
        assert!(!fixture.admits_source(&fixture.before, &moved));
        assert!(!fixture.admits_source(&(fixture.before.clone() + "user edit"), &source));
        let mut composer = source.clone();
        composer.copy_only_selection = true;
        assert!(!fixture.admits_source(&fixture.before, &composer));
        let other = ExternalFixture::new(
            lease("10000000-0000-4000-8000-000000000002", "7"),
            ExternalTargetApp::TextEdit,
        );
        assert!(!other.admits_source(&fixture.before, &source));
        assert_eq!(fixture.after.matches(&fixture.inserted).count(), 1);
    }

    #[test]
    fn history_revision_pin_is_exact_and_rejects_missing_inapplicable_or_changed_text() {
        let mut revision = crate::store::HistoryRevision {
            revision_id: 21,
            dictation_id: 11,
            created_at: "2026-10-18T00:00:00Z".into(),
            final_text: "selected cleanup result".into(),
            cleanup_status: Some("ai_success".into()),
            intent: Some(serde_json::json!({
                "operation": "cleanup",
                "source": "selected_text"
            })),
            model: None,
            context_policy: None,
            revision_reason: "ai_reclean".into(),
        };
        let identity = external_history_identity(&revision).unwrap();
        let fixture = ExternalFixture::with_inserted(
            lease("10000000-0000-4000-8000-000000000021", "7"),
            ExternalTargetApp::TextEdit,
            revision.final_text.clone(),
            Some(identity),
        );

        assert!(verify_external_history_revision(&fixture, Some(&revision)).is_ok());
        assert!(verify_external_history_revision(&fixture, None).is_err());

        revision.final_text.push_str(" changed");
        assert!(matches!(
            verify_external_history_revision(&fixture, Some(&revision)),
            Err(crate::paste::PasteError::InputChanged)
        ));

        let mut other_revision = revision.clone();
        other_revision.final_text = fixture.inserted.clone();
        other_revision.revision_id += 1;
        assert!(matches!(
            verify_external_history_revision(&fixture, Some(&other_revision)),
            Err(crate::paste::PasteError::InputChanged)
        ));

        let mut degraded = other_revision;
        degraded.revision_id -= 1;
        degraded.cleanup_status = Some("ai_failed_raw".into());
        assert!(matches!(
            verify_external_history_revision(&fixture, Some(&degraded)),
            Err(crate::paste::PasteError::InputChanged)
        ));

        let mut inapplicable = revision.clone();
        inapplicable.final_text = fixture.inserted.clone();
        inapplicable.intent = Some(serde_json::json!({
            "operation": "rewrite",
            "source": "selected_text"
        }));
        assert!(matches!(
            verify_external_history_revision(&fixture, Some(&inapplicable)),
            Err(crate::paste::PasteError::InputChanged)
        ));

        let fixed = ExternalFixture::new(
            lease("10000000-0000-4000-8000-000000000022", "7"),
            ExternalTargetApp::TextEdit,
        );
        assert!(verify_external_history_revision(&fixed, None).is_ok());

        let mut bounded = revision;
        bounded.final_text = "x".repeat(400);
        let bounded_fixture = ExternalFixture::with_inserted(
            lease("10000000-0000-4000-8000-000000000023", "7"),
            ExternalTargetApp::TextEdit,
            bounded.final_text.clone(),
            Some(external_history_identity(&bounded).unwrap()),
        );
        assert!(bounded_fixture.fits_ui_payload());
        assert!(verify_external_history_revision(&bounded_fixture, Some(&bounded)).is_ok());

        bounded.final_text = "x".repeat(512);
        let full_payload = ExternalFixture::with_inserted(
            lease("10000000-0000-4000-8000-000000000024", "7"),
            ExternalTargetApp::TextEdit,
            bounded.final_text.clone(),
            Some(external_history_identity(&bounded).unwrap()),
        );
        assert!(!full_payload.fits_ui_payload());
        assert!(matches!(
            verify_external_history_revision(&full_payload, Some(&bounded)),
            Err(crate::paste::PasteError::InputChanged)
        ));

        bounded.final_text = "x".repeat(513);
        assert!(external_history_identity(&bounded).is_err());
    }

    #[test]
    fn external_target_requires_complete_native_focus_identity() {
        let mut target = crate::context::TargetAppGuard {
            pid: 42,
            bundle_id: Some("com.example.unrelated".into()),
            browser_host: None,
            browser_target_token: None,
            browser_tab_token: None,
            window_token: Some(1),
            window_id: Some(2),
            window_identity_ambiguous: false,
            input_token: Some(3),
            secure_input: false,
        };
        assert!(matches!(
            external_target(&target, ExternalTargetApp::TextEdit),
            Err(crate::paste::PasteError::TargetUnavailable)
        ));
        target.bundle_id = Some("com.apple.TextEdit".into());
        target.input_token = None;
        assert!(matches!(
            external_target(&target, ExternalTargetApp::TextEdit),
            Err(crate::paste::PasteError::TargetUnavailable)
        ));
        target.input_token = Some(3);
        target.secure_input = true;
        assert!(matches!(
            external_target(&target, ExternalTargetApp::TextEdit),
            Err(crate::paste::PasteError::SecureInput)
        ));
    }

    #[test]
    fn receipt_preserves_capture_unknown_and_pre_delivery_clipboard_state() {
        let facts = ExternalCaptureFacts::default();
        assert_eq!(
            facts.stage,
            crate::context::NativeFocusCaptureStage::NotStarted
        );
        assert_eq!(facts.reason, None);
        assert_eq!(facts.delivery_stage, ExternalDeliveryStage::NotEntered);
        assert_eq!(
            serde_json::to_value(ExternalDeliveryStage::NotEntered).unwrap(),
            serde_json::json!("not_entered")
        );
        assert_eq!(
            serde_json::to_value(
                crate::context::NativeFocusCaptureReason::AccessibilityObservationUnknown
            )
            .unwrap(),
            serde_json::json!("accessibility_observation_unknown")
        );
        let mut receipt = ExternalReceipt {
            lease: lease("10000000-0000-4000-8000-000000000001", "7"),
            recording_generation: "8".into(),
            trigger_source: "app_command",
            permissions: (true, true, false),
            target: None,
            native_target: None,
            trial_current: false,
            readback: "unknown",
            clipboard_restoration: crate::paste::ClipboardRestoration::NotNeeded,
            capture_stage: crate::context::NativeFocusCaptureStage::FocusedElement,
            capture_reason: Some(
                crate::context::NativeFocusCaptureReason::AccessibilityObservationUnknown,
            ),
            delivery_stage: ExternalDeliveryStage::NotEntered,
            reason: Some("target_unavailable"),
            source: "product_same_field_ax_readback_not_independent_oracle",
            source_identity: None,
        };
        apply_external_delivery_error(
            &mut receipt,
            ExternalDeliveryStage::NotEntered,
            &crate::paste::PasteError::TargetUnavailable,
        );
        let json = serde_json::to_value(receipt).unwrap();
        assert_eq!(json["capture_stage"], "focused_element");
        assert_eq!(json["capture_reason"], "accessibility_observation_unknown");
        assert_eq!(json["delivery_stage"], "not_entered");
        assert_eq!(json["clipboard_restoration"], "not_needed");
        assert!(json.get("native_target").is_none());
        assert_eq!(
            json,
            serde_json::from_str::<serde_json::Value>(include_str!(
                "../../src/test/fixtures/external-trial-receipt.json"
            ))
            .unwrap(),
            "the shared TypeScript contract fixture must be emitted by this Rust receipt owner"
        );
    }

    #[test]
    fn coarse_capture_refusal_does_not_overwrite_native_reason_or_stage() {
        let facts = Arc::new(Mutex::new(ExternalCaptureFacts::default()));
        record_capture_failure(
            &facts,
            crate::context::NativeFocusCaptureStage::ApplicationIdentity,
            crate::context::NativeFocusCaptureReason::SelectedApplicationMismatch,
        );
        record_capture_failure_if_missing(
            &facts,
            crate::context::NativeFocusCaptureStage::FocusedElement,
            crate::context::NativeFocusCaptureReason::FocusedElementUnavailable,
        );
        let observed = *lock_recover(&facts);
        assert_eq!(
            observed.stage,
            crate::context::NativeFocusCaptureStage::ApplicationIdentity
        );
        assert_eq!(
            observed.reason,
            Some(crate::context::NativeFocusCaptureReason::SelectedApplicationMismatch)
        );
    }

    #[test]
    fn delivery_error_preserves_pre_entry_state_and_only_marks_entered_restoration_unknown() {
        let mut receipt = ExternalReceipt {
            lease: lease("10000000-0000-4000-8000-000000000001", "7"),
            recording_generation: "8".into(),
            trigger_source: "app_command",
            permissions: (true, true, false),
            target: None,
            native_target: None,
            trial_current: false,
            readback: "unknown",
            clipboard_restoration: crate::paste::ClipboardRestoration::NotNeeded,
            capture_stage: crate::context::NativeFocusCaptureStage::FocusedElement,
            capture_reason: Some(
                crate::context::NativeFocusCaptureReason::AccessibilityObservationUnknown,
            ),
            delivery_stage: ExternalDeliveryStage::NotEntered,
            reason: None,
            source: "product_same_field_ax_readback_not_independent_oracle",
            source_identity: None,
        };
        apply_external_delivery_error(
            &mut receipt,
            ExternalDeliveryStage::NotEntered,
            &crate::paste::PasteError::TargetUnavailable,
        );
        assert_eq!(receipt.readback, "unknown");
        assert_eq!(
            receipt.clipboard_restoration,
            crate::paste::ClipboardRestoration::NotNeeded
        );
        assert_eq!(receipt.delivery_stage, ExternalDeliveryStage::NotEntered);

        apply_external_delivery_error(
            &mut receipt,
            ExternalDeliveryStage::Entered,
            &crate::paste::PasteError::InputUnavailable,
        );
        assert_eq!(receipt.readback, "unknown");
        assert_eq!(
            receipt.clipboard_restoration,
            crate::paste::ClipboardRestoration::Unknown
        );
        assert_eq!(receipt.delivery_stage, ExternalDeliveryStage::NotEntered);
    }
    #[test]
    fn external_target_app_admits_only_its_exact_application_bundle() {
        let guard = crate::context::TargetAppGuard {
            pid: 42,
            bundle_id: Some("com.google.Chrome".into()),
            browser_host: None,
            browser_target_token: None,
            browser_tab_token: None,
            window_token: Some(1),
            window_id: Some(2),
            window_identity_ambiguous: false,
            input_token: Some(3),
            secure_input: false,
        };
        assert!(external_target(&guard, ExternalTargetApp::Chrome).is_ok());
        assert!(matches!(
            external_target(&guard, ExternalTargetApp::TextEdit),
            Err(crate::paste::PasteError::TargetUnavailable)
        ));
        assert_eq!(ExternalTargetApp::Chrome.bundle_id(), "com.google.Chrome");
        assert_eq!(
            ExternalTargetApp::TextEdit.bundle_id(),
            "com.apple.TextEdit"
        );
    }

    #[test]
    fn chrome_fixture_matches_the_owned_nonce_field_and_utf16_selection_exactly() {
        let fixture = ExternalFixture::new(
            lease("10000000-0000-4000-8000-000000000001", "7"),
            ExternalTargetApp::Chrome,
        );
        assert_eq!(
            fixture.before,
            format!(
                "VoiceFlow V7 oracle {CHROME_FIXTURE_NONCE}\n前缀🙂[REPLACE ONLY THIS]后缀\nEnd {CHROME_FIXTURE_NONCE}"
            )
        );
        assert_eq!(fixture.selection_start_utf16, 62);
        let source = crate::paste::CapturedTextActionSource {
            kind: crate::paste::TextActionSourceKind::Selection,
            text: fixture.selected.clone(),
            text_fingerprint: 1,
            field_fingerprint: Some(2),
            selection_range: Some((62, 17)),
            editable: true,
            copy_only_selection: false,
        };
        assert!(fixture.admits_source(&fixture.before, &source));
        assert!(!fixture.admits_source(&(fixture.before.clone() + "other"), &source));
    }
    #[test]
    fn prepared_external_continuation_does_not_survive_permission_lease_expiry_or_regrant() {
        let now = std::time::Instant::now();
        let a = lease("10000000-0000-4000-8000-000000000001", "7");
        let mut owner = TrialOwner::default();
        owner.replace(Some(a.clone()));
        owner.external = Some(ExternalContinuation {
            fixture: ExternalFixture::new(a.clone(), ExternalTargetApp::TextEdit),
            permissions: (true, true, false),
            deadline: now + EXTERNAL_TTL,
            result: None,
            operation: None,
        });
        let phase = owner.external.as_ref().unwrap();
        assert!(phase.current(&a, (true, true, false), now));
        assert!(!phase.current(&a, (true, false, false), now));
        assert!(!phase.current(&a, (true, true, false), now + EXTERNAL_TTL));
        assert!(!phase.current(&lease(&a.trial_id, "8"), (true, true, false), now));
        owner.begin_revocation();
        assert!(owner.external.is_none());
        owner.replace(Some(a));
        assert!(owner.external.is_none());
    }
    const A: &str = "10000000-0000-4000-8000-000000000001";
    const B: &str = "10000000-0000-4000-8000-000000000002";
    fn lease(id: &str, generation: &str) -> TrialLease {
        TrialLease {
            trial_id: id.into(),
            config_generation: generation.into(),
        }
    }
    #[test]
    fn uuid_boundary_rejects_noncanonical_or_invalid_identity() {
        assert!(validate_id(A).is_ok());
        for value in [
            "",
            "10000000-0000-4000-7000-000000000001",
            "10000000-0000-0000-8000-000000000001",
            "10000000-0000-4000-8000-00000000000A",
            "10000000-0000-4000-8000-000000000001-extra",
            "00000000-0000-0000-0000-000000000000",
        ] {
            assert!(
                validate_id(value).is_err(),
                "accepted invalid identity {value}"
            );
        }
    }
    #[test]
    fn replaced_lease_rejects_old_release_and_old_selection_and_keeps_preview_route() {
        let mut owner = TrialOwner::default();
        let original = lease(A, "9007199254740993");
        owner.replace(Some(original.clone()));
        owner.selected_text = Some("owned selected trial text".into());
        owner.begin_revocation();
        assert_eq!(owner.lease.as_ref(), Some(&original));
        assert!(!owner.matches(&original));
        assert!(owner.selected_text.is_none());
        let newer = lease(B, "9007199254740994");
        owner.replace(Some(newer.clone()));
        assert!(!owner.release_matches(A));
        assert!(!owner.matches(&original));
        assert!(owner.matches(&newer));
        assert!(owner.release_matches(B));
        owner.replace(None);
        assert!(!owner.matches(&newer));
        assert!(!owner.release_matches(B));
    }
    #[test]
    fn stop_claim_retains_trial_recording_identity_independently_of_processing_generation() {
        let mut manager = dictation::DictationManager::new();
        manager.phase = dictation::Phase::Recording;
        manager.session_generation = 41;
        manager.onboarding_trial = Some(TrialSnapshot {
            lease: lease(A, "18446744073709551615"),
            recording_generation: 41,
            trigger_source: "native_hotkey",
            permissions: (true, false, false),
        });
        let claim = dictation::claim_stop_manager(&mut manager).unwrap();
        assert_eq!(claim.recording_generation, 41);
        assert_eq!(claim.session_generation, 42);
        let trial = claim.onboarding_trial.unwrap();
        assert_eq!(trial.recording_generation, 41);
        let payload = serde_json::to_value(&trial.lease).unwrap();
        assert_eq!(payload["config_generation"], "18446744073709551615");
        assert_eq!(trial.trigger_source, "native_hotkey");
        manager.onboarding_trial = None;
        assert_eq!(trial.lease.trial_id, A);
    }
    #[test]
    fn paused_a_cannot_take_b_fixture_even_when_a_lease_is_reused_or_b_is_normal() {
        for reuse_lease in [false, true] {
            let a = test_trial_snapshot(A, "7", 41);
            let b = test_trial_snapshot(if reuse_lease { A } else { B }, "7", 43);
            let mut owner = TrialOwner::default();
            owner.replace(Some(a.lease.clone()));
            owner.selected_text = Some("fixture A".into());
            let mut manager = dictation::DictationManager::new();
            manager.phase = dictation::Phase::Starting;
            manager.session_generation = 41;
            manager.onboarding_trial = Some(a.clone());
            // A pauses after its accepted claim; release/replacement and B
            // claim happen before its identity-bound destructive source read.
            manager.cancellation.cancel();
            owner.begin_revocation();
            owner.replace(Some(b.lease.clone()));
            owner.selected_text = Some("fixture B stays owned by B".into());
            manager.cancellation = tokio_util::sync::CancellationToken::new();
            manager.session_generation = 43;
            manager.onboarding_trial = Some(b.clone());
            assert!(owner.take_selected_for_start(&manager, &a, 41).is_err());
            assert_eq!(
                owner.selected_text.as_deref(),
                Some("fixture B stays owned by B")
            );
            manager.onboarding_trial = None;
            assert!(owner.take_selected_for_start(&manager, &a, 41).is_err());
            assert_eq!(
                owner.selected_text.as_deref(),
                Some("fixture B stays owned by B")
            );
            assert!(a.lease.trial_id == A); // A's captured route remains trial.
            manager.onboarding_trial = Some(b.clone());
            assert_eq!(
                owner
                    .take_selected_for_start(&manager, &b, 43)
                    .unwrap()
                    .as_deref(),
                Some("fixture B stays owned by B")
            );
        }
    }
}
