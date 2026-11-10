//! Default-build stand-in for `research_trace.rs`. Without the research
//! harness no lease can exist: `Trace` is uninhabited, so every
//! `Option<Trace>` is `None` and owner hooks compile to nothing. Signatures
//! mirror the active module so production call sites need no cfg.
use crate::AppState;
use std::future::Future;

pub(crate) const ACTIVE: bool = false;

#[derive(Clone)]
pub(crate) enum Trace {}

/// Shape-compatible with the active facts; nothing reads them here.
#[allow(dead_code)]
pub(crate) enum Fact {
    CaptureDevice {
        requested_selection: Option<String>,
        clamshell_substituted: bool,
        system_default_name: Option<String>,
        effective_device_name: Option<String>,
    },
    WinnerSelected {
        origin: String,
    },
    CleanupResolved {
        stage: String,
        reason: Option<String>,
    },
    DeliveryTargetReadback {
        verified: Option<bool>,
    },
    #[cfg(feature = "research-harness")]
    LearningRecord {
        record: Box<crate::learning_research::LearningRecordV1>,
    },
}

#[allow(dead_code)]
pub(crate) struct FinalClipboardAssociation {
    pub snapshot_fixture_identity: String,
    pub snapshot_generation: String,
    pub temporary_generation: String,
    pub final_generation: String,
    pub source: &'static str,
}

impl Trace {
    pub(crate) fn allows(&self) -> bool {
        match *self {}
    }
    pub(crate) fn emit(&self, _fact: Fact) {
        match *self {}
    }
    pub(crate) fn stop(&self, _processing_generation: u64) {
        match *self {}
    }
    pub(crate) fn learning_consent(&self) -> Option<&crate::learning_research::LearningConsent> {
        match *self {}
    }
    pub(crate) fn target_is_current(&self, _state: &AppState) -> bool {
        match *self {}
    }
    pub(crate) fn bound_target_matches(&self, _pid: i32) -> bool {
        match *self {}
    }
    pub(crate) fn bind_audio(&self, _session: &str) {
        match *self {}
    }
    pub(crate) fn first_input(&self, _samples: usize) {
        match *self {}
    }
    pub(crate) fn capture_closed(&self, _samples: &[f32], _complete: bool) {
        match *self {}
    }
    pub(crate) fn capture_is_closed(&self) -> bool {
        match *self {}
    }
    pub(crate) fn completion(&self, _phase: &str) {
        match *self {}
    }
    pub(crate) fn revoke(&self, _reason: &'static str) {
        match *self {}
    }
    pub(crate) fn terminal(&self) {
        match *self {}
    }
    pub(crate) fn begin_clipboard_write(&self) {
        match *self {}
    }
    pub(crate) fn keyboard(&self) {
        match *self {}
    }
    pub(crate) fn clipboard_resolved(&self, _resolution: &'static str, _known: bool) {
        match *self {}
    }
    pub(crate) fn clipboard_restored_fixture(&self, _association: FinalClipboardAssociation) {
        match *self {}
    }
}

pub(crate) struct DeliveryWriter;
pub(crate) struct LearningTask;
impl Drop for LearningTask {
    fn drop(&mut self) {}
}

#[inline(always)]
pub(crate) fn current() -> Option<Trace> {
    None
}
#[inline(always)]
pub(crate) fn scope<F: Future>(_trace: Option<Trace>, future: F) -> F {
    future
}
#[inline(always)]
pub(crate) fn sync_scope<T>(_trace: Option<Trace>, operation: impl FnOnce() -> T) -> T {
    operation()
}
#[inline(always)]
pub(crate) fn spawn_blocking<F, R>(operation: F) -> tokio::task::JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(operation)
}
#[inline(always)]
pub(crate) fn tokio_spawn<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(future)
}
#[inline(always)]
pub(crate) fn allowed() -> bool {
    true
}
#[inline(always)]
pub(crate) fn emit(_fact: Fact) {}
#[inline(always)]
pub(crate) fn capture_device_fact(
    requested_selection: Option<String>,
    clamshell_substituted: bool,
    system_default_name: Option<String>,
    effective_device_name: Option<String>,
) -> Fact {
    Fact::CaptureDevice {
        requested_selection,
        clamshell_substituted,
        system_default_name,
        effective_device_name,
    }
}
#[inline(always)]
pub(crate) fn retain_operation(_state: &AppState) -> bool {
    false
}
#[inline(always)]
pub(crate) fn invalidate(_state: &AppState, _reason: &'static str) {}
#[inline(always)]
pub(crate) fn audio_trace(_app: &tauri::AppHandle, _session: &str) -> Option<Trace> {
    None
}
#[inline(always)]
pub(crate) fn delivery_writer() -> DeliveryWriter {
    DeliveryWriter
}
#[inline(always)]
pub(crate) fn learning_task() -> LearningTask {
    LearningTask
}
#[inline(always)]
pub(crate) fn learning_spawn<F>(future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    tauri::async_runtime::spawn(future);
}
#[inline(always)]
pub(crate) fn asr_attempt(_trace: Option<&Trace>, _provider: &str, _model: &str, _path: &str) {}
