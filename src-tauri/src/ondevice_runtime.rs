//! Local MLX sidecar process and bounded JSON-lines IPC.

use crate::mlx_diagnostics::{
    event, EventName, LocalAsrOperation, LocalAsrTerminalCode, Severity, Target,
};
use crate::observations::{LocalStage, LocalStageGuard};
use crate::ondevice_asr::{model_dir, model_files_are_ready, platform_supported};
use crate::ondevice_models::{mlx_model, MlxLanguagePolicy};
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
#[cfg(test)]
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::ExitStatus;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

#[cfg(any(test, feature = "evaluation", feature = "research-harness"))]
pub(crate) mod evaluation_source;

const PROTOCOL_VERSION: u32 = 1;
// Includes the response terminating LF; the Swift producer caps JSON at one byte less.
const MAX_RESPONSE_LINE_BYTES: usize = 1_048_576;
const MAX_AUDIO_BYTES: usize = 30_000_000;
const MAX_AUDIO_DURATION_SECS: u64 = 15 * 60;
const MODEL_IDLE_UNLOAD_SECS: u64 = 15 * 60;
const ASR_INSTALLED_MANIFEST: &str = ".voiceflow-model-manifest.json";
const ASR_DURABLE_PROOF_SCHEMA: u32 = 1;
const ASR_DURABLE_PROOF_MAX_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeSnapshot {
    pub platform_supported: bool,
    pub runtime_status: String,
    pub runtime_version: Option<String>,
    pub loaded_model_id: Option<String>,
    pub loaded_model_version: Option<String>,
    pub loading_model_id: Option<String>,
    pub error: Option<String>,
}

impl RuntimeSnapshot {
    fn initial() -> Self {
        Self {
            platform_supported: platform_supported(),
            runtime_status: if platform_supported() {
                if sidecar_executable().is_some() {
                    "not_checked".to_owned()
                } else {
                    "sidecar_missing".to_owned()
                }
            } else {
                "unsupported_platform".to_owned()
            },
            runtime_version: None,
            loaded_model_id: None,
            loaded_model_version: None,
            loading_model_id: None,
            error: None,
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone)]
struct StageEvent {
    name: &'static str,
}

#[cfg(test)]
struct FixtureConfig {
    expected_files: Vec<(String, u64, String)>,
    worker_program: PathBuf,
    worker_args: Vec<OsString>,
    observer: Arc<dyn Fn(StageEvent) + Send + Sync>,
}

#[cfg(test)]
struct FixtureRetirementGate {
    armed: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[cfg(test)]
impl FixtureRetirementGate {
    fn new() -> Self {
        Self {
            armed: AtomicBool::new(true),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        }
    }

    async fn hold_once(&self) {
        if self.armed.swap(false, Ordering::AcqRel) {
            self.entered.notify_one();
            self.release.notified().await;
        }
    }

    async fn wait_until_entered(&self) -> Result<(), String> {
        tokio::time::timeout(Duration::from_secs(6), self.entered.notified())
            .await
            .map_err(|_| "fixture owner barrier was not reached before timeout".to_owned())
    }

    fn release(&self) {
        self.release.notify_one();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Request {
    protocol_version: u32,
    request_id: String,
    session_id: Option<String>,
    model_version: Option<String>,
    op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_max_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens_per_chunk: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens_total: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
struct Response {
    protocol_version: u32,
    request_id: String,
    session_id: Option<String>,
    model_version: Option<String>,
    op: String,
    status: String,
    #[serde(default)]
    platform_supported: Option<bool>,
    #[serde(default)]
    runtime_version: Option<String>,
    #[serde(default)]
    loaded_model_id: Option<String>,
    #[serde(default)]
    loaded_model_version: Option<String>,
    #[serde(default)]
    result: Option<TranscriptionResult>,
    #[serde(default)]
    cleanup_result: Option<serde_json::Value>,
    #[serde(default)]
    warmup_result: Option<serde_json::Value>,
    #[serde(default)]
    loaded_attestation: Option<crate::observations::LoadedAttestation>,
    #[serde(default)]
    error: Option<SidecarFailure>,
}

#[derive(Debug, Clone, Deserialize)]
struct SidecarFailure {
    code: String,
    message: String,
}

impl Response {
    fn is_successful(&self) -> bool {
        matches!(
            self.status.as_str(),
            "ready" | "loaded" | "unloaded" | "completed"
        )
    }

    fn error_message(&self, fallback: &str) -> String {
        self.error
            .as_ref()
            .map(|error| format!("{}: {}", error.code, error.message))
            .unwrap_or_else(|| fallback.to_owned())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum LoadedIdentityError {
    MissingId,
    MissingRevision,
    WrongId,
    WrongRevision,
}
impl LoadedIdentityError {
    fn code(&self) -> &'static str {
        match self {
            Self::MissingId => "local_loaded_model_id_missing",
            Self::MissingRevision => "local_loaded_model_revision_missing",
            Self::WrongId => "local_loaded_model_id_mismatch",
            Self::WrongRevision => "local_loaded_model_revision_mismatch",
        }
    }
}
struct LoadedModelIdentity<'a> {
    id: &'a str,
    revision: &'a str,
}
impl<'a> LoadedModelIdentity<'a> {
    fn parse(
        id: Option<&'a str>,
        revision: Option<&'a str>,
        expected_id: &str,
        expected_revision: &str,
        stage: LocalStage,
    ) -> Result<Self, LoadedIdentityError> {
        let result = (|| {
            let id = id
                .filter(|v| !v.is_empty())
                .ok_or(LoadedIdentityError::MissingId)?;
            let revision = revision
                .filter(|v| !v.is_empty())
                .ok_or(LoadedIdentityError::MissingRevision)?;
            if id != expected_id {
                return Err(LoadedIdentityError::WrongId);
            }
            if revision != expected_revision {
                return Err(LoadedIdentityError::WrongRevision);
            }
            Ok(Self { id, revision })
        })();
        crate::observations::local_identity_check(
            expected_id,
            expected_revision,
            stage,
            id,
            revision,
            result.as_ref().err().map(LoadedIdentityError::code),
        );
        result
    }
}
#[derive(Debug, Clone, Deserialize)]
struct TranscriptionResult {
    text: String,
    #[serde(default)]
    language: Option<String>,
    generation_tokens: usize,
    covered_samples: u64,
    chunks_completed: usize,
    chunks_total: usize,
    completion: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LocalResultRejection {
    Incomplete,
    Coverage,
    GenerationBudget,
    Empty,
}
impl LocalResultRejection {
    fn terminal_code(&self) -> LocalAsrTerminalCode {
        match self {
            Self::Incomplete => LocalAsrTerminalCode::LocalCompletionIncomplete,
            Self::Coverage => LocalAsrTerminalCode::LocalCoverageIncomplete,
            Self::GenerationBudget => LocalAsrTerminalCode::LocalGenerationBudgetInvalid,
            Self::Empty => LocalAsrTerminalCode::LocalEmptyOutputNotAccepted,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Self::Incomplete => "local_completion_incomplete",
            Self::Coverage => "local_coverage_incomplete",
            Self::GenerationBudget => "local_generation_budget_invalid",
            Self::Empty => "local_empty_output_not_accepted",
        }
    }
}
impl TranscriptionResult {
    // Measurement is separate from the ordinary nonempty delivery admission.
    // Empty is qualified only after all preceding rejection gates have passed.
    fn observe_rejection(&self, rejection: &LocalResultRejection) {
        if *rejection == LocalResultRejection::Empty {
            crate::observations::asr_filter(
                &self.text,
                "",
                if self.text.is_empty() {
                    "local_complete_empty"
                } else {
                    "locally_suppressed"
                },
                Some(rejection.code()),
            );
        } else {
            crate::observations::asr_failure("provider_failed", rejection.code());
        }
    }
    fn rejection(&self, samples: u64, max_tokens: usize) -> Option<LocalResultRejection> {
        if self.completion != "complete" {
            Some(LocalResultRejection::Incomplete)
        } else if self.covered_samples != samples
            || self.chunks_completed == 0
            || self.chunks_completed != self.chunks_total
        {
            Some(LocalResultRejection::Coverage)
        } else if self.generation_tokens == 0 || self.generation_tokens > max_tokens {
            Some(LocalResultRejection::GenerationBudget)
        } else if self.text.trim().is_empty() {
            Some(LocalResultRejection::Empty)
        } else {
            None
        }
    }
}
struct ChildSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    registration: crate::model_resources::ChildRegistration,
}

type OwnedAsrSession = tokio::sync::OwnedMutexGuard<Option<ChildSession>>;

#[derive(Clone)]
struct AsrOwnerContext {
    active_pid: Arc<AtomicU32>,
    status: Arc<Mutex<RuntimeSnapshot>>,
    loaded_attestation: Arc<Mutex<Option<crate::observations::LoadedAttestation>>>,
    models_root: PathBuf,
    #[cfg(test)]
    fixture: bool,
    #[cfg(test)]
    fixture_reaped: Arc<Mutex<Vec<Result<ExitStatus, String>>>>,
    #[cfg(test)]
    fixture_observer: Option<Arc<dyn Fn(StageEvent) + Send + Sync>>,
    #[cfg(test)]
    fixture_retirement_gate: Option<Arc<FixtureRetirementGate>>,
}

impl AsrOwnerContext {
    fn commit_transcription(&self, response: &Response) {
        let mut status = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        status.runtime_status = "loaded".to_owned();
        status.runtime_version = response.runtime_version.clone();
        status.loaded_model_id = response.loaded_model_id.clone();
        status.loaded_model_version = response.loaded_model_version.clone();
        status.loading_model_id = None;
        status.error = None;
        drop(status);

        *self
            .loaded_attestation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = response.loaded_attestation.clone();
        if let (Some(model_id), Some(model_version)) = (
            response.loaded_model_id.as_deref(),
            response.loaded_model_version.as_deref(),
        ) {
            crate::observations::local_attestation(
                model_id,
                model_version,
                response.loaded_attestation.clone(),
            );
        }

        #[cfg(test)]
        if self.fixture {
            return;
        }
        if let Some(model_id) = response.loaded_model_id.as_deref() {
            schedule_idle_unload(&self.models_root, model_id);
        }
    }

    fn schedule_idle_unload_if_loaded(&self, model_id: &str) {
        let loaded = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .loaded_model_id
            .as_deref()
            == Some(model_id);
        if !loaded {
            return;
        }
        #[cfg(test)]
        if self.fixture {
            if let Some(observer) = &self.fixture_observer {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    observer(StageEvent {
                        name: "cancellation_idle_unload_scheduled",
                    });
                }));
            }
            return;
        }
        schedule_idle_unload(&self.models_root, model_id);
    }

    fn set_status(&self, name: &str, error: Option<String>) {
        let mut status = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        status.runtime_status = name.to_owned();
        status.runtime_version = None;
        status.loaded_model_id = None;
        status.loaded_model_version = None;
        status.error = error;
    }

    #[cfg(test)]
    fn observe_fixture(&self, name: &'static str) {
        if let Some(observer) = &self.fixture_observer {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                observer(StageEvent { name });
            }));
        }
    }

    async fn retire(
        &self,
        session: &mut OwnedAsrSession,
        _audio_file: &PrivateAudioFile,
        reason: &str,
    ) {
        #[cfg(test)]
        if let Some(gate) = &self.fixture_retirement_gate {
            gate.hold_once().await;
        }
        self.set_status("stopping", Some(reason.to_owned()));
        let Some(owned) = session.as_mut() else {
            self.set_status("runtime_failed", Some(reason.to_owned()));
            return;
        };
        let pid = owned.child.id().unwrap_or_default();
        let _ = owned.child.start_kill();
        let _exit_status = loop {
            match owned.child.wait().await {
                Ok(status) => break status,
                Err(error) => {
                    log::warn!("local ASR child exit remains unobserved; retaining its session and audio owner: {error}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        };
        owned.registration.clone().retire_after_exit();
        #[cfg(test)]
        {
            self.fixture_reaped
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(Ok(_exit_status));
            self.observe_fixture("cancellation_child_exit_observed");
        }
        if pid != 0 {
            let _ = self
                .active_pid
                .compare_exchange(pid, 0, Ordering::AcqRel, Ordering::Acquire);
        }
        **session = None;
        self.set_status("runtime_failed", Some(reason.to_owned()));
    }
}

enum AsrExchangeOutcome {
    Completed(Box<Response>),
    Cancelled,
}

enum AsrOwnerError {
    Exchange(String),
    RejectedResult {
        response: Box<Response>,
        rejection: LocalResultRejection,
    },
}

#[derive(Clone)]
struct LocalAsrDiagnostics {
    request_id: String,
    session_generation: Option<u64>,
    model_id: &'static str,
    revision: &'static str,
    operation: LocalAsrOperation,
}

impl LocalAsrDiagnostics {
    fn new(
        request_id: String,
        session_generation: Option<u64>,
        model_id: &'static str,
        revision: &'static str,
        operation: LocalAsrOperation,
    ) -> Self {
        Self {
            request_id,
            session_generation,
            model_id,
            revision,
            operation,
        }
    }

    fn stage(&self, stage: &'static str) -> LocalAsrStageTimer<LocalAsrStageEmitter> {
        self.stage_with_started_at(stage, Some(Instant::now()), emit_local_asr_stage)
    }

    fn deferred_stage(&self, stage: &'static str) -> LocalAsrStageTimer<LocalAsrStageEmitter> {
        self.stage_with_started_at(stage, None, emit_local_asr_stage)
    }

    fn stage_with_started_at<E>(
        &self,
        stage: &'static str,
        started: Option<Instant>,
        emitter: E,
    ) -> LocalAsrStageTimer<E>
    where
        E: FnMut(
                &LocalAsrDiagnostics,
                &'static str,
                &'static str,
                f64,
                Option<bool>,
                Option<LocalAsrTerminalCode>,
            ) + Send,
    {
        LocalAsrStageTimer {
            diagnostics: self.clone(),
            stage,
            started,
            spawned: None,
            finished: false,
            emitter,
        }
    }

    #[cfg(test)]
    fn stage_with_emitter<E>(&self, stage: &'static str, emitter: E) -> LocalAsrStageTimer<E>
    where
        E: FnMut(
                &LocalAsrDiagnostics,
                &'static str,
                &'static str,
                f64,
                Option<bool>,
                Option<LocalAsrTerminalCode>,
            ) + Send
            + 'static,
    {
        self.stage_with_started_at(stage, Some(Instant::now()), emitter)
    }

    fn emit(
        &self,
        stage: &'static str,
        outcome: &'static str,
        elapsed_ms: f64,
        spawned: Option<bool>,
        terminal_code: Option<LocalAsrTerminalCode>,
    ) {
        let severity = if outcome == "failed" {
            Severity::Warn
        } else {
            Severity::Info
        };
        let mut builder = event(EventName::LocalAsrStage, severity, Target::Runtime)
            .request_id(&self.request_id)
            .session_generation(self.session_generation)
            .local_asr_operation(self.operation)
            .phase("terminal")
            .stage(stage)
            .outcome(outcome)
            .model_identity(self.model_id, self.revision)
            .elapsed_ms(elapsed_ms);
        if stage == "inference_terminal" {
            if let Some(code) = terminal_code {
                builder = builder.local_asr_terminal_code(code);
            }
        }
        if let Some(spawned) = spawned {
            builder = builder.spawned_child(spawned);
        }
        builder.emit();
    }
}

type LocalAsrStageEmitter = fn(
    &LocalAsrDiagnostics,
    &'static str,
    &'static str,
    f64,
    Option<bool>,
    Option<LocalAsrTerminalCode>,
);

fn emit_local_asr_stage(
    diagnostics: &LocalAsrDiagnostics,
    stage: &'static str,
    outcome: &'static str,
    elapsed_ms: f64,
    spawned: Option<bool>,
    terminal_code: Option<LocalAsrTerminalCode>,
) {
    diagnostics.emit(stage, outcome, elapsed_ms, spawned, terminal_code);
}

struct LocalAsrStageTimer<E>
where
    E: FnMut(
            &LocalAsrDiagnostics,
            &'static str,
            &'static str,
            f64,
            Option<bool>,
            Option<LocalAsrTerminalCode>,
        ) + Send,
{
    diagnostics: LocalAsrDiagnostics,
    stage: &'static str,
    started: Option<Instant>,
    spawned: Option<bool>,
    finished: bool,
    emitter: E,
}

impl<E> LocalAsrStageTimer<E>
where
    E: FnMut(
            &LocalAsrDiagnostics,
            &'static str,
            &'static str,
            f64,
            Option<bool>,
            Option<LocalAsrTerminalCode>,
        ) + Send,
{
    fn spawned_child(&mut self, spawned: bool) {
        self.spawned = Some(spawned);
    }

    fn start(&mut self) {
        self.started = Some(Instant::now());
        self.finished = false;
    }

    fn finish(&mut self, outcome: &'static str) {
        self.finish_with_terminal_code(outcome, None);
    }

    fn finish_with_terminal_code(
        &mut self,
        outcome: &'static str,
        terminal_code: Option<LocalAsrTerminalCode>,
    ) {
        if self.finished {
            return;
        }
        self.finished = true;
        let Some(started) = self.started else {
            return;
        };
        let diagnostics = self.diagnostics.clone();
        let stage = self.stage;
        let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        let spawned = self.spawned;
        let emitter = &mut self.emitter;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            emitter(
                &diagnostics,
                stage,
                outcome,
                elapsed_ms,
                spawned,
                terminal_code,
            );
        }));
    }

    fn fail<T>(&mut self, error: String) -> Result<T, String> {
        self.finish(local_asr_error_outcome(&error));
        Err(error)
    }
}

impl<E> Drop for LocalAsrStageTimer<E>
where
    E: FnMut(
            &LocalAsrDiagnostics,
            &'static str,
            &'static str,
            f64,
            Option<bool>,
            Option<LocalAsrTerminalCode>,
        ) + Send,
{
    fn drop(&mut self) {
        self.finish("cancelled_or_abandoned");
    }
}

fn local_asr_error_outcome(error: &str) -> &'static str {
    if error == "Local model load was cancelled." {
        "cancelled_or_abandoned"
    } else {
        "failed"
    }
}

#[cfg(test)]
#[test]
fn asr_cancel_pair_rejects_identity_status_and_result_payload_mutations() {
    let request = Request {
        protocol_version: PROTOCOL_VERSION,
        request_id: "original".into(),
        session_id: Some("session".into()),
        model_version: Some("revision".into()),
        op: "transcribe".into(),
        model_id: Some("model".into()),
        model_directory: None,
        audio_path: Some("/fixture/audio.wav".into()),
        language: Some("en".into()),
        audio_max_bytes: Some(10),
        max_tokens_per_chunk: Some(4),
        max_tokens_total: Some(8),
    };
    let cancel = Request {
        protocol_version: PROTOCOL_VERSION,
        request_id: "fresh-cancel".into(),
        session_id: request.session_id.clone(),
        model_version: request.model_version.clone(),
        op: "cancel".into(),
        model_id: request.model_id.clone(),
        model_directory: None,
        audio_path: None,
        language: None,
        audio_max_bytes: None,
        max_tokens_per_chunk: None,
        max_tokens_total: None,
    };
    let terminal_json = serde_json::json!({
        "protocol_version": 1,
        "request_id": "original",
        "session_id": "session",
        "model_version": "revision",
        "op": "transcribe",
        "status": "cancelled",
        "platform_supported": true,
        "loaded_model_id": "model",
        "loaded_model_version": "revision"
    });
    let acknowledgement_json = serde_json::json!({
        "protocol_version": 1,
        "request_id": "fresh-cancel",
        "session_id": "session",
        "model_version": "revision",
        "op": "cancel",
        "status": "cancelled",
        "platform_supported": true,
        "loaded_model_id": "model",
        "loaded_model_version": "revision"
    });
    let terminal: Response = serde_json::from_value(terminal_json.clone()).unwrap();
    let acknowledgement: Response = serde_json::from_value(acknowledgement_json.clone()).unwrap();
    assert!(OnDeviceRuntime::cancel_pair_matches(
        &request,
        &cancel,
        &terminal,
        &acknowledgement
    ));

    for (field, value) in [
        ("protocol_version", serde_json::json!(2)),
        ("request_id", serde_json::json!("stale")),
        ("session_id", serde_json::json!("wrong-session")),
        ("model_version", serde_json::json!("old-revision")),
        ("op", serde_json::json!("load")),
        ("status", serde_json::json!("completed")),
        ("platform_supported", serde_json::json!(false)),
        ("loaded_model_id", serde_json::json!("wrong-model")),
        ("loaded_model_version", serde_json::json!("wrong-revision")),
        (
            "result",
            serde_json::json!({
                "text":"partial", "generation_tokens":1, "covered_samples":1,
                "chunks_completed":1, "chunks_total":1, "completion":"complete"
            }),
        ),
        ("cleanup_result", serde_json::json!({"text":"cleanup"})),
        ("warmup_result", serde_json::json!({"tokens":1})),
        (
            "error",
            serde_json::json!({"code":"failed", "message":"failed"}),
        ),
    ] {
        let mut mutated = terminal_json.clone();
        mutated[field] = value.clone();
        let mutated: Response = serde_json::from_value(mutated).unwrap();
        assert!(
            !OnDeviceRuntime::cancel_pair_matches(&request, &cancel, &mutated, &acknowledgement),
            "terminal field {field} must not establish reusable cancellation"
        );

        let mut mutated = acknowledgement_json.clone();
        mutated[field] = value;
        let mutated: Response = serde_json::from_value(mutated).unwrap();
        assert!(
            !OnDeviceRuntime::cancel_pair_matches(&request, &cancel, &terminal, &mutated),
            "acknowledgement field {field} must not establish reusable cancellation"
        );
    }
}

#[cfg(test)]
#[test]
fn local_asr_error_outcome_preserves_cancellation() {
    assert_eq!(
        local_asr_error_outcome("Local model load was cancelled."),
        "cancelled_or_abandoned"
    );
    assert_eq!(
        local_asr_error_outcome("model could not be loaded"),
        "failed"
    );
}

#[cfg(test)]
#[test]
fn local_asr_result_rejections_map_to_content_free_terminal_codes() {
    assert_eq!(
        LocalResultRejection::Incomplete.terminal_code(),
        LocalAsrTerminalCode::LocalCompletionIncomplete,
    );
    assert_eq!(
        LocalResultRejection::Coverage.terminal_code(),
        LocalAsrTerminalCode::LocalCoverageIncomplete,
    );
    assert_eq!(
        LocalResultRejection::GenerationBudget.terminal_code(),
        LocalAsrTerminalCode::LocalGenerationBudgetInvalid,
    );
    assert_eq!(
        LocalResultRejection::Empty.terminal_code(),
        LocalAsrTerminalCode::LocalEmptyOutputNotAccepted,
    );
}

#[cfg(test)]
#[test]
fn local_asr_stage_timer_finishes_or_abandons_once() {
    let diagnostics = LocalAsrDiagnostics::new(
        "vf-test".into(),
        Some(23),
        "local_asr_test",
        "test-revision",
        LocalAsrOperation::Transcribe,
    );
    let outcomes = Arc::new(Mutex::new(Vec::new()));

    let observed = Arc::clone(&outcomes);
    let mut completed =
        diagnostics.stage_with_emitter("model_load", move |_, _, outcome, _, _, code| {
            observed.lock().unwrap().push((outcome, code));
        });
    completed.finish("loaded");
    completed.finish("failed");
    drop(completed);

    let observed = Arc::clone(&outcomes);
    let mut failed =
        diagnostics.stage_with_emitter("inference_terminal", move |_, _, outcome, _, _, code| {
            observed.lock().unwrap().push((outcome, code));
        });
    failed.finish_with_terminal_code("failed", Some(LocalAsrTerminalCode::ResponseStatusFailed));
    failed.finish_with_terminal_code(
        "failed",
        Some(LocalAsrTerminalCode::LocalEmptyOutputNotAccepted),
    );
    drop(failed);

    let observed = Arc::clone(&outcomes);
    let abandoned =
        diagnostics.stage_with_emitter("request_total", move |_, _, outcome, _, _, code| {
            observed.lock().unwrap().push((outcome, code));
        });
    drop(abandoned);

    assert_eq!(
        *outcomes.lock().unwrap(),
        vec![
            ("loaded", None),
            ("failed", Some(LocalAsrTerminalCode::ResponseStatusFailed)),
            ("cancelled_or_abandoned", None),
        ]
    );
}

#[cfg(test)]
#[tokio::test]
async fn asr_stages_recorded_monotonic_for_fake_runtime() {
    let owner = crate::evaluation::fixtures::FixtureDirectory::new();
    let models_root = owner.0.join("models");
    let model_id = crate::ondevice_models::QWEN_ASR_06_ID;
    let model_directory = model_dir(&models_root, model_id);
    std::fs::create_dir_all(&model_directory).unwrap();
    let fixture_bytes = b"tiny synthetic model payload";
    std::fs::write(model_directory.join("fixture.bin"), fixture_bytes).unwrap();
    let expected_files = vec![(
        "fixture.bin".to_owned(),
        fixture_bytes.len() as u64,
        hex::encode(Sha256::digest(fixture_bytes)),
    )];
    let worker = owner.0.join("fake-worker.py");
    std::fs::write(
        &worker,
        r#"import json, sys
mode = sys.argv[1] if len(sys.argv) > 1 else "success"
for line in sys.stdin:
    request = json.loads(line)
    op = request["op"]
    if mode == "startup-eof" and op == "hello": sys.exit(0)
    status = {"hello": "ready", "load": "loaded", "transcribe": "completed", "unload": "unloaded"}.get(op, "ready")
    if mode == "load-reject" and op == "load": status = "failed"
    if mode == "inference-reject" and op == "transcribe": status = "failed"
    response = {"protocol_version": request["protocol_version"], "request_id": request["request_id"], "session_id": request.get("session_id"), "model_version": request.get("model_version"), "op": op, "status": status, "platform_supported": True, "runtime_version": "fixture-worker-1", "loaded_model_id": request.get("model_id") if op in ("load", "transcribe") else None, "loaded_model_version": request.get("model_version") if op in ("load", "transcribe") else None}
    if op == "transcribe":
        response["result"] = {"text": "synthetic speech", "language": "en", "generation_tokens": 1, "covered_samples": 160, "chunks_completed": 1, "chunks_total": 1, "completion": "complete"}
    print(json.dumps(response, separators=(",", ":")), flush=True)
"#,
    )
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed_events = Arc::clone(&events);
    let runtime = Arc::new(OnDeviceRuntime::with_fixture(
        models_root.clone(),
        FixtureConfig {
            expected_files: expected_files.clone(),
            worker_program: PathBuf::from("python3"),
            worker_args: vec![worker.clone().into_os_string(), OsString::from("success")],
            observer: Arc::new(move |event| {
                observed_events.lock().unwrap().push(event.name.to_owned());
                if event.name == "inference" {
                    panic!("fixture observer panic isolation regression");
                }
            }),
        },
    ));
    let fixture_command = runtime.worker_command().unwrap();
    let fixture_args = fixture_command
        .as_std()
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(fixture_args.first().map(String::as_str), Some("-B"));
    assert_eq!(
        fixture_args.get(1).map(String::as_str),
        Some(worker.to_string_lossy().as_ref())
    );
    let audio = synthetic_wav();
    let mut timer = crate::metrics::Metrics::default().stop_to_insert(
        crate::metrics::MetricGroup::unknown("test"),
        tokio_util::sync::CancellationToken::new(),
    );
    let trace = timer.bind_trace(41);
    let options = crate::asr::AsrOptions {
        language: Some("en".to_owned()),
        model: model_id.to_owned(),
        session_generation: Some(41),
        trace: Some(trace.clone()),
        ..crate::asr::AsrOptions::default()
    };
    let request_identity =
        crate::prefetch_asr::PrefetchRequestIdentity::new("ondevice", None, &options);
    let quota_scope =
        crate::queue::RequestScope::new("ondevice", None, &options.model, &options.api_key);
    let attempt = crate::ContextualAsrAttempt {
        options: options.clone(),
        request_identity,
        quota_scope: quota_scope.clone(),
        context_source: None,
    };
    let gate = crate::queue::RequestGate::new(None);
    let cold_runtime = Arc::clone(&runtime);
    let cold_attempt = attempt.clone();
    let cold = crate::execute_reprojected_asr(
        &gate,
        quota_scope.clone(),
        audio.clone(),
        tokio_util::sync::CancellationToken::new(),
        crate::AsrTraceContext {
            trace: Some(trace.clone()),
            provider_inference: crate::trace_provider_asr_call(crate::asr::AsrProtocol::OnDevice),
        },
        move || Some(cold_attempt.clone()),
        move |audio, options| {
            let runtime = Arc::clone(&cold_runtime);
            Box::pin(async move {
                let model = options.model;
                let language = options.language;
                let result = runtime
                    .transcribe(
                        &model,
                        language.as_deref(),
                        audio,
                        options.session_generation,
                        options.trace,
                    )
                    .await;
                result
                    .map(|(text, language)| crate::asr::Transcript {
                        text,
                        asr_text: None,
                        provider_cleaned_candidate: None,
                        language,
                        confidence: None,
                        segments: Vec::new(),
                        words: Vec::new(),
                        tokens: Vec::new(),
                        limits: crate::asr::RateLimits::default(),
                    })
                    .map_err(|error| {
                        crate::asr::AsrError::OnDeviceInferenceUnavailable(error.to_string())
                    })
            })
        },
    )
    .await;
    let hot_runtime = Arc::clone(&runtime);
    let hot_attempt = attempt.clone();
    let hot = crate::execute_reprojected_asr(
        &gate,
        quota_scope,
        audio,
        tokio_util::sync::CancellationToken::new(),
        crate::AsrTraceContext {
            trace: Some(trace.clone()),
            provider_inference: crate::trace_provider_asr_call(crate::asr::AsrProtocol::OnDevice),
        },
        move || Some(hot_attempt.clone()),
        move |audio, options| {
            let runtime = Arc::clone(&hot_runtime);
            Box::pin(async move {
                let model = options.model;
                let language = options.language;
                let result = runtime
                    .transcribe(
                        &model,
                        language.as_deref(),
                        audio,
                        options.session_generation,
                        options.trace,
                    )
                    .await;
                result
                    .map(|(text, language)| crate::asr::Transcript {
                        text,
                        asr_text: None,
                        provider_cleaned_candidate: None,
                        language,
                        confidence: None,
                        segments: Vec::new(),
                        words: Vec::new(),
                        tokens: Vec::new(),
                        limits: crate::asr::RateLimits::default(),
                    })
                    .map_err(|error| {
                        crate::asr::AsrError::OnDeviceInferenceUnavailable(error.to_string())
                    })
            })
        },
    )
    .await;
    runtime
        .shutdown_with_status()
        .await
        .expect("runtime shutdown succeeds");
    runtime.assert_fixture_reaped_count(1);
    let preload_runtime = OnDeviceRuntime::with_fixture(
        models_root,
        FixtureConfig {
            expected_files,
            worker_program: PathBuf::from("python3"),
            worker_args: vec![worker.into_os_string(), OsString::from("success")],
            observer: Arc::new(|_| {}),
        },
    );
    let first_preload = preload_runtime.preload(model_id).await;
    let second_preload = preload_runtime.preload(model_id).await;
    preload_runtime
        .shutdown_with_status()
        .await
        .expect("runtime shutdown succeeds");
    preload_runtime.assert_fixture_reaped_count(1);
    let terminal_total_ms = timer.elapsed().as_millis().min(u64::MAX as u128) as u64;
    timer.finish(crate::metrics::DeliveryOutcome::PasteConfirmed, None);
    assert!(first_preload.is_ok());
    assert!(second_preload.is_ok());
    assert_eq!(cold.unwrap().0.text, "synthetic speech");
    assert_eq!(hot.unwrap().0.text, "synthetic speech");
    assert!(events.lock().unwrap().contains(&"model_load".to_owned()));
    assert!(events.lock().unwrap().contains(&"model_reuse".to_owned()));
    let stages = trace.snapshot().stages;
    use crate::metrics::Stage;
    for required in [
        Stage::QueueWait,
        Stage::FileValidation,
        Stage::RuntimeStartup,
        Stage::ModelLoad,
        Stage::AsrInference,
    ] {
        assert!(
            stages.iter().any(|(stage, _, _)| *stage == required),
            "missing {required:?}"
        );
    }
    for (stage, expected) in [
        (Stage::QueueWait, 4),
        (Stage::FileValidation, 2),
        (Stage::AsrInference, 2),
        (Stage::RuntimeStartup, 1),
        (Stage::ModelLoad, 1),
    ] {
        assert_eq!(
            stages
                .iter()
                .filter(|(actual, _, _)| *actual == stage)
                .count(),
            expected,
            "unexpected {stage:?} count"
        );
    }
    assert!(stages
        .iter()
        .all(|(_, start, duration)| start.saturating_add(*duration) <= terminal_total_ms));
    assert!(stages.windows(2).all(|pair| pair[0].1 <= pair[1].1));
    assert_eq!(
        stages
            .iter()
            .map(|(stage, _, _)| *stage)
            .collect::<Vec<_>>(),
        vec![
            Stage::QueueWait,
            Stage::QueueWait,
            Stage::FileValidation,
            Stage::RuntimeStartup,
            Stage::ModelLoad,
            Stage::AsrInference,
            Stage::QueueWait,
            Stage::QueueWait,
            Stage::FileValidation,
            Stage::AsrInference
        ]
    );
    assert!(stages.iter().map(|(_, _, duration)| *duration).sum::<u64>() <= terminal_total_ms);
    assert!(stages.len() <= 256);
    assert_eq!(trace.snapshot().session_generation, 41);
}

