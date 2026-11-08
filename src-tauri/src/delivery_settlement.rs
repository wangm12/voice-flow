//! Transaction-scoped ownership for ordinary dictation clipboard writers.
//!
//! This owner proves only that clipboard-capable work admitted for one
//! recording has unwound. Writer closure is separate from insertion fidelity,
//! clipboard restoration, and permission to start another recording.

use chacha20poly1305::aead::{rand_core::RngCore, OsRng};
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, MutexGuard,
};
use tokio::sync::watch;

fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn opaque_identity_key(random: &mut impl RngCore) -> Option<String> {
    let mut bytes = [0; 16];
    random.try_fill_bytes(&mut bytes).ok()?;
    Some(hex::encode(bytes))
}

/// Random process-lifetime identity for one recording. The manager's wrapping
/// generation remains separate and is never used as the public transaction ID.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DictationTransactionId(Option<String>);

impl DictationTransactionId {
    pub(crate) fn fresh() -> Self {
        Self(opaque_identity_key(&mut OsRng))
    }

    fn public_value(&self) -> Option<String> {
        self.0.clone()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeliveryOutcomeKind {
    Inserted,
    CopyOnly,
    PasteUnverified,
    HistoryOnly,
    PreviewOnly,
    Failed,
    Cancelled,
    TimedOut,
    CompletedWithoutClipboard,
}

impl DeliveryOutcomeKind {
    fn ax_token(self) -> &'static str {
        match self {
            Self::Inserted => "inserted",
            Self::CopyOnly => "copy_only",
            Self::PasteUnverified => "paste_unverified",
            Self::HistoryOnly => "history_only",
            Self::PreviewOnly => "preview_only",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::CompletedWithoutClipboard => "completed_without_clipboard",
        }
    }
}

/// A bounded, content-free account of the product pipeline's terminal
/// classification. It carries no transcript or clipboard bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct DeliveryTerminalOutcome {
    pub(crate) kind: DeliveryOutcomeKind,
    pub(crate) phase: String,
    pub(crate) delivery_method: String,
    pub(crate) fallback_reason: Option<String>,
}

impl DeliveryTerminalOutcome {
    pub(crate) fn completed(
        phase: &str,
        delivery_method: &str,
        fallback_reason: Option<&str>,
    ) -> Self {
        let storage_failed = matches!(
            fallback_reason,
            Some("history_save_failed" | "history_save_failed_recovery_kept")
        );
        let kind = if fallback_reason == Some("processing_timeout") {
            DeliveryOutcomeKind::TimedOut
        } else if storage_failed {
            match delivery_method {
                "paste" => DeliveryOutcomeKind::Inserted,
                "paste_unverified" => DeliveryOutcomeKind::PasteUnverified,
                "clipboard" => DeliveryOutcomeKind::CopyOnly,
                _ => DeliveryOutcomeKind::Failed,
            }
        } else if phase == "error" {
            DeliveryOutcomeKind::Failed
        } else if phase == "unverified" || delivery_method == "paste_unverified" {
            DeliveryOutcomeKind::PasteUnverified
        } else if delivery_method == "paste" {
            DeliveryOutcomeKind::Inserted
        } else if delivery_method == "clipboard" {
            DeliveryOutcomeKind::CopyOnly
        } else if delivery_method == "history" || phase == "history" {
            DeliveryOutcomeKind::HistoryOnly
        } else if delivery_method == "onboarding" {
            DeliveryOutcomeKind::PreviewOnly
        } else {
            DeliveryOutcomeKind::CompletedWithoutClipboard
        };
        Self {
            kind,
            phase: phase.chars().take(48).collect(),
            delivery_method: delivery_method.chars().take(48).collect(),
            fallback_reason: fallback_reason.map(|reason| reason.chars().take(96).collect()),
        }
    }

    pub(crate) fn cancelled() -> Self {
        Self::completed("cancelled", "unknown", Some("delivery_outcome_unobserved"))
            .with_kind(DeliveryOutcomeKind::Cancelled)
    }

    pub(crate) fn timed_out() -> Self {
        Self::completed("error", "unknown", Some("processing_timeout"))
            .with_kind(DeliveryOutcomeKind::TimedOut)
    }

    fn with_kind(mut self, kind: DeliveryOutcomeKind) -> Self {
        self.kind = kind;
        self
    }
}

/// The storage owner can still block a later capture. Writer closure alone is
/// not evidence that the next recording can start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NextCaptureReadiness {
    Unverified,
}

/// Clipboard restoration is deliberately not inferred from Inserted or writer
/// closure. Product delivery currently does not publish its independent result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClipboardRestorationStatus {
    Unverified,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct DeliverySettlementSnapshot {
    pub(crate) transaction_id: Option<String>,
    pub(crate) terminal_outcome: Option<DeliveryTerminalOutcome>,
    pub(crate) writer_admission_sealed: bool,
    pub(crate) admitted_writers: usize,
    pub(crate) writers_closed: bool,
    pub(crate) clipboard_restoration: ClipboardRestorationStatus,
    pub(crate) next_capture_readiness: NextCaptureReadiness,
}

struct DeliverySettlementInner {
    terminal_outcome: Option<DeliveryTerminalOutcome>,
    admission_sealed: bool,
    admitted_writers: usize,
    writers_closed: bool,
}

/// Process-wide clock for public settlement snapshots. Revisions are serialized
/// as decimal strings so JavaScript never rounds them. Exhaustion fails closed.
struct SettlementStatusClock {
    /// Serializes lifecycle mutations with snapshot reads. Holding this gate
    /// across both the owner/registry change and its revision bump prevents a
    /// reader from pairing new state with an older revision.
    publication: Mutex<()>,
    revision: AtomicU64,
    exhausted: AtomicBool,
    changes: watch::Sender<()>,
}

