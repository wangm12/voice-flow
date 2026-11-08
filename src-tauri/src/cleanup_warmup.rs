//! Coordinates one bounded cleanup warmup at a time.
//!
//! The coordinator owns only scheduling identity and cancellation. Model and
//! prompt admission remain with the existing cleanup runtime in `lib.rs`.

use std::sync::{Mutex, MutexGuard};
use tokio_util::sync::CancellationToken;

const MAX_FOREGROUND_RETRIES: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WarmupEligibility {
    pub(crate) isolated_launch: bool,
    pub(crate) onboarded: bool,
    pub(crate) cleanup_enabled: bool,
    pub(crate) native_provider_selected: bool,
    pub(crate) model_available: bool,
}

impl WarmupEligibility {
    pub(crate) fn admitted(self) -> bool {
        !self.isolated_launch
            && self.onboarded
            && self.cleanup_enabled
            && self.native_provider_selected
            && self.model_available
    }
}

struct Inner<K> {
    generation: u64,
    requested: Option<K>,
    active: Option<(u64, CancellationToken)>,
    runner_active: bool,
    foreground_model_demands: usize,
    foreground_retries: u8,
    completed: Option<K>,
}

pub(crate) struct Coordinator<K> {
    inner: Mutex<Inner<K>>,
}

pub(crate) fn request_if_admitted<K: Clone + Eq>(
    coordinator: &Coordinator<K>,
    eligibility: WarmupEligibility,
    key: K,
) -> bool {
    eligibility.admitted() && coordinator.request(key)
}

pub(crate) fn may_start_runtime_work(
    operation_idle: bool,
    dictation_idle: bool,
    foreground_model_demand: bool,
) -> bool {
    operation_idle && dictation_idle && !foreground_model_demand
}

#[derive(Clone)]
pub(crate) struct Attempt<K> {
    pub(crate) generation: u64,
    pub(crate) key: K,
    pub(crate) cancellation: CancellationToken,
}

pub(crate) struct ModelDemandGuard<'a, K: Clone + Eq> {
    coordinator: &'a Coordinator<K>,
}

impl<K: Clone + Eq> Drop for ModelDemandGuard<'_, K> {
    fn drop(&mut self) {
        let mut inner = self.coordinator.lock();
        inner.foreground_model_demands = inner.foreground_model_demands.saturating_sub(1);
    }
}

pub(crate) struct StepProgress<K> {
    key: Option<K>,
    child_generation: Option<u64>,
    completed_steps: usize,
}

impl<K> Default for StepProgress<K> {
    fn default() -> Self {
        Self {
            key: None,
            child_generation: None,
            completed_steps: 0,
        }
    }
}

impl<K: Clone + Eq> StepProgress<K> {
    pub(crate) fn bind_key(&mut self, key: &K) {
        if self.key.as_ref() != Some(key) {
            self.key = Some(key.clone());
            self.child_generation = None;
            self.completed_steps = 0;
        }
    }

    /// Binds progress to the child that owns its in-memory prompt cache.
    /// Returns false and resets completed work when the child changes.
    pub(crate) fn bind_child(&mut self, child_generation: u64) -> bool {
        if self
            .child_generation
            .is_some_and(|previous| previous != child_generation)
        {
            self.child_generation = Some(child_generation);
            self.completed_steps = 0;
            return false;
        }
        self.child_generation = Some(child_generation);
        true
    }

    pub(crate) fn completed_steps(&self) -> usize {
        self.completed_steps
    }

    pub(crate) fn mark_step_completed(&mut self) {
        self.completed_steps = self.completed_steps.saturating_add(1);
    }

    pub(crate) fn record_completed_step(&mut self, child_generation: u64) -> bool {
        if !self.bind_child(child_generation) {
            return false;
        }
        self.mark_step_completed();
        true
    }

    pub(crate) fn retain_surviving_child(&mut self, child_generation: u64) -> bool {
        self.bind_child(child_generation)
    }

    /// A true runtime retirement clears progress; a surviving-child receipt
    /// preserves completed work because the cache still belongs to that child.
    pub(crate) fn child_retired(&mut self) {
        self.child_generation = None;
        self.completed_steps = 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttemptOutcome {
    Completed,
    RetryWhenIdle,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunnerAction {
    Continue,
    Stop,
}

impl<K> Default for Coordinator<K> {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                generation: 0,
                requested: None,
                active: None,
                runner_active: false,
                foreground_model_demands: 0,
                foreground_retries: 0,
                completed: None,
            }),
        }
    }
}