#[cfg(test)]
#[tokio::test]
async fn fake_runtime_records_only_reached_failure_stages() {
    for mode in [
        "hash-failure",
        "load-reject",
        "load-ready",
        "load-unloaded",
        "load-completed",
        "inference-reject",
        "startup-eof",
    ] {
        let owner = crate::evaluation::fixtures::FixtureDirectory::new();
        let models_root = owner.0.join("models");
        let model_id = crate::ondevice_models::QWEN_ASR_06_ID;
        let model_version = mlx_model(model_id)
            .expect("fixture model manifest")
            .revision;
        let model_directory = model_dir(&models_root, model_id);
        std::fs::create_dir_all(&model_directory).unwrap();
        let contents = b"small test-only model file";
        std::fs::write(model_directory.join("fixture.bin"), contents).unwrap();
        let expected = vec![(
            "fixture.bin".to_owned(),
            contents.len() as u64,
            hex::encode(Sha256::digest(contents)),
        )];
        if mode == "hash-failure" {
            std::fs::write(
                model_directory.join("fixture.bin"),
                vec![b'X'; contents.len()],
            )
            .unwrap();
        }
        let worker = owner.0.join("worker.py");
        let operations = owner.0.join("worker.ops");
        let requests = owner.0.join("worker.requests");
        let responses = owner.0.join("worker.responses");
        std::fs::write(&worker, r#"import json, pathlib, sys
mode = sys.argv[1]
for line in sys.stdin:
 r = json.loads(line); op = r["op"]
 with open(str(pathlib.Path(__file__).with_suffix(".requests")), "a", encoding="utf-8") as log:
  log.write(json.dumps(r) + "\n"); log.flush()
 with open(str(pathlib.Path(__file__).with_suffix(".ops")), "a", encoding="utf-8") as log:
  log.write(op + "\n"); log.flush()
 if mode == "startup-eof" and op == "hello": sys.exit(0)
 status = {"hello":"ready","load":"loaded","transcribe":"completed"}.get(op,"ready")
 if mode == "load-reject" and op == "load": status = "failed"
 if mode in ("load-ready", "load-unloaded", "load-completed") and op == "load": status = mode.removeprefix("load-")
 if mode == "inference-reject" and op == "transcribe": status = "failed"
 out = {"protocol_version":r["protocol_version"],"request_id":r["request_id"],"session_id":r.get("session_id"),"model_version":r.get("model_version"),"op":op,"status":status,"platform_supported":True,"runtime_version":"test"}
 if op in ("load","transcribe"): out.update(loaded_model_id=r.get("model_id"),loaded_model_version=r.get("model_version"))
 if op == "transcribe": out["result"] = {"text":"synthetic speech","language":"en","generation_tokens":1,"covered_samples":160,"chunks_completed":1,"chunks_total":1,"completion":"complete"}
 with open(str(pathlib.Path(__file__).with_suffix(".responses")), "a", encoding="utf-8") as log:
  log.write(json.dumps({"protocol_version":out["protocol_version"],"request_id":out["request_id"],"session_id":out.get("session_id"),"model_version":out.get("model_version"),"op":op,"status":status,"loaded_model_id":out.get("loaded_model_id"),"loaded_model_version":out.get("loaded_model_version")}) + "\n"); log.flush()
 print(json.dumps(out),flush=True)
"#).unwrap();
        let runtime = Arc::new(OnDeviceRuntime::with_fixture(
            models_root,
            FixtureConfig {
                expected_files: expected,
                worker_program: PathBuf::from("python3"),
                worker_args: vec![worker.into_os_string(), OsString::from(mode)],
                observer: Arc::new(|_| {}),
            },
        ));
        let mut timer = crate::metrics::Metrics::default().stop_to_insert(
            crate::metrics::MetricGroup::unknown("test"),
            tokio_util::sync::CancellationToken::new(),
        );
        let trace = timer.bind_trace(58);
        let result = runtime
            .transcribe(
                model_id,
                Some("en"),
                synthetic_wav(),
                Some(58),
                Some(trace.clone()),
            )
            .await;
        runtime
            .shutdown_with_status()
            .await
            .expect("runtime shutdown succeeds");
        runtime.assert_fixture_reaped_count(usize::from(mode != "hash-failure"));
        assert!(result.is_err(), "fake runtime must reject mode {mode}");
        if matches!(mode, "load-ready" | "load-unloaded" | "load-completed") {
            let error = result.expect_err("nonterminal LOAD response must reject transcription");
            assert!(error.contains("local model could not be loaded"), "{error}");
            let operations = std::fs::read_to_string(&operations).unwrap();
            assert_eq!(operations.lines().collect::<Vec<_>>(), ["hello", "load"]);
            let request_lines = std::fs::read_to_string(&requests).unwrap();
            let sent = request_lines
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(sent.len(), 2);
            assert_eq!(sent[1]["op"], "load");
            assert_eq!(sent[1]["model_id"], model_id);
            assert_eq!(sent[1]["model_version"], model_version);
            let response_lines = std::fs::read_to_string(&responses).unwrap();
            let replies = response_lines
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(replies.len(), 2);
            assert_eq!(replies[1]["op"], "load");
            assert_eq!(replies[1]["status"], mode.strip_prefix("load-").unwrap());
            assert_eq!(replies[1]["protocol_version"], sent[1]["protocol_version"]);
            assert_eq!(replies[1]["request_id"], sent[1]["request_id"]);
            assert_eq!(replies[1]["session_id"], sent[1]["session_id"]);
            assert_eq!(replies[1]["model_version"], sent[1]["model_version"]);
            assert_eq!(replies[1]["loaded_model_id"], model_id);
            assert_eq!(replies[1]["loaded_model_version"], model_version);
            assert!(runtime.snapshot().loaded_model_id.is_none());
            assert!(runtime.snapshot().loaded_model_version.is_none());
        }
        let stages = trace.snapshot().stages;
        use crate::metrics::Stage;
        let actual = stages
            .iter()
            .map(|(stage, _, _)| *stage)
            .collect::<Vec<_>>();
        let expected = match mode {
            "hash-failure" => vec![Stage::QueueWait, Stage::FileValidation],
            "startup-eof" => vec![
                Stage::QueueWait,
                Stage::FileValidation,
                Stage::RuntimeStartup,
            ],
            "load-reject" | "load-ready" | "load-unloaded" | "load-completed" => vec![
                Stage::QueueWait,
                Stage::FileValidation,
                Stage::RuntimeStartup,
                Stage::ModelLoad,
            ],
            "inference-reject" => vec![
                Stage::QueueWait,
                Stage::FileValidation,
                Stage::RuntimeStartup,
                Stage::ModelLoad,
                Stage::AsrInference,
            ],
            _ => unreachable!(),
        };
        assert_eq!(actual, expected, "unexpected reached stages for {mode}");
    }
}

#[cfg(test)]
#[tokio::test]
async fn fake_runtime_drop_finishes_started_stages_once() {
    let owner = crate::evaluation::fixtures::FixtureDirectory::new();
    let models_root = owner.0.join("models");
    let model_id = crate::ondevice_models::QWEN_ASR_06_ID;
    let model_directory = model_dir(&models_root, model_id);
    std::fs::create_dir_all(&model_directory).unwrap();
    let bytes = b"tiny fake model";
    std::fs::write(model_directory.join("fixture.bin"), bytes).unwrap();
    let expected = vec![(
        "fixture.bin".into(),
        bytes.len() as u64,
        hex::encode(Sha256::digest(bytes)),
    )];
    let marker = owner.0.join("inference-entered");
    let worker = owner.0.join("blocked.py");
    std::fs::write(&worker, r#"import json, pathlib, sys, time
marker = pathlib.Path(sys.argv[2])
for line in sys.stdin:
 r = json.loads(line); op = r["op"]
 if op == "transcribe":
  was = marker.exists(); marker.touch()
  if not was:
   while True: time.sleep(.01)
 status = {"hello":"ready","load":"loaded","transcribe":"completed"}.get(op,"ready")
 out = {"protocol_version":r["protocol_version"],"request_id":r["request_id"],"session_id":r.get("session_id"),"model_version":r.get("model_version"),"op":op,"status":status,"platform_supported":True,"runtime_version":"test"}
 if op in ("load","transcribe"): out.update(loaded_model_id=r.get("model_id"),loaded_model_version=r.get("model_version"))
 if op == "transcribe": out["result"] = {"text":"synthetic speech","language":"en","generation_tokens":1,"covered_samples":160,"chunks_completed":1,"chunks_total":1,"completion":"complete"}
 print(json.dumps(out),flush=True)
"#).unwrap();
    let runtime = Arc::new(OnDeviceRuntime::with_fixture(
        models_root,
        FixtureConfig {
            expected_files: expected,
            worker_program: PathBuf::from("python3"),
            worker_args: vec![
                worker.into_os_string(),
                OsString::from("blocked"),
                marker.as_os_str().to_owned(),
            ],
            observer: Arc::new(|_| {}),
        },
    ));
    let mut timer = crate::metrics::Metrics::default().stop_to_insert(
        crate::metrics::MetricGroup::unknown("test"),
        tokio_util::sync::CancellationToken::new(),
    );
    let trace = timer.bind_trace(61);
    let first_runtime = Arc::clone(&runtime);
    let first_trace = trace.clone();
    let mut task = tokio::spawn(async move {
        first_runtime
            .transcribe(
                model_id,
                Some("en"),
                synthetic_wav(),
                Some(61),
                Some(first_trace),
            )
            .await
    });
    let marker_wait = tokio::time::timeout(Duration::from_secs(5), async {
        while !marker.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    if marker_wait.is_err() {
        task.abort();
        let _ = (&mut task).await;
        runtime
            .shutdown_with_status()
            .await
            .expect("runtime shutdown succeeds");
    }
    assert!(
        marker_wait.is_ok(),
        "fake worker did not reach its block marker"
    );
    task.abort();
    let _ = task.await;
    let second = runtime
        .transcribe(
            model_id,
            Some("en"),
            synthetic_wav(),
            Some(61),
            Some(trace.clone()),
        )
        .await;
    assert_eq!(second.unwrap().0, "synthetic speech");
    runtime
        .shutdown_with_status()
        .await
        .expect("runtime shutdown succeeds");
    runtime.assert_fixture_reaped_count(2);
    let stages = trace.snapshot().stages;
    assert_eq!(
        stages
            .iter()
            .filter(|(s, _, _)| *s == crate::metrics::Stage::AsrInference)
            .count(),
        2,
        "the cancelled attempt and retry each finish one inference span"
    );
    assert_eq!(
        stages
            .iter()
            .filter(|(s, _, _)| *s == crate::metrics::Stage::RuntimeStartup)
            .count(),
        2,
        "both initial worker creation and stale-session replacement are traced"
    );
    assert_eq!(
        stages
            .iter()
            .filter(|(s, _, _)| *s == crate::metrics::Stage::ModelLoad)
            .count(),
        2,
        "each actual child session load is traced"
    );
    assert!(stages.len() <= 256);
}

#[cfg(test)]
struct AsrCancellationFixture {
    _owner: crate::evaluation::fixtures::FixtureDirectory,
    runtime: Arc<OnDeviceRuntime>,
    model_id: &'static str,
    started: PathBuf,
    cancel_seen: PathBuf,
    release_worker: PathBuf,
    worker_started: PathBuf,
    worker_cancel_observed: PathBuf,
    worker_ended: PathBuf,
    worker_joined: PathBuf,
    release_terminal: PathBuf,
    release_ack: PathBuf,
    terminal_sent: PathBuf,
    transcribes: PathBuf,
    pid: PathBuf,
    loads: PathBuf,
    overlap: PathBuf,
}

#[cfg(test)]
type AsrFixtureTranscriptionTask =
    tokio::task::JoinHandle<Result<(String, Option<String>), String>>;

#[cfg(test)]
impl AsrCancellationFixture {
    async fn teardown(
        &self,
        tasks: &mut [Option<AsrFixtureTranscriptionTask>],
        unload: &mut Option<tokio::task::JoinHandle<Result<(), String>>>,
    ) -> Result<usize, String> {
        let _ = std::fs::write(&self.release_worker, b"release");
        let _ = std::fs::write(&self.release_terminal, b"release");
        let _ = std::fs::write(&self.release_ack, b"release");
        if let Some(task) = unload.take() {
            if !task.is_finished() {
                task.abort();
            }
            let _ = task.await;
        }
        if let Some(gate) = self
            .runtime
            .fixture_retirement_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            gate.release();
        }
        if let Some(gate) = self
            .runtime
            .fixture_success_handoff_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            gate.release();
        }
        for task in tasks.iter_mut() {
            if let Some(task) = task.take() {
                if !task.is_finished() {
                    task.abort();
                }
                let _ = task.await;
            }
        }
        self.runtime
            .shutdown_with_status()
            .await
            .map_err(|reason| format!("fixture shutdown did not settle: {reason}"))?;
        let receipts = self
            .runtime
            .fixture_reaped
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if receipts.iter().any(Result::is_err) {
            return Err(format!("fixture child wait failed: {receipts:?}"));
        }
        let reaped_count = receipts.len();
        if self.runtime.active_pid.load(Ordering::Acquire) != 0 {
            return Err("fixture still has an active child identity after shutdown".into());
        }
        if self
            .runtime
            .session
            .try_lock()
            .map_or(true, |session| session.is_some())
        {
            return Err("fixture session still owns a child after shutdown".into());
        }
        Ok(reaped_count)
    }
}

#[cfg(test)]
async fn finish_cancellation_fixture<T>(
    fixture: AsrCancellationFixture,
    tasks: &mut [Option<AsrFixtureTranscriptionTask>],
    unload: &mut Option<tokio::task::JoinHandle<Result<(), String>>>,
    scenario: Result<T, String>,
) -> Result<(T, usize), String> {
    let reaped_count = match fixture.teardown(tasks, unload).await {
        Ok(count) => count,
        Err(error) => {
            std::mem::forget(fixture);
            return Err(error);
        }
    };
    drop(fixture);
    scenario.map(|facts| (facts, reaped_count))
}

/// Exercise the public caller-drop path with a held owner retirement, then let
/// the real runtime shutdown join/reap the synthetic child before returning
/// the same task-local observation capture to the evaluator journal test.
#[cfg(test)]
pub(crate) async fn dropped_public_transcribe_capture(
    publisher: crate::evaluation::CaptureJournal,
    hold_through_timeout: bool,
) -> Result<(), String> {
    let (journal, row_index) = publisher.journal_handle();
    let fixture = asr_cancellation_fixture("bad-completion", Arc::new(|_| {}));
    let gate = Arc::new(FixtureRetirementGate::new());
    fixture
        .runtime
        .set_fixture_retirement_gate(Arc::clone(&gate));
    let capture = publisher.observation_capture();
    let interrupt = tokio_util::sync::CancellationToken::new();
    let mut task = None;
    let mut unload = None;
    let scenario = async {
        let runtime = Arc::clone(&fixture.runtime);
        let model_id = fixture.model_id;
        let cancellation = interrupt.clone();
        task = Some(tokio::spawn(async move {
            crate::evaluation::select_capture(
                publisher,
                &cancellation,
                Instant::now() + Duration::from_secs(30),
                runtime.transcribe(model_id, Some("en"), synthetic_wav(), Some(405), None),
            )
            .await
        }));
        wait_for_fixture_path(&fixture.started).await?;
        interrupt.cancel();
        let selected = task
            .take()
            .ok_or_else(|| "runner capture-selection task was not retained".to_owned())?
            .await
            .map_err(|error| format!("runner capture-selection task failed: {error}"))?;
        let mut pending = None;
        match selected {
            crate::evaluation::CaptureSelection::Cancelled("sigint", capture) => {
                crate::evaluation::defer_cancelled_capture(&mut pending, capture);
            }
            crate::evaluation::CaptureSelection::Cancelled(reason, _) => {
                return Err(format!("runner changed SIGINT reason to {reason}"))
            }
            crate::evaluation::CaptureSelection::Completed(_, _) => {
                return Err("runner capture selection completed instead of observing SIGINT".into())
            }
        }
        gate.wait_until_entered().await?;
        let before_settlement = capture.snapshot();
        if before_settlement.local_asr.as_ref().is_some_and(|local| {
            local
                .private_audio_resources
                .iter()
                .any(|fact| fact.action == "removed_owned_file")
        }) {
            return Err("private WAV was removed before owner retirement was released".into());
        }
        if hold_through_timeout {
            let unsettled = crate::evaluation::shutdown_then_publish(
                pending,
                fixture.runtime.shutdown_with_status(),
            )
            .await;
            if unsettled != Err("local_owner_join_unsettled") {
                return Err(format!("held owner shutdown was not reported unsettled: {unsettled:?}"));
            }
            let unsettled_row = journal.row(row_index);
            if !unsettled_row.diagnostics.iter().any(|value| value == "local_shutdown_unsettled")
                || !matches!(unsettled_row.state, crate::evaluation::contracts::StageState::Cancelled { ref reason } if reason == "sigint")
                || unsettled_row.attempts != 1
            {
                return Err(format!("unsettled report did not preserve cancellation and attempt: {unsettled_row:?}"));
            }
            let local = unsettled_row.local_runtime.as_ref().ok_or_else(|| "unsettled runtime observation missing".to_owned())?;
            if !local.private_audio_resources.iter().any(|fact| fact.action == "created")
                || local.private_audio_resources.iter().any(|fact| fact.action == "removed_owned_file")
            {
                return Err("unsettled report claimed WAV retirement before owner release".into());
            }
            // The report is marked incomplete before the real owner is released.
            gate.release();
            fixture.runtime.shutdown_with_status().await
                .map_err(|reason| format!("released owner did not settle: {reason}"))?;
        } else {
            gate.release();
            crate::evaluation::shutdown_then_publish(pending, async {
                fixture.runtime.shutdown_with_status().await
            })
            .await
            .map_err(|reason| format!("normal owner shutdown did not settle: {reason}"))?;
        }
        let receipts = fixture
            .runtime
            .fixture_reaped
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if receipts.len() != 1 || receipts.iter().any(Result::is_err) {
            return Err(format!(
                "owner did not produce one successful child wait: {receipts:?}"
            ));
        }
        drop(receipts);
        if fixture.runtime.active_pid.load(Ordering::Acquire) != 0
            || fixture
                .runtime
                .session
                .try_lock()
                .map_or(true, |session| session.is_some())
        {
            return Err("runtime owner remained active after shutdown".into());
        }
        if !crate::model_resources::owned_children_for_root(&fixture.runtime.models_root).is_empty() {
            return Err("successful shutdown left an owned model child registered".into());
        }
        Ok(())
    }
    .await;
    // Always unblock the real owner before teardown, including assertion and
    // timeout failures in this regression.
    gate.release();
    if let Some(task) = task.take() {
        task.abort();
        let _ = task.await;
    }
    let mut tasks: [Option<AsrFixtureTranscriptionTask>; 2] = [None, None];
    let scenario = finish_cancellation_fixture(fixture, &mut tasks, &mut unload, scenario).await;
    scenario.map(|((), _)| ())
}

#[cfg(test)]
fn fixture_text(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
fn asr_cancellation_fixture(
    mode: &'static str,
    observer: Arc<dyn Fn(StageEvent) + Send + Sync>,
) -> AsrCancellationFixture {
    let owner = crate::evaluation::fixtures::FixtureDirectory::new();
    let models_root = owner.0.join("models");
    let model_id = crate::ondevice_models::QWEN_ASR_06_ID;
    let model_directory = model_dir(&models_root, model_id);
    std::fs::create_dir_all(&model_directory).unwrap();
    let bytes = b"tiny synthetic model";
    std::fs::write(model_directory.join("fixture.bin"), bytes).unwrap();
    let expected = vec![(
        "fixture.bin".into(),
        bytes.len() as u64,
        hex::encode(Sha256::digest(bytes)),
    )];
    let started = owner.0.join("transcription-started");
    let cancel_seen = owner.0.join("cancel-seen");
    let release_worker = owner.0.join("release-worker");
    let worker_started = owner.0.join("worker-started");
    let worker_cancel_observed = owner.0.join("worker-cancel-observed");
    let worker_ended = owner.0.join("worker-ended");
    let worker_joined = owner.0.join("worker-joined");
    let release_terminal = owner.0.join("release-terminal");
    let release_ack = owner.0.join("release-ack");
    let terminal_sent = owner.0.join("terminal-sent");
    let transcribes = owner.0.join("transcribe-requests");
    let pid = owner.0.join("worker-pid");
    let loads = owner.0.join("load-count");
    let overlap = owner.0.join("overlap-observed");
    let worker = owner.0.join("cooperative-worker.py");
    std::fs::write(
        &worker,
        r#"import json, os, pathlib, sys, time, threading
mode = sys.argv[1]
started_s, cancel_s, release_worker_s, worker_started_s, worker_cancel_observed_s, worker_ended_s, worker_joined_s, release_terminal_s, release_ack_s, terminal_sent_s, transcribes_s, pid_s, loads_s, overlap_s = sys.argv[2:]
started, cancel_seen, release_worker, worker_started, worker_cancel_observed, worker_ended, worker_joined, release_terminal, release_ack, terminal_sent, transcribes, pid, loads, overlap = map(pathlib.Path, (started_s, cancel_s, release_worker_s, worker_started_s, worker_cancel_observed_s, worker_ended_s, worker_joined_s, release_terminal_s, release_ack_s, terminal_sent_s, transcribes_s, pid_s, loads_s, overlap_s))
pid.write_text(str(os.getpid()))
loaded = None
active = None
worker = None
cancel_requested = threading.Event()
def publish_started(payload):
 tmp = started.with_name(started.name + ".tmp-" + str(os.getpid()))
 tmp.write_text(payload)
 os.replace(tmp, started)
def reply(r, status, facts=None, result=False):
 facts = facts or loaded or {}
 out = {"protocol_version":r["protocol_version"],"request_id":r["request_id"],"session_id":r.get("session_id"),"model_version":r.get("model_version"),"op":r["op"],"status":status,"platform_supported":True,"runtime_version":"fixture-worker-1"}
 if facts and facts.get("model_id"):
  out["loaded_model_id"] = facts["model_id"]
  out["loaded_model_version"] = facts["model_version"]
 if result:
  generated = {"text":"synthetic speech","language":"en","generation_tokens":1,"covered_samples":160,"chunks_completed":1,"chunks_total":1,"completion":"complete"}
  if mode == "result-reject-empty": generated["text"] = ""
  elif mode == "result-reject-whitespace": generated["text"] = " \n\t"
  elif mode == "result-reject-coverage": generated["covered_samples"] = 159
  elif mode == "result-reject-budget": generated["generation_tokens"] = 4097
  elif mode == "result-reject-incomplete": generated["completion"] = "budget_exhausted"
  out["result"] = generated
 if r["op"] == "transcribe":
  if mode == "invalid-admission-platform": out["platform_supported"] = False
  elif mode == "invalid-admission-error": out["error"] = {"code":"fixture_error","message":"fixture response must be rejected"}
  elif mode == "invalid-admission-cleanup": out["cleanup_result"] = {"text":"foreign cleanup payload"}
  elif mode == "invalid-admission-warmup": out["warmup_result"] = {"tokens":1}
 print(json.dumps(out, separators=(",", ":")), flush=True)
def synthetic_worker(r, hold):
 worker_started.touch()
 publish_started(r["audio_path"])
 if hold:
  while not release_worker.exists(): time.sleep(.01)
 if cancel_requested.is_set():
  worker_cancel_observed.touch()
  worker_ended.touch()
  return
 reply(r, "completed", loaded, True)
 worker_ended.touch()
for line in sys.stdin:
 r = json.loads(line); op = r["op"]
 if op == "hello": reply(r, "ready")
 elif op == "status": reply(r, "ready")
 elif op == "load":
  loaded = r
  loads.write_text(str(int(loads.read_text()) + 1) if loads.exists() else "1")
  if mode == "load-hold":
   publish_started("load-held")
   while not release_ack.exists(): time.sleep(.01)
  reply(r, "loaded", r)
 elif op == "transcribe":
  prior_dispatches = len(transcribes.read_text().splitlines()) if transcribes.exists() else 0
  with transcribes.open("a") as f: f.write("dispatch\n")
  if active is not None: overlap.touch()
  if mode in ("bad-completion", "loaded-with-result", "success-immediate", "unload-error") or mode.startswith("invalid-admission-") or mode.startswith("result-reject-"):
   publish_started(r["audio_path"])
   status = "loaded" if mode == "loaded-with-result" and prior_dispatches == 0 else "completed"
   reply(r, status, loaded, prior_dispatches > 0 or mode in ("loaded-with-result", "success-immediate", "unload-error") or mode.startswith("invalid-admission-") or mode.startswith("result-reject-"))
   continue
  if active is None:
   active = r
   cancel_requested.clear()
   worker = threading.Thread(target=synthetic_worker, args=(r, prior_dispatches == 0), daemon=True)
   worker.start()
   continue
  reply(r, "completed", loaded, True)
 elif op == "cancel":
  cancel_seen.touch()
  cancel_requested.set()
  if mode == "cooperative":
   if worker is None: raise RuntimeError("cancel arrived without a synthetic worker handle")
   worker.join()
   worker_joined.touch()
   while not release_terminal.exists(): time.sleep(.01)
   reply(active, "cancelled", loaded)
   terminal_sent.touch()
   while not release_ack.exists(): time.sleep(.01)
   reply(r, "cancelled", loaded)
   active = None; worker = None; cancel_requested.clear()
  elif mode == "ack-only": reply(r, "cancelled", loaded)
  elif mode == "terminal-only": reply(active, "cancelled", loaded)
  # timeout mode deliberately withholds both terminal and ACK
 elif op == "unload": reply(r, "failed" if mode == "unload-error" else "unloaded", loaded)
 else: reply(r, "failed", loaded)
"#,
    )
    .unwrap();
    let runtime = Arc::new(OnDeviceRuntime::with_fixture(
        models_root,
        FixtureConfig {
            expected_files: expected,
            worker_program: PathBuf::from("python3"),
            worker_args: vec![
                worker.into_os_string(),
                OsString::from(mode),
                started.as_os_str().to_owned(),
                cancel_seen.as_os_str().to_owned(),
                release_worker.as_os_str().to_owned(),
                worker_started.as_os_str().to_owned(),
                worker_cancel_observed.as_os_str().to_owned(),
                worker_ended.as_os_str().to_owned(),
                worker_joined.as_os_str().to_owned(),
                release_terminal.as_os_str().to_owned(),
                release_ack.as_os_str().to_owned(),
                terminal_sent.as_os_str().to_owned(),
                transcribes.as_os_str().to_owned(),
                pid.as_os_str().to_owned(),
                loads.as_os_str().to_owned(),
                overlap.as_os_str().to_owned(),
            ],
            observer,
        },
    ));
    AsrCancellationFixture {
        _owner: owner,
        runtime,
        model_id,
        started,
        cancel_seen,
        release_worker,
        worker_started,
        worker_cancel_observed,
        worker_ended,
        worker_joined,
        release_terminal,
        release_ack,
        terminal_sent,
        transcribes,
        pid,
        loads,
        overlap,
    }
}

#[cfg(test)]
async fn wait_for_fixture_path(path: &Path) -> Result<(), String> {
    tokio::time::timeout(Duration::from_secs(6), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .map_err(|_| format!("fixture worker did not reach barrier: {}", path.display()))
}

#[cfg(test)]
#[tokio::test]
async fn unload_error_kills_session_and_next_load_succeeds() {
    let fixture = asr_cancellation_fixture("unload-error", Arc::new(|_| {}));
    let mut tasks: [Option<AsrFixtureTranscriptionTask>; 2] = [None, None];
    let mut unload = None;
    let scenario = async {
        fixture
            .runtime
            .transcribe(
                fixture.model_id,
                Some("en"),
                synthetic_wav(),
                Some(601),
                None,
            )
            .await?;
        let first_pid = fixture.runtime.active_pid.load(Ordering::Acquire);
        if first_pid == 0 {
            return Err("initial transcription did not retain the owned worker PID".into());
        }

        let unload_result = fixture.runtime.unload(Some(fixture.model_id)).await;
        if unload_result.is_ok() {
            return Err("fixture unload failure was unexpectedly accepted".into());
        }
        let retired_pid = fixture.runtime.active_pid.load(Ordering::Acquire) == 0;
        let retired_session = fixture
            .runtime
            .session
            .try_lock()
            .is_ok_and(|session| session.is_none());
        let exit_observed = fixture
            .runtime
            .fixture_reaped
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .any(Result::is_ok);
        let failed_status = fixture.runtime.snapshot().runtime_status == "runtime_failed";
        if !(retired_pid && retired_session && exit_observed && failed_status) {
            return Err("failed unload did not wait for and retire the owned child session".into());
        }

        fixture
            .runtime
            .transcribe(
                fixture.model_id,
                Some("en"),
                synthetic_wav(),
                Some(602),
                None,
            )
            .await?;
        let loaded_again = fixture.runtime.snapshot().runtime_status == "loaded";
        let next_pid = fixture.runtime.active_pid.load(Ordering::Acquire);
        if !loaded_again || next_pid == 0 || next_pid == first_pid {
            return Err("next transcription did not load a fresh child session".into());
        }
        Ok((
            retired_pid,
            retired_session,
            exit_observed,
            failed_status,
            loaded_again,
        ))
    }
    .await;
    let scenario = finish_cancellation_fixture(fixture, &mut tasks, &mut unload, scenario).await;
    let (facts, reaped_count) =
        scenario.unwrap_or_else(|error| panic!("unload error retirement scenario failed: {error}"));
    assert!(
        facts.0,
        "failed unload must clear the PID after Child::wait"
    );
    assert!(facts.1, "failed unload must release the retired session");
    assert!(facts.2, "failed unload retirement must observe child exit");
    assert!(
        facts.3,
        "failed unload must not leave producer status loaded"
    );
    assert!(
        facts.4,
        "a subsequent real runtime load/transcription must succeed"
    );
    assert_eq!(reaped_count, 2, "both owned fake children must be reaped");
}

#[cfg(test)]
#[tokio::test]
async fn cancelled_transcription_reuses_loaded_child() {
    let idle_unload_scheduled = Arc::new(AtomicBool::new(false));
    let observed_idle_unload = Arc::clone(&idle_unload_scheduled);
    let observer: Arc<dyn Fn(StageEvent) + Send + Sync> = Arc::new(move |event| {
        if event.name == "cancellation_idle_unload_scheduled" {
            observed_idle_unload.store(true, Ordering::Release);
        }
    });
    let fixture = asr_cancellation_fixture("cooperative", observer);
    let mut tasks: [Option<AsrFixtureTranscriptionTask>; 2] = [None, None];
    let mut unload = None;
    let scenario = async {
        let runtime = Arc::clone(&fixture.runtime);
        let model_id = fixture.model_id;
        tasks[0] = Some(tokio::spawn(async move {
            runtime
                .transcribe(model_id, Some("en"), synthetic_wav(), Some(401), None)
                .await
        }));
        tokio::time::sleep(Duration::from_millis(100)).await;
        if tasks[0].as_ref().is_some_and(|task| task.is_finished()) {
            return Err("transcription ended before the fixture barrier".to_owned());
        }
        wait_for_fixture_path(&fixture.started).await?;
        let audio_path = PathBuf::from(fixture_text(&fixture.started)?);
        if !audio_path.exists() {
            return Err("request WAV was missing while inference was in flight".into());
        }
        let first_pid = fixture_text(&fixture.pid)?;
        let first = tasks[0]
            .take()
            .ok_or_else(|| "first transcription task was not retained".to_owned())?;
        first.abort();
        match first.await {
            Err(error) if error.is_cancelled() => {}
            Err(error) => return Err(format!("first task aborted unexpectedly: {error}")),
            Ok(_) => {
                return Err("first task returned instead of observing caller cancellation".into())
            }
        }
        wait_for_fixture_path(&fixture.cancel_seen).await?;
        wait_for_fixture_path(&fixture.worker_started).await?;
        let cancel_arrived_while_worker_held = !fixture.worker_ended.exists()
            && !fixture.worker_joined.exists()
            && !fixture.worker_cancel_observed.exists();

        let next_runtime = Arc::clone(&fixture.runtime);
        let next_model = fixture.model_id;
        tasks[1] = Some(tokio::spawn(async move {
            next_runtime
                .transcribe(next_model, Some("en"), synthetic_wav(), Some(401), None)
                .await
        }));
        tokio::time::sleep(Duration::from_millis(75)).await;
        let dispatches_before_terminal = fixture_text(&fixture.transcribes)?.lines().count();
        let wav_before_terminal = audio_path.exists();

        std::fs::write(&fixture.release_worker, b"release").map_err(|error| error.to_string())?;
        wait_for_fixture_path(&fixture.worker_cancel_observed).await?;
        wait_for_fixture_path(&fixture.worker_ended).await?;
        wait_for_fixture_path(&fixture.worker_joined).await?;
        let worker_ended_and_joined_before_terminal = !fixture.terminal_sent.exists();
        std::fs::write(&fixture.release_terminal, b"release").map_err(|error| error.to_string())?;
        wait_for_fixture_path(&fixture.terminal_sent).await?;
        let wav_between_terminal_and_ack = audio_path.exists();
        let dispatches_between_terminal_and_ack =
            fixture_text(&fixture.transcribes)?.lines().count();
        unload = Some({
            let runtime = Arc::clone(&fixture.runtime);
            let model_id = fixture.model_id;
            tokio::spawn(async move { runtime.unload(Some(model_id)).await })
        });
        tokio::time::sleep(Duration::from_millis(75)).await;
        let unload_blocked_between_terminal_and_ack =
            !unload.as_ref().is_some_and(|task| task.is_finished());
        if let Some(task) = unload.take() {
            task.abort();
            let _ = task.await;
        }

        std::fs::write(&fixture.release_ack, b"release").map_err(|error| error.to_string())?;
        let joined = {
            let next = tasks[1]
                .as_mut()
                .ok_or_else(|| "next transcription task was not retained".to_owned())?;
            tokio::time::timeout(Duration::from_secs(6), next)
                .await
                .map_err(|_| {
                    "next request stayed blocked after cancellation settlement".to_owned()
                })?
        };
        tasks[1] = None;
        let result = joined.map_err(|error| error.to_string())??;
        let same_pid = fixture_text(&fixture.pid)? == first_pid;
        let transcribes = fixture_text(&fixture.transcribes)?.lines().count();
        let loads = fixture_text(&fixture.loads)?;
        let no_overlap = !fixture.overlap.exists();
        let wav_removed = !audio_path.exists();
        let loaded_model =
            fixture.runtime.snapshot().loaded_model_id.as_deref() == Some(fixture.model_id);
        Ok((
            result.0,
            same_pid,
            transcribes,
            loads,
            no_overlap,
            wav_removed,
            loaded_model,
            dispatches_before_terminal,
            dispatches_between_terminal_and_ack,
            wav_before_terminal,
            wav_between_terminal_and_ack,
            unload_blocked_between_terminal_and_ack,
            cancel_arrived_while_worker_held,
            worker_ended_and_joined_before_terminal,
            idle_unload_scheduled.load(Ordering::Acquire),
        ))
    }
    .await;
    let scenario = finish_cancellation_fixture(fixture, &mut tasks, &mut unload, scenario).await;
    let (facts, reaped_count) = scenario.unwrap_or_else(|error| {
        panic!("cancellation reuse scenario failed after teardown: {error}")
    });
    assert_eq!(facts.0, "synthetic speech");
    assert!(facts.1, "valid cancellation must reuse the same child");
    assert_eq!(facts.2, 2);
    assert_eq!(facts.3, "1");
    assert!(facts.4, "at most one inference may be active");
    assert!(facts.5, "cancelled request WAV must be removed after ACK");
    assert!(facts.6, "the loaded model must remain admitted after ACK");
    assert_eq!(facts.7, 1, "N+1 must remain blocked before terminal");
    assert_eq!(
        facts.8, 1,
        "N+1 must remain blocked between terminal and ACK"
    );
    assert!(facts.9, "WAV must remain owned before terminal");
    assert!(facts.10, "WAV must remain owned between terminal and ACK");
    assert!(facts.11, "unload/model-delete admission must wait for ACK");
    assert!(
        facts.12,
        "the independent control reader must receive cancel while the synthetic worker is held"
    );
    assert!(
        facts.13,
        "the actual synthetic worker must end and its handle be joined before terminal"
    );
    assert!(
        facts.14,
        "cancellation must schedule idle unload for the resident model"
    );
    assert_eq!(reaped_count, 1);
}

#[cfg(test)]
#[tokio::test]
async fn transcription_cancel_timeout_falls_back_to_kill() {
    let audio_visible_at_exit = Arc::new(AtomicBool::new(false));
    let observed_audio = Arc::clone(&audio_visible_at_exit);
    let owner_path = Arc::new(Mutex::new(None::<PathBuf>));
    let observed_owner_path = Arc::clone(&owner_path);
    let observer: Arc<dyn Fn(StageEvent) + Send + Sync> = Arc::new(move |event| {
        if event.name == "cancellation_child_exit_observed" {
            if let Some(path) = observed_owner_path.lock().unwrap().as_ref() {
                observed_audio.store(path.exists(), Ordering::Release);
            }
        }
    });
    let fixture = asr_cancellation_fixture("timeout", observer);
    let mut tasks: [Option<AsrFixtureTranscriptionTask>; 2] = [None, None];
    let mut unload = None;
    let scenario = async {
        let runtime = Arc::clone(&fixture.runtime);
        let model_id = fixture.model_id;
        tasks[0] = Some(tokio::spawn(async move {
            runtime
                .transcribe(model_id, Some("en"), synthetic_wav(), Some(402), None)
                .await
        }));
        wait_for_fixture_path(&fixture.started).await?;
        let audio_path = PathBuf::from(fixture_text(&fixture.started)?);
        let first_pid = fixture_text(&fixture.pid)?;
        *owner_path.lock().map_err(|error| error.to_string())? = Some(audio_path.clone());
        if !audio_path.exists() {
            return Err("WAV absent before cancellation".into());
        }
        let first = tasks[0]
            .take()
            .ok_or_else(|| "first transcription task was not retained".to_owned())?;
        first.abort();
        match first.await {
            Err(error) if error.is_cancelled() => {}
            Err(error) => return Err(format!("first task aborted unexpectedly: {error}")),
            Ok(_) => {
                return Err("first task returned instead of observing caller cancellation".into())
            }
        }
        wait_for_fixture_path(&fixture.cancel_seen).await?;

        let next_runtime = Arc::clone(&fixture.runtime);
        let next_model = fixture.model_id;
        tasks[1] = Some(tokio::spawn(async move {
            next_runtime
                .transcribe(next_model, Some("en"), synthetic_wav(), Some(402), None)
                .await
        }));
        tokio::time::sleep(Duration::from_millis(75)).await;
        let dispatches_before_exit = fixture_text(&fixture.transcribes)?.lines().count();
        let wav_before_exit = audio_path.exists();
        let joined = {
            let next = tasks[1]
                .as_mut()
                .ok_or_else(|| "replacement transcription task was not retained".to_owned())?;
            tokio::time::timeout(Duration::from_secs(6), next)
                .await
                .map_err(|_| "replacement did not start after the old Child exited".to_owned())?
        };
        tasks[1] = None;
        let result = joined.map_err(|error| error.to_string())??;
        Ok((
            result.0,
            fixture_text(&fixture.pid)? != first_pid,
            fixture_text(&fixture.loads)?,
            !fixture.overlap.exists(),
            audio_visible_at_exit.load(Ordering::Acquire),
            !audio_path.exists(),
            dispatches_before_exit,
            wav_before_exit,
        ))
    }
    .await;
    let scenario = finish_cancellation_fixture(fixture, &mut tasks, &mut unload, scenario).await;
    let (facts, reaped_count) = scenario.unwrap_or_else(|error| {
        panic!("timeout cancellation scenario failed after teardown: {error}")
    });
    assert_eq!(facts.0, "synthetic speech");
    assert!(facts.1, "replacement must use a new child after timeout");
    assert_eq!(facts.2, "2");
    assert!(facts.3, "replacement cannot overlap the old child");
    assert!(facts.4, "WAV must remain owned through observed exit");
    assert!(facts.5, "WAV must be removed after observed exit");
    assert_eq!(
        facts.6, 1,
        "N+1 must remain blocked while exit is unobserved"
    );
    assert!(facts.7, "WAV must remain owned while exit is unobserved");
    assert_eq!(reaped_count, 2);
}

#[cfg(test)]
#[tokio::test]
async fn transcription_cancel_requires_both_ordered_frames() {
    for mode in ["ack-only", "terminal-only"] {
        let audio_visible_at_exit = Arc::new(AtomicBool::new(false));
        let observed_audio = Arc::clone(&audio_visible_at_exit);
        let owner_path = Arc::new(Mutex::new(None::<PathBuf>));
        let observed_owner_path = Arc::clone(&owner_path);
        let observer: Arc<dyn Fn(StageEvent) + Send + Sync> = Arc::new(move |event| {
            if event.name == "cancellation_child_exit_observed" {
                if let Some(path) = observed_owner_path.lock().unwrap().as_ref() {
                    observed_audio.store(path.exists(), Ordering::Release);
                }
            }
        });
        let fixture = asr_cancellation_fixture(mode, observer);
        let mut tasks: [Option<AsrFixtureTranscriptionTask>; 2] = [None, None];
        let mut unload = None;
        let scenario = async {
            let runtime = Arc::clone(&fixture.runtime);
            let model_id = fixture.model_id;
            tasks[0] = Some(tokio::spawn(async move {
                runtime
                    .transcribe(model_id, Some("en"), synthetic_wav(), Some(403), None)
                    .await
            }));
            wait_for_fixture_path(&fixture.started).await?;
            let audio_path = PathBuf::from(fixture_text(&fixture.started)?);
            *owner_path.lock().map_err(|error| error.to_string())? = Some(audio_path.clone());
            let first = tasks[0]
                .take()
                .ok_or_else(|| "first transcription task was not retained".to_owned())?;
            first.abort();
            match first.await {
                Err(error) if error.is_cancelled() => {}
                Err(error) => return Err(format!("first task aborted unexpectedly: {error}")),
                Ok(_) => {
                    return Err(
                        "first task returned instead of observing caller cancellation".into(),
                    )
                }
            }
            wait_for_fixture_path(&fixture.cancel_seen).await?;

            let next_runtime = Arc::clone(&fixture.runtime);
            let next_model = fixture.model_id;
            tasks[1] = Some(tokio::spawn(async move {
                next_runtime
                    .transcribe(next_model, Some("en"), synthetic_wav(), Some(403), None)
                    .await
            }));
            let joined = {
                let next = tasks[1]
                    .as_mut()
                    .ok_or_else(|| "replacement transcription task was not retained".to_owned())?;
                tokio::time::timeout(Duration::from_secs(6), next)
                    .await
                    .map_err(|_| format!("replacement did not follow {mode} retirement"))?
            };
            tasks[1] = None;
            let result = joined.map_err(|error| error.to_string())??;
            Ok((
                result.0,
                audio_visible_at_exit.load(Ordering::Acquire),
                !audio_path.exists(),
                fixture.overlap.exists(),
                fixture.runtime.fixture_reaped.lock().unwrap().len(),
            ))
        }
        .await;
        let scenario =
            finish_cancellation_fixture(fixture, &mut tasks, &mut unload, scenario).await;
        let (facts, reaped_count) = scenario.unwrap_or_else(|error| {
            panic!("{mode} cancellation scenario failed after teardown: {error}")
        });
        assert_eq!(facts.0, "synthetic speech");
        assert!(facts.1, "{mode} must retain WAV until exit");
        assert!(facts.2, "{mode} must release owned WAV after settlement");
        assert!(!facts.3, "{mode} must not admit N+1 before settlement");
        assert_eq!(
            facts.4, 1,
            "{mode} old child must be reaped before replacement"
        );
        assert_eq!(reaped_count, 2);
    }
}

#[cfg(test)]
#[tokio::test]
async fn owner_rejects_non_completed_transcription_status() {
    let fixture = asr_cancellation_fixture("loaded-with-result", Arc::new(|_| {}));
    let mut tasks: [Option<AsrFixtureTranscriptionTask>; 2] = [None, None];
    let mut unload = None;
    let scenario = async {
        let runtime = Arc::clone(&fixture.runtime);
        let model_id = fixture.model_id;
        tasks[0] = Some(tokio::spawn(async move {
            runtime
                .transcribe(model_id, Some("en"), synthetic_wav(), Some(404), None)
                .await
        }));
        wait_for_fixture_path(&fixture.started).await?;
        let task = tasks[0]
            .as_mut()
            .ok_or_else(|| "transcription task was not retained".to_owned())?;
        let response = tokio::time::timeout(Duration::from_secs(6), task)
            .await
            .map_err(|_| "invalid status did not retire the child".to_owned())?
            .map_err(|error| error.to_string())?;
        let rejected = response.is_err();
        let _ = tasks[0].take();
        Ok(rejected)
    }
    .await;
    let scenario = finish_cancellation_fixture(fixture, &mut tasks, &mut unload, scenario).await;
    let (rejected, reaped_count) = scenario
        .unwrap_or_else(|error| panic!("invalid-status scenario failed after teardown: {error}"));
    assert!(
        rejected,
        "only a completed status may admit a transcription result"
    );
    assert_eq!(
        reaped_count, 1,
        "invalid completed operation must retire its child"
    );
}

#[cfg(test)]
#[tokio::test]
async fn public_owner_preserves_typed_result_rejection_after_reap_and_audio_cleanup() {
    use crate::observations::{FixtureCapture, ObservedText};

    for (mode, expected_code, expected_terminal, confirmed_empty) in [
        (
            "result-reject-empty",
            "local_empty_output_not_accepted",
            "local_complete_empty",
            true,
        ),
        (
            "result-reject-whitespace",
            "local_empty_output_not_accepted",
            "locally_suppressed",
            true,
        ),
        (
            "result-reject-coverage",
            "local_coverage_incomplete",
            "provider_failed",
            false,
        ),
        (
            "result-reject-budget",
            "local_generation_budget_invalid",
            "provider_failed",
            false,
        ),
        (
            "result-reject-incomplete",
            "local_completion_incomplete",
            "provider_failed",
            false,
        ),
    ] {
        let fixture = asr_cancellation_fixture(mode, Arc::new(|_| {}));
        let capture = FixtureCapture::new(false);
        let result = capture
            .scope(async {
                fixture
                    .runtime
                    .transcribe(
                        fixture.model_id,
                        Some("en"),
                        synthetic_wav(),
                        Some(410),
                        None,
                    )
                    .await
            })
            .await;
        let status_after_rejection = fixture.runtime.snapshot();
        fixture
            .runtime
            .shutdown_with_status()
            .await
            .expect("runtime shutdown succeeds");
        let observed = capture.snapshot();
        let local = observed.local_asr.expect("public owner observations");
        let asr = observed.asr.expect("public ASR observation");
        let receipts = fixture
            .runtime
            .fixture_reaped
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let reaped = receipts.len() == 1 && receipts.iter().all(Result::is_ok);
        drop(receipts);

        assert!(result.is_err(), "{mode} must not return successful text");
        assert_eq!(fixture.runtime.active_pid.load(Ordering::Acquire), 0);
        assert!(
            reaped,
            "{mode} child must be actually waited before typed handoff"
        );
        assert_eq!(status_after_rejection.runtime_status, "runtime_failed");
        assert_eq!(
            local.coverage.as_ref().map(|coverage| coverage.accepted),
            Some(false)
        );
        assert_eq!(asr.filtering_reason.as_deref(), Some(expected_code));
        assert_eq!(asr.terminal, expected_terminal);
        assert_eq!(asr.confirmed_empty(), confirmed_empty);
        if confirmed_empty {
            assert!(matches!(
                asr.provider_text,
                ObservedText::KnownEmpty {} | ObservedText::KnownValue { .. }
            ));
            assert!(matches!(asr.filtered_text, ObservedText::KnownEmpty {}));
        }
        assert!(local
            .private_audio_resources
            .iter()
            .any(|fact| fact.action == "created"));
        let removed = local
            .private_audio_resources
            .iter()
            .find(|fact| fact.action == "removed_owned_file")
            .expect("owned WAV removal must be recorded from the owner task");
        assert!(!Path::new(&removed.selected_path).exists());
        assert_eq!(local.private_audio_resources.len(), 2);
    }
}

#[cfg(test)]
#[tokio::test]
async fn owner_rejects_completed_transcription_with_invalid_admission_fields() {
    for mode in [
        "invalid-admission-platform",
        "invalid-admission-error",
        "invalid-admission-cleanup",
        "invalid-admission-warmup",
    ] {
        let fixture = asr_cancellation_fixture(mode, Arc::new(|_| {}));
        let mut tasks: [Option<AsrFixtureTranscriptionTask>; 2] = [None, None];
        let mut unload = None;
        let scenario = async {
            let runtime = Arc::clone(&fixture.runtime);
            tasks[0] = Some(tokio::spawn(async move {
                runtime
                    .transcribe(
                        crate::ondevice_models::QWEN_ASR_06_ID,
                        Some("en"),
                        synthetic_wav(),
                        Some(407),
                        None,
                    )
                    .await
            }));
            let task = tasks[0]
                .as_mut()
                .ok_or_else(|| "invalid-admission task was not retained".to_owned())?;
            let result = tokio::time::timeout(Duration::from_secs(6), task)
                .await
                .map_err(|_| format!("{mode} response did not settle"))?
                .map_err(|error| format!("{mode} caller task failed: {error}"))?;
            let _ = tasks[0].take();
            Ok((result.is_err(), fixture.runtime.snapshot()))
        }
        .await;
        let (scenario, reaped_count) =
            finish_cancellation_fixture(fixture, &mut tasks, &mut unload, scenario)
                .await
                .unwrap_or_else(|error| panic!("{mode} response/teardown failed safely: {error}"));
        assert!(scenario.0, "{mode} must not return transcription text");
        assert_eq!(
            reaped_count, 1,
            "{mode} invalid response must reap its child"
        );
        assert_eq!(scenario.1.runtime_status, "runtime_failed");
        assert!(scenario.1.loaded_model_id.is_none());
    }
}

#[cfg(test)]
#[tokio::test]
async fn dropped_caller_keeps_rejected_response_retirement_owned() {
    let audio_visible_at_exit = Arc::new(AtomicBool::new(false));
    let observed_audio = Arc::clone(&audio_visible_at_exit);
    let owner_path = Arc::new(Mutex::new(None::<PathBuf>));
    let observed_owner_path = Arc::clone(&owner_path);
    let observer: Arc<dyn Fn(StageEvent) + Send + Sync> = Arc::new(move |event| {
        if event.name == "cancellation_child_exit_observed" {
            if let Some(path) = observed_owner_path.lock().unwrap().as_ref() {
                observed_audio.store(path.exists(), Ordering::Release);
            }
        }
    });
    let fixture = asr_cancellation_fixture("bad-completion", observer);
    let gate = Arc::new(FixtureRetirementGate::new());
    fixture
        .runtime
        .set_fixture_retirement_gate(Arc::clone(&gate));
    let mut tasks: [Option<AsrFixtureTranscriptionTask>; 2] = [None, None];
    let mut unload = None;
    let scenario = async {
        let runtime = Arc::clone(&fixture.runtime);
        let model_id = fixture.model_id;
        tasks[0] = Some(tokio::spawn(async move {
            runtime
                .transcribe(model_id, Some("en"), synthetic_wav(), Some(405), None)
                .await
        }));
        wait_for_fixture_path(&fixture.started).await?;
        let audio_path = PathBuf::from(fixture_text(&fixture.started)?);
        *owner_path.lock().map_err(|error| error.to_string())? = Some(audio_path.clone());
        gate.wait_until_entered().await?;
        let child_pid = fixture_text(&fixture.pid)?;
        let first = tasks[0]
            .take()
            .ok_or_else(|| "caller transcription task was not retained".to_owned())?;
        first.abort();
        match first.await {
            Err(error) if error.is_cancelled() => {}
            Err(error) => {
                return Err(format!(
                    "public task abort returned an unexpected error: {error}"
                ))
            }
            Ok(_) => return Err("public task survived explicit abort".into()),
        }
        let wav_held_after_caller_drop = audio_path.exists();
        let pid_held_after_caller_drop = fixture.runtime.active_pid.load(Ordering::Acquire) != 0;
        let next_runtime = Arc::clone(&fixture.runtime);
        let next_model = fixture.model_id;
        tasks[1] = Some(tokio::spawn(async move {
            next_runtime
                .transcribe(next_model, Some("en"), synthetic_wav(), Some(405), None)
                .await
        }));
        unload = Some({
            let runtime = Arc::clone(&fixture.runtime);
            let model_id = fixture.model_id;
            tokio::spawn(async move { runtime.unload(Some(model_id)).await })
        });
        tokio::time::sleep(Duration::from_millis(75)).await;
        let unload_blocked = !unload.as_ref().is_some_and(|task| task.is_finished());
        let only_first_dispatched = fixture_text(&fixture.transcribes)?.lines().count() == 1;
        let wav_held_while_exit_unobserved = audio_path.exists();
        gate.release();
        let joined = {
            let next = tasks[1]
                .as_mut()
                .ok_or_else(|| "replacement task was not retained".to_owned())?;
            tokio::time::timeout(Duration::from_secs(6), next)
                .await
                .map_err(|_| "replacement did not follow observed child exit".to_owned())?
        };
        tasks[1] = None;
        let result = joined.map_err(|error| error.to_string())??;
        Ok((
            result.0,
            wav_held_after_caller_drop,
            pid_held_after_caller_drop,
            unload_blocked,
            only_first_dispatched,
            wav_held_while_exit_unobserved,
            audio_visible_at_exit.load(Ordering::Acquire),
            !audio_path.exists(),
            fixture_text(&fixture.pid)? != child_pid,
            fixture_text(&fixture.loads)?,
        ))
    }
    .await;
    let scenario = finish_cancellation_fixture(fixture, &mut tasks, &mut unload, scenario).await;
    let (facts, reaped_count) = scenario.unwrap_or_else(|error| {
        panic!("dropped-caller retirement scenario failed after teardown: {error}")
    });
    assert_eq!(facts.0, "synthetic speech");
    assert!(facts.1, "caller Drop must not release its WAV");
    assert!(facts.2, "caller Drop must retain the actual Child PID");
    assert!(
        facts.3,
        "unload/model-delete admission must join the retiring owner"
    );
    assert!(
        facts.4,
        "N+1 must not dispatch before Child exit is observed"
    );
    assert!(
        facts.5,
        "WAV must remain owned while Child exit is unobserved"
    );
    assert!(facts.6, "WAV must still exist when Child exit is observed");
    assert!(facts.7, "WAV must be removed only after observed exit");
    assert!(facts.8, "replacement must use a fresh Child");
    assert_eq!(facts.9, "2", "replacement must reload after retirement");
    assert_eq!(reaped_count, 2);
}

#[cfg(test)]
#[tokio::test]
async fn begin_shutdown_retains_pid_until_child_exit_is_observed() {
    let fixture = asr_cancellation_fixture("cooperative", Arc::new(|_| {}));
    let mut tasks: [Option<AsrFixtureTranscriptionTask>; 2] = [None, None];
    let mut unload = None;
    let scenario = async {
        let runtime = Arc::clone(&fixture.runtime);
        let model_id = fixture.model_id;
        tasks[0] = Some(tokio::spawn(async move {
            runtime
                .transcribe(model_id, Some("en"), synthetic_wav(), Some(406), None)
                .await
        }));
        wait_for_fixture_path(&fixture.started).await?;
        let child_pid = fixture_text(&fixture.pid)?
            .parse::<u32>()
            .map_err(|error| error.to_string())?;
        fixture.runtime.begin_shutdown();
        let pid_retained_after_kill_request =
            fixture.runtime.active_pid.load(Ordering::Acquire) == child_pid;
        let joined = {
            let task = tasks[0]
                .as_mut()
                .ok_or_else(|| "transcription task was not retained".to_owned())?;
            tokio::time::timeout(Duration::from_secs(6), task)
                .await
                .map_err(|_| "shutdown did not settle the active Child".to_owned())?
        };
        tasks[0] = None;
        let request_failed = joined.map_err(|error| error.to_string())?.is_err();
        fixture
            .runtime
            .shutdown_with_status()
            .await
            .map_err(|reason| format!("shutdown PID scenario did not settle: {reason}"))?;
        let pid_cleared_after_shutdown = fixture.runtime.active_pid.load(Ordering::Acquire) == 0;
        Ok((
            pid_retained_after_kill_request,
            request_failed,
            pid_cleared_after_shutdown,
        ))
    }
    .await;
    let scenario = finish_cancellation_fixture(fixture, &mut tasks, &mut unload, scenario).await;
    let (facts, reaped_count) = scenario
        .unwrap_or_else(|error| panic!("shutdown PID scenario failed after teardown: {error}"));
    assert!(
        facts.0,
        "begin_shutdown must retain the Child identity until wait"
    );
    assert!(
        facts.1,
        "active transcription must fail after child termination"
    );
    assert!(facts.2, "shutdown may clear PID after observing Child exit");
    assert_eq!(reaped_count, 1);
}

#[cfg(test)]
#[tokio::test]
async fn dropping_public_load_future_retains_pid_until_child_wait() {
    let fixture = asr_cancellation_fixture("load-hold", Arc::new(|_| {}));
    let mut tasks: [Option<AsrFixtureTranscriptionTask>; 2] = [None, None];
    let mut unload = None;
    let scenario = async {
        let runtime = Arc::clone(&fixture.runtime);
        let model_id = fixture.model_id;
        tasks[0] = Some(tokio::spawn(async move {
            runtime
                .transcribe(model_id, Some("en"), synthetic_wav(), Some(408), None)
                .await
        }));
        wait_for_fixture_path(&fixture.started).await?;
        if fixture_text(&fixture.started)? != "load-held" {
            return Err("fixture did not hold the actual model-load exchange".into());
        }
        let child_pid = fixture.runtime.active_pid.load(Ordering::Acquire);
        if child_pid == 0 {
            return Err("model-load worker PID was not retained".into());
        }
        let task = tasks[0]
            .take()
            .ok_or_else(|| "model-load transcription task was not retained".to_owned())?;
        task.abort();
        match task.await {
            Err(error) if error.is_cancelled() => {}
            Err(error) => return Err(format!("load task aborted unexpectedly: {error}")),
            Ok(_) => return Err("aborted load future unexpectedly returned".into()),
        }
        let pid_retained_before_wait =
            fixture.runtime.active_pid.load(Ordering::Acquire) == child_pid;
        let file_path = model_dir(&fixture.runtime.models_root, model_id).join("fixture.bin");
        fixture.runtime.unload(Some(model_id)).await?;
        let pid_cleared_after_wait = fixture.runtime.active_pid.load(Ordering::Acquire) == 0;
        let receipts = fixture
            .runtime
            .fixture_reaped
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let wait_was_observed = receipts.len() == 1 && receipts.iter().all(Result::is_ok);
        drop(receipts);
        let session_cleared_after_wait = fixture
            .runtime
            .session
            .try_lock()
            .is_ok_and(|session| session.is_none());
        if !pid_cleared_after_wait || !session_cleared_after_wait || !wait_was_observed {
            return Err("unload did not wait for the retained load child before returning".into());
        }
        std::fs::remove_file(&file_path).map_err(|error| error.to_string())?;
        Ok((
            pid_retained_before_wait,
            pid_cleared_after_wait,
            session_cleared_after_wait,
            !file_path.exists(),
        ))
    }
    .await;
    let scenario = finish_cancellation_fixture(fixture, &mut tasks, &mut unload, scenario).await;
    let (facts, reaped_count) = scenario
        .unwrap_or_else(|error| panic!("dropped public load future PID scenario failed: {error}"));
    assert!(
        facts.0,
        "dropping the public load future must retain the PID until wait"
    );
    assert!(
        facts.1,
        "public unload clears the PID only after Child::wait"
    );
    assert!(
        facts.2,
        "public unload releases the child session only after wait"
    );
    assert!(
        facts.3,
        "fixture model file can be deleted only after unload reaps the child"
    );
    assert_eq!(reaped_count, 1);
}

#[cfg(test)]
#[tokio::test]
async fn successful_transcription_handoff_cannot_overwrite_later_unload_or_model_reload() {
    for replace_with_second_model in [false, true] {
        let fixture = asr_cancellation_fixture("success-immediate", Arc::new(|_| {}));
        let model_id = fixture.model_id;
        let gate = Arc::new(FixtureRetirementGate::new());
        fixture
            .runtime
            .set_fixture_success_handoff_gate(Arc::clone(&gate));
        let mut tasks: [Option<AsrFixtureTranscriptionTask>; 2] = [None, None];
        let mut unload = None;
        let scenario = async {
            let runtime = Arc::clone(&fixture.runtime);
            let model_id = fixture.model_id;
            tasks[0] = Some(tokio::spawn(async move {
                runtime
                    .transcribe(model_id, Some("en"), synthetic_wav(), Some(409), None)
                    .await
            }));
            gate.wait_until_entered().await?;
            if tasks[0].as_ref().is_some_and(|task| task.is_finished()) {
                return Err("caller handoff gate did not retain the response".into());
            }
            if replace_with_second_model {
                let previous_generation = fixture.runtime.load_generation.load(Ordering::Acquire);
                fixture.runtime.unload(Some(fixture.model_id)).await?;
                fixture.runtime.preload(fixture.model_id).await?;
                let reloaded_generation = fixture.runtime.load_generation.load(Ordering::Acquire);
                if reloaded_generation <= previous_generation {
                    return Err("same-profile reload did not advance the model generation".into());
                }
            } else {
                fixture.runtime.unload(Some(fixture.model_id)).await?;
            }
            let state_after_runtime_operation = fixture.runtime.snapshot();
            gate.release();
            let task = tasks[0]
                .as_mut()
                .ok_or_else(|| "success-handoff task was not retained".to_owned())?;
            let result = tokio::time::timeout(Duration::from_secs(6), task)
                .await
                .map_err(|_| "success-handoff caller did not resume".to_owned())?
                .map_err(|error| format!("success-handoff caller failed: {error}"))?;
            let _ = tasks[0].take();
            let final_state = fixture.runtime.snapshot();
            Ok((result?, state_after_runtime_operation, final_state))
        }
        .await;
        let (scenario, reaped_count) =
            finish_cancellation_fixture(fixture, &mut tasks, &mut unload, scenario)
                .await
                .unwrap_or_else(|error| {
                    panic!("successful handoff race failed after child-safe teardown: {error}")
                });
        assert_eq!(reaped_count, 1);
        let (transcription, state_after_runtime_operation, final_state) = scenario;
        assert_eq!(transcription.0, "synthetic speech");
        assert_eq!(
            state_after_runtime_operation.runtime_status,
            final_state.runtime_status
        );
        assert_eq!(
            state_after_runtime_operation.loaded_model_id, final_state.loaded_model_id,
            "a completed caller must not restore a stale loaded model after unload/switch"
        );
        if replace_with_second_model {
            assert_eq!(
                state_after_runtime_operation.loaded_model_id.as_deref(),
                Some(model_id),
                "the N+1 calibrated same-profile load remains resident after the old handoff"
            );
            assert_eq!(final_state.loaded_model_id.as_deref(), Some(model_id));
            assert_eq!(final_state.runtime_status, "loaded");
        } else {
            assert!(final_state.loaded_model_id.is_none());
            assert_eq!(final_state.runtime_status, "runtime_ready");
        }
    }
}

#[cfg(test)]
#[tokio::test]
async fn uncalibrated_asr_cold_load_is_denied_before_load_dispatch() {
    let fixture = asr_cancellation_fixture("success-immediate", Arc::new(|_| {}));
    let second_model_dir = model_dir(
        &fixture.runtime.models_root,
        crate::ondevice_models::QWEN_ASR_17_ID,
    );
    std::fs::create_dir_all(&second_model_dir).expect("create 1.7B fixture model directory");
    std::fs::copy(
        model_dir(&fixture.runtime.models_root, fixture.model_id).join("fixture.bin"),
        second_model_dir.join("fixture.bin"),
    )
    .expect("prepare the pinned file fixture for the uncalibrated model");

    let error = fixture
        .runtime
        .preload(crate::ondevice_models::QWEN_ASR_17_ID)
        .await
        .expect_err("uncalibrated ASR 1.7B cold loading is paused");
    assert!(error.contains("内存预算"));
    assert!(
        !fixture.loads.exists(),
        "admission refusal must happen before any ASR LOAD dispatch"
    );
    fixture
        .runtime
        .shutdown_with_status()
        .await
        .expect("runtime shutdown succeeds");
    let receipts = fixture
        .runtime
        .fixture_reaped
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(
        receipts.len(),
        1,
        "denied preload waits for its sidecar child"
    );
    assert!(receipts.iter().all(Result::is_ok));
}

#[cfg(test)]
#[tokio::test]
async fn fake_runtime_lock_wait_drop_finishes_once_without_spawning_child() {
    let owner = crate::evaluation::fixtures::FixtureDirectory::new();
    let models_root = owner.0.join("models");
    let model_id = crate::ondevice_models::QWEN_ASR_06_ID;
    let model_directory = model_dir(&models_root, model_id);
    std::fs::create_dir_all(&model_directory).unwrap();
    let bytes = b"tiny lock fixture";
    std::fs::write(model_directory.join("fixture.bin"), bytes).unwrap();
    let notify = Arc::new(tokio::sync::Notify::new());
    let callback_notify = Arc::clone(&notify);
    let runtime = Arc::new(OnDeviceRuntime::with_fixture(
        models_root,
        FixtureConfig {
            expected_files: vec![(
                "fixture.bin".into(),
                bytes.len() as u64,
                hex::encode(Sha256::digest(bytes)),
            )],
            worker_program: PathBuf::from("python3"),
            worker_args: vec![OsString::from("fixture-never-run.py")],
            observer: Arc::new(move |event| {
                if event.name == "runtime_lock_wait_started" {
                    callback_notify.notify_one();
                }
            }),
        },
    ));
    let mut timer = crate::metrics::Metrics::default().stop_to_insert(
        crate::metrics::MetricGroup::unknown("test"),
        tokio_util::sync::CancellationToken::new(),
    );
    let trace = timer.bind_trace(62);
    let held = runtime.session.lock().await;
    let task_runtime = Arc::clone(&runtime);
    let task_trace = trace.clone();
    let task = tokio::spawn(async move {
        task_runtime
            .transcribe(
                model_id,
                Some("en"),
                synthetic_wav(),
                Some(62),
                Some(task_trace),
            )
            .await
    });
    let notification = tokio::time::timeout(Duration::from_secs(5), notify.notified()).await;
    task.abort();
    let _ = task.await;
    drop(held);
    runtime
        .shutdown_with_status()
        .await
        .expect("runtime shutdown succeeds");
    runtime.assert_fixture_reaped_count(0);
    assert!(notification.is_ok(), "runtime-lock wait was not reached");
    let stages = trace.snapshot().stages;
    assert_eq!(
        stages
            .iter()
            .filter(|(stage, _, _)| *stage == crate::metrics::Stage::QueueWait)
            .count(),
        1
    );
    assert_eq!(
        stages.len(),
        1,
        "cancelled runtime-lock wait must not reach later stages"
    );
    timer.finish(crate::metrics::DeliveryOutcome::Cancelled, None);
}

#[cfg(test)]
fn synthetic_wav() -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(&mut bytes, spec).unwrap();
        for _ in 0..160 {
            writer.write_sample(0_i16).unwrap();
        }
        writer.finalize().unwrap();
    }
    bytes.into_inner()
}

#[cfg(test)]
#[test]
fn local_asr_stage_emitter_panic_is_isolated_and_does_not_repeat_on_drop() {
    let diagnostics = LocalAsrDiagnostics::new(
        "vf-test".into(),
        None,
        "local_asr_test",
        "test-revision",
        LocalAsrOperation::Transcribe,
    );
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let mut timer =
        diagnostics.stage_with_emitter("inference_terminal", move |_, _, _, _, _, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            panic!("test diagnostics emitter panic");
        });
    let result: Result<&str, &str> = Ok("unchanged result");

    timer.finish("completed");
    drop(timer);

    assert_eq!(result, Ok("unchanged result"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[cfg(test)]
#[test]
fn local_asr_diagnostics_catch_does_not_swallow_runtime_panic() {
    let diagnostics = LocalAsrDiagnostics::new(
        "vf-test".into(),
        None,
        "local_asr_test",
        "test-revision",
        LocalAsrOperation::Transcribe,
    );
    let runtime_panic = std::panic::catch_unwind(|| {
        let _timer = diagnostics.stage_with_emitter("request_total", |_, _, _, _, _, _| {});
        panic!("original runtime panic");
    });
    assert!(runtime_panic.is_err());
}

pub struct OnDeviceRuntime {
    models_root: PathBuf,
    active_pid: Arc<AtomicU32>,
    sequence: AtomicU64,
    load_generation: AtomicU64,
    shutting_down: AtomicBool,
    child_creation: Mutex<()>,
    loading_model_id: Mutex<Option<String>>,
    session: Arc<tokio::sync::Mutex<Option<ChildSession>>>,
    status: Arc<Mutex<RuntimeSnapshot>>,
    verified_files: Mutex<HashMap<String, Vec<FileIdentity>>>,
    hashed_model_bytes: Arc<AtomicU64>,
    loaded_attestation: Arc<Mutex<Option<crate::observations::LoadedAttestation>>>,
    idle_generation: AtomicU64,
    idle_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    transcription_owners: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    #[cfg(test)]
    fixture: Option<FixtureConfig>,
    #[cfg(test)]
    fixture_reaped: Arc<Mutex<Vec<Result<ExitStatus, String>>>>,
    #[cfg(test)]
    fixture_retirement_gate: Mutex<Option<Arc<FixtureRetirementGate>>>,
    #[cfg(test)]
    fixture_success_handoff_gate: Mutex<Option<Arc<FixtureRetirementGate>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    path: String,
    bytes: u64,
    modified_ns: u128,
    changed_ns: i128,
    device: u64,
    inode: u64,
    mode: u32,
    owner: u32,
}

#[derive(Debug, Deserialize)]
struct AsrInstalledManifest {
    schema_version: u32,
    id: String,
    revision: String,
    files: Vec<AsrInstalledManifestFile>,
}

#[derive(Debug, Deserialize)]
struct AsrInstalledManifestFile {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct AsrDurableProof {
    schema_version: u32,
    model_id: String,
    repository: String,
    revision: String,
    manifest_sha256: String,
    models_root: String,
    models_root_identity: crate::modelstore::DurableFileIdentity,
    model_directory: String,
    model_directory_identity: crate::modelstore::DurableFileIdentity,
    installed_manifest_sha256: String,
    installed_manifest_identity: crate::modelstore::DurableFileIdentity,
    files: Vec<AsrDurableProofFile>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AsrDurableProofFile {
    path: String,
    bytes: u64,
    sha256: String,
    identity: crate::modelstore::DurableFileIdentity,
}

struct AsrProofBinding<'a> {
    model_id: &'a str,
    repository: &'a str,
    revision: &'a str,
    manifest_sha256: String,
    expected: &'a [(String, u64, String)],
}

enum AsrProofPublishError {
    Cache(String),
    Integrity(String),
}

#[derive(Debug, PartialEq, Eq)]
struct AsrDurableSnapshot {
    models_root: String,
    models_root_identity: crate::modelstore::DurableFileIdentity,
    model_directory: String,
    model_directory_identity: crate::modelstore::DurableFileIdentity,
    installed_manifest_bytes: Vec<u8>,
    installed_manifest_identity: crate::modelstore::DurableFileIdentity,
    files: Vec<(String, u64, String, crate::modelstore::DurableFileIdentity)>,
    runtime_files: Vec<FileIdentity>,
}

impl OnDeviceRuntime {
    fn new(models_root: PathBuf) -> Self {
        Self {
            models_root,
            active_pid: Arc::new(AtomicU32::new(0)),
            sequence: AtomicU64::new(0),
            load_generation: AtomicU64::new(0),
            shutting_down: AtomicBool::new(false),
            child_creation: Mutex::new(()),
            loading_model_id: Mutex::new(None),
            session: Arc::new(tokio::sync::Mutex::new(None)),
            status: Arc::new(Mutex::new(RuntimeSnapshot::initial())),
            verified_files: Mutex::new(HashMap::new()),
            hashed_model_bytes: Arc::new(AtomicU64::new(0)),
            loaded_attestation: Arc::new(Mutex::new(None)),
            idle_generation: AtomicU64::new(0),
            idle_task: Mutex::new(None),
            transcription_owners: Mutex::new(Vec::new()),
            #[cfg(test)]
            fixture: None,
            #[cfg(test)]
            fixture_reaped: Arc::new(Mutex::new(Vec::new())),
            #[cfg(test)]
            fixture_retirement_gate: Mutex::new(None),
            #[cfg(test)]
            fixture_success_handoff_gate: Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn with_fixture(models_root: PathBuf, config: FixtureConfig) -> Self {
        Self {
            models_root,
            active_pid: Arc::new(AtomicU32::new(0)),
            sequence: AtomicU64::new(0),
            load_generation: AtomicU64::new(0),
            shutting_down: AtomicBool::new(false),
            child_creation: Mutex::new(()),
            loading_model_id: Mutex::new(None),
            session: Arc::new(tokio::sync::Mutex::new(None)),
            status: Arc::new(Mutex::new(RuntimeSnapshot {
                platform_supported: true,
                runtime_status: "not_checked".into(),
                runtime_version: None,
                loaded_model_id: None,
                loaded_model_version: None,
                loading_model_id: None,
                error: None,
            })),
            verified_files: Mutex::new(HashMap::new()),
            hashed_model_bytes: Arc::new(AtomicU64::new(0)),
            loaded_attestation: Arc::new(Mutex::new(None)),
            idle_generation: AtomicU64::new(0),
            idle_task: Mutex::new(None),
            transcription_owners: Mutex::new(Vec::new()),
            fixture: Some(config),
            fixture_reaped: Arc::new(Mutex::new(Vec::new())),
            fixture_retirement_gate: Mutex::new(None),
            fixture_success_handoff_gate: Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn set_fixture_retirement_gate(&self, gate: Arc<FixtureRetirementGate>) {
        *self
            .fixture_retirement_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(gate);
    }

    #[cfg(test)]
    fn set_fixture_success_handoff_gate(&self, gate: Arc<FixtureRetirementGate>) {
        *self
            .fixture_success_handoff_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(gate);
    }

    fn runtime_platform_supported(&self) -> bool {
        #[cfg(test)]
        if self.fixture.is_some() {
            return true;
        }
        platform_supported()
    }

    fn model_files_ready(&self, model_id: &str) -> bool {
        #[cfg(test)]
        if let Some(fixture) = &self.fixture {
            return fixture.expected_files.iter().all(|(relative, bytes, _)| {
                std::fs::metadata(model_dir(&self.models_root, model_id).join(relative))
                    .is_ok_and(|metadata| metadata.is_file() && metadata.len() == *bytes)
            });
        }
        model_files_are_ready(&self.models_root, model_id)
    }

    fn expected_model_files(&self, model_id: &str) -> Result<Vec<(String, u64, String)>, String> {
        #[cfg(test)]
        if let Some(fixture) = &self.fixture {
            return Ok(fixture.expected_files.clone());
        }
        let manifest = mlx_model(model_id).ok_or_else(|| "unsupported local model".to_owned())?;
        Ok(manifest
            .files
            .iter()
            .map(|file| (file.path.to_owned(), file.bytes, file.sha256.to_owned()))
            .collect())
    }

    fn worker_command(&self) -> Result<Command, String> {
        #[cfg(test)]
        if let Some(fixture) = &self.fixture {
            let mut command = Command::new(&fixture.worker_program);
            command
                .arg("-B")
                .args(&fixture.worker_args)
                .env("PYTHONDONTWRITEBYTECODE", "1");
            return Ok(command);
        }
        let executable = sidecar_executable()
            .ok_or_else(|| "The VoiceFlow MLX runtime is not included in this build.".to_owned())?;
        let mut command = Command::new(executable);
        command.arg("--models-root").arg(&self.models_root);
        command
            .arg("--audio-root")
            .arg(self.models_root.join("audio-tmp"));
        Ok(command)
    }

    #[cfg(test)]
    fn observe_fixture(&self, name: &'static str) {
        if let Some(fixture) = &self.fixture {
            let observer = Arc::clone(&fixture.observer);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                observer(StageEvent { name });
            }));
        }
    }

    pub async fn status(&self) -> RuntimeSnapshot {
        #[cfg(test)]
        if self.fixture.is_some() {
            return self.snapshot();
        }
        let current = self.snapshot();
        if self.shutting_down.load(Ordering::Acquire) || current.runtime_status == "loading" {
            return current;
        }
        if !self.runtime_platform_supported() {
            return self.update_status("unsupported_platform", None, None, None, None);
        }
        if sidecar_executable().is_none() {
            return self.update_status("sidecar_missing", None, None, None, None);
        }
        let mut session = match self.session.try_lock() {
            Ok(session) => session,
            Err(_) => return self.snapshot(),
        };
        if let Err(error) = self.ensure_session(&mut session, None, None, None).await {
            return self.update_status("runtime_failed", None, None, None, Some(error));
        }
        let request = self.request("status", None, None);
        let response = match Self::exchange(
            session.as_mut().expect("sidecar session is available"),
            &request,
            Duration::from_secs(5),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => {
                self.kill_session(&mut session).await;
                return self.update_status("runtime_failed", None, None, None, Some(error));
            }
        };
        if !self.response_matches(&request, &response) || !response.is_successful() {
            let message = response.error_message("sidecar status failed");
            self.kill_session(&mut session).await;
            return self.update_status("runtime_failed", None, None, None, Some(message));
        }
        let loaded_id = response.loaded_model_id.clone();
        let loaded_version = response.loaded_model_version.clone();
        self.update_status(
            if loaded_id.is_some() {
                "loaded"
            } else {
                "runtime_ready"
            },
            response.runtime_version,
            loaded_id,
            loaded_version,
            None,
        )
    }

    pub async fn transcribe(
        &self,
        model_id: &str,
        language: Option<&str>,
        audio: Vec<u8>,
        session_generation: Option<u64>,
        trace: Option<crate::metrics::TraceHandle>,
    ) -> Result<(String, Option<String>), String> {
        if let Some(trace) = &trace {
            if trace.snapshot().session_generation != session_generation.unwrap_or_default() {
                return Err("ASR trace generation does not match the admitted session".into());
            }
        }
        self.ensure_running()?;
        let manifest = mlx_model(model_id).ok_or_else(|| "unsupported local model".to_owned())?;
        if !self.runtime_platform_supported() {
            return Err(
                "Local MLX transcription requires Apple Silicon and macOS 14 or later.".into(),
            );
        }
        if !self.model_files_ready(model_id) {
            return Err("The selected local model files are missing or corrupt.".into());
        }
        let audio_samples = validate_wav(&audio)?;
        match manifest.language_policy {
            MlxLanguagePolicy::AutoOrExplicit => {
                if language.is_some_and(|value| !matches!(value, "zh" | "en")) {
                    return Err("Qwen local ASR accepts auto, Chinese, or English.".into());
                }
            }
            MlxLanguagePolicy::ExplicitChineseOrEnglish => {
                if !language.is_some_and(|value| matches!(value, "zh" | "en")) {
                    return Err("Cohere local ASR requires an explicit Chinese or English language setting.".into());
                }
            }
        }

        let request_id = self.next_id();
        let session_id = request_id.clone();
        let version = manifest.revision.to_owned();
        let diagnostics = LocalAsrDiagnostics::new(
            request_id.clone(),
            session_generation,
            manifest.id,
            manifest.revision,
            LocalAsrOperation::Transcribe,
        );
        let mut request_total = diagnostics.stage("request_total");
        let mut runtime_lock_span = trace
            .as_ref()
            .map(|trace| trace.begin(crate::metrics::Stage::QueueWait));
        let mut files_diagnostic = diagnostics.deferred_stage("files_validated");
        let mut session_diagnostic = diagnostics.deferred_stage("sidecar_session");
        let mut handshake_diagnostic = diagnostics.deferred_stage("handshake");
        let mut model_load_diagnostic = diagnostics.deferred_stage("model_load");
        let mut inference_terminal = diagnostics.deferred_stage("inference_terminal");
        let mut inference_exchange = diagnostics.deferred_stage("inference_exchange");
        let directory = match checked_model_directory(&self.models_root, model_id) {
            Ok(directory) => directory,
            Err(error) => {
                return request_total.fail(error);
            }
        };
        let mut queue_observation = diagnostics.stage("queue");
        #[cfg(test)]
        self.observe_fixture("runtime_lock_wait_started");
        if let Err(error) = self.join_transcription_owner().await {
            return request_total.fail(error);
        }
        let mut session = Arc::clone(&self.session).lock_owned().await;
        if let Some(span) = runtime_lock_span.as_mut() {
            span.finish();
        }
        queue_observation.finish("completed");
        let previous_status = self.snapshot();
        let loading_generation = match self.begin_model_load(&mut session, model_id, &version) {
            Ok(generation) => generation,
            Err(error) => {
                drop(session);
                return request_total.fail(error);
            }
        };
        let mut load_guard = loading_generation.map(|generation| {
            ModelLoadCancellationGuard::new(self, model_id, generation, previous_status)
        });
        let mut integrity_observation =
            LocalStageGuard::start(model_id, &version, LocalStage::FilesValidated);
        let mut file_validation_span = trace
            .as_ref()
            .map(|trace| trace.begin(crate::metrics::Stage::FileValidation));
        files_diagnostic.start();
        if let Err(error) = self.verify_model_integrity(model_id).await {
            integrity_observation.failed("local_model_integrity_failed");
            if let Some(guard) = load_guard.as_mut() {
                guard.finish_failure(&error);
            } else {
                self.finish_model_load_failure(model_id, loading_generation, &error);
            }
            files_diagnostic.finish("failed");
            return request_total.fail(error);
        }
        #[cfg(test)]
        self.observe_fixture("file_validation");
        files_diagnostic.finish("completed");
        if let Some(span) = file_validation_span.as_mut() {
            span.finish();
        }
        integrity_observation.completed(None, None);

        let mut session_observation =
            LocalStageGuard::start(model_id, &version, LocalStage::RuntimeSession);
        session_diagnostic.start();
        let session_spawned = match self
            .ensure_session(
                &mut session,
                load_guard.as_mut(),
                Some(&mut handshake_diagnostic),
                trace.as_ref(),
            )
            .await
        {
            Ok(spawned) => spawned,
            Err(error) => {
                session_observation.failed("local_runtime_unavailable_or_handshake_failed");
                if let Some(guard) = load_guard.as_mut() {
                    guard.finish_failure(&error);
                } else {
                    self.finish_model_load_failure(model_id, loading_generation, &error);
                }
                session_diagnostic.finish(local_asr_error_outcome(&error));
                return request_total.fail(error);
            }
        };
        #[cfg(test)]
        if session_spawned {
            self.observe_fixture("runtime_startup");
        }
        session_diagnostic.spawned_child(session_spawned);
        session_diagnostic.finish("completed");
        session_observation.completed(None, self.snapshot().runtime_version.as_deref());

        let pid = session
            .as_ref()
            .and_then(|child| child.child.id())
            .unwrap_or(0);
        if let Some(guard) = load_guard.as_mut() {
            guard.record_child_pid(pid);
        }
        let mut kill_guard = KillOnDrop::new(pid);
        let child = session.as_mut().expect("sidecar session is available");
        let mut model_load_span = if loading_generation.is_some() {
            trace
                .as_ref()
                .map(|trace| trace.begin(crate::metrics::Stage::ModelLoad))
        } else {
            None
        };
        model_load_diagnostic.start();
        let model_loaded = match self
            .ensure_model_loaded(
                child,
                model_id,
                &version,
                &directory,
                &request_id,
                &session_id,
                loading_generation,
            )
            .await
        {
            Ok(disposition) => disposition,
            Err(error) => {
                self.kill_session(&mut session).await;
                if let Some(guard) = load_guard.as_mut() {
                    guard.finish_failure(&error);
                } else {
                    self.finish_model_load_failure(model_id, loading_generation, &error);
                }
                model_load_diagnostic.finish(local_asr_error_outcome(&error));
                return request_total.fail(error);
            }
        };
        if let Some(guard) = load_guard.as_mut() {
            guard.disarm();
        }
        model_load_diagnostic.finish(if model_loaded { "loaded" } else { "reused" });
        if let Some(span) = model_load_span.as_mut() {
            span.finish();
        }
        #[cfg(test)]
        self.observe_fixture(if model_loaded {
            "model_load"
        } else {
            "model_reuse"
        });
        if let Err(error) = self.ensure_running() {
            drop(kill_guard);
            return request_total.fail(error);
        }
        // Startup scavenging runs during ensure_session. Create this request's
        // private WAV only after that boundary and while holding the one-model
        // runtime lock so no other request can mistake it for stale audio.
        let audio_file = match PrivateAudioFile::create(&self.models_root, &request_id, &audio) {
            Ok(file) => file,
            Err(error) => {
                drop(kill_guard);
                return request_total.fail(error);
            }
        };
        let audio_secs = audio_samples.div_ceil(16_000) as usize;
        let total_tokens = audio_secs.saturating_mul(128).clamp(4_096, 131_072);
        let request = Request {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            session_id: Some(session_id),
            model_version: Some(version.clone()),
            op: "transcribe".into(),
            model_id: Some(model_id.to_owned()),
            model_directory: None,
            audio_path: Some(audio_file.path.to_string_lossy().into_owned()),
            language: language.map(str::to_owned),
            audio_max_bytes: Some(MAX_AUDIO_BYTES),
            max_tokens_per_chunk: Some(4_096),
            max_tokens_total: Some(total_tokens),
        };
        let facts = file_request_facts(model_id, language, audio_samples);
        crate::observations::asr_request(facts);
        let deadline = Duration::from_secs((audio_secs as u64 * 20).clamp(120, 1_800));
        let mut inference_observation =
            LocalStageGuard::start(model_id, &version, LocalStage::Inference);
        let mut inference_span = trace
            .as_ref()
            .map(|trace| trace.begin(crate::metrics::Stage::AsrInference));
        inference_terminal.start();
        inference_exchange.start();
        let cancel_request = Request {
            protocol_version: PROTOCOL_VERSION,
            request_id: self.next_id(),
            session_id: request.session_id.clone(),
            model_version: request.model_version.clone(),
            op: "cancel".into(),
            model_id: request.model_id.clone(),
            model_directory: None,
            audio_path: None,
            language: None,
            audio_max_bytes: None,
            max_tokens_per_chunk: None,
            max_tokens_total: None,
        };
        let response_request = request.clone();
        let cancel = AsrCancellationSignal::new();
        let mut cancel_guard = CancelTranscriptionOnDrop::new(cancel.clone());
        let (response_sender, response_receiver) = tokio::sync::oneshot::channel();
        let owner_context = self.asr_owner_context();
        #[cfg(any(test, feature = "evaluation"))]
        let owner_capture = crate::observations::FixtureCapture::current();
        let admission_model_id = model_id.to_owned();
        let admission_version = version.clone();
        kill_guard.disarm();
        let owner_future = async move {
            let outcome = std::panic::AssertUnwindSafe(Self::drive_transcription(
                &mut session,
                &request,
                &cancel_request,
                &cancel,
                deadline,
            ))
            .catch_unwind()
            .await;
            match outcome {
                Err(_) => {
                    let error = "local ASR cancellation owner panicked";
                    owner_context.retire(&mut session, &audio_file, error).await;
                    drop(audio_file);
                    drop(session);
                    let _ = response_sender.send(Err(AsrOwnerError::Exchange(error.to_owned())));
                }
                Ok(Ok(AsrExchangeOutcome::Completed(response))) => {
                    let response = *response;
                    let result_rejection = response
                        .result
                        .as_ref()
                        .and_then(|result| result.rejection(audio_samples, total_tokens));
                    let loaded_identity_is_admitted = LoadedModelIdentity::parse(
                        response.loaded_model_id.as_deref(),
                        response.loaded_model_version.as_deref(),
                        &admission_model_id,
                        &admission_version,
                        LocalStage::Inference,
                    )
                    .is_ok();
                    let valid = Self::request_matches(&request, &response)
                        && response.status == "completed"
                        && response.platform_supported == Some(true)
                        && response.loaded_model_id.as_deref() == request.model_id.as_deref()
                        && response.loaded_model_version.as_deref()
                            == request.model_version.as_deref()
                        && response.cleanup_result.is_none()
                        && response.warmup_result.is_none()
                        && response.error.is_none()
                        && response.result.is_some()
                        && loaded_identity_is_admitted;
                    if valid && result_rejection.is_none() {
                        // Commit the loaded state before releasing the exact
                        // session gate, so a later unload or model switch cannot
                        // be overwritten by this caller's response handling.
                        owner_context.commit_transcription(&response);
                        drop(audio_file);
                        drop(session);
                        if response_sender.send(Ok(response)).is_err() {
                            log::debug!(
                                "caller dropped after a validated local transcription completed"
                            );
                        }
                    } else if valid {
                        let rejection = result_rejection.expect("checked result rejection");
                        let error = format!("local transcription rejected: {}", rejection.code());
                        owner_context
                            .retire(&mut session, &audio_file, &error)
                            .await;
                        drop(audio_file);
                        drop(session);
                        if response_sender
                            .send(Err(AsrOwnerError::RejectedResult {
                                response: Box::new(response),
                                rejection,
                            }))
                            .is_err()
                        {
                            log::debug!("caller dropped after rejected result retirement");
                        }
                    } else {
                        let error = "local ASR response identity or status was rejected".to_owned();
                        owner_context
                            .retire(&mut session, &audio_file, &error)
                            .await;
                        drop(audio_file);
                        drop(session);
                        if response_sender
                            .send(Err(AsrOwnerError::Exchange(error)))
                            .is_err()
                        {
                            log::debug!(
                                "caller dropped after rejected local ASR response retirement"
                            );
                        }
                    }
                }
                Ok(Ok(AsrExchangeOutcome::Cancelled)) => {
                    owner_context.schedule_idle_unload_if_loaded(&admission_model_id);
                    drop(audio_file);
                    drop(session);
                }
                Ok(Err(error)) => {
                    owner_context
                        .retire(&mut session, &audio_file, &error)
                        .await;
                    drop(audio_file);
                    drop(session);
                    let _ = response_sender.send(Err(AsrOwnerError::Exchange(error)));
                }
            }
        };
        let owner_handle = tokio::spawn(async move {
            #[cfg(any(test, feature = "evaluation"))]
            if let Some(capture) = owner_capture {
                capture.scope(owner_future).await;
            } else {
                owner_future.await;
            }
            #[cfg(not(any(test, feature = "evaluation")))]
            owner_future.await;
        });
        self.transcription_owners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(owner_handle);
        let response = match response_receiver.await {
            Ok(Ok(response)) => {
                cancel_guard.disarm();
                drop(cancel_guard);
                inference_exchange.finish("completed");
                response
            }
            Ok(Err(AsrOwnerError::RejectedResult {
                response,
                rejection,
            })) => {
                cancel_guard.disarm();
                drop(cancel_guard);
                let response = *response;
                let actual = LoadedModelIdentity::parse(
                    response.loaded_model_id.as_deref(),
                    response.loaded_model_version.as_deref(),
                    model_id,
                    &version,
                    LocalStage::Inference,
                )
                .expect("owner only returns typed rejection after identity admission");
                crate::observations::local_loaded_identity(
                    model_id,
                    &version,
                    actual.id,
                    actual.revision,
                );
                let result = response
                    .result
                    .as_ref()
                    .expect("owner only returns typed rejection with a result");
                crate::asr::observe_parsed_asr(
                    &result.text,
                    "local_model_output",
                    &result.completion,
                    response.loaded_model_id.as_deref(),
                );
                crate::observations::local_coverage(
                    model_id,
                    &version,
                    crate::observations::LocalCoverage {
                        expected_samples: audio_samples,
                        covered_samples: result.covered_samples,
                        chunks_completed: result.chunks_completed,
                        chunks_total: result.chunks_total,
                        generation_tokens: result.generation_tokens,
                        maximum_tokens: total_tokens,
                        completion: result.completion.clone(),
                        accepted: false,
                    },
                );
                inference_observation.failed(rejection.code());
                result.observe_rejection(&rejection);
                inference_exchange.finish("failed");
                inference_terminal
                    .finish_with_terminal_code("failed", Some(rejection.terminal_code()));
                return request_total.fail(
                    "Local transcription did not complete the full audio; the recording remains available for retry."
                        .into(),
                );
            }
            Ok(Err(AsrOwnerError::Exchange(error))) => {
                drop(cancel_guard);
                inference_observation.failed("local_inference_exchange_failed");
                inference_exchange.finish("failed");
                inference_terminal.finish_with_terminal_code(
                    "failed",
                    Some(LocalAsrTerminalCode::InferenceExchangeFailed),
                );
                return request_total.fail(error);
            }
            Err(_) => {
                drop(cancel_guard);
                inference_observation.failed("local_inference_exchange_cancelled");
                inference_exchange.finish("cancelled");
                inference_terminal.finish_with_terminal_code(
                    "cancelled",
                    Some(LocalAsrTerminalCode::InferenceExchangeFailed),
                );
                return request_total.fail("local transcription was cancelled".into());
            }
        };
        #[cfg(test)]
        let success_handoff_gate = self
            .fixture_success_handoff_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        #[cfg(test)]
        if let Some(gate) = success_handoff_gate {
            gate.hold_once().await;
        }
        let response_identity_matches = self.response_matches(&response_request, &response);
        let response_status_successful = response.status == "completed";
        if !response_identity_matches || !response_status_successful {
            inference_observation.failed("local_inference_response_identity_or_status_rejected");
            let terminal_code = if !response_identity_matches {
                LocalAsrTerminalCode::ResponseIdentityRejected
            } else {
                LocalAsrTerminalCode::ResponseStatusFailed
            };
            let message = response.error_message("local inference failed");
            self.update_status("runtime_failed", None, None, None, Some(message.clone()));
            inference_terminal.finish_with_terminal_code("failed", Some(terminal_code));
            return request_total.fail(message);
        }
        let actual = match LoadedModelIdentity::parse(
            response.loaded_model_id.as_deref(),
            response.loaded_model_version.as_deref(),
            model_id,
            &version,
            LocalStage::Inference,
        ) {
            Ok(actual) => actual,
            Err(reason) => {
                inference_observation.failed(reason.code());
                let error = reason.code().to_owned();
                self.update_status("runtime_failed", None, None, None, Some(error.clone()));
                inference_terminal.finish_with_terminal_code(
                    "failed",
                    Some(LocalAsrTerminalCode::LoadedModelIdentityRejected),
                );
                return request_total.fail(error);
            }
        };
        crate::observations::local_loaded_identity(model_id, &version, actual.id, actual.revision);
        let Some(result) = response.result else {
            inference_observation.failed("local_transcription_result_missing");
            let error = "local runtime omitted transcription result".to_owned();
            self.update_status("runtime_failed", None, None, None, Some(error.clone()));
            inference_terminal.finish_with_terminal_code(
                "failed",
                Some(LocalAsrTerminalCode::TranscriptionResultMissing),
            );
            return request_total.fail(error);
        };
        crate::asr::observe_parsed_asr(
            &result.text,
            "local_model_output",
            &result.completion,
            response.loaded_model_id.as_deref(),
        );
        let rejection = result.rejection(audio_samples, total_tokens);
        crate::observations::local_coverage(
            model_id,
            &version,
            crate::observations::LocalCoverage {
                expected_samples: audio_samples,
                covered_samples: result.covered_samples,
                chunks_completed: result.chunks_completed,
                chunks_total: result.chunks_total,
                generation_tokens: result.generation_tokens,
                maximum_tokens: total_tokens,
                completion: result.completion.clone(),
                accepted: rejection.is_none(),
            },
        );
        if let Some(rejection) = rejection {
            inference_observation.failed(rejection.code());
            result.observe_rejection(&rejection);
            let error =
                "Local transcription did not complete the full audio; the recording remains available for retry.";
            self.update_status("runtime_failed", None, None, None, Some(error.to_owned()));
            inference_terminal.finish_with_terminal_code("failed", Some(rejection.terminal_code()));
            return request_total.fail(error.into());
        }
        inference_terminal.finish("completed");
        #[cfg(test)]
        self.observe_fixture("inference");
        inference_observation.completed(None, response.runtime_version.as_deref());
        if let Some(span) = inference_span.as_mut() {
            span.finish();
        }
        crate::observations::asr_filter(&result.text, &result.text, "speech_text", None);
        kill_guard.disarm();
        request_total.finish("completed");
        Ok((result.text, result.language))
    }

    pub async fn preload(&self, model_id: &str) -> Result<(), String> {
        self.ensure_running()?;
        let manifest = mlx_model(model_id).ok_or_else(|| "unsupported local model".to_owned())?;
        if !self.runtime_platform_supported() {
            return Err(
                "Local MLX transcription requires Apple Silicon and macOS 14 or later.".into(),
            );
        }
        if !self.model_files_ready(model_id) {
            return Err("The selected local model files are missing or corrupt.".into());
        }
        let version = manifest.revision.to_owned();
        let directory = checked_model_directory(&self.models_root, model_id)?;
        let request_id = self.next_id();
        let session_id = request_id.clone();
        let diagnostics = LocalAsrDiagnostics::new(
            request_id.clone(),
            None,
            manifest.id,
            manifest.revision,
            LocalAsrOperation::Preload,
        );
        let mut request_total = diagnostics.stage("request_total");
        let mut files_diagnostic = diagnostics.deferred_stage("files_validated");
        let mut session_diagnostic = diagnostics.deferred_stage("sidecar_session");
        let mut handshake_diagnostic = diagnostics.deferred_stage("handshake");
        let mut model_load_diagnostic = diagnostics.deferred_stage("model_load");
        let mut queue_observation = diagnostics.stage("queue");
        if let Err(error) = self.join_transcription_owner().await {
            return request_total.fail(error);
        }
        let mut session = self.session.lock().await;
        queue_observation.finish("completed");
        let previous_status = self.snapshot();
        let loading_generation = match self.begin_model_load(&mut session, model_id, &version) {
            Ok(generation) => generation,
            Err(error) => {
                drop(session);
                return request_total.fail(error);
            }
        };
        let mut load_guard = loading_generation.map(|generation| {
            ModelLoadCancellationGuard::new(self, model_id, generation, previous_status)
        });
        files_diagnostic.start();
        if let Err(error) = self.verify_model_integrity(model_id).await {
            if let Some(guard) = load_guard.as_mut() {
                guard.finish_failure(&error);
            } else {
                self.finish_model_load_failure(model_id, loading_generation, &error);
            }
            files_diagnostic.finish("failed");
            return request_total.fail(error);
        }
        files_diagnostic.finish("completed");

        session_diagnostic.start();
        let session_spawned = match self
            .ensure_session(
                &mut session,
                load_guard.as_mut(),
                Some(&mut handshake_diagnostic),
                None,
            )
            .await
        {
            Ok(spawned) => spawned,
            Err(error) => {
                if let Some(guard) = load_guard.as_mut() {
                    guard.finish_failure(&error);
                } else {
                    self.finish_model_load_failure(model_id, loading_generation, &error);
                }
                session_diagnostic.finish(local_asr_error_outcome(&error));
                return request_total.fail(error);
            }
        };
        session_diagnostic.spawned_child(session_spawned);
        session_diagnostic.finish("completed");

        let pid = session
            .as_ref()
            .and_then(|child| child.child.id())
            .unwrap_or(0);
        if let Some(guard) = load_guard.as_mut() {
            guard.record_child_pid(pid);
        }
        let mut kill_guard = KillOnDrop::new(pid);
        let child = session.as_mut().expect("sidecar session is available");
        model_load_diagnostic.start();
        let model_loaded = match self
            .ensure_model_loaded(
                child,
                model_id,
                &version,
                &directory,
                &request_id,
                &session_id,
                loading_generation,
            )
            .await
        {
            Ok(disposition) => disposition,
            Err(error) => {
                self.kill_session(&mut session).await;
                if let Some(guard) = load_guard.as_mut() {
                    guard.finish_failure(&error);
                } else {
                    self.finish_model_load_failure(model_id, loading_generation, &error);
                }
                model_load_diagnostic.finish(local_asr_error_outcome(&error));
                return request_total.fail(error);
            }
        };
        if let Some(guard) = load_guard.as_mut() {
            guard.disarm();
        }
        model_load_diagnostic.finish(if model_loaded { "loaded" } else { "reused" });
        if let Err(error) = self.ensure_running() {
            drop(kill_guard);
            return request_total.fail(error);
        }
        kill_guard.disarm();
        self.schedule_idle_unload(model_id);
        request_total.finish("completed");
        Ok(())
    }

    fn observe_attestation(
        &self,
        model: &str,
        revision: &str,
        value: Option<crate::observations::LoadedAttestation>,
    ) {
        *self
            .loaded_attestation
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = value.clone();
        crate::observations::local_attestation(model, revision, value);
    }
    pub(crate) fn attestation(&self) -> Option<crate::observations::LoadedAttestation> {
        self.loaded_attestation
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    pub async fn unload(&self, model_id: Option<&str>) -> Result<(), String> {
        self.ensure_running()?;
        self.join_transcription_owner().await?;
        let mut session = self.session.lock().await;
        self.unload_locked(&mut session, model_id).await
    }

    async fn unload_if_idle(&self, expected_generation: u64, model_id: &str) -> Result<(), String> {
        self.join_transcription_owner().await?;
        let mut session = self.session.lock().await;
        if self.idle_generation.load(Ordering::Acquire) != expected_generation
            || self.snapshot().loaded_model_id.as_deref() != Some(model_id)
        {
            return Ok(());
        }
        self.unload_locked(&mut session, Some(model_id)).await
    }

    async fn unload_locked(
        &self,
        session: &mut Option<ChildSession>,
        model_id: Option<&str>,
    ) -> Result<(), String> {
        if session.is_none() {
            return Ok(());
        }
        let snapshot = self.snapshot();
        if let (Some(requested), Some(loaded)) = (model_id, snapshot.loaded_model_id.as_deref()) {
            if requested != loaded {
                return Ok(());
            }
        }
        if snapshot.loaded_model_id.is_none() {
            if snapshot.runtime_status == "stopping" && session.is_some() {
                self.kill_session(session).await;
                self.update_status("not_checked", None, None, None, None);
            }
            return Ok(());
        }
        let unload_id = model_id.or(snapshot.loaded_model_id.as_deref());
        let request = self.request(
            "unload",
            unload_id,
            snapshot.loaded_model_version.as_deref(),
        );
        let response = Self::exchange(
            session.as_mut().expect("checked sidecar session"),
            &request,
            Duration::from_secs(60),
        )
        .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                self.kill_session(session).await;
                self.update_status("runtime_failed", None, None, None, Some(error.clone()));
                return Err(error);
            }
        };
        if !self.response_matches(&request, &response) || !response.is_successful() {
            let error = response.error_message("local model unload failed");
            self.kill_session(session).await;
            self.update_status("runtime_failed", None, None, None, Some(error.clone()));
            return Err(error);
        }
        self.update_status("runtime_ready", response.runtime_version, None, None, None);
        Ok(())
    }

    pub(crate) async fn shutdown_with_status(&self) -> Result<(), &'static str> {
        self.begin_shutdown();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        match tokio::time::timeout_at(deadline, self.join_transcription_owner()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                log::warn!("local ASR cancellation owner could not be joined: {error}");
                return Err("local_owner_join_failed");
            }
            Err(_) => {
                log::warn!("timed out joining local ASR cancellation owner; preserving its Child and audio ownership");
                return Err("local_owner_join_unsettled");
            }
        }
        let Ok(mut session) = tokio::time::timeout_at(deadline, self.session.lock()).await else {
            log::warn!("timed out waiting for local runtime shutdown lock");
            return Err("local_shutdown_lock_unsettled");
        };
        let Some(child) = session.as_mut() else {
            drop(session);
            self.update_status("not_checked", None, None, None, None);
            return Ok(());
        };
        let _ = child.child.start_kill();
        let first_wait = tokio::time::timeout(Duration::from_secs(3), child.child.wait()).await;
        #[cfg(test)]
        let mut exit_observed = if self.fixture.is_some() {
            match &first_wait {
                Ok(Ok(status)) => {
                    self.fixture_reaped
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .push(Ok(*status));
                    true
                }
                Ok(Err(error)) => {
                    log::warn!("local ASR child wait failed during shutdown; retaining its session: {error}");
                    false
                }
                Err(_) => false,
            }
        } else {
            matches!(first_wait, Ok(Ok(_)))
        };
        #[cfg(not(test))]
        let mut exit_observed = matches!(first_wait, Ok(Ok(_)));
        if !exit_observed {
            let _ = child.child.start_kill();
            match tokio::time::timeout(Duration::from_secs(2), child.child.wait()).await {
                Ok(Ok(_status)) => {
                    exit_observed = true;
                    #[cfg(test)]
                    if self.fixture.is_some() {
                        self.fixture_reaped
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .push(Ok(_status));
                    }
                }
                Ok(Err(error)) => {
                    log::warn!("local ASR child wait failed during shutdown; retaining its session: {error}");
                }
                Err(_) => {
                    log::warn!("local ASR child exit remains unobserved after shutdown; retaining its session");
                }
            }
        }
        if !exit_observed {
            self.update_status("stopping", None, None, None, None);
            drop(session);
            return Err("local_child_exit_unobserved");
        }
        child.registration.clone().retire_after_exit();
        *session = None;
        self.active_pid.store(0, Ordering::Release);
        drop(session);
        self.update_status("not_checked", None, None, None, None);
        Ok(())
    }

    #[cfg(test)]
    fn assert_fixture_reaped_count(&self, expected: usize) {
        let receipts = self
            .fixture_reaped
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        assert_eq!(receipts.len(), expected, "fixture child reaped count");
        assert!(
            receipts.iter().all(Result::is_ok),
            "fixture child wait failed: {receipts:?}"
        );
    }

    /// Seal child creation before normal app exit and synchronously terminate
    /// an active sidecar so a request holding the async session lock can exit.
    pub fn begin_shutdown(&self) {
        let _creation = self
            .child_creation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }
        self.load_generation.fetch_add(1, Ordering::AcqRel);
        *self
            .loading_model_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.update_status("shutting_down", None, None, None, None);
        let pid = self.active_pid.load(Ordering::Acquire);
        kill_process(pid);
    }

    pub fn cancel_model_load(&self, model_id: &str) -> bool {
        let _creation = self
            .child_creation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.shutting_down.load(Ordering::Acquire) {
            return false;
        }
        let mut loading = self
            .loading_model_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if loading.as_deref() != Some(model_id) {
            return false;
        }
        self.load_generation.fetch_add(1, Ordering::AcqRel);
        *loading = None;
        drop(loading);
        let pid = self.active_pid.load(Ordering::Acquire);
        kill_process(pid);
        self.update_status("stopping", None, None, None, None);
        true
    }

    fn ensure_running(&self) -> Result<(), String> {
        if self.shutting_down.load(Ordering::Acquire) {
            Err("The local MLX runtime is shutting down.".into())
        } else {
            Ok(())
        }
    }

    fn begin_model_load(
        &self,
        session: &mut Option<ChildSession>,
        model_id: &str,
        model_version: &str,
    ) -> Result<Option<u64>, String> {
        self.ensure_running()?;
        let snapshot = self.snapshot();
        let live_session = session.as_mut().is_some_and(|session| {
            session
                .child
                .id()
                .is_some_and(|pid| self.active_pid.load(Ordering::Acquire) == pid)
                && session
                    .child
                    .try_wait()
                    .is_ok_and(|status| status.is_none())
        });
        if snapshot.runtime_status == "loaded"
            && snapshot.loaded_model_id.as_deref() == Some(model_id)
            && snapshot.loaded_model_version.as_deref() == Some(model_version)
            && live_session
        {
            return Ok(None);
        }
        let mut loading = self
            .loading_model_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.ensure_running()?;
        let generation = self.load_generation.fetch_add(1, Ordering::AcqRel) + 1;
        *loading = Some(model_id.to_owned());
        let mut snapshot = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        snapshot.runtime_status = "loading".into();
        snapshot.loading_model_id = Some(model_id.to_owned());
        snapshot.error = None;
        drop(snapshot);
        drop(loading);
        Ok(Some(generation))
    }

    fn load_is_current(&self, generation: u64) -> bool {
        !self.shutting_down.load(Ordering::Acquire)
            && self.load_generation.load(Ordering::Acquire) == generation
    }

    fn is_model_loading(&self) -> bool {
        self.loading_model_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

    fn clear_loading_model(&self, generation: Option<u64>) {
        let Some(generation) = generation else {
            return;
        };
        if self.load_generation.load(Ordering::Acquire) != generation {
            return;
        }
        *self
            .loading_model_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    fn finish_model_load_failure(&self, model_id: &str, generation: Option<u64>, error: &str) {
        if self.shutting_down.load(Ordering::Acquire)
            || generation.is_some_and(|generation| {
                !self.load_is_current(generation)
                    || self
                        .loading_model_id
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .as_deref()
                        != Some(model_id)
            })
        {
            return;
        }
        self.clear_loading_model(generation);
        self.update_status("runtime_failed", None, None, None, Some(error.to_owned()));
    }

    fn cancel_dropped_model_load(
        &self,
        model_id: &str,
        generation: u64,
        child_pid: Option<u32>,
        previous_status: &RuntimeSnapshot,
    ) {
        let mut loading = self
            .loading_model_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.shutting_down.load(Ordering::Acquire)
            || self.load_generation.load(Ordering::Acquire) != generation
            || loading.as_deref() != Some(model_id)
        {
            return;
        }
        *loading = None;

        if let Some(pid) = child_pid {
            // Dropping the load future can request termination, but only the
            // retained Child owner may clear its identity after wait observes exit.
            kill_process(pid);
        }

        let mut status = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if child_pid.is_some() {
            status.runtime_status = "stopping".to_owned();
            status.runtime_version = None;
            status.loaded_model_id = None;
            status.loaded_model_version = None;
            status.loading_model_id = None;
            status.error = None;
        } else {
            let mut restored = previous_status.clone();
            restored.loading_model_id = None;
            if restored.runtime_status == "loading" {
                restored.runtime_status = if restored.loaded_model_id.is_some() {
                    "loaded".to_owned()
                } else {
                    "not_checked".to_owned()
                };
            }
            *status = restored;
        }
    }

    fn request(&self, op: &str, model_id: Option<&str>, model_version: Option<&str>) -> Request {
        Request {
            protocol_version: PROTOCOL_VERSION,
            request_id: self.next_id(),
            session_id: None,
            model_version: model_version.map(str::to_owned),
            op: op.to_owned(),
            model_id: model_id.map(str::to_owned),
            model_directory: None,
            audio_path: None,
            language: None,
            audio_max_bytes: None,
            max_tokens_per_chunk: None,
            max_tokens_total: None,
        }
    }

    fn next_id(&self) -> String {
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("vf-{}-{seq}-{nanos}", std::process::id())
    }

    async fn ensure_session(
        &self,
        session: &mut Option<ChildSession>,
        load_guard: Option<&mut ModelLoadCancellationGuard<'_>>,
        mut handshake_diagnostic: Option<&mut LocalAsrStageTimer<LocalAsrStageEmitter>>,
        trace: Option<&crate::metrics::TraceHandle>,
    ) -> Result<bool, String> {
        let expected_load_generation = load_guard.as_ref().map(|guard| guard.generation);
        self.ensure_running()?;
        if expected_load_generation.is_some_and(|generation| !self.load_is_current(generation)) {
            return Err("Local model load was cancelled.".into());
        }
        if session.is_some() {
            let existing = session.as_mut().expect("session is present");
            let pid = existing.child.id().unwrap_or(0);
            if pid == 0 || self.active_pid.load(Ordering::Acquire) != pid {
                self.kill_session(session).await;
            } else {
                match existing.child.try_wait() {
                    Ok(None) => return Ok(false),
                    Ok(Some(_)) => self.kill_session(session).await,
                    Err(error) => {
                        self.kill_session(session).await;
                        return Err(error.to_string());
                    }
                }
            }
        }
        if !self.runtime_platform_supported() {
            return Err(
                "Local MLX transcription requires Apple Silicon and macOS 14 or later.".into(),
            );
        }
        // Resolve the native executable before filesystem setup, matching the
        // original fail-first startup ordering. Fixture commands are owned too.
        let mut command = self.worker_command()?;
        std::fs::create_dir_all(&self.models_root).map_err(|error| error.to_string())?;
        set_private_directory(&self.models_root)?;
        let audio_root = self.models_root.join("audio-tmp");
        std::fs::create_dir_all(&audio_root).map_err(|error| error.to_string())?;
        let audio_metadata =
            std::fs::symlink_metadata(&audio_root).map_err(|error| error.to_string())?;
        if audio_metadata.file_type().is_symlink() || !audio_metadata.is_dir() {
            return Err("private audio storage is unavailable".into());
        }
        set_private_directory(&audio_root)?;
        cleanup_private_audio_files(&audio_root);
        let creation = self
            .child_creation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.ensure_running()?;
        if expected_load_generation.is_some_and(|generation| !self.load_is_current(generation)) {
            return Err("Local model load was cancelled.".into());
        }
        let _startup_span = trace.map(|trace| trace.begin(crate::metrics::Stage::RuntimeStartup));
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("Could not start the local MLX runtime: {error}"))?;
        let pid = child.id().unwrap_or(0);
        if let Some(guard) = load_guard {
            guard.record_child_pid(pid);
        }
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "local runtime stdin is unavailable".to_owned())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "local runtime stdout is unavailable".to_owned())?;
        let registration = crate::model_resources::ChildRegistration::register(
            pid,
            self.load_generation.load(Ordering::Acquire),
            &self.models_root,
        )
        .ok_or_else(|| "local runtime process identity is unavailable".to_owned())?;
        *session = Some(ChildSession {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            registration,
        });
        self.active_pid.store(pid, Ordering::Release);
        drop(creation);
        let request = self.request("hello", None, None);
        let requested = self
            .loading_model_id
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let manifest = requested.as_deref().and_then(mlx_model);
        let mut hello_observation = manifest
            .map(|m| LocalStageGuard::start(m.id, m.revision, LocalStage::RuntimeHandshake));
        if let Some(diagnostic) = handshake_diagnostic.as_deref_mut() {
            diagnostic.start();
        }
        let response = match Self::exchange(
            session.as_mut().expect("new sidecar session is available"),
            &request,
            Duration::from_secs(8),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => {
                if let Some(observation) = &mut hello_observation {
                    observation.failed("local_hello_exchange_failed");
                }
                self.kill_session(session).await;
                if let Some(observation) = handshake_diagnostic.as_deref_mut() {
                    observation.finish(local_asr_error_outcome(&error));
                }
                return Err(error);
            }
        };
        if !self.response_matches(&request, &response)
            || !response.is_successful()
            || response.platform_supported != Some(true)
        {
            if let Some(observation) = &mut hello_observation {
                observation.failed("local_hello_identity_or_capability_rejected");
            }
            let message = response.error_message("local MLX runtime is unsupported or unavailable");
            self.kill_session(session).await;
            if let Some(observation) = handshake_diagnostic.as_deref_mut() {
                observation.finish("failed");
            }
            return Err(message);
        }
        if let Some(observation) = &mut hello_observation {
            observation.completed(None, response.runtime_version.as_deref());
        }
        if let Some(observation) = handshake_diagnostic {
            observation.finish("completed");
        }
        if self.is_model_loading() {
            self.update_status("loading", response.runtime_version, None, None, None);
        } else {
            self.update_status("runtime_ready", response.runtime_version, None, None, None);
        }
        Ok(true)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "Keep model identity, protocol session, and cancellable load generation explicit."
    )]
    async fn ensure_model_loaded(
        &self,
        child: &mut ChildSession,
        model_id: &str,
        model_version: &str,
        directory: &Path,
        request_id: &str,
        session_id: &str,
        loading_generation: Option<u64>,
    ) -> Result<bool, String> {
        self.ensure_running()?;
        let mut load_observation =
            LocalStageGuard::start(model_id, model_version, LocalStage::ModelLoad);
        let snapshot = self.snapshot();
        if snapshot.loaded_model_id.as_deref() == Some(model_id)
            && snapshot.loaded_model_version.as_deref() == Some(model_version)
        {
            let actual = LoadedModelIdentity::parse(
                snapshot.loaded_model_id.as_deref(),
                snapshot.loaded_model_version.as_deref(),
                model_id,
                model_version,
                LocalStage::ModelLoad,
            )
            .map_err(|e| e.code().to_owned())?;
            crate::observations::local_loaded_identity(
                model_id,
                model_version,
                actual.id,
                actual.revision,
            );
            crate::observations::local_attestation(model_id, model_version, self.attestation());
            load_observation.completed(
                Some("reused_previously_observed_loaded_identity"),
                snapshot.runtime_version.as_deref(),
            );
            return Ok(false);
        }
        if loading_generation.is_some_and(|generation| !self.load_is_current(generation)) {
            return Err("Local model load was cancelled.".into());
        }
        if snapshot.loaded_model_id.is_some() {
            let unload = Request {
                protocol_version: PROTOCOL_VERSION,
                request_id: self.next_id(),
                session_id: None,
                model_version: snapshot.loaded_model_version,
                op: "unload".into(),
                model_id: snapshot.loaded_model_id,
                model_directory: None,
                audio_path: None,
                language: None,
                audio_max_bytes: None,
                max_tokens_per_chunk: None,
                max_tokens_total: None,
            };
            let response = Self::exchange(child, &unload, Duration::from_secs(60)).await?;
            if !self.response_matches(&unload, &response) || !response.is_successful() {
                return Err(response.error_message("could not release the previous local model"));
            }
        }
        if loading_generation.is_some_and(|generation| !self.load_is_current(generation)) {
            return Err("Local model load was cancelled.".into());
        }
        let canonical = directory
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let request = Request {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.to_owned(),
            session_id: Some(session_id.to_owned()),
            model_version: Some(model_version.to_owned()),
            op: "load".into(),
            model_id: Some(model_id.to_owned()),
            model_directory: Some(canonical.to_string_lossy().into_owned()),
            audio_path: None,
            language: None,
            audio_max_bytes: None,
            max_tokens_per_chunk: None,
            max_tokens_total: None,
        };
        let reservation = child
            .registration
            .admit_cold_load(
                model_id,
                model_version,
                crate::model_resources::LoadKind::Asr,
            )
            .map_err(|_| "本机内存预算暂不可用，无法加载本地语音识别模型。".to_owned())?;
        let exchange_result = Self::exchange(child, &request, Duration::from_secs(300)).await;
        let response = match exchange_result {
            Ok(response) => response,
            Err(error) => {
                load_observation.failed("local_load_exchange_failed");
                return Err(error);
            }
        };
        Self::settle_asr_load_reservation_if_terminal(
            reservation,
            &request,
            &response,
            model_id,
            model_version,
        );
        if !Self::asr_load_response_is_successful(&request, &response, model_id, model_version) {
            load_observation.failed("local_load_response_identity_or_status_rejected");
            return Err(response.error_message("local model could not be loaded"));
        }
        if loading_generation.is_some_and(|generation| !self.load_is_current(generation)) {
            return Err("Local model load was cancelled.".into());
        }
        let actual = LoadedModelIdentity::parse(
            response.loaded_model_id.as_deref(),
            response.loaded_model_version.as_deref(),
            model_id,
            model_version,
            LocalStage::ModelLoad,
        )
        .map_err(|e| {
            load_observation.failed(e.code());
            e.code().to_owned()
        })?;
        crate::observations::local_loaded_identity(
            model_id,
            model_version,
            actual.id,
            actual.revision,
        );
        self.observe_attestation(model_id, model_version, response.loaded_attestation.clone());
        load_observation.completed(None, response.runtime_version.as_deref());
        self.clear_loading_model(loading_generation);
        self.update_status(
            "loaded",
            response.runtime_version,
            Some(actual.id.to_owned()),
            Some(actual.revision.to_owned()),
            None,
        );
        Ok(true)
    }

    async fn exchange(
        child: &mut ChildSession,
        request: &Request,
        deadline: Duration,
    ) -> Result<Response, String> {
        tokio::time::timeout(deadline, async {
            Self::write_sidecar_request(&mut child.stdin, request).await?;
            Self::read_sidecar_response_line(&mut child.stdout).await
        })
        .await
        .map_err(|_| "local runtime request timed out".to_owned())?
        .map_err(|error| error.to_string())
    }

    async fn write_sidecar_request(stdin: &mut ChildStdin, request: &Request) -> io::Result<()> {
        let mut encoded = serde_json::to_vec(request)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        encoded.push(b'\n');
        stdin.write_all(&encoded).await?;
        stdin.flush().await
    }

    async fn exchange_transcription_with_cancel(
        child: &mut ChildSession,
        request: &Request,
        cancel_request: &Request,
        cancel: &AsrCancellationSignal,
        deadline: Duration,
    ) -> Result<AsrExchangeOutcome, String> {
        if cancel.token.is_cancelled() {
            return Ok(AsrExchangeOutcome::Cancelled);
        }

        // Keep the write future alive if the caller disappears mid-frame. A
        // cancel frame may only follow a complete original request line.
        let initial_deadline = tokio::time::Instant::now() + deadline;
        let write_cancel_deadline = {
            let write = Self::write_sidecar_request(&mut child.stdin, request);
            tokio::pin!(write);
            tokio::select! {
                biased;
                result = tokio::time::timeout_at(initial_deadline, &mut write) => {
                    result.map_err(|_| "local runtime request timed out".to_owned())?
                        .map_err(|error| error.to_string())?;
                    None
                }
                _ = cancel.token.cancelled() => {
                    let cancel_deadline = cancel.deadline();
                    tokio::time::timeout_at(cancel_deadline, &mut write)
                        .await
                        .map_err(|_| "local ASR cancellation expired during request write".to_owned())?
                        .map_err(|error| error.to_string())?;
                    Some(cancel_deadline)
                }
            }
        };

        let (terminal, cancel_deadline) = {
            let read = Self::read_sidecar_response_line(&mut child.stdout);
            tokio::pin!(read);
            let cancel_deadline = if let Some(cancel_deadline) = write_cancel_deadline {
                cancel_deadline
            } else {
                tokio::select! {
                    biased;
                    result = tokio::time::timeout_at(initial_deadline, &mut read) => {
                        return result
                            .map_err(|_| "local runtime request timed out".to_owned())?
                            .map(|response| AsrExchangeOutcome::Completed(Box::new(response)))
                            .map_err(|error| error.to_string());
                    }
                    _ = cancel.token.cancelled() => cancel.deadline(),
                }
            };

            // Keep `read` alive: it owns any partial JSON frame already
            // consumed from BufReader while the cancel line is written.
            tokio::time::timeout_at(
                cancel_deadline,
                Self::write_sidecar_request(&mut child.stdin, cancel_request),
            )
            .await
            .map_err(|_| "local ASR cancellation expired during cancel write".to_owned())?
            .map_err(|error| error.to_string())?;
            let terminal = tokio::time::timeout_at(cancel_deadline, &mut read)
                .await
                .map_err(|_| "local ASR cancellation expired before terminal".to_owned())?
                .map_err(|error| error.to_string())?;
            (terminal, cancel_deadline)
        };
        let acknowledgement = tokio::time::timeout_at(
            cancel_deadline,
            Self::read_sidecar_response_line(&mut child.stdout),
        )
        .await
        .map_err(|_| "local ASR cancellation expired before acknowledgement".to_owned())?
        .map_err(|error| error.to_string())?;
        if !Self::cancel_pair_matches(request, cancel_request, &terminal, &acknowledgement) {
            return Err("local ASR cancellation response identity or status was rejected".into());
        }
        Ok(AsrExchangeOutcome::Cancelled)
    }

    async fn drive_transcription(
        session: &mut OwnedAsrSession,
        request: &Request,
        cancel_request: &Request,
        cancel: &AsrCancellationSignal,
        deadline: Duration,
    ) -> Result<AsrExchangeOutcome, String> {
        let child = session.as_mut().ok_or_else(|| {
            "local ASR session disappeared before transcription dispatch".to_owned()
        })?;
        Self::exchange_transcription_with_cancel(child, request, cancel_request, cancel, deadline)
            .await
    }

    fn cancel_pair_matches(
        request: &Request,
        cancel_request: &Request,
        terminal: &Response,
        acknowledgement: &Response,
    ) -> bool {
        let loaded_identity_matches = |response: &Response| {
            response.loaded_model_id.as_deref() == request.model_id.as_deref()
                && response.loaded_model_version.as_deref() == request.model_version.as_deref()
        };
        Self::request_matches(request, terminal)
            && terminal.status == "cancelled"
            && terminal.result.is_none()
            && terminal.cleanup_result.is_none()
            && terminal.warmup_result.is_none()
            && terminal.error.is_none()
            && terminal.platform_supported == Some(true)
            && loaded_identity_matches(terminal)
            && Self::request_matches(cancel_request, acknowledgement)
            && acknowledgement.status == "cancelled"
            && acknowledgement.result.is_none()
            && acknowledgement.cleanup_result.is_none()
            && acknowledgement.warmup_result.is_none()
            && acknowledgement.error.is_none()
            && acknowledgement.platform_supported == Some(true)
            && loaded_identity_matches(acknowledgement)
    }

    fn request_matches(request: &Request, response: &Response) -> bool {
        response.protocol_version == PROTOCOL_VERSION
            && response.request_id == request.request_id
            && response.session_id == request.session_id
            && response.model_version == request.model_version
            && response.op == request.op
    }

    fn asr_load_response_is_successful(
        request: &Request,
        response: &Response,
        model_id: &str,
        model_version: &str,
    ) -> bool {
        Self::request_matches(request, response)
            && response.status == "loaded"
            && response.loaded_model_id.as_deref() == Some(model_id)
            && response.loaded_model_version.as_deref() == Some(model_version)
    }

    fn settle_asr_load_reservation_if_terminal(
        reservation: crate::model_resources::PendingLoadReservation,
        request: &Request,
        response: &Response,
        model_id: &str,
        model_version: &str,
    ) -> bool {
        let terminal = Self::request_matches(request, response)
            && (response.status == "error"
                || Self::asr_load_response_is_successful(
                    request,
                    response,
                    model_id,
                    model_version,
                ));
        if terminal {
            reservation.settle();
        }
        terminal
    }

    async fn read_sidecar_response_line<R>(stdout: &mut R) -> io::Result<Response>
    where
        R: AsyncBufRead + Unpin,
    {
        let mut line = Vec::new();
        let mut bytes = 0usize;
        loop {
            let available = stdout.fill_buf().await?;
            if available.is_empty() {
                break;
            }
            let line_end = available.iter().position(|byte| *byte == b'\n');
            let take = line_end.map_or(available.len(), |end| end + 1);
            if bytes.saturating_add(take) > MAX_RESPONSE_LINE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "sidecar response is too large",
                ));
            }
            line.extend_from_slice(&available[..take]);
            stdout.consume(take);
            bytes += take;
            if line_end.is_some() {
                break;
            }
        }
        if bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "sidecar exited",
            ));
        }
        if line.len() > MAX_RESPONSE_LINE_BYTES || !line.ends_with(b"\n") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "sidecar response is too large",
            ));
        }
        serde_json::from_slice::<Response>(&line)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid sidecar response"))
    }

    fn response_matches(&self, request: &Request, response: &Response) -> bool {
        response.protocol_version == PROTOCOL_VERSION
            && response.request_id == request.request_id
            && response.session_id == request.session_id
            && response.model_version == request.model_version
            && response.op == request.op
    }

    async fn verify_model_integrity(&self, model_id: &str) -> Result<(), String> {
        let root = self.models_root.clone();
        let id = model_id.to_owned();
        let expected = self.expected_model_files(model_id)?;
        let hashed_model_bytes = Arc::clone(&self.hashed_model_bytes);
        let cached = self
            .verified_files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(model_id)
            .cloned();
        let identities = tokio::task::spawn_blocking(move || {
            verify_pinned_model_files(
                &root,
                &id,
                &expected,
                cached.as_deref(),
                None,
                &hashed_model_bytes,
            )
        })
        .await
        .map_err(|_| "model integrity verification was interrupted".to_owned())??;
        let mut verified = self
            .verified_files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        verified.insert(model_id.to_owned(), identities);
        Ok(())
    }

    #[cfg(test)]
    fn hashed_model_bytes(&self) -> u64 {
        self.hashed_model_bytes.load(Ordering::Relaxed)
    }

    async fn kill_session(&self, session: &mut Option<ChildSession>) {
        let Some(owned) = session.as_mut() else {
            return;
        };
        let pid = owned.child.id().unwrap_or_default();
        let _ = owned.child.start_kill();
        let _exit_status = loop {
            match owned.child.wait().await {
                Ok(status) => break status,
                Err(error) => {
                    log::warn!(
                        "local ASR child exit remains unobserved; retaining its session: {error}"
                    );
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        };
        owned.registration.clone().retire_after_exit();
        #[cfg(test)]
        if self.fixture.is_some() {
            self.fixture_reaped
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(Ok(_exit_status));
        }
        if pid != 0 {
            let _ = self
                .active_pid
                .compare_exchange(pid, 0, Ordering::AcqRel, Ordering::Acquire);
        }
        *session = None;
    }

    fn asr_owner_context(&self) -> AsrOwnerContext {
        AsrOwnerContext {
            active_pid: Arc::clone(&self.active_pid),
            status: Arc::clone(&self.status),
            loaded_attestation: Arc::clone(&self.loaded_attestation),
            models_root: self.models_root.clone(),
            #[cfg(test)]
            fixture: self.fixture.is_some(),
            #[cfg(test)]
            fixture_reaped: Arc::clone(&self.fixture_reaped),
            #[cfg(test)]
            fixture_observer: self
                .fixture
                .as_ref()
                .map(|fixture| Arc::clone(&fixture.observer)),
            #[cfg(test)]
            fixture_retirement_gate: self
                .fixture_retirement_gate
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        }
    }

    async fn join_transcription_owner(&self) -> Result<(), String> {
        let task = self
            .transcription_owners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain(..)
            .collect::<Vec<_>>();
        if task.is_empty() {
            return Ok(());
        }
        let mut handoff = RuntimeTaskHandoff {
            slot: &self.transcription_owners,
            tasks: task,
        };
        handoff
            .join()
            .await
            .map_err(|error| format!("local ASR cancellation owner failed: {error}"))
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn update_status(
        &self,
        runtime_status: &str,
        runtime_version: Option<String>,
        loaded_model_id: Option<String>,
        loaded_model_version: Option<String>,
        error: Option<String>,
    ) -> RuntimeSnapshot {
        let loading_model_id = self
            .loading_model_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if loaded_model_id.is_none() {
            *self
                .loaded_attestation
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = None;
        }
        let snapshot = RuntimeSnapshot {
            platform_supported: self.runtime_platform_supported(),
            runtime_status: if self.shutting_down.load(Ordering::Acquire) {
                "shutting_down".to_owned()
            } else if loading_model_id.is_some() && runtime_status == "runtime_ready" {
                "loading".to_owned()
            } else {
                runtime_status.to_owned()
            },
            runtime_version,
            loaded_model_id,
            loaded_model_version,
            loading_model_id,
            error: error.map(|message| sanitize_runtime_error(&message)),
        };
        *self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshot.clone();
        snapshot
    }

    fn schedule_idle_unload(&self, model_id: &str) {
        #[cfg(test)]
        if self.fixture.is_some() {
            return;
        }
        schedule_idle_unload(&self.models_root, model_id);
    }
}

impl Drop for OnDeviceRuntime {
    fn drop(&mut self) {
        if let Some(task) = self
            .idle_task
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            task.abort();
        }
        let pid = self.active_pid.load(Ordering::Acquire);
        kill_process(pid);
    }
}

pub fn shared_runtime(models_root: &Path) -> Arc<OnDeviceRuntime> {
    static RUNTIMES: OnceLock<Mutex<HashMap<PathBuf, Arc<OnDeviceRuntime>>>> = OnceLock::new();
    let runtimes = RUNTIMES.get_or_init(|| Mutex::new(HashMap::new()));
    let key = models_root.to_path_buf();
    let mut map = runtimes
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    map.entry(key.clone())
        .or_insert_with(|| Arc::new(OnDeviceRuntime::new(key)))
        .clone()
}

fn schedule_idle_unload(models_root: &Path, model_id: &str) {
    let runtime = shared_runtime(models_root);
    let generation = runtime.idle_generation.fetch_add(1, Ordering::AcqRel) + 1;
    let model_id = model_id.to_owned();
    let task_runtime = runtime.clone();
    let task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(MODEL_IDLE_UNLOAD_SECS)).await;
        if let Err(error) = task_runtime.unload_if_idle(generation, &model_id).await {
            log::debug!("idle local model unload failed: {error}");
        }
    });
    let previous = {
        runtime
            .idle_task
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .replace(task)
    };
    if let Some(previous) = previous {
        previous.abort();
    }
}