impl SettlementStatusClock {
    fn new() -> Self {
        let (changes, _) = watch::channel(());
        Self {
            publication: Mutex::new(()),
            revision: AtomicU64::new(0),
            exhausted: AtomicBool::new(false),
            changes,
        }
    }

    fn lock_publication(&self) -> MutexGuard<'_, ()> {
        lock_recover(&self.publication)
    }

    fn bump_locked(&self) {
        loop {
            let current = self.revision.load(Ordering::Acquire);
            let Some(next) = current.checked_add(1) else {
                self.exhausted.store(true, Ordering::Release);
                self.changes.send_replace(());
                return;
            };
            if self
                .revision
                .compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.changes.send_replace(());
                return;
            }
        }
    }

    fn value(&self) -> (u64, bool) {
        (
            self.revision.load(Ordering::Acquire),
            !self.exhausted.load(Ordering::Acquire),
        )
    }

    fn subscribe(&self) -> watch::Receiver<()> {
        self.changes.subscribe()
    }
}

/// Genuine admission and closure owner for one dictation transaction.
///
/// `admit_writer` and `seal` share one mutex: after sealing, no later writer
/// can be admitted. A lease is created before its blocking task is scheduled
/// and dropped by that task after its last clipboard-capable operation.
pub(crate) struct DeliverySettlementOwner {
    transaction_id: DictationTransactionId,
    inner: Mutex<DeliverySettlementInner>,
    status_clock: Mutex<Option<Arc<SettlementStatusClock>>>,
    closed: watch::Sender<bool>,
}

impl DeliverySettlementOwner {
    pub(crate) fn new(transaction_id: DictationTransactionId) -> Arc<Self> {
        let (closed, _) = watch::channel(false);
        Arc::new(Self {
            transaction_id,
            inner: Mutex::new(DeliverySettlementInner {
                terminal_outcome: None,
                admission_sealed: false,
                admitted_writers: 0,
                writers_closed: false,
            }),
            status_clock: Mutex::new(None),
            closed,
        })
    }

    fn attach_status_clock(&self, clock: Arc<SettlementStatusClock>) {
        let mut status_clock = lock_recover(&self.status_clock);
        if status_clock.is_none() {
            *status_clock = Some(clock);
        }
    }

    fn mutate_inner<T>(&self, update: impl FnOnce(&mut DeliverySettlementInner) -> (T, bool)) -> T {
        let clock = lock_recover(&self.status_clock).clone();
        let _publication = clock.as_ref().map(|clock| clock.lock_publication());
        let (result, changed) = update(&mut lock_recover(&self.inner));
        if changed {
            if let Some(clock) = clock.as_ref() {
                clock.bump_locked();
            }
        }
        result
    }

    pub(crate) fn transaction_token(&self) -> Option<String> {
        self.transaction_id.public_value()
    }

    /// Admission happens synchronously before the caller schedules blocking
    /// clipboard work. `None` means the transaction has already sealed.
    pub(crate) fn admit_writer(self: &Arc<Self>) -> Option<DeliveryWriterLease> {
        let admitted = self.mutate_inner(|inner| {
            if inner.admission_sealed || inner.writers_closed {
                return (false, false);
            }
            let Some(admitted_writers) = inner.admitted_writers.checked_add(1) else {
                return (false, false);
            };
            inner.admitted_writers = admitted_writers;
            (true, true)
        });
        if !admitted {
            return None;
        }
        Some(DeliveryWriterLease {
            owner: Arc::clone(self),
        })
    }

    /// Seal future admission and retain the first terminal outcome. The owner
    /// becomes closed only when every already-admitted lease has dropped.
    pub(crate) fn seal(&self, outcome: DeliveryTerminalOutcome) -> bool {
        let (sealed, became_closed) = self.mutate_inner(|inner| {
            if inner.admission_sealed {
                return ((false, false), false);
            }
            inner.admission_sealed = true;
            inner.terminal_outcome = Some(outcome);
            let became_closed = if inner.admitted_writers == 0 {
                inner.writers_closed = true;
                true
            } else {
                false
            };
            ((true, became_closed), true)
        });
        if became_closed {
            self.closed.send_replace(true);
        }
        sealed
    }

    pub(crate) async fn wait_closed(&self) {
        let mut closed = self.closed.subscribe();
        loop {
            if *closed.borrow_and_update() {
                return;
            }
            if closed.changed().await.is_err() {
                return;
            }
        }
    }

    pub(crate) fn snapshot(&self) -> DeliverySettlementSnapshot {
        let inner = lock_recover(&self.inner);
        DeliverySettlementSnapshot {
            transaction_id: self.transaction_id.public_value(),
            terminal_outcome: inner.terminal_outcome.clone(),
            writer_admission_sealed: inner.admission_sealed,
            admitted_writers: inner.admitted_writers,
            writers_closed: inner.writers_closed,
            clipboard_restoration: ClipboardRestorationStatus::Unverified,
            next_capture_readiness: NextCaptureReadiness::Unverified,
        }
    }

    fn release_writer(&self) {
        let became_closed = self.mutate_inner(|inner| {
            if inner.admitted_writers > 0 {
                inner.admitted_writers -= 1;
            } else {
                return (false, false);
            }
            let became_closed =
                if inner.admission_sealed && inner.admitted_writers == 0 && !inner.writers_closed {
                    inner.writers_closed = true;
                    true
                } else {
                    false
                };
            (became_closed, true)
        });
        if became_closed {
            self.closed.send_replace(true);
        }
    }
}

/// RAII proof that one clipboard-capable operation was admitted before task
/// scheduling and has not yet unwound.
pub(crate) struct DeliveryWriterLease {
    owner: Arc<DeliverySettlementOwner>,
}

