//! Run-owned facts survive dropping any individual adapter future.
use super::{
    artifacts::AtomicArtifact,
    contracts::*,
    execution::{self, DispatchObserver, ExecutionError},
    validation::InputError,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
#[cfg(test)]
type CopyProgressHook = Arc<dyn Fn(u64) + Send + Sync>;

struct State {
    report: RunRecord,
    artifact: AtomicArtifact,
    checkpoint: Option<super::resume::CheckpointOwner>,
    replay: Option<Arc<super::resume::ReplayPlan>>,
    scoring_dirty: std::collections::BTreeSet<usize>,
}
pub(crate) struct RunJournal {
    path: PathBuf,
    state: Mutex<State>,
    #[cfg(test)]
    copy_hook: Mutex<Option<CopyProgressHook>>,
}
impl RunJournal {
    #[cfg(test)]
    pub(crate) fn new(path: &Path, report: RunRecord) -> Result<Arc<Self>, ExecutionError> {
        Self::new_with_replay(path, report, None)
    }
    pub(super) fn new_with_replay(
        path: &Path,
        mut report: RunRecord,
        plan: Option<Arc<super::resume::ReplayPlan>>,
    ) -> Result<Arc<Self>, ExecutionError> {
        if report.replay.is_none() {
            report.replay = plan.as_ref().and_then(|p| p.summary());
        }
        // Report ownership is independent of checkpoint acquisition. A failed
        // checkpoint parent must not bypass the terminal report attempt.
        let artifact =
            AtomicArtifact::new(path).map_err(|e| ExecutionError::artifact(path, e, 0))?;
        let scoring_dirty = (0..report.cases.len()).collect();
        let journal = Arc::new(Self {
            path: path.to_owned(),
            state: Mutex::new(State {
                report,
                artifact,
                checkpoint: None,
                replay: plan.clone(),
                scoring_dirty,
            }),
            #[cfg(test)]
            copy_hook: Mutex::new(None),
        });
        if let Some(plan) = plan.filter(|p| p.identity.speech().is_some()) {
            match super::resume::CheckpointOwner::new(plan) {
                Ok(checkpoint) => {
                    let mut state = journal.state();
                    if let Some(summary) = &mut state.report.replay {
                        summary.checkpoint_acquisition = checkpoint.acquisition();
                    }
                    state.checkpoint = Some(checkpoint);
                }
                Err(error) => {
                    {
                        let mut state = journal.state();
                        if let Some(summary) = &mut state.report.replay {
                            summary.checkpoint_acquisition = error.checkpoint_acquisition.clone();
                            summary.checkpoint_artifact_outcome = error.checkpoint_outcome.clone();
                        }
                    }
                    return Err(journal.fail(error, None));
                }
            }
        }
        if let Err(error) = journal.persist() {
            return Err(journal.fail(error, None));
        }
        Ok(journal)
    }
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
    fn persist_state(&self, state: &mut State) -> Result<(), ExecutionError> {
        for index in std::mem::take(&mut state.scoring_dirty) {
            super::scoring::refresh_row(&mut state.report.cases[index]);
        }
        super::scoring::refresh_summary(&mut state.report);
        if let Some(summary) = &mut state.report.replay {
            summary.fresh_asr_cases = state
                .report
                .cases
                .iter()
                .filter(|r| r.attempt_facts.iter().any(|a| a.stage == AttemptStage::Asr))
                .count() as u64;
            summary.fresh_cleanup_cases = state
                .report
                .cases
                .iter()
                .filter(|r| {
                    r.attempt_facts
                        .iter()
                        .any(|a| a.stage == AttemptStage::Cleanup)
                })
                .count() as u64;
            summary.replay_guard_rejected = state
                .report
                .cases
                .iter()
                .filter(|r| {
                    r.cleanup_replay
                        .as_ref()
                        .is_some_and(|c| matches!(c.current_guard, StageState::Failed { .. }))
                })
                .count() as u64;
            summary.failed_cases = state
                .report
                .cases
                .iter()
                .filter(|r| matches!(r.state, StageState::Failed { .. }))
                .count() as u64;
            summary.cancelled_cases = state
                .report
                .cases
                .iter()
                .filter(|r| matches!(r.state, StageState::Cancelled { .. }))
                .count() as u64;
            summary.unknown_cases = state
                .report
                .cases
                .iter()
                .filter(|r| matches!(r.state, StageState::Unknown { .. }))
                .count() as u64;
            summary.not_run_cases = state
                .report
                .cases
                .iter()
                .filter(|r| matches!(r.state, StageState::NotRun { .. }))
                .count() as u64;
        }
        // Each artifact retains its own bound content/durability tuple. A
        // checkpoint failure still permits a terminal report write; these are
        // separate atomic commits, never a fabricated multi-file transaction.
        let mut checkpoint_error = None;
        if let Some(checkpoint) = &mut state.checkpoint {
            checkpoint_error = checkpoint.persist(&state.report).err();
            if let Some(summary) = &mut state.report.replay {
                summary.checkpoint_artifact_outcome = checkpoint.outcome();
            }
        }
        let report_result = state
            .artifact
            .write_json_with_attempts(&state.report, Some(state.report.provider_attempts))
            .map_err(|failure| {
                ExecutionError::artifact(&self.path, failure, state.report.provider_attempts)
            });
        match (checkpoint_error, report_result) {
            (Some(mut failure), Ok(())) => {
                failure.persistence_confirmed(state.artifact.outcome());
                Err(failure)
            }
            (Some(mut failure), Err(report_failure)) => {
                failure.merge_persistence_failure(report_failure);
                Err(failure)
            }
            (None, Err(mut failure)) => {
                failure.checkpoint_outcome = state.checkpoint.as_ref().and_then(|c| c.outcome());
                Err(failure)
            }
            (None, Ok(())) => Ok(()),
        }
    }

    pub(crate) fn persist(&self) -> Result<(), ExecutionError> {
        self.persist_state(&mut self.state())
    }
    pub(crate) fn update(&self, index: usize, update: impl FnOnce(&mut CaseRecordV1)) {
        let mut state = self.state();
        update(&mut state.report.cases[index]);
        state.scoring_dirty.insert(index);
    }
    pub(crate) fn row(&self, index: usize) -> CaseRecordV1 {
        self.state().report.cases[index].clone()
    }
    pub(crate) fn capture_candidate(
        &self,
        inputs: &super::validation::ValidatedInputs,
        index: usize,
        stage: CacheStage,
        parent: Option<&str>,
    ) {
        let mut state = self.state();
        let row = state.report.cases[index].clone();
        let run = state.report.run_id.clone();
        if let Some(owner) = &mut state.checkpoint {
            owner.capture(inputs, &row, &run, stage, parent);
        }
    }
    pub(crate) fn replayed(&self, stage: CacheStage) {
        if let Some(summary) = &mut self.state().report.replay {
            match stage {
                CacheStage::Asr => summary.resumed_asr += 1,
                CacheStage::Cleanup => summary.resumed_cleanup += 1,
            }
        }
    }
    pub(crate) fn local_cleanup(&self, identity: super::local_cleanup::LocalResearchIdentity) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .report
            .local_cleanup = Some(identity);
    }
    pub(crate) fn setup(&self, update: impl FnOnce(&mut SetupRecord)) {
        let mut state = self.state();
        update(state.report.setup.get_or_insert_with(SetupRecord::new));
    }
    pub(crate) fn copy_progress(&self, index: usize, written: u64) {
        self.setup(|s| s.copies[index].written_bytes = written);
        #[cfg(test)]
        {
            let hook = self
                .copy_hook
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            if let Some(hook) = hook {
                hook(written);
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn on_copy_progress(&self, hook: CopyProgressHook) {
        *self.copy_hook.lock().unwrap_or_else(|p| p.into_inner()) = Some(hook);
    }
    pub(super) fn attach_workspace(&self, workspace: &super::local_asr::Workspace) {
        workspace.attach(&mut self.state().report);
    }
    pub(crate) fn terminal(&self, state: StageState) {
        self.state().report.terminal = state;
    }
    pub(crate) fn input_error(&self, cause: InputError) -> ExecutionError {
        ExecutionError::input(cause, self.state().report.provider_attempts)
    }
    pub(crate) fn fail(&self, mut error: ExecutionError, index: Option<usize>) -> ExecutionError {
        let mut state = self.state();
        error.provider_attempts = state.report.provider_attempts;
        error.setup = state.report.setup.clone();
        error.checkpoint_acquisition = state
            .report
            .replay
            .as_ref()
            .and_then(|summary| summary.checkpoint_acquisition.clone());
        state.report.terminal = error.terminal.clone();
        if let Some(index) = index.filter(|i| *i < state.report.cases.len()) {
            state.scoring_dirty.insert(index);
            if let Some(row) = state.report.cases.get_mut(index) {
                row.state = StageState::Failed {
                    reason: error.code.into(),
                };
                row.diagnostics.push(error.code.into());
                error.case_id = Some(row.case_id.clone());
            }
        }
        match self.persist_state(&mut state) {
            Ok(()) => {
                error.persistence_confirmed(state.artifact.outcome());
            }
            Err(failed) => {
                error.merge_persistence_failure(failed);
            }
        }
        if let Some(checkpoint) = &state.checkpoint {
            error.checkpoint_outcome = checkpoint.outcome();
        }
        error
    }
    pub(crate) fn case_count(&self) -> usize {
        self.state().report.cases.len()
    }
    pub(crate) fn code_identity(&self) -> String {
        self.state()
            .report
            .identity
            .baseline()
            .executable_sha256
            .clone()
    }
    pub(crate) fn finish_identity(
        &self,
        end: BaselineIdentity,
        out: &Path,
    ) -> Result<(), InputError> {
        let mut state = self.state();
        let baseline = state.report.identity.baseline_mut();
        let checkout_changed = baseline
            .live_checkout
            .as_ref()
            .zip(end.live_checkout.as_ref())
            .is_some_and(|(start, end)| {
                let content_changed = start
                    .facts
                    .source_sha256
                    .as_ref()
                    .zip(end.facts.source_sha256.as_ref())
                    .is_some_and(|(a, b)| a != b);
                let commit_changed = start
                    .facts
                    .source_commit
                    .as_ref()
                    .zip(end.facts.source_commit.as_ref())
                    .is_some_and(|(a, b)| a != b);
                content_changed || commit_changed
            });
        baseline.live_checkout_end = end.live_checkout;
        if baseline.executable_sha256 != end.executable_sha256 {
            return Err(super::validation::error(
                out,
                "executable_identity_changed",
                "evaluator executable path content changed during run",
            ));
        }
        if checkout_changed {
            return Err(super::validation::error(out, "source_identity_changed",
                "runtime checkout changed during run; compiled source facts remain those embedded in this artifact"));
        }
        Ok(())
    }
    pub(crate) fn into_report(self: Arc<Self>) -> Result<RunRecord, ExecutionError> {
        match Arc::try_unwrap(self) {
            Ok(owner) => Ok(owner
                .state
                .into_inner()
                .unwrap_or_else(|p| p.into_inner())
                .report),
            Err(owner) => Err(owner.input_error(super::validation::error(
                &owner.path,
                "observer_resources_still_owned",
                "run observer still owns resources",
            ))),
        }
    }
    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> RunRecord {
        self.state().report.clone()
    }
    pub(crate) fn observer(self: &Arc<Self>, index: usize) -> RowObserver {
        RowObserver {
            journal: Arc::clone(self),
            index,
        }
    }
}
pub(crate) struct RowObserver {
    journal: Arc<RunJournal>,
    index: usize,
}
impl DispatchObserver for RowObserver {
    fn started(
        &self,
        case_id: &str,
        stage: AttemptStage,
        ordinal: u32,
    ) -> Result<(), ExecutionError> {
        let mut state = self.journal.state();
        if let Some(replay) = &state.replay {
            replay
                .admit_boundary()
                .map_err(|cause| ExecutionError::input(cause, state.report.provider_attempts))?;
        }
        let row = &mut state.report.cases[self.index];
        if row.case_id != case_id {
            return Err(ExecutionError::input(
                super::validation::error(
                    &self.journal.path,
                    "attempt_identity_mismatch",
                    "dispatch case identity changed",
                ),
                state.report.provider_attempts,
            ));
        }
        state.scoring_dirty.insert(self.index);
        let row = &mut state.report.cases[self.index];
        row.attempts += 1;
        row.attempt_facts.push(execution::fact(stage, ordinal));
        row.state = StageState::Unknown {
            reason: "local_adapter_attempt_in_flight".into(),
        };
        state.report.provider_attempts += 1;
        state.report.terminal = StageState::Unknown {
            reason: "run_in_progress".into(),
        };
        // This is the local adapter future creation/start boundary, not proof of HTTP/server receipt.
        self.journal.persist_state(&mut state)?;
        if let Some(replay) = &state.replay {
            replay
                .admit_boundary()
                .map_err(|cause| ExecutionError::input(cause, state.report.provider_attempts))?;
        }
        Ok(())
    }
    fn completed(
        &self,
        case_id: &str,
        stage: AttemptStage,
        ordinal: u32,
    ) -> Result<(), ExecutionError> {
        let mut state = self.journal.state();
        let row = &mut state.report.cases[self.index];
        if row.case_id != case_id {
            return Err(ExecutionError::input(
                super::validation::error(
                    &self.journal.path,
                    "attempt_identity_mismatch",
                    "completion case identity changed",
                ),
                state.report.provider_attempts,
            ));
        }
        state.scoring_dirty.insert(self.index);
        let row = &mut state.report.cases[self.index];
        if let Some(fact) = row
            .attempt_facts
            .iter_mut()
            .rev()
            .find(|fact| fact.stage == stage && fact.ordinal == ordinal)
        {
            fact.local_adapter = StageState::Completed {};
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{fixtures::FixtureDirectory, validation::sha256};
    use super::*;
    #[test]
    fn checkout_changes_are_recorded_without_relabeling_finalizer_code_or_compiled_source() {
        let d = FixtureDirectory::new();
        let (_, journal) = super::super::tests::journal_fixture(&d);
        std::fs::create_dir_all(d.0.join("src-tauri/src")).unwrap();
        std::fs::write(d.0.join("src-tauri/src/lib.rs"), b"pub fn before() {}\n").unwrap();
        std::fs::write(d.0.join("package-lock.json"), b"fixture node lock").unwrap();
        std::fs::write(d.0.join("src-tauri/Cargo.lock"), b"fixture cargo lock").unwrap();
        let start = super::super::source_identity::capture(&d.0);
        let observation = |facts| super::super::source_identity::CheckoutObservation {
            path: d.0.clone(),
            canonical_path: Some(d.0.clone()),
            content_comparison: super::super::source_identity::CheckoutComparison::Unknown {
                reason: "synthetic build fixture".into(),
            },
            facts,
        };
        journal.state().report.identity.baseline_mut().live_checkout = Some(observation(start));
        let initial = journal.snapshot().identity.baseline().clone();
        let code = journal.code_identity();
        assert_eq!(code, initial.executable_sha256);
        std::fs::write(d.0.join("src-tauri/src/lib.rs"), b"pub fn after() {}\n").unwrap();
        let mut end = initial.clone();
        end.live_checkout = Some(observation(super::super::source_identity::capture(&d.0)));
        assert_eq!(
            journal
                .finish_identity(end, &d.0.join("run.json"))
                .unwrap_err()
                .code,
            "source_identity_changed"
        );
        let recorded = journal.snapshot().identity.baseline().clone();
        assert_eq!(recorded.source_sha256, initial.source_sha256);
        assert_eq!(journal.code_identity(), code);
        assert_ne!(
            recorded.live_checkout.unwrap().facts.source_sha256,
            recorded.live_checkout_end.unwrap().facts.source_sha256
        );
    }
    #[test]
    fn unknown_baseline_checkout_preserves_artifact_identity_but_changed_executable_rejects() {
        let d = FixtureDirectory::new();
        let (_, journal) = super::super::tests::journal_fixture(&d);
        let mut end = journal.snapshot().identity.baseline().clone();
        assert!(journal
            .finish_identity(end.clone(), &d.0.join("run.json"))
            .is_ok());
        end.executable_sha256 = sha256(b"different artifact at executable path");
        assert_eq!(
            journal
                .finish_identity(end, &d.0.join("run.json"))
                .unwrap_err()
                .code,
            "executable_identity_changed"
        );
    }
    fn assert_current_binding(error: &ExecutionError, path: &Path, attempts: u64, confirmed: bool) {
        let bytes = std::fs::read(path).unwrap();
        let actual: RunRecord = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(actual.provider_attempts, attempts);
        let current = error.artifact_outcome.current.as_ref().unwrap();
        assert_eq!(current.sha256, sha256(&bytes));
        assert_eq!(current.provider_attempts, Some(attempts));
        assert_eq!(
            current.applicable_path.as_deref(),
            Some(path.to_str().unwrap())
        );
        assert!(matches!(current.file_sync, StageState::Completed {}));
        assert_eq!(
            matches!(current.directory_sync, StageState::Completed {}),
            confirmed
        );
    }
    #[test]
    fn installed_checkpoint_cleanup_failure_keeps_current_and_last_confirmed_tuples_distinct() {
        let d = FixtureDirectory::new();
        let (_, journal) = super::super::tests::journal_fixture(&d);
        let path = d.0.join("run.json");
        let previously_confirmed = journal.state().artifact.outcome().last_confirmed.unwrap();
        journal.state().artifact.faults.prior_cleanup = true;
        let failure = journal
            .observer(0)
            .started("one", AttemptStage::Asr, 1)
            .unwrap_err();
        assert_eq!(failure.provider_attempts, 1);
        assert_current_binding(&failure, &path, 1, false);
        let previous = failure.artifact_outcome.last_confirmed.as_ref().unwrap();
        assert_eq!(previous.sha256, previously_confirmed.sha256);
        assert_eq!(previous.file_identity, previously_confirmed.file_identity);
        assert_eq!(previous.provider_attempts, Some(0));
        assert!(matches!(previous.directory_sync, StageState::Completed {}));
        assert!(previous.applicable_path.is_none());
        assert!(failure.durable_record.is_none());
        assert_eq!(failure.durable_provider_attempts, 0);
        assert!(failure.artifact_failures[0].committed);
        let temp = path.with_extension(format!("tmp-{}", std::process::id()));
        let retained_bytes = std::fs::read(&temp).unwrap();
        assert_eq!(sha256(&retained_bytes), previous.sha256);
        // fail's retry collides with the retained prior record; it must not mix
        // the newer applicable file with the previous confirmed count.
        let retry = journal.fail(failure, Some(0));
        assert_current_binding(&retry, &path, 1, false);
        assert!(retry.durable_record.is_none());
        assert_eq!(retry.durable_provider_attempts, 0);
        assert_eq!(std::fs::read(temp).unwrap(), retained_bytes);
        assert_eq!(
            retry
                .artifact_outcome
                .last_confirmed
                .as_ref()
                .unwrap()
                .sha256,
            previously_confirmed.sha256
        );
        assert_eq!(retry.artifact_failures.len(), 2);
    }
    #[test]
    fn directory_sync_failure_then_successful_retry_binds_each_content_to_its_own_confirmation() {
        let d = FixtureDirectory::new();
        let (_, journal) = super::super::tests::journal_fixture(&d);
        let path = d.0.join("run.json");
        let old_sha = journal
            .state()
            .artifact
            .outcome()
            .last_confirmed
            .unwrap()
            .sha256;
        journal.state().artifact.faults.directory_sync = true;
        let failure = journal
            .observer(0)
            .started("one", AttemptStage::Cleanup, 1)
            .unwrap_err();
        assert_eq!(failure.provider_attempts, 1);
        assert_current_binding(&failure, &path, 1, false);
        assert!(failure.durable_record.is_none());
        assert_eq!(
            failure
                .artifact_outcome
                .last_confirmed
                .as_ref()
                .unwrap()
                .sha256,
            old_sha
        );
        assert!(!path
            .with_extension(format!("tmp-{}", std::process::id()))
            .exists());
        // This retry installs and directory-syncs the failed terminal record.
        let repaired = journal.fail(failure, Some(0));
        assert_current_binding(&repaired, &path, 1, true);
        assert_eq!(
            repaired.durable_record.as_deref(),
            Some(path.to_str().unwrap())
        );
        assert_eq!(repaired.durable_provider_attempts, 1);
        let confirmed = repaired.artifact_outcome.last_confirmed.as_ref().unwrap();
        let current = repaired.artifact_outcome.current.as_ref().unwrap();
        assert_eq!(confirmed.sha256, current.sha256);
        assert_eq!(confirmed.file_identity, current.file_identity);
        assert_eq!(confirmed.provider_attempts, Some(1));
        // The original failed outcome remains evidence of the prior unconfirmed install.
        assert_eq!(
            repaired.artifact_failures[0]
                .outcome
                .current
                .as_ref()
                .unwrap()
                .provider_attempts,
            Some(1)
        );
        assert!(matches!(
            repaired.artifact_failures[0]
                .outcome
                .current
                .as_ref()
                .unwrap()
                .directory_sync,
            StageState::Unknown { .. }
        ));
    }
}