pub fn sidecar_executable() -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    #[cfg(any(debug_assertions, feature = "evaluation", feature = "research-harness"))]
    let developer_override = std::env::var_os("VOICEFLOW_MLX_SIDECAR_PATH").map(PathBuf::from);
    #[cfg(not(any(debug_assertions, feature = "evaluation", feature = "research-harness")))]
    let developer_override: Option<PathBuf> = None;

    resolve_sidecar_executable(
        &current,
        developer_override.as_deref(),
        cfg!(any(
            debug_assertions,
            feature = "evaluation",
            feature = "research-harness"
        )),
    )
}

fn resolve_sidecar_executable(
    current_exe: &Path,
    developer_override: Option<&Path>,
    allow_developer_paths: bool,
) -> Option<PathBuf> {
    if allow_developer_paths {
        if let Some(path) = developer_override {
            // Keep the explicit override for debug and opt-in research/evaluation
            // builds. Formal builds never read or resolve this environment value.
            return executable_file(path).then(|| path.to_path_buf());
        }

        if let Some(parent) = current_exe.parent() {
            let candidates = [
                parent.join("voiceflow-mlx-sidecar"),
                parent.join("../Resources/voiceflow-mlx-sidecar"),
                parent.join("../Helpers/voiceflow-mlx-sidecar"),
                parent.join("Resources/voiceflow-mlx-sidecar"),
            ];
            if let Some(path) = candidates
                .into_iter()
                .find(|path| executable_file(path))
                .and_then(|path| path.canonicalize().ok())
            {
                return Some(path);
            }
        }
    }

    packaged_bundle_sidecar(current_exe)
}