impl<K: Clone + Eq> Coordinator<K> {
    fn lock(&self) -> MutexGuard<'_, Inner<K>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Returns true only when the caller must start the single runner.
    pub(crate) fn request(&self, key: K) -> bool {
        let mut inner = self.lock();
        if inner.requested.as_ref() == Some(&key) {
            return false;
        }
        if inner.completed.as_ref() == Some(&key) {
            if inner.requested.is_some() {
                inner.generation = inner.generation.wrapping_add(1);
                inner.requested = None;
                inner.foreground_retries = 0;
                if let Some((_, cancellation)) = &inner.active {
                    cancellation.cancel();
                }
            }
            return false;
        }

        inner.generation = inner.generation.wrapping_add(1);
        inner.requested = Some(key);
        inner.foreground_retries = 0;
        if let Some((_, cancellation)) = &inner.active {
            cancellation.cancel();
        }
        if inner.runner_active {
            false
        } else {
            inner.runner_active = true;
            true
        }
    }

    /// Invalidates the pending key and cancels its active attempt.
    pub(crate) fn invalidate(&self) {
        let mut inner = self.lock();
        inner.generation = inner.generation.wrapping_add(1);
        inner.requested = None;
        inner.foreground_retries = 0;
        if let Some((_, cancellation)) = &inner.active {
            cancellation.cancel();
        }
    }

    /// Invalidates only a matching request so a stale preflight cannot cancel
    /// a newer settings or model generation.
    #[cfg(test)]
    pub(crate) fn invalidate_if_requested(&self, key: &K) {
        let mut inner = self.lock();
        if inner.requested.as_ref() != Some(key) {
            return;
        }
        inner.generation = inner.generation.wrapping_add(1);
        inner.requested = None;
        inner.foreground_retries = 0;
        if let Some((_, cancellation)) = &inner.active {
            cancellation.cancel();
        }
    }

