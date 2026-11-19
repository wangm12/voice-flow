#[allow(unused_imports)]
use super::*;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Default)]
pub struct ActiveAudioSpoolSessions {
    pub(super) sessions: std::collections::HashMap<PathBuf, SpoolReservation>,
    pub(super) root_mutations: std::collections::HashMap<
        PathBuf,
        std::collections::HashMap<u64, std::sync::Arc<AtomicBool>>,
    >,
    pub(super) clearing_roots: std::collections::HashSet<PathBuf>,
    pub(super) next_generation: u64,
}

pub struct SpoolReservation {
    pub(super) generation: u64,
    pub(super) delete_when_released: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpoolLeaseKind {
    Session,
    Artifact,
}

pub static ACTIVE_AUDIO_SPOOL_SESSIONS: OnceLock<Mutex<ActiveAudioSpoolSessions>> = OnceLock::new();

#[cfg(test)]
pub fn hold_root_admission_lock_for_test() -> (
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::Sender<()>,
    std::thread::JoinHandle<()>,
) {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let join = std::thread::spawn(move || {
        let _guard = ACTIVE_AUDIO_SPOOL_SESSIONS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    (entered_rx, release_tx, join)
}
pub static PENDING_HISTORY_BUDGET_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
pub static PENDING_HISTORY_ACTIVE: OnceLock<Mutex<std::collections::HashSet<(PathBuf, String)>>> =
    OnceLock::new();

#[cfg(test)]
pub struct HistoryPromotionTestGate {
    pub(super) data_root: PathBuf,
    pub(super) entered: std::sync::mpsc::Sender<()>,
    pub(super) release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
pub struct HistoryDeleteSpoolWaitTestGate {
    pub(super) session_dir: PathBuf,
    pub(super) entered: std::sync::mpsc::Sender<()>,
    pub(super) release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
pub(super) static HISTORY_PROMOTION_TEST_GATE: OnceLock<Mutex<Option<HistoryPromotionTestGate>>> =
    OnceLock::new();

#[cfg(test)]
pub(super) static HISTORY_DELETE_SPOOL_WAIT_TEST_GATE: OnceLock<
    Mutex<Option<HistoryDeleteSpoolWaitTestGate>>,
> = OnceLock::new();

#[cfg(test)]
pub(super) struct PendingHistoryWriteTestGate {
    pub(super) data_root: PathBuf,
    pub(super) entered: std::sync::mpsc::Sender<()>,
    pub(super) release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
pub(super) struct SpoolQuotaWriteTestGate {
    pub(super) session_dir: PathBuf,
    pub(super) entered: std::sync::mpsc::Sender<()>,
    pub(super) release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
pub struct SpoolSizeWalkTestGate {
    pub(super) path: PathBuf,
    pub(super) entered: std::sync::mpsc::Sender<()>,
    pub(super) release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub enum SpoolQuotaScanFault {
    Error,
    Panic,
}

#[cfg(test)]
pub struct SpoolQuotaScanFaultInjection {
    pub(super) path: PathBuf,
    pub(super) fault: SpoolQuotaScanFault,
}

#[cfg(test)]
pub struct AtomicRenameTestGate {
    pub(super) temp_path: PathBuf,
    pub(super) entered: std::sync::mpsc::Sender<()>,
    pub(super) release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
pub(super) static PENDING_HISTORY_WRITE_TEST_GATE: OnceLock<
    Mutex<Option<PendingHistoryWriteTestGate>>,
> = OnceLock::new();

#[cfg(test)]
pub(super) static SPOOL_QUOTA_WRITE_TEST_GATE: OnceLock<Mutex<Option<SpoolQuotaWriteTestGate>>> =
    OnceLock::new();

#[cfg(test)]
pub(super) static SPOOL_SIZE_WALK_TEST_GATE: OnceLock<Mutex<Option<SpoolSizeWalkTestGate>>> =
    OnceLock::new();

#[cfg(test)]
pub(super) static SPOOL_QUOTA_SCAN_FAULT: OnceLock<Mutex<Option<SpoolQuotaScanFaultInjection>>> =
    OnceLock::new();

#[cfg(test)]
pub static ATOMIC_RENAME_TEST_GATE: OnceLock<Mutex<Option<AtomicRenameTestGate>>> = OnceLock::new();

#[cfg(test)]
pub fn pause_next_history_promotion_for_test(
    data_root: &Path,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
) {
    *HISTORY_PROMOTION_TEST_GATE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(HistoryPromotionTestGate {
        data_root: normalized_data_root(data_root),
        entered,
        release,
    });
}

#[cfg(test)]
pub fn wait_for_history_promotion_test_gate(data_root: &Path) {
    let gate = {
        let mut slot = HISTORY_PROMOTION_TEST_GATE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot
            .as_ref()
            .is_some_and(|gate| gate.data_root == normalized_data_root(data_root))
        {
            slot.take()
        } else {
            None
        }
    };
    if let Some(gate) = gate {
        let _ = gate.entered.send(());
        let _ = gate.release.recv_timeout(Duration::from_secs(15));
    }
}

#[cfg(test)]
pub fn pause_history_delete_spool_wait_for_test(
    session_dir: &Path,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
) {
    *HISTORY_DELETE_SPOOL_WAIT_TEST_GATE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(HistoryDeleteSpoolWaitTestGate {
        session_dir: normalized_data_root(session_dir),
        entered,
        release,
    });
}

#[cfg(test)]
pub fn wait_for_history_delete_spool_test_gate(session_dir: &Path) {
    let gate = {
        let mut slot = HISTORY_DELETE_SPOOL_WAIT_TEST_GATE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot
            .as_ref()
            .is_some_and(|gate| gate.session_dir == normalized_data_root(session_dir))
        {
            slot.take()
        } else {
            None
        }
    };
    if let Some(gate) = gate {
        let _ = gate.entered.send(());
        let _ = gate.release.recv_timeout(Duration::from_secs(15));
    }
}

#[cfg(test)]
pub fn pause_next_pending_history_write_for_test(
    data_root: &Path,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
) {
    *PENDING_HISTORY_WRITE_TEST_GATE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(PendingHistoryWriteTestGate {
        data_root: normalized_data_root(data_root),
        entered,
        release,
    });
}

#[cfg(test)]
pub fn pause_next_spool_write_after_quota_reservation_for_test(
    session_dir: &Path,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
) {
    *SPOOL_QUOTA_WRITE_TEST_GATE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(SpoolQuotaWriteTestGate {
        session_dir: normalized_data_root(session_dir),
        entered,
        release,
    });
}

#[cfg(test)]
pub fn pause_next_spool_size_walk_for_test(
    path: &Path,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
) {
    *SPOOL_SIZE_WALK_TEST_GATE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(SpoolSizeWalkTestGate {
        path: normalized_data_root(path),
        entered,
        release,
    });
}

#[cfg(test)]
pub fn inject_spool_quota_scan_fault_for_test(path: &Path, fault: SpoolQuotaScanFault) {
    *SPOOL_QUOTA_SCAN_FAULT
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(SpoolQuotaScanFaultInjection {
        path: normalized_data_root(path),
        fault,
    });
}

#[cfg(test)]
pub fn pause_next_atomic_rename_for_test(
    temp_path: &Path,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
) {
    *ATOMIC_RENAME_TEST_GATE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(AtomicRenameTestGate {
        temp_path: normalized_data_root(temp_path),
        entered,
        release,
    });
}

#[cfg(test)]
pub fn wait_for_spool_write_after_quota_reservation_for_test(session_dir: &Path) {
    let gate = {
        let mut slot = SPOOL_QUOTA_WRITE_TEST_GATE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot
            .as_ref()
            .is_some_and(|gate| gate.session_dir == normalized_data_root(session_dir))
        {
            slot.take()
        } else {
            None
        }
    };
    if let Some(gate) = gate {
        let _ = gate.entered.send(());
        let _ = gate.release.recv_timeout(Duration::from_secs(15));
    }
}

#[cfg(test)]
pub fn wait_for_spool_size_walk_test_gate(path: &Path) {
    let gate = {
        let mut slot = SPOOL_SIZE_WALK_TEST_GATE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot
            .as_ref()
            .is_some_and(|gate| gate.path == normalized_data_root(path))
        {
            slot.take()
        } else {
            None
        }
    };
    if let Some(gate) = gate {
        let _ = gate.entered.send(());
        let _ = gate.release.recv_timeout(Duration::from_secs(15));
    }
}

#[cfg(test)]
pub fn take_spool_quota_scan_fault_for_test(path: &Path) -> Option<SpoolQuotaScanFault> {
    let mut slot = SPOOL_QUOTA_SCAN_FAULT
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if slot
        .as_ref()
        .is_some_and(|fault| fault.path == normalized_data_root(path))
    {
        slot.take().map(|fault| fault.fault)
    } else {
        None
    }
}

#[cfg(test)]
pub fn wait_for_atomic_rename_test_gate(temp_path: &Path) {
    let gate = {
        let mut slot = ATOMIC_RENAME_TEST_GATE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot
            .as_ref()
            .is_some_and(|gate| gate.temp_path == normalized_data_root(temp_path))
        {
            slot.take()
        } else {
            None
        }
    };
    if let Some(gate) = gate {
        let _ = gate.entered.send(());
        let _ = gate.release.recv_timeout(Duration::from_secs(15));
    }
}

#[cfg(test)]
pub fn wait_for_pending_history_write_test_gate(data_root: &Path) {
    let gate = {
        let mut slot = PENDING_HISTORY_WRITE_TEST_GATE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot
            .as_ref()
            .is_some_and(|gate| gate.data_root == normalized_data_root(data_root))
        {
            slot.take()
        } else {
            None
        }
    };
    if let Some(gate) = gate {
        let _ = gate.entered.send(());
        let _ = gate.release.recv_timeout(Duration::from_secs(15));
    }
}

#[derive(Default)]
pub struct SpoolIoCommitState {
    pub(super) generation: u64,
    pub(super) roots: std::collections::HashMap<PathBuf, u64>,
    pub(super) scopes: std::collections::HashMap<(PathBuf, SpoolIoScope), u64>,
}

pub static SPOOL_IO_COMMIT_STATE: OnceLock<Mutex<SpoolIoCommitState>> = OnceLock::new();
// `active_scans` fences same-root admissions while recursive filesystem
// counts run without holding this mutex. The start-of-scan reservation total
// remains charged through admission, even if a completed write releases its
// in-flight reservation during the walk.
#[derive(Default)]
pub struct SpoolQuotaLedger {
    pub(super) reservations: std::collections::HashMap<PathBuf, u64>,
    pub(super) active_scans: std::collections::HashSet<PathBuf>,
}

pub static SPOOL_QUOTA_RESERVATIONS: OnceLock<(Mutex<SpoolQuotaLedger>, std::sync::Condvar)> =
    OnceLock::new();

pub fn spool_quota_ledger() -> &'static (Mutex<SpoolQuotaLedger>, std::sync::Condvar) {
    SPOOL_QUOTA_RESERVATIONS.get_or_init(|| {
        (
            Mutex::new(SpoolQuotaLedger::default()),
            std::sync::Condvar::new(),
        )
    })
}

#[cfg(test)]
pub struct AtomicParentSyncFailureTest {
    pub(super) parent: PathBuf,
}

#[cfg(test)]
pub static ATOMIC_PARENT_SYNC_FAILURE_TEST: OnceLock<Mutex<Option<AtomicParentSyncFailureTest>>> =
    OnceLock::new();

pub struct SpoolIoCommitPermit {
    pub(super) region: SpoolIoCommitRegion,
    pub(super) generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpoolIoScope {
    RecoverySpool,
    GoldAudio,
    HistoryDatabase,
    PendingHistoryIntent,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SpoolIoCommitRegion {
    Root(PathBuf),
    Scope(PathBuf, SpoolIoScope),
}

impl Drop for SpoolIoCommitPermit {
    fn drop(&mut self) {
        let mut state = SPOOL_IO_COMMIT_STATE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match &self.region {
            SpoolIoCommitRegion::Root(data_root) => {
                if state.roots.get(data_root) == Some(&self.generation) {
                    state.roots.remove(data_root);
                }
            }
            SpoolIoCommitRegion::Scope(scope_root, scope) => {
                let key = (scope_root.clone(), *scope);
                if state.scopes.get(&key) == Some(&self.generation) {
                    state.scopes.remove(&key);
                }
            }
        }
    }
}

pub fn normalized_data_root(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

pub fn session_data_root(session_dir: &Path) -> anyhow::Result<PathBuf> {
    session_dir
        .parent()
        .and_then(Path::parent)
        .map(normalized_data_root)
        .ok_or_else(|| anyhow::anyhow!("invalid spool session path"))
}

pub fn try_spool_io_commit(data_root: &Path) -> anyhow::Result<SpoolIoCommitPermit> {
    let data_root = normalized_data_root(data_root);
    let mut state = SPOOL_IO_COMMIT_STATE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state
        .roots
        .keys()
        .any(|owner_root| roots_overlap(owner_root, &data_root))
        || state
            .scopes
            .keys()
            .any(|(owner_scope, _)| roots_overlap(owner_scope, &data_root))
    {
        anyhow::bail!("audio spool I/O is busy");
    }
    state.generation = state.generation.wrapping_add(1).max(1);
    let generation = state.generation;
    state.roots.insert(data_root.clone(), generation);
    Ok(SpoolIoCommitPermit {
        region: SpoolIoCommitRegion::Root(data_root),
        generation,
    })
}

pub fn spool_io_scope_root(data_root: &Path, scope: SpoolIoScope) -> PathBuf {
    let data_root = normalized_data_root(data_root);
    match scope {
        SpoolIoScope::RecoverySpool => data_root.join("spool"),
        SpoolIoScope::GoldAudio => data_root.join("gold"),
        // These are logical lock paths. The History database also owns its
        // WAL, SHM and migration-backup siblings, so a single typed region
        // must cover all of them without claiming the unrelated audio roots.
        SpoolIoScope::HistoryDatabase => data_root.join("history.sqlite"),
        SpoolIoScope::PendingHistoryIntent => data_root.join("history-pending"),
    }
}

pub fn try_spool_io_commit_scope(
    data_root: &Path,
    scope: SpoolIoScope,
) -> anyhow::Result<SpoolIoCommitPermit> {
    let data_root = normalized_data_root(data_root);
    let scope_root = spool_io_scope_root(&data_root, scope);
    let mut state = SPOOL_IO_COMMIT_STATE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state
        .roots
        .keys()
        .any(|owner_root| roots_overlap(owner_root, &data_root))
        || state
            .scopes
            .keys()
            .any(|(owner_scope_root, _)| roots_overlap(owner_scope_root, &scope_root))
    {
        anyhow::bail!("audio spool I/O is busy");
    }
    state.generation = state.generation.wrapping_add(1).max(1);
    let generation = state.generation;
    state.scopes.insert((scope_root.clone(), scope), generation);
    Ok(SpoolIoCommitPermit {
        region: SpoolIoCommitRegion::Scope(scope_root, scope),
        generation,
    })
}

/// Recovery promotion is an idempotent History writer and may be requested
/// concurrently by explicit recovery and startup maintenance. Serialize only
/// this retained worker path with a bounded wait; capture admission and the
/// general scope API remain non-waiting.
pub fn try_history_database_commit_with_wait(
    data_root: &Path,
) -> anyhow::Result<SpoolIoCommitPermit> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match try_spool_io_commit_scope(data_root, SpoolIoScope::HistoryDatabase) {
            Ok(permit) => return Ok(permit),
            Err(error) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(2));
                drop(error);
            }
            Err(error) => return Err(error),
        }
    }
}

/// Acquire the path-derived scope for a registered spool owner. A live session
/// owns its exact directory; a legacy flat artifact owns its exact file. The
/// location and kind come from the registered lease, while ancestor-wide
/// recovery, retention, clear, and quota operations keep their root guards.
pub fn try_spool_io_commit_audio_session(
    lease: &AudioSpoolSessionLease,
) -> anyhow::Result<SpoolIoCommitPermit> {
    let session_dir = normalized_data_root(lease.session_dir());
    let data_root = session_data_root(&session_dir)?;
    if !spool_lease_can_write(lease) {
        anyhow::bail!("audio spool lease does not own a writable path");
    }
    let (scope, scope_root) = match lease.inner.kind {
        SpoolLeaseKind::Session => {
            let spool_root = data_root.join("spool");
            let session_id = session_dir.file_name().and_then(|value| value.to_str());
            if session_dir.parent() != Some(spool_root.as_path())
                || !session_id.is_some_and(valid_spool_session_id)
            {
                anyhow::bail!("audio spool lease does not own a live session scope");
            }
            (SpoolIoScope::RecoverySpool, session_dir)
        }
        SpoolLeaseKind::Artifact => {
            let parent = session_dir
                .parent()
                .ok_or_else(|| anyhow::anyhow!("invalid spool artifact path"))?;
            let scope = if parent == data_root.join("spool") {
                SpoolIoScope::RecoverySpool
            } else if parent == data_root.join("gold") {
                SpoolIoScope::GoldAudio
            } else {
                anyhow::bail!("spool artifact lease is outside a managed root");
            };
            (scope, session_dir)
        }
    };
    try_spool_io_commit_scope_at(&data_root, scope, scope_root)
}

/// A deferred removal revokes writes immediately, but its exact current owner
/// still needs a cleanup-only permit after accepted work has drained.
pub fn try_spool_io_commit_audio_session_cleanup(
    lease: &AudioSpoolSessionLease,
) -> anyhow::Result<SpoolIoCommitPermit> {
    if lease.inner.kind != SpoolLeaseKind::Session || !spool_lease_is_current(lease) {
        anyhow::bail!("audio spool cleanup lease no longer owns this session");
    }
    let session_dir = normalized_data_root(lease.session_dir());
    let data_root = session_data_root(&session_dir)?;
    let spool_root = data_root.join("spool");
    let session_id = session_dir.file_name().and_then(|value| value.to_str());
    if session_dir.parent() != Some(spool_root.as_path())
        || !session_id.is_some_and(valid_spool_session_id)
    {
        anyhow::bail!("audio spool cleanup lease does not own a session path");
    }
    try_spool_io_commit_scope_at(&data_root, SpoolIoScope::RecoverySpool, session_dir)
}

pub fn try_spool_io_commit_scope_at(
    data_root: &Path,
    scope: SpoolIoScope,
    scope_root: PathBuf,
) -> anyhow::Result<SpoolIoCommitPermit> {
    let data_root = normalized_data_root(data_root);
    let scope_root = normalized_data_root(&scope_root);
    let expected_root = spool_io_scope_root(&data_root, scope);
    if !matches!(scope, SpoolIoScope::RecoverySpool | SpoolIoScope::GoldAudio)
        || scope_root.parent() != Some(expected_root.as_path())
    {
        anyhow::bail!("invalid derived audio spool scope");
    }
    let mut state = SPOOL_IO_COMMIT_STATE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state
        .roots
        .keys()
        .any(|owner_root| roots_overlap(owner_root, &data_root))
        || state
            .scopes
            .keys()
            .any(|(owner_scope_root, _)| roots_overlap(owner_scope_root, &scope_root))
    {
        anyhow::bail!("audio spool I/O is busy");
    }
    state.generation = state.generation.wrapping_add(1).max(1);
    let generation = state.generation;
    state.scopes.insert((scope_root.clone(), scope), generation);
    Ok(SpoolIoCommitPermit {
        region: SpoolIoCommitRegion::Scope(scope_root, scope),
        generation,
    })
}

pub struct SpoolQuotaReservation {
    pub(super) root: PathBuf,
    pub(super) bytes: u64,
}

impl Drop for SpoolQuotaReservation {
    fn drop(&mut self) {
        let (ledger, _) = spool_quota_ledger();
        let mut ledger = ledger
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(reserved) = ledger.reservations.get_mut(&self.root) {
            *reserved = reserved.saturating_sub(self.bytes);
            if *reserved == 0 {
                ledger.reservations.remove(&self.root);
            }
        }
    }
}

pub struct SpoolQuotaScanReservation {
    pub(super) root: PathBuf,
}

impl Drop for SpoolQuotaScanReservation {
    fn drop(&mut self) {
        let (ledger, changed) = spool_quota_ledger();
        let mut ledger = ledger
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        ledger.active_scans.remove(&self.root);
        changed.notify_all();
    }
}

pub fn reserve_recovery_spool_bytes(
    root: &Path,
    bytes: u64,
) -> anyhow::Result<SpoolQuotaReservation> {
    let root = normalized_data_root(root);
    let (ledger, changed) = spool_quota_ledger();
    let mut state = ledger
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    while state.active_scans.contains(&root) {
        state = changed
            .wait(state)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
    state.active_scans.insert(root.clone());
    let inflight_at_scan_start = state.reservations.get(&root).copied().unwrap_or_default();
    drop(state);
    let _scan = SpoolQuotaScanReservation { root: root.clone() };
    let current = spool_size(&root)?;
    let mut state = ledger
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let inflight =
        inflight_at_scan_start.max(state.reservations.get(&root).copied().unwrap_or_default());
    if current.saturating_add(inflight).saturating_add(bytes) > MAX_SPOOL_BYTES {
        anyhow::bail!("audio spool quota exceeded");
    }
    *state.reservations.entry(root.clone()).or_default() += bytes;
    drop(state);
    drop(_scan);
    Ok(SpoolQuotaReservation { root, bytes })
}

#[cfg(test)]
pub(crate) fn recovery_spool_reserved_bytes_for_test(data_root: &Path) -> u64 {
    spool_quota_ledger()
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .reservations
        .get(&normalized_data_root(&data_root.join("spool")))
        .copied()
        .unwrap_or_default()
}

#[cfg(test)]
pub(super) fn fail_next_atomic_parent_sync_for_test(parent: &Path) {
    *ATOMIC_PARENT_SYNC_FAILURE_TEST
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(AtomicParentSyncFailureTest {
        parent: normalized_data_root(parent),
    });
}

#[cfg(test)]
pub(crate) fn spool_io_scope_is_busy_for_test(data_root: &Path, scope: SpoolIoScope) -> bool {
    match try_spool_io_commit_scope(data_root, scope) {
        Ok(permit) => {
            drop(permit);
            false
        }
        Err(_) => true,
    }
}

pub(super) fn try_spool_io_commit_pending_history(
    data_root: &Path,
    transaction_id: &str,
) -> anyhow::Result<SpoolIoCommitPermit> {
    if transaction_id.is_empty()
        || transaction_id.len() > 96
        || !transaction_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        anyhow::bail!("invalid pending History transaction identity");
    }
    // Pending-intent admission inspects the complete directory for quota and
    // identity accounting. Its commit scope must therefore own the complete
    // pending root, not just this transaction's destination directory.
    try_spool_io_commit_scope(data_root, SpoolIoScope::PendingHistoryIntent)
}

pub(super) fn acquire_pending_history_mutation_and_commit(
    data_root: &Path,
    transaction_id: &str,
) -> anyhow::Result<(SpoolRootMutationLease, SpoolIoCommitPermit)> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mutation = begin_spool_root_mutation(data_root)?;
        match try_spool_io_commit_pending_history(data_root, transaction_id) {
            Ok(commit) => return Ok((mutation, commit)),
            Err(error)
                if error.to_string() == "audio spool I/O is busy" && Instant::now() < deadline =>
            {
                drop(mutation);
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
}

#[derive(Clone)]
pub struct PendingHistoryActiveLease {
    pub(super) inner: std::sync::Arc<PendingHistoryActiveLeaseInner>,
}

pub struct PendingHistoryActiveLeaseInner {
    pub(super) key: (PathBuf, String),
    pub(super) cancelled: std::sync::atomic::AtomicBool,
}

impl Drop for PendingHistoryActiveLeaseInner {
    fn drop(&mut self) {
        PENDING_HISTORY_ACTIVE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.key);
    }
}

impl PendingHistoryActiveLease {
    fn matches(&self, data_root: &Path, transaction_id: &str) -> bool {
        self.inner.key == (normalized_data_root(data_root), transaction_id.to_owned())
    }

    pub(super) fn ensure_writable(
        &self,
        data_root: &Path,
        transaction_id: &str,
    ) -> anyhow::Result<()> {
        if !self.matches(data_root, transaction_id) {
            anyhow::bail!("pending History owner does not match the transaction");
        }
        if self
            .inner
            .cancelled
            .load(std::sync::atomic::Ordering::Acquire)
        {
            anyhow::bail!("pending History transaction was cancelled");
        }
        Ok(())
    }

    pub(super) fn poison_for_known_no_effect_cancel(
        &self,
        data_root: &Path,
        transaction_id: &str,
    ) -> anyhow::Result<()> {
        if !self.matches(data_root, transaction_id) {
            anyhow::bail!("pending History owner does not match the transaction");
        }
        self.inner
            .cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    pub(crate) fn poison_known_no_effect_cancel(&self) {
        self.inner
            .cancelled
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

pub fn new_pending_history_active_lease(key: (PathBuf, String)) -> PendingHistoryActiveLease {
    PendingHistoryActiveLease {
        inner: std::sync::Arc::new(PendingHistoryActiveLeaseInner {
            key,
            cancelled: std::sync::atomic::AtomicBool::new(false),
        }),
    }
}

pub fn register_pending_history_owner(
    data_root: &Path,
    transaction_id: &str,
) -> anyhow::Result<PendingHistoryActiveLease> {
    if transaction_id.is_empty()
        || transaction_id.len() > 96
        || !transaction_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        anyhow::bail!("invalid pending History transaction identity");
    }
    let key = (normalized_data_root(data_root), transaction_id.to_owned());
    let mut active = PENDING_HISTORY_ACTIVE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !active.insert(key.clone()) {
        anyhow::bail!("pending History transaction identity is already owned");
    }
    Ok(new_pending_history_active_lease(key))
}

pub fn reserve_pending_history_startup_owner(
    data_root: &Path,
    transaction_id: &str,
) -> Option<PendingHistoryActiveLease> {
    let key = (normalized_data_root(data_root), transaction_id.to_owned());
    let mut active = PENDING_HISTORY_ACTIVE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    active
        .insert(key.clone())
        .then_some(new_pending_history_active_lease(key))
}

pub struct AudioSpoolSessionLease {
    pub(super) inner: std::sync::Arc<AudioSpoolSessionLeaseInner>,
}

pub struct AudioSpoolSessionLeaseInner {
    pub(super) session_dir: PathBuf,
    pub(super) generation: u64,
    pub(super) needs_initialization: bool,
    pub(super) kind: SpoolLeaseKind,
}

impl Clone for AudioSpoolSessionLease {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl AudioSpoolSessionLease {
    pub(crate) fn session_dir(&self) -> &Path {
        &self.inner.session_dir
    }

    pub(crate) fn needs_initialization(&self) -> bool {
        self.inner.needs_initialization
    }

    pub(super) fn generation(&self) -> u64 {
        self.inner.generation
    }

    pub(crate) fn deletion_requested(&self) -> bool {
        let active = ACTIVE_AUDIO_SPOOL_SESSIONS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        active
            .sessions
            .get(self.session_dir())
            .filter(|reservation| reservation.generation == self.generation())
            .is_none_or(|reservation| reservation.delete_when_released)
    }
}

impl Drop for AudioSpoolSessionLeaseInner {
    fn drop(&mut self) {
        let mut active = ACTIVE_AUDIO_SPOOL_SESSIONS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active
            .sessions
            .get(&self.session_dir)
            .is_some_and(|reservation| reservation.generation == self.generation)
        {
            active.sessions.remove(&self.session_dir);
        }
    }
}

#[derive(Clone)]
pub struct SpoolRootMutationLease {
    pub(super) inner: std::sync::Arc<SpoolRootMutationLeaseInner>,
}

pub struct SpoolRootMutationLeaseInner {
    pub(super) data_root: PathBuf,
    pub(super) active: std::sync::Arc<AtomicBool>,
}

impl SpoolRootMutationLease {
    pub(crate) fn data_root(&self) -> &Path {
        &self.inner.data_root
    }
}

impl Drop for SpoolRootMutationLeaseInner {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

pub fn prune_dead_root_mutations(active: &mut ActiveAudioSpoolSessions) {
    active.root_mutations.retain(|_, generations| {
        generations.retain(|_, is_active| is_active.load(Ordering::Acquire));
        !generations.is_empty()
    });
}

pub fn begin_spool_root_mutation(data_root: &Path) -> anyhow::Result<SpoolRootMutationLease> {
    let data_root = normalized_data_root(data_root);
    let mut active = ACTIVE_AUDIO_SPOOL_SESSIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    prune_dead_root_mutations(&mut active);
    if root_is_clearing(&active, &data_root) {
        anyhow::bail!("data root is being cleared");
    }
    active.next_generation = active.next_generation.wrapping_add(1).max(1);
    let generation = active.next_generation;
    let lease_active = std::sync::Arc::new(AtomicBool::new(true));
    active
        .root_mutations
        .entry(data_root.clone())
        .or_default()
        .insert(generation, lease_active.clone());
    Ok(SpoolRootMutationLease {
        inner: std::sync::Arc::new(SpoolRootMutationLeaseInner {
            data_root,
            active: lease_active,
        }),
    })
}

pub fn try_begin_spool_root_mutation(data_root: &Path) -> anyhow::Result<SpoolRootMutationLease> {
    let data_root = normalized_data_root(data_root);
    let mut active = ACTIVE_AUDIO_SPOOL_SESSIONS
        .get_or_init(Default::default)
        .try_lock()
        .map_err(|_| anyhow::anyhow!("data root mutation admission is busy"))?;
    prune_dead_root_mutations(&mut active);
    if root_is_clearing(&active, &data_root) {
        anyhow::bail!("data root is being cleared");
    }
    active.next_generation = active.next_generation.wrapping_add(1).max(1);
    let generation = active.next_generation;
    let lease_active = std::sync::Arc::new(AtomicBool::new(true));
    active
        .root_mutations
        .entry(data_root.clone())
        .or_default()
        .insert(generation, lease_active.clone());
    Ok(SpoolRootMutationLease {
        inner: std::sync::Arc::new(SpoolRootMutationLeaseInner {
            data_root,
            active: lease_active,
        }),
    })
}

pub fn reserve_spool_session_path(
    session_dir: &Path,
    needs_initialization: bool,
    kind: SpoolLeaseKind,
) -> anyhow::Result<AudioSpoolSessionLease> {
    let session_dir = normalized_data_root(session_dir);
    let data_root = session_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow::anyhow!("invalid spool session path"))?;
    let mut active = ACTIVE_AUDIO_SPOOL_SESSIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if root_is_clearing(&active, data_root) {
        anyhow::bail!("audio spool is being cleared");
    }
    if active.sessions.contains_key(&session_dir) {
        anyhow::bail!("spool session is already owned by an audio worker");
    }
    active.next_generation = active.next_generation.wrapping_add(1).max(1);
    let generation = active.next_generation;
    active.sessions.insert(
        session_dir.clone(),
        SpoolReservation {
            generation,
            delete_when_released: false,
        },
    );
    Ok(AudioSpoolSessionLease {
        inner: std::sync::Arc::new(AudioSpoolSessionLeaseInner {
            session_dir,
            generation,
            needs_initialization,
            kind,
        }),
    })
}

pub fn valid_spool_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id.len() <= 128
        && session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub fn root_is_clearing(active: &ActiveAudioSpoolSessions, data_root: &Path) -> bool {
    let data_root = normalized_data_root(data_root);
    active
        .clearing_roots
        .iter()
        .any(|clearing_root| roots_overlap(&data_root, clearing_root))
}

pub fn roots_overlap(left: &Path, right: &Path) -> bool {
    let left = normalized_data_root(left);
    let right = normalized_data_root(right);
    left.starts_with(&right) || right.starts_with(&left)
}

pub fn lease_spool_session(session_dir: &Path) -> anyhow::Result<AudioSpoolSessionLease> {
    // Reserve in memory before inspecting the path, then do filesystem work
    // without holding the registry mutex. Removers see the reservation and
    // defer instead of racing this check.
    let lease = reserve_spool_session_path(session_dir, false, SpoolLeaseKind::Session)?;
    let metadata = match fs::symlink_metadata(session_dir) {
        Ok(metadata) => metadata,
        Err(error) => {
            drop(lease);
            return Err(error.into());
        }
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        drop(lease);
        anyhow::bail!("spool session path is not a private directory");
    }
    Ok(lease)
}

/// Initialize a reserved live audio session on its bounded spool-owner thread.
/// Directory creation is exclusive, so a failed start never removes or
/// overwrites a pre-existing recovery artifact.
pub fn initialize_audio_spool_session(lease: &AudioSpoolSessionLease) -> anyhow::Result<()> {
    if !lease.needs_initialization() || !spool_lease_can_write(lease) {
        anyhow::bail!("audio spool lease is not awaiting initialization");
    }
    let _commit = try_spool_io_commit_audio_session(lease)?;
    let session_dir = lease.session_dir();
    let parent = session_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid audio spool session path"))?;
    let session_id = session_dir
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| valid_spool_session_id(value))
        .ok_or_else(|| anyhow::anyhow!("invalid audio spool session id"))?;
    ensure_private_dir(parent)?;
    if lease.deletion_requested() {
        anyhow::bail!("audio spool initialization was cancelled");
    }
    fs::create_dir(session_dir)?;
    sync_directory(parent)?;
    if lease.deletion_requested() {
        let _ = fs::remove_dir_all(session_dir);
        let _ = sync_directory(parent);
        anyhow::bail!("audio spool initialization was cancelled");
    }
    let result = (|| {
        ensure_private_dir(session_dir)?;
        save_manifest(
            session_dir,
            &SpoolManifest {
                schema_version: SPOOL_MANIFEST_VERSION,
                session_id: session_id.to_owned(),
                created_at_ms: now_ms(),
                status: "active".into(),
                chunks: Vec::new(),
            },
        )
    })();
    if let Err(error) = result {
        // This path was created exclusively above, so cleanup cannot touch a
        // pre-existing recovery session. It also runs on the spool owner.
        let _ = fs::remove_dir_all(session_dir);
        return Err(error);
    }
    Ok(())
}

pub fn spool_session_is_leased(session_dir: &Path) -> bool {
    ACTIVE_AUDIO_SPOOL_SESSIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .sessions
        .contains_key(session_dir)
}

pub fn spool_lease_is_current(lease: &AudioSpoolSessionLease) -> bool {
    ACTIVE_AUDIO_SPOOL_SESSIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .sessions
        .get(lease.session_dir())
        .is_some_and(|reservation| reservation.generation == lease.generation())
}

pub fn spool_lease_can_write(lease: &AudioSpoolSessionLease) -> bool {
    ACTIVE_AUDIO_SPOOL_SESSIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .sessions
        .get(lease.session_dir())
        .is_some_and(|reservation| {
            reservation.generation == lease.generation() && !reservation.delete_when_released
        })
}

pub fn spool_artifact_is_leased(path: &Path) -> bool {
    let path = normalized_data_root(path);
    ACTIVE_AUDIO_SPOOL_SESSIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .sessions
        .keys()
        .any(|session_dir| path.starts_with(session_dir))
}

pub fn has_spool_owner_under(path: &Path) -> bool {
    let path = normalized_data_root(path);
    ACTIVE_AUDIO_SPOOL_SESSIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .sessions
        .keys()
        .any(|session_dir| session_dir.starts_with(&path))
}

pub fn defer_spool_artifact_removal_until_release(path: &Path) -> bool {
    let path = normalized_data_root(path);
    let mut active = ACTIVE_AUDIO_SPOOL_SESSIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut matched = false;
    for (session_dir, reservation) in &mut active.sessions {
        if path.starts_with(session_dir) || session_dir.starts_with(&path) {
            reservation.delete_when_released = true;
            matched = true;
        }
    }
    matched
}

pub struct SpoolClearGuard {
    pub(super) data_root: PathBuf,
}

impl SpoolClearGuard {
    pub(crate) fn owns_root(&self, data_root: &Path) -> bool {
        self.data_root == normalized_data_root(data_root)
    }
}

impl Drop for SpoolClearGuard {
    fn drop(&mut self) {
        ACTIVE_AUDIO_SPOOL_SESSIONS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clearing_roots
            .remove(&self.data_root);
    }
}

pub fn begin_spool_clear(data_root: &Path) -> anyhow::Result<SpoolClearGuard> {
    let data_root = normalized_data_root(data_root);
    let mut active = ACTIVE_AUDIO_SPOOL_SESSIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    prune_dead_root_mutations(&mut active);
    if root_is_clearing(&active, &data_root) {
        anyhow::bail!("audio spool cleanup is already running");
    }
    let mut busy = active
        .root_mutations
        .iter()
        .any(|(root, generations)| !generations.is_empty() && roots_overlap(root, &data_root));
    for session_dir in active.sessions.keys() {
        if session_dir.starts_with(&data_root) {
            busy = true;
        }
    }
    if busy {
        // A rejected clear request has no deletion side effect. In particular,
        // do not mark a live recording for deletion when reporting Busy.
        anyhow::bail!("audio spool cleanup is still pending for an active worker");
    }
    active.clearing_roots.insert(data_root.clone());
    Ok(SpoolClearGuard { data_root })
}

/// Claim the data root before any clear operation opens History or touches
/// audit, backup, spool, gold, or diagnostics state. The guard is memory-only
/// on Drop and must be retained by the actual bounded I/O owner.
pub fn begin_clear_all_data(data_root: &Path) -> anyhow::Result<SpoolClearGuard> {
    begin_spool_clear(data_root)
}

pub fn ensure_private_dir(path: &Path) -> anyhow::Result<()> {
    let mut missing = Vec::new();
    let mut cursor = path;
    while !cursor.exists() {
        missing.push(cursor.to_path_buf());
        cursor = cursor
            .parent()
            .ok_or_else(|| anyhow::anyhow!("invalid private directory path"))?;
    }
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        let mode = fs::metadata(path)?.permissions().mode() & 0o777;
        if mode != 0o700 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
            sync_directory(path)?;
        }
    }
    for created in missing.iter().rev() {
        if let Some(parent) = created.parent() {
            sync_directory(parent)?;
        }
        sync_directory(created)?;
    }
    Ok(())
}

pub fn sync_directory(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub fn sync_atomic_parent_directory(path: &Path) -> anyhow::Result<()> {
    #[cfg(test)]
    {
        let mut slot = ATOMIC_PARENT_SYNC_FAILURE_TEST
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot
            .as_ref()
            .is_some_and(|test| test.parent == normalized_data_root(path))
        {
            slot.take();
            anyhow::bail!("injected atomic parent directory sync failure");
        }
    }
    sync_directory(path)
}

pub fn restrict_file_mode(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

pub fn restrict_history_sidecars(dir: &Path) -> anyhow::Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let path = dir.join(format!("history.sqlite{suffix}"));
        if path.exists() {
            restrict_file_mode(&path)?;
        }
    }
    Ok(())
}

pub const MAX_SPOOL_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_GOLD_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const SPOOL_MANIFEST_VERSION: u32 = 1;
pub const AUDIO_SAMPLE_RATE: usize = 16_000;
/// Prefix of the retired app-layer spool encryption envelope. Such files are
/// rejected instead of being returned as audio.
pub const SPOOL_ENVELOPE_MAGIC: &[u8; 8] = b"VFSPOOL1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpoolChunkManifest {
    pub(super) index: usize,
    pub(super) start_secs: f32,
    pub(super) end_secs: f32,
    pub(super) status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) source_start_sample: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) sample_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpoolManifest {
    pub(super) schema_version: u32,
    pub(super) session_id: String,
    pub(super) created_at_ms: u64,
    pub(super) status: String,
    pub(super) chunks: Vec<SpoolChunkManifest>,
}

#[derive(Debug, Clone)]
pub struct RecoveredSpool {
    pub audio_path: PathBuf,
    pub duration_secs: f64,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn spool_manifest_age_secs(manifest: &SpoolManifest, now_ms: u64) -> Option<u64> {
    if manifest.created_at_ms == 0 || manifest.created_at_ms > now_ms {
        return None;
    }
    Some((now_ms - manifest.created_at_ms) / 1_000)
}

pub fn spool_session_expired(
    manifest: Option<&SpoolManifest>,
    keep_audio_days: u64,
    now_ms: u64,
) -> bool {
    let max_age = keep_audio_days.saturating_mul(86_400);
    keep_audio_days == 0
        || manifest
            .and_then(|manifest| spool_manifest_age_secs(manifest, now_ms))
            .is_none_or(|age| age > max_age)
}

pub fn spool_flat_artifact_expired(
    path: &Path,
    keep_audio_days: u64,
    now_ms: u64,
) -> anyhow::Result<bool> {
    if keep_audio_days == 0 {
        return Ok(true);
    }
    let modified_ms = fs::symlink_metadata(path)?
        .modified()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64);
    let max_age_secs = keep_audio_days.saturating_mul(86_400);
    Ok(modified_ms.is_none_or(|modified| {
        modified > now_ms || now_ms.saturating_sub(modified) / 1_000 > max_age_secs
    }))
}

/// Expire spool artifacts without rebuilding audio or promoting History.
/// The caller may provide a deterministic clock; all filesystem work still
/// runs under the root mutation and RecoverySpool commit fences.
pub fn expire_spool(
    dir: &Path,
    keep_audio_days: u64,
    now_ms: impl FnOnce() -> u64,
) -> anyhow::Result<()> {
    let _mutation = begin_spool_root_mutation(dir)?;
    let _commit = try_spool_io_commit_scope(dir, SpoolIoScope::RecoverySpool)?;
    let now_ms = now_ms();
    let root = dir.join("spool");
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            if spool_session_is_leased(&path) {
                continue;
            }
            let manifest = match load_manifest(&path) {
                Ok(manifest) if manifest.schema_version == SPOOL_MANIFEST_VERSION => Some(manifest),
                _ => None,
            };
            if spool_session_expired(manifest.as_ref(), keep_audio_days, now_ms) {
                fs::remove_dir_all(path)?;
            }
        } else if !spool_artifact_is_leased(&path)
            && spool_flat_artifact_expired(&path, keep_audio_days, now_ms)?
        {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

pub fn is_safe_spool_path(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

pub fn gold_root(dir: &Path) -> PathBuf {
    dir.join("gold")
}

pub fn is_safe_managed_audio_path(dir: &Path, path: &Path) -> bool {
    is_safe_spool_path(&dir.join("spool"), path) || is_safe_spool_path(&gold_root(dir), path)
}

pub fn managed_audio_exists(dir: &Path, path: &Path) -> bool {
    is_safe_managed_audio_path(dir, path) && path.is_file()
}

pub fn spool_size(path: &Path) -> anyhow::Result<u64> {
    if spool_session_is_leased(path) {
        return spool_size_live_owner_snapshot(path);
    }
    spool_size_walk(path)
}

pub fn spool_size_live_owner_snapshot(path: &Path) -> anyhow::Result<u64> {
    // Quota checks read lengths only. Audio chunks and manifests become
    // visible by atomic rename, and any in-progress temp file is counted
    // conservatively; recovery and content readers use the typed busy guard.
    spool_size_walk(path)
}

pub fn spool_size_walk(path: &Path) -> anyhow::Result<u64> {
    #[cfg(test)]
    match take_spool_quota_scan_fault_for_test(path) {
        Some(SpoolQuotaScanFault::Error) => {
            anyhow::bail!("injected spool quota scan failure");
        }
        Some(SpoolQuotaScanFault::Panic) => panic!("injected spool quota scan panic"),
        None => {}
    }
    #[cfg(test)]
    wait_for_spool_size_walk_test_gate(path);
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        // Atomic replacement and a sibling owner's scoped cleanup may remove
        // an entry after read_dir yielded it. Their quota reservations remain
        // charged until their actual writes return, so a disappeared entry is
        // freed space rather than an unaccounted in-flight write.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    if !metadata.is_dir() {
        return Ok(0);
    }
    let mut total: u64 = 0;
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        total = total.saturating_add(spool_size_walk(&entry.path())?);
    }
    Ok(total)
}

/// Read a recovery artifact. Artifacts written by the retired encrypted-spool
/// build are rejected rather than returned as ciphertext.
pub fn read_spool_file(path: &Path) -> anyhow::Result<Vec<u8>> {
    let Some(key) = spool_session_key_for_artifact(path) else {
        let bytes = fs::read(path)?;
        if bytes.starts_with(SPOOL_ENVELOPE_MAGIC) {
            anyhow::bail!("invalid spool file path");
        }
        return Ok(bytes);
    };
    let data_root = session_data_root(&key)?;
    let _mutation = begin_spool_root_mutation(&data_root)?;
    let lease = reserve_spool_session_path(&key, false, spool_lease_kind_for_artifact(path, &key))?;
    let _commit = try_spool_io_commit_audio_session(&lease)?;
    let bytes = fs::read(path)?;
    reject_retired_spool_envelope(&bytes)?;
    drop(lease);
    Ok(bytes)
}

/// Read a managed retry artifact with an explicit plaintext and on-disk size
/// cap. The retry command runs this from the retained audio owner so a slow
/// filesystem read cannot occupy an async runtime worker.
pub fn read_spool_file_limited(path: &Path, max_plaintext_bytes: usize) -> anyhow::Result<Vec<u8>> {
    let key = spool_session_key_for_artifact(path);
    let _mutation = if let Some(key) = key.as_ref() {
        Some(begin_spool_root_mutation(&session_data_root(key)?)?)
    } else {
        None
    };
    let _lease = key
        .as_ref()
        .map(|key| reserve_spool_session_path(key, false, spool_lease_kind_for_artifact(path, key)))
        .transpose()?;
    let _commit = _lease
        .as_ref()
        .map(try_spool_io_commit_audio_session)
        .transpose()?;
    let file = File::open(path)?;
    if file.metadata()?.len() > max_plaintext_bytes as u64 {
        anyhow::bail!("audio retry artifact exceeds the bounded read size");
    }
    let mut bytes = Vec::new();
    file.take(max_plaintext_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > max_plaintext_bytes {
        anyhow::bail!("audio retry artifact exceeds the bounded read size");
    }
    if bytes.starts_with(SPOOL_ENVELOPE_MAGIC) && key.is_none() {
        anyhow::bail!("invalid spool file path");
    }
    reject_retired_spool_envelope(&bytes)?;
    Ok(bytes)
}

pub fn read_audio_spool_file(
    lease: &AudioSpoolSessionLease,
    path: &Path,
) -> anyhow::Result<Vec<u8>> {
    if !path.starts_with(lease.session_dir()) || !spool_lease_can_write(lease) {
        anyhow::bail!("audio spool lease does not own the requested file");
    }
    let _commit = try_spool_io_commit_audio_session(lease)?;
    let bytes = fs::read(path)?;
    reject_retired_spool_envelope(&bytes)?;
    Ok(bytes)
}

pub fn reject_retired_spool_envelope(bytes: &[u8]) -> anyhow::Result<()> {
    if bytes.starts_with(SPOOL_ENVELOPE_MAGIC) {
        anyhow::bail!("encrypted audio spool artifacts are no longer supported");
    }
    Ok(())
}

/// Persist a retry/recovery audio file without exposing paths outside the
/// app-owned spool directory. The quota is checked before writing and the
/// final filename is installed with an atomic rename.
pub fn write_spool_file(dir: &Path, relative: &Path, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let key = spool_session_key_for_relative(dir, relative)?;
    let lease = reserve_spool_session_path(
        &key,
        false,
        if relative.components().count() > 1 {
            SpoolLeaseKind::Session
        } else {
            SpoolLeaseKind::Artifact
        },
    )?;
    if !spool_lease_can_write(&lease) {
        anyhow::bail!("spool session deletion is pending");
    }
    write_spool_file_internal(dir, relative, bytes, Some(&lease))
}

/// Persist a chunk for the one audio worker that owns the session lease.
/// Other readers and writers receive a typed busy error for this path.
pub fn write_audio_spool_file(
    lease: &AudioSpoolSessionLease,
    dir: &Path,
    relative: &Path,
    bytes: &[u8],
) -> anyhow::Result<PathBuf> {
    let target = validated_spool_target(dir, relative)?;
    let spool_root = dir.join("spool");
    if lease.session_dir().parent() != Some(spool_root.as_path())
        || !target.starts_with(lease.session_dir())
        || !spool_lease_can_write(lease)
    {
        anyhow::bail!("audio spool lease does not own the requested path");
    }
    write_spool_file_internal(dir, relative, bytes, Some(lease))
}

/// Commit one PCM chunk and its manifest interval under a single derived
/// session scope. A whole-tree scan/retention permit cannot interleave between
/// the chunk rename and the manifest ACK, while another leased session remains
/// free to make progress.
#[allow(clippy::too_many_arguments)]
pub fn write_audio_spool_chunk_with_samples(
    lease: &AudioSpoolSessionLease,
    dir: &Path,
    session_dir: &Path,
    relative: &Path,
    index: usize,
    start_secs: f32,
    end_secs: f32,
    status: &str,
    source_start_sample: u64,
    sample_count: u64,
    bytes: &[u8],
) -> anyhow::Result<PathBuf> {
    let target = validated_spool_target(dir, relative)?;
    let spool_root = dir.join("spool");
    if lease.session_dir() != session_dir
        || lease.session_dir().parent() != Some(spool_root.as_path())
        || !target.starts_with(lease.session_dir())
        || !spool_lease_can_write(lease)
    {
        anyhow::bail!("audio spool lease does not own the requested chunk");
    }
    if sample_count == 0 || source_start_sample.checked_add(sample_count).is_none() {
        anyhow::bail!("invalid recovery chunk sample interval");
    }
    let _commit = try_spool_io_commit_audio_session(lease)?;
    let path = write_spool_file_after_admission(&spool_root, &target, bytes, Some(lease))?;
    record_spool_chunk_inner(
        session_dir,
        index,
        start_secs,
        end_secs,
        status,
        Some((source_start_sample, sample_count)),
    )?;
    Ok(path)
}

pub fn validated_spool_target(dir: &Path, relative: &Path) -> anyhow::Result<PathBuf> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        anyhow::bail!("invalid spool path");
    }
    Ok(dir.join("spool").join(relative))
}

pub fn spool_session_key_for_relative(dir: &Path, relative: &Path) -> anyhow::Result<PathBuf> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        anyhow::bail!("invalid spool path");
    }
    let first = relative
        .components()
        .next()
        .ok_or_else(|| anyhow::anyhow!("invalid spool path"))?;
    Ok(dir.join("spool").join(first.as_os_str()))
}

pub fn spool_session_key_for_artifact(path: &Path) -> Option<PathBuf> {
    let managed_root = path.ancestors().find(|ancestor| {
        ancestor
            .file_name()
            .is_some_and(|name| name == "spool" || name == "gold")
    })?;
    let relative = path.strip_prefix(managed_root).ok()?;
    let first = relative.components().next()?;
    Some(managed_root.join(first.as_os_str()))
}

pub fn spool_lease_kind_for_artifact(path: &Path, owner_path: &Path) -> SpoolLeaseKind {
    if path == owner_path && !path.is_dir() {
        SpoolLeaseKind::Artifact
    } else {
        SpoolLeaseKind::Session
    }
}

pub fn write_spool_file_internal(
    dir: &Path,
    relative: &Path,
    bytes: &[u8],
    owner: Option<&AudioSpoolSessionLease>,
) -> anyhow::Result<PathBuf> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        anyhow::bail!("invalid spool path");
    }
    let root = dir.join("spool");
    let path = root.join(relative);
    if spool_artifact_is_leased(&path)
        && !owner.is_some_and(|lease| {
            path.starts_with(lease.session_dir()) && spool_lease_can_write(lease)
        })
    {
        anyhow::bail!("spool session is still owned by an audio worker");
    }
    if owner.is_some_and(|lease| {
        !path.starts_with(lease.session_dir()) || !spool_lease_can_write(lease)
    }) {
        anyhow::bail!("audio spool lease does not own the requested path");
    }
    let _commit = if let Some(lease) = owner {
        try_spool_io_commit_audio_session(lease)?
    } else {
        try_spool_io_commit_scope(dir, SpoolIoScope::RecoverySpool)?
    };
    write_spool_file_after_admission(&root, &path, bytes, owner)
}