fn packaged_bundle_sidecar(current_exe: &Path) -> Option<PathBuf> {
    let executable = current_exe.canonicalize().ok()?;
    let macos_dir = executable.parent()?;
    if macos_dir.file_name()? != "MacOS" {
        return None;
    }
    let contents_dir = macos_dir.parent()?;
    if contents_dir.file_name()? != "Contents" {
        return None;
    }
    if contents_dir.parent()?.extension()? != "app" {
        return None;
    }

    let expected_resources = contents_dir.join("Resources");
    let resources_dir = expected_resources.canonicalize().ok()?;
    if resources_dir != expected_resources
        || resources_dir.parent() != Some(contents_dir)
        || resources_dir.file_name()? != "Resources"
    {
        return None;
    }

    let sidecar = resources_dir.join("voiceflow-mlx-sidecar");
    if !executable_file(&sidecar) {
        return None;
    }
    let sidecar = sidecar.canonicalize().ok()?;
    if sidecar.parent() != Some(resources_dir.as_path())
        || sidecar.file_name()? != "voiceflow-mlx-sidecar"
    {
        return None;
    }
    Some(sidecar)
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn checked_model_directory(models_root: &Path, model_id: &str) -> Result<PathBuf, String> {
    let root = models_root
        .canonicalize()
        .map_err(|error| format!("model cache is unavailable: {error}"))?;
    let model = model_dir(models_root, model_id)
        .canonicalize()
        .map_err(|error| format!("model directory is unavailable: {error}"))?;
    if !model.starts_with(&root) || model.parent() != Some(root.as_path()) {
        return Err("model directory escaped the VoiceFlow model cache".into());
    }
    Ok(model)
}

pub(crate) fn file_request_facts(
    model_id: &str,
    language: Option<&str>,
    audio_samples: u64,
) -> crate::observations::ResolvedRequest {
    let mut facts = crate::observations::ResolvedRequest::new(
        "local_sidecar",
        "local_runtime",
        model_id,
        model_id,
    );
    if let Some(language) = language {
        facts
            .fields_constructed
            .insert("language".into(), serde_json::json!(language));
    }
    let version = mlx_model(model_id).map(|m| m.revision);
    facts
        .fields_constructed
        .extend(std::collections::BTreeMap::from([
            ("model_id".into(), serde_json::json!(model_id)),
            ("model_version".into(), serde_json::json!(version)),
            ("audio_max_bytes".into(), serde_json::json!(MAX_AUDIO_BYTES)),
            ("op".into(), serde_json::json!("transcribe")),
            ("max_tokens_per_chunk".into(), serde_json::json!(4096)),
            (
                "max_tokens_total".into(),
                serde_json::json!((audio_samples.div_ceil(16000) as usize)
                    .saturating_mul(128)
                    .clamp(4096, 131072)),
            ),
            (
                "protocol_version".into(),
                serde_json::json!(PROTOCOL_VERSION),
            ),
        ]));
    facts
}

fn validate_wav(bytes: &[u8]) -> Result<u64, String> {
    if bytes.len() > MAX_AUDIO_BYTES {
        return Err("local ASR audio exceeds the supported size limit".into());
    }
    let mut reader = hound::WavReader::new(std::io::Cursor::new(bytes))
        .map_err(|_| "local ASR requires a complete PCM WAV recording".to_owned())?;
    let spec = reader.spec();
    if spec.sample_rate != 16_000
        || spec.channels != 1
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
    {
        return Err("local ASR requires mono 16 kHz 16-bit PCM WAV audio".into());
    }
    let duration = reader.duration() as u64;
    if duration == 0 || duration > MAX_AUDIO_DURATION_SECS * 16_000 {
        return Err("local ASR audio is empty or exceeds the 15-minute limit".into());
    }
    for sample in reader.samples::<i16>() {
        sample.map_err(|_| "local ASR audio WAV is incomplete or corrupt".to_owned())?;
    }
    Ok(duration)
}

fn asr_proof_binding<'a>(
    model_id: &'a str,
    expected: &'a [(String, u64, String)],
) -> Option<AsrProofBinding<'a>> {
    let manifest = mlx_model(model_id)?;
    let pinned_files = manifest
        .files
        .iter()
        .map(|file| (file.path, file.bytes, file.sha256))
        .collect::<Vec<_>>();
    let language_policy = match manifest.language_policy {
        crate::ondevice_models::MlxLanguagePolicy::AutoOrExplicit => "auto_or_explicit",
        crate::ondevice_models::MlxLanguagePolicy::ExplicitChineseOrEnglish => {
            "explicit_chinese_or_english"
        }
    };
    let material = serde_json::to_vec(&(
        manifest.id,
        manifest.label,
        manifest.repository,
        manifest.revision,
        manifest.bytes,
        language_policy,
        pinned_files,
        expected,
    ))
    .ok()?;
    Some(AsrProofBinding {
        model_id,
        repository: manifest.repository,
        revision: manifest.revision,
        manifest_sha256: hex::encode(Sha256::digest(material)),
        expected,
    })
}