    /// Records actual foreground model demand. Capture-only activity must not
    /// call this; the warmup child may remain useful while audio is recording.
    pub(crate) fn begin_model_demand(&self) -> ModelDemandGuard<'_, K> {
        let mut inner = self.lock();
        inner.foreground_model_demands = inner.foreground_model_demands.saturating_add(1);
        ModelDemandGuard { coordinator: self }
    }

    pub(crate) fn has_model_demand(&self) -> bool {
        self.lock().foreground_model_demands != 0
    }

    pub(crate) fn begin_attempt(&self) -> Option<Attempt<K>> {
        let mut inner = self.lock();
        let Some(key) = inner.requested.clone() else {
            inner.runner_active = false;
            return None;
        };
        if inner.completed.as_ref() == Some(&key) {
            inner.requested = None;
            inner.runner_active = false;
            return None;
        }
        let cancellation = CancellationToken::new();
        let generation = inner.generation;
        inner.active = Some((generation, cancellation.clone()));
        Some(Attempt {
            generation,
            key,
            cancellation,
        })
    }

    pub(crate) fn is_current(&self, attempt: &Attempt<K>) -> bool {
        let inner = self.lock();
        inner.generation == attempt.generation
            && inner.requested.as_ref() == Some(&attempt.key)
            && inner
                .active
                .as_ref()
                .is_some_and(|(generation, cancellation)| {
                    *generation == attempt.generation && !cancellation.is_cancelled()
                })
    }

    pub(crate) fn finish(&self, attempt: &Attempt<K>, outcome: AttemptOutcome) -> RunnerAction {
        let mut inner = self.lock();
        if inner
            .active
            .as_ref()
            .is_some_and(|(generation, _)| *generation == attempt.generation)
        {
            inner.active = None;
        }
        let current = inner.generation == attempt.generation
            && inner.requested.as_ref() == Some(&attempt.key);
        if !current {
            if inner.requested.is_none() {
                inner.runner_active = false;
                return RunnerAction::Stop;
            }
            return RunnerAction::Continue;
        }
        match outcome {
            AttemptOutcome::Completed => {
                inner.completed = Some(attempt.key.clone());
                inner.requested = None;
                inner.runner_active = false;
                inner.foreground_retries = 0;
                RunnerAction::Stop
            }
            AttemptOutcome::RetryWhenIdle => {
                if inner.foreground_retries < MAX_FOREGROUND_RETRIES {
                    inner.foreground_retries += 1;
                    RunnerAction::Continue
                } else {
                    inner.requested = None;
                    inner.runner_active = false;
                    RunnerAction::Stop
                }
            }
            AttemptOutcome::Failed => {
                inner.requested = None;
                inner.runner_active = false;
                inner.foreground_retries = 0;
                RunnerAction::Stop
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        may_start_runtime_work, AttemptOutcome, Coordinator, RunnerAction, StepProgress,
        WarmupEligibility,
    };
    use std::time::Duration;

    #[test]
    fn model_demand_defers_next_step_without_cancelling_active_attempt() {
        let coordinator = Coordinator::default();
        let key = ("settings", "revision", 1_u64, 2_u64);
        assert!(coordinator.request(key));
        let attempt = coordinator.begin_attempt().expect("attempt starts");
        let first = coordinator.begin_model_demand();
        let second = coordinator.begin_model_demand();
        assert!(!attempt.cancellation.is_cancelled());
        assert!(coordinator.is_current(&attempt));
        assert!(!may_start_runtime_work(
            true,
            true,
            coordinator.has_model_demand()
        ));
        drop(first);
        assert!(coordinator.has_model_demand());
        assert!(!attempt.cancellation.is_cancelled());
        drop(second);
        assert!(!coordinator.has_model_demand());
        assert!(coordinator.is_current(&attempt));
        assert!(may_start_runtime_work(
            true,
            true,
            coordinator.has_model_demand()
        ));
    }

    #[test]
    fn first_request_waits_for_onboarding_cleanup_and_verified_model() {
        let coordinator = Coordinator::default();
        let key = ("native cleanup", "pinned revision", 1_u64);
        let mut eligibility = WarmupEligibility {
            isolated_launch: false,
            onboarded: false,
            cleanup_enabled: false,
            native_provider_selected: false,
            model_available: false,
        };
        assert!(!super::request_if_admitted(&coordinator, eligibility, key));

        eligibility.cleanup_enabled = true;
        eligibility.native_provider_selected = true;
        assert!(!super::request_if_admitted(&coordinator, eligibility, key));
        eligibility.onboarded = true;
        assert!(!super::request_if_admitted(&coordinator, eligibility, key));
        eligibility.model_available = true;
        assert!(super::request_if_admitted(&coordinator, eligibility, key));
        assert!(!super::request_if_admitted(&coordinator, eligibility, key));
        eligibility.isolated_launch = true;
        assert!(!super::request_if_admitted(&coordinator, eligibility, key));
    }

    #[test]
    fn same_signature_and_model_generation_coalesce_without_duplicate_runner() {
        let coordinator = Coordinator::default();
        let key = ("native cleanup settings", "pinned-revision", 7_u64);
        assert!(coordinator.request(key));
        let attempt = coordinator.begin_attempt().expect("one attempt starts");
        assert!(!coordinator.request(("native cleanup settings", "pinned-revision", 7)));
        assert!(!coordinator.request(("native cleanup settings", "pinned-revision", 7)));
        assert!(coordinator.is_current(&attempt));
        assert_eq!(
            coordinator.finish(&attempt, AttemptOutcome::Completed),
            RunnerAction::Stop
        );
        assert!(!coordinator.request(("native cleanup settings", "pinned-revision", 7)));
    }

    #[test]
    fn reserved_runner_handoff_drains_latest_request_after_preflight_invalidation() {
        let coordinator = Coordinator::default();
        let eligible = WarmupEligibility {
            isolated_launch: false,
            onboarded: true,
            cleanup_enabled: true,
            native_provider_selected: true,
            model_available: true,
        };

        // A reserves the single runner, then a model transition invalidates
        // A before the caller hands that reservation to run_cleanup_warmup.
        assert!(super::request_if_admitted(
            &coordinator,
            eligible,
            ("settings-a", "revision", 1_u64, 3_u64),
        ));
        coordinator.invalidate();

        // B coalesces into the still-owned reservation; the handed-off runner
        // must drain B instead of being stranded by A's stale preflight.
        assert!(!super::request_if_admitted(
            &coordinator,
            eligible,
            ("settings-b", "revision", 2_u64, 4_u64),
        ));
        let attempt = coordinator
            .begin_attempt()
            .expect("reserved runner drains B");
        assert_eq!(attempt.key.0, "settings-b");
        assert_eq!(
            coordinator.finish(&attempt, AttemptOutcome::Completed),
            RunnerAction::Stop
        );
        assert!(coordinator.begin_attempt().is_none());
    }

    #[test]
    fn active_capture_defers_new_steps_without_cancelling_the_current_warmup() {
        let coordinator = Coordinator::default();
        assert!(coordinator.request(("settings", "revision", 1_u64, 2_u64)));
        let attempt = coordinator.begin_attempt().unwrap();

        // Dictation start is a busy condition for the next warmup step, but
        // it must leave already useful runtime work alive.
        assert!(!may_start_runtime_work(false, false, false));
        assert!(!attempt.cancellation.is_cancelled());

        // Once capture has gone idle, an outstanding native cleanup request
        // still has priority and defers any further warmup admission.
        let demand = coordinator.begin_model_demand();
        assert!(!attempt.cancellation.is_cancelled());
        assert!(!may_start_runtime_work(
            true,
            true,
            coordinator.has_model_demand()
        ));
        drop(demand);
        assert!(may_start_runtime_work(
            true,
            true,
            coordinator.has_model_demand()
        ));
    }

    #[test]
    fn rapid_relevant_changes_cancel_old_work_and_keep_only_latest_key() {
        let coordinator = Coordinator::default();
        assert!(coordinator.request(("settings-a", "revision", 3_u64)));
        let old = coordinator.begin_attempt().unwrap();
        assert!(!coordinator.request(("settings-b", "revision", 3)));
        assert!(!coordinator.request(("settings-c", "revision", 3)));
        assert!(old.cancellation.is_cancelled());
        assert!(!coordinator.is_current(&old));
        coordinator.invalidate_if_requested(&("settings-a", "revision", 3));
        assert_eq!(
            coordinator.finish(&old, AttemptOutcome::RetryWhenIdle),
            RunnerAction::Continue
        );
        let latest = coordinator.begin_attempt().expect("latest key continues");
        assert_eq!(latest.key, ("settings-c", "revision", 3));
    }

    #[test]
    fn disabling_or_deleting_invalidates_inflight_attempt() {
        let coordinator = Coordinator::default();
        assert!(coordinator.request(("enabled", "revision", 11_u64)));
        let attempt = coordinator.begin_attempt().unwrap();
        coordinator.invalidate();
        assert!(attempt.cancellation.is_cancelled());
        assert!(!coordinator.is_current(&attempt));
        assert_eq!(
            coordinator.finish(&attempt, AttemptOutcome::Failed),
            RunnerAction::Stop
        );
        assert!(coordinator.begin_attempt().is_none());

        assert!(coordinator.request(("enabled", "revision", 12)));
        let restored_model = coordinator.begin_attempt().unwrap();
        assert_ne!(restored_model.key.2, attempt.key.2);
    }

    #[test]
    fn processing_generation_change_cancels_stale_attempt_and_binds_latest_key() {
        let coordinator = Coordinator::default();
        assert!(coordinator.request(("same warmup signature", "revision", 4_u64, 10_u64)));
        let stale = coordinator.begin_attempt().unwrap();
        assert!(!coordinator.request(("same warmup signature", "revision", 4, 11)));
        assert!(stale.cancellation.is_cancelled());
        assert!(!coordinator.is_current(&stale));
        assert_eq!(
            coordinator.finish(&stale, AttemptOutcome::Failed),
            RunnerAction::Continue
        );
        let current = coordinator
            .begin_attempt()
            .expect("new generation continues");
        assert_eq!(current.key.3, 11);
    }

    #[test]
    fn unrelated_edits_keep_the_same_key_and_do_not_duplicate_work() {
        let coordinator = Coordinator::default();
        let key = ("prompt-affecting signature", "revision", 2_u64);
        assert!(coordinator.request(key));
        let active = coordinator.begin_attempt().unwrap();
        // An unrelated theme or tray edit does not enter this key.
        assert!(!coordinator.request(("prompt-affecting signature", "revision", 2)));
        assert!(!active.cancellation.is_cancelled());
        assert!(coordinator.is_current(&active));
    }

    #[test]
    fn foreground_retries_are_bounded_per_signature() {
        let coordinator = Coordinator::default();
        assert!(coordinator.request(("settings", "revision", 1_u64)));
        for retry in 0..=2 {
            let attempt = coordinator
                .begin_attempt()
                .expect("attempt remains bounded");
            let expected = if retry < 2 {
                RunnerAction::Continue
            } else {
                RunnerAction::Stop
            };
            assert_eq!(
                coordinator.finish(&attempt, AttemptOutcome::RetryWhenIdle),
                expected
            );
        }
        assert!(coordinator.begin_attempt().is_none());
    }

    #[test]
    fn step_progress_is_bound_to_child_lifetime() {
        let key = ("settings", "revision", 1_u64, 2_u64);
        let mut progress = StepProgress::default();
        progress.bind_key(&key);
        assert_eq!(progress.completed_steps(), 0);
        assert!(progress.bind_child(41));
        progress.mark_step_completed();
        assert_eq!(progress.completed_steps(), 1);
        assert!(progress.bind_child(41));
        progress.mark_step_completed();
        assert_eq!(progress.completed_steps(), 2);

        // Cancellation of an active runtime future retires its child.
        progress.child_retired();
        assert_eq!(progress.completed_steps(), 0);
        assert!(progress.bind_child(42));
        assert_eq!(progress.completed_steps(), 0);

        // A child restart discovered between completed steps also resets the
        // claims and tells the runner to repeat from the first prompt.
        progress.mark_step_completed();
        assert!(!progress.bind_child(43));
        assert_eq!(progress.completed_steps(), 0);
    }

    #[test]
    fn surviving_child_receipt_preserves_only_completed_same_child_progress() {
        let key = ("model-a", 7_u64);
        let mut progress = StepProgress::default();
        progress.bind_key(&key);
        assert!(progress.record_completed_step(41));
        assert_eq!(progress.completed_steps(), 1);

        // The runner uses the surviving-child receipt for a cancelled in-flight step;
        // only the real completion above advances progress.
        assert!(progress.retain_surviving_child(41));
        assert_eq!(progress.completed_steps(), 1);
        assert!(!progress.retain_surviving_child(42));
        assert_eq!(progress.completed_steps(), 0);

        assert!(progress.record_completed_step(42));
        assert_eq!(progress.completed_steps(), 1);
        progress.bind_key(&("model-b", 7_u64));
        assert_eq!(progress.completed_steps(), 0);
    }

    #[tokio::test]
    async fn capture_keeps_warmup_alive_until_native_model_demand() {
        let coordinator = std::sync::Arc::new(Coordinator::default());
        assert!(coordinator.request(("settings", "revision", 1_u64)));
        let attempt = coordinator.begin_attempt().unwrap();
        let cancellation = attempt.cancellation.clone();
        let attempt_cancellation = attempt.cancellation.clone();
        let foreground_operation = tokio_util::sync::CancellationToken::new();
        let fake_work = tokio::spawn(async move {
            tokio::select! {
                _ = cancellation.cancelled() => (),
                _ = tokio::time::sleep(Duration::from_secs(30)) => panic!("fake warmup remained active"),
            }
        });

        // Ordinary capture and processing do not invoke the model-demand
        // seam; the existing warmup work remains useful while audio records.
        tokio::task::yield_now().await;
        assert!(!attempt.cancellation.is_cancelled());
        assert!(!coordinator.has_model_demand());

        // Foreground demand defers additional steps without cancelling the attempt.
        let demand = coordinator.begin_model_demand();
        assert!(coordinator.has_model_demand());
        assert!(!attempt_cancellation.is_cancelled());
        assert!(!foreground_operation.is_cancelled());
        drop(demand);
        assert!(!attempt.cancellation.is_cancelled());
        fake_work.abort();
        fake_work
            .await
            .expect_err("test worker is stopped after demand assertion");
        assert!(!coordinator.has_model_demand());
        assert_eq!(
            coordinator.finish(&attempt, AttemptOutcome::RetryWhenIdle),
            RunnerAction::Continue
        );
        let retry = coordinator
            .begin_attempt()
            .expect("request remains pending");
        assert_eq!(retry.key, attempt.key);
        assert!(!coordinator.has_model_demand());
    }
}