impl Drop for DeliveryWriterLease {
    fn drop(&mut self) {
        self.owner.release_writer();
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct DeliverySettlementRegistrySnapshot {
    pub(crate) schema: &'static str,
    pub(crate) process_identity: Option<String>,
    pub(crate) revision: String,
    pub(crate) revision_available: bool,
    pub(crate) current: Option<DeliverySettlementSnapshot>,
    pub(crate) last_terminal: Option<DeliverySettlementSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NativeAccessibleDeliveryStatus {
    pub(crate) identifier: String,
    pub(crate) label: String,
    pub(crate) value: String,
}

impl DeliverySettlementRegistrySnapshot {
    fn uses_chinese(&self, ui_language: &str) -> bool {
        ui_language == "zh"
    }

    pub(crate) fn accessible_label(&self, ui_language: &str) -> String {
        if self.uses_chinese(ui_language) {
            "VoiceFlow 听写状态".into()
        } else {
            "VoiceFlow dictation status".into()
        }
    }

    pub(crate) fn accessible_value(&self, ui_language: &str) -> String {
        let chinese = self.uses_chinese(ui_language);
        let current = self.current.as_ref();
        if !self.revision_available || self.process_identity.is_none() {
            return if chinese {
                "VoiceFlow 无法确认此听写状态的唯一身份。".into()
            } else {
                "VoiceFlow could not confirm a unique identity for this dictation status.".into()
            };
        }
        if current.is_some_and(|owner| owner.transaction_id.is_none()) {
            return if chinese {
                "VoiceFlow 无法确认此听写状态的唯一身份。".into()
            } else {
                "VoiceFlow could not confirm a unique identity for this dictation status.".into()
            };
        }
        if current.is_some_and(|owner| {
            !owner.writer_admission_sealed
                || !owner.writers_closed
                || owner.terminal_outcome.is_none()
        }) {
            return if chinese {
                "VoiceFlow 正在进行本次听写。".into()
            } else {
                "VoiceFlow dictation is in progress.".into()
            };
        }
        let terminal = current.or(self.last_terminal.as_ref());
        let Some(terminal) = terminal else {
            return if chinese {
                "尚无已完成的听写；下一次录音是否可用尚未确认。".into()
            } else {
                "No dictation has completed; availability of another recording is unverified."
                    .into()
            };
        };
        if terminal.transaction_id.is_none()
            || !terminal.writer_admission_sealed
            || !terminal.writers_closed
            || terminal.terminal_outcome.is_none()
        {
            return if chinese {
                "VoiceFlow 无法确认此听写事务的身份或写入状态。".into()
            } else {
                "VoiceFlow could not confirm this dictation transaction or writer state.".into()
            };
        }
        let outcome = terminal
            .terminal_outcome
            .as_ref()
            .map(|outcome| outcome.kind)
            .unwrap_or(DeliveryOutcomeKind::Failed);
        if chinese {
            match outcome {
                DeliveryOutcomeKind::Inserted => "听写已完成，文字已输入。剪贴板恢复情况和下一次录音是否可用均未确认。".into(),
                DeliveryOutcomeKind::CopyOnly => "听写已完成，文字已复制，请手动粘贴。剪贴板恢复情况和下一次录音是否可用均未确认。".into(),
                DeliveryOutcomeKind::PasteUnverified => "听写已结束，但无法确认文字是否已输入。请检查目标输入框。剪贴板恢复情况和下一次录音是否可用均未确认。".into(),
                DeliveryOutcomeKind::HistoryOnly => "听写已保存到历史记录，文字未确认输入。下一次录音是否可用未确认。".into(),
                DeliveryOutcomeKind::PreviewOnly => "听写预览已完成。下一次录音是否可用未确认。".into(),
                DeliveryOutcomeKind::Failed => "听写未完成，请检查 VoiceFlow。下一次录音是否可用未确认。".into(),
                DeliveryOutcomeKind::Cancelled => "听写已取消。剪贴板写入已结束，下一次录音是否可用未确认。".into(),
                DeliveryOutcomeKind::TimedOut => "听写已超时。文字交付未确认，下一次录音是否可用未确认。".into(),
                DeliveryOutcomeKind::CompletedWithoutClipboard => "听写流程已结束，但未进行剪贴板交付。下一次录音是否可用未确认。".into(),
            }
        } else {
            match outcome {
                DeliveryOutcomeKind::Inserted => "Dictation finished; text was inserted. Clipboard restoration and availability of another recording are unverified.".into(),
                DeliveryOutcomeKind::CopyOnly => "Dictation finished; text was copied for manual paste. Clipboard restoration and availability of another recording are unverified.".into(),
                DeliveryOutcomeKind::PasteUnverified => "Dictation ended, but insertion could not be confirmed. Check the target field. Clipboard restoration and availability of another recording are unverified.".into(),
                DeliveryOutcomeKind::HistoryOnly => "Dictation was saved to History; insertion was not confirmed. Availability of another recording is unverified.".into(),
                DeliveryOutcomeKind::PreviewOnly => "The dictation preview finished. Availability of another recording is unverified.".into(),
                DeliveryOutcomeKind::Failed => "Dictation did not finish. Check VoiceFlow. Availability of another recording is unverified.".into(),
                DeliveryOutcomeKind::Cancelled => "Dictation was cancelled. Clipboard writing has ended; availability of another recording is unverified.".into(),
                DeliveryOutcomeKind::TimedOut => "Dictation timed out. Text delivery was not confirmed; availability of another recording is unverified.".into(),
                DeliveryOutcomeKind::CompletedWithoutClipboard => "Dictation finished without clipboard delivery. Availability of another recording is unverified.".into(),
            }
        }
    }

    /// Machine-readable evidence kept in a native AX identifier. VoiceOver uses
    /// the separate label/value; this string contains bounded opaque IDs and
    /// fixed status tokens only, never transcript or clipboard text.
    pub(crate) fn native_ax_identifier(&self) -> String {
        fn owner_token(owner: Option<&DeliverySettlementSnapshot>) -> String {
            let Some(owner) = owner else {
                return "none".into();
            };
            let id = owner.transaction_id.as_deref().unwrap_or("unavailable");
            let outcome = owner
                .terminal_outcome
                .as_ref()
                .map(|outcome| outcome.kind.ax_token())
                .unwrap_or("pending");
            format!(
                "id={};outcome={};admission_sealed={};writers_closed={};admitted_writers={};clipboard_restoration={};next_capture_readiness={}",
                id,
                outcome,
                u8::from(owner.writer_admission_sealed),
                u8::from(owner.writers_closed),
                owner.admitted_writers,
                "restore_unverified",
                "readiness_unverified"
            )
        }
        format!(
            "com.voiceflow.dictation.settlement.v1|process={}|revision={}|revision_available={}|current={}|last={}",
            self.process_identity.as_deref().unwrap_or("unavailable"),
            self.revision,
            u8::from(self.revision_available),
            owner_token(self.current.as_ref()),
            owner_token(self.last_terminal.as_ref()),
        )
    }
}

#[derive(Default)]
struct DeliverySettlementRegistryInner {
    current: Option<Arc<DeliverySettlementOwner>>,
    last_terminal: Option<Arc<DeliverySettlementOwner>>,
}

/// Independent retained owner index for the current transaction and the last
/// sealed, closed terminal. It retains at most these two owners. Admission
/// order comes from current Arc identity, not the wrapping manager generation.
pub(crate) struct DeliverySettlementRegistry {
    inner: Mutex<DeliverySettlementRegistryInner>,
    process_identity: Option<String>,
    status_clock: Arc<SettlementStatusClock>,
}

impl Default for DeliverySettlementRegistry {
    fn default() -> Self {
        Self {
            inner: Mutex::new(DeliverySettlementRegistryInner::default()),
            process_identity: opaque_identity_key(&mut OsRng),
            status_clock: Arc::new(SettlementStatusClock::new()),
        }
    }
}

impl DeliverySettlementRegistry {
    /// Subscribe to owner-originated lifecycle changes. Writers notify this
    /// clock from their actual lease drop, including unwind paths.
    pub(crate) fn subscribe_changes(&self) -> watch::Receiver<()> {
        self.status_clock.subscribe()
    }

    /// Admit one fresh owner. An active owner cannot be displaced, a closed
    /// owner cannot be re-admitted, and the numeric manager generation is never
    /// used to infer ordering.
    pub(crate) fn begin(&self, owner: Arc<DeliverySettlementOwner>) -> bool {
        let snapshot = owner.snapshot();
        if snapshot.writer_admission_sealed
            || snapshot.writers_closed
            || snapshot.admitted_writers != 0
            || snapshot.terminal_outcome.is_some()
        {
            return false;
        }
        owner.attach_status_clock(Arc::clone(&self.status_clock));
        let _publication = self.status_clock.lock_publication();
        // Recheck after attaching the clock and under the publication gate.
        // This closes the only interval where an owner could have changed
        // before its first public revision was assigned.
        let snapshot = owner.snapshot();
        if snapshot.writer_admission_sealed
            || snapshot.writers_closed
            || snapshot.admitted_writers != 0
            || snapshot.terminal_outcome.is_some()
        {
            return false;
        }
        let mut inner = lock_recover(&self.inner);
        if inner
            .current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &owner))
        {
            return true;
        }
        if inner
            .last_terminal
            .as_ref()
            .is_some_and(|last| Arc::ptr_eq(last, &owner))
            || inner.current.is_some()
        {
            return false;
        }
        inner.current = Some(owner);
        self.status_clock.bump_locked();
        true
    }

    pub(crate) fn is_last_terminal_owner(&self, owner: &Arc<DeliverySettlementOwner>) -> bool {
        lock_recover(&self.inner)
            .last_terminal
            .as_ref()
            .is_some_and(|terminal| Arc::ptr_eq(terminal, owner))
    }

    pub(crate) fn clear_current(&self, owner: &Arc<DeliverySettlementOwner>) -> bool {
        let _publication = self.status_clock.lock_publication();
        let mut inner = lock_recover(&self.inner);
        if inner
            .current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, owner))
        {
            inner.current = None;
            self.status_clock.bump_locked();
            true
        } else {
            false
        }
    }

    pub(crate) fn record_terminal(&self, owner: Arc<DeliverySettlementOwner>) -> bool {
        let snapshot = owner.snapshot();
        if !snapshot.writer_admission_sealed
            || !snapshot.writers_closed
            || snapshot.terminal_outcome.is_none()
        {
            return false;
        }
        let _publication = self.status_clock.lock_publication();
        let mut inner = lock_recover(&self.inner);
        if inner
            .last_terminal
            .as_ref()
            .is_some_and(|last| Arc::ptr_eq(last, &owner))
        {
            // Idempotent duplicate from an older completion/cancel waiter.
            // Do not clear a newer current owner or regress the last terminal.
            return true;
        }
        if !inner
            .current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &owner))
        {
            return false;
        }
        inner.current = None;
        inner.last_terminal = Some(owner);
        self.status_clock.bump_locked();
        true
    }

    pub(crate) fn snapshot(&self) -> DeliverySettlementRegistrySnapshot {
        let _publication = self.status_clock.lock_publication();
        let revision = self.status_clock.value();
        let inner = lock_recover(&self.inner);
        let current = inner.current.as_ref().map(|owner| owner.snapshot());
        let last_terminal = inner.last_terminal.as_ref().map(|owner| owner.snapshot());
        DeliverySettlementRegistrySnapshot {
            schema: "voiceflow_delivery_settlement_v1",
            process_identity: self.process_identity.clone(),
            revision: revision.0.to_string(),
            revision_available: revision.1,
            current,
            last_terminal,
        }
    }
}