fn read_valid_installed_manifest(
    directory: &Path,
    binding: &AsrProofBinding<'_>,
) -> Option<(Vec<u8>, crate::modelstore::DurableFileIdentity)> {
    let path = directory.join(ASR_INSTALLED_MANIFEST);
    let before = crate::modelstore::durable_file_identity(&path).ok()?;
    let identity = std::fs::symlink_metadata(&path).ok()?;
    if identity.file_type().is_symlink()
        || !identity.is_file()
        || identity.len() > ASR_DURABLE_PROOF_MAX_BYTES
    {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    if bytes.len() as u64 > ASR_DURABLE_PROOF_MAX_BYTES {
        return None;
    }
    let manifest: AsrInstalledManifest = serde_json::from_slice(&bytes).ok()?;
    if manifest.schema_version != 1
        || manifest.id != binding.model_id
        || manifest.revision != binding.revision
        || manifest.files.len() != binding.expected.len()
        || manifest
            .files
            .iter()
            .zip(binding.expected)
            .any(|(actual, (path, count, sha))| {
                actual.path != *path || actual.bytes != *count || actual.sha256 != *sha
            })
    {
        return None;
    }
    let after = crate::modelstore::durable_file_identity(&path).ok()?;
    (after == before).then_some((bytes, before))
}

fn capture_asr_durable_snapshot(
    models_root: &Path,
    directory: &Path,
    binding: &AsrProofBinding<'_>,
) -> Option<AsrDurableSnapshot> {
    let root = models_root.canonicalize().ok()?;
    let canonical_directory = directory.canonicalize().ok()?;
    if !canonical_directory.starts_with(&root)
        || canonical_directory.parent() != Some(root.as_path())
    {
        return None;
    }
    let models_root_identity = crate::modelstore::durable_file_identity(&root).ok()?;
    let model_directory_identity =
        crate::modelstore::durable_file_identity(&canonical_directory).ok()?;
    let (installed_manifest_bytes, installed_manifest_identity) =
        read_valid_installed_manifest(&canonical_directory, binding)?;
    let mut files = Vec::with_capacity(binding.expected.len());
    let mut runtime_files = Vec::with_capacity(binding.expected.len());
    for (path, bytes, sha256) in binding.expected {
        let file_path = canonical_directory.join(path);
        let metadata = std::fs::symlink_metadata(&file_path).ok()?;
        if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != *bytes {
            return None;
        }
        let runtime_identity = file_identity(path, &metadata);
        let identity = crate::modelstore::durable_file_identity(&file_path).ok()?;
        files.push((path.clone(), *bytes, sha256.clone(), identity));
        runtime_files.push(runtime_identity);
    }
    Some(AsrDurableSnapshot {
        models_root: root.to_string_lossy().into_owned(),
        models_root_identity,
        model_directory: canonical_directory.to_string_lossy().into_owned(),
        model_directory_identity,
        installed_manifest_bytes,
        installed_manifest_identity,
        files,
        runtime_files,
    })
}

fn asr_proof_matches_snapshot(
    proof: &AsrDurableProof,
    binding: &AsrProofBinding<'_>,
    snapshot: &AsrDurableSnapshot,
) -> bool {
    proof.schema_version == ASR_DURABLE_PROOF_SCHEMA
        && proof.model_id == binding.model_id
        && proof.repository == binding.repository
        && proof.revision == binding.revision
        && proof.manifest_sha256 == binding.manifest_sha256
        && proof.models_root == snapshot.models_root
        && proof.models_root_identity == snapshot.models_root_identity
        && proof.model_directory == snapshot.model_directory
        && proof.model_directory_identity == snapshot.model_directory_identity
        && proof.installed_manifest_sha256
            == hex::encode(Sha256::digest(&snapshot.installed_manifest_bytes))
        && proof.installed_manifest_identity == snapshot.installed_manifest_identity
        && proof.files.len() == binding.expected.len()
        && proof.files.iter().zip(&snapshot.files).all(
            |(proof_file, (path, bytes, sha256, identity))| {
                proof_file.path == *path
                    && proof_file.bytes == *bytes
                    && proof_file.sha256 == *sha256
                    && proof_file.identity == *identity
            },
        )
}

fn asr_proof_cache_hit(
    models_root: &Path,
    directory: &Path,
    binding: &AsrProofBinding<'_>,
    current_files: &[FileIdentity],
) -> bool {
    let path = crate::modelstore::local_model_proof_path(models_root, binding.model_id);
    let Some(bytes) =
        crate::modelstore::read_private_proof_record(&path, ASR_DURABLE_PROOF_MAX_BYTES)
    else {
        return false;
    };
    let Ok(proof) = serde_json::from_slice::<AsrDurableProof>(&bytes) else {
        return false;
    };
    let Some(before) = capture_asr_durable_snapshot(models_root, directory, binding) else {
        return false;
    };
    if before.runtime_files != current_files
        || !asr_proof_matches_snapshot(&proof, binding, &before)
    {
        return false;
    }
    let Some(after) = capture_asr_durable_snapshot(models_root, directory, binding) else {
        return false;
    };
    before == after
}

fn publish_asr_durable_proof(
    models_root: &Path,
    directory: &Path,
    binding: &AsrProofBinding<'_>,
    verified_files: &[FileIdentity],
    validated_snapshot: &AsrDurableSnapshot,
) -> Result<(), AsrProofPublishError> {
    publish_asr_durable_proof_with_writer(
        models_root,
        directory,
        binding,
        verified_files,
        validated_snapshot,
        crate::modelstore::write_private_proof_record,
    )
}

fn publish_asr_durable_proof_with_writer(
    models_root: &Path,
    directory: &Path,
    binding: &AsrProofBinding<'_>,
    verified_files: &[FileIdentity],
    validated_snapshot: &AsrDurableSnapshot,
    write_record: impl FnOnce(&Path, &[u8]) -> Result<(), String>,
) -> Result<(), AsrProofPublishError> {
    let snapshot =
        capture_asr_durable_snapshot(models_root, directory, binding).ok_or_else(|| {
            AsrProofPublishError::Integrity(
                "the local model identity changed before proof publication".into(),
            )
        })?;
    if &snapshot != validated_snapshot || snapshot.runtime_files != verified_files {
        return Err(AsrProofPublishError::Integrity(
            "the local model identity changed after integrity verification".into(),
        ));
    }
    let proof = AsrDurableProof {
        schema_version: ASR_DURABLE_PROOF_SCHEMA,
        model_id: binding.model_id.to_owned(),
        repository: binding.repository.to_owned(),
        revision: binding.revision.to_owned(),
        manifest_sha256: binding.manifest_sha256.clone(),
        models_root: snapshot.models_root.clone(),
        models_root_identity: snapshot.models_root_identity.clone(),
        model_directory: snapshot.model_directory.clone(),
        model_directory_identity: snapshot.model_directory_identity.clone(),
        installed_manifest_sha256: hex::encode(Sha256::digest(&snapshot.installed_manifest_bytes)),
        installed_manifest_identity: snapshot.installed_manifest_identity.clone(),
        files: snapshot
            .files
            .iter()
            .map(|(path, bytes, sha256, identity)| AsrDurableProofFile {
                path: path.clone(),
                bytes: *bytes,
                sha256: sha256.clone(),
                identity: identity.clone(),
            })
            .collect(),
    };
    let serialized = serde_json::to_vec(&proof)
        .map_err(|error| AsrProofPublishError::Cache(error.to_string()))?;
    let proof_path = crate::modelstore::local_model_proof_path(models_root, binding.model_id);
    let write_result = write_record(&proof_path, &serialized);
    let after = capture_asr_durable_snapshot(models_root, directory, binding).ok_or_else(|| {
        AsrProofPublishError::Integrity(
            "the local model identity changed while publishing its proof".into(),
        )
    })?;
    if after != *validated_snapshot {
        return Err(AsrProofPublishError::Integrity(
            "the local model identity changed while publishing its proof".into(),
        ));
    }
    write_result.map_err(AsrProofPublishError::Cache)
}

fn verify_pinned_model_files(
    models_root: &Path,
    model_id: &str,
    expected: &[(String, u64, String)],
    cached: Option<&[FileIdentity]>,
    mut opened_files: Option<&mut Vec<std::fs::File>>,
    hashed_bytes: &AtomicU64,
) -> Result<Vec<FileIdentity>, String> {
    let directory = checked_model_directory(models_root, model_id)?;
    let proof_binding = opened_files
        .is_none()
        .then(|| asr_proof_binding(model_id, expected))
        .flatten();
    let allowed_weights: std::collections::HashSet<&str> = expected
        .iter()
        .map(|(relative, _, _)| relative.as_str())
        .filter(|relative| relative.ends_with(".safetensors"))
        .collect();
    let entries = std::fs::read_dir(&directory)
        .map_err(|_| "the pinned local model directory could not be inspected".to_owned())?;
    for entry in entries {
        let entry = entry
            .map_err(|_| "the pinned local model directory could not be inspected".to_owned())?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err("the pinned local model directory contains an invalid filename".into());
        };
        if name.ends_with(".safetensors") && !allowed_weights.contains(name) {
            return Err("the local model directory contains an unpinned weights file".into());
        }
    }
    let mut identities = Vec::with_capacity(expected.len());
    for (relative, expected_bytes, _) in expected {
        let path = directory.join(relative);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|_| "a pinned local model file is missing".to_owned())?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() != *expected_bytes
        {
            return Err("a pinned local model file has an unexpected type or size".into());
        }
        identities.push(file_identity(relative, &metadata));
    }
    if opened_files.is_none() && cached.is_some_and(|cached| cached == identities.as_slice()) {
        return Ok(identities);
    }

    if let Some(binding) = proof_binding.as_ref() {
        if asr_proof_cache_hit(models_root, &directory, binding, &identities) {
            return Ok(identities);
        }
    }

    let before_proof_snapshot = proof_binding
        .as_ref()
        .and_then(|binding| capture_asr_durable_snapshot(models_root, &directory, binding));

    use std::io::Read;
    for ((relative, expected_bytes, expected_sha), identity) in expected.iter().zip(&identities) {
        let path = directory.join(relative);
        let before = std::fs::symlink_metadata(&path)
            .map_err(|_| "a pinned local model file changed during verification".to_owned())?;
        if before.file_type().is_symlink() || file_identity(relative, &before) != *identity {
            return Err("a pinned local model file changed during verification".into());
        }
        let mut file = std::fs::File::open(&path)
            .map_err(|_| "a pinned local model file could not be opened".to_owned())?;
        let opened = file
            .metadata()
            .map_err(|_| "a pinned local model file could not be inspected".to_owned())?;
        if !opened.is_file() || file_identity(relative, &opened) != *identity {
            return Err("a pinned local model file changed during verification".into());
        }
        let mut reader = (&mut file).take(expected_bytes.saturating_add(1));
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 1024 * 1024];
        let mut total = 0u64;
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|_| "a pinned local model file could not be read".to_owned())?;
            if read == 0 {
                break;
            }
            total = total.saturating_add(read as u64);
            hashed_bytes.fetch_add(read as u64, Ordering::Relaxed);
            hasher.update(&buffer[..read]);
        }
        let after = std::fs::symlink_metadata(&path)
            .map_err(|_| "a pinned local model file changed during verification".to_owned())?;
        let digest = format!("{:x}", hasher.finalize());
        if total != *expected_bytes
            || digest != *expected_sha
            || file_identity(relative, &after) != *identity
        {
            return Err("a pinned local model file failed integrity verification".into());
        }
        if let Some(files) = opened_files.as_mut() {
            files.push(file);
        }
    }
    if let (Some(binding), Some(before)) = (proof_binding.as_ref(), before_proof_snapshot.as_ref())
    {
        let after =
            capture_asr_durable_snapshot(models_root, &directory, binding).ok_or_else(|| {
                "the local model or installed marker changed during integrity verification"
                    .to_owned()
            })?;
        if &after != before || after.runtime_files != identities {
            return Err(
                "the local model or installed marker changed during integrity verification".into(),
            );
        }
        match publish_asr_durable_proof(models_root, &directory, binding, &identities, &after) {
            Ok(()) => {}
            Err(AsrProofPublishError::Cache(_reason)) => {
                log::debug!("local ASR durable proof cache was unavailable");
            }
            Err(AsrProofPublishError::Integrity(reason)) => return Err(reason),
        }
    }
    Ok(identities)
}