pub fn write_spool_file_after_admission(
    root: &Path,
    path: &Path,
    stored_bytes: &[u8],
    owner: Option<&AudioSpoolSessionLease>,
) -> anyhow::Result<PathBuf> {
    // The reservation covers the recursive tree measurement and remains
    // charged until the real filesystem operation returns. Concurrent live
    // sessions can therefore use disjoint path permits without overshooting
    // the shared spool quota.
    let _quota = reserve_recovery_spool_bytes(root, stored_bytes.len() as u64)?;
    #[cfg(test)]
    if let Some(lease) = owner {
        wait_for_spool_write_after_quota_reservation_for_test(lease.session_dir());
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid spool path"))?;
    ensure_private_dir(parent)?;
    if owner.is_some_and(|lease| !spool_lease_can_write(lease)) {
        anyhow::bail!("audio spool lease was revoked before file commit");
    }
    write_atomic_bytes(path, stored_bytes)?;
    Ok(path.to_path_buf())
}

/// Persist a successful-dictation WAV under `gold/`. Quota is independent of
/// the recovery spool. Full quota returns an error; callers must not fail the
/// dictation itself.
pub fn write_gold_file(dir: &Path, file_name: &str, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let relative = Path::new(file_name);
    if file_name.is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        anyhow::bail!("invalid gold audio path");
    }
    let root = gold_root(dir);
    let path = root.join(relative);
    let lease = reserve_spool_session_path(&path, false, SpoolLeaseKind::Artifact)?;
    let _commit = try_spool_io_commit_scope(dir, SpoolIoScope::GoldAudio)?;
    let current = spool_size(&root)?;
    let existing = fs::symlink_metadata(&path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if current
        .saturating_sub(existing)
        .saturating_add(bytes.len() as u64)
        > MAX_GOLD_BYTES
    {
        anyhow::bail!("gold audio quota exceeded");
    }
    ensure_private_dir(&root)?;
    if !spool_lease_can_write(&lease) {
        anyhow::bail!("gold audio deletion is pending");
    }
    write_atomic_bytes(&path, bytes)?;
    Ok(path)
}

pub fn delete_audio_spool_session(lease: &AudioSpoolSessionLease) -> anyhow::Result<()> {
    if !spool_lease_is_current(lease) {
        anyhow::bail!("audio spool lease no longer owns this session");
    }
    let _commit = try_spool_io_commit_audio_session_cleanup(lease)?;
    let parent = lease
        .session_dir()
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid audio spool session path"))?;
    match fs::remove_dir_all(lease.session_dir()) {
        Ok(()) => sync_directory(parent),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Settle a capture spool after its History transaction has a durable SQLite
/// ACK. The pending intent remains available until this owner cleanup finishes,
/// so startup can repeat the cleanup without inserting another History row.
pub fn settle_capture_spool_after_history_ack(
    data_root: &Path,
    session_id: &str,
    history_audio_path: Option<&Path>,
) -> anyhow::Result<bool> {
    if !valid_spool_session_id(session_id) {
        anyhow::bail!("invalid capture spool identity in History intent");
    }
    let session_dir = data_root.join("spool").join(session_id);
    match fs::symlink_metadata(&session_dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error.into()),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => anyhow::bail!("capture spool session is not a regular private directory"),
    }
    if spool_session_is_leased(&session_dir) {
        return Ok(false);
    }
    let lease = match lease_spool_session(&session_dir) {
        Ok(lease) => lease,
        Err(error) if error.to_string().contains("already owned") => return Ok(false),
        Err(error) => return Err(error),
    };
    let recovery_path = session_dir.join("recovery.wav");
    let retain_recovery = history_audio_path == Some(recovery_path.as_path());
    if retain_recovery {
        mark_audio_spool_status(&lease, &session_dir, "recoverable")?;
    } else {
        mark_audio_spool_status(&lease, &session_dir, "completed")?;
        delete_audio_spool_session(&lease)?;
    }
    Ok(true)
}

pub fn write_atomic_bytes(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    write_atomic_bytes_with_parent_mode(path, bytes, true)
}

pub fn write_atomic_bytes_with_parent_mode(
    path: &Path,
    bytes: &[u8],
    private_parent: bool,
) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid atomic path"))?;
    if private_parent {
        ensure_private_dir(parent)?;
    } else {
        fs::create_dir_all(parent)?;
    }
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("invalid atomic filename"))?
        .to_string_lossy();
    let tmp = parent.join(format!(".{name}.tmp-{}", std::process::id()));
    let write_result = (|| -> anyhow::Result<()> {
        let mut file = File::create(&tmp)?;
        restrict_file_mode(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        #[cfg(test)]
        wait_for_atomic_rename_test_gate(&tmp);
        fs::rename(&tmp, path)?;
        restrict_file_mode(path)?;
        file.sync_all()?;
        sync_atomic_parent_directory(parent)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    write_result
}

pub fn manifest_path(session_dir: &Path) -> PathBuf {
    session_dir.join("manifest.json")
}

pub fn load_manifest(session_dir: &Path) -> anyhow::Result<SpoolManifest> {
    Ok(serde_json::from_slice(&fs::read(manifest_path(
        session_dir,
    ))?)?)
}

pub fn save_manifest(session_dir: &Path, manifest: &SpoolManifest) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(manifest)?;
    let data_root = session_data_root(session_dir)?;
    let _quota = reserve_recovery_spool_bytes(&data_root.join("spool"), bytes.len() as u64)?;
    write_atomic_bytes(&manifest_path(session_dir), &bytes)
}

#[cfg(test)]
pub fn begin_spool_session(root: &Path, session_id: &str) -> anyhow::Result<PathBuf> {
    let (session_dir, reservation) = begin_audio_spool_session(root, session_id)?;
    initialize_audio_spool_session(&reservation)?;
    if reservation.deletion_requested() {
        anyhow::bail!("spool session was removed while it was being initialized");
    }
    drop(reservation);
    Ok(session_dir)
}

/// Reserve a fresh live capture path without doing filesystem work. The
/// bounded spool owner initializes it after audio capture has started; an
/// existing recovery path is then rejected with exclusive `create_dir`.
pub fn begin_audio_spool_session(
    root: &Path,
    session_id: &str,
) -> anyhow::Result<(PathBuf, AudioSpoolSessionLease)> {
    if !valid_spool_session_id(session_id) {
        anyhow::bail!("invalid spool session id");
    }
    let requested_path = root.join("spool").join(session_id);
    let lease = reserve_spool_session_path(&requested_path, true, SpoolLeaseKind::Session)?;
    let session_dir = lease.session_dir().to_path_buf();
    Ok((session_dir, lease))
}

/// Persist one complete WAV as a recoverable spool session. This is used when
/// History is the only delivery recovery path and a failed History write must
/// leave audio that startup recovery can promote into a retryable History item.
pub fn persist_recovery_wav_session(
    root: &Path,
    session_id: &str,
    wav: &[u8],
) -> anyhow::Result<RecoveredSpool> {
    let duration_secs = crate::asr::wav_duration_seconds(wav)
        .filter(|duration| duration.is_finite() && *duration > 0.0)
        .ok_or_else(|| anyhow::anyhow!("recovery WAV duration could not be read"))?;
    let (session_dir, lease) = begin_audio_spool_session(root, session_id)?;
    initialize_audio_spool_session(&lease)?;
    let audio_path = write_audio_spool_file(
        &lease,
        root,
        &PathBuf::from(session_id).join("recovery.wav"),
        wav,
    )?;
    mark_audio_spool_status(&lease, &session_dir, "recoverable")?;
    Ok(RecoveredSpool {
        audio_path,
        duration_secs,
    })
}

/// Persist a complete retry WAV on behalf of the session's retained audio
/// owner. The caller's `Vec` remains the authoritative timeline until this
/// operation is acknowledged.
pub fn persist_audio_spool_recovery_wav(
    lease: &AudioSpoolSessionLease,
    root: &Path,
    wav: &[u8],
) -> anyhow::Result<RecoveredSpool> {
    if !spool_lease_can_write(lease) {
        anyhow::bail!("audio spool lease does not own the requested session");
    }
    let duration_secs = crate::asr::wav_duration_seconds(wav)
        .filter(|duration| duration.is_finite() && *duration > 0.0)
        .ok_or_else(|| anyhow::anyhow!("recovery WAV duration could not be read"))?;
    let session_id = lease
        .session_dir()
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("invalid spool session path"))?;
    let audio_path = write_audio_spool_file(
        lease,
        root,
        &PathBuf::from(session_id).join("recovery.wav"),
        wav,
    )?;
    mark_audio_spool_status(lease, lease.session_dir(), "recoverable")?;
    Ok(RecoveredSpool {
        audio_path,
        duration_secs,
    })
}

#[cfg(test)]
pub fn record_spool_chunk(
    session_dir: &Path,
    index: usize,
    start_secs: f32,
    end_secs: f32,
    status: &str,
) -> anyhow::Result<()> {
    let _lease = lease_spool_session(session_dir)?;
    let data_root = session_data_root(session_dir)?;
    let _commit = try_spool_io_commit_scope(&data_root, SpoolIoScope::RecoverySpool)?;
    record_spool_chunk_inner(session_dir, index, start_secs, end_secs, status, None)
}

/// Record a written chunk with exact source-timeline sample coverage.
/// Later status-only updates through `record_spool_chunk` preserve these fields.
#[cfg(test)]
pub fn record_spool_chunk_with_samples(
    session_dir: &Path,
    index: usize,
    start_secs: f32,
    end_secs: f32,
    status: &str,
    source_start_sample: u64,
    sample_count: u64,
) -> anyhow::Result<()> {
    let _lease = lease_spool_session(session_dir)?;
    if sample_count == 0 || source_start_sample.checked_add(sample_count).is_none() {
        anyhow::bail!("invalid recovery chunk sample interval");
    }
    let data_root = session_data_root(session_dir)?;
    let _commit = try_spool_io_commit_scope(&data_root, SpoolIoScope::RecoverySpool)?;
    record_spool_chunk_inner(
        session_dir,
        index,
        start_secs,
        end_secs,
        status,
        Some((source_start_sample, sample_count)),
    )
}

pub fn record_spool_chunk_inner(
    session_dir: &Path,
    index: usize,
    start_secs: f32,
    end_secs: f32,
    status: &str,
    source_interval: Option<(u64, u64)>,
) -> anyhow::Result<()> {
    let mut manifest = load_manifest(session_dir)?;
    if let Some(chunk) = manifest
        .chunks
        .iter_mut()
        .find(|chunk| chunk.index == index)
    {
        chunk.start_secs = start_secs;
        chunk.end_secs = end_secs;
        chunk.status = status.to_owned();
        if let Some((source_start_sample, sample_count)) = source_interval {
            chunk.source_start_sample = Some(source_start_sample);
            chunk.sample_count = Some(sample_count);
        }
    } else {
        manifest.chunks.push(SpoolChunkManifest {
            index,
            start_secs,
            end_secs,
            status: status.to_owned(),
            source_start_sample: source_interval.map(|(start, _)| start),
            sample_count: source_interval.map(|(_, count)| count),
        });
    }
    save_manifest(session_dir, &manifest)
}

pub fn record_audio_spool_chunk_status(
    lease: &AudioSpoolSessionLease,
    session_dir: &Path,
    index: usize,
    start_secs: f32,
    end_secs: f32,
    status: &str,
) -> anyhow::Result<()> {
    if lease.session_dir() != session_dir || !spool_lease_can_write(lease) {
        anyhow::bail!("audio spool lease does not own the requested session");
    }
    let _commit = try_spool_io_commit_audio_session(lease)?;
    record_spool_chunk_inner(session_dir, index, start_secs, end_secs, status, None)
}

pub fn mark_spool_status(session_dir: &Path, status: &str) -> anyhow::Result<()> {
    let _lease = lease_spool_session(session_dir)?;
    let data_root = session_data_root(session_dir)?;
    let _commit = try_spool_io_commit_scope(&data_root, SpoolIoScope::RecoverySpool)?;
    mark_spool_status_inner(session_dir, status)
}

pub fn mark_audio_spool_status(
    lease: &AudioSpoolSessionLease,
    session_dir: &Path,
    status: &str,
) -> anyhow::Result<()> {
    if lease.session_dir() != session_dir || !spool_lease_can_write(lease) {
        anyhow::bail!("audio spool lease does not own the requested session");
    }
    let _commit = try_spool_io_commit_audio_session(lease)?;
    mark_spool_status_inner(session_dir, status)
}

fn mark_spool_status_inner(session_dir: &Path, status: &str) -> anyhow::Result<()> {
    let mut manifest = load_manifest(session_dir)?;
    manifest.status = status.to_owned();
    save_manifest(session_dir, &manifest)
}

pub fn read_recovery_samples(
    session_dir: &Path,
    owner: Option<&AudioSpoolSessionLease>,
) -> anyhow::Result<Vec<f32>> {
    let chunks_dir = session_dir.join("chunks");
    if !chunks_dir.is_dir() {
        anyhow::bail!("recovery chunks are missing");
    }
    let mut chunks = fs::read_dir(&chunks_dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("f32") {
                return None;
            }
            let index = path
                .file_stem()
                .and_then(|value| value.to_str())
                .and_then(|value| value.parse::<usize>().ok())?;
            Some((index, path))
        })
        .collect::<Vec<_>>();
    chunks.sort_by_key(|(index, _)| *index);
    if chunks.is_empty() {
        anyhow::bail!("recovery contains no complete audio chunks");
    }

    let manifest = if manifest_path(session_dir).is_file() {
        Some(load_manifest(session_dir)?)
    } else {
        None
    };
    let mut manifest_chunks = std::collections::BTreeMap::new();
    if let Some(manifest) = &manifest {
        for chunk in &manifest.chunks {
            if manifest_chunks.insert(chunk.index, chunk).is_some() {
                anyhow::bail!("recovery manifest contains duplicate chunk indexes");
            }
            if chunk.source_start_sample.is_some() != chunk.sample_count.is_some() {
                anyhow::bail!("recovery manifest contains an incomplete sample interval");
            }
        }
    }

    let manifest_has_exact_intervals = manifest_chunks
        .values()
        .any(|chunk| chunk.source_start_sample.is_some());
    let mut decoded_chunks = Vec::with_capacity(chunks.len());
    for (index, path) in chunks {
        let bytes = if let Some(owner) = owner {
            read_audio_spool_file(owner, &path)?
        } else {
            read_spool_file(&path)?
        };
        if bytes.len() % std::mem::size_of::<f32>() != 0 {
            anyhow::bail!("recovery audio chunk is truncated");
        }
        let samples = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect::<Vec<_>>();
        let interval = manifest_chunks
            .get(&index)
            .and_then(|chunk| chunk.source_start_sample.zip(chunk.sample_count));
        decoded_chunks.push((index, samples, interval));
    }

    let exact_interval_count = decoded_chunks
        .iter()
        .filter(|(_, _, interval)| interval.is_some())
        .count();
    if exact_interval_count == 0 {
        if manifest_has_exact_intervals {
            anyhow::bail!("recovery sample intervals are missing for one or more chunks");
        }
        let mut legacy_timing = decoded_chunks
            .iter()
            .filter_map(|(index, _, _)| manifest_chunks.get(index).copied())
            .collect::<Vec<_>>();
        legacy_timing.sort_by_key(|chunk| chunk.index);
        if legacy_timing
            .windows(2)
            .any(|pair| pair[1].start_secs < pair[0].end_secs)
        {
            anyhow::bail!("legacy recovery chunks overlap but lack exact source sample intervals");
        }
        let mut samples: Vec<f32> = Vec::new();
        for (_, chunk_samples, _) in decoded_chunks {
            samples.extend(chunk_samples);
        }
        if samples.is_empty() {
            anyhow::bail!("recovery contains no audio samples");
        }
        return Ok(samples);
    }

    if exact_interval_count != decoded_chunks.len()
        || manifest_chunks.values().any(|chunk| {
            chunk.source_start_sample.is_some()
                && !decoded_chunks.iter().any(|(i, _, _)| i == &chunk.index)
        })
    {
        anyhow::bail!("recovery sample intervals are missing for one or more chunks");
    }

    let mut positioned = Vec::with_capacity(decoded_chunks.len());
    for (index, samples, interval) in decoded_chunks {
        let (source_start, sample_count) =
            interval.ok_or_else(|| anyhow::anyhow!("recovery sample intervals are incomplete"))?;
        if sample_count == 0 || u64::try_from(samples.len()).ok() != Some(sample_count) {
            anyhow::bail!("recovery chunk sample count does not match its manifest");
        }
        let source_end = source_start
            .checked_add(sample_count)
            .ok_or_else(|| anyhow::anyhow!("recovery chunk sample interval overflows"))?;
        positioned.push((source_start, source_end, index, samples));
    }

    positioned.sort_by_key(|(start, _, index, _)| (*start, *index));
    let mut samples: Vec<f32> = Vec::new();
    let mut covered_until = 0_u64;
    for (source_start, source_end, _, chunk_samples) in positioned {
        if source_start > covered_until {
            anyhow::bail!("recovery audio contains a gap in source samples");
        }
        let overlap_count = usize::try_from(covered_until.saturating_sub(source_start))
            .unwrap_or(usize::MAX)
            .min(chunk_samples.len());
        let overlap_start = usize::try_from(source_start)
            .map_err(|_| anyhow::anyhow!("recovery sample interval is too large"))?;
        for (offset, chunk_sample) in chunk_samples.iter().take(overlap_count).enumerate() {
            let existing = samples
                .get(overlap_start + offset)
                .ok_or_else(|| anyhow::anyhow!("recovery sample coverage is inconsistent"))?;
            if existing.to_bits() != chunk_sample.to_bits() {
                anyhow::bail!("overlapping recovery chunks contain conflicting samples");
            }
        }
        if source_end > covered_until {
            samples.extend_from_slice(&chunk_samples[overlap_count..]);
            covered_until = source_end;
        }
    }

    if samples.is_empty() {
        anyhow::bail!("recovery contains no audio samples");
    }
    Ok(samples)
}