/// Reconcile the current registry when a queued native publication actually
/// executes. The native main-thread queue must never replay a captured owner
/// snapshot after a newer recording has become current.
pub(crate) fn current_native_accessible_delivery_status(
    registry: &DeliverySettlementRegistry,
    ui_language: &str,
) -> NativeAccessibleDeliveryStatus {
    let snapshot = registry.snapshot();
    NativeAccessibleDeliveryStatus {
        identifier: snapshot.native_ax_identifier(),
        label: snapshot.accessible_label(ui_language),
        value: snapshot.accessible_value(ui_language),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        current_native_accessible_delivery_status, ClipboardRestorationStatus, DeliveryOutcomeKind,
        DeliverySettlementOwner, DeliverySettlementRegistry, DeliveryTerminalOutcome,
        SettlementStatusClock,
    };
    use std::sync::mpsc;

    fn owner() -> std::sync::Arc<DeliverySettlementOwner> {
        DeliverySettlementOwner::new(super::DictationTransactionId::fresh())
    }

    #[tokio::test]
    async fn sealed_queued_writer_stays_open_until_its_lease_unwinds() {
        let owner = owner();
        let transaction_id = owner
            .snapshot()
            .transaction_id
            .clone()
            .expect("OS entropy yields a public transaction identity");
        let queued_lease = owner.admit_writer().expect("owner is open");
        owner.seal(DeliveryTerminalOutcome::cancelled());

        let snapshot = owner.snapshot();
        assert!(snapshot.writer_admission_sealed);
        assert_eq!(snapshot.admitted_writers, 1);
        assert!(!snapshot.writers_closed);
        assert_eq!(
            snapshot.transaction_id.as_deref(),
            Some(transaction_id.as_str())
        );
        assert_eq!(
            snapshot
                .terminal_outcome
                .as_ref()
                .map(|outcome| outcome.kind),
            Some(DeliveryOutcomeKind::Cancelled)
        );
        assert_eq!(
            snapshot.next_capture_readiness,
            super::NextCaptureReadiness::Unverified
        );
        assert_eq!(
            snapshot.clipboard_restoration,
            ClipboardRestorationStatus::Unverified
        );
        assert!(owner.admit_writer().is_none());

        let wait_owner = owner.clone();
        let (closed_tx, mut closed_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            wait_owner.wait_closed().await;
            let _ = closed_tx.send(());
        });
        assert!(matches!(
            closed_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        drop(queued_lease);
        closed_rx
            .await
            .expect("closed notification follows lease drop");
        let closed = owner.snapshot();
        assert_eq!(closed.admitted_writers, 0);
        assert!(closed.writers_closed);
        assert_eq!(
            closed.transaction_id.as_deref(),
            Some(transaction_id.as_str())
        );
        assert_eq!(
            closed.terminal_outcome.as_ref().map(|outcome| outcome.kind),
            Some(DeliveryOutcomeKind::Cancelled)
        );
    }

    #[tokio::test]
    async fn panic_unwind_releases_the_sealed_writer_lease() {
        let owner = owner();
        let lease = owner.admit_writer().expect("owner is open");
        owner.seal(DeliveryTerminalOutcome::timed_out());
        let worker_owner = owner.clone();
        let (started_tx, started_rx) = mpsc::sync_channel(0);
        let (continue_tx, continue_rx) = mpsc::sync_channel(0);
        let worker = tokio::task::spawn_blocking(move || {
            let _lease = lease;
            started_tx.send(()).expect("test receiver is live");
            continue_rx.recv().expect("test releases worker");
            panic!("injected writer unwind");
        });
        started_rx.recv().expect("worker reached its boundary");
        let during = worker_owner.snapshot();
        assert_eq!(during.admitted_writers, 1);
        assert!(!during.writers_closed);
        continue_tx.send(()).expect("worker is waiting");
        assert!(worker
            .await
            .expect_err("panic is retained as JoinError")
            .is_panic());
        owner.wait_closed().await;
        let after = owner.snapshot();
        assert_eq!(after.admitted_writers, 0);
        assert!(after.writers_closed);
        assert_eq!(
            after.terminal_outcome.as_ref().map(|outcome| outcome.kind),
            Some(DeliveryOutcomeKind::TimedOut)
        );
    }

    #[test]
    fn registry_retains_current_and_last_terminal_with_distinct_opaque_ids() {
        let registry = DeliverySettlementRegistry::default();
        let first = owner();
        let first_id = first.snapshot().transaction_id.clone().expect("entropy");
        assert!(registry.begin(first.clone()));
        first.seal(DeliveryTerminalOutcome::completed("done", "paste", None));
        assert!(registry.record_terminal(first.clone()));

        let second = owner();
        let second_id = second.snapshot().transaction_id.clone().expect("entropy");
        assert_ne!(first_id, second_id);
        assert!(registry.begin(second.clone()));
        let snapshot = registry.snapshot();
        assert_eq!(
            snapshot
                .current
                .as_ref()
                .and_then(|owner| owner.transaction_id.as_deref()),
            Some(second_id.as_str())
        );
        assert_eq!(
            snapshot
                .last_terminal
                .as_ref()
                .and_then(|owner| owner.transaction_id.as_deref()),
            Some(first_id.as_str())
        );
        assert_eq!(
            snapshot
                .last_terminal
                .as_ref()
                .and_then(|owner| owner.terminal_outcome.as_ref())
                .map(|outcome| outcome.kind),
            Some(DeliveryOutcomeKind::Inserted)
        );
        assert!(snapshot
            .last_terminal
            .as_ref()
            .is_some_and(|owner| owner.writers_closed));
        assert_eq!(
            snapshot
                .current
                .as_ref()
                .map(|owner| owner.next_capture_readiness),
            Some(super::NextCaptureReadiness::Unverified)
        );
        assert!(snapshot.revision_available);
    }

    #[test]
    fn registry_refuses_a_sealed_owner_until_all_writers_close() {
        let registry = DeliverySettlementRegistry::default();
        let owner = owner();
        assert!(registry.begin(owner.clone()));
        let lease = owner.admit_writer().expect("owner is open");
        owner.seal(DeliveryTerminalOutcome::timed_out());
        assert!(!registry.record_terminal(owner.clone()));
        let current = registry.snapshot().current.expect("owner remains current");
        assert!(current.writer_admission_sealed);
        assert_eq!(current.admitted_writers, 1);
        assert!(!current.writers_closed);
        drop(lease);
        assert!(registry.record_terminal(owner));
    }

    #[test]
    fn inserted_is_distinct_from_clipboard_restoration_and_capture_readiness() {
        let owner = owner();
        owner.seal(DeliveryTerminalOutcome::completed("done", "paste", None));
        let snapshot = owner.snapshot();
        assert!(snapshot.writers_closed);
        assert_eq!(
            snapshot
                .terminal_outcome
                .as_ref()
                .map(|outcome| outcome.kind),
            Some(DeliveryOutcomeKind::Inserted)
        );
        assert_eq!(
            snapshot.clipboard_restoration,
            ClipboardRestorationStatus::Unverified
        );
        assert_eq!(
            snapshot.next_capture_readiness,
            super::NextCaptureReadiness::Unverified
        );
    }

    #[test]
    fn public_revision_tracks_exact_start_seal_close_and_terminal_publications() {
        let registry = DeliverySettlementRegistry::default();
        let owner = owner();
        let id = owner.snapshot().transaction_id.expect("entropy");

        assert_eq!(registry.snapshot().revision, "0");
        assert!(registry.begin(owner.clone()));
        let started = registry.snapshot();
        assert_eq!(started.revision, "1");
        assert_eq!(
            started
                .current
                .as_ref()
                .and_then(|current| current.transaction_id.as_deref()),
            Some(id.as_str())
        );
        assert!(
            !started
                .current
                .as_ref()
                .expect("current owner")
                .writers_closed
        );

        let lease = owner.admit_writer().expect("owner is open");
        let admitted = registry.snapshot();
        assert_eq!(admitted.revision, "2");
        assert_eq!(
            admitted
                .current
                .as_ref()
                .expect("current owner")
                .admitted_writers,
            1
        );

        owner.seal(DeliveryTerminalOutcome::completed("done", "paste", None));
        let sealed = registry.snapshot();
        assert_eq!(sealed.revision, "3");
        let sealed_owner = sealed
            .current
            .as_ref()
            .expect("sealed owner remains current");
        assert!(sealed_owner.writer_admission_sealed);
        assert!(!sealed_owner.writers_closed);
        assert_eq!(
            sealed_owner
                .terminal_outcome
                .as_ref()
                .map(|outcome| outcome.kind),
            Some(DeliveryOutcomeKind::Inserted)
        );

        drop(lease);
        let closed = registry.snapshot();
        assert_eq!(closed.revision, "4");
        assert!(
            closed
                .current
                .as_ref()
                .expect("closed owner remains current")
                .writers_closed
        );

        assert!(registry.record_terminal(owner));
        let terminal = registry.snapshot();
        assert_eq!(terminal.revision, "5");
        assert!(terminal.current.is_none());
        assert_eq!(
            terminal
                .last_terminal
                .as_ref()
                .and_then(|last| last.transaction_id.as_deref()),
            Some(id.as_str())
        );
    }

    #[test]
    fn public_snapshot_cannot_pair_a_mutation_with_its_previous_revision() {
        let registry = std::sync::Arc::new(DeliverySettlementRegistry::default());
        let owner = owner();
        assert!(registry.begin(owner.clone()));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let update_registry = registry.clone();
        let update_owner = owner.clone();
        let update_barrier = barrier.clone();
        let updater = std::thread::spawn(move || {
            update_barrier.wait();
            let _lease = update_owner.admit_writer().expect("owner is open");
            update_owner.seal(DeliveryTerminalOutcome::timed_out());
            update_registry.snapshot()
        });
        barrier.wait();
        let concurrent_snapshot = registry.snapshot();
        let final_snapshot = updater.join().expect("writer publication returns");

        for snapshot in [&concurrent_snapshot, &final_snapshot] {
            let Some(owner) = snapshot.current.as_ref() else {
                assert_eq!(
                    snapshot.revision, "1",
                    "only the initial owner may have no published writer update"
                );
                continue;
            };
            if owner.writer_admission_sealed {
                if owner.writers_closed {
                    assert_eq!(snapshot.revision, "4");
                    assert_eq!(owner.admitted_writers, 0);
                } else {
                    assert_eq!(snapshot.revision, "3");
                    assert_eq!(owner.admitted_writers, 1);
                }
                assert_eq!(
                    owner.terminal_outcome.as_ref().map(|outcome| outcome.kind),
                    Some(super::DeliveryOutcomeKind::TimedOut)
                );
            } else if owner.admitted_writers == 1 {
                assert_eq!(snapshot.revision, "2");
                assert!(!owner.writer_admission_sealed);
                assert!(!owner.writers_closed);
            } else {
                assert_eq!(snapshot.revision, "1");
                assert_eq!(owner.admitted_writers, 0);
                assert!(!owner.writers_closed);
            }
        }
        assert_eq!(final_snapshot.revision, "3");
        assert!(final_snapshot
            .current
            .as_ref()
            .is_some_and(|owner| owner.writer_admission_sealed && !owner.writers_closed));
    }

    #[test]
    fn accessible_status_requires_a_unique_sealed_closed_transaction() {
        let registry = DeliverySettlementRegistry::default();
        let owner = owner();
        assert!(registry.begin(owner.clone()));
        let lease = owner.admit_writer().expect("owner is open");
        owner.seal(DeliveryTerminalOutcome::completed("done", "paste", None));

        let open = registry.snapshot();
        let open_value = open.accessible_value("en");
        let open_owner = open
            .current
            .as_ref()
            .expect("active transaction remains current");
        assert!(open_owner.writer_admission_sealed);
        assert!(!open_owner.writers_closed);
        assert_eq!(open_owner.admitted_writers, 1);
        assert_eq!(
            open_owner.terminal_outcome.as_ref().map(|outcome| outcome.kind),
            Some(DeliveryOutcomeKind::Inserted),
            "terminal classification is known but is not public as complete while a writer remains open"
        );
        assert_eq!(open_value, "VoiceFlow dictation is in progress.");
        assert!(!open_value.contains("text was inserted"));

        drop(lease);
        let closed = registry.snapshot();
        let closed_owner = closed
            .current
            .as_ref()
            .expect("closed transaction remains current until terminal publication");
        assert!(closed_owner.writer_admission_sealed);
        assert!(closed_owner.writers_closed);
        assert_eq!(closed_owner.admitted_writers, 0);
        assert_eq!(
            closed_owner
                .terminal_outcome
                .as_ref()
                .map(|outcome| outcome.kind),
            Some(DeliveryOutcomeKind::Inserted)
        );
        assert!(closed.accessible_value("en").contains("text was inserted"));
        assert!(closed.accessible_value("en").contains("unverified"));

        let mut identity_missing = closed;
        identity_missing
            .current
            .as_mut()
            .expect("owner remains current until registry publication")
            .transaction_id = None;
        let missing_identity_value = identity_missing.accessible_value("en");
        assert!(missing_identity_value.contains("could not confirm"));
        assert!(!missing_identity_value.contains("text was inserted"));
    }

    #[test]
    fn queued_native_publications_resolve_the_latest_transaction_when_applied() {
        let registry = std::sync::Arc::new(DeliverySettlementRegistry::default());
        let transaction_a = owner();
        let transaction_a_id = transaction_a
            .snapshot()
            .transaction_id
            .clone()
            .expect("transaction A has a public identity");
        assert!(registry.begin(transaction_a.clone()));
        let writer_a = transaction_a
            .admit_writer()
            .expect("transaction A writer is admitted");
        transaction_a.seal(DeliveryTerminalOutcome::completed("done", "paste", None));
        drop(writer_a);
        assert!(registry.record_terminal(transaction_a));

        // This represents transaction A's native task waiting in the main
        // thread queue. It captures the registry and locale, never A's strings.
        let queued_a_registry = registry.clone();
        let queued_a = move || current_native_accessible_delivery_status(&queued_a_registry, "en");

        let transaction_b = owner();
        let transaction_b_id = transaction_b
            .snapshot()
            .transaction_id
            .clone()
            .expect("transaction B has a public identity");
        assert!(registry.begin(transaction_b.clone()));
        let writer_b = transaction_b
            .admit_writer()
            .expect("transaction B writer is admitted");
        let queued_b_registry = registry.clone();
        let queued_b = move || current_native_accessible_delivery_status(&queued_b_registry, "en");

        let applied_a = queued_a();
        assert!(applied_a.identifier.contains(&format!(
            "current=id={transaction_b_id};outcome=pending;admission_sealed=0;writers_closed=0;admitted_writers=1"
        )));
        assert!(applied_a.identifier.contains(&format!(
            "last=id={transaction_a_id};outcome=inserted;admission_sealed=1;writers_closed=1"
        )));
        assert_eq!(applied_a.value, "VoiceFlow dictation is in progress.");
        assert!(!applied_a.value.contains("text was inserted"));

        transaction_b.seal(DeliveryTerminalOutcome::completed("done", "paste", None));
        drop(writer_b);
        assert!(registry.record_terminal(transaction_b));
        let queued_b_terminal_registry = registry.clone();
        let queued_b_terminal =
            move || current_native_accessible_delivery_status(&queued_b_terminal_registry, "en");

        // Older queued tasks reconcile against the latest registry at execution,
        // so neither A's completion nor B's earlier active phase can regress B.
        for applied in [queued_b(), queued_b_terminal()] {
            assert!(applied.identifier.contains("|current=none|last=id="));
            assert!(applied.identifier.contains(&format!(
                "last=id={transaction_b_id};outcome=inserted;admission_sealed=1;writers_closed=1"
            )));
            assert!(applied.value.contains("text was inserted"));
        }
    }

    #[test]
    fn effective_ui_language_relocalizes_without_changing_machine_evidence() {
        let registry = DeliverySettlementRegistry::default();
        let active = owner();
        assert!(registry.begin(active.clone()));
        let _writer = active
            .admit_writer()
            .expect("active transaction has one writer");
        let before = registry.snapshot();

        let english = current_native_accessible_delivery_status(&registry, "en");
        let chinese = current_native_accessible_delivery_status(&registry, "zh");
        let after = registry.snapshot();

        assert_eq!(before.revision, after.revision);
        assert_eq!(english.identifier, chinese.identifier);
        assert_eq!(english.label, "VoiceFlow dictation status");
        assert_eq!(chinese.label, "VoiceFlow 听写状态");
        assert_eq!(english.value, "VoiceFlow dictation is in progress.");
        assert_eq!(chinese.value, "VoiceFlow 正在进行本次听写。");
    }

    #[test]
    fn delayed_duplicate_cannot_regress_last_terminal_after_a_newer_owner() {
        let registry = std::sync::Arc::new(DeliverySettlementRegistry::default());
        let cancelled = owner();
        assert!(registry.begin(cancelled.clone()));
        cancelled.seal(DeliveryTerminalOutcome::cancelled());

        let delayed_registry = registry.clone();
        let delayed_owner = cancelled.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(0);
        let (resume_tx, resume_rx) = mpsc::sync_channel(0);
        let delayed_finish = std::thread::spawn(move || {
            ready_tx.send(()).expect("root observes delayed finisher");
            resume_rx.recv().expect("root resumes delayed finisher");
            delayed_registry.record_terminal(delayed_owner)
        });
        ready_rx
            .recv()
            .expect("older finisher is paused before publish");

        assert!(registry.record_terminal(cancelled.clone()));
        let newer = owner();
        let newer_id = newer.snapshot().transaction_id.clone().expect("entropy");
        assert!(registry.begin(newer.clone()));
        newer.seal(DeliveryTerminalOutcome::completed("done", "paste", None));
        assert!(registry.record_terminal(newer));

        resume_tx
            .send(())
            .expect("resume older duplicate publication");
        assert!(!delayed_finish.join().expect("delayed publisher returns"));
        let snapshot = registry.snapshot();
        assert_eq!(
            snapshot
                .last_terminal
                .as_ref()
                .and_then(|owner| owner.transaction_id.as_deref()),
            Some(newer_id.as_str())
        );
    }

    #[test]
    fn unregistered_owner_cannot_publish_or_clear_the_registered_transaction() {
        let registry = DeliverySettlementRegistry::default();
        let admitted = owner();
        let admitted_id = admitted.snapshot().transaction_id.clone().expect("entropy");
        assert!(registry.begin(admitted.clone()));

        let fabricated = owner();
        fabricated.seal(DeliveryTerminalOutcome::completed("done", "paste", None));
        assert!(!registry.clear_current(&fabricated));
        assert!(!registry.record_terminal(fabricated));
        let snapshot = registry.snapshot();
        assert_eq!(
            snapshot
                .current
                .as_ref()
                .and_then(|owner| owner.transaction_id.as_deref()),
            Some(admitted_id.as_str())
        );
        assert!(snapshot.last_terminal.is_none());
    }

    #[test]
    fn process_revision_is_decimal_string_and_exhaustion_fails_closed() {
        let clock = SettlementStatusClock::new();
        clock
            .revision
            .store(u64::MAX, std::sync::atomic::Ordering::Release);
        let _publication = clock.lock_publication();
        clock.bump_locked();
        assert_eq!(clock.value(), (u64::MAX, false));
    }

    #[test]
    fn native_ax_identifier_contains_only_bounded_owner_metadata() {
        let registry = DeliverySettlementRegistry::default();
        let owner = owner();
        let id = owner.snapshot().transaction_id.clone().expect("entropy");
        assert!(registry.begin(owner.clone()));
        owner.seal(DeliveryTerminalOutcome::completed("done", "paste", None));
        assert!(registry.record_terminal(owner));
        let snapshot = registry.snapshot();
        let ax_identifier = snapshot.native_ax_identifier();
        assert!(ax_identifier.starts_with("com.voiceflow.dictation.settlement.v1|"));
        assert!(ax_identifier.contains(&id));
        assert!(ax_identifier.contains("outcome=inserted;admission_sealed=1;writers_closed=1;admitted_writers=0;clipboard_restoration=restore_unverified;next_capture_readiness=readiness_unverified"));
        assert!(!ax_identifier.contains("transcript"));
        assert!(!ax_identifier.contains("clipboard text"));
        let value = serde_json::to_value(snapshot).expect("public snapshot serializes");
        assert!(value["revision"].is_string());
        assert!(value["process_identity"].is_string());
        assert_eq!(value["last_terminal"]["transaction_id"], id);
        assert_eq!(
            value["last_terminal"]["terminal_outcome"]["kind"],
            "inserted"
        );
        assert_eq!(
            value["last_terminal"]["clipboard_restoration"],
            "unverified"
        );
        assert_eq!(
            value["last_terminal"]["next_capture_readiness"],
            "unverified"
        );
    }

    #[test]
    fn native_ax_identifier_carries_active_seal_and_closure_facts() {
        let registry = DeliverySettlementRegistry::default();
        let owner = owner();
        let id = owner.snapshot().transaction_id.expect("entropy");
        assert!(registry.begin(owner.clone()));
        let lease = owner.admit_writer().expect("owner is open");
        owner.seal(DeliveryTerminalOutcome::cancelled());
        let active = registry.snapshot();
        let active_id = active.native_ax_identifier();
        assert!(active_id.contains(&format!("current=id={id};outcome=cancelled;admission_sealed=1;writers_closed=0;admitted_writers=1;clipboard_restoration=restore_unverified;next_capture_readiness=readiness_unverified")));
        assert!(active_id.contains("last=none"));

        drop(lease);
        let closed = registry.snapshot();
        assert!(closed.native_ax_identifier().contains(&format!("current=id={id};outcome=cancelled;admission_sealed=1;writers_closed=1;admitted_writers=0;clipboard_restoration=restore_unverified;next_capture_readiness=readiness_unverified")));
    }

    #[test]
    fn history_persistence_failure_preserves_observed_delivery_outcome() {
        assert_eq!(
            DeliveryTerminalOutcome::completed(
                "error",
                "paste",
                Some("history_save_failed_recovery_kept")
            )
            .kind,
            DeliveryOutcomeKind::Inserted
        );
        assert_eq!(
            DeliveryTerminalOutcome::completed("error", "clipboard", Some("history_save_failed"))
                .kind,
            DeliveryOutcomeKind::CopyOnly
        );
        assert_eq!(
            DeliveryTerminalOutcome::completed("error", "history", Some("history_save_failed"))
                .kind,
            DeliveryOutcomeKind::Failed
        );
    }
}