fn file_identity(path: &str, metadata: &std::fs::Metadata) -> FileIdentity {
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    #[cfg(unix)]
    let (device, inode) = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let (device, inode) = (0, 0);
    FileIdentity {
        path: path.to_owned(),
        bytes: metadata.len(),
        modified_ns,
        changed_ns: {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                (metadata.ctime() as i128) * 1_000_000_000 + metadata.ctime_nsec() as i128
            }
            #[cfg(not(unix))]
            {
                0
            }
        },
        device,
        inode,
        mode: {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                metadata.mode() & 0o7777
            }
            #[cfg(not(unix))]
            {
                0
            }
        },
        owner: {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                metadata.uid()
            }
            #[cfg(not(unix))]
            {
                0
            }
        },
    }
}

#[cfg(all(test, target_os = "macos"))]
mod durable_asr_proof_tests {
    use super::{model_dir, FixtureConfig, OnDeviceRuntime, ASR_INSTALLED_MANIFEST};
    use crate::ondevice_models::QWEN_ASR_06_ID;
    use sha2::{Digest, Sha256};
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn fixture() -> (
        crate::evaluation::fixtures::FixtureDirectory,
        PathBuf,
        Vec<(String, u64, String)>,
    ) {
        let owner = crate::evaluation::fixtures::FixtureDirectory::new();
        let models_root = owner.0.join("models");
        let directory = model_dir(&models_root, QWEN_ASR_06_ID);
        std::fs::create_dir_all(&directory).unwrap();
        let payload = b"tiny synthetic safetensors payload";
        let expected = vec![(
            "model.safetensors".to_owned(),
            payload.len() as u64,
            hex::encode(Sha256::digest(payload)),
        )];
        std::fs::write(directory.join("model.safetensors"), payload).unwrap();
        let marker = serde_json::json!({
            "schema_version": 1,
            "id": QWEN_ASR_06_ID,
            "revision": crate::ondevice_models::mlx_model(QWEN_ASR_06_ID).unwrap().revision,
            "files": expected.iter().map(|(path, bytes, sha256)| serde_json::json!({
                "path": path,
                "bytes": bytes,
                "sha256": sha256,
            })).collect::<Vec<_>>(),
        });
        std::fs::write(
            directory.join(ASR_INSTALLED_MANIFEST),
            serde_json::to_vec(&marker).unwrap(),
        )
        .unwrap();
        (owner, models_root, expected)
    }