pub fn write_spool_session_file(
    session_dir: &Path,
    file_name: &str,
    bytes: &[u8],
) -> anyhow::Result<PathBuf> {
    let session_id = session_dir
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("invalid spool session path"))?;
    let root = session_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow::anyhow!("invalid spool session path"))?;
    write_spool_file(root, &PathBuf::from(session_id).join(file_name), bytes)
}

pub fn rebuild_recovery_wav(
    session_dir: &Path,
    owner: Option<&AudioSpoolSessionLease>,
) -> anyhow::Result<RecoveredSpool> {
    let samples = read_recovery_samples(session_dir, owner)?;
    let wav = crate::chunker::encode_wav(&samples).map_err(|error| anyhow::anyhow!(error))?;
    let audio_path = if let Some(owner) = owner {
        let root = session_dir
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| anyhow::anyhow!("invalid spool session path"))?;
        let session_id = session_dir
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("invalid spool session path"))?;
        write_audio_spool_file(
            owner,
            root,
            &PathBuf::from(session_id).join("recovery.wav"),
            &wav,
        )?
    } else {
        write_spool_session_file(session_dir, "recovery.wav", &wav)?
    };
    Ok(RecoveredSpool {
        audio_path,
        duration_secs: samples.len() as f64 / AUDIO_SAMPLE_RATE as f64,
    })
}

