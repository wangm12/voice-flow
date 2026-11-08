use super::{
    artifacts::{ArtifactFailure, ArtifactOutcome},
    contracts::{AttemptFact, AttemptStage, StageState},
    validation::InputError,
};
use serde::Serialize;
#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[error("{code}: {safe_detail} ({artifact_path})")]
pub struct ExecutionErrorFacts {
    pub code: &'static str,
    pub artifact_path: String,
    pub case_id: Option<String>,
    pub safe_detail: String,
    /// Local adapter attempt starts, not proven HTTP submission or server receipt.
    pub provider_attempts: u64,
    pub setup: Option<super::contracts::SetupRecord>,
    pub terminal: StageState,
    pub artifact_outcome: ArtifactOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_outcome: Option<ArtifactOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_acquisition: Option<super::contracts::CheckpointAcquisition>,
    pub durable_record: Option<String>,
    pub persistence: StageState,
    pub durable_provider_attempts: u64,
    pub artifact_failures: Vec<ArtifactFailure>,
    pub transport_submissions: Option<u64>,
    pub server_received: Option<u64>,
}
/// Keep rich failure facts owned together without inflating every Result/async frame.
/// Transparent serde preserves the public flat error object.
#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[serde(transparent)]
#[error(transparent)]
pub struct ExecutionError(Box<ExecutionErrorFacts>);
impl std::ops::Deref for ExecutionError {
    type Target = ExecutionErrorFacts;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for ExecutionError {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl ExecutionError {
    pub(crate) fn artifact(
        path: &std::path::Path,
        failure: ArtifactFailure,
        attempts: u64,
    ) -> Self {
        let outcome = failure.outcome.clone();
        let mut error = Self(Box::new(ExecutionErrorFacts {
            code: "report_error",
            artifact_path: path.display().to_string(),
            case_id: None,
            safe_detail: failure.reason.clone(),
            provider_attempts: attempts,
            setup: None,
            terminal: StageState::Failed {
                reason: "report_error".into(),
            },
            artifact_outcome: ArtifactOutcome::default(),
            checkpoint_outcome: None,
            checkpoint_acquisition: None,
            durable_record: None,
            persistence: StageState::Unknown {
                reason: "artifact_write_failed".into(),
            },
            durable_provider_attempts: 0,
            artifact_failures: vec![failure],
            transport_submissions: None,
            server_received: None,
        }));
        error.bind_outcome(outcome);
        error
    }
    pub(crate) fn input(cause: InputError, attempts: u64) -> Self {
        Self(Box::new(ExecutionErrorFacts {
            code: cause.code,
            artifact_path: cause.artifact_path,
            case_id: cause.case_id,
            safe_detail: cause.safe_detail,
            provider_attempts: attempts,
            setup: None,
            terminal: StageState::Failed {
                reason: cause.code.into(),
            },
            artifact_outcome: ArtifactOutcome::default(),
            checkpoint_outcome: None,
            checkpoint_acquisition: None,
            durable_record: None,
            persistence: StageState::NotRun {},
            durable_provider_attempts: 0,
            artifact_failures: vec![],
            transport_submissions: None,
            server_received: None,
        }))
    }
    fn bind_outcome(&mut self, outcome: ArtifactOutcome) {
        self.durable_record = outcome
            .last_confirmed
            .as_ref()
            .and_then(|fact| fact.applicable_path.clone());
        self.durable_provider_attempts = outcome
            .last_confirmed
            .as_ref()
            .and_then(|fact| fact.provider_attempts)
            .unwrap_or(0);
        self.artifact_outcome = outcome;
    }
    pub(crate) fn persistence_confirmed(&mut self, outcome: ArtifactOutcome) {
        self.bind_outcome(outcome);
        self.persistence = StageState::Completed {};
    }
    pub(crate) fn merge_persistence_failure(&mut self, failure: Self) {
        let failure = *failure.0;
        self.artifact_failures.extend(failure.artifact_failures);
        if failure.checkpoint_acquisition.is_some() {
            self.checkpoint_acquisition = failure.checkpoint_acquisition;
        }
        if failure.checkpoint_outcome.is_some() {
            self.checkpoint_outcome = failure.checkpoint_outcome;
        }
        self.artifact_outcome = failure.artifact_outcome;
        self.durable_record = failure.durable_record;
        self.durable_provider_attempts = failure.durable_provider_attempts;
        self.persistence = failure.persistence;
    }
}
#[derive(Debug, Serialize, thiserror::Error)]
#[serde(untagged)]
pub enum EvaluationError {
    #[error(transparent)]
    Input(#[from] InputError),
    #[error(transparent)]
    Execution(#[from] ExecutionError),
}
impl EvaluationError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Input(_) => 2,
            Self::Execution(e) if matches!(e.terminal, StageState::Cancelled { .. }) => 130,
            Self::Execution(e) if e.provider_attempts == 0 => 2,
            Self::Execution(_) => 1,
        }
    }
    pub fn code(&self) -> &'static str {
        match self {
            Self::Input(e) => e.code,
            Self::Execution(e) => e.code,
        }
    }
    pub fn provider_attempts(&self) -> u64 {
        match self {
            Self::Input(e) => e.provider_attempts,
            Self::Execution(e) => e.provider_attempts,
        }
    }
}
/// Called by the shared scheduling/dispatch owner, after creating the actual
/// adapter future and before polling it. Completion is independent of stage success.
pub(crate) trait DispatchObserver: Send + Sync {
    fn started(
        &self,
        case_id: &str,
        stage: AttemptStage,
        ordinal: u32,
    ) -> Result<(), ExecutionError>;
    fn completed(
        &self,
        case_id: &str,
        stage: AttemptStage,
        ordinal: u32,
    ) -> Result<(), ExecutionError>;
}
pub(crate) fn fact(stage: AttemptStage, ordinal: u32) -> AttemptFact {
    AttemptFact {
        stage,
        ordinal,
        local_adapter: StageState::Unknown {
            reason: "adapter_future_created_in_flight".into(),
        },
        transport_submission: StageState::Unknown {
            reason: "transport_boundary_not_observed".into(),
        },
        server_received: StageState::Unknown {
            reason: "server_receipt_not_observed".into(),
        },
        cache_source: "fresh".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boxed_execution_error_preserves_flat_serialization_and_partial_failure_facts() {
        let mut error = ExecutionError::input(
            super::super::validation::error(
                std::path::Path::new("fixture.wav"),
                "io",
                "fixture read failed",
            ),
            3,
        );
        error.case_id = Some("case-1".into());
        let confirmed = super::super::artifacts::ArtifactSnapshot {
            selected_path: "run.json".into(),
            applicable_path: Some("run.json".into()),
            file_identity: super::super::artifacts::ArtifactFileIdentity {
                device: 1,
                inode: 2,
            },
            sha256: "0".repeat(64),
            provider_attempts: Some(2),
            file_sync: StageState::Completed {},
            directory_sync: StageState::Completed {},
            path_applicability: StageState::Completed {},
        };
        let outcome = ArtifactOutcome {
            current: Some(Box::new(confirmed.clone())),
            last_confirmed: Some(Box::new(confirmed)),
        };
        let failure = ArtifactFailure {
            reason: "fixture temporary cleanup failed".into(),
            committed: false,
            outcome: outcome.clone(),
            retained: vec![super::super::artifacts::RetainedArtifact {
                path: "run.tmp-fixture".into(),
                ownership: "owned_handle_retained_path_unknown".into(),
                reason: "fixture identity changed".into(),
            }],
        };
        let persistence = ExecutionError::artifact(std::path::Path::new("run.json"), failure, 3);
        error.merge_persistence_failure(persistence);
        let expected = serde_json::json!({
            "code":"io", "artifact_path":"fixture.wav", "case_id":"case-1", "safe_detail":"fixture read failed",
            "provider_attempts":3, "setup":null, "terminal":{"state":"failed","reason":"io"}, "durable_record":"run.json",
            "persistence":{"state":"unknown","reason":"artifact_write_failed"}, "durable_provider_attempts":2,
            "artifact_outcome":outcome,
            "artifact_failures":[{"reason":"fixture temporary cleanup failed","committed":false,"outcome":outcome,"retained":[{"path":"run.tmp-fixture","ownership":"owned_handle_retained_path_unknown","reason":"fixture identity changed"}]}],
            "transport_submissions":null, "server_received":null,
        });
        assert_eq!(serde_json::to_value(&error).unwrap(), expected);
        assert_eq!(error.to_string(), "io: fixture read failed (fixture.wav)");
        let public_error = EvaluationError::from(error.clone());
        assert_eq!(serde_json::to_value(&public_error).unwrap(), expected);
        assert_eq!(public_error.provider_attempts(), 3);
        assert_eq!(public_error.exit_code(), 1);

        // Setup is an intentional additive flat member, both when absent and
        // when retained alongside a later execution/persistence failure.
        let mut setup = super::super::contracts::SetupRecord::new();
        setup.materialization = StageState::Completed {};
        setup.credentials = StageState::Completed {};
        setup.worker_join = StageState::Completed {};
        setup.workspace = Some(super::super::contracts::SetupWorkspace {
            selected_path: "fixture-work".into(),
            held_identity: Some(super::super::artifacts::ArtifactFileIdentity {
                device: 7,
                inode: 8,
            }),
            creation: StageState::Completed {},
            path_applicable: None,
            retained: Some(true),
        });
        setup.copies.push(super::super::contracts::SetupCopy {
            model_id: "fixture-model".into(),
            relative_path: "model.safetensors".into(),
            expected_bytes: 4,
            written_bytes: 4,
            state: StageState::Completed {},
        });
        error.setup = Some(setup);
        let mut expected_with_setup = expected.clone();
        expected_with_setup["setup"] = serde_json::json!({
            "materialization":{"state":"completed"}, "credentials":{"state":"completed"},
            "worker_join":{"state":"completed"}, "cancellation_requested":false,
            "workspace":{"selected_path":"fixture-work","held_identity":{"device":7,"inode":8},"creation":{"state":"completed"},"path_applicable":null,"retained":true},
            "copies":[{"model_id":"fixture-model","relative_path":"model.safetensors","expected_bytes":4,"written_bytes":4,"state":{"state":"completed"}}],
        });
        assert_eq!(serde_json::to_value(&error).unwrap(), expected_with_setup);
        let public_error = EvaluationError::from(error);
        assert_eq!(
            serde_json::to_value(&public_error).unwrap(),
            expected_with_setup
        );
        assert_eq!(public_error.provider_attempts(), 3);
        assert_eq!(public_error.exit_code(), 1);
    }
}