    fn runtime(
        models_root: PathBuf,
        expected_files: Vec<(String, u64, String)>,
    ) -> OnDeviceRuntime {
        OnDeviceRuntime::with_fixture(
            models_root,
            FixtureConfig {
                expected_files,
                worker_program: PathBuf::from("python3"),
                worker_args: vec![OsString::from("unused-fixture.py")],
                observer: Arc::new(|_| {}),
            },
        )
    }

    #[tokio::test]
    async fn fresh_runtime_skips_hash_with_durable_proof() {
        let (_owner, models_root, expected) = fixture();
        let first = runtime(models_root.clone(), expected.clone());
        first.verify_model_integrity(QWEN_ASR_06_ID).await.unwrap();
        assert!(first.hashed_model_bytes() > 0);
        drop(first);

        let fresh = runtime(models_root, expected);
        fresh.verify_model_integrity(QWEN_ASR_06_ID).await.unwrap();
        assert_eq!(fresh.hashed_model_bytes(), 0);
    }

    #[tokio::test]
    async fn changed_ctime_forces_rehash() {
        use std::os::unix::fs::PermissionsExt;

        let (_owner, models_root, expected) = fixture();
        let first = runtime(models_root.clone(), expected.clone());
        first.verify_model_integrity(QWEN_ASR_06_ID).await.unwrap();
        assert!(first.hashed_model_bytes() > 0);
        drop(first);

        let model_path = model_dir(&models_root, QWEN_ASR_06_ID).join("model.safetensors");
        let before = std::fs::metadata(&model_path).unwrap();
        let before_identity = super::file_identity("model.safetensors", &before);
        let permissions = before.permissions().mode() & 0o7777;
        std::fs::set_permissions(
            &model_path,
            std::fs::Permissions::from_mode(permissions ^ 0o100),
        )
        .unwrap();
        std::fs::set_permissions(&model_path, std::fs::Permissions::from_mode(permissions))
            .unwrap();
        std::fs::File::open(&model_path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(before.modified().unwrap()))
            .unwrap();
        let after = std::fs::metadata(&model_path).unwrap();
        let after_identity = super::file_identity("model.safetensors", &after);
        assert_eq!(after_identity.bytes, before_identity.bytes);
        assert_eq!(after_identity.modified_ns, before_identity.modified_ns);
        assert_eq!(after_identity.device, before_identity.device);
        assert_eq!(after_identity.inode, before_identity.inode);
        assert_eq!(after_identity.mode, before_identity.mode);
        assert_eq!(after_identity.owner, before_identity.owner);
        assert_ne!(after_identity.changed_ns, before_identity.changed_ns);

        let fresh = runtime(models_root, expected);
        fresh.verify_model_integrity(QWEN_ASR_06_ID).await.unwrap();
        assert!(fresh.hashed_model_bytes() > 0);
    }

    #[tokio::test]
    async fn equal_length_bad_payload_is_rejected_after_proof_miss() {
        let (_owner, models_root, expected) = fixture();
        let first = runtime(models_root.clone(), expected.clone());
        first.verify_model_integrity(QWEN_ASR_06_ID).await.unwrap();
        drop(first);

        let model_path = model_dir(&models_root, QWEN_ASR_06_ID).join("model.safetensors");
        let metadata = std::fs::metadata(&model_path).unwrap();
        let old_bytes = std::fs::read(&model_path).unwrap();
        let mut bad_bytes = old_bytes.clone();
        bad_bytes[0] ^= 1;
        assert_eq!(bad_bytes.len(), old_bytes.len());
        std::fs::write(&model_path, bad_bytes).unwrap();
        std::fs::File::open(&model_path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
            .unwrap();

        let fresh = runtime(models_root, expected);
        assert!(fresh.verify_model_integrity(QWEN_ASR_06_ID).await.is_err());
        assert!(fresh.hashed_model_bytes() > 0);
    }

    #[tokio::test]
    async fn durable_proof_write_failure_keeps_full_verified_model_usable() {
        let (owner, models_root, expected) = fixture();
        std::fs::write(
            owner.0.join(".voiceflow-model-integrity"),
            b"cache unavailable",
        )
        .unwrap();
        let runtime = runtime(models_root, expected);
        runtime
            .verify_model_integrity(QWEN_ASR_06_ID)
            .await
            .unwrap();
        assert!(runtime.hashed_model_bytes() > 0);
    }

    #[tokio::test]
    async fn unpinned_extra_weights_are_rejected_before_proof_hit() {
        let (_owner, models_root, expected) = fixture();
        let first = runtime(models_root.clone(), expected.clone());
        first.verify_model_integrity(QWEN_ASR_06_ID).await.unwrap();
        drop(first);
        std::fs::write(
            model_dir(&models_root, QWEN_ASR_06_ID).join("extra.safetensors"),
            b"extra",
        )
        .unwrap();

        let fresh = runtime(models_root, expected);
        assert!(fresh.verify_model_integrity(QWEN_ASR_06_ID).await.is_err());
        assert_eq!(fresh.hashed_model_bytes(), 0);
    }

    #[tokio::test]
    async fn malformed_durable_proof_is_a_miss_and_rehashes() {
        let (_owner, models_root, expected) = fixture();
        let first = runtime(models_root.clone(), expected.clone());
        first.verify_model_integrity(QWEN_ASR_06_ID).await.unwrap();
        drop(first);
        let proof = crate::modelstore::local_model_proof_path(&models_root, QWEN_ASR_06_ID);
        std::fs::write(proof, b"{}").unwrap();

        let fresh = runtime(models_root, expected);
        fresh.verify_model_integrity(QWEN_ASR_06_ID).await.unwrap();
        assert!(fresh.hashed_model_bytes() > 0);
    }

    #[tokio::test]
    async fn model_identity_change_during_proof_publish_fails_closed() {
        let (_owner, models_root, expected) = fixture();
        let verified_runtime = runtime(models_root.clone(), expected.clone());
        verified_runtime
            .verify_model_integrity(QWEN_ASR_06_ID)
            .await
            .unwrap();
        let binding = super::asr_proof_binding(QWEN_ASR_06_ID, &expected).unwrap();
        let directory = super::checked_model_directory(&models_root, QWEN_ASR_06_ID).unwrap();
        let snapshot =
            super::capture_asr_durable_snapshot(&models_root, &directory, &binding).unwrap();
        let verified_files = verified_runtime
            .verified_files
            .lock()
            .unwrap()
            .get(QWEN_ASR_06_ID)
            .cloned()
            .unwrap();
        let model_path = directory.join("model.safetensors");
        let mut changed = std::fs::read(&model_path).unwrap();
        changed[0] ^= 1;
        let proof_path = crate::modelstore::local_model_proof_path(&models_root, QWEN_ASR_06_ID);
        let result = super::publish_asr_durable_proof_with_writer(
            &models_root,
            &directory,
            &binding,
            &verified_files,
            &snapshot,
            |path, bytes| {
                std::fs::write(&model_path, changed).unwrap();
                crate::modelstore::write_private_proof_record(path, bytes)
            },
        );
        assert!(matches!(
            result,
            Err(super::AsrProofPublishError::Integrity(_))
        ));
        assert!(proof_path.exists());
    }

    #[tokio::test]
    async fn installed_marker_change_after_validation_fails_before_cache_write() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let (_owner, models_root, expected) = fixture();
        let verified_runtime = runtime(models_root.clone(), expected.clone());
        verified_runtime
            .verify_model_integrity(QWEN_ASR_06_ID)
            .await
            .unwrap();
        let binding = super::asr_proof_binding(QWEN_ASR_06_ID, &expected).unwrap();
        let directory = super::checked_model_directory(&models_root, QWEN_ASR_06_ID).unwrap();
        let snapshot =
            super::capture_asr_durable_snapshot(&models_root, &directory, &binding).unwrap();
        let verified_files = verified_runtime
            .verified_files
            .lock()
            .unwrap()
            .get(QWEN_ASR_06_ID)
            .cloned()
            .unwrap();
        let model_path = directory.join("model.safetensors");
        let before_weight = super::file_identity(
            "model.safetensors",
            &std::fs::metadata(&model_path).unwrap(),
        );
        let marker_path = directory.join(super::ASR_INSTALLED_MANIFEST);
        let mut marker_bytes = std::fs::read(&marker_path).unwrap();
        marker_bytes.push(b' ');
        std::fs::write(&marker_path, marker_bytes).unwrap();
        assert_eq!(
            before_weight,
            super::file_identity(
                "model.safetensors",
                &std::fs::metadata(&model_path).unwrap()
            )
        );

        let writer_called = AtomicBool::new(false);
        let result = super::publish_asr_durable_proof_with_writer(
            &models_root,
            &directory,
            &binding,
            &verified_files,
            &snapshot,
            |_, _| {
                writer_called.store(true, Ordering::Relaxed);
                Ok(())
            },
        );
        assert!(matches!(
            result,
            Err(super::AsrProofPublishError::Integrity(_))
        ));
        assert!(!writer_called.load(Ordering::Relaxed));
    }
}

/// This owner retains open descriptors; a path or .wav extension is not cleanup authority.
struct PrivateAudioFile {
    path: PathBuf,
    storage: PathBuf,
    name: String,
    file: Option<std::fs::File>,
    parent: Option<std::fs::File>,
    displaced_unknown: Option<String>,
}
fn retained_private_audio() -> &'static Mutex<Vec<PrivateAudioFile>> {
    static RETAINED: OnceLock<Mutex<Vec<PrivateAudioFile>>> = OnceLock::new();
    RETAINED.get_or_init(Mutex::default)
}
impl PrivateAudioFile {
    fn create(models_root: &Path, id: &str, bytes: &[u8]) -> Result<Self, String> {
        Self::create_with_writer(models_root, id, |file| file.write_all(bytes))
    }
    fn create_with_writer(
        models_root: &Path,
        id: &str,
        write: impl FnOnce(&mut std::fs::File) -> io::Result<()>,
    ) -> Result<Self, String> {
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
        {
            return Err("private audio ID is invalid".into());
        }
        if retained_private_audio()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len()
            >= 32
        {
            return Err("private audio cleanup requires attention before another request".into());
        }
        let root = models_root.join("audio-tmp");
        let metadata = std::fs::symlink_metadata(models_root).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("private audio storage is unavailable".into());
        }
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let before = std::fs::symlink_metadata(&root).map_err(|e| e.to_string())?;
        if before.file_type().is_symlink() || !before.is_dir() {
            return Err("private audio storage is unavailable".into());
        }
        let parent = std::fs::File::open(&root).map_err(|e| e.to_string())?;
        if !same_private_inode(&before, &parent.metadata().map_err(|e| e.to_string())?) {
            return Err("private audio parent identity changed".into());
        }
        let canonical = root.canonicalize().map_err(|e| e.to_string())?;
        let models = models_root.canonicalize().map_err(|e| e.to_string())?;
        if canonical.parent() != Some(models.as_path())
            || !same_private_inode(
                &before,
                &std::fs::metadata(&canonical).map_err(|e| e.to_string())?,
            )
        {
            return Err("private audio storage escaped its held root".into());
        }
        private_directory_permissions(&parent).map_err(|e| e.to_string())?;
        let name = format!("{id}.wav");
        let path = canonical.join(&name);
        let file = private_audio_open_new(&parent, &name).map_err(|e| e.to_string())?;
        let mut owner = Self {
            path,
            storage: canonical,
            name,
            file: Some(file),
            parent: Some(parent),
            displaced_unknown: None,
        };
        let applicable = owner.path_applicable();
        crate::observations::private_audio_resource(
            &owner.path,
            "created",
            applicable,
            None,
            Some(&owner.name),
            None,
        );
        if applicable != Some(true) {
            return Err("private audio path no longer names owned file".into());
        }
        let file = owner.file.as_mut().unwrap();
        write(file)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        if owner.path_applicable() != Some(true) {
            return Err("private audio path changed during creation".into());
        }
        Ok(owner)
    }
    fn path_applicable(&self) -> Option<bool> {
        let owned = self.file.as_ref()?.metadata().ok()?;
        match std::fs::symlink_metadata(&self.path) {
            Ok(current) => {
                Some(!current.file_type().is_symlink() && same_private_inode(&owned, &current))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Some(false),
            Err(_) => None,
        }
    }
    fn cleanup(&mut self) -> Result<&'static str, String> {
        let Some(file) = &self.file else {
            return Ok("no_held_audio");
        };
        if self.displaced_unknown.is_some() {
            return Err("a displaced unknown file is retained pending explicit recovery".into());
        }
        let parent = self
            .parent
            .as_ref()
            .ok_or("private audio parent owner unavailable")?;
        let owned = file.metadata().map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if owned.nlink() == 0 {
                self.file.take();
                self.parent.take();
                return Ok("released_already_unlinked_file");
            }
        }

        // Exclusive quarantine first: inspect the atomically moved entry before
        // unlink. If it was replaced, restore without overwriting a new entry.
        let mut quarantine = None;
        for _ in 0..8 {
            use chacha20poly1305::{
                aead::{AeadCore, OsRng},
                XChaCha20Poly1305,
            };
            let name = format!(
                ".audio-cleanup-{}",
                hex::encode(XChaCha20Poly1305::generate_nonce(&mut OsRng))
            );
            match private_audio_move_new(parent, &self.name, &name) {
                Ok(()) => {
                    quarantine = Some(name);
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        let quarantine = quarantine.ok_or("bounded private audio quarantine allocation failed")?;
        let actual = private_audio_metadata(parent, &quarantine).map_err(|e| e.to_string());
        if !actual.as_ref().is_ok_and(|m| same_private_inode(&owned, m)) {
            if private_audio_move_new(parent, &quarantine, &self.name).is_err() {
                self.displaced_unknown = Some(quarantine);
            }
            return Err(
                "private audio name was replaced; foreign content preserved, owned file retained"
                    .into(),
            );
        }
        self.name = quarantine;
        private_audio_unlink(parent, &self.name).map_err(|e| e.to_string())?;
        self.file.take();
        self.parent.take();
        Ok("removed_owned_file")
    }
}
impl Drop for PrivateAudioFile {
    fn drop(&mut self) {
        if self.file.is_none() {
            return;
        }
        let applicable = self.path_applicable();
        match self.cleanup() {
            Ok(action) => crate::observations::private_audio_resource(
                &self.path,
                action,
                applicable,
                None,
                Some(&self.name),
                None,
            ),
            Err(reason) => {
                crate::observations::private_audio_resource(
                    &self.path,
                    "retained_cleanup_failed",
                    applicable,
                    Some(&reason),
                    Some(&self.name),
                    self.displaced_unknown.as_deref(),
                );
                log::warn!(
                    "private local audio cleanup retained; explicit ownership recovery required"
                );
                let retained = Self {
                    path: self.path.clone(),
                    storage: self.storage.clone(),
                    name: self.name.clone(),
                    file: self.file.take(),
                    parent: self.parent.take(),
                    displaced_unknown: self.displaced_unknown.take(),
                };
                retained_private_audio()
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(retained);
            }
        }
    }
}
#[cfg(unix)]
fn same_private_inode(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}
#[cfg(not(unix))]
fn same_private_inode(_: &std::fs::Metadata, _: &std::fs::Metadata) -> bool {
    false
}
#[cfg(unix)]
fn private_directory_permissions(parent: &std::fs::File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    if unsafe { libc::fchmod(parent.as_raw_fd(), 0o700) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
#[cfg(unix)]
fn private_audio_open_new(parent: &std::fs::File, name: &str) -> io::Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let name = std::ffi::CString::new(name)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { std::fs::File::from_raw_fd(fd) })
    }
}
#[cfg(unix)]
fn private_audio_metadata(parent: &std::fs::File, name: &str) -> io::Result<std::fs::Metadata> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let name = std::ffi::CString::new(name)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe { std::fs::File::from_raw_fd(fd) }.metadata()
}
#[cfg(unix)]
fn private_audio_unlink(parent: &std::fs::File, name: &str) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let name = std::ffi::CString::new(name)?;
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
#[cfg(target_os = "macos")]
fn private_audio_move_new(
    parent: &std::fs::File,
    source: &str,
    destination: &str,
) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let source = std::ffi::CString::new(source)?;
    let destination = std::ffi::CString::new(destination)?;
    if unsafe {
        libc::renameatx_np(
            parent.as_raw_fd(),
            source.as_ptr(),
            parent.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_EXCL,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
#[cfg(target_os = "linux")]
fn private_audio_move_new(
    parent: &std::fs::File,
    source: &str,
    destination: &str,
) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let source = std::ffi::CString::new(source)?;
    let destination = std::ffi::CString::new(destination)?;
    if unsafe {
        libc::renameat2(
            parent.as_raw_fd(),
            source.as_ptr(),
            parent.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn private_audio_move_new(_: &std::fs::File, _: &str, _: &str) -> io::Result<()> {
    Err(io::Error::other("exclusive private audio move unavailable"))
}
#[cfg(not(unix))]
fn private_directory_permissions(_: &std::fs::File) -> io::Result<()> {
    Err(io::Error::other(
        "private audio directory ownership unavailable",
    ))
}
#[cfg(not(unix))]
fn private_audio_open_new(_: &std::fs::File, _: &str) -> io::Result<std::fs::File> {
    Err(io::Error::other("private audio ownership unavailable"))
}
#[cfg(not(unix))]
fn private_audio_metadata(_: &std::fs::File, _: &str) -> io::Result<std::fs::Metadata> {
    Err(io::Error::other("private audio ownership unavailable"))
}
#[cfg(not(unix))]
fn private_audio_unlink(_: &std::fs::File, _: &str) -> io::Result<()> {
    Err(io::Error::other("private audio ownership unavailable"))
}

struct ModelLoadCancellationGuard<'a> {
    runtime: &'a OnDeviceRuntime,
    model_id: String,
    generation: u64,
    previous_status: RuntimeSnapshot,
    child_pid: Option<u32>,
    armed: bool,
}

impl<'a> ModelLoadCancellationGuard<'a> {
    fn new(
        runtime: &'a OnDeviceRuntime,
        model_id: &str,
        generation: u64,
        previous_status: RuntimeSnapshot,
    ) -> Self {
        Self {
            runtime,
            model_id: model_id.to_owned(),
            generation,
            previous_status,
            child_pid: None,
            armed: true,
        }
    }

    fn record_child_pid(&mut self, pid: u32) {
        if pid != 0 {
            self.child_pid = Some(pid);
        }
    }

    fn finish_failure(&mut self, error: &str) {
        self.runtime
            .finish_model_load_failure(&self.model_id, Some(self.generation), error);
        self.armed = false;
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ModelLoadCancellationGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.runtime.cancel_dropped_model_load(
                &self.model_id,
                self.generation,
                self.child_pid,
                &self.previous_status,
            );
        }
    }
}

struct KillOnDrop {
    pid: u32,
    armed: bool,
}

#[derive(Clone)]
struct AsrCancellationSignal {
    token: tokio_util::sync::CancellationToken,
    cancelled_at: Arc<Mutex<Option<tokio::time::Instant>>>,
}

impl AsrCancellationSignal {
    fn new() -> Self {
        Self {
            token: tokio_util::sync::CancellationToken::new(),
            cancelled_at: Arc::new(Mutex::new(None)),
        }
    }

    fn cancel(&self) {
        let mut cancelled_at = self
            .cancelled_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cancelled_at.is_none() {
            *cancelled_at = Some(tokio::time::Instant::now());
        }
        self.token.cancel();
    }

    fn deadline(&self) -> tokio::time::Instant {
        self.cancelled_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .unwrap_or_else(tokio::time::Instant::now)
            + Duration::from_secs(2)
    }
}

struct CancelTranscriptionOnDrop {
    cancel: AsrCancellationSignal,
    armed: bool,
}

struct RuntimeTaskHandoff<'a> {
    slot: &'a Mutex<Vec<tokio::task::JoinHandle<()>>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl RuntimeTaskHandoff<'_> {
    async fn join(&mut self) -> Result<(), tokio::task::JoinError> {
        while let Some(task) = self.tasks.first_mut() {
            let result = task.await;
            self.tasks.remove(0);
            result?;
        }
        Ok(())
    }
}

impl Drop for RuntimeTaskHandoff<'_> {
    fn drop(&mut self) {
        self.slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .append(&mut self.tasks);
    }
}

#[cfg(test)]
#[tokio::test]
async fn runtime_task_handoff_restores_pending_join_handle_on_drop() {
    let slot = Mutex::new(vec![tokio::spawn(std::future::pending::<()>())]);
    let tasks = slot.lock().unwrap().drain(..).collect();
    let mut handoff = RuntimeTaskHandoff { slot: &slot, tasks };
    tokio::select! {
        _ = handoff.join() => panic!("pending owner unexpectedly completed"),
        _ = tokio::time::sleep(Duration::from_millis(10)) => {}
    }
    drop(handoff);
    let task = slot.lock().unwrap().pop().unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}

impl CancelTranscriptionOnDrop {
    fn new(cancel: AsrCancellationSignal) -> Self {
        Self {
            cancel,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CancelTranscriptionOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.cancel.cancel();
        }
    }
}

impl KillOnDrop {
    fn new(pid: u32) -> Self {
        Self { pid, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if self.armed {
            // Only Child::wait may retire this identity.
            kill_process(self.pid);
        }
    }
}

fn kill_process(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(unix)]
    unsafe {
        let _ = libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
}

fn set_private_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    let _ = path;
    Ok(())
}

fn sanitize_runtime_error(error: &str) -> String {
    let filtered = error
        .chars()
        .filter(|character| !character.is_control())
        .take(400)
        .collect::<String>();
    filtered
        .split("audio-tmp")
        .next()
        .unwrap_or("local runtime failed")
        .to_owned()
}

pub(crate) async fn shutdown_for_root_with_status(models_root: &Path) -> Result<(), &'static str> {
    let runtime = shared_runtime(models_root);
    runtime.shutdown_with_status().await?;
    cleanup_private_audio_for_root(models_root);
    Ok(())
}