pub fn read_existing_recovery_wav(session_dir: &Path) -> anyhow::Result<RecoveredSpool> {
    let audio_path = session_dir.join("recovery.wav");
    let wav = read_spool_file(&audio_path)?;
    let duration_secs = crate::asr::wav_duration_seconds(&wav)
        .filter(|duration| duration.is_finite() && *duration > 0.0)
        .ok_or_else(|| anyhow::anyhow!("stored recovery WAV duration could not be read"))?;
    Ok(RecoveredSpool {
        audio_path,
        duration_secs,
    })
}

/// Rebuild a retryable WAV from the complete audio chunks kept for a long
/// recording. This is also used when processing finishes in a degraded state
/// so the user can retry the whole recording instead of losing failed chunks.
#[cfg(test)]
pub fn rebuild_spool_recovery(session_dir: &Path) -> anyhow::Result<RecoveredSpool> {
    let lease = lease_spool_session(session_dir)?;
    rebuild_recovery_wav(session_dir, Some(&lease))
}

/// Recover interrupted sessions into retryable audio artifacts. Active
/// manifests are never treated as successful dictations: complete,
/// atomically-written chunks are rebuilt, while explicitly persisted single
/// WAV recovery sessions are retained for manual retry.
pub fn recover_spool(dir: &Path, keep_audio_days: u64) -> anyhow::Result<Vec<RecoveredSpool>> {
    let _mutation = begin_spool_root_mutation(dir)?;
    let root = dir.join("spool");
    if has_spool_owner_under(&root) {
        anyhow::bail!("audio spool initialization or recovery is still owned by a worker");
    }
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut recovered = Vec::new();
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if spool_session_is_leased(&path) {
                continue;
            }
            let manifest = match load_manifest(&path) {
                Ok(manifest) if manifest.schema_version == SPOOL_MANIFEST_VERSION => Some(manifest),
                Ok(manifest) => {
                    log::warn!(
                        "discarding unsupported spool manifest version {} at {}",
                        manifest.schema_version,
                        path.display()
                    );
                    None
                }
                Err(error) => {
                    log::warn!(
                        "discarding unreadable spool session at {}: {error}",
                        path.display()
                    );
                    None
                }
            };
            // Session-directory mtime changes whenever recovery output or the
            // manifest is rewritten. Retention must remain anchored to the
            // manifest's immutable creation time, otherwise every successful
            // startup could extend the lifetime indefinitely. Invalid or
            // future timestamps cannot establish a trustworthy age, so expire
            // those sessions fail-closed.
            let expired = spool_session_expired(manifest.as_ref(), keep_audio_days, now_ms());
            let recoverable = manifest.as_ref().is_some_and(|manifest| {
                matches!(
                    manifest.status.as_str(),
                    "active" | "recoverable" | "degraded"
                )
            });
            if recoverable && !expired {
                let recovery = rebuild_recovery_wav(&path, None)
                    .or_else(|chunk_error| {
                        read_existing_recovery_wav(&path).map_err(|wav_error| {
                            anyhow::anyhow!(
                                "chunks could not be rebuilt ({chunk_error}); stored WAV could not be read ({wav_error})"
                            )
                        })
                    });
                match recovery {
                    Ok(recovery) => {
                        let _ = mark_spool_status(&path, "recoverable");
                        recovered.push(recovery);
                    }
                    Err(error) => {
                        log::warn!(
                            "preserving spool session at {} without a rebuilt recovery WAV; the source chunks could not be reconstructed: {error}",
                            path.display()
                        );
                        let _ = mark_spool_status(&path, "abandoned");
                    }
                }
            } else if expired {
                fs::remove_dir_all(path)?;
            }
        } else {
            if spool_flat_artifact_expired(&path, keep_audio_days, now_ms())? {
                fs::remove_file(path)?;
            }
        }
    }
    Ok(recovered)
}