pub fn cleanup_private_audio_for_root(models_root: &Path) {
    cleanup_private_audio_files(&models_root.join("audio-tmp"));
}

fn cleanup_private_audio_files(root: &Path) {
    // Retry only descriptors retained by this producer. Historical orphan files
    // have unknown ownership and remain untouched; extension/path is not proof.
    let selected = root.canonicalize().unwrap_or_else(|_| root.into());
    let retry = {
        let mut retained = retained_private_audio()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mut retry = Vec::new();
        let mut keep = Vec::new();
        for owner in retained.drain(..) {
            if owner.storage == selected {
                retry.push(owner);
            } else {
                keep.push(owner);
            }
        }
        *retained = keep;
        retry
    };
    for owner in retry {
        drop(owner);
    }
}

#[cfg(test)]
mod evaluation_identity_tests {
    #[test]
    fn private_audio_drop_and_write_failure_remove_only_owned_file() {
        let d = crate::evaluation::fixtures::FixtureDirectory::new();
        let file = super::PrivateAudioFile::create(&d.0, "owned", b"fixture").unwrap();
        let path = file.path.clone();
        assert!(path.exists());
        drop(file);
        assert!(!path.exists());
        let failed = super::PrivateAudioFile::create_with_writer(&d.0, "failed", |file| {
            use std::io::Write;
            file.write_all(b"partial")?;
            Err(std::io::Error::other("controlled write failure"))
        });
        assert!(failed.is_err());
        assert!(!d.0.join("audio-tmp/failed.wav").exists());
        std::fs::write(
            d.0.join("audio-tmp/unowned.wav"),
            b"historical unknown orphan",
        )
        .unwrap();
        super::cleanup_private_audio_for_root(&d.0);
        assert_eq!(
            std::fs::read(d.0.join("audio-tmp/unowned.wav")).unwrap(),
            b"historical unknown orphan"
        );
    }
    #[test]
    fn private_audio_moved_parent_and_replaced_name_preserve_foreign_sentinels() {
        let d = crate::evaluation::fixtures::FixtureDirectory::new();
        let file = super::PrivateAudioFile::create(&d.0, "moved", b"owned").unwrap();
        let selected = d.0.join("audio-tmp");
        let moved = d.0.join("audio-moved");
        std::fs::rename(&selected, &moved).unwrap();
        std::fs::create_dir(&selected).unwrap();
        std::fs::write(selected.join("moved.wav"), b"foreign parent").unwrap();
        drop(file);
        assert!(!moved.join("moved.wav").exists());
        assert_eq!(
            std::fs::read(selected.join("moved.wav")).unwrap(),
            b"foreign parent"
        );
        let file = super::PrivateAudioFile::create(&d.0, "replaced", b"owned").unwrap();
        let path = file.path.clone();
        let saved = path.with_extension("owned-moved");
        std::fs::rename(&path, &saved).unwrap();
        std::fs::write(&path, b"foreign leaf").unwrap();
        drop(file);
        assert_eq!(std::fs::read(&path).unwrap(), b"foreign leaf");
        assert_eq!(std::fs::read(&saved).unwrap(), b"owned");
        super::cleanup_private_audio_for_root(&d.0);
        assert_eq!(std::fs::read(&path).unwrap(), b"foreign leaf");
    }
    #[test]
    fn write_failure_after_name_replacement_retains_owned_partial_and_foreign_leaf() {
        let d = crate::evaluation::fixtures::FixtureDirectory::new();
        let path = d.0.join("audio-tmp/write-replaced.wav");
        let moved = path.with_extension("partial-owned");
        let result = super::PrivateAudioFile::create_with_writer(&d.0, "write-replaced", |file| {
            use std::io::Write;
            file.write_all(b"partial owned")?;
            std::fs::rename(&path, &moved)?;
            std::fs::write(&path, b"foreign after write")?;
            Err(std::io::Error::other(
                "controlled write failure after replacement",
            ))
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"foreign after write");
        assert_eq!(std::fs::read(&moved).unwrap(), b"partial owned");
    }
    #[tokio::test]
    async fn cancelling_local_future_drops_its_private_audio_owner() {
        let d = crate::evaluation::fixtures::FixtureDirectory::new();
        let file = super::PrivateAudioFile::create(&d.0, "cancelled", b"owned").unwrap();
        let path = file.path.clone();
        let result = tokio::time::timeout(super::Duration::from_millis(1), async move {
            let _owner = file;
            std::future::pending::<()>().await;
        })
        .await;
        assert!(result.is_err());
        assert!(!path.exists());
    }

    use super::*;
    #[test]
    fn loaded_identity_admission_distinguishes_missing_wrong_and_old_revision() {
        for (id, revision, expected) in [
            (None, Some("revision"), LoadedIdentityError::MissingId),
            (Some("model"), None, LoadedIdentityError::MissingRevision),
            (
                Some("other"),
                Some("revision"),
                LoadedIdentityError::WrongId,
            ),
            (
                Some("model"),
                Some("old"),
                LoadedIdentityError::WrongRevision,
            ),
        ] {
            for stage in [LocalStage::ModelLoad, LocalStage::Inference] {
                assert_eq!(
                    LoadedModelIdentity::parse(id, revision, "model", "revision", stage)
                        .err()
                        .as_ref(),
                    Some(&expected)
                );
            }
        }
        let actual = LoadedModelIdentity::parse(
            Some("model"),
            Some("revision"),
            "model",
            "revision",
            LocalStage::ModelLoad,
        )
        .unwrap();
        assert_eq!(actual.id, "model");
        assert_eq!(actual.revision, "revision");
    }
    #[test]
    fn local_result_admission_separates_empty_coverage_budget_and_incomplete() {
        let mut result = TranscriptionResult {
            text: String::new(),
            language: None,
            generation_tokens: 1,
            covered_samples: 160,
            chunks_completed: 1,
            chunks_total: 1,
            completion: "complete".into(),
        };
        assert_eq!(
            result.rejection(160, 4096),
            Some(LocalResultRejection::Empty)
        );
        result.generation_tokens = 0;
        assert_eq!(
            result.rejection(160, 4096),
            Some(LocalResultRejection::GenerationBudget)
        );
        result.generation_tokens = 1;
        result.chunks_completed = 0;
        assert_eq!(
            result.rejection(160, 4096),
            Some(LocalResultRejection::Coverage)
        );
        result.chunks_completed = 1;
        result.covered_samples = 159;
        assert_eq!(
            result.rejection(160, 4096),
            Some(LocalResultRejection::Coverage)
        );
        result.completion = "budget_exhausted".into();
        assert_eq!(
            result.rejection(160, 4096),
            Some(LocalResultRejection::Incomplete)
        );
        result.completion = "complete".into();
        result.covered_samples = 160;
        result.text = "hello".into();
        assert!(result.rejection(160, 4096).is_none());
    }
    #[tokio::test]
    async fn only_qualified_local_empty_rejection_emits_measurement_without_accepting_delivery() {
        use crate::observations::{FixtureCapture, ObservedText};
        let complete_empty = TranscriptionResult {
            text: String::new(),
            language: None,
            generation_tokens: 1,
            covered_samples: 160,
            chunks_completed: 1,
            chunks_total: 1,
            completion: "complete".into(),
        };
        for (factor, expected) in [
            ("empty", LocalResultRejection::Empty),
            ("completion", LocalResultRejection::Incomplete),
            ("coverage", LocalResultRejection::Coverage),
            ("budget", LocalResultRejection::GenerationBudget),
        ] {
            let mut result = complete_empty.clone();
            match factor {
                "completion" => result.completion = "budget_exhausted".into(),
                "coverage" => result.covered_samples = 159,
                "budget" => result.generation_tokens = 0,
                _ => {}
            }
            let capture = FixtureCapture::new(false);
            capture
                .scope(async {
                    // Identity admission precedes this result owner in live code.
                    // This fixture supplies a declared parsed identity, not a load.
                    crate::asr::observe_parsed_asr(
                        &result.text,
                        "local_model_output",
                        &result.completion,
                        Some("fixture-model"),
                    );
                    let rejected = result.rejection(160, 4096).unwrap();
                    assert_eq!(rejected, expected);
                    result.observe_rejection(&rejected);
                    // The producing result still rejects delivery, including Empty.
                    assert!(result.rejection(160, 4096).is_some());
                })
                .await;
            let observed = capture.snapshot().asr.unwrap();
            assert_eq!(observed.confirmed_empty(), factor == "empty");
            assert!(matches!(
                observed.provider_text,
                ObservedText::KnownEmpty {}
            ));
            assert_eq!(
                observed.terminal,
                if factor == "empty" {
                    "local_complete_empty"
                } else {
                    "provider_failed"
                }
            );
            assert_eq!(observed.filtering_reason.as_deref(), Some(expected.code()));
        }
        let capture = FixtureCapture::new(false);
        capture
            .scope(async {
                let mut whitespace = complete_empty.clone();
                whitespace.text = " \n\t".into();
                crate::asr::observe_parsed_asr(
                    &whitespace.text,
                    "local_model_output",
                    "complete",
                    Some("fixture-model"),
                );
                let rejection = whitespace.rejection(160, 4096).unwrap();
                assert_eq!(rejection, LocalResultRejection::Empty);
                whitespace.observe_rejection(&rejection);
            })
            .await;
        let whitespace = capture.snapshot().asr.unwrap();
        assert!(whitespace.confirmed_empty());
        assert_eq!(whitespace.terminal, "locally_suppressed");
        assert!(matches!(
            whitespace.provider_text,
            ObservedText::KnownValue { .. }
        ));
        assert!(matches!(
            whitespace.filtered_text,
            ObservedText::KnownEmpty {}
        ));
        let capture = FixtureCapture::new(false);
        capture
            .scope(async {
                // Parsed raw empty by itself is insufficient: no admission receipt.
                crate::asr::observe_parsed_asr(
                    "",
                    "local_model_output",
                    "complete",
                    Some("fixture-model"),
                );
            })
            .await;
        assert!(!capture.snapshot().asr.unwrap().confirmed_empty());
    }
    #[tokio::test]
    async fn dropped_local_stage_is_unknown_and_keeps_unmatched_source_identity() {
        use crate::observations::{FixtureCapture, LocalStageState};
        let capture = FixtureCapture::new(false);
        let result = tokio::time::timeout(
            Duration::from_millis(1),
            capture.scope(async {
                let _stage = LocalStageGuard::start("model", "revision", LocalStage::Inference);
                let result = LoadedModelIdentity::parse(
                    Some("other"),
                    None,
                    "model",
                    "revision",
                    LocalStage::Inference,
                );
                assert_eq!(result.err(), Some(LoadedIdentityError::MissingRevision));
                std::future::pending::<()>().await;
            }),
        )
        .await;
        assert!(result.is_err());
        let observed = capture.snapshot().local_asr.unwrap();
        assert!(observed
            .events
            .iter()
            .any(|e| matches!(e.state, LocalStageState::Unknown)));
        assert!(observed.observed_model_id.is_none());
        assert!(observed.coverage.is_none());
        assert_eq!(
            observed.identity_checks[0].observed_model_id.as_deref(),
            Some("other")
        );
        assert!(observed.identity_checks[0].observed_revision.is_none());
        assert!(!observed.identity_checks[0].applicable);
    }
}

#[cfg(test)]
mod sidecar_resolution_tests {
    use super::{packaged_bundle_sidecar, resolve_sidecar_executable};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(1);

    struct TemporaryDirectory(PathBuf);

    impl TemporaryDirectory {
        fn new() -> Self {
            let serial = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "voiceflow-sidecar-resolution-{}-{serial}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn executable(&self, path: &Path) -> PathBuf {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, b"fixture executable").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            path.to_path_buf()
        }

        fn app_executable(&self) -> PathBuf {
            self.executable(&self.0.join("VoiceFlow.app/Contents/MacOS/voiceflow"))
        }

        fn packaged_sidecar(&self) -> PathBuf {
            self.executable(
                &self
                    .0
                    .join("VoiceFlow.app/Contents/Resources/voiceflow-mlx-sidecar"),
            )
        }
    }

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn formal_resolution_ignores_a_fake_external_override_and_uses_the_packaged_sidecar() {
        let fixture = TemporaryDirectory::new();
        let executable = fixture.app_executable();
        let packaged_sidecar = fixture.packaged_sidecar().canonicalize().unwrap();
        let fake_override = fixture.executable(&fixture.0.join("outside/fake-sidecar"));

        assert_eq!(
            resolve_sidecar_executable(&executable, Some(&fake_override), false),
            Some(packaged_sidecar)
        );
    }

    #[test]
    fn formal_resolution_does_not_use_a_fake_override_when_the_packaged_resource_is_missing() {
        let fixture = TemporaryDirectory::new();
        let executable = fixture.app_executable();
        let fake_override = fixture.executable(&fixture.0.join("outside/fake-sidecar"));

        assert_eq!(
            resolve_sidecar_executable(&executable, Some(&fake_override), false),
            None
        );
    }

    #[test]
    fn explicit_developer_tools_keep_the_sidecar_override() {
        let fixture = TemporaryDirectory::new();
        let executable = fixture.app_executable();
        let developer_override = fixture.executable(&fixture.0.join("research/helper"));

        assert_eq!(
            resolve_sidecar_executable(&executable, Some(&developer_override), true),
            Some(developer_override)
        );
    }

    #[cfg(unix)]
    #[test]
    fn formal_resolution_rejects_sidecars_that_escape_the_canonical_bundle_resources() {
        let fixture = TemporaryDirectory::new();
        let executable = fixture.app_executable();
        let resources = fixture
            .0
            .join("VoiceFlow.app/Contents/Resources/voiceflow-mlx-sidecar");
        std::fs::create_dir_all(resources.parent().unwrap()).unwrap();
        let external = fixture.executable(&fixture.0.join("outside/real-sidecar"));
        std::os::unix::fs::symlink(&external, &resources).unwrap();

        assert_eq!(packaged_bundle_sidecar(&executable), None);
    }

    #[cfg(unix)]
    #[test]
    fn formal_resolution_rejects_a_resources_directory_symlink_escape() {
        let fixture = TemporaryDirectory::new();
        let executable = fixture.app_executable();
        let contents = fixture.0.join("VoiceFlow.app/Contents");
        let external_resources = fixture.0.join("outside/Resources");
        std::fs::create_dir_all(&external_resources).unwrap();
        fixture.executable(&external_resources.join("voiceflow-mlx-sidecar"));
        std::os::unix::fs::symlink(&external_resources, contents.join("Resources")).unwrap();

        assert_eq!(packaged_bundle_sidecar(&executable), None);
    }
}

#[cfg(test)]
mod sidecar_response_line_bounds_tests {
    use super::{OnDeviceRuntime, MAX_RESPONSE_LINE_BYTES};
    use std::io;
    use std::time::Duration;
    use tokio::io::{AsyncWriteExt, BufReader};

    fn loaded_images(count: usize) -> Vec<String> {
        (0..count)
            .map(|index| {
                format!(
                    "/System/Library/Frameworks/Fixture{index:04}/Versions/A/Fixture|00000000-0000-0000-0000-{index:012x}"
                )
            })
            .collect()
    }

    fn load_response(images: Vec<String>) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "protocol_version": 1,
            "request_id": "fixture",
            "session_id": "fixture-session",
            "model_version": "fixture-revision",
            "op": "load",
            "status": "loaded",
            "loaded_attestation": {
                "schema_version": 1,
                "executable_sha256": "a".repeat(64),
                "metal_sha256": {
                    "mlx.metallib": "b".repeat(64),
                    "mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib": "c".repeat(64)
                },
                "loaded_images": images,
                "parameter_layout_sha256": "d".repeat(64),
                "parameter_count": 1,
                "parameter_dtypes": {"float32": 1},
                "decoder_recipe": "voiceflow-mlx-decoder-v1:chunks30s/min1s;temperature0;context-empty;qwen-repetition1.05/window32;inner-chunk1200s;cohere-topP1/topK0/vad-nil;ordered-trim-language-join"
            }
        }))
        .unwrap()
    }

    fn minimal_response_body() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "protocol_version": 1,
            "request_id": "fixture",
            "op": "hello",
            "status": "ready"
        }))
        .unwrap()
    }

    fn wire_line_with_exact_size(size: usize) -> Vec<u8> {
        assert!(size > 1);
        let mut line = minimal_response_body();
        assert!(line.len() < size - 1);
        line.resize(size - 1, b' ');
        line.push(b'\n');
        line
    }

    async fn read_from_bytes(line: Vec<u8>) -> io::Result<super::Response> {
        let capacity = line.len().max(1);
        let (mut writer, reader) = tokio::io::duplex(capacity);
        writer.write_all(&line).await.unwrap();
        drop(writer);
        let mut stdout = BufReader::new(reader);
        OnDeviceRuntime::read_sidecar_response_line(&mut stdout).await
    }

    #[tokio::test]
    async fn complete_inventory_above_old_reader_limit_is_not_truncated() {
        let expected = loaded_images(1_145);
        let mut line = load_response(expected.clone());
        line.push(b'\n');
        assert!(line.len() > 64 * 1024);
        assert!(line.len() <= MAX_RESPONSE_LINE_BYTES);
        let response = read_from_bytes(line).await.unwrap();
        let attestation = response.loaded_attestation.unwrap();
        assert_eq!(attestation.loaded_images, expected);
        assert!(attestation.complete());
    }

    #[tokio::test]
    async fn wire_line_limit_includes_newline_and_rejects_one_byte_over() {
        let exact = wire_line_with_exact_size(MAX_RESPONSE_LINE_BYTES);
        assert_eq!(exact.len(), MAX_RESPONSE_LINE_BYTES);
        assert!(read_from_bytes(exact).await.is_ok());

        let mut over = wire_line_with_exact_size(MAX_RESPONSE_LINE_BYTES);
        over.insert(over.len() - 1, b' ');
        assert_eq!(over.len(), MAX_RESPONSE_LINE_BYTES + 1);
        let error = read_from_bytes(over).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn eof_and_pending_read_keep_the_existing_terminal_behavior() {
        let (writer, reader) = tokio::io::duplex(8);
        drop(writer);
        let mut empty_stdout = BufReader::new(reader);
        let error = OnDeviceRuntime::read_sidecar_response_line(&mut empty_stdout)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);

        let (_writer, reader) = tokio::io::duplex(8);
        let mut pending_stdout = BufReader::new(reader);
        let result = tokio::time::timeout(
            Duration::from_millis(10),
            OnDeviceRuntime::read_sidecar_response_line(&mut pending_stdout),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn eof_before_line_terminator_remains_invalid_data() {
        let partial = minimal_response_body();
        let (mut writer, reader) = tokio::io::duplex(partial.len().max(1));
        writer.write_all(&partial).await.unwrap();
        drop(writer);
        let mut stdout = BufReader::new(reader);
        let error = OnDeviceRuntime::read_sidecar_response_line(&mut stdout)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}

#[cfg(test)]
mod asr_load_reservation_tests {
    use super::{OnDeviceRuntime, Request, Response, PROTOCOL_VERSION};
    use crate::model_resources::{owned_children_for_root, ChildRegistration};
    use std::process::Command;

    #[test]
    fn only_matching_load_terminals_settle_the_owned_reservation() {
        static NEXT_FIXTURE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let suffix = NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "voiceflow-asr-load-reservation-{}-{suffix}",
            std::process::id()
        ));
        let request: Request = serde_json::from_value(serde_json::json!({
            "protocol_version": PROTOCOL_VERSION,
            "request_id": "load-request",
            "session_id": "load-session",
            "model_version": "model-revision",
            "op": "load",
            "model_id": "model-id",
            "model_directory": "/fixture/model"
        }))
        .unwrap();
        let cases = [
            (
                "matching loaded",
                serde_json::json!({"status":"loaded","loaded_model_id":"model-id","loaded_model_version":"model-revision"}),
                true,
            ),
            (
                "matching error without loaded identity",
                serde_json::json!({"status":"error","loaded_model_id":null,"loaded_model_version":null}),
                true,
            ),
            (
                "stale request",
                serde_json::json!({"request_id":"stale","status":"loaded","loaded_model_id":"model-id","loaded_model_version":"model-revision"}),
                false,
            ),
            (
                "wrong session",
                serde_json::json!({"session_id":"other","status":"loaded","loaded_model_id":"model-id","loaded_model_version":"model-revision"}),
                false,
            ),
            (
                "wrong operation",
                serde_json::json!({"op":"status","status":"loaded","loaded_model_id":"model-id","loaded_model_version":"model-revision"}),
                false,
            ),
            (
                "wrong protocol",
                serde_json::json!({"protocol_version":PROTOCOL_VERSION + 1,"status":"loaded","loaded_model_id":"model-id","loaded_model_version":"model-revision"}),
                false,
            ),
            (
                "wrong echoed revision",
                serde_json::json!({"model_version":"other","status":"loaded","loaded_model_id":"model-id","loaded_model_version":"model-revision"}),
                false,
            ),
            (
                "wrong model",
                serde_json::json!({"status":"loaded","loaded_model_id":"other","loaded_model_version":"model-revision"}),
                false,
            ),
            (
                "wrong revision",
                serde_json::json!({"status":"loaded","loaded_model_id":"model-id","loaded_model_version":"other"}),
                false,
            ),
            (
                "unknown status",
                serde_json::json!({"status":"ready","loaded_model_id":"model-id","loaded_model_version":"model-revision"}),
                false,
            ),
            (
                "unloaded status",
                serde_json::json!({"status":"unloaded","loaded_model_id":"model-id","loaded_model_version":"model-revision"}),
                false,
            ),
            (
                "completed status",
                serde_json::json!({"status":"completed","loaded_model_id":"model-id","loaded_model_version":"model-revision"}),
                false,
            ),
            (
                "cancelled load status",
                serde_json::json!({"status":"cancelled"}),
                false,
            ),
            (
                "cancel acknowledgement",
                serde_json::json!({"op":"cancel","status":"cancelled"}),
                false,
            ),
        ];
        std::fs::create_dir_all(&root).unwrap();
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let mut registrations = Vec::new();
        let mut observations = Vec::new();
        for (index, (name, overrides, settles)) in cases.into_iter().enumerate() {
            let generation = index as u64 + 1;
            let registration = match ChildRegistration::register(child.id(), generation, &root) {
                Some(registration) => registration,
                None => {
                    observations.push((
                        name,
                        false,
                        None,
                        settles,
                        false,
                        false,
                        Some("child registration was rejected".to_owned()),
                    ));
                    break;
                }
            };
            let mut response_json = serde_json::json!({
                "protocol_version": PROTOCOL_VERSION,
                "request_id": "load-request",
                "session_id": "load-session",
                "model_version": "model-revision",
                "op": "load",
                "status": "loaded",
                "loaded_model_id": "model-id",
                "loaded_model_version": "model-revision"
            });
            for (key, value) in overrides.as_object().unwrap() {
                response_json[key] = value.clone();
            }
            let response: Response = match serde_json::from_value(response_json) {
                Ok(response) => response,
                Err(error) => {
                    registrations.push(registration);
                    observations.push((
                        name,
                        false,
                        None,
                        settles,
                        false,
                        false,
                        Some(error.to_string()),
                    ));
                    break;
                }
            };
            let did_settle = OnDeviceRuntime::settle_asr_load_reservation_if_terminal(
                registration.reserve_pending_load(),
                &request,
                &response,
                "model-id",
                "model-revision",
            );
            let load_successful = OnDeviceRuntime::asr_load_response_is_successful(
                &request,
                &response,
                "model-id",
                "model-revision",
            );
            let expected_load_successful = name == "matching loaded";
            let pending = owned_children_for_root(&root)
                .into_iter()
                .find(|(identity, _, _)| identity.generation == generation)
                .map(|record| record.2);
            observations.push((
                name,
                did_settle,
                pending,
                settles,
                load_successful,
                expected_load_successful,
                None,
            ));
            registrations.push(registration);
        }
        let _ = child.kill();
        let exit_observed = child.wait().is_ok();
        let registrations_retired = if exit_observed {
            for registration in registrations {
                registration.retire_after_exit();
            }
            let remaining = owned_children_for_root(&root);
            let retired = remaining.is_empty();
            std::fs::remove_dir_all(&root).unwrap();
            retired
        } else {
            false
        };
        assert!(exit_observed, "owned placeholder child exit observed");
        assert!(
            registrations_retired,
            "all registrations retired after exit"
        );
        for (
            name,
            did_settle,
            pending,
            expected_settles,
            load_successful,
            expected_load_successful,
            error,
        ) in observations
        {
            assert!(error.is_none(), "{name}: {error:?}");
            assert_eq!(did_settle, expected_settles, "{name} settle result");
            assert_eq!(
                pending,
                Some(!expected_settles),
                "{name} pending reservation"
            );
            assert_eq!(
                load_successful, expected_load_successful,
                "{name} load success"
            );
        }
    }
}