/// Recover only the spool owned by one failed live recording. Unlike startup
/// recovery, this path never enumerates, rewrites, or expires another session.
pub fn recover_spool_session(
    dir: &Path,
    session_id: &str,
    keep_audio_days: u64,
) -> anyhow::Result<Option<RecoveredSpool>> {
    if !valid_spool_session_id(session_id) {
        anyhow::bail!("invalid spool session id");
    }
    let _mutation = begin_spool_root_mutation(dir)?;
    let spool_root = dir.join("spool");
    let session_dir = spool_root.join(session_id);
    // A live initializer can own this ID before either directory exists.
    // Check the memory reservation before retention shortcuts or filesystem
    // probes so callers receive a typed busy result throughout initialization.
    if spool_session_is_leased(&session_dir) {
        anyhow::bail!("spool session is still owned by an audio worker");
    }
    // A zero-day policy means the recording is not retained at all. Do not
    // let sub-second age truncation make a freshly written spool recoverable.
    if keep_audio_days == 0 {
        return Ok(None);
    }
    match fs::symlink_metadata(&spool_root) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    match fs::symlink_metadata(&session_dir) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let manifest = load_manifest(&session_dir)?;
    if manifest.schema_version != SPOOL_MANIFEST_VERSION
        || manifest.session_id != session_id
        || !matches!(
            manifest.status.as_str(),
            "active" | "recoverable" | "degraded"
        )
    {
        return Ok(None);
    }
    let max_age = keep_audio_days.saturating_mul(86_400);
    let Some(age) = spool_manifest_age_secs(&manifest, now_ms()) else {
        return Ok(None);
    };
    if age > max_age {
        return Ok(None);
    }
    let recovery = rebuild_recovery_wav(&session_dir, None)
        .or_else(|chunk_error| {
            read_existing_recovery_wav(&session_dir).map_err(|wav_error| {
                anyhow::anyhow!(
                    "chunks could not be rebuilt ({chunk_error}); stored WAV could not be read ({wav_error})"
                )
            })
        })?;
    // The WAV itself is already an independent durable artifact. A failed
    // manifest update must not hide it from the caller's History-save attempt.
    let _ = mark_spool_status(&session_dir, "recoverable");
    Ok(Some(recovery))
}
